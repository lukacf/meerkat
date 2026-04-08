# 015 — Session Persistence (Rust)

Persist sessions to disk so agents survive restarts. Shows three storage
backends and the full session lifecycle.

## Concepts
- `JsonlStore` — file-based JSONL storage (simple, human-readable)
- `MemoryStore` — in-memory (for tests and ephemeral use)
- `SqliteSessionStore` — embedded SQLite database (production)
- `SessionFilter` — query sessions by date, limit, offset
- Session save/load roundtrip

## Storage Architecture
```
Agent ←→ StoreAdapter ←→ SessionStore trait
                              ↓
                    ┌─────────┼─────────┐
                    │         │         │
                 JsonlStore  Memory  SQLite
                 (files)    (RAM)   (sqlite DB)
```

## Run
```bash
# This is a reference implementation. For runnable examples, see meerkat/examples/.
```
