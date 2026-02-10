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

### Tool schemas

#### meerkat_run

Start a new Meerkat agent session with the given prompt.

Minimal example:
```json
{
  "prompt": "Write a haiku about Rust"
}
```

Full example:
```json
{
  "prompt": "Analyze this code for bugs",
  "system_prompt": "You are a senior code reviewer.",
  "model": "claude-opus-4-6",
  "max_tokens": 4096,
  "provider": "anthropic",
  "output_schema": {
    "type": "object",
    "properties": {
      "bugs": {"type": "array", "items": {"type": "string"}},
      "severity": {"type": "string"}
    },
    "required": ["bugs", "severity"]
  },
  "structured_output_retries": 2,
  "stream": false,
  "verbose": false,
  "tools": [
    {
      "name": "read_file",
      "description": "Read a file from disk",
      "input_schema": {
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "required": ["path"]
      },
      "handler": "callback"
    }
  ],
  "enable_builtins": false,
  "builtin_config": {
    "enable_shell": false,
    "shell_timeout_secs": 30
  },
  "host_mode": false,
  "comms_name": null,
  "hooks_override": null
}
```

##### Parameter reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `prompt` | `string` | Yes | — | User prompt for the agent |
| `system_prompt` | `string \| null` | No | `null` | Override system prompt |
| `model` | `string \| null` | No | config default | Model name (e.g. `"claude-opus-4-6"`, `"gpt-5.2"`) |
| `max_tokens` | `u32 \| null` | No | config default | Max tokens per turn |
| `provider` | `string \| null` | No | inferred | Provider: `"anthropic"`, `"openai"`, `"gemini"`, `"other"` |
| `output_schema` | `object \| null` | No | `null` | JSON schema for structured output (wrapper or raw schema) |
| `structured_output_retries` | `u32` | No | `2` | Max retries for structured output validation |
| `stream` | `bool` | No | `false` | Stream agent events via MCP notifications |
| `verbose` | `bool` | No | `false` | Enable verbose event logging (server-side) |
| `tools` | `array` | No | `[]` | Tool definitions for the agent (see `McpToolDef` below) |
| `enable_builtins` | `bool` | No | `false` | Enable built-in tools (task management, etc.) |
| `builtin_config` | `object \| null` | No | `null` | Config for builtins (only used when `enable_builtins` is true) |
| `builtin_config.enable_shell` | `bool` | No | `false` | Enable shell tools |
| `builtin_config.shell_timeout_secs` | `u64 \| null` | No | `30` | Default shell command timeout |
| `host_mode` | `bool` | No | `false` | Run in host mode for inter-agent comms (requires `comms_name`) |
| `comms_name` | `string \| null` | No | `null` | Agent name for comms |
| `hooks_override` | `HookRunOverrides \| null` | No | `null` | Run-scoped hook overrides |

##### McpToolDef schema

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | `string` | Yes | — | Tool name |
| `description` | `string` | Yes | — | Tool description |
| `input_schema` | `object` | Yes | — | JSON Schema for tool input |
| `handler` | `string \| null` | No | `"callback"` | Handler type (`"callback"` = result provided via `meerkat_resume`) |

When tools with `handler: "callback"` are provided and the agent requests a
tool call, the response includes `pending_tool_calls`. The MCP client must
execute the tool and provide results via `meerkat_resume`.

#### meerkat_resume

Resume an existing session or provide tool results for pending tool calls.

Minimal example:
```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000001",
  "prompt": "Continue with this"
}
```

Full example with tool results:
```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000001",
  "prompt": "",
  "stream": false,
  "verbose": false,
  "tools": [],
  "tool_results": [
    {
      "tool_use_id": "tc_abc123",
      "content": "File contents here...",
      "is_error": false
    }
  ],
  "enable_builtins": false,
  "builtin_config": null,
  "host_mode": false,
  "comms_name": null,
  "model": null,
  "max_tokens": null,
  "provider": null,
  "hooks_override": null
}
```

