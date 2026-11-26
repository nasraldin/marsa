//! Exercise boot and live replacement using the actual control stream.

use super::*;
use marsa_proto::guest::v1::{EngineHealthState, InstallCertificates, StartEngine};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

struct Source(Arc<Mutex<Vec<Vec<u8>>>>);

impl crate::guest_control::TrustSource for Source {
    fn certificates(&self) -> io::Result<Vec<Vec<u8>>> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn trust_precedes_clock_and_approved_proxy_and_live_removal_is_acknowledged() {
    let roots = Arc::new(Mutex::new(vec![vec![1, 2, 3]]));
    let proxy = boot_proxy();
    let initial = boot_proxy_script(
        &proxy,
        GuestBody::CommandResult(marsa_proto::guest::v1::CommandResult {
            status: CommandStatus::Ok.into(),
            ..Default::default()
        }),
    );
    let install = |roots: Vec<Vec<u8>>| {
        HostBody::InstallCertificates(InstallCertificates {
            certificates_der: roots,
        })
    };
    let mut script = initial[..2].to_vec();
    script.extend([
        expect(1, install(roots.lock().unwrap().clone())),
        command_result(1, CommandStatus::Ok),
        expect(
            2,
            HostBody::SetTime(SetTime {
                unix_seconds: 1_800_000_000,
                nanoseconds: 700,
            }),
        ),
        command_result(2, CommandStatus::Ok),
        expect(3, HostBody::ConfigureNetwork(proxy.clone())),
        command_result(3, CommandStatus::Ok),
        expect(4, HostBody::StartEngine(StartEngine {})),
        command_result(4, CommandStatus::Ok),
        send(0, fixtures::health(EngineHealthState::Ready)),
        expect(5, install(vec![])),
        command_result(5, CommandStatus::Unavailable),
        expect(6, install(vec![])),
        command_result(6, CommandStatus::Ok),
        AgentStep::Wait(WAIT),
    ]);
    let vm = vm(FakeScenario::default()
        .with_agent(CONTROL_PORT, script)
        .with_agent(DOCKER_API_PORT, docker_ping(200)))
    .await;
    let staged = Arc::new(tokio::sync::Notify::new());
    let reported = Arc::new(AtomicBool::new(false));
    let notification = staged.clone();
    let flag = reported.clone();
    let control = client()
        .with_trust_source(Source(roots.clone()))
        .observe_active(move |event| {
            if matches!(event, GuestEvent::TrustRestartRequired) {
                flag.store(true, Ordering::Release);
                notification.notify_one();
            }
        });
    let mut session = control
        .handshake_for_boot(vm.as_ref(), Some(&proxy))
        .await
        .unwrap();
    session.engine_ready(vm.as_ref()).await.unwrap();
    assert!(!reported.load(Ordering::Acquire));
    roots.lock().unwrap().clear();
    timeout(Duration::from_secs(8), staged.notified())
        .await
        .unwrap();
    assert!(reported.load(Ordering::Acquire));
}
