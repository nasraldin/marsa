//! Revalidate the authenticated guest and the fixed release resource roles.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
    os::unix::fs::FileExt,
    path::Path,
};

use marsa_core::guest_manifest::GuestManifest;
use serde::Deserialize;
use serde_json::Value;

use super::{TEXT_LIMIT, copy_executable, copy_file, invalid, platform, read_small, write_file};
use crate::{
    guest,
    release_notices::validation::{ORIGINAL_TEXT_LIMIT, REQUIRED, require_complete},
    upstream::sha256,
};

const MANIFEST: &str = "guest-manifest.json";
const SBOM: &str = "upstream-sbom-input.json";
const FONT_NOTICE: &str = "OFL-IBM-Plex.txt";
const PROJECT_NOTICE: &str = "LICENSE-Marsa.txt";
const MIB: u64 = 1024 * 1024;
const FONTS: [&str; 7] = [
    "IBMPlexMono-Regular.ttf",
    "IBMPlexSans-Medium.ttf",
    "IBMPlexSans-Regular.ttf",
    "IBMPlexSans-SemiBold.ttf",
    "IBMPlexSansArabic-Medium.ttf",
    "IBMPlexSansArabic-Regular.ttf",
    "IBMPlexSansArabic-SemiBold.ttf",
];

pub(super) struct Staged {
    guest: File,
    bin: File,
    plugins: File,
    sbom: Vec<u8>,
}

impl Staged {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let root = platform::directory(path)?;
        platform::owned_output_parent(&root)?;
        platform::require_names(&root, &["Resources", SBOM])?;
        let sbom = read_small(platform::input_file(&root, SBOM)?, TEXT_LIMIT)?;
        platform::with_current_directory(&root, || {
            let resources = platform::directory(Path::new("Resources"))?;
            let guest = platform::directory(Path::new("Resources/guest"))?;
            let bin = platform::directory(Path::new("Resources/bin"))?;
            let plugins = platform::directory(Path::new("Resources/bin/cli-plugins"))?;
            for directory in [&resources, &guest, &bin, &plugins] {
                platform::owned_output_parent(directory)?;
            }
            platform::require_names(&resources, &["guest", "bin"])?;
            platform::require_names(&guest, &["Image", "rootfs.img", MANIFEST])?;
            platform::require_names(
                &bin,
                &["docker", "docker-credential-osxkeychain", "cli-plugins"],
            )?;
            platform::require_names(&plugins, &["docker-compose", "docker-buildx"])?;
            Ok(Self {
                guest,
                bin,
                plugins,
                sbom,
            })
        })
    }

    pub(super) fn install(&self, output: &Path, guest_sha256: &str) -> io::Result<()> {
        self.install_guest(&output.join("guest"), guest_sha256)?;
        self.install_upstream(&output.join("bin"))?;
        write_file(&output.join("licenses").join(SBOM), &self.sbom, 0o644)
    }

    fn install_guest(&self, output: &Path, expected: &str) -> io::Result<()> {
        let bytes = read_small(platform::input_file(&self.guest, MANIFEST)?, 16 * 1024)?;
        if sha256(bytes.as_slice())? != expected {
            return Err(invalid("trusted guest manifest SHA-256 mismatch"));
        }
        let manifest = GuestManifest::parse(&bytes).map_err(io::Error::other)?;
        guest::verify_release_manifest(include_str!("../../../guest/versions.toml"), &manifest)?;
        let kernel = output.join("Image");
        let rootfs = output.join("rootfs.img");
        copy_file(
            platform::input_file(&self.guest, "Image")?,
            &kernel,
            64 * MIB,
            0o644,
        )?;
        copy_file(
            platform::input_file(&self.guest, "rootfs.img")?,
            &rootfs,
            1024 * MIB,
            0o644,
        )?;
        let kernel = File::open(kernel)?;
        let rootfs = File::open(rootfs)?;
        manifest
            .verify(&kernel, &rootfs)
            .map_err(io::Error::other)?;
        let mut kernel_magic = [0; 4];
        let mut rootfs_magic = [0; 4];
        kernel.read_exact_at(&mut kernel_magic, 56)?;
        rootfs.read_exact_at(&mut rootfs_magic, 1024)?;
        if kernel_magic != *b"ARM\x64"
            || rootfs_magic != 0xe0f5_e1e2_u32.to_le_bytes()
            || !rootfs.metadata()?.len().is_multiple_of(512)
        {
            return Err(invalid(
                "expected an ARM64 guest kernel and EROFS root disk",
            ));
        }
        write_file(&output.join(MANIFEST), &bytes, 0o644)
    }

    fn install_upstream(&self, output: &Path) -> io::Result<()> {
        let pins: BTreeMap<String, Pin> =
            toml::from_str(include_str!("../../../packaging/upstream-versions.toml"))
                .map_err(io::Error::other)?;
        let sbom: Value = serde_json::from_slice(&self.sbom).map_err(io::Error::other)?;
        let roles = [
            ("docker", "cli", "docker", &self.bin, output.to_owned()),
            (
                "compose",
                "compose",
                "docker-compose",
                &self.plugins,
                output.join("cli-plugins"),
            ),
            (
                "buildx",
                "buildx",
                "docker-buildx",
                &self.plugins,
                output.join("cli-plugins"),
            ),
            (
                "credential_helper",
                "docker-credential-helpers",
                "docker-credential-osxkeychain",
                &self.bin,
                output.to_owned(),
            ),
        ];
        let components = sbom["components"]
            .as_array()
            .filter(|components| components.len() == roles.len())
            .ok_or_else(|| invalid("expected the four staged upstream components"))?;
        for (key, repository, name, source, destination) in roles {
            let pin = pins
                .get(key)
                .ok_or_else(|| invalid(format!("missing upstream pin for {name}")))?;
            let component = components
                .iter()
                .find(|component| component["name"] == name)
                .ok_or_else(|| invalid(format!("missing staged upstream component {name}")))?;
            let installed_hash = verify_component(component, pin, repository, name)?;
            let destination = destination.join(name);
            copy_executable(platform::input_file(source, name)?, &destination)?;
            require_hash(&destination, installed_hash)?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Pin {
    version: String,
    sha256: String,
}

fn verify_component<'a>(
    component: &'a Value,
    pin: &Pin,
    repository: &str,
    name: &str,
) -> io::Result<&'a str> {
    let purl = format!("pkg:github/docker/{repository}@v{}", pin.version);
    let distribution = component["externalReferences"]
        .as_array()
        .and_then(|references| {
            references
                .iter()
                .find(|reference| reference["type"] == "distribution")
        });
    let installed_hash = component_hash(&component["hashes"])
        .ok_or_else(|| invalid(format!("{name}: missing staged executable SHA-256")))?;
    guest::digest(installed_hash)?;
    // Docker CLI pins an archive. Its executable hash is computed at
    // extraction by stage-release-resources. Each other pin is a standalone
    // executable and can be checked directly against its published SHA-256.
    if component["type"] != "application"
        || component["version"] != pin.version
        || component["purl"] != purl
        || distribution.and_then(|reference| component_hash(&reference["hashes"]))
            != Some(pin.sha256.as_str())
        || (name != "docker" && installed_hash != pin.sha256)
    {
        return Err(invalid(format!(
            "{name}: staged component disagrees with the release pin"
        )));
    }
    Ok(installed_hash)
}

