# dloop-mobkit report: MobKit side of meerkat #1102 (OB3 actor stall)

Worktree: /home/luka/src/wt/mobkit-dloop (base origin/main de602d55). Nothing pushed, no PR.
Line numbers below are against the head of the 0834 branch (ac7dc626) unless stated; they are
within a few lines on 6271b8bb because Part 2 only inserted code.

## Branches and commits

| Branch | Head | Commit | Hooks |
|---|---|---|---|
| fix/actor-stall-breaker-reload-verb | 6271b8bb | fix(identity-first): fail deliveries fast on an open actor stall, add reload_member and member_health | ON (pre-commit cargo fmt passed) |
| fix/actor-stall-breaker-reload-verb-0834 (stacked on 6271b8bb) | ac7dc626 | feat(identity-first): route reload_member and member_health through meerkat 0.8.34 primitives | OFF (`git -c core.hooksPath=/dev/null`); body carries "requires meerkat 0.8.34; recommit with hooks after the repin" |

Status now: holding, no cargo/nextest of mine running.

## Part 1 (compiles against meerkat =0.8.33)

### a. Circuit breaker keyed on the open stall_id

- New module `meerkat-mobkit/src/actor_loop_health.rs`: `ActorLoopHealth` (tokio `watch`) with
  `ActorLoopHealthState::{Live, Stalled{stall_id, since}, Terminated{stall_id, detail, at}}`,
  `mark_stalled / mark_recovered(id-correlated) / mark_terminated (terminal)`, `unhealthy()` future,
  and the wire projection `ActorLoopHealthReport {state: live|stalled|terminated|unobserved, stall_id,
  stalled_for_secs, detail}`. Exported at crate root.
- `UnifiedRuntime` owns one (`actor_loop_health` field, accessor `actor_loop_health()`); the probe
  writes it; `install_identity_first_context_authority` installs it into `IdentityRuntime::
  install_actor_loop_health`, which forwards to the bridge via the new trait method
  `SessionBridge::observe_actor_loop_health` (default no-op; `MobSessionBridge` stores it).
- `ActorAdmissionDeadline` (bridge.rs) now carries `Option<Arc<ActorLoopHealth>>`.
  `bound()` (bridge.rs:1220) refuses immediately with typed
  `BridgeError::ActorLoopStalled { operation, identity, stall_id, stalled_for }` when a stall is
  open, races every parked hop against `health.unhealthy()` so a stall opening mid-wait ends the
  wait the instant the probe pages, and passes through unchanged when live. Terminated loop yields
  `BridgeError::ActorTerminated { operation, identity, detail }`. `refusal_for` (bridge.rs:1177)
  logs one WARN per refusal with `stall_id` and `stalled_for_ms` (ERROR for terminated).
- Both are mirrored in `BridgeAdmissionError` and both `From` conversions, and mapped to typed
  `IdentityRuntimeError::ActorLoopStalled { identity, operation, stall_id, stalled_for }` /
  `ActorTerminated` on both send lanes (`admission_phase_error` for AwaitCommit, new
  `ingress_phase_error` for Ingress). Never `Mob(String)`, never routed into
  `repair_member_for_delivery` (the typed arms return before the repairable substring arm).
- Observability of a stall (folded dwatchdog scope): stall open = ERROR line from the default sink
  (`actor_loop_stalled: probe unanswered for Ns: ... stall_id N`), stall close = INFO line
  (`actor_loop_recovered: stall N resolved after Ns`), each refused delivery = WARN with stall_id and
  elapsed, and the typed error Display names `stall_id` and `stalled_for`.

### b. Probe semantics (unified_runtime/mod.rs)

- `actor_terminated_detail` (mod.rs:2500) classifies `MobError::ActorCommandChannelClosed |
  ActorReplyChannelClosed`, and the laundered form meerkat 0.8.33's `MobHandle::status`
  produces (`MobError::Internal("mob actor command channel closed; last actor-published phase is
  Running, ...")`), as termination.
- `run_actor_loop_probe` (mod.rs:2579): on termination (parked or fresh round trip) it emits the new
  `ErrorEvent::ActorLoopTerminated { stall_id: Option<u64>, detail }`, logs at ERROR "the process
  must restart", marks health terminated (so deliveries fail fast with `ActorTerminated`), never
  emits `ActorLoopRecovered`, and breaks (nothing left to watch). Stalls mark health stalled;
  correlated resolutions mark it live.
- `ErrorEvent::ActorLoopTerminated` added to `unified_runtime/types.rs` (Display, doc list). SDK
  `ErrorCategory` mirrors added in Python `types.py` and TS `types.ts` (+ message rendering, +
  their exact-set tests); the `sdk_error_category_parity` gate is green.

### c. mobkit/member_health

- `IdentityRuntime::member_health(&identity) -> MemberHealthReport` (in-process reads only, never
  touches the actor). Fields: `identity, member_id (runtime alias), state (lifecycle wire string),
  bootstrap_state (dormant|warming|active|broken), materialization_in_flight (bootstrap == warming),
  session_id, generation, last_delivery_error {class, detail, at_unix_ms}, actor_loop
  (ActorLoopHealthReport), open_stall_id, durability (Option<MemberDurability>, omitted when None),
  continuity_unrecoverable`.
