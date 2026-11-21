//! Exercise the public hypervisor contract without an Apple framework.

use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};
use tokio_stream::StreamExt;

use crate::{
    DiskSpec, Hypervisor, NetworkSpec, VmError, VmEvent, VmSpec, VmState,
    fake::{AgentStep, FakeHypervisor, FakeScenario},
};

const CONTROL: u32 = 5000;
const DELAY: Duration = Duration::from_millis(100);

fn spec() -> VmSpec {
    VmSpec {
        cpus: 2,
        memory_bytes: 512 * 1024 * 1024,
        kernel: "/fixture/Image".into(),
        initramfs: None,
        cmdline: "console=hvc0".into(),
        root_disk: DiskSpec {
            path: "/fixture/rootfs.img".into(),
            read_only: true,
        },
        data_disk: DiskSpec {
            path: "/fixture/data.img".into(),
            read_only: false,
        },
        shares: Vec::new(),
        network: NetworkSpec::Nat,
        console_log: "/fixture/console.log".into(),
    }
}

#[tokio::test]
async fn normal_boot_supports_bidirectional_loopback_vsock() {
    let script = vec![
        AgentStep::Send(b"hello".to_vec()),
        AgentStep::Expect(b"ack".to_vec()),
        AgentStep::Send(b"ready".to_vec()),
    ];
    let hypervisor = FakeHypervisor::new(FakeScenario::default().with_agent(CONTROL, script));
    let mut vm = hypervisor.create(spec()).await.unwrap();
    let mut events = vm.events();
    assert_eq!(vm.state(), VmState::Created);
    vm.start().await.unwrap();
    assert_eq!(
        timeout(DELAY * 2, events.next()).await.unwrap(),
        Some(VmEvent::StateChanged {
            previous: VmState::Created,
            current: VmState::Starting
        })
    );
    assert_eq!(
        timeout(DELAY * 2, events.next()).await.unwrap(),
        Some(VmEvent::StateChanged {
            previous: VmState::Starting,
            current: VmState::Running
        })
    );
    let mut stream = vm.connect_vsock(CONTROL).await.unwrap();
    let mut response = [0; 5];
    stream.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"hello");
    stream.write_all(b"ack").await.unwrap();
    stream.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"ready");
    vm.stop(DELAY).await.unwrap();
    assert_eq!(vm.state(), VmState::Stopped);
}

#[tokio::test(start_paused = true)]
async fn boot_timeout_has_a_typed_failure_and_can_be_stopped() {
    let hypervisor = FakeHypervisor::new(FakeScenario::default().with_start_timeout(DELAY));
    let mut vm = hypervisor.create(spec()).await.unwrap();
    assert!(matches!(
        vm.start().await,
        Err(VmError::Timeout {
            operation: "start",
            limit: DELAY
        })
    ));
    assert_eq!(vm.state(), VmState::Failed);
    vm.stop(DELAY).await.unwrap();
    assert_eq!(vm.state(), VmState::Stopped);
}

#[tokio::test(start_paused = true)]
async fn unexpected_exit_is_reported_and_closes_live_vsock() {
    let script = vec![
        AgentStep::Wait(DELAY * 2),
        AgentStep::Send(b"unused".to_vec()),
    ];
    let hypervisor = FakeHypervisor::new(
        FakeScenario::default()
            .with_exit_after(DELAY)
            .with_agent(CONTROL, script),
    );
    let mut vm = hypervisor.create(spec()).await.unwrap();
    vm.start().await.unwrap();
    let mut events = vm.events();
    let mut connection = vm.connect_vsock(CONTROL).await.unwrap();
    assert_eq!(
        timeout(DELAY * 2, events.next()).await.unwrap(),
        Some(VmEvent::StateChanged {
            previous: VmState::Running,
            current: VmState::Failed
        })
    );
    assert!(matches!(
        timeout(DELAY * 2, events.next()).await.unwrap(),
        Some(VmEvent::UnexpectedExit { .. })
    ));
    assert_eq!(connection.read(&mut [0; 1]).await.unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn slow_agent_handshake_obeys_the_caller_timeout() {
    let script = vec![AgentStep::Wait(DELAY * 2), AgentStep::Send(b"x".to_vec())];
    let hypervisor = FakeHypervisor::new(FakeScenario::default().with_agent(CONTROL, script));
    let mut vm = hypervisor.create(spec()).await.unwrap();
    vm.start().await.unwrap();
    let mut connection = vm.connect_vsock(CONTROL).await.unwrap();
    let mut response = [0; 1];
    assert!(
        tokio::time::timeout(DELAY, connection.read_exact(&mut response))
            .await
            .is_err()
    );
    connection.read_exact(&mut response).await.unwrap();
    assert_eq!(response, *b"x");
}

#[tokio::test]
async fn pause_resume_and_invalid_operations_are_visible() {
    let hypervisor = FakeHypervisor::new(
        FakeScenario::default().with_agent(CONTROL, vec![AgentStep::Send(b"x".to_vec())]),
    );
    let mut vm = hypervisor.create(spec()).await.unwrap();
    assert!(matches!(
        vm.pause().await,
        Err(VmError::InvalidState { .. })
    ));
    let mut events = vm.events();
    vm.start().await.unwrap();
    assert!(timeout(DELAY * 2, events.next()).await.unwrap().is_some());
    assert!(timeout(DELAY * 2, events.next()).await.unwrap().is_some());
    vm.pause().await.unwrap();
    assert_eq!(
        timeout(DELAY * 2, events.next()).await.unwrap(),
        Some(VmEvent::StateChanged {
            previous: VmState::Running,
            current: VmState::Paused
        })
    );
    assert!(matches!(
        vm.connect_vsock(CONTROL).await,
        Err(VmError::InvalidState { .. })
    ));
    vm.resume().await.unwrap();
    assert_eq!(
        timeout(DELAY * 2, events.next()).await.unwrap(),
        Some(VmEvent::StateChanged {
            previous: VmState::Paused,
            current: VmState::Running
        })
    );
    assert!(matches!(
        vm.connect_vsock(CONTROL + 1).await,
        Err(VmError::GuestPortUnavailable { .. })
    ));
}

#[test]
fn vm_spec_rejects_ambiguous_storage_and_unresolved_networking() {
    let mut invalid = spec();
    invalid.data_disk.path = invalid.root_disk.path.clone();
    assert!(matches!(
        invalid.validate(),
        Err(VmError::InvalidSpec {
            field: "data_disk",
            ..
        })
    ));
    assert!(NetworkSpec::try_from(marsa_core::settings::NetworkMode::Auto).is_err());
    assert!(spec().validate().is_ok());
}