fn component_hash(hashes: &Value) -> Option<&str> {
    let [hash] = hashes.as_array()?.as_slice() else {
        return None;
    };
    (hash["alg"] == "SHA-256")
        .then(|| hash["content"].as_str())
        .flatten()
}

pub(super) fn install_workspace_assets(
    workspace: &Path,
    output: &Path,
    notices: Option<&Path>,
) -> io::Result<()> {
    let icons = platform::directory(&workspace.join("assets/icons"))?;
    let fonts = platform::directory(&workspace.join("assets/fonts"))?;
    let icon_hashes = checksum_inventory(&icons)?;
    let font_hashes = checksum_inventory(&fonts)?;
    copy_asset(&icons, &icon_hashes, "Marsa.icns", output)?;
    for font in FONTS {
        copy_asset(&fonts, &font_hashes, font, &output.join("fonts"))?;
    }
    let licenses = output.join("licenses");
    copy_asset(&fonts, &font_hashes, FONT_NOTICE, &licenses)?;
    install_optional_fonts(workspace, output)?;
    copy_file(
        platform::input_file(&platform::directory(workspace)?, "LICENSE")?,
        &licenses.join(PROJECT_NOTICE),
        TEXT_LIMIT,
        0o644,
    )?;
    for locale in ["en-US", "ar"] {
        let path = Path::new("locales").join(locale);
        let source = platform::directory(&workspace.join(&path))?;
        copy_file(
            platform::input_file(&source, "marsa.ftl")?,
            &output.join(path).join("marsa.ftl"),
            TEXT_LIMIT,
            0o644,
        )?;
    }
    install_release_notes(workspace, output)?;
    if let Some(notices) = notices {
        install_notices(notices, &licenses)?;
    }
    Ok(())
}

fn install_optional_fonts(workspace: &Path, output: &Path) -> io::Result<()> {
    let catalog = platform::directory(&workspace.join("assets/fonts/catalog"))?;
    let inventory = checksum_inventory(&catalog)?;
    let fonts = output.join("fonts");
    let notices = output.join("licenses/font-notices");
    for line in inventory.lines() {
        let (_, name) = line
            .split_once("  ")
            .ok_or_else(|| invalid("invalid optional font checksum line"))?;
        let destination = if name.ends_with(".ttf") {
            &fonts
        } else if name.ends_with("-notice.txt") {
            &notices
        } else {
            return Err(invalid("unexpected optional font asset"));
        };
        copy_asset(&catalog, &inventory, name, destination)?;
    }
    Ok(())
}

