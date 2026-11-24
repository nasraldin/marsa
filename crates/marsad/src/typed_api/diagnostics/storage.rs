//! Open only bounded, private log files and publish a complete archive atomically.

use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

use marsa_core::paths::{MarsaPaths, is_daemon_log_name};
use nix::{
    errno::Errno,
    fcntl::{AtFlags, OFlag, openat, readlinkat},
    sys::stat::{Mode, mkdirat},
    unistd::{UnlinkatFlags, linkat, unlinkat},
};

use super::{Snapshot, summaries, versions};

pub(super) const MAX_LOG_BYTES: u64 = 256 * 1024;
const DIRECTORY_FLAGS: OFlag = OFlag::O_DIRECTORY.union(OFlag::O_NOFOLLOW);
const FILE_FLAGS: OFlag = OFlag::O_NOFOLLOW.union(OFlag::O_NONBLOCK);
const REVIEW: &str = "Review this archive before sharing it. Marsa saves it locally.\nراجع هذا الأرشيف قبل مشاركته. يحفظه Marsa محليًا.\n";

pub(super) fn collect(paths: &MarsaPaths, uid: u32, snapshot: Snapshot) -> io::Result<String> {
    let root = private_root(paths, uid)?;
    let output = paths.diagnostics_dir();
    // Check that the RPC can return the path before creating anything.
    output
        .to_str()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    let output_name = name(&output)?;
    match mkdirat(&root, output_name, Mode::S_IRWXU) {
        Ok(()) | Err(Errno::EEXIST) => (),
        Err(error) => return Err(error.into()),
    }
    let output_directory =
        directory_in(&root, output_name, uid).map_err(private_directory_error)?;

    let daemon = daemon_tail(&root, paths, uid);
    let console = console_tail(&root, paths, uid);
    let data = [
        ("versions.json", versions()),
        ("settings.json", snapshot.settings),
        ("state.json", snapshot.state),
        ("network.json", snapshot.network),
        (
            "daemon-log.json",
            summaries::report(&daemon, summaries::daemon),
        ),
        (
            "console-log.json",
            summaries::report(&console, summaries::console),
        ),
        (
            "engine-log.json",
            summaries::report(&console, summaries::engine),
        ),
    ];
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let staging = format!(".{id}.part");
    let filename = format!("marsa-diagnostics-{id}.tar");
    let mut archive = File::from(openat(
        &output_directory,
        staging.as_str(),
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | FILE_FLAGS,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )?);
    let result = (|| -> io::Result<()> {
        let mut tar = tar::Builder::new(&mut archive);
        entry(&mut tar, "review.txt", REVIEW.as_bytes())?;
        for (name, value) in data {
            let bytes = serde_json::to_vec_pretty(&value)?;
            entry(&mut tar, name, &bytes)?;
        }
        tar.finish()?;
        drop(tar);
        archive.sync_all()?;
        // Publish the fully written file in the verified directory without
        // replacing an existing archive or resolving parent paths again.
        linkat(
            &output_directory,
            staging.as_str(),
            &output_directory,
            filename.as_str(),
            AtFlags::empty(),
        )?;
        Ok(())
    })();
    if let Err(error) = unlinkat(
        &output_directory,
        staging.as_str(),
        UnlinkatFlags::NoRemoveDir,
    ) {
        tracing::warn!(kind = ?error, "diagnostics staging cleanup failed");
    }
    result?;
    // The archive has already been published. Report it even when directory
    // sync reports a durability issue so a retry does not create another copy.
    if let Err(error) = output_directory.sync_all() {
        tracing::warn!(kind = ?error.kind(), "diagnostics directory sync failed");
    }
    output
        .join(filename)
        .into_os_string()
        .into_string()
        .map_err(|_| io::ErrorKind::InvalidData.into())
}

fn entry(tar: &mut tar::Builder<&mut File>, name: &str, data: &[u8]) -> io::Result<()> {
    let mut header = tar::Header::new_ustar();
    header.set_mode(0o600);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(data.len() as u64);
    tar.append_data(&mut header, name, data)
}

fn daemon_tail(root: &File, paths: &MarsaPaths, uid: u32) -> io::Result<Vec<u8>> {
    tail_file(daemon_file(root, paths, uid)?)
}

