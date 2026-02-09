# Session Operational Contracts

## SessionService Lifecycle

All surfaces (SDK, CLI, REST, JSON-RPC, MCP) route through `SessionService`. The contract is identical regardless of transport.

### Session Creation

`create_session(req)` builds an agent and runs the first turn.

- Always works in every build profile.
- Returns `RunResult` with the session ID for subsequent turns.
- The session is live (in-memory) until archived.

### Turn Execution

`start_turn(id, req)` runs a new turn on an existing session.

- **Busy check**: Uses atomic compare-and-swap on the turn lock. Exactly one caller wins; others get `SESSION_BUSY`.
- **Sequential execution**: Turns run one at a time per session. No queueing — callers retry on Busy.
- **Event streaming**: If `event_tx` is provided, events are forwarded during the turn.

### Interruption

`interrupt(id)` cancels an in-flight turn.

- Checks `turn_lock` atomically. If no turn is running, returns `SESSION_NOT_RUNNING`.
- Sends cancel signal to the session task's agent.
- The interrupted turn returns `AgentError::Cancelled`.

### Reading Session State

`read(id)` returns `SessionView` (state + billing separated).

- For live sessions: queries the session task for a snapshot.
- For persistent sessions (when `session-store` is enabled): falls back to the store.
- Non-blocking — can run concurrently with a turn in progress.

### Listing Sessions

`list(query)` returns session summaries.

- Live sessions return real-time data from `SessionSummaryCache` (updated after each turn via watch channel).
- Persistent mode merges live + stored sessions (deduplicated by ID).
- Pagination via `offset` and `limit`.

### Archiving

`archive(id)` removes a session from the live map and shuts down its task.

- In persistent mode, the snapshot remains in the store for historical queries.
- Archiving during an in-flight turn sends shutdown after the turn completes (graceful).

## Compaction

Context compaction is an optional, non-fatal session capability.

### Trigger Rules

1. `last_input_tokens >= threshold` OR `estimated_history_tokens >= threshold`
2. Never on the first turn (`current_turn == 0`)
3. Blocked if `current_turn - last_compaction_turn < min_turns_between_compactions`

### Compaction Flow

1. Emit `CompactionStarted`
2. Clone messages, append compaction prompt as User message
3. Call LLM with empty tools and `max_summary_tokens`
4. On failure: emit `CompactionFailed`, continue with uncompacted history (non-fatal)
5. Extract summary text from response
6. Call `compactor.rebuild_history()` to produce new messages + discarded list
7. Replace session messages
8. Record compaction usage in budget
9. Emit `CompactionCompleted`

### Rebuild Algorithm

1. Preserve `Message::System` verbatim (extracted from messages, single source of truth)
2. Inject summary as `Message::User` with prefix
3. Retain recent complete turns (User -> Assistant -> ToolResults sequences) per `recent_turn_budget`
4. Everything else goes to `discarded` (for future memory indexing)

### Budget

- Compaction draws from the same `Budget` as regular turns
- If budget exhausted before compaction, compaction is skipped
- `max_summary_tokens` caps the summary LLM response

## Durability Guarantees

### Ephemeral Mode

No durability. Process death loses all session state.

### Persistent Mode (`session-store`)

- Snapshot saved AFTER a turn completes (not mid-turn)
- Crash mid-turn loses the in-flight turn but preserves up to the last completed turn
- redb provides ACID transactions with WAL

### Compaction Durability

- Compacted session is saved as a regular snapshot
- Pre-compaction history is NOT recoverable (unless indexed into MemoryStore)

## Data Governance

- No sensitive data filtering in compaction summaries
- Event store records all `AgentEvent` variants including `ToolCallRequested` (contains tool arguments)
- No encryption at rest (redb files are plaintext)
- `.rkat/` projection files are derived — deleting and replaying produces identical content
