---
name: meerkat-architecture
description: "Internal architecture guide for the Meerkat agent platform. This skill should be used when understanding crate ownership, trait contracts, the agent construction pipeline, session service lifecycle, runtime control plane, mob orchestration internals, machine authority, comms wiring, or making cross-cutting architectural changes. Oriented toward AI agents and developers working on meerkat internals, not end users."
---

# Meerkat Internal Architecture

Meerkat is a library-first agent runtime. The execution pipeline is shared across all surfaces. Understanding crate ownership, trait contracts, the runtime control plane, and the agent construction pipeline is essential for making changes that don't break architectural invariants.

## Core Principles

1. **Infrastructure, not application** — the agent loop is a composable primitive with no opinions about prompts, tools, or output.
2. **Trait contracts own the architecture** — `meerkat-core` defines contracts; implementations live in satellite crates.
3. **Surfaces are skins, not authorities** — CLI, REST, RPC, MCP, WASM route through shared substrate and factory seams, but runtime-backed surfaces own runtime semantics.
4. **Composition over configuration** — optional components are `Option<Arc<dyn Trait>>`, not feature-flagged defaults.
5. **Runtime conforms to machines** — the runtime must follow the verified machine model, not the other way around.
6. **Mob is the only multi-agent runtime path** — there is no separate sub-agent substrate. User-facing "delegate"/"sub-agent" flows still compile to mob members, often inside session-owned implicit mobs.
7. **Override-first resource injection** — `AgentBuildConfig` overrides take precedence over factory/config/filesystem resolution.
8. **Seams are formal too** — async owner handoffs, wait barriers, and surfaced terminal classes are part of the architecture and should be modeled/protocolized, not left to shell convention.

## Runtime Dogma

Use this as the first review lens for any cross-cutting design or cleanup. The full doctrine lives in:

- `docs/architecture/meerkat-runtime-dogma.md`

The short version:

1. **One semantic fact, one owner** — if a fact matters semantically, it must have exactly one canonical owner.
2. **Machines own semantics** — lifecycle, routing truth, barrier membership, terminal class, and similar facts belong in machines/compositions/protocols, not helper code.
3. **Shell owns mechanics, not meaning** — shell may execute IO, hold handles, and rebuild projections; it may not invent lifecycle truth or terminal meaning.
4. **One semantic condition, one terminal path** — loop-top vs in-call detection must not create divergent terminalization.
5. **Typed truth, never string folklore** — no JSON `"kind"` classifiers or error-text parsing where typed reasons are required.
6. **App-facing APIs expose domain handles** — public control nouns should be `job_id`, `MemberRef`, `member_id`, etc., not raw infra IDs.
7. **Raw infra identity must be canonical** — if raw operation identity escapes, it must resolve in the real owning registry.
8. **`Option` must not hide ownership uncertainty** — if owner context is required, require it in the type.
9. **Inherit / disable / set are different facts** — when all three meanings matter, use a tri-state override type.
10. **Dynamic policy follows dynamic identity** — if model/provider/session identity can change at runtime, policy derived from it must be recomputed at that seam.
11. **Derived projections are rebuildable, never authoritative** — if stale projection state can affect semantics, it is shadow truth.
12. **Surfaces are skins, not authorities** — CLI/REST/RPC/MCP/SDK/WASM may adapt transport; they must not own semantics.

When reviewing a proposal, ask first:

- What semantic facts are introduced or changed?
- Who is the one canonical owner of each fact?
- Is any shell/helper/cache still carrying parallel truth?
- Are app-facing APIs exposing domain handles rather than infra identity?
- If raw infra identity escapes, is it definitely canonical?
- If policy depends on dynamic identity, where is recomputation defined?
- Did the change reduce ambiguity, or merely relocate it?

## Crate Ownership