##### Parameter reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `session_id` | `string` | Yes | — | Session ID to resume |
| `prompt` | `string` | Yes | — | Follow-up prompt (can be empty when providing tool results) |
| `stream` | `bool` | No | `false` | Stream agent events via MCP notifications |
| `verbose` | `bool` | No | `false` | Enable verbose event logging |
| `tools` | `array` | No | `[]` | Tool definitions (should match the original run) |
| `tool_results` | `array` | No | `[]` | Tool results for pending tool calls |
| `tool_results[].tool_use_id` | `string` | Yes | — | ID of the tool call |
| `tool_results[].content` | `string` | Yes | — | Result content (or error message) |
| `tool_results[].is_error` | `bool` | No | `false` | Whether this is an error result |
| `enable_builtins` | `bool` | No | `false` | Enable built-in tools |
| `builtin_config` | `object \| null` | No | `null` | Builtin tool config |
| `host_mode` | `bool` | No | `false` | Enable host mode |
| `comms_name` | `string \| null` | No | from session | Agent name for comms |
| `model` | `string \| null` | No | from session | Model override |
| `max_tokens` | `u32 \| null` | No | from session | Max tokens override |
| `provider` | `string \| null` | No | from session | Provider override |
| `hooks_override` | `HookRunOverrides \| null` | No | `null` | Run-scoped hook overrides |

##### Response format

Both `meerkat_run` and `meerkat_resume` return MCP-standard tool results:

Success:
```json
{
  "content": [{
    "type": "text",
    "text": "{\"content\":[{\"type\":\"text\",\"text\":\"Agent response here\"}],\"session_id\":\"01936f8a-...\",\"turns\":1,\"tool_calls\":0,\"structured_output\":null,\"schema_warnings\":null}"
  }]
}
```

The inner `text` field is a JSON-encoded payload containing:

| Field | Type | Description |
|-------|------|-------------|
| `content` | `array` | MCP content blocks with the agent's text |
| `session_id` | `string` | Session ID (save for `meerkat_resume`) |
| `turns` | `u32` | Number of LLM calls made |
| `tool_calls` | `u32` | Number of tool calls executed |
| `structured_output` | `object \| null` | Parsed structured output |
| `schema_warnings` | `array \| null` | Schema compatibility warnings |

When a callback tool is pending:
```json
{
  "content": [{
    "type": "text",
    "text": "{\"content\":[{\"type\":\"text\",\"text\":\"Agent is waiting for tool results\"}],\"session_id\":\"01936f8a-...\",\"status\":\"pending_tool_call\",\"pending_tool_calls\":[{\"tool_name\":\"read_file\",\"args\":{\"path\":\"/tmp/data.txt\"}}]}"
  }]
}
```

#### meerkat_config

Get or update Meerkat config for this MCP server instance.

```json
{ "action": "get" }
```

```json
{ "action": "set", "config": { "agent": { "model": "gpt-5.2" } } }
```

```json
{ "action": "patch", "patch": { "agent": { "max_tokens_per_turn": 8192 } } }
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `action` | `string` | Yes | One of `"get"`, `"set"`, `"patch"` |
| `config` | `object \| null` | For `set` | Full config to replace |
| `patch` | `object \| null` | For `patch` | RFC 7396 merge-patch delta |

#### meerkat_capabilities

Returns the runtime capability set with status resolved against config.

Request:
```json
{}
```

Response:
```json
{
  "contract_version": {"major": 0, "minor": 1, "patch": 0},
  "capabilities": [
    {
      "id": "sessions",
      "description": "Session lifecycle management",
      "status": "Available"
    },
    {
      "id": "structured_output",
      "description": "Structured output extraction with JSON schema",
      "status": "Available"
    },
    {
      "id": "builtins",
      "description": "Built-in tools (task management)",
      "status": {"DisabledByPolicy": {"description": "Disabled by config"}}
    },
    {
      "id": "shell",
      "description": "Shell command execution",
      "status": {"DisabledByPolicy": {"description": "Disabled by config"}}
    },
    {
      "id": "comms",
      "description": "Inter-agent communication",
      "status": {"NotCompiled": {"feature": "comms"}}
    }
  ]
}
```

Possible `status` values:

| Status | Shape | Meaning |
|--------|-------|---------|
| `Available` | `"Available"` | Compiled in, config-enabled |
| `DisabledByPolicy` | `{"DisabledByPolicy": {"description": "..."}}` | Compiled in but disabled |
| `NotCompiled` | `{"NotCompiled": {"feature": "..."}}` | Feature flag absent |
| `NotSupportedByProtocol` | `{"NotSupportedByProtocol": {"reason": "..."}}` | Protocol does not support it |

### Hosting the MCP server

`meerkat-mcp-server` is a library crate that provides tool schemas and handlers.
If you need a standalone MCP server process, add a thin binary wrapper around
its `tools_list()` and `handle_tools_call*` entrypoints, or embed it in an
existing MCP host.
