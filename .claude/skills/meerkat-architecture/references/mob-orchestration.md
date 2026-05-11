# Mob Orchestration

Load this reference when working on mob creation, member provisioning, wiring, flow execution, frame/loop runtime, mob tools, or mob persistence.

## The single multi-agent path

There is no separate sub-agent runtime. All multi-agent work routes through mobs. The `delegate` UX is mob-backed: architecturally, a "sub-agent" is a mob member, usually inside an implicit session-owned mob.

```
MobBuilder::new(definition, storage)
  .with_session_service(service)
  .allow_ephemeral_sessions(true)
  .create() → MobHandle
```

`MobHandle` is clone-cheap (Arc-shared state). Sends commands to `MobActor` via channel.

## Definition-Only Creation

`MobDefinition` is the only creation input across CLI / REST / RPC / MCP / SDKs. No prefab enums, no hidden Rust-side skill/model injection. If a shortcut is needed, generate an explicit definition.

## Member Launch Modes

`MemberLaunchMode` (in `meerkat-mob/src/launch.rs`):

- `Fresh` — new session (default)
- `Resume { session_id }` — resume existing session
- `Fork { source_member_id, fork_context }` — fork from another member's history

`ForkContext`:

- `FullHistory` — `Session::fork()` (O(1) CoW)
- `LastMessages { count }` — `source.last_n(n).to_vec()` (shallow copy)

## Spawn Policies

`SpawnMemberSpec` carries: `launch_mode`, `tool_access_policy` (inherit/allow-list/deny-list), `budget_split_policy` (Equal/Proportional/Remaining/Fixed), `auto_wire_parent` (bool).

Budget splitting: orchestrator reads remaining budget, computes share per policy, decrements own budget, seeds child `BudgetLimits`.

## Helper Convenience

- `MobHandle::spawn_helper(prompt, opts)` — synthesize ephemeral mob, spawn, wait, return result, teardown
- `MobHandle::fork_helper(source_id, prompt, opts)` — same but with Fork launch mode

Profile source rule: agent-internal surfaces inherit from caller config. Non-agent surfaces (REST/RPC/CLI/MCP) require explicit config source — never silent defaults.

## Agent-Facing Delegation Tools

`AgentMobToolSurface` (`meerkat-mob-mcp/src/agent_tools.rs`) provides `delegate`, `mob_create`, `mob_destroy`, `mob_spawn_member`, `mob_retire_member`, `mob_check_member`, `mob_list_members`, `mob_list`, `mob_wire`, `mob_unwire`. When a realm profile store is present it also exposes `mob_profile_create`, `mob_profile_get`, `mob_profile_list`, `mob_profile_update`, `mob_profile_delete`, and `mob_profile_list_sources`.

`mob_wire` / `mob_unwire` create and remove comms trust relationships between mob members (local or external peers). Reuses `MobMcpState::mob_wire()` / `mob_unwire()` state API.

`owner_bridge_session_id` and `is_implicit` on `MobDefinition` are canonical for session-scoped access control, resume lookup, and cleanup.

`destroy_session_mobs()` is the canonical archive cleanup seam. Tool building and cleanup must share the same hydrated `MobMcpState`; parallel shadow registries are a bug.

Operator capabilities are runtime-injected through `MobToolAuthorityContext`. `can_create_mobs`, `can_mutate_profiles`, and `managed_mob_scope` are the authoritative checks; ambient mob enablement alone must not resurrect operator tools on resume.

## Lifecycle Control

- `retire_member(id)` — archive session, remove from roster
- `force_cancel_member(id)` — cancel in-flight turn (distinct from retire)
- `respawn(id, initial_message)` — helper convenience: retire old bridge/runtime binding → enqueue spawn with same identity/profile/wiring/labels → new runtime incarnation/fence (not machine-owned)
- `member_status(id)` → `MobMemberSnapshot` (status, agent_runtime_id, fence_token, output_preview, error, tokens_used, is_final, peer_connectivity, kickoff)
- `wait_one(id)`, `wait_all(ids)`, `collect_completed()`

## Provisioning

`MobActor` → `MobProvisioner` → session-backed provisioner → `session_service.create_session(req)`. Members are real sessions.

`SessionBackend::runtime_session_state()` is the canonical owner of session registration + runtime-loop attachment for mob members. Autonomous readiness helpers should only do autonomous-specific work (drain startup, capability checks), not duplicate registration.

**RuntimeBinding**: `SpawnMemberSpec.binding: Option<RuntimeBinding>` separates backend kind (definition level) from concrete runtime binding (spawn level). `RuntimeBinding::External { peer_id, address }` carries the real external process identity. The provisioner dispatches on `ProvisionMemberRequest.binding` (required `RuntimeBinding`, not optional). `resolve_binding()` in the actor translates `SpawnMemberSpec.binding` / legacy `backend` into `RuntimeBinding`, rejecting bare `External` without explicit binding.

