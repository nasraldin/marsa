# M10 Release

**Outcome:** a signed, notarized Marsa that installs cleanly, updates itself safely and can be rolled back.

Design: [12 Build, release and update](../12-build-release-update.md), [11 Updates and supply chain](../11-security.md#updates-and-supply-chain).

---

### M10-01 App bundle assembly

- **Status:** in progress
- **Depends on:** M6-09, M10-12, M10-13
- **Docs:** [12 App bundle layout](../12-build-release-update.md#app-bundle-layout)
- **Do:** Implement `cargo xtask bundle`: assemble `Marsa.app` with the host binaries, the verified guest and upstream resources from M10-12, locales, fonts, the launch agent definition, `Info.plist`, icons and third-party licences.
- **Deliver:** the bundle step and `packaging/macos/`.
- **Accept when:**
  - The bundle launches from any location and finds its resources.
  - `marsad` loads the guest image from inside the bundle.
  - Bundle size is reported by the build and tracked in CI.

The local qualification bundle was relocated to the user's Applications
directory. The bundled daemon booted the rebuilt guest and ran a public
ARM64 container on the physical Mac. Assembly reported 306,346,412 bytes
before signing. Tier 2 records this size as a workflow artifact. A Tier 2 run
and the combined M10-08 notices are still required before closing this task.

### M10-02 Signing and notarization pipeline

- **Status:** blocked (clean-Mac qualification, Q-04 and complete release notices pending)
- **Depends on:** M10-01, M3-10, M10-14, M10-15
- **Docs:** [12 Signing and notarization](../12-build-release-update.md#signing-and-notarization), Q-04
- **Do:** Complete `cargo xtask sign`. Sign every binary inside-out with a secure timestamp, hardened runtime and per-binary entitlements. Re-sign bundled upstream binaries. Verify the final submission artifact in private staging without concurrent writers, bind verification to its SHA-256, and confirm the digest before submitting the same artifact. Notarize, staple, and verify with Gatekeeper tooling. Credentials come only from the release environment.
- **Deliver:** the signing step, entitlements files.
- **Accept when:**
  - The stapled app opens on a clean Mac without warnings and boots the VM.
  - A verification step in CI fails if any binary is unsigned or lacks hardened runtime.
  - A release with a missing secure timestamp or a changed submission digest fails before submission.

The local signing mode passed the existing release signature policy for all
nine native executables. The notarized installer and clean-Mac open remain
subject to M3-10, Q-04, and the complete M10-08 notice handoff.

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

- **Status:** blocked (requires a policy-compliant maintained TUF verifier and the signed bundle qualification)
- **Depends on:** M10-03, M10-02, M10-11
- **Docs:** [12 Auto-update](../12-build-release-update.md#auto-update), [11 Updates and supply chain](../11-security.md#updates-and-supply-chain)
- **Do:** Implement the `Updates` service: check, verify the manifest signature, download, verify hash and code signature, stop the VM cleanly, swap the bundle atomically while keeping the previous version, relaunch, health-check, and roll back automatically on failure.
- **Deliver:** updater module and tests with a local update server.
- **Accept when:**
  - Tampered manifest, wrong hash, unsigned bundle and downgrade are each rejected (tested).
  - A new version that fails its health check is rolled back automatically and the user is told.
  - Running containers are never stopped for an update without the user confirming.

### M10-05 Update experience in GUI and CLI

- **Status:** blocked (the verified daemon updater and production release trust are unavailable)
- **Depends on:** M10-04, M7-10, M5-02
- **Docs:** [12 Auto-update](../12-build-release-update.md#auto-update)
- **Do:** Implement the update notice with release notes, install now or later, progress, and the rollback message in the GUI (four variants); `marsa update check|apply` in the CLI; channel selection in settings.
- **Deliver:** screens, commands and tests.
- **Accept when:**
  - The full update flow works from both the GUI and the CLI.
  - Release notes display in the selected language when a translation exists.

The desktop presents updater availability, channel settings, localized release
notes, progress, install confirmation and rollback status through the owner API.
Installation remains disabled until M10-04 exposes the verified service.

### M10-06 DMG and Homebrew cask

- **Status:** blocked (notarized distribution artifacts pending)
- **Depends on:** M10-02
- **Docs:** [12 Distribution channels](../12-build-release-update.md#distribution-channels)
- **Do:** Implement `cargo xtask package` producing the signed DMG and the update archive, and write the Homebrew cask definition.
- **Deliver:** packaging step, cask file.
- **Accept when:**
  - Installing from the DMG and from the cask both yield a working app.
  - The cask's uninstall stanza removes everything the uninstall test checks.

Packaging rejects a local signed app without a notarization receipt. The
installer, cask, and uninstall paths need qualification against signed,
notarized distribution artifacts.

### M10-07 Release workflow

- **Status:** todo
- **Depends on:** M10-04, M10-06, M9-09
- **Docs:** [12 Continuous integration](../12-build-release-update.md#continuous-integration)
- **Do:** Add the tag-triggered release workflow: full checks, guest build, bundle, sign, notarize, package, generate and sign the manifest, publish to the `beta` channel, with a manual promotion step to `stable`. Secrets are scoped to this workflow on protected tags.
- **Deliver:** `.github/workflows/release.yml`, a release runbook.
- **Accept when:**
  - A test tag produces a complete beta release end to end.
  - Signing secrets are not accessible to pull-request workflows.
  - Updates run the signed disk compatibility check against the archived previous release before promotion.

### M10-08 Licence notices, SBOM and source publication

- **Status:** blocked (authenticated supplier and supplemental Cargo notices pending)
- **Depends on:** M2-09, M10-01, Q-01
- **Docs:** [05 Licence obligations](../05-guest-image.md#licence-obligations)
- **Do:** Generate third-party notices for all Rust dependencies, bundled upstream binaries, fonts and guest contents; ship them in the app and show them in About. Publish the SBOM, the kernel source reference and the kernel config with each release automatically.
- **Deliver:** notice generation in `xtask`, release artifacts.
- **Accept when:**
  - Every shipped component appears in the notices with its licence.
  - The release workflow fails if the SBOM or source references are missing.

Notice staging and the About action are implemented. The pinned Docker CLI
has no embedded linked Go module list. The supplier handoff must include a
trusted inventory for the exact binary and the notices for each linked
component. It must also supply upstream notice files omitted from the locked
Cargo packages before the full distribution can be prepared.

### M10-09 User documentation

- **Status:** blocked (independent Arabic review and qualified agent operation pending)
- **Depends on:** M7-14, M8-09
- **Docs:** [09 Known and intended differences](../09-cli-and-docker-compat.md#known-and-intended-differences)
- **Do:** Write the user guide in English and Arabic: install, first run, sharing folders, networking and VPNs, resource settings, disk cleanup, agents, updating, troubleshooting, differences from Docker Desktop, uninstall.
- **Deliver:** `docs/user/` in both languages.
- **Accept when:**
  - Every intended difference and its error message is documented.
  - The Arabic guide is reviewed by the Arabic reviewer.

English and Arabic drafts are available in `docs/user/`. The independent
Arabic review and the qualified agent-operate walkthrough remain pending.

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


### M10-11 Qualify maintained Rust update verifier candidates

- **Status:** done
- **Depends on:** M10-03, M0-07
- **Docs:** [12 Auto-update](../12-build-release-update.md#auto-update), [Update manifest](../reference/update-manifest.md)
- **Do:** Evaluate current released Rust TUF verification candidates against the unchanged workspace dependency policy in disposable checkouts. Record exact versions, configuration, and findings for M10-04.
- **Deliver:** reproducible qualification results in the update manifest reference.
- **Accept when:**
  - The candidate graph is evaluated for licences, advisories, and dependency bans using the normal workspace lockfile and strict checks.
  - The evaluation records which options remain eligible for release integration and why. The production graph and update trust policy are unchanged.

### M10-12 Stage verified release resources

- **Status:** done
- **Depends on:** M2-09, M4-03
- **Docs:** [12 App bundle layout](../12-build-release-update.md#app-bundle-layout), [11 Updates and supply chain](../11-security.md#updates-and-supply-chain)
- **Do:** Add an unsigned resource stage for M10-01. Accept a three-file guest handoff and a guest manifest SHA-256 supplied independently by the trusted builder. Check the manifest, staged guest bytes and pinned component metadata. Fetch upstream binaries through M4-03 and generate their SBOM input from verified supplier bytes.
- **Deliver:** `cargo xtask stage-release-resources` with a task-local output containing `Resources/guest`, `Resources/bin`, and `upstream-sbom-input.json`. M10-01 owns app assembly and M10-08 owns the combined notices and SBOM.
- **Accept when:**
  - A real pinned guest and the four official macOS ARM64 Docker artifacts stage in the expected structure.
  - Changed guest or supplier bytes, a manifest that mismatches the trusted build record, unexpected names, symlinks and special files are rejected.
  - A failure leaves no partial output at the requested path. Publication is atomic and never replaces an existing entry.

- **Verified:** An Apple silicon staging run used the pinned Linux 7.3.5, Alpine 3.24.4 and Docker Engine 29.10.0 guest from source commit `800ec032618fda0005bec8de722115e24d9ffae1`. All three guest files matched their inputs byte-for-byte. The four verified official downloads were Docker CLI 29.10.0, Compose 5.8.0, Buildx 0.39.1 and the macOS keychain helper 0.9.13. Their installed SHA-256 digests matched the generated SBOM input. Tests cover guest and supplier corruption, unexpected and nonregular entries, and an output created between preflight and publication.

### M10-13 Prepare the native Harbor app icon

- **Status:** done
- **Depends on:** M0-01
- **Docs:** [12 App bundle layout](../12-build-release-update.md#app-bundle-layout), [Marsa Brand](https://www.figma.com/design/iXC1vWPKBgY2B6kDQVxI9W/Marsa-Desktop?node-id=2-17384)
- **Do:** Export the authoritative full, small and tiny Harbor icon artwork. Build the native macOS bundle icon independently of the complete desktop package.
- **Deliver:** original vector exports, an `icns` bundle resource and reproducible source-to-resource instructions.
- **Accept when:**
  - The native icon contains the platform size and scale variants with the intended small and tiny cuts.
  - macOS reads the icon from a bundle's resource directory. The verified asset does not require the installed app, a signing identity, or a system-setting change.

### M10-14 Validate release signatures before notarization

- **Status:** done
- **Depends on:** M0-01
- **Docs:** [12 App bundle layout](../12-build-release-update.md#app-bundle-layout), [12 Signing and notarization](../12-build-release-update.md#signing-and-notarization)
- **Do:** Add a macOS release verification command for the documented arm64 VZ bundle. Require an explicit trusted Team ID, verify the app and every Mach-O with Apple tools, and check hardened runtime and the expected role entitlements.
- **Deliver:** A reusable `cargo xtask` verification command and isolated negative fixtures.
- **Accept when:**
  - The verifier checks the expected app ID and native roles, including Mach-O files under resources.
  - A valid hardened ad-hoc fixture fails the Developer ID requirement.
  - M10-02 retains the production signing, notarization, stapling, Gatekeeper, and clean-Mac acceptance.

### M10-15 Require secure timestamps in release signature verification

- **Status:** done
- **Depends on:** M10-14
- **Docs:** [12 Signing and notarization](../12-build-release-update.md#signing-and-notarization)
- **Do:** Extend the offline release signature verifier to require Apple's certified signing timestamp for every native executable. The check is independent of signing and notarizing Marsa.
- **Deliver:** Secure timestamp verification in `cargo xtask verify-signature`.
- **Accept when:**
  - The verifier reads the typed timestamp through Security.framework after strict Developer ID verification and preserves the current entitlement and bundle checks.
  - A hardened ad-hoc Mach-O without a secure timestamp fails the timestamp policy. A Developer ID-signed reference executable with a secure timestamp passes the isolated timestamp check.
- **Evidence:** The native hardened ad-hoc fixture failed the new timestamp gate while preserving its existing structural checks. The same helper accepted a strictly verified Developer ID-signed reference executable on macOS 27.2.1. Workspace formatting, Clippy, and dependency policy passed.
