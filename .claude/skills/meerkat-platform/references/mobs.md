# Mobs

This is the detailed reference for Meerkat mobs across Rust SDK, CLI, MCP, REST, RPC, Python SDK, and TypeScript SDK.

## Positioning

- Mobs are an optional extension for multi-agent orchestration.
- Base Meerkat workflows remain session/turn centered.
- On CLI, primary mob UX is tool-driven through `run`/`run --resume` with `--tools full` or config `tools.mob_enabled=true`.
- Direct `rkat mob ...` is the helper/artifact operational surface. Lifecycle creation, wiring, and member management use agent `mob_*` tools or RPC `mob/*`.

## Runtime model

Core entities:

- `Mob`: persisted aggregate (definition, members, status, events).
- `Mob member`: spawned runtime participant identified by `agent_identity`.
- `Profile`: role contract (model/tools/skills posture).
- `Wiring`: peer graph edges.
- `Mob event`: append-only lifecycle records.
- `Flow run` (optional): DAG execution record with step and failure ledgers.

Lifecycle:

1. create
2. spawn
3. wire
4. turn and/or flow runs
5. stop/resume/complete
6. destroy

### Member runtime mode (current default)

- Default is `autonomous_host` when `runtime_mode` is omitted.
- `autonomous_host` members are long-lived peers; mob dispatch routes via injector/subscription path.
- `turn_driven` is explicit opt-in; mob dispatch routes via `start_turn`.

Override points:

- profile-level: `[profiles.<name>].runtime_mode`
- spawn-level: `runtime_mode` argument on spawn tool/command

## Definition model

Common sections:

- `[mob]`
- `[profiles.<name>]`
- `[skills.<name>]` (optional)
- `[wiring]`
- `[topology]` (optional)
- `[supervisor]` (optional)
- `[limits]` (optional)
- `[flows.<flow_id>]` (optional)

Important semantics:

- `orchestrator` chooses the orchestration role.
- `external_addressable` gates external turnability.
- `wiring.auto_wire_orchestrator` and `wiring.role_wiring` shape default graph edges.
- topology rules can enforce strict role-level communication policy.
- `runtime_mode` omitted means `autonomous_host` (new default).
- profile `durable = true` (0.9.5+) spawns durable workers: each member gets a durable identity a host re-attaches after a restart (`durable_identity_members()`). A mob whose member sessions do not outlive the process refuses the spawn with `NonDurableService` (`CAPABILITY_UNAVAILABLE`, data kind `mob_non_durable_service`): its session service answers `keeps_durable_member_sessions` false, which by default follows `supports_persistent_sessions`, and which `rkat run` answers false. `durable` is a profile field only (no spawn flag; an explicit Rust continuity intent wins), and a host older than 0.9.5 refuses a profile that sets it.
- a rebuilt durable worker's open run continues from its last committed round under the same run id (per-round commits: `references/sessions_runtime_surfaces_sessions.md`). `MobBuilder::with_durable_resume_gate(gate)` lets a host decide first: `DurableResumeGate::check(&DurableResumeRequest { mob_id, agent_identity, profile, session_id, run_id, bound_input_ids, attention_projection, attention_projection_unreadable })` (`attention_projection_unreadable` carries the decode error when the run's projection does not decode, and `attention_projection` is then `None`: a gate that reads the projection answers `Unavailable` or `Superseded` then, never treats the run as one without attention) answers `DurableResumeVerdict::Continue`, `Superseded { reason }` (the run is settled like a stop: bound inputs abandoned as refused resumes, run failed and closed as interrupted with its committed rounds kept) or `Unavailable { reason }` (asked again, 3 attempts of 8 s each over about 30 s, then settled like `Superseded`). The gate must be cancel-safe and idempotent and map its own failures to `Unavailable`. Only durable workers are asked; no gate = continue. The mob binds the gate when it registers a worker it provisions itself (spawn, cold restore, explicit resume, warm revival); the gate is an in-process callback of the mob's own host, so a durable worker placed on and materialized by another host resumes there without asking this host's gate, and a session another surface opens before the mob rebuilds the worker resumes unasked too: build mobs before serving other surfaces.

## Rust SDK (detailed)

Primary crates:

- `meerkat_mob` for runtime and state.
- `meerkat_mob_mcp` for mob tool dispatcher and in-memory state helper.

### Core Rust types

- `MobDefinition`
- `MobStorage` (SQLite persistent via `SqliteMobStores`, in-memory for tests, WASM and `rkat mob deploy`)
- `MobBuilder`
- `MobHandle`
- `MobSessionService`
- `MobState`
- `MobRun`, `MobRunStatus`
- `FlowRunConfig`

### `MobBuilder` API

