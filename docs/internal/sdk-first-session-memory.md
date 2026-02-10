# SDK-First Persistence and Memory Plan (with Post-SessionService Context Compaction)

## Summary
Implement a unified `SessionService` contract used by SDK, CLI, MCP, REST, and JSON-RPC, with persistence as an optional capability and compaction as an optional session capability.

When persistence is disabled, all interfaces run in **ephemeral session mode** (session IDs still exist for active runs), and persistence operations return a consistent capability error.

When compaction is enabled, long-running sessions are compacted provider-agnostically before context overflow, with deterministic events emitted across all interfaces.

Chosen defaults:
- 3 canonical planes: `SessionStore`, `EventStore`, `MemoryStore`.
- `.rkat` kept as async projection.
- local embeddings v1 with `hnswlib-rs`.
- `session-store`, `memory-store`, and `session-compaction` are optional capabilities.
- API shapes stay stable across interfaces; missing capability returns stable error codes.

## Public API / Capability Model

### Cargo features (final)
- `session-store` (optional): enables persistent `SessionStore + EventStore`.
- `memory-store` (optional): enables semantic project memory.
- `session-compaction` (optional): enables context compaction via `Compactor` + `DefaultCompactor`.
- default profile for CLI/surfaces can include all; embedders can disable any subset.

### Core service interface (always compiled)
```rust
pub trait SessionService: Send + Sync {
    async fn create_session(&self, req: CreateSession) -> Result<SessionCreated, SessionError>;
    async fn start_turn(&self, req: StartTurn) -> Result<TurnResult, SessionError>;
    async fn resume(&self, session_id: &SessionId, req: ResumeTurn) -> Result<TurnResult, SessionError>;
    async fn read(&self, session_id: &SessionId) -> Result<SessionView, SessionError>;
    async fn list(&self, q: SessionQuery) -> Result<Vec<SessionSummary>, SessionError>;
    async fn archive(&self, session_id: &SessionId) -> Result<(), SessionError>;
}
```

### Compaction contracts (always compiled in core)
- `Compactor` trait and `CompactionContext`/`CompactionResult` types live in `meerkat-core`.
- Agent/loop integration is generic and trait-driven.
- Default implementation lives in `meerkat-session` behind `session-compaction`.

### Implementations
- `EphemeralSessionService` (always): in-process runtime map, no durable resume/list/archive.
- `PersistentSessionService` (`session-store`): backed by `SessionStore + EventStore`.
- `DefaultCompactor` (`session-compaction`, in `meerkat-session`): summary-based rebuild with guardrails.

### Canonical capability errors
Introduce stable error variants/codes:
- `SessionError::PersistenceDisabled` → `SESSION_PERSISTENCE_DISABLED`
- `SessionError::CompactionDisabled` → `SESSION_COMPACTION_DISABLED` (only when explicitly requested/configured without capability)

## Cross-Interface Behavior Contract (Mandatory)

### SDK
- `create_session`, `start_turn`: always work.
- `resume/read/list/archive`: return `PersistenceDisabled` if no `session-store`.
- compaction:
  - if enabled and configured, compaction runs automatically.
  - if compaction config is requested without feature, return `CompactionDisabled`.

### CLI
- `run`: always works.
- `resume`, `sessions list/show/delete`: fail fast with clear message when no `session-store`:
  - `Session persistence is disabled (build without 'session-store').`
- compaction config flag (future phase): if used without feature, fail fast with:
  - `Session compaction is disabled (build without 'session-compaction').`

### JSON-RPC
- `session/create`, `turn/start`: always work.
- `session/read`, `session/list`, `session/archive`, resume-like operations:
  - JSON-RPC error with code `SESSION_PERSISTENCE_DISABLED` when feature absent.
- compaction config requests without feature:
  - JSON-RPC error with code `SESSION_COMPACTION_DISABLED`.

### REST
- create/turn endpoints available.
- session lifecycle endpoints return deterministic HTTP + body when no persistence:
  - e.g. `501 Not Implemented` + `{ "code": "SESSION_PERSISTENCE_DISABLED", ... }`.
- compaction config requests without feature:
  - deterministic HTTP + `{ "code": "SESSION_COMPACTION_DISABLED", ... }`.

### MCP
- tools requiring persisted lookup/history return tool error with `SESSION_PERSISTENCE_DISABLED`.
- non-persistent execution tools still function.
- compaction config requests without feature return tool error `SESSION_COMPACTION_DISABLED`.

### Event parity (all interfaces)
When compaction is enabled and triggers:
- emit `CompactionStarted`
- emit `CompactionCompleted` on success
- emit `CompactionFailed` on non-fatal failure (agent continues uncompacted)

## Storage Architecture (When `session-store` Enabled)

