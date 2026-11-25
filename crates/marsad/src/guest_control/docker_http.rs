//! Make bounded, read-only HTTP requests over the guest's Docker stream.

use std::{future::Future, io};

use http_body_util::{BodyExt, Empty, Limited};
use hyper::{
    Request, StatusCode,
    body::{Bytes, Incoming},
    client::conn::http1,
};
use hyper_util::rt::TokioIo;
use marsa_vm::VsockStream;

pub(crate) async fn get(stream: VsockStream, uri: &str, maximum: usize) -> io::Result<Bytes> {
    get_with(stream, uri, |body| async move {
        let body = Limited::new(body, maximum)
            .collect()
            .await
            .map_err(io::Error::other)?;
        Ok(body.to_bytes())
    })
    .await
}

/// Process a Docker body while driving its HTTP connection on the same task.
pub(crate) async fn get_with<T, F>(
    stream: VsockStream,
    uri: &str,
    read: impl FnOnce(Incoming) -> F,
) -> io::Result<T>
where
    F: Future<Output = io::Result<T>>,
{
    let (mut sender, connection) = http1::Builder::new()
        .max_buf_size(16 * 1024)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(io::Error::other)?;
    let request = async move {
        let request = Request::get(uri)
            .header("Host", "docker")
            .header("Connection", "close")
            .body(Empty::<Bytes>::new())
            .map_err(io::Error::other)?;
        let response = sender
            .send_request(request)
            .await
            .map_err(io::Error::other)?;
        if response.status() != StatusCode::OK {
            return Err(io::ErrorKind::InvalidData.into());
        }
        read(response.into_body()).await
    };
    let (body, _) = tokio::join!(request, connection);
    body
}
