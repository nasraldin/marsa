import { relative } from "node:path";

import { renderMermaid } from "@mermaid-js/mermaid-cli";
import puppeteer from "puppeteer";

export async function checkDiagrams(repository, documents) {
  const diagrams = documents.flatMap(({ path, diagrams }) =>
    diagrams.map((text, index) => ({ path, text, number: index + 1 })));
  if (!diagrams.length) return [];

  const errors = [];
  const browser = await puppeteer.launch();
  try {
    for (const { path, text, number } of diagrams) {
      try {
        const { data } = await renderMermaid(browser, text, "svg", {
          mermaidConfig: { securityLevel: "strict" },
          fontEmbed: false,
        });
        if (!data.length) throw new Error("Mermaid returned an empty SVG");
      } catch (error) {
        const message = error.message.replaceAll("\n", "\n  ");
        errors.push(`${relative(repository, path)}: Mermaid block ${number}: ${message}`);
      }
    }
  } finally {
    await browser.close();
  }
  return errors;
}
