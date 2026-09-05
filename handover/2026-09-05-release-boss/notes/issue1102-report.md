## Design verdict and plan (lead, 2026-09-05)

# OB3 fleet-wide delivery stall: trace, verdict, redesign, cold-reload verb, test plan

Scope: meerkat origin/main ae0394e65 (contains 0.8.33) and meerkat-mobkit origin/main de602d55 (pins meerkat =0.8.33; 0.8.31 + two post-release PRs). Read-only spike worktrees at `/home/luka/src/wt/meerkat-spike-ob3` and `/home/luka/src/wt/mobkit-spike-ob3`. No checkout modified, no cargo run. Line numbers are against those two commits.

Incident (OB3 twin, 2026-09-04, verbatim log in `/tmp/rb/ob3-lines.txt`):

- 21:35:00 `meerkat_runtime::runtime_loop` for review:singleton: `completed_run_terminal_persist` and `completed_boundary_commit` fail (continuity save -> BigQuery HTTP 0); "Runtime recovery is repair-blocked ... registration-authorized cold reload is required"; "runtime-loop exit teardown did not complete; retained for retry".
- 21:37:42 `actor_loop_stalled` (QueryPhase probe unanswered 30 s).
- 21:40 fleet-wide agent_events stop.
- 21:46:32, 21:56:32, 22:06:32, 22:16:33, 22:26:33: console sends to review, admin, review, person, person each abandoned after exactly 600 s on `deliver.submit_work`.
- OB3's own peer sends to review:singleton time out after 90 s.
- Later: `actor_loop_recovered`; 97-initiative board trigger sits at 0 claimed.

## 0. Summary

