//! Exchange bounded control messages without blocking on a partial frame.

use std::io;

use marsa_proto::guest::{
    decode_host, encode_guest, frame_length, negotiate,
    v1::{
        CommandResult, CommandStatus, ConfigureNetwork, GuestEnvelope, Hello, HostEnvelope,
        InstallCertificates, SetTime, guest_envelope, host_envelope,
    },
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
    time::{Duration, MissedTickBehavior, interval, timeout},
};

use crate::health::{self, Health};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const HEALTH_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) trait GuestState {
    fn set_time(&self, time: SetTime) -> io::Result<()>;
    async fn mark_ready(&self) -> io::Result<()>;
    async fn configure_network(&self, _request: ConfigureNetwork) -> io::Result<bool> {
        Err(io::ErrorKind::Unsupported.into())
    }
    async fn install_certificates(&self, _request: InstallCertificates) -> io::Result<bool> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

pub(crate) async fn session<T>(
    stream: T,
    hello: Hello,
    state: &impl GuestState,
    engine_socket: &str,
) -> io::Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    write(&mut writer, 0, guest_envelope::Body::Hello(hello)).await?;
    let envelope = timeout(HANDSHAKE_TIMEOUT, read(&mut reader)).await??;
    let Some(host_envelope::Body::HelloAck(ack)) = envelope.body else {
        return Err(invalid("expected the host's protocol acknowledgement"));
    };
    let version = ack
        .protocol
        .ok_or_else(|| invalid("missing protocol version"))?;
    if negotiate(version).map_err(io::Error::other)? != version {
        return Err(invalid("host selected an unsupported protocol version"));
    }

    let (sender, mut inbox) = mpsc::channel(1);
    let receiver = tokio::spawn(async move {
        loop {
            let next = read(&mut reader).await;
            let failed = next.is_err();
            if sender.send(next).await.is_err() || failed {
                break;
            }
        }
    });
    let result = exchange(&mut writer, &mut inbox, state, engine_socket).await;
    receiver.abort();
    result
}

async fn exchange(
    writer: &mut (impl AsyncWrite + Unpin),
    inbox: &mut mpsc::Receiver<io::Result<HostEnvelope>>,
    state: &impl GuestState,
    engine_socket: &str,
) -> io::Result<()> {
    let mut health = Health::default();
    let mut ready = false;
    let mut sample = interval(HEALTH_INTERVAL);
    sample.set_missed_tick_behavior(MissedTickBehavior::Skip);
    write(
        writer,
        0,
        guest_envelope::Body::EngineHealth(health.initial()),
    )
    .await?;
    loop {
        tokio::select! {
            envelope = inbox.recv() => {
                let Some(envelope) = envelope else { return Ok(()) };
                let envelope = envelope?;
                let (status, detail) = match envelope.body {
                    Some(host_envelope::Body::SetTime(time)) => {
                        match update_time(state, time, &mut ready).await {
                            Ok(()) => (CommandStatus::Ok, "Guest clock set"),
                            Err(error) => {
                                eprintln!("marsa-agent: set guest time: {error}");
                                (CommandStatus::Failed, "Could not set the guest clock")
                            }
                        }
                    }
                    Some(host_envelope::Body::ConfigureNetwork(request)) => configuration_result(
                        state.configure_network(request).await, "Guest network applied",
                        "DNS applied. Staged Docker settings require a controlled restart.",
                    ),
                    Some(host_envelope::Body::InstallCertificates(request)) => configuration_result(
                        state.install_certificates(request).await, "Guest CA bundle installed",
                        "CA bundle installed. Docker may need a controlled restart to reload trust.",
                    ),
                    _ => (CommandStatus::Unavailable, "Command is unavailable in this guest"),
                };
                write(writer, envelope.request_id, guest_envelope::Body::CommandResult(
                    CommandResult { status: status as i32, detail: detail.to_owned() }
                )).await?;
            }
            _ = sample.tick(), if ready => {
                if let Some(event) = health.observe(health::probe(engine_socket).await) {
                    write(writer, 0, guest_envelope::Body::EngineHealth(event)).await?;
                }
            }
        }
    }
}

fn configuration_result(
    result: io::Result<bool>,
    success: &'static str,
    staged: &'static str,
) -> (CommandStatus, &'static str) {
    match result {
        Ok(false) => (CommandStatus::Ok, success),
        Ok(true) => (CommandStatus::Unavailable, staged),
        Err(error) => {
            let status = match error.kind() {
                io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => CommandStatus::Invalid,
                io::ErrorKind::Unsupported => CommandStatus::Unavailable,
                _ => CommandStatus::Failed,
            };
            (status, "Guest configuration could not be applied")
        }
    }
}

async fn update_time(state: &impl GuestState, time: SetTime, ready: &mut bool) -> io::Result<()> {
    state.set_time(time)?;
    if !*ready {
        state.mark_ready().await?;
        *ready = true;
    }
    Ok(())
}

async fn read(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<HostEnvelope> {
    let mut header = [0; 4];
    reader.read_exact(&mut header).await?;
    let length = frame_length(header).map_err(io::Error::other)?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    decode_host(&payload).map_err(io::Error::other)
}

async fn write(
    writer: &mut (impl AsyncWrite + Unpin),
    request_id: u64,
    body: guest_envelope::Body,
) -> io::Result<()> {
    let frame = encode_guest(&GuestEnvelope {
        request_id,
        body: Some(body),
    })
    .map_err(io::Error::other)?;
    timeout(WRITE_TIMEOUT, writer.write_all(&frame)).await?
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests;
