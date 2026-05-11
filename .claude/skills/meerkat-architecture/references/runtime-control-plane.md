# Runtime Control Plane

Load this reference when working on runtime drivers, session registration, policy resolution, async operation lifecycle, persistence, or any cross-cutting runtime change.

## MeerkatMachine

`MeerkatMachine` implements `RuntimeControlPlane` in `meerkat-runtime`. Per-session state is an `Arc<Mutex<mm_dsl::MeerkatMachineAuthority>>` held on `RuntimeSessionEntry`, plus a `RuntimeDriver` (ephemeral or persistent) for IO plumbing.

All semantic state mutations route through the DSL authority via `dsl_apply(input)`. The runtime shell owns only mechanics: tokio lock topology, channel plumbing, IO, and observability projections.

## Runtime-backed build seam

Runtime-backed surfaces (CLI, REST, RPC, MCP) obtain `SessionRuntimeBindings` from `MeerkatMachine::prepare_bindings(session_id)` and pass them through `SessionBuildOptions.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings)`. Standalone paths (WASM, tests, embedded) use `RuntimeBuildMode::StandaloneEphemeral`.

`SessionRuntimeBindings` (in `meerkat-core/src/runtime_epoch.rs`) is the epoch-local bundle. In 0.6.5 it carries identity, ops/completion state, the machine-owned tool visibility projection, and all session-owned DSL handles that share the session's real `MeerkatMachineAuthority` via `HandleDslAuthority::from_shared(...)` — handle method calls and dispatch-driven transitions land on the same underlying state.

Identity:

- `session_id`, `epoch_id` — identity witnesses
- `cursor_state: Arc<EpochCursorState>` — shared completion-feed cursors

Ops + completions:

- `ops_lifecycle: Arc<dyn OpsLifecycleRegistry>` — typed projection/command surface over DSL ops state

Turn / drain / admission:

- `turn_state: Arc<dyn TurnStateHandle>` — turn execution transitions
- `comms_drain: Arc<dyn CommsDrainHandle>` — drain lifecycle transitions
- `session_admission: Arc<dyn SessionAdmissionHandle>` — session turn admission
- `session_claim_handle: Arc<dyn SessionClaimHandle>` — session-claim ownership/release transitions
- `session_context: Arc<dyn SessionContextHandle>` — system-context append transitions

Tool surface:

- `tool_visibility_owner: Arc<dyn ToolVisibilityOwner>` — tool visibility projection
- `external_tool_surface: Arc<dyn ExternalToolSurfaceHandle>` — MCP surface transitions
- `mcp_server_lifecycle: Arc<dyn McpServerLifecycleHandle>` — MCP server add/remove/reload lifecycle

Peer comms:

- `peer_comms: Arc<dyn PeerCommsHandle>` — peer envelope classification
- `peer_interaction: Arc<dyn PeerInteractionHandle>` — peer-driven interaction transitions
- `interaction_stream: Arc<dyn InteractionStreamHandle>` — interaction stream lifecycle

Model + auth:

- `model_routing: Arc<dyn ModelRoutingHandle>` — provider/model baseline resolution
- `auth_lease: Arc<dyn AuthLeaseHandle>` — published `AuthMachine` lease handle for the session

When you add a new handle field, `prepare_bindings()` and the factory's `SessionOwned` validation must be updated so the surface gets the same authority view as dispatch.

## Ownership split

- `PersistentRuntimeDriver::recover()` owns input/runtime/control recovery (replay from store)
- `MeerkatMachine` owns session-entry runtime recovery: `ops_lifecycle`, `epoch_id`, shared cursor state
- `SessionRuntimeBindings` are the epoch-local witness for that ownership

## Key operations

- `ingest(session_id, input)` — admit an input through policy resolution
- `retire(session_id)` — graceful drain (process queue, reject new input)
- `respawn(...)` — helper convenience: retire old runtime binding, spawn fresh with same identity/spec/wiring, advance runtime incarnation + fence token (not machine-owned)
- `reset(session_id)` — abandon pending, return to Idle
- `recover(session_id)` — replay from store for crash recovery
- `destroy(session_id)` — terminal state, no recovery
- `runtime_state(session_id)` — query current state (Initializing/Idle/Running/Recovering/Retired/Stopped/Destroyed)

## Policy engine

`DefaultPolicyTable` resolves `PolicyDecision` per input kind × runtime state:

- 9 input kinds: prompt, peer_message, peer_request, peer_response_progress, peer_response_terminal, flow_step, external_event, continuation, operation
- 2 states: idle, running

Each cell specifies `ApplyMode`, `WakeMode`, `QueueMode`, `ConsumePoint`, `DrainPolicy`, `RoutingDisposition`.

## Peer handling_mode override

