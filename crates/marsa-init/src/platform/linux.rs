//! Boot and orderly power-off for the Linux VM.

mod disk;
mod mounts;
mod supervisor;

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

use nix::{
    sys::reboot::{RebootMode, reboot},
    unistd::{getpid, sethostname, sync},
};

use marsa_core::guest_boot::Settings;
use mounts::Mounts;
use supervisor::{Service, Supervisor};

const RUN_DIR: &str = "/run/marsa";
const AGENT_READY: &str = "/run/marsa/agent.ready";
const BOOT_TIMINGS: &str = "/run/marsa/boot-timings";
const CONTAINERD_SOCKET: &str = "/run/containerd/containerd.sock";

pub(crate) fn run() -> io::Result<()> {
    if getpid().as_raw() != 1 {
        return Err(io::Error::other("marsa-init must run as guest PID 1"));
    }

    let mut mounts = Mounts::default();
    let mut supervisor = Supervisor::new()?;
    let result = boot(&mut mounts, &mut supervisor).and_then(|()| supervisor.serve());
    if let Err(error) = &result {
        eprintln!("marsa-init: {error}");
    }
    if let Err(error) = supervisor.stop_all() {
        eprintln!("marsa-init: stopping services: {error}");
    }
    if let Err(error) = mounts.unmount_shares() {
        eprintln!("marsa-init: unmounting shares: {error}");
    }
    if let Err(error) = disk::unmount() {
        eprintln!("marsa-init: unmounting data disk: {error}");
    }
    sync();
    reboot(RebootMode::RB_POWER_OFF).map(|never| match never {})?;
    result
}

fn boot(mounts: &mut Mounts, supervisor: &mut Supervisor) -> io::Result<()> {
    let start = Instant::now();
    mounts.system()?;
    fs::create_dir(RUN_DIR)?;
    let mut timings = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(BOOT_TIMINGS)?;
    record(&mut timings, "mount_system", start.elapsed())?;

    let settings = Settings::parse(&fs::read_to_string("/proc/cmdline")?)?;
    stage(&mut timings, "data_disk", disk::prepare)?;
    stage(&mut timings, "shared_folders", || {
        mounts.share_all(&settings.shares)
    })?;
    stage(&mut timings, "network", || configure_network(&settings))?;
    stage(&mut timings, "agent", || {
        supervisor.start(Service::Agent, "/usr/bin/marsa-agent", &[])?;
        supervisor.wait_until(Duration::from_secs(60), || Path::new(AGENT_READY).is_file())
    })?;
    stage(&mut timings, "containerd", || {
        supervisor.start(Service::Containerd, "/usr/local/bin/containerd", &[])?;
        supervisor.wait_until(Duration::from_secs(15), || {
            std::os::unix::net::UnixStream::connect(CONTAINERD_SOCKET).is_ok()
        })
    })?;
    stage(&mut timings, "dockerd", || {
        supervisor.start(
            Service::Engine,
            "/usr/local/bin/dockerd",
            &["--config-file", "/etc/docker/daemon.json"],
        )
    })
}

fn configure_network(settings: &Settings) -> io::Result<()> {
    sethostname(&settings.hostname)?;
    checked_tool("/sbin/ip", &["link", "set", "dev", "lo", "up"])?;
    if let Some(interface) = &settings.interface {
        checked_tool("/sbin/ip", &["link", "set", "dev", interface, "up"])?;
        checked_tool(
            "/sbin/udhcpc",
            &["-q", "-n", "-t", "5", "-T", "2", "-i", interface],
        )?;
    }
    Ok(())
}

fn checked_tool(path: &str, args: &[&str]) -> io::Result<()> {
    let status = Command::new(path)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .args(args)
        .status()?;
    if status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!("{path} returned {status}")))
}

fn stage(
    output: &mut impl Write,
    name: &str,
    action: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let start = Instant::now();
    action().map_err(|error| io::Error::new(error.kind(), format!("{name}: {error}")))?;
    record(output, name, start.elapsed())
}

fn record(output: &mut impl Write, name: &str, elapsed: Duration) -> io::Result<()> {
    let millis = elapsed.as_millis();
    eprintln!("marsa-init stage={name} duration_ms={millis}");
    writeln!(output, "{name}={millis}")
}
