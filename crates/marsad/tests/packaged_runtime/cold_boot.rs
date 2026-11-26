//! Exercise the default sparse disk on first start and after canceling its preparation.

use super::*;
use marsa_core::settings::NetworkMode as ConfiguredMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirstStart {
    Fresh,
    Stop,
    PowerLoss,
}

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest and network helper artifacts"]
async fn default_capacity_after_interrupted_first_start() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let (daemon, _) = stage_bundle(scratch.path());
    for mode in [ConfiguredMode::Userspace, ConfiguredMode::Auto] {
        for first in [FirstStart::Fresh, FirstStart::Stop, FirstStart::PowerLoss] {
            let home = tempfile::tempdir_in(scratch.path()).unwrap();
            let paths = MarsaPaths::from_home(home.path()).unwrap();
            let log = home.path().join("daemon.log");
            let interrupted = first != FirstStart::Fresh;
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
                        .contains("marsa-init: data disk blank scan started")
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
                if first == FirstStart::Stop {
                    let stopped = timeout(WAIT, engine.stop(action(3)))
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(stopped.into_inner().state(), EngineState::Stopped);
                } else {
                    // A graceful stop can finish formatting before VZ powers off.
                    // Kill only this fixture's daemon to also cover loss during the blank scan.
                    process.0.kill().unwrap();
                    process.0.wait().unwrap();
                    assert_eq!(
                        fs::metadata(&disk).unwrap().blocks(),
                        0,
                        "{}",
                        boot_stages(&paths)
                    );
                    process = recovery::spawn(&daemon, home.path(), &log);
                    let reconnected = connect(&mut process.0, &paths.daemon_socket(), &log).await;
                    engine = EngineClient::new(reconnected);
                }
                assert!(start.await.unwrap().is_err());
                Some(identity)
            } else {
                None
            };
            let started_at = Instant::now();
            let ready = timeout(Duration::from_secs(75), engine.start(action(4)))
                .await
                .unwrap_or_else(|_| panic!("boot exceeded 75 s: {}", boot_stages(&paths)))
                .unwrap();
            assert_eq!(ready.into_inner().state(), EngineState::Ready);
            eprintln!("{mode:?} first={first:?} ready={:?}", started_at.elapsed());
            assert_eq!(fs::metadata(&disk).unwrap().len(), 64 * GIB);
            if let Some(identity) = initial_inode {
                assert_eq!(disk_identity(&disk), identity);
            }
            let stopped = timeout(WAIT, engine.stop(action(5)))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stopped.into_inner().state(), EngineState::Stopped);
            recovery::assert_clean_filesystem(&disk);
            let console = fs::read_to_string(paths.console_log()).unwrap();
            assert!(
                console.contains("marsa-init stage=agent"),
                "missing guest boot evidence"
            );
            for marker in [
                "Internal error: Oops",
                "Kernel panic",
                "Fixing recursive fault",
            ] {
                assert!(
                    !console.contains(marker),
                    "guest kernel fault during {mode:?} first={first:?}: {marker}"
                );
            }
        }
    }
}

fn boot_stages(paths: &MarsaPaths) -> String {
    fs::read_to_string(paths.console_log())
        .unwrap_or_default()
        .lines()
        .filter(|line| {
            line.starts_with("marsa-init")
                || line.starts_with("Linux version ")
                || line.starts_with("Internal error:")
                || line.starts_with("Kernel panic")
        })
        .take(80)
        .collect::<Vec<_>>()
        .join("\n")
}
