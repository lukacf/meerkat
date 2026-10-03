---- MODULE run_start_hold_audit ----
\* Hand-written bounded audit of the machine-owned run-start hold (#1500).
\*
\* A mob Stop holds every member runtime's run starts: an input admitted
\* before the stop stays queued and runs after Resume releases the hold,
\* instead of starting a run while the mob is stopped. The hold must stop
\* every arm that establishes a new run (Prepare*, DrainQueued* and the
\* direct StartConversationRun / StartImmediateAppend starts from Idle,
\* Initializing and Attached), each of which has a Held twin that refuses
\* without changing state, and the hold itself must never touch the queue or
\* the current run.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and explores,
\* for one session, one queued input and two runs, every hold and release arm,
\* every gated arm and its Held twin, staging, one turn of the current run,
\* the commit, cancel and service run endings into Idle, Attached and
\* Retired, retire during a run and the retired queue drain. The turn start
\* of an already-current run is applied only for that run (the shell starts
\* one turn per run). Properties:
\*
\*   AuditNoNewRunWhileHeld (action property): no step taken while run starts
\*     are held establishes a new current run.
\*   AuditHoldReleaseLeaveQueueAndRun (action property): hold and release
\*     steps change neither the queue (input phases and lanes) nor the
\*     current run.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* run_start_hold_audit.sh. Run a deeper bound by hand with:
\*   specs/machines/meerkat_machine/run_start_hold_audit.sh 16
\*
\* Scope limit: the RunStartsReleased { queued } wake and the runtime loop's
\* park are shell behavior, covered by the meerkat-runtime tests
\* held_run_starts_park_the_loop_and_release_runs_the_input_once and
\* a_hold_landing_after_the_loop_woke_parks_it.
EXTENDS model

CONSTANT AuditMaxSteps

VARIABLES
    audit_refused,           \* a Held twin refused a run start
    audit_released_after,    \* a release followed a refusal
    audit_held_while_running,\* a hold landed while a run was current
    audit_refused_after_run, \* a refusal followed a run that ended while held
    audit_retired_refused    \* the retired queue drain was refused

auditVars == <<audit_refused, audit_released_after, audit_held_while_running,
               audit_refused_after_run, audit_retired_refused>>

AuditSession == "sessionid_1"
AuditInput == "input_1"
AuditRuns == {"runid_1", "runid_2"}
AuditPrefixLength == 4

AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None)
    \/ model_step_count = 2 /\ ResolveAdmissionPlanDefaultQueueKindIdle(AuditInput, "Prompt", None, "Ordinary", "Untyped", FALSE, None, FALSE, FALSE, FALSE)
    \/ model_step_count = 3 /\ QueueAcceptedIdle(AuditInput)

AuditHold ==
    \/ HoldRunStartsInitializing
    \/ HoldRunStartsIdle
    \/ HoldRunStartsAttached
    \/ HoldRunStartsRunning
    \/ HoldRunStartsRetired
    \/ HoldRunStartsInertStopped
    \/ HoldRunStartsInertDestroyed

AuditRelease ==
    \/ ReleaseRunStartsInitializing
    \/ ReleaseRunStartsIdle
    \/ ReleaseRunStartsAttached
    \/ ReleaseRunStartsRunning
    \/ ReleaseRunStartsRetired
    \/ ReleaseRunStartsStopped
    \/ ReleaseRunStartsDestroyed

ConversationArgs(Action(_, _, _, _, _, _), r) ==
    Action(r, "ConversationTurn", "conversation", FALSE, FALSE, 0)

\* Every arm that establishes a new run.
AuditGated ==
    \E r \in AuditRuns :
        \/ PrepareIdle(AuditSession, r)
        \/ PrepareIdleRetainingUnsettledCompletion(AuditSession, r)
        \/ PrepareAttached(AuditSession, r)
        \/ PrepareAttachedRetainingUnsettledCompletion(AuditSession, r)
        \/ DrainQueuedRunRetired(r)
        \/ DrainQueuedRunRetiredRetainingUnsettledCompletion(r)
        \/ ConversationArgs(StartConversationRunIdleWithBinding, r)
        \/ ConversationArgs(StartConversationRunInitializing, r)
        \/ ConversationArgs(StartConversationRunAttached, r)
        \/ StartImmediateAppendInitializing(r)
        \/ StartImmediateAppendAttached(r)

