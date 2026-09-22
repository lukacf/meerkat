# 014 — Semantic Memory (Rust)

Give agents searchable semantic recall outside conversation history. The
runnable example uses `SimpleMemoryStore`, so its indexed facts live only for
the current process. Production realms can use `HnswMemoryStore` with SQLite
for durable session-scoped recall across turns and resumes/restarts of the same
session. Persistence does not automatically share indexed history with unrelated
sessions.

## Concepts
- `MemoryStore` trait — index and search interface
- `HnswMemoryStore` - production HNSW-based implementation (SQLite)
- `SimpleMemoryStore` - in-memory store used by this example
- `memory_search` - the built-in agent tool for semantic recall
- `MemoryStore::index_scoped()` - the Rust API used to index facts (also runs automatically during compaction)

## Architecture
```
App indexes a fact via MemoryStore::index_scoped("team uses Rust")
  -> fact enters the selected store (in-memory keyword matching in this demo)

Later: Agent calls the memory_search("what language for backend?") tool
  -> search finds "team uses Rust"
  -> result injected into agent context
```

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 014-semantic-memory --features jsonl-store,memory-store-session
```