| Crate | Owns | Key Trait |
|-------|------|-----------|
| `meerkat-models` | Model catalog, provider profiles, parameter schemas (leaf crate, no meerkat deps) | — |
| `meerkat-core` | Agent loop, core types, retry/state machines, session-store contract, ALL trait contracts | `AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`, `SessionStore`, `SessionService`, `CommsRuntime`, `HookEngine`, `OpsLifecycleRegistry`, `MobToolsFactory` |
| `meerkat-contracts` | Wire types, catalogs, stable error codes, generated surface schemas | — |
| `meerkat-client` | LLM providers (Anthropic, OpenAI, Gemini) | Implements `AgentLlmClient` |
| `meerkat-store` | Session-store implementations and adapters (SQLite, Jsonl, Memory, Redb) | Implements `SessionStore` |
| `meerkat-tools` | Tool registry, builtins, shell, utility helpers, session-scoped task store (`SqliteTaskStore`) | Implements `AgentToolDispatcher` |
| `meerkat-mcp` | MCP client and protocol transport integration | — |
| `meerkat-session` | Session orchestration (Ephemeral, Persistent) | Implements `SessionService` |
| `meerkat-runtime` | Runtime control plane, input lifecycle, policy engine, detached wake, peer handling-mode validation | `RuntimeControlPlane`, `RuntimeDriver`, `RuntimeSessionAdapter` |
| `meerkat-comms` | Inter-agent messaging (inproc, TCP, UDS, Ed25519), peer identity claims | Implements `CommsRuntime` |
| `meerkat-hooks` | Hook runtimes (in-process, command, HTTP) | Implements `HookEngine` |
| `meerkat-skills` | Skill loading (filesystem, git, HTTP, embedded) | Implements `SkillEngine` |
| `meerkat-memory` | Semantic memory stores and retrieval | Implements `MemoryStore` |
| `meerkat-mob` | Multi-agent orchestration, member provisioning, flow runtime, frame/loop execution | `MobSessionService`, `MobProvisioner` |
| `meerkat-mob-pack` | Mobpack archive format, signing, trust policies, validation | — |
| `meerkat-mob-mcp` | MCP/operator mob surface plus agent-facing delegation tool surface | `MobMcpState`, `AgentMobToolSurfaceFactory` |
| `meerkat-web-runtime` | WASM browser deployment (wasm_bindgen exports) | — |
| `meerkat-machine-schema` | Rust-native machine/composition catalog plus seam/handoff protocol metadata — the formal authority | — |
| `meerkat-machine-kernels` | Generated kernel interpreter for all machines/compositions | `GeneratedMachineKernel` |
| `meerkat-machine-codegen` | TLA+ model generation, TLC verification, drift detection | — |
| `meerkat` (facade) | AgentFactory, FactoryAgentBuilder, build_persistent_service, build_ephemeral_service, persistence helpers, re-exports | Wires everything together |

**Rule: meerkat-core has zero I/O dependencies.** All I/O happens in satellite crates.

## Machine Authority

The system is formally modeled as canonical machines plus closed-world compositions, and is being extended with explicit seam/handoff protocols for async owner boundaries.

**Design rule:** centralized `meerkat-machine-kernels` is the enforced layout. Owner crates do NOT have `machines/mod.rs` re-export modules. The xtask verification explicitly rejects parallel owner modules.

**Machines:** InputLifecycleMachine, RuntimeControlMachine, RuntimeIngressMachine, OpsLifecycleMachine, PeerCommsMachine, ExternalToolSurfaceMachine, TurnExecutionMachine, MobLifecycleMachine, FlowRunMachine, FlowFrameMachine, LoopIterationMachine, MobOrchestratorMachine, plus seam-focused machines such as CommsDrainLifecycleMachine where an async boundary has its own lifecycle truth.

**Verification:** `cargo xtask machine-codegen --all` (regenerate), `cargo xtask machine-check-drift --all` (detect stale artifacts), `cargo xtask machine-verify --all` (TLC model checking + owner tests).

**Post-0.5.0 flow note:** repeat-until / frame execution is no longer shell-owned glue. `FlowFrameMachine`, `LoopIterationMachine`, and the generated `flow_frame_loop` composition own frame-body handoff, scheduler grants, and loop evaluation routing. Frame-step outcomes must route back through `FlowRunMachine`; do not mutate run truth directly from executor code.

### Seam closure model

Meerkat does not solve composition correctness by collapsing everything into one giant machine. Instead it:

- reuses the existing composition actor model
- extends effect-disposition rules with handoff protocol metadata
- treats semantically relevant owner handoffs as outstanding obligations
- generates helper APIs so owner feedback is correlation-safe and not ad hoc shell code
- derives surfaced terminal classification from machine truth

Use this mental model when reviewing changes:

- single-machine mutators protect local truth
- compositions protect routed truth
- seam protocols protect owner-realized async truth

Use the dogma above to reject designs that still leave semantic truth in:

- caches
- helper wrappers
- side maps
- broad surface handlers
- provider-name heuristics
- handwritten terminal mapping branches

