# 007 — Multi-Turn Sessions (Python SDK)

Build a multi-turn coding assistant that remembers context across turns.
Shows the complete session lifecycle: create, continue, read, list, archive.

## Concepts
- `create_session()` — first turn creates the session
- `start_turn()` — subsequent turns continue the conversation
- `start_turn_streaming()` — stream events in real-time
- `read_session()` — inspect session state (message count, tokens)
- `list_sessions()` / `archive_session()` — session management

## Session Lifecycle
```
create_session() → session_id
  ↓
start_turn(session_id, ...) → continues conversation
  ↓ (repeat)
read_session(session_id) → inspect state
  ↓
archive_session(session_id) → clean up
```

## Run
```bash
ANTHROPIC_API_KEY=sk-... python main.py
```