1. The 600 s waits are not a meerkat-runtime lock. meerkat-runtime fails the repair-blocked member fast and closed (`RecoveryRepairBlocked`) and holds no cross-session lock on the admission path.
2. The wait is the meerkat-mob actor: one serialized command loop (`meerkat-mob/src/runtime/actor.rs:24362`) that runs member-local, I/O-bearing work inline before it defers the `SubmitWork` reply. One member's stuck inline step queues every member's `SubmitWork`. `MobHandle::send_actor_command` (`handle.rs:5119-5145`) has no deadline and no cancellation; MobKit's 600 s budget only abandons the caller, the command stays queued and executes later.
3. The runtime-loop "retained for retry" teardown is a one-shot spawned watcher (`meerkat-runtime/src/runtime_loop.rs:4846-4867`); it takes no registration transaction lock, is not retried in a loop, and no other member ever needs it.
4. `MachineCleanupTaskSpawner` (`meerkat-runtime/src/meerkat_machine/mod.rs:2206-2233`) is a process-wide single-worker tokio runtime on which EVERY ingress admission runs. It is a latent fleet-wide chokepoint (one synchronous block there stalls every session's admission) but I found no synchronous block on the accept path; it is not the cause here.
5. Serialization of shared state is not the design mistake. Executing per-member blocking work inside the serialized loop is. HomeCore's boot stall is the same class: `ResumeLifecycle` runs two serial per-member loops inline (`actor.rs:16424-16500`), each member bounded at 5 s, so N members hold the loop for up to 10 s x N.
6. MobKit has no cold-reload verb. meerkat exposes the primitive and auto-mints the reload on the next executor registration. Today's operator path is `mobkit/retire_member` then send; `mobkit/respawn_member` is a destructive continuity reset and the wrong tool.

## 1. Q1: does one repair-blocked member block deliveries to other members? Trace with file:line

### 1.1 MobKit delivery path

- Budget: `meerkat-mobkit/src/identity_first/bridge.rs:838` `BRIDGE_ACTOR_ADMISSION_BUDGET = Duration::from_mins(10)`, env `MOBKIT_BRIDGE_ACTOR_ADMISSION_SECS` clamped 1..3600 (`bridge.rs:845-857`). Rationale in the doc comment 826-857: the actor is one serialized loop so an admission can queue behind another member's work; the budget is sized below a 962 s production hang.
- `submit_internal_bridge_work` (`bridge.rs:1011-1120`): round trip 1 `deliver.get_member` (1020-1027), round trip 2 `deliver.submit_work` (1073-1103, or `deliver.start_work` for completion-bearing).
- `ActorAdmissionTimeout` is returned to `identity_first/runtime.rs:130-135` as `IdentityRuntimeError::AdmissionFailed`; the bridge refuses to route it to repair (`bridge.rs:4677`). Nothing cancels the queued command.

### 1.2 `get_member` does not touch the actor

`MobHandle::get_member` (`meerkat-mob/src/runtime/handle.rs:7970-7980`) -> `execute_machine_command(GetMember)` -> `handle.rs:6136-6147`: `self.roster.read().await` plus a `machine_state_watch_rx.borrow()` snapshot. So "get_member passed, submit_work hung" carries no information about actor health. `deliver.submit_work` is the first actor round trip in a delivery, and all five abandonments were on it.

### 1.3 `submit_work` is an unbounded, uncancellable actor round trip

- `submit_work_with_mode` / `_and_delivery_identity` (`handle.rs:10842`, `10883`) -> `execute_machine_command(SubmitWork)` (`handle.rs:5648-5653`, runs on a stack-relief task) -> `handle.rs:5760-5820` builds `SubmitWorkPayload` -> `handle.rs:5816` `send_actor_command(MobCommand::SubmitWork)`.
- `send_actor_command` (`handle.rs:5119-5145`): `command_tx.send(routed).await` (bounded mpsc, blocks when full) then `reply_rx.await` with no timeout. If the caller drops after 600 s, the command remains in the queue. The actor later runs `handle_submit_work` in full and does `let _ = reply_tx.send(..)` (`actor.rs:21828-21830`). Therefore bridge.rs:257 ("the delivery did NOT happen") is not guaranteed: abandoned sends become ghost turns once the loop drains. Contrast `drive_resume_actor_operation` (`handle.rs:5150-5170`), which reserves channel capacity under a deadline; `SubmitWork` has no such variant.

### 1.4 What the actor does with `SubmitWork`, inline

`actor.rs:21806` `MobCommand::SubmitWork { payload, reply_tx }`: identity-convergence gate (21808, pure) then `Box::pin(self.handle_submit_work(payload)).await` (21826) INLINE on the loop. `handle_submit_work` (`actor.rs:50099`) awaits, still inline:

- 50183 `self.roster.read().await`
- 50189 `ensure_member_not_broken` (9732; reads restore diagnostics)
- 50208 `resolve_spawn_policy_via_machine`
- 50469 `ensure_member_event_pump` (35377) -> `member_pump_tap_material` (roster read) -> `MemberEventPumps::ensure_pump` (`event_pump.rs:1180`): takes the mob-wide `pump_transition` async mutex (1181) and, if the member's pump is being replaced, `task.abort(); let _ = task.await;` (1216-1220) with no bound, while holding `pump_transition`.
- 50540-50556 `MobMachineMutator::apply(SubmitWork)` and `SubmitWorkIngressAuthority::from_transition` (pure, this is the DSL critical section that genuinely needs the loop)
- 50560-50583 `dispatch_member_turn_after_machine_admission` (51438): 51555 `session_service.live_session_actor_registered(bridge_session_id)`, 51666 `ensure_autonomous_runtime_ready` (14960) -> `ensure_mob_comms_drain` (14970-15013: `provisioner.comms_runtime`, `adapter.maybe_spawn_mob_comms_drain`) and `ensure_autonomous_dispatch_capability` (15241; contains a 40 x 25 ms poll loop at 15379-15381), then 51679 the admission request build.

Only then does the arm return `AwaitTurnAdmission` and the loop calls `spawn_turn_admission_reply` (`actor.rs:51028-51106`), which spawns into the unbounded `actor_io_tasks` JoinSet (5579) a task that runs `provisioner.admit_turn` and sends the reply. The loop proceeds to the next command.

### 1.5 The spawned admission (per member)

- `admit_turn` (`meerkat-mob/src/runtime/provisioner.rs:9157`): `ops_adapter.report_member_progress` (`ops_adapter.rs:2008`; sync in-memory ops registry, no I/O) -> `admit_runtime_input` (3884): `runtime_session_state` (3657) takes the mob-wide `runtime_sessions` RwLock read; its writers (1596, 7839, 7890) only await `clear_queued_turns`, so it is not a long holder; `exact_operation_guard` (4784) is a per-session mutex; then `adapter.accept_input_with_completion_for_attachment` (`meerkat-runtime/src/meerkat_machine/runtime_control.rs:9938`).

### 1.6 meerkat-runtime admission: per-session gates, fail-fast for the degraded member

- `execute_meerkat_machine_ingress_command` (`dispatch_ingress.rs:955-972`) spawns the command onto `MachineCleanupTaskSpawner::acquire()` = process-wide single-worker runtime (`mod.rs:2206-2233`, `worker_threads(1)`, thread "meerkat-machine-cleanup"). Async tasks interleave there, so it only wedges everyone if a task blocks synchronously. On the accept path I found none (`block_on` only at `session_management.rs:1137` on a dedicated worker; `spawn_blocking` at 1169/1195). It remains a latent process-wide chokepoint and belongs on the hardening list.
- Handler `dispatch_ingress.rs:980-1300`: `sessions.read()` (1012); `lock_current_durability_ready_session_mutation_gate` (`mod.rs:5023`): per-session gate; if `require_durability_ready()` fails (mod.rs 5045) it detaches the attachment, spawns a bounded-ack hard-cancel retry (5121-5193) and returns `RecoveryRepairBlocked` (5194-5197) - no wait; `entry.require_durability_ready()` again at 1096-1101; per-session `driver.lock()` (1475); durable admission write `driver.accept_resolved_input` (1288 -> `driver/persistent.rs:1690`: `durable_idempotency_duplicate` 1698, `externalize_input_images` 1723, `persist_input_states_atomically` 1821).
- `mark_durability_reload_required` (`driver/persistent.rs:565-582`, called from `fail_terminal_transition` `meerkat_machine/driver.rs:4560-4566`) poisons only that session's `durability_health`.
- No `sessions.write()` is held across an await: I checked every candidate with an await nearby (comms_drain.rs:1223, dispatch_drain.rs:383, llm_reconfigure.rs:212, session_management.rs:4437, 8982, 9254, 9315); all release before awaiting.
- `lock_session_registration_transaction` (`mod.rs:7911`) is per session; the reload discard (`session_management.rs:4952-5060`) drops the registration guard before its worker runs (5040).
- MobKit's RuntimeStore: `SessionStoreBackedRuntimeStore` (`meerkat-mobkit/src/mob_handle_runtime.rs:1969`, built at 7524/7532) wraps an in-memory or provider runtime store; only the session snapshot commits go to the SDK-hosted session store (2210-2290, 2388-2500) -> `ContinuitySessionStoreAdapter` (`identity_first/adapters.rs:2578`, per-session `lock_session`) -> `GatewayContinuityStore` (`identity_first/gateway_bridges.rs:79`) -> `StdioCallbackBridge::call` (`bin/rpc_gateway.rs:7568-7625`): concurrent pending map, 130 s per-call timeout, but `stdout_tx.send().await` (7601) on the shared 64-slot stdout channel (10537) has no timeout and `write_gateway_stdout_line` (7360-7370) is a synchronous `std::io::stdout().lock(); writeln!; flush` inside an async task. If the SDK process stops reading, the whole callback lane and all event notifications wedge with no bound. The Python SDK reader thread does not block on callbacks (`sdk/python/meerkat_mobkit/_transport.py:143-204`), so this is a latent hazard, not the OB3 cause.

### 1.7 The "retained for retry" teardown

`runtime_loop.rs:4846-4867`: a spawned watcher awaits `teardown_slot.wait_until_published()`, calls `machine.observe_runtime_loop_teardown` (`session_management.rs:6309`; it takes `sessions.read()` at 6348 and the entry's std mutex at 6361, no registration transaction) once, logs the warning on error, and exits. "Retained" means the `runtime_loop_teardown` slot stays on the session entry for the reload discard to consume (`session_management.rs:5000-5006`). It holds nothing other members need. The session service is per-session actors (`PersistentSessionService`), not a shared actor.

### 1.8 Where the repair-blocked member's admission actually stops

For the degraded member, the accept path returns `RecoveryRepairBlocked` immediately (1.6). So the 600 s wait cannot be the runtime accept. It has to be an inline step in 1.4 that the degraded state makes hang, or a queue position behind such a step. Candidates, all reachable from post-degrade handling of that one member:

- `ensure_pump` `task.await` of the member's replaced pump under the mob-wide `pump_transition` (`event_pump.rs:1181, 1216-1220`).
- `maybe_spawn_mob_comms_drain` (`actor.rs:14996-15006`) against a registration whose executor attachment was detached by `lock_current_durability_ready_session_mutation_gate` (`mod.rs:5090-5106`).
- `live_session_actor_registered` (`actor.rs:51555`) against a session actor whose teardown is retained.
- Placed-completion cancellation retry loops with unbounded backoff (`actor.rs:50660-50700`), and `reconcile_joined_member_live_mutation` run inline every loop iteration (`actor.rs:24117`, `34201-34212`).

Which one wedged is not provable from the log. The `tracing::debug!` lines "MobActor handling SubmitWork command" (21817) and "handle_submit_work started" (50122) will pinpoint it if OB3 runs with `meerkat_mob::runtime::actor=debug`.

### 1.9 Why it is fleet-wide

Any inline await in 1.4 holds the single loop. Every later `SubmitWork` (any member), every `QueryPhase` probe, every `Retire`/`Wire`/`MemberLive*` command and every re-entering completion command sits in `command_rx` behind it. Timing out client-side does not dequeue. Event pumps and member-live reconciliation are actor-owned, so agent_events stop (21:40). The 10-minute cadence is sequential console sends each entering the queue and each abandoned at 600 s. OB3's 90 s peer sends fail the same way at a shorter budget.

Two caveats for the operator: (a) `actor_loop_recovered` is not proof of health: the probe treats any typed error as "live" (`unified_runtime/mod.rs:2510-2513`), and a fail-stopped actor closes `command_rx` (`actor.rs:24367-24375`), which resolves the parked probe with `ActorCommandChannelClosed` and emits "recovered" while the mob is dead (after that, sends fail instantly). (b) The `durable_uncertainty_fail_stop` setters (`actor.rs:8162-8257, 12040-12246, 13272-13733`) are placed-member/panic paths, not the local repair-blocked path; the sustained 600 s waits say the channel stayed open here.

### 1.10 Verdict on Q1

- meerkat-runtime: isolation is correct and by design (per-session gates, fail-fast repair-blocked, no shared lock across awaits).
- meerkat-mob actor: the single loop is by design and documented (bridge.rs:826-857, unified_runtime/mod.rs:2538-2550). Defects: (i) `handle_submit_work` performs per-member I/O-bearing awaits inline before deferring the reply; (ii) `send_actor_command` has no deadline and the actor never skips commands whose caller is gone; (iii) `ensure_pump` joins a task under a mob-wide mutex.
- MobKit: its only defence is a 600 s client budget that unqueues nothing, and a probe whose "recovered" can be false.

## 2. Q2 part 1: the boot materialization stall (HomeCore, ~30 s per member)

- `MobCommand::ResumeLifecycle` arm (`actor.rs:23366`) -> `ensure_autonomous_runtimes_from_roster(true, None)` (23412) -> body at `actor.rs:16356`:
  - Loop 1 (16424-16480): for every roster entry, `timeout(5 s, provisioner.ensure_runtime_session_state(member_ref) + ensure_mob_comms_drain(...))`. The runtime-backed `ensure_runtime_session_state` impl is `provisioner.rs:9390` (awaits `session_service.comms_runtime` and runtime session state); the default is a no-op (1020).
  - Loop 2 (16482-16500): for every entry, `timeout(5 s, ensure_autonomous_runtime_ready(...))` (14960: comms drain spawn + `ensure_autonomous_dispatch_capability` 15241 with its 40 x 25 ms poll).
  - Both loops are `for` over members, awaited inline in the actor. Bound is per member, so the loop is held for up to 10 s x N. With 17 members and slow members (SDK-hosted continuity loads, provers), 30-90 s of held loop is exactly what HomeCore sees, and the `QueryPhase` probe fires once because nothing else is queued at boot.
  - `progress.awaiting_member(...)` (16426, 16484) reports progress but the command channel is not drained while the loop runs.
- Spawn is already two-phase: `MobCommand::Spawn` (21285) -> `enqueue_spawn` (25538, provisioning off-loop) -> re-enters as `SpawnProvisioned` (21308) -> `handle_spawn_provisioned_batch` (28442) -> `finalize_spawn_from_pending` (29484) -> `finalize_spawn_admit` (pure DSL) and `finalize_spawn_activate` (29542-29543; ~15 awaits to 29861: wiring, pump ensure, autonomous runtime readiness, comms drain) inline and unbounded. So the provisioning cost was moved off the loop, but activation was not.
- MobKit's identity-first materialize drives `handle.ensure_member(...)` / `handle.spawn_spec(...)` (`identity_first/bridge.rs:3146, 3210, 3331`), so every identity materialization pays the inline activation cost on the shared loop.
- Other inline durable work on the loop: `Wire`/`Unwire`/`WireMembersBatch` trust installs (35431-35495, 10 s per side), `Retire`/`Respawn`/`RetireAll` (21795ff), `MemberLiveOpen/Close`, placed/remote turn record commits.

## 3. Verdict: is the serialized actor loop a design mistake?

No. Executing member-local blocking work inside it is. The evidence is in the code itself: the loop already offloads provisioning (`SpawnProvisioned`), the admission reply (`spawn_turn_admission_reply`), turn completion (`spawn_turn_completed_reply`), host status polls (`HostStatusPollCompleted`), kickoff outcomes (`KickoffOutcomeResolved`) and member-live mutations (`member_live_mutation_tasks`, 33541ff), with results re-entering as commands. Every stall reported (OB3, HomeCore boot, the 0.8.30 serial reconciliation, the 2026-08-14 spawned-admission hang) is in an arm that did not get that treatment.

Invariants that genuinely need one global critical section (all pure in-memory, microseconds):
- `MobMachine` DSL authority transitions and fence tokens (`MobMachineMutator::apply`, `SubmitWorkIngressAuthority::from_transition` at `actor.rs:50540-50556`; spawn admission in `finalize_spawn_admit`).
- Roster/topology mutation decisions and pending-spawn slot collisions (`actor.rs:13840-13880`).
- Identity convergence gates (`identity_admission_closed`, 21808), lifecycle phase (`Stop/Complete/Reset/Destroy/Shutdown`, 24404-24412), scope admission (`admit_command_scope`, 24393).
- Continuity generation / lease ownership decisions (who owns a session), but not the I/O executing them.

Invariants that are per member and must leave the critical section: runtime executor readiness, comms drain, event pump lifecycle, session-actor lookup, durable member-live records, placed/remote turn records, trust installs against a peer comms runtime, and anything touching the SDK callback bridge or the continuity store. Per-member delivery ordering needs a per-member FIFO (the runtime's own input queue plus a per-member single-flight), not the global loop.

## 4. Redesign options

### A. Keep one loop; every member-local await becomes non-blocking (recommended)

What changes:
- `SubmitWork`: the loop keeps 50183-50556 (roster read, broken check, DSL apply) and moves `ensure_member_event_pump` (50469), `live_session_actor_registered` (51555) and `ensure_autonomous_runtime_ready` (51666) into the task spawned by `spawn_turn_admission_reply` (51028), before `provisioner.admit_turn`. They are idempotent ensure-steps and already run per member.
- `ensure_pump` (`event_pump.rs:1180`): do not hold `pump_transition` across `task.await`; bound the join (2 s) then detach with a warn.
- `ResumeLifecycle`: `ensure_autonomous_runtimes_from_roster` loops (16424, 16482) become a `JoinSet` over members with the same 5 s per-member bound; each completion re-enters as a `MemberReadinessResolved` command (or reuses the existing progress reporter) so the loop can drain `QueryPhase` and other commands meanwhile.
- `SpawnProvisioned`: keep `finalize_spawn_admit` inline; move `finalize_spawn_activate` into `actor_io_tasks` with its outcome re-entering as a command (mirrors `SpawnProvisioned` itself).
- `send_actor_command`: skip queued commands whose `reply_tx.is_closed()` before executing them (check at `actor.rs:21806` for `SubmitWork`; generalize in `dispatch_command_boxed`); add `submit_work_with_mode_*_bounded` mirroring `drive_resume_actor_operation` (`handle.rs:5150-5170`).
Invariants kept: all authority decisions remain serialized in the loop. Failure isolation: a member's stall lives in its task. Backpressure: one in-flight admission task per member (per-member key in `actor_io_tasks` or a per-member queue); a second `SubmitWork` for the same member parks in that member's FIFO, never in the loop. Ordering per member: the DSL admission order is fixed in the loop; execution order is the runtime input queue plus per-member single-flight. Observability: promote the existing debug lines (21817, 50122) to an inline-step watchdog (warn with elapsed > 2 s and the step name); count parked-per-member depth. Migration risk: medium-low; same pattern as existing arms; runtime side already tolerates admission off-loop. Test: section 6.

### B. Per-member actors plus a thin coordinator

A mob coordinator owns roster/topology/leases/DSL authority behind short critical sections; each member has a task and mailbox. Best isolation and backpressure, but it moves the DSL authority boundary (single-owner `MobMachineMutator` today), touches ~130 `MobCommand` arms (`state.rs:698`), and changes cross-member ordering assumptions in flows and `swarm_integration`. Days to weeks. Follow-on only, after A shows where per-member mailboxes are still insufficient.

### C. Keep serialization, fail fast for degraded members

In the loop, before `handle_submit_work`, check the member's runtime durability health (add `MeerkatMachine::is_durability_ready(session_id)` reading `entry.require_durability_ready()` as at `dispatch_ingress.rs:1096`) and reject with a typed `MobError::MemberReloadRequired { member_id, reason }`. MobKit maps it to a typed `BridgeAdmissionError::ReloadRequired` (never `Mob(String)`), the identity runtime marks the identity `RepairBlocked` and stops admitting until reload. Correct and cheap, but it does not address the boot stalls, wire/retire stalls, and would not have saved OB3 alone if the wedge was `ensure_pump`/comms-drain rather than the accept. Do it, but not instead of A.

### Minimal change that satisfies HomeCore's invariant ("one member's blocking work cannot delay another member's dispatch or the liveness probe")

1. `actor.rs:50469, 51555, 51666`: move the three ensure-steps out of `handle_submit_work` into the `spawn_turn_admission_reply` task.
2. `event_pump.rs:1216-1220`: bound the join and release `pump_transition` before it.
3. `actor.rs:16424-16500`: run both readiness loops concurrently (`JoinSet`), keep the 5 s per-member bound, do not await the JoinSet inline: re-enter completions as commands.
4. `actor.rs:21806`: skip `SubmitWork` whose `reply_tx.is_closed()`; `handle.rs`: bounded `submit_work_*` variant.
5. Option C typed fast rejection.
6. MobKit: circuit breaker keyed on an open `stall_id` (fail sends fast while the loop is stalled), treat `ActorCommandChannelClosed`/`ActorReplyChannelClosed` on the probe as "actor terminated" not "recovered", map the typed error, ship the reload verb (section 5).

Recommendation: 0.8.32 (MobKit) ships item 6 and the verb; 0.8.34 (meerkat) ships items 1-5. Follow-on (0.8.35+): finish A for `finalize_spawn_activate`, `Wire`, `Retire`, `MemberLive*`; consider B only if per-member mailboxes prove insufficient; harden `MachineCleanupTaskSpawner` (either more workers or an assertion that nothing blocks on it) and put a timeout on `stdout_tx.send` in `StdioCallbackBridge::call`.

## 5. Q2 part 2: cold-reload verb

Status: does not exist in MobKit; meerkat has the primitive; partial via retire+send.

meerkat side:
- `MeerkatMachine::recover_or_discard_reload_required_registration_if_current` / `_with_operation_if_current` (`session_management.rs:4939`, `4952`): under the per-session registration transaction, installs a `ReloadRequiredDiscardCoordinator`, consumes the retained `runtime_loop_teardown` slot (5000-5006), spawns `run_reload_required_discard` (5040-5050), which calls `MobSessionService::discard_live_session_actor_after_durability_reload_required` (`meerkat-mob/src/runtime/session_service.rs:1611`; MobKit delegates at `mob_handle_runtime.rs:5247, 6104`). Returns `Discarded | NotDegraded | NotCurrent`.
- Auto-minting: a new executor registration for the same session that hits `ExistingExecutorClaim::ColdReloadDegradedRegistration` runs it and continues ("executor registration minted the cold reload", `session_management.rs:4258-4272`). So a resume/materialize of the same session performs the cold reload from durable truth.
- meerkat-mob calls it only from the retire path: `acquire_quiescent_runtime_turn_finalization_boundary` (`provisioner.rs:2213`) and `cancel_active_runtime_turn_before_retire_with_adapter_until` (`provisioner.rs:2442`), both preserving the ops binding via `retention_request()`.

MobKit side:
- No verb. Greps for `reload_required|cold reload|durability_reload|ReloadRequired|recover_or_discard|repair-blocked|RecoveryRepairBlocked` over src/sdk/docs hit only the passive delegation and comments (`identity_first/adapters.rs:2292, 4226, 4295`).
- Delivery classifier `is_repairable_bridge_delivery_error` (`bridge.rs:66-70`) does not match the repair-blocked text, so the error ends as `IdentityRuntimeError::AdmissionFailed` (`runtime.rs:130-135`) with no heal; `send` only re-materializes `Dormant`/`Uninitialized` identities (`runtime.rs:7501-7519`), and a degraded member stays Active.
- `mobkit/respawn_member` (`http_console.rs:7441`) -> `reset_member_alias_tracked` (`identity_first/runtime.rs:4693`) -> `reset_with_expected_member_alias`: documented "destructive continuity reset" (fence owner, advance generation, fresh continuity). Not a cold reload.
- `mobkit/retire_member` (`http_console.rs:7320`) -> `retire_member_alias_tracked` -> mob retire -> provisioner 2442 -> reload discard; identity becomes Dormant (to verify in `retire`), next `send` re-materializes and the executor registration mints the cold reload. This is today's two-step procedure and it cannot run while the actor loop is wedged (retire is an actor command).

Verb design: `mobkit/reload_member` (RPC in `bin/rpc_gateway.rs` method match and `http_console.rs` dispatch; console action `ACTION_AGENT_RELOAD` next to `ACTION_AGENT_RESPAWN` at `http_console.rs:1771`; Python `runtime.reload_member(identity)` in `sdk/python/meerkat_mobkit/runtime.py`, TS equivalent; document in `docs/api/rpc.mdx` beside `mobkit/respawn_member` at line 328).
- Semantics: non-destructive, same continuity generation, same session id, same identity alias.
- Steps under the identity's `lifecycle_lock_for` (`identity_first/runtime.rs:4829`): (1) resolve identity -> member -> bridge session; refuse if a materialization is in flight; (2) new `MobHandle::reload_member_registration(identity)` in meerkat-mob: quiesce via `cancel_active_runtime_turn_before_retire_with_adapter_until` (`provisioner.rs:2442`, already runs `recover_or_discard_reload_required_registration_with_operation_if_current` with the ops retention request), then re-run executor registration for the same session (the 4258 path mints the reload), then `ensure_autonomous_runtime_ready`; (3) return `{ reloaded: bool, disposition: "discarded"|"not_degraded"|"not_current", session_id, generation }`; `not_degraded` is a success no-op; a `RecoveryRepairBlocked` from the coordinator is a typed error with the reason.
- Make the same routine the automatic reaction to the typed `MemberReloadRequired` in the delivery path (one attempt per delivery, then fail typed), replacing the substring classifier for this case.
- Add `mobkit/member_health` returning `durability: "ready" | { "reload_required": { operation, reason } }` so operators see the state before it costs 600 s.

## 6. Deterministic test plan

Harness (meerkat-mob `tests/` for the actor, meerkat-mobkit `tests/` for the bridge and console):
- Mob with N runtime-backed members (N = 8 for the deterministic test, 48 for load) using `MeerkatMachine::persistent` over a test `RuntimeStore` wrapper and a test `ContinuityStore`/`SessionStore` wrapper with two switches keyed by session id: `fail_commit(session)` makes the committed-boundary commit return `WriteFailed` (drives `mark_durability_reload_required`, `driver/persistent.rs:565`, exactly OB3's path), and `park(session)` blocks that session's store calls on a `tokio::sync::Notify` (hang mode). A third switch `slow_readiness(member, d)` delays `comms_runtime`/`maybe_spawn_mob_comms_drain` for boot tests.
- Wedge member 0 deterministically: run one turn on member 0 with `fail_commit` on so its runtime enters `ReloadRequired`; then set `park` on member 0's pump/session-service path (or, more directly, park `MemberEventPumps::ensure_pump`'s replaced task via a test pump that never exits) so the pre-fix `handle_submit_work` inline step hangs. Assert pre-fix the test fails (probe fires), post-fix it passes.
- Assertions: concurrent `submit_work` to members 1..N-1 all admitted within 2 s wall clock (tight bound, not the 600 s budget); member 0 returns the typed reload-required error within 1 s, not a timeout; `QueryPhase` round trips complete under 1 s for the whole run (reuse `run_actor_loop_probe` with a 5 s budget and assert no `ActorLoopStalled` is emitted); abandoned-command test: drop the caller after 100 ms and assert the actor does not run the turn later (count runtime accepts).
- Per-member ordering: send 3 inputs to member 1 with distinct idempotency keys and assert the runtime input queue observes them in submission order; send interleaved inputs to members 1 and 2 and assert each member's order is preserved while cross-member order is unconstrained.
- Boot variant (HomeCore): 17 members with `slow_readiness(3 s)`; assert `ResumeLifecycle` completes in under 6 s and the probe never fires; pre-fix it takes ~50 s and fires.
- Load variant: 48 members, 200 concurrent deliveries, one member parked; assert p99 admission under 3 s and zero probe pages; assert per-member single-flight depth never exceeds the configured bound.
- Reload verb test: degrade member 0 with `fail_commit`, call `reload_member`, assert `disposition == discarded`, a fresh executor registration exists (same session id, same generation), and the next delivery to member 0 succeeds; call again and assert `not_degraded`.
- Regression: `swarm_integration.rs`, `identity_first_subprocess_reboot.rs`, `unified_console.rs`, `console_experience.rs` unchanged; `admission_timeout_names_the_operation_and_member_and_is_never_repairable` (`bridge.rs:6871`) stays green; new bridge test that `ReloadRequired` is typed and never routed to `repair_member_for_delivery`.
- HomeCore acceptance: run the isolation and boot variants against their production-clone lineage and check the boot log for zero `actor_loop_stalled`.

## 7. Immediate operator guidance (OB3)

- Lower `MOBKIT_BRIDGE_ACTOR_ADMISSION_SECS` for now; enable `meerkat_mob::runtime::actor=debug` to capture the last inline step before the next stall.
- Treat `actor_loop_recovered` as suspect until a real `QueryPhase` succeeds; if sends start failing instantly with `ActorCommandChannelClosed`, the actor terminated and the process must restart.
- Once the loop drains, use `mobkit/retire_member` then send on review:singleton; do not use `respawn_member` (destroys continuity).
