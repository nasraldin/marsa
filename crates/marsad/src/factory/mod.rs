//! Journal factory settings, data disk and permission grants as one private generation.

mod platform;
#[cfg(test)]
mod tests;

use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
};

use marsa_core::{paths::MarsaPaths, settings::Settings};
use serde::{Deserialize, Serialize};

use platform::{Directory, Inode, invalid};

const ACTIVE: &str = ".factory-reset";
const FINISHED: &str = ".factory-reset-finished";
const MANIFEST: &str = "manifest.json";
const COMMITTED: &str = "committed";
const MAX_MANIFEST: u64 = 4096;

#[derive(Clone, Copy)]
enum Member {
    Config,
    Disk,
    Grants,
}

const MEMBERS: [Member; 3] = [Member::Config, Member::Disk, Member::Grants];

impl Member {
    fn names(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Config => ("config.toml", "old-config", "new-config"),
            Self::Disk => ("data.img", "old-disk", "new-disk"),
            Self::Grants => ("grants.json", "old-grants", "new-grants"),
        }
    }
}

struct Layout {
    root: Directory,
    vm: Directory,
    state: Directory,
}

impl Layout {
    fn open(paths: &MarsaPaths, uid: u32) -> io::Result<Self> {
        let root = Directory::root(paths.root(), uid)?;
        let vm = root.child("vm")?.ok_or_else(invalid)?;
        let state = root.child("state")?.ok_or_else(invalid)?;
        if !root.same_device(&vm)? || !root.same_device(&state)? {
            return Err(invalid());
        }
        Ok(Self { root, vm, state })
    }

    fn directory(&self, member: Member) -> &Directory {
        match member {
            Member::Config => &self.root,
            Member::Disk => &self.vm,
            Member::Grants => &self.state,
        }
    }

    fn inodes(&self) -> io::Result<[Inode; 3]> {
        Ok([self.root.inode()?, self.vm.inode()?, self.state.inode()?])
    }