- `last_delivery_error.class` in `DeliveryErrorClass::{reload_required, actor_loop_stalled,
  actor_terminated, admission_timeout, admission_failed, completion}`; recorded on every failed
  delivery in `send_core`, cleared on success. `detail` is the full error text, so meerkat's
  reload-required reason ("WriteFailed: ...") is visible verbatim.
- `MemberDurability` typed now: `Ready` serializes as `"ready"`, `ReloadRequired { operation,
  reason }` as `{"reload_required": {"operation", "reason"}}`. Part 1 never populates it.
- Wired: console `"mobkit/member_health"` (http_console.rs; `agent.view`; accepts `member_id` or
  `identity`; stale generated alias rejected with `stale_identity_runtime_binding`; added to the
  capabilities method list), RPC `mob_methods::handle_member_health` + rpc.rs dispatch + catalog.

### d. mobkit/reload_member

- `IdentityRuntime::reload_locked` (runtime.rs:8678) under the identity lifecycle lock:
  1. state Active required; Dormant/Uninitialized -> `not_current` (nothing done);
     Broken/Retiring/Suspended -> typed `InvalidState { operation: "reload_member" }`;
  2. `retire_locked_with_intent(identity, LifecycleRetireIntent::Reload)` (runtime.rs:8504): the
     ordinary retire body (fence via lease re-acquire, continuity record re-upsert under the new
     token, `bridge.retire_member`, `bridge.unregister_session_runtime_state`, lease release) with
     the memory harvest hooks (`distill_before_rotation`, `note_identity_retired`) skipped;
  3. explicit `Retiring -> Dormant` (runtime.rs:8786), the only such transition in-process;
  4. `embody_identity_locked` (runtime.rs:4988; new lock-held split of `embody_identity`) resumes
     the SAME continuity record: `bridge.resume_session(..., &record.session_id, ...)` with
     `record.generation`; that resume is the executor registration where meerkat mints the cold
     reload from durable truth;
  5. returns `MemberReloadOutcome { reloaded: true, disposition: Discarded, session_id, generation }`
     (WARN if the resume fell back to a fresh session because meerkat reported the snapshot
     typed-absent).
- Public entries: `reload_member(&identity)` and cancellation-safe
  `reload_member_alias_tracked(&identity, expected_alias: Option<&str>)` (stale generated alias
  rejected under the same lock).
- Wire result: `{reloaded, disposition: "discarded" | "not_degraded" | "not_current", session_id,
  generation, identity, identity_first: true}`.
- Surfaces: console arm `"mobkit/reload_member"` next to `respawn_member` (http_console.rs; action
  mapping `one(ACTION_AGENT_RESPAWN, target)`, mutating list, capabilities list, stale-alias test
  list); RPC `mob_methods::handle_reload_member` + rpc.rs dispatch + catalog; Python
  `MobHandle.reload_member(member_id) -> MemberReloadResult` and `member_health(member_id) ->
  MemberHealth` (new dataclasses in types.py, exported from `meerkat_mobkit`); TS
  `reloadMember(memberId): Promise<MemberReloadResult>` and `memberHealth(memberId):
  Promise<MemberHealth>` (types + parsers in types.ts, exported from index.ts); docs
  `docs/api/rpc.mdx` (table rows + `### mobkit/reload_member` and `### mobkit/member_health`
  sections), `docs/concepts/roster.mdx`, `docs/guides/console.mdx`, `docs/sdks/python.mdx`;
  CHANGELOG `## [Unreleased]` Added (verbs, ActorLoopTerminated) and Fixed (breaker, probe,
  classification).
- Worker-plane members are refused with a typed message (the worker plane has no non-destructive
  reload; `respawn_member` is a destructive reset and is never called by this verb).
- No new access action: reload is strictly less than respawn (same session, same generation) so
  the existing `agent.respawn` grant covers it; a new vocabulary entry would have forced a console
  bundle rebuild.

### e. Delivery error classification

- `is_reload_required_bridge_delivery_error` (bridge.rs:90) matches meerkat 0.8.33's stable
  fragments: `"Runtime recovery is repair-blocked"` (`RuntimeDriverError::RecoveryRepairBlocked`
  Display), `"registration-authorized cold reload is required"` (composed by
  `mark_durability_reload_required`, meerkat-runtime driver/persistent.rs:565-582), and the bare
  token `"RecoveryRepairBlocked"`.
- `classify_submit_mob_error` (bridge.rs:101) types the submit-side `MobError` at the boundary into
  `BridgeError::ReloadRequired { identity, reason }`; every other error keeps the historical
  `Mob(String)` text form the substring repair classifier reads.
- `deliver_admitted_inner` returns `ReloadRequired` (and the stall/terminated refusals and the
  admission timeout) BEFORE the `is_repairable_bridge_delivery_error` arm, so it is never routed
  to `repair_member_for_delivery`.
- `send_core` (both lanes): on typed `IdentityRuntimeError::ReloadRequired` it runs exactly one
  `reload_for_delivery_locked` (-> `reload_locked`, under the lock already held), refreshes the
  fencing token, retries the delivery once; a second refusal returns the typed `ReloadRequired`;
  a failed reload returns `ReloadRequired` with both reasons. Never a loop, never
  `Internal(String)`.
