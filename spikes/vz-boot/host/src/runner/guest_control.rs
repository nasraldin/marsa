//! Exercise the shipped agent over the same framed control protocol as the host.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::ControlTimings;

use marsa_proto::guest::v1::guest_envelope::Body as GuestBody;
use marsa_proto::guest::v1::host_envelope::Body as HostBody;
use marsa_proto::guest::v1::{
    CommandStatus, ConfigureNetwork, EngineHealthState, GuestEnvelope, HelloAck, HostAlias,
    HostEnvelope, InstallCertificates, ProtocolVersion, Proxy, SetTime,
};
use marsa_proto::guest::{decode_guest, encode_host, frame_length};

// The fixed past epoch deliberately differs from the Virtualization.framework RTC.
const TEST_UNIX_SECONDS: i64 = 1_700_000_000;
const SET_TIME_REQUEST: u64 = 1;
const TEST_CA: &str = "MARSA_VZ_TEST_CA_DER";

fn test_epoch() -> i64 {
    if std::env::var_os(TEST_CA).is_some() {
        1_800_000_000
    } else {
        TEST_UNIX_SECONDS
    }
}

fn network(dns: &str) -> ConfigureNetwork {
    ConfigureNetwork {
        dns_servers: vec![dns.into()],
        search_domains: vec!["marsa.test".into()],
        proxy: Some(Proxy {
            http_url: Some("http://198.18.0.99:3128".into()),
            no_proxy: vec!["registry.marsa.test".into()],
            ..Default::default()
        }),
        host_aliases: ["registry.marsa.test", "host.docker.internal"]
            .into_iter()
            .map(|name| HostAlias {
                name: name.into(),
                address: "198.18.0.10".into(),
            })
            .collect(),
    }
}

fn command(stream: &mut (impl Read + Write), id: u64, body: HostBody) -> io::Result<()> {
    expect_command(stream, id, body, CommandStatus::Ok)
}

fn expect_command(
    stream: &mut (impl Read + Write),
    id: u64,
    body: HostBody,
    expected: CommandStatus,
) -> io::Result<()> {
    send(stream, id, body)?;
    for _ in 0..MAX_EVENTS {
        let envelope = receive(stream)?;
        match envelope.body {
            Some(GuestBody::CommandResult(result)) if envelope.request_id == id => {
                if result.status == expected as i32 {
                    return Ok(());
                }
                return Err(invalid(format!(
                    "guest command {id} failed: {}",
                    result.detail
                )));
            }
            Some(GuestBody::EngineHealth(_)) => {}
            _ => return Err(invalid("unexpected guest command response")),
        }
    }
    Err(invalid("guest command did not complete"))
}

const VERSION_1_0: ProtocolVersion = ProtocolVersion { major: 1, minor: 0 };
const MAX_EVENTS: usize = 32;

