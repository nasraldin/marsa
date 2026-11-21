//! Describe an activation's backend, VM settings, and completion notification.

use std::{io, path::PathBuf, sync::Arc};

use marsa_vm::{Hypervisor, VmSpec};

use super::network::Selection;

pub(crate) type Applied = Arc<dyn Fn() + Send + Sync>;
pub(crate) type Prepare = Arc<dyn Fn() -> io::Result<Launch> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Launch {
    pub(crate) backend: Arc<dyn Hypervisor>,
    pub(crate) spec: VmSpec,
    pub(crate) selection: Selection,
    pub(crate) applied: Option<Applied>,
}

#[derive(Clone)]
pub(super) enum Source {
    Fixed(Box<Launch>),
    Package(Prepare),
}

pub(super) struct Runtime {
    pub(super) source: Source,
    pub(super) console_log: PathBuf,
    pub(super) pause_resume: bool,
}
