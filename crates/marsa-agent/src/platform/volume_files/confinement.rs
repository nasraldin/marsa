//! Hold volume and selection inodes open while reading only confined Linux paths.

use std::{
    fs::{File, Metadata},
    io::{self, Read as _, Write},
    os::{fd::AsRawFd as _, unix::fs::MetadataExt as _},
    path::{Path, PathBuf},
};

use marsa_proto::guest::{
    VOLUME_READ_LIMIT,
    v1::{PrepareVolumeRead, SelectVolumeRead, VolumeReadFormat},
    validate_volume_path, validate_volume_selection,
};
use rustix::{
    fs::{self, AtFlags, CWD, Dir, Mode, OFlags, ResolveFlags},
    io::Errno,
};
use tar::{Builder, EntryType, Header};

const DOCKER_ROOT: &str = "/var/lib/docker";
const MAX_DEPTH: usize = 64;
const MAX_ENTRIES: usize = 100_000;
const MAX_PATH: usize = 4096;

pub(super) struct SelectedVolume {
    docker: File,
    volumes: File,
    volume: Volume,
    name: String,
}

pub(super) struct Selection {
    volume: SelectedVolume,
    path: PathBuf,
    selected: File,
    format: VolumeReadFormat,
}

struct Volume {
    directory: File,
    data: File,
}

impl Volume {
    fn open(volumes: &File, name: &str) -> io::Result<Self> {
        let directory = directory_beneath(volumes, Path::new(name))?;
        match fs::statat(&directory, "opts.json", AtFlags::SYMLINK_NOFOLLOW) {
            Err(Errno::NOENT) => {}
            Ok(_) => return Err(io::ErrorKind::Unsupported.into()),
            Err(error) => return Err(error.into()),
        }
        let data = directory_beneath(&directory, Path::new("_data"))?;
        Ok(Self { directory, data })
    }
}

impl SelectedVolume {
    pub(super) fn open(request: &SelectVolumeRead) -> io::Result<Self> {
        // This fixed boot path crosses the intended data-disk mount. Everything
        // below it uses the mount- and symlink-rejecting resolver.
        let docker = File::from(fs::openat2(
            CWD,
            DOCKER_ROOT,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS,
        )?);
        Self::open_at(&docker, request)
    }

    fn open_at(docker: &File, request: &SelectVolumeRead) -> io::Result<Self> {
        validate_volume_selection(request).map_err(io::Error::other)?;
        let volumes = directory_beneath(docker, Path::new("volumes"))?;
        let volume = Volume::open(&volumes, &request.name)?;
        Ok(Self {
            docker: docker.try_clone()?,
            volumes,
            volume,
            name: request.name.clone(),
        })
    }

    pub(super) fn prepare(self, request: &PrepareVolumeRead) -> io::Result<Selection> {
        validate_volume_path(&request.path).map_err(io::Error::other)?;
        self.revalidate()?;
        let path = PathBuf::from(
            request
                .path
                .strip_prefix('/')
                .filter(|path| !path.is_empty())
                .unwrap_or("."),
        );
        let selected = readable_beneath(&self.volume.data, &path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                io::ErrorKind::InvalidInput.into()
            } else {
                error
            }
        })?;
        let format = if request.prefer_single_file && selected.metadata()?.is_file() {
            VolumeReadFormat::SingleFile
        } else {
            VolumeReadFormat::Tar
        };
        if selected.metadata()?.len() > VOLUME_READ_LIMIT {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        Ok(Selection {
            volume: self,
            path,
            selected,
            format,
        })
    }

    fn revalidate(&self) -> io::Result<()> {
        let volumes = directory_beneath(&self.docker, Path::new("volumes"))
            .map_err(|_| selection_changed())?;
        same_file(&volumes, &self.volumes)?;
        let current = Volume::open(&volumes, &self.name).map_err(|_| selection_changed())?;
        same_file(&current.directory, &self.volume.directory)?;
        same_file(&current.data, &self.volume.data)
    }

    fn data(&self) -> &File {
        &self.volume.data
    }
}

impl Selection {
    pub(super) fn format(&self) -> VolumeReadFormat {
        self.format
    }

    /// Verify the name and selected path still resolve to the pinned objects.
    pub(super) fn revalidate(&self) -> io::Result<()> {
        self.volume.revalidate()?;
        self.revalidate_path(&self.path, &self.selected)
    }

