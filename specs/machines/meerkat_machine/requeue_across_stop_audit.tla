---- MODULE requeue_across_stop_audit ----
\* Hand-written bounded audit of keeping queued inputs across a graceful stop.
\*
\* An unregister records the disposition it began with for inputs still
\* queued behind the running turn (BeginUnregisterSession's queued_inputs:
\* Stop, the default, or Requeue with a max age). Every runtime stop is one
\* driver-lock transaction: the inputs the generated
\* ClassifyStopInputDisposition answers Abandon get the runless
\* RuntimeTerminated carrier, the executor exits (RuntimeExecutorExited*),
\* and the stop cleanup ends exactly the inputs answered Abandon with
\* AbandonInput(Stopped). An input answered Retain stays Queued with a
\* durable requeue mark and runs on the next start, unless it is older than
\* its max age: then the start's pre-serve settlement ends it with the
\* generated AbandonInput(RequeueExpired) before the runtime can serve.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and explores
\* two processes. Process 1 registers one session, admits an input A and runs
\* it, then admits an input B behind that run with every combination of kind
\* (Prompt, ExternalEvent), durability, directedness and objective binding (a
\* member's kickoff carries its objective id). It begins an unregister with
\* either disposition, or none at all, and cancels A's run (the run cancel
\* ends A Abandoned(Cancelled)), leaves it running, or, under Requeue,
\* suspends it (SuspendRunForStop: the run stops being current and is marked
\* resumable, so the stop keeps A Staged and bound to it), then stops the
\* runtime. The stop classifies each non-terminal input twice, as the shell
\* does: once for the carriers before the exit and once for the cleanup after
\* it (the shell refuses a stop whose two answers differ). After the stop it
\* explores a replayed stop transaction (terminalize_async_stop_once runs
\* again when no runless carrier was committed), an unregister begun only
\* after a plain stop, and the unregister drain. The restart is a NEW
\* process: every generated variable returns to its initial value
\* (AuditFreshMachine, derived from the generated Init on every run by
\* requeue_across_stop_audit.sh), and each durable non-terminal row is
\* recovered through the generated recovery arms (the durability and
\* normalization classifiers, RecoverAdmittedInput, RecoverInputLifecycle).
\* A suspended run is marked resumable when process 2 registers, before any
\* row is normalized (the audit takes its tail as committed: a round of the
\* run committed before the stop), and the generated normalization verdict
\* keeps A Staged and bound to it. Then each marked row is settled before
\* the serving gate, with the shell's expired observation either value (an
\* expired suspended run first ends its mark with SettleResumableRun), the
\* executor attaches and the runtime loop runs: a run marked resumable
\* starts under its own id and resumes (Prepare, then ResumeRun). Properties
\* (all invariants):
\*
\*   AuditRetainedNeverRunBeforeRestart (P1): no queued input answered
\*     Retain was ever staged into a run of process 1, and it stays Queued
\*     until the restart; the suspended run's input, staged before the stop,
\*     stays Staged.
\*   AuditSuspendedRunNeverRestaged: the suspended run's input is never
\*     staged again: process 2 resumes the run it stays bound to.
\*   AuditSuspendedRunNotTerminal: a run marked resumable never has a
\*     published turn terminal, so a run that completed, failed or was
\*     cancelled is never suspended.
\*   AuditRetainedGetsNoCarrier (P2): no input a stop kept is in the carrier
\*     set of any stop transaction of process 1, the first or a replay.
\*   AuditStoppedInputGetsItsCarrier (P2, converse): every input a stop
\*     ended Stopped got its carrier.
\*   AuditOneRunAcrossStopAndRestart (P3): once a stop has committed, every
\*     input was staged into at most one run across both processes: the
\*     cancelled A never runs again, and a kept B runs once, after the
\*     restart.
\*   AuditExpiredNeverRuns (P4): an input the pre-serve settlement ended
\*     RequeueExpired is never staged, and no kept input is staged before
\*     its settlement.
\*   AuditRetainOnlyUnderRequeue (P5): every Retain answer came from an
\*     unregister the host began with Requeue. Since every input gets
\*     exactly one answer (AuditStopVerdictIsAFunction), under Stop and on a
\*     stop outside an unregister every input is answered Abandon: today's
\*     blanket abandon.
\*   AuditRetainOnlyEligible: a kept input is a Queue-lane Prompt, durable
\*     and not directed, by the facts its admission recorded. (The
\*     suspended run's arm keeps any input bound to that run; A is a durable
\*     undirected prompt, so this property says nothing about that arm.)
\*   AuditRetainNeverObjectiveBound: a kept input is never objective-bound,
\*     so a kickoff the stopping process records failed is never run later.
\*   AuditStopVerdictIsAFunction: during a stop transaction exactly one of
\*     the two verdict arms is enabled for each input still to classify.
\*   AuditRequeuedWaitResolvesInEveryPhase: in every state of both
\*     processes but Destroyed, ResolveRuntimeCompletionWaitFailure answers
\*     exactly the RequeuedAcrossStop observation: it has no session guard.
\*   AuditRequeuedWaitAnswerChangesNothing (action property): the stopping
\*     process answering a kept input's waiter through that arm changes no
\*     state but the step count.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the invariants are not vacuous. The runner's
\* --mutants mode seeds seventeen defects, at least one per invariant, into a
\* copy of the generated model (or, for the serve-before-settle control, the
\* audit's settlement order) and requires TLC to refuse each with the
\* invariant it breaks.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* requeue_across_stop_audit.sh. No audit step repeats without bound (the run
\* is cancelled once, a stop is replayed once), so the reachable space is
\* finite: the lane's bound of 36 steps explores all of it (a deeper bound
\* finds no new state), and 32 steps reach the deepest witness. Check that by
\* hand with:
\*   specs/machines/meerkat_machine/requeue_across_stop_audit.sh 60
\*
\* Scope limits: the requeue mark, the waiter answers, the runless carriers
\* and the expiry clock are shell, so the carrier set here is the shell's
\* rule (the inputs answered Abandon), and the meerkat-runtime and
\* meerkat-mob rows pin the shell. The machine does not gate serving on the
\* expiry settlement (the shell runs it once, before the executor attachment
\* is returned), so P4's ordering half holds by that shell order; the
\* runner's serve-before-settle control shows it is not vacuous.
EXTENDS requeue_across_stop_fresh

