---
title: "Stopped-Phase Revival"
description: "Root-cause design record for the 0.7.19–0.7.23 resume-strand class: machine-owned revival of stopped sessions and cold-load snapshot reconciliation"
icon: "rotate-right"
---

# Stopped-Phase Revival — implementation plan (0.7.24)

Root cause (verified, wf_63cf2cb3-27c + field forensics): the MeerkatMachine session
DSL classifies Stopped as terminal (only exits: UnregisterSessionStopped→Idle teardown,
Destroy→Destroyed), yet both resume seeds deliver registered sessions IN Stopped
(warm: RuntimeExecutorExitedFrom*→Stopped, entry stays registered; cold:
RecoverRuntimeAuthorityStopped Initializing→Stopped from durable snapshot). Every
registration input accepts-and-preserves Stopped (RegisterSessionIdempotent,
PrepareBindingsStopped, EnsureSessionWithExecutorStopped self-loops), so the resume
build proceeds until the first Stopped-intolerant input — field: PublishLocalEndpoint
(per_phase [Idle, Attached, Running], dsl ~20194) via factory.rs:4628
install_peer_comms_on → wedge, background repair retries forever. Five releases of
symptoms are this one missing re-admission arc plus ad-hoc per-input tolerance arms
(HydrateSessionLlmStateStopped comment at dsl:6417-6424 documents the whack-a-mole).

## Fix: machine-owned revival at the two intent-to-use seams

DSL edits in meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs AND its mirror
meerkat-runtime/src/meerkat_machine/dsl.rs (machine-check-drift enforces parity):

