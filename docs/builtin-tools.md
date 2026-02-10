# Meerkat Built-in Tools Reference

This document is a comprehensive reference for all built-in tools available in Meerkat (`rkat`). Every tool name, parameter, type, and default value is derived directly from source code.

## Overview

| Tool Name | Category | Default Enabled | Feature Gate | Description |
|-----------|----------|-----------------|--------------|-------------|
| `task_create` | Task | Yes | -- | Create a new task in the project task list |
| `task_get` | Task | Yes | -- | Get a task by its ID |
| `task_list` | Task | Yes | -- | List tasks, optionally filtered by status or labels |
| `task_update` | Task | Yes | -- | Update an existing task |
| `shell` | Shell | No | -- | Execute a shell command |
| `shell_jobs` | Shell | No | -- | List all background shell jobs |
| `shell_job_status` | Shell | No | -- | Check status of a background shell job |
| `shell_job_cancel` | Shell | No | -- | Cancel a running background shell job |
| `agent_spawn` | Sub-Agent | No | `sub-agents` | Spawn a new sub-agent with clean context |
| `agent_fork` | Sub-Agent | No | `sub-agents` | Fork current agent with continued context |
| `agent_status` | Sub-Agent | No | `sub-agents` | Get status and output of a sub-agent |
| `agent_cancel` | Sub-Agent | No | `sub-agents` | Cancel a running sub-agent |
| `agent_list` | Sub-Agent | No | `sub-agents` | List all sub-agents spawned by this agent |
| `memory_search` | Memory | N/A (separate dispatcher) | `memory-store-session` | Search semantic memory for past conversation content |
| `wait` | Utility | Yes | -- | Pause execution for a specified duration |
| `datetime` | Utility | Yes | -- | Get current date and time |
| `send_message` | Comms | Yes (when comms active) | `comms` | Send a simple text message to a trusted peer |
| `send_request` | Comms | Yes (when comms active) | `comms` | Send a request to a trusted peer |
| `send_response` | Comms | Yes (when comms active) | `comms` | Send a response to a previous request |
| `list_peers` | Comms | Yes (when comms active) | `comms` | List all trusted peers and their connection status |

---

## Enabling and Disabling Tools

### Factory-Level Flags

`AgentFactory` controls which tool categories are available via builder methods:

| Flag | Builder Method | Default | Controls |
|------|---------------|---------|----------|
| `enable_builtins` | `.builtins(bool)` | `false` | Task tools, utility tools (`wait`, `datetime`) |
| `enable_shell` | `.shell(bool)` | `false` | All shell tools (`shell`, `shell_jobs`, `shell_job_status`, `shell_job_cancel`) |
| `enable_subagents` | `.subagents(bool)` | `false` | All sub-agent tools (requires `sub-agents` feature) |
| `enable_memory` | `.memory(bool)` | `false` | `memory_search` (requires `memory-store-session` feature) |
| `enable_comms` | `.comms(bool)` | `false` | All comms tools (requires `comms` feature) |

**Source:** `meerkat/src/factory.rs` -- `AgentFactory` struct and builder methods.

### Per-Build Overrides

Each `AgentBuildConfig` can override factory-level flags for a single agent build:

| Override Field | Type | Effect |
|---------------|------|--------|
| `override_builtins` | `Option<bool>` | Takes precedence over `enable_builtins` when `Some` |
| `override_shell` | `Option<bool>` | Takes precedence over `enable_shell` when `Some` |
| `override_subagents` | `Option<bool>` | Takes precedence over `enable_subagents` when `Some` |
| `override_memory` | `Option<bool>` | Takes precedence over `enable_memory` when `Some` |

All default to `None` (use factory-level setting).

**Source:** `meerkat/src/factory.rs` -- `AgentBuildConfig` struct.

### ToolPolicyLayer

Individual tools can be enabled or disabled via `ToolPolicyLayer` in `BuiltinToolConfig`. When shell is enabled, a `ToolPolicyLayer` is applied that activates the four shell tools (which have `default_enabled: false`). The same mechanism applies to sub-agent tools.

### Capability Registrations

Capabilities are registered via the `inventory` crate in `meerkat-tools/src/lib.rs`:

- **Builtins** (`CapabilityId::Builtins`): `task_list`, `task_create`, `task_get`, `task_update`, `wait`, `datetime`. Controlled by `config.tools.builtins_enabled`.
- **Shell** (`CapabilityId::Shell`): `shell`, `shell_jobs`, `shell_job_status`, `shell_job_cancel`. Controlled by `config.tools.shell_enabled`.
- **SubAgents** (`CapabilityId::SubAgents`): `agent_spawn`, `agent_fork`, `agent_status`, `agent_cancel`, `agent_list`. Requires `sub-agents` feature.

---

## Task Tools

Task tools provide structured work tracking. They are enabled by default when builtins are on. Tasks are persisted via `TaskStore` (either `FileTaskStore` on disk or `MemoryTaskStore` in-memory).

### Task Data Model

A `Task` object returned by task tools has these fields:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Unique task identifier |
| `subject` | `String` | Short subject/title |
| `description` | `String` | Detailed description |
| `status` | `String` | `"pending"`, `"in_progress"`, or `"completed"` |
| `priority` | `String` | `"low"`, `"medium"`, or `"high"` |
| `labels` | `String[]` | Labels/tags for categorization |
| `blocks` | `String[]` | IDs of tasks that this task blocks |
| `blocked_by` | `String[]` | IDs of tasks that block this task |
| `created_at` | `String` | ISO 8601 creation timestamp |
| `updated_at` | `String` | ISO 8601 last-updated timestamp |
| `created_by_session` | `String?` | Session ID that created this task |
| `updated_by_session` | `String?` | Session ID that last updated this task |
| `owner` | `String?` | Owner/assignee (agent name or user identifier) |
| `metadata` | `Object` | Arbitrary key-value metadata |

**Source:** `meerkat-tools/src/builtin/types.rs` -- `Task` struct.

### `task_create`

Create a new task in the project task list.

**Default enabled:** Yes

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `subject` | `String` | Yes | -- | Short subject/title of the task |
| `description` | `String` | Yes | -- | Detailed description of what needs to be done |
| `priority` | `String` | No | `"medium"` | `"low"`, `"medium"`, or `"high"` |
| `labels` | `String[]` | No | `[]` | Labels/tags for categorization |
| `blocks` | `String[]` | No | `[]` | Task IDs that this task blocks |
| `blocked_by` | `String[]` | No | `[]` | Task IDs that block this task |
| `owner` | `String` | No | `null` | Owner/assignee |
| `metadata` | `Object` | No | `{}` | Arbitrary key-value metadata |

**Returns:** The created `Task` object (see Task Data Model above).

**Source:** `meerkat-tools/src/builtin/tasks/task_create.rs`

### `task_get`

Get a task by its ID.

**Default enabled:** Yes

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `id` | `String` | Yes | -- | The task ID to retrieve |

**Returns:** The `Task` object matching the given ID.

**Error:** `ExecutionFailed` if the task ID is not found.

**Source:** `meerkat-tools/src/builtin/tasks/task_get.rs`

### `task_list`

List tasks in the project, optionally filtered by status or labels.

**Default enabled:** Yes

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `status` | `String` | No | `null` (all) | Filter by status: `"pending"`, `"in_progress"`, or `"completed"` |
| `labels` | `String[]` | No | `null` (all) | Filter by labels (tasks matching any label) |

**Returns:** Array of `Task` objects matching the filters.

**Source:** `meerkat-tools/src/builtin/tasks/task_list.rs`

### `task_update`

Update an existing task. Only provided fields are modified; omitted fields remain unchanged.

**Default enabled:** Yes

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `id` | `String` | Yes | -- | The task ID to update |
| `subject` | `String` | No | unchanged | New subject/title |
| `description` | `String` | No | unchanged | New description |
| `status` | `String` | No | unchanged | New status: `"pending"`, `"in_progress"`, or `"completed"` |
| `priority` | `String` | No | unchanged | New priority: `"low"`, `"medium"`, or `"high"` |
| `labels` | `String[]` | No | unchanged | Replace all labels |
| `owner` | `String` | No | unchanged | New owner/assignee |
| `metadata` | `Object` | No | unchanged | Merge metadata (set key to `null` to remove) |
| `add_blocks` | `String[]` | No | `[]` | Task IDs to add to the `blocks` list |
| `remove_blocks` | `String[]` | No | `[]` | Task IDs to remove from the `blocks` list |
| `add_blocked_by` | `String[]` | No | `[]` | Task IDs to add to the `blocked_by` list |
| `remove_blocked_by` | `String[]` | No | `[]` | Task IDs to remove from the `blocked_by` list |

