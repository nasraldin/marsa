//! Re-verify notarized inputs and produce the installer, update archive, and Homebrew cask.

use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

use crate::{
    release_notices::validation::require_complete, release_resources::platform as staging,
    release_signature::platform::verify, upstream::sha256,
};

use super::{
    APP, DITTO, DMG, RECEIPT, checked, copy_bundle, invalid, read_receipt, team, tool,
    verification::{assess_app, cdhash, validate_staple, verify_downloads},
    with_output, write_receipt,
};

const CASK: &str = include_str!("../../../../packaging/macos/marsa.rb.in");

pub(in crate::release_delivery) fn package_release(source: &Path, output: &Path) -> io::Result<()> {
    let team = team()?;
    let source = std::path::absolute(source)?;
    let signed = staging::directory(&source)?;
    staging::directory(&source.join(APP))?;
    let receipt = read_receipt(staging::input_file(&signed, RECEIPT).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            invalid("packaging requires the notarization receipt from cargo xtask sign")
        } else {
            error
        }
    })?)?;
    let dmg = staging::input_file(&signed, DMG)?;
    with_output(&source, output, |work, output| {
        let app = work.join(APP);
        copy_bundle(&source.join(APP), &app)?;
        verify(&app, &team)?;
        let licenses = app.join("Contents/Resources/licenses");
        require_complete(&licenses)?;
        let version = bundle_version(&app)?;
        let dmg_name = format!("Marsa-{version}.dmg");
        let zip_name = format!("Marsa-{version}.zip");
        let installer = output.join(&dmg_name);
        let archive = output.join(&zip_name);
        let mut staged_dmg = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&installer)?;
        io::copy(&mut &dmg, &mut staged_dmg)?;
        staged_dmg.sync_all()?;
        if sha256(File::open(&installer)?)? != receipt.stapled_dmg_sha256 {
            return Err(invalid(
                "the stapled DMG digest differs from the signing receipt",
            ));
        }
        verify_downloads(&app, &installer, work, &team, &receipt.app_cdhash)?;
        checked(
            tool(DITTO)
                .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
                .arg(&app)
                .arg(&archive),
            "could not create the update archive",
        )?;
        let extracted = work.join("archive");
        fs::create_dir(&extracted)?;
        checked(
            tool(DITTO).args(["-x", "-k"]).arg(&archive).arg(&extracted),
            "could not inspect the generated update archive",
        )?;
        let unpacked = extracted.join(APP);
        verify(&unpacked, &team)?;
        if cdhash(&unpacked)? != receipt.app_cdhash {
            return Err(invalid("the update archive contains a different app"));
        }
        validate_staple(&unpacked)?;
        assess_app(&unpacked)?;
        fs::write(
            output.join("SHA256SUMS"),
            format!(
                "{}  {dmg_name}\n{}  {zip_name}\n",
                receipt.stapled_dmg_sha256,
                sha256(File::open(&archive)?)?
            ),
        )?;
        write_receipt(&output.join(RECEIPT), &receipt)?;
        checked(
            tool(DITTO).arg(licenses).arg(output.join("notices")),
            "could not publish release notices and source inventory",
        )?;
        fs::write(
            output.join("marsa.rb"),
            CASK.replace("@VERSION@", &version)
                .replace("@DMG_SHA256@", &receipt.stapled_dmg_sha256),
        )
    })
}

fn bundle_version(app: &Path) -> io::Result<String> {
    let result = checked(
        tool("/usr/bin/plutil")
            .args([
                "-extract",
                "CFBundleShortVersionString",
                "raw",
                "-expect",
                "string",
                "-o",
                "-",
                "--",
            ])
            .arg(app.join("Contents/Info.plist")),
        "could not read the verified app version",
    )?;
    let version = std::str::from_utf8(&result.stdout)
        .map_err(io::Error::other)?
        .trim();
    let parts = version.split('.').collect::<Vec<_>>();
    if version.len() > 48
        || parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(invalid(
            "the release app version must have three numeric components",
        ));
    }
    Ok(version.to_owned())
}
