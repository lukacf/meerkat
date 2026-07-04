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
- Search memory when compacted context from earlier in this session would
  materially improve the current answer.
- Treat matches as recalled evidence with similarity scores, not as current
  truth. Verify against live stores, files, APIs, or WorkGraph when correctness
  matters.
- Use WorkGraph for pending, blocked, claimed, or terminal work.
- Use Schedule for future wakeups and recurrence.
- Use builtin tasks for private scratch tracking.

## Scores

- Scores range from `0.0` to `1.0`.
- Higher scores are more similar, not automatically more authoritative.
- Prefer several corroborating matches over one weak match.
