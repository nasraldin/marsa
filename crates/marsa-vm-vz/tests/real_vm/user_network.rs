//! Verify private user-space egress with the production VZ backend and a real guest.

use std::{
    env, fs, io,
    net::{IpAddr, SocketAddr, ToSocketAddrs as _},
    path::{Path, PathBuf},
    time::Duration,
};

use marsa_proto::guest::DOCKER_API_PORT;
use marsa_vm::{Hypervisor, NetworkSpec, Vm};
use marsa_vm_vz::VzHypervisor;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    time::timeout,
};

use super::{LIMIT, connect_control, handshake, spec};

fn scoped_query() -> Option<(String, IpAddr)> {
    let name = env::var("MARSA_VZ_SPLIT_DNS_NAME").ok()?;
    assert!(
        !name.is_empty()
            && name.len() <= 253
            && name.split('.').all(|part| {
                !part.is_empty()
                    && part.len() <= 63
                    && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            }),
        "expected a DNS hostname"
    );
    let host = (name.as_str(), 443)
        .to_socket_addrs()
        .unwrap()
        .map(|address: SocketAddr| address.ip())
        .find(IpAddr::is_ipv4)
        .expect("the scoped host must return an IPv4 result through the macOS resolver");
    Some((name, host))
}

async fn docker(vm: &dyn Vm, request: &[u8]) -> io::Result<Vec<u8>> {
    let mut connection = vm
        .connect_vsock(DOCKER_API_PORT)
        .await
        .map_err(io::Error::other)?;
    connection.write_all(request).await?;
    let mut reply = Vec::new();
    connection.take(256 * 1024).read_to_end(&mut reply).await?;
    Ok(reply)
}

#[tokio::test]
#[ignore = "requires virtualization entitlement, packaged guest and the Marsa network helper"]
async fn packaged_userspace_network_reaches_registry_with_verified_tls() {
    let helper =
        PathBuf::from(env::var_os("MARSA_VZ_NETWORK_HELPER").expect("set MARSA_VZ_NETWORK_HELPER"));
    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let query = scoped_query();
    let command = query.as_ref().map_or_else(
        || {
            "console=hvc0 root=/dev/vda rootfstype=erofs ro init=/usr/bin/marsa-init panic=-1"
                .to_owned()
        },
        |(name, _)| {
            format!(
                concat!(
                    "console=hvc0 root=/dev/vda rootfstype=erofs ro panic=-1 init=/bin/sh -- -c \"",
                    "exec </dev/hvc0 >/dev/hvc0 2>&1; (",
                    "while [ ! -S /var/run/docker.sock ]; do /bin/busybox sleep 1; done; ",
                    "/bin/busybox nslookup '{0}.'; echo MARSA_SCOPED_DNS_END",
                    ") & exec /usr/bin/marsa-init\""
                ),
                name
            )
        },
    );
    let mut settings = spec(&guest, scratch.path(), &command);
    let console = settings.console_log.clone();
    settings.network = NetworkSpec::Userspace;
    let backend = VzHypervisor::with_userspace(helper);
    assert!(backend.capabilities().userspace_network);
    let mut vm = backend.create(settings).await.unwrap();
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    let mut control = connect_control(vm.as_ref(), &console).await;
    timeout(LIMIT, handshake(&mut control)).await.unwrap();

    // A fresh disk can only inspect this image once the upstream registry pull succeeds.
    // Dockerd uses its normal registry TLS verification and guest clock was set by handshake.
    let pull = timeout(Duration::from_secs(60), docker(vm.as_ref(),
        b"POST /v1.44/images/create?fromImage=docker.io%2Flibrary%2Fhello-world&tag=latest HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )).await
    .unwrap()
    .unwrap();
    assert!(
        pull.starts_with(b"HTTP/1.1 200"),
        "Docker pull: {}",
        String::from_utf8_lossy(&pull)
    );
    let inspect = timeout(LIMIT, docker(vm.as_ref(),
        b"GET /v1.44/images/docker.io/library/hello-world:latest/json HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )).await
    .unwrap()
    .unwrap();
    assert!(
        inspect.starts_with(b"HTTP/1.1 200"),
        "Docker pull: {}",
        String::from_utf8_lossy(&pull)
    );

    let log = fs::read_to_string(console).unwrap();
    assert!(
        log.lines()
            .any(|line| line.starts_with("udhcpc: lease of "))
    );
    if let Some((name, expected)) = query {
        assert!(
            log.lines().any(|line| line == "MARSA_SCOPED_DNS_END")
                && log.lines().any(|line| line.contains(&expected.to_string())),
            "guest resolver did not return the same scoped answer as macOS for {name}"
        );
    }
    timeout(LIMIT, vm.stop(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
}
