---
name: Memory Retrieval
description: How semantic memory works with compaction and the MemoryStore trait
requires_capabilities: [memory_store]
---

# Memory Retrieval

## How It Works

When context compaction discards conversation history, the discarded messages
are indexed into semantic memory. This allows retrieval of past context that
was compacted away.

## The MemoryStore Trait

Two key operations:
- **index**: Store text with metadata (session ID, turn number, timestamp)
- **search**: Find similar text using semantic similarity

## Understanding Similarity Scores

- Scores range from 0.0 (no match) to 1.0 (exact match)
- Typical useful matches are above 0.7
- Results are ranked by similarity

## Session vs Cross-Session Memory

- Memory is indexed per-session by default
- Cross-session search enables knowledge transfer between runs
- Session ID in metadata allows filtering