**Returns:** The updated `Task` object.

**Error:** `ExecutionFailed` if the task ID is not found.

**Source:** `meerkat-tools/src/builtin/tasks/task_update.rs`

---

## Shell Tools

Shell tools execute commands and manage background jobs. They are all `default_enabled: false` and require the factory-level `enable_shell` flag (or a `ToolPolicyLayer` override) to be active.

### Shell Configuration

Shell behavior is controlled by `ShellConfig`:

| Config Field | Type | Default | Description |
|-------------|------|---------|-------------|
| `enabled` | `bool` | `false` | Master switch for shell tools |
| `default_timeout_secs` | `u64` | `30` | Default command timeout |
| `restrict_to_project` | `bool` | `true` | Restrict working dirs to project root |
| `shell` | `String` | `"nu"` | Shell binary name (Nushell by default; falls back to bash/zsh/sh) |
| `shell_path` | `PathBuf?` | `None` | Explicit shell binary path |
| `project_root` | `PathBuf` | CWD | Project root for path restriction |
| `max_completed_jobs` | `usize` | `100` | Maximum completed jobs retained |
| `completed_job_ttl_secs` | `u64` | `300` | TTL for completed job records (seconds) |
| `max_concurrent_processes` | `usize` | `10` | Maximum concurrent background processes |
| `security_mode` | `SecurityMode` | `Unrestricted` | Command security policy |
| `security_patterns` | `Vec<String>` | `[]` | Glob patterns for allow/deny lists |

**Source:** `meerkat-tools/src/builtin/shell/config.rs`

### Shell Security Modes

The `SecurityEngine` validates commands before execution using POSIX-compliant word splitting (`shlex`) and glob pattern matching (`globset`).

| Mode | Behavior |
|------|----------|
| `Unrestricted` | All commands allowed (default) |
| `AllowList` | Only commands matching `security_patterns` globs are allowed |
| `DenyList` | Commands matching `security_patterns` globs are rejected |

Patterns are matched against the canonicalized command invocation. The engine parses the full command string into individual words, then validates the executable (first word) and the full command string against the pattern set.

**Source:** `meerkat-tools/src/builtin/shell/security.rs`

### `shell`

Execute a shell command. Uses POSIX-style parsing for policy checks; runs via Nushell or fallback shell.

**Default enabled:** No (requires shell to be enabled)

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `command` | `String` | Yes | -- | The command to execute (POSIX-style parsing for policy checks) |
| `working_dir` | `String` | No | Project root | Working directory (relative to project root) |
| `timeout_secs` | `u64` | No | Config default (30) | Timeout in seconds |
| `background` | `bool` | No | `false` | If `true`, run in background and return job ID immediately |

**Returns (foreground):**

```json
{
  "exit_code": 0,
  "stdout": "...",
  "stderr": "...",
  "timed_out": false,
  "duration_secs": 1.23
}
```

Output is truncated to the last 100,000 characters if it exceeds that limit. Fields `stdout_lossy` and `stderr_lossy` indicate whether output was lossy-decoded from non-UTF-8 bytes.

**Returns (background, `background: true`):**

```json
{
  "job_id": "job_<uuid-v7>",
  "status": "running",
  "message": "Background job started"
}
```

**Source:** `meerkat-tools/src/builtin/shell/tool.rs`

### `shell_jobs`

List all background shell jobs.

**Default enabled:** No (requires shell to be enabled)

**Parameters:** None (empty object).

**Returns:** Array of `JobSummary` objects:

```json
[
  {
    "id": "job_<uuid-v7>",
    "command": "echo hello",
    "status": "running",
    "started_at_unix": 1700000000
  }
]
```

The `status` field is one of: `"running"`, `"completed"`, `"failed"`, `"timed_out"`, `"cancelled"`.

**Source:** `meerkat-tools/src/builtin/shell/jobs_list_tool.rs`

### `shell_job_status`

