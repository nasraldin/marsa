# 09 CLI and Docker compatibility

Two command-line surfaces exist. The upstream `docker` CLI does all container work, unchanged. The `marsa` CLI manages Marsa itself.

## Compatibility goal

Anything that works against Docker Desktop's engine works against Marsa without changes, as long as it uses linux/arm64 images. Planning named this as a make-or-break item: if IDEs, Testcontainers and Dev Containers do not work on day one, people uninstall.

## How clients find the engine

```mermaid
flowchart TB
    subgraph clients["Clients"]
        dcli["docker CLI"]
        sdk["SDKs and tools<br/>Testcontainers, IDEs, Dev Containers"]
        legacy["Tools hard-coded to<br/>/var/run/docker.sock"]
    end

    ctx["Docker context 'marsa'<br/>(registered on request)"]
    env["DOCKER_HOST<br/>(optional, via marsa env)"]
    link["/var/run/docker.sock symlink<br/>(opt-in, needs admin once)"]
    sock[("~/.marsa/run/docker.sock")]
    d["marsad"]

    dcli --> ctx --> sock
    sdk --> ctx
    sdk --> env --> sock
    legacy --> link --> sock
    sock --> d
```

| Mechanism | Default | Notes |
|---|---|---|
| Docker context `marsa` | Registered on request | Use `docker --context marsa` for one command or explicitly run `docker context use marsa` to change your current context |
| `DOCKER_HOST` | Set by the caller | Use `unix://$HOME/.marsa/run/docker.sock` with clients that take an endpoint. The host API supplies the exact value for `marsa env` |
| `/var/run/docker.sock` | Off | The optional administrator action is tracked in M4-04. Q-08 governs where it is offered |

### Client discovery

