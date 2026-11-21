//! Exercise the shipped agent over the same framed control protocol as the host.

use std::io::{self, Read, Write};
use std::time::Duration;

use marsa_proto::guest::v1::guest_envelope::Body as GuestBody;
use marsa_proto::guest::v1::host_envelope::Body as HostBody;
use marsa_proto::guest::v1::{
    CommandStatus, EngineHealthState, GuestEnvelope, HelloAck, HostEnvelope, ProtocolVersion,
    SetTime,
};
use marsa_proto::guest::{decode_guest, encode_host, frame_length};

// The fixed past epoch deliberately differs from the Virtualization.framework RTC.
const TEST_UNIX_SECONDS: i64 = 1_700_000_000;
const SET_TIME_REQUEST: u64 = 1;
const VERSION_1_0: ProtocolVersion = ProtocolVersion { major: 1, minor: 0 };
const MAX_EVENTS: usize = 32;

pub(super) fn verify(stream: &mut (impl Read + Write)) -> io::Result<()> {
    let Some(GuestBody::Hello(hello)) = receive(stream)?.body else {
        return Err(invalid("expected Hello as first control frame"));
    };
    if hello.protocol != Some(VERSION_1_0)
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
            "Hello lacked protocol 1.0, versions, or boot timings",
        ));
    }
    send(
        stream,
        0,
        HostBody::HelloAck(HelloAck {
            protocol: Some(VERSION_1_0),
        }),
    )?;
    send(
        stream,
        SET_TIME_REQUEST,
        HostBody::SetTime(SetTime {
            unix_seconds: TEST_UNIX_SECONDS,
            nanoseconds: 0,
        }),
    )?;

    let mut time_set = false;
    let mut engine_ready = false;
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
                engine_ready |= health.state == EngineHealthState::Ready as i32;
            }
            _ => return Err(invalid("unexpected guest control result or event")),
        }
        if time_set && engine_ready {
            return Ok(());
        }
    }
    Err(invalid(
        "expected SetTime result and ready health within 32 guest messages",
    ))
}

pub(super) fn clock_assertions(deadline: Duration) -> Vec<u8> {
    let latest = TEST_UNIX_SECONDS + deadline.as_secs() as i64;
    // A bug that creates agent.ready before SetTime leaves its mtime in the old epoch.
    format!(
        "marsa_fixture_clock=$(busybox date +%s)\n\
         marsa_fixture_ready=$(busybox stat -c %Y /run/marsa/agent.ready)\n\
         printf 'MARSA_GUEST_CLOCK=%s MARSA_AGENT_READY_MTIME=%s\\n' \"$marsa_fixture_clock\" \"$marsa_fixture_ready\"\n\
         test \"$marsa_fixture_ready\" -ge {TEST_UNIX_SECONDS}\n\
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
