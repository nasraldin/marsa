import { realpathSync } from "node:fs";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";

import { isMarkdown, readMarkdown } from "./markdown.mjs";

function inside(repository, path) {
  const name = relative(repository, path);
  return !isAbsolute(name) && name !== ".." && !name.startsWith(`..${sep}`);
}

export function checkLinks(repository, documents) {
  const byPath = new Map(documents.map((document) => [document.path, document]));
  const errors = [];
  for (const document of documents) {
    for (const href of new Set(document.targets)) {
      // Any explicit scheme or network host is outside the repository.
      if (/^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(href)) continue;
      try {
        const [, rawPath, rawAnchor] =
          /^([^?#]*)(?:\?[^#]*)?(?:#(.*))?$/s.exec(href);
        const path = decodeURIComponent(rawPath);
        const anchor = decodeURIComponent(rawAnchor ?? "");
        const unresolved = path.startsWith("/")
          ? resolve(repository, `.${path}`)
          : path ? resolve(dirname(document.path), path) : document.path;

        if (!inside(repository, unresolved)) throw new Error("Target is outside the repository");
        const target = realpathSync(unresolved);
        if (!inside(repository, target)) throw new Error("Target is outside the repository");
        if (anchor && isMarkdown(target)) {
          if (!byPath.has(target)) byPath.set(target, readMarkdown(target));
          if (!byPath.get(target).headings.some((heading) => heading.id === anchor)) {
            throw new Error(`Missing heading anchor #${anchor}`);
          }
        }
      } catch (error) {
        const message = error.code === "ENOENT" ? "Target does not exist" : error.message;
        errors.push(`${relative(repository, document.path)}: ${href}: ${message}`);
      }
    }
  }
  return errors;
}
