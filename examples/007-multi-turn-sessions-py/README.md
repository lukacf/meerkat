# 007 — Multi-Turn Sessions (Python SDK)

Build a multi-turn coding assistant that remembers context across turns.
Shows the complete session lifecycle: create, continue, read, list, archive.

## Concepts
- `create_session()` — first turn creates the session, returns `Session`
- `session.turn()` — subsequent turns continue the conversation
- `session.stream()` — stream events in real-time
- `client.read_session()` — inspect session state (message count, tokens)
- `client.list_sessions()` / `session.archive()` — session management

## Session Lifecycle
```
session = create_session(prompt)  → Session
  ↓
session.turn(prompt)  → continues conversation
  ↓ (repeat)
client.read_session(session.id)  → inspect state
  ↓
session.archive()  → clean up
```

## Run
```bash
ANTHROPIC_API_KEY=sk-... python main.py
```
