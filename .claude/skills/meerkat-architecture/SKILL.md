---
name: meerkat-architecture
description: "Internal architecture guide for the Meerkat agent platform. This skill should be used when understanding crate ownership, trait contracts, the agent construction pipeline, session service lifecycle, runtime control plane, mob orchestration internals, machine authority, comms wiring, or making cross-cutting architectural changes. Oriented toward AI agents and developers working on meerkat internals, not end users."
---

# Meerkat Internal Architecture

Meerkat is a library-first agent runtime. The execution pipeline is shared across all surfaces. Understanding crate ownership, trait contracts, the DSL-authoritative machine system, and the runtime control plane is essential for making changes that don't break architectural invariants.

This file is a lean navigator. Load the specific reference under `references/` when working on that domain.

## Core Principles

1. **Infrastructure, not application** — the agent loop is a composable primitive with no opinions about prompts, tools, or output.
2. **Trait contracts own the architecture** — `meerkat-core` defines contracts; implementations live in satellite crates.
3. **Surfaces are skins, not authorities** — CLI, REST, RPC, MCP, WASM route through shared substrate and factory seams. Runtime semantics (keep-alive, input admission, drain lifecycle, commit boundaries) are owned by the shared runtime control plane (`MeerkatMachine` in `meerkat-runtime`); runtime-backed surfaces lower into it, they do not own it.
4. **Composition over configuration** — optional components are `Option<Arc<dyn Trait>>`, not feature-flagged defaults.
5. **Runtime conforms to catalog-generated machines** — the runtime follows the verified DSL model, not the other way around. Production bridge modules import/invoke catalog-owned DSL bodies; they do not author competing machine semantics.
6. **Mob is the only multi-agent runtime path** — no separate sub-agent substrate. User-facing "delegate"/"sub-agent" flows compile to mob members, often inside session-owned implicit mobs.
7. **Override-first resource injection** — `AgentBuildConfig` overrides take precedence over factory/config/filesystem resolution.
8. **Seams are formal** — async owner handoffs, wait barriers, and surfaced terminal classes are modeled in the DSL or composition protocols, not left to shell convention.
9. **Parity gates are dogmatic** — production/catalog schema drift, command alphabet drift, string whitelists, and handwritten production machine bodies are CI failures, not review notes.

## Build And Test Architecture

Make is the developer-facing command surface. Cargo is still the default
backend; BuildBuddy/Bazel is an optional acceleration backend selected with the
single opt-in switch `MEERKAT_BUILDBUDDY=1`.

Architectural split:

- `Makefile` owns the stable local verbs: `make build`, `make check`,
  `make lint`, `make test`, `make test-unit`, `make test-int`, `make e2e-fast`,
  `make e2e-system`, `make e2e-live`, and `make e2e-smoke`.
- `scripts/run-build-backend-lane` is the backend switch. It runs
  `scripts/repo-cargo` by default and delegates to `scripts/buildbuddy-dev`
  only when BuildBuddy is explicitly enabled.
- `scripts/buildbuddy-dev` is the local BuildBuddy facade. Use it or the
  explicit `make buildbuddy-*` targets for local RBE lanes; avoid raw `bb`
  except when debugging the wrapper.
- `scripts/buildbuddy-bazel-poc` is the lower-level Bazel launcher and
  compatibility layer. It may temporarily rebase stale absolute local path
  dependencies in `MODULE.bazel.lock`, runs local lanes with
  `--lockfile_mode=error` against checked-in Bazel metadata, and restores the
  checked-in lockfile plus vendored generated BUILD bytes after Bazel exits.
  Persistent BUILD regeneration and module-lock updates are explicit maintenance
  steps, not normal local BuildBuddy lane behavior.
- `.github/workflows/ci.yml` runs a single Cargo lane on free GitHub-hosted
  runners (`cargo.yml`: change classification, lint+governance, tests,
  generation ratchets, Web SDK/wasm, cargo-deny — parallel jobs, shared
  rust-cache). Expensive low-churn lanes (feature matrices, minimal-feature,
  surface modularity, e2e-system) run in `nightly.yml`. The GCP BuildBuddy CI
  lane was retired 2026-07-03 (`buildbuddy.yml` is inert, pending deletion);
  BuildBuddy remains an OPTIONAL local backend (`MEERKAT_BUILDBUDDY=1`) and
  the hosted RELEASE binary flow is unchanged.

Test lane authority belongs in Rust, not shell glue. The canonical e2e lane
catalog is `tests/integration/src/e2e_lanes.rs`; scripts and Bazel targets
should route to that taxonomy rather than inventing parallel classifications.

For same-checkout multi-agent work, set distinct `RUST_LANE_ID` values when you
want stable warm local output roots. Separate Git worktrees are isolated by path
hash for both Cargo and BuildBuddy output roots.

## Current Branch Architecture Deltas

When updating architecture docs or reviewing current code, do not stop at the
0.6.5 live-adapter picture. The current branch since 0.6.23 also includes:

- `WorkGraphLifecycleMachine` and `WorkAttentionLifecycleMachine`, with
  `meerkat-workgraph` owning durable work items, dependencies, claims,
  evidence, ready derivation, snapshots, and goal attention binding
  pause/resume/stop/supersession state.
