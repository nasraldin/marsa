import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";

import { checkLinks } from "./links.mjs";
import { readMarkdownFiles } from "./markdown.mjs";
import { checkDiagrams } from "./mermaid.mjs";
import { checkTaskIds } from "./tasks.mjs";

function fixture(t, files) {
  const repository = realpathSync(mkdtempSync(join(tmpdir(), "marsa-docs-")));
  t.after(() => rmSync(repository, { recursive: true }));
  for (const [name, contents] of Object.entries(files)) {
    const path = join(repository, name);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, contents);
  }
  return { repository, documents: readMarkdownFiles(join(repository, "docs")) };
}

test("resolves inline and reference links, local images, and GitHub heading anchors", (t) => {
  const { repository, documents } = fixture(t, {
    "docs/index.md": [
      "# Same",
      "# **Same**",
      "[Duplicate heading](#same-1)",
      "[Reference link][ref]",
      "[Root-relative](/docs/sub/other%20page.md#with-code)",
      "![Image](image.png)",
      "",
      "[ref]: sub/other%20page.md#with-code",
      "",
      "```markdown",
      "[Example only](missing.md)",
      "```",
    ].join("\n"),
    "docs/image.png": "",
    "docs/sub/other page.md": "# With `code`\n",
  });
  assert.deepEqual(checkLinks(repository, documents), []);
});

test("reports missing targets and stale heading anchors", (t) => {
  const { repository, documents } = fixture(t, {
    "docs/index.md": "# Heading\n[Broken](missing.md)\n[Stale](#stale)\n",
  });
  assert.deepEqual(checkLinks(repository, documents), [
    "docs/index.md: missing.md: Target does not exist",
    "docs/index.md: #stale: Missing heading anchor #stale",
  ]);
});

test("reports task IDs without a matching milestone task heading", (t) => {
  const { repository, documents } = fixture(t, {
    "docs/plan/M0-foundation.md": "# Foundation\n### M0-01 Cargo workspace\n",
    "docs/plan/README.md": "See M0-01 and M0-99.\n\n```\n### M0-99 Example\n```\n",
  });
  assert.deepEqual(checkTaskIds(repository, documents), [
    "docs/plan/README.md: unknown task ID M0-99",
  ]);
});

test("renders a valid Mermaid diagram and reports the invalid block", async (t) => {
  const { repository, documents } = fixture(t, {
    "docs/diagrams.md": [
      "```mermaid",
      "flowchart LR",
      "  A --> B",
      "```",
      "",
      "~~~mermaid",
      "flowchart LR",
      "  A -->",
      "~~~",
    ].join("\n"),
  });
  const errors = await checkDiagrams(repository, documents);
  assert.equal(errors.length, 1);
  assert.match(errors[0], /^docs\/diagrams.md: Mermaid block 2: /);
});
