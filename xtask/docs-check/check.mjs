import { realpathSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { checkLinks } from "./links.mjs";
import { readMarkdownFiles } from "./markdown.mjs";
import { checkDiagrams } from "./mermaid.mjs";
import { checkTaskIds } from "./tasks.mjs";

const repository = realpathSync(fileURLToPath(new URL("../../", import.meta.url)));
const documents = readMarkdownFiles(join(repository, "docs"));
const errors = [
  ...checkLinks(repository, documents),
  ...checkTaskIds(repository, documents),
  ...await checkDiagrams(repository, documents),
];

if (errors.length) {
  console.error(errors.join("\n"));
  process.exitCode = 1;
} else {
  const diagrams = documents.reduce((count, document) => count + document.diagrams.length, 0);
  console.info(`Checked ${documents.length} Markdown files and rendered ${diagrams} Mermaid diagrams.`);
}
