//! Inspect the macOS release envelope and embedded CodeDirectories with Apple tools.

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::{Value, json};

const CODESIGN: &str = "/usr/bin/codesign";
const PLUTIL: &str = "/usr/bin/plutil";
const APP_ID: &str = "io.github.nasraldin.marsa";
const GUI: &str = "Contents/MacOS/marsa-gui";
const DAEMON: &str = "Contents/MacOS/marsad";
const ROLES: &[&str] = &[
    GUI,
    DAEMON,
    "Contents/MacOS/marsa",
    "Contents/Helpers/marsa-net-helper",
    "Contents/Resources/bin/docker",
    "Contents/Resources/bin/docker-credential-osxkeychain",
    "Contents/Resources/bin/cli-plugins/docker-buildx",
    "Contents/Resources/bin/cli-plugins/docker-compose",
];
const VIRTUALIZATION: &str = "com.apple.security.virtualization";
// Apple xnu, osfmk/kern/cs_blobs.h.
const CS_RUNTIME: u32 = 0x0001_0000;
// Apple SDK <mach/machine.h> and <mach-o/loader.h>.
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const MH_EXECUTE: u32 = 2;

pub(super) fn verify(bundle: &Path, team: &str) -> io::Result<usize> {
    if bundle.extension() != Some(OsStr::new("app")) || !bundle.symlink_metadata()?.is_dir() {
        return Err(invalid("expected a real .app directory"));
    }
    // Use absolute paths for Apple tool operands even if the caller supplied a
    // relative name beginning with a dash.
    let root = bundle.canonicalize()?;
    let code = inventory(&root)?;
    check_info(&root)?;

    verify_signature(&root, team, true)?;
    for relative in &code {
        let path = root.join(relative);
        verify_signature(&path, team, false)?;
        check_policy(&path, relative == Path::new(DAEMON))?;
    }
    Ok(code.len())
}

fn inventory(root: &Path) -> io::Result<BTreeSet<PathBuf>> {
    let mut paths = vec![root.to_path_buf()];
    let mut machos = BTreeSet::new();
    while let Some(directory) = paths.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                paths.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(invalid_at("unsupported bundle entry", &path));
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| invalid("bundle path escaped the input directory"))?;
            let required = ROLES.iter().any(|role| relative == Path::new(role));
            match native_code(&path)? {
                Some(file_type) => {
                    if metadata.nlink() != 1 || required && file_type != MH_EXECUTE {
                        return Err(invalid_at("unexpected Mach-O role or hard link", &path));
                    }
                    machos.insert(relative.to_owned());
                }
                None if required => return Err(invalid_at("missing a native executable", &path)),
                None => (),
            }
        }
    }
    for role in ROLES {
        if !machos.contains(Path::new(role)) {
            return Err(invalid_at("missing a native executable", Path::new(role)));
        }
    }
    Ok(machos)
}

fn native_code(path: &Path) -> io::Result<Option<u32>> {
    let mut header = Vec::with_capacity(16);
    File::open(path)?.take(16).read_to_end(&mut header)?;
    if header.starts_with(&0xfeed_facf_u32.to_le_bytes()) {
        // mach_header_64 begins with magic, cputype, cpusubtype, and filetype.
        let Some(fields) = header.first_chunk::<16>() else {
            return Err(invalid_at("truncated Mach-O header", path));
        };
        let [_, _, _, _, c0, c1, c2, c3, _, _, _, _, t0, t1, t2, t3] = *fields;
        if u32::from_le_bytes([c0, c1, c2, c3]) != CPU_TYPE_ARM64 {
            return Err(invalid_at("expected Mach-O arm64", path));
        }
        return Ok(Some(u32::from_le_bytes([t0, t1, t2, t3])));
    }
    // Reject legacy, byte-swapped, and universal native code. This release
    // consists of thin arm64 executables and must not skip a second slice.
    if [
        0xfeed_face_u32,
        0xcefa_edfe,
        0xfeed_facf,
        0xcafe_babe,
        0xbeba_feca,
        0xcafe_babf,
        0xbfba_feca,
    ]
    .iter()
    .any(|magic| header.starts_with(&magic.to_be_bytes()))
    {
        return Err(invalid_at("expected thin little-endian Mach-O arm64", path));
    }
    Ok(None)
}

