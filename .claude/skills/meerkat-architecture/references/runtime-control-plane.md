# Runtime Control Plane

Load this reference when working on runtime drivers, session registration, policy resolution, async operation lifecycle, persistence, or any cross-cutting runtime change.

## MeerkatMachine

`MeerkatMachine` implements `RuntimeControlPlane` in `meerkat-runtime`. Per-session state is an `Arc<Mutex<mm_dsl::MeerkatMachineAuthority>>` held on `RuntimeSessionEntry`, plus a `RuntimeDriver` (ephemeral or persistent) for IO plumbing.

All semantic state mutations route through the DSL authority via `dsl_apply(input)`. The runtime shell owns only mechanics: tokio lock topology, channel plumbing, IO, and observability projections.

## Runtime-backed build seam

Runtime-backed surfaces (CLI, REST, RPC, MCP) obtain `SessionRuntimeBindings` from `MeerkatMachine::prepare_bindings(session_id)` and pass them through `SessionBuildOptions.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings)`. Standalone paths (WASM, tests, embedded) use `RuntimeBuildMode::StandaloneEphemeral`.

`SessionRuntimeBindings` (in `crates/meerkat-core/src/runtime_epoch.rs`) is the
epoch-local bundle. It carries identity, ops/completion state, the
MeerkatMachine-owned tool visibility projection and handles, independently
owned auth lease authority, and cross-owner coordinators. The
MeerkatMachine-backed handles share the session's real
`MeerkatMachineAuthority` via `HandleDslAuthority::from_shared(...)`, so their
method calls and dispatch-driven transitions land on the same underlying state.
`AuthLeaseHandle` is the exception: `RuntimeAuthLeaseHandle` owns a
mutex-guarded registry of per-binding `AuthMachineAuthority` instances rather
than borrowing the session's MeerkatMachine authority.

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

- `tool_visibility_owner: GeneratedToolVisibilityOwner` — generated-authority
  carrier for tool visibility; `tool_visibility_owner()` returns
  `&GeneratedToolVisibilityOwner`. The underlying `ToolVisibilityOwner` trait
  delegates behind that carrier; a raw `Arc<dyn ToolVisibilityOwner>` is not a
  substitute for the runtime-minted installation authority.
- `external_tool_surface: Arc<dyn ExternalToolSurfaceHandle>` — MCP surface transitions
- `mcp_server_lifecycle: Arc<dyn McpServerLifecycleHandle>` — MCP server add/remove/reload lifecycle

Peer comms:

- `peer_comms_install: GeneratedPeerCommsInstallFactory` - peer envelope
  classification plus the machine-minted install authority for trust
  projection mutations; consumers read the handle through `peer_comms()`
- `peer_interaction: Arc<dyn PeerInteractionHandle>` — peer-driven interaction transitions
- `interaction_stream: Arc<dyn InteractionStreamHandle>` — interaction stream lifecycle

Model + auth:

- `model_routing: Arc<dyn ModelRoutingHandle>` — provider/model baseline resolution
- `sticky_model_fallback_commit_coordinator` - cancellation-safe durable
  fallback identity commit
- `auth_lease: GeneratedAuthLeaseHandle` - certified handle backed by the
  runtime's per-binding `AuthMachine` registry

Compaction:

- `compaction_commit_coordinator` - exact transcript-plus-memory handoff for
  the session/runtime epoch

When you add a new handle field, `prepare_bindings()` and the factory's
`SessionOwned` validation must be updated so the surface receives the correct
owner's certified authority view rather than a convenient session-local copy.

## Ownership split

- `PersistentRuntimeDriver::recover()` owns input/runtime/control recovery (replay from store)
- `MeerkatMachine` owns session-entry runtime recovery: `ops_lifecycle`, `epoch_id`, shared cursor state
- `SessionRuntimeBindings` are the epoch-local witness for that ownership
- Durable-tail recovery of the session document is a separate machine-owned pipeline (below); shells never promote or discard a durable tail

## Durable-tail recovery

