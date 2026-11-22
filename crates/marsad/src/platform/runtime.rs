//! Assemble the native runtime from immutable bundle resources and private user state.

use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, Read},
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use marsa_core::{
    guest_manifest::GuestManifest,
    paths::MarsaPaths,
    settings::{NetworkMode, ProxySettings, Settings, VmBackend},
};
use marsa_vm::{Capabilities, DiskSpec, Hypervisor, NetworkSpec, VmSpec};
use marsa_vm_vz::VzHypervisor;
use nix::libc;

use crate::diagnostics::localized;

mod backup;
mod disk;
use disk::ensure_disk;

const SECTOR_BYTES: u64 = 512;
const MIB: u64 = 1024 * 1024;
// Bound work on broken bundles well above the size of the shipped kernel and root image.
const MAX_KERNEL_BYTES: u64 = 64 * MIB;
const MAX_ROOTFS_BYTES: u64 = 1024 * MIB;
const BOOT_ARGS: &str =
    "console=hvc0 root=/dev/vda rootfstype=erofs ro init=/usr/bin/marsa-init panic=-1";

/// Runtime resources anchored to the canonical running executable.
pub(super) struct Packaged {
    bundle: PathBuf,
}

impl Packaged {
    /// Detect the packaged layout independently of the launch directory or environment.
    pub(super) fn discover() -> io::Result<Option<Self>> {
        let executable = std::env::current_exe()?.canonicalize()?;
        Ok(Self::from_executable(&executable))
    }

    fn from_executable(executable: &Path) -> Option<Self> {
        let macos = executable.parent()?;
        let contents = macos.parent()?;
        let bundle = contents.parent()?;
        (executable.file_name()? == "marsad"
            && macos.file_name()? == "MacOS"
            && contents.file_name()? == "Contents"
            && bundle.extension()? == "app")
            .then(|| Self {
                bundle: bundle.to_owned(),
            })
    }

    /// Report the backend supported by the packaged daemon.
    pub(super) fn capabilities(&self) -> Capabilities {
        self.backend().capabilities()
    }

    /// Locate the signed Docker CLI bundled by the upstream packaging task.
    pub(super) fn docker_cli(&self) -> io::Result<PathBuf> {
        let cli = Artifact::open(self.bundle.join("Contents/Resources/bin/docker"))?;
        let mode = cli.metadata.mode();
        if mode & 0o111 == 0
            || mode & 0o6022 != 0
            || !valid_signature(&self.bundle)?
            || !valid_signature(&cli.path)?
        {
            return Err(invalid_artifact(localized(
                "the bundled Docker CLI is not a verified executable",
                "ملف Docker CLI المضمّن غير قابل للتنفيذ أو لم يتم التحقق منه",
            )));
        }
        cli.check_unchanged()?;
        Ok(cli.path)
    }

    pub(super) fn verify_registry_keychain(&self) -> io::Result<()> {
        let helper = Artifact::open(
            self.bundle
                .join("Contents/Resources/bin/docker-credential-osxkeychain"),
        )?;
        let mode = helper.metadata.mode();
        if mode & 0o111 == 0
            || mode & 0o6022 != 0
            || !valid_signature(&self.bundle)?
            || !valid_signature(&helper.path)?
        {
            return Err(invalid_artifact(localized(
                "the bundled Docker credential helper is not a verified executable",
                "أداة اعتماد Docker المضمّنة غير قابلة للتنفيذ أو لم يتم التحقق منها",
            )));
        }
        helper.check_unchanged()
    }

