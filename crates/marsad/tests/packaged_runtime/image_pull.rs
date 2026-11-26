//! Exercise the desktop pull stream and its Keep and Remove choices on the packaged engine.

use super::*;
use marsa_client::docker::{DockerClient, ImageReference, ImageRemoval, RemovalMode};

#[tokio::test]
#[ignore = "requires signed Apple silicon virtualization, the packaged guest and registry access"]
async fn desktop_pull_resolves_the_reported_manifest_before_removal() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    let (daemon, _) = stage_bundle(temp.path());
    let log = temp.path().join("daemon.log");
    let mut process = recovery::spawn(&daemon, &home, &log);
    let channel = connect(&mut process.0, &paths.daemon_socket(), &log).await;
    recovery::start(&channel, &paths, 1).await;
    let docker = DockerClient::at(paths.docker_socket());
    let reference = ImageReference::parse("busybox:1.37").unwrap();
    let mut pull = docker.pull_image(&reference).await.unwrap();
    let mut digest = None;
    while let Some(progress) = pull.next().await.unwrap() {
        if let Some(value) = progress
            .status
            .as_deref()
            .and_then(|s| s.strip_prefix("Digest: "))
        {
            digest = Some(value.to_owned());
        }
    }
    let digest = digest.expect("pull reported the registry digest");
    let id = docker.pulled_image_id(&reference, &digest).await.unwrap();
    assert!(
        docker
            .inspect_image(&id)
            .await
            .unwrap()
            .repo_tags
            .iter()
            .any(|tag| tag == reference.as_str())
    );
    docker
        .remove_image(ImageRemoval::confirmed(
            id.clone(),
            RemovalMode::StoppedOnly,
        ))
        .await
        .unwrap();
    assert!(
        docker
            .images()
            .await
            .unwrap()
            .iter()
            .all(|image| image.id != id.as_str())
    );
    recovery::signal_running_daemon(&mut process).await;
}
