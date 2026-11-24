use std::os::unix::fs::PermissionsExt;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};

use super::{ContainerId, DockerClient, DockerError, LogSource};

const ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn container_paths_only_accept_full_ids() {
    assert!(ContainerId::parse(ID).is_ok());
    assert!(matches!(
        ContainerId::parse("../evil"),
        Err(DockerError::InvalidId)
    ));
}

#[tokio::test]
async fn docker_logs_preserve_source_and_trailing_fragments() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let path = directory.path().join("docker.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        assert!(
            String::from_utf8(request)
                .unwrap()
                .starts_with(&format!(
                    "GET /v1.57/containers/{ID}/logs?stdout=true&stderr=true&follow=true&tail=200&timestamps=true "
                ))
        );
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/vnd.docker.multiplexed-stream\r\nConnection: close\r\n\r\n").await.unwrap();
        let stdout = b"2026-11-23T08:12:03.123456789Z a\n2026-99-23T08:12:03Z tail";
        socket
            .write_all(&[1, 0, 0, 0, 0, 0, 0, stdout.len() as u8])
            .await
            .unwrap();
        socket.write_all(stdout).await.unwrap();
        socket
            .write_all(&[2, 0, 0, 0, 0, 0, 0, 2, b'b', b'\n'])
            .await
            .unwrap();
    });
    let client = DockerClient::at(path);
    let mut logs = client.logs(&ContainerId::parse(ID).unwrap()).await.unwrap();
    let stdout = logs.next().await.unwrap().unwrap();
    assert_eq!(stdout.text, "a");
    assert_eq!(
        stdout.timestamp.unwrap().format(false),
        "2026-11-23T08:12:03.123456789Z"
    );
    let stderr = logs.next().await.unwrap().unwrap();
    assert_eq!(stderr.source, LogSource::Stderr);
    assert_eq!(stderr.text, "b");
    assert!(stderr.timestamp.is_none());
    let trailing = logs.next().await.unwrap().unwrap();
    assert_eq!(trailing.text, "2026-99-23T08:12:03Z tail");
    assert!(trailing.timestamp.is_none());
    assert!(logs.next().await.unwrap().is_none());
    server.await.unwrap();
}
