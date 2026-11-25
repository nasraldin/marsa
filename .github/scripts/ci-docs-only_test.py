"""Exercise the CI classifier against real Git changes."""

from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("ci-docs-only.sh").resolve()


class DocsOnlyTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.repo = Path(directory.name)
        self.git("init", "-q")
        self.git("config", "user.email", "ci@example.invalid")
        self.git("config", "user.name", "CI test")
        self.write("src.rs")
        self.base = self.commit()

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.repo, text=True).strip()

    def write(self, path):
        file = self.repo / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text("documentation test\n")

    def commit(self):
        self.git("add", "-A")
        self.git("commit", "-qm", "test commit")
        return self.git("rev-parse", "HEAD")

    def docs_only(self, base, head):
        return subprocess.check_output(
            ["bash", SCRIPT, base, head],
            cwd=self.repo,
            stderr=subprocess.PIPE,
            text=True,
        ).strip()

    def test_docs_and_root_readme_only(self):
        self.write("docs/plan/with space\nand newline.md")
        self.write("README.md")
        self.assertEqual(self.docs_only(self.base, self.commit()), "true")

    def test_push_range_includes_earlier_code_commit(self):
        self.write("other.rs")
        self.commit()
        self.write("docs/plan/task.md")
        self.assertEqual(self.docs_only(self.base, self.commit()), "false")

    def test_rename_from_code_to_docs_counts_both_paths(self):
        (self.repo / "docs").mkdir()
        self.git("mv", "src.rs", "docs/moved.md")
        self.assertEqual(self.docs_only(self.base, self.commit()), "false")

    def test_workflow_and_non_markdown_changes_require_builds(self):
        for path in (".github/workflows/tier1.yml", "docs/generate.py", "guest/README.md"):
            with self.subTest(path=path):
                previous = self.git("rev-parse", "HEAD")
                self.write(path)
                self.assertEqual(self.docs_only(previous, self.commit()), "false")

    def test_merge_first_parent_excludes_base_branch_changes(self):
        self.git("checkout", "-qb", "docs")
        self.write("docs/plan/task.md")
        docs = self.commit()
        self.git("checkout", "-q", "--detach", self.base)
        self.write("base-only.rs")
        self.commit()
        self.git("merge", "-q", "--no-ff", "--no-edit", docs)
        merge = self.git("rev-parse", "HEAD")
        self.assertEqual(self.docs_only(f"{merge}^1", merge), "true")

    def test_missing_or_empty_diff_requires_builds(self):
        for base in (self.base, "", "0" * 40):
            with self.subTest(base=base):
                self.assertEqual(self.docs_only(base, self.base), "false")


if __name__ == "__main__":
    unittest.main()