## Agent Construction Pipeline

`AgentFactory::build_agent()` is the single entry point for ALL surfaces. Key steps:

1. Validate keep_alive
2. Resolve provider (infer from model or explicit)
3. Create LLM client (override > factory credentials > config)
4. Create LLM adapter (with event channel and event tap)
5. Resolve max_tokens
6. Build skill engine, tool dispatcher, and any session-scoped task store
7. Create session store (contract lives in `meerkat-core`; impls live in `meerkat-store`)
8. Compose tools with comms, then optionally late-bind mob tools via `MobToolsFactory`
9. Resolve hooks (override > filesystem layered config)
10. Build system prompt + AgentBuilder + wire memory/compactor/skill-engine/ops-lifecycle/event-tap/checkpointer
11. Build agent, set `SessionMetadata` (persist override intent, not flattened booleans)

**Precedence at every step:** `build_config override > factory field > config resolution > default`

**Dynamic tooling gotcha:** if a child dispatcher can change between turns (callback tools, agent mob tools), compose with `DynamicToolComposite` rather than a gateway that snapshots tool definitions once at construction.

## Agent Loop State Machine

`CallingLlm` → `WaitingForOps` → `DrainingEvents` → `Completed`

With branches: `ErrorRecovery`, `Cancelling`

**WaitingForOps is real, but the important truth is the barrier membership.** The machine must own which async operations are barriers for the active run. Raw operation IDs are no longer enough on their own; the architecture is moving toward typed async-op references and explicit wait policy (`Barrier` vs `Detached`) so long-lived background work cannot accidentally block the turn forever.

## Runtime Control Plane

`RuntimeSessionAdapter` implements the `RuntimeControlPlane` trait with per-session `RuntimeDriver` instances (ephemeral or persistent).

**Runtime-backed build seam:** product/runtime-backed surfaces should obtain
`SessionRuntimeBindings` from `RuntimeSessionAdapter::prepare_bindings(session_id)`
and pass them through `SessionBuildOptions.runtime_build_mode =
RuntimeBuildMode::SessionOwned(bindings)`. Standalone/testing/embedded paths
should opt into `RuntimeBuildMode::StandaloneEphemeral` explicitly.

**Ownership split:**
- `PersistentRuntimeDriver::recover()` owns input/runtime/control recovery
- `RuntimeSessionAdapter` owns session-entry runtime recovery:
  `ops_lifecycle`, `epoch_id`, and shared cursor state
- `SessionRuntimeBindings` are the epoch-local witness for that ownership

**Key operations:**
- `ingest` — admit an input through policy resolution
- `retire` — graceful drain (process queue, reject new input)
- `respawn` — helper convenience: retire old session, spawn fresh with same identity/spec/wiring, new session ID (not machine-owned)
- `reset` — abandon pending, return to Idle
- `recover` — replay from store for crash recovery
- `destroy` — terminal state, no recovery
- `runtime_state` — query current state (Initializing/Idle/Running/Recovering/Retired/Stopped/Destroyed)

**Policy engine:** `DefaultPolicyTable` resolves `PolicyDecision` per input kind × runtime state. 9 input kinds (prompt, peer_message, peer_request, peer_response_progress, peer_response_terminal, flow_step, external_event, continuation, operation) × 2 states (idle, running). Each cell specifies ApplyMode, WakeMode, QueueMode, ConsumePoint, InterruptPolicy, DrainPolicy, RoutingDisposition.

**Peer handling_mode override:** `PeerInput` with `Message`, `Request`, or no convention may carry an explicit `handling_mode` (`Queue` or `Steer`) that overrides kind-based policy defaults. `ResponseProgress` and `ResponseTerminal` MUST NOT carry `handling_mode` — enforced by `validate_peer_handling_mode` at runtime admission. Built-in comms bridges default to `None` (kind-based policy).

**Silent intent override:** If an incoming peer intent matches the session's `silent_comms_intents` list, the policy is overridden to `ApplyMode::Ignore`, `WakeMode::None` — no LLM turn triggered. This is canonical runtime-owned behavior.

**Keep-alive drain ownership:** The runtime owns the comms drain lifecycle via `CommsDrainLifecycleAuthority`. `maybe_spawn_comms_drain` spawns on `keep_alive=true` and aborts on `keep_alive=false`. The direct session-service path (substrate) does not support keep-alive — only runtime-backed surfaces can.