CONSTANTS AuditMaxSteps, AuditSettleBeforeServe

VARIABLES
    audit_stage,           \* which part of the two-process run this is
    audit_facts,           \* each admitted input's kind, durability, directedness, objective
    audit_disposition,     \* the disposition the host began its unregister with
    audit_attempts,        \* stop transactions committed in process 1
    audit_pending,         \* inputs the open stop pass still classifies
    audit_pre_verdict,     \* the open stop's verdicts before the exit (carriers)
    audit_post_verdict,    \* the open stop's verdicts after the exit (cleanup)
    audit_abandoned,       \* Abandon answers the open stop's cleanup ended
    audit_retained,        \* inputs a committed stop kept
    audit_replay_retained, \* inputs a committed replayed stop kept again
    audit_carriers,        \* inputs that got a stop's runless carrier
    audit_stopped,         \* inputs a stop's cleanup ended Stopped
    audit_unregistered,    \* process 1's unregister completed (a graceful end)
    audit_wait_answered,   \* the stopping process answered a kept input's waiter
    audit_staged_p1,       \* inputs staged into a run of process 1
    audit_staged_p2,       \* inputs staged into a run of process 2
    audit_runs,            \* runs each input was staged into, both processes
    audit_rows,            \* the durable non-terminal rows process 2 recovers
    audit_recovery,        \* each recovered row's progress through recovery
    audit_settled,         \* kept rows the pre-serve settlement handled
    audit_expired,         \* kept rows the settlement ended RequeueExpired
    audit_suspended,       \* process 1's requeue stop suspended A's run
    audit_resumed          \* process 2 resumed the suspended run

setupVars == <<audit_facts, audit_disposition, audit_attempts>>
regionVars == <<audit_pending, audit_pre_verdict, audit_post_verdict, audit_abandoned>>
historyVars == <<audit_retained, audit_replay_retained, audit_carriers, audit_stopped,
                 audit_unregistered>>
runVars == <<audit_staged_p1, audit_staged_p2, audit_runs>>
restartVars == <<audit_rows, audit_recovery, audit_settled, audit_expired>>
resumeVars == <<audit_suspended, audit_resumed>>
auditVars == <<audit_stage, setupVars, regionVars, historyVars, runVars, restartVars,
              audit_wait_answered, resumeVars>>

AuditSession == "sessionid_1"
AuditA == "input_1"
AuditB == "input_2"
AuditInputs == {AuditA, AuditB}
AuditRun == "runid_1"
AuditMaxAgeMs == 900000
AuditKinds == {"Prompt", "ExternalEvent"}
TerminalPhases == {"Consumed", "Superseded", "Coalesced", "Abandoned"}
NoVerdict == [x \in {} |-> "Abandon"]

PhaseOf(i) == IF i \in DOMAIN input_phases THEN input_phases[i] ELSE "Absent"
NonTerminal(i) == i \in DOMAIN input_phases /\ input_phases[i] \notin TerminalPhases
NonTerminalInputs == {i \in AuditInputs : NonTerminal(i)}
Attempts(i) == IF i \in DOMAIN input_attempt_counts THEN input_attempt_counts[i] ELSE 0
Lookup(map, i, default) == IF i \in DOMAIN map THEN map[i] ELSE default
Answered(verdicts, v) == {i \in DOMAIN verdicts : verdicts[i] = v}

\* ---- generated arms, one disjunct per phase ----

