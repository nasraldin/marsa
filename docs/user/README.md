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

The corresponding Marsa command is at `/Applications/Marsa.app/Contents/MacOS/marsa`. Run it as `marsa` in the examples below if you have already put it on your PATH. Run `marsa env` to see the shell export for Marsa’s private Docker socket. If your shell sets `DOCKER_CONTEXT`, it takes precedence over `DOCKER_HOST`.

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
| An image pull reports that it has no Linux ARM64 variant, or Docker reports `no matching manifest for linux/arm64`. | Select a tag that includes `linux/arm64`. Marsa runs ARM64 Linux workloads and does not emulate `linux/amd64`. |
| A bind mount is refused because the host path is outside the shared folders. | Review the request under **Settings → Sharing**. Share only the required project folder. Protected paths stay excluded. |
| A published port is reachable from the Mac but cannot be reached from another computer. | Check the forwarding and LAN options available in your release. Use `localhost` for local access. An unavailable LAN capability is not an engine error. |
| A local multi-platform build rejects an unsupported target or produces only ARM64 output. | Build `linux/arm64` locally. To build a different architecture, select a remote builder that supports the target. |

Docker commands and Marsa’s CLI output remain in English even when the desktop is in Arabic. [Known and intended differences](../09-cli-and-docker-compat.md#known-and-intended-differences)

## Updates and troubleshooting

In **Settings → Updates**, review the release channel and check for updates if the installed build offers that feature. If in-app updates are unavailable, obtain the next signed build from the project’s release page.

If the engine stops during startup, open **Machine** to see the failed stage and retry. Run `marsa status` for the current state and `marsa logs` for recent logs. `marsa diagnose` creates a private archive and prints its path. Inspect the archive before sharing it. If Docker is connecting to a different engine, check the active context and any `DOCKER_CONTEXT` or `DOCKER_HOST` settings.

## Uninstall

1. Back up workload data you want to keep. Stop the engine and turn off **Start at login** under **Settings → General**.
2. Quit Marsa and move `Marsa.app` from Applications to Trash.
3. Remove the `marsa` Docker context if you registered it. If it is your active context, switch to the context you want to use first.
4. In Finder, open `~/.marsa` through **Go → Go to Folder**. Move that folder to Trash only if you want to delete all of Marsa’s volumes, images, settings and local grants. Remove any shell PATH entries or links you added for Marsa.

The factory reset command also deletes workload data and requires typed confirmation. It resets a Marsa installation while the app is still installed.
