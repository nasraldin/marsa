//! Exercise the default sparse disk on first start and after canceling its preparation.

use super::*;
use marsa_core::settings::NetworkMode as ConfiguredMode;

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest and network helper artifacts"]
async fn default_capacity_after_interrupted_first_start() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let (daemon, _) = stage_bundle(scratch.path());
    for mode in [ConfiguredMode::Userspace, ConfiguredMode::Auto] {
        for interrupted in [false, true] {
            let home = tempfile::tempdir_in(scratch.path()).unwrap();
            let paths = MarsaPaths::from_home(home.path()).unwrap();
            let log = home.path().join("daemon.log");
            let output = fs::File::create(&log).unwrap();
            let mut process = Daemon(
                Command::new(&daemon)
                    .env("HOME", home.path())
                    .stdout(output.try_clone().unwrap())
                    .stderr(output)
                    .spawn()
                    .unwrap(),
            );
            let channel = connect(&mut process.0, &paths.daemon_socket(), &log).await;
            let mut settings = Settings::default();
            settings.vm.cpus = 2;
            settings.vm.memory = Some(1024 * MIB);
            settings.vm.idle_stop = false;
            settings.network.mode = mode;
            replace_settings(&channel, &settings, 1).await;
            let mut engine = EngineClient::new(channel);
            let disk = paths.data_disk();
            assert!(!disk.exists());
            let initial_inode = if interrupted {
                let mut starter = engine.clone();
                let start = tokio::spawn(async move { starter.start(action(2)).await });
                timeout(WAIT, async {
                    while !fs::read_to_string(paths.console_log())
                        .unwrap_or_default()
                        .contains("marsa-init stage=mount_system")
                    {
                        assert!(
                            !start.is_finished(),
                            "Start finished before the cancellation point"
                        );
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
                let identity = disk_identity(&disk);
                let stopped = timeout(WAIT, engine.stop(action(3)))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(stopped.into_inner().state(), EngineState::Stopped);
                assert!(start.await.unwrap().is_err());
                assert_eq!(fs::metadata(&disk).unwrap().blocks(), 0);
                Some(identity)
            } else {
                None
            };
            let started_at = Instant::now();
            let ready = timeout(Duration::from_secs(75), engine.start(action(4)))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(ready.into_inner().state(), EngineState::Ready);
            eprintln!(
                "{mode:?} interrupted={interrupted} ready={:?}",
                started_at.elapsed()
            );
            assert_eq!(fs::metadata(&disk).unwrap().len(), 64 * GIB);
            if let Some(identity) = initial_inode {
                assert_eq!(disk_identity(&disk), identity);
            }
            let stopped = timeout(WAIT, engine.stop(action(5)))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stopped.into_inner().state(), EngineState::Stopped);
        }
    }
}