fn install_release_notes(workspace: &Path, output: &Path) -> io::Result<()> {
    let release_notes = platform::directory(&workspace.join("packaging/release-notes"))?;
    let bytes = read_small(
        platform::input_file(&release_notes, concat!(env!("CARGO_PKG_VERSION"), ".json"))?,
        32 * 1024,
    )?;
    let notes: Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if notes.as_object().is_none_or(|notes| notes.len() != 3)
        || notes["version"] != env!("CARGO_PKG_VERSION")
        || ["en-US", "ar"].into_iter().any(|locale| {
            notes[locale].as_str().is_none_or(|text| {
                text.trim().is_empty()
                    || text.len() > 8 * 1024
                    || text.chars().any(|ch| ch.is_control() && ch != '\n')
            })
        })
    {
        return Err(invalid(
            "expected release notes for the bundle version in en-US and ar",
        ));
    }
    write_file(&output.join("release-notes.json"), &bytes, 0o644)
}

fn checksum_inventory(directory: &File) -> io::Result<String> {
    let bytes = read_small(platform::input_file(directory, "SHA256SUMS")?, TEXT_LIMIT)?;
    String::from_utf8(bytes).map_err(io::Error::other)
}

fn copy_asset(directory: &File, inventory: &str, name: &str, target: &Path) -> io::Result<()> {
    let expected = inventory
        .lines()
        .filter_map(|line| line.split_once("  "))
        .find_map(|(digest, file)| (file == name).then_some(digest))
        .ok_or_else(|| invalid(format!("missing checked-in SHA-256 for {name}")))?;
    let path = target.join(name);
    copy_file(
        platform::input_file(directory, name)?,
        &path,
        16 * MIB,
        0o644,
    )?;
    require_hash(&path, expected)
}

fn require_hash(path: &Path, expected: &str) -> io::Result<()> {
    if sha256(File::open(path)?)? != expected {
        return Err(invalid(format!("{}: SHA-256 mismatch", path.display())));
    }
    Ok(())
}

fn install_notices(source: &Path, output: &Path) -> io::Result<()> {
    let input = platform::directory(source)?;
    platform::owned_output_parent(&input)?;
    let mut names = vec![PROJECT_NOTICE, FONT_NOTICE, "texts"];
    names.extend(REQUIRED.iter().map(|(name, _)| *name));
    platform::require_names(&input, &names)?;
    for name in [PROJECT_NOTICE, FONT_NOTICE] {
        let supplied = read_small(platform::input_file(&input, name)?, TEXT_LIMIT)?;
        require_hash(&output.join(name), &sha256(supplied.as_slice())?)?;
    }
    for (name, limit) in REQUIRED {
        copy_file(
            platform::input_file(&input, name)?,
            &output.join(name),
            limit,
            0o644,
        )?;
    }
    let source_texts = source.join("texts");
    let target_texts = output.join("texts");
    let mut count = 0;
    let mut bytes = 0;
    copy_notice_tree(&source_texts, &target_texts, 0, &mut count, &mut bytes)?;
    if count == 0 {
        return Err(invalid("release dependency notice texts are missing"));
    }
    require_complete(output)
}

fn copy_notice_tree(
    source: &Path,
    output: &Path,
    depth: u8,
    count: &mut usize,
    bytes: &mut u64,
) -> io::Result<()> {
    if depth > 8 {
        return Err(invalid("release notice directory is nested too deeply"));
    }
    let directory = platform::directory(source)?;
    platform::owned_output_parent(&directory)?;
    fs::create_dir(output)?;
    for item in fs::read_dir(source)? {
        let item = item?;
        let kind = item.file_type()?;
        let name = item.file_name();
        let destination = output.join(&name);
        if kind.is_dir() {
            copy_notice_tree(&item.path(), &destination, depth + 1, count, bytes)?;
        } else if kind.is_file() {
            let name = name
                .to_str()
                .ok_or_else(|| invalid("notice file name is not UTF-8"))?;
            let input = platform::input_file(&directory, name)?;
            copy_file(input, &destination, ORIGINAL_TEXT_LIMIT, 0o644)?;
            *count += 1;
            *bytes += fs::metadata(destination)?.len();
            if *count > 4096 || *bytes > 64 * MIB {
                return Err(invalid("release notice tree exceeds its size limits"));
            }
        } else {
            return Err(invalid(
                "release notices must contain regular files and directories",
            ));
        }
    }
    Ok(())
}