fn daemon_file(root: &File, paths: &MarsaPaths, uid: u32) -> io::Result<File> {
    let path = paths.logs_dir();
    let directory = directory_in(root, name(&path)?, uid)?;
    let target = readlinkat(&directory, name(&paths.daemon_log())?)?;
    let target = Path::new(&target);
    let target_name = name(target)?;
    if !is_daemon_log_name(target_name)
        || !matches!(target.parent(), Some(parent) if parent.as_os_str().is_empty() || parent == path)
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    private_file(&directory, target_name, uid)
}

fn console_tail(root: &File, paths: &MarsaPaths, uid: u32) -> io::Result<Vec<u8>> {
    tail_file(console_file(root, paths, uid)?)
}

fn console_file(root: &File, paths: &MarsaPaths, uid: u32) -> io::Result<File> {
    let directory = directory_in(root, name(&paths.vm_dir())?, uid)?;
    private_file(&directory, name(&paths.console_log())?, uid)
}

pub(super) fn log_files(paths: &MarsaPaths, uid: u32) -> io::Result<[io::Result<File>; 2]> {
    let root = private_root(paths, uid)?;
    Ok([
        daemon_file(&root, paths, uid),
        console_file(&root, paths, uid),
    ])
}

fn private_root(paths: &MarsaPaths, uid: u32) -> io::Result<File> {
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(DIRECTORY_FLAGS.bits())
        .open(paths.root())
        .map_err(private_directory_error)?;
    require_directory(&root.metadata()?, uid)?;
    Ok(root)
}

fn private_file(directory: &File, name: &str, uid: u32) -> io::Result<File> {
    let file = openat(
        directory,
        name,
        FILE_FLAGS | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?;
    let file = File::from(file);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.uid() != uid
        || metadata.nlink() != 1
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(file)
}

fn tail_file(mut file: File) -> io::Result<Vec<u8>> {
    let offset = file.metadata()?.len().saturating_sub(MAX_LOG_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut data = Vec::new();
    file.take(MAX_LOG_BYTES).read_to_end(&mut data)?;
    if offset > 0 {
        match data.iter().position(|&byte| byte == b'\n') {
            Some(first_break) => {
                data.drain(..=first_break);
            }
            None => data.clear(),
        }
    }
    Ok(data)
}

fn directory_in(parent: &File, name: &str, uid: u32) -> io::Result<File> {
    let file = openat(
        parent,
        name,
        DIRECTORY_FLAGS | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?;
    let file = File::from(file);
    require_directory(&file.metadata()?, uid)?;
    Ok(file)
}

fn require_directory(metadata: &Metadata, uid: u32) -> io::Result<()> {
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

fn private_directory_error(error: io::Error) -> io::Error {
    if matches!(error.raw_os_error(), Some(code) if code == Errno::ENOTDIR as i32 || code == Errno::ELOOP as i32)
    {
        io::ErrorKind::PermissionDenied.into()
    } else {
        error
    }
}

fn name(path: &Path) -> io::Result<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::ErrorKind::InvalidData.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    #[test]
    fn rejects_indirect_log_files_and_nonprivate_output_directories() {
        let home = tempfile::tempdir().unwrap();
        let paths = MarsaPaths::from_home(home.path()).unwrap();
        let uid = nix::unistd::geteuid().as_raw();
        for path in [paths.root().to_owned(), paths.logs_dir(), paths.vm_dir()] {
            fs::create_dir(&path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let outside = home.path().join("outside");
        fs::write(&outside, "synthetic_secret").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&outside, paths.daemon_log()).unwrap();
        symlink(&outside, paths.console_log()).unwrap();

        let root = File::open(paths.root()).unwrap();
        assert!(daemon_tail(&root, &paths, uid).is_err());
        assert!(console_tail(&root, &paths, uid).is_err());
        fs::remove_file(paths.console_log()).unwrap();
        fs::hard_link(&outside, paths.console_log()).unwrap();
        assert!(console_tail(&root, &paths, uid).is_err());

        symlink(home.path(), paths.diagnostics_dir()).unwrap();
        let snapshot = Snapshot::new(
            &Default::default(),
            &Default::default(),
            &Default::default(),
            None,
            false,
        );
        let error = collect(&paths, uid, snapshot).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 2);
    }

    #[test]
    fn reads_a_bounded_tail_without_retaining_partial_lines() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("console.log");
        let mut data = vec![b'x'; MAX_LOG_BYTES as usize];
        data.extend_from_slice(b"\nlast record\n");
        fs::write(&file, data).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        let fd = File::open(directory.path()).unwrap();
        assert_eq!(
            tail_file(private_file(&fd, "console.log", nix::unistd::geteuid().as_raw()).unwrap())
                .unwrap(),
            b"last record\n"
        );
    }
}
