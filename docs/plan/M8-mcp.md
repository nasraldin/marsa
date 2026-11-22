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

- **Status:** todo
- **Depends on:** M8-01, M4-02
- **Docs:** [03 Typed API](../03-host-daemon.md#typed-api), [10 Approval flow](../10-mcp-server.md#approval-flow)
- **Do:** Implement the `AgentOps` service that executes engine operations for a named client through the policy engine, and the `Agents` service with the approval queue: enqueue with full detail, notify through events, approve or deny, timeout, and resume the waiting call.
- **Deliver:** both services and integration tests using the fake backend and a fake engine.
- **Accept when:**
  - A call needing approval blocks until approved, denied or timed out, and returns a structured result in each case.
  - Approving a request executes exactly the operation that was shown, not a re-read of mutable arguments.
  - Bind mounts requested by an agent pass both the agent policy and the shared-folder allowlist.

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
- **Depends on:** M8-04
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