    pub(super) fn write(&self, output: &mut impl Write) -> io::Result<()> {
        self.revalidate()?;
        match self.format {
            VolumeReadFormat::SingleFile => {
                let before = self.selected.metadata()?;
                let mut input = &self.selected;
                copy_file(&mut input, &before, output)?;
            }
            VolumeReadFormat::Tar => {
                let name = self
                    .path
                    .file_name()
                    .filter(|_| self.path != Path::new("."))
                    .map(Path::new)
                    .unwrap_or_else(|| Path::new(&self.volume.name));
                let mut archive = Builder::new(output);
                let mut entries = 0;
                self.append(
                    &mut archive,
                    &self.path,
                    name,
                    &self.selected,
                    0,
                    &mut entries,
                )?;
                archive.finish()?;
            }
            VolumeReadFormat::Unspecified => return Err(io::ErrorKind::InvalidData.into()),
        }
        self.revalidate()
    }

    fn append<W: Write>(
        &self,
        archive: &mut Builder<W>,
        path: &Path,
        archive_path: &Path,
        file: &File,
        depth: usize,
        entries: &mut usize,
    ) -> io::Result<()> {
        *entries += 1;
        if depth > MAX_DEPTH || *entries > MAX_ENTRIES || path.as_os_str().len() > MAX_PATH {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        let metadata = file.metadata()?;
        let mut header = Header::new_gnu();
        if metadata.is_dir() {
            header.set_entry_type(EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            archive.append_data(&mut header, archive_path, io::empty())?;
            for child in Dir::read_from(file)? {
                let child = child?;
                let name = child
                    .file_name()
                    .to_str()
                    .map_err(|_| io::ErrorKind::InvalidInput)?;
                if matches!(name, "." | "..") {
                    continue;
                }
                if name.chars().any(char::is_control) {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                let nested = path.join(name);
                // Always resolve the complete path from the volume root.
                // A previously opened directory might have been moved away.
                let member = readable_beneath(self.volume.data(), &nested)?;
                self.append(
                    archive,
                    &nested,
                    &archive_path.join(name),
                    &member,
                    depth + 1,
                    entries,
                )?;
            }
        } else {
            check_file(&metadata)?;
            header.set_entry_type(EntryType::Regular);
            header.set_mode(0o644);
            header.set_size(metadata.len());
            archive.append_data(&mut header, archive_path, file.take(metadata.len()))?;
            unchanged_file(&metadata, &file.metadata()?)?;
        }
        self.revalidate_path(path, file)
    }

    fn revalidate_path(&self, path: &Path, file: &File) -> io::Result<()> {
        let current = inspect_beneath(self.volume.data(), path).map_err(|_| selection_changed())?;
        same_file(&current, file)
    }
}

fn beneath(base: &File, path: &Path, flags: OFlags) -> io::Result<File> {
    Ok(File::from(fs::openat2(
        base,
        path,
        flags | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?))
}

fn directory_beneath(base: &File, path: &Path) -> io::Result<File> {
    beneath(base, path, OFlags::PATH | OFlags::DIRECTORY)
}

fn inspect_beneath(base: &File, path: &Path) -> io::Result<File> {
    beneath(base, path, OFlags::PATH)
}

fn readable_beneath(base: &File, path: &Path) -> io::Result<File> {
    let descriptor = inspect_beneath(base, path)?;
    let metadata = descriptor.metadata()?;
    if !metadata.is_dir() {
        check_file(&metadata)?;
    }
    // The kernel descriptor is owned by this call. Reopening that descriptor
    // avoids a second user path resolution and never opens a special file.
    File::open(format!("/proc/self/fd/{}", descriptor.as_raw_fd()))
}

fn check_file(metadata: &Metadata) -> io::Result<()> {
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    if metadata.len() > VOLUME_READ_LIMIT {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    Ok(())
}

fn same_file(a: &File, b: &File) -> io::Result<()> {
    let a = a.metadata()?;
    let b = b.metadata()?;
    if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
        return Err(selection_changed());
    }
    Ok(())
}

fn selection_changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::StaleNetworkFileHandle,
        "volume selection changed",
    )
}

fn unchanged_file(before: &Metadata, after: &Metadata) -> io::Result<()> {
    check_file(after)?;
    if (before.len(), before.ctime(), before.ctime_nsec())
        != (after.len(), after.ctime(), after.ctime_nsec())
    {
        return Err(selection_changed());
    }
    Ok(())
}

fn copy_file(input: &mut &File, before: &Metadata, output: &mut impl Write) -> io::Result<()> {
    check_file(before)?;
    if io::copy(&mut input.take(before.len()), output)? != before.len() {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    unchanged_file(before, &input.metadata()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    const FIXTURE_NAME: &str = "fixture-data";

    fn fixture() -> (tempfile::TempDir, File, PathBuf, PrepareVolumeRead) {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("volumes/fixture-data/_data");
        fs::create_dir_all(data.join("nested")).unwrap();
        fs::write(data.join("nested/test.bin"), [0, 7, 255]).unwrap();
        fs::write(root.path().join("outside"), b"outside").unwrap();
        let root_fd = File::open(root.path()).unwrap();
        let request = PrepareVolumeRead {
            path: "/nested/test.bin".into(),
            prefer_single_file: true,
        };
        (root, root_fd, data, request)
    }

    fn select(fd: &File) -> io::Result<SelectedVolume> {
        SelectedVolume::open_at(
            fd,
            &SelectVolumeRead {
                name: FIXTURE_NAME.into(),
                created_at: "2026-11-26T00:00:00Z".into(),
            },
        )
    }

    fn prepare(fd: &File, request: &PrepareVolumeRead) -> io::Result<Selection> {
        select(fd)?.prepare(request)
    }

    #[test]
    fn returns_bytes_or_a_scoped_tar() {
        let (_root, fd, _, mut request) = fixture();
        let mut output = Vec::new();
        let selected = prepare(&fd, &request).unwrap();
        assert_eq!(selected.format(), VolumeReadFormat::SingleFile);
        selected.write(&mut output).unwrap();
        assert_eq!(output, [0, 7, 255]);

        request.path = "/".into();
        let selected = prepare(&fd, &request).unwrap();
        assert_eq!(selected.format(), VolumeReadFormat::Tar);
        output.clear();
        selected.write(&mut output).unwrap();
        let mut archive = tar::Archive::new(output.as_slice());
        let files: Vec<_> = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.path().unwrap().into_owned(),
                    entry.header().entry_type(),
                )
            })
            .collect();
        assert_eq!(
            files,
            [
                ("fixture-data".into(), EntryType::Directory),
                ("fixture-data/nested".into(), EntryType::Directory),
                ("fixture-data/nested/test.bin".into(), EntryType::Regular),
            ]
        );
    }

    #[test]
    fn never_follows_links_special_files_or_mount_crossings() {
        let (root, fd, data, mut request) = fixture();
        symlink(root.path().join("outside"), data.join("symlink")).unwrap();
        fs::hard_link(root.path().join("outside"), data.join("hardlink")).unwrap();
        nix::unistd::mkfifo(&data.join("fifo"), nix::sys::stat::Mode::S_IRUSR).unwrap();
        for path in ["/symlink", "/hardlink", "/fifo", "/../outside"] {
            request.path = path.into();
            assert!(prepare(&fd, &request).is_err(), "{path}");
        }
        request.path = "/".into();
        let selected = prepare(&fd, &request).unwrap();
        assert!(selected.write(&mut Vec::new()).is_err());
        let root_fd = File::open("/").unwrap();
        assert!(directory_beneath(&root_fd, Path::new("proc")).is_err());
    }

    #[test]
    fn rejects_option_backed_and_replaced_volumes() {
        let (root, fd, data, request) = fixture();
        let approved = select(&fd).unwrap();
        let selected = prepare(&fd, &request).unwrap();
        let volume = data.parent().unwrap();
        fs::rename(volume, root.path().join("original")).unwrap();
        fs::create_dir_all(data.join("nested")).unwrap();
        fs::write(data.join("nested/test.bin"), b"other").unwrap();
        let mut output = Vec::new();
        assert!(selected.write(&mut output).is_err());
        assert!(output.is_empty());
        assert!(
            matches!(approved.prepare(&request), Err(error) if error.kind() == io::ErrorKind::StaleNetworkFileHandle)
        );

        fs::write(volume.join("opts.json"), b"{}").unwrap();
        assert!(
            matches!(prepare(&fd, &request), Err(error) if error.kind() == io::ErrorKind::Unsupported)
        );
    }
}
