//! Boot real previous-format data and verify a newer disk stays unchanged.

use super::*;
use recovery::{docker, offline_image, signal_running_daemon, spawn, start};
use support::stage_bundle_from;

const IMAGE: &str = "marsa-schema-test";
const CONTAINER: &str = "marsa-schema-container";
const VOLUME: &str = "marsa-schema-volume";

#[tokio::test]
#[ignore = "requires signed guest artifacts from the previous and current builds"]
async fn packaged_guest_upgrade_preserves_images_containers_and_volumes() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let root = scratch.path();
    let previous = env::var_os("MARSA_PREVIOUS_VZ_GUEST_DIR").expect("archived previous guest");
    let (old_daemon, _) = stage_bundle_from(&root.join("previous"), Path::new(&previous));
    let (current_daemon, _) = stage_bundle(&root.join("current"));
    let (home, paths) = private_home(root);
    let socket = paths.docker_socket();
    let mut old = launch(&old_daemon, &home, &paths, &root.join("previous.log")).await;
    replace_settings(&old.1, &settings(), 1).await;
    start(&old.1, &paths, 2).await;
    let fixture = offline_image(root);
    docker(
        &socket,
        &[
            "import",
            "--platform",
            "linux/arm64",
            fixture.to_str().unwrap(),
            IMAGE,
        ],
    )
    .await;
    docker(
        &socket,
        &[
            "run",
            "--pull=never",
            "--network=none",
            "--name",
            CONTAINER,
            "-v",
            &format!("{VOLUME}:/proof"),
            IMAGE,
            "/bin/busybox",
            "sh",
            "-c",
            "echo previous-data > /proof/marker",
        ],
    )
    .await;
    let image = docker(&socket, &["image", "inspect", "--format={{.Id}}", IMAGE]).await;
    let container = docker(
        &socket,
        &["container", "inspect", "--format={{.Id}}", CONTAINER],
    )
    .await;
    let identity = disk_identity(&paths.data_disk());
    signal_running_daemon(&mut old.0).await;
    drop(old);

    let mut current = launch(&current_daemon, &home, &paths, &root.join("current.log")).await;
    start(&current.1, &paths, 3).await;
    assert_eq!(disk_identity(&paths.data_disk()), identity);
    assert_eq!(
        docker(&socket, &["image", "inspect", "--format={{.Id}}", IMAGE]).await,
        image
    );
    assert_eq!(
        docker(
            &socket,
            &["container", "inspect", "--format={{.Id}}", CONTAINER]
        )
        .await,
        container
    );
    assert_eq!(
        docker(
            &socket,
            &["volume", "inspect", "--format={{.Name}}", VOLUME]
        )
        .await
        .trim(),
        VOLUME
    );
    assert_eq!(
        docker(
            &socket,
            &[
                "run",
                "--rm",
                "--pull=never",
                "--network=none",
                "-v",
                &format!("{VOLUME}:/proof"),
                IMAGE,
                "/bin/busybox",
                "cat",
                "/proof/marker",
            ]
        )
        .await
        .trim(),
        "previous-data"
    );
    signal_running_daemon(&mut current.0).await;
}

#[tokio::test]
#[ignore = "requires a signed guest and an offline ext4 fixture with schema 2"]
async fn packaged_guest_reports_newer_schema_without_writing_the_disk() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let root = scratch.path();
    let (home, paths) = private_home(root);
    let (daemon, _) = stage_bundle(root);
    let mut stage = launch(&daemon, &home, &paths, &root.join("refusal.log")).await;
    replace_settings(&stage.1, &settings(), 1).await;
    let future = PathBuf::from(env::var_os("MARSA_FUTURE_DISK_FIXTURE").expect("schema-2 fixture"));
    assert_eq!(future.metadata().unwrap().len(), GIB);
    let disk = paths.data_disk();
    let directory = disk.parent().unwrap();
    fs::create_dir_all(directory).unwrap();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    checked(
        Command::new("/bin/cp")
            .arg("-c")
            .arg(&future)
            .arg(paths.data_disk()),
    );
    fs::set_permissions(paths.data_disk(), fs::Permissions::from_mode(0o600)).unwrap();
    let prior = disk_identity(&paths.data_disk());

    let mut engine = EngineClient::new(stage.1.clone());
    let refusal = timeout(BOOT, engine.start(action(2)))
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        refusal
            .message()
            .contains("Data disk schema 2 requires a newer release"),
        "{refusal:?}"
    );
    let status = engine
        .status(EngineStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        status.boot_stage.as_deref(),
        Some("data_disk"),
        "{status:?}"
    );
    assert!(
        status
            .console_tail
            .unwrap()
            .contains("MARSA_BOOT_ERROR disk-schema 2 1")
    );
    signal_running_daemon(&mut stage.0).await;
    assert_eq!(disk_identity(&paths.data_disk()), prior);
    assert_sparse_image_equal(&future, &paths.data_disk());
}

fn private_home(root: &Path) -> (PathBuf, MarsaPaths) {
    let home = root.join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    (home, paths)
}

fn settings() -> Settings {
    let mut config = Settings::default();
    config.disk.data_size_limit = GIB;
    config.vm.memory = Some(1024 * MIB);
    config.vm.idle_stop = false;
    config.network.mode = marsa_core::settings::NetworkMode::Userspace;
    config
}

async fn launch(daemon: &Path, home: &Path, paths: &MarsaPaths, log: &Path) -> (Daemon, Channel) {
    let mut process = spawn(daemon, home, log);
    let channel = connect(&mut process.0, &paths.daemon_socket(), log).await;
    (process, channel)
}