    /// Verify the boot files and build a spec snapshot before initializing persistent storage.
    pub(super) fn prepare(
        &self,
        paths: &MarsaPaths,
        uid: u32,
        settings: &Settings,
    ) -> io::Result<(Arc<dyn Hypervisor>, VmSpec)> {
        settings
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        if settings.vm.backend != VmBackend::Vz {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                localized(
                    "the packaged daemon only supports Virtualization.framework",
                    "يدعم خادم التطبيق المجمّع إطار عمل Virtualization.framework فقط",
                ),
            ));
        }

        // A boot-time virtiofs attachment would bypass the future protected-path allowlist.
        if !settings.shares.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                localized(
                    "folder sharing requires the protected-path allowlist",
                    "تتطلب مشاركة المجلدات قائمة سماح تحمي المسارات الخاصة",
                ),
            ));
        }
        if !settings.network.dns.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                localized(
                    "custom DNS settings require the guest network policy",
                    "تتطلب إعدادات DNS المخصصة سياسة شبكة الضيف",
                ),
            ));
        }
        let proxy = &settings.network.proxy;
        if proxy.inject_into_containers
            || (*proxy != ProxySettings::default()
                && settings.network.mode != NetworkMode::Userspace)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                localized(
                    "the manual daemon proxy requires user-space networking and cannot be injected into containers",
                    "يتطلب وكيل الخادم اليدوي شبكة مساحة المستخدم ولا يمكن تمريره إلى الحاويات",
                ),
            ));
        }
        let (kernel, root_disk) = self.verify_guest()?;
        self.verify_helper()?;
        // The application seal detects replacement of both the manifest and its hashed files.
        if !valid_signature(&self.bundle)? {
            return Err(invalid_artifact(localized(
                "the Marsa app bundle has an invalid code signature or resource seal",
                "توقيع الشفرة أو ختم الموارد لحزمة تطبيق Marsa غير صالح",
            )));
        }
        let memory_bytes = match settings.vm.memory {
            Some(bytes) => bytes,
            None => settings.vm.memory_bytes(host_memory()?),
        };
        if !memory_bytes.is_multiple_of(MIB) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                localized(
                    "virtual machine memory must be a multiple of 1 MiB",
                    "يجب أن تكون ذاكرة الجهاز الافتراضي من مضاعفات 1 ميبيبايت",
                ),
            ));
        }
        let spec = VmSpec {
            cpus: u32::from(settings.vm.cpus),
            memory_bytes,
            kernel,
            initramfs: None,
            cmdline: BOOT_ARGS.into(),
            root_disk: DiskSpec {
                path: root_disk,
                read_only: true,
            },
            data_disk: DiskSpec {
                path: paths.data_disk(),
                read_only: false,
            },
            shares: Vec::new(),
            network: match settings.network.mode {
                NetworkMode::Auto | NetworkMode::Nat => NetworkSpec::Nat,
                NetworkMode::Userspace => NetworkSpec::Userspace,
            },
            console_log: paths.console_log(),
        };
        spec.validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        ensure_disk(paths, uid, settings.disk.data_size_limit)?;
        backup::set_excluded(&paths.data_disk(), settings.disk.exclude_from_backup)?;
        Ok((self.backend(), spec))
    }

    fn helper(&self) -> PathBuf {
        self.bundle.join("Contents/Helpers/marsa-net-helper")
    }

    fn backend(&self) -> Arc<dyn Hypervisor> {
        Arc::new(VzHypervisor::with_userspace(self.helper()))
    }

    fn verify_guest(&self) -> io::Result<(PathBuf, PathBuf)> {
        let guest = self.bundle.join("Contents/Resources/guest");
        let manifest = Artifact::open(guest.join("guest-manifest.json"))?;
        let kernel = Artifact::open(guest.join("Image"))?;
        let rootfs = Artifact::open(guest.join("rootfs.img"))?;
        if !(64..=MAX_KERNEL_BYTES).contains(&kernel.metadata.len())
            || !(4096..=MAX_ROOTFS_BYTES).contains(&rootfs.metadata.len())
        {
            return Err(invalid_artifact(localized(
                "the guest kernel or root disk has an invalid size",
                "حجم نواة الضيف أو قرص الجذر غير صالح",
            )));
        }
        let description = GuestManifest::read(&manifest.file).map_err(io::Error::other)?;
        let components = description.components();
        if components.marsa_init.is_none() || components.marsa_agent.is_none() {
            return Err(invalid_artifact(localized(
                "the guest manifest must identify the init program and guest agent",
                "يجب أن يحدد بيان الضيف برنامج التهيئة ووكيل الضيف",
            )));
        }
        description
            .verify(
                (&kernel.file).take(kernel.metadata.len() + 1),
                (&rootfs.file).take(rootfs.metadata.len() + 1),
            )
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut kernel_magic = [0; 4];
        let mut rootfs_magic = [0; 4];
        kernel.file.read_exact_at(&mut kernel_magic, 56)?;
        rootfs.file.read_exact_at(&mut rootfs_magic, 1024)?;
        if kernel_magic != *b"ARM\x64"
            || rootfs_magic != 0xe0f5_e1e2u32.to_le_bytes()
            || !rootfs.metadata.len().is_multiple_of(SECTOR_BYTES)
        {
            return Err(invalid_artifact(localized(
                "the guest must contain an ARM64 kernel and an EROFS root disk",
                "يجب أن تتضمن صورة الضيف نواة ARM64 وقرص جذر بنظام EROFS",
            )));
        }
        manifest.check_unchanged()?;
        kernel.check_unchanged()?;
        rootfs.check_unchanged()?;
        Ok((kernel.path, rootfs.path))
    }

    fn verify_helper(&self) -> io::Result<()> {
        let helper = Artifact::open(self.helper())?;
        let mode = helper.metadata.mode();
        if mode & 0o111 == 0 || mode & 0o6022 != 0 {
            return Err(invalid_helper());
        }
        if !valid_signature(&helper.path)? {
            return Err(invalid_helper());
        }
        helper.check_unchanged()
    }
}

