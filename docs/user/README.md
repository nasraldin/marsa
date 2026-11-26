# Marsa user guide

[العربية](ar/README.md) · [Agent setup](agents.md)

## Install and start

Marsa requires an Apple silicon Mac with macOS 27 or later. Download the signed release DMG through your browser, open it, and drag Marsa into Applications. Open Marsa in Finder. Follow the standard first-open confirmation if macOS displays it. If macOS blocks the app, check that the download came from the project’s release page and obtain a fresh copy.

Open **Machine** and start the engine. Wait for **Ready** before running a container. You can change the language and appearance under **Settings → General**. The desktop app follows the macOS appearance when the theme is set to **System**.

## Use Docker

Under **Settings → Docker**, register the `marsa` context. This adds a Docker context to your user configuration and preserves your current selection. Use the context explicitly when working with more than one engine.

```sh
docker --context marsa run --rm hello-world
```

If you have no Docker command on your PATH, use the bundled client.

```sh
/Applications/Marsa.app/Contents/Resources/bin/docker --context marsa run --rm hello-world
```

On a fresh Docker profile, the bundled command uses Marsa’s socket when the default Docker socket is absent. Run your usual `docker build` and `docker run` with this command. It prefers arm64 when available and selects amd64 for an amd64-only registry index. Automatic build selection examines local Dockerfiles. For a Dockerfile passed on standard input, an archive or a remote build context that needs amd64, use `docker build --platform linux/amd64`. A saved Docker context or an explicit host selects the engine you configured.

The corresponding Marsa command is at `/Applications/Marsa.app/Contents/MacOS/marsa`. Run it as `marsa` in the examples below if you have already put it on your PATH. Run `marsa env` to see the shell export for Marsa’s private Docker socket. If your shell sets `DOCKER_CONTEXT`, it takes precedence over `DOCKER_HOST`.

The desktop shows resources from Marsa, detected local providers, and configured Docker contexts. Use **Filter by context** to show one engine, or choose **All contexts** for a combined inventory. The source badge identifies each row’s engine. Click the context card in the sidebar to open **Settings → Docker** and scan or choose an engine.

External connections support user-owned Unix sockets, SSH with a known host, and mutually authenticated TLS. Configure remote credentials and host trust using Docker and OpenSSH. Contexts using plain TCP or skipping certificate verification appear unavailable. Marsa does not change the Docker CLI’s active context.

Open a container to inspect it, view logs, use its terminal, or see live stats. The row menu has lifecycle actions and confirmed removal. **Group actions** expands or collapses the visible groups.

Select an image for inspect, run, tag, push, export, or removal. Use **Pull image** or **Import image** and choose the destination engine. Exports use the selected immutable image ID. Apply a tag after importing if the image archive has no tag. Push supports anonymous registries, matching Docker auth-file entries, and the bundled macOS Keychain credential helper. Configure other credential helpers through the Docker CLI.

Volumes and networks support creation and confirmed removal. A network can attach or detach a container identified by name or full ID on the same engine. **Stacks** deploys from a local Compose file you select and supports logs and lifecycle controls for deployed projects.

The **Containers**, **Images**, **Volumes**, and **Networks** screens show resources from the connected engine. Docker Compose projects appear when Docker has containers with Compose labels. When the app asks you to choose a Compose file for an operation, select the file that belongs to the project. **Down** keeps named volumes by default. Removing the volumes requires typing the project name.

## Sharing folders

Add a project folder under **Settings → Sharing** and choose read-only or read and write. When a workload requests another host folder, review the exact paths. Choose **Share once**, **Always share**, or **Deny**. Some VM backends can attach a newly approved static share only at the next start. Save work before choosing to restart the engine.

Marsa protects credential directories such as `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.kube`, `~/.docker`, and its own private data directory. Sharing a parent folder does not make its protected children accessible. If Docker reports that a host path is outside the shared folders, share the intended project folder or review the pending request with `marsa share approvals`.

