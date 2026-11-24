"""Regression checks for the registry identities used in dependency audits."""

import importlib.util
import pathlib
import tomllib
import unittest

SCRIPT = pathlib.Path(__file__).with_name("audit-path-dependencies.py")
SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
CHECKSUM = "a" * 64
ORIGINS = {("upstream", "1.2.3"): CHECKSUM}


class AuditInputsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not SCRIPT.exists():
            raise AssertionError("Audit input generator is missing")
        spec = importlib.util.spec_from_file_location("audit_path_dependencies", SCRIPT)
        cls.audit = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.audit)

    def test_cargo_deny_metadata_keeps_graph_and_adds_registry_source(self):
        source_path = "/checkout/third-party/gpui/upstream-1.2.3/Cargo.toml"
        package_id = "path+file:///checkout/third-party/gpui/upstream-1.2.3#upstream@1.2.3"
        package = {
            "name": "upstream", "version": "1.2.3", "id": package_id,
            "source": None, "manifest_path": source_path,
        }
        metadata = {
            "packages": [package],
            "workspace_members": [],
            "resolve": {"nodes": [{"id": package_id}]},
        }
        origins = {**ORIGINS, ("optional-feature", "2.0.0"): "c" * 64}
        self.audit.restore_metadata_sources(metadata, origins, pathlib.Path("/checkout"))
        self.assertEqual(metadata["packages"][0]["source"], SOURCE)
        self.assertEqual(metadata["packages"][0]["manifest_path"], source_path)
        self.assertEqual(metadata["resolve"]["nodes"][0]["id"], package_id)

    def test_cargo_audit_lock_preserves_registry_and_workspace_packages(self):
        original = '''version = 4

[[package]]
name = "marsa-gui"
version = "0.1.0"
dependencies = ["upstream"]

[[package]]
name = "upstream"
version = "1.2.3"
dependencies = ["registry-dep"]

[[package]]
name = "registry-dep"
version = "3.4.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
'''
        restored = self.audit.restore_lock_sources(original, ORIGINS, {("marsa-gui", "0.1.0")})
        marsa, upstream, registry = tomllib.loads(restored)["package"]
        self.assertNotIn("source", marsa)
        self.assertEqual(upstream["source"], SOURCE)
        self.assertEqual(upstream["checksum"], CHECKSUM)
        self.assertEqual(upstream["dependencies"], ["registry-dep"])
        self.assertEqual(registry, tomllib.loads(original)["package"][2])

    def test_missing_provenance_fails(self):
        lock = 'version = 4\n\n[[package]]\nname = "upstream"\nversion = "1.2.3"\n'
        with self.assertRaisesRegex(ValueError, "upstream"):
            self.audit.restore_lock_sources(lock, {}, {("upstream", "1.0.0")})


if __name__ == "__main__":
    unittest.main()