Check status of a background shell job. Returns full job details including output when complete.

**Default enabled:** No (requires shell to be enabled)

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `job_id` | `String` | Yes | -- | The job ID to check |

**Returns:** Full `BackgroundJob` object:

```json
{
  "id": "job_<uuid-v7>",
  "command": "echo test",
  "working_dir": "/path/to/project",
  "timeout_secs": 30,
  "started_at_unix": 1700000000,
  "status": "completed"
}
```

**Error:** `ExecutionFailed` if the job ID is not found.

**Source:** `meerkat-tools/src/builtin/shell/job_status_tool.rs`

### `shell_job_cancel`

Cancel a running background shell job.

**Default enabled:** No (requires shell to be enabled)

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `job_id` | `String` | Yes | -- | The job ID to cancel |

**Returns:**

```json
{
  "job_id": "job_<uuid-v7>",
  "status": "cancelled"
}
```

**Error:** `ExecutionFailed` if the job ID is not found.

**Source:** `meerkat-tools/src/builtin/shell/job_cancel_tool.rs`

### Shell Types

**JobId format:** `"job_"` followed by a UUID v7 string.

**JobStatus variants:** `Running`, `Completed`, `Failed`, `TimedOut`, `Cancelled` (serialized as lowercase snake_case strings).

**Source:** `meerkat-tools/src/builtin/shell/types.rs`

---

## Sub-Agent Tools

Sub-agent tools allow spawning, forking, monitoring, and cancelling child agents. They are all `default_enabled: false` and require both the `sub-agents` Cargo feature and the factory-level `enable_subagents` flag.

### Shared Types

**ToolAccessInput** (tagged union, `"policy"` discriminator):

| Variant | Fields | Description |
|---------|--------|-------------|
| `inherit` | -- | Inherit all tools from parent |
| `allow_list` | `tools: String[]` | Only allow the listed tools |
| `deny_list` | `tools: String[]` | Block the listed tools |

Example: `{"policy": "allow_list", "tools": ["shell", "task_list"]}`

**BudgetInput** (used in `agent_spawn`):

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `max_tokens` | `u64` | No | Maximum tokens for the sub-agent |
| `max_turns` | `u32` | No | Maximum turns for the sub-agent |
| `max_tool_calls` | `u32` | No | Maximum tool calls for the sub-agent |

### `agent_spawn`

Spawn a new sub-agent with clean context. The sub-agent starts fresh with only the provided prompt.

**Default enabled:** No (requires `sub-agents` feature + subagents enabled)

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `prompt` | `String` | Yes | -- | Initial prompt/task for the sub-agent |
| `provider` | `String` | No | Parent's provider | LLM provider: `"anthropic"`, `"openai"`, or `"gemini"` |
| `model` | `String` | No | Provider's default | Model name (provider-specific) |
| `tool_access` | `ToolAccessInput` | No | `inherit` | Tool access policy (see Shared Types) |
| `budget` | `BudgetInput` | No | Inherited | Budget limits (see Shared Types) |
| `system_prompt` | `String` | No | `null` | Override system prompt for the sub-agent |
| `host_mode` | `bool` | No | `false` | Agent stays alive processing comms after initial prompt (only with `comms` feature) |

Note: The `host_mode` parameter is only present when the `comms` feature is enabled. Without `comms`, `SpawnParamsNoComms` is used which omits this field.

**Returns:**

```json
{
  "agent_id": "<uuid>",
  "name": "agent-<name>",
  "provider": "anthropic",
  "model": "claude-sonnet-4-5",
  "state": "running",
  "message": "Sub-agent spawned successfully"
}
```

When `comms` is enabled and `host_mode` is active, additional `comms` field is included with connection details.

**Source:** `meerkat-tools/src/builtin/sub_agent/spawn.rs`

### `agent_fork`

Fork the current agent with continued context. The forked agent inherits the parent's conversation history.

**Default enabled:** No (requires `sub-agents` feature + subagents enabled)

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `prompt` | `String` | Yes | -- | Additional prompt/instruction appended to inherited conversation history |
| `provider` | `String` | No | Parent's provider | `"anthropic"`, `"openai"`, or `"gemini"` |
| `model` | `String` | No | Provider's default | Model name (provider-specific) |
| `tool_access` | `ToolAccessInput` | No | `inherit` | Tool access policy (see Shared Types) |
| `budget_policy` | `String` | No | `null` | Budget allocation: `"equal"`, `"remaining"`, `"proportional"`, or `"fixed:N"` |