## Networking and VPNs

In **Settings → Network**, **Automatic** prefers native NAT and can use user-space networking when it cannot reach the network through NAT. Check the active mode in Settings or run `marsa network status`. A saved mode applies the next time the engine starts. Keep your usual VPN and firewall configuration when diagnosing a problem.

If a name fails to resolve, check the Mac’s own connection and DNS settings first. You can save DNS addresses and credential-free HTTP or HTTPS proxy endpoints in Network settings. Review the impact before applying proxy changes. Applying changes can restart the engine and interrupt running workloads.

Use `localhost` on the Mac for forwarded ports when the release supports forwarding. A port bind can fail because the port is already occupied or because macOS denied the bind. Use an unused, unprivileged host port, such as `8080`. A wildcard publish does not guarantee access from another machine. LAN forwarding depends on the release’s available network controls.

## Resources and disk cleanup

Under **Machine** or **Settings → Resources**, change the CPU count, memory and disk limit. Changes that require a restart apply on the next engine start. Stop the engine only after saving work in any running containers.

Under **Disk & cleanup**, review categories and preview cleanup candidates. The current build leaves bulk deletion unavailable while exact deletion is being qualified. Use **Reclaim** to return already-free blocks to macOS. The observed host allocation can differ from the preview estimate.

## Differences from Docker Desktop

| What you may see | What to do |
| --- | --- |
| An image pull reports that it has no Linux ARM64 variant, or Docker reports `no matching manifest for linux/arm64`. | Use Marsa’s bundled `docker` command for automatic selection of an amd64-only registry index on the Marsa engine. With a separate stock Docker CLI, select `--platform linux/amd64` for that index. |
| A bind mount is refused because the host path is outside the shared folders. | Review the request under **Settings → Sharing**. Share only the required project folder. Protected paths stay excluded. |
| A published port is reachable from the Mac but cannot be reached from another computer. | Check the forwarding and LAN options available in your release. Use `localhost` for local access. An unavailable LAN capability is not an engine error. |
| A local multi-platform build rejects an unsupported target or produces only ARM64 output. | Build `linux/arm64` locally. To build a different architecture, select a remote builder that supports the target. |

Docker commands and Marsa’s CLI output remain in English even when the desktop is in Arabic. [Known and intended differences](../09-cli-and-docker-compat.md#known-and-intended-differences)

## Updates and troubleshooting

In **Settings → Updates**, review the release channel and check for updates if the installed build offers that feature. If in-app updates are unavailable, obtain the next signed build from the project’s release page.

If the engine stops during startup, open **Machine** to see the failed stage and retry. Run `marsa status` for the current state and `marsa logs` for recent logs. `marsa diagnose` creates a private archive and prints its path. Inspect the archive before sharing it. If Docker is connecting to a different engine, check the active context and any `DOCKER_CONTEXT` or `DOCKER_HOST` settings.

## Uninstall

1. Back up any workload data you want to keep. If you installed the optional Docker socket link, remove it under **Settings → General** and approve the macOS prompt. Stop the engine and quit Marsa.
2. If `marsa` is your active Docker context, select the context you want to keep using with `docker context use`. If you used a custom `DOCKER_CONFIG` to register Marsa, use that same directory for the uninstall command.
3. While the app is still installed, run the command below. Substitute the app's installed path if needed. Type `UNINSTALL MARSA` to confirm permanent removal of the app's images, volumes, settings, grants, and logs.

   ```sh
   "/Applications/Marsa.app/Contents/MacOS/marsad" uninstall
   ```

4. Move the app to Trash. If you installed it with Homebrew, run `brew uninstall --cask marsa` to remove the app and the command link installed by the cask. Remove any shell PATH entries or links you added manually.

The command unregisters Marsa's login agent and removes the matching Marsa Docker context. It keeps other Docker contexts and configuration. The separate `marsa reset --factory` command resets an installed app and also deletes workload data.