**Detached-op wake:** idle keep-alive wake from background shell completions is runtime-owned. `DetachedWakeState` (`pending`, `signaled`, `notify`) sits beside the runtime loop; terminal detached ops fire `notify`, and the runtime injects `ContinuationInput::detached_background_op_completed()`. Do not spawn surface-local waker tasks or side channels for this.

**Respawn semantics:** Helper convenience (not a machine-owned primitive). Same member identity (MeerkatId), spec, and peer wiring — **new session ID**. Old session archived. Used for "agent is confused, restart it" recovery.

## OpsLifecycleRegistry

Trait in `meerkat-core/src/ops_lifecycle.rs`, concrete impl (`RuntimeOpsLifecycleRegistry`) in `meerkat-runtime/src/ops_lifecycle.rs`.

**Capabilities:** register/complete/fail/cancel/retire operations, `wait_all`, `collect_completed`, bounded completed-operation retention (FIFO eviction, default 256), multi-listener completion observation, peer info in snapshots, wall-clock timestamps (`created_at_ms`, `completed_at_ms`, `elapsed_ms` from SystemTime anchor), per-parent concurrency enforcement (`max_concurrent`), and detached-op wake signals for keep-alive runtimes.

Important current boundary:

- `OpsLifecycleRegistry` is still the authority-backed source of async operation truth
- tranche 1 barrier fixes stay inside `TurnExecution`
- tranche 2 is where barrier satisfaction gets reconciled end-to-end with authority truth instead of shell `wait_all` folklore
- active turns surface detached completions through typed `collect_completed`/`drain_completed` projection, not legacy event channels
- idle keep-alive sessions wake through the runtime-owned `DetachedWakeState` path above

## Session Service

All surfaces route through `SessionService` for the full lifecycle, but runtime-backed surfaces are the canonical product path:

```
CreateSessionRequest → SessionService::create_session() → RunResult
  └── SessionAgentBuilder::build_agent() → SessionAgent
      └── AgentFactory::build_agent() → DynAgent
```

Two implementations:
- `EphemeralSessionService<B>` — in-memory substrate (WASM, testing, embedded Queue-only use)
- `PersistentSessionService<B>` — durable substrate for runtime-backed product surfaces (CLI, RPC, REST, MCP; typically backed by sqlite or redb through `PersistenceBundle`)

`FactoryAgentBuilder` bridges `AgentFactory` into `SessionAgentBuilder`.

**Current usage rule:** if you are reviewing or designing a runtime-backed
surface, look for `prepare_bindings()` and `RuntimeBuildMode::SessionOwned(...)`.
If the code instead hand-rolls `register_session()` + registry extraction or
leans on implicit standalone fallback, treat that as architectural drift.

**Resume metadata:** `SessionTooling` is tri-state via `ToolCategoryOverride` (`Inherit`, `Enable`, `Disable`). Persist caller intent with `from_override()`, not resolved booleans, or resumed sessions will freeze tool availability at the capabilities of the build that created them.

**Store seam:** `SessionStore` now lives in `meerkat-core`; `meerkat-store` is the default implementation crate plus adapters. Custom backends should depend on the contract, not the impl crate.

## Persistence Pairing

Persistent realm opening is backend-owned in the `meerkat` facade through `PersistenceBundle`.

- Surfaces open a realm bundle, not a raw session store plus ad hoc runtime companion.
- The bundle carries the paired `SessionStore`, optional `RuntimeStore`, matching `RuntimeSessionAdapter`, and when enabled the blob/task companions for that realm.
- SQLite is now the default persistent realm backend when compiled; redb remains explicit for session realms, but mob persistence itself is SQLite/WAL-backed.
- `meerkat-tools::builtin::SqliteTaskStore` persists session-scoped tasks in the shared SQLite realm when `session-store` is enabled.
- `meerkat-mob::MobStorage::persistent()` uses `SqliteMobStores` (WAL, per-operation connections), and `MobStorage::custom()` is the extension seam for user-provided mob stores.
- Do not reintroduce long-lived exclusive handles to mob persistence paths; the redb-backed mob store was removed specifically because lingering handles kept file locks across in-process restarts.

## Mob Orchestration

**There is no separate sub-agent runtime.** All multi-agent work routes through mobs. The new `delegate` UX is still mob-backed: architecturally, a "sub-agent" is a mob member, usually inside an implicit session-owned mob.