- WorkGraph attention/goals are WorkGraph-owned, optional-feature state:
  public REST/RPC expose observability, CLI/trusted hosts expose narrow
  session-bound goal controls, and core Meerkat must not grow a separate
  generic `GoalLifecycleMachine`.
- Same-session transcript rewrite (`session/rewrite_transcript`,
  `session/transcript_revision`, `session/restore_transcript_revision`) updates
  the transcript head through audited revisions without changing session
  identity.
- Azure OpenAI auth/backend support, project-local CLI realm defaults, HTML
  artifact output, typed transcript notices, provider-native search/image
  improvements, structured skill identity, and model-aware compaction defaults.
- `gemini-3.5-flash` as the recommended Gemini text model; the older Gemini 3
  Flash preview row is not the featured/default model.
- Batch mob wiring (`wire_members_batch` / `MembersWiredBatch`), backpressured
  dense-mob delivery, autonomous member injector validation, spawn boundary
  customization, mob task-workflow guidance preloads, peer wake fixes, and safer
  active-turn retirement/respawn behavior.
- Runtime-store checkpointing and runtime-committed session projection saves,
  preserving explicit broken/lost states and keeping MobKit/UnifiedRuntime
  projections in sync after the machine commit succeeds.
- Active-turn live-boundary steer injection is a two-owner transaction:
  session services may stage boundary context, but the runtime machine remains
  the delivery authority. If the machine/runtime-store commit fails after
  staging, the session-side staged context must be rolled back by the same
  idempotency keys; otherwise the system has phantom or duplicate delivery.
- Release/docs/SDK hardening: docs validation, version/schema freshness checks,
  BuildBuddy/Web SDK recovery lanes, and Windows asset routing fixes.

Since the PR #759 dogma campaign (and its follow-up lanes), additionally:

- `MeerkatId` → `AgentIdentity` everywhere (wire field `meerkat_id` →
  `agent_identity`); binding atoms (`agent_runtime_id`, `fence_token`,
  bridge session ids) are bridge-internal, never app-facing.
- Machine-authorized post-discard member revival (observe → classify →
  realize; `Broken` is terminal and refuses retry) and MobMachine-owned
  `pending_recipient_trust` wiring obligations; `TrustStore` is PeerId-keyed.
- `SessionDocumentMachine` owns the archive lifecycle-terminal (the
  archive mode-split and resurrection window are unrepresentable).
- Auth acquire-first commits: the AuthMachine lifecycle marker is the durable
  proof-of-acquisition (`publish_token_lifecycle_acquired`); `TokenStore` is
  the vault contract.
- Comms classification carries typed `from_peer_id` and machine-echoes the
  canonical peer id; `CurrentTurnImageRef` newtype for turn image references.
- Image-gen routing follows session identity
  (`SessionModelRoutingStatus.session_provider`); planner-side
  `infer_from_model` deleted.
- Governance gates converted to typed syn-AST xtask gates (effect-authority,
  bridge-classifier, ownership-ledger `--check-drift`, strict RMAT,
  seam-inventory) — old shell-script scanners deleted.
- Net legacy elimination: migration shims, JSON codecs for machine payloads
  (`OpTerminalPayload` is a domain type), the comms-agent runtime, and the
  `MemberState` mirror were deleted; fail-closed v2-only persisted session
  versions are enforced by the generated
  `SessionPersistenceVersionAuthority`.

