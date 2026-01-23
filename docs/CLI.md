# rkat CLI Reference

Complete command-line reference for the Meerkat CLI (`rkat`).

## Overview

`rkat` is the command-line interface for Meerkat, a minimal, high-performance agent harness for LLM-powered applications. It allows you to run AI agents directly from your terminal with support for multiple providers, MCP tools, session management, and streaming output.

### Basic Usage

```bash
rkat <COMMAND> [OPTIONS]
```

### Commands

| Command | Description |
|---------|-------------|
| `run` | Execute an agent with a prompt |
| `resume` | Resume a previous session with a new prompt |
| `sessions` | Manage stored sessions |
| `mcp` | Configure MCP (Model Context Protocol) servers |

---

## run

Execute an agent with a prompt.

```bash
rkat run [OPTIONS] <PROMPT>
```

### Arguments

| Argument | Description |
|----------|-------------|
| `<PROMPT>` | The prompt to execute (required) |

### Options

#### Model Selection

| Flag | Default | Description |
|------|---------|-------------|
| `--model <MODEL>` | `claude-sonnet-4-20250514` | Model to use |
| `-p, --provider <PROVIDER>` | Auto-detected | LLM provider: `anthropic`, `openai`, or `gemini` |

The provider is automatically inferred from the model name:
- `claude-*` models use Anthropic
- `gpt-*`, `o1-*`, `o3-*`, `chatgpt-*` models use OpenAI
- `gemini-*` models use Gemini

#### Token Limits

| Flag | Default | Description |
|------|---------|-------------|
| `--max-tokens <N>` | `4096` | Maximum tokens per turn |
| `--max-total-tokens <N>` | Unlimited | Maximum total tokens for the run |

#### Budget Limits

| Flag | Default | Description |
|------|---------|-------------|
| `--max-duration <DURATION>` | Unlimited | Maximum duration (e.g., `5m`, `1h30m`, `30s`) |
| `--max-tool-calls <N>` | Unlimited | Maximum number of tool calls |

#### Output

| Flag | Default | Description |
|------|---------|-------------|
| `--output <FORMAT>` | `text` | Output format: `text` or `json` |
| `--stream` | Disabled | Stream response tokens as they arrive |

#### Provider Parameters

| Flag | Description |
|------|-------------|
| `--param <KEY=VALUE>` | Provider-specific parameter. Can be repeated. |