Docker CLI 29.10.0 accepts `--context marsa` and `DOCKER_CONTEXT=marsa`. In Docker CLI, `DOCKER_CONTEXT` takes precedence over the `DOCKER_HOST` environment variable and the saved current context. Use one selector in a given tool environment. [Docker CLI reference](https://docs.docker.com/reference/cli/docker/).

| Client checked | How to target Marsa |
|---|---|
| Docker CLI and Compose plugin | Run `docker --context marsa …` or `docker --context marsa compose …`. [Docker CLI reference](https://docs.docker.com/reference/cli/docker/), [Compose reference](https://docs.docker.com/reference/cli/docker/compose/) |
| Buildx | The default builder follows the Docker context. Explicitly selected builders can use another backend. [Docker builders](https://docs.docker.com/build/builders/) |
| VS Code Dev Containers | Set `containers.environment.DOCKER_CONTEXT` to `marsa` with the Container Tools extension. Dev Containers uses the current context without Container Tools. [VS Code Dev Containers](https://code.visualstudio.com/remote/advancedcontainers/develop-remote-host#_connect-using-docker-contexts) |
| JetBrains Docker integration | Set the Engine API URL to the Marsa `unix://` endpoint in the Docker connection settings. [JetBrains Docker connection settings](https://www.jetbrains.com/help/idea/settings-docker.html) |
| Testcontainers for Java 2.0.5 | Set `DOCKER_HOST` to the Marsa endpoint. The environment strategy reads the host directly. [Java provider source](https://github.com/testcontainers/testcontainers-java/blob/2.0.5/core/src/main/java/org/testcontainers/dockerclient/EnvironmentAndSystemPropertyClientProviderStrategy.java) |
| Testcontainers for Node.js 12.2.2 | Set `DOCKER_HOST` to the Marsa endpoint. The runtime configuration reads the host directly. [Node.js configuration source](https://github.com/testcontainers/testcontainers-node/blob/v12.2.2/packages/testcontainers/src/container-runtime/strategies/utils/config.ts) |
| Testcontainers for Go 0.45.0 | Set `DOCKER_HOST` to the Marsa endpoint. Its context-based lookup uses a Go context value. [Go host discovery source](https://github.com/testcontainers/testcontainers-go/blob/v0.45.0/internal/core/docker_host.go) |
| Testcontainers for Python 4.15.0 | Select the Marsa context or set `DOCKER_HOST` explicitly. The library checks `DOCKER_HOST` before looking up the Docker CLI context. [Python host discovery source](https://github.com/testcontainers/testcontainers-python/blob/testcontainers-v4.15.0/src/testcontainers/core/docker_client.py#L483-L508) |

For a tool that only reads the Docker host, set it in that tool's environment.

```sh
export DOCKER_HOST="unix://$HOME/.marsa/run/docker.sock"
```

The daemon's authenticated `DockerDiscovery.GetEndpoint` call returns the exact socket path, `DOCKER_HOST` value, and context name. `EnsureContext` runs the verified bundled CLI against a private staging directory, then installs only its generated `marsa` metadata. The complete context directory is published in one exclusive rename. It uses the absolute `DOCKER_CONFIG` configured for the daemon or the daemon's `$HOME/.docker`. It preserves existing selection and stops if the name is occupied by another configuration. [Pinned metadata layout](https://github.com/docker/cli/blob/v29.10.0/cli/context/store/metadatastore.go).

## Bundled upstream binaries

| Binary | Source | Location |
|---|---|---|
| `docker` | Docker CLI, darwin/arm64 | `Marsa.app/Contents/Resources/bin` |
| `docker-compose` | Compose CLI plugin | `Marsa.app/Contents/Resources/bin/cli-plugins` |
| `docker-buildx` | Buildx plugin | Same `cli-plugins` directory |
| `docker-credential-osxkeychain` | docker-credential-helpers | Same `bin` directory |

M4-03 verified the stable macOS ARM64 assets current on 2026-11-22. The Docker CLI
archive contains `bin/docker`. Compose, Buildx and the Keychain helper are
standalone Mach-O executables. The [version pins](../packaging/upstream-versions.toml)
record the SHA-256 published for each download.

| Binary | Verified release | Published checksum |
|---|---|---|
| Docker CLI | 29.10.0 | [Docker static download](https://download.docker.com/mac/static/stable/aarch64/docker-cli_29.10.0.tgz.sha256) |
| Compose | 5.8.0 | [Compose release checksum](https://github.com/docker/compose/releases/download/v5.8.0/docker-compose-darwin-aarch64.sha256) |
| Buildx | 0.39.1 | [Buildx macOS checksum list](https://github.com/docker/buildx/releases/download/v0.39.1/checksums-signed.txt) |
| Keychain helper | 0.9.13 | [Keychain helper checksum list](https://github.com/docker/docker-credential-helpers/releases/download/v0.9.13/checksums.txt) |

From the repository root, run `cargo xtask fetch-upstream` with `curl` and
`tar` available. Use the printed staging directory under `target/upstream/`.
Copy its `bin/` tree to the app’s `Contents/Resources/bin/`. Use
`upstream-sbom-input.json` as input for the release SBOM. The component hashes
describe the staged binaries before app signing. The distribution hashes
describe the original upstream downloads, including the Docker CLI archive.
The build publishes the staging directory only after all downloads match
their pins. A different pin file produces a different staging path.

For native version checks on an Apple silicon Mac, set `staged` to the printed
directory and point the Docker CLI at the staged plugins for the test:

```sh
config="$(mktemp -d)"
printf '{"cliPluginsExtraDirs":["%s/bin/cli-plugins"]}\n' "$staged" > "$config/config.json"
"$staged/bin/docker" --version
"$staged/bin/docker" --config "$config" compose version
"$staged/bin/docker" --config "$config" buildx version
"$staged/bin/docker-credential-osxkeychain" version
rm "$config/config.json"
rmdir "$config"
```

To verify context registration with the staged CLI, run the signed fixture. It uses a temporary home and Docker configuration.

```sh
MARSA_TEST_DOCKER_CLI="$staged/bin/docker" cargo test --locked -p marsad \
  --test lifecycle pinned_cli_sees_the_context_and_retains_user_selection -- --ignored
```

The staged executables retain their upstream bytes until app signing.
Docker CLI 29.10.0 discovers plugins through its Docker config directory or
`cliPluginsExtraDirs`, as shown in its [plugin manager source](https://github.com/docker/cli/blob/v29.10.0/cli-plugins/manager/manager.go#L35-L56).

- Marsa offers to link them into a directory on `PATH` (**Proposed:** `~/.marsa/bin`, added to the shell profile on request, or `/usr/local/bin` with an administrator prompt).
- If the user already has a `docker` CLI, Marsa does not replace it; it only adds the context.

## Coexistence with Docker Desktop and others

- Marsa never edits another tool's files or socket.
- The `marsa` context sits alongside `desktop-linux`, `orbstack`, `colima` and others; the user switches with `docker context use`.
- **Import (should-have):** copy images and volumes from another running engine through the Docker API (`save` and `load`, volume copy through a helper container).

## Compatibility checklist

Each line is an automated test in the conformance suite ([13](13-testing-and-benchmarks.md)).

| Area | Must work |
|---|---|
| Core | `run`, `exec`, `attach`, `logs -f`, `cp`, `stats`, `events`, `inspect` |
| Build | `docker build`, `docker buildx build`, cache mounts, secrets, SSH forwarding |
| Compose | `up`, `down`, `logs`, `watch`, profiles, healthchecks, depends_on |
| Networking | Published TCP and UDP ports on `localhost`, `host.docker.internal`, user-defined networks |
| Storage | Bind mounts of shared folders, named volumes, tmpfs |
| Tools | Testcontainers (Java, Node, Go, Python), VS Code Dev Containers, JetBrains Docker integration |
| Auth | `docker login` with credentials stored in the macOS keychain |
| Streams | Interactive TTY, terminal resize, signals |

### Known and intended differences

| Difference | Reason |
|---|---|
| `--platform linux/amd64` fails with a clear message | x86 images are out of scope |
| Bind mounts outside the shared-folder allowlist are refused with instructions | Security decision |
| Ports published on `0.0.0.0` bind to loopback unless LAN access is enabled | Security decision (proposed default) |
| Multi-arch builds produce arm64 only unless a remote builder is used | No emulation |

Each difference is documented for users and returned as an explicit error, never a silent failure.

## SSH agent forwarding

For Docker builds, pass `docker build --ssh default` with `SSH_AUTH_SOCK` pointing to the intended host agent. A Dockerfile step requests access using `RUN --mount=type=ssh,required=true`. Buildx forwards agent requests through that build's session and BuildKit supplies the step's socket. The private key stays with the host agent. [Build secrets documentation](https://docs.docker.com/build/building/secrets/#ssh-mounts) · [Pinned BuildKit provider](https://github.com/moby/buildkit/blob/v0.35.0/session/sshforward/sshprovider/agentprovider.go).

M4-17 tracks opt-in forwarding for bind-mounted agent sockets in regular containers. It requires a separate authorization and transport design. See [SSH agent access](11-security.md#ssh-agent-access).

Qualify build SSH forwarding on Apple silicon with the [signed VM test fixture](03-host-daemon.md#docker-socket-proxy). The test downloads checksum-pinned Alpine packages on the Mac, installs them without guest egress, and serves a disposable Git repository on the VM's loopback. Only the test public key is sent to that server. A separate macOS agent holds the fixture private key.

```sh
MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p marsad \
  --test packaged_runtime packaged_buildkit_ssh_session -- --ignored --nocapture
```

## The `marsa` CLI

A thin client of the typed API. It never talks to the VM directly.

```text
marsa status                    Engine and VM state, versions, resource use
marsa start | stop | restart    Control the engine VM
marsa env                       Print DOCKER_HOST export for the current shell
marsa config get|set|list       Read and change settings
marsa share list|add|remove     Manage the shared-folder allowlist
marsa disk usage                Show data-disk usage
marsa disk cleanup [--dry-run]  Preview and run cleanup, then reclaim space
marsa disk reclaim              Return freed space to macOS
marsa network status|mode       Show or pin the network mode
marsa logs [--follow]           Daemon and guest logs
marsa diagnose                  Collect a diagnostics bundle
marsa update check|apply        Updates
marsa mcp                       Run the MCP server on stdio
marsa agents list|approve|deny|revoke   Agent permissions
marsa reset [--factory]         Reset settings or wipe the data disk (typed confirmation)
marsa version                   Versions of every component
marsa completion <shell>        Shell completions
```

CLI rules:

- Every command supports `--json` for scripts and agents, with a stable schema.
- [Exit codes and JSON response shapes](reference/cli-exit-codes.md) are stable.
- Human output is localised (English and Arabic); `--json` output is not.
- Destructive commands require an explicit flag or a typed confirmation, and never run destructively when stdin is not a terminal unless `--yes` is given.
- `marsa` starts `marsad` if it is not running.

Run `eval "$(marsa env)"` in a POSIX-compatible shell to export the per-user
Docker socket for the current shell. `marsa env --json` returns the same
address in the `docker_host` field. An existing `DOCKER_CONTEXT` setting
continues to take precedence in the Docker client.

`marsa start` shows progress on standard error and returns when the engine
is ready. `marsa status --json` shows engine state, the installed component
versions and the latest resource observation from the guest. `marsa logs`
shows a bounded tail of classified daemon, guest console and engine records.
Use `--follow` to continue receiving new records.

`marsa diagnose` creates a private local archive and prints its path. It
works with the engine stopped. Review the archive before sharing it.

At a terminal, `marsa reset` asks for `RESET SETTINGS` and
`marsa reset --factory` asks for `FACTORY RESET`. Use `--yes` for an
explicit non-interactive confirmation. [Factory reset](plan/M9-hardening.md#m9-18-recover-private-storage-with-confirmed-factory-reset) stops the guest, restores the default settings, clears grants and creates a fresh private data disk.