### Canonical planes
1. `SessionStore`: typed session snapshot/checkpoint metadata.
2. `EventStore`: append-only session event log with monotonic seq.
3. `MemoryStore`: semantic project/session memory (optional via `memory-store`).

### Backends
- `redb` for session/event metadata/indexes.
- append-only event files for logs.
- `hnswlib-rs` + `redb` metadata for memory search.
- no SQLite.

### `.rkat` projection
- Async projector consumes `EventStore` and materializes CLI-friendly files.
- `.rkat` is derived output, never canonical source for resume.

### Compaction + memory alignment
- `CompactionResult` always returns `discarded` messages.
- initial compaction phase drops `discarded`.
- when `memory-store` is enabled, discarded content can be indexed before dropping (no API change required).

## Implementation Phases

1. **Capability scaffolding**
- Add `session-store`, `memory-store`, and `session-compaction` features.
- Add `SessionService` trait and `SessionError::PersistenceDisabled`.
- Reserve `SessionError::CompactionDisabled`.
- Add transport error mapping tables for all surfaces.

2. **Ephemeral baseline (always-on)**
- Implement `EphemeralSessionService`.
- Route SDK/CLI/MCP/REST/JSON-RPC through service trait.
- Ensure consistent no-persistence behavior contract everywhere.

3. **Persistent backend**
- Implement `RedbSessionStore` + `RedbEventStore`.
- Implement `PersistentSessionService` under `session-store`.
- Wire factory/runtime selection based on feature/capability.

4. **Projector and `.rkat` views**
- Add async projector + checkpoints.
- Materialize `.rkat/sessions/*` and project indexes.

5. **Post-SessionService context compaction**
- Create `meerkat-session` crate and place `DefaultCompactor` there.
- Add `Compactor`, `CompactionContext`, `CompactionResult` in `meerkat-core`.
- Wire agent loop trigger via `CompactionContext`:
  - proactive trigger from last input tokens + estimated history pressure
  - loop guard (`min_turns_between_compactions`)
- Rebuild history with summary + recent **turns** (not only user messages).
- Record compaction usage and emit `CompactionStarted`/`CompactionCompleted`/`CompactionFailed`.
- Wire facade/factory behind `session-compaction`.
- MVP configuration surface: SDK `AgentBuildConfig` only.
- Implementation details are defined in the self-contained "Phase 5 Detailed Spec" section below.

6. **Memory store**
- Implement `HnswMemoryStore` under `memory-store`.
- Add `search_project_history` integration to tools/interfaces.
- Integrate compaction discarded-message indexing into `MemoryStore` when present.

7. **Surface hardening**
- CLI command UX/messages finalized.
- RPC/REST/MCP error payloads aligned with one shared code enum.
- Event payload parity tests for compaction events.
- docs updated with capability matrix.

## Phase 5 Detailed Spec (Self-Contained)

### Prompts (canonical text)

Summarization prompt:
```text
You are performing a CONTEXT COMPACTION. Your job is to create a handoff summary so work can continue seamlessly.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences discovered
- What remains to be done (clear next steps)
- Any critical data, file paths, examples, or references needed to continue
- Tool call patterns that worked or failed

Be concise and structured. Prioritize information the next context needs to act, not narrate.
```

Summary prefix injected into rebuilt history:
```text
[Context compacted] A previous context produced the following summary of work so far. The current tool and session state is preserved. Use this summary to continue without duplicating work:
```

### Core trait shape

`meerkat-core` defines the capability contract; `meerkat-session` implements it.

```rust
pub trait Compactor: Send + Sync {
    fn should_compact(&self, ctx: &CompactionContext) -> bool;
    fn compaction_prompt(&self) -> &str;
    fn max_summary_tokens(&self) -> u32;
    fn rebuild_history(
        &self,
        system_prompt: Option<&str>,
        messages: &[Message],
        summary: &str,
    ) -> CompactionResult;
}
```

`max_summary_tokens` is mandatory to cap compaction budget usage.

### Trigger context and loop/failure semantics

- `CompactionContext` includes `last_input_tokens`, `message_count`, `estimated_history_tokens`, `last_compaction_turn`, `current_turn`.
- Trigger is proactive:
  - threshold by `last_input_tokens`
  - threshold by `estimated_history_tokens` (covers stale token telemetry or resume paths)
- Loop guard:
  - compaction is blocked when `current_turn - last_compaction_turn < min_turns_between_compactions`.
- Failure behavior:
  - compaction is best-effort and non-fatal.
  - emit `CompactionFailed`.
  - continue agent loop with uncompacted history.

### `estimate_tokens` helper (baseline)

Use a deterministic coarse estimator (JSON bytes / 4):

```rust
fn estimate_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|m| serde_json::to_string(m).map(|s| s.len()).unwrap_or(0))
        .sum::<usize>() as u64
        / 4
}
```

