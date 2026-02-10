# REST API

Meerkat ships a REST server for running and managing agent sessions over HTTP.
This is the best fit if you want a simple, language-agnostic API for the
Meerkat core.

---

## Running the server

```bash
# Development
cargo run --package meerkat-rest

# Production
cargo build --release --package meerkat-rest
./target/release/meerkat-rest
```

---

## Configuration

The REST server reads config from its instance data directory. The config file
is located at:

- macOS: `~/Library/Application Support/meerkat/rest/config.toml`
- Linux: `~/.local/share/meerkat/rest/config.toml`
- Windows: `%APPDATA%\meerkat\rest\config.toml`

Key sections:

```toml
[rest]
host = "127.0.0.1"
port = 8080

[agent]
model = "claude-opus-4-6"
max_tokens_per_turn = 8192

[tools]
builtins_enabled = false
shell_enabled = false
```

API keys are still provided via environment variables:

- `ANTHROPIC_API_KEY`
- `OPENAI_API_KEY`
- `GOOGLE_API_KEY`

---

## Endpoints

### POST /sessions

Create and run a new session.

Request (minimal):
```json
{
  "prompt": "Your prompt here"
}
```

Request (full):
```json
{
  "prompt": "Your prompt here",
  "system_prompt": "Optional system prompt",
  "model": "claude-opus-4-6",
  "provider": "anthropic",
  "max_tokens": 4096,
  "output_schema": {
    "schema": {"type": "object", "properties": {"answer": {"type": "string"}}},
    "name": "answer",
    "strict": false,
    "compat": "lossy",
    "format": "meerkat_v1"
  },
  "structured_output_retries": 2,
  "verbose": false,
  "host_mode": false,
  "comms_name": null,
  "hooks_override": null
}
```

#### Request fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `prompt` | `string` | Yes | — | The user prompt to send to the agent |
| `system_prompt` | `string \| null` | No | `null` | Override the system prompt for this session |
| `model` | `string \| null` | No | config default | Model name (e.g. `"claude-opus-4-6"`, `"gpt-5.2"`) |
| `provider` | `string \| null` | No | inferred from model | Provider: `"anthropic"`, `"openai"`, `"gemini"`, `"other"` |
| `max_tokens` | `u32 \| null` | No | config default | Max tokens per turn |
| `output_schema` | `OutputSchema \| null` | No | `null` | JSON schema for structured output extraction (wrapper or raw schema) |
| `structured_output_retries` | `u32` | No | `2` | Max retries for structured output validation |
| `verbose` | `bool` | No | `false` | Enable verbose event logging (server-side) |
| `host_mode` | `bool` | No | `false` | Run in host mode for inter-agent comms (requires `comms_name`) |
| `comms_name` | `string \| null` | No | `null` | Agent name for inter-agent communication |
| `hooks_override` | `HookRunOverrides \| null` | No | `null` | Run-scoped hook overrides (see [Hooks](./hooks.md)) |

#### Response fields

| Field | Type | Always present | Description |
|-------|------|----------------|-------------|
| `session_id` | `string` | Yes | UUID of the created session |
| `text` | `string` | Yes | The agent's response text |
| `turns` | `u32` | Yes | Number of LLM calls made |
| `tool_calls` | `u32` | Yes | Number of tool calls executed |
| `usage` | `WireUsage` | Yes | Token usage breakdown |
| `usage.input_tokens` | `u64` | Yes | Input tokens consumed |
| `usage.output_tokens` | `u64` | Yes | Output tokens generated |
| `usage.total_tokens` | `u64` | Yes | Total tokens (input + output) |
| `usage.cache_creation_tokens` | `u64 \| null` | No | Cache creation tokens (provider-specific) |
| `usage.cache_read_tokens` | `u64 \| null` | No | Cache read tokens (provider-specific) |
| `structured_output` | `object \| null` | No | Parsed structured output (when `output_schema` was provided) |
| `schema_warnings` | `array \| null` | No | Schema compatibility warnings per provider |

Response:
```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000001",
  "text": "Response text",
  "turns": 1,
  "tool_calls": 0,
  "usage": {
    "input_tokens": 50,
    "output_tokens": 200,
    "total_tokens": 250
  },
  "structured_output": null,
  "schema_warnings": null
}
```

#### Structured output example

Request:
```json
{
  "prompt": "Extract the capital of France",
  "model": "claude-opus-4-6",
  "output_schema": {
    "schema": {
      "type": "object",
      "properties": {
        "country": {"type": "string"},
        "capital": {"type": "string"}
      },
      "required": ["country", "capital"]
    },
    "name": "capital_extraction",
    "strict": false,
    "compat": "lossy",
    "format": "meerkat_v1"
  },
  "structured_output_retries": 2
}
```

