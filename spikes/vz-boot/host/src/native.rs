//! Configure and drive Virtualization.framework through the generated objc2 bindings.

use std::cell::RefCell;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixDatagram;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_virtualization::{
    VZVirtioSocketConnection, VZVirtioSocketDevice, VZVirtualMachine, VZVirtualMachineDelegate,
};

mod configuration;

use crate::runner::{Event, VSOCK_PORT, Vm, VmInputs};

type Connection = Rc<RefCell<Option<Retained<VZVirtioSocketConnection>>>>;

define_class!(
    // SAFETY: NSObject is the correct root class. The delegate is created and called on the main queue.
    #[unsafe(super(NSObject))]
    #[name = "MarsaSpikeVmDelegate"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Sender<Event>]
    struct Delegate;

    // SAFETY: NSObject provides the base protocol's implementation.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: Selectors and signatures match VZVirtualMachineDelegate; all callbacks use the VM's queue.
    unsafe impl VZVirtualMachineDelegate for Delegate {
        #[unsafe(method(guestDidStopVirtualMachine:))]
        fn guest_did_stop(&self, _vm: &VZVirtualMachine) {
            let _ = self.ivars().send(Event::Stopped);
        }

        #[unsafe(method(virtualMachine:didStopWithError:))]
        fn did_stop_with_error(&self, _vm: &VZVirtualMachine, error: &NSError) {
            let _ = self.ivars().send(Event::Error(error.localizedDescription().to_string()));
        }
    }
);

struct DirectVm {
    vm: Retained<VZVirtualMachine>,
    sender: Sender<Event>,
    connection: Connection,
    _delegate: Retained<Delegate>,
    _network_peer: UnixDatagram,
}

pub fn start(inputs: &VmInputs<'_>, sender: Sender<Event>) -> io::Result<Box<dyn Vm>> {
    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| io::Error::other("Virtualization must be started from the main queue"))?;
    let (network_handle, network_peer) = UnixDatagram::pair()?;
    let configuration = configuration::build(inputs, network_handle.as_fd())?;
    let delegate = Delegate::alloc(main_thread).set_ivars(sender.clone());
    // SAFETY: The allocated subclass is initialized by NSObject on its marked main queue.
    let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };

    let callback_sender = sender.clone();
    let completion = RcBlock::new(move |error: *mut NSError| {
        let event = objc_error(error).map_or(Event::Started, Event::Error);
        let _ = callback_sender.send(event);
    });
    // SAFETY: The configuration passed validation, and the default VM queue is the main queue.
    // The bridge retains both the VM and delegate for the full test.
    let vm = unsafe {
        if !VZVirtualMachine::isSupported() {
            return Err(io::Error::other("Virtualization.framework is unavailable"));
        }
        let vm = VZVirtualMachine::initWithConfiguration(VZVirtualMachine::alloc(), &configuration);
        vm.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        vm.startWithCompletionHandler(&completion);
        vm
    };
    Ok(Box::new(DirectVm {
        vm,
        sender,
        connection: Rc::default(),
        _delegate: delegate,
        _network_peer: network_peer,
    }))
}

impl Vm for DirectVm {
    fn connect(&self) -> io::Result<()> {
        // SAFETY: this bridge is called only on the VM's main queue.
        let device = unsafe { self.vm.socketDevices() }
            .firstObject()
            .and_then(|device| device.downcast::<VZVirtioSocketDevice>().ok())
            .ok_or_else(|| io::Error::other("configured VM has no virtio socket device"))?;
        let sender = self.sender.clone();
        let keep_alive = self.connection.clone();
        let completion = RcBlock::new(move |connection: *mut VZVirtioSocketConnection, error| {
            // SAFETY: VZ lends the typed connection only for this callback.
            let connection = unsafe { Retained::retain(connection) };
            let event = match connection {
                Some(socket) => duplicate_socket(socket, &keep_alive),
                None => Event::ConnectFailed(
                    objc_error(error).unwrap_or_else(|| "vsock connect failed".into()),
                ),
            };
            let _ = sender.send(event);
        });
        // SAFETY: device belongs to the live VM and completion observes its queue.
        unsafe { device.connectToPort_completionHandler(VSOCK_PORT, &completion) };
        Ok(())
    }
}

fn duplicate_socket(socket: Retained<VZVirtioSocketConnection>, keep_alive: &Connection) -> Event {
    // SAFETY: connection is retained and accessed on its VM queue.
    let descriptor = unsafe { socket.fileDescriptor() };
    if descriptor < 0 {
        return Event::Error("Virtualization.framework returned a closed vsock".into());
    }
    // SAFETY: The live connection owns the descriptor and the borrow ends before it can close.
    let descriptor = unsafe { BorrowedFd::borrow_raw(descriptor) };
    match descriptor.try_clone_to_owned() {
        Ok(owned) => {
            keep_alive.replace(Some(socket));
            Event::Connected(owned)
        }
        Err(error) => Event::Error(format!("duplicate vsock descriptor: {error}")),
    }
}

fn objc_error(error: *mut NSError) -> Option<String> {
    // SAFETY: VZ supplies a nil or live NSError for the duration of its callback.
    unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string())
}