See [Provider Parameters](#provider-parameters) for details.

#### Inter-Agent Communication

| Flag | Description |
|------|-------------|
| `--comms-name <NAME>` | Agent name for inter-agent communication. Enables comms if set. |
| `--comms-listen-tcp <ADDR>` | TCP address to listen on (e.g., `0.0.0.0:4200`) |
| `--no-comms` | Disable inter-agent communication entirely |

### Examples

Basic usage:

```bash
# Simple prompt with default model
rkat run "What is the capital of France?"

# Specify a model
rkat run --model claude-opus-4-5 "Explain quantum computing"

# Use OpenAI
rkat run --model gpt-4o "Write a haiku about Rust"

# Use Gemini
rkat run --model gemini-2.0-flash "What are the benefits of async/await?"
```

Output options:

```bash
# Stream response tokens as they arrive
rkat run --stream "Tell me a story"

# JSON output for scripting
rkat run --output json "What is 2+2?" | jq '.text'
```

Budget constraints:

```bash
# Limit run duration
rkat run --max-duration 30s "Solve this complex problem"

# Limit total tokens
rkat run --max-total-tokens 10000 "Analyze this codebase"

# Limit tool calls
rkat run --max-tool-calls 5 "Search for and summarize recent news"
```

Provider parameters:

```bash
# OpenAI reasoning effort
rkat run --model o3-mini --param reasoning_effort=high "Solve this logic puzzle"

# Anthropic thinking budget
rkat run --model claude-sonnet-4 --param thinking_budget=10000 "Plan a project"
```

---

## resume

Resume a previous session with a new prompt. The session's model and provider are restored from the session metadata.

```bash
rkat resume <SESSION_ID> <PROMPT>
```

### Arguments

| Argument | Description |
|----------|-------------|
| `<SESSION_ID>` | Session ID to resume |
| `<PROMPT>` | The prompt to continue with |

### Examples

```bash
# Resume a session
rkat resume 01936f8a-7b2c-7000-8000-000000000001 "Can you explain that in more detail?"

# First run a session, then resume it
rkat run "Let's discuss Rust error handling"
# Note the session ID from output: [Session: 01936f8a-...]
rkat resume 01936f8a-7b2c-7000-8000-000000000001 "What about the ? operator?"
```

---

## sessions

Manage stored sessions.

```bash
rkat sessions <COMMAND>
```

### Subcommands

#### list

List stored sessions.

```bash
rkat sessions list [OPTIONS]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--limit <N>` | `20` | Maximum number of sessions to show |

Output shows session ID, message count, creation time, and last update time.

```bash
$ rkat sessions list
ID                                       MESSAGES     CREATED              UPDATED
--------------------------------------------------------------------------------------------
01936f8a-7b2c-7000-8000-000000000001     8            2025-01-22 14:30     2025-01-22 15:45
01936f8a-7b2c-7000-8000-000000000002     3            2025-01-21 09:15     2025-01-21 09:20
```

#### show

Show details of a specific session.

```bash
rkat sessions show <ID>
```

Displays all messages in the session with their types (USER, ASSISTANT, SYSTEM, TOOL RESULTS).

```bash
$ rkat sessions show 01936f8a-7b2c-7000-8000-000000000001
Session: 01936f8a-7b2c-7000-8000-000000000001
Messages: 4
Version: 2
============================================================

[1] SYSTEM:
  You are a helpful assistant.

[2] USER:
  What is Rust?

[3] ASSISTANT:
  Rust is a systems programming language...

[4] USER:
  What about memory safety?
```

#### delete

Delete a session.

```bash
rkat sessions delete <SESSION_ID>
```

```bash
$ rkat sessions delete 01936f8a-7b2c-7000-8000-000000000001
Deleted session: 01936f8a-7b2c-7000-8000-000000000001
```

---

## mcp

Manage MCP (Model Context Protocol) server configuration. MCP servers provide tools that agents can use during execution.

```bash
rkat mcp <COMMAND>
```

### Subcommands

#### add

Add an MCP server configuration.

```bash
rkat mcp add [OPTIONS] <NAME> [-- <COMMAND>...]
```

| Argument | Description |
|----------|-------------|
| `<NAME>` | Server name (must be unique within scope) |
| `<COMMAND>...` | Command and arguments for stdio transport (after `--`) |

| Flag | Description |
|------|-------------|
| `-t, --transport <TYPE>` | Transport type: `stdio`, `http`, or `sse`. Auto-detected from options. |
| `-u, --url <URL>` | Server URL (for http/sse transports) |
| `-H, --header <KEY:VALUE>` | HTTP header. Can be repeated. (for http/sse transports) |
| `-e, --env <KEY=VALUE>` | Environment variable. Can be repeated. (for stdio transport) |
| `--user` | Add to user scope instead of project (default: project) |

##### Stdio Transport (Local Process)

For MCP servers that run as local processes:

```bash
# Add a stdio server
rkat mcp add my-tools -- npx -y @example/mcp-server

# With environment variables
rkat mcp add api-tools -e API_KEY=secret123 -- python mcp_server.py

# Add to user scope (global)
rkat mcp add global-tools --user -- /usr/local/bin/mcp-server
```

##### HTTP Transport (Remote Server)

For MCP servers accessible over HTTP:

```bash
# Streamable HTTP (default for URLs)
rkat mcp add remote-tools --url https://mcp.example.com/api

# With authentication header
rkat mcp add auth-tools --url https://mcp.example.com/api -H "Authorization:Bearer token123"

# Legacy SSE transport
rkat mcp add sse-tools -t sse --url https://old.example.com/sse
```

#### list

List configured MCP servers.

```bash
rkat mcp list [OPTIONS]
```

| Flag | Description |
|------|-------------|
| `--scope <SCOPE>` | Filter by scope: `user`, `project`, or `local` (alias for project) |
| `--json` | Output as JSON |

```bash
$ rkat mcp list
NAME                 SCOPE      TRANSPORT        TARGET
------------------------------------------------------------
project-tools        project    stdio            npx -y @example/mcp-server
global-api           user       streamable-http  https://api.example.com/mcp
```

#### get

Get details of a specific MCP server.

```bash
rkat mcp get [OPTIONS] <NAME>
```

| Flag | Description |
|------|-------------|
| `--scope <SCOPE>` | Search scope: `user`, `project`, or `local` |
| `--json` | Output as JSON |

```bash
$ rkat mcp get project-tools
Name:    project-tools
Scope:   project
Transport: stdio
Command: npx
Args:    -y @example/mcp-server
Env:
  API_KEY=se...et

$ rkat mcp get --json project-tools
{
  "name": "project-tools",
  "transport": "stdio",
  "command": "npx",
  "args": ["-y", "@example/mcp-server"],
  "env": {"API_KEY": "secret"},
  "scope": "project"
}
```

#### remove

Remove an MCP server configuration.

```bash
rkat mcp remove [OPTIONS] <NAME>
```

| Flag | Description |
|------|-------------|
| `--scope <SCOPE>` | Scope to remove from: `user`, `project`, or `local` |

If the server exists in multiple scopes and `--scope` is not specified, the command will error.

```bash
$ rkat mcp remove project-tools
Removed MCP server 'project-tools' from project config: .rkat/mcp.toml
```

---

## Configuration

### Environment Variables

Required API keys (at least one):

| Variable | Provider | Description |
|----------|----------|-------------|
| `ANTHROPIC_API_KEY` | Anthropic | API key for Claude models |
| `OPENAI_API_KEY` | OpenAI | API key for GPT models |
| `GOOGLE_API_KEY` | Gemini | API key for Gemini models |

### Config Files

#### CLI Configuration

CLI configuration is stored in TOML format:

| Scope | Path | Description |
|-------|------|-------------|
| User | `~/.config/rkat/config.toml` | User-level configuration |
| Project | `.rkat/config.toml` | Project-level configuration |

Project config overrides user config. CLI flags override both.

##### Example config.toml

```toml
[comms]
enabled = true
name = "my-agent"
listen_tcp = "127.0.0.1:4200"
listen_uds = "/tmp/my-agent.sock"
identity_dir = ".rkat/identity"
trusted_peers_path = ".rkat/peers.json"
ack_timeout_secs = 30
max_message_bytes = 1000000
```

#### MCP Configuration

MCP servers are configured in TOML format:

| Scope | Path | Description |
|-------|------|-------------|
| User | `~/.rkat/mcp.toml` | Global MCP servers |
| Project | `.rkat/mcp.toml` | Project-specific MCP servers |

Project servers take precedence over user servers with the same name.

##### Example mcp.toml

```toml
# Stdio server (local process)
[[servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/allowed/dir"]

# Stdio server with environment variables
[[servers]]
name = "database"
command = "python"
args = ["-m", "db_mcp_server"]
env = { DATABASE_URL = "${DATABASE_URL}" }

# HTTP server (streamable HTTP - default for URLs)
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

Environment variable expansion is supported using `${VAR_NAME}` syntax.

### Configuration Precedence

1. CLI flags (highest priority)
2. Project config (`.rkat/`)
3. User config (`~/.config/rkat/` or `~/.rkat/`)
4. Default values (lowest priority)

### Session Storage

Sessions are stored in the user's data directory:

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/meerkat/sessions/` |
| Linux | `~/.local/share/meerkat/sessions/` |
| Windows | `%APPDATA%\meerkat\sessions\` |

---

## Provider Parameters

The `--param` flag allows passing provider-specific parameters. Parameters are passed as `KEY=VALUE` pairs and can be repeated.

### Anthropic Parameters

| Parameter | Description |
|-----------|-------------|
| `thinking_budget` | Token budget for extended thinking (integer) |
| `top_k` | Top-k sampling parameter (integer) |

```bash
# Enable extended thinking with 10k token budget
rkat run --model claude-sonnet-4 --param thinking_budget=10000 "Solve this complex problem"
```

### OpenAI Parameters

| Parameter | Values | Description |
|-----------|--------|-------------|
| `reasoning_effort` | `low`, `medium`, `high` | Reasoning effort for o-series models |
| `seed` | Integer | Seed for deterministic outputs |
| `frequency_penalty` | -2.0 to 2.0 | Penalty for token frequency |
| `presence_penalty` | -2.0 to 2.0 | Penalty for token presence |

```bash
# Use high reasoning effort with o3-mini
rkat run --model o3-mini --param reasoning_effort=high "Prove this theorem"

# Deterministic output with seed
rkat run --model gpt-4o --param seed=42 "Generate a random number"
```

### Gemini Parameters

| Parameter | Description |
|-----------|-------------|
| `thinking_budget` | Token budget for extended thinking (integer) |
| `top_k` | Top-k sampling parameter (integer) |

```bash
# Gemini with thinking budget
rkat run --model gemini-2.0-flash --param thinking_budget=5000 "Analyze this data"
```

---

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Error |
| 2 | Budget exhausted (graceful termination) |

---

## Examples

### Basic Chat

```bash
# Simple question
rkat run "What is the Rust ownership model?"

# With streaming
rkat run --stream "Tell me about async programming in Rust"

# JSON output for scripts
result=$(rkat run --output json "What is 2+2?")
echo "$result" | jq -r '.text'
```

### Multi-Provider Comparison

```bash
# Same prompt to different providers
rkat run --model claude-sonnet-4 "Explain monads in one sentence"
rkat run --model gpt-4o "Explain monads in one sentence"
rkat run --model gemini-2.0-flash "Explain monads in one sentence"
```

### Using MCP Tools

```bash
# Set up a filesystem tool
rkat mcp add fs -- npx -y @modelcontextprotocol/server-filesystem /home/user/projects

# Run with tools available
rkat run "List all Rust files in the projects directory"

# The agent will use the filesystem tool to list files
```

### Session Workflow

```bash
# Start a conversation
rkat run "I want to learn about Rust iterators"
# Output: [Session: 01936f8a-7b2c-7000-8000-000000000001 | Turns: 1 | ...]

# Continue the conversation
rkat resume 01936f8a-7b2c-7000-8000-000000000001 "What about the collect() method?"

# Review the session
rkat sessions show 01936f8a-7b2c-7000-8000-000000000001

# Clean up when done
rkat sessions delete 01936f8a-7b2c-7000-8000-000000000001
```

### Budget-Constrained Runs

```bash
# Time-limited task
rkat run --max-duration 2m "Summarize this long document"

# Token-limited task
rkat run --max-total-tokens 5000 "Explain all Rust collection types"

# Tool-limited task (prevent runaway tool loops)
rkat run --max-tool-calls 3 "Find and read the most relevant file"
```

### Scripting with JSON Output

```bash
#!/bin/bash
# Process multiple prompts and collect results

prompts=("What is Rust?" "What is Go?" "What is Python?")

for prompt in "${prompts[@]}"; do
    result=$(rkat run --output json "$prompt")
    text=$(echo "$result" | jq -r '.text')
    tokens=$(echo "$result" | jq -r '.usage.input_tokens + .usage.output_tokens')
    echo "Q: $prompt"
    echo "A: $text"
    echo "Tokens: $tokens"
    echo "---"
done
```

---

## See Also

- [Quickstart Guide](./quickstart.md) - Get started with Meerkat
- [Configuration Guide](./configuration.md) - Detailed configuration options
- [Architecture](./architecture.md) - How Meerkat works
- [Examples](./examples.md) - More usage examples
