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

`MemberLaunchMode` (in `crates/meerkat-mob/src/launch.rs`):

- `Fresh` — new session (default)
- `Resume { bridge_session_id, resume_from_role }` - resume the exact bridge
  session. `resume_from_role` is an optional one-request predecessor-role
  declaration for a cold durable role migration; omission keeps strict
  same-role resume.
- `Fork { source_member_id, fork_context }` - start a fresh child session from
  another member's rendered history

Resume resolves through the REQUIRED (defaultless)
`MobSessionService::load_session_for_resume`, returning the typed
`ResumeSessionLoad`: `Active` / `Revivable` (archived but machine-revivable) /
`ArchivedNotRevivable { runtime_state }` / `Absent`. Archived-not-revivable is
NOT absence: it surfaces `MobError::SessionUnavailableForResume` with a typed
`SessionResumeUnavailableReason` and classifies as
`MobFailureClass::TargetArchived`. Implementations must state this truth
directly — a composition over the legacy optional reads can never produce
`ArchivedNotRevivable`, which is why the method has no default.

`ForkContext`:

- `FullHistory` - render the full source conversation into a text context block
  prepended to the fresh child's opening input
- `LastMessages { count }` - render the last `count` source messages into that
  context block

## Spawn Policies

`SpawnMemberSpec` carries the stable identity and role plus launch mode,
tool-access policy, auto-wiring, budget, auth, prompt/tooling overrides,
continuity intent, and optional multi-host placement. In-process executable
overrides such as external tools and a compaction curator are rejected for
remote placement because they have no portable wire representation.

## Helper Convenience

- `MobHandle::spawn_helper(identity, task, opts, result_label, max_text_bytes)` - spawn, wait for the exact admitted turn, return `BoundedHelperRunOutcome` (certified bounded result + exact turn result + optional `retirement_error`), teardown
- `MobHandle::fork_helper(source_identity, identity, task, fork_context, opts, result_label, max_text_bytes)` - same but with Fork launch mode

Profile source rule: agent-internal surfaces inherit from caller config. Non-agent surfaces (REST/RPC/CLI/MCP) require explicit config source — never silent defaults.

## Agent-Facing Delegation Tools

When composed with generated operator authority, `AgentMobToolSurface`
(`crates/meerkat-mob-mcp/src/agent_tools.rs`) provides
`delegate`, `conclude_objective`, `fork_off`, `council`, `mob_create`, `mob_destroy`,
`mob_spawn_member`, `mob_retire_member`, `mob_check_member`,
`mob_list_members`, `mob_list`, `mob_wire`, and `mob_unwire`. When a realm
profile store is present it also exposes `mob_profile_create`,
`mob_profile_get`, `mob_profile_list`, `mob_profile_update`,
`mob_profile_delete`, and `mob_profile_list_sources`.

- `fork_off` requires the current session to be a durable mob member. It
  creates a real child session from an exact committed transcript prefix,
  runs one bounded task, and retains the child in the normal roster. It is
  not `fork_helper`, which renders history into a fresh helper's context and
  tears that helper down afterward.
- `council` forks existing participants at their source execution owners,
  preserving their transcript prefixes, tools, auth, realm, and filesystem
  boundaries. The forks run bounded rounds in a short-lived mob and are
  cleaned up afterward. Both tools remain subject to operator authority and
  source/scope checks; neither depends on the optional profile-store tools.

`mob_wire` / `mob_unwire` create and remove comms trust relationships between mob members (local or external peers). Reuses `MobMcpState::mob_wire()` / `mob_unwire()` state API.

`owner_bridge_session_id` and `is_implicit` on `MobDefinition` are canonical for session-scoped access control, resume lookup, and cleanup.

`destroy_session_mobs()` is the canonical archive cleanup seam. Tool building and cleanup must share the same hydrated `MobMcpState`; parallel shadow registries are a bug.

Operator capabilities are runtime-injected through `MobToolAuthorityContext`. `can_create_mobs`, `can_mutate_profiles`, and `managed_mob_scope` are the authoritative checks; ambient mob enablement alone must not resurrect operator tools on resume.

## Lifecycle Control

- `MobHandle::retire(identity)` — terminal disposal barrier for this mob
  incarnation's owned work, removing the roster anchor and runtime binding.
  Mob-owned sessions are archived through their session service: persistent
  services archive durably, while explicitly enabled ephemeral services archive
  only in memory. Adopted host-owned sessions only
  release/unregister their member runtime and binding, not the host's durable
  document. The lower `MobProvisioner::retire_member` contract reports
  `MemberSessionDisposal::Archived` or `RuntimeReleasedOnlyHostOwned`; the
  handle returns `Result<(), MobError>`. This is not a drain guarantee for
  separate event-log projection tasks.
