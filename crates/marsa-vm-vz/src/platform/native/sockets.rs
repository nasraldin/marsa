//! Retain native vsock connections on the VM queue and lend duplicated descriptors.

use std::{
    os::fd::BorrowedFd,
    sync::{Arc, Mutex},
};

use block2::RcBlock;
use marsa_vm::{VmError, VmState};
use objc2::rc::Retained;
use objc2_foundation::{NSError, NSPOSIXErrorDomain};
use objc2_virtualization::{VZVirtioSocketConnection, VZVirtioSocketDevice};

use super::Controller;
use crate::platform::{
    state::backend,
    vsock::{self, CONNECT, Connected, Lease},
};

impl Controller {
    pub(in crate::platform) fn connect_socket(self: &Arc<Self>, port: u32, reply: vsock::Reply) {
        if reply.is_closed() {
            return;
        }
        let (device, generation, id) = match self.socket_request() {
            Ok(request) => request,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        let controller = Arc::downgrade(self);
        let reply = Mutex::new(Some(reply));
        let completion = RcBlock::new(
            move |connection: *mut VZVirtioSocketConnection, error: *mut NSError| {
                let Some(controller) = controller.upgrade() else {
                    return;
                };
                controller.queue.assert();
                let reply = reply
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .take();
                if let Some(reply) = reply.filter(|reply| !reply.is_closed()) {
                    // SAFETY: VZ lends a null or live connection during this callback.
                    let connection = unsafe { Retained::retain(connection) };
                    let result = controller.connected(port, id, &generation, connection, error);
                    // A cancelled receiver drops the descriptor and schedules the native release.
                    let _ = reply.send(result);
                }
            },
        );
        // SAFETY: The device belongs to this running VM on this queue. No slot borrow
        // is held since the framework can invoke the completion inline.
        unsafe { device.connectToPort_completionHandler(port, &completion) };
    }

    fn socket_request(&self) -> Result<(Retained<VZVirtioSocketDevice>, Arc<()>, u64), VmError> {
        self.require_running()?;
        let mut slot = self.slot.0.borrow_mut();
        if slot.power_change.is_some() {
            return Err(VmError::InvalidState {
                operation: CONNECT,
                state: self.lifecycle.current(),
            });
        }
        let id = slot.next_socket;
        slot.next_socket = id
            .checked_add(1)
            .ok_or_else(|| backend(CONNECT, "the socket lease IDs are exhausted"))?;
        let machine = slot
            .machine
            .as_ref()
            .ok_or_else(|| backend(CONNECT, "the native VM is unavailable"))?;
        // SAFETY: The machine and its devices are accessed on the owning queue.
        let device = unsafe { machine.vm.socketDevices() }
            .firstObject()
            .and_then(|device| device.downcast::<VZVirtioSocketDevice>().ok())
            .ok_or_else(|| backend(CONNECT, "the VM has no virtio socket device"))?;
        Ok((device, machine.generation.clone(), id))
    }

    fn connected(
        self: &Arc<Self>,
        port: u32,
        id: u64,
        generation: &Arc<()>,
        connection: Option<Retained<VZVirtioSocketConnection>>,
        error: *mut NSError,
    ) -> Result<Connected, VmError> {
        self.require_running()?;
        let mut slot = self.slot.0.borrow_mut();
        let machine = slot
            .machine
            .as_mut()
            .filter(|machine| Arc::ptr_eq(&machine.generation, generation))
            .ok_or_else(|| backend(CONNECT, "the VM restarted during the connection attempt"))?;
        // SAFETY: The framework lends a null or live NSError for this callback.
        let connection = connection.ok_or_else(|| socket_error(port, unsafe { error.as_ref() }))?;
        // SAFETY: The connection is used on the VM queue and owns its descriptor.
        let fd = unsafe { connection.fileDescriptor() };
        if fd < 0 {
            return Err(backend(CONNECT, "the native socket is already closed"));
        }
        // SAFETY: The retained connection outlives the borrow during duplication.
        let fd = unsafe { BorrowedFd::borrow_raw(fd) }
            .try_clone_to_owned()
            .map_err(vsock::io_error)?;
        machine.sockets.insert(id, connection);
        Ok(Connected {
            fd,
            lease: Lease {
                controller: self.clone(),
                id,
            },
        })
    }

    pub(in crate::platform) fn release_socket(&self, id: u64) {
        let socket = self
            .slot
            .0
            .borrow_mut()
            .machine
            .as_mut()
            .and_then(|machine| machine.sockets.remove(&id));
        drop(socket);
    }

    fn require_running(&self) -> Result<(), VmError> {
        let state = self.lifecycle.current();
        if state != VmState::Running {
            return Err(VmError::InvalidState {
                operation: CONNECT,
                state,
            });
        }
        Ok(())
    }
}

fn socket_error(port: u32, error: Option<&NSError>) -> VmError {
    let Some(error) = error else {
        return backend(CONNECT, "missing native socket result");
    };
    // SAFETY: Foundation provides the POSIX domain as an immutable NSString.
    if &*error.domain() == unsafe { NSPOSIXErrorDomain }
        && [libc::ECONNRESET as isize, libc::ECONNREFUSED as isize].contains(&error.code())
    {
        // VZ reports ECONNRESET on this platform until the guest starts listening.
        return VmError::GuestPortUnavailable { port };
    }
    backend(
        CONNECT,
        format!(
            "port {port}: {} ({}, {})",
            error.localizedDescription(),
            error.domain(),
            error.code()
        ),
    )
}
