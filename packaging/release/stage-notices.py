#!/usr/bin/env python3
"""Stage complete release notices from the locked Rust and verified supplier inputs."""

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
from collections import deque
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HOST_TOOLS = {
    "docker": "bin/docker",
    "docker-compose": "bin/cli-plugins/docker-compose",
    "docker-buildx": "bin/cli-plugins/docker-buildx",
    "docker-credential-osxkeychain": "bin/docker-credential-osxkeychain",
}
RUST_TARGETS = {
    "aarch64-apple-darwin": {"marsa-gui", "marsad", "marsa-cli", "marsa-mcp"},
    "aarch64-unknown-linux-musl": {"marsa-init", "marsa-agent"},
}
GUEST_FILES = ("guest-sbom.cdx.json", "guest-sources.json", "guest-notices.tar", "kernel.config")
FONTS = {
    "IBMPlexSans": "2.0.0",
    "IBMPlexSansArabic": "1.1.0",
    "IBMPlexMono": "2.5.0",
}
CHECKSUM = re.compile(r"[0-9a-f]{64}")
SEGMENT = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9._+-]*")
NOTICE = re.compile(r"(license|licence|copying|copyright|notice|unlicense|authors|patents)([._-].*)?", re.I)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def local_file(path, maximum=None):
    details = path.lstat()
    require(stat.S_ISREG(details.st_mode) and details.st_size > 0, f"Expected a nonempty file: {path}")
    require(maximum is None or details.st_size <= maximum, f"File exceeds its size limit: {path}")
    return path


def read_json(path, maximum=16 * 1024 * 1024):
    with local_file(path, maximum).open(encoding="utf-8") as source:
        return json.load(source)


def digest(path):
    with local_file(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def verify(path, expected):
    require(isinstance(expected, str) and CHECKSUM.fullmatch(expected) and digest(path) == expected,
            f"SHA-256 mismatch: {path}")


def verified_json(path, expected):
    maximum = 16 * 1024 * 1024
    with local_file(path, maximum).open("rb") as source:
        data = source.read(maximum + 1)
    require(len(data) <= maximum and hashlib.sha256(data).hexdigest() == expected,
            f"SHA-256 mismatch: {path}")
    return json.loads(data)


def safe_segment(value):
    require(isinstance(value, str) and SEGMENT.fullmatch(value), f"Invalid notice path segment: {value!r}")
    return value


def supplier_file(handoff, relative):
    source = handoff / relative
    require(source.resolve(strict=True).is_relative_to(handoff.resolve(strict=True)),
            f"Supplier notice leaves the handoff: {relative}")
    for parent in source.parents:
        if parent == handoff:
            break
        require(not parent.is_symlink(), f"Supplier notice directory is a link: {parent.name}")
    return local_file(source, 8 * 1024 * 1024)


def copy_notice(source, destination, stage):
    destination = stage / destination
    destination.parent.mkdir(parents=True, exist_ok=True)
    with local_file(source, 8 * 1024 * 1024).open("rb") as notice, destination.open("xb") as target:
        shutil.copyfileobj(notice, target)
    return destination.relative_to(stage).as_posix()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def component(kind, reference, name, version, licence, source=None):
    require(all(isinstance(text, str) and text.strip() for text in (reference, name, version, licence)),
            "Component identity or licence is missing")
    record = {"type": kind, "bom-ref": reference, "name": name, "version": version,
              "licenses": [{"expression": licence}]}
    if source:
        require(isinstance(source, str) and source.startswith("https://"), "Expected an HTTPS source reference")
        record["externalReferences"] = [{"type": "vcs", "url": source}]
    return record


def rust_graph(target, roots):
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", target],
        cwd=ROOT, stdout=subprocess.PIPE, check=True,
    )
    graph = json.loads(result.stdout)
    packages = {package["id"]: package for package in graph["packages"]}
    nodes = {node["id"]: node for node in graph["resolve"]["nodes"]}
    workspace = set(graph["workspace_members"])
    starts = {package["name"]: key for key, package in packages.items()
              if key in workspace and package["name"] in roots}
    require(set(starts) == roots, f"Missing Rust release roots for {target}")
    queue = deque(starts.values())
    reached = set()
    while queue:
        key = queue.popleft()
        if key in reached:
            continue
        reached.add(key)
        queue.extend(dep["pkg"] for dep in nodes[key]["deps"]
                     if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]))
    return {key: packages[key] for key in reached}, workspace


def package_notices(directory, declared):
    candidates = {path for path in directory.iterdir() if path.is_file() and NOTICE.fullmatch(path.name)}
    extra = directory / "LICENSES"
    if extra.is_dir() and not extra.is_symlink():
        candidates.update(path for path in extra.rglob("*") if path.is_file())
    if declared:
        path = directory / declared
        require(path.resolve().is_relative_to(directory.resolve()), "Cargo licence path leaves its package")
        candidates.add(path)
    return sorted(candidates)