**Returns:**

```json
{
  "agent_id": "<uuid>",
  "name": "fork-<name>",
  "provider": "anthropic",
  "model": "claude-sonnet-4-5",
  "state": "running",
  "messages_inherited": 12,
  "message": "Agent forked successfully"
}
```

**Source:** `meerkat-tools/src/builtin/sub_agent/fork.rs`

### `agent_status`

Get status and output of a sub-agent by ID. Use this to poll for completion.

**Default enabled:** No (requires `sub-agents` feature + subagents enabled)

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `agent_id` | `String` | Yes | -- | UUID of the sub-agent |

**Returns:**

```json
{
  "agent_id": "<uuid>",
  "state": "completed",
  "output": "The agent's final text output",
  "is_final": true,
  "duration_ms": 5432,
  "tokens_used": 1500
}
```

When the agent is still running, `is_final` is `false` and `output`/`duration_ms`/`tokens_used` may be absent. If the agent failed, an `error` field is present instead of `output`.

**Source:** `meerkat-tools/src/builtin/sub_agent/status.rs`

### `agent_cancel`

Cancel a running sub-agent.

**Default enabled:** No (requires `sub-agents` feature + subagents enabled)

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `agent_id` | `String` | Yes | -- | UUID of the sub-agent to cancel |

**Returns:**

```json
{
  "agent_id": "<uuid>",
  "success": true,
  "previous_state": "running",
  "message": "Agent cancelled"
}
```

**Source:** `meerkat-tools/src/builtin/sub_agent/cancel.rs`

### `agent_list`

List all sub-agents spawned by this agent.

**Default enabled:** No (requires `sub-agents` feature + subagents enabled)

**Parameters:** None (empty object).

**Returns:**

```json
{
  "agents": [
    {
      "id": "<uuid>",
      "name": "agent-<name>",
      "state": "running",
      "depth": 1,
      "running_ms": 3200
    }
  ],
  "running_count": 1,
  "completed_count": 0,
  "failed_count": 0,
  "total_count": 1
}
```

**Source:** `meerkat-tools/src/builtin/sub_agent/list.rs`

---

## Memory Tool

The memory search tool lives in the `meerkat-memory` crate and is composed into the tool dispatcher as a separate `MemorySearchDispatcher` (implementing `AgentToolDispatcher`). It is NOT part of `CompositeDispatcher` -- it is wired via `ToolGateway` when the `memory-store-session` feature is enabled and `enable_memory` is `true`.

### `memory_search`

Search semantic memory for past conversation content. Memory contains text from earlier conversation turns that were compacted away to save context space.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | `String` | Yes | -- | Natural language search query describing what you want to recall |
| `limit` | `usize` | No | `5` | Maximum number of results to return (max: 20) |

The `limit` is capped at 20 regardless of the requested value.

**Returns:** JSON array of memory results:

```json
[
  {
    "content": "The project codename is AURORA-7",
    "score": 0.87,
    "session_id": "<uuid>",
    "turn": 3
  }
]
```

Returns an empty array if no matches are found.

**Source:** `meerkat-memory/src/tool.rs`

---

## Utility Tools

Utility tools are general-purpose helpers enabled by default when builtins are on.

### `wait`

Pause execution for the specified number of seconds. Supports fractional seconds. Useful for polling loops or rate limiting.

**Default enabled:** Yes

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `seconds` | `f64` | Yes | -- | Duration to wait (range: 0.1 to 300.0) |

Maximum wait time is 300 seconds (5 minutes). Values below 0.1 or above 300.0 are rejected.

**Returns (completed):**

```json
{
  "waited_seconds": 5.0,
  "status": "complete"
}
```

**Returns (interrupted):**

```json
{
  "waited_seconds": 2.3,
  "requested_seconds": 5.0,
  "status": "interrupted",
  "reason": "cancellation signal received"
}
```

**Source:** `meerkat-tools/src/builtin/utility/wait.rs`

### `datetime`