- `MobBuilder::new(definition, storage)`
- `MobBuilder::from_mobpack(definition, packed_skills, storage)` - create from mobpack with inline skills
- `MobBuilder::for_resume(storage)`
- `.with_session_service(Arc<dyn MobSessionService>)`
- `.allow_ephemeral_sessions(bool)`
- `.notify_orchestrator_on_resume(bool)` (a notification that fails after the resume returned is recorded as an `OrchestratorResumeNotificationFailed` mob event once its input has failed terminally, after the runtime's bounded retries; one a retry delivers records nothing; the mob keeps running. The event is recorded in process: a crash before the input's terminal loses it)
- `.with_default_llm_client(client)` - override LLM client (primarily for testing)
- `.register_tool_bundle(name, dispatcher)` / `.register_tool_bundle_with_account(name, account, dispatcher)` (states the `OperationAccount` the bundle's operations act with)
- `.with_member_runtime_registration_hook(hook)` (`runtime-adapter`) - the host selects each member's destination and agent mandate. Since 0.9.4 `MemberRuntimeBuildIdentity::registration()` names the exact registration a call is for (a new member's prepared one, or a recovered member's current one): bind decisions to it, not to the session id or labels. The identity is no longer `UnwindSafe`; wrap it in `AssertUnwindSafe` to move it into `catch_unwind`
- `.with_fork_instruction_policy(policy)` (0.9.4+) - the host's `ForkInstructionPolicy` for every fork of a member with activated host instructions: `RequireExplicitActivation` (default), `ReactivateInherited` or `Refuse`. Never a tool argument or part of a definition; `MobMcpState::with_fork_instruction_policy` sets it for MCP-managed mobs
- `.create().await`
- `.resume().await`

### `MobHandle` API

- inspection: `status()`, `definition()`, `mob_id()`, `roster()`, `list_members()`, `list_all_members()`, `get_member()`, `events()`
- membership: `spawn_spec(spec)`, `spawn_many(specs)`, `retire(identity)`, `respawn(identity)`, `retire_all()`, `set_spawn_policy()` - all identity-keyed via `AgentIdentity`
- graph: `wire()`, `unwire()`
- turns: `member(id)` returns a `MemberHandle` with `send(content, handling_mode)` and `internal_turn(content)`
- lifecycle: `stop()` (returns a `MobStopReport`), `resume()`, `complete()`, `reset()`, `destroy()`, `shutdown()`, `shutdown_with_report(options)`
- flows: `list_flows()`, `run_flow()`, `run_flow_with_stream()`, `flow_status()`, `cancel_flow()`
- subscriptions: `subscribe_agent_events()`, `subscribe_all_agent_events()`, `subscribe_mob_events()`, `subscribe_mob_events_with_config()`
- admission: `close_if_idle()`, `close_admission(reason)`, `admission_closed()`, `observer()`, `acquire_owner()`, `is_public_owner()`
- delivery scope: `capture_member_delivery_scope(identity)`, `submit_work_with_mode_and_delivery_identity_bounded(&scope, ...)`, `recover_bounded_work_at_scope(&scope, ...)`, `retire_at_scope(&scope)` (0.9.4+)
- member history (0.9.4+): `member_binding_history()`, `member_binding_for_session(&session_id)`, `last_member_binding(&identity)`, `durable_identity_members()`, `identity_management(&identity)`; `read_handle()` has the same reads
- held spawns (native, `runtime-adapter`): `spawn_held_within(spec, deadline)` / `spawn_held_until(spec, deadline)`; publish the `HeldSpawn` with `activate()` or abort it with `abort()`

Admission close: every exported `MobHandle` clone shares one public-owner
token. When the last owner (or `acquire_owner()` lease) drops, the mob closes
admission as soon as it is idle; the builder's internal clones, `observer()`,
`read_handle()` and `events()` do not count as owners. A closed mob refuses
spawn, respawn, work submission, flow runs, resume, reset, peer messages,
live opens, wiring and host binds with `MobError::AdmissionClosed { mob_id,
reason }`; stop, shutdown, destroy, retirement, cancellation and reads stay
available, and admitted work is never stopped. Keep a `MobHandle` clone for
as long as the mob must keep admitting work. `close_if_idle()` answers
`Closed`, `AlreadyClosed(reason)` or `Busy(MobBusy)`. Since 0.9.4 every actor
exit closes admission first, with a typed `MobAdmissionCloseReason` (an
earlier close keeps its reason): an admitted Shutdown (`Shutdown`) or destroy
(`Destroyed`; a mob restored with an unfinished destroy starts closed), a
durability fail-stop (`DurabilityFailStop`), a lost actor task (`ActorLost`)
or a composition dispatch failure (`DispatchFailed`), besides `IdleClose`,
`LastOwnerReleased` and `StandDown`. While a Shutdown drains, a request the
close does not refuse gets the retryable `MobError::ShutdownInProgress`
(wire `SESSION_BUSY`); `stop()` returns it at once. `ActorCommandChannelClosed`
means the actor has exited or is exiting, and a command sent while it exits
fails instead of waiting. A failed Shutdown keeps admission closed for the
actor's life, with Stop, Shutdown and Destroy available. `MobError` is
`#[non_exhaustive]`.

Shutdown report: `shutdown_with_report(ShutdownOptions)`
(`ShutdownOptions::default().with_deadline(deadline)` bounds its waits;
since 0.9.5 the default `QueuedInputsOnShutdown::RequeueDurable`, which
`shutdown()` takes too, resolves per member to
`Requeue(RequeueMaxAge::DEFAULT)` (15 min) when the member's session commits
every round (persistent session service and a runtime persistence store) and
to `Stop` (0.9.4 behaviour) otherwise or when the member is already retiring;
`.with_queued_inputs(QueuedInputsOnShutdown::Requeue(max_age))` or `Stop`
applies to every member. Under Requeue the Shutdown holds run starts and
cancels no running turn: a run that commits every round is suspended at its
last committed round and continues at the next start (an autonomous member's
too, its kickoff kept open and resumed under the same objective, unless the
member is retiring or host-placed; a Stop with
`StopOptions::default().leave_runs_running()` before the Shutdown leaves that
kickoff open for the Shutdown to decide, while a plain Stop closes it; a run
with no round commits is cancelled), and durable queued prompts that are
neither directed nor bound to an objective (never a member's kickoff) are
kept to run after it; their waiters get
`BoundedTurnFailure::RequeuedAcrossStop`, or `RequeueExpired` when older than
`max_age` at that start)
returns a `MobShutdownReport` with per-member `members`
(`MemberShutdownOutcome`), `runs` and `run_starts`; `is_clean()` is true when
every member is `Unregistered`. Since 0.9.4 `UnregisterPending { stage,
settlement, .. }` (`#[non_exhaustive]`) carries a `MemberUnregisterSettlement`
that follows the member's exact runtime registration after Shutdown returns,
also after the actor exited, and holds the mob's runtime only weakly.
`try_state()` answers at once; `settle_within(deadline)` waits (dropping the
future cancels only the wait; `Pending` at the deadline, and a later call
continues), admitting the unregister itself when Shutdown's deadline passed
first. `MemberUnregisterState` is `Settled`, `Pending`, `Failed(reason)` or
`HandedOver`: terminal, a resumed mob or another caller took the
registration over in place, and nothing was unregistered or discarded. A
durability-degraded member goes through the runtime's reload-required discard
first (stage `durability_reload_discard` while the runtime refuses its
unregister until then), so it no longer stays registered until the process
exits.

Reload-required stops (since 0.9.4): Stop and Shutdown report a local member
whose registration is durability-degraded with run and run starts
`ReloadRequired` (`MemberStopRun::ReloadRequired`,
`MemberRunStarts::ReloadRequired`, both non-exhaustive; wire `stop_report`
`run` and `starts` `reload_required`), where they failed or waited on it. That
registration starts no run until a reload replaces it: Shutdown's teardown
discards it, and the `MobStop` a Stop staged holds the reloaded registration
until Resume. A Shutdown holds every member's run starts and reports the first
failure in roster order instead of stopping at it. In a multi-host mob a
remote member that needs a reload still makes a Stop fail with
`MobError::BridgeCommandRejected` (cause `internal`), and a Shutdown too when
that member is `AutonomousHost`: a failed Stop rolls back and the mob stays
Running, a failed Shutdown keeps every member; reload the member on its host,
then retry.

Stop hold: Stop holds run starts for every local member until Resume. Since
0.9.4 that includes a member reloaded while the mob is stopped, one spawned
into a stopped mob and one held again after a failed Resume; retire, Shutdown
and destroy release the hold for the members they dispose of. Since 0.9.4
`MobBuilder::resume` finishes a Stop (or Complete) that an earlier process
durably began and never finished before it returns the handle: it waits up to
5 s for the first attempt, so the handle reads Stopped (or Completed) and a
host lifts it as after a completed Stop. Before, the mob resumed as Running
and the background recovery stopped it a moment later, so a host that read
the phase right after resume never lifted it and every send failed with
`invalid state transition: Stopped -> Running`. An attempt refused or still
running after 5 s (held on a member or a placed completion) goes on in the
background; wait for its `MobStopped` if you need the phase.

Delivery scope: a host that persists effect-bearing deliveries captures the
member's `MemberDeliveryScope` (runtime incarnation, fence and session; it
serializes with a `version` field), persists it, then submits against it. A
member session that moved under a current fence is refused as
`MobError::StaleDeliveryScope` (wire `STALE_DELIVERY_SCOPE`) with zero effects
and is never retargeted: re-capture the scope. `recover_bounded_work_at_scope`
reads only the scope's session and returns a `ScopedWorkState` (`Absent` is
never retry permission). A member with no session binding has no scope
(`DeliveryScopeUnavailable`, wire `CAPABILITY_UNAVAILABLE`). Since 0.9.4 a
scope also names the mob incarnation it was captured at (version 2; a stored
version-1 scope decodes as `ScopeIncarnation::Legacy`, which only a mob whose
incarnation came from a pre-0.9.4 store honours; 0.9.3 refuses version 2). A
scope from another incarnation of the mob (destroyed and created again under
the same id) is refused first, with nothing admitted, as
`MobError::IncarnationMismatch` (`require_scope_incarnation` and the scoped
submit), and `recover_bounded_work_at_scope` answers
`ScopedRecoveryUnresolved::IncarnationNotHosted` without reading a session.
Member delivery addresses name the mob incarnation too
(`rt:mob-member:{mob}:{incarnation}:{identity}:{generation}`;
`MemberDeliveryAddress::incarnation`; `LogicalRuntimeId::for_member` refuses
an address without one, and the pre-0.9.4 `rt:member:` form is only read).
`meerkat::member_delivery_address` takes the live mob's incarnation
(`handle.incarnation()`). A continuation or detached-job delivery owed to a
member of a destroyed mob strands (`ContinuationStatus::Stranded`,
`OwnerRetired`) instead of reaching the member of the same identity in a mob
created again under that id, and a handle kept from the destroyed mob submits
nothing (`submit_continuation` and `submit_retained_continuation` answer
`OwnerRetired`). A continuation key submitted again after the mob was created
again replays its first receipt, bound to the old member's address. Rows
addressed before 0.9.4 name no incarnation and are served only by a mob
created before 0.9.4. 0.9.3 neither reads nor delivers `rt:mob-member:`
addresses. Known limitation: over in-memory mob storage every start creates
the mob again as a new incarnation, so deliveries owed to the earlier one
strand and its scopes get `IncarnationMismatch`; durable storage keeps the
incarnation across restarts.