\* The Retain verdict has two arms per phase: an eligible queued prompt, and
\* an input bound to a run marked resumable (here, the in-flight run a
\* requeue stop suspended).
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

ExitAny ==
    \/ RuntimeExecutorExitedFromIdle
    \/ RuntimeExecutorExitedFromAttached
    \/ RuntimeExecutorExitedFromRunning
    \/ RuntimeExecutorExitedFromRetired
    \/ RuntimeExecutorExitedFromStopped

BeginUnregisterAny(disp, age) ==
    \/ BeginUnregisterSessionIdle(AuditSession, None, None, None, None, disp, age)
    \/ BeginUnregisterSessionAttached(AuditSession, None, None, None, None, disp, age)
    \/ BeginUnregisterSessionRunning(AuditSession, None, None, None, None, disp, age)
    \/ BeginUnregisterSessionRetainsSnapshotStopped(AuditSession, None, None, None, None, disp, age)

ResolveWaitAny(sid, failure) ==
    \/ ResolveRuntimeCompletionWaitFailureRequeuedAcrossStopInitializing(sid, failure)
    \/ ResolveRuntimeCompletionWaitFailureRequeuedAcrossStopIdle(sid, failure)
    \/ ResolveRuntimeCompletionWaitFailureRequeuedAcrossStopAttached(sid, failure)
    \/ ResolveRuntimeCompletionWaitFailureRequeuedAcrossStopRunning(sid, failure)
    \/ ResolveRuntimeCompletionWaitFailureRequeuedAcrossStopRetired(sid, failure)
    \/ ResolveRuntimeCompletionWaitFailureRequeuedAcrossStopStopped(sid, failure)

AdmitPlan(i, kind) ==
    \/ ResolveAdmissionPlanDefaultQueueKindIdle(i, None, None, kind, None, "Ordinary", "InTurnEligible", FALSE, None, FALSE, FALSE, FALSE)
    \/ ResolveAdmissionPlanDefaultQueueKindRunning(i, None, None, kind, None, "Ordinary", "InTurnEligible", FALSE, None, TRUE, FALSE, FALSE)

\* The stop classification of input i under its admission facts.
Classify(i, verdicts) ==
    LET f == audit_facts[i] IN
    \/ RetainArm(i, f.durability, f.directed, f.objective_bound) /\ verdicts' = verdicts @@ (i :> "Retain")
    \/ AbandonArm(i, f.durability, f.directed, f.objective_bound) /\ verdicts' = verdicts @@ (i :> "Abandon")

KeepAudit == UNCHANGED auditVars

\* ---- process 1 ----

\* Scripted start: register, admit A, attach, run A, admit B behind the run.
AuditSetup ==
    /\ audit_stage = "Setup"
    /\ \/ model_step_count = 0 /\ Initialize /\ KeepAudit
       \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {}) /\ KeepAudit
       \/ model_step_count = 2 /\ AdmitPlan(AuditA, "Prompt") /\ KeepAudit
       \/ model_step_count = 3 /\ QueueAcceptedIdle(AuditA) /\ KeepAudit
       \/ model_step_count = 4 /\ EnsureSessionWithExecutorIdle(AuditSession) /\ KeepAudit
       \/ model_step_count = 5 /\ PrepareAttached(AuditSession, AuditRun) /\ KeepAudit
       \/ model_step_count = 6
          /\ StageForRunRunning(AuditA, AuditRun)
          /\ audit_staged_p1' = {AuditA}
          /\ audit_runs' = [audit_runs EXCEPT ![AuditA] = @ + 1]
          /\ UNCHANGED <<audit_stage, setupVars, regionVars, historyVars, audit_staged_p2,
                         restartVars,
                   audit_wait_answered, resumeVars>>
       \/ model_step_count = 7
          /\ \E kind \in AuditKinds, d \in InputDurabilityKindValues, dir \in BOOLEAN,
                obj \in BOOLEAN :
                /\ AdmitPlan(AuditB, kind)
                /\ audit_facts' = audit_facts @@ (AuditB :> [kind |-> kind, durability |-> d,
                                                             directed |-> dir,
                                                             objective_bound |-> obj])
          /\ UNCHANGED <<audit_stage, audit_disposition, audit_attempts, regionVars,
                         historyVars, runVars, restartVars,
                   audit_wait_answered, resumeVars>>
       \/ model_step_count = 8
          /\ QueueAcceptedRunning(AuditB)
          /\ audit_stage' = "P1"
          /\ UNCHANGED <<setupVars, regionVars, historyVars, runVars, restartVars,
                   audit_wait_answered, resumeVars>>

\* The host begins its unregister, before the stop or after a plain stop.
AuditBeginUnregister ==
    /\ audit_stage \in {"P1", "P1Stopped"}
    /\ audit_disposition = "None"
    /\ \E disp \in UnregisterQueuedInputsDispositionValues :
          /\ BeginUnregisterAny(disp, IF disp = "Requeue" THEN AuditMaxAgeMs ELSE 0)
          /\ audit_disposition' = disp
    /\ UNCHANGED <<audit_stage, audit_facts, audit_attempts, regionVars, historyVars,
                   runVars, restartVars,
                   audit_wait_answered, resumeVars>>