pub(super) fn verify(
    stream: &mut (impl Read + Write),
    use_host_time: bool,
) -> io::Result<ControlTimings> {
    let Some(GuestBody::Hello(hello)) = receive(stream)?.body else {
        return Err(invalid("expected Hello as first control frame"));
    };
    let hello_at = Instant::now();
    if !hello
        .protocol
        .is_some_and(|version| version.major == VERSION_1_0.major)
        || hello.image_version.is_empty()
        || hello.components.is_empty()
        || ["mount_system", "data_disk", "shared_folders", "network"]
            .into_iter()
            .any(|stage| {
                !hello
                    .boot_timings
                    .iter()
                    .any(|timing| timing.stage == stage)
            })
    {
        return Err(invalid(
            "Hello lacked compatible protocol, versions, or boot timings",
        ));
    }
    send(
        stream,
        0,
        HostBody::HelloAck(HelloAck {
            protocol: Some(VERSION_1_0),
            defer_engine_start: false,
        }),
    )?;
    if !use_host_time && let Some(ca) = std::env::var_os(TEST_CA) {
        command(
            stream,
            2,
            HostBody::ConfigureNetwork(network("198.18.0.53")),
        )?;
        let mut certificate = Vec::new();
        std::fs::File::open(ca)?
            .take(64 * 1024 + 1)
            .read_to_end(&mut certificate)?;
        if certificate.len() > 64 * 1024 {
            return Err(invalid("test CA is too large"));
        }
        command(
            stream,
            3,
            HostBody::InstallCertificates(InstallCertificates {
                certificates_der: vec![certificate],
            }),
        )?;
    }
    let time = if use_host_time {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(invalid)?;
        SetTime {
            unix_seconds: i64::try_from(since_epoch.as_secs()).map_err(invalid)?,
            nanoseconds: since_epoch.subsec_nanos(),
        }
    } else {
        SetTime {
            unix_seconds: test_epoch(),
            nanoseconds: 0,
        }
    };
    send(stream, SET_TIME_REQUEST, HostBody::SetTime(time))?;

    let mut time_set = false;
    let mut engine_ready = None;
    for _ in 0..MAX_EVENTS {
        let envelope = receive(stream)?;
        match envelope.body {
            Some(GuestBody::CommandResult(result))
                if !time_set && envelope.request_id == SET_TIME_REQUEST =>
            {
                if result.status != CommandStatus::Ok as i32 {
                    return Err(invalid("SetTime did not succeed"));
                }
                time_set = true;
            }
            Some(GuestBody::EngineHealth(health)) => {
                if health.state == EngineHealthState::Ready as i32 {
                    engine_ready.get_or_insert_with(Instant::now);
                }
            }
            Some(GuestBody::Stats(_) | GuestBody::PortEvent(_)) => {}
            _ => return Err(invalid("unexpected guest control result or event")),
        }
        if time_set && let Some(engine_ready) = engine_ready {
            if !use_host_time && std::env::var_os(TEST_CA).is_some() {
                command(
                    stream,
                    4,
                    HostBody::ConfigureNetwork(network("198.18.0.54")),
                )?;
                let mut staged = network("198.18.0.54");
                if let Some(proxy) = &mut staged.proxy {
                    proxy.http_url = Some("http://198.18.0.98:3128".into());
                }
                expect_command(
                    stream,
                    5,
                    HostBody::ConfigureNetwork(staged),
                    CommandStatus::Unavailable,
                )?;
            }
            return Ok(ControlTimings {
                hello: hello_at,
                engine_ready,
            });
        }
    }
    Err(invalid(
        "expected SetTime result and ready health within 32 guest messages",
    ))
}

pub(super) fn clock_assertions(deadline: Duration) -> Vec<u8> {
    let epoch = test_epoch();
    let latest = epoch + deadline.as_secs() as i64;
    // A bug that creates agent.ready before SetTime leaves its mtime in the old epoch.
    format!(
        "marsa_fixture_clock=$(busybox date +%s)\n\
         marsa_fixture_ready=$(busybox stat -c %Y /run/marsa/agent.ready)\n\
         printf 'MARSA_GUEST_CLOCK=%s MARSA_AGENT_READY_MTIME=%s\\n' \"$marsa_fixture_clock\" \"$marsa_fixture_ready\"\n\
         test \"$marsa_fixture_ready\" -ge {epoch}\n\
         test \"$marsa_fixture_clock\" -ge \"$marsa_fixture_ready\"\n\
         test \"$marsa_fixture_clock\" -le {latest}\n"
    )
    .into_bytes()
}

fn receive(stream: &mut impl Read) -> io::Result<GuestEnvelope> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let size = frame_length(length).map_err(invalid)?;
    let mut payload = vec![0; size];
    stream.read_exact(&mut payload)?;
    decode_guest(&payload).map_err(invalid)
}

fn send(stream: &mut impl Write, request_id: u64, body: HostBody) -> io::Result<()> {
    let frame = encode_host(&HostEnvelope {
        request_id,
        body: Some(body),
    })
    .map_err(invalid)?;
    stream.write_all(&frame)
}

fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
