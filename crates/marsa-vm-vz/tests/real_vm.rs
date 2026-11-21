//! Entitled system tests using the actual packaged ARM64 guest on Apple silicon.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::{
    env,
    fs::{self, File},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use marsa_proto::guest::{
    CONTROL_PORT, PROTOCOL_VERSION, decode_guest, encode_host, frame_length, negotiate,
    v1::{
        CommandStatus, EngineHealthState, GuestEnvelope, HelloAck, HostEnvelope, SetTime,
        guest_envelope::Body as GuestBody, host_envelope::Body as HostBody,
    },
};
use marsa_vm::{
    DiskSpec, Hypervisor, NetworkSpec, Vm, VmError, VmEvent, VmEventStream, VmSpec, VmState,
    VsockStream,
};
use marsa_vm_vz::VzHypervisor;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::{sleep, timeout},
};
use tokio_stream::StreamExt;

const LIMIT: Duration = Duration::from_secs(15);
const BOOT: &str = "console=hvc0 root=/dev/vda rootfstype=erofs ro init=/usr/bin/marsa-init marsa.interface=none panic=-1";
const GUEST_POWEROFF: &str = concat!(
    "console=hvc0 root=/dev/vda rootfstype=erofs ro panic=-1 init=/bin/sh -- -c \"",
    "/bin/busybox sleep 1; /bin/busybox poweroff -f\"",
);

fn spec(guest: &Path, scratch: &Path, cmdline: &str) -> VmSpec {
    let data = scratch.join("data.img");
    File::create(&data)
        .unwrap()
        .set_len(512 * 1024 * 1024)
        .unwrap();
    VmSpec {
        cpus: 2,
        memory_bytes: 1024 * 1024 * 1024,
        kernel: guest.join("Image"),
        initramfs: None,
        cmdline: cmdline.into(),
        root_disk: DiskSpec {
            path: guest.join("rootfs.img"),
            read_only: true,
        },
        data_disk: DiskSpec {
            path: data,
            read_only: false,
        },
        shares: Vec::new(),
        network: NetworkSpec::Nat,
        console_log: scratch.join("console.log"),
    }
}

async fn transition(events: &mut VmEventStream, previous: VmState, current: VmState) {
    assert_eq!(
        timeout(LIMIT, events.next()).await.unwrap(),
        Some(VmEvent::StateChanged { previous, current }),
    );
}

