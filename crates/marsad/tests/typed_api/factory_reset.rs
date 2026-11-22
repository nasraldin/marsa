//! Verify factory reset against owner-scoped daemon fixtures with no native guest.

use std::{
    fs,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};

use marsa_proto::host::v1::{
    ConfirmFactoryReset, PrepareFactoryReset, ResetRequest, ResetResponse, engine_server::Engine,
    reset_request::Action, reset_response::Result as ResultBody,
};

use super::*;

#[tokio::test]
async fn confirmed_factory_recreates_storage_even_when_the_engine_cannot_start() {
    let fixture = Fixture::new(Vec::new()).await;
    let paths = private_storage(&fixture);
    let mut engine = EngineClient::new(fixture.channel.clone());
    assert!(
        timeout(WAIT, engine.start(action(91)))
            .await
            .unwrap()
            .is_err()
    );
    let unauthorized = fixture
        .api
        .reset(tonic::Request::new(prepare_request(92)))
        .await;
    assert_eq!(unauthorized.unwrap_err().code(), Code::PermissionDenied);
    let old_disk = fs::metadata(paths.data_disk()).unwrap().ino();
    let before = fs::read(paths.data_disk()).unwrap();
    let challenge = prepare(&mut engine, 93).await;
    assert_eq!(fs::read(paths.data_disk()).unwrap(), before);
    assert!(paths.grants_file().exists());

    let confirmed = confirmation(94, challenge.clone());
    let unauthorized = fixture
        .api
        .reset(tonic::Request::new(confirmed.clone()))
        .await;
    assert_eq!(unauthorized.unwrap_err().code(), Code::PermissionDenied);
    for _ in 0..2 {
        assert!(matches!(
            engine
                .reset(confirmed.clone())
                .await
                .unwrap()
                .into_inner()
                .result,
            Some(ResultBody::Completed(_))
        ));
    }
    assert_eq!(
        engine
            .reset(confirmation(95, challenge))
            .await
            .unwrap_err()
            .code(),
        Code::FailedPrecondition
    );
    let disk = fs::metadata(paths.data_disk()).unwrap();
    assert_ne!(disk.ino(), old_disk);
    assert_eq!(disk.len(), Settings::default().disk.data_size_limit);
    assert!(disk.blocks() * 512 < disk.len());
    assert!(!paths.grants_file().exists());
    assert_eq!(
        Settings::from_toml(&fs::read_to_string(paths.config_file()).unwrap()).unwrap(),
        Settings::default(),
    );
    assert_eq!(
        engine
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Stopped,
    );
    assert_eq!(
        fs::read(paths.root().join("unrelated")).unwrap(),
        b"untouched"
    );
}

#[tokio::test]
async fn stale_factory_confirmation_does_not_change_any_private_member() {
    let fixture = Fixture::new(Vec::new()).await;
    let paths = private_storage(&fixture);
    let mut engine = EngineClient::new(fixture.channel.clone());
    let challenge = prepare(&mut engine, 96).await;
    let before = fs::read(paths.data_disk()).unwrap();
    fs::rename(paths.grants_file(), paths.state_dir().join("prior-grants")).unwrap();
    fs::write(paths.grants_file(), b"new grants").unwrap();

    assert_eq!(
        engine
            .reset(confirmation(97, challenge))
            .await
            .unwrap_err()
            .code(),
        Code::Aborted,
    );
    assert_eq!(fs::read(paths.data_disk()).unwrap(), before);
    assert_eq!(fs::read(paths.grants_file()).unwrap(), b"new grants");
    assert!(!paths.config_file().exists());
}

#[tokio::test]
async fn unrecognized_journal_keeps_activation_and_persistent_reads_closed() {
    let fixture = Fixture::new(Vec::new()).await;
    let paths = private_storage(&fixture);
    let mut engine = EngineClient::new(fixture.channel.clone());
    let challenge = prepare(&mut engine, 98).await;
    let before = fs::read(paths.data_disk()).unwrap();
    let pending = paths.root().join(".factory-reset");
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(pending, fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(
        engine
            .reset(confirmation(99, challenge))
            .await
            .unwrap_err()
            .code(),
        Code::Unavailable,
    );
    assert_eq!(
        engine.start(action(100)).await.unwrap_err().code(),
        Code::Unavailable
    );
    assert!(
        matches!(fixture.api.connect_docker().await, Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
    assert_eq!(
        SettingsClient::new(fixture.channel.clone())
            .get(GetSettingsRequest {})
            .await
            .unwrap_err()
            .code(),
        Code::Unavailable,
    );
    let config = UpdateSettingsRequest {
        request_id: action(101).request_id,
        expected_revision: 1,
        settings_toml: Settings::default().to_toml().unwrap(),
    };
    assert_eq!(
        SettingsClient::new(fixture.channel.clone())
            .update(config)
            .await
            .unwrap_err()
            .code(),
        Code::Unavailable,
    );
    assert_eq!(fs::read(paths.data_disk()).unwrap(), before);
}

fn private_storage(fixture: &Fixture) -> MarsaPaths {
    let paths = MarsaPaths::from_home(fixture.home.path()).unwrap();
    for directory in [paths.root().to_owned(), paths.vm_dir(), paths.state_dir()] {
        fs::create_dir_all(&directory).unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let disk = fs::File::options()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(paths.data_disk())
        .unwrap();
    disk.set_len(512).unwrap();
    fs::write(paths.grants_file(), b"old grants").unwrap();
    fs::write(paths.root().join("unrelated"), b"untouched").unwrap();
    paths
}

fn prepare_request(id: u32) -> ResetRequest {
    ResetRequest {
        request_id: action(id).request_id,
        action: Some(Action::PrepareFactory(PrepareFactoryReset {})),
    }
}

async fn prepare(engine: &mut EngineClient<Channel>, id: u32) -> Vec<u8> {
    let ResetResponse {
        result: Some(ResultBody::Challenge(challenge)),
    } = engine
        .reset(prepare_request(id))
        .await
        .unwrap()
        .into_inner()
    else {
        panic!("expected owner factory challenge");
    };
    assert!(challenge.expires_at_unix_millis > 0);
    challenge.confirmation_token
}

fn confirmation(id: u32, token: Vec<u8>) -> ResetRequest {
    ResetRequest {
        request_id: action(id).request_id,
        action: Some(Action::ConfirmFactory(ConfirmFactoryReset {
            confirmation_token: token,
        })),
    }
}
