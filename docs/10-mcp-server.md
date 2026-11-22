# 10 MCP server

Marsa ships a Model Context Protocol server so AI coding agents can inspect and operate containers. **Decided:** it is in version 1.

Because the Docker socket is equivalent to root over the VM and every shared folder, the MCP server is designed around permissions first and tools second.

## Design goals

1. An agent can do the everyday read-only work (list, logs, inspect, stats) without interrupting the user.
2. Anything that changes or destroys state needs the user's approval, given in the GUI or CLI.
3. An agent can never do something the GUI could not show and the user could not review.
4. Everything an agent does is recorded.

## Protocol version

The local stdio server implements MCP `2026-07-28` and the `2025-11-25` compatibility handshake. M8-04 tested the official Rust SDK `rmcp` 3.11.0 with only the server and stdio features. Its required dependencies include `chrono` 0.4.45, which fails the workspace maintenance policy under RUSTSEC-2026-0543. Marsa uses its existing Tokio and Serde dependencies for this bounded stdio transport. [MCP specification](https://modelcontextprotocol.io/specification/2026-07-28), [Pinned SDK release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.11.0), [RustSec advisory](https://rustsec.org/advisories/RUSTSEC-2026-0543.html)

## Architecture

```mermaid
flowchart LR
    agent["AI agent / MCP client"]
    subgraph mcp["marsa-mcp"]
        tools["Tool handlers"]
        guard["Permission guard"]
    end
    subgraph d["marsad"]
        pol["Policy engine"]
        appr["Approval queue"]
        audit["Audit log"]
        api["Typed API and Docker proxy"]
    end
    gui["Marsa GUI or marsa CLI<br/>(user approves)"]
    eng["dockerd"]

    agent -->|stdio or local HTTP| tools
    tools --> guard
    guard --> pol
    pol -->|allowed| api
    pol -->|needs approval| appr
    appr --> gui
    gui -->|approve or deny| appr
    appr --> api
    api --> eng
    pol --> audit
    api --> audit
```

- Configure a trusted local MCP host to launch `marsa mcp --client-id ID --project /absolute/project`. The launch configuration supplies the identity and project. Protocol metadata can describe the peer but cannot change the registered identity. The CLI launches its sibling `marsa-mcp` binary. The bridge registers with the owner-only local daemon and uses a restricted `AgentOps` capability for every read.
- Standard output carries one JSON-RPC message per line. The bridge accepts a maximum incoming message size of 64 KiB and eight in-flight read calls. The daemon permits eight concurrent Docker reads across sessions.
- **Proposed:** an optional local HTTP transport bound to loopback with a per-client token, for clients that need it. Off by default.
- The MCP server holds no privileges of its own. Every call goes through `marsad`'s policy engine.

## Permission tiers

| Tier | Examples | Default |
|---|---|---|
| **Read** | List containers, images, volumes, networks; inspect; read logs; read stats; engine status; disk usage | Allowed without asking |
| **Operate** | Start, stop, restart a container; run a new container from a local image; `exec` a command; Compose up and down; pull an image | Ask the first time per client and per project; the user may grant "always for this project" |
| **Destroy** | Remove containers, images, volumes, networks; prune; factory reset | Always ask, every time, showing exactly what will be removed. Never grantable as "always" |
| **Host-affecting** | Add a shared folder; bind-mount a new host path; publish a port to the LAN; change settings | Always ask, every time |

Rules:

- **Volumes are precious.** Removing a volume always shows its name, size and which containers used it.
- **`exec` is powerful.** It runs arbitrary commands inside a container and is in the Operate tier with its command shown in the approval.
- **Bind mounts are checked twice:** by the agent policy and by the shared-folder allowlist.
- **No privilege flags by default.** `--privileged`, host network, host PID and mounting the Docker socket into a container are refused for agents unless the user enables an explicit "advanced agent access" setting, and even then each use asks.

## Approval flow

```mermaid
sequenceDiagram
    autonumber
    participant A as Agent
    participant M as marsa-mcp
    participant P as Policy engine
    participant Q as Approval queue
    participant U as User (GUI or CLI)
    participant E as dockerd

    A->>M: tool call: remove_volume("pgdata")
    M->>P: evaluate(client, tool, args)
    P-->>M: needs approval (Destroy tier)
    M->>Q: enqueue request with full detail
    Q->>U: Notification: agent X wants to remove volume pgdata (2.1 GB)
    alt user approves
        U->>Q: approve
        Q->>E: remove volume
        E-->>M: done
        M-->>A: result: removed
    else user denies or times out
        U->>Q: deny
        M-->>A: result: denied by user
    end
    Note over P,Q: Every step is written to the audit log
```

- Requests time out (default a few minutes) and return a clear "not approved" result so the agent can continue or ask the user in its own interface.
- If no GUI is open, the tray and CLI are planned owner clients for the same approval API.
- The owner-only daemon socket serves `Agents.OpenSession` and approval decisions. A trusted local owner client registers the agent and project, then delegates a 12-hour capability to the restricted `agent.sock` endpoint. The agent capability authenticates `AgentOps` and has no owner permissions. Do not give an agent the owner connection or raw Docker socket.
- This protects against an agent using only its delegated operations. Local processes running with the unrestricted owner's Unix access are inside the trusted host boundary. The daemon does not attest physical user presence. The owner GUI or CLI is responsible for collecting the user's decision. [Terminal approval](09-cli-and-docker-compat.md#agent-access-from-the-terminal) displays a queued request and accepts an explicit terminal response.
- M8-02 implements scoped engine status and the owner approval queue. Container mutations require an engine provider that can bind execution to the immutable target reviewed by the owner.
- The pinned Docker stop endpoint resolves the target again when applying the operation and may interpret a missing full ID as a name. A prior inspect cannot guarantee the stop targets the reviewed container. [Moby stop handler](https://github.com/moby/moby/blob/docker-v29.10.0/daemon/stop.go#L25-L39), [Moby target resolution](https://github.com/moby/moby/blob/docker-v29.10.0/daemon/container.go#L37-L53)
- M8-10 enables requests that create bind mounts after the agent and active host share checks are integrated. Until then, the daemon refuses container creation through `AgentOps`.

## Tools

**Proposed** first set. Names are stable; arguments use JSON Schema.

| Tool | Tier | Purpose |
|---|---|---|
| `engine_status` | Read | State, versions, resources |
| `list_containers`, `inspect_container` | Read | Containers and their details |
| `container_logs` | Read | Tail or range, with a size cap |
| `container_stats` | Read | One sample or a short window |
| `list_images`, `inspect_image` | Read | Images |
| `list_volumes`, `list_networks` | Read | Volumes and networks |
| `compose_list`, `compose_ps`, `compose_logs` | Read | Compose projects |
| `disk_usage`, `cleanup_preview` | Read | What could be reclaimed |
| `start_container`, `stop_container`, `restart_container` | Operate | Lifecycle |
| `run_container` | Operate | Create and start from a spec |
| `exec_in_container` | Operate | Run a command, return output with a size cap |
| `pull_image` | Operate | Pull by reference |
| `compose_up`, `compose_down`, `compose_restart` | Operate | Compose lifecycle (`down` without volumes) |
| `remove_container`, `remove_image`, `remove_network` | Destroy | Removal |
| `remove_volume`, `compose_down_volumes`, `prune` | Destroy | Data-destroying removal |
| `add_share` | Host-affecting | Request a new shared folder |

M8-04 advertises engine status, container and image list and inspect, bounded container logs and stats, and network and volume lists. Read tools observe the local Docker engine. Project registration scopes the client identity and any future grants. M8-11 adds Compose and disk read tools when their providers are available.

List tools return live pages of up to 50 items using an opaque cursor. Container and image inspection accepts full IDs obtained from list results. Logs accept a full container ID, a tail size up to 200 and an optional timestamp cursor for an earlier page. Logs and stats report truncation when a response exceeds the daemon's byte budget. Results mark workload data as untrusted and carry it in a separate data field.

Tool rules:

- Outputs are bounded (logs and exec output are truncated with a clear marker and a way to page).
- Errors are structured: a code, a message, and when relevant the reason for denial.
- Tool descriptions state the tier, so a well-behaved agent can predict approvals.

## Treating container output as untrusted

Logs, exec output, image labels and container names are data from the workload. They can contain text crafted to look like instructions. The server:

- Returns them as plain data in clearly delimited fields, never merged into tool descriptions or system text.
- Notes in each tool description that returned content is untrusted workload output.
- Never lets returned content influence the permission decision.

## Client identity and grants

- Each MCP client is identified at first connection (name reported by the client, plus the launching process where it can be determined) and shown to the user.
- Grants are stored per client and per project directory in `~/.marsa/state/grants.json`, visible and revocable in Settings and with `marsa agents`.
- An approval can grant one ordinary Operate action for the authenticated client and the project directory. The daemon binds it to the canonical path and filesystem identity. Reusing the path for a different directory requires a new grant.
- Grants expire after 30 days of disuse. An operation that actually uses a grant renews it. Factory reset clears the grants and publishes an empty grant list. Grant reads and revocation return an unavailable result while a factory reset is in progress or storage recovery is required.
- Factory reset leaves current read-only sessions subject to their original expiry and project identity checks. These sessions can query engine status. They carry no grant-based authority to mutate workloads.
- The daemon stores the versioned grant document with owner-only permissions and replaces it atomically. If the file is malformed or unsafe, the daemon treats it as unavailable for authorizing an operation.

## Audit log

The daemon records one terminal entry per decoded `AgentOps.Execute` call. A retry gets its own entry even when the request ID replays a previous result. Each entry records receipt time, the registered client's fingerprint when authenticated, the typed operation and a redacted argument summary, the decision, the local owner role when someone decided, and the result. Argument values, tokens, client and container IDs, paths, and workload output are kept out of the audit log. Unsupported operations record a refusal with the typed operation and omit all arguments.

`Agents.ListAudit` is available on the owner connection. It pages newest completed entries first and filters by client fingerprint, operation, decision, and receipt time. Four owner-only JSON Lines files retain up to 2 MiB inside `~/.marsa/state`. If rotation removes a cursor, the owner client starts a new query. Reviewed execution checks that the audit storage is accessible first, and the daemon syncs its terminal entry before returning the result. A caller that disconnects during normal operation can still have a completed entry. An abrupt process or storage failure can interrupt an in-flight entry.

The GUI Activity screen and explicit diagnostic export are separate clients of the private audit data.

## One API for everything

The MCP server, the GUI and the CLI all call the same `marsad` API through `marsa-client`. There is no agent-only code path into the engine, so the policy engine and audit log cannot be bypassed from the MCP side.