```
MobBuilder::new(definition, storage)
  .with_session_service(service)
  .allow_ephemeral_sessions(true)
  .create() → MobHandle
```

`MobHandle` is clone-cheap (Arc-shared state). Sends commands to `MobActor` via channel.

### Definition-Only Creation

- Prefabs are gone. `MobDefinition` is the only creation input across CLI/REST/RPC/MCP/SDKs.
- If you want a shortcut, generate an explicit definition. Do not reintroduce string-keyed prefab enums or hidden Rust-side skill/model injection.

### Member Launch Modes

`MemberLaunchMode` (in `meerkat-mob/src/launch.rs`):
- `Fresh` — new session (default)
- `Resume { session_id }` — resume existing session
- `Fork { source_member_id, fork_context }` — fork from another member's history

`ForkContext`:
- `FullHistory` — `Session::fork()` (O(1) CoW)
- `LastMessages { count }` — `source.last_n(n).to_vec()` (shallow copy)

### Spawn Policies

`SpawnMemberSpec` carries: `launch_mode`, `tool_access_policy` (inherit/allow-list/deny-list), `budget_split_policy` (Equal/Proportional/Remaining/Fixed), `auto_wire_parent` (bool).

Budget splitting: orchestrator reads remaining budget, computes share per policy, decrements own budget, seeds child `BudgetLimits`.

### Helper Convenience

- `MobHandle::spawn_helper(prompt, opts)` — synthesize ephemeral mob, spawn, wait, return result, teardown
- `MobHandle::fork_helper(source_id, prompt, opts)` — same but with Fork launch mode

Profile source rule: agent-internal surfaces inherit from caller config. Non-agent surfaces (REST/RPC/CLI/MCP) require explicit config source — never silent defaults.

### Agent-Facing Delegation Tools

- `AgentMobToolSurface` (`meerkat-mob-mcp/src/agent_tools.rs`) provides `delegate`, `mob_create`, `mob_destroy`, `mob_spawn_member`, `mob_retire_member`, `mob_check_member`, `mob_list_members`, and `mob_list`.
- `owner_session_id` and `is_implicit` on `MobDefinition` are canonical for session-scoped access control, resume lookup, and cleanup.
- `destroy_session_mobs()` is the canonical archive cleanup seam. Tool building and cleanup must share the same hydrated `MobMcpState`; parallel shadow registries are a bug.
- Operator capabilities are runtime-injected. `MobToolAccessContext::OperatorCapabilitiesPresent` / `override_mob == Some(true)` is the authoritative signal; ambient mob enablement alone must not resurrect operator tools on resume.

### Lifecycle Control

- `retire_member(id)` — archive session, remove from roster
- `force_cancel_member(id)` — cancel in-flight turn (distinct from retire)
- `respawn(id, initial_message)` — helper convenience: retire old session → enqueue spawn with same identity/profile/wiring/labels → new session ID (not machine-owned)
- `member_status(id)` → `MobMemberSnapshot` (status, output, error, timestamps, tokens, is_final, peer_metadata)
- `wait_one(id)`, `wait_all(ids)`, `collect_completed()`

### Provisioning

`MobActor` → `MobProvisioner` → session-backed provisioner → `session_service.create_session(req)`. Members are real sessions.

`SessionBackend::runtime_session_state()` is the canonical owner of session registration + runtime-loop attachment for mob members. Autonomous readiness helpers should only do autonomous-specific work (drain startup, capability checks), not duplicate registration.

### Wiring

Definition has `WiringRules` with `role_wiring: [{a, b}]`. At spawn time, `MobActor` computes wiring targets and establishes bidirectional trust via comms.

`delegate` auto-wiring is capability-based, not a promise. Report actual wired/not-wired results and never claim bidirectional comms unless both trust edges were established.

### Flows

- Flat DAG steps still exist, but `FlowSpec.root: FrameSpec` and `RepeatUntilSpec` now enable frame-based execution.
- `FlowFrameEngine` drives frame execution; `FlowFrameMachine` owns frame-local state; `LoopIterationMachine` owns repeat-until iteration lifecycle; `FlowRunMachine` owns scheduler grants (`GrantNodeSlot`, `GrantBodyFrameStart`), frame-step projection, and terminalization.
- `FlowEngine::execute_step_with_all_guards()` is the single canonical step path used by both flat-step execution and the frame adapter. `FlowTurnExecutorAdapter` is intentionally thin.
- Recovery lives in `meerkat-mob/src/runtime/recovery.rs`: it repairs ready-frame / pending-body-frame drift when possible and returns typed incompatibility for pre-v2 active runs when not.
- Gotcha: never append step/failure/event projections from a parallel executor path if the run machine already owns that projection.

