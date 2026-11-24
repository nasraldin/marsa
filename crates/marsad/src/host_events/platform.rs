//! Subscribe to public macOS notifications from a daemon without a Cocoa run loop.

#![allow(unsafe_code)]

use std::{
    ffi::c_void,
    io,
    ptr::{self, NonNull},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc as sync_mpsc,
    },
    time::Duration,
};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use tokio::sync::{Semaphore, mpsc};

use super::{HostEvent, HostNotification};

// Values from IOKit/IOMessage.h and mach/error.h.
const IO_KIT_COMMON: u32 = 0x38 << 26;
const CAN_SYSTEM_SLEEP: u32 = IO_KIT_COMMON | 0x270;
const SYSTEM_WILL_SLEEP: u32 = IO_KIT_COMMON | 0x280;
const SYSTEM_HAS_POWERED_ON: u32 = IO_KIT_COMMON | 0x300;
// Keep Apple's 30-second power acknowledgment fallback out of normal operation.
const ACK_LIMIT: Duration = Duration::from_secs(2);

type Sender = mpsc::UnboundedSender<HostNotification>;

/// Lifetime of the daemon's native power and default network path subscriptions.
pub struct Watcher {
    _power: PowerWatcher,
    _network: NetworkWatcher,
}

impl Watcher {
    /// Subscribe on private dispatch queues and return a source for the daemon event loop.
    pub fn start() -> io::Result<(Self, mpsc::UnboundedReceiver<HostNotification>)> {
        let (sender, source) = mpsc::unbounded_channel();
        let power = PowerWatcher::start(sender.clone())?;
        let network = NetworkWatcher::start(sender)?;
        Ok((
            Self {
                _power: power,
                _network: network,
            },
            source,
        ))
    }
}

struct PowerContext {
    sender: Sender,
    connection: u32,
}

struct PowerWatcher {
    queue: DispatchRetained<DispatchQueue>,
    registration: Option<PowerRegistration>,
}

struct PowerRegistration {
    port: NonNull<c_void>,
    notifier: u32,
    context: Box<PowerContext>,
}

// SAFETY: The port is an owned IOKit handle. Its refcon stays at a stable box
// address, is immutable after registration and is freed on the callback queue.
unsafe impl Send for PowerRegistration {}

impl PowerWatcher {
    fn start(sender: Sender) -> io::Result<Self> {
        let queue = DispatchQueue::new("org.marsa.host-power", None);
        let mut context = Box::new(PowerContext {
            sender,
            connection: 0,
        });
        let mut port = ptr::null_mut();
        let mut notifier = 0;
        // SAFETY: Outputs are writable, the boxed refcon has a stable address,
        // and no callbacks are dispatched until we attach the queue.
        let connection = unsafe {
            IORegisterForSystemPower(
                ptr::from_mut(context.as_mut()).cast(),
                &mut port,
                power_changed,
                &mut notifier,
            )
        };
        let Some(port) = NonNull::new(port).filter(|_| connection != 0) else {
            return Err(io::Error::other("IORegisterForSystemPower failed"));
        };
        context.connection = connection;
        // SAFETY: The successfully registered port and dispatch queue are live.
        unsafe { IONotificationPortSetDispatchQueue(port.as_ptr(), &*queue) };
        Ok(Self {
            queue,
            registration: Some(PowerRegistration {
                port,
                notifier,
                context,
            }),
        })
    }
}

impl Drop for PowerWatcher {
    fn drop(&mut self) {
        if let Some(registration) = self.registration.take() {
            self.queue.exec_sync(move || registration.close());
        }
    }
}

impl PowerRegistration {
    fn close(mut self) {
        // SAFETY: This runs on the serial callback queue, after any in-flight
        // callback. Detaching cancels further callbacks before the context drops.
        unsafe {
            IONotificationPortSetDispatchQueue(self.port.as_ptr(), ptr::null());
            IODeregisterForSystemPower(&mut self.notifier);
            IOServiceClose(self.context.connection);
            IONotificationPortDestroy(self.port.as_ptr());
        }
    }
}