### `rebuild_history` algorithm (canonical)

1. Preserve `Message::System` verbatim if present.
2. Add one `Message::User` containing `SUMMARY_PREFIX + summary`.
3. Walk original messages from newest to oldest, collecting complete recent turns until `recent_turn_budget` is exhausted.
4. A turn means contiguous `User -> Assistant/BlockAssistant -> ToolResults` fragments that belong to one interaction.
5. Insert retained turns back in chronological order after the summary.
6. Everything not retained goes to `CompactionResult.discarded` in original order.

Constraint:
- Keep recent **turns**, not just user messages, so tool-call/result continuity remains intact.

### `run_compaction` flow (agent loop)

`Agent::run_compaction(...)` executes this sequence:

1. Emit `CompactionStarted { input_tokens, estimated_history_tokens, message_count }`.
2. Read `compactor.compaction_prompt()`.
3. Clone current messages and append the compaction prompt as a `User` message.
4. Call `client.stream_response(...)` with empty tools and `max_summary_tokens()`.
5. If summarization call fails: emit `CompactionFailed`, return without mutating session.
6. Extract summary text from response blocks.
7. Detect current system prompt (if any).
8. Call `compactor.rebuild_history(system_prompt, messages, summary)`.
9. Replace session message list with compacted messages.
10. Keep `discarded` available for future memory indexing, then drop in MVP.
11. Reset `last_input_tokens` to `0`.
12. Set `last_compaction_turn = Some(current_turn)`.
13. Record compaction call usage against budget and session usage accounting.
14. Emit `CompactionCompleted { summary_tokens, messages_before, messages_after }`.

### File-level implementation map

- `Cargo.toml` (workspace):
  - add `meerkat-session` member.
- `meerkat-session/` (new crate):
  - `src/compactor.rs` with `CompactionConfig`, `DefaultCompactor`, prompt constants, rebuild logic.
  - `src/lib.rs` exports.
- `meerkat-core/src/compact.rs` (new):
  - `Compactor`, `CompactionContext`, `CompactionResult`.
- `meerkat-core/src/agent/compact.rs` (new):
  - `run_compaction`, `estimate_tokens`.
- `meerkat-core/src/agent.rs`:
  - add `compactor`, `last_input_tokens`, `last_compaction_turn`.
- `meerkat-core/src/agent/builder.rs`:
  - builder field and `.compactor(...)`.
- `meerkat-core/src/agent/state.rs`:
  - trigger check in `CallingLlm`, update `last_input_tokens` after LLM calls.
- `meerkat-core/src/event.rs`:
  - `CompactionStarted`, `CompactionCompleted`, `CompactionFailed` + formatting + serde tests.
- `meerkat-core/src/lib.rs`:
  - export compact module/types.
- `meerkat/Cargo.toml`:
  - optional dep `meerkat-session`, feature `session-compaction`.
- `meerkat/src/factory.rs`:
  - wire `DefaultCompactor` when compaction config is present and feature enabled.
- `meerkat/src/lib.rs`:
  - conditional public re-export of compaction config/types.

## Test Cases and Scenarios

### Capability matrix
- Build A: `--no-default-features` (no persistence, no memory, no compaction).
- Build B: `--features session-store`.
- Build C: `--features memory-store`.
- Build D: `--features session-compaction`.
- Build E: `--features session-store,memory-store`.
- Build F: `--features session-store,memory-store,session-compaction`.

### Contract tests (must pass across interfaces)
1. Create + run turn works in all builds.
2. In no-session-store build, resume/read/list/archive return `SESSION_PERSISTENCE_DISABLED`.
3. In session-store build, resume/list/archive works across process restart.
4. Error payload/code parity across SDK/CLI/MCP/REST/JSON-RPC.
5. `.rkat` projection replay/idempotence from event log.
6. Memory search only enabled when `memory-store` is present; otherwise capability-disabled error.
7. Compaction triggers at threshold when `session-compaction` is enabled.
8. Compaction does not trigger below threshold and respects loop guards.
9. Compaction failure is non-fatal and emits `CompactionFailed`.
10. Compaction event propagation parity across SDK/CLI/MCP/REST/JSON-RPC streams.
11. Compaction config request without feature returns `SESSION_COMPACTION_DISABLED`.

## Assumptions and Defaults
- No migration/backward compatibility required.
- `foamy-weaving-tome.md` changes are already implemented in target branch.
- Session capability is optional by design, but session API shape remains stable.
- Ephemeral mode is accepted as first-class for zero-shot deployments.
- Compaction defaults:
  - `auto_compact_threshold`: configured per SDK request.
  - keep recent **turns** by budget (not only user messages).
  - non-fatal on compaction call failure.
- Config TOML + CLI compaction flags are follow-up after SDK configuration lands.