### Actor Decomposition

MobActor is composed of narrowly-scoped service objects:
- `MobLifecycleOwner` — state transitions (lock-free AtomicU8)
- `MobOrchestratorKernel` — coordinator binding, spawn/flow tracking
- `FlowRunKernel` — flow run creation, scheduler state, frame-step projection, terminalization
- `FlowFrameEngine` — scheduler-backed frame execution and revisit/healing
- `LoopIterationAuthority` — loop body/evaluate seam ownership
- `SpawnPolicyService` — runtime policy swap (RwLock)
- `MobOpsAdapter` — bridges to `OpsLifecycleRegistry`
- `MobTaskBoardService` — task board validation and persistence

## Multimodal Content

`ContentBlock` (meerkat-core): `Text { text }` or `Image { media_type, data, source_path }`.
`ContentInput` (meerkat-core): `Text(String)` or `Blocks(Vec<ContentBlock>)`.
`ToolOutput` (meerkat-tools): `Json(Value)` or `Blocks(Vec<ContentBlock>)`.

## Tool Scoping

`ToolScope` manages runtime tool visibility with staged-then-applied semantics:
- **External filters** — staged via `ToolScopeHandle`, applied atomically at `CallingLlm` boundary.
- **Per-turn overlays** — `TurnToolOverlay` for mob flow step-scoped restrictions.
- **Live MCP mutation** — `McpRouter` staging queue, applied at turn boundary.
- **Composition rule** — most-restrictive wins.
- **Dynamic child surfaces** — use `DynamicToolComposite` when child dispatchers can change between turns (callback tools, agent mob tools). Static tool-list caching is a regression risk.

## Comms Model

- **InprocRegistry** — process-global peer discovery.
- **CommsRuntime** — per-session, created by `AgentFactory::build_agent()` when `comms_name` is set.
- **Wiring** — bidirectional trust. Each peer has `TrustedPeerSpec`.
- **Unified trust state** — single `Arc<parking_lot::RwLock<TrustedPeers>>`.
- **Not all mob members are peers.** Wiring rules control which members can communicate.
- **Comms drain lifecycle** — background inbox consumption is a real lifecycle seam. Spawn/abort/exit feedback must return through the formal lifecycle path; turn-boundary suppression is a local projection of that truth.
- **Session identity claims** — process-global claim tables must be released on teardown/recovery (`release_session_claim`, `clear_all_session_claims`) if you manage runtimes manually. Dangling tasks can leak identity across in-process restarts.

## Post-0.5.0 Gotchas

Use this as the first regression checklist when touching post-`v0.5.0` architecture:

- Prefabs are gone. `MobDefinition` is the only creation input.
- "Delegate" / "sub-agent" UX is mob-backed and session-owned. Use canonical `owner_session_id`, `is_implicit`, and `destroy_session_mobs()` seams.
- Persist `ToolCategoryOverride` intent; never flatten `Inherit` into a resolved bool on save/resume paths.
- Background-op wake is runtime-owned via `DetachedWakeState` + `ContinuationInput`; surfaces must not spawn bespoke waker loops.
- Only actionable peer inputs may carry `handling_mode`; response progress/terminal traffic must fall back to kind-based policy.
- `FlowEngine::execute_step_with_all_guards()` is the only canonical step path; frame-step outcomes route back through `FlowRunMachine`.
- Mob persistence is SQLite/WAL-backed. Avoid lock-holding backends or split store state.
- Agent mob tools and archive cleanup must share the same hydrated `MobMcpState`; parallel shadow states are architectural bugs.

## Key Architectural Invariants

