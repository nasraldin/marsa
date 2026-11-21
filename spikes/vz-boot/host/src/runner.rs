//! Own the test inputs, serial transcript, timeouts, and challenge outside the VM.

use std::fs::{File, OpenOptions, symlink_metadata};
use std::io::{self, Read, Write};
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use objc2::rc::autoreleasepool;
use objc2_foundation::{NSDate, NSRunLoop};

mod console;
#[cfg(feature = "guest-control")]
mod guest_control;

use console::SerialCapture;

const DEADLINE: Duration = Duration::from_secs(90);
const MAX_SCRIPT_BYTES: u64 = 16 * 1024;
const I_O_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_RETRY: Duration = Duration::from_millis(100);
pub const VSOCK_PORT: u32 = 5_000;

pub enum Backend {
    Rust,
    #[cfg(feature = "swift")]
    Swift,
}

pub struct VmInputs<'a> {
    pub fixtures: &'a Path,
    pub serial: BorrowedFd<'a>,
    pub writable_share: bool,
    pub read_only_root: bool,
    pub via_init: bool,
    pub boot_test: bool,
}

pub enum Event {
    Started,
    Connected(OwnedFd),
    ConnectFailed(String),
    RoundTrip(Option<ControlTimings>),
    Stopped,
    Error(String),
}

pub struct ControlTimings {
    hello: Instant,
    engine_ready: Instant,
}

pub trait Vm {
    fn connect(&self) -> io::Result<()>;
}

#[derive(Default)]
struct Progress {
    read_only_root: bool,
    boot_test: bool,
    kernel_ms: Option<u64>,
    pid1_started: Option<Instant>,
    dockerd_started: Option<Instant>,
    control_timings: Option<ControlTimings>,
    started: bool,
    devices: bool,
    listening: bool,
    shell: bool,
    connect_requested: bool,
    round_trip: bool,
    commands_sent: bool,
    commands_finished: bool,
    shutdown_sent: bool,
}

pub struct BootOptions {
    pub writable_share: bool,
    pub read_only_root: bool,
    pub via_init: bool,
    pub guest_control: bool,
    pub boot_test: bool,
}

pub fn run(
    backend: Backend,
    fixtures: &Path,
    console: &Path,
    script: Option<&Path>,
    options: BootOptions,
) -> io::Result<()> {
    let BootOptions {
        writable_share,
        read_only_root,
        via_init,
        guest_control,
        boot_test,
    } = options;
    validate_fixtures(fixtures, read_only_root, boot_test)?;
    let script = read_script(script, guest_control && !boot_test)?;
    let (mut stream, guest_serial) = UnixStream::pair()?;
    stream.set_read_timeout(Some(Duration::from_millis(10)))?;
    stream.set_write_timeout(Some(I_O_TIMEOUT))?;
    let mut transcript = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(console)?;
    let (sender, receiver) = mpsc::channel();
    use std::os::fd::AsFd;
    let inputs = VmInputs {
        fixtures,
        serial: guest_serial.as_fd(),
        writable_share,
        read_only_root,
        via_init,
        boot_test,
    };
    let started_at = Instant::now();
    let vm = match backend {
        Backend::Rust => super::native::start(&inputs, sender.clone())?,
        #[cfg(feature = "swift")]
        Backend::Swift => super::swift::start(&inputs, sender.clone())?,
    };
    drop(guest_serial);

    let deadline = Instant::now() + DEADLINE;
    let mut progress = Progress {
        read_only_root,
        boot_test,
        ..Progress::default()
    };
    let mut connect_at = Instant::now();
    let mut connect_error = None;
    let mut capture = SerialCapture::default();
    let run_loop = NSRunLoop::mainRunLoop();
    while Instant::now() < deadline {
        autoreleasepool(|_| {
            run_loop.runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
        });
        capture.read(&mut stream, &mut transcript, &mut progress)?;

        for event in receiver.try_iter() {
            match event {
                Event::Started => {
                    progress.started = true;
                    println!("received VM start completion");
                }
                Event::Connected(fd) => {
                    connect_error = None;
                    let sender = sender.clone();
                    thread::spawn(move || exchange(fd, sender, guest_control, boot_test));
                }
                Event::ConnectFailed(detail) if guest_control => {
                    connect_error = Some(detail);
                    connect_at = Instant::now() + CONNECT_RETRY;
                    progress.connect_requested = false;
                }
                Event::ConnectFailed(detail) => {
                    return Err(io::Error::other(detail));
                }
                Event::RoundTrip(timings) => {
                    progress.control_timings = timings;
                    progress.round_trip = true;
                    if guest_control {
                        println!(
                            "verified guest control handshake, time result, and engine readiness"
                        );
                    } else {
                        println!("verified 16-byte serial-independent vsock challenge");
                    }
                }
                Event::Stopped => {
                    if !progress.shutdown_sent {
                        return Err(io::Error::other(
                            "guest stopped before completing the boot checks",
                        ));
                    }
                    transcript.flush()?;
                    if boot_test {
                        report_timings(&progress, started_at)?;
                    }
                    println!("received guest shutdown delegate callback");
                    return Ok(());
                }
                Event::Error(detail) => return Err(io::Error::other(detail)),
            }
        }
        if progress.started
            && progress.listening
            && !progress.connect_requested
            && Instant::now() >= connect_at
        {
            vm.connect()?;
            progress.connect_requested = true;
        }
        if progress.devices && progress.shell && progress.round_trip && !progress.commands_sent {
            stream.write_all(&script)?;
            progress.commands_sent = true;
        }
        if progress.commands_finished && !progress.shutdown_sent {
            // EROFS fixture mode powers off directly after checking the root mount.
            if via_init {
                stream.write_all(b"busybox kill -TERM 1\n")?;
            } else if !read_only_root {
                stream.write_all(b"busybox sync\nbusybox poweroff -f\n")?;
            }
            progress.shutdown_sent = true;
        }
    }
    if !progress.round_trip
        && let Some(detail) = connect_error
    {
        return Err(io::Error::other(format!(
            "guest control connect timed out: {detail}"
        )));
    }
    Err(io::Error::other(
        "VM test timed out; inspect the console transcript",
    ))
}