Get the current date and time. Returns ISO 8601 formatted datetime and Unix timestamp.

**Default enabled:** Yes

**Parameters:** None (empty object).

**Returns:**

```json
{
  "iso8601": "2025-01-15T14:30:00+01:00",
  "date": "2025-01-15",
  "time": "14:30:00",
  "timezone": "+01:00",
  "unix_timestamp": 1736949000,
  "year": 2025,
  "month": 1,
  "day": 15,
  "weekday": "Wednesday"
}
```

**Source:** `meerkat-tools/src/builtin/utility/datetime.rs`

---

## Comms Tools

Comms tools enable inter-agent communication. They require the `comms` Cargo feature (`dep:meerkat-comms`) and the factory-level `enable_comms` flag. Unlike other tool categories, comms tools are provided as a separate `CommsToolSurface` dispatcher (implementing `AgentToolDispatcher`) and composed via `ToolGateway`, NOT bundled into `CompositeDispatcher`.

Comms tools are only shown to the agent when at least one trusted peer is configured (controlled by `CommsToolSurface::peer_availability()`).

Tool definitions (name, description, input schema) are dynamically loaded from `meerkat_comms::tools_list()`.

### `send_message`

Send a simple text message to a trusted peer. The peer receives it in their inbox.

**Default enabled:** Yes (when comms is active)

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `to` | `String` | Yes | -- | Peer name to send the message to |
| `body` | `String` | Yes | -- | Message content |

**Returns:**

```json
{
  "status": "sent"
}
```

**Source:** `meerkat-comms/src/mcp/tools.rs` -- `SendMessageInput`

### `send_request`

Send a request to a trusted peer and wait for acknowledgement. Use this for structured interactions where you expect a response.

**Default enabled:** Yes (when comms is active)

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `to` | `String` | Yes | -- | Peer name to send the request to |
| `intent` | `String` | Yes | -- | Request intent/action identifier |
| `params` | `any` | No | `null` | Request parameters (arbitrary JSON) |

**Returns:**

```json
{
  "status": "sent"
}
```

**Source:** `meerkat-comms/src/mcp/tools.rs` -- `SendRequestInput`

### `send_response`

Send a response back to a previous request from a peer.

**Default enabled:** Yes (when comms is active)

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `to` | `String` | Yes | -- | Peer name to send the response to |
| `in_reply_to` | `String` (UUID) | Yes | -- | ID of the request being responded to |
| `status` | `String` | Yes | -- | Response status: `"accepted"`, `"completed"`, or `"failed"` |
| `result` | `any` | No | `null` | Response result data (arbitrary JSON) |

**Returns:**

```json
{
  "status": "sent"
}
```

**Source:** `meerkat-comms/src/mcp/tools.rs` -- `SendResponseInput`

### `list_peers`

List all trusted peers and their connection status.

**Default enabled:** Yes (when comms is active)

**Parameters:** None (empty object).

**Returns:**

```json
{
  "peers": [
    {
      "name": "agent-beta",
      "peer_id": "<derived-peer-id>",
      "address": "tcp://127.0.0.1:4200"
    }
  ]
}
```

**Source:** `meerkat-comms/src/mcp/tools.rs` -- `ListPeersInput`

---

## Cargo Feature Gates

Tool availability depends on compile-time features in the `meerkat-tools` crate:

| Feature | Default | Dependencies | Tools Unlocked |
|---------|---------|-------------|----------------|
| `sub-agents` | Yes | `dep:meerkat-client` | `agent_spawn`, `agent_fork`, `agent_status`, `agent_cancel`, `agent_list` |
| `comms` | Yes | `dep:meerkat-comms` | `send_message`, `send_request`, `send_response`, `list_peers` |
| `mcp` | Yes | `dep:meerkat-mcp` | External MCP tool routing (not builtin tools) |

The `memory_search` tool is gated by the `memory-store-session` feature on the `meerkat` facade crate (not `meerkat-tools`), since `MemorySearchDispatcher` lives in `meerkat-memory`.

**Source:** `meerkat-tools/Cargo.toml`

---

## CODE ISSUES FOUND

No code issues were found during this audit. All tool names, parameter names, types, defaults, and feature gates are consistent across tool implementations, capability registrations, and factory wiring.
