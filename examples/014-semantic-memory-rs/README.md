# 014 — Semantic Memory (Rust)

Give agents persistent, searchable memory that spans across sessions.
Unlike conversation history, semantic memory uses similarity search
to find relevant past knowledge.

## Concepts
- `MemoryStore` trait — index and search interface
- `HnswMemoryStore` — production HNSW-based implementation (redb)
- `SimpleMemoryStore` — in-memory for tests
- `memory_store` / `memory_search` — built-in agent tools

## Architecture
```
Agent calls memory_store("team uses Rust")
  → text indexed in HNSW vector space

Later: Agent calls memory_search("what language for backend?")
  → similarity search finds "team uses Rust"
  → result injected into agent context
```

## Run
```bash
ANTHROPIC_API_KEY=sk-... cargo run --example 014_semantic_memory
```
