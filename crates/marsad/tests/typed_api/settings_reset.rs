//! Exercise reset authorization and private storage through the typed Unix API.

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

use marsa_proto::host::v1::{
    ConfirmFactoryReset, PrepareSettingsReset, ResetRequest, ResetSettings, engine_server::Engine,
    reset_request::Action, reset_response::Result as ResetResult,
};

use super::*;

#[tokio::test]
async fn confirmed_settings_reset_keeps_disk_grants_and_engine_untouched() {
    let fixture = Fixture::new(vec![]).await;
    let paths = MarsaPaths::from_home(fixture.home.path()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(paths.vm_dir()).unwrap();
    fs::write(paths.data_disk(), b"private test disk").unwrap();
    fs::write(paths.grants_file(), b"private test grants").unwrap();

    let mut settings = SettingsClient::new(fixture.channel.clone());
    let before = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    let mut customized = Settings::default();
    customized.vm.cpus += 1;
    let customized = settings
        .update(UpdateSettingsRequest {
            request_id: action(61).request_id,
            expected_revision: before.revision,
            settings_toml: customized.to_toml().unwrap(),
        })
        .await
        .unwrap()
        .into_inner();

    let mut engine = EngineClient::new(fixture.channel.clone());
    let forbidden = engine
        .reset(ResetRequest {
            request_id: action(62).request_id,
            action: Some(Action::Settings(ResetSettings {
                confirmation_token: vec![],
            })),
        })
        .await
        .unwrap_err();
    assert_eq!(forbidden.code(), Code::InvalidArgument);

    let confirmation = prepare(&mut engine, 63, 64).await;
    let mut another = engine.clone();
    let (first, retry) = tokio::join!(
        engine.reset(confirmation.clone()),
        another.reset(confirmation.clone()),
    );
    for response in [first, retry] {
        let completed = response.unwrap().into_inner();
        assert!(matches!(completed.result, Some(ResetResult::Completed(_))));
    }
    let reused = engine
        .reset(ResetRequest {
            request_id: action(65).request_id,
            ..confirmation
        })
        .await
        .unwrap_err();
    assert_eq!(reused.code(), Code::FailedPrecondition);

    let after = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(after.revision, customized.revision + 1);
    let defaults = Settings::default().to_toml().unwrap();
    assert_eq!(after.settings_toml, defaults);
    assert_eq!(fs::read_to_string(paths.config_file()).unwrap(), defaults);
    assert_eq!(fs::read(paths.data_disk()).unwrap(), b"private test disk");
    assert_eq!(
        fs::read(paths.grants_file()).unwrap(),
        b"private test grants"
    );
    assert_eq!(
        engine
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Stopped
    );
}

#[tokio::test]
async fn reset_rejects_stale_revisions_and_replaced_members() {
    let fixture = Fixture::new(vec![]).await;
    let paths = MarsaPaths::from_home(fixture.home.path()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = EngineClient::new(fixture.channel.clone());
    let mut settings = SettingsClient::new(fixture.channel.clone());

    let mut document = Settings::default();
    document.vm.cpus += 1;
    let prior = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    let stale = prepare(&mut engine, 71, 72).await;
    settings
        .update(UpdateSettingsRequest {
            request_id: action(73).request_id,
            expected_revision: prior.revision,
            settings_toml: document.to_toml().unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(engine.reset(stale).await.unwrap_err().code(), Code::Aborted);

    let original = fs::read(paths.config_file()).unwrap();
    let replaced = prepare(&mut engine, 74, 75).await;
    let substitute = paths.root().join("substitute.toml");
    fs::write(&substitute, &original).unwrap();
    fs::set_permissions(
        &substitute,
        fs::metadata(paths.config_file()).unwrap().permissions(),
    )
    .unwrap();
    fs::rename(&substitute, paths.config_file()).unwrap();
    assert_eq!(
        engine.reset(replaced).await.unwrap_err().code(),
        Code::Aborted
    );
    assert_eq!(fs::read(paths.config_file()).unwrap(), original);

    let link = prepare(&mut engine, 76, 77).await;
    let target = fixture.home.path().join("outside.toml");
    fs::rename(paths.config_file(), &target).unwrap();
    symlink(&target, paths.config_file()).unwrap();
    assert_eq!(engine.reset(link).await.unwrap_err().code(), Code::Aborted);
    assert_eq!(fs::read_link(paths.config_file()).unwrap(), target);
    assert_eq!(fs::read(&target).unwrap(), original);
    let malformed = engine
        .reset(ResetRequest {
            request_id: action(78).request_id,
            action: Some(Action::PrepareSettings(PrepareSettingsReset {})),
        })
        .await
        .unwrap_err();
    assert_eq!(malformed.code(), Code::FailedPrecondition);

    fs::remove_file(paths.config_file()).unwrap();
    fs::hard_link(&target, paths.config_file()).unwrap();
    let hard_link = engine
        .reset(ResetRequest {
            request_id: action(79).request_id,
            action: Some(Action::PrepareSettings(PrepareSettingsReset {})),
        })
        .await
        .unwrap_err();
    assert_eq!(hard_link.code(), Code::FailedPrecondition);
    assert_eq!(fs::read(&target).unwrap(), original);
}

#[tokio::test]
async fn settings_challenges_require_the_owner_and_cannot_confirm_factory_reset() {
    let fixture = Fixture::new(vec![]).await;
    let paths = MarsaPaths::from_home(fixture.home.path()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let preparation = ResetRequest {
        request_id: action(81).request_id,
        action: Some(Action::PrepareSettings(PrepareSettingsReset {})),
    };
    let unauthorized = fixture
        .api
        .reset(tonic::Request::new(preparation))
        .await
        .unwrap_err();
    assert_eq!(unauthorized.code(), Code::PermissionDenied);

    let mut engine = EngineClient::new(fixture.channel.clone());
    let superseded = prepare(&mut engine, 82, 83).await;
    let confirmation = prepare(&mut engine, 84, 85).await;
    assert_eq!(prepare(&mut engine, 84, 85).await, confirmation);
    assert_eq!(
        engine.reset(superseded).await.unwrap_err().code(),
        Code::FailedPrecondition
    );

    let Some(Action::Settings(settings)) = confirmation.action.clone() else {
        panic!("settings challenge must use the settings action");
    };
    let factory = engine
        .reset(ResetRequest {
            request_id: action(87).request_id,
            action: Some(Action::ConfirmFactory(ConfirmFactoryReset {
                confirmation_token: settings.confirmation_token,
            })),
        })
        .await
        .unwrap_err();
    assert_eq!(factory.code(), Code::FailedPrecondition);
    assert!(!paths.config_file().exists());

    assert!(matches!(
        engine
            .reset(confirmation)
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(ResetResult::Completed(_))
    ));
    assert!(paths.config_file().exists());
}

async fn prepare(
    engine: &mut EngineClient<Channel>,
    preparation: u32,
    confirmation: u32,
) -> ResetRequest {
    let response = engine
        .reset(ResetRequest {
            request_id: action(preparation).request_id,
            action: Some(Action::PrepareSettings(PrepareSettingsReset {})),
        })
        .await
        .unwrap()
        .into_inner();
    let Some(ResetResult::SettingsChallenge(challenge)) = response.result else {
        panic!("expected a settings reset challenge");
    };
    assert!(challenge.expires_at_unix_millis > 0);
    ResetRequest {
        request_id: action(confirmation).request_id,
        action: Some(Action::Settings(ResetSettings {
            confirmation_token: challenge.confirmation_token,
        })),
    }
}
