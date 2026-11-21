"""Prepare verified stock Alpine boot files and append the guest test fixture."""

import argparse
import gzip
import hashlib
import pathlib
import struct
import subprocess
import tempfile


ISO = "alpine-virt-3.24.4-aarch64.iso"
URL = f"https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64/{ISO}"
ISO_SHA256 = "cf88d8ba67389d20d9ed307366b52e487c15d7d3ee3233b62b95f709daa1d430"
KERNEL_SHA256 = "a2cecb9b6fdbd83e87bd43c8040de67300d5b8dde1fc50c34714faa272888f17"


def checked_digest(data: bytes, expected: str, filename: str) -> None:
    if hashlib.sha256(data).hexdigest() != expected:
        raise SystemExit(f"SHA-256 mismatch for {filename}")


def raw_kernel(data: bytes) -> bytes:
    magic, offset, size = struct.unpack_from("<4sII", data, 4)
    if magic != b"zimg" or not data[24:32].startswith(b"gzip\0"):
        raise SystemExit("expected Alpine's EFI zboot gzip image")
    if offset < 64 or size == 0 or offset + size > len(data):
        raise SystemExit("invalid EFI zboot payload bounds")
    image = gzip.decompress(data[offset : offset + size])
    if image[56:60] != b"ARM\x64":
        raise SystemExit("expected a decompressed ARM64 Linux Image")
    checked_digest(image, KERNEL_SHA256, "Image")
    return image


def newc(entries: list[tuple[str, int, bytes]]) -> bytes:
    archive = bytearray()
    for number, (name, mode, content) in enumerate(entries + [("TRAILER!!!", 0, b"")], 1):
        filename = name.encode("ascii") + b"\0"
        fields = (number, mode, 0, 0, 1, 0, len(content), 0, 0, 0, 0, len(filename), 0)
        archive.extend(b"070701" + "".join(f"{field:08x}" for field in fields).encode("ascii"))
        archive.extend(filename)
        archive.extend(b"\0" * (-len(archive) % 4))
        archive.extend(content)
        archive.extend(b"\0" * (-len(archive) % 4))
    return bytes(archive)


def extract(iso: pathlib.Path, path: str) -> bytes:
    return subprocess.check_output(["bsdtar", "-xOf", str(iso), path])


def prepare(iso: pathlib.Path, guest: pathlib.Path, output: pathlib.Path) -> None:
    checked_digest(iso.read_bytes(), ISO_SHA256, ISO)
    output.mkdir(mode=0o700)
    (output / "Image").write_bytes(raw_kernel(extract(iso, "boot/vmlinuz-virt")))
    (output / "modules.img").write_bytes(extract(iso, "boot/modloop-virt"))
    init = pathlib.Path(__file__).resolve().with_name("guest") / "init.sh"
    appendix = newc([
        ("marsa-spike-init", 0o100755, init.read_bytes()),
        ("marsa-vsock-fixture", 0o100755, guest.read_bytes()),
    ])
    (output / "initramfs").write_bytes(
        extract(iso, "boot/initramfs-virt") + gzip.compress(appendix, mtime=0)
    )
    with (output / "data.img").open("xb") as target:
        target.truncate(64 * 1024 * 1024)
    share = output / "share"
    share.mkdir(mode=0o700)
    (share / "fixture").write_bytes(b"marsa-vz-fixture\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("guest", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--iso", type=pathlib.Path, help="use an already downloaded Alpine ISO")
    args = parser.parse_args()
    if args.iso:
        prepare(args.iso.resolve(), args.guest, args.output)
        return
    with tempfile.TemporaryDirectory(prefix="marsa-vz-") as temp:
        iso = pathlib.Path(temp, ISO)
        subprocess.run(
            ["curl", "--proto", "=https", "--fail", "--location", "--silent", "--show-error",
             "--max-time", "180", "--output", str(iso), URL],
            check=True,
        )
        prepare(iso, args.guest, args.output)


if __name__ == "__main__":
    main()