\* The run cancel: A's run ends cancelled and its staged input ends
\* Abandoned(Cancelled).
AuditCancelRun ==
    /\ audit_stage = "P1"
    \* The generated RunCancelled may repeat; the run is cancelled once.
    /\ \/ terminal_outcome # Some("Cancelled") /\ RunCancelled(AuditRun)
       \/ CancelRunningToAttached(AuditRun)
       \/ /\ PhaseOf(AuditA) = "Staged"
          /\ terminal_outcome = Some("Cancelled")
          /\ turn_terminal_run_id = Some(AuditRun)
          /\ AbandonAny(AuditA, "Cancelled", Attempts(AuditA))
    /\ KeepAudit

\* The graceful requeue stop of a durable session suspends the in-flight run
\* instead of cancelling it, before the stop's first classification. A's run
\* stops being current and is marked resumable, so the stop keeps A Staged
\* and bound to it (ClassifyStopInputDispositionRetainResumable).
AuditSuspendRun ==
    /\ audit_stage = "P1"
    /\ ~audit_suspended
    /\ SuspendRunForStopRunningToAttached(AuditRun, TRUE)
    /\ audit_suspended' = TRUE
    /\ UNCHANGED <<audit_stage, setupVars, regionVars, historyVars, runVars, restartVars,
                   audit_resumed, audit_wait_answered>>

\* A stop transaction: the first stop of the runtime, or a replay of it from
\* Stopped (once). The open step is the shell taking the driver lock.
AuditOpenStop ==
    /\ \/ audit_stage = "P1" /\ phase \in {"Idle", "Attached", "Running", "Retired"}
       \/ audit_stage = "P1Stopped" /\ phase = "Stopped" /\ audit_attempts = 1
    /\ audit_stage' = "PreExit"
    /\ audit_pending' = NonTerminalInputs
    /\ audit_pre_verdict' = NoVerdict
    /\ audit_post_verdict' = NoVerdict
    /\ audit_abandoned' = {}
    /\ UNCHANGED vars
    /\ UNCHANGED <<setupVars, historyVars, runVars, restartVars,
                   audit_wait_answered, resumeVars>>

\* Before the exit: the inputs answered Abandon get the runless carrier.
AuditClassifyForCarriers ==
    /\ audit_stage = "PreExit"
    /\ \E i \in audit_pending :
          /\ Classify(i, audit_pre_verdict)
          /\ audit_pending' = audit_pending \ {i}
          /\ audit_carriers' = IF audit_pre_verdict'[i] = "Abandon"
                                  THEN audit_carriers \cup {i} ELSE audit_carriers
    /\ UNCHANGED <<audit_stage, setupVars, audit_post_verdict, audit_abandoned,
                   audit_retained, audit_replay_retained, audit_stopped,
                   audit_unregistered, runVars, restartVars,
                   audit_wait_answered, resumeVars>>

AuditStopExit ==
    /\ audit_stage = "PreExit"
    /\ audit_pending = {}
    /\ ExitAny
    /\ audit_stage' = "PostExit"
    /\ audit_pending' = NonTerminalInputs
    /\ UNCHANGED <<setupVars, audit_pre_verdict, audit_post_verdict, audit_abandoned,
                   historyVars, runVars, restartVars,
                   audit_wait_answered, resumeVars>>

\* After the exit: the cleanup classifies again and ends the Abandon set.
AuditClassifyForCleanup ==
    /\ audit_stage = "PostExit"
    /\ \E i \in audit_pending :
          /\ Classify(i, audit_post_verdict)
          /\ audit_pending' = audit_pending \ {i}
    /\ UNCHANGED <<audit_stage, setupVars, audit_pre_verdict, audit_abandoned,
                   historyVars, runVars, restartVars,
                   audit_wait_answered, resumeVars>>

AuditStopAbandon ==
    /\ audit_stage = "PostExit"
    /\ \E i \in Answered(audit_post_verdict, "Abandon") \ audit_abandoned :
          /\ AbandonAny(i, "Stopped", Attempts(i))
          /\ audit_abandoned' = audit_abandoned \cup {i}
          /\ audit_stopped' = audit_stopped \cup {i}
    /\ UNCHANGED <<audit_stage, setupVars, audit_pending, audit_pre_verdict,
                   audit_post_verdict, audit_retained, audit_replay_retained,
                   audit_carriers, audit_unregistered, runVars, restartVars,
                   audit_wait_answered, resumeVars>>

