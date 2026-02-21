# 022 — REST API Client (Python)

Interact with Meerkat via standard HTTP requests. No SDK required — any
language or tool that speaks HTTP can integrate.

## Concepts
- REST API server (`rkat rest`)
- Session lifecycle over HTTP
- Server-Sent Events (SSE) for streaming
- Comms webhooks for external message injection

## API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| POST | `/sessions` | Create session + first turn |
| POST | `/sessions/:id/messages` | Continue session |
| GET | `/sessions` | List sessions |
| GET | `/sessions/:id` | Read session state |
| POST | `/sessions/:id/archive` | Archive session |
| GET | `/sessions/:id/events` | SSE event stream |

## Setup
```bash
# Terminal 1: Start REST server
ANTHROPIC_API_KEY=sk-... rkat rest --port 8000

# Terminal 2: Run the example
python main.py
```