Mob incarnation and provenance (since 0.9.4): a mob keeps its
`MobIncarnationId` across resume. Its create mints it once and stores it with
`MobCreated`; only a destroy and a create under the same id start a new
incarnation, so a mob operator grant still reaches the mob after it resumes. A
mob created before 0.9.4 gets its incarnation at its first resume by 0.9.4,
recorded with the provenance `Unrecorded`. The grant functions
(`grant_manage_mob`, `grant_spawn_profile_in_mob`,
`grant_spawn_profiles_in_mob`, `mint_mob_operator_authority_from_facts`) take
the witness `LiveMobIncarnation` (`MobHandle::live_incarnation`), never an id
decoded from input. `MobIncarnationId` serializes as a lowercase hyphenated
UUID (`Display`, `FromStr`, `Ord`). Opening a SQLite mob store migrates it to
schema 12 (the creation record), which 0.9.3 refuses as a future schema: keep
a copy of the mob stores from before the upgrade to roll back. A store whose
creation record and log disagree is not resumed
(`MobError::IncarnationInconsistent`, kind `IncarnationInconsistency`), and
one that holds a finished destroy whose files remain answers
`IncarnationDestroyed` (was `Internal`). Failure classes: `IncarnationDestroyed`
and `IncarnationMismatch` are `TargetMissing`, `IncarnationInconsistent` is
`Internal`, `ProvenanceUnavailable` is `RuntimeRejected`. `MobProvenance`
(`Member`, `Host`, `Session`, `Ambiguous`, `Unrecorded`, `OperatorSession` or
`OperatorMember`) is recorded once at create: `MobHandle::provenance`,
`MobStorage::creation_provenance` and `creation_incarnation` (no runtime
needed), `MobBuilder::with_provenance`
(default `Host`). A `MobMcpState` create for a caller session (the agent mob
tools, council seating) records that session's provenance from every hosted
roster and is refused with `ProvenanceUnavailable`, creating nothing, while
that lookup cannot be complete: before the mob set is restored (a host that
inserts handles itself calls `declare_mob_set_restored`), while a handle is
being inserted, or when a held child mob's stored roster cannot be read.
Insert a mob's handle before its members run turns. Lookups:
`MobHandle::member_of_session`, `MobStorage::stored_member_of_session` and
`stored_members_by_session`, `MobMcpState::session_member_matches`.

Creator grant after a restart (since 0.9.4): a session's persisted mob
operator authority binds no mob, so the manage grant a create returned was
gone after a restart. The agent mob tools now re-derive it from the mob's
creation record, for the incarnation the mob runs at, while the session may
still create mobs, and only for a create the session made itself with its
`mob_create`: for `MobProvenance::OperatorSession` naming that session, and
for `OperatorMember` only while the session is still the recorded member's
bridge session at the recorded generation in the recorded parent incarnation.
A mob a host created for the session
(`mob_create_definition_with_owner_bridge_session`, recorded as `Session` or
`Member`) is owned by the session but gets no grant. `Host`, `Ambiguous` and
`Unrecorded` records, other sessions, later generations and a session whose
operator removed its create capability get no grant either. Known limitation: a
creating member that was reset to its next generation over the same session no
longer manages the mob after a restart, because the record names its earlier
generation; before the restart the session keeps the grant its create
returned, and an operator grant over the mob restores manage.

Unresumable stores at restore (since 0.9.4): one persisted mob whose resume is
refused no longer fails `MobMcpState`'s whole restore (0.9.3 then failed every
mob verb and brought no mob back). That mob is left unrestored with its store
kept as found (`MobMcpState::unrestored_mobs`, not in `mob_list`), a lookup or
create of its id answers its refusal (`IncarnationInconsistent` and
`IncarnationDestroyed` keep their variants, any other refusal is `Internal`
with its message), and the other mobs restore. Its stored roster still counts
for provenance, so a create by a session it names is refused with
`ProvenanceUnavailable`. An empty store whose creation record is not finalized
is kept the same way (`IncarnationInconsistent`, `RecordWithoutCreation`;
`MobStorage::unfinalized_creation_mob` reads it). A transport or persistence
fault, or a store that names no mob, still fails the restore.

Retire at scope (since 0.9.4): `retire_at_scope(&scope)` retires exactly the
incarnation a `MemberDeliveryScope` names, with `retire`'s process-tree
semantics. MobMachine admits every step (each descendant and the member) on
the scope in the same actor command that retires it, so a successor minted
meanwhile (respawn, reset) is never retired through an older scope. Refusals:
`MobError::IncarnationMismatch` (a scope captured at another incarnation of
the mob, one destroyed and created again under its id; answered before any
step), `MobError::StaleFenceToken` (another incarnation is current),
`StaleDeliveryScope` (the session is not the member's binding or the one its
retirement is releasing), `InvalidTransition` (completed or destroyed mob); a
refusal stops the call. `Ok(())` is `retire`'s barrier: it waits out a
retirement the mob still owns and answers `MemberRetirementStuck` for a stuck
one. Like `retire`, a never-seen identity answers `Ok(())`; it needs the
`Retire` control scope.

