# 014 — Semantic Memory (Rust)

Give agents persistent, searchable memory that spans across sessions.
Unlike conversation history, semantic memory uses similarity search
to find relevant past knowledge.

## Concepts
- `MemoryStore` trait — index and search interface
- `HnswMemoryStore` — production HNSW-based implementation (SQLite)
- `SimpleMemoryStore` — in-memory for tests
- `memory_search` — the built-in agent tool for semantic recall
- `MemoryStore::index_scoped()` — the Rust API used to index facts (also runs automatically during compaction)

## Architecture
```
App indexes a fact via MemoryStore::index_scoped("team uses Rust")
  → text indexed in HNSW vector space

Later: Agent calls the memory_search("what language for backend?") tool
  → similarity search finds "team uses Rust"
  → result injected into agent context
```

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 014-semantic-memory --features jsonl-store,memory-store-session
```
