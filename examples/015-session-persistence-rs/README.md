# 015 — Session Persistence (Rust)

Persist sessions to disk so agents survive restarts. Shows three storage
backends and the full session lifecycle.

## Concepts
- `JsonlStore` — file-based JSONL storage (simple, human-readable)
- `MemoryStore` — in-memory (for tests and ephemeral use)
- `RedbSessionStore` — embedded B+tree database (production)
- `SessionFilter` — query sessions by date, limit, offset
- Session save/load roundtrip

## Storage Architecture
```
Agent ←→ StoreAdapter ←→ SessionStore trait
                              ↓
                    ┌─────────┼─────────┐
                    │         │         │
                 JsonlStore  Memory  RedbStore
                 (files)    (RAM)   (redb DB)
```

## Run
```bash
ANTHROPIC_API_KEY=sk-... cargo run --example 015_session_persistence
```