- `force_cancel_member(id)` — cancel in-flight turn (distinct from retire)
- `respawn(id, initial_message)` — retire old bridge/runtime binding → enqueue spawn with same identity/profile/wiring/labels → new runtime incarnation/fence. Restore/respawn failure classification fans out over MobMachine-owned restore edges (`member_restore_failures: Map<AgentIdentity, String>` in the DSL); the orchestration sequencing is shell convenience, the lifecycle facts are machine-owned.
- `member_status(id)` returns `MobMemberSnapshot`. Its current public fields
  are `status`, `output_preview`, `error`, `tokens_used`, `is_final`,
  `current_session_id`, `peer_connectivity`, `kickoff`, `external_member`,
  `resolved_capabilities`, `progress`, `placement`, `control_reachability`,
  `comms_reachability`, `last_seen_ms`, `freshness_reason`,
  `lifecycle_capabilities`, and `non_portable_disabled`. Binding atoms
  (`agent_identity`, `agent_runtime_id`, `fence_token`, and
  `current_bridge_session_id`) are `pub(crate)` plus `#[serde(skip)]` -
  bridge-internal, not app-facing.
- `wait_one(id)`, `wait_all(ids)`, `collect_completed()`
- `MobHandle::member(identity)` → `MemberHandle` — per-member handle. `MemberHandle::internal_turn(content)` is the in-process direct turn write (no peer comms, no handling-mode selection); it is distinct from `mob/turn_start` (RPC turn with overrides via the bridge session) and `mob/member_send` (peer delivery over comms into the member's inbox). Keep the three delivery paths separate.

**Machine-authorized revival**: post-discard member revival is MobMachine-owned, not a shell retry loop. Live-session discard creates a `member_revival_pending: Set<AgentIdentity>` obligation; revival flows observe → classify → realize: the shell reports a `MemberLiveMaterializationClassified { observation, verdict, reason }` input, the machine issues the verdict (`MemberRevivalVerdictKind`), and the shell realizes only machine-authorized revivals (`ResolveMemberRevivalSucceeded` / `ResolveMemberRevivalFailed`). `Broken` is a terminal lifecycle classification: its `not_broken` guard refuses any further revival/retry, and `member()` access to a Broken member surfaces a typed `MobError::MemberRestoreFailed` instead of handing out a dead handle.

## Provisioning

`MobActor` → `MobProvisioner` → session-backed provisioner → `session_service.create_session(req)`. Members are real sessions.

`SessionBackend::runtime_session_state()` is the canonical owner of session registration + runtime-loop attachment for mob members. Autonomous readiness helpers should only do autonomous-specific work (drain startup, capability checks), not duplicate registration.

**RuntimeBinding**: `SpawnMemberSpec.binding: Option<RuntimeBinding>` separates
backend kind (definition level) from concrete runtime binding (spawn level).
`Session` is a controlling-host session. `HostMaterialized { host }` is still a
managed Meerkat session, but MobMachine placement sends materialization to a
bound member host. `External` is only for an unmanaged or preconstructed
external process and carries its address, Ed25519 public identity, and typed
supervisor `bootstrap_token`; the real peer id is derived from the public key.
The provisioner dispatches on required `ProvisionMemberRequest.binding`.
`resolve_binding()` translates `SpawnMemberSpec.binding` or legacy `backend`
into that concrete binding and rejects bare `External` without details.

**External member identity**: `BackendPeer.peer_id` is the real external process comms key, not the placeholder session's key. The bridge session still exists for lifecycle transport. `trusted_peer_spec()` uses the bridge key (from `comms.public_key()`, passed as `fallback_peer_id`) for transport trust, keeping identity and transport separate.

Managed placement must not be described as External binding. The controlling
host retains roster, placement, grant, and teardown authority while the bound
member host owns the placed session's runtime. Member-live control is
WebSocket-only for both local and placed members; controller-level `session/*`
live methods do not proxy a placed member, and the placed-member surface does
not currently offer WebRTC.

**Member kickoff state** lives in MobMachine DSL as a multi-field decomposition keyed on `AgentIdentity`: `member_kickoff_pending` / `member_kickoff_starting` / `member_kickoff_callback_pending` / `member_kickoff_started` / `member_kickoff_failed` / `member_kickoff_cancelled` (Sets) plus `member_kickoff_error: Map<AgentIdentity, String>`. `MobActor` drives `KickoffMarkPending`/`KickoffMarkStarting`/`KickoffResolveStarted`/`KickoffCancelRequested` transitions directly via `self.dsl_authority.apply(mob_dsl::MobMachineInput::…)` (in-crate access; no cross-crate handle needed). Persistence flows through `MobEventKind::MemberKickoffUpdated` emitted from the DSL effect handler.

## Wiring

Definition has `WiringRules` with `role_wiring: [{a, b}]`. At spawn time, `MobActor` computes wiring targets and establishes bidirectional trust via comms.

`delegate` auto-wiring is capability-based, not a promise. Report actual wired/not-wired results and never claim bidirectional comms unless both trust edges were established.

Recipient-side trust is a machine-owned obligation, not fire-and-forget: MobMachine tracks `pending_recipient_trust: Set<PeerId>` so an unacknowledged recipient trust edge stays an explicit pending fact until resolved. Trust entries themselves are `PeerId`-keyed via `TrustStore` (crates/meerkat-comms/src/trust.rs); duplicate `PeerId` inserts are structurally rejected.

`mob_wire` / `mob_unwire` agent tools: create and remove peer-to-peer comms trust between mob members. For local members (both in roster), wiring is bidirectional. For external members, the supervisor bridge binds against the external runtime using the typed bootstrap token and signed comms identity. A remote `rkat run --comms-listen-tcp ... --comms-binding-out <path>` process can now supply the binding directly; `rkat-rpc --tcp` remains JSON-RPC host transport and is not the peer/comms listener.

## Flows

- Flat DAG steps still exist, but `FlowSpec.root: FrameSpec` and `RepeatUntilSpec` enable frame-based execution.
- Frame execution is owned by MobMachine DSL: frame-local state, loop iteration lifecycle, scheduler grants (`GrantNodeSlot`, `GrantBodyFrameStart`), frame-step projection, and terminalization all live in the MobMachine DSL as transitions. `FlowEngine` is the thin execution shell.
- `flow_run`, `flow_frame`, and `loop_iteration` under `crates/meerkat-mob/src/run/` are MobMachine-owned fail-closed projection reducers. They define the persisted `MobRun` shape and reducer vocabulary, but every semantic reducer command must be authorized by a MobMachine input/effect path.
- `FlowEngine::execute_step_with_all_guards()` is the single canonical step path used by both flat-step execution and the frame adapter. `FlowTurnExecutorAdapter` is intentionally thin.
- Recovery lives in `crates/meerkat-mob/src/runtime/recovery.rs`: repairs ready-frame / pending-body-frame drift when possible and returns typed incompatibility for pre-v2 active runs when not.
- Gotcha: never append step/failure/event projections from a parallel executor path if the DSL already owns that projection.

## Actor Decomposition

`MobActor` is the serialized shell around one generated
`MobMachineAuthority`. Current supporting components have narrower mechanical
roles:

- `RosterAuthority` - actor-owned event-backed roster projection; membership
  and lifecycle meaning stay in MobMachine
- `FlowEngine` - thin flow execution shell over MobMachine-authorized run and
  step transitions
- `FlowFrameEngine` and `FlowFrameKernel` - scheduler-backed frame execution
  plus an authority-validating store mutation adapter
- `MobTopologyService` - pure evaluator for declarative topology rules; it
  owns no runtime state
- `SpawnPolicyService` - dynamic callback observation source whose resolution
  is submitted back to MobMachine before auto-provisioning
- `MobOpsAdapter` - runtime-adapter capability plumbing to
  `OpsLifecycleRegistry`, not lifecycle authority

Mob no longer owns a separate task-board service. Durable cross-agent
commitments live in WorkGraph; mob-owned services stay focused on orchestration,
member lifecycle, flow execution, wiring, and runtime bridges.

The `*_authority.rs` modules under `crates/meerkat-mob/src/runtime/` are bounded
shell helpers, projections, or sealed mutation adapters, not competing state
machines. Canonical transition match tables live in MobMachine DSL.

## Key files

- `crates/meerkat-mob/src/definition.rs` — `MobDefinition`, `FrameSpec`, `RepeatUntilSpec`, `owner_bridge_session_id`, `is_implicit`
- `crates/meerkat-mob/src/build.rs` — mob profile → `AgentBuildConfig`, operator capability gating
- `crates/meerkat-mob/src/launch.rs` — `MemberLaunchMode`, `ForkContext`
- `crates/meerkat-mob/src/storage.rs` — `MobStorage`, SQLite/custom storage seams
- `crates/meerkat-mob/src/backend.rs` — `MobBackendKind`, `RuntimeBinding`
- `crates/meerkat-mob/src/runtime/handle.rs` — `MobHandle`, `SpawnMemberSpec`, `MobMemberSnapshot`
- `crates/meerkat-mob/src/runtime/actor.rs` — `MobActor` (spawn, wire, flow, kickoff)
- `crates/meerkat-mob/src/runtime/flow.rs` — canonical step execution path
- `crates/meerkat-mob/src/runtime/flow_frame_engine.rs` — frame runtime executor
- `crates/meerkat-mob/src/runtime/recovery.rs` — frame/loop recovery and incompatibility checks
- `crates/meerkat-mob/src/runtime/tools.rs` — operator mob tool surface
- `crates/meerkat-mob-mcp/src/agent_tools.rs` — agent-facing delegation/orchestration tool surface
- `crates/meerkat-mob-pack/src/lib.rs` — mobpack archive format, signing, trust
- `crates/meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` — MobMachine DSL (source of truth)
