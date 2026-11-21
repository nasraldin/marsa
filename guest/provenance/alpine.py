"""Inventory the shipped Alpine packages and copy reviewed upstream notices.

The lock keys each source origin by every installed APK revision and its declared
license. When Alpine changes, review the new exact aports recipe, verify its
source archives with the recipe checksums, and refresh the pinned notices from
those sources. Keep SPDX license-list copies distinct from upstream notices.
"""

import hashlib
import json
import posixpath
import re
import tarfile
from pathlib import Path, PurePosixPath


LOCK = Path(__file__).with_name("alpine-lock.json")
TEXTS = Path(__file__).with_name("alpine-notices")
MAX_DATABASE_BYTES = 4 * 1024 * 1024
ALPINE_SOURCE = "https://gitlab.alpinelinux.org/alpine/aports/-/tree"
COMMIT = re.compile(r"[0-9a-f]{40}\Z")


def inventory(rootfs: tarfile.TarFile, notices: Path) -> tuple[list[dict], list[dict]]:
    """Return installed APK component and source metadata for a rootfs tar."""
    members = {}
    for member in rootfs.getmembers():
        path = relative_path(member.name)
        if path in members:
            raise ValueError(f"duplicate rootfs path: {path}")
        members[path] = member

    database = members.get("lib/apk/db/installed")
    if database is None or not database.isfile() or database.size > MAX_DATABASE_BYTES:
        raise ValueError("missing or oversized Alpine installed-package database")
    with rootfs.extractfile(database) as stream:
        packages = parse_database(stream.read(MAX_DATABASE_BYTES + 1).decode("utf-8"))
    pinned = json.loads(LOCK.read_text(encoding="utf-8"))
    sources = verified_sources(packages, pinned, notices)

    owned = {}
    for package in packages:
        for name in package["files"]:
            if name in owned:
                raise ValueError(f"duplicate Alpine file owner: {name}")
            owned[name] = package["name"]

    components = []
    for package in packages:
        source = sources[package["origin"]]
        files = [name for name in package["files"] if name in members and not members[name].isdir()]
        if package["name"] == "busybox":
            # The APK post-install script adds applet links outside its file list.
            files.extend(
                path for path, member in members.items()
                if path not in owned and is_busybox_applet(path, member)
            )
        components.append({
            "name": package["name"],
            "version": package["version"],
            "license": package["license"],
            "source": source["recipe_url"],
            "files": sorted(files),
        })
    return components, list(sources.values())


def parse_database(database: str) -> list[dict]:
    """Read APK v2 package records, separating ownership from package metadata."""
    packages = {}
    for stanza in database.strip().split("\n\n"):
        metadata = {}
        files = []
        directory = None
        for line in stanza.splitlines():
            field, delimiter, value = line.partition(":")
            if len(field) != 1 or not delimiter:
                raise ValueError("malformed Alpine installed-package record")
            if field == "F":
                directory = relative_path(value)
            elif field == "R":
                if directory is None or not value or "/" in value:
                    raise ValueError("malformed Alpine file ownership")
                files.append(relative_path(f"{directory}/{value}"))
            elif field in "PVLAoc":
                if field in metadata:
                    raise ValueError(f"duplicate APK metadata field: {field}")
                metadata[field] = value
        if any(not metadata.get(field) for field in "PVLAoc") or not COMMIT.fullmatch(metadata["c"]):
            raise ValueError("incomplete Alpine package provenance record")
        name = metadata["P"]
        if name in packages:
            raise ValueError(f"duplicate Alpine package: {name}")
        packages[name] = {
            "name": name,
            "version": metadata["V"],
            "license": metadata["L"],
            "origin": metadata["o"],
            "commit": metadata["c"],
            "architecture": metadata["A"],
            "files": files,
        }
    return [packages[name] for name in sorted(packages)]


def verified_sources(packages: list[dict], lock: dict, notices: Path) -> dict:
    observed = {}
    for package in packages:
        origin = package["origin"]
        observed.setdefault(origin, []).append({
            field: package[field] for field in ("name", "version", "license", "commit", "architecture")
        })
    if set(lock) != set(observed):
        missing = sorted(set(observed) - set(lock))
        stale = sorted(set(lock) - set(observed))
        raise ValueError(f"Alpine notice coverage changed; missing origins {missing}, stale origins {stale}")
    output = {}
    expected_files = set()
    for origin, observed_packages in sorted(observed.items()):
        source = lock[origin]
        if source["packages"] != observed_packages:
            raise ValueError(f"Alpine notice pins are stale for {origin}")
        version = observed_packages[0]["version"]
        commit = observed_packages[0]["commit"]
        files = source["notices"]
        if not files:
            raise ValueError(f"missing pinned Alpine notices for {origin}")
        staged = []
        for item in files:
            name = relative_path(item["file"])
            source_file = relative_path(f"{origin}/{name}")
            if source_file in expected_files:
                raise ValueError(f"duplicate Alpine notice pin: {source_file}")
            expected_files.add(source_file)
            content = (TEXTS / source_file).read_bytes()
            if not content or hashlib.sha256(content).hexdigest() != item["sha256"]:
                raise ValueError(f"Alpine notice digest mismatch: {source_file}")
            path = f"alpine/{source_file}"
            destination = notices / path
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(content)
            staged.append({**item, "file": path})
        output[origin] = {
            "name": origin,
            "version": version,
            "commit": commit,
            "recipe_url": f"{ALPINE_SOURCE}/{commit}/main/{origin}",
            "recipe_sha256": source["recipe_sha256"],
            "sources": source["sources"],
            "notices": staged,
        }
    actual_files = {
        path.relative_to(TEXTS).as_posix() for path in TEXTS.rglob("*")
        if path.is_file() and path != TEXTS / ".gitattributes"
    }
    if actual_files != expected_files:
        raise ValueError(f"unexpected Alpine notice files: {sorted(actual_files - expected_files)}")
    return output


def relative_path(value: str) -> str:
    path = PurePosixPath(value)
    if path.is_absolute() or ".." in path.parts or "\0" in value:
        raise ValueError(f"invalid archive-relative path: {value!r}")
    return str(path)


def is_busybox_applet(path: str, member: tarfile.TarInfo) -> bool:
    if not member.issym():
        return False
    if member.linkname.startswith("/"):
        target = member.linkname
    else:
        target = posixpath.join("/", posixpath.dirname(path), member.linkname)
    return posixpath.normpath(target) == "/bin/busybox"
