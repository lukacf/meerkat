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
App indexes a fact via MemoryStore::index_scoped(MemoryIndexRequest)
  -> fact enters the selected store (in-memory keyword matching in this demo)

Later: Agent calls the memory_search("what language for backend?") tool
  -> search finds "team uses Rust"
  -> result injected into agent context
```

Indexing and search are scoped to the same session owner. For production
factory wiring, enable `AgentFactory::new(store_path).memory(true)`; its HNSW
store is rooted at `<factory store_path>/memory/`, not a fixed `.rkat/memory/`.

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 014-semantic-memory --features jsonl-store,memory-store-session
```

## Deterministic behavior test

```bash
./scripts/repo-cargo test -p meerkat --example 014-semantic-memory \
  --features jsonl-store,memory-store-session
```

The test invokes the same client-injected async body as `main`, using the real
agent, memory store, session scope and `memory_search` dispatcher. A scripted
client requests searches on both turns and derives its replies from the actual
tool results; the test requires all five indexed facts, accumulated history,
and scoped directory cleanup. It does not use external transport or test
credentials. Production still constructs the normal Anthropic client and uses
`claude-sonnet-4-6`.