fn check_info(root: &Path) -> io::Result<()> {
    let plist = root.join("Contents/Info.plist");
    for (key, expected) in [
        ("CFBundleIdentifier", APP_ID),
        ("CFBundleExecutable", "marsa-gui"),
        ("CFBundlePackageType", "APPL"),
    ] {
        let output = command(PLUTIL)
            .args(["-extract", key, "raw", "-expect", "string", "-o", "-", "--"])
            .arg(&plist)
            .output()?;
        if !output.status.success()
            || output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout) != expected.as_bytes()
        {
            return Err(invalid(format!("unexpected {key} in Contents/Info.plist")));
        }
    }
    Ok(())
}

fn verify_signature(path: &Path, team: &str, outer: bool) -> io::Result<()> {
    if !signature_result(path, team, outer)?.status.success() {
        return Err(invalid_at(
            "strict Developer ID Application signature verification failed",
            path,
        ));
    }
    Ok(())
}

fn signature_result(path: &Path, team: &str, outer: bool) -> io::Result<Output> {
    // The caller validates the Team ID before it can reach this expression.
    // TN3127 requires the Apple anchor when evaluating the certificate OIDs.
    // A leading equals sign passes literal source instead of a requirement file.
    let mut requirement = format!(
        "=anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{team}\""
    );
    let mut signer = command(CODESIGN);
    signer.args(["--verify", "--strict", "--all-architectures"]);
    if outer {
        signer.arg("--deep");
        requirement.push_str(&format!(" and identifier \"{APP_ID}\""));
    }
    signer
        .args(["--test-requirement", &requirement])
        .arg(path)
        .output()
}

fn check_policy(path: &Path, needs_virtualization: bool) -> io::Result<()> {
    let output = command(CODESIGN)
        .args(["--display", "--verbose=4", "--xml", "--entitlements", "-"])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(invalid_at("could not inspect code signature", path));
    }
    let flags = code_directory_flags(&output.stderr);
    if flags.is_none_or(|flags| flags & CS_RUNTIME == 0) {
        return Err(invalid_at("missing Hardened Runtime flag", path));
    }
    let entitlements = if output.stdout.is_empty() {
        json!({})
    } else {
        let mut plist = tempfile::NamedTempFile::new()?;
        plist.write_all(&output.stdout)?;
        let converted = command(PLUTIL)
            .args(["-convert", "json", "-o", "-", "--"])
            .arg(plist.path())
            .output()?;
        if !converted.status.success() {
            return Err(invalid_at("could not decode code entitlements", path));
        }
        serde_json::from_slice::<Value>(&converted.stdout)
            .map_err(|_| invalid_at("invalid code entitlements", path))?
    };
    let expected = if needs_virtualization {
        json!({ VIRTUALIZATION: true })
    } else {
        json!({})
    };
    if entitlements != expected {
        return Err(invalid_at(
            "unexpected entitlements for this code role",
            path,
        ));
    }
    Ok(())
}

fn code_directory_flags(stderr: &[u8]) -> Option<u32> {
    let mut lines = std::str::from_utf8(stderr)
        .ok()?
        .lines()
        .filter(|line| line.starts_with("CodeDirectory "));
    let line = lines.next()?;
    // Reject an ambiguous display, including a path containing a fake line.
    if lines.next().is_some() {
        return None;
    }
    let (_, flags) = line.split_once(" flags=0x")?;
    let (hex, _) = flags.split_once('(')?;
    u32::from_str_radix(hex, 16).ok()
}