The intra-turn persistence hook writes a provisional physical successor
outside the boundary transaction and returns an exact `RunCheckpointReceipt`.
The compatibility type name does not mean checkpoint authority is embedded in
`Session`. A crash can leave a durable tail - up to a fully completed turn -
whose boundary commit never landed. Ownership splits three ways (never-discard;
full contract in `docs/reference/machine-authority.mdx`):

- The store retains the committed physical authority and any exact
  provisional-tail authority; a recovery candidate is never returned as an
  ordinary session. `SessionDocumentMachine` classifies the observed tail as
  `DurableTailRecoveryClass` (`CompletedCandidate` /
  `InterruptedRepairableCandidate` / `Ambiguous`; any dangling `tool_use` is
  Ambiguous - held, never closed with synthetic results).
- `MeerkatMachine` authorizes: `AuthorizeDurableTailRecovery` judges the
  persisted lifecycle row, the prior-commit receipt comparison, and input
  attributability; both hold paths are machine-minted; commit verdicts emit
  `DurableTailRecoveryCommitAuthorized` with the machine-minted boundary
  sequence (one past the last committed receipt).
- `RuntimeStore` realizes a sealed `PreparedRuntimeSessionCommit` through
  `commit_prepared_session_boundary`: one atomic boundary for the recovered
  profile-specific session state, receipt, and input terminalization, fenced
  on the observed lifecycle-row version and per-input-row digests
  (`expected_row_digest` MUST be enforced inside the writing transaction;
  typed `InputRowVersionConflict` / `MachineLifecycleVersionConflict`).

The public preparation seam is `recover_durable_tail(store, session_id)` in
`crates/meerkat-runtime/src/recovery.rs`. It loads opaque store-bound evidence,
derives and proves the exact candidate, and drives/consumes the generated
classification internally. Callers supply only the `RuntimeStore` and stable
`SessionId`, never documents, heads, classifications, receipts, or CAS tokens.
WholeBlob and HeadCanonical recovery have distinct preparation paths that
produce the sealed `PreparedRuntimeSessionCommit` handed to the store.
While a tail is held or evidence is quarantined, resume fails typed
(`SessionError::DurableTailHeldForRecovery` / `DurableEvidenceQuarantined`)
with content retained. Read-triggered recovery runs under an exclusive
per-session fence and converges idempotently when a competing process wins.

## Key operations

- `ingest(runtime_id, input)` - admit an input through policy resolution
- `publish_event(event)` - publish an event to the logical runtime's current incarnation
- `retire(runtime_id)` - graceful drain (process the queue and reject new input)
- `recycle(runtime_id)` - rebuild the driver shell while preserving canonical non-terminal pending work
- `reset(runtime_id)` - abandon pending work and return to `Idle`
- `recover(runtime_id)` - replay durable runtime state after a crash or restart
- `runtime_state(runtime_id)` - query `Initializing`, `Idle`, `Attached`, `Running`, `Retired`, `Stopped`, or terminal `Destroyed`
- `destroy(runtime_id)` - enter terminal `Destroyed` state with no recovery
- `load_boundary_receipt(runtime_id, run_id, sequence)` - read an exact committed boundary receipt for verification

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

`delegate()` is an exact bounded one-turn task/result contract. It provisions a
turn-driven helper with `initial_message` cleared, admits the task through
`start_work_for_identity_bounded`, waits for that exact admitted turn to reach
its committed terminal boundary, and returns the labeled bounded result with
explicit truncation, session attribution, usage, turn/tool counts, and any
retirement cleanup error. Bidirectional comms wiring is attempted separately
and reported by `wired`; it is useful for helper-to-parent messaging but is not
the result carrier. Use explicit mob members for long-lived collaborators.

## Completion-feed wake

Idle keep-alive wake from background shell completions is runtime-owned. Terminal `BackgroundToolOp` entries land in `RuntimeCompletionFeed`; the runtime loop tracks `EpochCursorState.runtime_observed_seq` and `runtime_last_injected_seq`, checks `is_quiescent_for_detached_wake()`, and injects `ContinuationInput::detached_background_op_completed()` from `runtime_loop.rs`. Do not spawn surface-local waker tasks or side channels.