Since 0.7.12 (PR #821, the MobKit upstream asks):

- **Typed injected context** — `TranscriptUserRole::InjectedContext` marks
  host-attached ambient context delivered as SEPARATE user-channel messages
  immediately before the turn's user message. The role is slot-derived: hosts
  supply `injected_context: Vec<ContentInput>` on `StartTurnRequest` /
  `CreateSessionRequest`, RPC `turn/start` / `session/create` params, REST
  continue/create, mob `WorkSpec` (submit-work lane), and
  `BridgeDeliveryPayload` (remote members) — never a free-form role string.
  `Message::indexable_content()` now consults `transcript_role`:
  `CompactionSummary` and `InjectedContext` are typed `MemoryIndexExclusion`
  variants (discarded summaries and injected memory never re-index). Rewrite
  ingress accepts only {conversational, injected_context};
  `compaction_summary` is rejected fail-closed (runtime-mintable only).
  `ConversationAppendRole::InjectedContext` is the runtime-path append
  carrier, ordered BEFORE the user append.
- **MemoryStore lifecycle** — the trait gains `drop_scope(&MemoryOwner)` and
  paged `enumerate_scoped` (deterministic durable-id order, optional
  `source_overlap` / `indexed_after` filters), defaulting to typed
  `MemoryStoreError::Unsupported`. `HnswMemoryStore` loads scope indexes
  lazily (open() no longer scans/embeds every session in the realm),
  migrated the shared sqlite to an indexed `session_id` column (race-safe
  BEGIN IMMEDIATE migration + idempotent NULL-row heal before every scoped
  op) and allocates point ids from a durable counter table (never reused
  after drop).
- **Compaction curator** — `CompactionCurator` (meerkat-core/src/compact.rs)
  substitutes summary CONTENT production for the compaction LLM call
  (`Option<Arc<dyn CompactionCurator>>` on Agent/AgentBuilder;
  `AgentBuildConfig.compaction_curator_override`). Curator failure is the
  typed `CompactionFailureReason::CuratorFailed` — no silent LLM fallback.
  Trigger, commit, and the indexing-gates-commit invariant are unchanged.
- **Transcript revision reads** —
  `SessionServiceHistoryExt::list_transcript_revisions` + RPC
  `session/transcript_revisions` (JSON-RPC only, like the rest of the
  transcript-edit family); restore parses `RevisionSelector` (restoring
  `current` surfaces the typed NoOpRewrite instead of a stringly lookup).
- **Comms content taint** — `SenderContentTaint { Clean, Tainted }` is
  core-owned vocabulary (meerkat-core/src/comms.rs) carried as an optional
  field INSIDE the signed `MessageKind` region (absent = byte-identical to
  pre-field envelopes; present fails verification closed on old receivers).
  Sender side: `CommsRuntime::set_outbound_content_taint` + tri-state
  `SendTaintOverride` on send params (absent = inherit the runtime
  declaration). Receiver side: `SystemNoticeBlock::Comms.sender_taint` is
  the typed transcript carrier. None ≠ Clean — never coalesce. Taint makes
  no admission/routing decision; it is content-adjacent typed metadata, NOT
  machine-echoed.
- **Dispatch-time provenance for hooks** — `HookToolCall` / `HookToolResult`
  carry `provenance: Option<ToolProvenance>`; `HookLlmResponse` carries
  `server_tool_content: Vec<ServerToolKind>` — a foreground hook classifies
  tool-source and provider-native (web-search) ingestion synchronously,
  before results commit.
- **Call-level tool authorization** — `ops::ToolAccessPolicy` is now
  ENFORCED (previously admission-gated then dropped at the actor spawn
  sites): `ToolExecutionPolicy` (sealed resolved form) +
  `ExecutionPolicyGatedDispatcher`
  (meerkat-core/src/tool_execution_policy.rs) gate execution while leaving
  `tools()`/`tool_catalog()` byte-identical (prompt-cache preserved). Deny
  surfaces as an ordinary `access_denied` tool error (run continues); hook
  denials remain the run-fatal channel. The gate wraps OUTERMOST in the
  factory; the effective policy persists in session metadata
  (`tooling.tool_access_policy`) and `Inherit` resolves transitively to the
  parent's effective policy (no escape-by-spawn).
- **Host-runnable schedule targets** — `TargetBinding::HostRunnable`
  (`HostRunnableName` + wire-opaque params) + feature-owned
  `ScheduleRunnableHost` / `HostRunnableRegistry`
  (meerkat-schedule/src/runnable.rs);
  `SharedScheduleTargetAdapter::with_runnable_host` routes probe/deliver
  through the normal occurrence lifecycle. Machines untouched (targets stay
  opaque `TargetBindingId` keys); failures map onto existing typed reasons
  (unregistered → TargetMissing, callback error → RuntimeRejected).

Since the Ask 9/10 follow-ups (post-0.7.12):

- **Host-consumable taint surfaces** — `AgentEvent::PeerContentIngested`
  projects the committed incoming `SystemNoticeBlock::Comms` (canonical peer
  identity, comms kind, request id, `sender_taint`) onto the observe stream
  at BOTH commit sites (queued transcript appends and steer boundary
  context), via the single projection owner
  `event::peer_content_ingested_events` — hosts classify peer ingestion from
  typed facts, never projection text. Outbound: the core `CommsRuntime`
  trait gains `set_outbound_content_taint` (default typed
  `SendError::Unsupported` — never a silent no-op), reachable per mob member
  via `MobHandle::declare_member_outbound_taint` /
  `MemberHandle::declare_outbound_taint` (local members: the member's own
  session comms runtime; external members:
  `BridgeCommand::DeclareMemberOutboundTaint` over the supervisor bridge).
  The declaration is host-owned carrier config — it does not survive
  respawn (fresh-context semantics; hosts re-declare).
- **Runtime schedule host runnables** — `spawn_runtime_backed_schedule_host`
  and `_with_mobs` accept `Option<Arc<dyn ScheduleRunnableHost>>`, wiring
  the host's `HostRunnableRegistry` into the runtime-backed occurrence
  driver (Ask 7's primitives were previously unreachable from this spawner).

Since 0.8.4 (PR #912, the storage unification arc):

- **`StorageLayout` path authority** (meerkat-core/src/storage_layout.rs) —
  resolved once at bootstrap, carried through composition; the
  `storage-ambient-gate` CI gate bans ambient root resolution (`dirs::*`,
  `HOME`/`XDG`) outside the bootstrap/layout modules. Realm state roots
  resolve realm-id-first across the project-local and user-global
  candidates (`RealmConfig::resolve_locator_dual_root`): explicit root wins,
  a single existing candidate is used where it lies, both is a typed
  `RealmSplitBrain` refusal, neither goes to the surface default. Probing
  fails closed (typed `RootProbeFailed` / `RealmDirectoryCollision` /
  `ManifestUnreadable`); `DualRootResolution.candidate_roots` arms the
  cross-candidate first-start reservation
  (`ensure_realm_manifest_pin_with_candidates`) so racing first starts
  cannot mint twins.
- **`meerkat-sqlite`** — new leaf crate owning the shared SQLite mechanics
  (profiles, ledger, fences, `JsonColumnBytes`, error classification; see
  crate table). Opens are DDL-free; every database gains a `meerkat_schema`
  ledger and a sibling `<file>.mfence` fence-lock; a future-schema file
  refuses typed (`SchemaFromTheFuture`) preflight, before WAL.
- **`RealmStorageProvider` seam** (meerkat/src/storage_provider.rs) — one
  provider supplies all durable stores for a realm; store-only by design
  (mob storage stays mob-owned to avoid the facade↔mob cycle). Realm
  manifest v2 adds `provider` / `ephemeral_domains`; future formats and
  provider-pin mismatches refuse typed. Fail-closed durability: exactly one
  `DurabilityDeclaration` per required domain (sessions, runtime, schedule,
  workgraph, blobs, artifacts); an undeclared non-persistent `Durable` slot
  is a startup `DurabilityViolation`, never a silent in-memory fallback.
- **`meerkat-store-conformance`** — published per-trait conformance
  chapters any backend runs by supplying store factories; the in-repo
  stores run the same suite (meerkat-store/tests/conformance.rs).
- **`rkat storage doctor|migrate|prune`** — CLI storage verbs dispatch
  BEFORE runtime-scope resolution (no leases, no realm creation;
  `--isolated`/`--default-model` are usage errors; only `migrate --apply`
  opens a realm's persistence bundle, under the exclusive fence). Doctor
  renders the shape-stable `StorageMigrator::diagnose` seam
  (meerkat-core/src/storage_diagnostics.rs; disk impl
  meerkat-store/src/doctor.rs). Migrate is dry-run by default, fenced under
  `--apply`, refuses split-brain without `--adopt-root`, and adopts legacy
  checkpoints in bulk (`PersistentSessionService::adopt_legacy_checkpoints`).
  Prune deletes registered `*.pre-<version>-<timestamp>` /
  `*.corrupt-<timestamp>` artifacts only, aged by the name's embedded
  timestamp.
- **Machine schemas are constructed once per process** — the `machine!`
  macro emits a `LazyLock`-cached `schema_static()`; `schema()` clones the
  cache (meerkat-machine-dsl-core/src/gen_schema.rs; pointer-identity
  pinned by meerkat-machine-schema/tests/schema_construction_cache.rs).
  Previously every schedule-host tick re-parsed machine DSLs per persisted
  row — an idle busy-loop that escalated to restart availability loss.
- **Stream-inactivity watchdog** — `RetryPolicy::stream_inactivity_timeout`
  (`[retry] stream_inactivity_timeout`; default ON at 300s, `"disabled"`
  opts out). A silent provider stream aborts with the retryable
  `LlmFailureReason::StreamStalled`; stream events re-arm the window, the
  hard call timeout stays fixed at call start; applies only to
  liveness-reporting clients (`stream_activity_count`).
- **Force-cancel is legal from `Running`** — MobMachine gained wedge arms
  (`ForceCancelRunningRuntimeNotLive`, `ForceCancelRunningAlreadyRetiring`)
  that admit as idempotent no-ops with no emission; only the active
  `ForceCancelRunning` arm emits `FlowTerminalized` and authorizes the
  mechanical interrupt. A roster-unknown identity answers `MemberNotFound`
  before machine admission.
- **Checkpointer-gate wedge fix** — a cancelled per-session checkpointer
  gate now fails the SessionStore projection with a retryable error instead
  of silently skipping it, so the terminal-recovery drain re-projects the
  committed RuntimeStore snapshot on restart and the two checkpoint
  authorities reconverge (the authority-conflict error names both stamps).

## Runtime Dogma (first review lens)

Canonical doctrine: `docs/architecture/meerkat-dogma.md` (nine rules; mirrored
into the `meerkat-dogma-inquisition` skill via
`scripts/sync-meerkat-dogma-skill-docs.sh`, drift-gated). Commentary:
`docs/architecture/meerkat-dogma-commentary.md`. Public summary:
`docs/reference/machine-authority.mdx`. Historical archive (legacy rules
#1–#20; the canonical doc carries the legacy-number mapping):
`docs-internal/archive/public-docs-removed-2026-05-11/architecture/meerkat-runtime-dogma.md`.

The nine canonical rules:

1. Authority Is Singular.
2. Generated Machines Own Canonical Change.
3. Shells, Stores, and Projections Are Mechanical.
4. Truth Is Typed, Identity Is Canonical (domain handles like `job_id` / `AgentIdentity`, not raw infra IDs; `Option<T>` must not hide ownership).
5. Composability Is Feature-Owned.
6. Surfaces Are Thin Over The Shared Runtime.
7. Providers and Policy Stay Behind Their Owning Seams (tri-state inherit/disable/set; dynamic policy follows dynamic identity).
8. Terminality and Faults Are Explicit.
9. Contracts, Crates, and Generation Are Ratchets.

### Live audio/video adapter vocabulary (public noun)

Live (audio/video) channels are exposed through the caller-initiated
`live/*` surface. Capability detection still uses `ModelCapabilities.realtime`
to decide whether a model can back a live channel; channel lifecycle
is **caller-initiated** through the `live/*` JSON-RPC methods (and
their typed SDK wrappers). The previous capability-driven attachment
plane (`session/realtime_attachment_status`,
`mob/member_status.realtime_attachment_status`, `realtime/open_info`,
`RealtimeAttachmentStatus`, `MeerkatMachine::reconfigure_live_topology`
and the realtime-binding DSL state) has been removed.

Public vocabulary:

- `ModelCapabilities.realtime` — capability bit that gates whether
  `live/open` succeeds for a session.
- `live/open` — open a live channel on a session; returns a typed
  `LiveOpenResult` carrying transport bootstrap (e.g. WS URL for
  `rkat-rpc --live-ws`'s `/live/ws` listener), `WireLiveChannelCapabilities`,
  and `WireLiveContinuityMode`.
- `live/status`, `live/refresh`, `live/send_input`, `live/commit_input`,
  `live/interrupt`, `live/truncate`, `live/close` — channel lifecycle.
- The `--live-ws <addr>` flag on `rkat-rpc` enables the WebSocket
  listener. Without it the `live/*` methods are not registered (router
  arms gated on `live_ws_state.is_some()`).

Wire types live in `meerkat-contracts/src/wire/live.rs`. Adapter
internals live in `meerkat-core::live_adapter` (`LiveAdapterStatus`,
`LiveChannelCapabilities`, `LiveContinuityMode`,
`LiveTransportBootstrap`, `LiveAdapterObservation`, etc.). Provider
implementations currently sit in `meerkat-openai::live`.

The DSL realtime-binding plane and `reconfigure_live_topology`
orchestration were deleted, not renamed; the live-adapter implementation
lives in `meerkat-live`. For a deeper internal reference,
`meerkat-live/src/host.rs`, `meerkat-rpc/src/handlers/live.rs`, and
`meerkat-contracts/src/wire/live.rs` are the authoritative surface;
`docs/guides/realtime.mdx` is the user-facing Live Channels companion.

## The canonical machine catalog

The machine roster is owned by `canonical_machine_schemas()` and
`canonical_composition_schemas()` in `meerkat-machine-schema/src/catalog/mod.rs`,
with one DSL source per machine in `meerkat-machine-schema/src/catalog/dsl/`.
Do not maintain a copy of the machine list here — read the registry (and
`canonical_machine_production_owner_relations()` for per-machine production
owners; the public mirror is `docs/reference/machine-authority.mdx`).

`MeerkatMachine` and `MobMachine` are the two runtime kernels; the remaining
catalog machines (auth, approval, session document, session turn admission,
schedule/occurrence, workgraph/attention) are scoped authorities for perimeter
state. (Per-member realtime intent and the realtime-binding plane were removed —
live channels are caller-initiated via `live/*`, gated on each member's
session-level `ModelCapabilities.realtime`.)

**Primary semantic authority lives in the catalog-generated machines.** Production modules are bridge shells around catalog-owned DSL bodies and crate-local bridging types. Handwritten `*_authority.rs` helpers that still exist are adapter mechanics, projections, planners, or sealed mutators, not competing semantic owners.

Phase 1 of the machine-authority convergence is closed:

- Catalog DSL is the source for production machine bodies and generated kernels.
- `runtime_schema_parity` asserts catalog/production schema equality for all canonical machines.
- `runtime_alphabet_parity` uses typed command classification manifests; string whitelists are forbidden.
- `flow_run`, `flow_frame`, and `loop_iteration` are MobMachine-owned fail-closed projection reducers. They are support modules for `MobRun` projection shape, not canonical machines.

Detailed architecture, DSL ↔ schema ↔ kernel ↔ TLA+ flow, the field-driven design principle, signals vs inputs, and the cross-crate handle trait pattern: **load `references/machine-system.md`**.

## Identity-first mob model

Stable per-member identity is separate from per-runtime binding:

- **`AgentIdentity`** — assigned at spawn, persists across respawns and runtime-binding changes. Keys all public mob APIs (`mob/member_status`, delegate targets, wiring, etc.).
- **`AgentRuntimeId`** — per-runtime binding detail. Rotates on respawn. DSL guards keyed on `{agent_runtime_id, fence_token}` use this for binding-level rotation safety.
- **`FenceToken`** — monotonic epoch counter for runtime bindings. DSL guards enforce `fence_token` ordering.
- **`Generation`** — mob-member generation counter; increments on respawn.

When adding state or effects keyed on member identity, choose
`AgentIdentity` if the fact survives respawn (wiring preferences,
durable per-member configuration), `AgentRuntimeId` if it's
per-binding (ops registry membership, adapter ownership for a running channel).

Member lifecycle facts (kickoff phases, restore failures, revival obligations,
`Broken` terminality) are MobMachine-owned and keyed on `AgentIdentity`;
`MemberHandle::internal_turn` is the in-process member turn path.

## Realm config inheritance

A realm is a config + state namespace. Config inherits along a parent chain;
state never does. The chain authority is `RealmChain` in
`meerkat-core/src/connection.rs`.

- **`global` is the universal default head.** Reserved slug
  `GLOBAL_REALM_SLUG = "global"` (`RealmId::global()` / `RealmId::is_global()`,
  mirroring `is_env_default`). Its config doc is the single HOME-rooted
  `~/.rkat/config.toml` (`Config::global_config_path()`), not a per-workspace
  file.
- **`RealmChain::resolve(config, head)`** walks `RealmConfigSection.parent`
  edges head → parent chain → implicit `global` tail, in `[head, parent.., global?]`
  order (fully edge-determined, no map-iteration dependence). There is no flat
  sibling scan and no literal `default` realm. Fail-closed, panic-free,
  `BTreeSet`-deduped, depth-capped at `MAX_REALM_CHAIN_DEPTH = 16`; typed
  `RealmChainError` (Cycle / DepthExceeded / MissingParent / GlobalHasParent /
  ParentIsEnvDefault). Iterative (wasm stack-safe). An absent head yields a
  single-node chain that contributes nothing, then the global tail.
- **Two consumers, one chain.** (a) The connection/credential resolvers walk the
  chain lazily, feeding `materialize_connection_target` the OWNING realm's own
  `RealmConnectionSet`. (b) `EffectiveConfigReader::effective_config_over_head`
  (`config_store.rs`, via `compose_effective_config` in `config.rs`) eagerly folds
  the per-realm `Config` docs (parent-first, child-wins) into the flat config the
  agent reads. Both call the same chain authority.
- **Owning-realm credential provenance.** A resolved binding's
  `AuthBindingRef.realm` is the realm that DEFINES it, not the requesting realm,
  because materialize is fed the owner's own set (`realm.realm_id == owner` by
  construction). This keeps the strict equality at
  `meerkat-llm-core/.../registry.rs` (`auth_binding.realm != realm.realm_id`) a
  real invariant with no relaxation, and `RealmConnectionSet`/`AuthBindingRef`
  gain no field — zero wire/schema churn. Env/InlineSecret/Command material is
  realm-agnostic (provenance only bites for the realm-namespaced TokenKey/LeaseKey).
- **Reads inherit, writes are strict-owner.** `resolve_write_owner` +
  `WriteOwnerError` (`connection.rs`) reject writing into an inherited realm's
  doc from a child: `auth login` / config `set`/`patch` go to the realm that owns
  the binding. `ConfigStore.get` returns the RAW head doc (read/write split) so an
  inherited entry is never flattened into a child; only the agent-build path reads
  the composed `EffectiveConfigReader` view.
- **Config inherits, state never does.** Models, mcp servers, hooks, skills,
  limits, and auth bindings inherit (per the merge table in
  `compose_effective_config`); children may add/override but cannot remove
  inherited mcp/hook entries (no tombstones). Sessions, SQLite/JSONL stores, and
  event stores are realm-local (`meerkat-store` `realm_paths_in(head)`).
- **Presence-based merge (MF-4).** `Config.max_tokens` and
  `AgentConfig.max_tokens_per_turn` are `Option<u32>` (`None` = inherit; resolved
  at point-of-use via `Config::resolved_max_tokens()` /
  `AgentConfig::resolved_max_tokens_per_turn()`), so a child can override a
  non-default parent value back down to the template default — merge is
  presence-based (`is_some`), not a `!= default` heuristic.
- **All surfaces compose.** CLI, REST, RPC, and MCP-server compose the effective
  config over the head realm via `effective_config_over_head` on the build path
  (`meerkat/src/service_factory.rs` plus each surface), so inheritance is not
  CLI-only. WASM (`meerkat-web-runtime`) synthesizes its api-key binding section
  under `global`, giving a degenerate single-realm chain identical to today.
- **Migration.** Legacy `dev`-realm logins migrate to `global` on the run path
  (both the credential token and the `[realm.global]` binding section),
  no-clobber + idempotent.

Machine authority is untouched: chain invariants are enforced by plain Rust
(typed `RealmChain` newtype, private field, fallible ctor, typed error), not a new
DSL/machine domain. Key files: `meerkat-core/src/connection.rs` (`RealmChain`,
`RealmChainError`, resolvers, `resolve_write_owner`),
`meerkat-core/src/config.rs` (`compose_effective_config`, `global_config_path`),
`meerkat-core/src/config_store.rs` (`RealmConfigSource`, `EffectiveConfigReader`),
`meerkat-store/src/realm.rs` (filesystem source + `realm_paths_in`).

## Crate Ownership

| Crate | Owns | Key Trait |
|-------|------|-----------|
| `meerkat-sqlite` | Shared SQLite mechanics: named connection profiles (DDL-free opens), `meerkat_schema` migration ledger (pinned concurrent-open protocol, typed `SchemaFromTheFuture` refusal), `JsonColumnBytes`, per-operation maintenance-fence guards, error classification (rusqlite only, no meerkat deps) | — |
| `meerkat-models` | Canonical provider model catalog/capabilities data; exposes `canonical()` `ModelCatalog` (core stays provider-free) | meerkat-core |
| `meerkat-core` | Agent loop, core types, session-store contract, ALL trait contracts, DSL handle traits, `StorageLayout` path authority + realm-id-first dual-root resolution, `DurabilityClass` vocabulary, `StorageMigrator` diagnose seam | `AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`, `SessionStore`, `SessionService`, `CommsRuntime`, `HookEngine`, `OpsLifecycleRegistry`, `StorageMigrator`, `TurnStateHandle`, `CommsDrainHandle`, `ExternalToolSurfaceHandle`, `PeerCommsHandle`, `SessionAdmissionHandle`, `ModelRoutingHandle`, `AuthLeaseHandle`, `McpServerLifecycleHandle`, `PeerInteractionHandle`, `SessionContextHandle`, `SessionClaimHandle`, `InteractionStreamHandle` |
| `meerkat-store-conformance` | Published storage conformance harness: per-trait capability profiles (baseline / incremental / guarded-projection), capability-discovery, append-only media, legacy-data, blob/artifact chapters | Consumes meerkat-core contracts |
| `meerkat-contracts` | Wire types, catalogs, stable error codes, generated surface schemas, **supervisor bridge protocol (`BridgeCommand`, `BridgeReply`, `BridgePeerSpec`, `BridgeSupervisorPayload`)** | — |
| `meerkat-client` | Compatibility client shim that re-exports provider surfaces | Compatibility exports only |
| `meerkat-auth-core` | Shared auth primitives, token stores, OAuth helpers, MCP OAuth discovery/DCR/PKCE/refresh, cloud authorizers | — |
| `meerkat-providers` | Compatibility provider-runtime/auth shim surface | — |
| `meerkat-anthropic` / `meerkat-openai` / `meerkat-gemini` | Provider-specific client/runtime implementations | Implements `AgentLlmClient` via provider-specific crates |
| `meerkat-store` | Session-store implementations and adapters (SQLite, Jsonl, Memory), realm manifest v2 pinning + cross-candidate first-start reservation, disk doctor/migrate (`doctor.rs`, `migrate.rs`) | Implements `SessionStore`, `StorageMigrator` |
| `meerkat-tools` | Tool registry, builtins, shell, session-scoped task store | Implements `AgentToolDispatcher` |
| `meerkat-mcp` | MCP client, protocol transport, router mechanics (routes to `ExternalToolSurfaceHandle`; asks injected auth resolver for bearer tokens but does not own OAuth lifecycle) | — |
| `meerkat-session` | Session orchestration (Ephemeral, Persistent), turn admission slot (shell) | Implements `SessionService` |
| `meerkat-runtime` | Runtime control plane, policy engine, completion-feed wake, DSL handle impls | `RuntimeControlPlane`, `RuntimeDriver`, `MeerkatMachine` |
| `meerkat-comms` | Inter-agent messaging (inproc, TCP, UDS, Ed25519), peer identity claims, pure peer data types | Implements `CommsRuntime` |
| `meerkat-hooks` | Hook runtimes (in-process, command, HTTP) | Implements `HookEngine` |
| `meerkat-skills` | Skill loading (filesystem, git, HTTP, embedded) | Implements `SkillEngine` |
| `meerkat-memory` | Semantic memory stores and retrieval | Implements `MemoryStore` |
| `meerkat-workgraph` | Realm-scoped durable WorkGraph store, service, lifecycle policy, tool surface, host observability, and goal attention bindings | `WorkGraphStore` |
| `meerkat-mob` | Multi-agent orchestration, member provisioning, flow runtime, **identity-first binding model, supervisor bridge** | `MobSessionService`, `MobProvisioner`, `MobMemberRuntimeBridge` |
| `meerkat-mob-pack` | Mobpack archive format, signing, trust policies, validation | — |
| `meerkat-mob-mcp` | MCP/operator mob surface plus agent-facing delegation tool surface | `MobMcpState`, `AgentMobToolSurfaceFactory` |
| `meerkat-live` | Live channel host and WebSocket transport glue for `live/*` methods | `LiveAdapterHost`, `LiveProjectionSink` |
| `meerkat-schedule` | Scheduler subsystem; `Schedule::apply` / `Occurrence::apply` on domain types | `ScheduleService`, `ScheduleDriver`, `ScheduleStore` |
| `meerkat-web-runtime` | WASM browser deployment (wasm_bindgen exports) | — |
| `meerkat-machine-schema` | Rust-native machine/composition catalog DSL — the formal authority | — |
| `meerkat-machine-kernels` | Generated kernel interpreter for all machines/compositions | `GeneratedMachineKernel` |
| `meerkat-machine-codegen` | TLA+ model generation, TLC verification, drift detection | — |
| `meerkat` (facade) | `AgentFactory`, `FactoryAgentBuilder`, persistence helpers, re-exports, `RealmStorageProvider` seam + `DiskStorageProvider` + fail-closed durability enforcement | Wires everything together |

**Rule: `meerkat-core` has zero I/O dependencies.** All I/O happens in satellite crates.

For detailed crate-by-crate reference: load `references/crate_map.md`.

## Reference Navigator

Load these as needed. SKILL.md alone is intentionally minimal — everything else lives in `references/` for progressive disclosure.

- **`references/machine-system.md`** — load when touching DSL sources, catalog schemas, generated kernels, TLC verification, production bridge modules, parity gates, command classification, authority cutover, handle trait design, or any "where does this semantic state live" question. Covers the DSL → MachineSchema → kernel → TLA+ → runtime flow, the catalog/production parity ratchets, the field-driven design principle, signals vs inputs, and the `HandleDslAuthority` cross-crate pattern.
- **`references/runtime-control-plane.md`** — load when working on `MeerkatMachine`, runtime drivers, session registration, policy resolution, `RuntimeBuildMode` / `SessionRuntimeBindings`, `OpsLifecycleRegistry`, session service lifecycle, persistence pairing, detached-op wake, or test harness ownership.
- **`references/agent-construction.md`** — load when touching `AgentFactory::build_agent()`, agent builder, multimodal content types, or runtime tool scoping.
- **`references/mob-orchestration.md`** — load when working on mobs: creation, launch modes, spawn policies, delegation tools, lifecycle control, provisioning, wiring, flow/frame execution, mob persistence, or `MobActor` decomposition.
- **`references/comms-model.md`** — load when working on peer trust, inter-agent messaging, comms drain lifecycle, envelope classification, or session identity claims.
- **`references/gotchas.md`** — load as the first review lens for non-trivial changes. Regression checklist of architectural invariants that quietly re-break.
- **`references/crate_map.md`** — detailed crate-by-crate reference.

## Key files (quick index)

For comprehensive file lists, see the matching reference. This is a minimal pointer index for the most common landmarks.

- `meerkat-machine-schema/src/catalog/dsl/` — DSL sources (one per canonical machine; the roster and count are owned by `canonical_machine_schemas()`)
- `meerkat-machine-schema/src/catalog/mod.rs` — `canonical_machine_schemas()` registry
- `meerkat-machine-kernels/src/runtime.rs` — `GeneratedMachineKernel` interpreter
- `meerkat-runtime/src/meerkat_machine/` — `MeerkatMachine`, session management, dispatch paths, DSL adapter
- `meerkat-runtime/src/handles/` — runtime impls of DSL handle traits
- `meerkat-core/src/handles.rs` — DSL handle trait definitions
- `meerkat-core/src/runtime_epoch.rs` — `SessionRuntimeBindings`, `RuntimeBuildMode`
- `meerkat-core/src/storage_layout.rs` — `StorageLayout` path authority (dual-root resolution lives in `runtime_bootstrap.rs`)
- `meerkat-core/src/storage_durability.rs`, `meerkat-core/src/storage_diagnostics.rs` — `DurabilityClass`/`DurabilityDeclaration`, `StorageDiagnosis`/`StorageMigrator`
- `meerkat-sqlite/src/{profile,ledger,fence,json_column,error}.rs` — shared SQLite mechanics
- `meerkat/src/storage_provider.rs` — `RealmStorageProvider`, `DiskStorageProvider`, `enforce_fail_closed_durability`
- `meerkat-store/src/{doctor,migrate,realm}.rs` — disk diagnosis, offline migration mechanics, realm manifest v2 pinning
- `meerkat-cli/src/storage_migrate.rs` — `rkat storage migrate`/`prune` orchestration
- `meerkat-live/src/host.rs`, `meerkat-live/src/transport.rs` — live channel host and WebSocket transport
- `meerkat-rpc/src/handlers/live.rs` — `live/*` JSON-RPC handlers
- `meerkat-core/src/agent.rs`, `meerkat-core/src/agent/*.rs` — agent loop
- `meerkat-core/src/tool_execution_policy.rs` — `ToolExecutionPolicy`, `ExecutionPolicyGatedDispatcher` (list-preserving call-level tool gate)
- `meerkat-core/src/compact.rs` — `Compactor`, `CompactionCurator` (host-supplied summary producer)
- `meerkat-schedule/src/runnable.rs` — `ScheduleRunnableHost`, `HostRunnableRegistry` (host-runnable schedule targets)
- `meerkat/src/factory.rs` — `AgentFactory::build_agent()` (pipeline)
- `meerkat-session/src/{ephemeral,persistent}.rs` — session services
- `meerkat-workgraph/src/{types,store,service,tools}.rs` — WorkGraph domain model, durable stores, service policy, and agent tools
- `meerkat-mob/src/runtime/actor.rs` — `MobActor`
- `meerkat-mob/src/backend.rs`, `meerkat-mob/src/ids.rs` — identity-first binding model
- `meerkat-mob/src/runtime/supervisor_bridge.rs` — supervisor bridge transport
- `meerkat-mob/src/runtime/local_bridge.rs` — in-process MeerkatMachine bridge
- `meerkat-mob-mcp/src/agent_tools.rs` — agent-facing delegation/orchestration tools
- `meerkat-contracts/src/wire/supervisor_bridge.rs` — bridge protocol types
- `xtask/src/{effect_authority,bridge_classifier,ownership_ledger,rmat_audit,seam_inventory}.rs` — typed governance gates
- `docs/architecture/meerkat-dogma.md`, `docs/architecture/meerkat-dogma-commentary.md` — canonical dogma doctrine
- `docs/reference/machine-authority.mdx` — public machine-authority summary
- `docs/reference/build-and-ci.mdx` — public BuildBuddy/Cargo/CI guide
- `docs-internal/archive/public-docs-removed-2026-05-11/architecture/meerkat-runtime-dogma.md` — historical internal doctrine archive
- `docs-internal/archive/public-docs-removed-2026-05-11/architecture/identity-first-live-voice-proposal.md` — historical live + identity design notes
- `tests/integration/src/e2e_lanes.rs` — authoritative e2e lane catalog
- `scripts/build-backend-env`, `scripts/run-build-backend-lane`, `scripts/buildbuddy-dev` — local build backend switch and BuildBuddy facade
