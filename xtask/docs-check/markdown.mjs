import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

import { Marked } from "marked";
import { getHeadingList, gfmHeadingId, resetHeadings } from "marked-gfm-heading-id";

const markdown = new Marked(gfmHeadingId());

export function isMarkdown(path) {
  return /\.(?:md|markdown)$/i.test(path);
}

export function readMarkdownFiles(directory) {
  return readdirSync(directory, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && isMarkdown(entry.name))
    .map((entry) => join(entry.parentPath, entry.name))
    .sort()
    .map(readMarkdown);
}

export function readMarkdown(path) {
  const text = readFileSync(path, "utf8");
  const tokens = markdown.lexer(text);
  const targets = [];
  const diagrams = [];

  markdown.walkTokens(tokens, (token) => {
    if (token.type === "link" || token.type === "image") {
      targets.push(token.href);
    }
    if (token.type === "code" && token.lang?.split(/\s+/, 1)[0] === "mermaid") {
      diagrams.push(token.text);
    }
  });

  // Run the heading extension on the same tokens. The HTML is never used.
  resetHeadings();
  markdown.parser(tokens);
  const headings = getHeadingList();
  return { path, text, targets, diagrams, headings };
}
