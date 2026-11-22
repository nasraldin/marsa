# M8 MCP server

**Outcome:** AI agents can inspect and operate containers through MCP, under permission tiers, with human approval for anything destructive and a full audit trail.

Design: [10 MCP server](../10-mcp-server.md), D-013.

Before starting, verify the current MCP specification version and the maintained Rust SDK options; pin what you use and note it in [10](../10-mcp-server.md).

---

### M8-01 Policy engine and grants

- **Status:** done
- **Depends on:** M3-08
- **Docs:** [10 Permission tiers](../10-mcp-server.md#permission-tiers), [10 Client identity and grants](../10-mcp-server.md#client-identity-and-grants)
- **Do:** Implement the policy engine in `marsad`: the tier of every operation, the decision function (allow, needs approval, refuse), the refusal of privilege flags for agents, grants per client and project with expiry, and the grants store.
- **Deliver:** policy module, `grants.json` handling, exhaustive tests.
- **Accept when:**
  - A table-driven test covers every tool against every grant state.
  - Destroy and host-affecting tiers can never be granted as "always" (enforced in code and tested).
  - Privileged, host-network, host-PID and Docker-socket mounts are refused for agents unless the advanced setting is on, and still need approval when it is.

### M8-02 `AgentOps` service and approval queue

- **Status:** done
- **Depends on:** M8-01, M4-02
- **Docs:** [03 Typed API](../03-host-daemon.md#typed-api), [10 Approval flow](../10-mcp-server.md#approval-flow)
- **Do:** Implement read-only engine status on the daemon-issued restricted agent session. Implement owner registration and decisions over the separate trusted local socket. Queue immutable approval documents, publish updates and expire unanswered requests. Refuse container mutations before queuing.
- **Deliver:** both services, owner-decision tests and fake runtime tests for scoped reads and unsupported writes.
- **Accept when:**
  - An internal immutable approval fixture waits for owner approval, denial or expiry. The queue resolves the same immutable request shown to the owner.
  - An agent session cannot approve its own requests or impersonate another client or project.
  - Even an exact-ID stop or a bind-capable mutation cannot reach a running Docker engine or create an approval.

M8-10 owns bind preparation and active host share policy checks. Operate tools require a provider that binds a mutation to the same container identity the owner reviewed. Moby's stop endpoint performs a fresh ID-or-name lookup, so a preceding inspect cannot supply that guarantee.

### M8-03 Audit log

- **Status:** todo
- **Depends on:** M8-02
- **Docs:** [10 Audit log](../10-mcp-server.md#audit-log)
- **Do:** Record every agent call: time, client, tool, arguments with secrets redacted, decision, approver, result. Store it append-only with rotation and expose it through the API with filters.
- **Deliver:** audit module and tests.
- **Accept when:**
  - Every `AgentOps` call produces exactly one audit record, including refused and timed-out calls.
  - Known secret patterns in arguments are redacted (tested).

### M8-04 MCP server and read tools

- **Status:** todo
- **Depends on:** M8-02, M5-01
- **Docs:** [10 Tools](../10-mcp-server.md#tools), [10 Architecture](../10-mcp-server.md#architecture)
- **Do:** Implement `marsa-mcp` on stdio using `marsa-client` only. Identify the client at connection. Implement all read-tier tools with JSON Schema arguments, bounded outputs with paging, and structured errors.
- **Deliver:** `crates/marsa-mcp` with protocol-level tests.
- **Accept when:**
  - A standard MCP client can list tools and call every read tool.
  - Log and stats outputs are capped and say when they are truncated.
  - The crate has no dependency on the Docker socket or VM crates.

### M8-05 Operate tools

- **Status:** todo
- **Depends on:** M8-04, M8-10
- **Docs:** [10 Tools](../10-mcp-server.md#tools)
- **Do:** Implement start, stop, restart, run, exec, pull and Compose up, down (without volumes) and restart, each passing through the approval flow per the tier rules, with the command or spec shown in the approval.
- **Deliver:** tool implementations and tests.
- **Accept when:**
  - The first operate call from a new client and project asks; after "always for this project" the same tier does not ask again for that project.
  - `exec` approvals show the exact command.

### M8-06 Destroy and host-affecting tools

- **Status:** todo
- **Depends on:** M8-05
- **Docs:** [10 Tools](../10-mcp-server.md#tools)
- **Do:** Implement remove container, image, network and volume; Compose down with volumes; prune; and add share. Every approval lists exactly what will be affected, with names and sizes.
- **Deliver:** tool implementations and tests.
- **Accept when:**
  - Each call asks every time, regardless of grants.
  - A volume removal approval names the volume, its size and the containers that used it.
  - A denied call changes nothing.

### M8-07 CLI commands for agents

- **Status:** todo
- **Depends on:** M8-04, M5-02
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Implement `marsa mcp` (run the server on stdio) and `marsa agents list|approve|deny|revoke`, so approvals work from a terminal with no GUI.
- **Deliver:** command implementations and tests.
- **Accept when:**
  - A pending approval can be answered from the CLI and the waiting agent call resumes.
  - `marsa agents list --json` shows clients, grants and pending requests.

### M8-08 Untrusted-output handling and adversarial tests

- **Status:** todo
- **Depends on:** M8-06
- **Docs:** [10 Treating container output as untrusted](../10-mcp-server.md#treating-container-output-as-untrusted), [13 Security tests](../13-testing-and-benchmarks.md#security-tests)
- **Do:** Ensure workload output is returned only in delimited data fields and never merged into descriptions. State in each tool description that returned content is untrusted. Add adversarial tests: logs, labels and names containing instruction-like text must not change any policy decision or tool metadata.
- **Deliver:** tests and any hardening they require.
- **Accept when:**
  - The adversarial suite passes.
  - No code path lets returned content reach the policy decision.

### M8-09 Agent setup guides

- **Status:** todo
- **Depends on:** M8-07
- **Docs:** [10 MCP server](../10-mcp-server.md)
- **Do:** Write user documentation for connecting common MCP clients to `marsa mcp`, explaining the permission tiers and how to review and revoke grants. English and Arabic.
- **Deliver:** `docs/user/agents.md` and its Arabic counterpart.
- **Accept when:**
  - Following the guide on a clean machine connects a client and completes one read call and one approved operate call.


### M8-10 Agent bind authorization and run preparation

- **Status:** blocked (stock Docker start lacks an exact container identity guarantee)
- **Depends on:** M8-02, M4-06
- **Docs:** [10 Permission tiers](../10-mcp-server.md#permission-tiers), [07 Shared folders](../07-file-sharing-and-storage.md#shared-folders), D-012, D-013
- **Do:** Validate typed container run mounts and privilege effects. Apply both agent policy and the current attached host share policy. Keep the normalized run request immutable across approval and revalidate share authorization before forwarding it to Docker.
- **Deliver:** run preparation and integration tests for both permission boundaries.
- **Accept when:**
  - A bind requested through AgentOps passes both the agent policy and the active shared-folder allowlist before Docker receives it.
  - Denied, unattached and revoked paths are refused even if the owner approved the agent operation.
  - A signed packaged test observes a permitted host bind, and a fake engine test proves it received no request for a rejected bind.

- **Blocker:** The pinned engine can prepare a create request using a local immutable image digest and an explicit platform. Containerd image indexes also require verifying the resolved platform manifest. Docker starts the resulting container in a separate operation. Its start lookup can fall back to a name or prefix after attempting the full ID, so the create response does not guarantee the start target. Host-side authorization also needs to bind the approved mount to the same live VM activation through execution. A signed running-bind acceptance requires both guarantees. Delegated writes remain refused while they are missing. [Pinned image resolver](https://github.com/moby/moby/blob/docker-v29.10.0/daemon/containerd/image.go#L132-L170) · [Pinned start handler](https://github.com/moby/moby/blob/docker-v29.10.0/daemon/start.go#L48-L75) · [Pinned container lookup](https://github.com/moby/moby/blob/docker-v29.10.0/daemon/container.go#L37-L66)
