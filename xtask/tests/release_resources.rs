//! Exercise the real release-stage command without contacting a supplier on invalid input.

#![cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use marsa_core::guest_manifest::{GuestComponentVersions, GuestManifest};
use sha2::{Digest, Sha256};

struct Fixture {
    scratch: tempfile::TempDir,
    tree: PathBuf,
    manifest_checksum: String,
}

impl Fixture {
    fn new() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let tree = scratch.path().canonicalize().unwrap();
        let source = tree.join("guest");
        fs::create_dir(&source).unwrap();
        let mut kernel = vec![0; 64];
        kernel[56..60].copy_from_slice(b"ARM\x64");
        let mut rootfs = vec![0; 4096];
        rootfs[1024..1028].copy_from_slice(&0xe0f5_e1e2u32.to_le_bytes());
        let manifest_checksum = write_guest(&source, &kernel, &rootfs);
        Self {
            scratch,
            tree,
            manifest_checksum,
        }
    }

    fn command(&self, source: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        self.append_args(&mut command, source, &self.tree.join("release"));
        command
    }

    fn append_args(&self, command: &mut Command, source: &Path, output: &Path) {
        command
            .args(["stage-release-resources", "--guest-dir"])
            .arg(source)
            .arg("--guest-manifest-sha256")
            .arg(&self.manifest_checksum)
            .arg("--output")
            .arg(output);
    }

    fn reject(&self, source: &Path, message: &str) {
        self.reject_command(self.command(source), message);
    }

    fn reject_command(&self, mut command: Command, message: &str) {
        let entries = || {
            fs::read_dir(self.scratch.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let before = entries();
        let result = command.output().unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(!result.status.success());
        assert!(stderr.contains(message), "{stderr}");
        assert_eq!(entries(), before, "the failed stage left an entry behind");
    }
}

fn write_guest(directory: &Path, kernel: &[u8], rootfs: &[u8]) -> String {
    let pins: toml::Table = toml::from_str(include_str!("../../guest/versions.toml")).unwrap();
    let value = |group: &str, name: &str| pins[group][name].as_str().unwrap().to_owned();
    let components = GuestComponentVersions {
        linux: value("kernel", "version"),
        alpine: value("alpine", "version"),
        docker: value("docker", "version"),
        buildkit: value("docker", "buildkit"),
        containerd: value("docker", "containerd"),
        runc: value("docker", "runc"),
        docker_init: value("docker", "docker_init"),
        erofs_utils: value("erofs", "version"),
        marsa_init: Some(env!("CARGO_PKG_VERSION").into()),
        marsa_agent: Some(env!("CARGO_PKG_VERSION").into()),
    };
    let manifest = GuestManifest::create(
        env!("CARGO_PKG_VERSION").to_owned(),
        components,
        Some("a".repeat(40)),
        kernel,
        rootfs,
    )
    .unwrap()
    .to_json()
    .unwrap();
    fs::write(directory.join("Image"), kernel).unwrap();
    fs::write(directory.join("rootfs.img"), rootfs).unwrap();
    fs::write(directory.join("guest-manifest.json"), &manifest).unwrap();
    Sha256::digest(manifest)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn a_rewritten_manifest_cannot_authenticate_its_modified_guest() {
    let fixture = Fixture::new();
    let source = fixture.tree.join("guest");
    let mut kernel = fs::read(source.join("Image")).unwrap();
    let rootfs = fs::read(source.join("rootfs.img")).unwrap();
    kernel[0] ^= 1;
    write_guest(&source, &kernel, &rootfs);
    fixture.reject(&source, "trusted guest manifest SHA-256 mismatch");
}

#[test]
fn an_altered_root_disk_yields_no_release_directory() {
    let fixture = Fixture::new();
    let source = fixture.tree.join("guest");
    let mut disk = fs::read(source.join("rootfs.img")).unwrap();
    disk[0] ^= 1;
    fs::write(source.join("rootfs.img"), disk).unwrap();
    fixture.reject(&source, "guest checksum mismatch for rootfs.img");
}

#[test]
fn guest_handoff_rejects_unexpected_names_and_nonregular_entries() {
    use std::os::unix::{fs::symlink, net::UnixListener};

    let fixture = Fixture::new();
    let source = fixture.tree.join("guest");
    let extra = source.join("unlisted");
    fs::create_dir(&extra).unwrap();
    fixture.reject(&source, "unexpected guest handoff entry");
    fs::remove_dir(extra).unwrap();

    let kernel = source.join("Image");
    let original = fixture.tree.join("original");
    fs::rename(&kernel, &original).unwrap();
    let expected = "guest handoff entry Image must be a regular file";
    symlink(&original, &kernel).unwrap();
    fixture.reject(&source, expected);
    fs::remove_file(&kernel).unwrap();
    let _socket = UnixListener::bind(&kernel).unwrap();
    fixture.reject(&source, expected);
}

#[test]
fn guest_source_ancestor_symlinks_are_refused() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let link = fixture.tree.join("redirect");
    symlink(&fixture.tree, &link).unwrap();
    fixture.reject(&link.join("guest"), "cargo xtask stage-release-resources:");
}

#[test]
fn an_existing_output_symlink_is_left_alone() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let target = fixture.tree.join("sentinel");
    fs::write(&target, "preserved").unwrap();
    let output = fixture.tree.join("release");
    symlink(&target, &output).unwrap();
    fixture.reject(
        &fixture.tree.join("guest"),
        "release resources output already exists",
    );
    assert_eq!(fs::read_link(output).unwrap(), target);
    assert_eq!(fs::read_to_string(target).unwrap(), "preserved");
}

