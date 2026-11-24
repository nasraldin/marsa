//! Build and validate the minimal Virtualization.framework configuration on its queue.

use std::{fs::File, os::fd::IntoRawFd, path::Path};

use marsa_vm::{DiskSpec, ShareSpec, VmError};
use objc2::{AnyThread, rc::Retained};
use objc2_foundation::{NSArray, NSFileHandle, NSString, NSURL};
use objc2_virtualization::{
    VZDirectorySharingDeviceConfiguration, VZDiskImageCachingMode,
    VZDiskImageStorageDeviceAttachment, VZDiskImageSynchronizationMode,
    VZFileHandleNetworkDeviceAttachment, VZFileHandleSerialPortAttachment, VZLinuxBootLoader,
    VZMACAddress, VZNATNetworkDeviceAttachment, VZSharedDirectory, VZSingleDirectoryShare,
    VZVirtioBlockDeviceConfiguration, VZVirtioConsoleDeviceSerialPortConfiguration,
    VZVirtioEntropyDeviceConfiguration, VZVirtioFileSystemDeviceConfiguration,
    VZVirtioNetworkDeviceConfiguration, VZVirtioSocketDeviceConfiguration,
    VZVirtioTraditionalMemoryBalloonDeviceConfiguration, VZVirtualMachine,
    VZVirtualMachineConfiguration,
};

use super::{
    spec::Prepared,
    state::{backend, invalid_spec},
};

pub(super) fn build(
    prepared: Prepared,
    network_socket: Option<File>,
) -> Result<Retained<VZVirtualMachineConfiguration>, VmError> {
    let spec = prepared.spec;
    let kernel = file_url("kernel", &spec.kernel)?;
    let initramfs = spec
        .initramfs
        .as_deref()
        .map(|path| file_url("initramfs", path))
        .transpose()?;

    let shares = spec
        .shares
        .iter()
        .map(share)
        .collect::<Result<Vec<_>, _>>()?;

    // SAFETY: All framework allocations, configuration calls and drops occur on the
    // controller's serial queue. The device setters retain or copy their inputs.
    unsafe {
        let cpu_limits = VZVirtualMachineConfiguration::minimumAllowedCPUCount()
            ..=VZVirtualMachineConfiguration::maximumAllowedCPUCount();
        if !cpu_limits.contains(&(spec.cpus as usize)) {
            return Err(invalid_spec(
                "cpus",
                format!("allowed CPU count is {cpu_limits:?}"),
            ));
        }
        let memory_limits = VZVirtualMachineConfiguration::minimumAllowedMemorySize()
            ..=VZVirtualMachineConfiguration::maximumAllowedMemorySize();
        if !memory_limits.contains(&spec.memory_bytes) {
            return Err(invalid_spec(
                "memory_bytes",
                format!("allowed memory range is {memory_limits:?}"),
            ));
        }

        if !VZVirtualMachine::isSupported() {
            return Err(backend("create", "Virtualization.framework is unavailable"));
        }
        let disk_root = disk("root_disk", &spec.root_disk)?;
        let disk_data = disk("data_disk", &spec.data_disk)?;

        let loader = VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &kernel);
        loader.setInitialRamdiskURL(initramfs.as_deref());
        loader.setCommandLine(&NSString::from_str(&spec.cmdline));

        let console = VZVirtioConsoleDeviceSerialPortConfiguration::new();
        let console_out = handle(prepared.console_out);
        let console_attachment =
            VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
                VZFileHandleSerialPortAttachment::alloc(),
                None,
                Some(&console_out),
            );
        console.setAttachment(Some(&console_attachment));
        let network = VZVirtioNetworkDeviceConfiguration::new();
        if let Some(socket) = network_socket {
            let file = handle(socket);
            let attachment = VZFileHandleNetworkDeviceAttachment::initWithFileHandle(
                VZFileHandleNetworkDeviceAttachment::alloc(),
                &file,
            );
            let address = VZMACAddress::initWithString(
                VZMACAddress::alloc(),
                &NSString::from_str(super::userspace::GUEST_MAC),
            )
            .ok_or_else(|| backend("create", "invalid user-space MAC address"))?;
            network.setMACAddress(&address);
            network.setAttachment(Some(&attachment));
        } else {
            network.setAttachment(Some(&VZNATNetworkDeviceAttachment::new()));
        }

        let config = VZVirtualMachineConfiguration::new();
        config.setCPUCount(spec.cpus as usize);
        config.setMemorySize(spec.memory_bytes);
        config.setBootLoader(Some(&loader));
        config.setStorageDevices(&NSArray::from_retained_slice(&[
            disk_root.into_super(),
            disk_data.into_super(),
        ]));
        config.setDirectorySharingDevices(&NSArray::from_retained_slice(&shares));
        config.setMemoryBalloonDevices(&NSArray::from_retained_slice(&[
            VZVirtioTraditionalMemoryBalloonDeviceConfiguration::new().into_super(),
        ]));
        config.setSerialPorts(&NSArray::from_retained_slice(&[console.into_super()]));
        config.setSocketDevices(&NSArray::from_retained_slice(&[
            VZVirtioSocketDeviceConfiguration::new().into_super(),
        ]));
        config.setEntropyDevices(&NSArray::from_retained_slice(&[
            VZVirtioEntropyDeviceConfiguration::new().into_super(),
        ]));
        config.setNetworkDevices(&NSArray::from_retained_slice(&[network.into_super()]));
        config.validateWithError().map_err(|error| {
            invalid_spec("configuration", error.localizedDescription().to_string())
        })?;
        Ok(config)
    }
}

fn share(spec: &ShareSpec) -> Result<Retained<VZDirectorySharingDeviceConfiguration>, VmError> {
    let url = file_url("shares.host_path", &spec.host_path)?;
    let tag = NSString::from_str(&spec.tag);
    // SAFETY: The approved directory and the unique tag passed preflight. All
    // share objects are constructed, used and released on the VM queue.
    unsafe {
        VZVirtioFileSystemDeviceConfiguration::validateTag_error(&tag).map_err(|error| {
            invalid_spec("shares.tag", error.localizedDescription().to_string())
        })?;
        let directory = VZSharedDirectory::initWithURL_readOnly(
            VZSharedDirectory::alloc(),
            &url,
            spec.read_only,
        );
        let directory =
            VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &directory);
        let device = VZVirtioFileSystemDeviceConfiguration::initWithTag(
            VZVirtioFileSystemDeviceConfiguration::alloc(),
            &tag,
        );
        device.setShare(Some(&directory));
        Ok(device.into_super())
    }
}

fn disk(
    field: &'static str,
    spec: &DiskSpec,
) -> Result<Retained<VZVirtioBlockDeviceConfiguration>, VmError> {
    let url = file_url(field, &spec.path)?;
    // SAFETY: The checked URL is a regular raw disk and configuration stays on the queue.
    unsafe {
        let attachment = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_cachingMode_synchronizationMode_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &url,
            spec.read_only,
            VZDiskImageCachingMode::Automatic,
            VZDiskImageSynchronizationMode::Full,
        )
        .map_err(|error| invalid_spec(field, error.localizedDescription().to_string()))?;
        Ok(VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &attachment,
        ))
    }
}

fn file_url(field: &'static str, path: &Path) -> Result<Retained<NSURL>, VmError> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid_spec(field, "expected a UTF-8 path"))?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(text)))
}

fn handle(file: File) -> Retained<NSFileHandle> {
    NSFileHandle::initWithFileDescriptor_closeOnDealloc(
        NSFileHandle::alloc(),
        file.into_raw_fd(),
        true,
    )
}
