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
        r#"{{"name":"docker","status":"Stopped","dir":"/Users/owner/.lima/docker","vmType":"vz","arch":"aarch64","cpus":2,"memory":4294967296,"disk":21474836480,"config":{{"networks":{network},"networkOpts":{{"hostLoopback":true}},"portForwards":[{{"guestSocket":"/run/user/501/docker.sock","hostSocket":"{docker_forward}"}}]}}}}"#
    )
    .into_bytes()
}

#[test]
fn effective_config_must_bind_the_selected_docker_socket() {
    let socket = Path::new("/Users/owner/.lima/docker/sock/docker.sock");
    let mut bytes = fixture(socket.to_str().unwrap(), "[]");
    bytes.push(b'\n');
    let settings = decode_settings(&bytes, "docker", socket, true).unwrap();
    assert_eq!(settings.backend, "Virtualization.framework");
    assert_eq!(settings.architecture, Some("aarch64"));
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
        backend: "Virtualization.framework",
        architecture: Some("aarch64"),
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

#[test]
fn lifecycle_commands_use_one_validated_instance_and_graceful_stop() {
    let root = PathBuf::from("/Users/owner/.lima");
    let instance = LimaInstance {
        socket: root.join("docker/sock/docker.sock"),
        root,
        name: "docker".into(),
    };
    let stopped = decode_settings(
        &fixture(instance.socket.to_str().unwrap(), "[]"),
        &instance.name,
        &instance.socket,
        true,
    )
    .unwrap();
    let running = LimaSettings {
        stopped: false,
        running: true,
        ..stopped
    };
    assert_eq!(
        lifecycle_arguments(stopped, true).unwrap(),
        Some(&["start", "--timeout=180s"][..]),
    );
    assert_eq!(
        lifecycle_arguments(running, false).unwrap(),
        Some(&["stop"][..]),
    );
    assert!(lifecycle_arguments(running, true).unwrap().is_none());
    assert!(lifecycle_arguments(stopped, false).unwrap().is_none());
    assert!(matches!(
        lifecycle_arguments(
            LimaSettings {
                stopped: false,
                ..stopped
            },
            true,
        ),
        Err(LimaSettingsError::LifecycleState)
    ));
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "requires a disposable Lima VM on a macOS virtualization runner"]
async fn lima_changes_allocation_between_a_graceful_stop_and_start() {
    let socket = PathBuf::from(
        env::var_os("MARSA_DISPOSABLE_LIMA_DOCKER_SOCKET")
            .expect("set this to the configured Docker forward of a disposable Lima instance"),
    );
    let instance = LimaInstance::from_socket(&socket).unwrap();
    assert!(instance.root.starts_with("/private/tmp"));
    assert_eq!(
        env::var_os("LIMA_HOME").as_deref(),
        Some(instance.root.as_os_str())
    );

    let stopped = instance.set_running(false).await.unwrap();
    assert!(stopped.stopped);
    let cpus = if stopped.cpus == 1 { 2 } else { 1 };
    let updated = instance
        .update(
            stopped,
            LimaSettingsUpdate {
                cpus: Some(cpus),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.cpus, cpus);
    let running = instance.set_running(true).await.unwrap();
    assert!(running.running);
    assert_eq!(running.cpus, cpus);
    let restarted = instance.restart().await.unwrap();
    assert!(restarted.running);
    assert_eq!(restarted.cpus, cpus);
    let stopped_again = instance.set_running(false).await.unwrap();
    assert!(stopped_again.stopped);
    assert!(matches!(
        instance.restart().await,
        Err(LimaSettingsError::LifecycleState)
    ));
}
