"""Format only the disposable fixture using a networkless, software-emulated guest."""

import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import sys

base = pathlib.Path(sys.argv[1])
src = pathlib.Path("guest-fixture/image-rootfs.tar")
slot = pathlib.Path("formatter/rootfs.slot")
log = pathlib.Path("formatter/qemu.log")

if src.stat().st_size % 512:
    raise RuntimeError("formatter input must be aligned to 512 bytes")
with slot.open("wb") as output:
    output.truncate(256 * 1024 * 1024)
with log.open("wb") as stream:
    subprocess.run([
        "qemu-system-aarch64", "-machine", "virt,accel=tcg", "-cpu", "cortex-a72",
        "-nic", "none",
        "-smp", "1", "-m", "1024", "-nodefaults", "-display", "none", "-monitor", "none",
        "-chardev", "stdio,id=guestconsole", "-device", "virtio-serial-device",
        "-device", "virtconsole,chardev=guestconsole",
        "-device", "virtio-blk-device,drive=source",
        "-drive", f"if=none,file={src},format=raw,readonly=on,id=source",
        "-device", "virtio-blk-device,drive=result",
        "-drive", f"if=none,file={slot},format=raw,id=result",
        "-kernel", str(base / "Image"), "-initrd", "formatter/initramfs.gz",
        "-append", "console=hvc0 rdinit=/init panic=-1",
    ], stdout=stream, stderr=stream, stdin=subprocess.DEVNULL, timeout=190, check=True)

reported = re.findall(rb"^M4-21-formatter-size:([0-9]+)\r?$", log.read_bytes(), re.MULTILINE)
if len(reported) != 1:
    raise RuntimeError("isolated formatter did not report completion")
size = int(reported[0])
if size <= 0 or size > slot.stat().st_size or size % 4096:
    raise RuntimeError("isolated formatter returned an invalid size")
with slot.open("r+b") as output:
    output.truncate(size)
fixture = src.parent
rootfs = fixture / "rootfs.img"
slot.replace(rootfs)
shutil.copy(base / "Image", fixture / "Image")
manifest = json.loads((base / "guest-manifest.json").read_text())
with rootfs.open("rb") as image:
    manifest["rootfs_sha256"] = hashlib.file_digest(image, "sha256").hexdigest()
(fixture / "guest-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
print("Verified private EROFS fixture size", size)
