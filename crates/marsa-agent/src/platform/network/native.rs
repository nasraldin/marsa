//! Supply the native gateway aliases to Docker's bridge and embedded resolvers.
//! Keep a stable DNS listener while the host replaces its upstream routes.

use std::{fs, io};

use marsa_proto::guest::v1::ConfigureNetwork;
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};

use marsa_core::guest_boot::NATIVE_NAT_ARG;
use serde_json::{Value, json};

use super::{DAEMON, HOSTS, MAX_CONFIG_BYTES, atomic_file, read_bounded};
use crate::{
    native_dns::servers,
    platform::{network_facts, operations::stats},
};

pub(super) const SERVERS: &str = "/run/marsa/native-dns.servers";
const PID_FILE: &str = "/run/marsa/native-dns.pid";

pub(in crate::platform) fn configure() -> io::Result<()> {
    let cmdline = stats::read_bounded("/proc/cmdline", 4096)?;
    if !cmdline
        .split_ascii_whitespace()
        .any(|arg| arg == NATIVE_NAT_ARG)
    {
        return Ok(());
    }
    let Some(interface) = network_facts::interface()? else {
        return Ok(());
    };
    let Some(facts) = network_facts::facts(&interface)? else {
        return Ok(());
    };
    let Some(gateway) = facts.gateway else {
        return Ok(());
    };
    // Bind explicitly so both TCP and UDP accept packets sent to the stable
    // guest address by Docker bridges. Drop external ingress before publication.
    for protocol in ["tcp", "udp"] {
        let status = std::process::Command::new("/usr/sbin/iptables")
            .args([
                "-w", "5", "-I", "INPUT", "1", "-i", &interface, "-p", protocol, "--dport", "53",
                "-j", "DROP",
            ])
            .status()?;
        if !status.success() {
            return Err(io::Error::other("could not isolate native DNS listener"));
        }
    }
    let names = "host.docker.internal host.marsa.internal gateway.docker.internal";
    let aliases = format!("{gateway} {names}\n");
    let alias_file = "/run/marsa/native-hosts";
    fs::write(alias_file, &aliases)?;
    let mut hosts = read_bounded(HOSTS, MAX_CONFIG_BYTES)?;
    hosts.push(b'\n');
    hosts.extend_from_slice(aliases.as_bytes());
    atomic_file(HOSTS, &hosts, 0o644)?;
    let mut config: serde_json::Map<String, Value> =
        serde_json::from_slice(&read_bounded(DAEMON, MAX_CONFIG_BYTES)?)?;
    config.insert("dns".into(), json!([facts.address.to_string()]));
    config.insert("host-gateway-ips".into(), json!([gateway.to_string()]));
    atomic_file(DAEMON, &serde_json::to_vec(&config)?, 0o600)?;
    let upstream = stats::read_bounded("/etc/resolv.conf", MAX_CONFIG_BYTES)?;
    atomic_file(
        SERVERS,
        servers(&upstream, &ConfigureNetwork::default())?.as_bytes(),
        0o644,
    )?;
    // Publish the config last: its existence releases the resolver in PID 1.
    let resolver = format!(
        "no-hosts\nlog-facility=-\nno-resolv\nservers-file={SERVERS}\nlocal=/host.docker.internal/host.marsa.internal/gateway.docker.internal/\naddn-hosts={alias_file}\nbind-interfaces\nlisten-address=127.0.0.1,{address}\nuser=nobody\ngroup=nogroup\ncache-size=0\npid-file={PID_FILE}\n",
        address = facts.address,
    );
    atomic_file("/run/marsa/native-dns.conf", resolver.as_bytes(), 0o600)
}

pub(super) fn reload() -> io::Result<()> {
    let pid = match stats::read_bounded(PID_FILE, 32) {
        Ok(pid) => pid,
        // A newly started resolver reads the committed servers file at startup.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let pid = pid.trim().parse::<i32>().map_err(io::Error::other)?;
    if pid <= 1 {
        return Err(super::invalid("invalid native resolver pid"));
    }
    kill(Pid::from_raw(pid), Signal::SIGHUP)?;
    Ok(())
}
