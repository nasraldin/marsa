"""Append test-only endpoints and certificate trust to a disposable image tar."""

import io
import pathlib
import sys
import tarfile

base = pathlib.Path(sys.argv[1])
destination = pathlib.Path(sys.argv[2])
helper = pathlib.Path(sys.argv[3])
intercept = sys.argv[4:] == ["--intercept"]
original_init = "./usr/bin/marsa-init"
guest_certificates = "./etc/ssl/certs/ca-certificates.crt"

launcher = b"""#!/bin/sh
/sbin/ip link set lo up
/usr/bin/marsa-proxy-fixture &
exec /usr/bin/marsa-init.real "$@"
"""


def add(archive, name, mode, data):
    info = tarfile.TarInfo(name)
    info.mode = mode
    info.size = len(data)
    archive.addfile(info, io.BytesIO(data))


if intercept:
    launcher = launcher.replace(b"marsa-proxy-fixture &", b"marsa-proxy-fixture --intercept &")

init_found = certs_found = False
with tarfile.open(base / "image-rootfs.tar", "r") as source:
    with tarfile.open(destination / "image-rootfs.tar", "w") as image:
        for member in source:
            if member.name == original_init:
                member.name = original_init + ".real"
                init_found = True
            if member.name == guest_certificates:
                certs_found = True
            if member.name == guest_certificates and not intercept:
                trusted = source.extractfile(member).read()
                test_ca = (destination / "ca.pem").read_bytes()
                add(image, guest_certificates, 0o644, trusted + b"\n" + test_ca)
            else:
                content = source.extractfile(member) if member.isreg() else None
                image.addfile(member, content)
        if not (init_found and certs_found):
            raise RuntimeError("expected packaged guest init and trusted CA bundle")
        add(image, original_init, 0o755, launcher)
        add(image, "./usr/bin/marsa-proxy-fixture", 0o755, helper.read_bytes())
        folder = tarfile.TarInfo("./usr/local/lib/marsa-fixture")
        folder.type = tarfile.DIRTYPE
        folder.mode = 0o700
        image.addfile(folder)
        for name in ("leaf.pem", "leaf.key"):
            add(image, folder.name + "/" + name, 0o600, (destination / name).read_bytes())
