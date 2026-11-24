# Release notices and source references

The distribution pipeline requires three matching inputs.

- The M10-12 release resources directory containing the verified guest and macOS Docker programs.
- The private output of `cargo xtask guest all` for the same guest manifest, including `guest-sbom.cdx.json`, `guest-sources.json`, `guest-notices.tar`, and `kernel.config`.
- The exact pre-signing network-helper binary and a separately authenticated handoff covering that binary, the four upstream Docker programs, and upstream notices omitted from Cargo packages.

Run the staging command in the release checkout after restoring the locked Cargo source cache and the Go toolchain.

```sh
cargo xtask stage-release-notices \
  --version "$release_version" \
  --release-resources "$release_workspace/resources" \
  --guest-provenance "$guest_output" \
  --network-helper "$release_workspace/marsa-net-helper" \
  --tool-notices "$supplier_notice_handoff" \
  --tool-notices-sha256 "$trusted_supplier_manifest_sha256" \
  --output "$release_workspace/licenses"
```

Obtain the supplier manifest SHA-256 from the authenticated producer record. A digest computed only after receiving the handoff cannot establish its provenance. The supplier handoff contains `manifest.json` and original licence, notice, authorship and patent files under `texts/suppliers/`. The manifest records the version, SHA-256 of each unsigned native executable, and its linked Go component inventory.

```json
{
  "version": 1,
  "workspace_commit": "<full release commit>",
  "cargo_lock_sha256": "<SHA-256 of the release Cargo.lock>",
  "rust": {},
  "artifacts": {
    "docker": {
      "sha256": "<pinned binary SHA-256>",
      "components": {
        "docker": {
          "name": "Docker CLI",
          "module": "github.com/docker/cli",
          "version": "29.10.0",
          "license": "Apache-2.0",
          "source": "https://github.com/docker/cli/tree/v29.10.0",
          "notices": { "LICENSE": "<SHA-256 of the copied text>" }
        }
      }
    }
  }
}
```

The example illustrates one component record. Provide entries for `docker`, `docker-compose`, `docker-buildx`, `docker-credential-osxkeychain`, and `marsa-net-helper`. Each requires a primary component keyed by the artifact name and one component for every module reported by the exact binary’s Go build metadata, including the Go standard library as module `std`. If a supplier's Go build lists replacements, qualify their source and notice handling before publication. Put each referenced file at `texts/suppliers/<artifact>/<component-key>/<filename>`. For the network helper, use the [pinned vendor tree and toolchain evidence](../../network-helper/README.md#licence-and-advisory-evidence).

The pinned Docker CLI binary exposes its Go version, main package path, and
build flags. Its Go build metadata omits the linked module list. For that
artifact, provide `module_inventory` with the `name` and `sha256` of the
authenticated producer's record under `inventories/`. The record contains
`version` set to `1`, the exact `binary_sha256`, and `modules` mapping each
module path to its version. Include `std` with the embedded Go version.

For each locked Cargo package whose published or vendored sources omit the
notice files, add a `rust` entry keyed by `name@version`. Include the package's
declared `license`, the exact upstream HTTPS `source`, and `notices` mapping
original filenames to their SHA-256. Store those files at
`texts/rust/<name>/<version>/<filename>`. The handoff applies to the exact
release commit and lockfile. Use upstream notice bytes from the identified
revision. The package's SPDX metadata alone is insufficient.

Notices staging walks the locked Cargo graphs for the two release targets, preserves original notice bytes, verifies font checksums, binds the guest SBOM and kernel configuration to the packaged guest, and checks the supplier manifest against the bytes of each host program. Missing or mismatched files fail the staging command.

Pass the generated directory to bundling with `--licenses-dir` for a distribution build. Keep and publish the following files alongside the signed release.

- `marsa-sbom.cdx.json`
- `guest-sbom.cdx.json`
- `guest-sources.json`
- `guest-notices.tar`
- `kernel.config`
- `THIRD-PARTY-NOTICES.txt` and the accompanying notice texts

The release job must fail before signing or publication if any of this material is missing. The current checkout has no complete supplier notice handoff or automated publication job. A local launch build can be assembled independently while that evidence is prepared.
