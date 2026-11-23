use super::*;

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn only_exact_lima_docker_socket_paths_identify_instances() {
    let root = Path::new("/Users/owner/.lima");
    for name in ["docker", "Docker.2", "docker-test_3"] {
        let socket = root.join(name).join("sock/docker.sock");
        assert_eq!(match_socket(&socket, root), Some(name));
    }
    for path in [
        "../docker/sock/docker.sock",
        "-docker/sock/docker.sock",
        "test.yaml/sock/docker.sock",
        "test..one/sock/docker.sock",
        "docker/nested/sock/docker.sock",
        "docker/sock/custom.sock",
        "docker/other/docker.sock",
    ] {
        assert_eq!(match_socket(&root.join(path), root), None, "{path}");
    }
}

fn fixture(docker_forward: &str, network: &str) -> Vec<u8> {
    format!(
        r#"{{"name":"docker","status":"Stopped","dir":"/Users/owner/.lima/docker","vmType":"vz","cpus":2,"memory":4294967296,"disk":21474836480,"config":{{"networks":{network},"networkOpts":{{"hostLoopback":true}},"portForwards":[{{"guestSocket":"/run/user/501/docker.sock","hostSocket":"{docker_forward}"}}]}}}}"#
    )
    .into_bytes()
}

#[test]
fn effective_config_must_bind_the_selected_docker_socket() {
    let socket = Path::new("/Users/owner/.lima/docker/sock/docker.sock");
    let mut bytes = fixture(socket.to_str().unwrap(), "[]");
    bytes.push(b'\n');
    let settings = decode_settings(&bytes, "docker", socket, true).unwrap();
    assert_eq!(settings.cpus, 2);
    assert_eq!(settings.memory_bytes, 4 * GIB);
    assert_eq!(settings.disk_bytes, 20 * GIB);
    assert!(settings.stopped);
    assert_eq!(settings.host_loopback, Some(true));

    let bytes = fixture("/Users/owner/.lima/other/sock/docker.sock", "[]");
    assert!(matches!(
        decode_settings(&bytes, "docker", socket, true),
        Err(LimaSettingsError::UnsafeInstance)
    ));
    assert!(matches!(
        decode_settings(
            &fixture(socket.to_str().unwrap(), "[]"),
            "other",
            socket,
            true
        ),
        Err(LimaSettingsError::UnsafeInstance)
    ));
}

#[test]
fn loopback_capability_is_restricted_to_supported_networks() {
    let socket = Path::new("/Users/owner/.lima/docker/sock/docker.sock");
    let bytes = fixture(socket.to_str().unwrap(), r#"[{"lima":"user-v2"}]"#);
    let settings = decode_settings(&bytes, "docker", socket, true).unwrap();
    assert_eq!(settings.host_loopback, None);

    let bytes = fixture(socket.to_str().unwrap(), "[]");
    let settings = decode_settings(&bytes, "docker", socket, false).unwrap();
    assert_eq!(settings.host_loopback, None);
}

#[test]
fn edits_reject_running_guests_stale_settings_and_disk_shrink() {
    let current = LimaSettings {
        cpus: 2,
        memory_bytes: 4 * GIB,
        disk_bytes: 20 * GIB,
        stopped: true,
        running: false,
        host_loopback: Some(true),
    };
    let update = LimaSettingsUpdate {
        disk_bytes: Some(19 * GIB),
        ..Default::default()
    };
    assert!(matches!(
        edit_values(current, current, update),
        Err(LimaSettingsError::DiskShrink)
    ));
    assert!(matches!(
        edit_values(
            LimaSettings {
                stopped: false,
                ..current
            },
            current,
            update
        ),
        Err(LimaSettingsError::RequiresStopped)
    ));
    assert!(matches!(
        edit_values(current, LimaSettings { cpus: 3, ..current }, update),
        Err(LimaSettingsError::Changed)
    ));
    assert_eq!(
        edit_values(current, current, LimaSettingsUpdate::default()).unwrap(),
        current
    );
}
