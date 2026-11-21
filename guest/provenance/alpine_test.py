"""Regression checks for Alpine package coverage and notice integrity."""

import hashlib
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import alpine


APK_RECORD = "\n".join([
    "P:busybox",
    "V:1.37.0-r31",
    "A:aarch64",
    "o:busybox",
    "L:GPL-2.0-only",
    "c:" + "1" * 40,
    "F:bin",
    "R:busybox",
    "F:etc",
    "R:udhcpc.conf",
])


class AlpineInventoryTests(unittest.TestCase):
    def test_preserves_exact_license_and_file_ownership(self):
        package, = alpine.parse_database(APK_RECORD)
        self.assertEqual(package["license"], "GPL-2.0-only")
        self.assertEqual(package["files"], ["bin/busybox", "etc/udhcpc.conf"])
        self.assertEqual(package["commit"], "1" * 40)
        with self.assertRaisesRegex(ValueError, "invalid archive-relative path"):
            alpine.parse_database(APK_RECORD.replace("F:etc", "F:../etc"))

    def test_only_busybox_applet_links_are_attributed_to_busybox(self):
        member = tarfile.TarInfo("usr/bin/awk")
        member.type = tarfile.SYMTYPE
        member.linkname = "../../bin/busybox"
        self.assertTrue(alpine.is_busybox_applet(member.name, member))
        member.linkname = "../../bin/sh"
        self.assertFalse(alpine.is_busybox_applet(member.name, member))

    def test_notice_pins_fail_when_package_or_bytes_change(self):
        packages = alpine.parse_database(APK_RECORD)
        notice = b"fixture copyright and permission\n"
        lock = {"busybox": {
            "packages": [{key: value for key, value in packages[0].items()
                           if key not in {"origin", "files"}}],
            "recipe_sha256": "2" * 64,
            "sources": [],
            "notices": [{"file": "LICENSE", "sha256": hashlib.sha256(notice).hexdigest()}],
        }}
        with tempfile.TemporaryDirectory() as temporary:
            staging = Path(temporary)
            texts = staging / "texts"
            license_path = texts / "busybox" / "LICENSE"
            license_path.parent.mkdir(parents=True)
            license_path.write_bytes(notice)
            with patch.object(alpine, "TEXTS", texts):
                source = alpine.verified_sources(packages, lock, staging)
                copied = source["busybox"]["notices"][0]["file"]
                self.assertEqual((staging / copied).read_bytes(), notice)
                with self.assertRaisesRegex(ValueError, "pins are stale"):
                    changed = alpine.parse_database(APK_RECORD.replace("r31", "r32"))
                    alpine.verified_sources(changed, lock, staging)
                license_path.write_bytes(b"changed\n")
                with self.assertRaisesRegex(ValueError, "digest mismatch"):
                    alpine.verified_sources(packages, lock, staging)


if __name__ == "__main__":
    unittest.main()
