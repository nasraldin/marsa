# M0 Foundation

**Outcome:** a Cargo workspace that builds, lints and tests in CI, with shared types and protocol definitions in place.

---

### M0-01 Create the Cargo workspace

- **Status:** done
- **Depends on:** none
- **Docs:** [02 Repository layout](../02-architecture-overview.md#6-repository-layout), [02 Crate dependencies](../02-architecture-overview.md#7-crate-dependencies)
- **Do:** Create the workspace with every crate listed in the repository layout as a compiling stub (library or binary). Add `rust-toolchain.toml` pinning a stable toolchain, `rustfmt.toml`, workspace-level lint settings, `.gitignore` and `.editorconfig`.
- **Deliver:** `Cargo.toml`, `crates/*/Cargo.toml`, `crates/*/src/lib.rs` or `main.rs`, toolchain and lint config files.
- **Accept when:**
  - `cargo build --workspace` succeeds on macOS arm64.
  - `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass.
  - Crate dependencies match the crate graph; no forbidden edges exist.

### M0-02 Add project governance files

- **Status:** done
- **Depends on:** M0-01
- **Docs:** [16 Open questions](../16-risks-and-open-questions.md) (Q-01, Q-09), [11 Vulnerability response](../11-security.md#vulnerability-response)
- **Do:** Add `CONTRIBUTING.md`, `SECURITY.md` (private reporting channel placeholder to be filled by the owner), `CODE_OF_CONDUCT.md`, issue and pull-request templates that ask for the task ID. Add the `LICENSE` file **only after Q-01 is answered**; until then leave a clearly marked note in `README.md`.
- **Deliver:** the governance files under the repository root and `.github/`.
- **Accept when:**
  - The pull-request template requires a task ID, test notes and resolved **Verify** items.
  - No licence file or licence header exists unless Q-01 is answered and recorded in the decision records.

### M0-03 Build the `xtask` runner

- **Status:** done
- **Depends on:** M0-01
- **Docs:** [12 Toolchain](../12-build-release-update.md#toolchain)
- **Do:** Create the `xtask` crate with the following subcommands: `check`, `test`, `guest`, `fetch-upstream`, `bundle`, `sign`, `package`, `bench`. Implement `check` (fmt, clippy, deny) and `test` now. Commit a baseline permissive licence policy so `check` is usable before M0-07 expands it. Future commands appear in help and fail until their implementation lands.
- **Deliver:** `xtask/`, a baseline `deny.toml`, and a `.cargo/config.toml` alias so `cargo xtask <cmd>` works.
- **Accept when:**
  - `cargo xtask check` runs all static checks and fails on any warning.
  - `cargo xtask --help` lists every subcommand with a one-line description.

### M0-04 Implement `marsa-core`

- **Status:** done
- **Depends on:** M0-01
- **Docs:** [03 Settings](../03-host-daemon.md#settings), [03 Filesystem layout](../03-host-daemon.md#filesystem-layout-on-the-host)
- **Do:** Implement shared types: the paths module (all `~/.marsa` locations behind one API, overridable for tests), the settings model with validation and defaults, versioned settings migration, the common error types, and a `platform` trait module with a macOS implementation stub.
- **Deliver:** `crates/marsa-core` with unit tests.
- **Accept when:**
  - Settings round-trip through TOML and reject invalid values with clear errors.
  - A settings file from an older schema version migrates in a test.
  - No code outside the paths module builds a `~/.marsa` path by hand.

### M0-05 Define the protocols in `marsa-proto`

- **Status:** done
- **Depends on:** M0-01
- **Docs:** [03 Typed API](../03-host-daemon.md#typed-api), [05 marsa-agent](../05-guest-image.md#marsa-agent)
- **Do:** Write protobuf definitions for the host API (`marsa.v1`: all services in the typed-API table) and the guest control protocol (`marsa.guest.v1`: handshake, time, network config, certificates, port events, health, stats, trim, shares, shutdown). Set up code generation. Define protocol version constants and a compatibility rule.
- **Deliver:** `.proto` files, generated Rust modules, a `PROTOCOL.md` describing versioning and the handshake.
- **Accept when:**
  - Generated code compiles for both the host target and `aarch64-unknown-linux-musl`.
  - Every message that crosses the guest boundary has a documented maximum size.
  - A test rejects a handshake with an incompatible protocol version.

### M0-06 Set up continuous integration

- **Status:** done
- **Depends on:** M0-03
- **Docs:** [12 Continuous integration](../12-build-release-update.md#continuous-integration), V-13, Q-05
- **Do:** Add the Tier 1 workflow (static checks, unit and integration tests on hosted runners). Verify whether hosted Apple silicon runners can run Virtualization.framework guests (V-13) and record the finding. Add the Tier 2 workflow definition targeting a runner label for a Mac with virtualization, disabled until a runner and real-VM test package exist. Restrict dispatch to the default branch of a private validation repository.
- **Deliver:** `.github/workflows/tier1.yml`, `.github/workflows/tier2.yml`, an updated V-13 entry in the risks doc.
- **Accept when:**
  - Tier 1 runs on every push and pull request and is green.
  - The V-13 finding is written into [16](../16-risks-and-open-questions.md) with its source.
  - Tier 2 is defined and documented, with instructions for registering the runner in a private validation repository.

### M0-07 Add dependency policy checks

- **Status:** done
- **Depends on:** M0-03
- **Docs:** [11 Updates and supply chain](../11-security.md#updates-and-supply-chain)
- **Do:** Configure `cargo-deny` (allowed licences, banned crates, advisory database, duplicate detection) and a scheduled `cargo-audit` run. The allowed-licence list stays permissive-only until Q-01 is answered.
- **Deliver:** `deny.toml`, a scheduled workflow.
- **Accept when:**
  - `cargo xtask check` fails when a dependency with a disallowed licence is added (proved by a test branch or a documented dry run).
  - The scheduled audit opens an issue or fails visibly on a new advisory.

### M0-08 Check the documentation in CI

- **Status:** todo
- **Depends on:** M0-06
- **Docs:** [docs/README](../README.md)
- **Do:** Add a CI job that checks internal Markdown links and anchors in `docs/`, validates Mermaid blocks by rendering them, and verifies that every task ID referenced anywhere in `docs/` exists in a milestone file.
- **Deliver:** a docs-check script under `xtask` and a CI job.
- **Accept when:**
  - A broken link, an invalid Mermaid block or a reference to a missing task ID fails the job.
  - The job passes on the current docs.
