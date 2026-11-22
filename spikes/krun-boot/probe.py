"""Verify that libkrun boots the pinned Marsa image and returns a guest hello."""

import hashlib
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import time


def check_inputs(guest):
    manifest = json.loads((guest / "guest-manifest.json").read_text(encoding="utf-8"))
    for part, name in (("kernel", "Image"), ("rootfs", "rootfs.img")):
        with (guest / name).open("rb") as source:
            actual = hashlib.file_digest(source, "sha256").hexdigest()
        if actual != manifest[f"{part}_sha256"]:
            raise RuntimeError(f"guest {part} hash did not match")
    return manifest["image_version"]


def hello(socket_path, process):
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"VMM exited before guest hello ({process.returncode})")
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(0.5)
            try:
                client.connect(str(socket_path))
                with client.makefile("rb") as wire:
                    header = wire.read(4)
                    if len(header) == 4:
                        (length,) = struct.unpack(">I", header)
                        if not 0 < length <= 16 * 1024:
                            raise RuntimeError(f"invalid guest frame length {length}")
                        payload = wire.read(length)
                        if len(payload) == length:
                            return payload
            except (FileNotFoundError, ConnectionError, TimeoutError):
                pass
        time.sleep(0.05)
    raise RuntimeError("no guest hello before the boot deadline")


def decode(payload):
    root = Path(__file__).resolve().parents[2] / "crates" / "marsa-proto" / "proto"
    result = subprocess.run(
        [
            os.environ.get("PROTOC", "protoc"),
            "-I", str(root),
            "--decode=marsa.guest.v1.GuestEnvelope",
            str(root / "marsa" / "guest" / "v1" / "control.proto"),
        ],
        input=payload,
        capture_output=True,
        check=True,
        timeout=5,
    )
    return result.stdout.decode()


def run(executable, guest, output):
    version = check_inputs(guest)
    output.mkdir(mode=0o700, exist_ok=False)
    disk = output / "fixture.img"
    with disk.open("xb") as image:
        os.ftruncate(image.fileno(), 1024 ** 3)
    console = output / "guest.log"
    with tempfile.TemporaryDirectory(prefix="marsa-krun-", dir="/tmp") as scratch:
        control = Path(scratch) / "control.sock"
        share = Path(scratch) / "empty-share"
        share.mkdir(mode=0o700)
        with (output / "vmm.log").open("xb") as log:
            process = subprocess.Popen(
                [executable, guest / "Image", guest / "rootfs.img", disk, console, control, share],
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=log,
                env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"},
            )
            try:
                status = decode(hello(control, process))
                if not status.startswith("hello {") or f'image_version: "{version}"' not in status:
                    raise RuntimeError("guest did not advertise the verified image version")
                boot = console.read_text(encoding="utf-8", errors="replace")
                for evidence in ("Mounted root (erofs filesystem) readonly", "EXT4-fs (vdb)",
                                 "discovered new tag: fixture", "broadcasting discover"):
                    if evidence not in boot:
                        raise RuntimeError(f"guest console lacks {evidence!r}")
                (output / "hello.txt").write_text(status, encoding="utf-8")
                print(f"libkrun booted the stock Marsa image {version} to a validated guest hello")
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)


if __name__ == "__main__":
    if len(sys.argv) != 4 or os.geteuid() == 0:
        sys.exit("Usage as a normal user: probe.py SIGNED_EXECUTABLE GUEST_DIRECTORY NEW_OUTPUT")
    os.umask(0o077)
    run(*(Path(argument).resolve() for argument in sys.argv[1:]))
