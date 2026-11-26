//! Boot and orderly power-off for the Linux VM.

mod console;
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

    if let Err(error) = console::replay_early_kernel_log() {
        eprintln!("marsa-init: cannot replay early kernel boot log: {error}");
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
        // Native DNS must be serving before the host asks the agent to probe egress.
        // Offline and user-space guests have no resolver configuration.
        let resolver = Path::new("/run/marsa/native-dns.conf");
        supervisor.wait_until(Duration::from_secs(60), || {
            resolver.is_file() || Path::new(AGENT_READY).is_file()
        })?;
        if resolver.try_exists()? {
            supervisor.start(
                Service::Resolver,
                "/usr/sbin/dnsmasq",
                &[
                    "--keep-in-foreground",
                    "--conf-file=/run/marsa/native-dns.conf",
                ],
            )?;
        }
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
    configure_links(settings.interface.as_deref(), checked_tool)
}

fn configure_links(
    interface: Option<&str>,
    mut run: impl FnMut(&str, &[&str]) -> io::Result<()>,
) -> io::Result<()> {
    run("/sbin/ip", &["link", "set", "dev", "lo", "up"])?;
    if let Some(interface) = interface {
        let upstream = run("/sbin/ip", &["link", "set", "dev", interface, "up"]).and_then(|()| {
            run(
                "/sbin/udhcpc",
                &["-q", "-n", "-t", "5", "-T", "2", "-i", interface],
            )
        });
        if let Err(error) = upstream {
            eprintln!("marsa-init: upstream interface {interface} is offline: {error}");
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_errors_keep_the_guest_available() {
        for failing_call in [2, 3] {
            let mut calls = 0;
            let result = configure_links(Some("eth0"), |_, _| {
                calls += 1;
                if calls == failing_call {
                    Err(io::Error::other("no upstream network"))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(calls, failing_call);
        }
    }

    #[test]
    fn loopback_failure_stops_network_setup() {
        let mut calls = 0;
        let result = configure_links(Some("eth0"), |_, _| {
            calls += 1;
            Err(io::Error::other("cannot bring up loopback"))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn interface_setting_controls_dhcp() {
        for interface in [Some("eth0"), None] {
            let mut calls = Vec::new();
            configure_links(interface, |path, arguments| {
                calls.push((path.to_owned(), arguments.join(" ")));
                Ok(())
            })
            .unwrap();
            assert_eq!(calls[0].1, "link set dev lo up");
            if interface.is_some() {
                assert_eq!(calls[1].1, "link set dev eth0 up");
                assert_eq!(calls[2].0, "/sbin/udhcpc");
                assert_eq!(calls[2].1, "-q -n -t 5 -T 2 -i eth0");
                assert_eq!(calls.len(), 3);
            } else {
                assert_eq!(calls.len(), 1);
            }
        }
    }
}
