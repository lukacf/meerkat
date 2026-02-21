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
| POST | `/sessions/:id/event` | Push external event (webhook) |
| POST | `/comms/send` | Send comms message |
| GET | `/comms/peers` | List comms peers |
| GET | `/config` | Read runtime config |
| GET | `/capabilities` | List capabilities |

## Setup
```bash
# Terminal 1: Start REST server (default port 8080, configured in [rest] section)
ANTHROPIC_API_KEY=sk-... rkat-rest

# Terminal 2: Run the example
python main.py
```