1. **Never bypass build_agent()** — all agent construction goes through this pipeline.
2. **Never import implementations in business logic** — use traits from meerkat-core.
3. **No sub-agent system** — all multi-agent work goes through mobs. No SubAgentManager, no agent_spawn/agent_fork.
4. **Runtime conforms to machines** — runtime behavior must match verified machine schemas. No owner-crate `machines/mod.rs` re-exports; centralized `meerkat-machine-kernels` is the enforced design.
5. **Shell may execute mechanics; it may not invent seam semantics** — evidence capture is allowed, but feedback mapping, barrier membership, and terminal class must be protocol-constrained or machine-derived.
6. **Errors separate mechanism from policy** — `ToolError → AgentError → SessionError`.
7. **Wire types ≠ domain types** — `meerkat-contracts` owns wire format; `meerkat-core` owns domain types.
8. **Sessions are first-class, persistence is optional** — Ephemeral and Persistent share the same trait.
9. **No backward compatibility aliases** — 0.5 is a clean cut. No serde aliases for old names.
10. **No `.unwrap()`/`.expect()`/`panic!()` in library code** — use `?` propagation or explicit error handling.
11. **No shadow semantic truth** — if a helper, cache, queue, or surface carries authoritative meaning beside the machine/composition/protocol owner, the design is wrong.
12. **No raw infra IDs as app-facing control nouns** — use domain handles publicly and keep canonical raw identity infra-only unless there is a very strong reason not to.
13. **Definition-only mob creation** — `MobDefinition` is the only creation input. Do not resurrect prefabs or hidden template injection.
14. **Persist intent, not resolved defaults** — use `ToolCategoryOverride` / `from_override()` when storing tooling policy.
15. **One canonical step path** — `execute_step_with_all_guards()` is shared by flat and frame execution; parallel executors are a regression factory.
16. **Operator authority is injected, not ambient** — mob support being enabled must not surface operator tools without runtime context.
17. **Runtime owns detached wake** — background-op completion wakeups must flow through `DetachedWakeState` + `ContinuationInput`, not surface code.

## Key Files

- `meerkat-core/src/agent.rs` — agent loop state machine
- `meerkat-core/src/agent/state.rs` — run_loop, WaitingForOps dispatch
- `meerkat-core/src/ops_lifecycle.rs` — OpsLifecycleRegistry trait
- `meerkat-core/src/session.rs` — `SessionMetadata`, `SessionTooling`, `ToolCategoryOverride`
- `meerkat-core/src/session_store.rs` — canonical `SessionStore` contract
- `meerkat/src/factory.rs` — `AgentFactory::build_agent()` (the pipeline)
- `meerkat/src/service_factory.rs` — `FactoryAgentBuilder`
- `meerkat-runtime/src/session_adapter.rs` — RuntimeSessionAdapter, RuntimeControlPlane impl
- `meerkat-runtime/src/policy_table.rs` — DefaultPolicyTable (policy matrix)
- `meerkat-runtime/src/ops_lifecycle.rs` — RuntimeOpsLifecycleRegistry
- `meerkat-runtime/src/detached_wake.rs` — idle keep-alive wake for detached background ops
- `meerkat-runtime/src/peer_handling_mode.rs` — peer `handling_mode` validation
- `meerkat-runtime/src/silent_intent.rs` — silent intent override
- `meerkat-session/src/ephemeral.rs` — EphemeralSessionService
- `meerkat-tools/src/builtin/sqlite_store.rs` — session-scoped `SqliteTaskStore`
- `meerkat-mob/src/definition.rs` — `MobDefinition`, `FrameSpec`, `RepeatUntilSpec`, `owner_session_id`, `is_implicit`
- `meerkat-mob/src/build.rs` — mob profile → `AgentBuildConfig`, operator capability gating
- `meerkat-mob/src/launch.rs` — MemberLaunchMode, ForkContext, BudgetSplitPolicy
- `meerkat-mob/src/storage.rs` — `MobStorage`, SQLite/custom storage seams
- `meerkat-mob/src/runtime/handle.rs` — MobHandle, SpawnMemberSpec, MobMemberSnapshot
- `meerkat-mob/src/runtime/actor.rs` — MobActor (spawn, wire, flow)
- `meerkat-mob/src/runtime/flow.rs` — canonical step execution path
- `meerkat-mob/src/runtime/flow_frame_engine.rs` — frame runtime executor
- `meerkat-mob/src/runtime/recovery.rs` — frame/loop recovery and incompatibility checks
- `meerkat-mob/src/runtime/tools.rs` — operator mob tool surface
- `meerkat-mob-mcp/src/agent_tools.rs` — agent-facing delegation/orchestration tool surface
- `meerkat-machine-schema/src/catalog/` — machine/composition definitions
- `meerkat-machine-kernels/src/runtime.rs` — GeneratedMachineKernel interpreter
- `meerkat-comms/src/runtime/comms_runtime.rs` — CommsRuntime

For detailed crate-by-crate reference, load: `references/crate_map.md`.
