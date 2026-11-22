//! Exercise the same admission and lifecycle paths used by maintenance owners.

use super::*;

#[tokio::test(start_paused = true)]
async fn one_owner_holds_the_lifecycle_and_docker_gates() {
    let (supervisor, created) = create(config(), "/fixture/console.log".into(), ready(), None);
    let revision = supervisor.snapshot().lifecycle_revision;
    let (a, b) = tokio::join!(
        supervisor.reserve(Some(revision)),
        supervisor.reserve(Some(revision))
    );
    let permit = match (a, b) {
        (Ok(permit), Err(SupervisorError::Busy)) | (Err(SupervisorError::Busy), Ok(permit)) => {
            permit
        }
        _ => panic!("expected exactly one maintenance owner"),
    };
    assert!(matches!(
        supervisor.ensure_ready().await,
        Err(SupervisorError::Busy)
    ));
    assert!(matches!(
        supervisor.restart().await,
        Err(SupervisorError::Busy)
    ));
    assert!(matches!(
        supervisor.pause().await,
        Err(SupervisorError::Busy)
    ));
    assert!(matches!(
        supervisor.resume().await,
        Err(SupervisorError::Busy)
    ));
    assert!(matches!(
        supervisor.stop().await,
        Err(SupervisorError::Busy)
    ));
    assert_eq!(
        supervisor.connect_docker().await.err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(created.load(Ordering::Relaxed), 0);
    permit.stop().await.unwrap();
    permit.start().await.unwrap();
    assert_eq!(supervisor.snapshot().state, State::Ready);
    drop(permit);
    assert!(matches!(
        supervisor.reserve(Some(revision)).await,
        Err(SupervisorError::StaleRevision { .. })
    ));
    supervisor.ensure_ready().await.unwrap();
    assert_eq!(created.load(Ordering::Relaxed), 1);
}

#[tokio::test(start_paused = true)]
async fn maintenance_stop_cancels_an_already_requested_ordinary_boot() {
    let pending = FakeScenario::default().with_agent(PORT, vec![AgentStep::Wait(STEP * 20)]);
    let (supervisor, created) = create(
        config(),
        "/fixture/console.log".into(),
        ready(),
        Some(pending),
    );
    let mut states = supervisor.snapshots();
    let starter = supervisor.clone();
    let boot = tokio::spawn(async move { starter.ensure_ready().await });
    states
        .wait_for(|state| state.state == State::Booting)
        .await
        .unwrap();

    let permit = supervisor.reserve(None).await.unwrap();
    assert!(matches!(boot.await.unwrap(), Err(SupervisorError::Busy)));
    permit.stop().await.unwrap();
    assert_eq!(supervisor.snapshot().state, State::Stopped);
    permit.start().await.unwrap();
    assert_eq!(created.load(Ordering::Relaxed), 2);
    drop(permit);
    supervisor.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn dropping_an_owner_during_boot_cleans_up_before_the_next_activation() {
    let pending = FakeScenario::default().with_agent(PORT, vec![AgentStep::Wait(STEP * 20)]);
    let (supervisor, created) = create(
        config(),
        "/fixture/console.log".into(),
        ready(),
        Some(pending),
    );
    let mut states = supervisor.snapshots();
    let permit = supervisor.reserve(None).await.unwrap();
    {
        let start = permit.start();
        tokio::pin!(start);
        tokio::select! {
            result = &mut start => panic!("unexpected first startup result {result:?}"),
            _ = states.wait_for(|state| state.state == State::Booting) => {}
        }
    }
    assert_eq!(created.load(Ordering::Relaxed), 1);
    drop(permit);
    supervisor.ensure_ready().await.unwrap();
    assert_eq!(created.load(Ordering::Relaxed), 2);
    assert_eq!(supervisor.snapshot().state, State::Ready);
}

#[tokio::test(start_paused = true)]
async fn canceled_reservation_does_not_interrupt_an_ordinary_boot() {
    use super::super::actor::Command;
    use tokio::sync::oneshot;

    let mut settings = config();
    settings.boot_timeout = STEP * 40;
    let slow_ready = FakeScenario::default().with_agent(
        PORT,
        vec![AgentStep::Wait(STEP * 20), AgentStep::Send(b"HR".to_vec())],
    );
    let (supervisor, created) = create(settings, "/fixture/console.log".into(), slow_ready, None);
    let mut states = supervisor.snapshots();
    let starter = supervisor.clone();
    let boot = tokio::spawn(async move { starter.ensure_ready().await });
    states
        .wait_for(|state| state.state == State::Booting)
        .await
        .unwrap();

    let (release, released) = oneshot::channel();
    let (reply, response) = oneshot::channel();
    drop(response);
    drop(release);
    supervisor
        .commands
        .send(Command::Reserve {
            expected: None,
            release: released,
            reply,
        })
        .await
        .unwrap();
    // Send a second ordinary request queued behind the canceled reservation.
    let join = supervisor.ensure_ready();
    let (a, b) = tokio::join!(boot, join);
    a.unwrap().unwrap();
    b.unwrap();
    assert_eq!(created.load(Ordering::Relaxed), 1);
}

#[tokio::test(start_paused = true)]
async fn maintenance_candidate_failure_does_not_retry() {
    let mut settings = config();
    settings.max_retries = 3;
    let failure = FakeScenario::default().with_start_timeout(STEP);
    let (supervisor, created) = create(
        settings,
        "/fixture/console.log".into(),
        ready(),
        Some(failure),
    );
    let permit = supervisor.reserve(None).await.unwrap();
    assert!(matches!(
        permit.start().await,
        Err(SupervisorError::Boot(_))
    ));
    advance(STEP * 20).await;
    assert_eq!(created.load(Ordering::Relaxed), 1);
    assert_eq!(supervisor.snapshot().state, State::Stopped);
    assert!(matches!(
        supervisor.ensure_ready().await,
        Err(SupervisorError::Busy)
    ));
    drop(permit);
    supervisor.ensure_ready().await.unwrap();
    assert_eq!(created.load(Ordering::Relaxed), 2);
}

#[tokio::test(start_paused = true)]
async fn old_docker_stream_closes_on_owner_stop_and_new_admission_waits_for_release() {
    let guest = ready().with_agent(
        DOCKER_API_PORT,
        vec![
            AgentStep::Expect(b"request".to_vec()),
            AgentStep::Send(b"reply".to_vec()),
            AgentStep::Wait(STEP * 100),
        ],
    );
    let (supervisor, _) = create(config(), "/fixture/console.log".into(), guest, None);
    let mut old = supervisor.connect_docker().await.unwrap();
    let permit = supervisor.reserve(None).await.unwrap();
    // The second half of the public connect must independently enforce
    // admission when readiness completed before maintenance was reserved.
    let (reply, result) = tokio::sync::oneshot::channel();
    supervisor
        .commands
        .send(super::super::actor::Command::ConnectDocker(reply))
        .await
        .unwrap();
    assert_eq!(
        result.await.unwrap().err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    old.write_all(b"request").await.unwrap();
    let mut response = [0; 5];
    old.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"reply");
    permit.stop().await.unwrap();
    assert_eq!(old.read(&mut response).await.unwrap(), 0);
    permit.start().await.unwrap();
    assert_eq!(
        supervisor.connect_docker().await.err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(permit);
    let _fresh = supervisor.connect_docker().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn maintenance_suspends_idle_stop_and_release_reenables_it() {
    let mut settings = config();
    settings.idle_stop = true;
    let (supervisor, _) = create(settings, "/fixture/console.log".into(), ready(), None);
    supervisor.ensure_ready().await.unwrap();
    let permit = supervisor.reserve(None).await.unwrap();
    supervisor.report_containers(0).await.unwrap();
    advance(STEP * 8).await;
    assert_eq!(supervisor.snapshot().state, State::Ready);
    drop(permit);
    supervisor.report_containers(0).await.unwrap();
    let mut states = supervisor.snapshots();
    timeout(
        STEP * 20,
        states.wait_for(|state| state.state == State::Stopped),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_reserved_lifecycle_does_not_run_a_previously_scheduled_retry() {
    let mut settings = config();
    settings.max_retries = 1;
    settings.initial_backoff = STEP * 4;
    let failure = FakeScenario::default().with_start_timeout(STEP);
    let (supervisor, created) = create(
        settings,
        "/fixture/console.log".into(),
        ready(),
        Some(failure),
    );
    let mut states = supervisor.snapshots();
    let starter = supervisor.clone();
    let boot = tokio::spawn(async move { starter.ensure_ready().await });
    states
        .wait_for(|state| state.state == State::Failed)
        .await
        .unwrap();
    let permit = supervisor.reserve(None).await.unwrap();
    assert!(matches!(boot.await.unwrap(), Err(SupervisorError::Busy)));
    advance(STEP * 12).await;
    assert_eq!(created.load(Ordering::Relaxed), 1);
    assert_eq!(supervisor.snapshot().state, State::Stopped);
    drop(permit);
    supervisor.ensure_ready().await.unwrap();
    assert_eq!(created.load(Ordering::Relaxed), 2);
}

#[tokio::test(start_paused = true)]
async fn releasing_a_ready_uncommitted_candidate_stops_it_before_admission() {
    let (supervisor, created) = create(config(), "/fixture/console.log".into(), ready(), None);
    let permit = supervisor.reserve(None).await.unwrap();
    permit.start_candidate().await.unwrap();
    assert_eq!(supervisor.snapshot().state, State::Ready);
    drop(permit);
    let mut snapshots = supervisor.snapshots();
    snapshots
        .wait_for(|snapshot| snapshot.state == State::Stopped)
        .await
        .unwrap();
    supervisor.ensure_ready().await.unwrap();
    assert_eq!(created.load(Ordering::Relaxed), 2);
}

#[tokio::test(start_paused = true)]
async fn releasing_a_committed_candidate_keeps_the_same_vm() {
    let (supervisor, created) = create(config(), "/fixture/console.log".into(), ready(), None);
    let permit = supervisor.reserve(None).await.unwrap();
    permit.start_candidate().await.unwrap();
    permit.keep_candidate().await.unwrap();
    drop(permit);
    supervisor.ensure_ready().await.unwrap();
    assert_eq!(supervisor.snapshot().state, State::Ready);
    assert_eq!(created.load(Ordering::Relaxed), 1);
}
