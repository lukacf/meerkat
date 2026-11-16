# 022 — REST API Client (Python)

Interact with Meerkat via standard HTTP requests. No SDK required — any
language or tool that speaks HTTP can integrate.

Fatal requests exit nonzero. HTTP error responses report the server's status
and body rather than being mislabeled as connection failures; connection failures
retain startup guidance. The initial failure path still prints the API reference.
See the [offline regression checks](../003-hello-meerkat-ts/README.md#offline-regression-checks)
for local success, HTTP 503, and connection-refusal subprocess tests.

## Concepts
- REST API server (`rkat-rest`)
- Session lifecycle over HTTP
- Server-Sent Events (SSE) for streaming

## API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| POST | `/sessions` | Create session + first turn |
| POST | `/sessions/:id/messages` | Continue session |
| GET | `/sessions/:id` | Read session state |
| GET | `/sessions/:id/events` | SSE event stream |
| POST | `/sessions/:id/external-events` | Queue external event (webhook) |
| POST | `/comms/send` | Send comms message |
| GET | `/comms/peers` | List comms peers |
| GET | `/config` | Read runtime config |
| GET | `/capabilities` | List capabilities |

## Setup
```bash
# From the repository root, build the REST server first:
./scripts/repo-cargo build -p meerkat-rest --bin rkat-rest
export RKAT_REST="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rest"

# Terminal 1: Start REST server (default port 8080, configured in [rest] section)
ANTHROPIC_API_KEY=sk-... "$RKAT_REST"

# Terminal 2: Run the example
python3 examples/022-rest-api-client-py/main.py
```

### On a fresh machine

This example creates its session with a `system_prompt`, so it runs as
shown. A request of your own without one does not, where the default context
directory of the server does not exist (starting the server does not create
it):

```bash
curl -s -i -X POST http://localhost:8080/sessions -H 'content-type: application/json' \
  -d '{"prompt": "Say hi in three words.", "model": "claude-sonnet-5-5"}'
```

answers `HTTP/1.1 500 Internal Server Error` with the body

```json
{"error":"agent error: Build error: Config error: project AGENTS.md '<data directory>/meerkat/rest' is not readable: No such file or directory (os error 2)","code":"AGENT_ERROR"}
```

on every call until the directory exists. `<data directory>` is the data
directory of the platform: on Linux `~/.local/share`, or `$XDG_DATA_HOME` when
that variable is set. This was run on the released `rkat-rest` 0.9.14 and
0.9.15 on Linux (and it is the same on older releases back to at least
0.8.28). Either of these cures it:

```bash
# Independent of the platform and of where its data directory is: start the
# server with a context root that exists (an empty directory is enough):
mkdir -p "$HOME/rkat-rest-context" && "$RKAT_REST" --context-root "$HOME/rkat-rest-context"

# Or create the directory the message names; a running server picks it up,
# no restart. On Linux by default:
mkdir -p ~/.local/share/meerkat/rest
```