fn report_timings(progress: &Progress, start: Instant) -> io::Result<()> {
    let incomplete = || io::Error::other("missing kernel, PID 1, dockerd or control timing");
    let kernel_ms = progress.kernel_ms.ok_or_else(incomplete)?;
    let pid1 = progress.pid1_started.ok_or_else(incomplete)?;
    let dockerd = progress.dockerd_started.ok_or_else(incomplete)?;
    let control = progress.control_timings.as_ref().ok_or_else(incomplete)?;
    let since = |later: Instant, earlier: Instant| {
        later
            .checked_duration_since(earlier)
            .ok_or_else(|| io::Error::other("boot markers arrived out of order"))
    };
    println!(
        "MARSA_BOOT_METRICS kernel_ms={kernel_ms} init_to_hello_ms={} dockerd_to_ready_ms={} start_to_ready_ms={}",
        since(control.hello, pid1)?.as_millis(),
        since(control.engine_ready, dockerd)?.as_millis(),
        since(control.engine_ready, start)?.as_millis(),
    );
    Ok(())
}

fn read_script(path: Option<&Path>, guest_control: bool) -> io::Result<Vec<u8>> {
    let prefix = b"(\nset -e\n";
    let mut commands = prefix.to_vec();
    #[cfg(feature = "guest-control")]
    if guest_control {
        commands.extend(guest_control::clock_assertions(DEADLINE));
    }
    #[cfg(not(feature = "guest-control"))]
    let _ = guest_control;
    let start = commands.len();
    if let Some(path) = path {
        File::open(path)?
            .take(MAX_SCRIPT_BYTES + 1)
            .read_to_end(&mut commands)?;
        let contents = &commands[start..];
        if contents.len() as u64 > MAX_SCRIPT_BYTES {
            return Err(io::Error::other("guest script exceeds 16 KiB"));
        }
        if contents
            .iter()
            .any(|byte| byte.is_ascii_control() && !matches!(byte, b'\t' | b'\n'))
            || contents
                .split(|byte| *byte == b'\n')
                .any(|line| line.len() > 2048)
        {
            return Err(io::Error::other(
                "guest script contains a control byte or a line over 2048 bytes",
            ));
        }
    } else {
        commands.extend_from_slice(b":");
    }
    commands.extend_from_slice(b"\n)\nprintf '\\nMARSA_COMMANDS_%s\\n' \"$?\"\n");
    Ok(commands)
}

fn exchange(fd: OwnedFd, sender: Sender<Event>, guest_control: bool, boot_test: bool) {
    let result = (|| -> io::Result<Option<ControlTimings>> {
        let mut stream = UnixStream::from(fd);
        stream.set_read_timeout(Some(I_O_TIMEOUT))?;
        stream.set_write_timeout(Some(I_O_TIMEOUT))?;
        #[cfg(feature = "guest-control")]
        if guest_control {
            return guest_control::verify(&mut stream, boot_test).map(Some);
        }
        #[cfg(not(feature = "guest-control"))]
        let _ = (guest_control, boot_test);
        let mut challenge = [0; 16];
        File::open("/dev/urandom")?.read_exact(&mut challenge)?;
        stream.write_all(&challenge)?;
        let mut response = [0; 16];
        stream.read_exact(&mut response)?;
        if challenge != response {
            return Err(io::Error::other(
                "guest returned an incorrect vsock challenge",
            ));
        }
        Ok(None)
    })();
    let event = match result {
        Ok(timings) => Event::RoundTrip(timings),
        Err(error) => Event::Error(format!("vsock exchange: {error}")),
    };
    let _ = sender.send(event);
}

fn validate_fixtures(root: &Path, read_only_root: bool, boot_test: bool) -> io::Result<()> {
    let directory = symlink_metadata(root)?;
    if !directory.is_dir() || directory.mode() & 0o077 != 0 {
        return Err(io::Error::other("fixture root must be a private directory"));
    }
    let files = if read_only_root {
        &["Image", "rootfs.img"][..]
    } else if boot_test {
        &["Image", "rootfs.img", "data.img"][..]
    } else {
        &["Image", "initramfs", "modules.img", "data.img"][..]
    };
    for &name in files {
        let metadata = symlink_metadata(root.join(name))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(io::Error::other(format!(
                "{name} must be a nonempty fixture file"
            )));
        }
        if name == "data.img" && (metadata.nlink() != 1 || metadata.len() % 512 != 0) {
            return Err(io::Error::other(
                "data.img must be a private sector-aligned disk file",
            ));
        }
    }
    if !read_only_root && !symlink_metadata(root.join("share"))?.is_dir() {
        return Err(io::Error::other("share must be a fixture directory"));
    }
    Ok(())
}
