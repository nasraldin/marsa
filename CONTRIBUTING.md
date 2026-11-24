# Contributing to Marsa

Start with the [architecture and component documentation](docs/README.md) and the [implementation plan](docs/plan/README.md). Check that a task's dependencies are complete before starting it. Follow [AGENTS.md](AGENTS.md) for the task workflow, code conventions, and security requirements.

## Build and verify

Use an Apple silicon Mac with Rustup. The repository's toolchain file selects the Rust version and required components.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

See [GPUI source and upgrade policy](docs/reference/gpui-upgrade.md) when changing GUI dependencies. Run the task's acceptance checks and record the commands and results in the pull request. The [testing guide](docs/13-testing-and-benchmarks.md) describes checks for VM, security, and GUI changes.

## Submit a change

- Use one branch and one pull request per task. Name the branch `task/<task-id>-<short-slug>`.
- Start each commit subject with the task ID and use an imperative summary.
- Include the task ID, what changed, test results, resolved **Verify** items, and any follow-up tasks in the pull request.
- Remove credentials, private paths, and personal data from logs and screenshots before publishing them.
- Follow the [Code of Conduct](CODE_OF_CONDUCT.md) and use the [security policy](SECURITY.md) for sensitive reports.

Marsa uses the [MIT licence](LICENSE). Additional contribution terms are pending [Q-09](docs/16-risks-and-open-questions.md#open-questions-for-the-project-owner).
