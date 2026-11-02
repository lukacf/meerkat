---- MODULE materialization_rollback_keep_audit ----
\* Hand-written bounded audit of the Keep disposition: the rollback of a
\* materialization that was refused before it served (a durable tail held for
\* recovery, a failed agent build) keeps every input its registration
\* recovered from the store exactly as the store holds it, and still stops an
\* input admitted while the registration stood. So does the retirement of an
\* executor attachment that never served (a pending attachment's abort, a
\* failed startup's cleanup), through the unserved-attachment unregister,
\* while a retire or an archive that meets such an attachment still stops.
\* The machine grants the keep only to a registration that served no live
\* work: once it admitted an input, waited on a deduplicated resubmission or
\* was recycled, its unregister records a requested Keep as Stop
\* (registration_live_input_seen).
\*
\* This audit keeps the generated MeerkatMachine model unchanged and explores
\* three processes. Each restart is a new process: every generated variable
\* returns to its initial value (AuditFreshMachine, derived from the
\* generated Init on every run by materialization_rollback_keep_audit.sh).
\*
\* Process 1 registers one session, admits A, attaches, stages A into a run,
\* admits B behind that run (every kind and durability) under an idempotency
\* key, and crashes, so A (Staged) and B (Queued) are its durable
\* non-terminal rows. Process 2 registers and recovers each durable row
\* through the generated recovery arms (the durability classifier discards an
\* Ephemeral row, the staged A is queued again, and B's key is registered
\* again). Or its registration finds process 1's run continuable and marks
\* it resumable before any row is normalized (MarkRunResumable): A then
\* stays Staged and bound to that run, and a stop of the marked run
\* (StopCurrentRunForRun, which needs no executor) may be recorded before
\* the materialization is refused, with no runtime loop to settle it. A
\* process 2 that did not mark the run may then serve live work, each at
\* most once: admit a new input C, resolve a resubmission D under B's key
\* as a duplicate of B (D waits on B), or take a host recycle, whose
\* recovery admits every active input to the recovered witness again, C
\* included. Its materialization is refused before it serves: the host
\* rolls the registration back with an unregister that begins with Keep,
\* or, as the control, with Stop. Or the host attaches
\* an executor that never serves (C's admission and D's resolution may land
\* while it is pending), and the unserved-attachment unregister begins with
\* the Keep its abort asks for, or with the Stop of a retire that meets it.
\* C's admission and D's resolution may also land between that begin and
\* the stop. The
\* rollback's stop is one driver-lock transaction, as in the shell: it
\* classifies each non-terminal input before the executor's exit (an input
\* answered Abandon gets the runless carrier) and again after it, ends the
\* inputs answered Abandon as stopped, and commits. Process 3 registers,
\* recovers the durable rows again, attaches and stages each recovered input
\* into a run. Rows recover, and inputs are classified and ended, in one
\* fixed order, so the interleavings of independent rows do not multiply the
\* space. Properties (all invariants):
\*
\*   AuditRollbackVerdictIsAFunction: during the rollback's stop exactly one
\*     verdict arm (Retain, Keep or Abandon) answers each input still to
\*     classify.
\*   AuditStoppedResumableRunInputNotKept: once a stop of the run process 2
\*     marked resumable is recorded, A, the input bound to that run, is
\*     never answered Keep or kept, so no later start resumes the stopped
\*     run.
\*   AuditKeepEndsNoRecoveredInput: under Keep, when process 2 served no
\*     live work, no input it recovered is answered Abandon, gets a runless
\*     carrier or ends stopped, but for A once the run it is bound to was
\*     stopped.
\*   AuditKeepStopsWhatItAdmitted: an input admitted in process 2 is never
\*     answered Keep and never runs in process 3.
\*   AuditKeepOnlyUnderKeep: every Keep answer came from an unregister the
\*     host began with Keep.
\*   AuditKeptInputSurvivesTheRollback: an input the rollback kept is still
\*     non-terminal once the stop committed, so process 3 recovers it.
\*   AuditLiveWorkForfeitsKeep: once process 2 served live work (an
\*     admission, a deduplicated wait or a recycle) before its stop, nothing
\*     is answered Keep.
\*   AuditRetireOverUnservedStops: a retire that meets the unserved
\*     attachment keeps nothing.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the invariants are not vacuous. The runner's
\* --mutants mode seeds thirteen defects into a copy of the generated model
\* and one into a copy of this audit, and requires TLC to refuse each with
\* the invariant it breaks: the machine without the Keep arm (Keep stops
\* like Stop), a Keep arm that ignores the recovery witness (with admissions
\* that leave no live-work mark, which on its own already stops a Keep from
\* reaching an admitted input), an Abandon arm that overlaps Keep, a Keep arm
\* that ignores the disposition, an unregister begin that records a
\* requested Keep after live work, an admission that leaves a recorded Keep
\* standing, an admission, a deduplicated wait or a recycle that does not
\* mark the registration as having served live work, an unserved-attachment
\* unregister that ignores the requested disposition (it always stops, as
\* before the attachment's abort asked to keep), one that records a
\* requested Keep after live work, the machine without the exception for a
\* stopped run marked resumable (its bound input is kept as before), the
\* exception without its stop conjunct (it also ends the bound input of a
\* marked run nobody stopped), and, in this audit, a retire that meets the
\* unserved attachment and requests Keep.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* materialization_rollback_keep_audit.sh. No audit step repeats, so the
\* reachable space is finite: the lane's bound of 50 steps explores all of it
\* (a deeper bound finds no new state; the lane records the distinct-state
\* count here, 32,682 before the marked-run branch), and 44 steps reach the
\* deepest witness. Check that by hand with:
\*   specs/machines/meerkat_machine/materialization_rollback_keep_audit.sh 80
\*
\* Scope limits: the requeue mark, the waiter answers, the runless carriers
\* and the durable rows are shell, so the carrier set here is the shell's rule
\* (the inputs answered Abandon) and a kept row is the recovered row
\* unchanged. A recycle's recovery is modelled by its witness step
\* (RecoverAdmittedInput for every active input); its lifecycle replay
\* changes no verdict input. The unserved-attachment route is driven from
\* Attached only, and a recycle from Idle only: the unserved arm's Idle and
\* Running variants and RecycleFromAttached rest on the machine's ci TLC
\* run. The marked run is process 2's only: process 3 queues a kept bound
\* input again, as a registration that finds no continuable tail does (the
\* requeue-across-stop audit resumes a marked run), and a process 2 that
\* marked the run serves no live work, which would forfeit the keep anyway.
\* The meerkat facade rows pin the shell: a refused restart keeps the
\* stranded input, the dry run still names it, and the next start runs it
\* once; after an admission, a recycle or a deduplicated resubmission the
\* rollback keeps nothing. Machine rows pin the exception's scope: a queued
\* input that only still names the stopped run is kept.
EXTENDS materialization_rollback_keep_fresh

CONSTANTS AuditMaxSteps

VARIABLES
    audit_stage,          \* which part of the three-process run this is
    audit_facts,          \* each admitted input's kind and durability
    audit_disposition,    \* the disposition process 2's unregister began with
    audit_route,          \* how process 2's registration was unregistered
    audit_c_planned,      \* process 2's admission of C was planned
    audit_admitted_p2,    \* inputs admitted while process 2's registration stood
    audit_p2_stop,        \* a stop of the run process 2 marked resumable was recorded
    audit_live_p2,        \* live work process 2 served before its stop opened
    audit_draining_live,  \* some of it landed after the unregister began
    audit_recycle_pending, \* active inputs process 2's recycle has yet to recover
    audit_rerecovered,    \* inputs process 2's recycle recovered again
    audit_pending,        \* inputs the open stop pass still classifies
    audit_pre_verdict,    \* the stop's verdicts before the exit (carriers)
    audit_post_verdict,   \* the stop's verdicts after the exit (cleanup)
    audit_abandoned,      \* Abandon answers the stop's cleanup ended
    audit_kept,           \* inputs the committed stop kept
    audit_carriers,       \* inputs that got the stop's runless carrier
    audit_stopped,        \* inputs the stop's cleanup ended Stopped
    audit_rows,           \* the durable non-terminal rows the process recovers
    audit_recovery,       \* each recovered row's progress through recovery
    audit_recovered_p2,   \* inputs process 2 recovered from the store
    audit_staged_p3       \* inputs staged into a run of process 3

setupVars == <<audit_facts, audit_disposition, audit_route, audit_c_planned, audit_admitted_p2,
               audit_p2_stop>>
liveVars == <<audit_live_p2, audit_draining_live, audit_recycle_pending, audit_rerecovered>>
regionVars == <<audit_pending, audit_pre_verdict, audit_post_verdict, audit_abandoned>>
historyVars == <<audit_kept, audit_carriers, audit_stopped>>
restartVars == <<audit_rows, audit_recovery, audit_recovered_p2, audit_staged_p3>>
auditVars == <<audit_stage, setupVars, liveVars, regionVars, historyVars, restartVars>>

AuditSession == "sessionid_1"
AuditEpoch == "runtimeepochid_1"
AuditA == "input_1"
AuditB == "input_2"
AuditC == "input_3"
\* The resubmission of B: never admitted, it waits on B.
AuditD == "input_4"
\* B's idempotency key.
AuditKey == "key_b"
\* The fixed order rows recover and inputs are classified and ended in. C,
\* the input admitted live, comes first, so a defect that keeps it breaks the
\* property about admitted inputs before any other.
AuditOrder == <<AuditC, AuditA, AuditB>>
AuditInputs == {AuditA, AuditB, AuditC}
AuditRun == "runid_1"
\* The dispositions the shell requests for the unserved-attachment
\* unregister: the attachment's own abort (or a failed startup's cleanup)
\* asks to keep, a retire or an archive that meets it stops.
AbortDisposition == "Keep"
RetireDisposition == "Stop"
AuditKinds == {"Prompt", "ExternalEvent"}
TerminalPhases == {"Consumed", "Superseded", "Coalesced", "Abandoned"}
NoVerdict == [x \in {} |-> "Abandon"]

