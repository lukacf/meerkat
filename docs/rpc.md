# JSON-RPC Stdio Server

Meerkat exposes a JSON-RPC 2.0 stdio interface for IDE integration, desktop
apps, and automation tools. Unlike REST and MCP, the RPC server keeps agents
alive between turns for fast multi-turn conversations.

---

## Starting the server

```bash
rkat rpc
```

The server reads newline-delimited JSON (JSONL) from stdin and writes JSONL to
stdout. Each line is a complete JSON-RPC 2.0 message.

---

## Protocol

Standard JSON-RPC 2.0 with `"jsonrpc": "2.0"` on every message. Three message
types:

- **Request** (client -> server): has `id`, `method`, `params`
- **Response** (server -> client): has `id`, `result` or `error`
- **Notification** (server -> client): has `method`, `params`, no `id`

### Lifecycle

```
Client                          Server
  |                                |
  |-- initialize ----------------->|
  |<-- result { capabilities } ----|
  |-- initialized (notification) ->|
  |                                |
  |-- session/create { prompt } -->|
  |<-- session/event (notif) ------|  // AgentEvent stream
  |<-- session/event (notif) ------|
  |<-- result { session_id, ... } -|
  |                                |
  |-- turn/start { session_id } -->|
  |<-- session/event (notif) ------|
  |<-- result { text, usage } -----|
  |                                |
  |-- turn/interrupt { sid } ----->|
  |<-- result {} ------------------|
  |                                |
  |-- session/archive { sid } ---->|
  |<-- result {} ------------------|
```

---

## Methods

### initialize

Handshake. Returns server capabilities.

Request:
```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
```

Response:
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "server_info": {
      "name": "meerkat-rpc",
      "version": "0.1.0"
    },
    "methods": [
      "session/create", "session/list", "session/read",
      "session/archive", "turn/start", "turn/interrupt",
      "config/get", "config/set", "config/patch"
    ]
  }
}
```

### session/create

Create a new session and run the first turn.

Request:
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "session/create",
  "params": {
    "prompt": "What is Rust?",
    "model": "claude-sonnet-4-5",
    "max_tokens": 4096,
    "system_prompt": "You are a helpful assistant.",
    "enable_builtins": false,
    "enable_shell": false
  }
}
```

Only `prompt` is required. All other fields are optional and fall back to
config defaults.

Response:
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "session_id": "01936f8a-7b2c-7000-8000-000000000001",
    "text": "Rust is a systems programming language...",
    "turns": 1,
    "tool_calls": 0,
    "usage": {
      "input_tokens": 50,
      "output_tokens": 200
    }
  }
}
```

During execution, `session/event` notifications are emitted (see
[Notifications](#notifications)).

### session/list

List active sessions.

Request:
```json
{"jsonrpc":"2.0","id":3,"method":"session/list"}
```

Response:
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "sessions": [
      {"session_id": "01936f8a-...", "state": "idle"},
      {"session_id": "01936f8b-...", "state": "running"}
    ]
  }
}
```

### session/read

Get session state.

Request:
```json
{"jsonrpc":"2.0","id":4,"method":"session/read","params":{"session_id":"01936f8a-..."}}
```

### session/archive

Remove a session from the runtime.

Request:
```json
{"jsonrpc":"2.0","id":5,"method":"session/archive","params":{"session_id":"01936f8a-..."}}
```

### turn/start

Start a new turn on an existing session.

Request:
```json
{
  "jsonrpc": "2.0",
  "id": 6,
  "method": "turn/start",
  "params": {
    "session_id": "01936f8a-...",
    "prompt": "Can you explain ownership?"
  }
}
```

Returns the same result shape as `session/create`. Fails with error code
`-32001` (SESSION_BUSY) if a turn is already in progress.

### turn/interrupt

Cancel an in-flight turn. No-op if the session is idle.

Request:
```json
{"jsonrpc":"2.0","id":7,"method":"turn/interrupt","params":{"session_id":"01936f8a-..."}}
```

### config/get

Read current config.

```json
{"jsonrpc":"2.0","id":8,"method":"config/get"}
```

### config/set

Replace the config.

```json
{"jsonrpc":"2.0","id":9,"method":"config/set","params":{"agent":{"model":"gpt-5.2"}}}
```

### config/patch

Merge-patch the config (RFC 7396).

```json
{"jsonrpc":"2.0","id":10,"method":"config/patch","params":{"max_tokens":8192}}
```

---

## Notifications

During turn execution, the server emits `session/event` notifications
containing serialized `AgentEvent` payloads:

```json
{
  "jsonrpc": "2.0",
  "method": "session/event",
  "params": {
    "session_id": "01936f8a-...",
    "event": {
      "type": "text_delta",
      "delta": "Rust is"
    }
  }
}
```

Event types match the `AgentEvent` enum in `meerkat-core`:

| Event | Description |
|-------|-------------|
| `run_started` | Turn execution began |
| `text_delta` | Streaming text chunk from LLM |
| `text_complete` | Full text for this turn |
| `tool_call_requested` | LLM wants to call a tool |
| `tool_execution_started` | Tool dispatch began |
| `tool_execution_completed` | Tool returned a result |
| `turn_started` | New LLM call within the turn |
| `turn_completed` | LLM call finished |
| `run_completed` | Turn execution finished successfully |
| `run_failed` | Turn execution failed |
| `budget_warning` | Approaching resource limits |
| `retrying` | Retrying after transient error |

---

## Error Codes

Standard JSON-RPC codes plus Meerkat-specific application codes:

| Code | Name | Description |
|------|------|-------------|
| -32700 | Parse error | Invalid JSON |
| -32600 | Invalid request | Not a valid JSON-RPC request |
| -32601 | Method not found | Unknown method |
| -32602 | Invalid params | Missing or invalid parameters |
| -32603 | Internal error | Server error |
| -32000 | Session not found | Session ID does not exist |
| -32001 | Session busy | Turn already in progress |
| -32002 | Provider error | LLM provider issue (missing key, auth) |
| -32003 | Budget exhausted | Resource limits reached |
| -32004 | Hook denied | Hook blocked the operation |

---

## Architecture

The RPC server is **stateful**: agents stay alive between turns. This is the
key difference from REST (stateless per-request) and MCP (callback pattern).

```
Client (IDE, app) <--stdio JSONL--> RpcServer
                                       |
                                  MethodRouter
                                       |
                                  SessionRuntime
                                   /    |    \
                            Session  Session  Session
                             Task    Task     Task
                              |       |        |
                            Agent   Agent    Agent
                         (exclusive ownership)
```

Each session gets a dedicated tokio task that exclusively owns the `Agent`.
This solves the `cancel(&mut self)` requirement without mutex. Commands
(`StartTurn`, `Interrupt`, `Shutdown`) are sent via channels.

**Backpressure:** The notification channel is bounded. When the client reads
slowly, the agent naturally slows down.

---

## Comparison with Other Surfaces

| | CLI | REST | MCP | RPC |
|---|---|---|---|---|
| Stateful | No | No | No | **Yes** |
| Streaming | stderr | SSE | Optional | JSONL notifications |
| Multi-session | No | No | No | **Yes** |
| Cancellation | Ctrl+C | N/A | N/A | `turn/interrupt` |
| Bidirectional | No | No (SSE one-way) | Partial | **Yes** |

---

## See Also

- [CLI Reference](./CLI.md)
- [REST API](./rest.md)
- [MCP](./mcp.md)
- [Architecture](./architecture.md)
