# Parser fuzzing

The scheduled workflow runs these coverage-guided targets for 30 seconds each on a hosted Linux runner:

| Target | Boundary | Corpus format |
|---|---|---|
| `guest_control` | Guest and host protobuf decoders and frame length validation | Four big-endian length bytes followed by a protobuf payload |
| `settings` | TOML loading, defaulting, migrations and validation | Raw TOML |
| `update_manifest` | Marsa metadata in an authenticated TUF target | Target name, a newline and the JSON custom fields |

The run uses the Rust version pinned by the workspace, `cargo-fuzz` 0.13.2 and `libfuzzer-sys` 0.4.13. On stable Rust, `--sanitizer none` keeps coverage-guided mutation and Rust assertions active. It omits sanitizer checks. The update target exercises Marsa's metadata validation. TUF signature and integrity checks belong to the updater.

Install protoc 36.2 and a C++ compiler, then run at the repository root.

```sh
cargo install cargo-fuzz --version 0.13.2 --locked
bash fuzz/run.sh "$(mktemp -d)"
```

Each run copies the committed seeds into its own directory and records failures in the corresponding `crashes/` directory. On CI, download the `parser-fuzz-crashes` artifact if a run fails. Replay a single input with the compiler and target used by the scheduled run.

```sh
cargo fuzz run --sanitizer none <target> <input>
```

After fixing a failure, check the fix against the input and commit the smallest regression input under `fuzz/corpus/<target>/`. Keep corpus files synthetic and free of user data. The fuzz harness has its own lockfile and targets, so the libFuzzer dependency is excluded from application builds.
