//! Exchange bounded control messages without blocking on a partial frame.

use std::io;

use marsa_proto::guest::{
    decode_host, encode_guest, frame_length, negotiate,
    v1::{
        CommandResult, CommandStatus, ConfigureNetwork, GuestEnvelope, Hello, HostEnvelope,
        InstallCertificates, PortEvent, SetTime, VmStats, guest_envelope, host_envelope,
    },
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
    task::JoinSet,
    time::{Duration, MissedTickBehavior, interval, timeout},
};

use crate::health::{self, Health};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const HEALTH_INTERVAL: Duration = Duration::from_millis(250);
const STATS_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) trait GuestState {
    fn set_time(&self, time: SetTime) -> io::Result<()>;
    async fn mark_ready(&self) -> io::Result<()>;

    async fn configure_network(&self, _request: ConfigureNetwork) -> io::Result<bool> {
        Err(io::ErrorKind::Unsupported.into())
    }
    async fn install_certificates(&self, _request: InstallCertificates) -> io::Result<bool> {
        Err(io::ErrorKind::Unsupported.into())
    }

    async fn command(&self, body: host_envelope::Body) -> io::Result<Option<guest_envelope::Body>> {
        let _ = body;
        Ok(None)
    }

    fn start_port_events(&self, _: &str, _: mpsc::Sender<PortEvent>, _: &mut JoinSet<()>) {}

    async fn stats(&self) -> io::Result<Option<VmStats>> {
        Ok(None)
    }

    fn shutdown(&self) -> io::Result<()> {
        Ok(())
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
    let mut statistics = interval(STATS_INTERVAL);
    statistics.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let (port_sender, mut port_events) = mpsc::channel(64);
    let mut port_tasks = JoinSet::new();
    let mut pending = None;
    write(
        writer,
        0,
        guest_envelope::Body::EngineHealth(health.initial()),
    )
    .await?;
    loop {
        tokio::select! {
            envelope = inbox.recv(), if pending.is_none() => {
                let Some(envelope) = envelope else { return Ok(()) };
                let envelope = envelope?;
                let request_id = envelope.request_id;
                let body = envelope.body.ok_or_else(|| invalid("missing host command"))?;
                let shutdown = matches!(body, host_envelope::Body::Shutdown(_));
                let response = match body {
                    host_envelope::Body::SetTime(time) => {
                        let previous = ready;
                        match update_time(state, time, &mut ready).await {
                            Ok(()) => {
                                if !previous {
                                    state.start_port_events(engine_socket, port_sender.clone(), &mut port_tasks);
                                }
                                success("Guest clock set")
                            }
                            Err(error) => {
                                eprintln!("marsa-agent: set guest time: {error}");
                                result(CommandStatus::Failed, "Could not set the guest clock")
                            }
                        }
                    }
                    host_envelope::Body::HelloAck(_) => return Err(invalid("unexpected HelloAck")),
                    _ if !ready && !shutdown && !matches!(
                        body,
                        host_envelope::Body::ConfigureNetwork(_) | host_envelope::Body::InstallCertificates(_)
                    ) => result(CommandStatus::Unavailable, "Guest setup is incomplete"),
                    body => {
                        // Poll one operation at a time so a slow trim cannot stall health or events.
                        pending = Some(Box::pin(async move {
                            (request_id, shutdown, dispatch(state, body).await)
                        }));
                        continue;
                    }
                };
                write(writer, request_id, response).await?;
            }
            Some((request_id, shutdown, outcome)) = async {
                match pending.as_mut() {
                    Some(operation) => Some(operation.await),
                    None => None,
                }
            }, if pending.is_some() => {
                pending = None;
                let response = match outcome {
                    Ok(Some(response)) => response,
                    Ok(None) => result(CommandStatus::Unavailable, "Command is unavailable in this guest"),
                    Err(error) => {
                        eprintln!("marsa-agent: guest operation: {error}");
                        let status = match error.kind() {
                            io::ErrorKind::InvalidInput => CommandStatus::Invalid,
                            _ => CommandStatus::Failed,
                        };
                        result(status, "Guest operation failed")
                    }
                };
                let accepted_shutdown = shutdown && matches!(
                    &response,
                    guest_envelope::Body::CommandResult(result) if result.status == CommandStatus::Ok as i32
                );
                write(writer, request_id, response).await?;
                if accepted_shutdown {
                    state.shutdown()?;
                    return Ok(());
                }
            }
            Some(event) = port_events.recv(), if ready => {
                write(writer, 0, guest_envelope::Body::PortEvent(event)).await?;
            }
            _ = statistics.tick(), if ready => {
                match state.stats().await {
                    Ok(Some(stats)) => write(writer, 0, guest_envelope::Body::Stats(stats)).await?,
                    Ok(None) => {},
                    Err(error) => eprintln!("marsa-agent: guest statistics: {error}"),
                }
            }
            _ = sample.tick(), if ready => {
                if let Some(event) = health.observe(health::probe(engine_socket).await) {
                    write(writer, 0, guest_envelope::Body::EngineHealth(event)).await?;
                }
            }
        }
    }
}

pub(crate) fn success(detail: &str) -> guest_envelope::Body {
    result(CommandStatus::Ok, detail)
}

fn result(status: CommandStatus, detail: &str) -> guest_envelope::Body {
    guest_envelope::Body::CommandResult(CommandResult {
        status: status as i32,
        detail: detail.to_owned(),
    })
}

async fn dispatch(
    state: &impl GuestState,
    body: host_envelope::Body,
) -> io::Result<Option<guest_envelope::Body>> {
    let response = match body {
        host_envelope::Body::ConfigureNetwork(request) => configuration_result(
            state.configure_network(request).await,
            "Guest network applied",
            "DNS applied. Staged Docker settings require a controlled restart.",
        ),
        host_envelope::Body::InstallCertificates(request) => configuration_result(
            state.install_certificates(request).await,
            "Guest CA bundle installed",
            "CA bundle installed. Docker may need a controlled restart to reload trust.",
        ),
        body => return state.command(body).await,
    };
    Ok(Some(response))
}

fn configuration_result(
    outcome: io::Result<bool>,
    success: &'static str,
    staged: &'static str,
) -> guest_envelope::Body {
    match outcome {
        Ok(false) => result(CommandStatus::Ok, success),
        Ok(true) => result(CommandStatus::Unavailable, staged),
        Err(error) => {
            let status = match error.kind() {
                io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => CommandStatus::Invalid,
                io::ErrorKind::Unsupported => CommandStatus::Unavailable,
                _ => CommandStatus::Failed,
            };
            result(status, "Guest configuration could not be applied")
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