unsafe extern "C" fn power_changed(
    context: *mut c_void,
    _service: u32,
    message: u32,
    token: *mut c_void,
) {
    // SAFETY: We register the boxed context before enabling the callback queue
    // and retain it until callbacks have been disabled on that queue.
    let context = unsafe { &*context.cast::<PowerContext>() };
    match message {
        SYSTEM_WILL_SLEEP => {
            let (completed, wait_for_pause) = sync_mpsc::channel();
            let mut notification = HostNotification::from(HostEvent::WillSleep);
            notification.completed = Some(completed);
            if context.sender.send(notification).is_ok() {
                // This is the private IOKit queue. The Tokio and VZ queues remain
                // free to pause, and timeout still permits the Mac to sleep.
                let _ = wait_for_pause.recv_timeout(ACK_LIMIT);
            }
        }
        SYSTEM_HAS_POWERED_ON => {
            let _ = context.sender.send(HostEvent::Woke.into());
        }
        _ => {}
    }
    if matches!(message, CAN_SYSTEM_SLEEP | SYSTEM_WILL_SLEEP) {
        // SAFETY: The IOKit contract passes an intptr_t acknowledgement token
        // in the callback pointer for these two messages. It is never dereferenced.
        let status = unsafe { IOAllowPowerChange(context.connection, token as isize) };
        if status != 0 {
            tracing::warn!(status, "IOAllowPowerChange failed");
        }
    }
}

struct NetworkWatcher {
    monitor: NonNull<c_void>,
    _queue: DispatchRetained<DispatchQueue>,
}

impl NetworkWatcher {
    fn start(sender: Sender) -> io::Result<Self> {
        let queue = DispatchQueue::new("org.marsa.host-network", None);
        let initial_update = AtomicBool::new(true);
        let pending_path = Arc::new(Semaphore::new(1));
        let handler = RcBlock::new(move |_: *mut c_void| {
            if !initial_update.swap(false, Ordering::Relaxed)
                && let Ok(pending) = pending_path.clone().try_acquire_owned()
            {
                let mut notification = HostNotification::from(HostEvent::NetworkChanged);
                notification._pending_path = Some(pending);
                let _ = sender.send(notification);
            }
        });
        // SAFETY: Create returns an owned monitor. The monitor copies the
        // callback block and calls it on the serial dispatch queue.
        let monitor = unsafe {
            let monitor = NonNull::new(nw_path_monitor_create())
                .ok_or_else(|| io::Error::other("nw_path_monitor_create failed"))?;
            nw_path_monitor_set_queue(monitor.as_ptr(), &*queue);
            nw_path_monitor_set_update_handler(monitor.as_ptr(), RcBlock::as_ptr(&handler).cast());
            nw_path_monitor_start(monitor.as_ptr());
            monitor
        };
        Ok(Self {
            monitor,
            _queue: queue,
        })
    }
}

impl Drop for NetworkWatcher {
    fn drop(&mut self) {
        // SAFETY: We own the monitor. Its copied block keeps its captures alive
        // for the lifetime of any callback already scheduled by the framework.
        unsafe {
            nw_path_monitor_cancel(self.monitor.as_ptr());
            nw_release(self.monitor.as_ptr());
        }
    }
}

type PowerCallback = unsafe extern "C" fn(*mut c_void, u32, u32, *mut c_void);

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IORegisterForSystemPower(
        context: *mut c_void,
        port: *mut *mut c_void,
        callback: PowerCallback,
        notifier: *mut u32,
    ) -> u32;
    fn IONotificationPortSetDispatchQueue(port: *mut c_void, queue: *const DispatchQueue);
    fn IOAllowPowerChange(connection: u32, token: isize) -> i32;
    fn IODeregisterForSystemPower(notifier: *mut u32) -> i32;
    fn IOServiceClose(connection: u32) -> i32;
    fn IONotificationPortDestroy(port: *mut c_void);
}

#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    fn nw_path_monitor_create() -> *mut c_void;
    fn nw_path_monitor_set_queue(monitor: *mut c_void, queue: *const DispatchQueue);
    fn nw_path_monitor_set_update_handler(monitor: *mut c_void, handler: *const c_void);
    fn nw_path_monitor_start(monitor: *mut c_void);
    fn nw_path_monitor_cancel(monitor: *mut c_void);
    fn nw_release(object: *mut c_void);
}
