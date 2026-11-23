//! Tests the daemon's on-disk configuration contract.

use marsa_core::error::SettingsError;
use marsa_core::settings::{NetworkMode, Settings, VmBackend};
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn new_settings_have_safe_defaults() {
    let settings = Settings::default();
    assert_eq!(settings.vm.backend, VmBackend::Vz);
    assert!(settings.vm.idle_stop);
    assert_eq!(settings.network.mode, NetworkMode::Auto);
    assert!(!settings.network.proxy.inject_into_containers);
    assert!(settings.shares.is_empty());
    assert!(!settings.agents.advanced_access);
    assert!(!settings.updates.auto_install);
    assert!(!settings.telemetry.enabled);
    settings.validate().unwrap();
}

#[test]
fn automatic_memory_is_proportional_and_capped() {
    let mut settings = Settings::default();
    assert_eq!(settings.vm.memory_bytes(8 * GIB), 4 * GIB);
    assert_eq!(settings.vm.memory_bytes(32 * GIB), 8 * GIB);
    settings.vm.memory = Some(2 * GIB);
    assert_eq!(settings.vm.memory_bytes(32 * GIB), 2 * GIB);
}

#[test]
fn customized_settings_survive_serialization() {
    let settings = Settings::from_toml(
        r#"
schema_version = 1
shares = ["/Users/example/project"]

[vm]
cpus = 4
memory = 2147483648

[network]
dns = ["192.0.2.10", "2001:db8::53"]

[network.proxy]
http = "http://[2001:db8::10]:8080"

[ui]
language = "ar"
theme = "dark"
"#,
    )
    .unwrap();

    assert_eq!(settings.vm.cpus, 4);
    assert_eq!(settings.vm.memory_bytes(32 * GIB), 2 * GIB);
    assert_eq!(settings.shares[0], Path::new("/Users/example/project"));
    assert_eq!(
        settings.network.dns[0],
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10))
    );
    assert_eq!(
        Settings::from_toml(&settings.to_toml().unwrap()).unwrap(),
        settings
    );
}

#[test]
fn unversioned_settings_migrate_without_losing_overrides() {
    let settings = Settings::from_toml("[vm]\ncpus = 3\nidle_timeout = 120\n").unwrap();

    assert_eq!(settings.vm.cpus, 3);
    assert_eq!(settings.vm.idle_timeout, 120);
    assert!(settings.vm.idle_stop);
    let current_document = settings.to_toml().unwrap();
    assert!(current_document.contains("schema_version = 1"));
    assert_eq!(Settings::from_toml(&current_document).unwrap(), settings);
}

#[test]
fn future_schema_is_rejected_before_decoding_unknown_fields() {
    assert!(matches!(
        Settings::from_toml("schema_version = 2\nfuture_option = 42"),
        Err(SettingsError::UnsupportedVersion(2))
    ));
}

#[test]
fn invalid_values_identify_the_field() {
    for (document, expected_field) in [
        ("[vm]\ncpus = 0", "vm.cpus"),
        ("[vm]\nmemory = 512", "vm.memory"),
        ("[vm]\nidle_timeout = 0", "vm.idle_timeout"),
        ("[disk]\ndata_size_limit = 0", "disk.data_size_limit"),
        ("shares = ['relative/project']", "shares"),
        ("shares = ['/Users/example/../private']", "shares"),
        ("[network.proxy]\nhttp = 'ftp://host'", "network.proxy.http"),
        (
            "[network.proxy]\nhttps = 'https://host:99999'",
            "network.proxy.https",
        ),
        (
            "[network.proxy]\nhttp = 'http://host/#secret'",
            "network.proxy.http",
        ),
        ("[telemetry]\nenabled = true", "telemetry.enabled"),
    ] {
        let error = Settings::from_toml(document).unwrap_err();
        assert!(
            matches!(error, SettingsError::InvalidValue { field, .. } if field == expected_field),
            "{document}: {error}"
        );
        assert!(error.to_string().contains(expected_field));
    }
}

#[test]
fn serializing_an_invalid_update_fails() {
    let mut settings = Settings::default();
    settings.vm.cpus = 0;
    assert!(matches!(
        settings.to_toml(),
        Err(SettingsError::InvalidValue {
            field: "vm.cpus",
            ..
        })
    ));
}

#[test]
fn errors_keep_original_config_values_private() {
    for document in [
        "[network.proxy]\nhttps = 'https://name:sensitive-marker@proxy.example'",
        "[ui]\nlanguage = 'sensitive-marker'",
        "[[broken\nsensitive-marker",
    ] {
        let error = Settings::from_toml(document).unwrap_err();
        assert!(!error.to_string().contains("sensitive-marker"));
        assert!(!format!("{error:?}").contains("sensitive-marker"));
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_share_paths_are_rejected_before_serialization() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let mut settings = Settings::default();
    settings
        .shares
        .push(OsString::from_vec(b"/tmp/non-utf8-\xff".to_vec()).into());

    assert!(matches!(
        settings.validate(),
        Err(SettingsError::InvalidValue {
            field: "shares",
            ..
        })
    ));
    assert!(matches!(
        settings.to_toml(),
        Err(SettingsError::InvalidValue {
            field: "shares",
            ..
        })
    ));
}

#[test]
fn compose_template_round_trips_and_can_be_restored() {
    let mut settings = Settings::from_toml("schema_version = 1\n").unwrap();
    assert_eq!(settings.ui.compose_template, None);
    let source = "services:\n  app:\n    image: nginx:alpine\n# قالب المشروع\n";
    settings.ui.compose_template = Some(source.to_owned());
    let mut reloaded = Settings::from_toml(&settings.to_toml().unwrap()).unwrap();
    assert_eq!(reloaded.ui.compose_template.as_deref(), Some(source));
    reloaded.ui.compose_template = None;
    assert!(!reloaded.to_toml().unwrap().contains("compose_template"));
}

#[test]
fn compose_template_rejects_blank_oversized_and_nul_input() {
    let mut settings = Settings::default();
    for invalid in ["  \n".to_owned(), "a\0b".to_owned(), "a".repeat(16_385)] {
        settings.ui.compose_template = Some(invalid);
        assert!(matches!(
            settings.to_toml(),
            Err(SettingsError::InvalidValue {
                field: "ui.compose_template",
                ..
            })
        ));
    }
}

#[test]
fn display_preferences_are_optional_and_round_trip() {
    let legacy = Settings::from_toml("[ui]\nlanguage = 'ar'").unwrap();
    assert!(!legacy.to_toml().unwrap().contains("[ui.display]"));

    let customized = Settings::from_toml(
        "[ui.display]\nui_font = 'system'\nui_font_size = 'large'\nterminal_font = 'menlo'\ntime_format = 'twelve_hour'",
    )
    .unwrap();
    let saved = customized.to_toml().unwrap();
    assert!(saved.contains("time_format = \"twelve_hour\""));
    assert_eq!(Settings::from_toml(&saved).unwrap(), customized);
}
