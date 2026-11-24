//! Exercise factory recovery in a private signed bundle with an unavailable backend.

use std::os::unix::fs::OpenOptionsExt;

use marsa_core::settings::VmBackend;
use marsa_proto::host::v1::{
    ConfirmFactoryReset, PrepareFactoryReset, ResetRequest, reset_request::Action,
    reset_response::Result as ResetResult,
};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};

use super::*;

#[tokio::test]
#[ignore = "requires Apple silicon and built guest and network helper artifacts"]
async fn packaged_factory_reset_works_without_a_bootable_engine() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    let (daemon, _) = stage_bundle(scratch.path());
    let log = scratch.path().join("daemon.log");
    let output = fs::File::create(&log).unwrap();
    let mut process = Daemon(
        Command::new(daemon)
            .env("HOME", &home)
            .env("LC_ALL", "C")
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .spawn()
            .unwrap(),
    );
    let channel = connect(&mut process.0, &paths.daemon_socket(), &log).await;
    let mut unavailable = Settings::default();
    unavailable.vm.backend = VmBackend::Krun;
    replace_settings(&channel, &unavailable, 111).await;

    let mut old = fs::File::options()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(paths.data_disk())
        .unwrap();
    old.write_all(b"private disposable disk marker").unwrap();
    old.set_len(GIB).unwrap();
    old.sync_all().unwrap();
    drop(old);
    let original_disk = disk_identity(&paths.data_disk());
    fs::write(paths.grants_file(), b"private disposable grants").unwrap();
    let unrelated = paths.state_dir().join("unrelated");
    fs::write(&unrelated, b"unchanged").unwrap();

    let mut engine = EngineClient::new(channel.clone());
    let failure = timeout(WAIT, engine.start(action(112))).await.unwrap();
    assert!(
        failure.is_err(),
        "the deliberately unsupported backend started"
    );
    let challenge = engine
        .reset(ResetRequest {
            request_id: action(113).request_id,
            action: Some(Action::PrepareFactory(PrepareFactoryReset {})),
        })
        .await
        .unwrap()
        .into_inner()
        .result;
    let Some(ResetResult::Challenge(challenge)) = challenge else {
        panic!("expected a factory challenge");
    };
    assert_eq!(disk_identity(&paths.data_disk()), original_disk);
    assert!(paths.grants_file().exists());
    let completed = engine
        .reset(ResetRequest {
            request_id: action(114).request_id,
            action: Some(Action::ConfirmFactory(ConfirmFactoryReset {
                confirmation_token: challenge.confirmation_token,
            })),
        })
        .await
        .unwrap()
        .into_inner()
        .result;
    assert!(matches!(completed, Some(ResetResult::Completed(_))));
    assert_ne!(disk_identity(&paths.data_disk()), original_disk);
    assert!(!paths.grants_file().exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"unchanged");
    let defaults = Settings::default();
    assert_eq!(
        fs::read_to_string(paths.config_file()).unwrap(),
        defaults.to_toml().unwrap()
    );
    let metadata = fs::metadata(paths.data_disk()).unwrap();
    assert_eq!(metadata.len(), defaults.disk.data_size_limit);
    assert!(metadata.blocks() * 512 < metadata.len());
    let mut sector = [1; 512];
    fs::File::open(paths.data_disk())
        .unwrap()
        .read_exact(&mut sector)
        .unwrap();
    assert_eq!(sector, [0; 512]);
    assert_eq!(
        engine
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Stopped,
    );

    drop(engine);
    drop(channel);
    kill(
        Pid::from_raw(process.0.id().try_into().unwrap()),
        Signal::SIGTERM,
    )
    .unwrap();
    assert!(
        timeout(WAIT, async {
            loop {
                if let Some(status) = process.0.try_wait().unwrap() {
                    break status.success();
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap()
    );
}