PhaseOf(i) == IF i \in DOMAIN input_phases THEN input_phases[i] ELSE "Absent"
NonTerminal(i) == i \in DOMAIN input_phases /\ input_phases[i] \notin TerminalPhases
NonTerminalInputs == {i \in AuditInputs : NonTerminal(i)}
Attempts(i) == IF i \in DOMAIN input_attempt_counts THEN input_attempt_counts[i] ELSE 0
Lookup(map, i, default) == IF i \in DOMAIN map THEN map[i] ELSE default
Answered(verdicts, v) == {i \in DOMAIN verdicts : verdicts[i] = v}
\* The first input of S in the fixed order.
FirstOf(S) == AuditOrder[CHOOSE n \in 1..Len(AuditOrder) :
                            /\ AuditOrder[n] \in S
                            /\ \A m \in 1..(n - 1) : AuditOrder[m] \notin S]

\* ---- generated arms, one disjunct per phase ----

RetainArm(i, d, dir, obj) ==
    \/ ClassifyStopInputDispositionRetainInitializing(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainIdle(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainAttached(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainRunning(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainRetired(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainStopped(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainResumableInitializing(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainResumableIdle(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainResumableAttached(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainResumableRunning(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainResumableRetired(i, d, dir, obj)
    \/ ClassifyStopInputDispositionRetainResumableStopped(i, d, dir, obj)

KeepArm(i, d, dir, obj) ==
    \/ ClassifyStopInputDispositionKeepInitializing(i, d, dir, obj)
    \/ ClassifyStopInputDispositionKeepIdle(i, d, dir, obj)
    \/ ClassifyStopInputDispositionKeepAttached(i, d, dir, obj)
    \/ ClassifyStopInputDispositionKeepRunning(i, d, dir, obj)
    \/ ClassifyStopInputDispositionKeepRetired(i, d, dir, obj)
    \/ ClassifyStopInputDispositionKeepStopped(i, d, dir, obj)

AbandonArm(i, d, dir, obj) ==
    \/ ClassifyStopInputDispositionAbandonInitializing(i, d, dir, obj)
    \/ ClassifyStopInputDispositionAbandonIdle(i, d, dir, obj)
    \/ ClassifyStopInputDispositionAbandonAttached(i, d, dir, obj)
    \/ ClassifyStopInputDispositionAbandonRunning(i, d, dir, obj)
    \/ ClassifyStopInputDispositionAbandonRetired(i, d, dir, obj)
    \/ ClassifyStopInputDispositionAbandonStopped(i, d, dir, obj)

AbandonAny(i, reason, n) ==
    \/ AbandonInputIdle(i, reason, n)
    \/ AbandonInputAttached(i, reason, n)
    \/ AbandonInputRunning(i, reason, n)
    \/ AbandonInputRetired(i, reason, n)
    \/ AbandonInputStopped(i, reason, n)

AdmitPlan(i, kind) ==
    \/ ResolveAdmissionPlanDefaultQueueKindIdle(i, None, None, kind, None, "Ordinary", "InTurnEligible", FALSE, None, FALSE, FALSE, FALSE)
    \/ ResolveAdmissionPlanDefaultQueueKindAttached(i, None, None, kind, None, "Ordinary", "InTurnEligible", FALSE, None, FALSE, FALSE, FALSE)
    \/ ResolveAdmissionPlanDefaultQueueKindRunning(i, None, None, kind, None, "Ordinary", "InTurnEligible", FALSE, None, TRUE, FALSE, FALSE)

\* The stop classification of input i under its admission facts. A shell
\* asks durability, directedness and the objective binding; the inputs here
\* are undirected and bound to no objective.
Classify(i, verdicts) ==
    LET f == audit_facts[i] IN
    \/ RetainArm(i, f.durability, FALSE, FALSE) /\ verdicts' = verdicts @@ (i :> "Retain")
    \/ KeepArm(i, f.durability, FALSE, FALSE) /\ verdicts' = verdicts @@ (i :> "Keep")
    \/ AbandonArm(i, f.durability, FALSE, FALSE) /\ verdicts' = verdicts @@ (i :> "Abandon")

AuditUnchanged == UNCHANGED auditVars

\* ---- process 1 ----

\* Scripted start: register, admit A, attach, stage A into a run, admit B
\* behind the run and register B's idempotency key.
AuditSetup ==
    /\ audit_stage = "Setup"
    /\ \/ model_step_count = 0 /\ Initialize /\ AuditUnchanged
       \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {}) /\ AuditUnchanged
       \/ model_step_count = 2 /\ AdmitPlan(AuditA, "Prompt") /\ AuditUnchanged
       \/ model_step_count = 3 /\ QueueAcceptedIdle(AuditA) /\ AuditUnchanged
       \/ model_step_count = 4 /\ EnsureSessionWithExecutorIdle(AuditSession) /\ AuditUnchanged
       \/ model_step_count = 5 /\ PrepareAttached(AuditSession, AuditRun) /\ AuditUnchanged
       \/ model_step_count = 6 /\ StageForRunRunning(AuditA, AuditRun) /\ AuditUnchanged
       \/ model_step_count = 7
          /\ \E kind \in AuditKinds, d \in InputDurabilityKindValues :
                /\ AdmitPlan(AuditB, kind)
                /\ audit_facts' = audit_facts @@ (AuditB :> [kind |-> kind, durability |-> d])
          /\ UNCHANGED <<audit_stage, audit_disposition, audit_route, audit_c_planned,
                         audit_admitted_p2, audit_p2_stop, liveVars, regionVars, historyVars,
                         restartVars>>
       \/ model_step_count = 8 /\ QueueAcceptedRunning(AuditB) /\ AuditUnchanged
       \/ model_step_count = 9
          /\ RegisterAcceptedIdempotencyRunning(AuditB, AuditKey)
          /\ audit_stage' = "P1"
          /\ UNCHANGED <<setupVars, liveVars, regionVars, historyVars, restartVars>>

RowOf(i) ==
    [phase |-> input_phases[i],
     attempts |-> Attempts(i),
     seq |-> Lookup(input_admission_seq, i, 0),
     lane |-> Lookup(input_recovery_lanes, i, "Queue"),
     boundary |-> Lookup(input_runtime_boundary, i, "RunStart"),
     exec |-> Lookup(input_runtime_execution_kind, i, "ContentTurn"),
     is_prompt |-> Lookup(input_is_prompt, i, FALSE),
     key |-> Lookup(input_idempotency_keys, i, "")]

\* A new process: the durable non-terminal rows, every generated variable at
\* its initial value.
Restart(next_stage) ==
    /\ AuditFreshMachine
    /\ audit_rows' = [i \in NonTerminalInputs |-> RowOf(i)]
    /\ audit_recovery' = [i \in NonTerminalInputs |-> "Durability"]
    /\ audit_stage' = next_stage

\* Process 1 crashes with A staged into its run and B queued behind it.
AuditCrashProcess1 ==
    /\ audit_stage = "P1"
    /\ Restart("P2Start")
    /\ UNCHANGED <<setupVars, liveVars, regionVars, historyVars, audit_recovered_p2,
                   audit_staged_p3>>

\* Processes 2 and 3 register under an entry epoch, so a host may recycle
\* the runtime.
StartProcess(start_stage, recover_stage) ==
    /\ audit_stage = start_stage
    /\ \/ Initialize /\ AuditUnchanged
       \/ /\ RegisterSessionIdle(AuditSession, Some(AuditEpoch), {})
          /\ audit_stage' = recover_stage
          /\ UNCHANGED <<setupVars, liveVars, regionVars, historyVars, restartVars>>

AuditStartProcess2 == StartProcess("P2Start", "P2Recover")
AuditStartProcess3 == StartProcess("P3Start", "P3Recover")

RecoveryStep(i, next) ==
    /\ audit_recovery' = [audit_recovery EXCEPT ![i] = next]
    /\ UNCHANGED <<audit_stage, setupVars, liveVars, regionVars, historyVars, audit_rows,
                   audit_recovered_p2, audit_staged_p3>>

Unrecovered == {i \in DOMAIN audit_recovery : audit_recovery[i] \notin {"Recovered", "Discarded"}}

\* Process 2's registration may find process 1's run continuable (the audit
\* takes a round of it as committed before the crash) and mark it resumable
\* before any row is normalized, as the shell's registration does. A,
\* staged into that run, then stays Staged and bound to it.
AuditMarkResumableRun ==
    /\ audit_stage = "P2Recover"
    /\ resumable_run_id = None
    /\ AuditA \in DOMAIN audit_rows
    /\ audit_rows[AuditA].phase = "Staged"
    /\ \A i \in DOMAIN audit_recovery : audit_recovery[i] = "Durability"
    /\ MarkRunResumableIdle(AuditRun)
    /\ AuditUnchanged

\* Cold recovery of each durable non-terminal row through the generated arms,
\* one row at a time in the fixed order: the durability classifier (an
\* Ephemeral row is discarded), the normalization classifier for the row's
\* phase (a Staged row is queued again, unless the run it is staged into is
\* marked resumable: the stored cursor binds it, and it stays Staged), then
\* the admitted input, its lifecycle (Queued, or Staged and bound to the
\* marked run) and, for a row with one, its idempotency key.
AuditRecoverRow ==
    /\ audit_stage \in {"P2Recover", "P3Recover"}
    /\ Unrecovered # {}
    /\ LET i == FirstOf(Unrecovered)
           r == audit_rows[i]
           f == audit_facts[i]
       IN
       \/ /\ audit_recovery[i] = "Durability"
          /\ \/ ClassifyRecoveredInputDurabilityRetainDurableDerivedOrMissingIdle(i, f.durability)
                /\ RecoveryStep(i, "Normalize")
             \/ ClassifyRecoveredInputDurabilityDiscardEphemeralIdle(i, f.durability)
                /\ RecoveryStep(i, "Discarded")
       \/ /\ audit_recovery[i] = "Normalize"
          /\ \/ /\ r.phase = "Queued"
                /\ NormalizeRecoveredInputQueuedIdle(i, r.phase, None, FALSE)
                /\ RecoveryStep(i, "Admit")
             \/ /\ r.phase = "Staged"
                /\ resumable_run_id = None
                /\ NormalizeRecoveredInputStagedIdle(i, r.phase, None, FALSE)
                /\ RecoveryStep(i, "Admit")
             \/ /\ r.phase = "Staged"
                /\ resumable_run_id = Some(AuditRun)
                /\ NormalizeRecoveredInputStagedResumableIdle(i, r.phase, None, TRUE)
                /\ RecoveryStep(i, "AdmitBound")
       \/ /\ audit_recovery[i] \in {"Admit", "AdmitBound"}
          /\ RecoverAdmittedInputIdle(i, f.kind, r.boundary, r.exec, None, r.lane)
          /\ RecoveryStep(i, IF audit_recovery[i] = "Admit" THEN "Lifecycle"
                                                           ELSE "LifecycleBound")
       \/ /\ audit_recovery[i] = "Lifecycle"
          /\ RecoverInputLifecycleIdle(i, "Queued", None, None, None, None, 0,
                r.attempts, None, None, Some(r.seq), None, Some(r.lane),
                Some(r.lane), Some(r.boundary), Some(r.exec), None, r.is_prompt)
          /\ RecoveryStep(i, IF r.key = "" THEN "Recovered" ELSE "Key")
       \* A row the marked run binds stays Staged, associated with its run.
       \/ /\ audit_recovery[i] = "LifecycleBound"
          /\ RecoverInputLifecycleIdle(i, "Staged", None, None, None, None, 0,
                r.attempts, Some(AuditRun), None, Some(r.seq), None, Some(r.lane),
                None, None, None, None, FALSE)
          /\ RecoveryStep(i, IF r.key = "" THEN "Recovered" ELSE "Key")
       \/ /\ audit_recovery[i] = "Key"
          /\ RegisterAcceptedIdempotencyIdle(i, r.key)
          /\ RecoveryStep(i, "Recovered")

RecoveredRows == {i \in DOMAIN audit_recovery : audit_recovery[i] = "Recovered"}

AuditRecoveryDone ==
    /\ audit_stage \in {"P2Recover", "P3Recover"}
    /\ Unrecovered = {}
    /\ audit_stage' = IF audit_stage = "P2Recover" THEN "P2Ready" ELSE "P3Serve"
    /\ audit_recovered_p2' = IF audit_stage = "P2Recover" THEN RecoveredRows
                                                          ELSE audit_recovered_p2
    /\ UNCHANGED vars
    /\ UNCHANGED <<setupVars, liveVars, regionVars, historyVars, audit_rows, audit_recovery,
                   audit_staged_p3>>

\* ---- process 2: refused before it serves ----

\* Live work landing at stage s: before the unregister began, or between its
\* begin and its stop.
LiveWork(kind) ==
    /\ audit_live_p2' = audit_live_p2 \cup {kind}
    /\ audit_draining_live' = (audit_draining_live \/ audit_stage = "P2Draining")

\* A caller plans C while the registration stands (an executor attachment
\* may be pending), and admits it then or once the unregister began (once).
\* Live work, here and below, comes only where no run is marked resumable.
AuditAdmitInProcess2 ==
    /\ resumable_run_id = None
    /\ \/ /\ audit_stage \in {"P2Ready", "P2Attached"}
          /\ ~audit_c_planned
          /\ AdmitPlan(AuditC, "Prompt")
          /\ audit_c_planned' = TRUE
          /\ UNCHANGED <<audit_stage, audit_facts, audit_disposition, audit_route,
                         audit_admitted_p2, audit_p2_stop, liveVars, regionVars, historyVars,
                         restartVars>>
       \/ /\ audit_stage \in {"P2Ready", "P2Attached", "P2Draining"}
          /\ audit_c_planned
          /\ audit_admitted_p2 = {}
          /\ QueueAcceptedIdle(AuditC) \/ QueueAcceptedAttached(AuditC)
          /\ audit_admitted_p2' = {AuditC}
          /\ LiveWork("Admit")
          /\ UNCHANGED <<audit_stage, audit_facts, audit_disposition, audit_route,
                         audit_c_planned, audit_p2_stop, audit_recycle_pending,
                         audit_rerecovered, regionVars, historyVars, restartVars>>

\* A caller resubmits B under its key: the machine resolves it as a
\* duplicate of B, which it then waits on (once).
AuditDedupeInProcess2 ==
    /\ audit_stage \in {"P2Ready", "P2Attached", "P2Draining"}
    /\ resumable_run_id = None
    /\ "Dedupe" \notin audit_live_p2
    /\ \/ ResolveAdmissionIdempotencyDuplicateIdle(AuditD, Some(AuditKey))
       \/ ResolveAdmissionIdempotencyDuplicateAttached(AuditD, Some(AuditKey))
    /\ LiveWork("Dedupe")
    /\ UNCHANGED <<audit_stage, setupVars, audit_recycle_pending, audit_rerecovered,
                   regionVars, historyVars, restartVars>>

\* A host recycles the runtime while the registration stands (once). Its
\* recovery admits every active input to the recovered witness again, one
\* at a time in the fixed order.
AuditRecycleInProcess2 ==
    /\ audit_stage = "P2Ready"
    /\ resumable_run_id = None
    /\ "Recycle" \notin audit_live_p2
    /\ RecycleFromIdleOrRetired
    /\ audit_stage' = "P2Recycle"
    /\ audit_recycle_pending' = NonTerminalInputs
    /\ LiveWork("Recycle")
    /\ UNCHANGED <<setupVars, audit_rerecovered, regionVars, historyVars, restartVars>>

AuditRecycleRecover ==
    /\ audit_stage = "P2Recycle"
    /\ audit_recycle_pending # {}
    /\ LET i == FirstOf(audit_recycle_pending)
           r == RowOf(i)
       IN
       /\ RecoverAdmittedInputIdle(i, audit_facts[i].kind, r.boundary, r.exec, None, r.lane)
       /\ audit_recycle_pending' = audit_recycle_pending \ {i}
       /\ audit_rerecovered' = audit_rerecovered \cup {i}
    /\ UNCHANGED <<audit_stage, setupVars, audit_live_p2, audit_draining_live, regionVars,
                   historyVars, restartVars>>

AuditRecycleDone ==
    /\ audit_stage = "P2Recycle"
    /\ audit_recycle_pending = {}
    /\ audit_stage' = "P2Ready"
    /\ UNCHANGED vars
    /\ UNCHANGED <<setupVars, liveVars, regionVars, historyVars, restartVars>>

\* A stop of the run process 2 marked resumable (turn/stop_run reaches any
\* registered session): the machine records it on the run, Idle or with an
\* executor attached that never serves, while the registration stands
\* (once). No runtime loop settles it before the materialization is refused.
AuditStopResumableRun ==
    /\ audit_stage \in {"P2Ready", "P2Attached"}
    /\ ~audit_p2_stop
    /\ \/ StopCurrentRunForRunResumableIdle(AuditRun)
       \/ StopCurrentRunForRunResumableAttached(AuditRun)
    /\ audit_p2_stop' = TRUE
    /\ UNCHANGED <<audit_stage, audit_facts, audit_disposition, audit_route, audit_c_planned,
                   audit_admitted_p2, liveVars, regionVars, historyVars, restartVars>>

\* The materialization is refused: the host rolls its registration back with
\* an unregister that begins with Keep, or, as the control, with Stop. The
\* registration never attached an executor, so its runtime is Idle.
AuditBeginRollback ==
    /\ audit_stage = "P2Ready"
    /\ \E disp \in {"Keep", "Stop"} :
          /\ BeginUnregisterSessionIdle(AuditSession, None, None, None, Some(AuditEpoch), disp, 0)
          /\ audit_disposition' = disp
    /\ audit_route' = "Rollback"
    /\ audit_stage' = "P2Draining"
    /\ UNCHANGED <<audit_facts, audit_c_planned, audit_admitted_p2, audit_p2_stop, liveVars,
                   regionVars, historyVars, restartVars>>

\* Or the host attaches an executor that never serves: the attachment is
\* pending (its startup failed or its owner has not committed it).
AuditAttachUnserved ==
    /\ audit_stage = "P2Ready"
    /\ EnsureSessionWithExecutorIdle(AuditSession)
    /\ audit_stage' = "P2Attached"
    /\ UNCHANGED <<setupVars, liveVars, regionVars, historyVars, restartVars>>

\* The unserved-attachment unregister: the attachment's abort (or its failed
\* startup's cleanup) asks to keep, a retire or an archive that meets the
\* pending attachment stops.
AuditBeginUnservedUnregister ==
    /\ audit_stage = "P2Attached"
    /\ \E route \in {"UnservedAbort", "UnservedRetire"} :
          LET disp == IF route = "UnservedAbort" THEN AbortDisposition ELSE RetireDisposition IN
          /\ BeginUnregisterUnservedAttachmentAttached(AuditSession, None, None, None,
                Some(AuditEpoch), disp)
          /\ audit_disposition' = disp
          /\ audit_route' = route
    /\ audit_stage' = "P2Draining"
    /\ UNCHANGED <<audit_facts, audit_c_planned, audit_admitted_p2, audit_p2_stop, liveVars,
                   regionVars, historyVars, restartVars>>

\* The rollback's stop transaction. The open step is the shell taking the
\* driver lock.
AuditOpenStop ==
    /\ audit_stage = "P2Draining"
    /\ audit_stage' = "PreExit"
    /\ audit_pending' = NonTerminalInputs
    /\ audit_pre_verdict' = NoVerdict
    /\ audit_post_verdict' = NoVerdict
    /\ audit_abandoned' = {}
    /\ UNCHANGED vars
    /\ UNCHANGED <<setupVars, liveVars, historyVars, restartVars>>

\* Before the exit: the inputs answered Abandon get the runless carrier.
AuditClassifyForCarriers ==
    /\ audit_stage = "PreExit"
    /\ audit_pending # {}
    /\ LET i == FirstOf(audit_pending) IN
          /\ Classify(i, audit_pre_verdict)
          /\ audit_pending' = audit_pending \ {i}
          /\ audit_carriers' = IF audit_pre_verdict'[i] = "Abandon"
                                  THEN audit_carriers \cup {i} ELSE audit_carriers
    /\ UNCHANGED <<audit_stage, setupVars, liveVars, audit_post_verdict, audit_abandoned,
                   audit_kept, audit_stopped, restartVars>>

AuditStopExit ==
    /\ audit_stage = "PreExit"
    /\ audit_pending = {}
    /\ RuntimeExecutorExitedFromIdle \/ RuntimeExecutorExitedFromAttached
    /\ audit_stage' = "PostExit"
    /\ audit_pending' = NonTerminalInputs
    /\ UNCHANGED <<setupVars, liveVars, audit_pre_verdict, audit_post_verdict, audit_abandoned,
                   historyVars, restartVars>>

\* After the exit: the cleanup classifies again and ends the Abandon set.
AuditClassifyForCleanup ==
    /\ audit_stage = "PostExit"
    /\ audit_pending # {}
    /\ LET i == FirstOf(audit_pending) IN
          /\ Classify(i, audit_post_verdict)
          /\ audit_pending' = audit_pending \ {i}
    /\ UNCHANGED <<audit_stage, setupVars, liveVars, audit_pre_verdict, audit_abandoned,
                   historyVars, restartVars>>

ToEnd == Answered(audit_post_verdict, "Abandon") \ audit_abandoned

AuditStopAbandon ==
    /\ audit_stage = "PostExit"
    /\ audit_pending = {}
    /\ ToEnd # {}
    /\ LET i == FirstOf(ToEnd) IN
          /\ AbandonAny(i, "Stopped", Attempts(i))
          /\ audit_abandoned' = audit_abandoned \cup {i}
          /\ audit_stopped' = audit_stopped \cup {i}
    /\ UNCHANGED <<audit_stage, setupVars, liveVars, audit_pending, audit_pre_verdict,
                   audit_post_verdict, audit_kept, audit_carriers, restartVars>>

AuditCommitStop ==
    /\ audit_stage = "PostExit"
    /\ audit_pending = {}
    /\ ToEnd = {}
    /\ audit_stage' = "P2Stopped"
    /\ audit_kept' = Answered(audit_post_verdict, "Keep")
    /\ UNCHANGED vars
    /\ UNCHANGED <<setupVars, liveVars, regionVars, audit_carriers, audit_stopped, restartVars>>

\* The unregister drain may complete before the next process starts.
AuditUnregisterDrain ==
    /\ audit_stage = "P2Stopped"
    /\ \/ RuntimeLoopStoppedForUnregisterStopped(AuditSession, FALSE)
       \/ CommsDrainExitedForUnregisterStopped(AuditSession, FALSE)
       \/ CompletionWaitersResolvedForUnregisterStopped(AuditSession)
       \/ UnregisterSessionStopped(AuditSession, None, None, None, Some(AuditEpoch))
    /\ AuditUnchanged

\* ---- process 3: a later start ----

AuditRestartProcess3 ==
    /\ audit_stage = "P2Stopped"
    /\ Restart("P3Start")
    /\ UNCHANGED <<setupVars, liveVars, regionVars, historyVars, audit_recovered_p2,
                   audit_staged_p3>>

AuditServe ==
    /\ audit_stage = "P3Serve"
    /\ \/ EnsureSessionWithExecutorIdle(AuditSession) /\ AuditUnchanged
       \/ PrepareAttached(AuditSession, AuditRun) /\ AuditUnchanged
       \/ \E i \in AuditInputs :
             /\ StageForRunRunning(i, AuditRun)
             /\ audit_staged_p3' = audit_staged_p3 \cup {i}
             /\ UNCHANGED <<audit_stage, setupVars, liveVars, regionVars, historyVars,
                            audit_rows, audit_recovery, audit_recovered_p2>>

AuditNext ==
    \/ AuditSetup
    \/ AuditCrashProcess1
    \/ AuditStartProcess2
    \/ AuditMarkResumableRun
    \/ AuditRecoverRow
    \/ AuditRecoveryDone
    \/ AuditAdmitInProcess2
    \/ AuditDedupeInProcess2
    \/ AuditRecycleInProcess2
    \/ AuditRecycleRecover
    \/ AuditRecycleDone
    \/ AuditStopResumableRun
    \/ AuditBeginRollback
    \/ AuditAttachUnserved
    \/ AuditBeginUnservedUnregister
    \/ AuditOpenStop
    \/ AuditClassifyForCarriers
    \/ AuditStopExit
    \/ AuditClassifyForCleanup
    \/ AuditStopAbandon
    \/ AuditCommitStop
    \/ AuditUnregisterDrain
    \/ AuditRestartProcess3
    \/ AuditStartProcess3
    \/ AuditServe

AuditInit ==
    /\ Init
    /\ audit_stage = "Setup"
    /\ audit_facts = (AuditA :> [kind |-> "Prompt", durability |-> "Durable"])
                     @@ (AuditC :> [kind |-> "Prompt", durability |-> "Durable"])
    /\ audit_disposition = "None"
    /\ audit_route = "None"
    /\ audit_c_planned = FALSE
    /\ audit_admitted_p2 = {}
    /\ audit_p2_stop = FALSE
    /\ audit_live_p2 = {}
    /\ audit_draining_live = FALSE
    /\ audit_recycle_pending = {}
    /\ audit_rerecovered = {}
    /\ audit_pending = {}
    /\ audit_pre_verdict = NoVerdict
    /\ audit_post_verdict = NoVerdict
    /\ audit_abandoned = {}
    /\ audit_kept = {}
    /\ audit_carriers = {}
    /\ audit_stopped = {}
    /\ audit_rows = [x \in {} |-> 0]
    /\ audit_recovery = [x \in {} |-> "Durability"]
    /\ audit_recovered_p2 = {}
    /\ audit_staged_p3 = {}

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* ---- properties ----

StopStages == {"PreExit", "PostExit"}
OpenAnswers(v) == Answered(audit_pre_verdict, v) \cup Answered(audit_post_verdict, v)

AuditRollbackVerdictIsAFunction ==
    audit_stage \in StopStages =>
        \A i \in audit_pending :
            LET d == audit_facts[i].durability
                r == IF ENABLED RetainArm(i, d, FALSE, FALSE) THEN 1 ELSE 0
                k == IF ENABLED KeepArm(i, d, FALSE, FALSE) THEN 1 ELSE 0
                a == IF ENABLED AbandonArm(i, d, FALSE, FALSE) THEN 1 ELSE 0
            IN r + k + a = 1

\* A stop of the run process 2 marked resumable ends A, the input bound to
\* it, under Keep too.
StoppedRunInputs == IF audit_p2_stop THEN {AuditA} ELSE {}

AuditStoppedResumableRunInputNotKept ==
    audit_p2_stop => AuditA \notin OpenAnswers("Keep") \cup audit_kept

AuditKeepEndsNoRecoveredInput ==
    (audit_disposition = "Keep" /\ audit_live_p2 = {}) =>
        LET recovered == audit_recovered_p2 \ StoppedRunInputs IN
        /\ recovered \cap OpenAnswers("Abandon") = {}
        /\ recovered \cap audit_carriers = {}
        /\ recovered \cap audit_stopped = {}

AuditKeepStopsWhatItAdmitted ==
    /\ audit_admitted_p2 \cap (OpenAnswers("Keep") \cup audit_kept) = {}
    /\ audit_admitted_p2 \cap audit_staged_p3 = {}

AuditKeepOnlyUnderKeep ==
    OpenAnswers("Keep") \cup audit_kept # {} => audit_disposition = "Keep"

AuditKeptInputSurvivesTheRollback ==
    audit_stage = "P2Stopped" => audit_kept \subseteq NonTerminalInputs

AuditLiveWorkForfeitsKeep ==
    audit_live_p2 # {} => OpenAnswers("Keep") \cup audit_kept = {}

AuditRetireOverUnservedStops ==
    audit_route = "UnservedRetire" => OpenAnswers("Keep") \cup audit_kept = {}

\* ---- reachability goals, each checked as an expected violation ----

\* B, queued behind process 1's run and recovered by process 2, is kept by
\* the Keep rollback and runs in process 3.
AuditWitnessKeptQueuedInputRuns ==
    /\ audit_disposition = "Keep"
    /\ AuditB \in audit_kept
    /\ AuditB \in audit_staged_p3
\* A, staged into process 1's run and queued again by process 2's recovery,
\* is kept by the Keep rollback and runs in process 3.
AuditWitnessKeptRequeuedInputRuns ==
    /\ audit_disposition = "Keep"
    /\ AuditA \in audit_kept
    /\ AuditA \in audit_staged_p3
\* C, admitted while process 2's registration stood, gets its carrier and
\* ends Stopped under Keep.
AuditWitnessAdmittedInputStoppedUnderKeep ==
    /\ audit_disposition = "Keep"
    /\ AuditC \in audit_stopped
    /\ AuditC \in audit_carriers
\* The control: a Stop rollback ends the recovered B.
AuditWitnessStopRollbackEndsRecoveredInput ==
    /\ audit_disposition = "Stop"
    /\ AuditB \in audit_stopped
\* C, admitted and then recovered again by a host recycle, so it sits in the
\* recovered witness, still ends Stopped under a requested Keep.
AuditWitnessRecycledAdmittedInputStoppedUnderKeep ==
    /\ audit_disposition = "Keep"
    /\ AuditC \in audit_rerecovered
    /\ AuditC \in audit_stopped
\* A resubmission waiting on B is the only live work, and the Keep rollback
\* still ends B Stopped.
AuditWitnessDedupedInputStoppedUnderKeep ==
    /\ audit_disposition = "Keep"
    /\ audit_live_p2 = {"Dedupe"}
    /\ AuditB \in audit_stopped
\* Live work that lands after a Keep unregister began turns the recorded
\* Keep into Stop: the recovered B ends Stopped.
AuditWitnessDrainingLiveWorkStopsRecoveredInput ==
    /\ audit_disposition = "Keep"
    /\ audit_draining_live
    /\ AuditB \in audit_stopped

\* B, recovered by process 2, is kept by the abort of an attachment that
\* never served and runs in process 3.
AuditWitnessUnservedAbortKeepsRecoveredInput ==
    /\ audit_route = "UnservedAbort"
    /\ AuditB \in audit_kept
    /\ AuditB \in audit_staged_p3
\* The control: a retire that meets the pending attachment ends the
\* recovered B.
AuditWitnessUnservedRetireStopsRecoveredInput ==
    /\ audit_route = "UnservedRetire"
    /\ AuditB \in audit_stopped
\* Live work before the abort's unregister began (C admitted while the
\* attachment was pending, for one) turns its Keep into Stop: the recovered
\* B ends Stopped.
AuditWitnessUnservedAbortAfterLiveWorkStops ==
    /\ audit_route = "UnservedAbort"
    /\ audit_live_p2 # {}
    /\ ~audit_draining_live
    /\ AuditB \in audit_stopped

\* Process 2 marks process 1's run resumable and its stop is recorded: the
\* Keep rollback (or the unserved attachment's abort) ends A, bound to the
\* stopped run, as stopped, and still keeps the recovered B.
AuditWitnessStoppedResumableRunEndsUnderKeep ==
    /\ audit_p2_stop
    /\ audit_disposition = "Keep"
    /\ audit_live_p2 = {}
    /\ AuditA \in audit_stopped
    /\ AuditB \in audit_kept

NotAuditWitnessKeptQueuedInputRuns == ~AuditWitnessKeptQueuedInputRuns
NotAuditWitnessKeptRequeuedInputRuns == ~AuditWitnessKeptRequeuedInputRuns
NotAuditWitnessAdmittedInputStoppedUnderKeep == ~AuditWitnessAdmittedInputStoppedUnderKeep
NotAuditWitnessStopRollbackEndsRecoveredInput == ~AuditWitnessStopRollbackEndsRecoveredInput
NotAuditWitnessRecycledAdmittedInputStoppedUnderKeep ==
    ~AuditWitnessRecycledAdmittedInputStoppedUnderKeep
NotAuditWitnessDedupedInputStoppedUnderKeep == ~AuditWitnessDedupedInputStoppedUnderKeep
NotAuditWitnessDrainingLiveWorkStopsRecoveredInput ==
    ~AuditWitnessDrainingLiveWorkStopsRecoveredInput
NotAuditWitnessUnservedAbortKeepsRecoveredInput ==
    ~AuditWitnessUnservedAbortKeepsRecoveredInput
NotAuditWitnessUnservedRetireStopsRecoveredInput ==
    ~AuditWitnessUnservedRetireStopsRecoveredInput
NotAuditWitnessUnservedAbortAfterLiveWorkStops ==
    ~AuditWitnessUnservedAbortAfterLiveWorkStops
NotAuditWitnessStoppedResumableRunEndsUnderKeep ==
    ~AuditWitnessStoppedResumableRunEndsUnderKeep
====
