import Darwin
import Foundation
import Virtualization

public typealias MarsaEvent = @convention(c) (
    UnsafeMutableRawPointer, Int32, Int32, UnsafePointer<CChar>?
) -> Void

private enum Event: Int32 {
    case started = 1
    case connected = 2
    case stopped = 3
    case failed = 4
}

private final class Session: NSObject, VZVirtualMachineDelegate {
    private let context: UnsafeMutableRawPointer
    private let callback: MarsaEvent
    private let vm: VZVirtualMachine
    private let networkPeer: FileHandle
    private var connection: VZVirtioSocketConnection?
    private var active = true

    init(root: URL, serial: Int32, context: UnsafeMutableRawPointer, callback: MarsaEvent) throws {
        self.context = context
        self.callback = callback
        let (config, peer) = try configure(root: root, serial: serial)
        self.networkPeer = peer
        self.vm = VZVirtualMachine(configuration: config)
        super.init()
        vm.delegate = self
    }

    func start() {
        vm.start(completionHandler: { [weak self] result in
            switch result {
            case .success: self?.emit(.started)
            case let .failure(error): self?.emit(.failed, error: error)
            }
        })
    }

    func connect(port: UInt32) {
        guard let socket = vm.socketDevices.first as? VZVirtioSocketDevice else {
            emit(.failed, error: NSError(domain: "MarsaSpikeMissingVirtioSocket", code: 1))
            return
        }
        socket.connect(toPort: port, completionHandler: { [weak self] result in
            guard let self, self.active else { return }
            switch result {
            case let .success(connection):
                self.connection = connection
                let descriptor = fcntl(connection.fileDescriptor, F_DUPFD_CLOEXEC, 0)
                if descriptor < 0 {
                    self.emit(.failed, error: NSError(domain: NSPOSIXErrorDomain, code: Int(errno)))
                    return
                }
                self.emit(.connected, descriptor: descriptor)
            case let .failure(error):
                self.emit(.failed, error: error)
            }
        })
    }

    func deactivate() {
        active = false
        vm.delegate = nil
    }

    func guestDidStop(_ virtualMachine: VZVirtualMachine) {
        emit(.stopped)
    }

    func virtualMachine(_ virtualMachine: VZVirtualMachine, didStopWithError error: any Error) {
        emit(.failed, error: error)
    }

    private func emit(_ event: Event, descriptor: Int32 = -1, error: (any Error)? = nil) {
        guard active else { return }
        if let error {
            error.localizedDescription.withCString { callback(context, event.rawValue, descriptor, $0) }
        } else {
            callback(context, event.rawValue, descriptor, nil)
        }
    }
}

private func configure(root: URL, serial: Int32) throws -> (VZVirtualMachineConfiguration, FileHandle) {
    let configuration = VZVirtualMachineConfiguration()
    configuration.cpuCount = 2
    configuration.memorySize = 1024 * 1024 * 1024

    let loader = VZLinuxBootLoader(kernelURL: root.appending(path: "Image"))
    loader.initialRamdiskURL = root.appending(path: "initramfs")
    loader.commandLine = "console=hvc0 rdinit=/marsa-spike-init panic=-1"
    configuration.bootLoader = loader

    let console = VZVirtioConsoleDeviceSerialPortConfiguration()
    console.attachment = try VZFileHandleSerialPortAttachment(
        fileHandleForReading: duplicate(serial), fileHandleForWriting: duplicate(serial)
    )
    configuration.serialPorts = [console]

    let modules = try VZDiskImageStorageDeviceAttachment(url: root.appending(path: "modules.img"), readOnly: true)
    let data = try VZDiskImageStorageDeviceAttachment(url: root.appending(path: "data.img"), readOnly: false)
    configuration.storageDevices = [
        VZVirtioBlockDeviceConfiguration(attachment: modules),
        VZVirtioBlockDeviceConfiguration(attachment: data),
    ]

    let share = VZVirtioFileSystemDeviceConfiguration(tag: "marsa-spike")
    let directory = VZSharedDirectory(url: root.appending(path: "share"), readOnly: true)
    share.share = VZSingleDirectoryShare(directory: directory)
    configuration.directorySharingDevices = [share]

    let nat = VZVirtioNetworkDeviceConfiguration()
    nat.attachment = VZNATNetworkDeviceAttachment()
    var descriptors = [Int32](repeating: -1, count: 2)
    guard socketpair(AF_UNIX, SOCK_DGRAM, 0, &descriptors) == 0 else {
        throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
    }
    let handle = VZVirtioNetworkDeviceConfiguration()
    handle.attachment = VZFileHandleNetworkDeviceAttachment(
        fileHandle: FileHandle(fileDescriptor: descriptors[0], closeOnDealloc: true)
    )
    let peer = FileHandle(fileDescriptor: descriptors[1], closeOnDealloc: true)
    configuration.networkDevices = [nat, handle]
    configuration.socketDevices = [VZVirtioSocketDeviceConfiguration()]
    configuration.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]
    configuration.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
    try configuration.validate()
    return (configuration, peer)
}

private func duplicate(_ descriptor: Int32) throws -> FileHandle {
    let cloned = fcntl(descriptor, F_DUPFD_CLOEXEC, 0)
    guard cloned >= 0 else {
        throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
    }
    return FileHandle(fileDescriptor: cloned, closeOnDealloc: true)
}

@c public func marsa_vz_start(
    _ root: UnsafePointer<CChar>, _ serial: Int32, _ context: UnsafeMutableRawPointer,
    _ callback: MarsaEvent
) -> UnsafeMutableRawPointer? {
    precondition(Thread.isMainThread)
    do {
        let path = URL(fileURLWithPath: String(cString: root), isDirectory: true)
        let session = try Session(root: path, serial: serial, context: context, callback: callback)
        session.start()
        return Unmanaged.passRetained(session).toOpaque()
    } catch {
        error.localizedDescription.withCString { callback(context, Event.failed.rawValue, -1, $0) }
        return nil
    }
}

@c public func marsa_vz_connect(_ handle: UnsafeMutableRawPointer, _ port: UInt32) {
    precondition(Thread.isMainThread)
    Unmanaged<Session>.fromOpaque(handle).takeUnretainedValue().connect(port: port)
}

@c public func marsa_vz_destroy(_ handle: UnsafeMutableRawPointer) {
    precondition(Thread.isMainThread)
    Unmanaged<Session>.fromOpaque(handle).takeRetainedValue().deactivate()
}
