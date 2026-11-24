//! Sign release code and bind the notarization submission to the verified disk image.

use std::{
    ffi::OsStr,
    fs::{self, File, Permissions},
    io,
    os::unix::fs::{PermissionsExt as _, symlink},
    path::Path,
    process::Command,
};

use serde::Deserialize;

use crate::{
    release_notices::validation::require_complete,
    release_resources::platform as staging,
    release_signature::platform::{DAEMON, GUI, signing_inventory, verify, verify_disk_image},
    upstream::sha256,
};

use super::{
    APP, CODESIGN, DMG, HDIUTIL, IDENTITY, NOTARY_PROFILE, RECEIPT, Receipt, XCRUN, checked,
    copy_bundle, invalid, release_setting, team, tool, valid_uuid,
    verification::{cdhash, check_mounted_app, verify_downloads},
    with_output, write_receipt,
};

const ENTITLEMENTS: &str = include_str!("../../../../packaging/macos/marsad.entitlements.plist");

pub(in crate::release_delivery) fn sign_release(source: &Path, output: &Path) -> io::Result<()> {
    let profile = release_setting(NOTARY_PROFILE)?;
    sign(source, output, Some(&profile))
}

pub(in crate::release_delivery) fn sign_local(source: &Path, output: &Path) -> io::Result<()> {
    sign(source, output, None)
}

fn sign(source: &Path, output: &Path, profile: Option<&OsStr>) -> io::Result<()> {
    let team = team()?;
    let identity = release_setting(IDENTITY)?;
    let source = std::path::absolute(source)?;
    staging::directory(&source)?;
    if source.extension() != Some(OsStr::new("app")) {
        return Err(invalid("the signing input must be a real .app directory"));
    }
    signing_inventory(&source)?;
    with_output(&source, output, |work, output| {
        let app = output.join(APP);
        let entitlements = work.join("marsad.entitlements.plist");
        let dmg = output.join(DMG);
        copy_bundle(&source, &app)?;
        if profile.is_some() {
            require_complete(&app.join("Contents/Resources/licenses"))?;
        }
        fs::write(&entitlements, ENTITLEMENTS)?;
        sign_bundle(&app, &identity, &entitlements)?;
        verify(&app, &team)?;
        let Some(profile) = profile else {
            return Ok(());
        };
        let app_cdhash = cdhash(&app)?;

        let volume = work.join("volume");
        fs::create_dir(&volume)?;
        copy_bundle(&app, &volume.join(APP))?;
        symlink("/Applications", volume.join("Applications"))?;
        checked(
            tool(HDIUTIL)
                .args([
                    "create",
                    "-format",
                    "UDZO",
                    "-volname",
                    "Marsa",
                    "-srcfolder",
                ])
                .arg(volume)
                .arg(&dmg),
            "could not build the installer disk image",
        )?;
        checked(
            tool(CODESIGN)
                .arg("--sign")
                .arg(&identity)
                .args([
                    "--timestamp",
                    "--identifier",
                    "io.github.nasraldin.marsa.installer",
                ])
                .arg(&dmg),
            "could not sign the installer disk image",
        )?;

        fs::set_permissions(&dmg, Permissions::from_mode(0o400))?;
        let submitted_sha256 = sha256(File::open(&dmg)?)?;
        verify_disk_image(&dmg, &team)?;
        check_mounted_app(&dmg, work, &team, &app_cdhash)?;
        let notarization_id = submit(&dmg, profile, &submitted_sha256)?;

        fs::set_permissions(&dmg, Permissions::from_mode(0o600))?;
        staple(&dmg)?;
        staple(&app)?;
        verify_downloads(&app, &dmg, work, &team, &app_cdhash)?;
        let receipt = Receipt {
            notarization_id,
            submitted_sha256,
            stapled_dmg_sha256: sha256(File::open(&dmg)?)?,
            app_cdhash,
        };
        write_receipt(&output.join(RECEIPT), &receipt)
    })
}

fn sign_bundle(app: &Path, identity: &OsStr, entitlements: &Path) -> io::Result<()> {
    let mut native = signing_inventory(app)?.into_iter().collect::<Vec<_>>();
    native.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for relative in native {
        if relative != Path::new(GUI) {
            let mut sign = signer(identity);
            if relative == Path::new(DAEMON) {
                sign.arg("--entitlements").arg(entitlements);
            }
            checked(
                sign.arg(app.join(relative)),
                "could not sign embedded release code",
            )?;
        }
    }
    // macOS signs the main Mach-O as part of its enclosing application seal.
    checked(signer(identity).arg(app), "could not sign the app bundle")?;
    Ok(())
}

fn signer(identity: &OsStr) -> Command {
    let mut sign = tool(CODESIGN);
    sign.args(["--force", "--options", "runtime", "--timestamp", "--sign"])
        .arg(identity);
    sign
}

fn submit(dmg: &Path, profile: &OsStr, expected_sha256: &str) -> io::Result<String> {
    let mut command = tool(XCRUN);
    command
        .args(["notarytool", "submit"])
        .arg(dmg)
        .arg("--keychain-profile")
        .arg(profile)
        .args(["--wait", "--timeout", "60m", "--output-format", "json"]);
    // Checked immediately before spawning, in the same private stage and on
    // the exact path which was mounted and checked.
    if sha256(File::open(dmg)?)? != expected_sha256 {
        return Err(invalid("the verified disk image changed before submission"));
    }
    let result = command.output()?;
    if let Ok(reply) = serde_json::from_slice::<NotaryResponse>(&result.stdout)
        && let Some(id) = reply.id.filter(|id| valid_uuid(id))
    {
        if result.status.success() && reply.status == "Accepted" {
            return Ok(id);
        }
        return Err(invalid(format!(
            "Apple did not accept the notarization submission {id}"
        )));
    }
    Err(invalid(
        "Apple did not return a valid accepted notarization submission",
    ))
}

#[derive(Deserialize)]
struct NotaryResponse {
    status: String,
    id: Option<String>,
}

fn staple(path: &Path) -> io::Result<()> {
    checked(
        tool(XCRUN).args(["stapler", "staple"]).arg(path),
        "Apple stapler could not attach a ticket",
    )?;
    Ok(())
}