`PeerInput` with `Message`, `Request`, or no convention may carry an explicit `handling_mode` (`Queue` or `Steer`) that overrides kind-based policy defaults. `ResponseProgress` and `ResponseTerminal` MUST NOT carry `handling_mode` — enforced by `validate_peer_handling_mode` at runtime admission. Built-in comms bridges default to `None` (kind-based policy).

## Silent intent override

`silent_comms_intents` is a generic runtime feature for suppressing lifecycle notices. Mob lifecycle uses typed silent inputs (`mob.peer_added`, `mob.peer_retired`, `mob.peer_unwired`) and typed visible inputs (`mob.kickoff_failed`, `mob.kickoff_cancelled`) — never string matching on intent names for canonical routing.

## Peer ingress ownership

The runtime owns the comms drain lifecycle via MeerkatMachine's `drain_phase` / `drain_mode` DSL fields. Surfaces provide `keep_alive` + comms context through `update_peer_ingress_context()`, and the runtime reconciles the canonical mode: `AttachedSession` while runtime-backed sessions are live, `PersistentHost` only for idle `keep_alive=true`. The direct session-service path (standalone) does not support keep-alive — only runtime-backed surfaces can.

## Delegate semantics

`delegate()` is communication-first. It spawns and auto-wires a helper, delivers the opening prompt via the existing initial-message path, and then parent/helper communicate via ordinary comms. There is no hidden task contract or peer reservation stream. If the helper fails during bootstrap before it can reliably report for itself, the bridge emits a typed lifecycle notice and durable kickoff state records the failure.

## Completion-feed wake

Idle keep-alive wake from background shell completions is runtime-owned. Terminal `BackgroundToolOp` entries land in `RuntimeCompletionFeed`; the runtime loop tracks `EpochCursorState.runtime_observed_seq` and `runtime_last_injected_seq`, checks `is_quiescent_for_detached_wake()`, and injects `ContinuationInput::detached_background_op_completed()` from `runtime_loop.rs`. Do not spawn surface-local waker tasks or side channels.

## Respawn semantics

Helper convenience (not a machine-owned primitive). Same `AgentIdentity`, spec, and peer wiring; new runtime incarnation and fence token. Old bridge binding is archived. Used for "agent is confused, restart it" recovery.

## Agent loop state machine

`CallingLlm` → `WaitingForOps` → `DrainingEvents` → `Completed`, with `ErrorRecovery` and `Cancelling` branches.

Turn-level state lives in MeerkatMachine DSL (`turn_phase`, `pending_op_refs`, `barrier_operation_ids`, `boundary_count`, `extraction_attempts`, `terminal_outcome`, etc.). The Agent reads it via `TurnStateHandle`. Barrier membership is DSL-authoritative; shell code does not decide what's a barrier or when the barrier is satisfied.

## OpsLifecycleRegistry

Trait in `meerkat-core/src/ops_lifecycle.rs`. Concrete impl `RuntimeOpsLifecycleRegistry` in `meerkat-runtime/src/ops_lifecycle.rs` is a thin projection/command surface over MeerkatMachine's ops state (`op_statuses`, `op_terminal_outcomes`, `op_peer_ready`, `op_progress_count`, `wait_active`, `wait_operation_ids`). DSL is the sole authority; the registry exposes:

- Typed commands: `register_operation`, `provisioning_succeeded`/`failed`, `peer_ready`, `report_progress`, `complete_operation`, `fail_operation`, `abort_provisioning`, `cancel_operation`, `request_retire`, `mark_retired`, `terminate_owner`. Each routes to a DSL transition.
- Typed read surface: `snapshot(id)`, `list_operations()`, `register_watcher(id)`.
- `wait_all` + `collect_completed` + `drain_completed` for barrier coordination.
- Bounded completed-operation retention (FIFO eviction; default 256).
- Multi-listener completion observation, peer info in snapshots, wall-clock timestamps (`created_at_ms`, `completed_at_ms`, `elapsed_ms` from SystemTime anchor), per-parent concurrency enforcement (`max_concurrent`).
- Completion-feed wake signals for keep-alive runtimes.

Completion feed: the registry owns a `FeedBuffer` that produces `CompletionEntry` events on terminal transitions. `RuntimeCompletionFeed` (read handle) implements `CompletionFeed` (meerkat-core trait). Consumer cursors are epoch-owned via `EpochCursorState` on `SessionRuntimeBindings`.

Persistence channel: when wired via `set_persistence_channel()`, terminal transitions capture a `PersistedOpsSnapshot` (DSL op state + completion entries + cursor values) and queue it to a bounded mpsc channel. A dedicated persistence task drains to `RuntimeStore::persist_ops_lifecycle()`.

Recovery: `MeerkatMachine::recover_or_create_ops_state()` loads persisted snapshots via `RuntimeOpsLifecycleRegistry::from_recovered()`. Non-terminal operations are stripped. The feed buffer is pre-seeded with persisted completion entries. Consumer cursors are restored.

