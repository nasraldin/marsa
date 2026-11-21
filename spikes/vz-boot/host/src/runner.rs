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

use console::SerialCapture;

const DEADLINE: Duration = Duration::from_secs(90);
const MAX_SCRIPT_BYTES: u64 = 16 * 1024;
const I_O_TIMEOUT: Duration = Duration::from_secs(5);
pub const VSOCK_PORT: u32 = 5_000;

pub enum Backend {
    Rust,
    #[cfg(feature = "swift")]
    Swift,
}

pub struct VmInputs<'a> {
    pub fixtures: &'a Path,
    pub serial: BorrowedFd<'a>,
}

pub enum Event {
    Started,
    Connected(OwnedFd),
    RoundTrip,
    Stopped,
    Error(String),
}

pub trait Vm {
    fn connect(&self) -> io::Result<()>;
}

#[derive(Default)]
struct Progress {
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

pub fn run(
    backend: Backend,
    fixtures: &Path,
    console: &Path,
    script: Option<&Path>,
) -> io::Result<()> {
    validate_fixtures(fixtures)?;
    let script = read_script(script)?;
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
    };
    let vm = match backend {
        Backend::Rust => super::native::start(&inputs, sender.clone())?,
        #[cfg(feature = "swift")]
        Backend::Swift => super::swift::start(&inputs, sender.clone())?,
    };
    drop(guest_serial);

    let deadline = Instant::now() + DEADLINE;
    let mut progress = Progress::default();
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
                    let sender = sender.clone();
                    thread::spawn(move || exchange(fd, sender));
                }
                Event::RoundTrip => {
                    progress.round_trip = true;
                    println!("verified 16-byte serial-independent vsock challenge");
                }
                Event::Stopped => {
                    if !progress.shutdown_sent {
                        return Err(io::Error::other(
                            "guest stopped before the runner requested shutdown",
                        ));
                    }
                    transcript.flush()?;
                    println!("received guest shutdown delegate callback");
                    return Ok(());
                }
                Event::Error(detail) => return Err(io::Error::other(detail)),
            }
        }
        if progress.started && progress.listening && !progress.connect_requested {
            vm.connect()?;
            progress.connect_requested = true;
        }
        if progress.devices && progress.shell && progress.round_trip && !progress.commands_sent {
            stream.write_all(&script)?;
            progress.commands_sent = true;
        }
        if progress.commands_finished && !progress.shutdown_sent {
            // Leave the interactive shell running until its serial output has been verified.
            stream.write_all(b"busybox sync\nbusybox poweroff -f\n")?;
            progress.shutdown_sent = true;
        }
    }
    Err(io::Error::other(
        "VM test timed out; inspect the console transcript",
    ))
}

fn read_script(path: Option<&Path>) -> io::Result<Vec<u8>> {
    let prefix = b"(\nset -e\n";
    let mut commands = prefix.to_vec();
    if let Some(path) = path {
        File::open(path)?
            .take(MAX_SCRIPT_BYTES + 1)
            .read_to_end(&mut commands)?;
        let contents = &commands[prefix.len()..];
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

fn exchange(fd: OwnedFd, sender: Sender<Event>) {
    let result = (|| -> io::Result<()> {
        let mut stream = UnixStream::from(fd);
        stream.set_read_timeout(Some(I_O_TIMEOUT))?;
        stream.set_write_timeout(Some(I_O_TIMEOUT))?;
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
        Ok(())
    })();
    let event = match result {
        Ok(()) => Event::RoundTrip,
        Err(error) => Event::Error(format!("vsock exchange: {error}")),
    };
    let _ = sender.send(event);
}

fn validate_fixtures(root: &Path) -> io::Result<()> {
    let directory = symlink_metadata(root)?;
    if !directory.is_dir() || directory.mode() & 0o077 != 0 {
        return Err(io::Error::other("fixture root must be a private directory"));
    }
    for name in ["Image", "initramfs", "modules.img", "data.img"] {
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
    if !symlink_metadata(root.join("share"))?.is_dir() {
        return Err(io::Error::other("share must be a fixture directory"));
    }
    Ok(())
}
