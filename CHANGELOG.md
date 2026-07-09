# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

**Versioning policy (pre-1.0):** meerkat 0.x PATCH releases may contain
breaking public-API changes — this project ships deliberate clean breaks
instead of compatibility shims. Downstreams must EXACT-PIN the meerkat crate
family (e.g. `meerkat = "=0.7.24"`) and bump deliberately. In exchange, every
release that breaks public API declares it under a `### Breaking` heading
naming the changed signatures (enforced by the `semver-breaks` release gate
via cargo-semver-checks against the published baselines).

## [0.7.26] - 2026-07-09

Meerkat 0.7.26 closes the field bugs reported against 0.7.25's first week:
the torn-shutdown save wedge on classic (non-incremental) session stores,
the cold-revival re-bind rejection that broke identity-first member
revival, the misleading "session not found" laundering in front of it, and
the whole-mob blast radius of a single member's composition-dispatch
rejection. It also documents the 0.7.25 storage-layout migration and the
durable runtime_state vocabulary, and hardens the release pipeline's
Homebrew tap update against assets-only backfills of old tags.

### Fixed

- Cold revival of a stopped session re-binds under its fresh registration
  epoch (field, HomeCore on 0.7.24/0.7.25 identity-first gateways: member
  revival failed terminally with "session not found in runtime adapter
  after registration" / "DSL rejected PrepareBindings: GuardRejected
  { phase: Attached }"). The 0.7.24 revival arcs preserved the runtime
  binding tuple — but that tuple is epoch-scoped: phase Stopped proves the
  bound epoch's executor exited, and a cold revival registers under a
  freshly minted epoch, so every `PrepareBindings` arm (all guarded
  absent-or-same) rejected the re-bind by construction. Warm revivals
  (same process, same epoch) passed, which is why the 0.7.24 class tests
  were green. The revival arcs (`RegisterSessionResumesStopped`,
  `EnsureSessionWithExecutorStopped`) now clear the dead epoch's binding
  tuple while preserving session identity and hydrated LLM/capability
  state; the next `PrepareBindings` / mob `RequestRuntimeBinding` binds the
  new epoch. Red-verified with a torn-shutdown cold-restart class test.
- `MeerkatMachine::prepare_local_session_bindings` no longer launders every
  preparation failure into "session not found in runtime adapter after
  registration": typed machine rejections (e.g. a `PrepareBindings` guard)
  now surface verbatim as `RuntimeBindingsError::PrepareFailed` — the
  misleading not-found string cost the field a debugging cycle.
- One member's typed composition-dispatch rejection no longer terminates
  the whole mob actor task ("composition dispatch failed; terminating mob
  actor task" killed every healthy sibling). Every routed effect carries
  its target bridge session, so a session-scoped dispatch failure now
  degrades that member through the machine-owned restore-failure fact
  (`RecoverMemberRestoreFailure` + restore diagnostic) and drops the
  member's queued effects, while the actor keeps serving. Failures with no
  session scope, unknown sessions, and destroy-cleanup dispatch failures
  keep the fatal/bubble-up contract (incomplete destroy stays retryable).
  Adjacent gap fixed: the per-session routed-effect discard now also covers
  `RequestRuntimeIngress`.

- The torn-shutdown save wedge now also clears on CLASSIC (non-incremental)
  session stores (field, identity-first gateways on meerkat 0.7.25: resume
  saves rejected with "message count 165 is shorter than previously
  persisted 167" retried forever). 0.7.24's write-half rollback
  (`ResolveRuntimeProjectionRollback` → `RebuildToAuthority`) was consulted
  on the incremental head-canonical save path and the audited
  authoritative-projection path, but external `SessionStore` implementations
  route plain projection saves through the storage-normalization bridge —
  which re-threw `MonotonicityViolation` against a checkpointer-stamped
  ahead row on every retry. The bridge now consults the same machine-owned
  arbitration and converges via the CAS projection write; unstamped or
  content-forked rows keep failing closed.

## [0.7.25] - 2026-07-08

Meerkat 0.7.25 is the outstanding-asks sweep: O(delta) incremental session
persistence, mob revival completeness, restart-first-class status queries,
the structural peer-reply affordance, attention-binding uniqueness with
break-glass and GC, a metadata-only session read seam — plus the field fix
for the runtime-queue defer wedge and the pre-1.0 exact-pin versioning
policy with its release gate.

### Breaking

- `WorkGraphAttentionTurnOverlayError::MultipleActiveBindings` (facade) is
  removed. Multiple active bindings resolving to one session now arbitrate
  deterministically (newest binding wins) with a loud diagnostic instead of
  hard-failing every turn; new duplicates are prevented at the store.
- `meerkat_core::event_injector::SubscribableInjector` gained a required
  method `inject_with_interaction_id(...)`. Implementors outside this repo
  must provide it (delegating to `inject` and discarding the id is NOT
  faithful — the id must reach runtime admission).
- `meerkat_mob::WorkSpec` gained the public field
  `interaction_id: Option<InteractionId>`. Struct-literal constructors must
  set it (serde `default` keeps wire compatibility).
- `SqliteSessionStore` storage is now head-canonical: session heads and
  transcript strands replace the monolithic `session_json` blob for new
  writes, with a one-time transparent migration on open. External tools
  reading `sessions.session_json` via direct SQL will not see content for
  sessions written by 0.7.25+ binaries; use the store API (or `load_meta`
  for metadata-only reads). Migration note and the supported read paths are
  documented in `docs/reference/session-contracts.mdx` ("Session storage
  layout (0.7.25+)").
- `session/input_state` / `session/input_status` for persistent sessions
  without a live runtime registration now answer from durable state
  (`Ok`/`NotFound`) instead of `NotReady { reason: Destroyed }`. Pollers
  that treated `NotReady` as "try again later" get truthful terminal
  answers after a host restart.
- `meerkat_rpc::SessionRuntime::create_session` now takes
  `self: &Arc<Self>` (callers holding a bare `&SessionRuntime` must hold an
  `Arc`, which every runtime consumer already does). The gratuitous
  `&mut self` receivers on `set_config_runtime`, `set_realm_config_source`,
  `set_default_llm_client`, `set_agent_llm_client_decorator`, and
  `set_skill_identity_roots` are relaxed to `&self` (they were interior
  mutability all along).

### Fixed

- Agent-authored schedules fire in embedded `SessionRuntime` hosts (field,
  0.7.23: `meerkat_schedule_create` returned an id and the planner minted
  occurrences, but they stayed pending 12+ hours past due). The runtime
  binds the `meerkat_schedule_*` agent tools to its own schedule store at
  construction, but only the RPC router's startup path spawned the firing
  host — an embedder constructing `SessionRuntime` directly handed agents a
  store nothing drives. The runtime now arms its firing host itself the
  moment agent work can run (session creation and executor attach; atomic
  fast path, loud warn + retry on start failure), so
  tools-bound-but-undriven is no longer a representable topology.
  REST/MCP/CLI already start their hosts eagerly and are unaffected.

- A failed input batch no longer wedges the runtime queue when the machine's
  own failure realization resolves batch members out of the queued world
  (field, 0.7.23 crew gateway: a steer-shaped input's third failed stage
  attempt was abandoned by the machine's retry policy, the loop's
  whole-batch defer sweep then hit "DSL rejected DeferInputBehindBacklog:
  GuardRejected { phase: Attached }", dropped the pending wake, and
  stranded the backlog for ~13 minutes until an unrelated external wake).
  The defer sweep is now machine-owned-total:
  `DeferInputBehindBacklogAlreadyResolved` accepts tracked members that
  provably left `Queued` (max-attempts abandonment, boundary-applied
  members) as an explicit no-op, while a tracked-but-lane-less input still
  claiming `Queued` stays a loud rejection (projection corruption). The
  runtime loop's three batch-failure arms (apply failure, primitive
  rejection, turn-state preparation failure) now share one canonical
  backlog resolution: defer the failed batch and keep draining other queued
  work in the same wake, park only when nothing else is queued, and fail
  closed through the canonical executor-stop path — not a silent
  wake-dropping return — if the defer genuinely breaks.
- Dogma-audit fixes (#853 + review hardening): the embedded
  `mob-communication` skill is registered by the facade again (with a
  content pin so an empty or drifted skill body fails tests); the
  TypeScript and Python SDK model-catalog parsers fail closed on malformed
  catalogs instead of coalescing missing fields; the ownership-ledger
  `cfg(test)` detector recurses into `any()`/`all()` attribute groups and
  `feature = "test-support"`; Bazel test-support variant generation is
  seeded for `meerkat-mcp` alongside `meerkat-runtime`.

### Added

- `IncrementalSessionStore` (upstream ask 11): O(delta) session
  persistence. `SessionStore::as_incremental` exposes the optional trait;
  the SQLite store becomes head-canonical (`SessionHead` +
  `TranscriptStrandId` vocabulary, per-strand append tables, CAS'd head
  swings), `PersistentSessionService` writes deltas when the store offers
  them, and compaction rewrites the head so shrunken context is durable —
  saving a long session no longer serializes the full transcript on every
  turn.
- Mob member revival completeness (studio M2): reviving a stopped member
  recomposes its per-spawn external tools from the durable spawn spec
  (spawn/respawn/dispose threading plus cold-restore seeding), and
  `MemberSpawnedEvent` durably carries `effective_profile_override` so
  roster replay restores the member's effective profile instead of `None`.
- Restart-first-class terminal status (studio M4): `session/input_status`
  and `session/input_state` answer from durable runtime rows without
  requiring a live registration — a fresh host process can report terminal
  outcomes for work finished before the restart, with zero wire-shape
  changes.
- Interaction ids persist onto committed transcript messages (mobkit ask 15
  addendum): `SubscribableInjector::inject_with_interaction_id` and
  `WorkSpec.interaction_id` thread host-supplied interaction identity to
  runtime admission, so transcript messages join with the caller's live
  interaction frames.
- Structural peer-reply affordance (upstream ask 26): peer-message
  deliveries mint a `PeerReplyCapability` into the turn's dispatch context
  (key `comms.peer_reply`), and the pre-addressed `reply_to_peer` comms
  tool replies to the delivering peer without the agent restating
  addressing. Capability minting is batch-level (`TurnToolOverlay::compose`
  lifted to `meerkat-core` as the one canonical compose); errors are typed
  and fail closed. No wire changes.
- Metadata-only session read seam (upstream ask 24 clause 3):
  `SessionStore::load_meta` (SQL projection over stored metadata; ambiguity
  delegates to the full canonical load) and
  `MobSessionService::load_persisted_session_metadata` — list/status paths
  read session metadata without deserializing full transcripts.
- `semver-breaks` release-preflight gate (studio M3): cargo-semver-checks
  runs against the last published crates.io baselines; detected public-API
  breaks fail the release unless declared under a `### Breaking` changelog
  heading. The exact-pin policy in this file's header is the other half of
  the contract.
- Attention-binding uniqueness is now service/store-owned (mobkit ask 25):
  at most one ACTIVE attention binding per target, enforced transactionally
  inside the same store write that mints or revives a binding (`create_goal`,
  reassignment, `resume_attention`), with a typed `Conflict` naming the
  occupant binding. Host-layer admission guards demote to defense-in-depth.
- Break-glass host-plane attention reassignment (mobkit ask 23, reframed):
  `WorkGraphService::break_glass_reassign_attention` moves any non-terminal
  binding regardless of mode-derived authority, for the one state the graph
  cannot heal agent-natively (a binding stuck on a wedged/retired agent with
  no coordinator holding authority). Mandatory principal + reason are
  recorded in the workgraph event stream and a WARN log. Host API only —
  never exposed on the agent tool surface or wire catalogs; the agent-plane
  mode restriction is untouched.
- Terminal attention-binding GC (mobkit ask 24):
  `WorkGraphService::prune_terminal_attention` deletes superseded/stopped
  binding rows (optionally bounded by `updated_before`); the event stream
  keeps the audit history.

### Changed

- SQLite attention queries push realm/namespace/status/target filters into
  SQL over new indexed `status`/`target_key` columns (mobkit ask 24) instead
  of decoding every row in the store. The columns are added and backfilled
  by an idempotent open-time migration; all readers are NULL-tolerant, so
  stores shared with older binaries stay correct.
- Multiple active attention bindings resolving to one session no longer
  hard-fail every turn (`MultipleActiveBindings` removed): the turn overlay
  arbitrates deterministically — newest binding wins — with a loud
  diagnostic, mirroring the newest-session arbitration. Reachable only via
  legacy rows, mixed-version writers, or cross-kind targets; new duplicates
  are prevented at the store.
- `make machine-verify` and the pre-push machine gate now route through the
  canonical TLC lane (`xtask/tests/machine_verify_all_tlc_test.sh`), which
  owns the documented over-budget composition skips (`meerkat_mob_seam` /
  `adaptive_mob_bundle` full ci.cfg sweeps) and the bounded adaptive witness
  proof. The previous bare `machine-verify --all` form ran the full mob-seam
  state-space sweep, which fits no local or pre-push budget; the unbounded
  sweep remains available on demand as `make machine-verify-full`.

## [0.7.24] - 2026-07-08

Meerkat 0.7.24 closes the 0.7.19–0.7.23 resume-strand class at its root:
the machine now owns revival of stopped sessions, and cold resume
reconciles a stale runtime snapshot against the durable store head. This
is the release for downstream pins chasing broken resumes
("guard rejected transition from phase Stopped"), stranded disposals, or
permanent save rejections after a torn shutdown.

### Fixed

- The Stopped phase is no longer absorbing for resume — the root cause of
  the entire 0.7.19–0.7.23 resume-strand class (field: "guard rejected
  transition from phase Stopped for input::PublishLocalEndpoint", archive
  NotFound strands, disposal escalations, wedged mob members retrying
  forever). The machine now owns revival at the canonical resume seams:
  `RegisterSession` on a same-session Stopped machine re-admits it to Idle
  preserving identity, runtime bindings, and hydrated LLM state
  (`RegisterSessionResumesStopped`); `EnsureSessionWithExecutor` on Stopped
  re-admits to Attached and grants the active executor claim; `Retire`
  admits Stopped so disposal of an executor-stopped session is a machine
  transition (the mob durable-retire helper's shell phase probes are gone).
  Both revival arms refuse while an unregister drain window is open, and a
  machine-emitted `Recover` notice keys a durable lifecycle persist so a
  revived session is never left durably `Stopped` for cross-process
  readers. The accumulated per-input Stopped-tolerance arms
  (`HydrateSessionLlmStateStopped`, `PrepareBindingsStopped`,
  `PublishCommittedVisibleSetStopped`, `SetSilentIntentsStopped`, and the
  Stopped admissions of `SetModelRoutingBaseline`/`StagePersistentFilter`/
  `RequestDeferredTools`) are deleted outright: post-revival the resume
  build never runs at Stopped, so a build input reaching a Stopped machine
  is a loud typed rejection, never a silent self-loop.
- Cold resume no longer wedges permanently when the committed runtime
  session snapshot froze as a stale strict prefix of the durable store head
  (a completed turn's boundary save landed before the snapshot recommitted
  under a torn shutdown; every subsequent save then tripped the append-only
  guard forever). The `SessionDocumentMachine` owns the read-source verdict
  over three typed observations — the head provably extends the snapshot
  (the save guard's own continuity proof), the head row's intra-turn
  checkpoint provenance stamp, and in-process liveness. A cold load of an
  unstamped continuity-proven extension serves the store head; stamped
  ahead rows (uncommitted intra-turn residue), diverged rows, and live
  sessions keep the snapshot authoritative.

### Testing

- Stopped-phase revival class battery: the machine-schema exit-lattice pin
  (every arc out of Stopped is enumerated; the revival/teardown/disposal/
  destruction set is exact), resume-intent totality and
  no-tolerance-arms-return sweeps, runtime warm class tests (the exact
  field repro: peer-comms install succeeds post-revival, rejected at
  Stopped), the revival↔stop race pin (the historical error signature now
  only means a raced stop), retire-from-Stopped, drain-window refusal in
  both entry orders, and the mob stopped-member disposal sibling of the
  ask-21d regression. Red-verified against the pre-fix machine.
- Stale-snapshot read-source pins: cold strict-prefix defers to the head
  (red-verified against the old unconditional snapshot preference),
  checkpoint-stamped and diverged heads stay snapshot-served, live sessions
  unaffected; the former fail-closed-resume expectation — which WAS the
  field wedge — now pins a successful resume.

## [0.7.23] - 2026-07-08

Meerkat 0.7.23 completes the never-run-member disposal arc for
identity-first workers (upstream ask 21d), ships the end-to-end WorkGraph
goals and attention wiring, and folds in the fixes from the pre-release
adversarial review of that wiring.

### Added

- WorkGraph goals and attention are wired end to end (#850): goal creation
  with attention bindings, machine-backed CAS reassignment on the
  attention-scoped tool surface (with a server-injected authority witness —
  callers cannot forge projections), per-turn attention overlays injected on
  every runtime surface, escalate-only completion-policy transitions guarded
  in the machine lattice, SQLite workgraph store hardening
  (busy_timeout/FULL synchronous/IMMEDIATE transactions on all mutating
  paths), and a fail-closed public MCP split for workgraph tools.

### Fixed

- Identity-first mob workers no longer strand in archive-NotFound during
  disposal (upstream ask 21d, #849): when the archive authority reports
  NotFound for a session whose runtime is in the terminal Retired phase
  while still registered, disposal completes (durable retire + runtime
  unregister) instead of escalating. Stopped runtimes are deliberately
  excluded — they are recoverable.
- Scheduled prompt delivery no longer snapshots the WorkGraph attention
  projection at enqueue time (facade schedule host and CLI schedule host).
  A queued prompt behind a running turn that mutated its work item carried a
  projection that failed exact-currency validation at apply, deterministically
  failing the scheduled delivery. The runtime executors inject a fresh
  projection at apply time — the single canonical injection point — so the
  enqueue-time composition was removed outright.
- WorkGraph attention overlays now arbitrate newest-session-wins on the
  canonical apply-time injection path. The arbitration guard previously ran
  only on the (removed) enqueue-time composition path, so during mob member
  respawn overlap both the old and new session matched the same owner-key
  binding and both received the attention overlay. The session-listing
  arbitration is now threaded through every injection surface (CLI, REST,
  RPC, MCP, mob provisioner, runtime executors); a session not yet visible in
  the listing (mid-creation) is treated as the newest carrier of its labels.

### Testing

- Pinned the apply-time attention injection on `CliRuntimeExecutor` and the
  newest-session arbitration semantics (overlap denial + mid-creation allow).
- Pinned the non-wasm fail-closed factory contract: WorkGraph enabled without
  a supplied dispatcher refuses to build with the typed `Config` error.
- Pinned the SQLite UNIQUE-violation → typed `Conflict` mapping for duplicate
  work item and attention binding inserts.

## [0.7.22] - 2026-07-07

Meerkat 0.7.22 fixes the runtime-loop self-deadlock that was the root
producer of the never-run-member disposal failures (upstream asks 20, 21,
21b) and, on 0.7.21, wedged entire mobs (ask 21c). This is the release for
downstream pins.

### Fixed

- Runtime-loop stop paths never run executor cleanup under the session
  mutation gate (upstream ask 21c, P0). The loop task acquired the
  per-session gate for terminal/effect handling and, still holding it,
  entered stop realization — whose cleanup re-enters the machine (the mob
  executor unregisters its session), and unregister's first await is the
  same non-reentrant gate: the task parked forever, leaking a permanently
  registered session (the state asks 20/21/21b kept hitting) and, on
  0.7.21, deadlocking the whole single-task mob actor behind the gate.
  Stops are now guard-free by construction: the effect drain surfaces stop
  effects instead of applying them (callers apply after releasing their
  authority guard), every stop call site drops its live guard first, and
  `retire_runtime_control_plane` acquires the gate with a 30s bound and a
  typed error naming the deadlock class so future regressions fast-fail
  instead of wedging mobs. Defensive class tests drive a machine-re-entrant
  executor through three distinct stop entry paths under hard timeouts —
  red-verified to deadlock on the unfixed code.
- With unregister completing, never-run member disposal converges through
  the 0.7.20/0.7.21 archive arms instead of manufacturing the
  permanently-registered state.

## [0.7.21] - 2026-07-07

Meerkat 0.7.21 fixes the 0.7.19/0.7.20 external-tool poison regression
(every MCP tool call failing after the session bind) and makes archive
retries converge for the last never-run-member retiring strand. It also
supersedes 0.7.20, whose registry packages published but whose Windows
GitHub-release assets were blocked by a runner disk flake — install
0.7.21.

### Fixed

- External tool servers staged/connected before the session-authority bind
  no longer poison the tool surface (meerkat-studio P0 regression on
  0.7.19/0.7.20). 0.7.19's composite bind forwarding delivered the session
  bind to nested MCP adapters for the first time, and the adapter refused
  pre-bind surface facts with a permanently poisoned handle — every
  subsequent call failed with "surface is poisoned … refusing to replay
  handwritten snapshot". The pre-bind state is machine-validated state on
  the construction-time authority, not a handwritten snapshot: the bind
  now re-derives every fact through generated inputs on the session
  authority (the sibling pattern to the MCP lifecycle bind's
  connect-pending seed), and only a genuine machine refusal still poisons
  fail-closed.
- Archive retries converge for the partial state left by a failed runtime
  retire (upstream ask 21b): an Archived document with a still-registered
  runtime used to resolve AlreadyArchived → NotFound on every retry,
  leaving the runtime registered forever (never-run mob-plane members
  stranded in `retiring`). The SessionDocumentMachine now owns the
  convergence — the re-archive completes the retire (no document rewrite)
  and reports success; quiescent duplicates keep the public NotFound
  contract. The REST archive retry that completes retained mob cleanup now
  returns 200 and removes the retry anchor instead of 404-with-a-leaked
  runtime.

## [0.7.20] - 2026-07-07

Meerkat 0.7.20 stops the one-shot occurrence-regeneration runaway (HomeCore:
223 misfired occurrences in ~2 minutes from one past-due one-shot) and
unbricks retire/respawn for created-but-never-run mob members.

### Fixed

- A one-shot (or any trigger) whose occurrence went terminal no longer
  regenerates occurrences unboundedly (upstream ask 22, P0). Root cause: the
  machine-owned planning cursor is millisecond precision while trigger due
  times carried nanoseconds — `truncate_ms(due) < due`, so the planner
  re-yielded an already-planned due every tick once nothing pending was left
  to dedupe against. The trigger engine now yields and compares
  ms-truncated timestamps (one fact, one representation); existing runaway
  stores heal in place on upgrade. Defense in depth: the
  ScheduleLifecycleMachine now owns planning monotonicity —
  `RecordPlanningWindow` refuses a cursor that does not strictly advance,
  so any future planner or representation bug converges as a visible
  per-tick refill fault instead of unbounded generation.
- Archiving a created-but-never-run session (durable record exists, runtime
  snapshot never committed) no longer rejects as a store-only projection
  (upstream ask 21, P1). Archive is a lifecycle terminal, not a projection
  promotion: the durable record is the complete truth for a never-run
  session, so mob members that never received a prompt can be
  retired/respawned instead of stranding in `retiring`. Control MUTATIONS
  (context appends, tool staging) still reject store-only projections, and
  already-archived sessions keep the typed NotFound contract.

### Security

- `crossbeam-epoch` 0.9.18 → 0.9.20 (RUSTSEC-2026-0204).

## [0.7.19] - 2026-07-06

Meerkat 0.7.19 hardens the schedule subsystem against poisoned durable rows
(HomeCore field incident: one bad row silently starved every schedule), makes
member cancellation and retire/respawn real for embedders (meerkat-studio P0s),
and gives library embedders first-class MCP wiring and a durable
run-reconciliation query.

### Fixed

- Mid-turn member cancellation no longer crashes the host process
  (meerkat-studio M1, P0). A boundary handle that re-entered
  `MeerkatMachine::cancel_after_boundary` from inside the machine's own
  dispatch — the shape meerkat's own blocked-method error text steered
  embedders into — recursed unboundedly until the tokio worker overflowed
  its stack (SIGABRT). The machine now owns a
  `boundary_cancel_dispatch_pending` fact: the first cancel in a turn window
  dispatches, repeats converge as typed no-ops, and a new turn re-arms
  dispatching. The misleading contract text on
  `PersistentSessionService::interrupt`/`cancel_after_boundary` and the
  `MobSessionService` cancel seams now states that implementations apply
  the cancel to the live agent and must never re-enter the machine.
- Retire/respawn no longer fails deterministically for session-owned mob
  members (meerkat-studio K1, P0). Members adopted from a host-owned session
  service (e.g. embedder console sessions via `ensure_member`) have no
  record in the mob archive authority; disposal escalated that to a fatal
  "authority returned NotFound for registered runtime session" while
  leaving the runtime registered, so every retry failed identically and
  the member could be neither respawned nor removed. Disposal now reads the
  typed ownership fact from the authority itself
  (`session_known_to_archive_authority`) before archiving: host-owned
  members retire their runtime session and release the binding; the
  fail-closed split-state escalation is preserved verbatim for
  authority-owned sessions.
- One poisoned schedule row no longer starves every schedule (upstream asks
  16–19, HomeCore field incident). The sqlite claim scan and driver tick are
  now per-row tolerant: rows that fail typed recovery, due classification,
  or per-schedule horizon refill are skipped as typed, attributable faults
  (`ScheduleStoreRowFault`, `ScheduleRefillFault` on `ScheduleTickReport`)
  while healthy neighbors keep claiming; the in-memory store mirrors the
  same per-row semantics. Store write failures inside the claim transaction
  still abort wholesale — a committed misfire receipt without its
  terminalized occurrence row can never split-commit.
- Schedule host ticks are no longer silent: tick failures and degraded
  (row-fault) ticks log an ERROR incident on change, a rate-limited WARN
  heartbeat while the same condition persists, and an INFO recovery line
  with the full outage length. The tracker uses the wasm-safe clock
  (`std::time::Instant` panics on wasm32-unknown-unknown and would have
  killed the browser schedule host on its first tick).
- Legacy `Deleted` schedule tombstones that persisted a planning cursor
  (written before the Delete transitions cleared it) heal at the
  durable-format parse boundary instead of failing every strict `list()`
  wholesale; each healed drop is logged. The claim scan is bounded in SQL
  (active schedules, live-phase occurrences, due/lease-expired only), so
  multi-GB terminal history never pays per-tick deserialization and a
  poisoned terminal row cannot poison the claim path; the live-phase list
  is compile-time ratcheted against `OccurrencePhase`.
- Strict schedule listing failures now name the poisoned row
  (`schedule row '<id>' failed typed recovery`) instead of an
  unattributable serialization error.
- Resuming a durably-stopped session no longer fails on the LLM capability
  hydration phase guard ("guard rejected transition from phase Stopped") —
  the hydrate transitions now admit Stopped and Retired sessions like the
  sibling baseline/visibility inputs already did, so restart-resume repair
  converges instead of retrying the same guard forever.

### Added

- Declarative MCP for library embedders (meerkat-studio M2):
  `SessionBuildOptions::mcp_servers` / `AgentBuildConfig::mcp_servers`
  materialize MCP stdio/HTTP servers into a session-owned router bound to
  the build mode's canonical external-tool surface authority — composing
  with builtins, no test-code ephemeral-handle incantation. Mob profiles
  gain the durable `tools.mcp_servers`, so revived members recompose their
  MCP tools from the profile instead of losing the in-process spawn
  overlay. `CompositeDispatcher` now forwards
  `bind_external_tool_surface_handle`/`bind_mcp_server_lifecycle_handle` to
  its external child, making session-time late binding reach nested MCP
  adapters. `meerkat::mcp::standalone_router()` is the supported
  constructor for raw-router hosts.
- Durable run reconciliation (meerkat-studio M4): the machine-owned
  idempotency binding is queryable read-only —
  `SessionServiceRuntimeExt::input_state_by_idempotency_key` returns the
  input's stored state (terminal outcome, resolving run id, boundary
  sequence) and survives host restart via session re-registration recovery.
  Exposed over JSON-RPC as `session/input_state` (by input id or
  idempotency key) with Python (`client.input_state`) and TypeScript
  (`client.inputState`) SDK wrappers, so embedders can delete hand-rolled
  run journals.
- Versioning and compatibility policy (meerkat-studio M3): documented in
  `docs/guides/cd-and-distribution.md` — pre-1.0 patch releases may change
  public APIs, embedders pin exact versions, the only supported crate
  combination is exact version parity, breaking changes land under a
  `### Breaking` changelog heading — plus a downstream compatibility
  matrix.

### Changed

- Repeat `cancel_after_boundary` calls while a dispatch is outstanding are
  accepted no-ops (previously each call re-dispatched and could saturate
  the runtime effect channel); the next turn re-arms dispatching.

## [0.7.18] - 2026-07-06

Meerkat 0.7.18 fixes the resume regression that stranded idle mob members
after prompt-drifting host upgrades (mobkit 0.7.23 field incident: 14 of 15
identities permanently refused resume).

### Fixed

- Chained turn-less resume refreshes no longer strand sessions. A member
  whose system prompt carries drifting parts (comms rosters, host context)
  gets a `resume-system-prompt-refresh` rewrite committed on every boot;
  with no turn in between, the transcript history graph retains several
  chained refresh commits, and the rewrite-chain walk failed closed on the
  next turn's run-boundary commit ("incoming append-only save would change
  retained transcript revision graph"): it selected an OLDER refresh commit
  under the system-refresh equivalence, walked forward onto the revision
  the cursor had already reached, and aborted as a cycle — rejecting a
  valid plain-append continuation. Reproduced end-to-end against stores
  seeded by real meerkat 0.7.13/0.7.14/0.7.15 binaries. The walk now
  proves continuity in authority order per iteration: exact graph edge
  first (real rewrite commits stay on the audited persistence chain), then
  the plain append continuation (with the leading-System-refresh
  equivalence still unprovable there, so unaudited System swaps keep
  failing), and only then the fuzzy refresh-equivalence edge; both
  selection scans skip already-visited revisions. Sessions stranded by
  this bug recover on their next resume — no data was lost (the durable
  transcripts were preserved the whole time).
- Crafted-state hardening from the adversarial review of the fix: the
  run-boundary rewrite branch re-validates the incoming transcript history
  state; the non-empty-chain arm validates every retained commit's
  recorded bodies (mirroring the empty-chain arm); and the revision
  ancestry walk is bounded so cyclic revision-parent metadata fails closed
  instead of hanging the boundary commit.


## [0.7.17] - 2026-07-05

Meerkat 0.7.17 fixes the machine-catalog compile blowup at its root and
supersedes 0.7.16, whose registry packages published but whose GitHub
release assets were blocked by the Windows binary lane. (Same shape as
0.7.15 superseding 0.7.14 — install 0.7.17.)

### Fixed

- The generated machine catalog no longer blows up rustc. The DSL
  `schema()` emission produced one enormous function per machine, driving
  rustc/LLVM to tens of GB of peak memory and deeply recursive opt-3
  passes — the root cause of every Windows release-lane failure from
  v0.7.14 through v0.7.16 (LLVM out-of-memory on `meerkat-mcp`,
  `0xc0000409` stack overruns on `meerkat-runtime` and
  `meerkat-machine-schema`) and of multi-hour local release builds. The
  emission is now chunked so the catalog compiles with sane memory on
  every platform (#835).

### Changed

- Windows release lane: with the generator fixed, every opt-level pin
  that papered over the blowup is removed (`meerkat-machine-schema`'s
  release pin from #832 in #835; the lane-local `meerkat-runtime` /
  `meerkat-mcp` pins from #834 in #836) — release binaries ship full
  codegen again on every platform. The lane keeps a two-job parallelism
  cap as commit-capacity headroom on the 16GB no-overcommit runner
  (#833, #836), alongside the pagefile expansion (#831).

## [0.7.16] - 2026-07-05

Meerkat 0.7.16 closes all six rows of the 2026-07-04 dogma audit — most
visibly, JSONL realms gain durable runtime authority and OpenAI live channels
resolve credentials per-open from the session's own auth binding — and moves
Windows release binaries to GitHub-hosted runners. Every row was
adversarially re-verified against HEAD before the fix, and the combined diff
went through a second adversarial review whose confirmed findings are all
resolved in this release.

### Fixed

- OpenAI live channels authenticate with the session's credential, not a
  process-default one. `rkat-rpc` used to resolve the default OpenAI binding
  once at startup and bake that secret into every realtime socket, so a
  session with an explicit `auth_binding` had live admission scoped to one
  identity while the provider socket authenticated as another. Realtime
  credentials now resolve per-open from the session's `SessionLlmIdentity`
  (owning-realm binding provenance, live config store — never a stale startup
  clone) through a new registry-owned
  `ProviderRuntime::build_realtime_session_factory` seam that carries the
  same fail-closed backend/auth gating as the text path (ChatGPT-backend,
  Azure, authorizer-auth, and custom-base-url bindings are rejected with the
  typed errors instead of opening a mis-keyed socket; the gate matrix has one
  shared owner). The facade never extracts secrets;
  `attach_external_session` carries the session identity; startup performs a
  wiring preflight only, so explicit-binding-only configs boot.
- JSONL realms run on durable runtime authority. The jsonl persistence
  bundle used to carry no runtime store, silently degrading every
  runtime-backed surface to an in-memory control plane (fresh ops epoch per
  restart: queued inputs, run-boundary receipts, and admission state all
  evaporated). JSONL realms now mount a sqlite runtime companion
  (`runtime.sqlite3`, typed `RealmPaths::runtime_sqlite_path`) next to the
  JSONL session documents; `runtime_store` is non-optional across
  `PersistenceBundle` and `PersistentSessionService`, and every store-only
  compatibility branch is deleted. Cold-restart parity for jsonl realms
  (runtime snapshot, boundary receipts, queued-input recovery) is pinned by
  tests. A durable document carrying the Archived lifecycle terminal with no
  runtime lifecycle state (legacy store-only archives) reads as archived —
  terminal and non-resumable — instead of failing realm `list()` closed with
  an untyped error.
- Skill inventory shadowing is computed over canonical identity.
  `CompositeSkillSource` canonicalizes every descriptor through the
  `SourceIdentityRegistry` before computing active/shadowed status
  (canonical-host rule: only the source physically hosting the canonical key
  can be active; remapped-away copies list inactive naming the active host),
  so remap/merge lineage can no longer advertise duplicate active canonical
  skills in browse/introspection. Listing fails closed without a registry;
  the registry-less composite constructors are deleted; the engine never
  rewrites keys and fail-closed-resolves entries that carry no typed source
  identity, so inventory can never advertise a skill the load authority
  refuses.
- `memory_search` no longer advertises previous-session recall. The
  model-facing ToolDef, the embedded memory-retrieval skill, the crate docs,
  and the guides now state the session-scoped contract (recall from earlier
  in this session, including turns compacted away before a resume), matching
  the typed scope model and the HNSW session filter; the dead second
  guidance string (`usage_instructions`) is deleted and the contract is
  pinned by tests.
- Python and TypeScript mob member-status wrappers stopped dropping and
  fabricating wire facts: required `member_ref` is surfaced, missing
  `tokens_used`/`is_final` fail closed instead of coercing to `0`/`false`,
  `peer_connectivity` is parsed as the tagged tri-state (unknown tags and
  the legacy flat shape rejected) with non-negative-integer validation on
  counts and tokens, and present-but-malformed `kickoff` payloads fail
  closed — mirroring the Web SDK reference in both SDKs. The four
  `mob/member_status` grandfather entries are deleted from the signature
  parity baseline (cap ratcheted down and enforced).
- The WASM browser contract mirror is truthful and actually executed. The
  wasm-bindgen suite still asserted the pre-0.6.23 contract (in-band
  `status` strings, addressable archived state after `destroy_session`) and
  — deeper — lacked `run_in_browser`, so even the BuildBuddy chrome lane had
  been silently skipping it while exiting green. The suite is rewritten to
  the shipped contract (tagged `WirePromptInput`, typed `WireRunResult`
  assertions, fail-closed `invalid_session_handle` after destroy) and now
  genuinely runs in headless Chrome.

### Changed

- CI: `wasm-check` compiles and lints all wasm32 targets (`--all-targets`),
  and a new path-filtered `wasm-contract` job executes the browser contract
  suite via `wasm-pack test --headless --chrome` on every wasm-relevant
  change (unconditionally in nightly), closing the gate hole that let the
  stale mirror survive.
- Windows release binaries build on GitHub-hosted runners (the org pool has
  no self-hosted Windows RBE executors); Linux/macOS release binaries stay
  on the BuildBuddy lane. `meerkat-machine-schema` pins its release opt
  level to avoid the rustc-LLVM OOM in the Windows lane.

## [0.7.15] - 2026-07-04

Meerkat 0.7.15 fixes cold-restart transcript loss for hosts that re-send
explicit system prompts on resume (the SDK-gateway shape), with the
system-prompt reconciliation hardened by an adversarial review of its tail
verification and continuation-walker seams. It also carries the scoped
Windows release-lane ThinLTO fix. (0.7.14 published to all registries but its
GitHub release assets were blocked by offline Windows build executors; 0.7.15
supersedes it.)

### Fixed

- Cold-restart resume no longer loses the transcript when the host re-sends
  an explicit per-request system prompt (the SDK-gateway shape: member specs
  carry `SystemPromptOverride::Set` on every build). The factory build used
  to blind-replace the resumed session's leading System message with the
  re-assembled base prompt, discarding every runtime-applied system-context
  append (comms rosters, host context) and re-stamping the typed
  `mutation_kind` — so the resumed projection was no longer a continuation
  of the persisted transcript revision, the continuity preflight failed
  closed on the very first post-resume persist, the live session was
  discarded, and downstream hosts fell back to fresh empty spawns (silent
  history loss on every restart, including sessions freshly written by the
  same version). Resume now reconciles instead of replacing
  (`Session::reconcile_resumed_system_prompt`): a base the persisted System
  message already carries — identical, or extended only by runtime
  system-context appends — is preserved byte-for-byte (digest unchanged),
  and a genuinely changed base is committed as a typed transcript rewrite
  (`resume-system-prompt-refresh`) that preserves the runtime-appended tail
  (reconstructed byte-exactly from the new
  `SessionBuildState.assembled_system_prompt` record, with an
  applied-append re-render fallback), so the first post-resume persist
  proves a transcript graph edge from the persisted head instead of failing
  closed. The rewrite-chain continuation walker also no longer aborts as a
  cycle when a same-length rewrite commit sits exactly at the persisted
  head (`find_transcript_rewrite_commit_chain_extending_session` skips
  commits that cannot advance the cursor), which previously rejected the
  first post-resume turn after such a rewrite. Regression coverage: resume
  with a runtime-context-appended prompt (unchanged and changed explicit
  base) over durable sqlite realm stores, mob cold-restart revival with a
  context-appended member prompt, and unit pins on the reconciliation
  outcomes.
- Reconciliation hardening from the adversarial review of the fix above: an
  all-context System prompt (empty/`Disable` base plus runtime appends) is
  carried onto a new base instead of being misread as an "empty tail"; when
  an unverifiable tail must be dropped, the orphaned applied-append records
  and their idempotency keys are cleared (with a warning) so keyed re-sends
  can restore the context instead of deduplicating forever; a shortened
  explicit base whose removed remainder merely looks like a context tail
  (the separator is ordinary markdown) is applied through the audited
  rewrite instead of silently ignored — byte-exact verification against the
  recorded prior base runs first, and the structural fast path is admitted
  by the canonical `SessionDocumentMachine` from the typed
  `RuntimeContextAppend` provenance, not shape alone. The rewrite-chain
  walker's plain-append fallback no longer accepts untyped leading-System
  replacements via the system-refresh equivalence (that equivalence now
  bridges commit PARENT bookkeeping only), and the empty-chain acceptance
  re-checks graph-head/message-digest agreement. `run_boundary_snapshot_save_guard`
  adopts a first runtime-boundary commit that carries a validated typed
  rewrite graph (resume/import over fresh runtime state); the plain
  `SessionStore::save` first-save seeding rejection is unchanged. Both
  resume rewrite sites drive `authorize_system_prompt_mutation` (the
  generated durable-config authority) instead of hand-stamping the mutation
  provenance, and the append-path block rendering is shared with the
  resume-time tail verification so the two compositions cannot drift.

## [0.7.14] - 2026-07-04

Meerkat 0.7.14 makes cold-restart resume survivable (content-addressed
transcript revisions + machine-owned resume projection authority) and lets
sqlite stores read legacy TEXT JSON rows carried in from external writers.

### Fixed

- Sqlite stores accept legacy TEXT JSON payload rows (upstream ask A). Meerkat
  writes JSON payload columns as BLOB, but SQLite affinity keeps whatever type
  a writer bound, so carried stores written by external hosts could hold the
  same UTF-8 JSON as TEXT — and one such row failed every
  `list()`/`get()`/claim with `Invalid column type Text`. All JSON payload
  reads in `meerkat-store` (schedule store including the claim-due JOIN,
  session store `session_json`/`metadata_json`, session index `meta_json`) now
  go through a typed dual-encoding read boundary (`Text | Blob` → UTF-8 JSON
  bytes). Writes stay canonical BLOB; rewritten rows normalize. Upgrade-carry
  tests degrade committed rows via `CAST` and assert every read path carries.
- Cold-restart resume no longer fails closed on re-stamped transcripts
  (upstream ask B, part 1). The transcript revision digest is now a CONTENT
  address: construction bookkeeping — `TranscriptMessageIdentity`
  (run/interaction ids, re-minted by every re-created runtime authority) and
  `created_at` timestamps — is erased from the digest form, so a resume that
  re-projects the same conversation digests to the same revision as the
  persisted row instead of tripping `append_only_save_guard`
  (`TranscriptContinuityViolation`) and discarding the live session. Typed
  semantic facts (`transcript_role`, `mutation_kind`, `render_metadata`,
  notice kinds/blocks) stay in the digest. Persisted rewrite graphs carrying
  pre-0.7.14 revision strings heal at the durable-format parse boundary:
  every retained revision body re-verifies under the legacy digest and
  re-derives to the content-addressed format (`TranscriptHistoryState` /
  `TranscriptRewriteRecord` deserialization); unverifiable strings are left
  for the validators to reject exactly as before. Hosts holding revision-id
  strings from a pre-0.7.14 process should re-list
  `session/transcript_revisions` after upgrading.
- Cold-restart resume converges an ahead-of-authority durable row instead of
  stranding the session (upstream ask B, part 2). Every runtime-backed turn
  has two non-atomic durable commit points — the intra-turn best-effort
  checkpointer writes the session-store row, the machine boundary commit
  writes the runtime-store snapshot — so a host kill between them (or an
  in-process lifecycle-commit failure that evicted the uncommitted live turn)
  left the row carrying turn content the machine never acknowledged. Every
  subsequent runtime-authoritative persist then rejected against the newer
  row (`MonotonicityViolation`), permanently stranding the session (mob
  members came back terminally `Broken`; only respawn recovered). The
  canonical `SessionDocumentMachine` now owns the projection-rollback
  disposition (`ResolveRuntimeProjectionRollback`): the intra-turn
  checkpointer stamps its rows with a typed provenance fact
  (`session_runtime_checkpoint_provenance_v1`, stripped by every
  boundary-following persist), and when an ahead-of-authority row both
  carries that stamp AND is a faithful continuation of the authority
  transcript — judged by the same run-boundary proof the save guard uses —
  the CAS projection write rebuilds the row onto committed truth. Rows
  without the system's own provenance stamp (out-of-band writers) and
  genuine content forks keep failing closed. The runtime authority stays
  singular — the row is never adopted as truth — and no user input is lost:
  the discarded tail's durably admitted input (durable-before-ack) is
  redelivered after restart and re-executes through a fresh
  machine-committed run. Regression suites cover mob revival after a kill
  between commit points (including input redelivery), compaction across
  restarts (flushed, shrink + post-restart compaction, and lagged durable
  row), stale out-of-band row divergence staying fail-closed, and
  bookkeeping-divergent persisted rows.

## [0.7.13] - 2026-07-03

Meerkat 0.7.13 completes the MobKit content-taint story (upstream asks 9+10)
and ships provider prompt-cache hints and Gemini video-by-URI support.

### Added

- Host-consumable content-taint surfaces (upstream ask 9, completing ask 5):
  - New `peer_content_ingested` agent event — a typed projection of committed
    inbound peer content (canonical peer identity, comms kind, request id, and
    the sender's signed `sender_taint` declaration), emitted for both queued
    and steered deliveries. Host taint trackers consume typed facts instead of
    parsing rendered peer-message text; covers peer requests and cross-process
    senders. Joins the wire event inventory across all three SDKs.
  - Per-member outbound taint declaration: the core `CommsRuntime` trait gains
    `set_outbound_content_taint` (typed `Unsupported` default), reachable per
    mob member via `MobHandle::declare_member_outbound_taint` /
    `MemberHandle::declare_outbound_taint`. The declaration installs on the
    member's own comms runtime, so every outbound content-bearing envelope
    carries it inside the signed region; external-bound members receive it
    over the supervisor bridge via the new
    `BridgeCommand::DeclareMemberOutboundTaint`. Declarations reset on
    respawn/reset (fresh-context taint semantics).
- Runtime-backed schedule hosts can register host runnables (upstream ask 10):
  `spawn_runtime_backed_schedule_host` / `_with_mobs` accept an optional
  `ScheduleRunnableHost`, so `HostRunnable` schedule targets dispatch through
  the runtime host's occurrence driver.
- Provider params now expose typed prompt-cache hints for OpenAI, Anthropic,
  and Gemini. OpenAI Responses requests still default to `store: false`, with
  explicit overrides for `store`, `prompt_cache_key`, and
  `prompt_cache_retention`; Anthropic can mark the stable system prefix for
  `cache_control`; Gemini can pass an explicit cached-content resource name.
- OpenAI Responses streaming can reuse stored response IDs as
  `previous_response_id` continuation hints when `store: true` is explicitly
  enabled, while preserving full local transcript replay as the fallback path.
- Gemini video inputs can now be passed by provider-readable URI as
  `VideoData::Uri` / `WireVideoData::Uri`. Vertex accepts `gs://` references
  directly. Gemini API clients use public or already-registered file URIs as
  `fileData`, and can register `gs://` references through the Files API before
  generation when Google bearer auth is available.

## [0.7.12] - 2026-07-02

Meerkat 0.7.12 lands the eight upstream asks from the MobKit agent-memory
initiative, plus a pre-existing schedule-store fix.

### Added

- Typed injected-context transcript class (`TranscriptUserRole::InjectedContext`):
  hosts attach ambient context as separate typed user-channel messages on every
  submit-work path (service/RPC/REST create+turn, runtime inputs, mob
  `WorkSpec`/`mob/turn_start`/`mob/submit_work`, supervisor-bridge delivery,
  Python/TS wrappers). Injected context and discarded compaction summaries are
  excluded from semantic-memory indexing; transcript rewrites may carry the
  role (compaction summaries stay runtime-mintable-only, rejected fail-closed).
- `MemoryStore` lifecycle: per-scope `drop_scope` delete/GC and paged
  `enumerate_scoped` (durable-id order, `source_range` overlap and
  `indexed_after` filters). `HnswMemoryStore` opens lazily — no more
  every-session re-embed on each agent build — with an in-place SQLite
  migration, transactional never-reused point-ID allocation, and mixed-version
  row healing.
- Host-supplied compaction curator (`CompactionCurator`): produces the summary
  instead of the LLM call (zero-LLM-cost compaction); curator failure is a
  typed `CompactionFailed` reason with no silent LLM fallback.
- `session/transcript_revisions`: transcript revision list (with head) on the
  JSON-RPC transcript family; restore now resolves the `current` selector.
- Comms content-taint channel: signed-when-present `content_taint` declaration
  on content-bearing envelopes, host-set outbound declaration with tri-state
  per-send override, receiver-side typed `sender_taint` on comms transcript
  notices. Hook payloads gain typed tool `provenance` and provider-native
  `server_tool_content` for synchronous dispatch-time classification.
- Call-level tool authorization: `SpawnMemberSpec.tool_access_policy` is now
  enforced end-to-end via a list-preserving execution gate (prompt-cache
  prefix unchanged; denials are ordinary tool errors). `Inherit` resolves to
  the parent's effective policy on persistent AND ephemeral backends, and
  agent-created schedules with helper targets inherit the creator's policy at
  creation.
- Host-runnable schedule targets (`target_kind: "host_runnable"`): library
  hosts register named runnables; occurrences flow through the normal
  occurrence lifecycle.

### Fixed

- Persisted Flow-target schedule rows were unreadable (raw `Box<RawValue>`
  params cannot deserialize through internally-tagged serde buffering); Flow
  params now use a canonicalizing carrier and old rows heal on read.
- Prior compaction summaries and injected-context messages no longer count
  toward the compactor's retained-turn budget; retained turns keep their
  preceding injected-context run.

## [0.7.3] - 2026-06-14

Meerkat 0.7.3 is a follow-up to 0.7.2 fixing two mob-teardown regressions
(surfaced by the e2e-smoke mob lane, which is not in CI), an e2e-system lane
gap, and two confirmed dogma violations with immediate operational effect.

### Fixed

- **Bounded comms-drain teardown await** (#770) — `unregister_session`'s
  two-phase drain awaited the comms-drain task unbounded while the runtime-loop
  handle was grace-bounded. An external member (e.g. a TCP transport drain)
  whose task does not observe cooperative abort promptly could wedge teardown.
  The comms-drain await is now bounded by a grace window + abort, matching the
  loop handle. Regular CI-gated test added.
- **Idempotent `discard_live_session`** (#770) — the teardown drain quiesces
  the runtime loop, whose clean exit discards the live session; an explicit
  caller-side discard then raced it and returned `NotFound`, breaking
  same-process mob restart. `discard_live_session` is now idempotent
  (codifies the existing `Ok(()) | Err(NotFound) => Ok(())` caller idiom).
  Regular CI-gated test added.
- **`rkat auth refresh` for `external_chatgpt_tokens`** (#770) — refreshability
  was derived from a hand-maintained raw-string allowlist that omitted the
  `external_chatgpt_tokens` OAuth-login mode, so refresh wrongly reported a
  no-op ("credentials don't expire") and never exchanged the persisted token.
  It now branches on the typed auth-method → persisted-mode owner.
- **`@rkat/web` helper profile option** (#770) — `spawnHelper`/`forkHelper`
  serialized `profile_name`, which the WASM helper deserializer silently
  dropped, so the `profileName` option built helpers with the default profile.
  The web SDK now serializes the canonical `role_name`, matching the wire
  contract and the Python/TypeScript SDKs.
- **e2e-system `cli-resume` lane** — built rkat without `memory-store-session`,
  so the yolo `memory` tool degraded to Disable and the scenario's assertion
  failed; the scenario now builds with the memory feature.

### Notes

- Nine additional confirmed/partial dogma rows with no immediate operational
  effect are tracked for a follow-up.

## [0.7.2] - 2026-06-14

Meerkat 0.7.2 hardens machine authority over shell-driven inputs. A campaign to
discipline a confirmed inventory of "undisciplined shell input" sites makes the
inputs the shell feeds canonical machine flows pass through a machine-owned
lifecycle: teardown now drains each producer class under an explicit
machine-owned obligation window, and inputs that legitimately arrive after
teardown resolve as typed no-ops instead of runtime errors. "Not currently
expected" no longer means "runtime ERROR".

### Changed

- **Machine-owned teardown drain (Layer 1)** — `MeerkatMachine` session
  unregister, `AuthMachine` OAuth-flow release, and `MobMachine` kickoff
  teardown open a machine-owned draining window: a `Begin*` transition emits one
  drain-request effect per producer class, the shell quiesces those producers
  (abort + bounded await), feedback inputs discharge the obligations, and the
  final teardown transition is guarded on every obligation being closed. A
  teardown can no longer commit while one of its producer classes is still live.
- **Total post-teardown inputs (Layer 2)** — inputs that legitimately interleave
  after a teardown window (late completion acks, drained-producer callbacks,
  post-unregister lifecycle signals) resolve as machine-owned no-op transitions
  or typed-benign dispatch rather than surfacing as runtime errors.
- **Destroy/detach obligation pairing** — effect handoff protocols pair
  `EffectTeardownClass::DestroyRequest` with the `DetachBeforeDestroy`
  obligation, with seam-inventory audits enforcing drain completeness across
  compositions.

### Notes

- The token-gating pilot (Layer 3 of the campaign) is deferred to a follow-up
  release pending a deeper review of the token model.

## [0.7.1] - 2026-06-12

Meerkat 0.7.1 restores the core/provider boundary: the model catalog returns
to a real `meerkat-models` crate and `meerkat-core` is provider-free again,
guarded by a recurrence gate. It also ships adaptive flow mobpacks and a
one-liner for switching the default model.

### Added

- **`meerkat-models` is a real crate again** (#763) — all provider model data
  (catalog entries, per-provider capability tables, image-generation profiles,
  defaults) lives in `meerkat-models`, restoring the pre-B2 public API
  (`meerkat_models::{catalog, capabilities, profile}` and the historical
  function surface). Downstream consumers that imported `meerkat_models::*`
  before 0.7.0 work again unchanged.
- **Adaptive flow mobpacks** (#762) — mobpack flows adapt at runtime.
- **`rkat --default-model <model>`** (#763) — validates against the catalog
  and configured custom models, persists `agent.model` through the
  scope-resolved config (project/user/realm), and exits when given bare or
  applies before the given command. Documented in `rkat help`'s built-in
  reference, the CLI skill, and the docs site.

### Changed

- **BREAKING (Rust API): core takes the catalog as an explicit parameter**
  (#763) — `Config::validate`, `Config::model_registry`,
  `ModelRegistry::from_config*`, `MemoryConfigStore::new`, and
  `FileConfigStore::{new, global, project}` now accept a
  `meerkat_core::ModelCatalog` (pass `meerkat_models::canonical()`).
  `Provider::infer_from_model` is removed; use
  `meerkat_models::infer_provider`. meerkat-core embeds no provider data and
  must not depend on meerkat-models (enforced by a workspace gate).
- **Default model resolves through the catalog** (#763) — fresh configs no
  longer pin a model id; an empty `agent.model` resolves through the catalog's
  per-provider defaults and the global default (`gpt-5.5`) at session-creation
  time. Existing configs with a pinned model are unaffected.
- **Mob storage removal fails closed** (#763) — `mob_destroy` now surfaces
  storage-file removal failures (with post-remove verification) instead of
  reporting a clean destroy over a surviving store.
- **Dogma ledger fixes** (#764) — the SessionDocument recovery verdict owns
  runtime-projection quarantine (no shell-side fallback); WebRTC input
  handoff failures terminate with typed client error frames; Codemob built-in
  packs require machine-owned `main` flows (local comms completion removed).
- **Adaptive pack format hardened before first release** — packs with an
  `[adaptive]` section are stamped with a required `adaptive_flow` capability
  at build time, and hosts now enforce `[requires]` fail-closed (unknown or
  unsatisfied capabilities reject the pack with the supported set named).
  Hosts older than 0.7.1 ignore `[requires]` and will silently run adaptive
  packs as static packs — deploy adaptive packs to 0.7.1+ hosts only.
  `adaptive/policies.toml` is parsed and completeness-validated at pack time
  (garbage TOML or zero limits fail the build, not the run), and
  `adaptive/layer-decision.schema.json` is emitted by the builder from the
  canonical types (hand-rolled or stale schemas fail closed with a
  regenerate hint); inline layer profiles are now structurally validated.
- **BREAKING (flow semantics): step `output_format` defaults are
  schema-aware** — an omitted `output_format` resolves to `json` when the
  step declares an expected schema and `text` otherwise (previously always
  `json`, which made schema-less free-text steps fail as "malformed JSON
  output"). Definitions that wrote the field explicitly are unchanged;
  explicit `json` without a schema now earns a definition-time warning.
  Adaptive plan flows that relied on the old implicit json default must say
  `output_format = "json"` explicitly.
- **BREAKING (CLI behavior): legacy default-model healing removed** (#764) —
  a persisted `agent.model` naming an old built-in default (e.g.
  `claude-opus-4-7`) is now used as-is instead of being silently redirected
  to the current catalog default. An explicitly configured model always wins;
  use `rkat --default-model <id>` to change it. This deliberately supersedes
  the earlier by-design adjudication that kept healing (dogma resolution
  log #254): silently overriding a user's persisted choice was itself
  surface-owned magic.

## [0.7.0] - 2026-06-11

Meerkat 0.7.0 promotes the 0.7 line from generated-authority canary to the
stable release train. The stable delta focuses on dogma-driven correctness:
typed ownership at decision points, fail-closed terminality, generated-artifact
freshness gates, and surface/runtime alignment across the Rust crates, SDKs,
WASM, REST/RPC/MCP, and release tooling.

### Added

- **Contributor-visible generated-artifact gates** (#756) — REST surface
  alignment, schema freshness, SDK codegen freshness, RPC/REST wrapper parity,
  and machine drift checks now run in the default contributor CI lanes instead
  of only owner-only BuildBuddy paths.
- **Typed governance and remediation gates** (#760) — doctrine mirror checking,
  stricter RPC SDK wrapper alignment, and machine-backed evidence for previously
  overflagged dogma rows are now part of the workspace gates.
- **Provider/auth lease ownership** (#757) — MCP OAuth freshness is bound to an
  injected per-binding `AuthMachine` lease, while provider backend and auth
  defaults now flow through typed provider-matrix owners.

### Changed

- **Typed decision ownership across surfaces** (#754, #755, #757, #760) —
  string/JSON/Option/bool re-derivation was replaced with typed owners across
  LLM identity overrides, mob bindings, auth methods, model/provider defaults,
  structured-output retry policy, SDK status parsing, and image routing.
- **Generated contracts and SDK surfaces aligned** (#756) — RPC catalog types,
  REST OpenAPI paths, MCP tool rosters, docs tables, SDK wrappers, Web SDK
  runtime version checks, and workspace crate version inheritance are now bound
  to their source authorities by tests or rerun-and-diff gates.
- **Dependency floor refreshed for the 0.7 release** (#761) — updates include
  `tokio` 1.52.3, `uuid` 1.23.3, `tempfile` 3.27.0, `toml` 0.9,
  `toml_edit` 0.25, `strum` 0.28, `fs4` 0.13, and Bazel `platforms` 1.1.0,
  with the Bazel lockfile refreshed.

### Fixed

- **Terminal faults now propagate instead of laundering to success or empty
  defaults** (#758, #759) — malformed provider stream data, incomplete
  Anthropic EOF, ops-lifecycle invariant failures, poisoned completion cursors,
  config-read failures, durable projection writes, corrupt realm leases,
  pending-event lag, SDK registration errors, RPC result serialization failures,
  and session-store sidecar faults now retain typed fault information.
- **Mob/comms trust and routing correctness** (#754, #759, #760) — typed member
  bindings fixed the `mob.{id}`/`mob:{id}` persisted-session mismatch, comms
  namespace authority moved behind typed peers, supervisor trust rollback is now
  scope-correct and machine-owned, and bridge acknowledgements carry canonical
  truth.
- **Auth and surface correctness regressions** (#755, #759, #760) — direct-secret
  auth profile creation no longer rejects valid methods, scheduled sessions now
  honor configured structured-output retries, token writes acquire durable
  lifecycle markers before persisting bytes, and SDK/WASM parsing paths fail
  closed on unknown or malformed wire data.

## [0.7.0-alpha.0] - 2026-06-04

Meerkat 0.7.0-alpha.0 publishes the generated-authority canary for downstream
testing. It includes breaking wire and SDK-surface changes, so downstream
canaries should pin exact dependencies with `=0.7.0-alpha.0`.

### Changed

- **Supervisor bridge protocol bumped to V3 (breaking wire change)** — peer
  wiring commands (`WireMember`/`UnwireMember`) now carry the MobMachine peer
  overlay. The protocol negotiates cleanly across versions: V2 is still accepted
  for persisted authority records and pre-overlay peers, a V3 payload sent to a
  V2 binary is rejected with a typed `UnsupportedProtocolVersion` (not a
  deserialization error), and a V2 wiring payload received by a V3 binary is
  rejected with the same typed cause rather than silently failing. **Mixed-version
  distributed mobs cannot wire peers across the V2/V3 boundary — upgrade all
  hosts in a distributed mob together.**

### Removed

- **`reachability` / `last_unreachable_reason` removed from `PeerDirectoryEntry`
  (breaking wire change)** — peer reachability is no longer projected onto the
  wire directory entry. SDK consumers pinned to an earlier release that read
  these fields must update; the 0.x patch-compatibility predicate does not
  signal this removal.

### Deprecated / Compatibility

- **Pre-`status_info` event logs are not resumable** — session event logs written
  before the `ToolConfigChanged` event gained its structured `status_info` field
  (roughly v0.4–v0.5) that recorded only the legacy `status` string can no longer
  be replayed; resuming such a session fails fast with a clear error. Re-create
  the session. Logs written by current releases replay unchanged (regression-tested).

## [0.6.34] - 2026-06-03

Meerkat 0.6.34 adds interactive OAuth for streamable HTTP MCP servers and
promotes the Homebrew tap install path across the public docs.

### Added

- **Interactive OAuth for HTTP MCP servers** (#752) — adds `rkat mcp add` URL
  forms for Codex/Claude-compatible HTTP MCP servers, runtime-discovered OAuth
  with DCR, PKCE, refresh, stored and interactive modes, plus `rkat mcp login`.
- **Homebrew install documentation** (#753) — promotes
  `brew install lukacf/meerkat/rkat` in the README and quickstart, documents
  macOS/Linux tap support, and expands distribution docs for tap credentials and
  companion binaries.

## [0.6.33] - 2026-06-03

Meerkat 0.6.33 adds paired TCP comms for remote mob targets and hardens
supervisor bridge completion for signed remote sessions.

### Added

- **Paired TCP comms for remote mob targets** (#751) — adds `rkat run` comms
  flags for signed TCP listeners, external binding output, pairing password
  sources, and target metadata labels, plus password-proof pairing that installs
  trusted peers without sending the raw password.

### Fixed

- **Remote supervisor bridge completion** (#751) — returns bridge delivery
  completion through the comms drain and local bridge so external mob targets
  can complete supervisor requests reliably.
- **Remote mob shutdown cleanup** (#751) — stops the mob supervisor bridge on
  shutdown and normalizes idle peer `steer` admission so execution runs through
  the runtime loop as `queue`.

## [0.6.32] - 2026-06-02

Meerkat 0.6.32 fixes external supervisor bridge responses for remote mob
members after bind and wire trust changes.

### Fixed

- **External supervisor bridge response routing** (#750) — installs a scoped
  response route from the supervisor descriptor carried in bridge payloads,
  preserves existing supervisor trust after replies, routes bind-validation
  failures back to verified requesters, and keeps wire/unwire acknowledgements
  reliable across idempotent trust-projection races.

## [0.6.31] - 2026-06-02

Meerkat 0.6.31 fixes supervisor bridge response routing, compaction retry
cadence, and OpenAI streaming completion fallback behavior.

### Fixed

- **Supervisor bridge response routing** (#748) — installs an authenticated
  supervisor response route before authorized bridge replies, preferring
  private trusted-peer registration so supervisor routing does not leak through
  public peer discovery.
- **Compaction retry cadence** (#749) — persists failed compaction attempt
  boundaries and feeds them into the compaction cadence guard so sessions over
  threshold do not immediately retry the same failing compaction path.
- **OpenAI stream done fallback** (#749) — treats `data: [DONE]` as a terminal
  fallback only after streamed output, tool, or reasoning content has actually
  been observed.

## [0.6.30] - 2026-06-01

Meerkat 0.6.30 fixes production external TCP bridge replies and compacted
singleton retire/archive recovery.

### Fixed

- **External bridge reply routing** (#746) — repairs target-runtime trust
  projection before idempotent `BindMember` acknowledgements, and decodes the
  canonical `BridgeReply` envelope emitted by production `comms_drain` so
  external TCP member binds can complete the supervisor round trip.
- **Compacted singleton retire stranding** (#747) — lets runtime-backed session
  projections persist after a legitimate compaction when the durable store lags
  across the compaction boundary, avoiding `MonotonicityViolation` during
  archive.
- **Mob disposal hardening** (#747) — removes dead roster anchors even when
  `ArchiveSession` fails, so respawn/reset can recover without requiring a
  process restart.

## [0.6.29] - 2026-06-01

Meerkat 0.6.29 fixes remote external mob binding so TCP-backed members can
reply through a routable supervisor bridge.

### Fixed

- **Remote external bind routing** (#744) — gives mob supervisor bridge
  runtimes a signed TCP listener while preserving in-process descriptors for
  local members, and uses recipient-aware supervisor descriptors so TCP
  external members bind and reply through a routable supervisor address.
- **External TCP bind regression coverage** (#744) — adds coverage for a TCP
  external bind followed by a peer turn on the remote runtime.

## [0.6.28] - 2026-05-31

Meerkat 0.6.28 updates the Anthropic default catalog target to Claude Opus
4.8 and refreshes the surrounding examples, docs, and capability metadata.

### Changed

- **Anthropic Opus 4.8 default** (#743) — promotes the default Anthropic
  catalog model from `claude-opus-4-7` to `claude-opus-4-8`, retargets
  examples/tests/docs away from older Opus defaults, and keeps Opus 4.7 only
  where it remains an intentional legacy or fallback catalog entry.
- **Opus 4.8 capability metadata** (#743) — documents the current Opus 4.8 API
  behavior around adaptive thinking, `xhigh` effort, fast mode, beta/header
  handling, and unsupported non-default sampling knobs.

## [0.6.27] - 2026-05-28

Meerkat 0.6.27 hardens mob lifecycle cleanup when session-bound members fail
or pending spawns are cancelled.

### Fixed

- **Mob cleanup retry anchors** (#742) — keeps respawn and retire cleanup
  anchors retryable after archive/unregister failures, tracks pending-spawn
  cancellation cleanup as a retryable lifecycle operation, and fails lifecycle
  commands closed when archive cleanup is ambiguous instead of dropping roster
  truth early.

## [0.6.26] - 2026-05-27

Meerkat 0.6.26 fixes runtime-backed mob session continuity after transcript
rewrite and checkpoint projection failures.

### Fixed

- **Mob runtime session continuity** (#741) — preserves runtime authority
  across transcript rewrite projections, bridges storage-normalized append
  histories after media externalization, and keeps checkpoint quarantine from
  promoting unrelated store-only session projections.

## [0.6.25] - 2026-05-27

Meerkat 0.6.25 is a release packaging hotfix for Windows native assets.

### Fixed

- **Cross-platform JSONL session write locks** — replaces the Unix-only
  `nix::fcntl` JSONL session lock with a portable `fs4` file lock so hosted
  BuildBuddy Windows release builds compile the session store.

## [0.6.24] - 2026-05-27

Meerkat 0.6.24 adds same-session transcript rewrite/restore flows and
WorkGraph-backed goal attention surfaces.

### Added

- **Same-session transcript rewrites** (#739) — adds durable transcript
  rewrite revision history, restore support, canonical audit events, and
  REST/RPC/Rust/Python/TypeScript/Web SDK surfaces for rewrite, revision, and
  restore flows while preserving full assistant block traces.
- **WorkGraph goals and attention bindings** (#740) — adds high-level goal
  creation/status flows, attention-bound continuations, scoped WorkGraph tool
  authority, and REST/RPC/CLI/SDK observability for goal and attention state.

### Changed

- **WorkGraph machine authority reference** (#740) — documents WorkGraph
  attention as a canonical lifecycle machine and composition protocol, including
  scoped continuation, projection currentness, and goal completion policy
  behavior.

## [0.6.23] - 2026-05-22

Meerkat 0.6.23 is a mob runtime hotfix release for nonblocking observation,
profile-scoped auto-wiring, and active-turn steer admission.

### Fixed

- **Mob observation during active turns** (#738) — broad mob observation now
  reads actor-published snapshots instead of querying each mob actor, so
  `mob_list` and profile observation do not block behind in-flight member turns.
- **Profile-scoped auto-wire spawns** (#738) — profile-scoped legacy
  `spawn_member` calls can request `auto_wire_parent` without requiring full
  manage authority, while preserving owner/profile context for agent mob spawns.
- **Active-turn steer admission** (#738) — keeps operator and member steers
  runnable during active turns, routes staging through runtime-backed live
  boundary probes, keeps runtime steers transient, and rolls back staged live
  boundary state on commit failure.

### Changed

- **Release packaging Python selection** (#738) — release packaging checks now
  prefer Python 3.11 for more predictable local packaging validation.

## [0.6.22] - 2026-05-21

Meerkat 0.6.22 is a mob runtime hotfix release for turn-completion caller
semantics and actor-loop responsiveness.

### Fixed

- **TurnCompleted actor-loop responsiveness** (#737) — preserves
  `TurnCompleted` caller semantics while moving the runtime-completion wait out
  of the serialized mob actor command loop, allowing same-mob observation and
  mutation commands such as member listing and spawn to proceed while the
  turn-driven runtime call is still in flight.

## [0.6.21] - 2026-05-20

Meerkat 0.6.21 is a runtime hotfix release for interrupt-yielding live steer
injection during active runs.

### Fixed

- **Interrupt-yielding live steer injection** (#736) — adds a machine-owned
  live boundary path for accepted steer inputs during active runs, projecting
  them into pending system context so interrupt-yielding peer messages can be
  consumed at the next inner boundary without cancelling or replaying the
  active run.

## [0.6.20] - 2026-05-20

Meerkat 0.6.20 refreshes Gemini model guidance and fixes mob peer delivery
lifecycle behavior under backpressure.

### Changed

- **Gemini model and post-0.6.5 guidance refresh** (#734) — updates the
  featured Gemini text model to `gemini-3.5-flash` across the catalog, default
  provider model, tests, examples, public docs, and Meerkat skill guidance.

### Fixed

- **Classified inbox wake and peer delivery lifecycle** (#735) — fixes
  classified inbox capacity wake registration, wakes senders on close, and
  moves mob peer-message delivery out of the actor command loop so
  backpressured recipients do not wedge unrelated mob handle calls.

## [0.6.19] - 2026-05-19

Meerkat 0.6.19 is a runtime/session projection hotfix release for
runtime-committed session checkpointing.

### Fixed

- **Runtime-committed session projections** (#733) — checkpoints committed
  runtime snapshots back into the `SessionStore` projection after the machine
  commit succeeds, restoring per-turn projection saves for MobKit and
  UnifiedRuntime consumers without introducing a pre-commit split-brain.

## [0.6.18] - 2026-05-19

Meerkat 0.6.18 is a runtime/session reliability hotfix release for persistent
session checkpointing and lost mob session state.

### Fixed

- **Runtime-store session checkpointing** (#731) — fixes persistent session
  checkpointing through the runtime store so session state is written and
  recovered through the intended storage path.
- **Lost mob session status** (#732) — marks lost mob sessions as broken,
  preserving an explicit failed state instead of leaving unavailable sessions
  looking active or recoverable.

## [0.6.17] - 2026-05-18

Meerkat 0.6.17 improves mob spawn boundary configuration, task-workflow
guidance, and steer cancellation behavior.

### Added

- **Mob spawn boundary customization** (#728) — mob spawn flows can customize
  member spawn boundaries through the mob runtime path, with tests covering the
  builder, runtime handle, actor, and tool surfaces.
- **Mob task workflow guidance preload** (#729) — mob build profiles now preload
  task workflow guidance so spawned members receive the expected workflow context
  at construction time.

### Fixed

- **Steer admission boundary cancellation** (#730) — fixes cancellation behavior
  at the steer admission boundary and updates the machine contract/spec coverage
  around the cancellation path.

## [0.6.16] - 2026-05-18

Meerkat 0.6.16 is a hotfix release for mob peer wake behavior and
BuildBuddy-backed Windows release assets.

### Changed

- **Mob peer send and tool bridge parity** (#727) — aligns mob peer send
  behavior with the tool bridge path so peer-to-peer wake and delivery semantics
  stay consistent across mob surfaces.
- **Windows BuildBuddy release endpoint** (#726) — Windows release binary builds
  now route directly to the hosted King BuildBuddy endpoint instead of a mutable
  secret-backed endpoint, preventing stale private endpoint routing from
  breaking release asset publication.

### Fixed

- **Peer wake interrupt semantics** (#727) — fixes peer wake interruption so
  mob members are woken correctly when new peer traffic arrives.

## [0.6.15] - 2026-05-18

Meerkat 0.6.15 is a smoke-test hotfix release for mob lifecycle, image comms,
and release asset reliability.

### Changed

- **Release asset routing** (#717) — BuildBuddy release assets now cover
  validation plus Linux and macOS binaries, while the Windows release binary is
  always built on GitHub-hosted runners to avoid the known Windows remote
  execution input-tree failures.

### Fixed

- **E2E smoke mob lifecycle and image comms** (#718) — fixes the mob lifecycle
  and generated-image communication regressions found by e2e smoke testing,
  including runtime/session handling and MCP/tool-surface behavior needed for
  the smoke suite to pass again.

## [0.6.14] - 2026-05-17

Meerkat 0.6.14 adds batched local mob-member wiring, with public RPC, REST, MCP,
Python, and TypeScript SDK surfaces.

### Added

- **Batch mob topology materialization** (#715) — mobs can now wire dense local
  member graphs through `MobHandle::wire_members_batch(...)`, preserving
  MobMachine ownership of peer wiring truth while coalescing roster projection
  and event replay into a compact `MembersWiredBatch` event.
- **Batch wiring public surfaces** (#716) — `mob/wire_members_batch`,
  `POST /mob/{id}/wire-members-batch`, `meerkat_mob_wire_members_batch`,
  `MeerkatClient.mob_wire_members_batch(...)`, `Mob.wire_members_batch(...)`,
  `client.mobWireMembersBatch(...)`, and `mob.wireMembersBatch(...)` now expose
  the local-member batch wiring path across JSON-RPC, REST, MCP, Python, and
  TypeScript.

### Changed

- **Burst mob delivery backpressure** (#715) — runtime-originated in-process
  peer sends, lifecycle notifications, peer-retire fanout, mob command capacity,
  and deferred turn-event buffering now handle bursty dense mob workloads without
  converting full queues into semantic message loss.

## [0.6.13] - 2026-05-16

Meerkat 0.6.13 is a hotfix release for committed lifecycle and runtime effect
delivery under burst mob load.

### Fixed

- **Committed lifecycle backpressure** (#713) — mob lifecycle signal projection
  and committed runtime effect delivery now use backpressured sends instead of
  fail-fast bounded queue sends, preserving semantic lifecycle/effect facts
  when large mob fan-outs temporarily fill actor queues.
- **Rollback cleanup peer absence** (#713) — rollback compensation now treats
  typed `PeerNotFound` send errors as benign already-absent cleanup fallout
  while preserving other rollback failures.

## [0.6.12] - 2026-05-15

Meerkat 0.6.12 is a hotfix release for autonomous mob member capability
validation and active-turn retirement safety.

### Fixed

- **Autonomous member injector capability** (#712) — autonomous mob members
  must now expose `interaction_event_injector` before spawn/resume dispatch
  treats them as seated and active, surfacing missing injector support as typed
  `MissingMemberCapability` / `missing_member_capability` errors.
- **Mob retire active-turn teardown** (#711) — runtime retirement now waits for
  archive/unregister coordination and machine-observed quiescence before
  unregistering, avoiding races where an active turn could emit state
  transitions after the runtime was marked retired.

## [0.6.11] - 2026-05-15

Meerkat 0.6.11 is a hotfix release for turn-driven mob spawn prompt delivery.

### Fixed

- **Turn-driven spawn initial messages** (#710) — explicit initial messages for
  turn-driven mob spawns and respawns are now submitted through `SubmitWork`
  after the member is live, preserving caller intent instead of silently
  dropping the prompt during deferred session initialization.

## [0.6.10] - 2026-05-14

Meerkat 0.6.10 is a hotfix release for mob delegate authority inheritance.

### Fixed

- **Mob delegate authority inheritance** (#708) — child delegates can now
  inherit explicit profile-spawn authority within a mob without being granted
  full mob-management scope, preserving the typed authority boundary while
  allowing nested mob delegation workflows to continue.
- **Reasoning-only LLM completions** (#709) — assistant turns that only emit
  reasoning content or provider continuity metadata now satisfy core commit
  validation, and streamed reasoning deltas are finalized on successful
  completion so GPT-5.5-style silent reasoning responses are preserved.

## [0.6.9] - 2026-05-14

Meerkat 0.6.9 is a hotfix release for turn cancellation and multimodal mob
comms delivery, plus release-lane recovery improvements from the 0.6.8 rollout.

### Changed

- **Web SDK release recovery** — release workflows can now publish `@rkat/web`
  from an already-built `rkat-web-package` artifact, allowing npm publish
  retries without rebuilding the WebAssembly runtime.
- **Web SDK release build parallelism** — the Web SDK release package build no
  longer serializes Cargo work, reducing cold release-package build time on the
  GCP runner lane.

### Fixed

- **Steer-boundary cancellation** (#706) — fixed turn-state cancellation around
  steer boundaries so interrupted or redirected turns do not leave stale
  cancellation state in the runtime.
- **Multimodal comms notices** (#707) — fixed multimodal comms notice delivery
  across Anthropic, Gemini, OpenAI, and OpenAI-compatible providers so generated
  image notices and related mob comms survive provider-specific content
  projection.
- **Web SDK package/archive checks** — fixed release packaging checks that could
  fail a valid `@rkat/web` tarball under `pipefail`, and made npm publish use an
  absolute tarball path so local recovery artifacts are not interpreted as git
  package specs.

## [0.6.8] - 2026-05-13

Meerkat 0.6.8 is a hotfix release for long-context model compaction defaults
and Web SDK release publishing.

### Changed

- **Model-aware compaction defaults** (#704) — default session compaction now
  scales from the selected model catalog context window, using 80% of the
  resolved context window for known long-context models while preserving
  explicit custom thresholds and the conservative fallback for unknown models.
- **Web SDK release publishing** (#705) — release workflows now build the
  `@rkat/web` package once as an artifact and publish that tarball from the
  credentialed npm step, so Rust/Python/TypeScript publishing no longer waits on
  a cold WebAssembly rebuild.

### Fixed

- **OpenAI long-context sessions** (#704) — `gpt-5.5` and other large-context
  catalog models no longer compact prematurely at the old static 100k-token
  threshold.
- **Web SDK npm metadata** (#705) — normalized the `rkat-web-proxy` bin path so
  npm publish no longer auto-corrects the package metadata.

## [0.6.7] - 2026-05-13

Meerkat 0.6.7 is a feature-bearing release: WorkGraph,
Azure OpenAI, project-local CLI defaults, HTML artifacts, typed transcript
notices, capability companion skills, provider image/search improvements, and
several runtime/provider fixes.

### Added

- **WorkGraph subsystem** (#684, #688, #689, #694, #696, #699, #702) — added
  `meerkat-workgraph`, a realm-scoped durable commitment graph with work
  items, namespaces, labels, priorities, due/not-before/snooze gates, owners,
  claim leases, external references, evidence references, topology edges,
  revision/CAS checks, event history, ready-item derivation, snapshots, and the
  catalog-owned `WorkGraphLifecycleMachine`.
- **WorkGraph agent tools and host observability** (#684, #699, #702) — agents
  can mutate WorkGraph through `workgraph_create`, `workgraph_claim`,
  `workgraph_release`, `workgraph_update`, `workgraph_block`,
  `workgraph_close`, `workgraph_link`, and `workgraph_add_evidence`, while
  CLI/RPC/REST/SDK callers get read-only lookup through `list`, `show/get`,
  `ready`, `snapshot`, and `events` surfaces.
- **WorkGraph SDK support** (#684, #702) — Python and TypeScript clients now
  forward `enable_workgraph` / `enableWorkGraph` on session creation and expose
  typed read-only WorkGraph APIs for items, ready work, snapshots, and events.
- **Azure OpenAI backend support** (#700) — OpenAI provider bindings can now
  target Azure OpenAI through `backend_kind = "azure_openai"` and
  `auth_method = "azure_api_key"`, including Azure endpoint normalization,
  `api-key` auth, deployment-name model selection, image-generation deployment
  options, Azure image headers, and RKAT-prefixed env overrides.
- **HTML output mode** (#693) — `rkat run` now supports `--output html`,
  `--html`, `--browser`, `--open-in-browser`, `--html-template`, and
  `--html-template-file`, writing standalone HTML artifacts under the active
  realm's presentation output.
- **Project-local CLI realms** — CLI `run`, `run --resume`, and `session`
  commands now default to a workspace-derived `ws-...` realm under
  `<context-root>/.rkat/realms`, with `--context-root`, `--state-root`,
  `--realm`, and `--isolated` preserving explicit control.
- **Provider-native search/image delegation** (#682, #691) — Anthropic,
  Gemini, and OpenAI gained provider-native search fallback executors; OpenAI
  hosted image generation now supports `provider_params.reasoning_effort` and
  `provider_params.web_search` on the hosted `gpt-image-2` path.
- **Typed capability registry** — added `meerkat-capabilities` as the typed
  capability vocabulary and feature-owned registration point for sessions,
  streaming, structured output, hooks, builtins, shell, comms, memory,
  schedule, WorkGraph, session store, compaction, skills, MCP live, and live.
- **Companion workflow skills** (#694) — added/expanded embedded companion
  skills for WorkGraph, scheduling, skill discovery, and built-in utility
  workflows so tool descriptions can stay schema-oriented while operational
  guidance lives in skills.
- **Dogma and WorkGraph documentation** (#689, #694) — added public Meerkat
  dogma docs, the `meerkat-dogma-inquisition` skill, WorkGraph concept/guide/
  reference/example docs, realm docs, Azure OpenAI docs, HTML-output docs, and
  capability/skill reference updates.

### Changed

- **Default CLI model is OpenAI** — the CLI now defaults to OpenAI `gpt-5.5`,
  with provider-aware default-model resolution that follows auth binding
  provider/defaults, repairs legacy built-in defaults, and preserves explicit
  user model choices.
- **Runtime metadata is typed transcript data** (#686) — comms, external
  events, MCP changes, tool config, auth, background jobs, and runtime notices
  are persisted as typed `system_notice` blocks instead of being reclassified
  from `[SYSTEM NOTICE]` user-text prefixes. Provider-facing notice text is now
  a projection assembled for the model, not the stored operator prompt.
- **Mob task-board surface retired** — the old mob task-board model and
  `mob_tasks` profile knob were removed in favor of agent-facing mob tools and
  WorkGraph for durable cross-agent commitments.
- **BuildBuddy and release preflight hardened** — release preflight now runs
  `scripts/release-doctor`; BuildBuddy runs keep pending jobs queued; release
  backend selection, wasm-pack checks, npm/crates readiness, and Web SDK
  recovery timeouts were tightened.
- **OpenAI request behavior tightened** — image and Responses API calls now
  use `store: false` where appropriate, hosted image generation streams through
  the ChatGPT backend path, and OpenAI/Azure request construction is more
  explicit about backend-specific headers and URLs.
- **Skills identity tightened** — skill references now use structured
  `SkillKey { source_uuid, skill_name }` identity, legacy string refs are
  rejected on the wire, and large skill collections use a collection summary
  mode rather than injecting all content eagerly.

### Fixed

- **CLI session handle ambiguity** — short session handles now use the UUID
  tail, while resume accepts either prefix or tail and ambiguous matches report
  fuller realm/session refs, avoiding UUIDv7 timestamp-prefix collisions such
  as repeated `019e...` handles.
- **WorkGraph runtime gaps** (#688, #689, #702) — WorkGraph machine authority
  is durable, legacy refresh paths are hardened, invalid graph topology is
  rejected, and WorkGraph/delegate runtime wiring stays available across turns.
- **OpenAI image and stream failures** (#691, #692, #695) — OAuth-backed image
  generation, streamed image error events, HTTP failure bodies, compaction
  stream failures, `response.failed` handling, and image smoke timeout behavior
  now surface as structured provider/runtime failures instead of timeouts or
  parse surprises.
- **Anthropic/Claude OAuth edge cases** — fixed Anthropic OAuth scope refresh
  failures, Claude AI OAuth request envelopes, beta header composition, and
  rate-limit routing via the `x-app` header.
- **Deferred session and mob runtime behavior** (#697, #701, #702) — fixed
  deferred session context promotion, forwarded CLI mob runtime apply, and kept
  mob wiring responsive while turns are in flight.
- **Azure OpenAI regressions** (#700) — fixed CI regressions, env precedence,
  endpoint normalization, image-deployment edge cases, and auth-contract
  vocabulary coverage for Azure OpenAI.
- **Typed notice regressions** (#686) — fixed CLI typed notice display, clippy
  issues, typed turn-gate behavior, smoke summaries, and provider projections.
- **HTML artifact output edge cases** (#693) — fixed browser/open handling and
  artifact output behavior for HTML mode.

## [0.6.6] - 2026-05-11

Meerkat 0.6.6 is a patch release for live transport smoke coverage, session
scheduling ergonomics, auth binding fallback behavior, inherited mob tooling,
and release/documentation polish on top of the 0.6.5 live-adapter release.

### Added

- **Current-session schedule targets** (#671) — scheduler tools now accept a
  `current_session` shortcut for create/update calls and persist it as the
  concrete running `resumable_session` target during agent execution.
- **Live WebRTC smoke transport** (#676) — the JSON-RPC live path now has a
  WebRTC smoke-test surface and live controller peer ingress forwards helper
  and delegate comms into the active live adapter.
- **Docs validation gate** (#672) — public docs now include a Mintlify
  validation command that checks navigation, frontmatter, links, anchors,
  fences, orphan pages, and generated HTML before publishing.

### Changed

- **Auth binding fallback resolution** (#674) — agent construction now scans
  configured realm bindings when provider/model/auth binding are omitted, while
  preserving strict behavior for explicit provider, model, or binding requests.
- **Public docs refresh** (#672) — architecture, mobs, self-hosting, runtime,
  machine, and build documentation were rewritten around the current 0.6.5+
  surfaces and obsolete/internal public pages were removed from navigation.
- **Dependency refresh** (#663, #664, #665) — updated `apple_support`,
  `rules_cc`, and `tower-http` to keep toolchain and HTTP middleware
  dependencies current.
- **Release workflow hardening** — release CI now accepts the reusable Cargo
  gate shape, uses native Rust toolchain executables on Windows, and keeps Web
  SDK recovery builds alive while serializing the expensive WASM publish build.

### Fixed

- **Inherited mob tooling category caps** (#677) — inherited parent-visible tool
  filters no longer get silently capped by the selected profile's tool category
  booleans before the inherited allow-list is applied.
- **Live peer ingress visibility** (#676) — comms tools remain visible before
  trusted peers are wired so live sessions can use peer and message tools after
  later live wiring.
- **Auth binding model precedence** (#674) — explicit model selections now stay
  ahead of resolved binding `default_model` values after a fallback binding is
  selected.

## [0.6.5] - 2026-05-10

Meerkat 0.6.5 ships the live-adapter MVP and a SessionRuntime split that moves
runtime ownership into clearer session-runtime modules while keeping public
surfaces aligned through regenerated SDKs, schemas, and release packaging.

### Added

- **Live adapter MVP** (#659) — `meerkat-live` provides the composable
  WebSocket transport for live channels, `rkat-rpc --live-ws` exposes
  live/open token flow end to end, and the OpenAI realtime bridge now supports
  live input, output, interruption, and refresh observations through typed
  public contracts.
- **Live channel SDK helpers** (#659) — Python and TypeScript SDKs gain typed
  live/open and live-channel helpers backed by generated wire contracts,
  including named payload types for inline variants and explicit unknown
  variants for forward-compatible mirrors.
- **SessionRuntime split** (#659) — runtime admission, staged promotion,
  recovery, live orchestration, LLM reconfiguration, skill identity, and runtime
  state observers now live behind session-runtime modules instead of the older
  monolithic runtime shape.

### Changed

- **Realtime model catalog** (#659) — live realtime support is aligned with the
  current `gpt-realtime-2` catalog and model-affecting config changes propagate
  to live channels without overwriting per-session model overrides.
- **Live public boundary typing** (#659) — live adapter status, observations,
  refresh results, config rejection reasons, transcript sources, modality
  continuity, and transport results now use typed wire contracts rather than
  opaque values or string detail fields.
- **Release surface** (#659) — the new live-adapter crates are included in the
  release crate list and generated Bazel metadata, keeping packaging and
  BuildBuddy release validation in sync.

### Fixed

- **Live session lifecycle** (#659) — deferred-session promotion, duplicate
  live channel rejection, channel ownership cleanup, provider EOF propagation,
  cancel-safe receive handling, and text-only response requests are covered by
  regression tests and typed runtime observations.
- **Live WebSocket and examples** (#659) — example protocol usage, docs, and
  smoke scenarios were refreshed for the live/open flow and the deleted legacy
  realtime channel surface.
- **BuildBuddy batch diagnostics** (#660) — CI batch diagnostics now preserve
  clearer lane-level failure context for release and validation runs.

## [0.6.4] - 2026-05-08

### Fixed

- **Release package publishing** — Rust crate publishing now follows a
  dependency-ordered crate list, streams per-crate `cargo publish` logs, applies
  a bounded timeout, and skips Cargo's duplicate verifier during real uploads
  because release validation already packages and links the published surface.
- **Web SDK publishing** — npm Web SDK publish steps now publish the artifact
  already built by the workflow instead of re-running the expensive wasm build
  through `prepublishOnly`; Web SDK recovery also restores the Rust cache and a
  bounded job timeout.
- **BuildBuddy binary release metadata** — generated Bazel Rust targets now set
  `CARGO_PKG_VERSION` from the workspace package version, and release packaging
  rejects binaries that do not embed the requested release version.

## [0.6.3] - 2026-05-08

Meerkat 0.6.3 is a patch release for published Rust crate consumers. It fixes
the AgentFactory facade/core bridge symbol selection in registry layouts and
adds a packaged-crate downstream link smoke to the release gate.

### Fixed

- **Published facade linking** — `meerkat` now resolves the exact same-version
  `meerkat-core` package when computing the private AgentFactory policy bridge
  symbol, preventing downstream native link failures when older registry
  versions are cached locally.

## [0.6.2] - 2026-05-08

Meerkat 0.6.2 is a patch release for runtime authority, provider/tooling polish, blob/file tooling, OpenAI replay continuity, and the recovered BuildBuddy release path. It makes the runtime lifecycle spine machine-owned, improves model/tool visibility and multimodal handling, and simplifies published release assets to the four supported public runtime binaries.

### Added

- **Blob file tools** (#648) — agents can read, write, inspect, and route blob-backed files through the builtin utility tool surface.
- **Machine-owned runtime lifecycle spine** (#636) — runtime lifecycle authority now flows through the machine-owned spine, keeping lifecycle decisions under the generated machine contract.
- **OpenAI replay continuity** (#648) — OpenAI response replay preserves continuity across provider-native tool and content events.
- **Provider replay projection contract** (#639) — provider replay facts now have a typed projection contract for downstream replay/debug consumers.
- **Typed transcript fork/edit API** (#642) — transcript fork and edit flows are exposed through typed runtime APIs instead of ad hoc mutation paths.
- **Universal agent LLM client decorator** (#643) — agent LLM clients can be wrapped consistently across providers and runtime surfaces.
- **Resolved model capability visibility** (#645) — resolved model capabilities are surfaced explicitly so clients can inspect provider/model feature availability.
- **Canonical assistant image event** — assistant image generation now has a canonical event shape for runtime and SDK consumers.

### Changed

- **Provider web search defaults** (#646) — provider-native web search is enabled by default when the selected model supports it.
- **Release artifacts** — GitHub Releases and Homebrew now publish only `rkat`, `rkat-rpc`, `rkat-rest`, and `rkat-mcp`; all mini binaries are source-build-only custom profiles.
- **BuildBuddy release lanes** — BuildBuddy release builds now target the same four public runtime binaries as the GitHub-hosted release path.
- **OpenAI realtime dependency** — Meerkat now uses the published `oai-rt-rs` crate instead of an unpublished/local realtime dependency.
- **Release validation** — release readiness now parallelizes independent contract, Rust packaging, Python SDK, and TypeScript SDK checks inside the BuildBuddy validation lane.

### Fixed

- **BuildBuddy release backend** — full and asset-recovery release workflows can now select the BuildBuddy-backed release binary path while leaving the public GitHub-hosted path as the default.
- **BuildBuddy release diagnostics** — `buildbuddy-doctor` now enforces the no-mini public release surface and verifies the BuildBuddy release branch selection.
- **Cross-platform release assets** — BuildBuddy release packaging was repaired for Linux arm64, macOS arm64/x86_64, and Windows asset collection/output layout.
- **Private BuildBuddy endpoint handling** — enterprise BuildBuddy endpoint selection stays secret-scoped and owner-only; public contributors continue through the standard GitHub-hosted release path.
- **Rust package verification** — Rust release packaging and publish dry-runs work on Bash 3 and preserve the private `meerkat-core` bridge lookup needed by the facade crate during Cargo package verification.
- **Release recovery flows** — asset-only and Web SDK recovery lanes now rebuild from the release tag and skip unrelated registry/validation work.
- **Release recovery cleanup** — removed the obsolete one-off mini asset repair workflow so future repairs cannot accidentally republish mini binaries.
- **Mob executor and SDK realtime edge cases** — fixed a mob executor `handling_mode` leak and Python SDK realtime deserialization issue.
- **LLM error reporting** — provider/runtime LLM errors now surface with clearer structured context.

## [0.6.1] - 2026-05-06

Meerkat 0.6.1 is a focused patch release for provider-native tool visibility. It restores provider web search plumbing across request injection and response capture, and adds a typed profile visibility seam for image generation in mob/session tooling.

### Added

- **Provider-native search evidence** (#637) — server-executed search output now flows through typed `ServerToolContent` blocks across LLM events, assistant blocks, agent events, and wire projections. Anthropic, OpenAI, and Gemini search/grounding metadata are captured instead of being dropped as unknown content.
- **Image-generation profile visibility** (#638) — `tools.image_generation` now maps through `ToolCategoryOverride` into session build config and persisted session tooling, so mob profiles can explicitly expose or hide `generate_image` without conflating visibility with substrate availability.

### Fixed

- **Provider web search defaults** (#637) — provider-native web search defaults are no longer suppressed by Meerkat tool category overrides when the selected model supports web search.
- **Image-generation dispatcher rebinding** (#638) — image-generation visibility overrides are preserved when dispatcher state is rebound, keeping resumed/recovered sessions aligned with the profile contract.

## [0.6.0] - 2026-05-05

Meerkat 0.6.0 is the machine-authority release. It converges the runtime onto **five canonical machines** generated from a single DSL source of truth, lands **identity-first live voice** so realtime attachment is keyed on stable `AgentIdentity` rather than per-runtime bindings, completes the AuthMachine OAuth freshness model under scoped leases, types every major runtime contract that previously rode on strings or `serde_json::Value`, hardens fail-closed semantics across the runtime/REST/RPC/WASM surfaces, makes durable event storage sequence-authoritative, retires the last legacy compatibility paths from peer ingress and runtime visibility, and ships a realtime audio example plus a documentation refresh for every shipping surface.

This is a breaking release. Wire contracts that previously accepted free-form strings now require typed variants (`TerminalCause`, `CommsIntent`, `CommsResult`, `HookId`, `terminal_status`, `MemberSessionBinding`, `ProviderRuntimeBackend`, etc.). Several legacy session/runtime verbs and the `mob/realtime_attach` / `mob/realtime_detach` REST endpoints have been removed. SDK consumers should regenerate against `meerkat-contracts@0.6.0`.

### Added

#### Machine-authority architecture
- **Single-source machine DSL** (#259) — new `machine_dsl!` proc macro with parser and code generators emits runtime dispatch, phase projection, input/signal/effect enums, state structs, and `MachineSchema` artifacts from one DSL body. Catalog DSL (`meerkat-machine-schema/src/catalog/dsl/`) is the sole source of truth for production machine semantics.
- **Two-kernel DSL cutover** (#259) — handwritten authorities absorbed into canonical machines; production modules become bridge shells around catalog-owned DSL bodies. The old hand-written machine catalog has been deleted.
- **Five canonical machines** — `MeerkatMachine`, `MobMachine`, `ScheduleLifecycleMachine`, `OccurrenceLifecycleMachine`, and `AuthMachine` (split out of `MeerkatMachine` in 0.6) are now the only canonical machines, each with a DSL source under `meerkat-machine-schema/src/catalog/dsl/`.
- **Composition-protocol seams** — five typed composition protocols formalized at the seams: `meerkat_mob_seam`, `schedule_bundle`, `schedule_runtime_bundle`, `schedule_mob_bundle`, `auth_lease_bundle`.
- **Catalog/production parity gates** — `runtime_schema_parity` and `runtime_alphabet_parity` are CI-enforced ratchets; string whitelists for command classification are forbidden, replaced by typed alphabet manifests.
- **TLA+ generation and TLC verification** — `meerkat-machine-codegen` generates TLA+ models from the catalog DSL and runs TLC for closed-world composition verification.

#### Identity-first live voice
- **Identity-first live voice groundwork** (#250) — realtime attachment is keyed on stable `AgentIdentity` (survives respawn) and `AgentRuntimeId` / `FenceToken` for per-binding rotation safety. SDKs now resolve identity first, then open the realtime channel.
- **Capability-driven realtime transport** — `ModelCapabilities.realtime` on the session's resolved model decides attach/detach. There is no caller-initiated attach/detach RPC.
- **`realtime_attachment_status` projection** — typed projection on session and per-member surfaces (`session/realtime_attachment_status`, `mob/member_status.realtime_attachment_status`).
- **Live-topology reconfigure flow** — `reconfigure_live_topology` orchestration in `meerkat-runtime/src/meerkat_machine/llm_reconfigure.rs` covers in-place model/provider swaps without tearing down the session.
- **Python and TypeScript SDK ports** — both SDKs ported to identity-first resolve-then-open for `RealtimeChannel.mob_member`.

#### Realtime audio and protocol
- **Realtime audio Python example** (#539) — end-to-end OpenAI Realtime API example with streaming audio input/output, proper shutdown, and error handling.
- **Typed realtime protocol version** (#584) — `protocol_version` is now wire-typed; runtime owns version validation.
- **Realtime tool timeouts via runtime dispatch** (#610) — tool timeout enforcement routed through runtime dispatch rather than inline, so timeouts are observable per session.
- **Realtime transcript canonicalization** (#611) — transcript appends canonicalized at ingress to prevent duplicates and ordering skew across reconnects.
- **Machine-owned realtime bootstrap eligibility** (#587) — realtime attachment eligibility validated at the machine boundary; invalid reconnect states rejected.

#### Durable event storage
- **FileEventStore sequence authority** (#591) — file-backed event streams carry typed sequence authority; importers gate on sequence numbers to prevent gaps and out-of-order replay.

#### Typed machine contracts
- **Typed terminal cause spine** (#564) — structured `TerminalCause` enum replaces stringly-typed terminal reasons; invalid terminal transitions rejected at the DSL boundary.
- **Typed comms intent/result contract** (#572) — `CommsIntent` and `CommsResult` typed variants replace bare strings; routing authority validated at the machine level.
- **Typed background job completion status** (#579) — `background_job_completed` events now require typed `terminal_status`; the legacy `status` string is retained only as an optional display mirror.
- **Typed `HookId`** — hook event errors carry typed `HookId` instead of string names.
- **Typed provider runtime backend/auth matrix** (#571) — provider overrides typed with validated policy lookups.
- **Typed mob spawn-many member outcomes** (#586) — `SpawnMemberOutcome` variants typed for success/failed/skipped paths; spawn-many batches envelope results.
- **Typed mob lifecycle action dispatch** (#577) — mob lifecycle actions carry typed dispatch envelopes through machine transitions.
- **Typed `EventEnvelope` source identity** (#585) — source identity carried as typed context; source-string drift scanner replaces text-based fallbacks.
- **Typed peer directory wire facts** — peer directory facts (LUC-154) and peer endpoint parity scanner (#559) replaced with typed AST-based authority.
- **Typed runtime alphabet manifests** — generated named string domains constrained at codegen (LUC-290, LUC-292).

#### AuthMachine and OAuth freshness
- **AuthMachine OAuth freshness gate** (#612) — OAuth freshness enforced at turn admission; stale tokens rejected before execution.
- **AuthMachine cloud authorizer freshness** (#575) — cloud token leases tracked under the auth seam; freshness enforced via lease authority.
- **OAuth flow lifecycle under scoped auth authority** (#521) — OAuth flow transitions move under the scoped auth machine.
- **Managed OAuth freshness under lease** (#552) — managed OAuth admission recut under explicit lease semantics; stale poll persistence and admission resurrection prevented.
- **Auth status derived from typed phase** (#407) — auth status surfaces are lease-owned and projected from typed phases (LUC-58, LUC-193).
- **OAuth terminal state machine-owned** (#598) — OAuth terminal transitions (success/error/cancelled) owned by AuthMachine; no external override path.

#### Machine-owned policy and lifecycle
- **Machine-owned budget exhaustion** (#599) — budget-exceeded transitions owned by the machine; stale budget state in agents prevented.
- **Machine-owned hook failure policy** (#597) — hook denial/failure handling policy-enforced at the machine; terminalization atomic.
- **Mob admission via MobMachine guards** — mob membership admission centralized in MobMachine guards (LUC-189, LUC-200, LUC-205, LUC-214).
- **Surface request lifecycle classification** (#432) — request lifecycle (start/resume/external-event) routed through canonical lifecycle authority (LUC-190).
- **Comms ingress classification at machine** (#427) — comms ingress classification owned by the machine; stale/duplicate ingress events prevented.

#### Profile and tool scoping
- **`profile.tools.{mob,mcp}` actually-scoping** (#600) — tool scoping applies the canonical resolver and provenance filter; tools inherit from parent scope correctly.
- **Catalog-owned image generation defaults** (#583) — image generation defaults sourced from the model catalog rather than provider-specific overrides.
- **Provider-aware model capability boundary** (#562) — capability detection (vision, web search, reasoning) gated on the provider boundary.

#### Runtime composition and recovery
- **Boundary-atomic runtime terminalization** (#608) — surface request terminals routed through canonical lifecycle authority; cleanup atomic.
- **Scoped session recovery config** (#601) — session recovery configuration is realm-scoped and supports pluggable recovery strategies per session family.
- **Canonical runtime identity** (#526) — runtime identity split from session aliases; canonical alias recovery and snapshot authority preserved across restarts (LUC-209).
- **Mob-aware schedule delivery** (#446) — schedule delivery unified across mob members under a single typed dispatch path (LUC-93).

#### Web/SDK improvements
- **Generated Web auth wire contracts** (#580) — Web SDK auth contracts generated from `meerkat-contracts`; bearer-string fallback removed.
- **Profile overrides for TS/Web auth helpers** — auth helpers in TS and Web SDKs accept profile overrides (LUC-48).
- **Typed MCP add-config contract** (#596) — MCP add-config operations validated through typed contracts.
- **Configured MCP tools exposed to RPC mob members** (#566) — mob members reached over RPC see the same configured MCP toolset as direct callers.
- **RPC mob MCP transports kept alive** (#609) — mob MCP transports survive idle periods on RPC.

#### Help and discoverability
- **Dedicated Meerkat help surfaces** (#629) — first-class `rkat help` and platform help skill provide accurate, grounded answers about CLI commands, flags, and surfaces (LUC-443).
- **Embedded `rkat` CLI help skill** — CLI help skill embedded in the binary for offline use; help prompt grounding hardened against fabrication.
- **Refreshed platform help skill CLI facts** (#630) — platform help skill updated with current CLI command names, flags, aliases, and negative facts.
- **`meerkat-cli-reference` skill** — exact CLI command contract published as the authority for help answers.

#### Completion events
- **Structured output in completion events** (#627) — completion events now carry typed structured-output payloads (text + tool result blocks) for downstream consumption; replaces opaque string concatenation.

#### Robustness and validation
- **Fail-closed supervisor rotation** (#625) — mob supervisor rotation rejects partial / inconsistent rotations rather than advancing local authority on a divergent peer (LUC-438).
- **Fail-closed partial mob destroy** (#626) — partial mob destroy rejected; either the entire mob tears down cleanly or the operation errors and rolls back.
- **Quarantined hook semantic rewrites** (#624) — hooks that attempt semantic rewrites on event contents are now quarantined and rejected at the boundary.
- **Amputated skill builtin raw ingress paths** (#623) — built-in skills no longer admit raw ingress; all skill content flows through the typed skill resolver.
- **Hardened mobpack validation** (#621) — mobpack archive validation rejects malformed/inconsistent archives earlier and surfaces structured errors.
- **Release-grade auth smoke lane** (#616) — new dedicated smoke lane exercises live auth flows end-to-end before tag-cut.

#### Dogma gate and machine-schema audit
- **Dogma cleanup review gate** (#558) — mandatory review gate for dogma cleanup changes.
- **Self-validating immutable dogma gate** (#589) — dogma gate self-validates and audits its own freshness against generated artifacts.
- **AST-based machine drift detection** — peer terminal string ratchet and other text-based drift detectors replaced by AST checks.

#### CI / infrastructure
- **BuildBuddy stabilization** — workspace CI profiling, stale run cancellation, and self-hosted runner tuning for deterministic builds.
- **Per-branch stale CI cancellation** — superseded CI runs on the same branch are cancelled to reduce slot burn.
- **0.6 release gate stabilization** (#606) — release-preflight lanes stabilized for the 0.6 cut.

#### Documentation
- **README architecture diagram** — README now ships a top-level architecture diagram and surface map, placed in the architecture section.
- **README and example refresh** (#613, #614) — README rewritten for current Meerkat surfaces; examples updated and validated.
- **Architecture and API references refresh** (#622) — architecture and API reference docs updated for 0.6 surfaces, contracts, and machine boundaries.
- **Skills refresh for 0.6** (#615) — `meerkat-platform`, `meerkat-architecture`, `meerkat-wasm` skills updated for 0.6 wire contracts and surfaces.

### Changed

- `background_job_completed` events require typed `terminal_status` for completion semantics; the legacy `status` string is retained only as an optional display mirror.
- `TerminalCause`, `CommsIntent`, `CommsResult`, and `HookId` are now typed enums on the wire — code matching on bare strings will fail to deserialize.
- `MobMemberListEntry` and `SpawnResult` tightened to use typed `MemberSessionBinding` atoms.
- Provider policy overrides validated against catalog owner authority; mismatches rejected at admission.
- Session capacity is now active-work bounded; agents that previously accumulated unbounded queued turns will be admission-limited (LUC-294, LUC-298).
- OAuth freshness enforced at turn admission rather than at provider-call time; previously-stale tokens that would have been rejected mid-turn now fail before execution.
- Stale session projections from persistence rejected if they disagree with live runtime state; persistent stores with diverged state will fail recovery instead of silently masquerading (#560, #573).
- Tool-call argument projection now fail-closed (#581) — serialization failures block tool execution rather than silently degrading.
- Tool-call result content now preserves multimodal blocks through hook events and persisted history.
- Multimodal history preserved through compaction via blob-backed placeholders rather than text degradation.
- Provider identity sourced from the agent builder's durable identity, not from runtime overrides.
- WASM subscription mutations that fail to serialize now rejected at the boundary (#569) — prevents client/server subscription desync.
- REST terminal operations require runtime-stamped terminal evidence (#593).
- Inproc comms sends routed by canonical peer identity (LUC-287).
- Zero-pubkey peer trust paths rejected at admission (#545, LUC-286).
- Web mob decoders fail closed on schema mismatch (#567, LUC-339).
- `connection_ref` renamed to `auth_binding` across all wire contracts, REST/RPC payloads, and SDK surfaces (#618, LUC-404). Semantics unchanged.
- `rkat` default tracing quieted (#620) — default log output trimmed to user-relevant signal; verbose runtime tracing now opt-in.
- Default session capacity raised (hotfix 67afe9b65) — single-process default cap increased to better fit current mob workloads.

### Removed

- **Hand-written machine catalog** — handwritten machine bodies have been deleted; the catalog DSL is the sole source of truth for production machine semantics. Production modules that previously authored competing semantics are now bridge shells around catalog-owned DSL bodies.
- **`mob/realtime_attach` and `mob/realtime_detach` REST endpoints** — caller-initiated realtime attach/detach removed; transport is capability-driven via `ModelCapabilities.realtime`. Use the `realtime_attachment_status` projection instead.
- **Retired runtime/session verbs** — Python SDK and RPC docs scrubbed of dead verbs (`status`, `submit`, `retire`, `reset`, `submission`, `submissions`, `realtime_attachment_statuses`); typed `auth_binding` and typed realm context replace the legacy locator shapes.
- **WASM bearer-string auth fallback** (#516) — legacy string-based WASM auth path removed; all WASM surfaces route through the typed auth seam (`auth_binding` / `AuthProfile`).
- **`connection_ref` (renamed to `auth_binding`)** (#618) — the `connection_ref` field name is removed from all wire contracts, REST/RPC payloads, and SDKs; use `auth_binding` instead. The semantics are unchanged (LUC-404).
- **Peer ingress compatibility authority** (#568) — legacy peer ingress routes removed; the typed peer ingress machine is the sole authority.
- **Runtime visibility fallback** (#561) — fallback visibility restoration removed; visibility must be machine-owned or denied.
- **Recovered runtime force-authority fallback** (fd8ac40ec) — the force-authority fallback used during recovery has been amputated.
- **Runtime session compat nouns** (#576) — legacy compat nouns retired from the runtime session API surface (LUC-345).
- **Runtime session control compat routes** (#563) — legacy session-control compat routes retired.
- **Store-only session promotion** (#578) — automatic promotion of store-only sessions to live runtime removed; explicit recovery required (LUC-350).
- **Source-string machine drift scanner** (#592) — text-based drift scanner demoted in favor of AST-based detection.

### Fixed

- **Realtime reconnect retry truth machine-owned** — reconnect retry state fully machine-owned; stale retry attempts across runtime cycles prevented.
- **Flow projection persistence atomic** — flow projection state persistence atomic; half-written flow state prevented.
- **MCP identity for mob wiring** — mob-to-peer wiring uses typed identities instead of placeholder session keys.
- **Durable session fallback truth** (#401) — durable session recovery gates on real persistence state, not soft defaults.
- **Hook denial terminalization** — pre-tool and post-tool hook denials now properly terminate the turn; stale pending turns cleaned up.
- **Deferred tool load authority** (#542) — deferred tool admission owned by the machine; previously-skipped tools no longer silently disappear (LUC-288).
- **Effect authority audit self-test** (LUC-305) — multiline interrupt audit coverage restored; effect authority audit shrunk and self-validated.
- **Memory prompt extraction state** — memory prompt extraction state corrected (LUC-91).
- **Mob flow supervisor authority** — mob flow supervisor authority corrected (LUC-90).
- **Hook execution allocations** — hook execution path optimized to remove redundant allocations.
- **AGX Orin / aarch64 build hygiene** — feature-matrix lanes and surface modularity gates stabilized for cross-target builds.
- **OAuth browser auth release UX** (#619) — OAuth browser-flow release path now surfaces clear status to the user; release acknowledgement no longer hangs.
- **OAuth login config TOML serialization** (9e78ce719) — OAuth login config round-trips correctly through TOML; previous serialization could drop nested fields.
- **OAuth provider canaries** (52e50acf8) — provider canary jobs now exercise the full OAuth admission path; previously they short-circuited and missed regressions.
- **e2e-smoke realtime + WASM setup** (#617) — fixed setup races in the e2e smoke lane that surfaced as flaky realtime / WASM failures.
- **e2e-smoke realtime mob root cause** (dbdd3114f) — root-caused intermittent realtime mob failures in the smoke lane and stabilized the lane.
- **BuildBuddy bazel metadata refresh** (c02e6695f) — Bazel metadata refresh now correctly invalidates stale lanes after lockfile churn.
- **Decoupled structured output extraction terminalization** (#634) — structured-output extraction no longer terminalizes the turn on its own; ordering now matches the rest of the completion path.
- **Image tool visible without image auth** (#633) — image tool is now listed in the agent's tool catalog even when image-generation auth is absent; previously it disappeared silently from prompts.
- **Runtime boundary rollback and REST capacity tests** (7ed7cd712) — fixed flaky runtime boundary rollback and REST capacity tests.
- **Bazel generator awareness of CLI help skill** (fb2421e17) — Bazel BUILD-file generator now includes the embedded CLI help skill.

## [0.5.2] - 2026-04-12

### Added

#### Self-hosted model registry
- Two-tier model registry: server definitions (`[self_hosted.servers.<id>]`) and model aliases (`[self_hosted.models.<alias>]`).
- OpenAI-compatible transport with `chat_completions` and `responses` API styles — works with Ollama, vLLM, LM Studio, and any `/v1/chat/completions` endpoint.
- Self-hosted aliases merge into the runtime model catalog as first-class models. Provider inferred by exact alias match before prefix inference.
- `rkat doctor` validates server reachability, bearer token resolution, and remote model availability.
- `rkat models catalog` shows self-hosted models under the `self_hosted` provider group with backing server metadata.
- Per-model capability flags: `vision`, `supports_thinking`, `supports_reasoning`, `supports_web_search`, `context_window`, `max_output_tokens`, `call_timeout_secs`.
- Bearer token resolution via environment variable reference (`bearer_token_env`) — no literal tokens in config.
- Self-hosted models work identically across all surfaces (CLI, REST, RPC, MCP, SDKs).

#### RuntimeBinding — first step toward identity-first mobs
- New `RuntimeBinding` enum (`Session` | `External { peer_id, address }`) separates backend kind (definition/profile level) from runtime binding (spawn/provision level).
- External mob members now carry real process comms identity instead of phantom placeholder session keys.
- `SpawnMemberSpec.binding` field on all spawn surfaces; `ProvisionMemberRequest.binding` replaces bare `MobBackendKind` tag at provisioner level.
- `WireRuntimeBinding` wire type in `meerkat-contracts` for public MCP and RPC spawn inputs.
- Respawn preserves binding from old roster entry, maintaining real identity across incarnations.
- Bare `MobBackendKind::External` without `RuntimeBinding` is rejected — external members must declare their process identity at spawn time.
- Conflict handling: `backend` + `binding` on the same request is rejected if they disagree.

#### mob_wire / mob_unwire agent tools
- `mob_wire` and `mob_unwire` tools added to `AgentMobToolSurface` — agents can now create and remove comms trust relationships between mob members.
- Supports both local peers (within the same mob roster) and external peers (outside the roster with explicit identity).
- Reuses the existing `MobMcpState::mob_wire()` / `mob_unwire()` state API.

#### Hive agent (example 035)
- Full `SessionRuntime` + RPC server in the kennel binary for the hive agent.
- Hive mob created on startup with external-backend target profiles; targets spawned as members on registration.
- `PeerWire` / `PeerUnwire` kennel payloads for target-to-target comms mesh wiring.
- Kennel sends `PeerWire` at registration time — all-to-all mesh so targets can communicate directly.
- Target handles `PeerWire` by adding the other target as a trusted comms peer.
- `TargetRegistered` includes hive pubkey + comms address for bidirectional trust.
- Hive system prompt updated with `mob_wire`, `mob_unwire`, `mob_list_members` guidance.

#### Deferred tool catalog
- Adaptive deferred catalog discovery: tools from MCP servers and other async sources are discovered in the background and become available as each source completes.
- Typed schemas for catalog control tools.
- Deferred catalog composition and exactness hardened.

#### TCP RPC server
- `serve_tcp` and `serve_tcp_connection` in `meerkat-rpc` for JSON-RPC over TCP.
- `--tcp` flag on `rkat-rpc` binary for network listener mode.
- TCP e2e tests that spawn the real `rkat-rpc` binary.
- Concurrent connection handling (spawns each connection independently).

#### Default-on provider web search
- Web search enabled by default for all verified catalog models: Anthropic (`web_search_20250305`), OpenAI (`web_search`), Gemini (`google_search`).
- New `[provider_tools]` config section with per-provider `web_search`/`google_search` toggle (presence-aware TOML merge).
- `ModelProfile.supports_web_search` capability flag gates injection per model family.
- `SelfHostedModelConfig.supports_web_search` (default `false`) for self-hosted model opt-in.
- Non-persisted `AgentConfig.provider_tool_defaults` re-derived on every build (including resume) from current config + profile.
- Per-turn merge via RFC 7396 merge-patch in `state.rs`; extraction turns strip tool keys.
- Opt-out: `provider_tools.anthropic.web_search = false`, `provider_tools.openai.web_search = false`, or `provider_tools.gemini.google_search = false` in config; `rkat run --no-web-search`; or the provider-native null key in `provider_params` per request.

#### Realm-scoped mob profiles
- `RealmProfileStore` with `InMemoryRealmProfileStore` and `SqliteRealmProfileStore` for realm-local reusable profile definitions.
- Profile CRUD tools: `mob_profile_create`, `mob_profile_get`, `mob_profile_list`, `mob_profile_update`, `mob_profile_delete`, `mob_profile_list_sources`.
- Public MCP tools: `meerkat_mob_profile_create|get|list|update|delete|list_sources`.
- RPC methods: `mob/profile/create|get|list|update|delete` and `mob/profile/sources/list`.
- `SpawnTooling` enum (`InheritParent`, `Minimal`, `Profile`) for agent-driven child spawn tooling modes.
- `ProfileBinding::RealmRef` for mob definitions that reference realm-stored profiles.
- `ToolProvenance` metadata on `ToolDef` with `ToolSourceKind` propagated across builtins, shell, comms, schedule, mob, callback, and MCP tool sources.
- `ToolScope` snapshot seam for parent-selected child tooling at spawn time.
- `effective_profile_override` persisted in mob roster for lifecycle-safe respawn/restore.
- `tooling` parameter exposed in `delegate` and `mob_spawn_member` tool schemas.

#### Scheduler as first-class surface capability
- `ScheduleToolDispatcher` implements `AgentToolDispatcher`, wired natively into CLI, REST, RPC, and MCP surfaces.
- Schedule tools added to MDM target and hive configurations.

#### Test lane reorganization
- Unified e2e test lanes under cargo aliases: `e2e-fast` (deterministic), `e2e-system` (real local-resource), `e2e-live` (targeted live-provider), and `e2e-smoke` (kitchen-sink live smoke).
- Legacy aliases `int-real` (→ `e2e-system`) and `e2e` (→ `e2e-live` + `e2e-smoke`) retained for compatibility.

#### Homebrew tap and macOS codesigning
- `lukacf/homebrew-meerkat` Homebrew tap: `brew install lukacf/meerkat/rkat` installs all 4 binaries.
- Release workflow auto-updates the tap formula on tag push with correct checksums.
- macOS release binaries are now ad-hoc codesigned before packaging.

### Changed

#### Comms tool split (breaking)
- Agent-facing `send` tool split into three purpose-specific tools: `send_message`, `send_request`, and `send_response`.
- `handling_mode` (steer/queue) is now a required field on `send_message` and `send_request`.

#### Peer ingress machine-owned
- Peer ingress handling is now machine-owned via `PeerIngressMachine` with full handling-mode rollout.
- `wait` removed from peer ingress; peer reservations removed.

#### Communication-first delegate
- Delegate flow redesigned to be communication-first; peer reservations removed from the delegate path.

#### Mob lifecycle seams promoted to canonical machines
- `MobMemberBootstrapMachine`, session turn admission, and peer ingress owners promoted from ad-hoc seams to canonical machine authority.

#### Surface recipes replace RuntimeSessionHost
- `RuntimeSessionHost` extracted and then replaced with free recipe functions at proper crate boundaries: `wire_runtime_bindings`, `materialize_session`, `configure_peer_ingress`, `default_persistent_executor`.

#### AgentBuilder facade now uses AgentFactory
- Public `meerkat::AgentBuilder` now routes builds through `AgentFactory::build_agent()` and returns `Result<DynAgent, BuildAgentError>`.
- Standalone-only direct injections (`provider_tool_defaults`, `compactor`, `memory_store`, `with_turn_state_handle`) now fail loudly on the facade builder instead of being ignored; configure the factory path for facade-owned settings.

### Removed
- **Redb persistence**: All `RedbSessionIndex`, `RedbSessionStore`, and redb-backed storage removed. All persistence is now SQLite (WAL mode) via `SqliteSessionIndex`.
- Unused MCP runtime ingress helper removed.
- Peer reservations removed from comms and delegate paths.

### Fixed
- TUX scroll overflow: text no longer renders below the input box.
- TUX auto-scroll resumes when user scrolls to bottom instead of staying paused permanently.
- TUX session resume now loads and displays conversation history in the timeline.
- TUX idle CPU usage reduced from ~25% to <1% via dirty-flag rendering and coarser timers.
- Scenario 56 test passes API key to RPC/REST subprocesses and skips when unavailable.
- Unused `meerkat-schedule` dependency removed from example 035.
- Post-merge test regressions: send tool rename assertions and redb rejection paths updated.
- Mob peer auto-wiring semantics corrected.
- Mob authority via typed tool effects (no re-entrant deadlock).
- `InterruptYielding` wired to cooperative wait interrupts and emits `WakeRuntime` for queued inputs.
- Comms drain notification race fixed.
- Auto-derive `comms_name` for CLI keep-alive sessions.
- Mob member list projection made non-blocking; `list_members` stall during concurrent spawn fixed.
- Mob state restored across REST with callback peer smoke coverage.
- Terminal peer responses correctly produce single-event runtime work.
- Centralized `extract_prompt` and context-only dispatch on `RunPrimitive`.
- Typed `PostAdmissionSignal` with batch-safe `execution_kind`.
- Canonical `StartTurnDisposition` for turn admissibility.
- `ResumePending` boundary check counts staged tool results; mixed-batch assert removed.
- `execution_kind` forwarded in `MobRpcRuntimeExecutor`.
- Shared `JsonlStore` instance in example 035 target.
- Scheduler schema and CLI first-turn tools fixed.
- WASM `async_trait` on schedule dispatcher fixed.
- TUX scheduler delivery and tool guidance fixed.
- CI prereqs for wasm and shared-realm e2e lanes fixed.
- Inert session bug in MCP/mob/RPC ingress: `contains_session` replaced with `session_has_executor` to prevent turn/start hangs on sessions without a runtime loop.
- Executor notification sink staleness: sink is now read at apply time from `RwLock`, not captured at construction.
- `MethodRouter::new()` preserves existing mob state instead of overwriting per TCP connection.
- `serve_tcp` spawns connections concurrently instead of sequentially.
- `start_turn_via_runtime` no longer downgrades externally-configured comms drain.
- `trusted_peer_spec` for external members uses bridge comms key (transport) instead of `BackendPeer.peer_id` (identity) — fixes identity/transport conflation.
- External member phantom identity: `BackendPeer.peer_id` is now the real external process key, not the placeholder session's comms key.
- Self-hosted model switching: `config_runtime` set and provider inferred correctly.
- Hot-swap filter persistence and audit lane hardened.
- Projector replay writes hardened.
- Web and shared-realm test flows stabilized.

## [0.5.1] - 2026-04-06

Meerkat 0.5.1 is a feature release adding the scheduler subsystem, flow-frame loops, background job completion notifications, and the runtime epoch model — plus broad correctness fixes across mob orchestration, session recovery, and tool visibility.

### Highlights

- **Scheduler subsystem** (`meerkat-schedule`): cron and interval triggers, occurrence lifecycle, misfire/overlap/missing-target policies, schedule tools, and surface rollout across CLI, REST, RPC, and MCP.
- **Flow-frame loops**: `repeat_until` loop construct for mob flows, with frame-based execution, loop iteration authority, and durable resume/recovery.
- **Background job completion**: `CompletionFeed` delivers canonical completion entries to the agent boundary, enabling `[BG_JOB]` notices and idle wake on shell job completion.
- **Runtime epoch model**: `SessionRuntimeBindings` + `RuntimeBuildMode` eliminate the split-owner ops lifecycle bug class. All runtime-backed surfaces use `prepare_bindings()`.
- **Mob delegation tools**: Agent-facing tools for delegate, mob_create, mob_spawn_member, mob_send, and mob lifecycle management.

### Added

#### Scheduler subsystem
- New `meerkat-schedule` crate with `ScheduleLifecycleAuthority`, `OccurrenceLifecycleAuthority`, `ScheduleDriver`, `ScheduleService`, and `ScheduleStore`.
- Cron and interval trigger specs with `next_due_after()` and `occurrences_for_horizon()`.
- Misfire, overlap, and missing-target policies with configurable behavior.
- Schedule tools (`schedule_create`, `schedule_update`, `schedule_list`, `schedule_read`, `schedule_delete`, `schedule_pause`, `schedule_resume`) exposed across all surfaces.
- `ScheduleTargetDelivery` and `ScheduleTargetProbe` traits for pluggable delivery backends.
- Schedule host surface integration with `RuntimeSessionAdapter` for runtime-backed delivery.
- TLA+ formal specs for schedule and occurrence lifecycle state machines.
- Atomic planning mutations (`atomic_plan_mutation()`) on `ScheduleStore` for safe multi-step schedule changes.

#### Flow-frame loops
- `repeat_until` loop construct in mob flow specs via `FlowFrameMachine` and `LoopIterationMachine`.
- Frame-based execution model replacing flat-step dispatch for loop bodies.
- Durable loop state: iteration count, evaluation results, and resume context survive restart.
- Loop body/evaluate seam ownership via `LoopIterationAuthority`.

#### Background job completion and runtime epoch model
- `CompletionFeed` trait and `RuntimeOpsLifecycleRegistry` integration for canonical completion delivery.
- Agent boundary `[BG_JOB]` notices when background shell jobs complete.
- Idle wake fires when all background ops complete and agent is idle.
- `RuntimeEpochId`, `SessionRuntimeBindings`, `RuntimeBuildMode` types in `meerkat-core`.
- `prepare_bindings()` on `RuntimeSessionAdapter` — single canonical helper replacing hand-rolled register/extract/pass pattern.
- Factory validates `SessionOwned` bindings session_id matches build session.
- `StandaloneEphemeral` is the explicit default for test/standalone/WASM surfaces.
- `EpochCursorState` with shared atomics for persistence-ready cursor tracking.
- `PersistedOpsSnapshot` type for durable ops lifecycle recovery (bounded-loss, no invisible completions).
- `recover_or_create_ops_state()` shared recovery helper on `RuntimeSessionAdapter`.
- Persistence channel on terminal transitions (capture-and-queue pattern).
- `persist_ops_lifecycle` / `load_ops_lifecycle` on `RuntimeStore` trait with SQLite, Redb, and in-memory implementations.

#### Mob delegation and orchestration
- Agent-facing delegation tools: `delegate`, `mob_create`, `mob_spawn_member`, `mob_send`, `mob_list_members`, `mob_read_member`, `mob_finalize` via `AgentMobToolSurface`.
- `MobMcpState` and `MobMcpDispatcher` for MCP-hosted mob tool exposure.
- Built-in mob tools wired into example 035 MDM TUX target.

#### Storage and infrastructure
- `SqliteTaskStore` implementing unified storage trait contracts.
- SQLite replaces Redb for mob storage (eliminates single-writer lock contention).
- Session identity claim leak fix in mob storage migration.
- Unified `StorageTrait` contracts across task, mob, and session stores.

#### Comms and peers
- Typed `handling_mode` override on `PeerInput` for actionable peer conventions (Message, Request). ResponseProgress and ResponseTerminal are validated to reject the field at runtime admission.
- `handling_mode` field on MCP `meerkat_comms_send` tool for parity with RPC/REST.
- Shared `CommsRuntime` replaces per-surface homebrew comms wiring in example 035.

#### Examples
- Example 035: MDM TUX — ratatui device manager using P2P comms with target and TUX binaries.

#### Multimodal content
- Inline video content blocks (`ContentBlock::Video`) with `VideoData::Inline` for base64-encoded video. Gemini-only native support (`inlineData`); Anthropic and OpenAI degrade replayed video to `[video: media_type]` text placeholders.
- `duration_ms` field on video blocks for caller-provided clip duration, used in token estimation for compaction.
- `inline_video: bool` on `ModelProfile` for capability detection (Gemini=true, Anthropic/OpenAI=false).
- Video ingress validation at RPC, REST, and ephemeral session boundaries — rejects non-Gemini video with typed error.
- Video in tool results rejected at all three providers and the agent state machine.
- Compaction strips video blocks to text placeholders alongside images. Token estimation uses `max(data_size/4, duration_ms*300/1000)`.

#### Typed notices and build-seam cleanup
- `Message::SystemNotice(SystemNoticeMessage)` variant with typed `SystemNoticeKind` enum (`McpPending`, `BackgroundJob`, `ToolScope`, `ToolScopeWarning`, `Generic`). Replaces stringly-typed `Message::User` with `[SYSTEM NOTICE]` prefixes.
- Backward-compatible deserialization: old `Message::User` with `[SYSTEM NOTICE]` prefixes auto-promote to `Message::SystemNotice` on load.
- `render_metadata: Option<RenderMetadata>` on `UserMessage` for structured classification.
- `WireSessionMessage::SystemNotice` variant in wire contracts.
- All three LLM providers render `SystemNotice` as user-role text with `rendered_text()`.

#### Other
- `ToolCategoryOverride` enum (`Inherit | Enable | Disable`) for typed tool category control in `SessionTooling`.
- Typed `RejectReason` enum on `AcceptOutcome::Rejected` replacing bare `String` (NotReady, DurabilityViolation, PeerHandlingModeInvalid).
- Callback-pending completion outcome for runtime-backed surfaces.
- `codemob-mcp` session continuation, skills, and UX improvements.

### Changed
- `SessionBuildOptions.runtime_build_mode` is now a required field (default: `StandaloneEphemeral`). Replaces `ops_lifecycle_override`.
- `PreparedSurfaceSession` carries `SessionRuntimeBindings` instead of bare `ops_lifecycle`.
- All runtime-backed surfaces (CLI, RPC, REST, MCP, example 035) migrated to `prepare_bindings()` + `RuntimeBuildMode::SessionOwned(bindings)`.
- Mob provisioner pre-registers sessions via `prepare_bindings()` before `create_session()` with orphan reconciliation.
- `PersistentSessionService` uses `set_runtime_bindings_provider()` instead of `set_ops_lifecycle_provider()`.
- Prefab enum and all prefab-based mob creation deleted.
- Redundant `MobActorCoreExecutor` deleted; `ensure_autonomous_runtime_ready` slimmed.
- Mob operator tool authority boundaries tightened.
- Tool override fields on `SessionBuildOptions` and `AgentBuildConfig` migrated from `Option<bool>` to `ToolCategoryOverride`. Tool-specific bits removed from `ResumeOverrideMask`.

### Fixed
- Background shell job completions now correctly wake the agent in all runtime-backed surfaces (previously silent due to split-owner registry bug).
- Tool category suppression on session resume: new tool categories (e.g. mob tools added after session creation) now inherit correctly instead of being frozen at creation time.
- RPC stdout `WouldBlock` crash on high-throughput streaming.
- Callback tools wired into mob agents (previously missing).
- 035 target freeze: replaced homebrew comms with `CommsRuntime` for correct lifecycle management.
- Shared `CommsRuntime` no longer overrides per-session comms identity.
- Session identity claim leak in mob storage.
- Version corrected from 0.6.0 to 0.5.1.

## [0.5.0] - 2026-03-26

Meerkat 0.5 is a large architecture and surface cutover. It formalizes runtime ownership around generated authorities and runtime-backed session services, removes a wide set of legacy public-surface residue, brings persistent session and mob recovery much closer to truthful replay, and adds a realm blob store for image content.

### Highlights

- Runtime-backed semantics are now the canonical public model across CLI, REST, RPC, MCP, Rust, Python, TypeScript, and Web surfaces.
- `host_mode` has been fully replaced by `keep_alive`, with stricter validation, clearer tri-state behavior, and consistent cross-surface ownership.
- Generated machine authorities, formal schemas, and seam-audit enforcement now back the most important runtime, mob, comms, turn, and ops-lifecycle semantics.
- Durable image handling moved to the new blob-backed model, with aligned history/read semantics and better multimodal behavior across providers and comms paths.
- Persistent session and mob resume behavior is much closer to truthful replay, including stronger identity continuity, broken-member handling, and runtime-backed recovery.
- Surface cancellation, commit-boundary, and external-event behavior were tightened so successful committed work is preserved and invalid/runtime-owned requests fail more honestly.

### Upgrade notes

- Treat 0.5 as a real semantic cutover from 0.4.x, not a small additive release.
- Runtime-backed session services are now the intended integration path; direct low-level construction is an expert/internal escape hatch.
- Expect `keep_alive`, blob-backed image durability, richer structured content/history models, and stronger typed lifecycle semantics across surfaces and SDKs.
- If you are upgrading an older integration, use the 0.4x -> 0.5 migration guidance in the docs/skills rather than assuming 0.4 behavior still holds.

### Added

#### Formal runtime authorities, machine schema, and seam enforcement
- New machine-authority toolchain:
  - `meerkat-machine-schema`
  - `meerkat-machine-codegen`
  - `meerkat-machine-kernels`
- New generated authority / protocol artifacts across runtime, mob, comms, external tools, and ops lifecycles.
- New formal specs and compositions under `specs/machines` and `specs/compositions`, plus `xtask` support for machine/codegen/audit workflows.
- New architecture doctrine and audit material:
  - `docs/architecture/meerkat-runtime-dogma.md`
  - `docs/architecture/formal-seam-closure.md`
  - `docs/architecture/RMAT.md`
  - `docs/architecture/finite-ownership-ledger.md`
- New CI / pre-push enforcement around schema freshness, generated artifacts, clippy cleanliness, and seam-audit drift.

#### Runtime-backed request execution and cancellation
- Shared cancellable surface request execution helpers in `meerkat::surface`.
- Runtime-backed request lifecycle and cancellation support added across:
  - JSON-RPC
  - REST
  - MCP stdio hosting
  - `examples/034-codemob-mcp`
- JSON-RPC now supports explicit request cancellation notifications.
- MCP stdio hosting now supports long-running cancellable tool execution without serially blocking the read loop.
- Successful state-advancing operations now publish committed success correctly instead of being rewritten to cancellation by late races.

#### Realm blob storage for image content
- New `BlobId`, `BlobRef`, `BlobPayload`, and `BlobStore` contracts in core.
- New built-in blob store implementations:
  - `MemoryBlobStore`
  - `FsBlobStore`
- `PersistenceBundle` now owns a matched set of:
  - `SessionStore`
  - `RuntimeStore`
  - `BlobStore`
  - `RuntimeSessionAdapter`
- New blob fetch surfaces:
  - REST `GET /blobs/{blob_id}`
  - RPC `blob/get`
  - MCP `meerkat_blob_get`
  - CLI `rkat blob get`
  - SDK blob-get helpers

#### New and expanded contracts on public surfaces
- Public session history is now fully runtime-backed and aligned across REST/RPC/MCP/SDK surfaces.
- REST external-event ingress is now canonical at `POST /sessions/{id}/external-events`.
- JSON-RPC external-event ingress is now canonical at `session/external_event`.
- REST/RPC/MCP contracts were regenerated and expanded:
  - richer RPC and REST catalogs
  - refreshed wire types and schema artifacts
  - generated web event types from contracts
- New `ErrorCode::RequestCancelled` / request-cancelled semantics in contracts.

#### Mob runtime and orchestration improvements
- New runtime-owned Broken-member projection for partial persistent resume.
- Persistent mob resume now restores missing member sessions from durable state with:
  - same `session_id`
  - preserved transcript/history
  - preserved durable LLM identity
  - preserved native inproc comms identity / `peer_id`
- New stronger mob lifecycle/orchestrator/runtime authorities and kernels.
- New real-API mob smoke coverage, including collaborative-resume and multimodal pictionary scenarios.

### Changed

#### Runtime architecture and ownership
- Runtime, comms, mob, and surface semantics now route through explicit authorities instead of shell-side lifecycle decisions.
- Input lifecycle, runtime ingress, runtime control, comms drain lifecycle, peer comms, peer reachability, ops lifecycle, turn execution, mob lifecycle, mob orchestrator, and flow-run semantics were all formalized and tightened.
- Surface code is now more explicitly “skin/mechanics only,” with semantic truth moved into authorities, protocols, and typed control seams.

#### Session service and public surface defaults
- Runtime-backed `SessionService` embedding is now the documented and tested default across Rust docs, examples, CLI, REST, RPC, MCP, and SDK guides.
- Direct `AgentBuilder` construction is now treated as an expert/internal escape hatch rather than the primary integration path.
- Session create/continue/resume behavior is more explicitly split between:
  - live/runtime-backed mutation
  - rebuild-required paths
  - committed-create vs pre-commit failure behavior

#### `host_mode` → `keep_alive`
- `host_mode` was renamed to `keep_alive` across:
  - core/session metadata
  - RPC/REST/MCP/CLI surfaces
  - SDKs
  - docs/examples/skills
  - schema/codegen artifacts
- Keep-alive now follows stricter tri-state and validation rules:
  - explicit overrides are preserved across resumed/rebuilt sessions
  - invalid keep-alive requests are rejected before stateful execution
  - disabling keep-alive now actually stops existing drain ownership

#### Image content storage and history semantics
- Durable session/runtime state no longer treats inline base64 image bytes as canonical truth.
- Durable session history and durable runtime inputs now store blob-backed image data instead of inline-only image payloads.
- Compaction now strips session images from active history, replacing them with textual placeholders, so compacted sessions do not keep paying context cost for image-bearing turns.
- History/read surfaces now align with the new blob-backed image model instead of implying old inline-image durability.

#### Mobs and comms behavior
- `AutonomousHost` behavior was tightened so active autonomous members remain live for peer ingress instead of depending on a one-shot loop handle.
- Mob runtime now uses one canonical runtime adapter per runtime instance instead of splitting turn execution and comms ingress across different adapters.
- Persistent resume no longer silently fresh-creates missing session-backed members on persistent services.
- Broken members are now consistently excluded from wiring/selection/host-loop startup paths while remaining inspectable and repairable.

#### SDKs and generated types
- Python, TypeScript, and Web SDKs were realigned with the runtime-backed/session-first API.
- Generated SDK types and helpers were updated to reflect:
  - deferred sessions
  - richer session history contracts
  - blob-backed image content
  - regenerated event/catalog artifacts
- Python and TypeScript history parsing now preserve structured text/image content instead of flattening it back to plain strings.

### Removed

- Legacy `host_mode` terminology from public docs/examples/SDKs/contracts.
- Removed old `docs/architecture/0.5/*` planning dump in favor of the new normative architecture docs plus `.rct` material.
- Removed / scrubbed legacy delegated/helper-agent and sub-agent public-surface residue that no longer matched the settled 0.5 surface model.
- Removed a variety of dead or obsolete runtime shell helpers, stale driver entry methods, and old host-mode ownership residue that no longer matched authority-owned semantics.

### Fixed

#### Persistent resume, recovery, and identity continuity
- Fixed persistent mob resume so session-backed members preserve durable identity instead of silently coming back as fresh sessions.
- Fixed idle-live session detection during mob resume; persisted-only summaries no longer masquerade as live sessions, and live idle sessions are no longer misclassified as missing.
- Fixed session-scoped native comms identity so resumed sessions preserve `peer_id` across runtime roots.
- Fixed autonomous member runtime ownership so comms drains and runtime turns use the same canonical adapter.

#### Keep-alive / continue / resume correctness
- Fixed keep-alive ordering so validation happens before stateful execution and rejected requests are side-effect free.
- Fixed keep-alive propagation across RPC/REST/MCP/CLI resume and turn paths.
- Fixed drain survival / cleanup semantics for committed keep-alive sessions on error paths.
- Fixed late-cancel races so successful committed `turn/start` / `meerkat_resume` / similar operations are not rewritten to `REQUEST_CANCELLED`.

#### Multimodal comms and image handling
- Fixed multimodal peer ingress for autonomous mob members after kickoff completion.
- Fixed comms/runtime paths that flattened or dropped multimodal image blocks.
- Fixed multimodal body-vs-rendered-text handling so raw peer message bodies are preserved instead of replaced by lossy projections.
- Fixed provider-specific image serialization regressions (including Gemini user messages and Anthropic tool-result images).

#### Surface and contract regressions
- Fixed CLI runtime-backed teardown/output pipeline regressions.
- Fixed RPC/REST/MCP runtime-backed ingress, resume, and external-event regressions.
- Fixed REST request cancellation races and cleanup paths.
- Fixed MCP cancellability and responsiveness in `034-codemob-mcp`.
- Fixed runtime batch staging, metadata merge, UTF-8 panic, callback timeout, and retry-hint propagation regressions.

#### Compaction and budget correctness
- Fixed compaction cadence fallback across reused / legacy sessions.
- Fixed compaction token estimation so base64 image data and tiny text blocks are not miscounted.
- Fixed timeout / time-budget terminalization so structured timeout conditions retain their typed meaning.

#### Clippy, WASM, CI, and publishability
- Brought the workspace to clean `clippy -D warnings` status across a very large legacy warning backlog.
- Fixed multiple WASM compilation and gating issues across `meerkat-web-runtime`, web bindings, and example paths.
- Fixed publish / path-dependency / workspace packaging issues across crates.
- Hardened pre-push and CI gates for feature branches and generated artifact freshness.

### Breaking changes

- `host_mode` has been renamed to `keep_alive` across public surfaces and generated SDK/contracts.
- Runtime-backed session services, not direct builder execution, are now the intended public integration path.
- Durable image/history semantics changed to the new blob-backed model; old inline-image durable formats are not preserved.
- Python/TypeScript/Web SDK content and history models changed to preserve structured content instead of flattening it.
- A significant amount of stale legacy/internal surface residue was removed or renamed to match the settled 0.5 contracts.

## [0.4.13] - 2026-03-16

### Fixed

- **Mob multimodal content silently discarded** — `MobHandle::send_message` took `String`, flattening multimodal `ContentInput` (images + text) to plain text before it reached the session service. Threaded `ContentInput` end-to-end through `MobHandle`, `MobCommand`, actor dispatch, `SpawnMemberSpec`, and `to_create_session_request`. TurnDriven mode now passes `ContentInput` directly to `StartTurnRequest`; AutonomousHost mode extracts text at the `EventInjector` boundary (known limitation).
- **Wire boundaries blocked multimodal mob content** — JSON-RPC mob param structs (`MobSpawnParams`, `MobSendParams`, `MobRespawnParams`) accepted `String` instead of `ContentInput`. Updated to accept `ContentInput` directly via serde untagged deserialization (backward compatible — plain strings still work). WASM bindings now parse incoming strings as `ContentInput` JSON with text fallback. Python SDK mob methods widened to `str | list[dict]`, TypeScript/Web SDK mob methods widened to `string | ContentBlock[]`.

## [0.4.12] - 2026-03-16

### Added

#### Multimodal Content Support (Images) (#154)
- **`ContentBlock` type** — `Text` and `Image` variants in `meerkat-core`, threaded through tool results (`ToolResult.content: Vec<ContentBlock>`), user messages (`UserMessage.content: Vec<ContentBlock>`), and all provider adapters. Backwards-compatible serde: plain strings deserialize to `[Text]`, text-only content serializes as string.
- **`ContentInput` type** — untagged `Text(String) | Blocks(Vec<ContentBlock>)` accepted by `CreateSessionRequest.prompt` and `StartTurnRequest.prompt` across all surfaces (REST, RPC, CLI, MCP Server).
- **`ToolOutput` enum** — `Json(Value) | Blocks(Vec<ContentBlock>)` replaces `Value` return on `BuiltinTool::call()`, enabling tools to return multimodal content.
- **`view_image` builtin tool** — reads images from disk with path sandboxing (symlink-safe via `canonicalize`), 5MB size limit, extension validation (PNG/JPEG/GIF/WebP/SVG). Returns `ToolOutput::Blocks` with base64-encoded image data. Guarded with `#[cfg(not(target_arch = "wasm32"))]`.
- **Provider capability gating** — `ModelProfile` gains `vision` and `image_tool_results` fields. `view_image` hidden via `ToolScope` external filter for models that can't process image tool results (OpenAI). Dynamic refresh on model hot-swap: filter composes with existing restrictions instead of clobbering.
- **Provider image serialization** — Anthropic: native `image.source.base64` format in user messages and tool results. OpenAI: `image_url` data URIs in user messages, text degradation for tool results. Gemini: `inlineData` parts in user messages and alongside `functionResponse` for tool results.
- **MCP image passthrough** — `McpConnection::call_tool()` returns `Vec<ContentBlock>`, capturing `image` content from MCP servers as `ContentBlock::Image`.
- **Comms multimodal plumbing** — `blocks: Option<Vec<ContentBlock>>` added alongside `body: String` at every comms layer (`MessageKind`, `CommsContent`, `InteractionContent`, `CommsCommand`, `PlainMessage`, `InboxItem`, `SendInput`). CBOR backwards compat verified. Turn-boundary drain and host-mode batching paths preserve blocks.
- **Runtime multimodal routing** — `CoreRenderable::Blocks` variant, `PromptInput.blocks` and `PeerInput.blocks` fields, `extract_prompt()` returns `ContentInput` on both RPC and REST runtime executors.
- **Wire types** — `WireContentBlock` (no `source_path`), `WireContentInput`, `WireToolResultContent` in `meerkat-contracts`. Schema regenerated. Forward-compatible `Unknown` variant.
- **SDK multimodal prompts** — Python: `prompt: str | list[dict]` on all session methods. TypeScript: `prompt: string | ContentBlock[]`. Web SDK: `ContentInput` parsed at WASM bridge.
- **Hook `has_images` flag** — `HookToolResult.has_images` and `ToolExecutionCompleted.has_images` for downstream consumers.
- **Hook patch rebuild rule** — deterministic: strip text blocks, prepend patched text, append image blocks in original order.
- **Compaction image stripping** — `strip_images_for_compaction()` replaces images with `[image: {media_type}]` placeholders. `source_path` excluded from placeholders to prevent filesystem path leaks.
- **`Display` impl for `ContentBlock`** — delegates to `text_projection()`.
- **Dispatch-time tool gating** — hidden tools (via ToolScope external filter) are blocked at execution time, not just advertisement time.

### Changed

- **`ToolResult.content`** — `String` → `Vec<ContentBlock>` (breaking Rust API). `ToolResult::new()` still accepts `String`. Use `.text_content()` for string access.
- **`UserMessage.content`** — `String` → `Vec<ContentBlock>` (breaking Rust API). `UserMessage::text()` constructor for common case.
- **`BuiltinTool::call()` return** — `Result<Value, _>` → `Result<ToolOutput, _>` (breaking for custom tool implementors).
- **`CreateSessionRequest.prompt` / `StartTurnRequest.prompt`** — `String` → `ContentInput` (breaking, use `.into()` from String).
- **`AgentRunner::run()` / `run_with_events()`** — accept `ContentInput` instead of `String`.
- **`content_blocks_serde`** — only collapses single text block to string; multi-block text arrays serialize as arrays to preserve block boundaries.
- **`source_path`** — `#[serde(skip_serializing)]`: never persisted to session stores, only used in-memory for compaction re-read hints.

## [0.4.11] - 2026-03-15

### Fixed

- **RPC in-session model switching silently dropped** — `turn/start` model/provider/provider_params overrides were built in the handler but never reached the executor because `RuntimeTurnMetadata` did not carry those fields. Added model/provider/provider_params to `RuntimeTurnMetadata`, extracted `hot_swap_llm_client()`, and wired it into `apply_runtime_turn` so overrides propagate end-to-end.
- **MCP drain race in tests** — `set_inflight_calls_for_testing` could fire before the MCP router was ready. Now waits for `wait_until_ready` first.

## [0.4.10] - 2026-03-15

### Added

#### `meerkat-models` — Curated Model Catalog (#148)
- New leaf crate `meerkat-models` as the single source of truth for model defaults, allowlists, capability detection, and parameter schemas. Consolidates model data previously scattered across `config.rs`, `config_template.toml`, and client adapters.
- Catalog module with curated entries for all supported providers (Anthropic, OpenAI, Gemini) including default models, allowed model lists, and per-model parameter schemas.
- Profile module with provider-specific rules: per-model param schemas that document exactly which `provider_params` keys each adapter reads and processes (e.g., opus-4-6 gets adaptive thinking + effort + compaction; non-gpt-5 models don't advertise `reasoning_effort`).
- `models/catalog` endpoint on all surfaces: CLI (`rkat models catalog`), REST, RPC (`models/catalog`), and MCP Server.
- Wire types in `meerkat-contracts` (`ModelsCatalogResponse`) with SDK codegen.

#### Mid-Session Model/Provider Hot-Swap (#147)
- `Agent::replace_client()` swaps the LLM client on a live agent without rebuilding.
- `SessionAgent::replace_client()` trait method (default no-op) and `SessionService::set_session_client()` (default `Unsupported`).
- RPC `turn/start` now accepts `model`/`provider`/`provider_params` on materialized sessions, builds a new client via factory and hot-swaps before the turn.

### Fixed

#### Comms Interrupt Regressions (#147)
- **False wakes from non-actionable traffic** — raw `inbox_notify` woke `wait` on responses, acks, lifecycle traffic, and plain events. Added single-pass ingress classification in `meerkat-comms` with narrow `actionable_input_notify` that fires only for `ActionableMessage`/`ActionableRequest`. Untrusted items dropped at ingress with snapshot semantics.
- **Wait tool not interruptible on some dispatcher paths** — override and WASM dispatcher paths never wired wait interruption. Added `bind_wait_interrupt()` and `supports_wait_interrupt()` on `AgentToolDispatcher` trait with implementations on `CompositeDispatcher`, `ToolGateway`, and `FilteredToolDispatcher`. Factory probes before consuming bind and falls back gracefully.
- **Wait budget overshoot** — wait could overshoot `max_duration` by up to 1800s. `MAX_WAIT_SECONDS` reduced to 60s.
- **Trust state split between async and sync locks** — collapsed `tokio::sync::RwLock` and `parking_lot::RwLock` sidecar into single `Arc<parking_lot::RwLock<TrustedPeers>>` shared by Router, `IngressClassificationContext`, and `trusted_peers_shared()` callers. Mutations through any handle are immediately visible to classification.
- **ChildInproc trust is not inferred at construction** — `CommsBootstrap::prepare` now fails closed unless generated wiring installs parent trust through typed comms trust authority.
- **Lifecycle intent serialization** — `PeerAdded`/`PeerRetired` now serialize as `"mob.peer_added"`/`"mob.peer_retired"` via explicit `#[serde(rename)]`.
- **Host-mode hot loop spin** — legacy fallback falls through to `tokio::select!` when no work performed instead of unconditional continue that spins until budget exhaustion.
- **Legacy drain classification** — turn-boundary drain fallback now classifies by `InteractionContent` (peer lifecycle batching, response inline injection) instead of raw concat. Host-mode drain routes per-interaction with subscriber/tap/sink support.
- **Shared dispatcher consumed during bind** — factory now checks `Arc::strong_count` before `bind_wait_interrupt` and skips binding for shared dispatchers instead of consuming the caller's dispatcher.

### Changed

- **`meerkat-core` reads model defaults from catalog** — `ModelDefaults` no longer reads from `config_template.toml`; all model defaults come from `meerkat-models` catalog.
- **OpenAI adapter delegates detection to shared profiles** — capability detection logic moved from `meerkat-client` to `meerkat-models` profile rules.
- **Legacy delegated-agent validation uses catalog for fallback resolution** — the remaining `sub-agents` compatibility path in `meerkat-tools` now resolves fallback models via catalog instead of hardcoded strings.
- **Stale template defaults updated** — `gpt-4.1` → `gpt-5.2`, `gemini-1.5-pro` → `gemini-3-flash-preview`.
- **`SessionError::Unsupported` variant** — new error variant for capability negotiation across session service implementations.

## [0.4.9] - 2026-03-14

### Fixed

- **MCP tools invisible via RPC** — `start_turn_via_runtime()` (the V9 runtime path used by all RPC sessions) bypassed `apply_mcp_boundary()`, so MCP servers staged via `mcp/add` were never connected or made visible to agents. MCP tools were completely broken on the RPC surface since 0.4.7.
- **Wait tool not interruptible by peer comms** — `WaitTool` was created without interrupt support, so peer messages arriving during a `wait()` call queued silently until the wait completed. Added `WakeMode::InterruptYielding` policy for `peer_message`/`peer_request` while running, wired comms `inbox_notify` → `WaitTool` interrupt channel via factory bridge task. Agents now respond to peer messages during wait instead of blocking for up to the full requested duration.
- **Wait tool cap raised** from 300s to 1800s — sleep costs zero budget; the old 300s cap forced unnecessary LLM round-trips. Note: budget checks happen at loop boundaries, not during tool dispatch, so the cap balances responsiveness against overshoot risk in non-comms sessions.

### Changed

- **`WakeMode::InterruptYielding`** — new policy variant for peer inputs while running. Interrupts cooperative yielding points (e.g., wait tool) without cancelling active work or waking idle runtimes. Applied to `peer_message` and `peer_request` in `DefaultPolicyTable`.

## [0.4.8] - 2026-03-13

### Fixed

- **Cross-crate `include_str!` in facade** — `meerkat` crate referenced `../../meerkat-mob/skills/mob-communication/SKILL.md` which works in workspace builds but breaks when the crate is pulled from crates.io. Copied the skill file into the facade crate.
- **`meerkat-runtime` missing from crate publish order** — `meerkat-session` depends on `meerkat-runtime` but it wasn't in the CI publish sequence, causing registry publish failures.
- **Path-only dependencies break `cargo publish`** — 6 crates referenced `meerkat-runtime` via `path = "../meerkat-runtime"` without `workspace = true`, causing `cargo publish` to reject them. Fixed all to use workspace dependencies.
- **Release workflow publish decoupled from binary builds** — `publish_registries` job no longer depends on `build_binaries`, enabling manual dispatch to re-run publishing without rebuilding all 5 platforms.

## [0.4.7] - 2026-03-13

### Added

#### `meerkat-runtime` — Canonical Lifecycle Runtime (#140)
- New `meerkat-runtime` crate implementing the v9 Canonical Lifecycle and Execution Specification with a strict 3-layer model: Core (run primitives), Runtime (input lifecycle), and Surfaces (protocol adapters).
- 6 input type variants with `InputHeader`, `InputOrigin`, `PeerConvention`.
- `InputState` lifecycle: 9 states with validated transition table (`AppliedPendingConsumption` → `Queued` explicitly rejected).
- Policy engine: `DefaultPolicyTable` with `ApplyMode`, `WakeMode`, `QueueMode`, `ConsumePoint`, and `record_transcript`.
- `RuntimeState`: 7 states with strict transition table.
- Durability validation: derived forbidden for required input types.
- Coalescing/supersession: scope-based, cross-kind forbidden.
- `EphemeralRuntimeDriver` and `PersistentRuntimeDriver` (durable-before-ack via `RuntimeStore`).
- `InMemoryRuntimeStore` and `RedbRuntimeStore` backends.
- `SqliteRuntimeStore` — SQLite backend for runtime state persistence.
- `CommsInputBridge`: `InboxInteraction` → `PeerInput` conversion.
- `SessionServiceRuntimeExt` + `RuntimeSessionAdapter`: per-session driver registry.
- `MobRuntimeAdapter`: flow step delivery, member registration.
- Core lifecycle primitives in `meerkat-core/src/lifecycle/`: `RunPrimitive`, `RunEvent`, `RunBoundaryReceipt`, split executor boundary/interrupt handles, `CoreExecutor` trait, `RunId`/`InputId` newtypes.
- 222+ tests across the crate.

#### JSON-RPC Parity — 9 Wire-Feasible Gaps Closed (#138)
- **Deferred session creation**: `session/create` with `initial_turn: false` returns session ID without running a turn. `DeferredSession` class in Python and TypeScript SDKs.
- **Per-turn overrides**: `model`, `provider`, `max_tokens` overrides on `turn/start` for pending sessions.
- **Rich session responses**: `session/list` returns `WireSessionSummary` (timestamps, message counts, token totals) with pagination (`limit`/`offset`). `session/read` returns `WireSessionInfo` (timestamps, message counts, last assistant text).
- **Batch mob spawning**: `mob/spawn_many` with per-spec error reporting.
- **Scoped event streaming**: `scope_id`/`scope_path` preserved on `session/stream_event` notifications for delegated child-scope / flow-scope forwarding.
- **Persistent session recovery**: `turn/start` recovers persisted sessions after runtime restart, mirroring the REST recovery path. Archived sessions are rejected.
- **Callback tool protocol**: bidirectional JSON-RPC request/response for SDK-provided tool implementations. `ToolCallbackHandler` helpers in Python and TypeScript SDKs.
- 6 new RPC handler methods with legacy-mode guard.

#### Session History Across Surfaces (#142)
- Public session history (message listing) exposed on all surfaces: CLI `session history`, REST `GET /sessions/{id}/history`, RPC `session/history`, MCP `meerkat_session_history`.
- `SessionService` trait extended with history read capability.
- Wire types (`WireSessionHistory`, `WireSessionMessage`) added to `meerkat-contracts`.
- SDK codegen updated for Python and TypeScript.

#### SQLite Persistent Realm Backend (#143)
- `SqliteSessionStore` in `meerkat-store` — SQLite-backed session persistence for realm backends.
- SQLite is now the default persistent realm backend.
- Backend-owned persistence bundle pattern: realm backends own their persistence infrastructure rather than having it constructed externally.

### Changed

- **Persistence architecture** — persistence bundles are now opened from realm backends rather than assembled externally. Surfaces (CLI, REST, RPC) use the new bundle pattern for cleaner resource lifecycle.
- **Documentation** — refreshed persistence and session history docs across API reference, concepts, SDK guides, and architecture pages.

### Fixed

- **Host loop comms continuation** — idle host loop now triggers a continuation run when terminal comms responses (`Completed`/`Failed`) are delivered, instead of leaving agents unresponsive until the next external message. Scoped narrowly to terminal responses only; `Accepted` does not wake the host. Emits `RunStarted` before the continuation for correct stream lifecycle (#139).
- **Archived session recovery** — RPC and REST runtime recovery now rejects archived sessions instead of attempting to reconstruct them, preventing stale session resurrection.
- **Persistence helper panic** — avoided panic in persistence helper on unexpected backend state.
- **MCP tools schema test counts** — corrected test assertions after schema changes.
- **SQLite store clippy** — fixed clippy style issues in new sqlite stores.

## [0.4.6] - 2026-03-10

### Changed

- **Clean-cut comms/observability split** — removed mixed public interaction-stream APIs across Rust SDK, CLI, REST, RPC, MCP, WASM, and both Python/TypeScript SDKs. Public comms now exposes delivery (`inject`, `send_message`, `comms/send`) and explicit observation surfaces separately; interaction-scoped comms stream helpers remain runtime-internal only.
- **First-class mob and session parity across all SDKs** — Python SDK, TypeScript SDK, and Web SDK now expose explicit `Mob` and `Session` classes with typed mob lifecycle, member management, flow control, and event subscription methods. RPC surface adds dedicated `mob/*` methods (`mob/create`, `mob/list`, `mob/status`, `mob/members`, `mob/spawn`, `mob/retire`, `mob/respawn`, `mob/wire`, `mob/unwire`, `mob/lifecycle`, `mob/send`, `mob/events`, `mob/stream_open`, `mob/stream_close`, `mob/append_system_context`, `mob/flows`, `mob/flow_run`, `mob/flow_status`, `mob/flow_cancel`) as the canonical typed substrate for SDKs.
- **EventSubscription replaces CommsEventStream** — Python SDK exports `EventSubscription` instead of `CommsEventStream`/`CommsStreamEvent`. TypeScript SDK exports `EventSubscription<T>` (generic, async-iterable). Web SDK's `EventSubscription<T>` is now generic with a `parseEvents` callback.
- **Standalone session event streaming** — RPC adds `session/stream_open` and `session/stream_close` for explicit event stream lifecycle. Web SDK adds `Session.subscribe()` returning `EventSubscription<EventEnvelope>`.
- **Mob subscription methods are now async** — Web SDK `Mob.subscribe()` and `Mob.subscribeAll()` return `Promise<EventSubscription<T>>` instead of synchronous handles. `mob_member_subscribe` and `mob_subscribe_events` WASM exports are now async.
- **Mob events use numeric cursors** — `mob_events` WASM export takes a numeric `afterCursor` parameter instead of a string. Web SDK `Mob.events()` converts string cursors to numbers internally.
- **Mob create returns string directly** — `mob_create` WASM export returns a plain string mob ID instead of JSON. `mob_run_flow` similarly returns a plain string run ID.
- **Typed mob observation** — `Mob.subscribeAll()` returns `EventSubscription<AttributedEvent>` (source + profile + envelope) on Web SDK. `Mob.events()` returns `MobEvent[]` (cursor + timestamp + mob_id + kind). TypeScript RPC SDK uses `AttributedMobEvent` and `AgentEventEnvelope` types for mob/member subscriptions.
- **Web SDK types refined** — `SpawnResult` now has `status: 'ok' | 'error'` with optional `member_ref` and `error` fields. `MobMember` includes `member_ref`, `runtime_mode`, `state`, `wired_to`, and `labels`. `MobStatus` uses `state` field instead of `status` + `member_count`.

### Removed

- **`inject_and_subscribe`** — removed from WASM exports, Web SDK `Mob` class, and `MobWasmBindings` interface. Use `Mob.sendMessage()` + `Mob.subscribe()` separately.
- **`CommsEventStream` / `CommsStreamEvent`** — removed from Python SDK. Use `EventSubscription` instead.
- **`openCommsStream` / `sendAndStream`** — removed from TypeScript SDK `MeerkatClient` and `Session`. Use explicit mob/session observation subscriptions instead.

### Fixed

- **Stream termination and WASM subscription cleanup** — fixed event stream termination handling and subscription resource cleanup in the WASM runtime and RPC router.
- **Config runtime lock cleanup** — hardened lock cleanup in the config runtime to prevent leaked locks on error paths.
- **Mob-backed session routing** — hardened routing and control for mob-backed sessions, including proper session resolution for mob members.
- **MCP run handler without comms** — fixed MCP `meerkat_run` handler crash when the `comms` feature is not compiled in.
- **Pre-push unit hook stability** — serialized pre-push unit test runs across worktrees with a per-tree cache and one retry if `nextest` discovery hangs.

## [0.4.5] - 2026-03-07

### Fixed

- **Gemini tool schema validation** — `strip_gemini_function_parameters_unsupported_keywords` no longer strips property names from `properties` maps. A user-defined field named `"title"` was being removed as a JSON Schema keyword, causing Gemini to reject the schema with `required[1]: property is not defined`. The stripper now recurses into each property's schema individually without touching the properties map keys.
- **Example 033 sprite 404s** — sprite loader capped at 6 frames (00–05) instead of 8; eliminates 40 console 404s per page load.
- **API key leak in Vite bundle** — removed `define` block from example 033's `vite.config.ts` that baked raw `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `GEMINI_API_KEY` into the production bundle. Only `import.meta.env.VITE_*` (opt-in) pattern remains.
- **WASM not shipped in `@rkat/web`** — `build:wasm` script now removes wasm-pack's generated `.gitignore` (which contained `*`) so npm includes wasm artifacts in the published package.

### Added

#### Example 034: codemob-mcp — Implement Pack & User Mob CRUD
- **`implement` pack** — gated implementation with iterative review loop. Two comms-based agents (implementer + reviewer) iterate until the reviewer approves (max 3 rounds). Uses diverse models (claude-sonnet-4-6 + gpt-5.3-codex) for genuine review independence.
- **User mob CRUD tools** — `create_mob`, `get_mob`, `update_mob`, `delete_mob` MCP tools. User-created mobs stored as JSON under `.codemob-mcp/mobs/` and loaded dynamically into the pack registry without MCP restart. Both `comms` and `flow` execution modes supported.
- **Activity-based comms termination** — comms-based execution now tracks active agent turns via `RunStarted`/`RunCompleted` events instead of a fixed quiescence timeout. Agents can work for as long as needed (up to 1 hour hard cap); termination triggers when all agents are idle for a 30-second grace period.
- **Dynamic orchestrator routing** — comms initial message target and result capture now use the mob definition's orchestrator profile instead of hardcoded `"moderator"`.

## [0.4.4] - 2026-03-06

### Added

#### Session System-Context Control Plane (#121)
- `SessionServiceControlExt::append_system_context()` — inject runtime system context into a live session without rebuilding it.
- Staged appends are applied at the `CallingLlm` boundary just before the next model call.
- Idempotency enforced per session via `idempotency_key`. Duplicate keys return `Duplicate` status.
- Canonical system prompt remains the first `Message::System`; appended context follows as additional system messages.
- State carried through checkpoints, clones, forks, and persistence.
- Wired across all surfaces:
  - CLI: `session inject-context`
  - REST: `POST /sessions/{id}/system_context`
  - RPC: `session/inject_context`
  - WASM: `append_system_context(handle, request_json)`
  - Web SDK: `Session.appendSystemContext(options)`

#### Mob Member System-Context Control (#122)
- `mob_append_system_context(mob_id, meerkat_id, request_json)` WASM export — append system context to an individual mob member's session by resolving its live session through the mob roster.
- Web SDK: `Mob.appendSystemContext(meerkatId, options)` with `MobAppendSystemContextResult` type.
- Shared `resolve_mob_member_session_id()` helper deduplicates roster lookup logic with `mob_member_subscribe`.

#### Fire-and-Forget JS Tool Registration (#120)
- `register_js_tool(name, description, schema_json)` WASM export for tools that return `"acknowledged"` immediately without a JS callback.
- Agents get proper schema-validated tool calling; the host watches `ToolCallRequested` events in the stream and responds asynchronously via `mob_send_message`.
- Existing `register_tool_callback` (callback-based) unchanged — backwards compatible.
- Duplicate tool names are latest-wins across both registration modes.
- Web SDK: `MeerkatRuntime.registerFireAndForgetTool()` static method.

#### Structured Output Extraction Fix (#118)
- Structured-output extraction now unwraps provider-style named envelopes (e.g., `{"advisor": {...}}`) when the inner object matches the configured schema shape.
- `FlowStepSpec.output_format` option: `"json"` (default) or `"text"` to allow non-JSON agent outputs without parse failures.

#### Example 033: The Office — 10-Agent WASM Multi-Agent Demo (#123)
- Browser-based demo: 10 autonomous AI agents in a pixel-art office process events, coordinate via desk phone calls, store knowledge, and route actions through compliance.
- Demonstrates: mob orchestration in WASM, `autonomous_host` mode, inter-agent comms with visual phone arcs, fire-and-forget JS tools (`request_human_approval`, `upsert_record`, `revoke_access`, `restore_access`), system context injection for admin trust policy.
- Dieter Rams inspired UI: warm cream/copper chrome, tabbed log/records/graph panel, Cytoscape.js knowledge graph, floating approval panel, trust toggle (The Boss / Outsider).
- 6 pre-built scenarios: Client Escalation, Server Room Alert, Expense Report, Calendar Conflict, New Hire Onboarding, Security Breach.

#### Example 034: force-mcp — Multi-Agent Teams as MCP Tools (#119)
- Standalone MCP server exposing Meerkat mobs as `consult`, `deliberate`, and `list_packs` tools for Claude Code.
- 7 mobpacks: advisor, review, architect, brainstorm, red-team, panel (comms-driven), rct (full pipeline).
- MCP progress notifications for live progress bars during deliberation.

### Changed

- **Model names**: Updated model table — added `gpt-5.3-codex`, `gemini-3.1-pro-preview`, `gemini-3.1-flash-lite-preview`, `claude-sonnet-4-6`. Removed deprecated `o1-*`/`o3-*`/`o4-*` prefixes from provider inference (#117).
- **Demo server mode**: Diplomacy (031) and WebCM (032) demos support `?proxy=` query param for hosting behind `@rkat/web` proxy with server-side key injection (#116).
- **MCP readiness**: Aligned readiness waits with the real async connect budget; reduced adapter polling latency. Fixes flakiness under full-suite load (#121).

### Fixed

- Session context persistence: live persistent appends no longer mutate runtime state before durable save succeeds.
- Unknown session IDs no longer leak checkpointer gates.
- Pending-session promotion preserves injected context during first turn/start.
- Successful promotion no longer reports a false turn failure if replay staging has a post-create problem.
- Diplomacy demo map: filled Kosovo, North Macedonia, Albania gaps; Croatia/Slovenia render as Austria-Hungary; resolution mode cropping fix.
- Proxy CORS: `x-goog-api-key` added to allowed headers.

## [0.4.2] - 2026-03-04

### Added

#### `@rkat/web` npm Package
- New `sdks/web/` TypeScript wrapper around wasm_bindgen exports with idiomatic camelCase API.
- `MeerkatRuntime` class: `init()`, `initFromMobpack()`, `createMob()`, `createSession()`, version validation.
- `Mob` class: spawn, wire, retire, flows, event subscriptions.
- `Session` class: multi-turn agent loop, event polling.
- `EventSubscription` class: typed `poll()` / `close()` over WASM subscription handles.
- `registerTool()` static method for JS tool callback registration before init.
- TypeScript types aligned to exact Rust serde wire format (`AgentEvent`, `Profile`, `WiringRules`, `ToolConfig`).

#### Provider Proxy
- Node.js auth-injecting reverse proxy in `sdks/web/proxy/`.
- `npx @rkat/web proxy --port 3100` standalone CLI.
- Composable `createProxyHandler()` for integration into existing Node.js servers.
- Routes: `/anthropic/*`, `/openai/*`, `/gemini/*` with per-provider auth injection.
- CORS support, SSE streaming, `Accept-Encoding`/`Origin`/`Referer` header stripping.

#### Per-Provider Base URLs
- `anthropic_base_url`, `openai_base_url`, `gemini_base_url` on `Credentials`, `RuntimeConfig`, and `SessionConfig` in the WASM runtime.
- Backward-compatible: single `base_url` still works as fallback for the default model's provider.

#### MCP Server Loading
- `--wait-for-mcp` flag on `run`/`resume` blocks until all MCP servers finish connecting before first turn.
- Non-blocking parallel MCP server loading: servers connect in background, tools become available as each completes.
- Per-server `connect_timeout_secs` in `.rkat/mcp.toml` (default: 10s).
- `[MCP_PENDING]` system notice informs the LLM while servers are still connecting.

#### Mob Enhancements
- `SpawnMemberSpec.additional_instructions`: per-member system prompt additions, wired through `BuildAgentConfigParams` to `AgentBuildConfig`.
- `runtime_version()` wasm_bindgen export for JS/WASM version mismatch detection.

### Changed

- **Gemini auth**: use `x-goog-api-key` header instead of `?key=` query parameter in URL.
- **wasm32 clippy clean**: cfg-gated filesystem-only functions, removed dead imports, zero warnings with `-D warnings`.
- **CI**: added wasm32 clippy step to CI workflow.
- **Version parity**: web SDK added to `bump-sdk-versions.sh`, `verify-version-parity.sh` (6 files must now agree on version).
- **Release CI**: `@rkat/web` publish step added to release workflow, `meerkat-mob-pack` added to crate publish order.

### Fixed

- Backfill empty `Response.url` to prevent reqwest panic on wasm32 (#111).
- Proxy: strip `Accept-Encoding` from forwarded requests, `Content-Encoding`/`Transfer-Encoding` from responses (prevents `ERR_CONTENT_DECODING_FAILED`).
- Proxy: strip `Origin`/`Referer` headers to prevent Anthropic CORS rejection (#113).
- Gemini function schema lowering for type arrays and const values.
- Gemini `additionalProperties` normalization for nested schemas.
- Mob provider params propagation and Gemini reasoning deltas.
- Documentation accuracy audit: 20+ fixes across reference, guides, examples, and skills.

## [0.4.1] - 2026-02-28

### Added

#### WASM Browser Runtime
- `meerkat-web-runtime` crate with 25+ `wasm_bindgen` exports for browser deployment.
- 9 crates compile for `wasm32-unknown-unknown`: meerkat-store, meerkat-skills, meerkat-hooks, meerkat-comms, meerkat-tools, meerkat-session, meerkat (facade), meerkat-mob, meerkat-mob-mcp.
- Override-first resource injection: `AgentBuildConfig` accepts `tool_dispatcher_override`, `session_store_override`, `hook_engine_override`, `skill_engine_override` — bypasses filesystem resolution on wasm32.
- `AgentFactory::minimal()` filesystem-free factory constructor for browser environments.
- `CompositeDispatcher::new_wasm()` tool dispatcher without shell tools.
- `MobStorage::in_memory()` ephemeral mob storage for browser-hosted mobs.
- `FactoryAgentBuilder` default injection propagates wasm32-compatible resources to mob-spawned sessions automatically.
- Time compatibility layer (`meerkat_core::time_compat`) using `web-time` for `SystemTime`/`Instant` on wasm32.
- Anthropic client auto-adds `anthropic-dangerous-direct-browser-access` header on wasm32.

#### Tool Scoping and Runtime Tool Control
- `ToolScope` contract with `ToolFilter` enum (whitelist/blacklist) for per-turn tool visibility.
- Live MCP mutation: `mcp/add`, `mcp/remove`, `mcp/reload` RPC methods for runtime server provisioning.
- `tool_scope` field on `SessionBuildOptions` for session-wide tool visibility defaults.

#### Mob API Enhancements (Architecture Review)
- `Roster::session_id()` convenience accessor for direct session ID retrieval.
- `MobHandle::subscribe_agent_events()` per-member event subscription with point-in-time snapshots.
- `AttributedEvent` type for mob-level event sourcing with member attribution.
- Non-blocking `respawn` command (atomic retire + spawn).
- `SpawnPolicy` trait as extension point for auto-provisioning strategy on external turns.
- `MobEventRouter` independent async task merging per-member event streams.
- `inject_and_subscribe()` request-reply pattern for sync-like interactions over autonomous agents.

#### EventEnvelope Standardization
- Hard-cut canonical `EventEnvelope` contract: `{timestamp_ms, source_id, seq, event_id, payload}` across all surfaces.
- Strict `seq` ordering for replay and idempotency.
- Client-side malformed event guards with structured logging.

#### Schema Hardening
- Recursive `additionalProperties: false` injection at all nesting levels for Anthropic schema compliance.
- Handles arrays of objects, union types (`anyOf`), and deeply nested properties.
- Preserves user-provided `additionalProperties` settings.

#### Mobpack Archive Format
- `meerkat-mob-pack` crate: portable multi-agent deployment bundles.
- Pack/deploy/sign pipeline with Ed25519 signing, digest verification, and allowlist trust.
- WASM web build target support for browser deployment of mobpacks.

#### Documentation
- 10 new feature pages: sessions, tools, structured output, hooks, skills, memory, comms, mobs, mobpack, WASM.
- Examples gallery with 11 curated showcase applications.
- Universal surface tabs showing CLI, RPC, REST, MCP, Python, TypeScript, and Rust implementations.
- 20+ stale references fixed (version numbers, crate counts, CI descriptions, build matrix).

#### Examples
- Expanded to 31 polished examples (up from 27) covering all features and surfaces.
- All examples compile and exercise real features (validated in CI).
- WASM mini-diplomacy demo: 3-faction browser app with real mob orchestration.

#### CI/CD
- WASM compilation check job in CI workflow.
- `cargo fmt --all` auto-fix on pre-commit hook stage.
- `cargo build --workspace` added to pre-push hook stage.

### Changed
- CI parallelized to 8 jobs (~3.5 min): fmt-lint, test, test-minimal, test-feature-matrix-lib, test-feature-matrix-surface-checks, test-surface-smoke, audit, gate.
- Adopted `nextest` for faster parallel test execution across all CI jobs.
- Pedantic clippy with `-D warnings` enforced across full feature matrix.
- Toolchain updated to 1.93.1 with provenance tracking.
- Cross-surface parity: all 7 surfaces (CLI, RPC, REST, MCP, Python SDK, TypeScript SDK, Rust SDK) support EventEnvelope, tool scoping, live MCP controls, and mob feature-gating.
- Facade re-exports expanded: `ConfigStore`, `ConfigError`, `SessionServiceCommsExt`.
- CODEOWNERS file added for maintenance coverage.

### Fixed
- WASM32 runtime panics: `SystemTime`/`Instant` replaced with `web-time` shims, CORS headers for Anthropic, JSON schema validation restored.
- Mob spawn pipeline on WASM: `FactoryAgentBuilder` default injection ensures mob-spawned sessions inherit wasm32-compatible resources.
- Inproc comms enabled on wasm32 with override-first pattern.
- All 31 examples compile without warnings (non-exhaustive match, unused_mut, vec![] → array literals).
- Import ordering fixed for `cargo fmt` compliance across workspace.

## [0.4.0] - 2026-02-23

### Added

#### Mobs (Multi-Agent Orchestration)
- Introduced first-class mob runtime (`meerkat-mob`) for built-in multi-agent orchestration.
- Added DAG-based flow engine with conditions, branching, fan-out/fan-in, and dependency-aware step execution.
- Added full mob lifecycle operations with in-memory and redb-backed persistence.
- Added parallel spawn provisioning/finalization paths to support large swarm initialization.
- Added autonomous-host default runtime mode with supervision and escalation behavior.
- Added dedicated mob MCP surface (`meerkat-mob-mcp`) and integrated mob tools into CLI run/resume workflows.
- **Consolidated mob tool surface from 19 → 12 tools** with clear mob-level (`mob_*`) and member-level (`meerkat_*`) taxonomy.
- **Gated mob tools behind opt-in `--enable-mob` / `-M` flag** (default: disabled). Mob tools no longer pollute regular agent sessions.
- Mob enablement persists in session metadata for deterministic resume.

#### CLI UX
- **Stdin pipe support**: `cat file.txt | rkat run "Summarize"` reads piped input as context. Supports chaining: `cat data | rkat run "Extract" | rkat run "Analyze"`.
- **Live event streaming**: `tail -f app.log | rkat run --host --stdin "Monitor"` reads stdin line-by-line as events (infinite pipes).
- **Default `run` subcommand**: `rkat "hello"` is equivalent to `rkat run "hello"`.
- **Compact session references**: `rkat resume last`, `rkat resume ~2`, `rkat resume 019c8b99` (short prefix, git-style).
- **`continue` command**: `rkat continue "keep going"` (alias: `rkat c`) resumes the most recent session.
- Session output shows compact 8-char ID instead of full UUID.

#### MobHandle SDK Renames
- `external_turn` → `send_message`, `list_meerkats` → `list_members`, `get_meerkat` → `get_member`.
- `spawn_member_ref*` → `spawn` / `spawn_with_backend` / `spawn_with_options`.
- `spawn_many_member_refs` → `spawn_many`. `spawn()` now returns `MemberRef` (old `SessionId` compat path removed).

#### Skills v2.1
- Added strict source-pinned skill identity model (`SkillKey`) and structured skill refs.
- Enforced explicit model-mediated skill activation (`load_skill`) and removed legacy fuzzy fallback behavior.
- Brought CLI and SDK parity for skills v2.1 references and diagnostics.

#### Docs and Examples
- Added comprehensive examples library (27 examples) covering providers, hooks, memory, comms, and mob coordination.
- Added design philosophy reference and updated architecture summaries.
- Rewrote README for clearer platform/surface positioning.

### Changed
- Added config CLI CAS UX (`--expected-generation`) and generation-aware responses.
- Added configurable compaction settings to config surface and runtime wiring.
- Documented and tested config merge/override semantics across scalar/list/map fields.
- Added deferred initial-turn policy support for session creation, used by mob spawns.
- Added session-wide event stream API parity across services/surfaces.
- TypeScript SDK package renamed from `@meerkat/sdk` to `@rkat/sdk`.
- Release pipeline now publishes all 18 Rust crates (added `meerkat-mob`, `meerkat-mob-mcp`, `rkat`).

### Fixed
- Fixed OpenAI streaming duplicate/replay edge cases and strengthened error mapping.
- Fixed skills source fallback and identity resolution bugs across CLI/SDK surfaces.
- Fixed multiple mob lifecycle correctness issues (ordering races, shutdown/startup sequencing, duplicate wire side-effects).
- Stabilized host-mode and provider-agnostic integration tests.
- Addressed workspace clippy blockers and CI/push gate regressions.
- Fixed release tooling portability (`sed`-portable hook path) and lock-step contract/package version handling.

## [0.3.0] - 2026-02-14

### Added

#### Comms Command Plane Redesign
- Canonical `send` and `peers` tools replacing 4 legacy tools (`send`, `send_request`, `send_response`, `peers`)
  - `send`: unified command dispatch with flat `kind` discriminator for all comms operations
  - `peers`: list all visible peers
- `comms/send` and `comms/peers` RPC methods with flat-schema validation
- `POST /comms/send` and `GET /comms/peers` REST endpoints
- Python SDK methods: `send()` and `peers()` replacing `push_event()`
- TypeScript SDK methods: `send()`, `send_and_stream()`, and `peers()` replacing `pushEvent()`
- Optional peer authentication with fallback to in-process peer context
- `TrustedAndInproc` trust source for hybrid peer resolution

#### Interaction-Scoped Event Streaming
- `EventTap` mechanism for scoped event subscription per interaction
- `SubscribableInjector` extending `EventInjector` with `inject_with_subscription()` for dedicated interaction streams
- `InteractionSubscription`, `InteractionId`, `InteractionContent`, and `ResponseStatus` types in meerkat-core
- Host-mode interaction FSM with scoped event delivery
- Terminal completion events (`InteractionComplete`, `InteractionFailed`) for stream lifecycle management

#### CD Infrastructure
- Version parity verification: `make verify-version-parity` enforces Rust workspace, Python SDK, TypeScript SDK, and contract version alignment as a CI gate
- Schema freshness check: `make verify-schema-freshness` detects stale committed schema artifacts
- `cargo-release` configuration with pre-release hook that bumps SDK versions, regenerates schemas, and verifies parity
- Release scripts: `scripts/verify-version-parity.sh`, `scripts/bump-sdk-versions.sh`, `scripts/release-hook.sh`, `scripts/verify-schema-freshness.sh`
- `make regen-schemas` target for re-emitting schemas and running SDK codegen
- `make release-preflight` for full pre-release checklist (CI + schema freshness)
- `make publish-dry-run` for cargo publish readiness checks across all crates

### Changed

#### Versioning (Breaking)
- Package version and contract version are now lock-stepped (both `0.3.0`)
- Contract version bumped to `0.3.0` reflecting comms API changes
- All schema artifacts and SDK generated types regenerated for contract version `0.3.0`

#### Comms API (Breaking)
- Comms tools reduced from 4 to 2: `send` (with `kind` discriminator) and `peers`
- RPC: `event/push` removed, replaced by `comms/send`
- REST: `POST /sessions/{id}/event` deprecated in favor of `POST /comms/send`
- SDK: `push_event()`/`pushEvent()` removed; use `send()`/`peers()` instead

#### Host Mode
- Strict state transitions via `.transition()` instead of raw assignment
- Interaction processing classified into individual vs batched modes
- Host drain state reset on all exit paths

#### Dependencies
- Removed vendored `hnsw_rs` (was unmodified upstream v0.3.3); now resolved from crates.io
- `verify-version-parity` wired into `make ci` pipeline

### Fixed
- `ToolUse` args deserialization robust under Message buffering with custom deserializer
- Idempotent `stream_close` preventing duplicate close errors
- Comms stream completion cleanup preventing reservation leaks
- Comms self-input guard preventing agents from responding to their own messages
- E2E test model names updated to canonical providers (`gpt-5.2`, `gemini-3-pro-preview`)
- Clippy fix: `.or_insert_with(Vec::new)` → `.or_default()` in `SessionProjector`

### Removed
- 4 legacy comms tools (`send`, `send_request`, `send_response`, `peers`) -- now return "Unknown tool"
- `event/push` RPC method
- `push_event()`/`pushEvent()` SDK methods
- `vendor/hnsw_rs/` directory and `[patch.crates-io]` section

## [0.2.0] - 2026-02-12

### Added

#### Contracts and Capabilities
- `meerkat-contracts` crate: single source of truth for all wire-facing types, capability model, error contracts, and schema emission
- `CapabilityId` enum with distributed `inventory`-based registration across feature-gated crates
- `CapabilityStatus` (Available, DisabledByPolicy, NotCompiled, NotSupportedByProtocol) for runtime status
- `WireError` canonical error envelope with `ErrorCode` projections to JSON-RPC codes, HTTP status, and CLI exit codes
- `ContractVersion` with semver compatibility checking (currently 0.1.0)
- Composable request fragments: `CoreCreateParams`, `StructuredOutputParams`, `CommsParams`, `HookParams`, `SkillsParams`
- Wire response types: `WireUsage`, `WireRunResult`, `WireEvent`, `WireSessionInfo`, `WireSessionSummary`
- Feature-gated `JsonSchema` derives on all wire types
- `emit-schemas` binary for deterministic schema artifact generation (`artifacts/schemas/`)
- `capabilities/get` endpoint on all four surfaces (CLI, REST, MCP Server, JSON-RPC)

#### Skills System
- `meerkat-skills` crate with skill sources (filesystem, embedded, in-memory, composite), parser, resolver, renderer, and engine
- Core skill contracts in `meerkat-core/src/skills/`: `SkillId`, `SkillScope`, `SkillDescriptor`, `SkillDocument`, `SkillError`, `SkillSource` and `SkillEngine` traits
- 8 embedded skills: `task-workflow`, `shell-patterns`, `sub-agent-orchestration` (legacy name), `multi-agent-comms`, `mcp-server-setup`, `hook-authoring`, `memory-retrieval`, `session-management`
- Skill inventory section injected into system prompt via `extra_sections` slot
- Per-turn skill injection via `<skill>` tagged blocks prepended to user messages
- `SkillsResolved` and `SkillResolutionFailed` agent events
- Filesystem skill sources: `.rkat/skills/` (project) and `~/.rkat/skills/` (user)

#### Python and TypeScript SDKs
- SDK codegen pipeline (`tools/sdk-codegen/`) reading from `artifacts/schemas/`
- Python SDK (`sdks/python/`): async MeerkatClient with subprocess lifecycle, capability gating, version checks
- TypeScript SDK (`sdks/typescript/`): MeerkatClient with subprocess lifecycle, capability gating, version checks
- Generated types committed (Python: dataclasses, TypeScript: interfaces)
- SDK error types: `MeerkatError`, `CapabilityUnavailableError`, `SessionNotFoundError`, `SkillNotFoundError`
- Python conformance tests (8 type/error tests)

#### SDK Builder
- Builder tool (`tools/sdk-builder/build.py`): resolves features, builds runtime, emits schemas, runs codegen, emits bundle manifest
- Profile presets: `profiles/minimal.toml`, `profiles/standard.toml`, `profiles/full.toml`
- Bundle manifest with source commit, features, contract version, hashes, timestamp

#### Hooks System
- `meerkat-hooks` crate with `DefaultHookEngine`
- 3 hook runtimes: in-process (Rust handlers), command (stdin/stdout JSON), HTTP (remote endpoints)
- 8 hook points: `run_started`, `run_completed`, `run_failed`, `pre_llm_request`, `post_llm_response`, `pre_tool_execution`, `post_tool_execution`, `turn_boundary`
- Guardrail semantics: first deny short-circuits, deny always wins over allow
- Patch semantics: foreground patches applied in `(priority ASC, registration_index ASC)` order
- Background hooks with observe-only pre-hooks and `HookPatchEnvelope` post-hooks
- Failure policies: observe defaults to fail-open, guardrail/rewrite default to fail-closed
- Per-run hook overrides via `HookRunOverrides` (add entries, disable hooks)

#### Legacy Sub-Agent Surface (pre-0.5)
- `agent_spawn` and `agent_fork` tools for parallel delegated child work
- `agent_status`, `agent_cancel`, `agent_list` management tools
- `SubAgentManager` with concurrency limits, nesting depth control, and budget allocation
- `ContextStrategy` for spawn context: `FullHistory`, `LastTurns(n)`, `Summary`, `Custom`
- `ToolAccessPolicy`: `Inherit`, `AllowList`, `DenyList` for delegated child-agent tool filtering
- `ForkBudgetPolicy`: `EqualSplit`, `Proportional`, `Fixed` for budget allocation
- Model allowlists per provider for delegated child-agent spawns

#### Comms (Inter-Agent Communication)
- `meerkat-comms` crate with `Router`, `Inbox`, `InprocRegistry`
- 3 transport backends: Unix Domain Sockets (UDS), TCP, in-process
- `Keypair`/`PubKey`/`Signature` identity system with Ed25519
- `TrustedPeers` trust model with peer verification
- `Envelope` wire format with `MessageKind` variants: `Message`, `Request`, `Response`, `Ack`
- Comms tools: `comms_send`, `comms_request`, `comms_response`, `comms_peers`
- Host mode for long-running agents that process comms messages

#### Memory and Compaction
- `meerkat-memory` crate with `HnswMemoryStore` (hnsw_rs + redb)
- `SimpleMemoryStore` for testing
- `MemoryStore` trait in meerkat-core: `index`, `search`, similarity scoring
- `memory_search` builtin tool for agent access to semantic memory
- Memory indexing of compaction discards wired into agent loop
- `DefaultCompactor` in meerkat-session: auto-compact at token threshold, LLM summary, history rebuild
- `CompactionConfig` for threshold tuning

#### Structured Output
- `OutputSchema` type with `MeerkatSchema`, name, strict mode, compat, and format options
- Schema validation and retry logic for structured output
- `SchemaWarning` for compilation issues
- Provider-specific schema adaptation (Anthropic, OpenAI, Gemini)

#### Session Management
- `SessionService` trait in meerkat-core: create, turn, interrupt, read, list, archive
- `EphemeralSessionService` (in-memory) and `PersistentSessionService` (redb-backed)
- `RedbEventStore` append-only event log
- `SessionProjector` materializing `.rkat/sessions/` files from events
- `RedbSessionStore` for session persistence
- All four surfaces (CLI, REST, MCP Server, JSON-RPC) route through `SessionService`

#### JSON-RPC Server
- `meerkat-rpc` crate with JSON-RPC 2.0 over JSONL stdin/stdout
- `SessionRuntime`: stateful agent manager with dedicated tokio tasks per session
- Methods: `initialize`, `session/create`, `session/list`, `session/read`, `session/archive`, `turn/start`, `turn/interrupt`, `config/get`, `config/set`, `config/patch`
- `session/event` notifications with `AgentEvent` payload during turns

#### Builtin Tools
- Task management: `task_create`, `task_update`, `task_get`, `task_list`
- Shell execution: `shell` (Nushell backend), `shell_jobs`, `shell_job_status`, `shell_job_cancel`
- Utility: `wait`, `datetime`
- Three-tier tool policy: `ToolPolicyLayer` soft policies, `EnforcedToolPolicy` hard constraints, per-tool `default_enabled()`

#### MCP Server Capabilities
- `meerkat-mcp-server` crate exposing `meerkat_run` and `meerkat_resume` as MCP tools
- `McpRouterAdapter` relocated from CLI to `meerkat-mcp` for all surfaces

#### Build Profiles
- Profile presets for controlling feature composition: `profiles/minimal.toml`, `profiles/standard.toml`, `profiles/full.toml`
- Profiles drive SDK builder feature resolution and bundle manifests

#### E2E Tests
- 21-scenario E2E smoke test suite across 5 surfaces (CLI, REST, MCP Server, RPC, SDK)
- Integration-real tests for process spawning and live APIs
- Fast test suite gating for CI (unit + integration-fast, skipping doctests)
- Kitchen-sink compound RPC test replacing mock-only coverage

#### Prompt Assembly
- Unified `assemble_system_prompt` with documented precedence: per-request override > config file > config inline > default + AGENTS.md
- `extra_sections` slot for skill inventory injection
- Config fields `agent.system_prompt`, `agent.system_prompt_file`, `agent.tool_instructions` fully wired

### Changed
- Project renamed from "raik" to "Meerkat" with CLI binary `rkat`
- `AgentFactory::build_agent()` is now the centralized agent construction pipeline for all surfaces
- `FactoryAgentBuilder` bridges `AgentFactory` into `SessionAgentBuilder` trait
- All wire types consolidated into `meerkat-contracts` (removed per-surface duplicates)
- Error handling unified via `WireError` with protocol-specific projections
- Helper functions deduplicated: `resolve_host_mode()` to meerkat-comms, `resolve_store_path()` to meerkat-store, `spawn_event_forwarder()` to facade
- OpenAI and Gemini added to default CLI features
- Test infrastructure stabilized: fast test target isolation, real E2E gating, pre-commit hook fixes for bin-only crates

### Changed - Feature defaults
- `meerkat-tools`: comms, mcp, and the legacy `sub-agents` compatibility feature are now optional features (default: on)
  - `--no-default-features` builds tools with zero optional deps
  - Features: `comms`, `mcp`, `sub-agents`
- `meerkat` facade: comms, mcp, and the legacy `sub-agents` compatibility feature are now optional features (default: on)
  - Features: `comms`, `mcp`, `sub-agents`
- `meerkat-rpc`: comms, mcp, mob, and the legacy `sub-agents` compatibility feature are optional features (default: on)
  - `--no-default-features` builds the minimal server surface
  - Features: `comms`, `mcp`, `mob`, `sub-agents`
- `meerkat-rest`: comms is opt-in (default: on), no comms code when disabled
- `meerkat-mcp-server`: comms is opt-in (default: on), no comms code when disabled
- `meerkat-cli`: comms and mcp are opt-in (default: on), all inline code cfg-gated
- `agent_spawn` tool: `host_mode` field removed from schema when comms feature is off

### Fixed
- Anthropic streaming: emit `ToolCallComplete` on `content_block_stop`
- SDK E2E tests: session list uses `session_id` not `id`
- Python SDK async issues and TypeScript SDK brought to feature parity
- `active_skill_ids` now collects from all skill sources (not just embedded)
- SDK builder memory-store feature resolution
- SDK builder feature forwarding and dead `usage_instructions` removal
- `CapabilityStatus` parsing in SDKs and `contract_version` field inclusion
- RPC `session/create` expanded to full `AgentBuildConfig` parity
- Provider schema lowering moved from core to adapters, removing provider leakage
- `thought_signature` removed from generic `ToolCall`/`ToolResult` (provider-specific only)
- Config-driven delegated child-agent compatibility policy with fail-closed validation
- Legacy `sub-agents` compatibility, comms, and memory enabled through RPC/SDK surfaces

### Removed
- Dead files in meerkat-core: `comms_runtime.rs`, `comms_bootstrap.rs`, `comms_config.rs`, `agent/comms.rs`
- Duplicate `LlmClientAdapter`/`DynLlmClientAdapter` in meerkat-tools (uses canonical from meerkat-client)
- Per-surface wire type definitions (replaced by `meerkat-contracts`)
- Duplicated helper functions across surface crates

## [0.1.0] - 2026-01-15

Initial development release.

[Unreleased]: https://github.com/lukacf/meerkat/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/lukacf/meerkat/compare/alpha/v0.7.0-alpha.0...v0.7.0
[0.7.0-alpha.0]: https://github.com/lukacf/meerkat/releases/tag/alpha/v0.7.0-alpha.0
[0.6.23]: https://github.com/lukacf/meerkat/compare/v0.6.22...v0.6.23
[0.6.22]: https://github.com/lukacf/meerkat/compare/v0.6.21...v0.6.22
[0.6.21]: https://github.com/lukacf/meerkat/compare/v0.6.20...v0.6.21
[0.6.20]: https://github.com/lukacf/meerkat/compare/v0.6.19...v0.6.20
[0.6.19]: https://github.com/lukacf/meerkat/compare/v0.6.18...v0.6.19
[0.6.18]: https://github.com/lukacf/meerkat/compare/v0.6.17...v0.6.18
[0.6.17]: https://github.com/lukacf/meerkat/compare/v0.6.16...v0.6.17
[0.6.16]: https://github.com/lukacf/meerkat/compare/v0.6.15...v0.6.16
[0.6.15]: https://github.com/lukacf/meerkat/compare/v0.6.14...v0.6.15
[0.6.14]: https://github.com/lukacf/meerkat/compare/v0.6.13...v0.6.14
[0.6.13]: https://github.com/lukacf/meerkat/compare/v0.6.12...v0.6.13
[0.6.12]: https://github.com/lukacf/meerkat/compare/v0.6.11...v0.6.12
[0.6.11]: https://github.com/lukacf/meerkat/compare/v0.6.10...v0.6.11
[0.6.10]: https://github.com/lukacf/meerkat/compare/v0.6.9...v0.6.10
[0.6.9]: https://github.com/lukacf/meerkat/compare/v0.6.8...v0.6.9
[0.6.8]: https://github.com/lukacf/meerkat/compare/v0.6.7...v0.6.8
[0.6.7]: https://github.com/lukacf/meerkat/compare/v0.6.6...v0.6.7
[0.6.6]: https://github.com/lukacf/meerkat/compare/v0.6.5...v0.6.6
[0.6.5]: https://github.com/lukacf/meerkat/compare/v0.6.4...v0.6.5
[0.6.4]: https://github.com/lukacf/meerkat/compare/v0.6.3...v0.6.4
[0.6.3]: https://github.com/lukacf/meerkat/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/lukacf/meerkat/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/lukacf/meerkat/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/lukacf/meerkat/compare/v0.5.2...v0.6.0
[0.5.2]: https://github.com/lukacf/meerkat/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/lukacf/meerkat/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/lukacf/meerkat/compare/v0.4.9...v0.5.0
[0.4.9]: https://github.com/lukacf/meerkat/compare/v0.4.8...v0.4.9
[0.4.8]: https://github.com/lukacf/meerkat/compare/v0.4.7...v0.4.8
[0.4.7]: https://github.com/lukacf/meerkat/compare/v0.4.6...v0.4.7
[0.4.6]: https://github.com/lukacf/meerkat/compare/v0.4.5...v0.4.6
[0.4.5]: https://github.com/lukacf/meerkat/compare/v0.4.4...v0.4.5
[0.4.4]: https://github.com/lukacf/meerkat/compare/v0.4.2...v0.4.4
[0.4.2]: https://github.com/lukacf/meerkat/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/lukacf/meerkat/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/lukacf/meerkat/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/lukacf/meerkat/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/lukacf/meerkat/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/lukacf/meerkat/releases/tag/v0.1.0