Fork instruction policy (since 0.9.4): a fork child holds the instruction
activations it copied from its source as history and runs no model turn until
each inherited key is activated in the child. Under the default
`RequireExplicitActivation`, `fork_member` keeps the host's activation window,
but the forks that start the child's turn themselves (`fork_off`,
`fork_member_then_run_bounded` and `_detached`, `fork_member_at_turn_boundary`
and so the live delegation's durable fork, council participants, local or
placed) are refused before anything is committed with
`MobError::ForkInstructionRefused { source_member_id, refusal }` (wire
`FORK_INSTRUCTION_REFUSED`, -32037, HTTP 422, exit 57, data kind
`fork_instruction_refused`). `ReactivateInherited` makes the fork append the
child's own activation of every key effective in the source
(`PlannedForkRequest::inherited_instructions`; `MobSessionService` decorators
forward it); `Refuse` refuses every fork whose child would inherit one. 0.9.3
committed such a child and its first turn failed ("forked session must
explicitly activate inherited instruction keys"). A council ends with
`TemporaryCouncilExitReason::ParticipantForkInstructionRefused
{ participant_order, refusal }` and the slot's `TemporaryCouncilAcquisition`
is `Refused` (nothing to revoke, no cleanup debt); a 0.9.3 reader cannot decode
such a council record. Upgrade member hosts before their controller: a 0.9.3
member host answers `ReactivateInherited` with the refusal cause
`member_host_unsupported` and forks participants without the policy otherwise.
A durable fork child's model reads each copied activation row as a
`[meerkat-inherited-instruction-v2]` header without its body. Copied
`load_skill` results and per-turn skill context follow the host's declared
policy (`DurableSessionForkTarget::declared_instructions`): named without
the body under `RequireExplicitActivation`, refused under `Refuse`
(`refusal.skills`), carried under `ReactivateInherited`; a fork without an
activation window strips them under the default instead of refusing. A row
that does not hold the bytes its identity names fails
`Session::messages_for_model_boundary` with `ModelBoundaryProjectionError`,
and the turn is refused.

Prompt-context forks (`MemberLaunchMode::Fork`, `fork_helper`): the
child gets a planned text of the source's history, decided by each row's
type under the same policy. The source's system prompt and host system
context are never carried. Under `RequireExplicitActivation` activation
bodies, skill bodies the source loaded with `load_skill` (a line names
each skill) and host-injected context rows (memory recall) are left out;
under `Refuse` such a fork is refused with `ForkInstructionRefused`
(`refusal.skills` names the skills); under `ReactivateInherited` they are
carried. A copied comms notice keeps its title, its sender (or recipient),
a status and payload preview and the sender's taint marker
(`interaction::SENDER_TAINT_MARKER`); a peer message also keeps its text.
It never keeps a request or delivery id or reply instructions: the content
of a request, response or lifecycle notice is the runtime's rendering of
those for the source, so it is left out.
The rows fit a byte budget, newest first (half the child model's
window at four bytes per token, at most 256 KiB); a newest row that alone
does not fit is refused before anything is spawned with
`MobError::ForkContextTooLarge` (`INVALID_PARAMS`, data kind
`fork_context_too_large`). The child's create build carries
`SessionBuildOptions::prompt_context_fork_source`. A child created on
this host gets the context as one injected-context row and a typed fork
notice (`SystemNoticeBlock::ForkBoundary`, wire `fork_boundary`) before
its first input, whoever starts that turn, so a turn-driven helper sees
it in its task turn; the session holds them until then as the metadata
entry `session_fork_seed_v1`. A child placed on a member host gets them
the same way when its host serves prompt-context fork children
(supervisor bridge protocol 7; the controller asks the host with
`HostFeatures` first); a host of 0.9.4 or earlier gives it the context
as a prompt prefix ending in a `[fork_notice]:` line. A durable
fork's child (`fork_member` and the forks built on it) gets a notice of
kind `transcript` at its first turn. A notice names the source and, by
name only, the instructions and tools the child lost or gained and what
a prompt-context fork carried. A 0.9.4 reader decodes the block as an
unknown notice with its summary.

Refused restore (since 0.9.4): when a registration hook (`select_destination`,
`select_agent_mandate`, `prepare_member_surface` or `verify_member_surface`)
refuses a restored member, the restore removes that registration exactly (the
canonical unregister, recorded in the session's history; inputs it still held
end `Stopped`) and releases its owner context. The member stays Broken with
`MobError::MemberRegistrationRefused { member_id, session_id, call, reason }`
(data kind `mob_member_registration_refused` with `hook_call`), shown in
`MobMemberSnapshot::registration_refusal` (`MemberRegistrationHookCall`) and
in member status as `registration_refusal`, until an explicit repair;
`ensure_member_live` returns it without asking the host again, and retire,
respawn and destroy still work. A member `MobBuilder::resume` restores at boot
gets the same refusal, and so does one it recreates on a fresh session (for
example when the session service keeps no durable sessions): only that member
is Broken and the others resume, where the refusal failed the whole resume
before. Any other failure to recreate a member still fails the resume. A
runtime that did not install the destination the host
selected answers `MobError::MemberDestinationNotInstalled { session_id, error }`
(data kind `mob_member_destination_not_installed` with a snake_case `cause`;
before, a `WiringError`). Its failure class is `RuntimeRejected`, so schedule
delivery records `runtime_rejected` instead of `mob_rejected`; the restored
member keeps its registration, and a re-selection must advance
`context_generation` (else the cause is `scoped_context_generation_not_advanced`).

Respawn restrictions (since 0.9.4): `MobHandle::respawn` and the `mob_respawn`
tool give a local member's successor the predecessor's launch tool access
policy and inherited visibility ceiling, as its session recorded them,
intersected with the successor's own and never widened. The successor keeps
the auth binding the predecessor's session recorded only when its own spec
names none and its profile resolves to the provider that binding was recorded
for; otherwise it selects its own, as a fresh spawn does. The successor's
profile and provider are resolved before anything is retired. A respawn whose
predecessor's restrictions cannot be established, whose successor profile
cannot be resolved, or whose member changes while they are read is refused
before anything is retired with
`MobError::RespawnPredecessorRestrictionsUnavailable`, and the member is kept.
It has no dedicated wire code: the message is
`respawn of member <id> refused: <reason>`. That includes a Broken member
whose session was lost, which respawn no longer repairs: retire it and spawn a
fresh member, which recovers nothing of the lost restrictions. A custom
`MobSessionService` that wraps a stock one must delegate
`predecessor_tool_restrictions` (default `Unsupported`), or its members'
respawns are refused. Not checked before retirement: a recorded binding since
removed from the realm configuration or re-pointed to another provider, so the
successor's build fails after the predecessor is retired
(`MobRespawnError::SpawnAfterRetireWithCause`); name a binding in a spawn
customizer, or retire and spawn. A peer-only member never bound to a local
session keeps its own spec's policy and ceiling; placed members are
unchanged. The authorization integration guide covers the security side
("Preserve restrictions across respawn").

Reload-required members (since 0.9.4): `force_cancel_member` and
`cancel_all_work` on a member whose runtime needs a durability reload answer
`MobError::MemberReloadRequired` at once (wire `MEMBER_RELOAD_REQUIRED`:
-32029, HTTP 409, exit 49); reload with `reload_member_registration`, then
cancel again. 0.9.3 failed with `MobError::Internal`.

Member binding history (since 0.9.4): `member_binding_history()` folds the
mob's durable member journal and Core's identity intent rows, read in one
store transaction, into a `MemberBindingHistory`. A `MemberBinding` carries
identity, generation, session (`None` for a peer-only member), role,
continuity intent, a `MemberBindingState` (`Live`; `Moved`: a binding
recovery moved the membership to another session; `Retired`: retired or
ended by a mob reset) and a `MemberIdentityManagement` (`CommandDriven`;
`IntentManaged`: Core's identity reconciler drives the identity, so do not
send it lifecycle commands too; `IntentUnreadable`: an intent row this build
cannot read, do not drive it). `binding_for_session` spans mob resets;
`last_binding`, `durable_identity_members` (live members spawned with
`SpawnContinuityIntent::DurableIdentity`) and `durable_identity_bindings`
(live or retired) see only spawns since the latest mob reset. A retired
identity keeps its last binding, and its session is the one `ensure_member`
with `MemberLaunchMode::Resume` can restore. The reads never enter the actor
queue, need `ControlScope::List` (`MobError::ScopeDenied`), and fail rather
than answer empty; a storage composition without the built-in identity and
member store refuses with
`MobStoreError::IdentityMemberAtomicPersistenceUnavailable`. Pruning never
removes the events they read. Destroy clears the journal, so a mob created
again under the same id starts a new history and `journal_cursor()` values
can repeat. With no running mob, `MobStorage::read_member_binding_history(path)`
(native builds) reads a persistent store with the same fold, read-only: it
never creates or migrates the store and refuses with
`MemberBindingHistoryReadError::StoreMissing` or `ForeignSchema` (`Read` for
a failed read).

Scratch `task_create` / `task_update` / `task_list` / `task_get` are separately
enabled agent tools, not `MobHandle` methods. Durable shared commitments belong
to WorkGraph, not that scratch-task surface.

### Rust example: full lifecycle via `MobBuilder` + `MobHandle`

```rust
use std::sync::Arc;
use meerkat_mob::{
    AgentIdentity, FlowId, MobBuilder, MobDefinition, MobSessionService, MobStorage,
    SpawnMemberSpec,
};

async fn run_mob(
    definition_toml: &str,
    session_service: Arc<dyn MobSessionService>,
) -> Result<(), Box<dyn std::error::Error>> {
    let definition = MobDefinition::from_toml(definition_toml)?;
    let storage = MobStorage::persistent("./mob.db")?; // SQLite/WAL-backed

    let handle = MobBuilder::new(definition, storage)
        .with_session_service(session_service)
        .create()
        .await?;

    handle
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-1")))
        .await?;
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("worker-1")))
        .await?;
    handle
        .wire(
            AgentIdentity::from("lead-1"),
            AgentIdentity::from("worker-1"),
        )
        .await?;
    handle
        .member(&AgentIdentity::from("lead-1"))
        .await?
        .send(
            "Coordinate a short execution plan.".to_string(),
            meerkat_core::types::HandlingMode::Queue,
        )
        .await?;

    let run_id = handle
        .run_flow(FlowId::from("release_flow"), serde_json::json!({"severity":"critical"}))
        .await?;
    let _run = handle.flow_status(run_id).await?;

    handle.complete().await?;
    Ok(())
}
```

### Rust example: high-level in-memory mob state helper

```rust
use meerkat_mob::{AgentIdentity, MobDefinition, ProfileName};
use meerkat_mob_mcp::MobMcpState;

async fn in_memory() -> Result<(), Box<dyn std::error::Error>> {
    let state = MobMcpState::new_in_memory();
    let definition = MobDefinition::from_toml(r#"
[mob]
id = "my-mob"
orchestrator = "lead"

[profiles.lead]
model = "claude-opus-4-8"
external_addressable = true

[profiles.lead.tools]
builtins = true
comms = true
mob = true

[profiles.worker]
model = "claude-sonnet-4-6"

[profiles.worker.tools]
builtins = true
comms = true
"#)?;
    let mob_id = state.mob_create_definition(definition).await?;

    state
        .mob_spawn(
            &mob_id,
            ProfileName::from("lead"),
            AgentIdentity::from("lead-1"),
            None,
            None,
            None, // no explicit host placement
        )
        .await?;
    let _status = state.mob_status(&mob_id).await?;
    Ok(())
}
```

## Shared integration model (`meerkat-mob-mcp`)

Outside direct `meerkat_mob` usage, agent-facing mob capability is provided by
late-binding `meerkat_mob_mcp::AgentMobToolSurfaceFactory` into
`SessionBuildOptions.mob_tools`. The factory receives the owning session ID,
runtime-injected `MobToolAuthorityContext`, effective authority handle, and
optional comms runtime during `AgentFactory::build_agent()`, then returns the
session-scoped `AgentMobToolSurface` dispatcher.

```rust
use std::sync::Arc;
use meerkat_core::ToolCategoryOverride;
use meerkat_core::service::{MobToolsFactory, SessionBuildOptions};
use meerkat_mob::{MobControlPrincipal, MobSessionService};
use meerkat_mob_mcp::{AgentMobToolSurfaceFactory, MobMcpState};

fn with_mob_tools(
    session_service: Arc<dyn MobSessionService>,
) -> Result<SessionBuildOptions, Box<dyn std::error::Error>> {
    // `new` is fallible (runtime adapter acquisition); `into_shared()` lets the
    // state run its own council recovery sweep.
    let state = MobMcpState::new(session_service, MobControlPrincipal::Owner)?.into_shared();
    let factory: Arc<dyn MobToolsFactory> =
        Arc::new(AgentMobToolSurfaceFactory::new(state));
    let mut build = SessionBuildOptions {
        mob_tools: Some(factory),
        ..Default::default()
    };
    build.apply_generated_create_only_mob_operator_access(ToolCategoryOverride::Enable);
    Ok(build)
}
```

Pass the returned options as `CreateSessionRequest.build` to a factory-backed service.
It supplies both dispatcher infrastructure and explicit per-build enablement;
the factory/runtime handoff mints generated create-only operator authority.
It does not grant access to arbitrary existing mobs. `.mob(true)` alone is
only an ambient factory default; never construct authority or runtime bindings
by hand to compensate for missing composition.

`external_tools` is still the right slot for callback tools and MCP-backed
dispatchers. Mob orchestration uses the separate `mob_tools` slot so operator
authority and session-scoped wiring are injected at build time.

## Surface matrix

| Surface | Mob access | Current behavior |
|---|---|---|
| CLI `run` / `run --resume` | `mob_*` tools in prompt-driven runs when mob tools are enabled | Primary CLI mob UX |
| CLI `rkat mob ...` | helper, artifact, and explicit operator commands | Secondary operational surface, including member-host, grant, observation, and live verbs |
| CLI `rkat mob pack/deploy/web build` | artifact and browser distribution | Portable deploy + web target |
| RPC | explicit `mob/*` methods | canonical typed substrate for SDKs; generic `mob/tools` / `mob/call` escape hatches do not exist |
| REST | session HTTP endpoints plus mob helpers | Existing helper/member routes plus the read-only multi-host observations |
| MCP | `meerkat_*` session tools plus `meerkat_mob_*` public tools, including `meerkat_mob_wait_ready` and profile CRUD | typed public mob control plane for host access |
| Python SDK | `Mob` class via `create_mob()` | first-class mob lifecycle, member mgmt, flow control, event subscriptions |
| TypeScript SDK | `Mob` class via `createMob()` | first-class mob lifecycle, member mgmt, flow control, event subscriptions |
| Web SDK | `Mob` class via `createMob()` | same WASM-backed mob lifecycle with typed `EventSubscription<T>` |

### Multi-host surface matrix

This is the authoritative exposure matrix for the multi-host console family.
The asymmetry is intentional: an absent mutation is a security boundary, not
an SDK backlog.

| Surface | Multi-host exposure |
|---|---|
| JSON-RPC | Full typed family: `mob/grant_scopes`, `mob/revoke_scopes`, `mob/grants`; `mob/member_history`, `mob/hosts`, `mob/route_installs`; `mob/bind_host`, `mob/revoke_host`; `mob/hard_cancel_member`; and `mob/member_live_open`, `mob/member_live_close`, `mob/member_live_status`, `mob/member_live_control`. `mob/spawn` and each `mob/spawn_many` spec also accept optional `placement`. |
| REST | Read-only additions only: `GET /mob/{id}/members/{agent_identity}/history`, `GET /mob/{id}/hosts`, and `GET /mob/{id}/route-installs`. Existing helper, event, status, cancel, and respawn routes remain remote-transparent and use the same typed multi-host error projector. No host/grant/live mutation endpoints. |
| Public MCP | Read-only additions only: `meerkat_mob_member_history`, `meerkat_mob_hosts`, and `meerkat_mob_route_installs`. No host/grant/live/hard-cancel tools, so an LLM-reachable MCP roster cannot acquire those operator mutations. |
| CLI | `rkat mob host`, `bind-host`, `revoke-host`, `hosts`; `grant`, `revoke-grant`, `grants`; `member-history`, `route-installs`; and the `live open/close/status/control` family. Existing helper/member verbs route by stable member identity. |
| Python / TypeScript | First-class wrappers for every JSON-RPC method above, including placed spawn. Return the shared generated wire types; do not invent SDK-only envelopes. |
| Agent tools | `delegate` and `mob_spawn_member` accept optional `placement`. Host/grant/observation/live/hard-cancel console verbs are deliberately absent from the LLM-visible roster. |
| Web / WASM | Single-host. Spawn carriers accept the source-compatible optional placement field, but a non-local placement is rejected by the typed capability boundary. No member-host, grant, observation-console, or member-live console exports. |

Member-live open/control is WebSocket-only for both local and placed members.
The generic session `live/*` family can negotiate WebRTC only when the session
is local to that RPC host; a controlling host cannot use `session/*` or
`live/*` to proxy a placed member's remote session.

All console surfaces consume the same four classified failures (wire codes
`SCOPE_DENIED`, `HOST_UNAVAILABLE`, `STALE_CURSOR`, and `STALE_FENCE`), plus
`STALE_DELIVERY_SCOPE` (JSON-RPC -32036, HTTP 409) for a scope-bound submit
whose member session moved under a current fence (re-capture the scope;
`STALE_FENCE` means re-resolve the member).
Unclassified failures keep each route or protocol's legacy rendering.

Runtime-mode behavior is shared across these surfaces because dispatch comes from the same mob runtime:

- autonomous members: event injection/subscription dispatch
- turn-driven members: direct `start_turn` dispatch

### Destroy completion

A destroy (`MobMcpState::mob_destroy`, RPC `mob/destroy`, the lifecycle
destroy on RPC, MCP and Web, the agent `mob_destroy` tool, and the cleanup of
a session's owned mobs on session archive) removes the mob's database files
only after its actor task ended and its database observer drained. While a
store, storage bundle, handle or reader in the same process still owns that
observer (including a `MobHandle` the caller kept across the destroy, or one
held by another `MobMcpState`), it keeps the files and answers
`MobMcpDestroyError::Incomplete`, reported on every surface with
`code: "mob_destroy_incomplete"`, the `destroy_report` and `retryable: true`
(JSON-RPC and MCP: internal-error data; REST session archive: HTTP 500). Drop
those handles and retry as the same caller; the retry continues the owed
cleanup and answers the canonical report. Creating, restoring or inserting a
mob under that id is refused until the cleanup is done. An admitted destroy
closes the mob's admission first (`MobAdmissionCloseReason::Destroyed`).
Since 0.9.4 a member whose boundary commit fails while the destroy waits on its
ingress detach is reloaded and its retirement retried once;
`MobDestroyReport::reloaded_members` names it (0.9.3 answered `Incomplete`).
Since 0.9.5, when the runtime refuses a member reload as repair-blocked (for
example while a boundary commit past its deadline has not reported its
outcome), the destroy answers `Incomplete` with the member in
`MobDestroyReport::member_reload_refusals` (`kind: "repair_blocked"` and the
runtime's `reason`): retry the destroy later. The SDKs read that report typed
(`WireMobDestroyReport`): `mob_destroy_incomplete_report(error)` in Python,
`mobDestroyIncompleteReport(error)` in TypeScript and `@rkat/web`. A refusal
in the reload pass before the destroy starts leaves the mob unchanged; a later
one comes after the destroy's cleanup has begun. A retire meets the same
refusal at its quiesce stage and answers `MemberRetirementStuck` whose cause
is `MobError::MemberReloadRepairBlocked` (data kind
`mob_member_reload_repair_blocked`, `retryable: true`); redrive it with
`MobHandle::redrive_retirement` once the outcome is reported. Since 0.9.5 a
retire that started before the member degraded reloads it at its ingress
detach and completes (retry an in-progress answer); a refusal there sticks
with the same cause. The stuck error's data carries the cause's data as
`cause_data` (since 0.9.5). A
host that drains observers itself reads a `SqliteMobObserverDrainReport`;
since 0.9.4 its `retained_registrations` counts change-watch notification
registrations still running when the drain stopped them (a struct literal
must set it).

## External members

This section covers one already-running peer process that Meerkat does not
materialize or own. It does not cover a managed session-backed member placed on
a bound member host. Managed cross-host members use `SpawnMemberSpec.placement`
with `RuntimeBinding::Session`; placement is machine-owned and is not an
`External` backend.

An unmanaged external member usually runs in another process, sandbox, or
host. It requires an explicit `RuntimeBinding::External` at spawn time; a bare
external backend tag is not enough. The current binding shape carries `kind:
"external"`, the advertised comms address, the external runtime's Ed25519
public identity, and a typed `bootstrap_token` for supervisor bridge binding.

For a remote `rkat` member, generate that binding from the target process:

```bash
rkat run \
  --comms-name remote-worker \
  --comms-listen-tcp 0.0.0.0:4200 \
  --comms-advertise-tcp 203.0.113.7:4200 \
  --comms-binding-out ./remote-worker.binding.json \
  --keep-alive \
  "You are a remote worker."
```
`--comms-advertise-tcp`
takes `HOST:PORT` or `tcp://HOST:PORT` (0.9.1); HOST is a DNS name, an IPv4
address or a bracketed IPv6 address, resolved when a peer connects, and it is
advertised as `tcp://HOST:PORT`. A wildcard host and a missing or zero port
are refused (0.9.0 accepted only IP literals). A wildcard
`--comms-listen-tcp 0.0.0.0:...` needs the flag or a realm `[comms]
advertise_address`.

Use `rkat run --comms-listen-tcp` for the signed Meerkat peer channel.
`rkat-rpc --tcp` is only JSON-RPC host transport and does not make an agent
reachable as a signed peer or external mob member.

### Spawn startup policy

- Mob member spawn uses deferred initial turn semantics.
- Session creation for spawn registers the session without immediately running a model turn.
- Fresh runtime-backed `autonomous_host` members start their host loops and
  automatically admit an initial kickoff after activation/readiness. The
  kickoff uses `initial_message` when supplied, otherwise a fallback prompt.
  This is real runtime prompt dispatch and can call the provider without any
  later user message, peer message, or flow step.
- `turn_driven` members have a separate dispatch path: an explicitly supplied
  initial message can run an initial turn, but an omitted message does not
  imply the autonomous fallback kickoff. Recovery that resumes an
  authoritative transcript suppresses a manufactured autonomous kickoff.
- Concurrent spawns provision in parallel; actor finalization stays serialized for deterministic state transitions.
- `spawn_many(Vec<SpawnMemberSpec>)` exposes this as first-class runtime API.

## Multi-surface examples

### CLI tool-driven (primary)

```bash
rkat run --tools full "Create a mob with one lead and three workers, wire lead to all workers, and report status."
rkat run --tools full --resume <session_id> "Retire worker-2 and add worker-4, then summarize."
```

### CLI direct commands (explicit operational)

```bash
rkat mob spawn-helper team-mob "Join as lead-1" --profile lead --agent-identity lead-1 --result-label join-report --max-text-bytes 4096
rkat mob fork-helper team-mob lead-1 "Investigate the failing test cluster." --agent-identity worker-2 --profile worker --result-label investigation --max-text-bytes 4096 --json
rkat mob member-status team-mob lead-1 --json
rkat mob force-cancel team-mob worker-1
rkat mob respawn team-mob worker-1 --initial-message "restart"
rkat mob run ./dist/release-triage.mobpack --prompt "triage latest regressions" --trust-policy permissive --json
rkat mob runs team-mob --json
rkat mob status team-mob <run_id> --json
rkat mob attach team-mob <run_id> --json
rkat mob run-flow team-mob --flow triage --stream
```

### CLI artifact + web deployment

```bash
rkat mob pack ./mobs/release-triage -o ./dist/release-triage.mobpack \
  --sign ./keys/release.key --signer-id team@example.com   # --sign requires --signer-id
rkat mob inspect ./dist/release-triage.mobpack
rkat mob validate ./dist/release-triage.mobpack --trust-policy permissive
rkat mob run ./dist/release-triage.mobpack --flow main --trust-policy permissive
rkat mob web build ./dist/release-triage.mobpack -o ./dist/release-triage-web \
  --wasm <PKG_DIR|name_bg.wasm> --trust-policy permissive
```

Packing signs the artifact but does not install its signer. The explicit
permissive policy is therefore the local posture until the signer is installed;
signature verification still runs and warns that the signer is unknown.
`--wasm` is required and names prebuilt wasm-pack `--target web` output.

### WASM browser surface

The web build is a surface over the shared `MeerkatMachine`. Direct sessions and mob members use `SessionOwned` bindings for the same admission, keep-alive, cancellation, terminality, and streaming semantics as other runtime-backed surfaces.

**How it works:**
- `meerkat-core` + `meerkat-client` compile to wasm32 via `tokio_with_wasm` (drop-in tokio replacement)
- `reqwest` uses browser `fetch` on wasm32 - no custom JS bridge
- `web-time` replaces `std::time` types (SystemTime, Instant) for browser compatibility
- Anthropic CORS header added automatically on wasm32 targets

The shared `BrowserRuntimeProfile` controls availability. In-memory sessions,
foreground execution, keep-alive, in-process comms, transient turn context, and
embedded skills are supported. Excluded capabilities include durable storage,
background execution after page teardown, host processes, configured hooks,
runtime skill discovery, MCP clients, TCP/UDS comms, and remote placement.
Requests for exclusions return `CAPABILITY_UNAVAILABLE` with typed clearing
actions; inspect the profile for the complete current capability set.

**WASM API:**
Key mob-related exports (a partial list; the full WASM export reference is not part of `rkat help`):
```
mob_create(definition_json) → mob_id string  [async]
mob_spawn(mob_id, specs_json) → result JSON  [async]
mob_wire / mob_unwire / mob_wire_peer / mob_unwire_peer / mob_retire / mob_respawn  [async]
mob_list_members / mob_member_send / mob_events(mob_id, after_cursor: string, limit: u32) / mob_status / mob_list  [async]
mob_member_status / mob_spawn_helper / mob_fork_helper / mob_force_cancel / mob_stop_member_run / mob_append_system_context  [async]
mob_lifecycle(mob_id, action)  [async]
mob_run_flow → run_id string / mob_flow_status / mob_cancel_flow  [async]
mob_member_subscribe / mob_subscribe_events → stream_id string  [async]; poll_subscription / close_subscription  [sync]
```
There is no `wire_cross_mob` export.
There is also no browser `mob_run_result` export: `mob_flow_status` returns a
`MobFlowStatusResult` JSON envelope with `run` (state and ledgers, or null).
The Web SDK's `mob.flowStatus(runId)` unwraps it to `FlowStatus | null`.
Native RPC `mob/run_result` and native SDK run-result helpers are separate
surface contracts.

### RPC

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"session/create","params":{"prompt":"Use mob_* tools to create a lead/worker mob and return status.","enable_mob":true}}
```

A mob's members reach the callback tools registered on the JSON-RPC
connection that holds the mob's callback route: the connection whose
`mob/create` created it, or the one that claimed it with
`mob/claim_callback_route {mob_id, claim_token}`, presenting the claim token
`mob/create` returned once to the creator (the mob keeps only its SHA-256).
A mob restored after a restart, created any other way, or whose connection
closed, has no route: its members' callbacks fail `tool_unavailable` until a
connection claims it. A mob created other than by `mob/create` has no token
and is not claimable unless its host mints one
(`MobMcpState::mint_callback_claim_token`, Rust only). The claim reaches live
and later members alike; another token is refused with `-32002`
(`claim_token_mismatch`), and so is a claim while the holding connection is
open (`held_by_open_connection`).

### REST

```bash
curl -X POST http://127.0.0.1:8080/sessions \
  -H "Content-Type: application/json" \
  -d '{"prompt":"Use mob_* tools to create a lead/worker mob and return status.","enable_mob":true}'
```

### MCP

```json
{
  "name": "meerkat_run",
  "arguments": {
    "prompt": "Use mob_* tools to create a lead/worker mob and return status.",
    "enable_mob": true
  }
}
```

### Python SDK

```python
from meerkat import MeerkatClient

client = MeerkatClient()
await client.connect(realm_id="team-alpha")
result = await client.create_session("Design a mob topology for release triage.")
print(result.text)
await client.close()
```

### TypeScript SDK

```typescript
import { MeerkatClient } from "@rkat/sdk";

const client = new MeerkatClient();
await client.connect({ realmId: "team-alpha" });
const result = await client.createSession(
  "Use mob_* tools to create a lead/worker mob and return status.",
  { enableMob: true },
);
console.log(result.text);
await client.close();
```

These execution examples require a host with mob support and a composed
`MobToolsFactory`; `enable_mob` / `enableMob` explicitly requests generated
create-only operator capability. It does not grant arbitrary existing-mob
scopes or bypass per-call authorization. The Python design-only prompt does
not require the model to execute mob tools.

## Flows (subfeature)

Flows add DAG orchestration to mobs.

### v1 flows (flat step DAG)

Flow essentials:

- `depends_on` + `depends_on_mode` (`all`/`any`)
- `dispatch_mode` (`one_to_one`/`fan_out`/`fan_in`)
- `collection_policy` (`any`/`all`/`quorum`)
- optional `condition` and `branch`
- persisted `step_ledger` and `failure_ledger`

### v2 flows (frame-based execution with loops)

All flows execute through `FlowFrameEngine` with a canonical
`FlowSpec.root: FrameSpec`. Flat step declarations normalize to that root at
decode/construction; an explicitly authored `root` takes precedence over the
generated structure. These are two authoring forms, not separate engines.

Key types:

- `FrameSpec` - a set of `FlowNodeSpec` nodes forming a dependency graph within a frame
- `FlowNodeSpec` - either `Step(FrameStepSpec)` or `RepeatUntil(RepeatUntilSpec)`
- `FrameStepSpec` - references a named `step_id`, with `depends_on` and
  `depends_on_mode`; role/message stay in the flow's `steps` map
- `RepeatUntilSpec` - loop with `loop_id`, `depends_on`, `depends_on_mode`,
  `body: FrameSpec`, `until: ConditionExpr`, `max_iterations: u32`

Execution model:

- `MobMachine` owns per-frame state (node readiness, completion tracking)
- `MobMachine` owns loop body/evaluate lifecycle
- `MobMachine` owns scheduler grants (`GrantNodeSlot`, `GrantBodyFrameStart`), frame-step projection, and terminalization
- `flow_run`, `flow_frame`, and `loop_iteration` are MobMachine-owned fail-closed projection reducers used to materialize `MobRun` snapshots. They are not standalone machines.
- Frame-step outcomes route back through MobMachine-owned transitions; direct mutation from executor code is prohibited
- Recovery handles ready-frame / pending-body-frame drift and returns typed incompatibility for pre-v2 active runs

Definition example inside a mob with `tester` and `lead` profiles:

```toml
[flows.release_flow]
description = "Iterative release check"

[flows.release_flow.steps.run_tests]
role = "tester"
message = 'Run tests and return only JSON with an "all_pass" boolean: true if all pass, false otherwise.'
output_format = "json"

[flows.release_flow.steps.ship]
role = "lead"
message = "Ship it."

[flows.release_flow.root.nodes.check_quality]
kind = "repeat_until"
loop_id = "quality_loop"
depends_on = []
depends_on_mode = "all"
max_iterations = 5
until = { op = "eq", path = "steps.run_tests.all_pass", value = true }

[flows.release_flow.root.nodes.check_quality.body.nodes.run_tests]
kind = "step"
step_id = "run_tests"
depends_on = []
depends_on_mode = "all"

[flows.release_flow.root.nodes.ship]
kind = "step"
step_id = "ship"
depends_on = ["check_quality"]
depends_on_mode = "all"
```

### Operational flow controls

- list flows
- run flow
- check flow status
- cancel flow

### Agent-facing delegation tools

With generated authority, `AgentMobToolSurface`
(`crates/meerkat-mob-mcp/src/agent_tools.rs`) exposes thirteen base definitions:

| Tool | Purpose |
|------|---------|
| `delegate` | Quick helper spawn - creates implicit mob on first use, spawns member, auto-wires comms |
| `conclude_objective` | Supply only `outcome` to conclude the kickoff objective pre-addressed by the current turn |
| `mob_create` | Create a mob from a definition |
| `mob_destroy` | Destroy a mob and archive all members |
| `mob_spawn_member` | Spawn a member into an authorized mob |
| `fork_off` | Fork the current durable member's committed transcript prefix through the resume path into a child the caller owns, and run the task in the background; the outcome is recorded in the forker's transcript as a durable `BackgroundJob` notice (blocking on one-shot hosts) |
| `council` | Fork existing specialist members into a temporary discussion mob, run bounded rounds/merge, and clean up; the sealed outcome is recorded in the convener's transcript as a durable `BackgroundJob` notice (blocking on one-shot hosts) |
| `mob_retire_member` | Archive a member and its session (manage scope, or the caller owns the member) |
| `mob_check_member` | Check a member's execution status and output (manage scope, or the caller owns the member) |
| `mob_list_members` | List members of a mob (without manage scope: only the caller's own descendants) |
| `mob_list` | List all mobs |
| `mob_wire` | Wire a member to a local or external peer (creates comms trust) |
| `mob_unwire` | Remove a wiring relationship between a member and a peer |

`conclude_objective` requires an objective-correlated turn and its resolved
lead principal. `fork_off` requires the caller to be a durable mob member with
spawn authority; supply `member_id` and `task`, optionally `message_count`,
`expected_output` (prompt guidance, not a validated schema), `result_label`,
`max_text_bytes`, and `max_run_secs`. Unknown arguments are rejected. The
child runs turn-driven whatever its role's default runtime mode. This
durable transcript fork is distinct from `MemberLaunchMode::Fork` /
`fork_helper`, which seed a fresh session with a planned text of the
source's history (see "Prompt-context forks");
low-level `Session::fork()` / `fork_at()` / `fork_replacing()` are separate
structural primitives. Every fork starts with zero usage: the source's
lifetime token counters are not copied into the child.

`fork_off` is detached where the host can deliver a later completion: it
declares `DetachedCompletionDelivery::Available` and has a runtime adapter
(the default for hosts built with one: the JSON-RPC, REST and MCP servers,
Meta; also `rkat run --keep-alive` and `rkat mob deploy --surface rpc`), and
it has bound its runtime delivery inbox with `MobMcpState::bind_continuations`
(the Core hosts above do). It returns once the child is seated and its turn
admitted: `status: "running"`,
`agent_identity`, `member_ref`, `fork_session_id`, a `job_id`, and a `note`
(no `cache_inheritance`: that host-side accounting stays on
`ForkMemberResult`). When the child's turn ends, its outcome is recorded
once in the forker's transcript as a durable `BackgroundJob` system notice
(header "Background fork_off job <id> finished (<status>):" with status
`completed`, `terminated` for an autokill, or `failed`; the outcome JSON is the
typed block's `detail`, with `persisted: true`). The outcome is committed once
as a `ForkJobTerminal` event in the child's mob, then submitted to the forker
as a durable continuation under the idempotency key `fork_off:<job_id>` and
applied as a runtime input with steer handling: a forker mid-turn sees it at
that turn's next model call (a durable in-turn append, saved once, no
second turn; one follow-up turn only if the turn ends first), an idle forker
runs one wake turn that sees it, and a non-live forker is revived through its
mob first. It is
readable in later turns and through session history even after the child is
retired. The outcome status is `completed` (with `bounded_result`, `usage`,
`turns`, `tool_calls`), `failed` (child retired), `max_run_elapsed` (run
cancelled, child retired), `supervisor_stopped`, or `restart_interrupted`. The
child's durable `ForkJobRecord` lets a restarted host re-link it: a one-time
pass after restore (or when Meta inserts a restored handle) delivers a reply
already in the child's durable transcript, observes a still-running child with
`max_run` measured from the original start, or delivers `restart_interrupted`,
under the same idempotency key, and wakes an idle forker; a job whose
completion was already admitted is never re-linked, and a respawned child
carries no job. The re-link routes by the job's `owner_session_id` (never the
child's roster entry); a limit already elapsed delivers `max_run_elapsed` at
once and retires the child only after delivery settles (no
`retirement_error` on the re-linked outcome); a respawned forker is
`OwnerGone`; a forker that cannot be served yet (its mob not running, or its
resume still in progress) receives the durable continuation once it is. The
`rkat` CLI declares `Unavailable` unless it stays alive, so `rkat run` without
`--keep-alive` and one-shot `rkat mob` commands block and return the child's
result directly, with `blocked_because` (`host_declared_unavailable`,
`no_runtime_adapter`, `no_owner_revival_host`, or `no_continuation_owner` on a
host that never bound its delivery inbox). Neither form has a default deadline, and the agent
loop's default tool deadline does not cut it (an explicit
`tools.tool_timeouts.fork_off` still does); `max_run_secs` is an opt-in
autokill, honored in both forms, that cancels the run and retires the child.

The forker owns its child, and transitively every member that child forks
(ownership never flows upward). Ownership is durable spawn provenance
(`RosterEntry::spawned_by`, kept across resume, respawn and successor-spec
respawn) checked against the caller's own session binding, never against
arguments. Without manage scope the forker can still observe its descendants
with `mob_check_member` (a status read never waits on the member's turn; when
it could not observe the member's session it still answers, with a typed
`preview_unavailable` of `observation_deadline`, `read_failed`,
`session_absent` or `not_observed_while_retiring`, meaning `output_preview`
and `tokens_used` are missing rather than empty, and a `note` that says
whether checking again helps), retire them with `mob_retire_member`, and see
them, and only them, in `mob_list_members`; the member operator tools
`member_status`, `retire_member`, `force_cancel_member`, and `list_members`
apply the same rule. Retirement cascades to descendants, deepest first, in
the core retire (`MobHandle::retire`; `retire_with_descendants` is an alias):
the retire tools, autokill, failed-child cleanup, host `mob/retire` /
`meerkat_mob_retire`, and Meta's retire paths all take it, and a child
forked mid-cascade is retired too. A child whose own turn fails is
retired automatically; a child whose turn completes stays seated until its
forker retires it. Meerkat adds no retention limit; Meta applies its
`idle_retire_secs` policy.

`council` requires creation authority and scope over each source mob. Supply a
`topic` and participants with `mob_id`, `member_id`, and discussion `role`;
optional controls include `council_id`, `max_rounds`, `max_exchanges`,
`max_result_bytes`, `timeout_seconds`, and `merge`. Source profiles are
resolved by the tool; participants are forks, not the original members. Like
`fork_off`, a council is detached where the host can deliver a later
completion: the call returns `status: "running"`, the `council_id`, and a
`job_id`, and the sealed outcome (`result`, `cleanup`, `replayed`, or `error`
when it fails after the call returned) is recorded and delivered the same way
as a "Background council job ... finished" notice; a failure exit reason gives
it status `failed`. A detached council also re-links after a restart: its
custody record carries the convener's job, and the outcome (or
`coordinator_interrupted` once the dead coordinator's lease expires) is
delivered once. The council recovery sweep that does this runs only on a
`MobMcpState` built with `into_shared()` (Meta 0.8.43 does; a plain
`Arc::new` never runs it). `council_id` may contain only ASCII alphanumerics,
`-` and `_` (the derived default is `agent-<uuid>`). On a
one-shot host, or for a plain-session convener on a host without a
`DetachedOwnerHost` (a top-level REST, MCP-server or keep-alive CLI session;
`blocked_because: "no_owner_revival_host"`), the call blocks and returns the
sealed outcome, so no detached job is owed.
`timeout_seconds` bounds it; the agent loop's default tool deadline does not.

Visibility alone satisfies none of these per-call prerequisites.

Model-facing inputs decode through the public contract: `mob_create` takes a
`MobDefinitionInput`, and the profile tools and an inline `tooling` profile
take a `MobProfileInput`, so host-only fields such as `mcp_servers` or
`rust_bundles` are refused with `InvalidArguments`, as are host-path skill
sources. `tooling` on `delegate` and `mob_spawn_member` is `inherit_parent`
(default; `allow_overlay`/`deny_overlay` narrow it), `minimal` (comms tools
only), or `profile` (`realm_profile` or `inline`), and a profile is capped by
the spawning agent's visible tools. On a managed host
(`MobMcpState::with_tool_consequence_policy_registry`), members of
agent-created child mobs and `delegate` helpers run under
`MobMcpState::with_child_application_tool_policy`; without one, `mob_create`,
`delegate` and spawns into child mobs are refused
(`child_tool_policy_required`). A host registers each Rust tool bundle for
child mobs as `HostOnly` (default), `ChildAvailable` (supplied to every inline
child profile) or, since 0.9.4, `ChildOffered`: supplied to no profile, and
mounted only for an inline child profile that names it in `tools.bundles`.
Any other name is refused with `MobError::ToolBundleNotOffered`, which lists
the offered names; an MCP server cannot be `ChildOffered`
(`ChildMcpServerRegistrationError::OfferNotSupported`). The opt-in persists
with the definition and the offer is its ceiling at every build:
`MobHandle::withheld_tool_bundles` reports opted bundles the host no longer
offers, and the member is built without them.

A realm profile store adds five profile-management tools for reusable,
versioned member templates:

| Tool | Purpose |
|------|---------|
| `mob_profile_create` | Register a named profile in the realm |
| `mob_profile_get` | Read a profile (with revision) |
| `mob_profile_list` | List profiles in the realm |
| `mob_profile_update` | Update a profile with `expected_revision` for CAS |
| `mob_profile_delete` | Delete a profile with `expected_revision` for CAS |

With both that store and a parent tool snapshot provider,
`mob_profile_list_sources` additionally lists visible tool sources grouped by
provenance. A profile store alone does not expose it.

These tools are composed into the agent's tool dispatcher via `MobToolsFactory` late-binding. Operator authority is injected at runtime through `MobToolAuthorityContext`; ambient mob enablement alone does not surface operator tools on resume.

## Practical guidance

Use mobs when you need:

- long-lived role-based multi-agent systems,
- explicit peer graph control,
- durable operational history.

Use plain sessions when:

- single-agent execution is sufficient,
- no shared graph/lifecycle state is required.