- The pre-existing `admission_timeout_names_the_operation_and_member_and_is_never_repairable`
  stays green (verified in both parts).

### f. Tests (Part 1)

All run with `./scripts/repo-cargo nextest run -p meerkat-mobkit` filters on 6271b8bb.

Lane 1: 82 tests run, 82 passed. New tests among them:

- `actor_loop_health::tests::{starts_live_and_reports_no_stall, stall_then_recovery_round_trips_by_id,
  termination_is_terminal, unhealthy_resolves_immediately_when_already_stalled,
  unhealthy_wakes_when_a_stall_opens_and_parks_while_live, report_wire_shape_is_snake_case_and_minimal}`
- `unified_runtime::tests::{channel_closed_on_parked_probe_is_termination_not_recovery,
  laundered_channel_closed_text_classifies_as_termination,
  channel_closed_on_fresh_probe_terminates_without_a_stall_id,
  health_reads_stalled_while_parked_and_live_after_recovery,
  termination_is_logged_as_an_error_naming_the_restart}` (plus the six pre-existing probe tests,
  updated for the new `health` parameter, all passing)
- `identity_first::bridge::tests::{open_stall_refuses_admission_fast_naming_the_stall,
  stall_opening_mid_wait_cuts_the_wait_short, recovered_loop_admits_again,
  terminated_loop_refuses_with_a_distinct_error, repair_blocked_text_classifies_as_typed_reload_required}`
  (plus pre-existing `admission_timeout_names_the_operation_and_member_and_is_never_repairable`,
  `stalled_actor_fails_typed_instead_of_hanging`, `serialized_round_trips_share_one_budget`)
- `tests/identity_first_runtime.rs::{send_reload_required_runs_one_automatic_reload_then_retries,
  send_reload_required_twice_fails_typed_after_one_reload,
  reload_member_verb_preserves_session_and_generation,
  member_health_reports_open_stall_from_the_shared_verdict}` (fake `CountingBridge` gained a
  `reload_required_times` knob that refuses N deliveries with the typed class)
- `rpc::tests::reload_member_and_member_health_rpcs_use_identity_authority`
- `http_console::tests::console_runtime_identity_reads_reject_stale_runtime_aliases` (list now
  includes `mobkit/member_health` and `mobkit/reload_member`)

Lane 2: 192 tests run, 192 passed: binaries `sdk_error_category_parity`, `identity_first_runtime`,
`sdk_enum_mirror_parity`, `identity_first_completion_bearing_send`.

Clippy: `./scripts/repo-cargo clippy -p meerkat-mobkit --all-targets -- -D warnings` clean.
Python: `PYTHONPATH=sdk/python python3 -m pytest sdk/python/tests/test_member_reload_health.py
sdk/python/tests/test_types.py sdk/python/tests/test_rpc_method_names.py` = 176 passed (new file
`sdk/python/tests/test_member_reload_health.py`, 6 tests). TypeScript: `npm run build` then
`npm test` in sdk/typescript = 729 pass, 0 fail (new `describe` blocks `MobHandle.reloadMember()`
and `MobHandle.memberHealth()` in tests/runtime.test.ts; `ErrorCategory` exact-set test updated).
Bazel: `node scripts/generate-bazel-rust-builds.mjs` and `--check`: no BUILD changes.

## Retire + re-materialize: does it preserve session id and generation?

Answer: yes for the continuity binding, but plain "retire_member then send" is NOT a working
in-process path, so the verb adds one transition.

Evidence (meerkat-mobkit/src/identity_first/runtime.rs, 0834 head):

- `retire_locked` marks `Retiring` (runtime.rs:8511 `mark_lifecycle_in_progress(identity,
  IdentityLifecycleState::Retiring)`) and leaves the entry in `Retiring` after lease release
  (runtime.rs:8628 `Ok(()) => (IdentityLifecycleState::Retiring, None)`). Before my change no code
  path moved an entry from `Retiring` to `Dormant` (the only `state = Dormant` setter,
  runtime.rs:3985, applies to `Active | Suspended` on lease release).
- `embody_identity` refuses `Retiring` (runtime.rs:5087 `InvalidState { operation: "materialize" }`
  in the `Broken | Retiring | Suspended` arm), and `send_core` only materializes
  `Dormant || Uninitialized` (runtime.rs:7680-7681) and then requires `Active`. So after retire a
  send fails typed; the analyst's "retire then send" procedure does not re-materialize.
- The continuity record is untouched by retire: `advance_existing_continuity_fence`
  (runtime.rs:7331) only re-upserts the same record under the new fencing token; nothing rotates
  `session_id` or `generation`. Only `reset_*` advances the generation (doc header runtime.rs:9449
  "2. Advance ContinuityGeneration"), and `mobkit/respawn_member` calls `reset_member_alias_tracked`
  (http_console.rs:7473; rpc/mob_methods.rs same), i.e. it is the destructive reset.
- Re-materialization resumes the SAME session with the SAME generation: `embody_identity_locked`
  calls `bridge.register_session_runtime_state(&record.session_id, identity, record.generation,
  ...)` and `bridge.resume_session(..., &record.session_id, snapshot)` (runtime.rs:5241, 5271,
  5276); `MobSessionBridge::resume_session` builds a `MemberLaunchMode::Resume` spawn spec on that
  session id.
