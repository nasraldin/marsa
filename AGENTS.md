# AGENTS.md

Instructions for AI coding agents (and people) working in this repository. Read this file fully before changing anything.

## What this project is

Marsa is a fast, open-source, Docker-compatible container desktop for macOS on Apple silicon. The design is in [`docs/`](docs/README.md). The work is broken into tasks in [`docs/plan/`](docs/plan/README.md).

## Before you start any task

1. Read [`docs/README.md`](docs/README.md) and [`docs/02-architecture-overview.md`](docs/02-architecture-overview.md).
2. Read the component doc(s) linked from your task.
3. Read [`docs/15-decisions.md`](docs/15-decisions.md). Decisions marked Accepted are not yours to change.
4. Check [`docs/16-risks-and-open-questions.md`](docs/16-risks-and-open-questions.md). If your task is blocked by an open question (Q-xx), stop and report; do not decide it yourself.

## How to pick and run a task

1. Open [`docs/plan/README.md`](docs/plan/README.md) and find the earliest milestone with unfinished tasks.
2. Pick a task whose **Depends on** tasks are all `done`. Never start a task with unmet dependencies.
3. Set its **Status** to `in progress` in the task file, in the same change as your first commit.
4. Work on a branch named `task/<task-id>-<short-slug>`, for example `task/M3-06-vm-supervisor`.
5. Do exactly what the task says. If you find the task is wrong or incomplete, fix the task file first and say why in the commit message.
6. Meet every line under **Accept when**. Then set **Status** to `done` and open a pull request that names the task ID.

One task per pull request. Do not bundle tasks.

## Status values

`todo`, `in progress`, `blocked (reason)`, `done`. Only these.

## The three labels

The docs mark statements as **Decided**, **Proposed** or **Verify**.

- **Decided:** follow it. To change it, write a new decision record and get the owner's agreement first.
- **Proposed:** follow it by default. You may refine it when implementation shows a better way; record the change in `docs/15-decisions.md` and update the affected doc in the same pull request.
- **Verify:** this is an unconfirmed external fact. Before relying on it, check the current upstream documentation or source for the exact version you are using. Then update the doc with what you found and remove or resolve the label. Never write code against an API from memory.

Fast-moving dependencies that always need checking: GPUI, gpui-component, libkrun, Apple framework APIs, the Docker engine and its static-binary layout, the MCP specification.

## Hard rules

- **Do not modify the Docker engine.** Marsa ships upstream binaries unchanged.
- **Do not add a `LICENSE` file or licence headers** until open question Q-01 is answered.
- **Never commit secrets,** signing identities, tokens or private keys. Never print them in logs.
- **No `left` or `right` in GUI layout code.** Use the `start` and `end` API from `marsa-ui`.
- **Every user-facing string exists in both `en-US` and `ar`.** The build must fail otherwise.
- **Every GUI change is checked in four variants:** English light, English dark, Arabic light, Arabic dark.
- **No root.** Nothing in Marsa runs as root on the host. The one optional administrator action is the `/var/run/docker.sock` symlink.
- **No telemetry** unless a task explicitly adds an opt-in.
- **Do not weaken a security control to make a test pass.** Fix the code or the test.
- **Destructive operations need explicit confirmation** in the CLI and GUI, and human approval when requested by an MCP client.
- **Do not add a dependency** without checking its licence with `cargo-deny` and stating why it is needed in the pull request.

## Code conventions

- Rust stable, edition and toolchain pinned in `rust-toolchain.toml`.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must pass.
- Library crates return typed errors (`thiserror`); binaries may use `anyhow` at the top level.
- No `unwrap()` or `expect()` outside tests and provably infallible cases with a comment.
- No `unsafe` without a `// SAFETY:` comment explaining the invariant, and none outside the VM backend, FFI and platform modules.
- Async code uses Tokio. Do not block the runtime; use `spawn_blocking` for blocking work.
- Everything received from the guest, from Docker clients and from the network is untrusted input: validate sizes and shapes.
- Public items have doc comments. Module-level docs explain the why.
- Platform-specific code lives behind traits in a `platform` module so Windows and Linux can be added later.
- Dependency direction must match the crate graph in `docs/02-architecture-overview.md`. The GUI, CLI and MCP server depend only on `marsa-client` (and `marsa-ui` for the GUI).

## Tests

Follow the "Definition of done" in [`docs/13-testing-and-benchmarks.md`](docs/13-testing-and-benchmarks.md#definition-of-done-for-any-task).

- Most logic must be testable with the `FakeHypervisor`, without a real VM.
- Tests that need a real VM are marked so they run only on the virtualization runner.
- A bug fix comes with a test that fails before the fix.

## Commits and pull requests

- Commit subject: `<task-id>: <imperative summary>`, for example `M3-06: implement VM supervisor state machine`.
- The pull request description lists: what changed, how it was tested, which **Verify** items were resolved and what was found, and any follow-up tasks created.
- Keep pull requests small enough to review. If a task is too big for one reviewable change, split the task in the plan first.

## When you are blocked or unsure

- Mark the task `blocked (reason)` and write a short note under the task explaining what is needed.
- If two docs disagree, the decision records win, then the component doc, then the task file. Fix the disagreement in your pull request.
- If something in the design seems wrong, say so in the pull request with evidence. Do not silently build something different.

## Adding tasks

New work found during a task goes into the right milestone file with the next free ID, the same fields as existing tasks and a `todo` status. Mention it in your pull request.
