"""Restore upstream registry identities in temporary RustSec audit inputs."""

import argparse
import csv
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[2]
VENDOR = pathlib.Path("third-party/gpui")
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"


def read_toml(path):
    return tomllib.loads(path.read_text(encoding="utf-8"))


def provenance(root, manifest):
    vendor = (root / VENDOR).resolve()
    checksum_by_package = {}
    with (vendor / "UPSTREAM.tsv").open(encoding="utf-8", newline="") as stream:
        rows = csv.DictReader(stream, delimiter="\t")
        if rows.fieldnames != ["crate", "version", "registry_sha256", "repository"]:
            raise ValueError("Unexpected UPSTREAM.tsv header")
        for row in rows:
            identity = (row["crate"], row["version"])
            checksum = row["registry_sha256"]
            if identity in checksum_by_package or not re.fullmatch(r"[0-9a-f]{64}", checksum or ""):
                raise ValueError(f"Duplicate or invalid provenance for {identity}")
            checksum_by_package[identity] = checksum

    patched_packages = set()
    for patch in manifest["patch"]["crates-io"].values():
        patch_path = (root / patch["path"]).resolve()
        if not patch_path.is_relative_to(vendor):
            raise ValueError(f"Unexpected registry patch path: {patch_path}")
        package = read_toml(patch_path / "Cargo.toml")["package"]
        patched_packages.add((package["name"], package["version"]))

    if not patched_packages or patched_packages != checksum_by_package.keys():
        raise ValueError("Registry patches and UPSTREAM.tsv must describe the same packages")
    return checksum_by_package


def restore_metadata_sources(metadata, origins, root):
    workspace = set(metadata["workspace_members"])
    vendor = (root / VENDOR).resolve()
    for package in metadata["packages"]:
        if package["source"] is not None or package["id"] in workspace:
            continue
        identity = (package["name"], package["version"])
        path = pathlib.Path(package["manifest_path"]).resolve()
        if identity not in origins or not path.is_relative_to(vendor):
            raise ValueError(f"Missing upstream provenance for {identity}")
        package["source"] = REGISTRY
    # Cargo resolves some optional edges only in the lockfile, which the
    # scheduled cargo-audit scan checks.


def restore_lock_sources(lock, origins, workspace_packages):
    sections = re.split(r"(?m)^\[\[package\]\]\n", lock)
    restored = set()
    for index, section in enumerate(sections[1:], 1):
        # The final package section can also include unrelated lockfile tables.
        package, = tomllib.loads("[[package]]\n" + section)["package"]
        identity = (package["name"], package["version"])
        if "source" in package or identity in workspace_packages:
            continue
        if identity not in origins:
            raise ValueError(f"Missing upstream provenance for {identity}")
        version_line = f'version = "{package["version"]}"\n'
        registry_lines = f'source = "{REGISTRY}"\nchecksum = "{origins[identity]}"\n'
        sections[index] = section.replace(version_line, version_line + registry_lines, 1)
        if sections[index] == section:
            raise ValueError(f"Could not restore lockfile source for {identity}")
        restored.add(identity)
    if restored != origins.keys():
        raise ValueError("A registry patch is missing from Cargo.lock")
    return "[[package]]\n".join(sections)


def populate_registry_index(origins):
    # Cargo does not fetch registry records for a crate resolved by a path patch.
    # Cargo Deny uses the sparse registry cache to check whether it was yanked.
    with tempfile.TemporaryDirectory(prefix="marsa-audit-registry-") as directory:
        for name, version in sorted(origins):
            subprocess.run(
                ["cargo", "info", "--quiet", "--registry", "crates-io", f"{name}@{version}"],
                cwd=directory,
                check=True,
                stdout=subprocess.DEVNULL,
                timeout=60,
            )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("format", choices=["metadata", "lock"])
    args = parser.parse_args()

    manifest = read_toml(ROOT / "Cargo.toml")
    origins = provenance(ROOT, manifest)
    if args.format == "metadata":
        command = ["cargo", "metadata", "--all-features", "--locked", "--format-version", "1"]
        metadata = json.loads(subprocess.check_output(command, cwd=ROOT))
        restore_metadata_sources(metadata, origins, ROOT)
        populate_registry_index(origins)
        json.dump(metadata, sys.stdout)
    else:
        workspace_packages = set()
        workspace = manifest["workspace"]
        for member in workspace["members"]:
            package = read_toml(ROOT / member / "Cargo.toml")["package"]
            version = package["version"]
            if version == {"workspace": True}:
                version = workspace["package"]["version"]
            workspace_packages.add((package["name"], version))
        lock = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
        sys.stdout.write(restore_lock_sources(lock, origins, workspace_packages))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        sys.exit(f"Could not prepare RustSec audit input: {error}")