- The mob-plane retire preserves the durable session and a later resume revives it with the same
  id: existing test `identity_first_resume_revives_terminally_retired_runtime`
  (meerkat-mobkit/tests/identity_first_cold_restart_continuity.rs:599) retires on the mob plane
  and asserts `record.session_id == original_session_id` after resume.
- Therefore `reload_locked` = retire (memory hooks skipped) + explicit `Retiring -> Dormant`
  (runtime.rs:8786) + `embody_identity_locked`, all under one lifecycle lock. Tests
  `reload_member_verb_preserves_session_and_generation` and
  `send_reload_required_runs_one_automatic_reload_then_retries` assert `session_id` and
  `generation` unchanged, `create_calls == 0` (never a fresh session), `retire_calls == 1`,
  `resume_calls == 1`. This matches the OB3 data point (fresh executor registration for the same
  session from durable truth recovers the member; the false WriteFailed left durable state intact).

## Part 2 (fix/actor-stall-breaker-reload-verb-0834, ac7dc626)

Built against /home/luka/src/wt/meerkat-dloop (branch fix/mob-actor-member-work-off-loop,
uncommitted working tree at the time; last commit ae0394e65) via a working-tree-only
`[patch.crates-io]` block naming all 35 meerkat crates present in MobKit's Cargo.lock (every
crates.io meerkat crate must be patched together or two `meerkat-core`s collide).

meerkat 0.8.34 API I coded against, verbatim from that worktree:

- meerkat-mob/src/error.rs:509
  `MemberReloadRequired { member_id: AgentIdentity, reason: String }`
  Display: `member {member_id} requires a runtime reload before it can accept work: {reason}`
- meerkat-mob/src/error.rs:530
  `ActorCommandTimedOut { command_kind: &'static str, stage: &'static str }`
- meerkat-mob/src/runtime/handle.rs:2718
  `pub enum MemberReloadDisposition { Discarded, NotDegraded, NotCurrent }` (serde snake_case)
- meerkat-mob/src/runtime/handle.rs:2737
  `pub struct MemberReloadOutcome { pub disposition: MemberReloadDisposition, pub session_id:
  SessionId, pub generation: crate::ids::Generation }` (both re-exported at `meerkat_mob::`;
  `Generation::get() -> u64`, `Generation::INITIAL`)
- meerkat-mob/src/runtime/handle.rs:11179
  `pub async fn reload_member_registration(&self, member: &AgentIdentity)
  -> Result<MemberReloadOutcome, MobError>`
- meerkat-mob/src/runtime/handle.rs:11083
  `pub async fn submit_work_with_mode_bounded(&self, runtime_id: AgentRuntimeId, fence_token:
  FenceToken, work_ref: WorkRef, spec: WorkSpec, handling_mode: HandlingMode, deadline: Instant)
  -> Result<WorkDeliveryReceipt, MobError>` where `Instant` is `meerkat_core::time_compat::Instant`
  (= `std::time::Instant` on native; I pass `tokio::time::Instant::into_std()`)
- meerkat-mob/src/runtime/handle.rs:11111
  `pub async fn submit_work_with_mode_and_delivery_identity_bounded(&self, runtime_id:
  AgentRuntimeId, fence_token: FenceToken, spec: WorkSpec, handling_mode: HandlingMode,
  delivery_identity: MobDeliveryIdentity, deadline: Instant) -> Result<WorkDeliveryReceipt, MobError>`
- meerkat-runtime/src/meerkat_machine/mod.rs:1950
  `pub struct SessionDurabilityReloadRequired { pub operation: String, pub reason: String }`
  (exported from `meerkat_runtime`)
- meerkat-runtime/src/meerkat_machine/mod.rs:5669
  `pub async fn durability_reload_required(&self, session_id: &SessionId)
  -> Option<SessionDurabilityReloadRequired>`
- meerkat-runtime/src/meerkat_machine/mod.rs:5682
  `pub async fn is_durability_ready(&self, session_id: &SessionId) -> bool`
- `MobSessionService::runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>>`
  (unchanged from 0.8.33, meerkat-mob/src/runtime/session_service.rs:1103)

Changes in Part 2:

- bridge.rs `classify_submit_mob_error(member_id, error, deadline)`: matches
  `MobError::MemberReloadRequired { member_id, reason }` first -> `BridgeError::ReloadRequired`;
  `MobError::ActorCommandTimedOut { command_kind, stage }` -> `BridgeError::ActorAdmissionTimeout`
  (waited = deadline elapsed, WARN "the command was NOT executed"); text fallback kept.
- `submit_internal_bridge_work` uses `submit_work_with_mode_bounded` /
  `submit_work_with_mode_and_delivery_identity_bounded` with `deadline.deadline.into_std()`, so an
  abandoned delivery is skipped by the actor instead of running as a ghost turn. The
  completion-bearing `start_work_*` path is unchanged (no bounded variant exists).