def rust_components(stage, notes, handoff, supplemental):
    found = {}
    workspace = set()
    for target, roots in RUST_TARGETS.items():
        packages, members = rust_graph(target, roots)
        found.update(packages)
        workspace.update(members)
    for key, package in sorted(found.items()):
        name, version = package["name"], package["version"]
        label = f"{name} {version}"
        directory = Path(package["manifest_path"]).parent
        notices = package_notices(directory, package["license_file"])
        if name == "gpui-kit-assets" and directory.is_relative_to(ROOT / "third-party/gpui"):
            # The icon archive also carries a separate Lucide notice. The
            # Rust source itself has the same Apache notice as Base from
            # this exact GPUI Kit release.
            notices.append(directory.parent / f"gpui-base-{version}" / "LICENSE-APACHE")
        if not notices and key in workspace:
            notices = [ROOT / "LICENSE"]
        licence = package["license"]
        require(licence, f"Cargo package is missing a declared SPDX licence: {label}")
        base = Path("texts/rust") / safe_segment(name) / safe_segment(version)
        files = []
        for notice in notices:
            relative = (notice.relative_to(directory) if notice.is_relative_to(directory)
                        else Path("LICENSE-Marsa.txt" if notice == ROOT / "LICENSE" else notice.name))
            require(not any(part in ("", ".", "..") for part in relative.parts), f"Invalid notice path: {relative}")
            files.append(copy_notice(notice, base / relative, stage))
        source = None
        if not files:
            missing = f"{name}@{version}"
            require(missing in supplemental, f"Missing authenticated upstream Cargo notice: {label}")
            upstream = supplemental[missing]
            require(upstream["license"] == licence, f"Cargo licence differs from upstream notice record: {label}")
            source = upstream["source"]
            files = supplier_notices(handoff, upstream["notices"], base, stage)
            require(files, f"Missing upstream Cargo notice text: {label}")
        notes.append((label, licence, files))
        record = component("library", f"cargo:{name}@{version}", name, version, licence, source)
        record["purl"] = f"pkg:cargo/{name}@{version}"
        if package["source"] and package["source"].startswith("registry+"):
            record.setdefault("externalReferences", []).append({"type": "distribution",
                "url": f"https://crates.io/api/v1/crates/{name}/{version}/download"})
        yield record


def guest_components(resources, provenance, stage, notes):
    manifest_path = resources / "Resources/guest/guest-manifest.json"
    manifest = read_json(manifest_path)
    verify(provenance / "guest-manifest.json", digest(manifest_path))
    for name, field in (("Image", "kernel_sha256"), ("rootfs.img", "rootfs_sha256")):
        verify(manifest_path.parent / name, manifest[field])
    bom = read_json(provenance / "guest-sbom.cdx.json")
    require(bom["bomFormat"] == "CycloneDX" and bom["specVersion"] == "1.7", "Unexpected guest SBOM format")
    root_hashes = bom["metadata"]["component"]["hashes"]
    require(any(item["alg"] == "SHA-256" and item["content"] == manifest["rootfs_sha256"] for item in root_hashes),
            "Guest SBOM does not identify the packaged root filesystem")
    sources = read_json(provenance / "guest-sources.json")
    kernel = sources["kernel"]
    require(sources["version"] == 1 and sources["licence_texts"] == "guest-notices.tar"
            and kernel["configuration"] == "kernel.config"
            and kernel["url"].startswith("https://cdn.kernel.org/pub/linux/kernel/")
            and CHECKSUM.fullmatch(kernel["sha256"]), "Missing exact kernel source or configuration reference")
    verify(provenance / "kernel.config", kernel["configuration_sha256"])
    for filename in GUEST_FILES:
        shutil.copyfile(local_file(provenance / filename), stage / filename)
    require(bom["components"], "Guest component inventory is empty")
    for record in bom["components"]:
        licence = ", ".join(entry.get("expression") or entry["license"].get("id") or entry["license"]["name"]
                            for entry in record["licenses"])
        require(licence, f"Guest component has no licence: {record['name']}")
        notes.append((f"Guest {record['name']} {record['version']}", licence, ["guest-notices.tar"]))
        yield record


