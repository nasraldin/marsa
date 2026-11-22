//! Store bounded, short-lived bind approvals with exact prepared directories.

use std::{
    collections::VecDeque,
    io,
    sync::Arc,
    time::{Duration, Instant},
};

use marsa_proto::host::v1::{BindApproval, BindApprovalList};
use marsa_share::Allowlist;

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
    roots: Arc<Allowlist>,
    expiry: Instant,
    visible: bool,
}

impl Pending {
    pub(super) fn enqueue(
        &mut self,
        roots: Arc<Allowlist>,
        now: Instant,
        unix_ms: i64,
    ) -> io::Result<Option<String>> {
        let mut paths: Vec<_> = roots
            .roots()
            .iter()
            .map(|path| path.to_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or(io::ErrorKind::InvalidData)?;
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
                expires_at_unix_millis: unix_ms.saturating_add(LIFETIME.as_secs() as i64 * 1000),
            },
            roots,
            expiry: now + LIFETIME,
            visible: true,
        });
        Ok(Some(id))
    }

    pub(super) fn get(&self, id: &str, now: Instant) -> Option<Arc<Allowlist>> {
        self.entries
            .iter()
            .find(|prompt| prompt.wire.id == id && now < prompt.expiry && prompt.visible)
            .map(|prompt| prompt.roots.clone())
    }

    pub(super) fn finish(&mut self, id: &str, denied: bool) {
        let Some(index) = self.entries.iter().position(|prompt| prompt.wire.id == id) else {
            return;
        };
        if denied {
            // Keep a short-lived record so retries do not create repeated prompts.
            self.entries[index].visible = false;
        } else {
            self.entries.remove(index);
        }
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
                .filter(|prompt| now < prompt.expiry && prompt.visible)
                .map(|prompt| prompt.wire.clone())
                .collect(),
        }
    }

    fn prune(&mut self, now: Instant) {
        self.entries.retain(|prompt| now < prompt.expiry);
    }
}