fn valid_signature(path: &Path) -> io::Result<bool> {
    Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
}

struct Artifact {
    path: PathBuf,
    file: File,
    metadata: Metadata,
}

impl Artifact {
    fn open(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW_ANY | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|error| {
                io::Error::new(error.kind(), format!("{}: {error}", path.display()))
            })?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(invalid_artifact(localized(
                "expected a regular, nonempty bundle resource",
                "يجب أن يكون مورد التطبيق المجمّع ملفًا عاديًا غير فارغ",
            )));
        }
        Ok(Self {
            path,
            file,
            metadata,
        })
    }

    fn check_unchanged(&self) -> io::Result<()> {
        let current = Self::open(self.path.clone())?.metadata;
        let original = &self.metadata;
        if (current.dev(), current.ino(), current.len())
            != (original.dev(), original.ino(), original.len())
            || (current.mtime(), current.mtime_nsec()) != (original.mtime(), original.mtime_nsec())
            || (current.ctime(), current.ctime_nsec()) != (original.ctime(), original.ctime_nsec())
        {
            return Err(invalid_artifact(localized(
                "a bundle resource changed during validation",
                "تغيّر مورد للتطبيق المجمّع أثناء التحقق منه",
            )));
        }
        Ok(())
    }
}

fn invalid_artifact(detail: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail)
}

fn invalid_helper() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        localized(
            "the bundled user-space network helper needs a valid signature and executable permissions",
            "يجب أن تكون أداة شبكة مساحة المستخدم المجمّعة قابلة للتنفيذ وتحمل توقيعًا صالحًا",
        ),
    )
}

#[allow(unsafe_code)]
fn host_memory() -> io::Result<u64> {
    let mut bytes = 0u64;
    let mut length = size_of::<u64>();
    // SAFETY: The name is NUL-terminated and the writable buffer has the advertised size.
    // The null input and zero input length make this a read-only sysctl call.
    let result = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut bytes).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if length != size_of::<u64>() || bytes == 0 {
        return Err(invalid_artifact(localized(
            "could not determine host memory size",
            "تعذّر تحديد حجم ذاكرة المضيف",
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
