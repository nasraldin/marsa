//! Dispatch native calls, retain the VM across callbacks and dispose of it on the same queue.

use std::{
    cell::RefCell,
    sync::{Arc, Weak},
};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use marsa_vm::{VmError, VmState};
use objc2::{
    AnyThread, DefinedClass, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::ProtocolObject,
};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_virtualization::{VZVirtualMachine, VZVirtualMachineDelegate, VZVirtualMachineState};
use tokio::sync::oneshot;

use super::{
    configuration,
    spec::Prepared,
    state::{Lifecycle, backend},
};

type StartReply = oneshot::Sender<Result<(), VmError>>;

#[derive(PartialEq, Eq)]
pub(super) enum StopRequest {
    WaitForStart,
    Graceful,
    Force,
}

pub(super) struct Controller {
    queue: DispatchRetained<DispatchQueue>,
    // Confined access is enforced by submit and checked at the native callback boundary.
    slot: QueueSlot,
    pub(super) lifecycle: Lifecycle,
}

#[derive(Default)]
struct Slot {
    machine: Option<Machine>,
    start_reply: Option<StartReply>,
    force_in_flight: bool,
    closing: bool,
}

struct QueueSlot(RefCell<Slot>);

// SAFETY: The slot is private to its controller. Only jobs on its one serial
// queue read or write it. close releases native state on that queue before
// the last owner can drop the controller on another thread.
unsafe impl Send for QueueSlot {}
// SAFETY: The same queue invariant serializes all access to the RefCell.
// Jobs and completion blocks carry only an Arc to the controller.
unsafe impl Sync for QueueSlot {}

struct Machine {
    vm: Retained<VZVirtualMachine>,
    _delegate: Retained<Delegate>,
}

define_class!(
    // SAFETY: NSObject provides the delegate's base class. The ivars are a weak
    // Rust reference. All delegate calls and the owned retain use the VM queue.
    #[unsafe(super(NSObject))]
    #[name = "MarsaProductionVZDelegate"]
    #[thread_kind = AnyThread]
    #[ivars = Weak<Controller>]
    struct Delegate;

    // SAFETY: NSObject implements NSObjectProtocol for this subclass.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: The selectors and argument types match VZVirtualMachineDelegate.
    // Virtualization.framework invokes them on the VM's serial queue.
    unsafe impl VZVirtualMachineDelegate for Delegate {
        #[unsafe(method(guestDidStopVirtualMachine:))]
        fn guest_did_stop(&self, _vm: &VZVirtualMachine) {
            if let Some(controller) = self.ivars().upgrade() {
                controller.queue.assert();
                controller.submit(|controller| controller.exit(None));
            }
        }

        #[unsafe(method(virtualMachine:didStopWithError:))]
        fn did_stop_with_error(&self, _vm: &VZVirtualMachine, error: &NSError) {
            if let Some(controller) = self.ivars().upgrade() {
                controller.queue.assert();
                let detail = error.localizedDescription().to_string();
                controller.submit(move |controller| controller.exit(Some(detail)));
            }
        }
    }
);

