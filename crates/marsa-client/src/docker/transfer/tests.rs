use std::os::unix::fs::PermissionsExt;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

use super::*;

const ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn push_routes_require_one_tag_and_exclude_path_traversal() {
    for (image, path, registry) in [
        (
            "org/app:Tag",
            "/images/org/app/push?tag=Tag",
            "https://index.docker.io/v1/",
        ),
        (
            "localhost:5000/a/b:v2",
            "/images/localhost:5000/a/b/push?tag=v2",
            "localhost:5000",
        ),
        (
            "[2001:db8::1]:5000/app:v2",
            "/images/[2001:db8::1]:5000/app/push?tag=v2",
            "[2001:db8::1]:5000",
        ),
    ] {
        let reference = ImageReference::parse(image).unwrap();
        let push = PushTarget::parse(&reference).unwrap();
        assert_eq!(push.path(), path);
        assert_eq!(push.registry, registry);
        assert!(external_transfer_route(&Method::POST, path, false));
    }

    for image in [
        "org/app",
        "org/../app:latest",
        "//evil:tag",
        "org/app:tag@sha256:00",
        "host:/app:tag",
    ] {
        assert!(
            PushTarget::parse(&ImageReference::parse(image).unwrap()).is_err(),
            "{image}"
        );
    }
    for path in [
        "/images/a/../app/push?tag=latest",
        "/images/a%2Fapp/push?tag=latest",
        "/images/app/push",
        "/images/app/push?tag=latest&all=true",
        "/networks/short/disconnect",
        "/images/load?quiet=false",
    ] {
        assert!(
            !external_transfer_route(&Method::POST, path, false),
            "{path}"
        );
        assert!(
            !external_transfer_route(&Method::POST, path, true),
            "{path}"
        );
    }
}

#[tokio::test]
async fn truncated_export_does_not_publish_an_archive() {
    let (directory, listener, client) = endpoint();
    let destination = directory.path().join("image.tar");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = request_headers(&mut socket).await;
        assert!(request.starts_with(&format!("GET /v1.57/images/sha256:{ID}/get HTTP/1.1")));
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: 20\r\n\r\npartial").await.unwrap();
    });
    let image = ImageId::parse(format!("sha256:{ID}")).unwrap();
    assert!(client.export_image(&image, &destination).await.is_err());
    server.await.unwrap();
    assert!(!destination.exists());
    assert_eq!(directory.path().read_dir().unwrap().count(), 1);
}

#[tokio::test]
async fn oversized_export_does_not_publish_an_archive() {
    let (directory, listener, client) = endpoint();
    let destination = directory.path().join("image.tar");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request_headers(&mut socket).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n1234\r\n4\r\n5678\r\n0\r\n\r\n")
            .await
            .unwrap();
    });
    let image = ImageId::parse(format!("sha256:{ID}")).unwrap();
    assert!(matches!(
        client
            .export_image_with_limit(&image, &destination, 5)
            .await,
        Err(DockerError::ImageArchive(_))
    ));
    server.await.unwrap();
    assert!(!destination.exists());
    assert_eq!(directory.path().read_dir().unwrap().count(), 1);
}

#[tokio::test]
async fn complete_export_publishes_an_owner_only_archive() {
    let (directory, listener, client) = endpoint();
    let destination = directory.path().join("image.tar");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request_headers(&mut socket).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: 7\r\n\r\narchive").await.unwrap();
    });
    let image = ImageId::parse(format!("sha256:{ID}")).unwrap();
    client.export_image(&image, &destination).await.unwrap();
    server.await.unwrap();
    assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"archive");
    assert_eq!(
        destination.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(directory.path().read_dir().unwrap().count(), 2);
}

#[tokio::test]
async fn successful_export_preserves_existing_destination() {
    let (directory, listener, client) = endpoint();
    let destination = directory.path().join("image.tar");
    tokio::fs::write(&destination, b"existing").await.unwrap();
    let server = tokio::spawn(async move {
        if let Ok(Ok((mut socket, _))) =
            timeout(Duration::from_millis(200), listener.accept()).await
        {
            request_headers(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: 7\r\n\r\narchive").await.unwrap();
        }
    });
    let image = ImageId::parse(format!("sha256:{ID}")).unwrap();
    assert!(matches!(
        client.export_image(&image, &destination).await,
        Err(DockerError::ImageArchive(_))
    ));
    server.await.unwrap();
    assert_eq!(tokio::fs::read(destination).await.unwrap(), b"existing");
    assert_eq!(directory.path().read_dir().unwrap().count(), 2);
}

#[tokio::test]
async fn image_load_streams_the_archive_and_rejects_engine_errors() {
    let (directory, listener, client) = endpoint();
    let archive = directory.path().join("image.tar");
    let contents = vec![0xAB; CHUNK_SIZE * 3];
    tokio::fs::write(&archive, &contents).await.unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let headers = request_headers(&mut socket).await.to_ascii_lowercase();
        assert!(headers.starts_with("post /v1.57/images/load?quiet=true http/1.1"));
        assert!(headers.contains("content-type: application/x-tar\r\n"));
        assert!(headers.contains(&format!("content-length: {}\r\n", contents.len())));
        let mut received = vec![0; contents.len()];
        socket.read_exact(&mut received).await.unwrap();
        assert_eq!(received, contents);
        socket.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"error\":\"archive is invalid\"}\r\n").await.unwrap();
    });
    assert!(matches!(
        client.import_image(&archive).await,
        Err(DockerError::ImageLoad)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn disconnect_uses_full_ids_without_forcing() {
    let (_directory, listener, client) = endpoint();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let headers = request_headers(&mut socket).await;
        assert!(headers.starts_with(&format!("POST /v1.57/networks/{ID}/disconnect HTTP/1.1")));
        let expected = format!(r#"{{"Container":"{ID}","Force":false}}"#);
        let mut payload = vec![0; expected.len()];
        socket.read_exact(&mut payload).await.unwrap();
        assert_eq!(payload, expected.as_bytes());
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });
    client
        .disconnect_network(
            &NetworkId::parse(ID).unwrap(),
            &ContainerId::parse(ID).unwrap(),
        )
        .await
        .unwrap();
    server.await.unwrap();
}

fn endpoint() -> (tempfile::TempDir, UnixListener, DockerClient) {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let path = directory.path().join("docker.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    (directory, listener, DockerClient::at(path))
}

async fn request_headers(socket: &mut UnixStream) -> String {
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.push(socket.read_u8().await.unwrap());
    }
    String::from_utf8(request).unwrap()
}