## Recycle versus mob respawn

`RuntimeControlPlane::recycle` rebuilds the driver shell for the same logical
runtime while preserving its canonical non-terminal pending work. It is not a
member respawn and does not mint a new mob runtime binding.

`MobHandle::respawn` is the separate member-level operation: it preserves the
member's `AgentIdentity`, spec, and intended peer wiring while replacing the
runtime incarnation and fence token and archiving the old bridge binding. The
mob-side lifecycle facts behind that operation are machine-owned: restore
failures (`member_restore_failures`) and post-discard revival
(`member_revival_pending`, observe, classify, realize, with `Broken` terminal)
live in MobMachine DSL. See the mob-orchestration reference.

## Agent loop and turn phases

`LoopState` is the persisted and user-facing coarse projection. Its closed
roster is `CallingLlm`, `WaitingForOps`, `DrainingEvents`, `ErrorRecovery`,
`Cancelling`, and terminal `Completed`.

Canonical turn state is the finer `TurnPhase` owned by MeerkatMachine DSL. Its
closed roster is `Ready`, `ApplyingPrimitive`, `CallingLlm`, `WaitingForOps`,
`DrainingBoundary`, `Extracting`, `ErrorRecovery`, `Cancelling`, `Completed`,
`Failed`, and `Cancelled`. The state also carries `pending_op_refs`,
`barrier_operation_ids`, `boundary_count`, `extraction_attempts`,
`terminal_outcome`, and related facts. The Agent reads it via
`TurnStateHandle`. Barrier membership is DSL-authoritative; shell code does not
decide what is a barrier or when the barrier is satisfied.

## OpsLifecycleRegistry

Trait in `crates/meerkat-core/src/ops_lifecycle.rs`. Concrete impl `RuntimeOpsLifecycleRegistry` in `crates/meerkat-runtime/src/ops_lifecycle.rs` is a thin projection/command surface over MeerkatMachine's ops state (`op_statuses`, `op_terminal_outcomes`, `op_peer_ready`, `op_progress_count`, `wait_active`, `wait_operation_ids`). DSL is the sole authority; the registry exposes:

- Typed commands: `register_operation`, `provisioning_succeeded`/`failed`, `peer_ready`, `report_progress`, `complete_operation`, `fail_operation`, `abort_provisioning`, `cancel_operation`, `request_retire`, `mark_retired`, `terminate_owner`. Each routes to a DSL transition.
- Typed read surface: `snapshot(id)`, `list_operations()`, `register_watcher(id)`.
- `wait_all` + `collect_completed` + `drain_completed` for barrier coordination.
- Bounded completed-operation retention (FIFO eviction; default 256).
- Multi-listener completion observation, peer info in snapshots, wall-clock timestamps (`created_at_ms`, `completed_at_ms`, `elapsed_ms` from SystemTime anchor), per-parent concurrency enforcement (`max_concurrent`).
- Completion-feed wake signals for keep-alive runtimes.

Completion feed: the registry owns a `FeedBuffer` that produces `CompletionEntry` events on terminal transitions. `RuntimeCompletionFeed` (read handle) implements `CompletionFeed` (meerkat-core trait). Consumer cursors are epoch-owned via `EpochCursorState` on `SessionRuntimeBindings`.

Persistence channel: when wired via `set_persistence_channel()`, terminal
persistence captures a `PersistedOpsSnapshot` (DSL op state + completion
entries + cursor values) and sends it over an unbounded mpsc queue of
`OpsLifecyclePersistenceRequest`. A dedicated worker calls
`RuntimeStore::persist_ops_lifecycle()` and acknowledges the actual durable
result through each request's separate one-slot reply channel. Terminal
persistence waits for that acknowledgement; queue closure, worker loss, and
store failures propagate rather than becoming successful durable writes.
This optional persistence path is distinct from bounded completion-feed
retention and the best-effort event projector.

