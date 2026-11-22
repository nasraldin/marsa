//! Store bounded, short-lived bind approvals with exact prepared directories.

use std::{
    collections::VecDeque,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

use marsa_proto::host::v1::{BindApproval, BindApprovalList};

const MAX_PENDING: usize = 32;
const MAX_PROMPT_PATHS: usize = 16;
const MAX_DISPLAY_BYTES: usize = 16 * 1024;
pub(super) const LIFETIME: Duration = Duration::from_secs(120);

#[derive(Default)]
pub(super) struct Pending {
    entries: VecDeque<Prompt>,
}

struct Prompt {
    wire: BindApproval,
    expiry: Instant,
}

impl Pending {
    pub(super) fn enqueue(
        &mut self,
        mut paths: Vec<PathBuf>,
        now: Instant,
        unix_ms: i64,
    ) -> io::Result<Option<String>> {
        let mut paths: Vec<_> = paths
            .drain(..)
            .map(|path| path.into_os_string().into_string())
            .collect::<Result<_, _>>()
            .map_err(|_| io::ErrorKind::InvalidData)?;
        paths.sort_unstable();
        paths.dedup();
        if paths.is_empty()
            || paths.len() > MAX_PROMPT_PATHS
            || paths.iter().map(String::len).sum::<usize>() > MAX_DISPLAY_BYTES
        {
            return Ok(None);
        }
        self.prune(now);
        if self.entries.len() == MAX_PENDING {
            return Ok(None);
        }
        if self
            .entries
            .iter()
            .any(|prompt| prompt.wire.directories == paths)
        {
            return Ok(None);
        }
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let id = format!("{:032x}", u128::from_be_bytes(random));
        self.entries.push_back(Prompt {
            wire: BindApproval {
                id: id.clone(),
                directories: paths,
                expires_at_unix_millis: unix_ms.saturating_add(120_000),
            },
            expiry: now + LIFETIME,
        });
        Ok(Some(id))
    }

    pub(super) fn take(&mut self, id: &str, now: Instant) -> Option<BindApproval> {
        self.prune(now);
        let index = self
            .entries
            .iter()
            .position(|prompt| prompt.wire.id == id)?;
        self.entries.remove(index).map(|prompt| prompt.wire)
    }

    pub(super) fn expire(&mut self, id: &str, now: Instant) -> bool {
        let previous = self.entries.len();
        self.entries
            .retain(|prompt| prompt.wire.id != id || now < prompt.expiry);
        self.entries.len() != previous
    }

    pub(super) fn list(&self, now: Instant) -> BindApprovalList {
        BindApprovalList {
            approvals: self
                .entries
                .iter()
                .filter(|prompt| now < prompt.expiry)
                .map(|prompt| prompt.wire.clone())
                .collect(),
        }
    }

    fn prune(&mut self, now: Instant) {
        self.entries.retain(|prompt| now < prompt.expiry);
    }
}
