//! Finish or roll back the recorded generation before opening the service.

use std::{
    collections::HashSet,
    io::{self, Read},
};

use super::{
    ACTIVE, COMMITTED, Directory, FINISHED, Journal, Layout, MANIFEST, MEMBERS, STAGING, invalid,
};

const MAX_MANIFEST: u64 = 4096;

pub(super) fn recover_layout(layout: &Layout, checkpoint: &mut impl FnMut()) -> io::Result<()> {
    // No original member moves until the directory is renamed to ACTIVE.
    if let Some(directory) = layout.root.child(STAGING)? {
        clear_unpublished(&layout.root, &directory)?;
    }
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

pub(super) fn clear_unpublished(root: &Directory, directory: &Directory) -> io::Result<()> {
    for name in [MANIFEST, "new-config", "new-disk"] {
        if let Some(inode) = directory.entry_inode(name)? {
            directory.remove(name, inode)?;
        }
    }
    root.remove_directory(STAGING, directory.inode()?)
}
