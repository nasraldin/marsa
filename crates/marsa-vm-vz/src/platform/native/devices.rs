//! Apply queue-ordered power changes, memory targets, and network snapshots.

use std::sync::Arc;

use block2::RcBlock;
use marsa_vm::{NetworkInfo, VmError, VmState};
use objc2_foundation::NSError;
use objc2_virtualization::{VZVirtioTraditionalMemoryBalloonDevice, VZVirtualMachineConfiguration};
use tokio::sync::watch;

use super::{Controller, Reply, Slot, callback_error};
use crate::platform::{
    spec::MIB,
    state::{backend, invalid_spec},
};

pub(super) struct PowerChange {
    pub(super) reply: Reply,
    pub(super) finished: watch::Sender<bool>,
}

#[derive(Clone, Copy)]
pub(in crate::platform) enum Power {
    Pause,
    Resume,
}

impl Power {
    pub(in crate::platform) fn name(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
        }
    }

    fn states(self) -> (VmState, VmState) {
        match self {
            Self::Pause => (VmState::Running, VmState::Paused),
            Self::Resume => (VmState::Paused, VmState::Running),
        }
    }
}

impl Controller {
    pub(in crate::platform) fn change_power(self: &Arc<Self>, power: Power, reply: Reply) {
        let mut slot = self.slot.0.borrow_mut();
        let state = self.lifecycle.current();
        let Slot {
            machine,
            power_change,
            ..
        } = &mut *slot;
        let Some(machine) = machine else {
            let _ = reply.send(Err(backend(power.name(), "the native VM is unavailable")));
            return;
        };
        // SAFETY: Native state is inspected and changed only on the VM queue.
        let ready = unsafe {
            match power {
                Power::Pause => machine.vm.canPause(),
                Power::Resume => machine.vm.canResume(),
            }
        };
        if power_change.is_some() || state != power.states().0 || !ready {
            let _ = reply.send(Err(VmError::InvalidState {
                operation: power.name(),
                state,
            }));
            return;
        }
        *power_change = Some(PowerChange {
            reply,
            finished: watch::channel(false).0,
        });
        let controller = self.clone();
        let completion = RcBlock::new(move |error: *mut NSError| {
            controller.queue.assert();
            let error = callback_error(error);
            // Enqueue completion to keep an inline callback out of the slot.
            controller.submit(move |controller| controller.power_changed(power, error));
        });
        // SAFETY: The framework checked the legal transition on this serial queue.
        unsafe {
            match power {
                Power::Pause => machine.vm.pauseWithCompletionHandler(&completion),
                Power::Resume => machine.vm.resumeWithCompletionHandler(&completion),
            }
        }
    }

    fn power_changed(self: &Arc<Self>, power: Power, error: Option<String>) {
        let change = self.slot.0.borrow_mut().power_change.take();
        let (previous, current) = power.states();
        let result = if let Some(detail) = error {
            Err(backend(power.name(), detail))
        } else if self.lifecycle.current() == previous {
            self.lifecycle.transition(current);
            Ok(())
        } else {
            Err(backend(power.name(), "the VM exited during the transition"))
        };
        if let Some(change) = change {
            change.finished.send_replace(true);
            let _ = change.reply.send(result);
        }
        self.cleanup();
    }

    pub(in crate::platform) fn set_memory_target(&self, bytes: u64) -> Result<(), VmError> {
        let slot = self.slot.0.borrow();
        let state = self.lifecycle.current();
        if state != VmState::Running || slot.power_change.is_some() {
            return Err(VmError::InvalidState {
                operation: "set memory target",
                state,
            });
        }
        let machine = slot
            .machine
            .as_ref()
            .ok_or_else(|| backend("set memory target", "the native VM is unavailable"))?;
        // SAFETY: The configured VM and its balloon are used on their owning queue.
        unsafe {
            let minimum = VZVirtualMachineConfiguration::minimumAllowedMemorySize();
            if !bytes.is_multiple_of(MIB) || !(minimum..=machine.memory_bytes).contains(&bytes) {
                return Err(invalid_spec(
                    "memory_target",
                    format!(
                        "use 1 MiB increments between {minimum} and {} bytes",
                        machine.memory_bytes
                    ),
                ));
            }
            let balloon = machine
                .vm
                .memoryBalloonDevices()
                .firstObject()
                .and_then(|device| {
                    device
                        .downcast::<VZVirtioTraditionalMemoryBalloonDevice>()
                        .ok()
                })
                .ok_or_else(|| backend("set memory target", "the VM has no memory balloon"))?;
            balloon.setTargetVirtualMachineMemorySize(bytes);
            if balloon.targetVirtualMachineMemorySize() != bytes {
                return Err(backend(
                    "set memory target",
                    "Virtualization.framework changed the requested target",
                ));
            }
        }
        Ok(())
    }

    pub(in crate::platform) fn publish_network(
        &self,
        observed: &watch::Receiver<VmState>,
        facts: Option<NetworkInfo>,
    ) -> bool {
        // Compare the observed state on the queue that orders all transitions.
        if matches!(observed.has_changed(), Ok(false))
            && self.slot.0.borrow().power_change.is_none()
        {
            self.lifecycle.set_network(facts);
        }
        self.lifecycle.network().is_some()
    }
}
