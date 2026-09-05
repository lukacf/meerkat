# Architecture review: #1102 actor-loop redesign (received via teammate message, 2026-09-05 01:48Z)
Branch fix/mob-actor-member-work-off-loop, worktree /home/luka/src/wt/meerkat-dloop. Reviewer: architecture teammate (Meerkat Rust Zealot). Read-only.

## Verdict: APPROVE (grudgingly). Two medium findings to fix in the next train, none merge-blocking.

## Q1 - Authority boundary
Respected, with one exception. Every MobMachine transition stays on the loop: SubmitWork apply in handle_submit_work; StartupMarkReady at actor.rs:17509; revival at actor.rs:22668-22683 (ReviveMemberLiveMaterialization re-enters and re-resolves roster, broken state, and session binding at 16053-16066); lane settlement at 15872. Detached tasks (run_member_turn_admission 15914, spawn_member_readiness_tasks 17396, spawn_member_registration_reload 16114) perform I/O only and re-enter as commands.
Exception: spawn_member_registration_reload decides off-loop to session_service.discard_live_session (actor.rs:16148) based on two non-atomic reads (session_has_no_executor_registration then has_live_session, 16140-16147). Live-session lifecycle decision, not machine state; TOCTOU outside the loop. Finding 2.

## Q2 - Idempotency of re-entry commands against stale generations
- ResumeLifecycleReadinessResolved: fenced by ticket via take_if (actor.rs:17668-17678); stale fan-outs dropped with warn. StartupMarkReady is fence-token gated (17512), so retired/respawned members fail typed in the DSL. Correct, but the typed failure is folded into first_error and fails the whole Resume (finding 5).
- ReviveMemberLiveMaterialization: MemberNotFound on retire (16053-16059), ensure_member_not_broken (16060), rejects if member_ref.bridge_session_id() differs (16062-16066). Covered.
- MemberTurnAdmissionSettled: lane.inflight != Some(ticket) guard (15876). Correct.

## Q3 - Backpressure unit and ordering
Per-member single-flight lane (MemberAdmissionLane, actor.rs:2061) is the right mob-side unit: bounds parked deliveries per member (MEMBER_ADMISSION_LANE_CAPACITY, handle.rs:2755), costs the loop nothing, single-flight keeps each member's deliveries in DSL-admission order; cross-member order unconstrained, matching the runtime's per-session queue. Caveat: lane capacity and caller-liveness are checked after DSL admission (finding 1). submit_work_*_bounded (handle.rs:11083, 11111) via send_actor_command_until (5303) reserves channel capacity under the deadline and bounds the reply; abandoned caller skipped (15888, 15926); test actor_isolation.rs:1013.

## Q4 - Surfaces-as-skins / runtime-owns-lifecycle
Respected. Mob calls provisioner.reload_degraded_runtime_registration and the runtime's durability_reload_required read (meerkat-runtime/src/meerkat_machine/mod.rs:5670); runtime gained a read-only projection (SessionDurabilityReloadRequired, mod.rs:1951). meerkat-core untouched.

## Q5 - Duplicate functionality
Clear. durability_reload_required projects RuntimeSessionEntry::require_durability_ready (same gate as dispatch_ingress.rs:1096). MemberReloadDisposition mirrors ReloadRequiredRegistrationDisposition in mob vocabulary. send_actor_command_until reuses the drive_resume_actor_operation shape.

## Idioms (non-blocking)
- MEMBER_RETIRE_TOTAL_TIMEOUT reused as the reload deadline (actor.rs:16128): give it its own const.
- Ticket spaces are bare u64: prefer MemberAdmissionTicket(u64) / ResumeLifecycleTicket(u64).
- SessionDurabilityReloadRequired stringifies a typed operation (Display projection).

## Conflict assessment and ranked findings
(remainder requested separately; see below when appended)

## Conflict assessment (received 01:49Z)
- No symbol collisions with meerkat-d1090 or meerkat-dflake (durability_reload_required, is_durability_ready, SessionDurabilityReloadRequired).
- Textual risk low: dloop's insertion sits above test_install_session_peer_comms_handle_on_runtime (dloop mod.rs:5695 vs siblings 5642/5643).
- d1090 (stale-discard-observability): dloop's off-loop discard heuristic (actor.rs:16140-16152) and d1090's registration-witness stale-discard target the same state. After d1090 lands, dloop should use the witness seam instead of two racy reads. No pre-emption.
- dflake (teardown-grace-1104): until-terminal stop/unregister and retirement disposal change provisioner internals that reload_degraded_runtime_registration builds on. Semantic rebase risk; re-run actor_isolation after rebasing.

## Ranked findings
1. [medium, non-blocking] Lane capacity and caller-liveness checked AFTER DSL admission: SubmitWork apply on loop, then enqueue_member_turn_admission (actor.rs:23203, 23227) may reject BacklogFull (15806-15818) or skip an abandoned entry (15888, 15926); machine recorded an unrealized ingress. Run the pure capacity check before the DSL apply.
2. [medium, non-blocking] Reload bypasses the member lane and discards off-loop on a TOCTOU read: spawn_member_registration_reload (actor.rs:16127-16196) runs concurrently with the member's delivery task; discard_live_session (16148) follows two non-atomic reads (16140-16147). Route reload through the member lane; use the d1090 witness seam.
3. [medium, non-blocking, contracts follow-up] MemberReloadRequired maps to ErrorCode::SessionBusy (error.rs:1401-1404) and TargetBusy (1494-1495) while JSON says retryable: false (1322). Needs a distinct ErrorCode plus regen-schemas.
4. [low] ensure_autonomous_runtimes_from_roster still awaits its JoinSet inline (actor.rs:17312-17323, caller 25462); bounded and concurrent now, but the comment at 17300 claiming the loop keeps draining is false.
5. [low] Member retired during Resume fan-out fails the whole Resume: apply_member_readiness_outcomes (actor.rs:17477-17526) folds the fence-mismatch StartupMarkReady error into first_error. Filter outcomes no longer in roster or with a stale fence; log, do not fail.

VERDICT: APPROVE