AuditHeldStart ==
    \E r \in AuditRuns :
        \/ PrepareHeldIdle(AuditSession, r)
        \/ PrepareHeldAttached(AuditSession, r)
        \/ ConversationArgs(StartConversationRunHeldIdle, r)
        \/ ConversationArgs(StartConversationRunHeldInitializing, r)
        \/ ConversationArgs(StartConversationRunHeldAttached, r)
        \/ StartImmediateAppendHeldInitializing(r)
        \/ StartImmediateAppendHeldAttached(r)

AuditRetiredHeld == \E r \in AuditRuns : DrainQueuedRunHeldRetired(r)

AuditRun ==
    \/ EnsureSessionWithExecutorIdle(AuditSession)
    \/ QueueAcceptedAttached(AuditInput)
    \/ RetireRequestedWhileRunBound(AuditSession)
    \/ RetireRequestedWhileRunUnbound(AuditSession)
    \/ \E r \in AuditRuns :
        \/ StageForRunRunning(AuditInput, r)
        \* The turn start of the already-current run only.
        \/ current_run_id = Some(r) /\ ConversationArgs(StartConversationRunRunning, r)
        \/ PrimitiveAppliedConversation(r)
        \/ LlmReturnedTerminal(r)
        \/ BoundaryCompleteCompleted(r)
        \/ RunCompleted(r)
        \/ RunCancelled(r)
        \/ CommitRunningToIdle(AuditInput, r)
        \/ CommitRunningToAttached(AuditInput, r)
        \/ CommitRunningToRetired(AuditInput, r)
        \/ CancelRunningToIdle(r)
        \/ CancelRunningToAttached(r)
        \/ CancelRunningToRetired(r)
        \/ ServiceTurnCommittedRunningToIdle(r)
        \/ ServiceTurnCommittedRunningToAttached(r)

KeepHistory == UNCHANGED auditVars

AuditBody ==
    \/ AuditHold
       /\ audit_held_while_running' = (audit_held_while_running \/ current_run_id # None)
       /\ UNCHANGED <<audit_refused, audit_released_after, audit_refused_after_run, audit_retired_refused>>
    \/ AuditRelease
       /\ audit_released_after' = (audit_released_after \/ audit_refused)
       /\ UNCHANGED <<audit_refused, audit_held_while_running, audit_refused_after_run, audit_retired_refused>>
    \/ AuditHeldStart
       /\ audit_refused' = TRUE
       /\ audit_refused_after_run' = (audit_refused_after_run \/ audit_held_while_running)
       /\ UNCHANGED <<audit_released_after, audit_held_while_running, audit_retired_refused>>
    \/ AuditRetiredHeld
       /\ audit_refused' = TRUE
       /\ audit_retired_refused' = TRUE
       /\ audit_refused_after_run' = (audit_refused_after_run \/ audit_held_while_running)
       /\ UNCHANGED <<audit_released_after, audit_held_while_running>>
    \/ (AuditGated \/ AuditRun) /\ KeepHistory

AuditNext ==
    \/ AuditPrefix /\ KeepHistory
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditInit ==
    /\ Init
    /\ audit_refused = FALSE
    /\ audit_released_after = FALSE
    /\ audit_held_while_running = FALSE
    /\ audit_refused_after_run = FALSE
    /\ audit_retired_refused = FALSE

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditNoNewRunWhileHeld ==
    [][~(run_starts_held = TRUE /\ current_run_id' # None /\ current_run_id' # current_run_id)]_<<vars, auditVars>>

AuditHoldReleaseLeaveQueueAndRun ==
    [][(AuditHold \/ AuditRelease) => UNCHANGED <<input_phases, input_lane, current_run_id>>]_<<vars, auditVars>>

\* Reachability goals, each checked as an expected violation of its negation.
QueuedInput == AuditInput \in DOMAIN input_phases /\ input_phases[AuditInput] = "Queued"
\* A held start was refused and the input is still queued.
AuditWitnessRefused == audit_refused /\ QueuedInput /\ run_starts_held = TRUE
\* After a refusal, the release lets the queued input start a run.
AuditWitnessReleasedRuns == audit_released_after /\ current_run_id # None /\ run_starts_held = FALSE
\* A hold landed during a run; the run ended; a later start was refused.
AuditWitnessRunFinishesThenRefused == audit_refused_after_run /\ current_run_id = None
\* The retired queue drain is refused while held.
AuditWitnessRetiredDrainRefused == audit_retired_refused

NotAuditWitnessRefused == ~AuditWitnessRefused
NotAuditWitnessReleasedRuns == ~AuditWitnessReleasedRuns
NotAuditWitnessRunFinishesThenRefused == ~AuditWitnessRunFinishesThenRefused
NotAuditWitnessRetiredDrainRefused == ~AuditWitnessRetiredDrainRefused
====