impl Controller {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: DispatchQueue::new("org.marsa.vz", None),
            slot: QueueSlot(RefCell::new(Slot::default())),
            lifecycle: Lifecycle::new(),
        })
    }

    pub(super) fn submit(self: &Arc<Self>, work: impl FnOnce(&Arc<Self>) + Send + 'static) {
        let controller = self.clone();
        self.queue.exec_async(move || {
            controller.queue.assert();
            autoreleasepool(|_| work(&controller));
        });
    }

    pub(super) async fn call<T: Send + 'static>(
        self: &Arc<Self>,
        operation: &'static str,
        work: impl FnOnce(&Arc<Self>) -> Result<T, VmError> + Send + 'static,
    ) -> Result<T, VmError> {
        let (send, receive) = oneshot::channel();
        self.submit(move |controller| {
            let _ = send.send(work(controller));
        });
        receive
            .await
            .map_err(|_| backend(operation, "the VZ queue lost the result"))?
    }

    pub(super) fn create(self: &Arc<Self>, prepared: Prepared) -> Result<(), VmError> {
        let configuration = configuration::build(prepared)?;
        let delegate = Delegate::alloc().set_ivars(Arc::downgrade(self));
        // SAFETY: NSObject initializes the allocated subclass on its owning queue.
        let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
        // SAFETY: The configuration passed validation. The current queue is serial
        // and is also the queue supplied to the initializer.
        let vm = unsafe {
            let vm = VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &configuration,
                &self.queue,
            );
            vm.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            vm
        };
        self.slot.0.borrow_mut().machine = Some(Machine {
            vm,
            _delegate: delegate,
        });
        Ok(())
    }

    pub(super) fn start(self: &Arc<Self>, reply: StartReply) {
        let mut slot = self.slot.0.borrow_mut();
        let Slot {
            machine,
            start_reply,
            ..
        } = &mut *slot;
        let Some(machine) = machine else {
            let _ = reply.send(Err(backend("start", "the native VM is unavailable")));
            return;
        };
        let state = self.lifecycle.current();
        // SAFETY: Only a job on the owning serial queue can reach this native object.
        if !matches!(state, VmState::Created | VmState::Stopped)
            || !unsafe { machine.vm.canStart() }
        {
            let _ = reply.send(Err(VmError::InvalidState {
                operation: "start",
                state,
            }));
            return;
        }
        self.lifecycle.transition(VmState::Starting);
        *start_reply = Some(reply);
        let controller = self.clone();
        let completion = RcBlock::new(move |error: *mut NSError| {
            controller.queue.assert();
            let error = callback_error(error);
            // Queue another job to avoid borrowing the native slot during an inline callback.
            controller.submit(move |controller| controller.started(error));
        });
        // SAFETY: The VM can start and the copied completion is invoked on this queue.
        unsafe { machine.vm.startWithCompletionHandler(&completion) };
    }

    fn started(self: &Arc<Self>, error: Option<String>) {
        let reply = self.slot.0.borrow_mut().start_reply.take();
        let result = if let Some(detail) = error {
            self.lifecycle.transition(VmState::Failed);
            Err(backend("start", detail))
        } else if self.lifecycle.current() == VmState::Starting {
            self.lifecycle.transition(VmState::Running);
            Ok(())
        } else {
            Err(backend("start", "the VM exited during startup"))
        };
        if let Some(reply) = reply {
            let _ = reply.send(result);
        }
        self.cleanup();
    }

    pub(super) fn request_stop(&self) -> Result<StopRequest, VmError> {
        let current = self.lifecycle.current();
        match current {
            VmState::Stopped => Ok(StopRequest::Force),
            VmState::Created => {
                self.lifecycle.transition(VmState::Stopped);
                Ok(StopRequest::Force)
            }
            VmState::Failed => {
                if !self.can_force_stop() {
                    self.lifecycle.transition(VmState::Stopped);
                }
                Ok(StopRequest::Force)
            }
            VmState::Stopping => Ok(StopRequest::Graceful),
            VmState::Starting => Ok(StopRequest::WaitForStart),
            VmState::Running | VmState::Paused => {
                let slot = self.slot.0.borrow();
                let machine = slot
                    .machine
                    .as_ref()
                    .ok_or_else(|| backend("stop", "the native VM is unavailable"))?;
                self.lifecycle.transition(VmState::Stopping);
                // SAFETY: The VM is on its owning queue. A rejected guest shutdown
                // request is followed by the host's forced stop.
                let requested = unsafe {
                    machine.vm.canRequestStop() && machine.vm.requestStopWithError().is_ok()
                };
                Ok(if requested {
                    StopRequest::Graceful
                } else {
                    StopRequest::Force
                })
            }
        }
    }

    pub(super) fn force_stop(self: &Arc<Self>) -> Result<(), VmError> {
        let mut slot = self.slot.0.borrow_mut();
        let Slot {
            machine,
            force_in_flight,
            ..
        } = &mut *slot;
        if *force_in_flight {
            return Ok(());
        }
        let machine = machine
            .as_ref()
            .ok_or_else(|| backend("stop", "the native VM is unavailable"))?;
        // SAFETY: Read native state and submit shutdown on the VM's serial queue.
        unsafe {
            if !machine.vm.canStop() {
                if self.lifecycle.current() == VmState::Stopped
                    || machine.vm.state() == VZVirtualMachineState::Stopped
                {
                    self.lifecycle.transition(VmState::Stopped);
                    return Ok(());
                }
                let detail = "the native VM is unable to stop";
                self.lifecycle.unexpected_exit(detail.into());
                return Err(backend("stop", detail));
            }
            self.lifecycle.transition(VmState::Stopping);
            *force_in_flight = true;
            let controller = self.clone();
            let completion = RcBlock::new(move |error: *mut NSError| {
                controller.queue.assert();
                let error = callback_error(error);
                controller.submit(move |controller| controller.forced(error));
            });
            machine.vm.stopWithCompletionHandler(&completion);
        }
        Ok(())
    }

    fn forced(self: &Arc<Self>, error: Option<String>) {
        let mut slot = self.slot.0.borrow_mut();
        slot.force_in_flight = false;
        // SAFETY: This test runs on the VM queue. The guest may have stopped
        // between the forced-stop call and its completion.
        let stopped = slot
            .machine
            .as_ref()
            .is_some_and(|machine| unsafe { machine.vm.state() == VZVirtualMachineState::Stopped });
        drop(slot);
        if let Some(detail) = error.filter(|_| !stopped) {
            self.lifecycle.unexpected_exit(detail);
            let mut slot = self.slot.0.borrow_mut();
            if slot.closing {
                // This teardown attempt failed. Release our native retains on the queue.
                slot.machine.take();
                return;
            }
        } else {
            self.lifecycle.transition(VmState::Stopped);
        }
        self.cleanup();
    }

    fn exit(self: &Arc<Self>, error: Option<String>) {
        if error.is_none() && self.lifecycle.current() == VmState::Stopping {
            if !self.slot.0.borrow().force_in_flight {
                self.lifecycle.transition(VmState::Stopped);
            }
        } else {
            self.lifecycle.unexpected_exit(
                error.unwrap_or_else(|| "the guest shut down without a stop request".into()),
            );
        }
        self.cleanup();
    }

    pub(super) fn close(self: &Arc<Self>) {
        self.slot.0.borrow_mut().closing = true;
        self.cleanup();
    }

    fn can_force_stop(&self) -> bool {
        self.slot
            .0
            .borrow()
            .machine
            .as_ref()
            .is_some_and(|machine| {
                // SAFETY: The native object is only inspected on its owning queue.
                unsafe { machine.vm.canStop() }
            })
    }

    fn cleanup(self: &Arc<Self>) {
        let slot = self.slot.0.borrow();
        if !slot.closing || slot.start_reply.is_some() || slot.force_in_flight {
            return;
        }
        drop(slot);
        if self.can_force_stop() {
            let _ = self.force_stop();
        }
        let mut slot = self.slot.0.borrow_mut();
        if !slot.force_in_flight {
            slot.machine.take();
        }
    }
}

fn callback_error(error: *mut NSError) -> Option<String> {
    // SAFETY: The framework lends either null or a live NSError until its callback returns.
    autoreleasepool(|_| {
        unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string())
    })
}
