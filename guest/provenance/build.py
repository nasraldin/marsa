#!/usr/bin/env python3
"""Generate deterministic guest inventory, source references and first-party notices."""

import argparse
from hashlib import file_digest
import io
import json
from pathlib import Path
import re
import shutil
import tarfile
import tomllib

import alpine
import binaries
import engine

WORKSPACE = Path(__file__).resolve().parents[2]
KERNEL_NOTICES = ("COPYING", "LICENSES/preferred/GPL-2.0", "LICENSES/exceptions/Linux-syscall-note")
KERNEL_NOTICE_LIMIT = 64 * 1024


def sha256(path):
    with path.open("rb") as contents:
        return file_digest(contents, "sha256").hexdigest()


def write_json(path, data):
    path.write_text(json.dumps(data, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
                    encoding="utf-8")


def archive_reference(output, url, digest):
    file = output / url.rsplit("/", 1)[-1]
    if sha256(file) != digest:
        raise ValueError(f"Source archive checksum mismatch: {file.name}")
    return {"url": url, "sha256": digest}


def kernel_inventory(output, kernel, notices):
    version = kernel["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("Invalid kernel release")
    major = version.split(".", 1)[0]
    prefix = f"linux-{version}"
    url = f"https://cdn.kernel.org/pub/linux/kernel/v{major}.x/{prefix}.tar.xz"
    source = archive_reference(output, url, kernel["sha256"])
    with tarfile.open(output / f"{prefix}.tar.xz", "r:xz") as archive:
        for member in archive:
            path = member.name.removeprefix(prefix + "/")
            if path not in KERNEL_NOTICES:
                continue
            if not member.isfile() or not 0 < member.size <= KERNEL_NOTICE_LIMIT:
                raise ValueError(f"Invalid kernel notice {path}")
            destination = notices / "linux" / path
            if destination.exists():
                raise ValueError(f"Duplicate kernel notice {path}")
            destination.parent.mkdir(parents=True, exist_ok=True)
            with archive.extractfile(member) as contents, destination.open("wb") as target:
                shutil.copyfileobj(contents, target)
    if any(not (notices / "linux" / path).is_file() for path in KERNEL_NOTICES):
        raise ValueError("Kernel source archive is missing its licence texts")
    installed = {
        "name": "Linux",
        "version": version,
        "license": "GPL-2.0-only",
        "source": url,
        "files": ["Image"],
    }
    return installed, dict(source, configuration="kernel.config", configuration_sha256=sha256(
        output / "kernel.config"
    ))


def component(item, kind, scope):
    locations = [{"location": f"{scope}/{path}"} for path in item["files"]]
    licence = (
        {"license": {"name": item["license"], "acknowledgement": "declared"}}
        if item["license"] == "LGPL-1.0-only"
        else {"expression": item["license"], "acknowledgement": "declared"}
    )
    entry = {
        "type": kind,
        "bom-ref": f'{scope}:{item["name"]}@{item["version"]}',
        "name": item["name"],
        "version": item["version"],
        "licenses": [licence],
        "externalReferences": [{"type": "source-distribution", "url": item["source"]}],
    }
    if locations:
        entry["evidence"] = {"occurrences": locations}
    return entry


def write_notices(stage, notices):
    with tarfile.open(stage / "guest-notices.tar", "w", format=tarfile.USTAR_FORMAT) as archive:
        for source in sorted(notices.rglob("*")):
            if source.is_symlink():
                raise ValueError(f"Notice links are unsupported: {source.relative_to(notices)}")
            if source.is_file():
                contents = source.read_bytes()
                entry = tarfile.TarInfo(source.relative_to(notices).as_posix())
                entry.mode = 0o644
                entry.size = len(contents)
                archive.addfile(entry, io.BytesIO(contents))


def project_inventory(root, notices):
    source = "https://github.com/nasraldin/marsa"
    commit = root.get("source_commit")
    if commit is not None:
        if not re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", commit):
            raise ValueError("Invalid guest source commit")
        source += f"/tree/{commit}"

    location = notices / "marsa"
    location.mkdir()
    shutil.copyfile(WORKSPACE / "LICENSE", location / "LICENSE")

    programs = []
    for name in ("marsa-init", "marsa-agent"):
        version = root["components"][name.replace("-", "_")]
        if version != root["image_version"]:
            raise ValueError(f"Missing guest program release for {name}")
        programs.append({
            "name": name,
            "version": version,
            "license": "MIT",
            "source": source,
            "files": [f"usr/bin/{name}"],
        })
    return programs, {"source": source, "git_commit": commit, "license": "MIT"}


def build(output, stage):
    versions = tomllib.loads((WORKSPACE / "guest/versions.toml").read_text(encoding="utf-8"))
    root = json.loads((output / "guest-manifest.json").read_text(encoding="utf-8"))
    notices = stage / "notices"
    notices.mkdir()
    linux, kernel_source = kernel_inventory(output, versions["kernel"], notices)
    if sha256(output / "Image") != root["kernel_sha256"]:
        raise ValueError("Kernel does not match the guest manifest")
    image_digest = sha256(output / "rootfs.img")
    if image_digest != root["rootfs_sha256"]:
        raise ValueError("Root image does not match the guest manifest")
    rootfs_tar = output / "image-rootfs.tar"
    input_digest = (output / "rootfs-input.sha256").read_text(encoding="ascii").splitlines()
    if input_digest != [
        f"{sha256(rootfs_tar)}  /output/image-rootfs.tar",
        f"{image_digest}  /output/rootfs.img",
    ]:
        raise ValueError("Root filesystem archive does not match the packed image")
    docker = engine.inventory(versions["docker"], notices)
    programs, project_source = project_inventory(root, notices)
    with tarfile.open(rootfs_tar, "r:") as rootfs:
        packages, alpine_sources = alpine.inventory(rootfs, notices)
        binaries.verify_inventory(rootfs, packages + docker + programs)

    project_version = root["image_version"]
    bom = {
        "$schema": "https://cyclonedx.org/schema/bom-1.7.schema.json",
        "bomFormat": "CycloneDX",
        "specVersion": "1.7",
        "version": 1,
        "metadata": {"component": {
            "type": "operating-system",
            "bom-ref": f"marsa-guest@{project_version}",
            "name": "Marsa guest",
            "version": project_version,
            "hashes": [{"alg": "SHA-256", "content": image_digest}],
        }},
        "components": [
            component(linux, "operating-system", "boot"),
            *(component(item, "application", "rootfs") for item in docker + programs),
            *(component(item, "library", "rootfs") for item in packages),
        ],
    }
    docker_url = (
        "https://download.docker.com/linux/static/stable/aarch64/"
        f'docker-{versions["docker"]["version"]}.tgz'
    )
    write_json(stage / "guest-sbom.cdx.json", bom)
    write_json(stage / "guest-sources.json", {
        "version": 1,
        "kernel": kernel_source,
        "alpine": alpine_sources,
        "engine": {
            "distribution": archive_reference(output, docker_url, versions["docker"]["sha256"]),
            "components": [
                {key: value for key, value in item.items() if key != "files"} for item in docker
            ],
        },
        "marsa": dict(project_source, components=[
            {key: value for key, value in item.items() if key != "files"} for item in programs
        ]),
        "licence_texts": "guest-notices.tar",
    })
    write_notices(stage, notices)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="Directory of verified guest build artifacts")
    parser.add_argument("stage", type=Path, help="Private staging directory for emitted artifacts")
    arguments = parser.parse_args()
    try:
        build(arguments.output, arguments.stage)
    except (OSError, ValueError, KeyError, tarfile.TarError) as error:
        parser.exit(1, f"Guest provenance: {error}\n")