AuditCommitStop ==
    /\ audit_stage = "PostExit"
    /\ audit_pending = {}
    /\ Answered(audit_post_verdict, "Abandon") \subseteq audit_abandoned
    /\ audit_stage' = "P1Stopped"
    /\ audit_retained' = audit_retained \cup Answered(audit_post_verdict, "Retain")
    /\ audit_replay_retained' =
          IF audit_attempts > 0
          THEN audit_replay_retained \cup Answered(audit_post_verdict, "Retain")
          ELSE audit_replay_retained
    /\ audit_attempts' = audit_attempts + 1
    /\ UNCHANGED vars
    /\ UNCHANGED <<audit_facts, audit_disposition, regionVars, audit_carriers,
                   audit_stopped, audit_unregistered, runVars, restartVars,
                   audit_wait_answered, resumeVars>>

AuditUnregisterDrain ==
    /\ audit_stage = "P1Stopped"
    /\ \/ /\ \/ RuntimeLoopStoppedForUnregisterStopped(AuditSession, FALSE)
             \/ CommsDrainExitedForUnregisterStopped(AuditSession, FALSE)
             \/ CompletionWaitersResolvedForUnregisterStopped(AuditSession)
          /\ KeepAudit
       \/ /\ UnregisterSessionStopped(AuditSession, None, None, None, None)
          /\ audit_unregistered' = TRUE
          /\ UNCHANGED <<audit_stage, setupVars, regionVars, audit_retained,
                         audit_replay_retained, audit_carriers, audit_stopped, runVars,
                         restartVars,
                   audit_wait_answered, resumeVars>>

\* The stopping process answers a kept input's waiter from the generated
\* wait-failure class (once here; the arm repeats freely).
AuditAnswerRequeuedWait ==
    /\ audit_stage = "P1Stopped"
    /\ audit_retained # {}
    /\ ~audit_wait_answered
    /\ ResolveWaitAny(AuditSession, "RequeuedAcrossStop")
    /\ audit_wait_answered' = TRUE
    /\ UNCHANGED <<audit_stage, setupVars, regionVars, historyVars, runVars, restartVars,
                   resumeVars>>

\* ---- the restart: a new process ----

RowOf(i) ==
    [phase |-> input_phases[i],
     attempts |-> Attempts(i),
     seq |-> Lookup(input_admission_seq, i, 0),
     lane |-> Lookup(input_recovery_lanes, i, "Queue"),
     boundary |-> Lookup(input_runtime_boundary, i, "RunStart"),
     exec |-> Lookup(input_runtime_execution_kind, i, "ContentTurn"),
     is_prompt |-> Lookup(input_is_prompt, i, FALSE)]

\* After a committed stop, a graceful end (after UnregisterSession) or a
\* crash at any later point: the new process sees the durable rows.
AuditRestart ==
    /\ audit_stage = "P1Stopped"
    /\ AuditFreshMachine
    /\ audit_rows' = [i \in NonTerminalInputs |-> RowOf(i)]
    /\ audit_recovery' = [i \in NonTerminalInputs |-> "Durability"]
    /\ audit_stage' = "P2Start"
    /\ UNCHANGED <<setupVars, regionVars, historyVars, runVars, audit_settled,
                   audit_expired,
                   audit_wait_answered, resumeVars>>

AuditStartProcess2 ==
    /\ audit_stage = "P2Start"
    /\ \/ Initialize /\ KeepAudit
       \/ /\ RegisterSessionIdle(AuditSession, None, {})
          /\ audit_stage' = "P2Recover"
          /\ UNCHANGED <<setupVars, regionVars, historyVars, runVars, restartVars,
                   audit_wait_answered, resumeVars>>

\* The registration of process 2 finds the suspended run's continuable tail
\* and marks the run resumable before any row is normalized. The audit takes
\* the tail as committed: a round of the run committed before the stop (a
\* run with no committed round leaves no tail, and its input is requeued as
\* on any restart).
SuspendedRunContinues ==
    /\ audit_suspended
    /\ AuditA \in DOMAIN audit_rows
    /\ audit_rows[AuditA].phase = "Staged"

AuditMarkSuspendedRun ==
    /\ audit_stage = "P2Recover"
    /\ SuspendedRunContinues
    /\ resumable_run_id = None
    /\ \A i \in DOMAIN audit_recovery : audit_recovery[i] = "Durability"
    /\ MarkRunResumableIdle(AuditRun)
    /\ KeepAudit

