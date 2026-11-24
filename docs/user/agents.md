# Set up an MCP client for Marsa

[العربية](ar/agents.md) · [User guide](README.md)

Configure a trusted local MCP host to launch Marsa over standard input and output. Choose a client identifier and an absolute path for the project. For hosts that use a JSON MCP server configuration, place the entry below in the host’s documented server list.

```json
{
  "command": "/Applications/Marsa.app/Contents/MacOS/marsa",
  "args": ["mcp", "--client-id", "local-dev", "--project", "/Users/you/projects/example"]
}
```

Replace the project path with yours. The daemon issues the MCP bridge a restricted capability at startup. Keep the Docker socket and the owner’s daemon connection out of the agent’s configuration.

1. Start Marsa and connect the MCP client. Ask it to read the engine status or list running containers.
2. Use **Activity** or `marsa agents list` to see client registrations, active grants and pending requests.
3. If a supported operation asks for approval, review the client, project, operation and affected resources. Choose **Approve once** or **Deny**. A project grant is available only for eligible operations. Destructive and host-affecting actions require a separate approval when the build supports them.
4. Revoke a grant under **Activity** or run `marsa agents revoke GRANT` and confirm the exact grant. A pending approval can expire while you review it.

Read operations are available in the current implementation. The production operate provider requires exact workload identity checks before it can safely execute approved writes. If a write is refused, complete the operation yourself in Marsa or Docker. An approval cannot override protected folders or the engine’s authorization policy. [Permission tiers and approval flow](../10-mcp-server.md#permission-tiers)
