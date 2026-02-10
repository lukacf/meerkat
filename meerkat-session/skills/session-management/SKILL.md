---
name: Session Management
description: Session persistence, resume patterns, event store replay, compaction tuning
requires_capabilities: [session_store]
---

# Session Management

## Session Persistence

Sessions are persisted via the event store (redb backend). The `.rkat/sessions/`
directory contains derived projection files — NOT canonical state. Deleting
them and replaying from the event store produces identical content.

## Resume Patterns

To resume a session:
1. Use the `session_id` from a previous run
2. The session's full history is replayed from the event store
3. New turns continue from where the previous run left off

## Compaction Threshold Tuning

Compaction triggers when token usage exceeds the configured threshold:
- Lower threshold = more frequent compaction, smaller context
- Higher threshold = less compaction, richer context but higher cost
- Default threshold balances cost and context quality

## Event Store Replay

The event store records every mutation. Full replay produces:
- Reconstructed session state
- Materialized `.rkat/sessions/` projection files
- Verified consistency via the SessionProjector
