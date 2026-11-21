import { relative, sep } from "node:path";

const taskId = /\bM\d+-\d{2}\b/g;
const milestoneFile = /^docs\/plan\/(M\d+)-[^/]+\.md$/;

export function checkTaskIds(repository, documents) {
  const definitions = new Set();
  const errors = [];

  for (const document of documents) {
    const path = relative(repository, document.path).split(sep).join("/");
    const milestone = milestoneFile.exec(path)?.[1];
    if (!milestone) continue;

    for (const heading of document.headings) {
      if (heading.level !== 3) continue;
      const id = /^(M\d+-\d{2})\b/.exec(heading.raw)?.[1];
      if (!id) continue;
      if (!id.startsWith(`${milestone}-`)) {
        errors.push(`${path}: task ${id} is declared under ${milestone}`);
      } else if (definitions.has(id)) {
        errors.push(`${path}: task ${id} is declared twice`);
      } else {
        definitions.add(id);
      }
    }
  }

  for (const document of documents) {
    const path = relative(repository, document.path);
    for (const id of new Set(document.text.match(taskId))) {
      if (!definitions.has(id)) errors.push(`${path}: unknown task ID ${id}`);
    }
  }
  return errors;
}
