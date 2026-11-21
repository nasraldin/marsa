//! Observe child exits and forward guest shutdown signals in dependency order.

#![allow(unsafe_code)]

use std::{
    io,
    os::unix::process::CommandExt,
    process::Command,
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::{
        signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, kill, sigaction},
        signalfd::{SfdFlags, SignalFd},
        wait::{WaitPidFlag, WaitStatus, waitpid},
    },
    unistd::Pid,
};
use std::os::fd::AsFd as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Service {
    Agent,
    Containerd,
    Engine,
}

struct Child {
    service: Service,
    pid: Pid,
}

pub(super) struct Supervisor {
    events: SignalFd,
    processes: Vec<Child>,
    shutdown: bool,
}

extern "C" fn shutdown_handler(_: nix::libc::c_int) {}

impl Supervisor {
    pub(super) fn new() -> io::Result<Self> {
        let mut watched = SigSet::empty();
        for signal in [Signal::SIGTERM, Signal::SIGINT, Signal::SIGPWR] {
            // SAFETY: The handler does no work and its C ABI matches sigaction.
            // Register before blocking so Linux can deliver these signals to PID 1.
            unsafe {
                sigaction(
                    signal,
                    &SigAction::new(
                        SigHandler::Handler(shutdown_handler),
                        SaFlags::empty(),
                        SigSet::empty(),
                    ),
                )?;
            }
            watched.add(signal);
        }
        watched.add(Signal::SIGCHLD);
        watched.thread_block()?;
        let events =
            SignalFd::with_flags(&watched, SfdFlags::SFD_CLOEXEC | SfdFlags::SFD_NONBLOCK)?;
        Ok(Self {
            events,
            processes: Vec::new(),
            shutdown: false,
        })
    }

    pub(super) fn start(&mut self, service: Service, path: &str, args: &[&str]) -> io::Result<()> {
        let mut command = Command::new(path);
        command
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
            .args(args);
        // SAFETY: This hook only changes the signal mask through sigprocmask
        // and converts a returned errno. It does not allocate or lock.
        unsafe {
            command.pre_exec(|| SigSet::empty().thread_set_mask().map_err(Into::into));
        }
        let child = command.spawn()?;
        let pid = Pid::from_raw(i32::try_from(child.id()).map_err(io::Error::other)?);
        self.processes.push(Child { service, pid });
        Ok(())
    }

    pub(super) fn wait_until(
        &mut self,
        timeout: Duration,
        ready: impl Fn() -> bool,
    ) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            self.check_running(50u16.into())?;
            if self.shutdown {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "shutdown during guest boot",
                ));
            }
            if ready() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "service readiness timed out",
                ));
            }
        }
    }

    pub(super) fn serve(&mut self) -> io::Result<()> {
        while !self.shutdown {
            self.check_running(PollTimeout::NONE)?;
        }
        Ok(())
    }

    pub(super) fn stop_all(&mut self) -> io::Result<()> {
        let mut first_error = None;
        for service in [Service::Engine, Service::Containerd, Service::Agent] {
            if let Err(error) = self.stop(service) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn check_running(&mut self, timeout: PollTimeout) -> io::Result<()> {
        let exits = self.poll_events(timeout)?;
        if !self.shutdown
            && let Some((service, status)) = exits.first()
        {
            return Err(io::Error::other(format!("{service:?} exited: {status:?}")));
        }
        Ok(())
    }

    fn stop(&mut self, service: Service) -> io::Result<()> {
        let Some(pid) = self
            .processes
            .iter()
            .find(|child| child.service == service)
            .map(|child| child.pid)
        else {
            return Ok(());
        };
        for (signal, timeout) in [(Signal::SIGTERM, 35), (Signal::SIGKILL, 5)] {
            if let Err(error) = kill(pid, signal)
                && error != Errno::ESRCH
            {
                return Err(error.into());
            }
            let deadline = Instant::now() + Duration::from_secs(timeout);
            while Instant::now() < deadline {
                self.poll_events(100u16.into())?;
                if !self.processes.iter().any(|child| child.pid == pid) {
                    return Ok(());
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{service:?} did not stop"),
        ))
    }

    fn poll_events(&mut self, timeout: PollTimeout) -> io::Result<Vec<(Service, WaitStatus)>> {
        poll(
            &mut [PollFd::new(self.events.as_fd(), PollFlags::POLLIN)],
            timeout,
        )?;
        while let Some(event) = self.events.read_signal()? {
            if matches!(
                Signal::try_from(event.ssi_signo as i32),
                Ok(Signal::SIGTERM | Signal::SIGINT | Signal::SIGPWR)
            ) {
                self.shutdown = true;
            }
        }
        let mut exits = Vec::new();
        loop {
            match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
                Ok(status @ (WaitStatus::Exited(pid, _) | WaitStatus::Signaled(pid, _, _))) => {
                    if let Some(index) = self.processes.iter().position(|child| child.pid == pid) {
                        exits.push((self.processes.swap_remove(index).service, status));
                    }
                }
                Ok(WaitStatus::StillAlive) | Err(Errno::ECHILD) => break,
                Ok(_) | Err(Errno::EINTR) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(exits)
    }
}