\* The shell's normalization observation: the stored cursor binds the row to
\* the run the registration marked (the suspended run's Staged input).
BoundToResumableRun(i) ==
    /\ audit_suspended
    /\ i = AuditA
    /\ resumable_run_id = Some(AuditRun)

RecoveryStep(i, next) ==
    /\ audit_recovery' = [audit_recovery EXCEPT ![i] = next]
    /\ UNCHANGED <<audit_stage, setupVars, regionVars, historyVars, runVars, audit_rows,
                   audit_settled, audit_expired,
                   audit_wait_answered, resumeVars>>

\* The generated normalization verdict decides how a row is recovered: kept
\* Staged and bound to the marked run, or Queued again.
NormalizeArm(i, p, bound) ==
    \/ /\ p = "Queued"
       /\ NormalizeRecoveredInputQueuedIdle(i, p, None, bound)
       /\ RecoveryStep(i, "Admit")
    \/ /\ p = "Staged"
       /\ NormalizeRecoveredInputStagedIdle(i, p, None, bound)
       /\ RecoveryStep(i, "Admit")
    \/ /\ p = "Staged"
       /\ NormalizeRecoveredInputStagedResumableIdle(i, p, None, bound)
       /\ RecoveryStep(i, "AdmitBound")

\* Cold recovery of each durable non-terminal row through the generated
\* arms: the durability classifier (an Ephemeral row is discarded), the
\* normalization classifier for the row's phase, then the admitted input and
\* its lifecycle: Queued (a staged row is queued again), or Staged and bound
\* to the marked run when the verdict kept it bound.
AuditRecoverRow ==
    /\ audit_stage = "P2Recover"
    /\ \E i \in DOMAIN audit_recovery :
          LET r == audit_rows[i]
              f == audit_facts[i]
          IN
          \/ /\ audit_recovery[i] = "Durability"
             /\ \/ ClassifyRecoveredInputDurabilityRetainDurableDerivedOrMissingIdle(i, f.durability)
                   /\ RecoveryStep(i, "Normalize")
                \/ ClassifyRecoveredInputDurabilityDiscardEphemeralIdle(i, f.durability)
                   /\ RecoveryStep(i, "Discarded")
          \/ /\ audit_recovery[i] = "Normalize"
             /\ (SuspendedRunContinues => resumable_run_id = Some(AuditRun))
             /\ NormalizeArm(i, r.phase, BoundToResumableRun(i))
          \/ /\ audit_recovery[i] \in {"Admit", "AdmitBound"}
             /\ RecoverAdmittedInputIdle(i, f.kind, r.boundary, r.exec, None, r.lane)
             /\ RecoveryStep(i, IF audit_recovery[i] = "Admit" THEN "Lifecycle"
                                                              ELSE "LifecycleBound")
          \/ /\ audit_recovery[i] = "Lifecycle"
             /\ RecoverInputLifecycleIdle(i, "Queued", None, None, None, None, 0,
                   r.attempts, None, None, Some(r.seq), None, Some(r.lane),
                   Some(r.lane), Some(r.boundary), Some(r.exec), None, r.is_prompt)
             /\ RecoveryStep(i, "Recovered")
          \* A row the verdict kept bound stays Staged, associated with its run.
          \/ /\ audit_recovery[i] = "LifecycleBound"
             /\ RecoverInputLifecycleIdle(i, "Staged", None, None, None, None, 0,
                   r.attempts, Some(AuditRun), None, Some(r.seq), None, Some(r.lane),
                   None, None, None, None, FALSE)
             /\ RecoveryStep(i, "Recovered")

RecoveredRows == {i \in DOMAIN audit_recovery : audit_recovery[i] = "Recovered"}
MarkedRows == RecoveredRows \cap audit_retained

AuditRecoveryDone ==
    /\ audit_stage = "P2Recover"
    /\ \A i \in DOMAIN audit_recovery : audit_recovery[i] \in {"Recovered", "Discarded"}
    /\ audit_stage' = "P2Settle"
    /\ UNCHANGED vars
    /\ UNCHANGED <<setupVars, regionVars, historyVars, runVars, restartVars,
                   audit_wait_answered, resumeVars>>

\* The pre-serve settlement: the shell observes each marked row's age, and
\* an expired row ends with the generated AbandonInput(RequeueExpired).
SettleStages == IF AuditSettleBeforeServe THEN {"P2Settle"} ELSE {"P2Settle", "P2Serve"}

AuditSettle ==
    /\ audit_stage \in SettleStages
    /\ \E i \in MarkedRows \ audit_settled :
          /\ \/ /\ \/ ~audit_suspended
                   \/ i # AuditA
                   \/ resumable_run_id = None
                /\ AbandonAny(i, "RequeueExpired", Attempts(i))
                /\ audit_expired' = audit_expired \cup {i}
             \/ /\ \/ ~audit_suspended
                   \/ i # AuditA
                   \/ resumable_run_id = Some(AuditRun)
                /\ UNCHANGED vars
                /\ UNCHANGED audit_expired
          /\ audit_settled' = audit_settled \cup {i}
    /\ UNCHANGED <<audit_stage, setupVars, regionVars, historyVars, runVars, audit_rows,
                   audit_recovery,
                   audit_wait_answered, resumeVars>>

\* An expired suspended run: the settlement first ends the run's mark
\* (SettleResumableRun), then abandons its input RequeueExpired (AuditSettle).
AuditSettleSuspendedRun ==
    /\ audit_stage \in SettleStages
    /\ audit_suspended
    /\ AuditA \in MarkedRows \ audit_settled
    /\ resumable_run_id = Some(AuditRun)
    /\ \/ SettleResumableRunIdle(AuditRun, "RequeueExpired")
       \/ SettleResumableRunAttached(AuditRun, "RequeueExpired")
    /\ KeepAudit

\* The serving gate: the executor attaches once the settlement is done.
AuditServeGate ==
    /\ audit_stage = "P2Settle"
    /\ AuditSettleBeforeServe => MarkedRows \subseteq audit_settled
    /\ EnsureSessionWithExecutorIdle(AuditSession)
    /\ audit_stage' = "P2Serve"
    /\ UNCHANGED <<setupVars, regionVars, historyVars, runVars, restartVars,
                   audit_wait_answered, resumeVars>>

AuditServe ==
    /\ audit_stage = "P2Serve"
    /\ \/ PrepareAttached(AuditSession, AuditRun) /\ KeepAudit
       \/ /\ ResumeRunRunning(AuditRun)
          /\ audit_resumed' = TRUE
          /\ UNCHANGED <<audit_stage, setupVars, regionVars, historyVars, runVars,
                         restartVars, audit_suspended, audit_wait_answered>>
       \/ \E i \in AuditInputs :
             /\ StageForRunRunning(i, AuditRun)
             /\ audit_staged_p2' = audit_staged_p2 \cup {i}
             /\ audit_runs' = [audit_runs EXCEPT ![i] = @ + 1]
             /\ UNCHANGED <<audit_stage, setupVars, regionVars, historyVars,
                            audit_staged_p1, restartVars,
                   audit_wait_answered, resumeVars>>

AuditNext ==
    \/ AuditSetup
    \/ AuditBeginUnregister
    \/ AuditCancelRun
    \/ AuditSuspendRun
    \/ AuditOpenStop
    \/ AuditClassifyForCarriers
    \/ AuditStopExit
    \/ AuditClassifyForCleanup
    \/ AuditStopAbandon
    \/ AuditCommitStop
    \/ AuditUnregisterDrain
    \/ AuditAnswerRequeuedWait
    \/ AuditRestart
    \/ AuditStartProcess2
    \/ AuditMarkSuspendedRun
    \/ AuditRecoverRow
    \/ AuditRecoveryDone
    \/ AuditSettle
    \/ AuditSettleSuspendedRun
    \/ AuditServeGate
    \/ AuditServe

AuditInit ==
    /\ Init
    /\ audit_stage = "Setup"
    /\ audit_facts = (AuditA :> [kind |-> "Prompt", durability |-> "Durable", directed |-> FALSE,
                                  objective_bound |-> FALSE])
    /\ audit_disposition = "None"
    /\ audit_attempts = 0
    /\ audit_pending = {}
    /\ audit_pre_verdict = NoVerdict
    /\ audit_post_verdict = NoVerdict
    /\ audit_abandoned = {}
    /\ audit_retained = {}
    /\ audit_replay_retained = {}
    /\ audit_carriers = {}
    /\ audit_stopped = {}
    /\ audit_unregistered = FALSE
    /\ audit_wait_answered = FALSE
    /\ audit_staged_p1 = {}
    /\ audit_staged_p2 = {}
    /\ audit_runs = [i \in AuditInputs |-> 0]
    /\ audit_rows = [x \in {} |-> 0]
    /\ audit_recovery = [x \in {} |-> "Durability"]
    /\ audit_settled = {}
    /\ audit_expired = {}
    /\ audit_suspended = FALSE
    /\ audit_resumed = FALSE

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* ---- properties ----

Process1Stages == {"P1", "PreExit", "PostExit", "P1Stopped"}
OpenRetains == Answered(audit_pre_verdict, "Retain") \cup Answered(audit_post_verdict, "Retain")

\* A kept input is a queued prompt, or the input of the run the stop
\* suspended, which was staged before the stop.
SuspendedRetains == IF audit_suspended THEN audit_retained \cap {AuditA} ELSE {}
QueuedRetains == audit_retained \ SuspendedRetains

AuditRetainedNeverRunBeforeRestart ==
    /\ QueuedRetains \cap audit_staged_p1 = {}
    /\ audit_stage \in Process1Stages => \A i \in QueuedRetains : PhaseOf(i) = "Queued"
    /\ audit_stage \in Process1Stages => \A i \in SuspendedRetains : PhaseOf(i) = "Staged"

\* The suspended run's input is never staged into a run again: process 2
\* resumes the run it is bound to.
AuditSuspendedRunNeverRestaged ==
    audit_suspended => AuditA \notin audit_staged_p2

\* A run marked resumable has no published turn terminal: a run that
\* completed, failed or was cancelled is never suspended or resumed.
AuditSuspendedRunNotTerminal ==
    resumable_run_id # None => turn_terminal_run_id # resumable_run_id

AuditRetainedGetsNoCarrier ==
    audit_retained \cap audit_carriers = {}

AuditStoppedInputGetsItsCarrier ==
    audit_stopped \subseteq audit_carriers

AuditOneRunAcrossStopAndRestart ==
    audit_attempts > 0 => \A i \in AuditInputs : audit_runs[i] <= 1

AuditExpiredNeverRuns ==
    /\ audit_expired \cap audit_staged_p2 = {}
    /\ audit_retained \cap audit_staged_p2 \subseteq audit_settled

AuditRetainOnlyUnderRequeue ==
    OpenRetains \cup audit_retained # {} => audit_disposition = "Requeue"

AuditRetainOnlyEligible ==
    \A i \in OpenRetains \cup audit_retained :
        /\ audit_facts[i].kind = "Prompt"
        /\ audit_facts[i].durability = "Durable"
        /\ ~audit_facts[i].directed

AuditRetainNeverObjectiveBound ==
    \A i \in OpenRetains \cup audit_retained : ~audit_facts[i].objective_bound

AuditStopVerdictIsAFunction ==
    audit_stage \in {"PreExit", "PostExit"} =>
        \A i \in audit_pending :
            LET f == audit_facts[i] IN
            (ENABLED RetainArm(i, f.durability, f.directed, f.objective_bound))
                # (ENABLED AbandonArm(i, f.durability, f.directed, f.objective_bound))

AuditRequeuedWaitResolvesInEveryPhase ==
    phase # "Destroyed" =>
        \A sid \in SessionIdValues, failure \in RuntimeCompletionWaitFailureObservationValues :
            (ENABLED ResolveWaitAny(sid, failure)) = (failure = "RequeuedAcrossStop")

AuditRequeuedWaitAnswerChangesNothing ==
    [][(~audit_wait_answered /\ audit_wait_answered') => UNCHANGED AuditStateButStep]_<<vars, auditVars>>

\* ---- reachability goals, each checked as an expected violation ----

\* The graceful end: Requeue keeps B through the stop and a replay, the
\* unregister completes, and B runs after the restart.
AuditWitnessRetainedRunsAfterGracefulStop ==
    /\ audit_unregistered
    /\ AuditB \in audit_replay_retained
    /\ AuditB \in audit_staged_p2
\* A crash after the stop committed, before the unregister completed: the
\* kept B runs after the restart.
AuditWitnessRetainedRunsAfterCrash ==
    /\ ~audit_unregistered
    /\ AuditB \in audit_retained
    /\ AuditB \in audit_staged_p2
\* A kept B older than its max age ends RequeueExpired, and the runtime
\* serves without it.
AuditWitnessExpiredSettled ==
    AuditB \in audit_expired /\ audit_stage = "P2Serve" /\ current_run_id # None
\* Under Requeue, the in-flight A, still staged at the stop, gets its carrier
\* and ends Stopped while B is kept.
AuditWitnessStagedNotRetained ==
    /\ audit_disposition = "Requeue"
    /\ AuditA \in audit_stopped
    /\ AuditB \in audit_retained
\* Under Stop, an eligible queued B gets its carrier and ends Stopped.
AuditWitnessStopAbandonsEligible ==
    /\ audit_disposition = "Stop"
    /\ AuditB \in audit_stopped
    /\ audit_facts[AuditB].kind = "Prompt"
    /\ audit_facts[AuditB].durability = "Durable"
    /\ ~audit_facts[AuditB].directed
    /\ ~audit_facts[AuditB].objective_bound
\* A stop outside an unregister ends B, and the restart recovers nothing.
AuditWitnessPlainStopAbandons ==
    /\ audit_disposition = "None"
    /\ AuditB \in audit_stopped
    /\ audit_stage = "P2Settle"

\* The stopping process answers a kept input's waiter as requeued.
AuditWitnessRequeuedWaitAnswered == audit_wait_answered

\* The requeue stop suspends A's in-flight run, keeps A, and process 2
\* resumes the run without staging A again.
AuditWitnessSuspendedRunResumes ==
    /\ audit_suspended
    /\ AuditA \in audit_retained
    /\ audit_resumed

NotAuditWitnessRetainedRunsAfterGracefulStop == ~AuditWitnessRetainedRunsAfterGracefulStop
NotAuditWitnessRequeuedWaitAnswered == ~AuditWitnessRequeuedWaitAnswered
NotAuditWitnessRetainedRunsAfterCrash == ~AuditWitnessRetainedRunsAfterCrash
NotAuditWitnessExpiredSettled == ~AuditWitnessExpiredSettled
NotAuditWitnessStagedNotRetained == ~AuditWitnessStagedNotRetained
NotAuditWitnessStopAbandonsEligible == ~AuditWitnessStopAbandonsEligible
NotAuditWitnessPlainStopAbandons == ~AuditWitnessPlainStopAbandons
NotAuditWitnessSuspendedRunResumes == ~AuditWitnessSuspendedRunResumes
====
