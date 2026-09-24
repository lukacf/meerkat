---
name: Memory Retrieval
description: How semantic memory works with compaction and the MemoryStore trait
requires_capabilities: [memory_store]
---

# Memory Retrieval

Use memory for knowledge retrieval and long-horizon context recall. Memory is
not live work state, not a scheduler, and not a commitment graph.

## Operating Rules

- Memory is scoped to the current session: it recalls turns compacted away
  earlier in this session, including turns compacted before a restart or
  resume of the same session.
- Use `memory_search` when compacted context from earlier in this session
  would materially improve the current answer. It accepts `query` and an
  optional `limit` (default 5, capped at 20).
- Treat matches as recalled evidence with similarity scores, not as current
  truth. Verify against live stores, files, APIs, or WorkGraph when correctness
  matters.
- Search results carry source message ranges when available. Use that typed
  provenance rather than guessing which later turn caused the compaction.
- Compaction summaries and host-injected context are intentionally excluded
  from memory indexing. Memory indexes eligible content discarded by
  compaction, not every message the model has seen.
- Use WorkGraph for pending, blocked, claimed, or terminal work.
- Use Schedule for future wakeups and recurrence.
- Use builtin tasks for private scratch tracking.

## Scores

- Scores range from `0.0` to `1.0`.
- Higher scores are more similar, not automatically more authoritative.
- Prefer several corroborating matches over one weak match.