#[test]
fn wrong_supplier_bytes_never_publish_the_verified_guest() {
    let fixture = Fixture::new();
    install_curl(
        &fixture.tree,
        r#"#!/bin/sh
while [ "$1" != "--output" ]; do
    [ "$#" -gt 0 ] || exit 1
    shift
done
printf 'unverified fixture' > "$2"
"#,
    );
    let mut command = fixture.command(&fixture.tree.join("guest"));
    command.env("PATH", &fixture.tree);
    fixture.reject_command(command, "bin/docker SHA-256 mismatch");
}

#[test]
fn scratch_is_private_under_a_permissive_umask() {
    let fixture = Fixture::new();
    let parent = fixture.tree.join("output");
    fs::create_dir(&parent).unwrap();
    let tools = fixture.tree.join("tools");
    fs::create_dir(&tools).unwrap();
    install_curl(
        &tools,
        r#"#!/bin/sh
while [ "$1" != "--output" ]; do
    [ "$#" -gt 0 ] || exit 1
    shift
done
ls -ld "${2%/*}" > "$SCRATCH_MODE"
printf 'unverified fixture' > "$2"
"#,
    );
    let report = fixture.tree.join("scratch-mode");
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "umask 000; exec \"$@\"", "release-test"])
        .arg(env!("CARGO_BIN_EXE_xtask"));
    fixture.append_args(
        &mut command,
        &fixture.tree.join("guest"),
        &parent.join("release"),
    );
    let result = command
        .env("PATH", format!("{}:/bin", tools.display()))
        .env("SCRATCH_MODE", &report)
        .output()
        .unwrap();

    assert!(!result.status.success());
    let mode = fs::read_to_string(report).unwrap();
    assert!(mode.starts_with("drwx------"), "{mode}");
    assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
}

#[test]
fn supplier_stage_and_cleanup_stay_in_the_open_parent_after_a_path_change() {
    let fixture = Fixture::new();
    let output_parent = fixture.tree.join("output");
    let moved_parent = fixture.tree.join("moved");
    fs::create_dir(&output_parent).unwrap();
    let tools = fixture.tree.join("tools");
    fs::create_dir(&tools).unwrap();
    install_curl(
        &tools,
        r#"#!/bin/sh
while [ "$1" != "--output" ]; do
    [ "$#" -gt 0 ] || exit 1
    shift
done
mv "$OUTPUT_PARENT" "$MOVED_PARENT" || exit 1
mkdir "$OUTPUT_PARENT" || exit 1
printf 'unverified fixture' > "$2"
"#,
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    fixture.append_args(
        &mut command,
        &fixture.tree.join("guest"),
        &output_parent.join("release"),
    );
    let result = command
        .env("PATH", format!("{}:/bin", tools.display()))
        .env("OUTPUT_PARENT", &output_parent)
        .env("MOVED_PARENT", &moved_parent)
        .output()
        .unwrap();

    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(!result.status.success());
    assert!(stderr.contains("bin/docker SHA-256 mismatch"), "{stderr}");
    assert_eq!(fs::read_dir(moved_parent).unwrap().count(), 0);
    assert_eq!(fs::read_dir(output_parent).unwrap().count(), 0);
}

fn install_curl(directory: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;

    let curl = directory.join("curl");
    fs::write(&curl, script).unwrap();
    fs::set_permissions(curl, fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn rejects_a_darwin_output_parent_with_an_inheritable_acl() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let output_parent = fixture.tree.join("output");
    fs::create_dir(&output_parent).unwrap();
    fs::set_permissions(&output_parent, fs::Permissions::from_mode(0o700)).unwrap();
    let status = Command::new("/bin/chmod")
        .args([
            "+a",
            "everyone allow list,search,add_file,file_inherit,directory_inherit",
        ])
        .arg(&output_parent)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        fs::metadata(&output_parent).unwrap().permissions().mode() & 0o777,
        0o700
    );

    install_curl(&fixture.tree, "#!/bin/sh\nexit 99\n");
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    fixture.append_args(
        &mut command,
        &fixture.tree.join("guest"),
        &output_parent.join("release"),
    );
    command.env("PATH", &fixture.tree);
    fixture.reject_command(
        command,
        "release output parent has a macOS access control list",
    );
    assert_eq!(fs::read_dir(output_parent).unwrap().count(), 0);
}
