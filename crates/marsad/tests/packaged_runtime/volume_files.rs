//! Qualify confined reads against real Docker in a disposable signed VZ guest.

use super::*;
use marsa_client::{
    DaemonEndpoint,
    docker::{ContainerFileFormat, DockerClient, VolumeDownloadError, VolumeSummary},
};

#[tokio::test]
#[ignore = "requires signed Apple silicon virtualization, production guest artifacts, and Docker CLI"]
async fn packaged_volume_downloads_stay_in_the_selected_volume() {
    assert!(!nix::unistd::geteuid().is_root());
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = Settings::default();
    settings.vm.memory = Some(1024 * MIB);
    settings.vm.idle_stop = false;
    let mut config = fs::File::create(paths.config_file()).unwrap();
    config
        .set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    config
        .write_all(settings.to_toml().unwrap().as_bytes())
        .unwrap();
    drop(config);

    let (daemon, _) = stage_bundle(scratch.path());
    let logs = scratch.path().join("daemon.log");
    let mut process = Daemon(
        Command::new(daemon)
            .env("HOME", &home)
            .stdout(fs::File::create(&logs).unwrap())
            .stderr(fs::File::options().append(true).open(&logs).unwrap())
            .spawn()
            .unwrap(),
    );
    let channel = connect(&mut process.0, &paths.daemon_socket(), &logs).await;
    let client = DaemonEndpoint::at(paths.daemon_socket())
        .connect()
        .await
        .unwrap();
    assert!(matches!(
        client.check_volume_provider().await,
        Err(VolumeDownloadError::Unavailable)
    ));
    let mut engine = EngineClient::new(channel);
    let ready = timeout(BOOT, engine.start(action(1)))
        .await
        .unwrap()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{}\n{}",
                fs::read_to_string(&logs).unwrap(),
                fs::read_to_string(paths.console_log()).unwrap_or_default()
            )
        })
        .into_inner();
    assert_eq!(ready.state(), EngineState::Ready);
    client.check_volume_provider().await.unwrap();

    let suffix = scratch
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_lowercase();
    let volume = format!("marsa-m751{suffix}");
    let image = format!("marsa-m751{suffix}:local");
    let container = format!("marsa-m751{suffix}-seed");
    let root = scratch.path().join("image");
    fs::create_dir_all(root.join("seed/sub")).unwrap();
    fs::write(root.join("seed/file.txt"), b"scoped fixture").unwrap();
    fs::write(root.join("seed/sub/clean.txt"), b"nested fixture").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", root.join("seed/escape")).unwrap();
    let archive = scratch.path().join("seed.tar");
    checked(
        Command::new("/usr/bin/tar")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(&root)
            .arg("seed"),
    );
    let docker_config = scratch.path().join("docker");
    fs::create_dir(&docker_config).unwrap();
    let host = format!("unix://{}", paths.docker_socket().display());
    let docker = |args: &[&str]| {
        let mut command = Command::new("docker");
        for (key, _) in
            env::vars_os().filter(|(key, _)| key.to_string_lossy().starts_with("DOCKER_"))
        {
            command.env_remove(key);
        }
        checked(
            command
                .arg("--config")
                .arg(&docker_config)
                .args(["--host", &host])
                .args(args),
        );
    };
    docker(&[
        "import",
        "--change",
        "VOLUME /seed",
        archive.to_str().unwrap(),
        &image,
    ]);
    docker(&["volume", "create", &volume]);
    let mount = format!("type=volume,source={volume},target=/seed");
    docker(&[
        "create",
        "--pull",
        "never",
        "--network",
        "none",
        "--name",
        &container,
        "--mount",
        &mount,
        &image,
        "/not-executed",
    ]);

    let inventory = DockerClient::at(paths.docker_socket());
    let selected = find_volume(&inventory, &volume).await;
    let abandoned = client.select_volume_download(&selected).await.unwrap();
    drop(abandoned);
    // A cancelled picker releases the guest session without even starting transfer.
    timeout(WAIT, client.check_volume_provider())
        .await
        .unwrap()
        .unwrap();

    for (single_file, expected, filename) in [
        (true, ContainerFileFormat::SingleFile, "scoped.txt"),
        (false, ContainerFileFormat::Tar, "scoped.tar"),
    ] {
        let prepared = prepare(&client, &selected, "/file.txt", single_file)
            .await
            .unwrap();
        assert!(prepared.format() == expected);
        let destination = scratch.path().join(filename);
        prepared.save(&destination).await.unwrap();
        if single_file {
            assert_eq!(fs::read(destination).unwrap(), b"scoped fixture");
        } else {
            let mut tar = tar::Archive::new(fs::File::open(destination).unwrap());
            let mut entries = tar.entries().unwrap();
            let mut entry = entries.next().unwrap().unwrap();
            assert_eq!(entry.path().unwrap(), Path::new("file.txt"));
            let mut output = Vec::new();
            entry.read_to_end(&mut output).unwrap();
            assert_eq!(output, b"scoped fixture");
            assert!(entries.next().is_none());
        }
    }
    let clean_directory = prepare(&client, &selected, "/sub", true).await.unwrap();
    assert!(clean_directory.format() == ContainerFileFormat::Tar);
    let directory_output = scratch.path().join("directory.tar");
    clean_directory.save(&directory_output).await.unwrap();
    let mut archive = tar::Archive::new(fs::File::open(directory_output).unwrap());
    let mut entries = archive.entries().unwrap();
    assert!(
        entries
            .next()
            .unwrap()
            .unwrap()
            .header()
            .entry_type()
            .is_dir()
    );
    let mut file = entries.next().unwrap().unwrap();
    assert_eq!(file.path().unwrap(), Path::new("sub/clean.txt"));
    let mut output = Vec::new();
    file.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"nested fixture");
    assert!(entries.next().is_none());

    let option_volume = format!("{volume}-options");
    docker(&[
        "volume",
        "create",
        "--opt",
        "type=tmpfs",
        "--opt",
        "device=tmpfs",
        &option_volume,
    ]);
    let unsupported = find_volume(&inventory, &option_volume).await;
    assert!(matches!(
        client.select_volume_download(&unsupported).await,
        Err(VolumeDownloadError::Unsupported)
    ));
    docker(&["volume", "rm", &option_volume]);

    assert!(matches!(
        prepare(&client, &selected, "/escape", true).await,
        Err(VolumeDownloadError::InvalidPath)
    ));
    let directory = prepare(&client, &selected, "/", true).await.unwrap();
    assert!(directory.format() == ContainerFileFormat::Tar);
    let destination = scratch.path().join("blocked.tar");
    assert!(directory.save(&destination).await.is_err());
    assert!(!destination.exists());

    assert!(matches!(
        prepare(&client, &selected, "/missing", true).await,
        Err(VolumeDownloadError::InvalidPath)
    ));
    let replacement = prepare(&client, &selected, "/file.txt", true)
        .await
        .unwrap();
    docker(&["rm", &container]);
    docker(&["volume", "rm", &volume]);
    docker(&["volume", "create", &volume]);
    let destination = scratch.path().join("replaced.txt");
    assert!(matches!(
        replacement.save(&destination).await,
        Err(VolumeDownloadError::Changed)
    ));
    assert!(!destination.exists());
    let fresh = find_volume(&inventory, &volume).await;
    let reviewed = client.select_volume_download(&fresh).await.unwrap();
    docker(&["volume", "rm", &volume]);
    docker(&["volume", "create", &volume]);
    assert!(matches!(
        reviewed.prepare("/".into(), false).await,
        Err(VolumeDownloadError::Changed)
    ));
    docker(&["volume", "rm", &volume]);
    docker(&["image", "rm", &image]);
    assert_eq!(
        engine.stop(action(2)).await.unwrap().into_inner().state(),
        EngineState::Stopped
    );
}

async fn find_volume(engine: &DockerClient, name: &str) -> VolumeSummary {
    engine
        .volumes()
        .await
        .unwrap()
        .into_iter()
        .find(|volume| volume.name == name)
        .unwrap()
}

async fn prepare(
    client: &marsa_client::MarsaClient,
    volume: &VolumeSummary,
    path: &str,
    prefer_single_file: bool,
) -> Result<marsa_client::docker::VolumeDownload, VolumeDownloadError> {
    client
        .select_volume_download(volume)
        .await?
        .prepare(path.into(), prefer_single_file)
        .await
}
