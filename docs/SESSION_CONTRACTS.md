# Session Operational Contracts

## SessionService Lifecycle

All surfaces (CLI, REST, JSON-RPC, MCP Server) route through `SessionService` for the full
session lifecycle. The contract is identical regardless of transport.

```
CLI ─────────┐
REST ────────┤
MCP Server ──┤──► SessionService ──► AgentFactory::build_agent()
JSON-RPC ────┘
```

### Agent Construction

Agent construction is fully centralized in `AgentFactory::build_agent()`. Surface crates
contain zero `AgentBuilder::new()` calls. The `FactoryAgentBuilder` bridges `AgentFactory` into
the `SessionAgentBuilder` trait used by `EphemeralSessionService`.

Surfaces that need per-request configuration (e.g. `override_builtins`, `override_shell`,
specific tools) stage a full `AgentBuildConfig` into the `build_config_slot` **before**
calling `create_session()`:

```rust
// Surface code (CLI, REST, etc.)
let build_config = AgentBuildConfig::new(model)
    .with_system_prompt(system_prompt)
    .with_override_builtins(override_builtins)
    .with_override_shell(override_shell);

factory_builder.stage_config(build_config).await;
let result = service.create_session(req).await?;
```

If no config is staged, `FactoryAgentBuilder` falls back to building a minimal config from the
`CreateSessionRequest` fields.

### Session Creation

`create_session(req)` builds an agent and runs the first turn.

- Always works in every build profile.
- Returns `RunResult` with the session ID for subsequent turns.
- The session is live (in-memory) until archived.
- Supports `host_mode`: process the prompt then stay alive for comms messages.

### Turn Execution

`start_turn(id, req)` runs a new turn on an existing session.

- **Busy check**: Uses atomic compare-and-swap on the turn lock. Exactly one caller wins; others get `SESSION_BUSY`.
- **Sequential execution**: Turns run one at a time per session. No queueing -- callers retry on Busy.
- **Event streaming**: If `event_tx` is provided, events are forwarded during the turn.
- **Host mode**: When `host_mode: true`, the agent processes the prompt then stays alive
  waiting for comms messages (peer-to-peer inter-agent communication).

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
- `.rkat/sessions/` files are derived projection output (materialized by `SessionProjector`), NOT
  canonical state. Deleting them and replaying from the event store produces identical content.

## See Also

- [CAPABILITY_MATRIX.md](./CAPABILITY_MATRIX.md) -- build profiles, error codes, feature behavior
- [architecture.md](./architecture.md) -- crate structure and agent loop details
