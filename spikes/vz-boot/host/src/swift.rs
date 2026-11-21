//! Bridge Swift's C callbacks into the same event stream as the direct binding.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::ptr::NonNull;
use std::sync::mpsc::Sender;

use objc2::MainThreadMarker;

use crate::runner::{Event, VSOCK_PORT, Vm, VmInputs};

type Callback = unsafe extern "C" fn(*mut c_void, i32, i32, *const c_char);

// SAFETY: Bridge.swift exports these functions using Swift's stable C ABI.
unsafe extern "C" {
    fn marsa_vz_start(
        root: *const c_char,
        serial: i32,
        context: *mut c_void,
        callback: Callback,
    ) -> *mut c_void;
    fn marsa_vz_connect(handle: *mut c_void, port: u32);
    fn marsa_vz_destroy(handle: *mut c_void);
}

struct CallbackState {
    sender: Sender<Event>,
    last_error: RefCell<Option<String>>,
}

struct SwiftVm {
    handle: NonNull<c_void>,
    _callback: Box<CallbackState>,
}

pub fn start(inputs: &VmInputs<'_>, sender: Sender<Event>) -> io::Result<Box<dyn Vm>> {
    if MainThreadMarker::new().is_none() {
        return Err(io::Error::other(
            "Swift Virtualization bridge requires the main queue",
        ));
    }
    let path = CString::new(inputs.fixtures.as_os_str().as_bytes())?;
    let mut callback = Box::new(CallbackState {
        sender,
        last_error: RefCell::new(None),
    });
    let state = std::ptr::from_mut(&mut *callback).cast::<c_void>();
    // SAFETY: The path is live for the call, and the callback allocation is owned by the VM
    // until marsa_vz_destroy disables all callbacks. Swift duplicates the live console FD.
    let handle =
        unsafe { marsa_vz_start(path.as_ptr(), inputs.serial.as_raw_fd(), state, receive) };
    match NonNull::new(handle) {
        Some(handle) => Ok(Box::new(SwiftVm {
            handle,
            _callback: callback,
        })),
        None => Err(io::Error::other(
            callback
                .last_error
                .take()
                .unwrap_or_else(|| "Swift VM configuration failed".into()),
        )),
    }
}

impl Vm for SwiftVm {
    fn connect(&self) -> io::Result<()> {
        // SAFETY: The retained handle is valid and the runner calls this only on the main queue.
        unsafe { marsa_vz_connect(self.handle.as_ptr(), VSOCK_PORT) };
        Ok(())
    }
}

impl Drop for SwiftVm {
    fn drop(&mut self) {
        // SAFETY: Swift callbacks are disabled on this main queue before the Rust context is freed.
        unsafe { marsa_vz_destroy(self.handle.as_ptr()) };
    }
}

unsafe extern "C" fn receive(context: *mut c_void, event: i32, fd: i32, message: *const c_char) {
    // SAFETY: Swift enters synchronously on the main queue with the live allocation from start.
    let callback = unsafe { &*context.cast::<CallbackState>() };
    let event = match event {
        1 => Event::Started,
        2 if fd >= 0 => {
            // SAFETY: Swift transfers its independently duplicated descriptor exactly once.
            Event::Connected(unsafe { OwnedFd::from_raw_fd(fd) })
        }
        3 => Event::Stopped,
        4 if !message.is_null() => {
            // SAFETY: Swift's withCString keeps the NUL-terminated text live across this call.
            let error = unsafe { CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned();
            callback.last_error.replace(Some(error.clone()));
            Event::Error(error)
        }
        _ => Event::Error("unexpected Swift callback".into()),
    };
    let _ = callback.sender.send(event);
}