**External member identity**: `BackendPeer.peer_id` is the real external process comms key, not the placeholder session's key. The bridge session still exists for lifecycle transport. `trusted_peer_spec()` uses the bridge key (from `comms.public_key()`, passed as `fallback_peer_id`) for transport trust, keeping identity and transport separate.

**Member kickoff state** lives in MobMachine DSL (`member_kickoff: Map<MeerkatId, KickoffPhase>`). `MobActor` drives `MarkPending`/`MarkStarting`/`ResolveOutcome`/`CancelRequested` transitions directly via `self.dsl_authority.apply(mob_dsl::MobMachineInput::…)` (in-crate access; no cross-crate handle needed). Persistence flows through `MobEventKind::MemberKickoffUpdated` emitted from the DSL effect handler.

## Wiring

Definition has `WiringRules` with `role_wiring: [{a, b}]`. At spawn time, `MobActor` computes wiring targets and establishes bidirectional trust via comms.

`delegate` auto-wiring is capability-based, not a promise. Report actual wired/not-wired results and never claim bidirectional comms unless both trust edges were established.

`mob_wire` / `mob_unwire` agent tools: create and remove peer-to-peer comms trust between mob members. For local members (both in roster), wiring is bidirectional. For external members, trust is exchanged between bridge sessions (inproc transport). Real external process wiring requires host-level relay (e.g., kennel `PeerWire` payloads).

## Flows

- Flat DAG steps still exist, but `FlowSpec.root: FrameSpec` and `RepeatUntilSpec` enable frame-based execution.
- Frame execution is owned by MobMachine DSL: frame-local state, loop iteration lifecycle, scheduler grants (`GrantNodeSlot`, `GrantBodyFrameStart`), frame-step projection, and terminalization all live in the MobMachine DSL as transitions. `FlowEngine` is the thin execution shell.
- `flow_run`, `flow_frame`, and `loop_iteration` under `meerkat-mob/src/run/` are MobMachine-owned fail-closed projection reducers. They define the persisted `MobRun` shape and reducer vocabulary, but every semantic reducer command must be authorized by a MobMachine input/effect path.
- `FlowEngine::execute_step_with_all_guards()` is the single canonical step path used by both flat-step execution and the frame adapter. `FlowTurnExecutorAdapter` is intentionally thin.
- Recovery lives in `meerkat-mob/src/runtime/recovery.rs`: repairs ready-frame / pending-body-frame drift when possible and returns typed incompatibility for pre-v2 active runs when not.
- Gotcha: never append step/failure/event projections from a parallel executor path if the DSL already owns that projection.

## Actor Decomposition

`MobActor` is composed of narrowly-scoped service objects:

- `MobLifecycleOwner` — state transitions (lock-free AtomicU8)
- `MobOrchestratorKernel` — coordinator binding, spawn/flow tracking
- `FlowRunKernel` — flow run creation, scheduler state, frame-step projection, terminalization
- `FlowFrameEngine` — scheduler-backed frame execution and revisit/healing
- `SpawnPolicyService` — runtime policy swap (RwLock)
- `MobOpsAdapter` — bridges to `OpsLifecycleRegistry`
- `MobTaskBoardService` — task board validation and persistence

Remaining `*_authority.rs` files under `meerkat-mob/src/runtime/` (roster, loop_iteration, member_lifecycle, wiring, runtime_bridge) are shell mechanics — data projections, pure planners, sealed mutators — not state machines. They never contain `fn apply(input) -> Result<Transition, Error>` match tables; that logic lives in MobMachine DSL.

## Key files

- `meerkat-mob/src/definition.rs` — `MobDefinition`, `FrameSpec`, `RepeatUntilSpec`, `owner_bridge_session_id`, `is_implicit`
- `meerkat-mob/src/build.rs` — mob profile → `AgentBuildConfig`, operator capability gating
- `meerkat-mob/src/launch.rs` — `MemberLaunchMode`, `ForkContext`, `BudgetSplitPolicy`
- `meerkat-mob/src/storage.rs` — `MobStorage`, SQLite/custom storage seams
- `meerkat-mob/src/backend.rs` — `MobBackendKind`, `RuntimeBinding`
- `meerkat-mob/src/runtime/handle.rs` — `MobHandle`, `SpawnMemberSpec`, `MobMemberSnapshot`
- `meerkat-mob/src/runtime/actor.rs` — `MobActor` (spawn, wire, flow, kickoff)
- `meerkat-mob/src/runtime/flow.rs` — canonical step execution path
- `meerkat-mob/src/runtime/flow_frame_engine.rs` — frame runtime executor
- `meerkat-mob/src/runtime/recovery.rs` — frame/loop recovery and incompatibility checks
- `meerkat-mob/src/runtime/tools.rs` — operator mob tool surface
- `meerkat-mob-mcp/src/agent_tools.rs` — agent-facing delegation/orchestration tool surface
- `meerkat-mob-pack/src/lib.rs` — mobpack archive format, signing, trust
- `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` — MobMachine DSL (source of truth)
