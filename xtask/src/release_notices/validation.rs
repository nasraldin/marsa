//! The complete licence and source handoff required by distribution builds.

use std::{
    fs::File,
    io::{self, Read as _},
    path::{Component, Path},
};

use serde_json::Value;

use crate::{release_resources::platform, upstream::sha256};

const MIB: u64 = 1024 * 1024;
const TEXT_LIMIT: u64 = 4 * MIB;
const SBOM_LIMIT: u64 = 32 * MIB;

/// The per-file limit for original text preserved by stage-notices.py.
pub(crate) const ORIGINAL_TEXT_LIMIT: u64 = 8 * MIB;
const NOTICES: &str = "THIRD-PARTY-NOTICES.txt";
const APP_SBOM: &str = "marsa-sbom.cdx.json";
const GUEST_SBOM: &str = "guest-sbom.cdx.json";
const GUEST_NOTICES: &str = "guest-notices.tar";
const SOURCES: &str = "guest-sources.json";
const CONFIG: &str = "kernel.config";

/// The required staged notices, source references, and generated inventories.
pub(crate) const REQUIRED: [(&str, u64); 6] = [
    (NOTICES, TEXT_LIMIT),
    (APP_SBOM, SBOM_LIMIT),
    (GUEST_SBOM, SBOM_LIMIT),
    (GUEST_NOTICES, 128 * MIB),
    (SOURCES, TEXT_LIMIT),
    (CONFIG, TEXT_LIMIT),
];

/// Revalidate the notices after app assembly and before any distribution work.
pub(crate) fn require_complete(licenses: &Path) -> io::Result<()> {
    let root = platform::directory(licenses)?;
    for (name, maximum) in REQUIRED {
        required_file(&root, name, maximum)?;
    }
    for name in ["LICENSE-Marsa.txt", "OFL-IBM-Plex.txt"] {
        required_file(&root, name, TEXT_LIMIT)?;
    }

    let sources = json(&root, SOURCES, TEXT_LIMIT)?;
    let kernel = &sources["kernel"];
    let configuration_hash = required_file(&root, CONFIG, TEXT_LIMIT).and_then(sha256)?;
    if sources["version"] != 1
        || sources["licence_texts"] != GUEST_NOTICES
        || kernel["configuration"] != CONFIG
        || kernel["configuration_sha256"] != configuration_hash
        || !kernel["url"].as_str().is_some_and(|url| {
            url.starts_with("https://cdn.kernel.org/pub/linux/kernel/")
                && !url.bytes().any(|byte| byte.is_ascii_whitespace())
        })
        || !kernel["sha256"].as_str().is_some_and(|hash| {
            hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(invalid(
            "the kernel source and configuration evidence is incomplete",
        ));
    }

    let app = json(&root, APP_SBOM, SBOM_LIMIT)?;
    let guest = json(&root, GUEST_SBOM, SBOM_LIMIT)?;
    for document in [&app, &guest] {
        if document["bomFormat"] != "CycloneDX"
            || document["specVersion"] != "1.7"
            || document["components"].as_array().is_none_or(Vec::is_empty)
        {
            return Err(invalid(
                "a release CycloneDX inventory is missing or incomplete",
            ));
        }
    }
    // Supplier hashes describe the authenticated inputs to signing. The
    // Developer ID verifier independently checks the resulting signed bytes.
    if app["metadata"]["component"]["bom-ref"] != "marsa-release" {
        return Err(invalid(
            "the combined SBOM does not identify the Marsa release",
        ));
    }
    let guest_dir = licenses
        .parent()
        .ok_or_else(|| invalid("expected the app's Resources/licenses directory"))?
        .join("guest");
    let guest_dir = platform::directory(&guest_dir)?;
    let manifest = json(&guest_dir, "guest-manifest.json", TEXT_LIMIT)?;
    if !guest["metadata"]["component"]["hashes"]
        .as_array()
        .is_some_and(|hashes| {
            hashes.iter().any(|hash| {
                hash["alg"] == "SHA-256"
                    && hash["content"].is_string()
                    && hash["content"] == manifest["rootfs_sha256"]
            })
        })
    {
        return Err(invalid(
            "the guest SBOM does not identify the bundled guest image",
        ));
    }
    require_indexed_notices(licenses, &root)
}

fn require_indexed_notices(licenses: &Path, root: &File) -> io::Result<()> {
    let mut index = String::new();
    required_file(root, NOTICES, TEXT_LIMIT)?
        .take(TEXT_LIMIT + 1)
        .read_to_string(&mut index)?;
    let mut groups = [false; 3];
    for entry in index.lines().filter_map(|line| line.strip_prefix("  ")) {
        if matches!(entry, GUEST_NOTICES | "OFL-IBM-Plex.txt") {
            continue;
        }
        let path = Path::new(entry);
        if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(invalid("the notice index contains an invalid text path"));
        }
        let group = ["texts/rust", "texts/suppliers", "texts/icons"]
            .iter()
            .position(|prefix| path.starts_with(prefix))
            .ok_or_else(|| invalid("the notice index contains an unexpected text path"))?;
        let parent = path
            .parent()
            .ok_or_else(|| invalid("incomplete notice text path"))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("incomplete notice text path"))?;
        required_file(
            &platform::directory(&licenses.join(parent))?,
            name,
            ORIGINAL_TEXT_LIMIT,
        )?;
        groups[group] = true;
    }
    if groups.contains(&false) {
        return Err(invalid("the release dependency notice index is incomplete"));
    }
    Ok(())
}

fn required_file(directory: &File, name: &str, maximum: u64) -> io::Result<File> {
    let file = platform::input_file(directory, name).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("missing or invalid release evidence {name}: {error}"),
        )
    })?;
    if !(1..=maximum).contains(&file.metadata()?.len()) {
        return Err(invalid(format!("invalid size for release evidence {name}")));
    }
    Ok(file)
}

fn json(directory: &File, name: &str, maximum: u64) -> io::Result<Value> {
    serde_json::from_reader(required_file(directory, name, maximum)?.take(maximum + 1))
        .map_err(|_| invalid(format!("invalid JSON in release evidence {name}")))
}

fn invalid(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
