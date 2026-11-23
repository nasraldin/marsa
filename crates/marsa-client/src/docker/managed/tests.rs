use std::os::unix::fs::PermissionsExt;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};

use super::*;

fn unix(name: &str, socket: PathBuf) -> ManagedEndpointConfig {
    ManagedEndpointConfig {
        name: name.into(),
        transport: ManagedEndpointTransport::Unix { socket },
    }
}

#[tokio::test]
async fn saved_edits_keep_identity_across_reopen_and_removal() {
    let scratch = tempfile::tempdir().unwrap();
    let store = ManagedEndpointStore::at(scratch.path().to_path_buf());
    let added = store
        .add(unix("Work", scratch.path().join("one.sock")))
        .await
        .unwrap();
    assert!(added.id.starts_with("managed:"));
    let reopened = ManagedEndpointStore::at(scratch.path().to_path_buf());
    assert_eq!(reopened.list().await.unwrap(), std::slice::from_ref(&added));

    let edited = reopened
        .edit(&added.id, unix("Team", scratch.path().join("two.sock")))
        .await
        .unwrap();
    assert_eq!(edited.id, added.id);
    assert_eq!(store.list().await.unwrap(), std::slice::from_ref(&edited));
    assert!(matches!(
        store.remove("work-context").await,
        Err(ManagedEndpointError::NotFound)
    ));
    store.remove(&edited.id).await.unwrap();
    assert!(reopened.list().await.unwrap().is_empty());
    assert!(matches!(
        reopened.edit(&edited.id, edited.config).await,
        Err(ManagedEndpointError::NotFound)
    ));
}

#[tokio::test]
async fn malformed_addresses_cannot_be_saved_or_tested() {
    let scratch = tempfile::tempdir().unwrap();
    let store = ManagedEndpointStore::at(scratch.path().to_path_buf());
    for transport in [
        ManagedEndpointTransport::Unix {
            socket: "relative.sock".into(),
        },
        ManagedEndpointTransport::Ssh {
            address: "ssh://user:password@server.example".into(),
        },
        ManagedEndpointTransport::Tls {
            address: "tcp://engine.example:0".into(),
            ca: "/tmp/ca.pem".into(),
            cert: "/tmp/cert.pem".into(),
            key: "/tmp/key.pem".into(),
        },
    ] {
        let config = ManagedEndpointConfig {
            name: "Work".into(),
            transport,
        };
        assert!(matches!(
            config.test().await,
            Err(ManagedEndpointError::InvalidAddress)
        ));
        assert!(matches!(
            store.add(config).await,
            Err(ManagedEndpointError::InvalidAddress)
        ));
    }
    assert!(store.list().await.unwrap().is_empty());
}

fn unix_server(path: &Path, mode: u32, payload: &'static str) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(stream.read_u8().await.unwrap());
        }
        assert!(request.starts_with(b"GET /info HTTP/1.1\r\n"));
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    })
}

#[tokio::test]
async fn testing_reports_authenticated_docker_identity() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let socket = scratch.path().join("e.sock");
    let server = unix_server(
        &socket,
        0o600,
        r#"{"ID":"engine-one","Name":"workhost","ServerVersion":"29.10.0"}"#,
    );
    let verified = unix("Work", socket).test().await.unwrap();
    assert_eq!(verified.id, "engine-one");
    assert_eq!(verified.name, "workhost");
    assert_eq!(verified.version, "29.10.0");
    server.await.unwrap();
}

#[tokio::test]
async fn testing_rejects_a_docker_socket_accessible_to_other_users() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let socket = scratch.path().join("e.sock");
    let server = unix_server(
        &socket,
        0o666,
        r#"{"ID":"engine-one","Name":"workhost","ServerVersion":"29.10.0"}"#,
    );
    assert!(matches!(
        unix("Work", socket).test().await,
        Err(ManagedEndpointError::Connection(DockerError::Socket(error)))
            if error.kind() == io::ErrorKind::PermissionDenied
    ));
    assert!(!server.is_finished());
    server.abort();
}

#[tokio::test]
async fn a_captured_connection_keeps_its_target_after_edit_and_removal() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let store = ManagedEndpointStore::at(scratch.path().to_path_buf());
    let one = scratch.path().join("first.sock");
    let two = scratch.path().join("second.sock");
    let original = unix_server(
        &one,
        0o600,
        r#"{"ID":"engine-one","Name":"first","ServerVersion":"29.10.0"}"#,
    );
    let replacement = unix_server(
        &two,
        0o600,
        r#"{"ID":"engine-two","Name":"second","ServerVersion":"29.10.0"}"#,
    );
    let saved = store.add(unix("Work", one)).await.unwrap();
    let pinned = ExternalEndpoint::managed(
        saved.id.clone(),
        saved.config.capture_transport().await.unwrap(),
    );
    let updated = store.edit(&saved.id, unix("Work", two)).await.unwrap();
    assert_eq!(updated.config.test().await.unwrap().id, "engine-two");
    store.remove(&saved.id).await.unwrap();
    assert_eq!(pinned.verify_engine().await.unwrap().id, "engine-one");
    original.await.unwrap();
    replacement.await.unwrap();
}

#[tokio::test]
async fn simultaneous_writers_preserve_both_managed_endpoints() {
    let scratch = tempfile::tempdir().unwrap();
    let store = ManagedEndpointStore::at(scratch.path().to_path_buf());
    let other = store.clone();
    let (first, second) = tokio::join!(
        store.add(unix("Work", scratch.path().join("first.sock"))),
        other.add(unix("Home", scratch.path().join("second.sock"))),
    );
    let records = store.list().await.unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.contains(&first.unwrap()));
    assert!(records.contains(&second.unwrap()));
}

#[tokio::test]
async fn private_store_refuses_a_symbolic_link_in_place_of_settings() {
    let scratch = tempfile::tempdir().unwrap();
    let store = ManagedEndpointStore::at(scratch.path().to_path_buf());
    let unrelated = scratch.path().join("unrelated.json");
    fs::write(&unrelated, "[]").await.unwrap();
    std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&unrelated, scratch.path().join(STORE_FILE)).unwrap();
    assert!(matches!(
        store.list().await,
        Err(ManagedEndpointError::Store(_))
    ));
    assert!(matches!(
        store
            .add(unix("Work", scratch.path().join("docker.sock")))
            .await,
        Err(ManagedEndpointError::Store(_))
    ));
    assert_eq!(fs::read_to_string(unrelated).await.unwrap(), "[]");
}

#[tokio::test]
async fn test_rejects_invalid_identity_even_after_authenticating() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let socket = scratch.path().join("e.sock");
    let server = unix_server(
        &socket,
        0o600,
        r#"{"ID":" ","Name":"workhost","ServerVersion":"29.10.0"}"#,
    );
    assert!(matches!(
        unix("Work", socket).test().await,
        Err(ManagedEndpointError::InvalidEngineIdentity)
    ));
    server.await.unwrap();
}
