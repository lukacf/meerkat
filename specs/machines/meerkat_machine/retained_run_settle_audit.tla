---- MODULE retained_run_settle_audit ----
\* Hand-written bounded audit of a host's settle of the run its session
\* retained (SettleRetainedRun).
\*
\* A durable session that stopped inside a run keeps the run open, and the
\* registration that opens the session again marks it resumable
\* (`resumable_run_id`). A host that must not let the run continue (an app's
\* Recover flow) holds the session's run starts with its own reason,
\* HostRecovery, and settles the run: the machine decides the verdict of
\* SettleRetainedRun, which carries the run the caller observed marked. An
\* accepted settle records the run's stop (`run_stop_requested`), as a stop
\* of a run marked resumable does; the runtime then settles the run
\* (SettleResumableRun, its bound inputs abandoned as Cancelled), ahead of
\* any run-start hold, and the run never resumes. A busy or not-retained
\* verdict changes nothing.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and explores,
\* for one session, the restart a crash leaves: a registration with every
\* subset of the host's HostRecovery and a mob Stop's MobStop, the kept run
\* marked resumable, an input staged and bound to it and an input queued
\* behind it. It then drives every settle arm with the observation of no run,
\* the kept run or another run, the attachment, the start of the kept run or
\* of a new one (and their held twins), the resume of the kept run, its
\* settlement as cancelled or as a refused resume, a stop of the kept run,
\* the abandonment of either input, the hold and release of every reason
\* explored, the start of an unregister, a retire, the exit of the runtime
\* executor and a destroy. A side path marks the run before the runtime
\* initializes and settles there. Properties:
\*
\*   AuditSettleChangesOnlyTheStop (action property): a settle step changes
\*     no phase, run, mark, hold, input or registration fact: at most the
\*     stop it records.
\*   AuditAcceptedSettlesTheRetainedRun (action property): an accepted
\*     settle happens only with no run current and a run marked, and records
\*     the stop of exactly that run.
\*   AuditSettleNotAcceptedWhileDraining (action property): no settle is
\*     accepted while the registration drains for its unregister.
\*   AuditRefusedSettleRecordsNothing (action property): a busy or
\*     not-retained settle records no stop.
\*   AuditSettledRunNeverResumes (action property): no step resumes a run
\*     whose settle was accepted.
\*   AuditSettledRunEndsCancelled (action property): a run whose settle was
\*     accepted is never settled as a refused resume.
\*   AuditRunningHasACurrentRun (invariant): the runtime is Running only
\*     with a run current, so the busy arm for a Running registration with
\*     no run current (SettleRetainedRunBusyNotServingRunning) and its
\*     not-retained twin are total-arm completions no state reaches.
\*
\* Four more busy arms are total-arm completions no state reaches:
\*   SettleRetainedRunBusyRunCurrentInitializing,
\*   SettleRetainedRunBusyRunCurrentIdle,
\*   SettleRetainedRunBusyRunCurrentAttached and
\*   SettleRetainedRunBusyRunCurrentStopped
\* need a run current in a phase where the generated invariant
\* current_run_only_while_running_or_retired allows none (every run of this
\* audit checks it with the model's other generated invariants).
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous. The runner's
\* --mutants mode also checks that eight seeded model defects are refused (an
\* accept of a run other than the marked one, an accept with no run marked,
\* an accept with a run current, an accept while the registration drains, an
\* accept that records no stop, a busy settle that records the stop, a resume
\* that ignores the stop, and a settlement of a stopped run as a refused
\* resume).
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* retained_run_settle_audit.sh. Run a deeper bound by hand with:
\*   specs/machines/meerkat_machine/retained_run_settle_audit.sh 14
\*
\* Scope limits: the runtime loop's settlement of a stopped marked run
\* before its hold check, the waiters the host's verb waits on, the facts it
\* reads under the session's gate before it answers Settled, and the
\* never-woken dormant member are shell, covered by the meerkat-runtime rows
\* in tests/recovery_contract.rs (a_host_settle_*) and the meerkat-mob rows in
\* tests/cold_restart_mob_resume.rs.
EXTENDS model

CONSTANT AuditMaxSteps

VARIABLES
    audit_accepted,           \* runs whose settle was accepted
    audit_accepts,            \* accepted settle steps taken
    audit_accepted_held,      \* a settle was accepted while HostRecovery held
    audit_settled_held,       \* a run whose settle was accepted settled while held
    audit_busy_current,       \* a settle was busy with a run current
    audit_busy_stale,         \* a settle was busy with a stale observation
    audit_busy_draining,      \* a settle was busy while draining
    audit_retry_not_retained, \* a settle found nothing retained after an accept
    audit_not_serving,        \* phases a settle was busy in, not serving the run
    audit_destroyed_inert     \* a settle met a destroyed runtime

auditVars == <<audit_accepted, audit_accepts, audit_accepted_held,
               audit_settled_held, audit_busy_current, audit_busy_stale,
               audit_busy_draining, audit_retry_not_retained,
               audit_not_serving, audit_destroyed_inert>>

AuditSession == "sessionid_1"
AuditQueued == "input_1"
AuditBound == "input_2"
AuditInputs == {AuditQueued, AuditBound}
AuditRunKept == "runid_1"
AuditRunNew == "runid_2"
AuditRuns == {AuditRunKept, AuditRunNew}
\* The reasons a registration starts with: the host's own and a mob Stop's.
AuditRegistrationReasons == {"HostRecovery", "MobStop"}
\* The reasons held and released in the body: the tools hold's release must
\* not end the host's hold.
AuditReasons == {"HostRecovery", "MobStop", "ToolsNotPublished"}
\* What a caller can observe marked: no run, the kept run, another run.
Observations == {None, Some(AuditRunKept), Some(AuditRunNew)}
AuditPrefixLength == 7

Attempts(i) == IF i \in DOMAIN input_attempt_counts THEN input_attempt_counts[i] ELSE 0

HostHeld == "HostRecovery" \in run_start_holds

\* A step that resumes run r: it was marked and is current after the step,
\* and the mark is gone.
Resumes(r) ==
    /\ resumable_run_id = Some(r)
    /\ resumable_run_id' = None
    /\ current_run_id' = Some(r)

KeepHistory == UNCHANGED auditVars

\* Scripted restart: register with the holds the host staged, mark the kept
\* run resumable, then recover the queued input and the input bound to the
\* marked run through the generated recovery arms.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize /\ KeepHistory
    \/ model_step_count = 1
       /\ \E init \in SUBSET AuditRegistrationReasons : RegisterSessionIdle(AuditSession, None, init)
       /\ KeepHistory
    \/ model_step_count = 2 /\ MarkRunResumableIdle(AuditRunKept) /\ KeepHistory
    \/ model_step_count = 3
       /\ RecoverAdmittedInputIdle(AuditQueued, "Prompt", "RunStart", "ContentTurn", None, "Queue")
       /\ KeepHistory
    \/ model_step_count = 4
       /\ RecoverInputLifecycleIdle(AuditQueued, "Queued", None, None, None, None, 0, 0,
             None, None, Some(1), None, Some("Queue"), Some("Queue"), Some("RunStart"),
             Some("ContentTurn"), None, TRUE)
       /\ KeepHistory
    \/ model_step_count = 5
       /\ RecoverAdmittedInputIdle(AuditBound, "Prompt", "RunStart", "ContentTurn", None, "Queue")
       /\ KeepHistory
    \/ model_step_count = 6
       /\ RecoverInputLifecycleIdle(AuditBound, "Staged", None, None, None, None, 0, 1,
             Some(AuditRunKept), None, Some(2), None, Some("Queue"), None, None, None, None,
             FALSE)
       /\ KeepHistory

SettleAccepted(o) ==
    \/ SettleRetainedRunAcceptedIdle(o)
    \/ SettleRetainedRunAcceptedAttached(o)

SettleBusyCurrent(o) ==
    \/ SettleRetainedRunBusyRunCurrentInitializing(o)
    \/ SettleRetainedRunBusyRunCurrentIdle(o)
    \/ SettleRetainedRunBusyRunCurrentAttached(o)
    \/ SettleRetainedRunBusyRunCurrentRunning(o)
    \/ SettleRetainedRunBusyRunCurrentRetired(o)
    \/ SettleRetainedRunBusyRunCurrentStopped(o)

SettleBusyDraining(o) ==
    \/ SettleRetainedRunBusyDrainingIdle(o)
    \/ SettleRetainedRunBusyDrainingAttached(o)

SettleBusyStale(o) ==
    \/ SettleRetainedRunBusyStaleRunIdle(o)
    \/ SettleRetainedRunBusyStaleRunAttached(o)

SettleBusyNotServing(o) ==
    \/ SettleRetainedRunBusyNotServingInitializing(o)
    \/ SettleRetainedRunBusyNotServingRunning(o)
    \/ SettleRetainedRunBusyNotServingRetired(o)
    \/ SettleRetainedRunBusyNotServingStopped(o)

SettleNotRetained(o) ==
    \/ SettleRetainedRunNotRetainedInitializing(o)
    \/ SettleRetainedRunNotRetainedIdle(o)
    \/ SettleRetainedRunNotRetainedAttached(o)
    \/ SettleRetainedRunNotRetainedRunning(o)
    \/ SettleRetainedRunNotRetainedRetired(o)
    \/ SettleRetainedRunNotRetainedStopped(o)
    \/ SettleRetainedRunInertDestroyed(o)

SettleRefused(o) ==
    \/ SettleBusyCurrent(o)
    \/ SettleBusyDraining(o)
    \/ SettleBusyStale(o)
    \/ SettleBusyNotServing(o)
    \/ SettleNotRetained(o)

SettleStep == \E o \in Observations : SettleAccepted(o) \/ SettleRefused(o)

\* The runtime's settlement of the kept run without resuming it.
LoopSettle(reason) ==
    \/ SettleResumableRunIdle(AuditRunKept, reason)
    \/ SettleResumableRunAttached(AuditRunKept, reason)

HoldFor(r) ==
    \/ HoldRunStartsIdle(r)
    \/ HoldRunStartsAttached(r)
    \/ HoldRunStartsRunning(r)

ReleaseFor(r) ==
    \/ ReleaseRunStartsLastIdle(r)
    \/ ReleaseRunStartsLastAttached(r)
    \/ ReleaseRunStartsLastRunning(r)
    \/ ReleaseRunStartsStillHeldIdle(r)
    \/ ReleaseRunStartsStillHeldAttached(r)
    \/ ReleaseRunStartsStillHeldRunning(r)

\* The attachment, the start of the kept run or of a new one (refused while
\* held), the resume of the kept run, a stop of the kept run, the
\* abandonment of an input, the start of an unregister, a retire, the exit of
\* the runtime executor, and a destroy.
AuditRunStep ==
    \/ EnsureSessionWithExecutorIdle(AuditSession)
    \/ \E r \in AuditRuns :
        \/ PrepareIdle(AuditSession, r)
        \/ PrepareAttached(AuditSession, r)
        \/ PrepareHeldIdle(AuditSession, r)
        \/ PrepareHeldAttached(AuditSession, r)
    \/ ResumeRunRunning(AuditRunKept)
    \/ StopCurrentRunForRunResumableIdle(AuditRunKept)
    \/ StopCurrentRunForRunResumableAttached(AuditRunKept)
    \/ \E i \in AuditInputs :
        \/ AbandonInputIdle(i, "Cancelled", Attempts(i))
        \/ AbandonInputAttached(i, "Cancelled", Attempts(i))
    \/ BeginUnregisterSessionIdle(AuditSession, None, None, None, None, "Stop", 0)
    \/ BeginUnregisterSessionAttached(AuditSession, None, None, None, None, "Stop", 0)
    \/ RetireRequestedFromIdle(AuditSession)
    \/ RetireRequestedFromIdleUnbound(AuditSession)
    \/ RuntimeExecutorExitedFromIdle
    \/ RuntimeExecutorExitedFromAttached
    \/ Destroy(AuditSession)

\* A settle the registration cannot serve now, recorded with its phase.
AuditBusyNotServing(o) ==
    /\ SettleBusyNotServing(o)
    /\ audit_not_serving' = audit_not_serving \cup {phase}
    /\ UNCHANGED <<audit_accepted, audit_accepts, audit_accepted_held,
                   audit_settled_held, audit_busy_current, audit_busy_stale,
                   audit_busy_draining, audit_retry_not_retained,
                   audit_destroyed_inert>>

AuditBody ==
    \/ \E o \in Observations :
        /\ SettleAccepted(o)
        /\ audit_accepted' = audit_accepted \cup {r \in AuditRuns : o = Some(r)}
        /\ audit_accepts' = audit_accepts + 1
        /\ audit_accepted_held' = (audit_accepted_held \/ HostHeld)
        /\ UNCHANGED <<audit_settled_held, audit_busy_current, audit_busy_stale,
                       audit_busy_draining, audit_retry_not_retained,
                       audit_not_serving, audit_destroyed_inert>>
    \/ \E o \in Observations :
        /\ SettleBusyCurrent(o)
        /\ audit_busy_current' = TRUE
        /\ UNCHANGED <<audit_accepted, audit_accepts, audit_accepted_held,
                       audit_settled_held, audit_busy_stale, audit_busy_draining,
                       audit_retry_not_retained, audit_not_serving,
                       audit_destroyed_inert>>
    \/ \E o \in Observations :
        /\ SettleBusyStale(o)
        /\ audit_busy_stale' = TRUE
        /\ UNCHANGED <<audit_accepted, audit_accepts, audit_accepted_held,
                       audit_settled_held, audit_busy_current, audit_busy_draining,
                       audit_retry_not_retained, audit_not_serving,
                       audit_destroyed_inert>>
    \/ \E o \in Observations :
        /\ SettleBusyDraining(o)
        /\ audit_busy_draining' = TRUE
        /\ UNCHANGED <<audit_accepted, audit_accepts, audit_accepted_held,
                       audit_settled_held, audit_busy_current, audit_busy_stale,
                       audit_retry_not_retained, audit_not_serving,
                       audit_destroyed_inert>>
    \/ \E o \in Observations :
        /\ SettleNotRetained(o)
        /\ audit_retry_not_retained' = (audit_retry_not_retained \/ audit_accepted # {})
        /\ audit_destroyed_inert' = (audit_destroyed_inert \/ phase = "Destroyed")
        /\ UNCHANGED <<audit_accepted, audit_accepts, audit_accepted_held,
                       audit_settled_held, audit_busy_current, audit_busy_stale,
                       audit_busy_draining, audit_not_serving>>
    \/ \E o \in Observations : AuditBusyNotServing(o)
    \/ \E reason \in {"Cancelled", "ResumeRefused"} :
        /\ LoopSettle(reason)
        /\ audit_settled_held' =
              (audit_settled_held \/ (AuditRunKept \in audit_accepted /\ HostHeld))
        /\ UNCHANGED <<audit_accepted, audit_accepts, audit_accepted_held,
                       audit_busy_current, audit_busy_stale, audit_busy_draining,
                       audit_retry_not_retained, audit_not_serving,
                       audit_destroyed_inert>>
    \/ (\E r \in AuditReasons : HoldFor(r) \/ ReleaseFor(r)) /\ KeepHistory
    \/ AuditRunStep /\ KeepHistory

\* Side path: the run is marked before the runtime initializes, and a settle
\* meets the initializing runtime.
AuditInitializingSide ==
    /\ phase = "Initializing"
    /\ \/ MarkRunResumableInitializing(AuditRunKept) /\ KeepHistory
       \/ \E o \in Observations : AuditBusyNotServing(o)
       \/ \E o \in Observations : SettleNotRetained(o) /\ KeepHistory

AuditNext ==
    \/ AuditPrefix
    \/ AuditInitializingSide
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditInit ==
    /\ Init
    /\ audit_accepted = {}
    /\ audit_accepts = 0
    /\ audit_accepted_held = FALSE
    /\ audit_settled_held = FALSE
    /\ audit_busy_current = FALSE
    /\ audit_busy_stale = FALSE
    /\ audit_busy_draining = FALSE
    /\ audit_retry_not_retained = FALSE
    /\ audit_not_serving = {}
    /\ audit_destroyed_inert = FALSE

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditSettleChangesOnlyTheStop ==
    [][SettleStep =>
         UNCHANGED <<phase, current_run_id, resumable_run_id, run_start_holds,
                     input_phases, input_lane, input_run_associations,
                     registration_phase, turn_terminal_run_id,
                     input_controller_holds>>]_<<vars, auditVars>>

AuditAcceptedSettlesTheRetainedRun ==
    [][\A o \in Observations :
         SettleAccepted(o) =>
            /\ current_run_id = None
            /\ resumable_run_id # None
            /\ run_stop_requested' = resumable_run_id]_<<vars, auditVars>>

AuditSettleNotAcceptedWhileDraining ==
    [][\A o \in Observations :
         SettleAccepted(o) => registration_phase # "Draining"]_<<vars, auditVars>>

AuditRefusedSettleRecordsNothing ==
    [][\A o \in Observations :
         SettleRefused(o) => UNCHANGED run_stop_requested]_<<vars, auditVars>>

AuditSettledRunNeverResumes ==
    [][\A r \in audit_accepted : ~Resumes(r)]_<<vars, auditVars>>

AuditSettledRunEndsCancelled ==
    [][\A r \in audit_accepted :
         ~(SettleResumableRunIdle(r, "ResumeRefused")
           \/ SettleResumableRunAttached(r, "ResumeRefused"))]_<<vars, auditVars>>

\* Running always has a run current: no state reaches the settle arms of a
\* Running registration with no run current.
AuditRunningHasACurrentRun == phase = "Running" => current_run_id # None

\* Reachability goals, each checked as an expected violation of its negation.
\* The host's settle is accepted while its own hold holds the session.
AuditWitnessAcceptedUnderHostHold == audit_accepted_held
\* The accepted run is settled, with no model request, while the host still
\* holds the session: the mark is gone and the host's hold stands.
AuditWitnessSettledUnderHostHold ==
    /\ audit_settled_held
    /\ resumable_run_id = None
    /\ HostHeld
\* A second settle of the same run is accepted again (idempotent per run).
AuditWitnessRepeatAccepted == audit_accepts >= 2
\* The kept run resumed (nothing held it): a settle is busy and stops nothing.
AuditWitnessBusyWithRunCurrent == audit_busy_current
\* A settle that names a run other than the marked one is busy.
AuditWitnessBusyStaleObservation == audit_busy_stale
\* A settle while the registration drains for its unregister is busy.
AuditWitnessBusyWhileDraining == audit_busy_draining
\* A retry after the run was settled finds nothing retained.
AuditWitnessRetryNotRetained == audit_retry_not_retained
\* A settle meets a runtime still initializing, with the run marked: busy.
AuditWitnessBusyNotServingInitializing == "Initializing" \in audit_not_serving
\* A settle meets a retired runtime that still marks the run: busy.
AuditWitnessBusyNotServingRetired == "Retired" \in audit_not_serving
\* A settle meets a runtime whose executor exited, the run marked: busy.
AuditWitnessBusyNotServingStopped == "Stopped" \in audit_not_serving
\* A settle meets a destroyed runtime: nothing retained, nothing changed.
AuditWitnessInertDestroyed == audit_destroyed_inert

NotAuditWitnessAcceptedUnderHostHold == ~AuditWitnessAcceptedUnderHostHold
NotAuditWitnessSettledUnderHostHold == ~AuditWitnessSettledUnderHostHold
NotAuditWitnessRepeatAccepted == ~AuditWitnessRepeatAccepted
NotAuditWitnessBusyWithRunCurrent == ~AuditWitnessBusyWithRunCurrent
NotAuditWitnessBusyStaleObservation == ~AuditWitnessBusyStaleObservation
NotAuditWitnessBusyWhileDraining == ~AuditWitnessBusyWhileDraining
NotAuditWitnessRetryNotRetained == ~AuditWitnessRetryNotRetained
NotAuditWitnessBusyNotServingInitializing == ~AuditWitnessBusyNotServingInitializing
NotAuditWitnessBusyNotServingRetired == ~AuditWitnessBusyNotServingRetired
NotAuditWitnessBusyNotServingStopped == ~AuditWitnessBusyNotServingStopped
NotAuditWitnessInertDestroyed == ~AuditWitnessInertDestroyed
====
