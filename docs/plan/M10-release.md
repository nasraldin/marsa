# M10 Release

**Outcome:** a signed, notarized Marsa that installs cleanly, updates itself safely and can be rolled back.

Design: [12 Build, release and update](../12-build-release-update.md), [11 Updates and supply chain](../11-security.md#updates-and-supply-chain).

---

### M10-01 App bundle assembly

- **Status:** todo
- **Depends on:** M2-09, M4-03, M6-09
- **Docs:** [12 App bundle layout](../12-build-release-update.md#app-bundle-layout)
- **Do:** Implement `cargo xtask bundle`: assemble `Marsa.app` with the host binaries, the guest image and manifest, the upstream binaries, locales, fonts, the launch agent definition, `Info.plist`, icons and third-party licences.
- **Deliver:** the bundle step and `packaging/macos/`.
- **Accept when:**
  - The bundle launches from any location and finds its resources.
  - `marsad` loads the guest image from inside the bundle.
  - Bundle size is reported by the build and tracked in CI.

### M10-02 Signing and notarization pipeline

- **Status:** todo
- **Depends on:** M10-01, M3-10
- **Docs:** [12 Signing and notarization](../12-build-release-update.md#signing-and-notarization), Q-04
- **Do:** Complete `cargo xtask sign`: sign every binary inside-out with hardened runtime and per-binary entitlements, re-sign bundled upstream binaries, notarize, staple, and verify with Gatekeeper tooling. Credentials come only from the release environment.
- **Deliver:** the signing step, entitlements files.
- **Accept when:**
  - The stapled app opens on a clean Mac without warnings and boots the VM.
  - A verification step in CI fails if any binary is unsigned or lacks hardened runtime.

### M10-03 Updater design and manifest format

- **Status:** done
- **Depends on:** M3-08
- **Docs:** [12 Auto-update](../12-build-release-update.md#auto-update)
- **Do:** Evaluate building the updater in `marsad` against adopting an existing macOS update framework, on security, headless support and maintenance. Decide and record it. Define the update manifest format, the signature scheme and key handling (offline or hardware-backed key, pinned public key, rotation plan), channels and the downgrade rule.
- **Deliver:** a decision record, `docs/reference/update-manifest.md`, manifest types in `marsa-core`.
- **Accept when:**
  - The decision record states the choice and the reasons.
  - The key rotation and compromise procedures are written down.

### M10-04 Updater implementation and rollback

- **Status:** todo
- **Depends on:** M10-03, M10-02
- **Docs:** [12 Auto-update](../12-build-release-update.md#auto-update), [11 Updates and supply chain](../11-security.md#updates-and-supply-chain)
- **Do:** Implement the `Updates` service: check, verify the manifest signature, download, verify hash and code signature, stop the VM cleanly, swap the bundle atomically while keeping the previous version, relaunch, health-check, and roll back automatically on failure.
- **Deliver:** updater module and tests with a local update server.
- **Accept when:**
  - Tampered manifest, wrong hash, unsigned bundle and downgrade are each rejected (tested).
  - A new version that fails its health check is rolled back automatically and the user is told.
  - Running containers are never stopped for an update without the user confirming.

### M10-05 Update experience in GUI and CLI

- **Status:** todo
- **Depends on:** M10-04, M7-10, M5-02
- **Docs:** [12 Auto-update](../12-build-release-update.md#auto-update)
- **Do:** Implement the update notice with release notes, install now or later, progress, and the rollback message in the GUI (four variants); `marsa update check|apply` in the CLI; channel selection in settings.
- **Deliver:** screens, commands and tests.
- **Accept when:**
  - The full update flow works from both the GUI and the CLI.
  - Release notes display in the selected language when a translation exists.

### M10-06 DMG and Homebrew cask

- **Status:** todo
- **Depends on:** M10-02
- **Docs:** [12 Distribution channels](../12-build-release-update.md#distribution-channels)
- **Do:** Implement `cargo xtask package` producing the signed DMG and the update archive, and write the Homebrew cask definition.
- **Deliver:** packaging step, cask file.
- **Accept when:**
  - Installing from the DMG and from the cask both yield a working app.
  - The cask's uninstall stanza removes everything the uninstall test checks.

### M10-07 Release workflow

- **Status:** todo
- **Depends on:** M10-04, M10-06
- **Docs:** [12 Continuous integration](../12-build-release-update.md#continuous-integration)
- **Do:** Add the tag-triggered release workflow: full checks, guest build, bundle, sign, notarize, package, generate and sign the manifest, publish to the `beta` channel, with a manual promotion step to `stable`. Secrets are scoped to this workflow on protected tags.
- **Deliver:** `.github/workflows/release.yml`, a release runbook.
- **Accept when:**
  - A test tag produces a complete beta release end to end.
  - Signing secrets are not accessible to pull-request workflows.

### M10-08 Licence notices, SBOM and source publication

- **Status:** todo
- **Depends on:** M2-09, M10-01, Q-01
- **Docs:** [05 Licence obligations](../05-guest-image.md#licence-obligations)
- **Do:** Generate third-party notices for all Rust dependencies, bundled upstream binaries, fonts and guest contents; ship them in the app and show them in About. Publish the SBOM, the kernel source reference and the kernel config with each release automatically.
- **Deliver:** notice generation in `xtask`, release artifacts.
- **Accept when:**
  - Every shipped component appears in the notices with its licence.
  - The release workflow fails if the SBOM or source references are missing.

### M10-09 User documentation

- **Status:** todo
- **Depends on:** M7-14, M8-09
- **Docs:** [09 Known and intended differences](../09-cli-and-docker-compat.md#known-and-intended-differences)
- **Do:** Write the user guide in English and Arabic: install, first run, sharing folders, networking and VPNs, resource settings, disk cleanup, agents, updating, troubleshooting, differences from Docker Desktop, uninstall.
- **Deliver:** `docs/user/` in both languages.
- **Accept when:**
  - Every intended difference and its error message is documented.
  - The Arabic guide is reviewed by the Arabic reviewer.

### M10-10 Beta release dry run (gate G7)

- **Status:** todo
- **Depends on:** M10-05, M10-07, M10-08, M10-09, M9-10, M9-11, M11-04, Q-03
- **Docs:** [12 Release checklist](../12-build-release-update.md#release-checklist)
- **Do:** Run the full release checklist on the beta channel: install on a clean Mac, update from the previous build, force a failed update to exercise rollback, run conformance and benchmarks on the released build, and uninstall cleanly. Confirm the name and trademark question (Q-03) is settled before anything is announced.
- **Deliver:** a checklist report and a go or no-go recommendation for the project owner.
- **Accept when:**
  - Every checklist item is ticked with evidence.
  - Update and rollback both work on the released artifacts.
  - The owner has recorded the go decision.
