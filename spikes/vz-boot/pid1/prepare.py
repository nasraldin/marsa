"""Stage the real PID 1, packaged engine, and a test agent in a private initramfs."""

import argparse
import gzip
from pathlib import Path, PurePosixPath
import tarfile


def write_entry(output, number, name, mode, data):
    """Write one root-owned newc entry to the fixture archive."""
    name = name.encode("utf-8") + b"\0"
    fields = (number, mode, 0, 0, 1, 0, len(data), 0, 0, 0, 0, len(name), 0)
    output.write(b"070701" + "".join(f"{field:08x}" for field in fields).encode("ascii"))
    output.write(name)
    output.write(b"\0" * (-(110 + len(name)) % 4))
    output.write(data)
    output.write(b"\0" * (-len(data) % 4))


def entries(rootfs, init, vsock, docker_archive):
    """Stream the existing rootfs and overlay only the test binaries."""
    replacements = {"usr/bin/marsa-init", "usr/bin/marsa-agent"}
    with tarfile.open(rootfs) as source:
        for member in source:
            path = PurePosixPath(member.name)
            name = str(path)
            if name == "." or name in replacements:
                continue
            if path.is_absolute() or ".." in path.parts:
                raise ValueError(f"unsafe rootfs entry {member.name!r}")
            if member.isdir():
                yield name, 0o040000 | member.mode, b""
            elif member.issym():
                yield name, 0o120000 | member.mode, member.linkname.encode("utf-8")
            elif member.isfile() or member.islnk():
                with source.extractfile(member) as data:
                    yield name, 0o100000 | member.mode, data.read()
            else:
                raise ValueError(f"unsupported rootfs entry {member.name!r}")
    yield "usr/bin/marsa-init", 0o100755, init.read_bytes()
    yield "usr/bin/marsa-agent", 0o100755, Path(__file__).with_name("agent.sh").read_bytes()
    yield "marsa-vsock-fixture", 0o100755, vsock.read_bytes()
    with tarfile.open(docker_archive) as source:
        with source.extractfile("docker/docker") as data:
            yield "marsa-test-docker", 0o100755, data.read()
    yield "TRAILER!!!", 0, b""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("rootfs", "init", "vsock", "docker_archive", "output"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    with args.output.open("xb") as raw:
        with gzip.GzipFile(fileobj=raw, mode="wb", mtime=0, compresslevel=3) as archive:
            for number, entry in enumerate(
                entries(args.rootfs, args.init, args.vsock, args.docker_archive), start=1
            ):
                write_entry(archive, number, *entry)


if __name__ == "__main__":
    main()