## Session Service

All surfaces route through `SessionService`. Runtime-backed surfaces are the canonical product path:

```
CreateSessionRequest → SessionService::create_session() → RunResult
  └── SessionAgentBuilder::build_agent() → SessionAgent
      └── AgentFactory::build_agent() → DynAgent
```

Two implementations:
- `EphemeralSessionService<B>` — in-memory substrate (WASM, testing, embedded Queue-only use)
- `PersistentSessionService<B>` — durable substrate for runtime-backed product surfaces (CLI, RPC, REST, MCP; typically backed by sqlite or jsonl through `PersistenceBundle`)

`FactoryAgentBuilder` bridges `AgentFactory` into `SessionAgentBuilder`.

Usage rule: for runtime-backed surfaces, look for `prepare_bindings()` and `RuntimeBuildMode::SessionOwned(...)`. If code hand-rolls registration + registry extraction or leans on implicit standalone fallback, treat that as architectural drift.

Resume metadata: `SessionTooling` is tri-state via `ToolCategoryOverride` (`Inherit`, `Enable`, `Disable`). Persist caller intent with `from_override()`, not resolved booleans, or resumed sessions freeze tool availability at the build-time capabilities.

Store seam: `SessionStore` lives in `meerkat-core`; `meerkat-store` is the default implementation crate plus adapters. Custom backends depend on the contract, not the impl crate.

## Persistence Pairing

Persistent realm opening is backend-owned in the `meerkat` facade through `PersistenceBundle`.

- Surfaces open a realm bundle, not a raw session store plus ad hoc runtime companion.
- The bundle carries the paired `SessionStore`, optional `RuntimeStore`, matching `MeerkatMachine`, and when enabled the blob/task companions for that realm.
- SQLite is the default persistent realm backend when compiled; jsonl is the file-backed alternative. Mob persistence is SQLite/WAL-backed.
- `meerkat-tools::builtin::SqliteTaskStore` persists session-scoped tasks in the shared SQLite realm when `session-store` is enabled.
- `meerkat-mob::MobStorage::persistent()` uses `SqliteMobStores` (WAL, per-operation connections). `MobStorage::custom()` is the extension seam for user-provided mob stores.
- Do not reintroduce long-lived exclusive handles to mob persistence paths — lingering handles keep file locks across in-process restarts.

## Test Harness Ownership

Repository-wide test lanes are part of the architecture:

- `unit` and `int` are the deterministic inner loops
- `e2e-fast` and `e2e-system` are the required deterministic/system lanes
- `e2e-live` and `e2e-smoke` are the live-provider lanes

The authoritative end-to-end lane catalog lives in `tests/integration/src/e2e_lanes.rs`. If a change affects how a scenario is classified, filtered, or bootstrapped, update the lane harness first rather than reintroducing script-owned truth.

Make is the stable command surface for those lanes. Cargo remains the default
backend; `MEERKAT_BUILDBUDDY=1` switches the same broad local verbs to the
optional BuildBuddy/Bazel backend through `scripts/run-build-backend-lane` and
`scripts/buildbuddy-dev`. Keep lane semantics identical across backends: if a
Cargo lane gains or loses coverage, the matching BuildBuddy lane should be
updated rather than replaced by a static or host-only placeholder.

For multi-agent work, same-checkout agents should use distinct `RUST_LANE_ID`
values when they need stable warm output roots. Separate worktrees are isolated
by path hash; do not reintroduce shared in-repo target directories or raw Cargo
entrypoints that bypass `scripts/repo-cargo`.

## Key files

- `meerkat-runtime/src/meerkat_machine/mod.rs` — MeerkatMachine implementation
- `meerkat-runtime/src/meerkat_machine/session_management.rs` — session registration, recovery
- `meerkat-runtime/src/meerkat_machine/dispatch_*.rs` — dispatch paths per input family
- `meerkat-runtime/src/driver/ephemeral.rs`, `driver/persistent.rs` — per-session drivers
- `meerkat-runtime/src/ops_lifecycle.rs` — `RuntimeOpsLifecycleRegistry`
- `meerkat-runtime/src/policy_table.rs` — `DefaultPolicyTable`
- `meerkat-runtime/src/runtime_loop.rs` — completion-feed wake injection and runtime loop
- `meerkat-core/src/completion_feed.rs` — monotonic completion-feed contract
- `meerkat-runtime/src/peer_handling_mode.rs` — handling_mode validation
- `meerkat-core/src/runtime_epoch.rs` — `SessionRuntimeBindings`, `RuntimeBuildMode`
- `meerkat-core/src/ops_lifecycle.rs` — `OpsLifecycleRegistry` trait
- `meerkat-session/src/ephemeral.rs`, `persistent.rs` — session services