#[tokio::test]
#[ignore = "requires virtualization entitlement and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn packaged_init_boot_and_native_lifecycle_events() {
    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();

    let data = tempfile::tempdir().unwrap();
    let spec_boot = spec(&guest, data.path(), BOOT);
    let console = spec_boot.console_log.clone();
    let mut vm = VzHypervisor.create(spec_boot).await.unwrap();
    assert_eq!(vm.state(), VmState::Created);
    let mut events = vm.events();
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    transition(&mut events, VmState::Created, VmState::Starting).await;
    transition(&mut events, VmState::Starting, VmState::Running).await;

    timeout(LIMIT, async {
        loop {
            let text = fs::read_to_string(&console).unwrap();
            if text.contains("marsa-init stage=data_disk") {
                assert!(
                    text.contains("Mounted root (erofs filesystem) readonly"),
                    "{text}"
                );
                assert!(
                    text.contains("Run /usr/bin/marsa-init as init process"),
                    "{text}"
                );
                assert!(text.contains("marsa-init stage=mount_system"), "{text}");
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the packaged PID 1 did not reach data disk initialization");

    timeout(LIMIT, vm.stop(Duration::from_millis(50)))
        .await
        .unwrap()
        .unwrap();
    transition(&mut events, VmState::Running, VmState::Stopping).await;
    transition(&mut events, VmState::Stopping, VmState::Stopped).await;
    assert_eq!(vm.state(), VmState::Stopped);
    drop(vm);

    let poweroff = tempfile::tempdir().unwrap();
    let mut vm = VzHypervisor
        .create(spec(&guest, poweroff.path(), GUEST_POWEROFF))
        .await
        .unwrap();
    let mut events = vm.events();
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    transition(&mut events, VmState::Created, VmState::Starting).await;
    transition(&mut events, VmState::Starting, VmState::Running).await;
    transition(&mut events, VmState::Running, VmState::Failed).await;
    assert!(matches!(
        timeout(LIMIT, events.next()).await.unwrap(),
        Some(VmEvent::UnexpectedExit { detail }) if detail.contains("without a stop request"),
    ));
    assert_eq!(vm.state(), VmState::Failed);
    drop(vm);

    let cleanup = tempfile::tempdir().unwrap();
    let mut vm = VzHypervisor
        .create(spec(&guest, cleanup.path(), BOOT))
        .await
        .unwrap();
    let mut events = vm.events();
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    transition(&mut events, VmState::Created, VmState::Starting).await;
    transition(&mut events, VmState::Starting, VmState::Running).await;
    drop(vm);
    transition(&mut events, VmState::Running, VmState::Stopping).await;
    transition(&mut events, VmState::Stopping, VmState::Stopped).await;
}

const CHECK_ROOT: &str = concat!(
    "console=hvc0 root=/dev/vda rootfstype=erofs ro marsa.interface=none panic=-1 init=/bin/sh -- -c \"",
    "exec </dev/hvc0 >/dev/hvc0 2>&1; ",
    "/bin/busybox test -b /dev/vda || exit 1; ",
    "/bin/busybox mount -t sysfs -o ro,nodev,noexec,nosuid sysfs /sys || exit 1; ",
    "read -r ro </sys/class/block/vda/ro; /bin/busybox umount /sys || exit 1; ",
    "test $ro = 1 || exit 1; echo MARSA_ROOT_DEVICE_RO; ",
    "if /bin/busybox dd if=/dev/zero of=/dev/vda bs=1 count=1 conv=notrunc; then exit 1; fi; ",
    "if /bin/busybox touch /.marsa-vz-write-test; then exit 1; fi; ",
    "echo MARSA_ROOT_WRITE_REJECTED; exec /usr/bin/marsa-init\"",
);

async fn read_guest(stream: &mut VsockStream) -> GuestEnvelope {
    let mut header = [0; 4];
    stream.read_exact(&mut header).await.unwrap();
    let mut payload = vec![0; frame_length(header).unwrap()];
    stream.read_exact(&mut payload).await.unwrap();
    decode_guest(&payload).unwrap()
}

async fn send_host(stream: &mut VsockStream, request_id: u64, body: HostBody) {
    let frame = encode_host(&HostEnvelope {
        request_id,
        body: Some(body),
    })
    .unwrap();
    stream.write_all(&frame).await.unwrap();
}

async fn handshake(stream: &mut VsockStream) {
    let greeting = read_guest(stream).await;
    assert_eq!(greeting.request_id, 0);
    let Some(GuestBody::Hello(hello)) = greeting.body else {
        panic!("expected the packaged agent's greeting: {greeting:?}");
    };
    assert_eq!(hello.image_version, env!("CARGO_PKG_VERSION"));
    let protocol = negotiate(hello.protocol.unwrap()).unwrap();
    assert_eq!(protocol, PROTOCOL_VERSION);
    send_host(
        stream,
        0,
        HostBody::HelloAck(HelloAck {
            protocol: Some(protocol),
        }),
    )
    .await;

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    send_host(
        stream,
        1,
        HostBody::SetTime(SetTime {
            unix_seconds: i64::try_from(now.as_secs()).unwrap(),
            nanoseconds: now.subsec_nanos(),
        }),
    )
    .await;

    let mut time_acknowledged = false;
    let mut engine_ready = false;
    while !time_acknowledged || !engine_ready {
        let event = read_guest(stream).await;
        match event.body {
            Some(GuestBody::CommandResult(result)) => {
                assert_eq!(event.request_id, 1);
                assert_eq!(result.status, CommandStatus::Ok as i32, "{result:?}");
                time_acknowledged = true;
            }
            Some(GuestBody::EngineHealth(health)) => {
                engine_ready |= health.state == EngineHealthState::Ready as i32;
            }
            _ => {}
        }
    }
}

#[tokio::test]
#[ignore = "requires virtualization entitlement and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn packaged_agent_handshake_read_only_root_and_full_console() {
    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let mut spec = spec(&guest, scratch.path(), CHECK_ROOT);
    let original_root = spec.root_disk.path.clone();
    let test_root = scratch.path().join("rootfs.img");
    fs::copy(&original_root, &test_root).unwrap();
    let root_digest = Sha256::digest(fs::read(&test_root).unwrap());
    spec.root_disk.path = test_root.clone();
    let console = spec.console_log.clone();
    let mut vm = VzHypervisor.create(spec).await.unwrap();
    assert!(matches!(
        vm.connect_vsock(CONTROL_PORT).await,
        Err(VmError::InvalidState {
            state: VmState::Created,
            ..
        })
    ));
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();

    let connection = timeout(LIMIT, async {
        loop {
            match vm.connect_vsock(CONTROL_PORT).await {
                Ok(stream) => break stream,
                Err(VmError::GuestPortUnavailable { port }) if port == CONTROL_PORT => {
                    sleep(Duration::from_millis(20)).await;
                }
                Err(error) => panic!("connect to port {CONTROL_PORT}: {error:?}"),
            }
        }
    })
    .await;
    let mut stream = connection.unwrap_or_else(|error| {
        panic!(
            "agent did not listen within {LIMIT:?}: {error}\n{}",
            fs::read_to_string(&console).unwrap()
        );
    });
    let unused = timeout(LIMIT, vm.connect_vsock(5_999)).await.unwrap();
    assert!(
        matches!(unused, Err(VmError::GuestPortUnavailable { port: 5_999 })),
        "{:?}",
        unused.err()
    );
    timeout(LIMIT, handshake(&mut stream)).await.unwrap();
    timeout(LIMIT, vm.stop(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    drop(stream);

    let log = fs::read_to_string(console).unwrap();
    let mut rest = log.as_str();
    for marker in [
        "virtio_blk ",
        "Mounted root (erofs filesystem) readonly",
        "Run /bin/sh as init process",
        "MARSA_ROOT_DEVICE_RO\r\n",
        "dd: can't open '/dev/vda':",
        "touch: /.marsa-vz-write-test: Read-only file system",
        "MARSA_ROOT_WRITE_REJECTED\r\n",
        "marsa-init: replaying early kernel boot log",
        "Booting Linux on physical CPU",
        "Kernel command line:",
        "marsa-init: end of early kernel boot log",
        "marsa-init stage=mount_system",
        "marsa-init stage=containerd",
        "marsa-init stage=dockerd",
        "API listen on /var/run/docker.sock",
    ] {
        let Some((_, tail)) = rest.split_once(marker) else {
            panic!("console missing ordered marker {marker:?}:\n{log}");
        };
        rest = tail;
    }
    assert_eq!(
        log.matches("Booting Linux on physical CPU").count(),
        1,
        "{log}"
    );
    assert_eq!(Sha256::digest(fs::read(test_root).unwrap()), root_digest);
}

#[tokio::test]
#[ignore = "requires virtualization entitlement and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn packaged_guest_share_balloon_and_pause_resume() {
    use marsa_vm::ShareSpec;

    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    // Use a pathname with characters the boot arguments must encode.
    let shares = tempfile::tempdir_in("/private/tmp").unwrap();
    let shared = shares.path().join("Project % files");
    fs::create_dir(&shared).unwrap();
    fs::write(shared.join("proof"), b"from-host-virtiofs").unwrap();

    // The child waits for the actual PID 1 setup, then verifies its mounted path
    // and the DHCP-configured interface. The parent becomes packaged PID 1.
    let cmdline = format!(
        "console=hvc0 root=/dev/vda rootfstype=erofs ro marsa.interface=none panic=-1 init=/bin/sh -- -c \"\
         exec </dev/hvc0 >/dev/hvc0 2>&1; (\
         while [ ! -S /var/run/docker.sock ]; do /bin/busybox sleep 1; done; \
         /bin/busybox cat '{path}/proof' && echo && \
         if /bin/busybox touch '{path}/denied'; then exit 1; fi && \
         echo MARSA_SHARE_RO; \
         while ! /bin/busybox grep -q '^balloon_inflate [1-9]' /proc/vmstat; do /bin/busybox sleep 1; done; \
         /bin/busybox grep '^balloon_inflate ' /proc/vmstat; echo MARSA_BALLOON_INFLATED; \
         ) & exec /usr/bin/marsa-init\"",
        path = shared.display(),
    );
    let mut spec = spec(&guest, scratch.path(), &cmdline);
    spec.shares.push(ShareSpec {
        tag: "project".into(),
        host_path: shared.clone(),
        read_only: true,
    });
    let console = spec.console_log.clone();
    let capabilities = VzHypervisor.capabilities();
    assert!(capabilities.memory_balloon && capabilities.pause_resume);
    assert!(!capabilities.dynamic_shares && !capabilities.userspace_network);

    let mut vm = VzHypervisor.create(spec).await.unwrap();
    assert!(vm.network_info().is_none());
    assert!(matches!(
        vm.pause().await,
        Err(VmError::InvalidState {
            state: VmState::Created,
            ..
        })
    ));
    let mut events = vm.events();
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    transition(&mut events, VmState::Created, VmState::Starting).await;
    transition(&mut events, VmState::Starting, VmState::Running).await;

    let mut stream = connect_control(&*vm, &console).await;
    timeout(LIMIT, handshake(&mut stream)).await.unwrap();
    let log = console_with(&console, "MARSA_SHARE_RO\r\n").await;
    assert!(log.contains("from-host-virtiofs\r\n"), "{log}");
    assert!(!shared.join("denied").exists());

    assert!(matches!(
        vm.set_memory_target(1).await,
        Err(VmError::InvalidSpec {
            field: "memory_target",
            ..
        })
    ));
    vm.set_memory_target(896 * 1024 * 1024).await.unwrap();
    let balloon_log = console_with(&console, "MARSA_BALLOON_INFLATED\r\n").await;
    assert!(
        balloon_log
            .lines()
            .any(|line| line.starts_with("balloon_inflate "))
    );
    vm.set_memory_target(1024 * 1024 * 1024).await.unwrap();

    timeout(LIMIT, vm.pause()).await.unwrap().unwrap();
    transition(&mut events, VmState::Running, VmState::Paused).await;
    assert_eq!(vm.state(), VmState::Paused);
    timeout(LIMIT, vm.resume()).await.unwrap().unwrap();
    transition(&mut events, VmState::Paused, VmState::Running).await;
    assert_eq!(vm.state(), VmState::Running);
    timeout(LIMIT, vm.stop(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    transition(&mut events, VmState::Running, VmState::Stopping).await;
    transition(&mut events, VmState::Stopping, VmState::Stopped).await;
    assert!(vm.network_info().is_none());
}

async fn connect_control(vm: &dyn Vm, console: &Path) -> VsockStream {
    timeout(LIMIT, async {
        loop {
            match vm.connect_vsock(CONTROL_PORT).await {
                Ok(stream) => break stream,
                Err(VmError::GuestPortUnavailable { .. }) => sleep(Duration::from_millis(25)).await,
                Err(error) => panic!("{error}\n{}", fs::read_to_string(console).unwrap()),
            }
        }
    })
    .await
    .unwrap_or_else(|error| panic!("{error}\n{}", fs::read_to_string(console).unwrap()))
}

async fn console_with(console: &Path, marker: &str) -> String {
    timeout(LIMIT, async {
        loop {
            let log = fs::read_to_string(console).unwrap();
            if log.contains(marker) {
                break log;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "console did not contain {marker:?}: {error}\n{}",
            fs::read_to_string(console).unwrap()
        )
    })
}

#[tokio::test]
#[ignore = "requires virtualization entitlement, native NAT and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn packaged_guest_nat_address_gateway_and_https() {
    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let cmdline = concat!(
        "console=hvc0 root=/dev/vda rootfstype=erofs ro panic=-1 init=/bin/sh -- -c \"",
        "exec </dev/hvc0 >/dev/hvc0 2>&1; (",
        "while [ ! -S /var/run/docker.sock ]; do /bin/busybox sleep 1; done; ",
        "/sbin/ip -4 -o addr show dev eth0; ",
        "/sbin/ip -4 route show default dev eth0; echo MARSA_NET_CONFIG_CAPTURED; ",
        "/bin/busybox wget -T 5 -O /dev/null https://example.com && echo MARSA_NAT_HTTPS_OK; ",
        ") & exec /usr/bin/marsa-init\"",
    );
    let spec = spec(&guest, scratch.path(), cmdline);
    let console = spec.console_log.clone();
    let mut vm = VzHypervisor.create(spec).await.unwrap();
    assert!(vm.network_info().is_none());
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    let mut stream = connect_control(&*vm, &console).await;
    timeout(LIMIT, handshake(&mut stream)).await.unwrap();
    let network = timeout(LIMIT, async {
        loop {
            if let Some(facts) = vm.network_info() {
                break facts;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the backend did not get network facts");
    let log = console_with(&console, "MARSA_NET_CONFIG_CAPTURED\r\n").await;
    assert!(
        log.lines()
            .any(|line| line.contains(&format!("eth0    inet {}/", network.address))),
        "{log}"
    );
    let gateway = network.gateway.expect("NAT default gateway");
    let gateway_words = ["default", "via", &gateway.to_string()];
    assert!(
        log.lines()
            .any(|line| line.split_ascii_whitespace().take(3).eq(gateway_words)),
        "{log}"
    );
    println!(
        "guest DHCP address {} via {gateway} matches the backend snapshot",
        network.address
    );
    console_with(&console, "MARSA_NAT_HTTPS_OK\r\n").await;
    timeout(LIMIT, vm.stop(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    assert!(vm.network_info().is_none());
}