1. RegisterSession (~5611) / RegisterSessionIdempotent (~5639): remove Stopped from
   per_phase. Add:
   - RegisterSessionResumesStopped: guard lifecycle_phase==Stopped, same_session
     (self.session_id == Some(session_id)), not_draining (registration_phase !=
     Draining). update: registration_phase=Queuing; runtime_stop_deferred=false.
     PRESERVES session_id, active_runtime_id/fence/generation/epoch, hydrated LLM
     identity/capability surface. to Idle. emit RuntimeNotice{Recover, "stopped
     session re-admitted for resume"}. Semantics: a stopped executor is a fact about
     the previous epoch; a re-registered session with no executor is Idle.
   - RegisterSessionNewBindingFromStopped: guards new_session_binding + not_draining;
     update = RegisterSession's full LLM-state reset block AND clear binding fields
     (active_runtime_id/fence/generation/epoch = None — new tenant, previous
     tenant's binding facts must not leak; verifier amendment). to Idle.
2. EnsureSessionWithExecutorStopped (~9357): empty self-loop → revival mirroring the
   Idle arm: not_draining guard; update registration_phase=Active,
   runtime_stop_deferred=false; to Attached; emit RuntimeNotice{Recover,...}.
   (Fixes schedule delivery + mob re-dispatch with NO shell change — the executor
   claim at mod.rs:1101-1122 then grants.)
3. DELETE PrepareBindingsStopped (~7546); remove Stopped from PrepareBindingsIdempotent
   per_phase (~7409). Post-revival PrepareBindings never legitimately sees Stopped;
   reaching it is a loud GuardRejected (ordering bug), never a silent rebind.
4. DELETE tolerance arms whose purpose was the missing arc: HydrateSessionLlmStateStopped
   (~6425), PublishCommittedVisibleSetStopped (~8252), SetSilentIntentsStopped (~9395);
   remove Stopped from SetModelRoutingBaseline (~6756), StagePersistentFilter (~7361),
   RequestDeferredTools (~7370) admissions. KEEP all Retired arms (Retired resume-build
   is the archive-completion flow, different invariant, out of scope).
   AMENDMENT-5 caution: stage_persistent_filter / request_deferred_tools /
   publish_committed_visible_set are public MeerkatMachine APIs — sweep callers; each
   caller must either be post-revival or handle typed rejection.
5. AMENDMENT-1 (load-bearing): add RetireRequestedFromStopped (Stopped→Retired,
   mirroring RetireRequestedFromIdle guard family ~8315) and DELETE the shell phase
   probes / early-return matches in meerkat-mob/src/runtime/session_service.rs
   (:93-104, :116-128). Registered-Stopped retire currently hits
   RuntimeControlPlane::retire at :105 → Retire has no Stopped arm → guard-reject.
   Sequence with provisioner.rs:950: add Stopped to the ask-21d disposal match ONLY
   after the Retire arm lands.
6. AMENDMENT-2: revival must persist durable lifecycle via machine-emitted typed
   effect (pattern: durability_authority.action == DeleteSnapshot keying the unregister
   persist, session_management.rs:1667-1675). stage_session_dsl_input DISCARDS
   committed effects (dsl_effects.rs:90-99) — route revival effects to the persistence
   channel; persist_current_machine_lifecycle("resume") after revival commit.
   Without it, a revived-but-not-yet-turned session leaves durable Stopped visible
   cross-process (the ask-21d split-state read class).

## Class tests (can't-happen-again battery)

- TOTALITY SWEEP (class-killer): walk canonical_machine_schemas() MeerkatMachine
  transitions; assert the set of inputs admissible in phase Stopped == explicit
  allowlist DERIVED FROM THE DSL (not asserted): registration/revival inputs, ensure,
  BeginUnregisterSessionRetainsSnapshot, drain-feedback trio (conditional on
  Draining), UnregisterSession, Destroy, Retire (new), ResolveRuntimeOpsLifecycle
  Durability, observability self-loops (PublishEvent, ResolveRuntimeCompletionResult*,
  SetPeerIngressContext, Abort/Wait...). NOTE: StopRuntimeExecutor has NO Stopped arm
  today — must NOT appear in allowlist (verifier: sweep red on day one otherwise).
- MACHINE UNITS: revival preserves identity/bindings/LLM state, registration==Queuing;
  ensure revives to Attached+Active; revival refused while Draining (BOTH entry
  orders: Begin at Stopped; Begin at Attached then executor exit → Stopped+Draining);
  new-binding-from-Stopped resets LLM state AND bindings; UnregisterSessionStopped
  still clears; retire-from-Stopped → Retired.
- RUNTIME WARM: prepare_bindings_after_stop_yields_live_machine_and_installs_peer_comms
  (THE field repro: register+ensure+stop via stop_runtime_executor, prepare_bindings,
  assert phase != Stopped, install_generated_peer_comms_on_target succeeds). Assert
  visible_phase != Stopped after revival (projection sync, dsl:10573-10586).
  ensure_after_stop_attaches_new_executor_and_admits_input (schedule/mob re-dispatch
  class: accept_input_with_completion admits, turn completes).
- RUNTIME COLD: durable Stopped snapshot (stop+unregister retains snapshot) → fresh
  machine over same store → prepare_bindings → live phase, epoch cursor continuity,
  full comms build succeeds. Variant: seed snapshot verbatim (old-binary pattern).
- RACE PIN: StopRuntimeExecutor landing between revival and comms install re-enters
  Stopped and fails the build with the SAME "guard rejected ... PublishLocalEndpoint"
  string — correct fail-closed; pin so the old signature is distinguishable from
  regression.
- MOB CLASS: respawn/refresh-resume of stopped member (incl. comms wiring
  AddDirectPeerEndpoint post-revival); archive_of_stopped_member_durably_retires;
  mob-seam routed PrepareBindings against Stopped member → typed rejection surfaced
  (composition.rs:346-358 route has no revival — production safe only because
  prepare_local_session_bindings stages RegisterSession first; pin it).
- STOP-WINDOW GUARDS: stop→cleanup→unregister still retains snapshot from Stopped;
  register_during_drain_rejects_typed then succeeds after drain.

Red-verification: write tests + fix; then temporarily restore RegisterSessionIdempotent
per_phase Stopped + empty EnsureSessionWithExecutorStopped and confirm the warm
runtime test deadlock/fails with the field error string.

## Codegen cascade

make machine-codegen (kernels, specs/machines/meerkat_machine/model.tla, mob-seam
composition artifacts) + cargo xtask protocol-codegen (ulimit -s 65520 on macOS).
Gates: machine-check-drift, machine-authority-docs-gate, verify-machine-poster-coverage,
audit-generated-headers, runtime-authority-bypass, rmat-audit. TLC step bounds may need
adjustment (authority-lane precedent). No contracts wire-type changes.

## Bug B (same release): stale runtime snapshot vs store head — AS IMPLEMENTED

meerkat-session/src/persistent.rs load_authoritative_session_base_with_replay_info
preferred the runtime snapshot whenever present, no freshness comparison; quarantine
only covered snapshot-absent. Field: snapshot froze at 83 msgs, store head 91; resume
loaded 83, save rejected by the append-only guard — permanent wedge.

Fix (machine-owned, mirrors the runtime-projection-rollback region): new
SessionDocumentMachine input `ResolveRuntimeSnapshotReadSource` with THREE typed
observations extracted by the shell —
1. store_head_extends_snapshot: the head strictly extends the snapshot (prefix
   transcript digest equality, the save guard's own continuity proof);
2. store_head_is_runtime_checkpoint: the head row carries the intra-turn
   checkpointer's provenance stamp (uncommitted residue; the rollback region
   converges it at save time — serving it would expose an evicted turn);
3. session_is_live: a live session's snapshot lag is transient (the live runtime
   recommits past it); only a COLD load defers to the head.
Verdict `RuntimeSnapshotReadSourceResolved { read_from_store_head }`: head wins iff
extends && !checkpoint && !live. Shell mirrors, decides nothing.

Pins: test_stale_prefix_runtime_snapshot_defers_to_extending_store_head (red-verified
against the old unconditional snapshot preference),
test_checkpoint_stamped_ahead_store_head_stays_snapshot_served,
test_diverged_runtime_snapshot_stays_authoritative,
test_authoritative_load_ignores_newer_raw_store_projection_when_runtime_exists (live),
and the rewritten cold tail of
test_live_runtime_list_status_and_resume_fail_closed_on_stale_raw_store_metadata —
whose former expectation (resume failing closed against the longer persisted row)
WAS the field wedge, now asserted to resume successfully.
