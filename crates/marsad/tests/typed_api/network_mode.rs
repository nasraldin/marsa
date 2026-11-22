//! Validate network mode mutations through the authenticated Unix service.

use super::*;
use marsa_core::settings::NetworkMode as ConfigMode;
use marsa_proto::host::v1::SetNetworkModeRequest;
use std::fs;

fn set(step: u32, mode: NetworkMode) -> SetNetworkModeRequest {
    SetNetworkModeRequest {
        request_id: format!("55000000-0000-4000-8000-{step:012}"),
        mode: mode.into(),
    }
}

#[tokio::test]
async fn network_mode_checks_the_peer_enum_and_request_id_before_editing() {
    let fixture = Fixture::new(Vec::new()).await;
    let mut settings = SettingsClient::new(fixture.channel.clone());
    let before = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        Network::set_mode(&fixture.api, Request::new(set(1, NetworkMode::Nat)))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied,
    );
    let mut network = NetworkClient::new(fixture.channel.clone());
    for invalid in [
        set(1, NetworkMode::Unspecified),
        SetNetworkModeRequest {
            mode: i32::MAX,
            ..set(2, NetworkMode::Nat)
        },
        SetNetworkModeRequest {
            request_id: "invalid".into(),
            ..set(3, NetworkMode::Nat)
        },
    ] {
        assert_eq!(
            network.set_mode(invalid).await.unwrap_err().code(),
            Code::InvalidArgument
        );
    }
    let paths = MarsaPaths::from_home(fixture.home.path()).unwrap();
    assert!(!paths.config_file().exists());
    assert_eq!(
        settings
            .get(GetSettingsRequest {})
            .await
            .unwrap()
            .into_inner(),
        before
    );
}

#[tokio::test]
async fn network_mode_preserves_the_active_attachment_and_other_settings() {
    let fixture = Fixture::with_network_info(
        vec![AgentStep::Send(b"HR".to_vec())],
        Some(NetworkInfo {
            address: "192.0.2.2".parse().unwrap(),
            gateway: Some("192.0.2.1".parse().unwrap()),
        }),
    )
    .await;
    let mut settings = SettingsClient::new(fixture.channel.clone());
    let mut network = NetworkClient::new(fixture.channel.clone());
    let original = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    let mut expected = Settings::from_toml(&original.settings_toml).unwrap();
    expected.vm.cpus += 1;
    expected.network.dns.push("192.0.2.53".parse().unwrap());
    expected.network.proxy.https = Some("http://fixture.invalid:3128".into());
    let customized = settings
        .update(UpdateSettingsRequest {
            request_id: action(12).request_id,
            expected_revision: original.revision,
            settings_toml: expected.to_toml().unwrap(),
        })
        .await
        .unwrap()
        .into_inner();
    let mut engine = EngineClient::new(fixture.channel.clone());
    engine.start(action(1)).await.unwrap();
    let boot = fixture.supervisor.snapshot();

    let request = set(1, NetworkMode::Userspace);
    let mut duplicate = network.clone();
    let (first, replay) = tokio::join!(
        network.set_mode(request.clone()),
        duplicate.set_mode(request.clone()),
    );
    let first = first.unwrap().into_inner();
    assert_eq!(replay.unwrap().into_inner(), first);
    assert_eq!(first.configured_mode(), NetworkMode::Userspace);
    assert_eq!(first.active_mode(), NetworkMode::Nat);
    assert_eq!(first.restart_pending, Some(true));
    assert_eq!(first.guest_address.as_deref(), Some("192.0.2.2"));
    assert_eq!(first.gateway_address.as_deref(), Some("192.0.2.1"));
    assert_eq!(first.reason.as_deref(), Some("probe_unavailable"));
    assert_eq!(
        fixture.supervisor.snapshot().lifecycle_revision,
        boot.lifecycle_revision
    );
    assert_eq!(
        engine
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Ready
    );

    let saved = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved.revision, customized.revision + 1);
    assert!(saved.restart_required);
    let paths = MarsaPaths::from_home(fixture.home.path()).unwrap();
    assert_eq!(
        fs::read_to_string(paths.config_file()).unwrap(),
        saved.settings_toml
    );
    expected.network.mode = ConfigMode::Userspace;
    assert_eq!(Settings::from_toml(&saved.settings_toml).unwrap(), expected);
    assert_eq!(
        network
            .set_mode(set(1, NetworkMode::Nat))
            .await
            .unwrap_err()
            .code(),
        Code::AlreadyExists,
    );
    assert_eq!(
        settings
            .update(UpdateSettingsRequest {
                request_id: action(13).request_id,
                expected_revision: customized.revision,
                settings_toml: customized.settings_toml,
            })
            .await
            .unwrap_err()
            .code(),
        Code::Aborted,
    );

    network
        .set_mode(set(2, NetworkMode::Userspace))
        .await
        .unwrap();
    assert_eq!(
        settings
            .get(GetSettingsRequest {})
            .await
            .unwrap()
            .into_inner(),
        saved
    );
    engine.stop(action(2)).await.unwrap();
    assert_eq!(network.set_mode(request).await.unwrap().into_inner(), first);
    let stopped = network
        .status(NetworkStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(stopped.configured_mode(), NetworkMode::Userspace);
    assert_eq!(stopped.active_mode(), NetworkMode::Unspecified);
    assert_eq!(stopped.restart_pending, Some(false));
    assert_eq!(
        (
            stopped.guest_address,
            stopped.gateway_address,
            stopped.reason
        ),
        (None, None, None)
    );
}

#[tokio::test]
async fn network_mode_serializes_with_other_settings_writers_after_a_failed_boot() {
    let fixture = Fixture::new(Vec::new()).await;
    let mut network = NetworkClient::new(fixture.channel.clone());
    let mut engine = EngineClient::new(fixture.channel.clone());
    assert_eq!(
        engine.start(action(1)).await.unwrap_err().code(),
        Code::Unavailable
    );
    let mut settings = SettingsClient::new(fixture.channel.clone());
    let before = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    let mut replacement = Settings::from_toml(&before.settings_toml).unwrap();
    replacement.vm.cpus += 1;

    let (changed, other) = tokio::join!(
        network.set_mode(set(3, NetworkMode::Nat)),
        settings.update(UpdateSettingsRequest {
            request_id: action(12).request_id,
            expected_revision: before.revision,
            settings_toml: replacement.to_toml().unwrap(),
        }),
    );
    let response = changed.unwrap().into_inner();
    assert_eq!(response.configured_mode(), NetworkMode::Nat);
    assert_eq!(response.active_mode(), NetworkMode::Unspecified);
    assert_eq!(
        (
            response.guest_address,
            response.gateway_address,
            response.reason
        ),
        (None, None, None)
    );
    let saved = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    let now = Settings::from_toml(&saved.settings_toml).unwrap();
    assert_eq!(now.network.mode, ConfigMode::Nat);
    match other {
        Ok(_) => assert_eq!(now.vm.cpus, replacement.vm.cpus),
        Err(error) => {
            assert_eq!(error.code(), Code::Aborted);
            assert_eq!(now.vm.cpus + 1, replacement.vm.cpus);
        }
    }
}
