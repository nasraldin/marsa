"""Prepare the exact storage tools. Alpine verifies each package in the guest."""

import hashlib
import pathlib
import subprocess

ISO_SHA256 = "cf88d8ba67389d20d9ed307366b52e487c15d7d3ee3233b62b95f709daa1d430"
REPOSITORY = "https://dl-cdn.alpinelinux.org/alpine/v3.24/main/aarch64"

ISO_PACKAGES = (
    "e2fsprogs-1.47.4-r0",
    "e2fsprogs-extra-1.47.4-r0",
    "e2fsprogs-libs-1.47.4-r0",
    "libblkid-2.42.3-r1",
    "libcom_err-1.47.4-r0",
    "libeconf-0.8.3-r0",
    "libuuid-2.42.3-r1",
    "musl-1.2.6-r3",
)
REPOSITORY_PACKAGES = {
    "fstrim-2.42.3-r1": "2a8f896aec5059e80c3e57bbb678d38470cdaaa6a0926ce631baf3bc70d7e92d",
    "libmount-2.42.3-r1": "a2a6fc6aab7a59c70ec060f5fb8c950be42d1a017934bf32e5dd6984eaab15cc",
}


def digest(path: pathlib.Path) -> str:
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def prepare_packages(iso: pathlib.Path, destination: pathlib.Path) -> None:
    if digest(iso) != ISO_SHA256:
        raise ValueError("Alpine ISO SHA-256 mismatch")
    destination.mkdir(mode=0o700)
    for name in ISO_PACKAGES:
        filename = f"{name}.apk"
        with (destination / filename).open("xb") as output:
            subprocess.run(
                ["bsdtar", "-xOf", str(iso), f"apks/aarch64/{filename}"],
                stdout=output,
                check=True,
            )
    for name, checksum in REPOSITORY_PACKAGES.items():
        filename = f"{name}.apk"
        path = destination / filename
        subprocess.run(
            [
                "curl", "--proto", "=https", "--proto-redir", "=https", "--fail",
                "--location", "--silent", "--show-error", "--max-time", "30",
                "--max-filesize", "2097152", "--output", str(path),
                f"{REPOSITORY}/{filename}",
            ],
            check=True,
        )
        if digest(path) != checksum:
            raise ValueError(f"SHA-256 mismatch for {filename}")
