---
name: MCP Server Setup
description: How to configure MCP servers in .rkat/mcp.toml
---

# MCP Server Setup

Use MCP when an external server should provide tools to Meerkat. MCP server
configuration controls tool discovery and transport; it does not move semantic
ownership away from the server or Meerkat runtime.

## Operating Rules

- Use `rkat mcp add` to register servers in project or user config. Project is
  the default scope; `--scope local` is an alias for project.
- Use stdio for local command-backed servers, streamable HTTP for modern
  network servers, and SSE only for legacy servers that require it.
- Use `rkat mcp list` and `rkat mcp get` to inspect configured servers.
- Use `rkat mcp login <name>` for OAuth-protected HTTP servers. Do not paste
  bearer tokens into prompts or command arguments.
- Project configuration wins when the same server name exists in both scopes.
  Specify `--scope` when removing an ambiguous name.
- Start or resume a session to load configured MCP tools into the agent.
- For live session mutation, use JSON-RPC `mcp/add`, `mcp/remove`, and
  `mcp/reload`, or the REST `/sessions/{id}/mcp/*` routes. Mutations are staged
  and take effect at a turn boundary; editing config does not mutate an active
  session mid-turn.
- Keep durable work state in WorkGraph and time-based wakeups in Schedule. MCP
  tools are actuators or external integrations.

## Examples

```bash
rkat mcp add files -- npx -y @modelcontextprotocol/server-filesystem .
rkat mcp add issue-api --url https://mcp.example.com
rkat mcp login issue-api
rkat mcp list
```

## Configuration Format

```toml
[[servers]]
name = "local-tools"
command = "path/to/server"
args = ["--flag", "value"]
env = { KEY = "value" }

[[servers]]
name = "issue-api"
url = "https://mcp.example.com"
connect_timeout_secs = 10
```

Project config lives at `<context-root>/.rkat/mcp.toml`; user config lives at
`~/.rkat/mcp.toml`. Prefer the CLI because it preserves unrelated TOML content
and coordinates concurrent mutations.
