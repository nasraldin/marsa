"""Inventory the executables shipped unchanged from Docker's static distribution."""

from concurrent.futures import ThreadPoolExecutor
from hashlib import sha256
import json
from pathlib import Path
import re
from urllib.request import urlopen

LOCK = Path(__file__).with_name("engine-notices.json")
NOTICE_LIMIT = 1024 * 1024


def inventory(versions, notices):
    releases = {
        "dockerd": (versions["version"], "docker-v", ["dockerd", "docker-proxy"]),
        "buildkit": (versions["buildkit"], "v", ["dockerd"]),
        "containerd": (versions["containerd"], "v", ["containerd", "containerd-shim-runc-v2"]),
        "runc": (versions["runc"], "v", ["runc"]),
        "tini": (versions["docker_init"], "v", ["docker-init"]),
    }
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    # The build excludes the CLI. Keep the lock limited to shipped components.
    if lock.keys() != releases.keys():
        raise ValueError("Engine notice lock does not match the guest's shipped components")
    components = []
    downloads = []
    for name, (version, prefix, executables) in releases.items():
        entry = lock[name]
        repository, tag = entry["repository"], entry["tag"]
        if tag != prefix + version or not re.fullmatch(r"[\w.-]+/[\w.-]+", repository, re.ASCII):
            raise ValueError(f"Engine notice pin does not match {name} {version}")
        if not re.fullmatch(r"(docker-)?v\d+\.\d+\.\d+", tag):
            raise ValueError(f"Invalid release tag for {name}")
        source = f"https://github.com/{repository}/tree/{tag}"
        for filename, digest in entry["notices"].items():
            if filename not in {"LICENSE", "NOTICE"} or not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise ValueError(f"Invalid notice lock for {name}")
            url = f"https://raw.githubusercontent.com/{repository}/{tag}/{filename}"
            downloads.append((url, digest, notices / "engine" / name / filename))
        if "LICENSE" not in entry["notices"]:
            raise ValueError(f"Missing licence text for {name}")
        components.append({
            "name": name,
            "version": version,
            "license": "MIT" if name == "tini" else "Apache-2.0",
            "source": source,
            "files": [f"usr/local/bin/{executable}" for executable in executables],
        })

    with ThreadPoolExecutor(max_workers=5) as pool:
        list(pool.map(download, downloads))
    return components


def download(item):
    url, digest, destination = item
    with urlopen(url, timeout=30) as response:
        if response.geturl() != url:
            raise ValueError(f"Unexpected redirect while retrieving {url}")
        contents = response.read(NOTICE_LIMIT + 1)
    if len(contents) > NOTICE_LIMIT or sha256(contents).hexdigest() != digest:
        raise ValueError(f"Upstream licence checksum mismatch for {url}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(contents)
