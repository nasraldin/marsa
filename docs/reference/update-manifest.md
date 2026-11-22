# Update manifest

The macOS app embeds a trusted public root for The Update Framework (TUF) in its signed resources. One repository serves both channels. Its signed `targets.json` authorizes each immutable archive by exact name, byte length and SHA-256 digest. The target's signed `custom` object holds the release metadata. Root, timestamp, snapshot and targets metadata follow TUF's canonical serialization and verification rules. [TUF specification](https://theupdateframework.github.io/specification/v1.0.33/).

## Release metadata

A target named `releases/marsa-<version>-darwin-arm64.zip` contains one signed, notarized `Marsa.app`. An existing target name always refers to the same bytes. Stable promotion authorizes the existing beta archive for stable in the signed target metadata.

```json
{
  "schema_version": 1,
  "version": "1.4.0",
  "minimum_macos": "27.0.0",
  "channels": ["beta", "stable"],
  "security_recommended": false,
  "notes": {
    "en-US": "Improved VM recovery after macOS wakes.",
    "ar": "تحسين استعادة الجهاز الافتراضي بعد تنشيط macOS."
  }
}
```

| Field | Rule |
|---|---|
| `schema_version` | Version 1. Reject unknown fields or invalid shapes. |
| `version` | Three decimal unsigned 32-bit components, without leading zeroes. Compare numerically. |
| `minimum_macos` | Same version syntax and at least 27.0.0. |
| `channels` | One or both of `stable` and `beta`, with unique entries. Select only a target with the configured channel. |
| `security_recommended` | Highlight the recommendation. Action approval and workload handling still apply. |
| `notes` | Nonempty plain text in both locales, at most 8 KiB each. Render without executing markup or fetching remote resources. |

Enforce TUF expiry and previous metadata versions with a private, persistent datastore. Use bounded HTTPS requests to the configured repository origin and approved artifact hosts. Verify TLS after every redirect. Set finite metadata, download and extracted-size limits. Authenticated target names identify archives. Release content cannot change the repository trust or destination policy.

Automatic updates must advance the installed app version, respect the macOS minimum, and stay above the highest successfully installed version. A deliberate downgrade requires explicit user confirmation, a compatible disk schema and a currently authenticated eligible target. Automatic rollback may restore only the previous verified bundle saved for the failed installation. Record both app identities in the local transaction journal. Preserve trusted TUF metadata versions across rollback and downgrade.

Stream the archive through the TUF verifier before staging. Confine extraction to private staging, reject duplicate or special entries, and validate symlinks. Verify the complete code signature, expected Developer ID designated requirement, bundle version, architecture and notarization before replacing the app. If the current user cannot replace the installation safely, instruct the user to update manually. [Apple code signing requirements](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements).

## Keys and expiry

| Role | Policy |
|---|---|
| Root | Two of three independently held Ed25519 keys, offline or hardware-backed. Embed the current public metadata as the trust anchor. Rotate or renew within one year. |
| Targets | Two of three independent offline or hardware-backed Ed25519 keys. Sign the Apple-verified archive's hash, length, channel and release notes. Renew before the 90-day expiry. |
| Snapshot | Dedicated online Ed25519 key. Include metadata versions, exact lengths and SHA-256 hashes. Renew before the seven-day expiry. |
| Timestamp | Separate online Ed25519 key. Include the current snapshot version, length and SHA-256 hash. Renew before the 24-hour expiry. |

Designate the project custodians under Q-04. Keep root and targets private keys out of CI, Git and the app. The release workflow can publish approved signed metadata and call separately scoped online services for snapshot and timestamp. Apple Developer ID signing has its own custody.

## Rotation and compromise

1. For planned rotation, generate replacement keys in the assigned custody. Publish the next root version signed by both the old and new thresholds, then publish replacement role metadata. Keep a consecutive chain of roots available for old clients. Verify the oldest supported client can reach the current root one version at a time.
2. Publish the archives and versioned targets and snapshot metadata first, then update timestamp. Rehearse root rotation, expiry and interrupted publication in a separate beta test repository before production changes.
3. On suspected exposure, stop publication and isolate the signer. Use the trusted root threshold to revoke the affected key and publish freshly signed metadata with advanced versions. Reject expired, replayed or untrusted metadata. Verify the Apple signature of each retained archive and notify users of affected releases.
4. If the old root threshold is compromised or unavailable, stop serving the affected update repository and re-establish trust out of band with a newly notarized app through a verified release channel. Publish manual recovery instructions. Revoke a compromised Apple identity through Apple's procedures and re-establish that trust separately.

M10-04 integrates an audited Rust TUF verifier. The current candidate is `tough` 0.24.0. Its published API covers bounded metadata loading, expiry, persistent rollback detection and replaceable transport. Check and pin the reviewed release when integrating it. [Verifier release](https://github.com/awslabs/tough/releases/tag/tough-v0.24.0), [loader API](https://docs.rs/tough/0.24.0/tough/struct.RepositoryLoader.html).
