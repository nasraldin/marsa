"""Check that executable inventory fails closed if the image gains an unknown binary."""

import contextlib
import io
import tarfile
import unittest

from binaries import verify_inventory


@contextlib.contextmanager
def fixture(*records):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as archive:
        for path, mode, contents in records:
            member = tarfile.TarInfo(path)
            member.mode = mode
            member.size = len(contents)
            archive.addfile(member, io.BytesIO(contents))
    buffer.seek(0)
    with tarfile.open(fileobj=buffer, mode="r:") as archive:
        yield archive


class BinaryInventory(unittest.TestCase):
    def test_unowned_binary_magic_is_rejected_without_executable_permission(self):
        with (
            fixture(("usr/bin/new", 0o644, b"\x7fELF")) as rootfs,
            self.assertRaisesRegex(ValueError, "Guest executable has no version and licence"),
        ):
            verify_inventory(rootfs, [])

    def test_known_binary_and_generated_configuration_are_accepted(self):
        files = [("usr/bin/probe", 0o755, b"\x7fELF"), ("etc/generated", 0o644, b"content")]
        with fixture(*files) as rootfs:
            verify_inventory(rootfs, [{"files": ["usr/bin/probe"]}])

    def test_inventory_cannot_claim_an_absent_binary(self):
        with fixture() as rootfs, self.assertRaisesRegex(ValueError, "absent file"):
            verify_inventory(rootfs, [{"files": ["usr/bin/probe"]}])