Response:
```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000002",
  "text": "{\"country\":\"France\",\"capital\":\"Paris\"}",
  "turns": 1,
  "tool_calls": 0,
  "usage": {
    "input_tokens": 80,
    "output_tokens": 30,
    "total_tokens": 110
  },
  "structured_output": {
    "country": "France",
    "capital": "Paris"
  },
  "schema_warnings": [
    {
      "provider": "gemini",
      "path": "/properties/choice/oneOf",
      "message": "Removed unsupported keyword 'oneOf'"
    }
  ]
}
```

When `output_schema` is provided, the `text` field contains the schema-only JSON
string produced by the extraction turn (no extra prose). The `structured_output`
field contains the parsed JSON value for convenience.

`output_schema` may be provided as a wrapper object (shown above) or as a raw
JSON Schema object. The wrapper enables explicit `compat`/`format` settings.

### POST /sessions/{id}/messages

Continue an existing session.

Request:
```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000001",
  "prompt": "Follow-up message"
}
```

#### Request fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `session_id` | `string` | Yes | — | Session ID (must match the path `{id}`) |
| `prompt` | `string` | Yes | — | Follow-up prompt |
| `system_prompt` | `string \| null` | No | `null` | Override the system prompt |
| `model` | `string \| null` | No | from session | Model override for this turn |
| `provider` | `string \| null` | No | from session | Provider override for this turn |
| `max_tokens` | `u32 \| null` | No | from session | Max tokens override for this turn |
| `output_schema` | `OutputSchema \| null` | No | `null` | Structured output schema for this turn |
| `structured_output_retries` | `u32` | No | `2` | Max retries for structured output validation |
| `verbose` | `bool` | No | `false` | Enable verbose event logging |
| `host_mode` | `bool` | No | `false` | Enable host mode for this turn |
| `comms_name` | `string \| null` | No | from session | Agent name for comms |
| `hooks_override` | `HookRunOverrides \| null` | No | `null` | Run-scoped hook overrides |

Response: same shape as `POST /sessions`.

### GET /sessions/{id}

Fetch session metadata and usage.

Response:
```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000001",
  "created_at": "2025-01-15T10:30:00Z",
  "updated_at": "2025-01-15T10:31:00Z",
  "message_count": 4,
  "total_tokens": 500
}
```

### GET /sessions/{id}/events

Server-Sent Events (SSE) stream for real-time updates. Event types include:

- `session_loaded` — emitted on connect with session metadata
- `run_started`
- `turn_started`
- `text_delta`
- `tool_call_requested`
- `tool_execution_started`
- `tool_execution_completed`
- `turn_completed`
- `run_completed`
- `run_failed`
- `budget_warning`
- `retrying`
- `done` — emitted when the broadcast channel closes

### GET /health

Returns `"ok"` (HTTP 200). Use for liveness checks.

### GET /capabilities

Returns runtime capabilities with status resolved against config.

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
      "id": "shell",
      "description": "Shell command execution",
      "status": {"DisabledByPolicy": {"description": "Disabled by config"}}
    }
  ]
}
```

### Config endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET /config` | Read | Returns the current `Config` object |
| `PUT /config` | Replace | Sets the entire config (body: full `Config` JSON) |
| `PATCH /config` | Merge-patch | RFC 7396 merge patch on the config (body: partial JSON) |

---

## Error responses

All errors are returned as JSON with an HTTP status code:

```json
{
  "error": "Human-readable error message",
  "code": "ERROR_CODE"
}
```

| HTTP Status | Code | When |
|-------------|------|------|
| 400 | `BAD_REQUEST` | Invalid parameters, session ID mismatch, host_mode without comms |
| 404 | `NOT_FOUND` | Session does not exist |
| 500 | `CONFIGURATION_ERROR` | Config store read/write failure |
| 500 | `AGENT_ERROR` | LLM provider error, tool dispatch failure, agent loop error |
| 500 | `INTERNAL_ERROR` | Store initialization failure, unexpected server error |

Example error response (session not found):
```json
{
  "error": "Session not found: 01936f8a-7b2c-7000-8000-000000000099",
  "code": "NOT_FOUND"
}
```

Example error response (bad request):
```json
{
  "error": "Session ID mismatch: path=abc body=def",
  "code": "BAD_REQUEST"
}
```

---

## Notes

- Host mode requires `host_mode: true` and a `comms_name`. If `host_mode` is
  requested but the binary was not compiled with comms support, the server
  returns a `BAD_REQUEST` error.
- `hooks_override` allows per-request hook overrides including adding extra hook
  entries and disabling specific hooks by ID. See [Hooks](./hooks.md) for the
  `HookRunOverrides` schema.