    fn open_member(&self, member: Member) -> io::Result<Option<File>> {
        let (name, _, _) = member.names();
        let file = self
            .directory(member)
            .entry(name, matches!(member, Member::Disk))?;
        if let Some(file) = &file
            && matches!(member, Member::Disk)
        {
            let metadata = file.metadata()?;
            if metadata.mode() & 0o7777 != 0o600
                || metadata.len() == 0
                || !metadata.len().is_multiple_of(512)
            {
                return Err(invalid());
            }
        }
        Ok(file)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Stamp {
    inode: Inode,
    mode: u32,
    length: u64,
    modification: Option<(i64, i64, i64, i64)>,
}

fn stamp(file: &File, member: Member) -> io::Result<Stamp> {
    let metadata = file.metadata()?;
    // Disk contents can legitimately change until the guest is stopped.
    let modification = (!matches!(member, Member::Disk)).then(|| {
        (
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    });
    Ok(Stamp {
        inode: Inode::from(&metadata),
        mode: metadata.mode(),
        length: metadata.len(),
        modification,
    })
}

struct Captured {
    _file: File,
    stamp: Stamp,
}

pub(crate) struct Prepared {
    _layout: Layout,
    directories: [Inode; 3],
    original: [Option<Captured>; 3],
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    prior: Option<Inode>,
    replacement: Option<Inode>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u16,
    directories: [Inode; 3],
    members: [Record; 3],
}

#[derive(Debug)]
pub(crate) enum ResetError {
    Refused(io::Error),
    RecoveryRequired(io::Error),
}

impl Prepared {
    pub(crate) fn capture(paths: &MarsaPaths, uid: u32) -> io::Result<Self> {
        let layout = Layout::open(paths, uid)?;
        let directories = layout.inodes()?;
        let mut original = [None, None, None];
        for (index, member) in MEMBERS.into_iter().enumerate() {
            if let Some(file) = layout.open_member(member)? {
                original[index] = Some(Captured {
                    stamp: stamp(&file, member)?,
                    _file: file,
                });
            }
        }
        Ok(Self {
            _layout: layout,
            directories,
            original,
        })
    }

    pub(crate) fn apply(
        self,
        paths: &MarsaPaths,
        uid: u32,
        defaults: &Settings,
        document: &str,
    ) -> Result<(), ResetError> {
        self.apply_with(paths, uid, defaults, document, || {})
    }

    fn apply_with(
        self,
        paths: &MarsaPaths,
        uid: u32,
        defaults: &Settings,
        document: &str,
        mut checkpoint: impl FnMut(),
    ) -> Result<(), ResetError> {
        let layout = Layout::open(paths, uid).map_err(ResetError::Refused)?;
        if layout
            .root
            .child(ACTIVE)
            .map_err(ResetError::RecoveryRequired)?
            .is_some()
            || layout
                .root
                .child(FINISHED)
                .map_err(ResetError::RecoveryRequired)?
                .is_some()
        {
            return Err(ResetError::RecoveryRequired(invalid()));
        }
        let disk = self.verify(&layout).map_err(ResetError::Refused)?;
        if let Some(disk) = &disk {
            disk.try_lock()
                .map_err(|error| ResetError::Refused(error.into()))?;
        }
        let nonce =
            getrandom::u64().map_err(|error| ResetError::Refused(io::Error::other(error)))?;
        let temporary = format!(".factory-stage.{nonce:016x}");
        let directory = layout
            .root
            .create_directory(&temporary)
            .map_err(ResetError::Refused)?;
        let staged = (|| {
            let data = directory.create_file("new-disk")?;
            let size = defaults.disk.data_size_limit;
            data.set_len(size - size % 512)?;
            data.sync_all()?;
            let disk = Some(Inode::from(&data.metadata()?));
            // Drop writable handles before assigning their durable live names.
            drop(data);
            let mut config = directory.create_file("new-config")?;
            config.write_all(document.as_bytes())?;
            config.sync_all()?;
            let config = Some(Inode::from(&config.metadata()?));
            let replacement = [config, disk, None];
            let journal = Journal {
                version: 1,
                directories: self.directories,
                members: std::array::from_fn(|index| Record {
                    prior: self.original[index]
                        .as_ref()
                        .map(|captured| captured.stamp.inode),
                    replacement: replacement[index],
                }),
            };
            let mut manifest = directory.create_file(MANIFEST)?;
            serde_json::to_writer(&mut manifest, &journal)?;
            manifest.sync_all()?;
            directory.sync()?;
            let identity = directory.inode()?;
            self.verify(&layout)?;
            checkpoint();
            layout.root.move_directory(&temporary, ACTIVE, identity)?;
            Ok::<_, io::Error>(journal)
        })();
        let journal = match staged {
            Ok(journal) => journal,
            Err(error) => {
                if layout
                    .root
                    .child(ACTIVE)
                    .map_err(ResetError::RecoveryRequired)?
                    .is_some()
                {
                    return Err(ResetError::RecoveryRequired(error));
                }
                let _ = clear_unpublished(&layout.root, &directory, &temporary);
                return Err(ResetError::Refused(error));
            }
        };

        checkpoint();
        let operations = (|| {
            for (member, record) in MEMBERS.into_iter().zip(journal.members) {
                let live = layout.directory(member);
                let (name, backup, stage) = member.names();
                if let Some(prior) = record.prior {
                    live.move_entry(name, &directory, backup, prior)?;
                } else if live.entry_inode(name)?.is_some() {
                    return Err(invalid());
                }
                checkpoint();
                if let Some(next) = record.replacement {
                    directory.move_entry(stage, live, name, next)?;
                }
                checkpoint();
            }
            // The originals remain recoverable until all live directory entries are durable.
            directory.create_file(COMMITTED)?.sync_all()?;
            directory.sync()?;
            checkpoint();
            Ok::<_, io::Error>(())
        })();
        match (operations, recover_layout(&layout, &mut checkpoint)) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => {
                // A visible commit marker implies that the complete new generation
                // survived. The recovery path always establishes one generation.
                if self.live_replacement(&layout, &journal).unwrap_or(false) {
                    Ok(())
                } else {
                    Err(ResetError::Refused(error))
                }
            }
            (_, Err(error)) => Err(ResetError::RecoveryRequired(error)),
        }
    }

    pub(crate) fn validate(&self, paths: &MarsaPaths, uid: u32) -> io::Result<()> {
        self.verify(&Layout::open(paths, uid)?)?;
        Ok(())
    }

    fn verify(&self, layout: &Layout) -> io::Result<Option<File>> {
        if layout.inodes()? != self.directories {
            return Err(invalid());
        }
        let mut disk = None;
        for (index, member) in MEMBERS.into_iter().enumerate() {
            let file = layout.open_member(member)?;
            let observed = file.as_ref().map(|open| stamp(open, member)).transpose()?;
            if observed != self.original[index].as_ref().map(|captured| captured.stamp) {
                return Err(invalid());
            }
            if matches!(member, Member::Disk) {
                disk = file;
            }
        }
        Ok(disk)
    }

    fn live_replacement(&self, layout: &Layout, journal: &Journal) -> io::Result<bool> {
        for (member, record) in MEMBERS.into_iter().zip(journal.members) {
            if layout.directory(member).entry_inode(member.names().0)? != record.replacement {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Finish or roll back interrupted factory storage before publishing either daemon socket.
///
/// Call under the daemon instance lock with its private directories already openable.
pub fn recover(paths: &MarsaPaths, uid: u32) -> io::Result<()> {
    let layout = Layout::open(paths, uid)?;
    recover_layout(&layout, &mut || {})
}

fn recover_layout(layout: &Layout, checkpoint: &mut impl FnMut()) -> io::Result<()> {
    if let Some(directory) = layout.root.child(FINISHED)? {
        clear_finished(layout, &directory)?;
    }
    let Some(directory) = layout.root.child(ACTIVE)? else {
        return Ok(());
    };
    let journal = read_journal(layout, &directory)?;
    let committed = commit_marker(&directory)?;
    // Validate the complete generation before recovery makes its first change.
    for (member, record) in MEMBERS.into_iter().zip(journal.members) {
        let (name, backup, stage) = member.names();
        let current = layout.directory(member).entry_inode(name)?;
        let old = directory.entry_inode(backup)?;
        let new = directory.entry_inode(stage)?;
        let allowed = if committed {
            current == record.replacement && (old.is_none() || old == record.prior) && new.is_none()
        } else {
            (current.is_none() || current == record.prior || current == record.replacement)
                && (old.is_none() || old == record.prior)
                && (new.is_none() || new == record.replacement)
                && record
                    .prior
                    .is_none_or(|prior| (current == Some(prior)) != (old == Some(prior)))
                && record
                    .replacement
                    .is_none_or(|next| !(current == Some(next) && new == Some(next)))
        };
        if !allowed {
            return Err(invalid());
        }
    }
    for (member, record) in MEMBERS.into_iter().zip(journal.members) {
        let live = layout.directory(member);
        let (name, backup, stage) = member.names();
        if !committed {
            if let Some(next) = record.replacement
                && live.entry_inode(name)? == Some(next)
            {
                live.remove(name, next)?;
                checkpoint();
            }
            if let Some(prior) = record.prior
                && directory.entry_inode(backup)? == Some(prior)
            {
                directory.move_entry(backup, live, name, prior)?;
                checkpoint();
            }
        }
        if committed && let Some(prior) = directory.entry_inode(backup)? {
            if Some(prior) != record.prior {
                return Err(invalid());
            }
            directory.remove(backup, prior)?;
            checkpoint();
        }
        if let Some(next) = directory.entry_inode(stage)? {
            if Some(next) != record.replacement {
                return Err(invalid());
            }
            directory.remove(stage, next)?;
            checkpoint();
        }
    }
    layout
        .root
        .move_directory(ACTIVE, FINISHED, directory.inode()?)?;
    checkpoint();
    clear_finished(layout, &directory)
}

fn commit_marker(directory: &Directory) -> io::Result<bool> {
    let Some(commit) = directory.entry(COMMITTED, false)? else {
        return Ok(false);
    };
    if commit.metadata()?.len() != 0 {
        return Err(invalid());
    }
    // Re-establish durability before discarding a recoverable original.
    commit.sync_all()?;
    directory.sync()?;
    Ok(true)
}

fn read_journal(layout: &Layout, directory: &Directory) -> io::Result<Journal> {
    let manifest = directory.entry(MANIFEST, false)?.ok_or_else(invalid)?;
    let mut bytes = Vec::new();
    manifest.take(MAX_MANIFEST + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err(invalid());
    }
    let journal: Journal = serde_json::from_slice(&bytes)?;
    if journal.version != 1
        || journal.directories != layout.inodes()?
        || journal.members[0].replacement.is_none()
        || journal.members[1].replacement.is_none()
        || journal.members[2].replacement.is_some()
    {
        return Err(invalid());
    }
    let mut seen = HashSet::new();
    for identity in journal
        .members
        .iter()
        .flat_map(|record| [record.prior, record.replacement])
        .flatten()
    {
        if !seen.insert(identity) {
            return Err(invalid());
        }
    }
    Ok(journal)
}

fn clear_finished(layout: &Layout, directory: &Directory) -> io::Result<()> {
    if let Some(manifest) = directory.entry_inode(MANIFEST)? {
        let journal = read_journal(layout, directory)?;
        let mut old_generation = true;
        let mut new_generation = true;
        for (member, record) in MEMBERS.into_iter().zip(journal.members) {
            let (name, backup, stage) = member.names();
            let current = layout.directory(member).entry_inode(name)?;
            old_generation &= current == record.prior;
            new_generation &= current == record.replacement;
            if directory.entry_inode(backup)?.is_some() || directory.entry_inode(stage)?.is_some() {
                return Err(invalid());
            }
        }
        if !old_generation && !new_generation {
            return Err(invalid());
        }
        if let Some(marker) = directory.entry_inode(COMMITTED)? {
            if !new_generation {
                return Err(invalid());
            }
            directory.remove(COMMITTED, marker)?;
        }
        directory.remove(MANIFEST, manifest)?;
    }
    layout.root.remove_directory(FINISHED, directory.inode()?)
}

fn clear_unpublished(root: &Directory, directory: &Directory, name: &str) -> io::Result<()> {
    for name in [MANIFEST, "new-config", "new-disk"] {
        if let Some(inode) = directory.entry_inode(name)? {
            directory.remove(name, inode)?;
        }
    }
    root.remove_directory(name, directory.inode()?)
}
