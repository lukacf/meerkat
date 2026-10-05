---- MODULE run_start_hold_audit ----
\* Hand-written bounded audit of the machine-owned run-start holds (#1500).
\*
\* A runtime's run starts are held while ANY reason holds them
\* (`run_start_holds`, a set of RunStartHoldReason: a mob Stop's MobStop, a
\* host's ToolsNotPublished), and each holder releases only its own reason.
\* While held, an input admitted earlier stays queued and runs after the last
\* release, instead of starting a run. The holds must stop every arm that
\* establishes a new run (Prepare*, DrainQueued* and the direct
\* StartConversationRun / StartImmediateAppend starts from Idle, Initializing
\* and Attached), each of which has a Held twin that refuses without changing
\* state, and a hold or release must never touch the queue or the current
\* run. A registration carries the holds its binding starts with: a fresh
\* binding starts with exactly them, a re-registration of the same binding
\* (idempotent, or resuming a Stopped runtime) adds them to what it holds.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and explores,
\* for one session, one queued input and two runs: registration with every
\* subset of reasons, every hold and release arm for every reason (Last and
\* StillHeld), every gated arm and its Held twin, staging, one turn of the
\* current run, the commit, cancel and service run endings into Idle,
\* Attached and Retired, retire during a run, the retired queue drain, the
\* executor exit into Stopped, and re-registration (idempotent and resuming
\* Stopped) with every subset of reasons. The turn start of an
\* already-current run is applied only for that run (the shell starts one
\* turn per run). Properties:
\*
\*   AuditNoNewRunWhileHeld (action property): no step taken while any reason
\*     holds run starts establishes a new current run.
\*   AuditHoldReleaseLeaveQueueAndRun (action property): hold and release
\*     steps change neither the queue (input phases and lanes) nor the
\*     current run.
\*   AuditHoldAndReleaseOwnReasonOnly (action property): a hold adds exactly
\*     its reason, and a release removes exactly its reason, leaving every
\*     other reason held.
\*   AuditRegistrationAppliesHolds (action property): a fresh registration
\*     holds exactly the reasons it carries; a re-registration adds them to
\*     the reasons already held.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous. The runner's
\* --mutants mode also checks that three seeded model defects are refused
\* (a registration that ignores its holds, gates that test only MobStop, a
\* release that clears every reason).
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
    audit_registered,          \* the reasons the first registration carried
    audit_hold_taken,          \* a hold step was taken
    audit_refused,             \* a Held twin refused a run start
    audit_released_after,      \* a release followed a refusal
    audit_held_while_running,  \* a hold landed while a run was current
    audit_refused_after_run,   \* a refusal followed a run that ended while held
    audit_retired_refused,     \* the retired queue drain was refused
    audit_partial_release,     \* a release left another reason holding
    audit_refused_after_partial, \* a refusal followed a partial release
    audit_stopped_hold,        \* a hold was recorded while Stopped
    audit_resumed_held,        \* a Stopped runtime resumed with a hold recorded
    audit_refused_after_resume \* a refusal followed that resume

auditVars == <<audit_registered, audit_hold_taken, audit_refused,
               audit_released_after, audit_held_while_running,
               audit_refused_after_run, audit_retired_refused,
               audit_partial_release, audit_refused_after_partial,
               audit_stopped_hold, audit_resumed_held,
               audit_refused_after_resume>>

AuditSession == "sessionid_1"
AuditInput == "input_1"
AuditRuns == {"runid_1", "runid_2"}
AuditReasons == RunStartHoldReasonValues
AuditPrefixLength == 4

FreshRegister(init) == RegisterSessionIdle(AuditSession, None, init)

AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
       /\ UNCHANGED auditVars
    \/ model_step_count = 1
       /\ \E init \in SUBSET AuditReasons :
            /\ FreshRegister(init)
            /\ audit_registered' = init
       /\ UNCHANGED <<audit_hold_taken, audit_refused, audit_released_after,
                      audit_held_while_running, audit_refused_after_run,
                      audit_retired_refused, audit_partial_release,
                      audit_refused_after_partial, audit_stopped_hold,
                      audit_resumed_held, audit_refused_after_resume>>
    \/ model_step_count = 2 /\ ResolveAdmissionPlanDefaultQueueKindIdle(AuditInput, "Prompt", None, "Ordinary", "Untyped", FALSE, None, FALSE, FALSE, FALSE)
       /\ UNCHANGED auditVars
    \/ model_step_count = 3 /\ QueueAcceptedIdle(AuditInput)
       /\ UNCHANGED auditVars

HoldFor(r) ==
    \/ HoldRunStartsInitializing(r)
    \/ HoldRunStartsIdle(r)
    \/ HoldRunStartsAttached(r)
    \/ HoldRunStartsRunning(r)
    \/ HoldRunStartsRetired(r)
    \/ HoldRunStartsStopped(r)
    \/ HoldRunStartsInertDestroyed(r)

ReleaseLastFor(r) ==
    \/ ReleaseRunStartsLastInitializing(r)
    \/ ReleaseRunStartsLastIdle(r)
    \/ ReleaseRunStartsLastAttached(r)
    \/ ReleaseRunStartsLastRunning(r)
    \/ ReleaseRunStartsLastRetired(r)
    \/ ReleaseRunStartsLastStopped(r)
    \/ ReleaseRunStartsLastDestroyed(r)

ReleaseStillHeldFor(r) ==
    \/ ReleaseRunStartsStillHeldInitializing(r)
    \/ ReleaseRunStartsStillHeldIdle(r)
    \/ ReleaseRunStartsStillHeldAttached(r)
    \/ ReleaseRunStartsStillHeldRunning(r)
    \/ ReleaseRunStartsStillHeldRetired(r)
    \/ ReleaseRunStartsStillHeldStopped(r)
    \/ ReleaseRunStartsStillHeldDestroyed(r)

ReleaseFor(r) == ReleaseLastFor(r) \/ ReleaseStillHeldFor(r)

AuditHold == \E r \in AuditReasons : HoldFor(r)
AuditRelease == \E r \in AuditReasons : ReleaseFor(r)
AuditPartialRelease == \E r \in AuditReasons : ReleaseStillHeldFor(r)

\* Re-registration of the bound session: it adds the reasons it carries.
IdempotentRegister(init) ==
    \/ RegisterSessionIdempotentIdle(AuditSession, None, init)
    \/ RegisterSessionIdempotentAttached(AuditSession, None, init)
    \/ RegisterSessionIdempotentRunning(AuditSession, None, init)
    \/ RegisterSessionIdempotentRetired(AuditSession, None, init)

UnionRegister(init) ==
    \/ IdempotentRegister(init)
    \/ RegisterSessionResumesStopped(AuditSession, None, init)

AuditIdempotentRegister == \E init \in SUBSET AuditReasons : IdempotentRegister(init)
AuditResume == \E init \in SUBSET AuditReasons : RegisterSessionResumesStopped(AuditSession, None, init)

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
    \/ RuntimeExecutorExitedFromIdle
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

Refusal(retired) ==
    /\ audit_refused' = TRUE
    /\ audit_refused_after_run' = (audit_refused_after_run \/ audit_held_while_running)
    /\ audit_refused_after_partial' = (audit_refused_after_partial \/ audit_partial_release)
    /\ audit_refused_after_resume' = (audit_refused_after_resume \/ audit_resumed_held)
    /\ audit_retired_refused' = (audit_retired_refused \/ retired)
    /\ UNCHANGED <<audit_registered, audit_hold_taken, audit_released_after,
                   audit_held_while_running, audit_partial_release,
                   audit_stopped_hold, audit_resumed_held>>

KeepHistory == UNCHANGED auditVars

AuditBody ==
    \/ AuditHold
       /\ audit_hold_taken' = TRUE
       /\ audit_held_while_running' = (audit_held_while_running \/ current_run_id # None)
       /\ audit_stopped_hold' = (audit_stopped_hold \/ phase = "Stopped")
       /\ UNCHANGED <<audit_registered, audit_refused, audit_released_after,
                      audit_refused_after_run, audit_retired_refused,
                      audit_partial_release, audit_refused_after_partial,
                      audit_resumed_held, audit_refused_after_resume>>
    \/ (\E r \in AuditReasons : ReleaseLastFor(r))
       /\ audit_released_after' = (audit_released_after \/ audit_refused)
       /\ UNCHANGED <<audit_registered, audit_hold_taken, audit_refused,
                      audit_held_while_running, audit_refused_after_run,
                      audit_retired_refused, audit_partial_release,
                      audit_refused_after_partial, audit_stopped_hold,
                      audit_resumed_held, audit_refused_after_resume>>
    \/ AuditPartialRelease
       /\ audit_released_after' = (audit_released_after \/ audit_refused)
       /\ audit_partial_release' = TRUE
       /\ UNCHANGED <<audit_registered, audit_hold_taken, audit_refused,
                      audit_held_while_running, audit_refused_after_run,
                      audit_retired_refused, audit_refused_after_partial,
                      audit_stopped_hold, audit_resumed_held,
                      audit_refused_after_resume>>
    \/ AuditIdempotentRegister /\ KeepHistory
    \/ AuditResume
       /\ audit_resumed_held' = (audit_resumed_held \/ audit_stopped_hold)
       /\ UNCHANGED <<audit_registered, audit_hold_taken, audit_refused,
                      audit_released_after, audit_held_while_running,
                      audit_refused_after_run, audit_retired_refused,
                      audit_partial_release, audit_refused_after_partial,
                      audit_stopped_hold, audit_refused_after_resume>>
    \/ AuditHeldStart /\ Refusal(FALSE)
    \/ AuditRetiredHeld /\ Refusal(TRUE)
    \/ (AuditGated \/ AuditRun) /\ KeepHistory

AuditNext ==
    \/ AuditPrefix
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditInit ==
    /\ Init
    /\ audit_registered = {}
    /\ audit_hold_taken = FALSE
    /\ audit_refused = FALSE
    /\ audit_released_after = FALSE
    /\ audit_held_while_running = FALSE
    /\ audit_refused_after_run = FALSE
    /\ audit_retired_refused = FALSE
    /\ audit_partial_release = FALSE
    /\ audit_refused_after_partial = FALSE
    /\ audit_stopped_hold = FALSE
    /\ audit_resumed_held = FALSE
    /\ audit_refused_after_resume = FALSE

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditNoNewRunWhileHeld ==
    [][~(run_start_holds # {} /\ current_run_id' # None /\ current_run_id' # current_run_id)]_<<vars, auditVars>>

AuditHoldReleaseLeaveQueueAndRun ==
    [][(AuditHold \/ AuditRelease) => UNCHANGED <<input_phases, input_lane, current_run_id>>]_<<vars, auditVars>>

AuditHoldAndReleaseOwnReasonOnly ==
    [][\A r \in AuditReasons :
         /\ HoldFor(r) => run_start_holds' = run_start_holds \cup {r}
         /\ ReleaseFor(r) => run_start_holds' = run_start_holds \ {r}]_<<vars, auditVars>>

AuditRegistrationAppliesHolds ==
    [][\A init \in SUBSET AuditReasons :
         /\ FreshRegister(init) => run_start_holds' = init
         /\ UnionRegister(init) => run_start_holds' = run_start_holds \cup init]_<<vars, auditVars>>

\* Reachability goals, each checked as an expected violation of its negation.
QueuedInput == AuditInput \in DOMAIN input_phases /\ input_phases[AuditInput] = "Queued"
\* A held start was refused and the input is still queued.
AuditWitnessRefused == audit_refused /\ QueuedInput /\ run_start_holds # {}
\* After a refusal, the last release lets the queued input start a run.
AuditWitnessReleasedRuns == audit_released_after /\ current_run_id # None /\ run_start_holds = {}
\* A hold landed during a run; the run ended; a later start was refused.
AuditWitnessRunFinishesThenRefused == audit_refused_after_run /\ current_run_id = None
\* The retired queue drain is refused while held.
AuditWitnessRetiredDrainRefused == audit_retired_refused
\* The holds a registration carried refuse a start with no hold step taken.
AuditWitnessRegisteredHeldRefused == audit_refused /\ ~audit_hold_taken /\ audit_registered # {}
\* Releasing one of two reasons leaves the runtime held: a start is refused.
AuditWitnessStillHeldRefused == audit_refused_after_partial /\ QueuedInput
\* A hold recorded while Stopped survives the resume: a start is refused.
AuditWitnessStoppedHoldSurvivesResume == audit_refused_after_resume

NotAuditWitnessRefused == ~AuditWitnessRefused
NotAuditWitnessReleasedRuns == ~AuditWitnessReleasedRuns
NotAuditWitnessRunFinishesThenRefused == ~AuditWitnessRunFinishesThenRefused
NotAuditWitnessRetiredDrainRefused == ~AuditWitnessRetiredDrainRefused
NotAuditWitnessRegisteredHeldRefused == ~AuditWitnessRegisteredHeldRefused
NotAuditWitnessStillHeldRefused == ~AuditWitnessStillHeldRefused
NotAuditWitnessStoppedHoldSurvivesResume == ~AuditWitnessStoppedHoldSurvivesResume
====
