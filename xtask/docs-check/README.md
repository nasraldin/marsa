# Documentation checks

Use Node.js 26.13.1 and a stable Google Chrome installation. From the repository root, run the checks with the Chrome executable for your platform.

```sh
npm --prefix xtask/docs-check ci --ignore-scripts
export PUPPETEER_EXECUTABLE_PATH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
npm --prefix xtask/docs-check test
npm --prefix xtask/docs-check run check
```

The checker reads Markdown under `docs/`, resolves local Markdown links and GitHub heading anchors, and checks task IDs against the milestone files. It renders Mermaid code blocks to SVG in memory. The tests exercise broken links and anchors, undefined tasks, and invalid Mermaid syntax.
