//! Prioritize the final command without breaking the in-flight request.

use std::{future::poll_fn, task::Poll, time::Duration};

use marsa_proto::guest::{
    CONTROL_PORT, DOCKER_API_PORT,
    v1::{
        CommandStatus, Shutdown, Trim, TrimResult, VmStats, guest_envelope::Body as GuestBody,
        host_envelope::Body as HostBody,
    },
};
use marsa_vm::fake::{AgentStep, FakeScenario};
use tokio::time::timeout;

use super::{
    TEST_LIMIT, WAIT,
    fixtures::{client, command_result, docker_ping, expect, send, startup, vm},
};
use crate::guest_control::{
    GuestControlError, GuestEvent, GuestResponse, session::REQUEST_CAPACITY,
};

use crate::supervisor::GuestProbe;

#[tokio::test(start_paused = true)]
async fn shutdown_bypasses_full_queue_and_preserves_the_in_flight_request() {
    let capacity = REQUEST_CAPACITY;
    let mut script = startup();
    script.extend([
        expect(2, HostBody::Trim(Trim {})),
        send(0, GuestBody::Stats(VmStats::default())),
        AgentStep::Wait(Duration::from_millis(50)),
        send(
            2,
            GuestBody::TrimResult(TrimResult {
                bytes_trimmed: 512,
                data_disk_used_bytes: 1024,
            }),
        ),
        expect(3 + capacity as u64, HostBody::Shutdown(Shutdown {})),
        command_result(3 + capacity as u64, CommandStatus::Ok),
        AgentStep::Wait(WAIT),
    ]);
    let vm = vm(FakeScenario::default()
        .with_agent(CONTROL_PORT, script)
        .with_agent(DOCKER_API_PORT, docker_ping(200)))
    .await;
    let control = client();
    let mut events = control.events();
    let mut session = control.handshake(vm.as_ref()).await.unwrap();
    session.engine_ready(vm.as_ref()).await.unwrap();

    assert!(matches!(
        control.request(HostBody::Shutdown(Shutdown {})).await,
        Err(GuestControlError::Sequence(_))
    ));

    let mut current = Box::pin(control.request(HostBody::Trim(Trim {})));
    poll_fn(|cx| {
        assert!(current.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    timeout(TEST_LIMIT, async {
        while !matches!(events.recv().await.unwrap(), GuestEvent::Stats(_)) {}
    })
    .await
    .unwrap();

    let mut queued = (0..capacity)
        .map(|_| Box::pin(control.request(HostBody::Trim(Trim {}))))
        .collect::<Vec<_>>();
    poll_fn(|cx| {
        for request in &mut queued {
            assert!(request.as_mut().poll(cx).is_pending());
        }
        Poll::Ready(())
    })
    .await;
    assert!(matches!(
        control.request(HostBody::Trim(Trim {})).await,
        Err(GuestControlError::Busy)
    ));

    let mut shutdown = Box::pin(session.shutdown());
    poll_fn(|cx| {
        assert!(shutdown.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert!(matches!(
        control.request(HostBody::Trim(Trim {})).await,
        Err(GuestControlError::Disconnected)
    ));
    for request in queued {
        assert!(matches!(
            timeout(TEST_LIMIT, request).await.unwrap(),
            Err(GuestControlError::Disconnected)
        ));
    }
    assert!(matches!(
        timeout(TEST_LIMIT, current).await.unwrap().unwrap(),
        GuestResponse::Trim(result) if result.bytes_trimmed == 512
    ));
    timeout(TEST_LIMIT, shutdown).await.unwrap().unwrap();
}
