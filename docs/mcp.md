# MCP

Model Context Protocol (MCP) is the tool protocol Meerkat uses in two ways:

1. As a **client**: Meerkat can call external MCP servers for tools.
2. As a **server**: Meerkat can expose itself as MCP tools.

This page covers both.

---

## Use MCP servers as tools (client)

### CLI configuration (recommended)

Use the `rkat mcp` commands to manage MCP servers:

```bash
# Add a stdio server
rkat mcp add filesystem -- npx -y @anthropic/mcp-server-filesystem /tmp

# Add a streamable HTTP server
rkat mcp add api --url https://mcp.example.com/api

# List servers
rkat mcp list

# Remove a server
rkat mcp remove filesystem
```

### Config file format

MCP servers are stored in TOML in two scopes:

- **Project**: `.rkat/mcp.toml`
- **User**: `~/.rkat/mcp.toml`

Project config wins on name collisions.

Example:

```toml
# Stdio server (local process)
[[servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/allowed/dir"]

# Stdio server with env
[[servers]]
name = "database"
command = "python"
args = ["-m", "db_mcp_server"]
env = { DATABASE_URL = "${DATABASE_URL}" }

# HTTP server (streamable HTTP is default for URLs)
[[servers]]
name = "remote-api"
url = "https://mcp.example.com/api"
headers = { Authorization = "Bearer ${MCP_API_TOKEN}" }

# Legacy SSE server
[[servers]]
name = "legacy-sse"
url = "https://old.example.com/sse"
transport = "sse"
```

Notes:
- Supported transports: **stdio**, **streamable HTTP** (default for URLs), and **SSE**.
- Environment variables in config use `${VAR_NAME}` syntax.

---

## Meerkat as an MCP server

The crate `meerkat-mcp-server` exposes Meerkat as MCP tools that other clients
can call. It currently provides these tools:

- `meerkat_run` — start a new session
- `meerkat_resume` — continue a session / provide tool results
- `meerkat_config` — get or update server config for this instance

### Using Meerkat MCP tools with other clients

You can connect any MCP-capable client to a Meerkat MCP server. The key is to
run a server process that exposes the `meerkat_*` tools (stdio or HTTP), then
point the client at it.

Below are practical setups for popular MCP clients. Replace the placeholder
command/URL with your Meerkat MCP server wrapper.

#### Claude Code

Claude Code reads MCP servers from a `.mcp.json` file in your project root.
Example stdio config:

```json
{
  "mcpServers": {
    "meerkat": {
      "command": "meerkat-mcp-server",
      "args": [],
      "env": {
        "ANTHROPIC_API_KEY": "your-key"
      }
    }
  }
}
```

#### Codex CLI

Codex CLI supports MCP servers via a command and also via config. Example:

```bash
codex mcp add meerkat --env ANTHROPIC_API_KEY=your-key -- meerkat-mcp-server
```

You can also configure MCP servers in `~/.codex/config.toml` under
`[mcp_servers.<name>]`.

#### Gemini CLI

Gemini CLI can add MCP servers via CLI or config. Example:

```bash
# HTTP/streamable HTTP
gemini mcp add --scope=project --transport=http meerkat https://your-mcp-server.example.com
```

Or configure in `.gemini/settings.json` (project-scoped) with an `mcpServers`
map that points to your server URL.

### Tool payloads (examples)

Minimal `meerkat_run`:

```json
{
  "prompt": "Write a haiku about Rust",
  "model": "claude-3-7-sonnet-20250219",
  "max_tokens": 512
}
```

Minimal `meerkat_resume`:

```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000001",
  "prompt": "Continue with this"
}
```

`meerkat_config` (get):

```json
{ "action": "get" }
```

For full schemas and optional fields (streaming, output schema, host mode,
provider override), see `docs/api-reference.md`.

### Hosting the MCP server

`meerkat-mcp-server` is a library crate that provides tool schemas and handlers.
If you need a standalone MCP server process, add a thin binary wrapper around
its `tools_list()` and `handle_tools_call*` entrypoints, or embed it in an
existing MCP host.
