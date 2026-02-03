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
model = "claude-3-7-sonnet-20250219"
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
  "model": "claude-3-7-sonnet-20250219",
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
  "comms_name": null
}
```

Response:
```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000001",
  "text": "Response text",
  "turns": 1,
  "tool_calls": 0,
  "usage": {
    "input_tokens": 50,
    "output_tokens": 200
  },
  "stop_reason": "end_turn",
  "structured_output": {"answer": "example"},
  "schema_warnings": [
    {
      "provider": "gemini",
      "path": "/properties/choice/oneOf",
      "message": "Removed unsupported keyword 'oneOf'"
    }
  ]
}
```

### POST /sessions/{id}/messages

Continue an existing session.

```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000001",
  "prompt": "Follow-up message"
}
```

### GET /sessions/{id}

Fetch session metadata and usage.

### GET /sessions/{id}/events

Server-Sent Events (SSE) stream for real-time updates. Event types include:

- `run_started`
- `turn_started`
- `text_delta`
- `tool_call_requested`
- `tool_result_received`
- `turn_completed`
- `run_completed`
- `run_failed`

---

## Notes

- When `output_schema` is provided, the `text` field in the response is the
  schema-only JSON string produced by the extraction turn (no extra text).
- `output_schema` may be provided as a wrapper object (shown above) or as a raw
  JSON Schema object. The wrapper enables explicit compat/format settings.
- Host mode requires `host_mode: true` and a `comms_name`.