def linked_go_modules(binary):
    result = subprocess.run(
        ["go", "version", "-m", str(binary.resolve())], cwd=ROOT, text=True,
        stdout=subprocess.PIPE, check=True, env={**os.environ, "GOTOOLCHAIN": "local"},
    )
    lines = result.stdout.splitlines()
    version = re.search(r"\bgo[0-9]+\.[0-9]+(?:\.[0-9]+)?$", lines[0]) if lines else None
    require(version, f"No Go link metadata found for {binary.name}")
    modules = {"std": version.group()[2:]}
    for line in lines[1:]:
        fields = line.split()
        if len(fields) >= 3 and fields[0] in ("mod", "dep"):
            require(fields[1] not in modules, f"Duplicate Go module metadata: {binary.name}")
            modules[fields[1]] = fields[2]
        elif fields and fields[0] == "=>":
            raise ValueError(f"Go replacements require a separately qualified notice scheme: {binary.name}")
    return modules


def supplier_go_modules(binary, artifact, handoff, name):
    modules = linked_go_modules(binary)
    if len(modules) > 1:
        return modules
    # The pinned Docker CLI exposes the Go version and package path, but
    # omits its linked module list. Require an authenticated producer record
    # for the exact binary instead of inferring its graph from version strings.
    require(name == "docker", f"No linked Go module inventory found for {name}")
    evidence = artifact["module_inventory"]
    filename = safe_segment(evidence["name"])
    record_path = supplier_file(handoff, Path("inventories") / filename)
    record = verified_json(record_path, evidence["sha256"])
    require(record["version"] == 1 and record["binary_sha256"] == artifact["sha256"],
            "Docker CLI module inventory must identify the exact staged binary")
    reported = record["modules"]
    require(isinstance(reported, dict) and len(reported) > 2
            and reported.get("std") == modules["std"]
            and all(isinstance(value, str) and value.strip() for value in reported.values()),
            "Docker CLI module inventory is incomplete or mismatches its embedded Go toolchain version")
    return reported


def supplier_notices(handoff, notices, base, stage):
    files = []
    for filename, expected in sorted(notices.items()):
        source = supplier_file(handoff, base / safe_segment(filename))
        notice = copy_notice(source, base / filename, stage)
        verify(stage / notice, expected)
        files.append(notice)
    return files


def supplier_components(resources, helper, handoff, manifest, stage, notes):
    expected_files = {name: resources / "Resources" / member for name, member in HOST_TOOLS.items()}
    expected_files["marsa-net-helper"] = helper
    native = read_json(resources / "upstream-sbom-input.json")["components"]
    upstream = {record["name"]: record for record in native}
    require(len(native) == len(upstream) and upstream.keys() == HOST_TOOLS.keys(),
            "Staged Docker inventory is incomplete")
    require(manifest["version"] == 1 and manifest["artifacts"].keys() == expected_files.keys(),
            "Supplier notices must cover every shipped host tool and the network helper")

    for name, binary in expected_files.items():
        artifact = manifest["artifacts"][name]
        verify(binary, artifact["sha256"])
        if name in upstream:
            upstream_hashes = upstream[name]["hashes"]
            require(any(item["alg"] == "SHA-256" and item["content"] == artifact["sha256"] for item in upstream_hashes),
                    f"Supplier source does not apply to the staged binary: {name}")
        inventory = artifact["components"]
        require(name in inventory, f"Missing supplier primary component: {name}")
        linked = supplier_go_modules(binary, artifact, handoff, name)
        supplied = {item["module"]: item["version"] for item in inventory.values()}
        require(len(supplied) == len(inventory) and supplied.keys() == linked.keys(),
                f"Supplier notices do not cover the linked Go modules of {name}")
        require(all(version == "(devel)" or supplied[module].removeprefix("v") == version.removeprefix("v")
                    for module, version in linked.items()), f"Supplier Go versions differ from {name}")
        for key, item in sorted(inventory.items()):
            base = Path("texts/suppliers") / safe_segment(name) / safe_segment(key)
            record = component("application" if key == name else "library", f"supplier:{name}:{key}",
                               item["name"], item["version"], item["license"], item["source"])
            files = supplier_notices(handoff, item["notices"], base, stage)
            require(files, f"Missing supplier notice text: {name}/{key}")
            if key == name:
                record["hashes"] = [{"alg": "SHA-256", "content": artifact["sha256"]}]
                if name in upstream:
                    original = upstream[name]
                    require(item["version"] == original["version"]
                            and any(licence["license"]["id"] == item["license"] for licence in original["licenses"]),
                            f"Supplier inventory differs from pinned tool identity: {name}")
                    record["purl"] = original["purl"]
                    record["externalReferences"].extend(original["externalReferences"])
            notes.append((f"{name}: {item['name']} {item['version']}", item["license"], files))
            yield record