- New `SessionBridge` methods with defaults: `reload_member_registration(&self, runtime_id) ->
  Result<Option<BridgeMemberReload>, BridgeError>` (default `Ok(None)` = primitive not exposed) and
  `member_durability(&self, session_id) -> Option<MemberDurability>` (default `None`).
  `BridgeMemberReload { disposition: MemberReloadDisposition, session_id, registration_generation:
  u64 }` is exported from `identity_first`. `MobSessionBridge` implements both: the reload through
  `handle.reload_member_registration(&mid)` under the admission deadline (operation
  `reload.reload_member_registration`), the durability through
  `session_service.runtime_adapter()?.durability_reload_required(session_id)` mapped by
  `member_durability_from_machine` (`None -> Ready`, `Some -> ReloadRequired{operation, reason}`).
- runtime.rs `reload_locked`: after the Active check, if the bridge returns `Some(reload)`:
  `Discarded | NotDegraded` return immediately with `reloaded = (disposition == Discarded)` and the
  identity's own unchanged `session_id`/`generation` (WARN if meerkat's registration session differs
  from the continuity binding); `NotCurrent` logs and falls through to the 0.8.33
  retire-then-rematerialize path. The automatic delivery reload inherits this. Bridge errors map to
  `IdentityRuntimeError::Internal("bridge reload_member_registration: ...")`.
- `member_health.durability` = `bridge.member_durability(&record.session_id)` when a bridge and a
  continuity record exist.
- docs/api/rpc.mdx `durability` row reworded; CHANGELOG Added entry for the 0.8.34 wiring.

### f. Tests (Part 2)

New:
- `identity_first::bridge::tests::typed_member_reload_required_classifies_ahead_of_text`
- `identity_first::bridge::tests::mob_reload_outcome_and_durability_project_into_wire_vocabulary`
- `tests/identity_first_runtime.rs::reload_member_prefers_the_mob_primitive_when_the_bridge_exposes_it`
  (fake `CountingBridge` gained `expose_reload_registration(disposition)` and `set_durability`)
- `tests/identity_first_runtime.rs::member_health_carries_durability_when_the_bridge_observes_it`

