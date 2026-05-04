# 022 — REST API Client (Python)

Interact with Meerkat via standard HTTP requests. No SDK required — any
language or tool that speaks HTTP can integrate.

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
python3 main.py
```