fn command(path: &str) -> Command {
    let mut command = Command::new(path);
    command.env("LC_ALL", "C");
    command
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn invalid_at(message: &str, path: &Path) -> io::Error {
    invalid(format!("{message} at {path:?}"))
}

#[cfg(all(test, target_arch = "aarch64"))]
mod tests {
    use super::*;

    const EXTRA: &str = "Contents/Resources/nested/extra-binary.dat";
    const TEST_TEAM: &str = "TESTTEAM01";

    #[test]
    fn structurally_valid_hardened_adhoc_app_cannot_pass_release_verification() {
        let scratch = tempfile::tempdir().unwrap();
        let app = scratch.path().join("Fixture with spaces.app");
        let main = app.join(GUI);
        let source = std::env::current_exe().unwrap();
        let vm = scratch.path().join("virtualization.plist");
        write_plist(&vm, &format!("<key>{VIRTUALIZATION}</key><true/>"));
        for role in ROLES.iter().copied().chain([EXTRA]) {
            let path = app.join(role);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::copy(&source, &path).unwrap();
            if path != main {
                sign(&path, true, (role == DAEMON).then_some(vm.as_path()));
            }
        }
        write_plist(
            &app.join("Contents/Info.plist"),
            &format!(
                "<key>CFBundleIdentifier</key><string>{APP_ID}</string>\
                 <key>CFBundleExecutable</key><string>marsa-gui</string>\
                 <key>CFBundlePackageType</key><string>APPL</string>"
            ),
        );
        let resource = app.join("Contents/Resources/receipt.txt");
        fs::write(&resource, "sealed").unwrap();
        sign(&app, true, None);
        assert!(structurally_signed(&app));

        // The outer identifier applies to the app, even when validating nested
        // code with different designated identifiers.
        let outer = command(CODESIGN)
            .args(["--verify", "--strict", "--deep", "--test-requirement"])
            .arg(format!("=identifier \"{APP_ID}\""))
            .arg(&app)
            .output()
            .unwrap();
        assert!(
            outer.status.success(),
            "{}",
            String::from_utf8_lossy(&outer.stderr)
        );

        check_info(&app).unwrap();
        let code = inventory(&app).unwrap();
        assert_eq!(code.len(), ROLES.len() + 1);
        assert!(code.contains(Path::new(EXTRA)));
        for role in code {
            let path = app.join(&role);
            assert!(structurally_signed(&path), "{role:?}");
            check_policy(&path, role == Path::new(DAEMON)).unwrap();
        }

        let rejected = signature_result(&app, TEST_TEAM, true).unwrap();
        let reason = String::from_utf8_lossy(&rejected.stderr);
        assert!(!rejected.status.success());
        assert!(
            reason.contains("code failed to satisfy specified code requirement"),
            "{reason}"
        );
        let error = verify(&app, TEST_TEAM).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("strict Developer ID Application signature verification failed"),
            "{error}"
        );
        fs::write(&resource, "changed").unwrap();
        assert!(!structurally_signed(&app));
    }

    #[test]
    fn signed_macho_with_wrong_runtime_or_entitlements_fails_policy() {
        let scratch = tempfile::tempdir().unwrap();
        let fixture = scratch.path().join("fixture");
        fs::copy(std::env::current_exe().unwrap(), &fixture).unwrap();
        sign(&fixture, false, None);
        assert!(structurally_signed(&fixture));
        assert!(
            check_policy(&fixture, false)
                .unwrap_err()
                .to_string()
                .contains("Hardened Runtime")
        );
        sign(&fixture, true, None);
        assert!(
            check_policy(&fixture, true)
                .unwrap_err()
                .to_string()
                .contains("entitlements")
        );

        let unsafe_entitlements = scratch.path().join("unsafe.plist");
        write_plist(
            &unsafe_entitlements,
            "<key>com.apple.security.get-task-allow</key><true/>",
        );
        sign(&fixture, true, Some(&unsafe_entitlements));
        assert!(structurally_signed(&fixture));
        assert!(
            check_policy(&fixture, false)
                .unwrap_err()
                .to_string()
                .contains("entitlements")
        );
    }

    #[test]
    fn runtime_flag_is_unknown_if_codesign_displays_multiple_directories() {
        assert_eq!(
            code_directory_flags(b"CodeDirectory v=20400 flags=0x10000(runtime)"),
            Some(CS_RUNTIME)
        );
        assert_eq!(code_directory_flags(
            b"Executable=/a/path/with\nCodeDirectory v=20400 flags=0x10000(runtime)\nCodeDirectory v=20400 flags=0x2(adhoc)"
        ), None);
    }

    #[test]
    fn inventory_does_not_follow_bundle_symlinks() {
        let scratch = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), scratch.path().join("outside")).unwrap();
        let error = inventory(scratch.path()).unwrap_err();
        assert!(error.to_string().contains("unsupported bundle entry"));
    }

    fn structurally_signed(path: &Path) -> bool {
        command(CODESIGN)
            .args(["--verify", "--strict", "--deep", "--all-architectures"])
            .arg(path)
            .output()
            .unwrap()
            .status
            .success()
    }

    fn sign(path: &Path, runtime: bool, entitlements: Option<&Path>) {
        let mut signer = command(CODESIGN);
        signer.args(["--force", "--sign", "-", "--timestamp=none"]);
        if runtime {
            signer.args(["--options", "runtime"]);
        }
        if let Some(plist) = entitlements {
            signer.arg("--entitlements").arg(plist);
        }
        let output = signer.arg(path).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write_plist(path: &Path, values: &str) {
        fs::write(
            path,
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>{values}</dict></plist>"),
        )
        .unwrap();
    }
}