def font_components(stage, notes):
    fonts = ROOT / "assets/fonts"
    for line in (fonts / "SHA256SUMS").read_text(encoding="ascii").splitlines():
        expected, filename = line.split()
        verify(fonts / safe_segment(filename), expected)
        if filename.endswith(".ttf"):
            notes.append((filename, "OFL-1.1", ["OFL-IBM-Plex.txt"]))
            family = filename.split("-", 1)[0]
            record = component("data", f"font:{filename}", filename, FONTS[family], "OFL-1.1",
                               "https://github.com/IBM/plex")
            record["hashes"] = [{"alg": "SHA-256", "content": expected}]
            yield record
    copy_notice(fonts / "OFL-IBM-Plex.txt", "OFL-IBM-Plex.txt", stage)

    catalog = fonts / "catalog"
    manifest = read_json(catalog / "manifest.json")
    checksums = {}
    for line in (catalog / "SHA256SUMS").read_text(encoding="ascii").splitlines():
        expected, filename = line.split()
        verify(catalog / safe_segment(filename), expected)
        checksums[filename] = expected
    for family in manifest["families"]:
        name = family["name"]
        license_id = family["license"]
        original_notice = safe_segment(family["notice"])
        require(original_notice in checksums, f"Missing font notice checksum: {original_notice}")
        notice = copy_notice(catalog / original_notice, f"texts/fonts/{original_notice}", stage)
        for asset in family["files"]:
            filename = safe_segment(asset["file"])
            checksum = checksums[filename]
            require(checksum == asset["sha256"], f"Font manifest hash disagrees: {filename}")
            notes.append((filename, license_id, [notice]))
            record = component("data", f"font:{filename}", name, manifest["revision"], license_id,
                               family["source"])
            record["hashes"] = [{"alg": "SHA-256", "content": checksum}]
            yield record


def icon_component(stage, notes):
    candidates = list((ROOT / "third-party/gpui").glob("gpui-kit-assets-*/lucide.json"))
    require(len(candidates) == 1, "Expected one pinned GPUI icon inventory")
    assets = candidates[0]
    version = read_json(assets)["version"]
    notice = copy_notice(assets.parent / "LICENSE-LUCIDE", "texts/icons/LICENSE-LUCIDE.txt", stage)
    notes.append((f"Lucide icons {version}", "ISC AND MIT", [notice]))
    return component("data", f"lucide:{version}", "Lucide icons", version, "ISC AND MIT",
                     "https://github.com/lucide-icons/lucide")


def release_source_commit():
    status = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=normal"],
        cwd=ROOT, text=True, stdout=subprocess.PIPE, check=True,
    )
    require(not status.stdout, "Generate release notices from a clean checkout")
    commit = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, stdout=subprocess.PIPE, check=True,
    ).stdout.strip()
    require(re.fullmatch(r"[0-9a-f]{40}", commit), "Cannot identify the exact release source commit")
    return commit


def prepare(args, stage):
    commit = release_source_commit()
    handoff = args.tool_notices
    manifest = verified_json(handoff / "manifest.json", args.tool_notices_sha256)
    require(manifest["workspace_commit"] == commit, "Supplier handoff applies to a different release source")
    verify(ROOT / "Cargo.lock", manifest["cargo_lock_sha256"])
    notes = []
    components = [
        *rust_components(stage, notes, handoff, manifest["rust"]),
        *guest_components(args.release_resources, args.guest_provenance, stage, notes),
        *supplier_components(args.release_resources, args.network_helper, handoff, manifest, stage, notes),
        *font_components(stage, notes),
        icon_component(stage, notes),
    ]
    copy_notice(ROOT / "LICENSE", "LICENSE-Marsa.txt", stage)
    app = component("application", "marsa-release", "Marsa", args.version, "MIT",
                    f"https://github.com/nasraldin/marsa/tree/{commit}")
    write_json(stage / "marsa-sbom.cdx.json", {
        "$schema": "https://cyclonedx.org/schema/bom-1.7.schema.json", "bomFormat": "CycloneDX",
        "specVersion": "1.7", "version": 1, "metadata": {"component": app}, "components": components,
    })
    text = [
        f"Marsa {args.version} — third-party notices",
        "",
        "The included files preserve the component publishers' notice text.",
        "Guest source references and the resolved kernel configuration accompany this inventory.",
        "",
    ]
    for name, licence, files in sorted(notes):
        text.extend([f"{name} — {licence}", *(f"  {item}" for item in files), ""])
    (stage / "THIRD-PARTY-NOTICES.txt").write_text("\n".join(text), encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    for name in ("release-resources", "guest-provenance", "network-helper", "tool-notices", "output"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--tool-notices-sha256", required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][a-zA-Z0-9.-]+)?", args.version), "Invalid release version")
    require(args.output.is_absolute(), "Supply an absolute path for the output")
    parent = args.output.parent.resolve(strict=True)
    details = parent.stat()
    require(details.st_uid == os.getuid() and details.st_mode & 0o077 == 0, "Use a private release workspace")
    output = parent / safe_segment(args.output.name)
    output.mkdir(mode=0o700)
    prepare(args, output)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, AttributeError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"Marsa release notices: {error}") from None
