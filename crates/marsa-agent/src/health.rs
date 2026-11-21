//! Classify transitions in Docker's bounded Unix-socket ping probe.

use marsa_proto::guest::v1::{EngineHealth, EngineHealthState};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time::{Duration, timeout},
};

const PING_TIMEOUT: Duration = Duration::from_millis(250);
const PING_REQUEST: &[u8] = b"GET /_ping HTTP/1.0\r\nHost: docker\r\nConnection: close\r\n\r\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probe {
    Missing,
    Failed,
    Healthy,
}

pub(crate) struct Health {
    state: EngineHealthState,
    was_ready: bool,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            state: EngineHealthState::Starting,
            was_ready: false,
        }
    }
}

impl Health {
    pub(crate) fn initial(&self) -> EngineHealth {
        event(self.state)
    }

    pub(crate) fn observe(&mut self, probe: Probe) -> Option<EngineHealth> {
        let next = match probe {
            Probe::Healthy => {
                self.was_ready = true;
                EngineHealthState::Ready
            }
            Probe::Missing if self.was_ready => EngineHealthState::Restarting,
            Probe::Missing => EngineHealthState::Starting,
            Probe::Failed => EngineHealthState::Unhealthy,
        };
        if next == self.state {
            return None;
        }
        self.state = next;
        Some(event(next))
    }
}

fn event(state: EngineHealthState) -> EngineHealth {
    let detail = match state {
        EngineHealthState::Starting => "Docker endpoint is starting",
        EngineHealthState::Ready => "Docker ping succeeded",
        EngineHealthState::Restarting => "Docker endpoint disappeared after readiness",
        _ => "Docker ping failed",
    };
    EngineHealth {
        state: state as i32,
        detail: detail.to_owned(),
    }
}

pub(crate) async fn probe(path: &str) -> Probe {
    timeout(PING_TIMEOUT, ping(path))
        .await
        .unwrap_or(Probe::Failed)
}

async fn ping(path: &str) -> Probe {
    let mut socket = match UnixStream::connect(path).await {
        Ok(socket) => socket,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Probe::Missing;
        }
        Err(_) => return Probe::Failed,
    };
    if socket.write_all(PING_REQUEST).await.is_err() {
        return Probe::Failed;
    }

    let mut response = [0_u8; 128];
    let mut filled = 0;
    while filled < response.len() {
        match socket.read(&mut response[filled..]).await {
            Ok(0) | Err(_) => return Probe::Failed,
            Ok(count) => filled += count,
        }
        if let Some(line) = response[..filled].split(|byte| *byte == b'\n').next()
            && line.len() < filled
        {
            return if [b"HTTP/1.0 200 ".as_slice(), b"HTTP/1.1 200 ".as_slice()]
                .iter()
                .any(|prefix| line.starts_with(prefix))
            {
                Probe::Healthy
            } else {
                Probe::Failed
            };
        }
    }
    Probe::Failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_changes_and_recovery() {
        let mut health = Health::default();
        assert_eq!(health.initial().state, EngineHealthState::Starting as i32);
        assert!(health.observe(Probe::Missing).is_none());
        for (input, expected) in [
            (Probe::Healthy, EngineHealthState::Ready),
            (Probe::Missing, EngineHealthState::Restarting),
            (Probe::Failed, EngineHealthState::Unhealthy),
            (Probe::Healthy, EngineHealthState::Ready),
        ] {
            assert_eq!(health.observe(input).unwrap().state, expected as i32);
            assert!(health.observe(input).is_none());
        }
    }
}
