//! Exercise the relay with native client sockets and a simulated Docker engine.

#![cfg(unix)]

use std::{
    future::pending,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use nix::unistd::geteuid;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};

const WAIT: Duration = Duration::from_secs(5);

const STARTUP: marsa_dockerproxy::Startup = marsa_dockerproxy::Startup {
    timeout: WAIT,
    timed_out: "Marsa timed out starting the engine",
    unavailable: "The Marsa engine is unavailable",
};

struct Proxy {
    _dir: tempfile::TempDir,
    task: JoinHandle<io::Result<()>>,
    client: UnixStream,
    engine: UnixStream,
}

impl Proxy {
    async fn connect() -> Self {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let client_path = dir.path().join("docker.sock");
        let upstream_path = dir.path().join("engine.sock");
        let client_listener = UnixListener::bind(&client_path).unwrap();
        let engine_listener = UnixListener::bind(&upstream_path).unwrap();
        let task = tokio::spawn(marsa_dockerproxy::serve(
            client_listener,
            geteuid().as_raw(),
            STARTUP,
            move || {
                let path = upstream_path.clone();
                async move { UnixStream::connect(path).await }
            },
        ));
        let client = UnixStream::connect(client_path).await.unwrap();
        let (engine, _) = timeout(WAIT, engine_listener.accept())
            .await
            .unwrap()
            .unwrap();
        Self {
            _dir: dir,
            task,
            client,
            engine,
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn streams_chunked_responses_and_reuses_the_engine_connection() {
    let Proxy { client, engine, .. } = &mut Proxy::connect().await;
    timeout(WAIT, async {
        let request = b"GET /v1.57/containers/test/logs?follow=1 HTTP/1.1\r\nHost: docker\r\n\r\n";
        send(client, engine, request).await;
        // Progress must reach the client before the engine sends the final chunk.
        let opening = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\none\r\n";
        send(engine, client, opening).await;
        send(engine, client, b"3\r\ntwo\r\n0\r\n\r\n").await;
        send(
            client,
            engine,
            b"GET /version HTTP/1.1\r\nHost: docker\r\n\r\n",
        )
        .await;
        send(engine, client, b"HTTP/1.1 204 No Content\r\n\r\n").await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn upgrades_round_trip_binary_data_and_keep_reply_open_after_input_eof() {
    let Proxy { client, engine, .. } = &mut Proxy::connect().await;
    timeout(WAIT, async {
        send(
            client,
            engine,
            b"POST /v1.57/exec/test/start HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: tcp\r\n\r\n",
        )
        .await;
        send(
            engine,
            client,
            b"HTTP/1.1 101 UPGRADED\r\nConnection: Upgrade\r\nUpgrade: tcp\r\n\r\n\xff\x00",
        )
        .await;
        send(client, engine, b"\x00\xff\x80").await;
        client.shutdown().await.unwrap();
        expect_eof(engine).await;
        send(engine, client, b"\x12\x34").await;
        engine.shutdown().await.unwrap();
        expect_eof(client).await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn closing_a_quiet_client_drops_the_upstream_in_both_directions() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let path = dir.path().join("docker.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let (opened, mut engine) = mpsc::channel(1);
    let proxy = tokio::spawn(marsa_dockerproxy::serve(
        listener,
        geteuid().as_raw(),
        STARTUP,
        move || {
            let opened = opened.clone();
            async move {
                let (stream, other) = tokio::io::duplex(16);
                opened.send(other).await.unwrap();
                Ok(stream)
            }
        },
    ));
    let client = UnixStream::connect(path).await.unwrap();
    let mut upstream = timeout(WAIT, engine.recv()).await.unwrap().unwrap();
    drop(client);
    timeout(WAIT, expect_eof(&mut upstream)).await.unwrap();
    // Reading EOF alone could also come from only shutting down the writer.
    assert_eq!(
        upstream.write_all(b"x").await.unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
    proxy.abort();
}

#[tokio::test]
async fn closing_a_client_cancels_a_pending_upstream_open() {
    struct Dropped(mpsc::Sender<()>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.try_send(()).unwrap();
        }
    }

    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let path = dir.path().join("docker.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let (opened, mut started) = mpsc::channel(1);
    let (dropped, mut canceled) = mpsc::channel(1);
    let proxy = tokio::spawn(marsa_dockerproxy::serve(
        listener,
        geteuid().as_raw(),
        STARTUP,
        move || {
            let opened = opened.clone();
            let dropped = Dropped(dropped.clone());
            async move {
                let _dropped = dropped;
                opened.send(()).await.unwrap();
                pending::<io::Result<UnixStream>>().await
            }
        },
    ));
    let client = UnixStream::connect(path).await.unwrap();
    timeout(WAIT, started.recv()).await.unwrap().unwrap();
    drop(client);
    timeout(WAIT, canceled.recv()).await.unwrap().unwrap();
    proxy.abort();
}

#[tokio::test]
async fn rejects_wrong_peer_uid_before_opening_an_engine_connection() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let path = dir.path().join("docker.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let opened = Arc::new(AtomicBool::new(false));
    let proxy = tokio::spawn(marsa_dockerproxy::serve(
        listener,
        geteuid().as_raw() ^ 1,
        STARTUP,
        {
            let opened = opened.clone();
            move || {
                opened.store(true, Ordering::Relaxed);
                async { Err::<UnixStream, _>(io::ErrorKind::ConnectionRefused.into()) }
            }
        },
    ));
    let mut client = UnixStream::connect(path).await.unwrap();
    timeout(WAIT, expect_eof(&mut client)).await.unwrap();
    assert!(!opened.load(Ordering::Relaxed));
    proxy.abort();
}

async fn send(from: &mut UnixStream, to: &mut UnixStream, bytes: &[u8]) {
    let mut got = vec![0; bytes.len()];
    from.write_all(bytes).await.unwrap();
    to.read_exact(&mut got).await.unwrap();
    assert_eq!(got, bytes);
}

async fn expect_eof(stream: &mut (impl AsyncReadExt + Unpin)) {
    let mut byte = [0];
    assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
}

#[tokio::test]
async fn a_startup_deadline_returns_a_docker_json_error() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let path = dir.path().join("docker.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let proxy = tokio::spawn(marsa_dockerproxy::serve(
        listener,
        geteuid().as_raw(),
        marsa_dockerproxy::Startup {
            timeout: Duration::from_millis(30),
            timed_out: "Marsa timed out starting the engine",
            unavailable: "The Marsa engine is unavailable",
        },
        pending::<io::Result<UnixStream>>,
    ));
    for (method, body) in [
        (
            "GET",
            br#"{"message":"Marsa timed out starting the engine"}"#.as_slice(),
        ),
        ("HEAD", b"".as_slice()),
    ] {
        let mut client = UnixStream::connect(&path).await.unwrap();
        client
            .write_all(format!("{method} /_ping HTTP/1.1\r\nHost: docker\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut bytes = Vec::new();
        timeout(WAIT, client.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert!(bytes.starts_with(b"HTTP/1.1 504 "));
        let separator = bytes
            .windows(4)
            .position(|item| item == b"\r\n\r\n")
            .unwrap();
        assert_eq!(&bytes[separator + 4..], body);
    }
    proxy.abort();
}
