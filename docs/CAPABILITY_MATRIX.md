# Capability Matrix

Meerkat is modular. Every feature is opt-in. This matrix shows what works in each build profile.

## Build Profiles

| Profile | Cargo Features | Use Case |
|---------|---------------|----------|
| **Minimal** | `--no-default-features` | Ephemeral agent, no persistence, no compaction |
| **Persistent** | `session-store` | Sessions survive restart, redb-backed storage |
| **Compacting** | `session-compaction` | Auto-compact long conversations |
| **Full** | `session-store,session-compaction` | Persistent + compacting |

## Capability Behavior

| Operation | Minimal | Persistent | Compacting | Full |
|-----------|---------|-----------|------------|------|
| `create_session` | Works | Works | Works | Works |
| `start_turn` | Works | Works + snapshot saved | Works | Works + snapshot saved |
| `interrupt` | Works | Works | Works | Works |
| `read` (live session) | Works | Works | Works | Works |
| `read` (archived) | `SESSION_NOT_FOUND` | Falls back to store | `SESSION_NOT_FOUND` | Falls back to store |
| `list` | Live sessions only | Live + stored sessions | Live sessions only | Live + stored sessions |
| `archive` | Removes from memory | Removes from memory, snapshot kept | Removes from memory | Removes + snapshot kept |
| Auto-compaction | No-op | No-op | Triggers at threshold | Triggers at threshold |

## Error Codes

Deterministic error codes across all surfaces:

| `SessionError` | Code | JSON-RPC | REST | MCP | CLI |
|----------------|------|----------|------|-----|-----|
| `NotFound` | `SESSION_NOT_FOUND` | -32001 | 404 | tool error | exit 1 |
| `Busy` | `SESSION_BUSY` | -32002 | 409 | tool error | exit 1 |
| `PersistenceDisabled` | `SESSION_PERSISTENCE_DISABLED` | -32003 | 501 | tool error | stderr |
| `CompactionDisabled` | `SESSION_COMPACTION_DISABLED` | -32004 | 501 | tool error | stderr |
| `NotRunning` | `SESSION_NOT_RUNNING` | -32005 | 409 | tool error | exit 1 |
| `Agent` | `AGENT_ERROR` | -32000 | 500 | tool error | exit 1 |

## Pick Only What You Need

```toml
# Minimal: just the agent loop
[dependencies]
meerkat = { version = "0.1", default-features = false, features = ["anthropic"] }

# Add persistence
meerkat = { version = "0.1", default-features = false, features = ["anthropic", "session-store"] }

# Add compaction
meerkat = { version = "0.1", default-features = false, features = ["anthropic", "session-store", "session-compaction"] }

# Kitchen sink
meerkat = { version = "0.1", features = ["all-providers", "session-store", "session-compaction", "comms", "mcp"] }
```

## Compaction Behavior

When `session-compaction` is enabled:

- **Trigger**: `last_input_tokens >= threshold` OR `estimated_history_tokens >= threshold`
- **Guards**: Never on first turn. Minimum 3 turns between compactions.
- **Failure**: Non-fatal. `CompactionFailed` event emitted, agent continues with uncompacted history.
- **Budget**: Compaction LLM call draws from the same token budget as regular turns.

Events emitted: `CompactionStarted`, `CompactionCompleted`, `CompactionFailed`.

## Concurrency

- At most one turn runs per session at a time.
- Second `start_turn` while one is in-flight returns `SESSION_BUSY`.
- `interrupt()` cancels the in-flight turn.
- `read()` and `list()` are non-blocking.
- No queueing: callers retry on `Busy`.

## Durability

- **Ephemeral**: Process death loses all state.
- **Persistent**: Snapshot saved AFTER turn completes. Crash mid-turn loses the in-flight turn.
- `.rkat/sessions/` files are derived (projected) output, not canonical source.
