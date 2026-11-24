//! Verify complete Apple distribution artifacts, including the app mounted from the disk image.

use std::{fs, io, path::Path};

use crate::release_signature::platform::{verify, verify_disk_image};

use super::{APP, CODESIGN, HDIUTIL, XCRUN, checked, invalid, tool, valid_hash};

pub(super) fn check_mounted_app(
    dmg: &Path,
    work: &Path,
    team: &str,
    expected: &str,
) -> io::Result<()> {
    let mount = tempfile::Builder::new()
        .prefix("read-only-")
        .tempdir_in(work)?;
    let path = mount.path();
    checked(
        tool(HDIUTIL)
            .args([
                "attach",
                "-readonly",
                "-verify",
                "-mount",
                "required",
                "-nobrowse",
                "-noautoopen",
                "-mountpoint",
            ])
            .arg(path)
            .arg(dmg),
        "could not mount the disk image read-only",
    )?;
    let result = (|| {
        let app = path.join(APP);
        verify(&app, team)?;
        if cdhash(&app)? != expected {
            return Err(invalid("the mounted disk image contains a different app"));
        }
        let link = fs::read_link(path.join("Applications"))?;
        if link != Path::new("/Applications") {
            return Err(invalid(
                "the disk image has an unexpected Applications link",
            ));
        }
        Ok(())
    })();
    let detach = checked(
        tool(HDIUTIL).arg("detach").arg(path),
        "could not detach the disk image",
    );
    match detach {
        Ok(_) => result,
        Err(error) => {
            let _ = tool(HDIUTIL).arg("detach").arg(path).arg("-force").output();
            Err(error)
        }
    }
}

pub(super) fn validate_staple(path: &Path) -> io::Result<()> {
    checked(
        tool(XCRUN).args(["stapler", "validate"]).arg(path),
        "Apple stapler could not validate the ticket",
    )?;
    Ok(())
}

pub(super) fn verify_downloads(
    app: &Path,
    dmg: &Path,
    work: &Path,
    team: &str,
    expected: &str,
) -> io::Result<()> {
    verify(app, team)?;
    if cdhash(app)? != expected {
        return Err(invalid(
            "the stapled app does not match the signing receipt",
        ));
    }
    verify_disk_image(dmg, team)?;
    validate_staple(app)?;
    validate_staple(dmg)?;
    assess_app(app)?;
    checked(
        tool("/usr/sbin/spctl")
            .args([
                "--assess",
                "--type",
                "open",
                "--context",
                "context:primary-signature",
            ])
            .arg(dmg),
        "Gatekeeper rejected the installer disk image",
    )?;
    check_mounted_app(dmg, work, team, expected)
}

pub(super) fn assess_app(app: &Path) -> io::Result<()> {
    checked(
        tool("/usr/bin/syspolicy_check")
            .arg("distribution")
            .arg(app),
        "Gatekeeper rejected the app",
    )?;
    Ok(())
}

pub(super) fn cdhash(app: &Path) -> io::Result<String> {
    let result = checked(
        tool(CODESIGN).args(["--display", "--verbose=4"]).arg(app),
        "could not read the app CodeDirectory hash",
    )?;
    let text = std::str::from_utf8(&result.stderr).map_err(io::Error::other)?;
    let mut hashes = text.lines().filter_map(|line| line.strip_prefix("CDHash="));
    let hash = hashes.next().filter(|hash| valid_hash(hash, 40));
    match (hash, hashes.next()) {
        (Some(hash), None) => Ok(hash.to_owned()),
        _ => Err(invalid(
            "the app does not have exactly one CodeDirectory hash",
        )),
    }
}