Recovery: `MeerkatMachine::recover_or_create_ops_state()` loads persisted
snapshots via `RuntimeOpsLifecycleRegistry::from_recovered()`. Generated
recovery classification retains valid terminal records and running
`DetachedJobWait` bindings, discarding other volatile nonterminal operations
by default. Recovering a running wait does not mint a terminal outcome or
completion entry; detached-job lifecycle remains feature-owned. The separate
internal `from_recovered_preserving_operation` path handles an exact typed
`ReloadRequired` preservation request matching persisted identity, kind, and
source, not a blanket retain-all exception. The feed buffer is pre-seeded
with persisted completion entries, and consumer cursors are restored.

## Durable event projection

When a persistent host installs event projection, session event envelopes feed
two different consumers. The UI broadcast is bounded at 256 entries and reports a
`StreamTruncated` event with `StreamLagged { dropped }` when a subscriber falls
behind. The singular durable audit projector receives the same shared envelope
over a separate unbounded queue and warns at a backlog of 1,024, so UI-ring lag
does not drop its input. EventStore append is asynchronous best-effort derived
state, not session commit authority. An append failure latches projection halt
and replay fails closed without undoing the committed turn; derived `.rkat`
views and their cursors remain rebuildable projections.

## Durable jobs and runtime delivery

Reusable detached work is not an ops-lifecycle entry. `DetachedJobMachine` and
`DetachedJobStore` own job submission deduplication, attempts, fences, leases,
progress, cancellation, terminal results, subscriptions, and the terminal
outbox. `RuntimeDeliveryMachine` and its inbox own delivery identity, sequence,
replay, and the applied cursor; `RuntimeStore` supplies CAS persistence for
that machine-owned state. The `job_runtime_delivery` composition transfers one
exact outbox entry into that inbox and acknowledges it back to the job store. A schedule or
WorkGraph item may reference or await a job, but neither becomes a second job
lifecycle owner.

## Session Service

Runtime-backed product surfaces route through `SessionService`:

```
CreateSessionRequest → SessionService::create_session() → RunResult
  └── SessionAgentBuilder::build_agent() → SessionAgent
      └── AgentFactory::build_agent() → DynAgent
```

Two implementations:
- `EphemeralSessionService<B>` — in-memory substrate (WASM, testing, embedded Queue-only use)
- `PersistentSessionService<B>` — durable substrate for runtime-backed product surfaces (CLI, RPC, REST, MCP; typically backed by sqlite or jsonl through `PersistenceBundle`)

`FactoryAgentBuilder` bridges `AgentFactory` into `SessionAgentBuilder`.
Embedded Rust may instead use the public facade `AgentBuilder` to compose an
explicit standalone agent directly through `AgentFactory`; it is not a
`SessionService` path.

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

- `crates/meerkat-runtime/src/meerkat_machine/mod.rs` — MeerkatMachine implementation
- `crates/meerkat-runtime/src/meerkat_machine/session_management.rs` — session registration, recovery
- `crates/meerkat-runtime/src/meerkat_machine/dispatch_*.rs` — dispatch paths per input family
- `crates/meerkat-runtime/src/driver/ephemeral.rs`, `driver/persistent.rs` — per-session drivers
- `crates/meerkat-runtime/src/ops_lifecycle.rs` — `RuntimeOpsLifecycleRegistry`
- `crates/meerkat-runtime/src/policy_table.rs` — `DefaultPolicyTable`
- `crates/meerkat-runtime/src/runtime_loop.rs` — completion-feed wake injection and runtime loop
- `crates/meerkat-runtime/src/recovery.rs` — store-bound durable-tail preparation, classification, authorization, and realization (`recover_durable_tail(store, session_id)`)
- `crates/meerkat-runtime/src/store/mod.rs` — `RuntimeStore` contract (fenced records, boundary receipts)
- `crates/meerkat-core/src/completion_feed.rs` — monotonic completion-feed contract
- `crates/meerkat-runtime/src/peer_handling_mode.rs` — handling_mode validation
- `crates/meerkat-core/src/runtime_epoch.rs` — `SessionRuntimeBindings`, `RuntimeBuildMode`
- `crates/meerkat-core/src/ops_lifecycle.rs` — `OpsLifecycleRegistry` trait
- `crates/meerkat-session/src/ephemeral.rs`, `persistent.rs` — session services
