---
name: Session Management
description: Session persistence, resume patterns, event store replay, compaction tuning
requires_capabilities: [session_store]
---

# Session Management

Use session management guidance when resuming, inspecting, or reasoning about a
conversation's persisted runtime state.

## Operating Rules

- Resume with the `session_id` from a previous run. The runtime rebuilds the
  session from durable state when persistence is enabled.
- Treat event logs, catalogs, and projection files as derived. A current
  `Session` is domain-only conversation and metadata; the store issues the
  physical authority used to commit or recover it. Never repair state with ad
  hoc file edits.
- An archived session is lifecycle-terminal and is not an ordinary resume
  target. Durable-tail recovery can also hold or quarantine a resume when the
  persisted evidence is ambiguous; preserve the data and surface the typed
  failure instead of fabricating a repair.
- Compaction may replace old transcript detail with summaries. Use skills,
  memory, files, and WorkGraph to recover relevant context when needed.
- System messages are ordinary ordered transcript messages. They may repeat at
  turn boundaries; do not assume there is one mutable system-prompt slot.
- Host-injected context is a typed, separate user-channel message delivered
  immediately before the turn input. It is excluded from memory indexing and
  is not a free-form transcript role.
- Session persistence preserves conversation continuity. It does not replace
  WorkGraph for shared durable work or Schedule for future wakeups.

## Compaction

- Lower thresholds compact more often and reduce live context.
- Higher thresholds preserve more live context but cost more tokens.
- Compaction summaries are typed runtime-minted messages. Hosts and transcript
  rewrite callers cannot mint that role, and summaries are not re-indexed into
  memory.
- If compaction causes ambiguity, inspect durable artifacts or ask for the
  missing detail instead of inventing it.