Verification against the patched tree: `./scripts/repo-cargo clippy -p meerkat-mobkit
--all-targets -- -D warnings` clean (the only warnings are deprecations inside the patched
meerkat-mcp dependency). Last complete targeted lane (same filter as Part 1 plus
`test(durability) | test(mob_reload_outcome)`): 102 tests run, 101 passed, 1 failed. The failure
was `reload_member_prefers_the_mob_primitive_when_the_bridge_exposes_it`: the fake bridge mints a
fresh session id per `deliver_admitted`, which `send_core` treats as a session rotation, so the
`before.session_id` comparison after the `send` step failed. Fixed by pinning
`bridge.set_deliver_session_id(before.session_id)` at the top of the test. The rerun hit a rustc
ICE (`verify_ich`, incremental cache of the test binary, after overlapping runs); the
non-incremental rerun (`CARGO_INCREMENTAL=0`) was killed by the system for low memory, and the
hold forbids a new run. So exactly ONE test still needs a re-run:
`reload_member_prefers_the_mob_primitive_when_the_bridge_exposes_it` (binary
`identity_first_runtime`). Suggested command after "resume":
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=48 ./scripts/repo-cargo nextest run -j 16 -p meerkat-mobkit
-E 'binary(identity_first_runtime)'` on the 0834 branch with the patch block in the working tree.

## No [patch.crates-io] committed

Before `git commit` on the 0834 branch I ran `git checkout -- Cargo.toml Cargo.lock`, confirmed
`git status --short` listed neither file, then committed. Checks on the committed tree:
`git show HEAD:Cargo.toml | grep -c "patch.crates-io"` = 0; `git show --stat HEAD` lists no
Cargo.toml/Cargo.lock; `git diff 6271b8bb ac7dc626 -- Cargo.toml Cargo.lock | wc -l` = 0 (Part 1
never touched them either; Cargo.lock churn from the patch was 70 deletions, all discarded). The
patch block was then re-applied to the working tree only (`Cargo.toml` shows as modified in
`git status`; saved copy at /tmp/mobkit-dloop-Cargo.toml.patched) so the post-resume rerun can use
it. Cargo.lock in the working tree is currently clean (it re-churns on the next build).

## Left out / caveats

- Console coverage of the new methods is the stale-alias console test plus the RPC-level test; no
  dedicated happy-path console test for `mobkit/member_health` / `mobkit/reload_member`.
- No new access action (`agent.reload`); `reload_member` is gated by `agent.respawn`. Adding a
  vocabulary entry would touch console/src/panels/AccessPanel.tsx and the built bundle.
- Part 1 cannot observe durability, so an Active identity is always reloaded (`discarded`) by the
  verb; `not_degraded` only appears once the 0.8.34 primitive is exposed (Part 2).
- meerkat-mob `NotCurrent` from the primitive on an Active identity falls back to the 0.8.33
  retire-then-rematerialize path rather than failing; documented in code.
- The 0834 branch must be recommitted with hooks after the pin moves to meerkat 0.8.34 (pre-commit
  runs `cargo fmt --all`, which cannot compile against 0.8.33 for that branch).
- Part 2 lane counts above are from the last COMPLETE run; the single fixed test is committed but
  unverified (see f above).

## Update after "resume" (capped runs)

- Part 2 re-run: `CARGO_BUILD_JOBS=32 ./scripts/repo-cargo nextest run -j 8 -p meerkat-mobkit
  -E 'binary(identity_first_runtime)'` on the 0834 branch with the patch block in the working tree,
  after removing only the corrupted incremental dirs (`identity_first_runtime-*`,
  `meerkat_mobkit-*`): 176 tests run, 176 passed, including
  `reload_member_prefers_the_mob_primitive_when_the_bridge_exposes_it`. (The `CARGO_INCREMENTAL=0`
  variant was killed twice by the harness's low-memory heuristic while the host showed 705 GB
  available; the incremental variant completed in the foreground.)
- New console happy-path test on the Part 1 branch:
  `http_console::tests::console_member_health_and_reload_member_happy_path` (member_health for the
  durable identity and its current runtime alias; reload_member discarded with same session id and
  generation 2, identity stays Active; worker-plane alias refused with -32001). Ran together with
  `console_runtime_identity_reads_reject_stale_runtime_aliases`: 2/2 passed.
- Commits now:
  - fix/actor-stall-breaker-reload-verb: 6271b8bb (Part 1) + 0d60f04a `test(console): happy-path
    coverage for mobkit/member_health and mobkit/reload_member` (hooks on, cargo fmt passed).
  - fix/actor-stall-breaker-reload-verb-0834: rebased onto 0d60f04a; head 38ef90c8 (same Part 2
    content, hooks were off at original commit time). `git show 38ef90c8:Cargo.toml | grep -c
    patch.crates-io` = 0; `git diff 0d60f04a 38ef90c8 --stat` = 6 files, no Cargo.toml/Cargo.lock.
- Working tree: on the 0834 branch, clean (patch block removed; saved copy at
  /tmp/mobkit-dloop-Cargo.toml.patched for future 0.8.34 runs).
- No cargo, nextest or build process of mine is running.

## Update: MemberAdmissionBacklogFull mapping (0834 branch)

- Meerkat worker committed at baf557dc8; every signature I coded against matched verbatim (lead
  confirmed). Patch block path unchanged (/home/luka/src/wt/meerkat-dloop/<crate>).
- New commit 68494fdd on fix/actor-stall-breaker-reload-verb-0834 (hooks off; body carries the
  "requires meerkat 0.8.34; recommit with hooks after the repin" line):
  `MobError::MemberAdmissionBacklogFull { member_id, depth }` -> `BridgeError::AdmissionBacklogFull
  { identity, depth }` (classified in `classify_submit_mob_error`, returned by
  `deliver_admitted_inner` before the repairable arm), mirrored in `BridgeAdmissionError` and both
  `From` conversions, mapped on both send lanes to `IdentityRuntimeError::AdmissionBacklogFull
  { identity, depth }` (Display says "retryable once the lane drains"), recorded as
  `DeliveryErrorClass::AdmissionBacklogFull` (`admission_backlog_full`) for member_health. Never
  repair, never reload, never a bare string. docs/api/rpc.mdx class list and CHANGELOG updated.
- Unit test: `identity_first::bridge::tests::member_admission_backlog_full_classifies_typed_and_retryable`.
- Verification against the patched tree (CARGO_BUILD_JOBS=32, -j 8): `cargo fmt`, `clippy -p
  meerkat-mobkit --all-targets -D warnings` clean, nextest lane `test(backlog) | test(reload) |
  test(member_health) | test(admission) | test(repair_blocked) | test(stall) |
  binary(identity_first_runtime) | binary(sdk_error_category_parity)` = 234 run, 234 passed.
- Committed tree check: `git show 68494fdd:Cargo.toml | grep -c patch.crates-io` = 0; `git show
  --stat 68494fdd` = 5 files (CHANGELOG.md, docs/api/rpc.mdx, bridge.rs, runtime.rs, types.rs), no
  Cargo.toml/Cargo.lock. Working tree clean; patch copy remains at /tmp/mobkit-dloop-Cargo.toml.patched.
- Behavioural notes honoured: the automatic reload is exactly one attempt per delivery
  (`reload_attempted` flag in `send_core`, both lanes); if the fresh registration degrades again the
  retry's `ReloadRequired` is returned typed with no further reload, and a failed reload returns
  `ReloadRequired` carrying both reasons. `MemberReloadDisposition::NotDegraded` is treated as a
  success no-op (`reloaded: false`). Placed members: meerkat's `UnsupportedForMode` from
  `reload_member_registration` surfaces as `IdentityRuntimeError::Internal("bridge
  reload_member_registration: ...")` from the verb (typed refusal text, no fallback retire).
- Branch heads now: fix/actor-stall-breaker-reload-verb = 0d60f04a (6271b8bb + console test);
  fix/actor-stall-breaker-reload-verb-0834 = 68494fdd (38ef90c8 + backlog mapping), stacked on 0d60f04a.
- No cargo, nextest or build process of mine is running.

## Update: MemberReloadRefused / MemberReloadTimedOut mappings (0834 branch)

- Built against meerkat e8a6db482 (5 commits, fixups squashed). Signatures used:
  `MobError::MemberReloadRefused { session_id: SessionId, reason: String }`,
  `MobError::MemberReloadTimedOut { session_id: SessionId, stage: &'static str }`; upstream
  `#[non_exhaustive]` on `MemberReloadOutcome`, `MemberAdmissionBacklogSnapshot`,
  `SessionDurabilityReloadRequired` (production code reads named fields only; the bridge test
  builds `MemberReloadOutcome` via serde and tests the degraded mapping through a
  `member_durability_degraded(operation, reason)` helper).
- New commit 7c21f592 on fix/actor-stall-breaker-reload-verb-0834 (hooks off; body carries
  "requires meerkat 0.8.34; recommit with hooks after the repin"; 10 files, no Cargo.toml or
  Cargo.lock; `git show 7c21f592:Cargo.toml | grep -c patch.crates-io` = 0).
  - `classify_reload_mob_error` at the bridge's reload seam: Refused -> `BridgeError::ReloadRefused
    { identity, session_id, reason }` (WARN "store not healthy yet; retry later"), TimedOut ->
    `BridgeError::ReloadTimedOut { identity, session_id, stage }` (ERROR naming the stage). Both
    mirrored in `BridgeAdmissionError` and the `From` impls; mapped in `admission_phase_error`,
    `ingress_phase_error`, and the new `reload_phase_error` to
    `IdentityRuntimeError::ReloadRefused` (Display: "store not healthy, reload refused, retry later";
    retryable) and `IdentityRuntimeError::ReloadTimedOut` (Display names the stage; "not retryable
    without inspection").
  - Automatic one-per-delivery reload: a refused or timed-out reload is returned typed (not folded
    into `ReloadRequired`), recorded as delivery error class `reload_refused` / `reload_timed_out`,
    still exactly one attempt, no retry after a refusal (deliver_calls stays 1).
  - `member_health.last_reload {outcome: discarded | not_degraded | not_current | refused |
    timed_out | failed, detail?, at_unix_ms}`; recorded on every verb/automatic reload; a refusal's
    detail is meerkat's reason verbatim. Python `MemberHealth.last_reload` and TS
    `MemberHealth.lastReload` added (additive). docs/api/rpc.mdx (class list, `last_reload` row,
    reload_member failure paragraph) and CHANGELOG updated.
- Tests: `identity_first::bridge::tests::reload_refused_and_timed_out_classify_typed`;
  `tests/identity_first_runtime.rs::reload_refused_and_timed_out_surface_typed_and_are_recorded`
  (verb refused -> typed + last_reload.refused with reason; automatic path refused -> typed,
  1 deliver call, class reload_refused; verb timed out -> stage recorded; not_degraded recorded);
  Python `test_member_health_carries_last_reload`; TS `lastReload` assertion.
- Verification (detached chain, CARGO_BUILD_JOBS=32, -j 8, meerkat e8a6db482 via the working-tree
  patch): `cargo fmt`; `clippy -p meerkat-mobkit --all-targets -D warnings` clean; nextest lane
  `test(reload) | test(member_health) | test(backlog) | test(admission) | test(repair_blocked) |
  test(stall) | test(durability) | binary(identity_first_runtime) |
  binary(sdk_error_category_parity)` = 240 run, 240 passed. TS SDK `npm run build && npm test` =
  729/729. Python `test_member_reload_health.py` + `test_types.py` = 107 passed.
  (Two runs hit the rustc incremental ICE `verify_ich` after edits; clearing the mobkit crate's
  incremental dirs resolved it. The harness's low-memory watchdog killed two non-detached runs;
  the final chain ran detached with setsid/nohup as the lead allowed.)
- Branch heads now: fix/actor-stall-breaker-reload-verb = 0d60f04a;
  fix/actor-stall-breaker-reload-verb-0834 = 7c21f592 (68494fdd + this commit), stacked on 0d60f04a.
- Working tree clean on 0834 (patch block removed; copy at /tmp/mobkit-dloop-Cargo.toml.patched).
  No cargo, nextest or build process of mine is running.

## Update: 0834 rebased onto the rebased Part 1

- Lead rebased Part 1 onto MobKit main 1d9577e0 (CHANGELOG conflict with #402): new Part 1 head
  754769ae (b69141b5 + 754769ae).
- 0834 rebased with `git -c core.hooksPath=/dev/null rebase --onto 754769ae 0d60f04a` (only the
  three Part 2 commits replayed): new head 1d54237c = 0d721df6 (primitives) -> fc4ab5c2 (backlog)
  -> 1d54237c (reload refused / timed out). Applied cleanly, no conflicts.
- Verified: 0 conflict markers in CHANGELOG, no `[patch.crates-io]` in HEAD Cargo.toml, no
  Cargo.toml/Cargo.lock diff vs 754769ae, 11 files changed vs 754769ae. Working tree clean; no
  cargo running. Branch heads: Part 1 = 754769ae, 0834 = 1d54237c.

## #404: retired identity stuck in Retiring (new branch)

- Worktree /home/luka/src/wt/mobkit-dretire, branch fix/retired-identity-removable off origin/main
  b669d362 (includes Part 1). Head e95f3e90 (hooks ON, pre-commit cargo fmt passed). Not pushed.
- Root cause: `Retiring` is both the transient in-transaction marker (`mark_lifecycle_in_progress`)
  and the retired terminal form `retire_locked` leaves behind (runtime.rs sets
  `state = Retiring` after lease release), and the roster reconciler
  (`reconcile_roster_remove` / `reconcile_roster_replace`), `embody_identity` and `send` treated
  every `Retiring` as in-progress and refused it. Because every lifecycle mutation runs under the
  identity's lifecycle lock, a lock-holder can only observe the terminal form, so the refusal was
  never protecting anything and made a retired identity un-removable and un-resumable forever.
- Design: keep `Retiring` as the retired terminal state (chosen over a `Dormant` end state because
  the console's `retired` health / `RetiredReadable` visibility, `reset_all`'s `LeavingFleet`
  classification (a retired identity must NOT be reset), the implicit-delegate sweeper, and the
  existing lifecycle tests all read post-retire `Retiring`; switching to `Dormant` would have made
  retired identities addressable and reset-able) and make the lock-holding doors accept it like
  `Dormant`: remove releases any lease and drops the entry, replace adopts the new spec, embody
  resumes the recorded session (same session id and generation), send/dispatch re-materialize.
  `reload_locked` shares that door (its explicit `Retiring -> Dormant` flip removed) and reports
  `not_current` for a retired identity. `respawn_member` unchanged.
- Files: meerkat-mobkit/src/identity_first/runtime.rs, tests/identity_first_builder.rs,
  tests/identity_first_runtime.rs, CHANGELOG.md (Unreleased -> Fixed, references #404).
- Tests (new): `identity_first_retired_identity_is_removed_by_topology_refresh` (builder harness;
  retire beta via identity authority, roster.set([alpha]), `refresh_desired_topology()` succeeds
  twice, beta gone, snapshot ready), `retired_identity_send_rematerializes_the_same_session`
  (resume_calls 1, create_calls 0, same session id and generation, Active),
  `retired_identity_profile_change_is_adopted_by_the_reconciler` (no second retire, new profile
  adopted, entry Dormant). Lane (CARGO_BUILD_JOBS=32, -j 8, detached): `binary(identity_first_runtime)
  | binary(identity_first_builder) | test(retire) | test(reload) | test(reconcile) | test(reset_all) |
  test(implicit_delegate) | test(member_health)` = 309 run, 309 passed; `clippy -p meerkat-mobkit
  --all-targets -D warnings` clean; compiles against meerkat =0.8.33 (no patch block).
- Note: `lazy_register_flow` does not drive removals (only `apply_roster_controlled` via
  `refresh_desired_topology` / bootstrap does), which is why the regression lives in the builder
  test file.

### #404 addendum: retired peers and indirect hydration

- Lead datum: on 0.8.31 a hand-off to a retired target logged
  `identity_materialization_failure ... during materialize_reachable_peers: cannot materialize
  identity ... in state Retiring` and the cycle completed (N-1). Decision made explicit: revival
  is ONLY by an operation addressed to the identity (`send`, `dispatch`, `materialize`,
  `reload_member`); indirect hydration (`materialize_reachable_peers` from a peer hand-off, fleet
  `materialize_all`, the background warm) refuses a retired identity typed exactly as before
  (new `retired_terminal_refusal` guard in `best_effort_materialize_identity` and
  `materialize_for_background`), so a hand-off never silently brings a retired member back.
- Commit amended (hooks on, cargo fmt passed): fix/retired-identity-removable head b1159375
  (replaces e95f3e90). 4 files, +398/-31.
- New test `retired_peer_is_refused_typed_by_hand_off_hydration` (a<->b wired, b retired:
  `materialize_reachable_peers(a)` completes without b, b stays Retiring, no resume,
  `IdentityMaterializationFailure { identity: b, initiator: a, operation:
  materialize_reachable_peers, error contains "Retiring"/"materialize" }` paged; `materialize_all`
  skips b; a direct `send(b)` revives it with one resume).
- Lane (CARGO_BUILD_JOBS=32, -j 8, detached): `binary(identity_first_runtime) |
  binary(identity_first_builder) | test(retire) | test(reload) | test(reconcile) | test(reset_all) |
  test(implicit_delegate) | test(member_health) | test(materialize)` = 312 run, 312 passed;
  `clippy -p meerkat-mobkit --all-targets -D warnings` clean; against =0.8.33. CHANGELOG entry
  extended with the explicit-revival sentence. No cargo running.

### #404 ruling applied: explicit send revives, implicit hand-off refuses

- Lead ruling matched the amended design: `send`/`dispatch`/`materialize`/`reload_member` addressed
  to the identity revive it (same session id, same generation); `materialize_reachable_peers`,
  `materialize_all` and the background warm refuse a retired identity typed
  (`IdentityMaterializationFailure`, initiator + operation, "cannot materialize ... in state
  Retiring") and the fan-out completes for the other peers.
- Test (a) `retired_peer_is_refused_typed_by_hand_off_hydration` extended with a third healthy
  peer c (a<->b, a<->c; b retired): b refused typed, c's record returned and c Active, b stays
  Retiring, no resume, failure event paged with initiator a; `materialize_all` skips b; a direct
  `send(b)` revives with one resume. Test (b) `retired_identity_send_rematerializes_the_same_session`
  (same session id and generation). CHANGELOG states which path revives and which refuses.
- Commit amended (hooks on, cargo fmt passed): fix/retired-identity-removable head 9251b52c
  (replaces b1159375). Lane 312/312, clippy --all-targets -D warnings clean, =0.8.33. No cargo running.
