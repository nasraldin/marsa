"""Check the staged guest tree and engine bytes against independently pinned inputs."""

import hashlib
import json
import sys
import tarfile
import tomllib
from pathlib import Path, PurePosixPath

ENGINE = (
    "containerd", "containerd-shim-runc-v2", "docker-init",
    "docker-proxy", "dockerd", "runc",
)


def digest(archive, path):
    with archive.extractfile(path) as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


output = Path(sys.argv[1])
with (Path(__file__).resolve().parents[1] / "versions.toml").open("rb") as pins:
    docker_version = tomllib.load(pins)["docker"]["version"]

with (
    tarfile.open(output / "rootfs.tar") as root,
    tarfile.open(output / f"docker-{docker_version}.tgz") as upstream,
):
    entries = {
        PurePosixPath(member.name).as_posix(): member for member in root.getmembers()
    }
    for name in ENGINE:
        installed = entries[f"usr/local/bin/{name}"]
        assert installed.isfile() and installed.mode == 0o755, name
        assert digest(root, installed) == digest(upstream, f"docker/{name}"), name

    for tool in ("usr/local/bin/docker", "usr/local/bin/ctr"):
        assert tool not in entries, tool

    for tool in (
        "sbin/ip", "sbin/mkfs.ext4", "sbin/e2fsck", "usr/sbin/resize2fs",
        "sbin/fstrim", "usr/sbin/iptables", "usr/sbin/ip6tables",
    ):
        assert tool in entries, tool

    assert entries["sbin/blkid"].isfile(), "util-linux blkid replaces the BusyBox symlink"
    for directory in (
        "var/lib/marsa", "var/lib/docker", "var/lib/containerd", "Users", "Volumes", "private", "mnt",
    ):
        assert entries[directory].isdir(), directory

    forbidden = {
        "var/cache/apk", "etc/inittab", "sbin/init", "sbin/openrc", "sbin/getty", "usr/sbin/crond",
    }
    for path, member in entries.items():
        assert (member.uid, member.gid) == (0, 0), (path, member.uid, member.gid)
        name = PurePosixPath(path).name
        assert not path.startswith(("var/cache/apk/", "etc/init.d/", "etc/runlevels/", "etc/ssh/")), path
        assert path not in forbidden and name not in {"sshd", "dropbear"}, path
        assert not (name.endswith("history") or member.mode & 0o6000), path

    with root.extractfile(entries["etc/docker/daemon.json"]) as stream:
        config = json.load(stream)
    assert config["hosts"] == ["unix:///var/run/docker.sock"]
    assert config["authorization-plugins"] == ["MarsaBindAuthorization"]
    assert config["containerd"] == "/run/containerd/containerd.sock"
    assert config["seccomp-profile"] == "builtin"
    assert config["experimental"] is False and config["live-restore"] is False
    assert "userns-remap" not in config
    assert config["firewall-backend"] == "iptables"

size = (output / "rootfs.tar").stat().st_size
assert (output / "rootfs-size.txt").read_text() == f"rootfs.tar: {size} bytes\n"
