---- MODULE requester_batch_audit ----
\* Hand-written bounded audit of the requester batch rule: inputs of different
\* requesters never share a run, and the Steer lane is first in, first out
\* across requesters.
\*
\* An admitted input's batch key (input_authority_batch_keys) is its governed
\* binding's batch identity or, for an ungoverned input whose host supplied a
\* requester, that requester's key; an ungoverned input without a requester
\* has none, and absence is a key value too. The machine keeps one key per
\* run: StageForRun's homogeneous_run_authority stages only inputs of the key
\* the run staged first, and JoinLiveBoundaryDurableAppend's and
\* ResolveLiveBoundaryContextReceipt's live_join_same_requester let an input
\* join a running run only when its key is the key that run staged. The
\* Steer lane's order across keys is the machine's too: StageForRun,
\* JoinLiveBoundaryDurableAppend and ResolveLiveBoundaryContextReceipt
\* refuse a Steer-lane input while a stageable Steer-lane input admitted
\* before it waits with another key (steer_lane_fifo_across_requesters).
\*
\* The audit keeps the generated model's variables, Init, actions and
\* invariants unchanged. A deterministic prefix starts one runtime-loop run
\* whose batch input carries the key named by AuditBatchKeyName. With
\* AuditEarlyKeyName "absent" (the requester runs) TLC then explores one late
\* Steer input of the key named by AuditLateKeyName: its admission (every
\* turn-append shape), acceptance, durable join and its three resolutions,
\* live-boundary normalization and receipts, staging into the running run,
\* the turn phases and every run-ending arm, a follow-up run the late input
\* may be staged into, and a store fence gap that suspends the batch input's
\* run, which the registration then starts again under its own id and
\* resumes (ResumeRun stages the run's batch key again: the key of its bound
\* inputs). With an AuditEarlyKeyName (the order runs) the prefix also admits
\* an earlier Steer input of that key while the batch input's run is going,
\* and TLC explores both Steer inputs through the same actions.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* requester_batch_audit.sh (same constants and every generated invariant,
\* plus the audit values, properties and step bound), so a DSL change never
\* leaves the audit on a stale constant block. The runner checks:
\*   - requester safety, for each pair of different keys of the batch input
\*     and the late input (key_a and key_b, key_a and none, none and key_a):
\*     the late input is never associated with the batch input's run, and
\*     the bound inputs of each run share one key;
\*   - order safety, for each pair of an earlier input of another key and a
\*     late input of the batch input's key (key_b, none and key_a behind a
\*     key_a run; key_a behind a run without a key): no step binds an input
\*     waiting in the Steer lane to a run (a join, or staging into the
\*     running run or a follow-up run) while a Steer input admitted before it
\*     waits in the Steer lane with another key, and the bound inputs of each
\*     run share one key;
\*   - witnesses: a late input of the batch input's key joins the running
\*     run (AuditLateNeverJoins violated); a late input of another key is
\*     staged into a run of its own (AuditLateNeverRunsAlone violated); with
\*     an earlier input of another key waiting, the earlier input runs in the
\*     follow-up run (AuditEarlyNeverRunsAlone violated), and the late input
\*     joins the running run once the earlier one left the Steer lane
\*     (AuditLateNeverJoins violated);
\*   - mutants, on a copy of the generated model: with the join guard
\*     live_join_same_requester removed, or with admission no longer
\*     recording an ungoverned input's key, the key_a and key_b requester run
\*     fails; with ResumeRun staging no batch key, the key_a and none
\*     requester run fails; with the order guard removed from
\*     JoinLiveBoundaryDurableAppend, or from StageForRun, the key_b order run
\*     fails.
\* The canonical TLC lane (crates/xtask/tests/machine_verify_all_tlc_test.sh)
\* runs it with --mutants at 14 steps; run a deeper bound by hand with:
\*   specs/machines/meerkat_machine/requester_batch_audit.sh 16 --mutants
\*
\* Scope limits: the generated model has no crash action, so a restart's
\* recovery of the kept rows' keys (BindInputRequesterBatchKey) is outside
\* this audit; the resume it explores is a fence gap's, whose ResumeRun is
\* the step a restart's resume takes too. The Rust row
\* a_person_s_run_resumed_after_a_restart_keeps_the_person_s_key covers the
\* restart: its recovery binds the kept input's key, the resumed run stages
\* it, and a requester-less steer may not join. host_requester_survives_restart
\* checks the requester on the input's receipt across a restart. A Steer
\* input the runtime normalized to the Queue lane after its own live
\* delivery was unavailable (LiveBoundaryUnavailable) is Queue-lane work from
\* then on, outside the Steer lane's order; the runtime never normalizes a
\* steer the machine refused to join (the Rust rows
\* an_earlier_steer_of_another_requester_runs_before_a_later_one and
\* a_requester_less_steer_keeps_its_place_ahead_of_a_later_steer).
EXTENDS model

\* Upper bound on model_step_count, including the deterministic prefix.
CONSTANT AuditMaxSteps

\* The keys of the batch input and of the late input: "none" for no key.
CONSTANT AuditBatchKeyName
CONSTANT AuditLateKeyName

\* The key of the earlier Steer input ("none" for no key), or "absent" for no
\* earlier Steer input.
CONSTANT AuditEarlyKeyName

AuditRun == "runid_1"
AuditFollowUpRun == "runid_2"
AuditSession == "sessionid_1"
AuditBatch == "input_batch"
AuditEarly == "input_early"
AuditLate == "input_late"
AuditHasEarly == AuditEarlyKeyName # "absent"
AuditSteers == IF AuditHasEarly THEN {AuditEarly, AuditLate} ELSE {AuditLate}
AuditInputs == {AuditBatch} \cup AuditSteers
AuditRuns == {AuditRun, AuditFollowUpRun}
AuditLanes == {"Queue", "Steer"}
AuditPrefixLength == IF AuditHasEarly THEN 10 ELSE 8

AuditKey(name) == IF name = "none" THEN None ELSE Some(name)

\* A Steer input's admission while a run is going, under every turn-append
\* shape value; admission refuses Untyped for a content kind.
AuditSteerAdmission(input, key_name) ==
    \E shape \in AdmissionTurnAppendShapeValues :
        ResolveAdmissionPlanRequestedSteerRunning(input, None, AuditKey(key_name), "Prompt", Some("Steer"), "Ordinary", shape, FALSE, None, TRUE, TRUE, FALSE)

\* A single deterministic prefix reaches the interesting state: a registered
\* session running one runtime-loop run whose batch input, admitted with its
\* key and no governed binding, is Staged and whose turn is at a model
\* boundary (and, when the order runs, the earlier Steer input admitted and
\* accepted). Everything after it is explored exhaustively.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ ResolveAdmissionPlanDefaultQueueKindIdle(AuditBatch, None, AuditKey(AuditBatchKeyName), "Prompt", None, "Ordinary", "InTurnEligible", FALSE, None, FALSE, FALSE, FALSE)
    \/ model_step_count = 3 /\ QueueAcceptedIdle(AuditBatch)
    \/ model_step_count = 4 /\ PrepareIdle(AuditSession, AuditRun)
    \/ model_step_count = 5 /\ StageForRunRunning(AuditBatch, AuditRun)
    \/ model_step_count = 6 /\ StartConversationRunRunning(AuditRun, "ConversationTurn", "conversation", FALSE, FALSE, 0)
    \/ model_step_count = 7 /\ PrimitiveAppliedConversation(AuditRun)
    \/ AuditHasEarly /\ model_step_count = 8 /\ AuditSteerAdmission(AuditEarly, AuditEarlyKeyName)
    \/ AuditHasEarly /\ model_step_count = 9 /\ SteerAcceptedRunning(AuditEarly)

AuditLateAdmission ==
    \/ AuditSteerAdmission(AuditLate, AuditLateKeyName)
    \/ SteerAcceptedRunning(AuditLate)

AuditTurn ==
    \/ LlmReturnedToolCallsPositive(AuditRun, 1)
    \/ ToolCallsResolvedToCalling(AuditRun)
    \/ LlmReturnedTerminal(AuditRun)
    \/ RequestCancelAfterBoundary(AuditRun)
    \/ CancelNow(AuditRun)
    \/ CancellationObserved(AuditRun)
    \/ FatalFailure(AuditRun, "Llm", "alpha")
    \/ RunCompleted(AuditRun)
    \/ RunFailed(AuditRun, None, None, FALSE, None, "alpha")
    \/ RunCancelled(AuditRun)

AuditLiveBoundary ==
    \/ \E input \in AuditSteers :
        \/ JoinLiveBoundaryDurableAppendRunning(AuditRun, input)
        \/ \E lane \in AuditLanes : \E observation \in LiveBoundaryJoinObservationValues :
            \/ ResolveLiveBoundaryDurableAppendJoinNotAppliedRunning(AuditRun, input, lane, observation)
            \/ ResolveLiveBoundaryDurableAppendJoinAppliedDiscardedRunning(AuditRun, input, lane, observation)
            \/ ResolveLiveBoundaryDurableAppendJoinAppliedRetainedRunning(AuditRun, input, lane, observation)
        \/ LiveBoundaryUnavailableRunning(input)
    \/ \E input \in AuditInputs : ResolveLiveBoundaryContextReceiptRunning(AuditRun, input)
    \/ \E sequence \in 1..2 : CommitTerminalBoundarySequenceRunning(AuditRun, sequence)

\* Staging a Steer input into a running run: the request-only live-boundary
\* realization stages a Steer input into the current run, and a follow-up
\* run stages it into a run of its own.
AuditStaging ==
    \/ \E input \in AuditSteers : \E run \in AuditRuns : StageForRunRunning(input, run)
    \/ PrepareIdle(AuditSession, AuditFollowUpRun)

\* A store fence gap suspends the batch input's run: the run stops being
\* current and is marked resumable, its bound inputs keep it. The
\* registration starts the marked run again under its own id (the ordinary
\* Prepare, which a marked run admits only for itself) and resumes it.
\*
\* The runtime applies ResumeRun only with the capability the generated
\* resume plan mints (authorize_resumed_run_from_state), which resumes the
\* marked run with exactly the inputs bound to it and never without one.
\* AuditInputLifecycle's ConsumeInputIdle (no phase guard) can consume the
\* suspended run's last bound input, a step the runtime takes only at a run's
\* terminal; the resume arm keeps the plan's condition, so the audit never
\* resumes an emptied run.
AuditResumePlanned ==
    \E input \in AuditInputs :
        /\ input \in DOMAIN input_run_associations
        /\ input_run_associations[input] = AuditRun
        /\ input \in DOMAIN input_phases
        /\ input_phases[input] \in {"Staged", "Applied", "AppliedPendingConsumption"}

AuditResume ==
    \/ SuspendRunForStopFenceGapRunningToIdle(AuditRun, TRUE, "FenceGap")
    \/ resumable_run_id = Some(AuditRun) /\ PrepareIdle(AuditSession, AuditRun)
    \/ /\ AuditResumePlanned
       /\ ResumeRunRunning(AuditRun)

AuditInputLifecycle ==
    \/ \E input \in AuditInputs : MarkAppliedRunning(input)
    \/ \E input \in AuditInputs : MarkAppliedPendingConsumptionRunning(input)
    \/ \E input \in AuditInputs : RecordBoundarySeqRunning(input, AuditRun, Some("RunCheckpoint"))
    \/ \E input \in AuditInputs : ConsumeInputRunning(input)
    \/ \E input \in AuditInputs : ConsumeInputIdle(input)
    \/ \E input \in AuditInputs : AbandonInputRunning(input, "Cancelled", 1)
    \/ \E input \in AuditInputs : \E lane \in AuditLanes :
        \/ ResolveStagedRollbackQueuedRunning(input, lane)
        \/ ChangeLaneRunning(input, lane)

AuditRunEnding ==
    \/ \E input \in AuditInputs : CommitRunningToIdle(input, AuditRun)
    \/ FailRunningToIdle(AuditRun)
    \/ CancelRunningToIdle(AuditRun)
    \/ RollbackRunRunningToIdle(AuditRun)
    \/ ServiceTurnCommittedRunningToIdle(AuditRun)
    \/ RuntimeExecutorExitedFromRunning

AuditBody ==
    \/ AuditLateAdmission
    \/ AuditTurn
    \/ AuditLiveBoundary
    \/ AuditStaging
    \/ AuditResume
    \/ AuditInputLifecycle
    \/ AuditRunEnding

AuditNext ==
    \/ AuditPrefix
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditSpec == Init /\ [][AuditNext]_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditBoundPhases == {"Staged", "Applied", "AppliedPendingConsumption"}

AuditKeyOf(input) ==
    IF input \in DOMAIN input_authority_batch_keys
        THEN Some(input_authority_batch_keys[input])
        ELSE None

AuditBoundTo(input, run) ==
    /\ input \in DOMAIN input_run_associations
    /\ input_run_associations[input] = run
    /\ input \in DOMAIN input_phases
    /\ input_phases[input] \in AuditBoundPhases

AuditBound(input) == \E run \in AuditRuns : AuditBoundTo(input, run)

\* Safety, in the machine's own vocabulary: the inputs bound to one run share
\* one key (absence included).
AuditRunsShareOneKey ==
    \A run \in AuditRuns : \A left, right \in AuditInputs :
        (AuditBoundTo(left, run) /\ AuditBoundTo(right, run))
            => AuditKeyOf(left) = AuditKeyOf(right)

\* Safety, in the audit's own vocabulary (the keys the inputs were admitted
\* with, which the mutants cannot hide): the late input, whose key differs
\* from the batch input's in every requester run, is never associated with
\* the batch input's run, neither joined nor staged.
AuditLateNeverInBatchRun ==
    ~ ( /\ AuditLate \in DOMAIN input_run_associations
        /\ input_run_associations[AuditLate] = AuditRun )

AuditWaitsInSteerLane(input) ==
    /\ input \in DOMAIN input_lane
    /\ input_lane[input] = "Steer"
    /\ input \in DOMAIN input_phases
    /\ input_phases[input] = "Queued"

AuditAdmittedBefore(earlier, later) ==
    /\ earlier \in DOMAIN input_admission_seq
    /\ later \in DOMAIN input_admission_seq
    /\ input_admission_seq[earlier] < input_admission_seq[later]

\* Safety (action property), the Steer lane's order across requesters: no
\* step binds an input waiting in the Steer lane to a run (a durable join, or
\* staging into the running run or a follow-up run) while a Steer input
\* admitted before it waits in the Steer lane with another key. An input
\* normalized to the Queue lane (LiveBoundaryUnavailable) is Queue-lane work
\* from then on, outside the Steer lane's order (see the scope limits): the
\* runtime stages Queue-lane work only at a run's start, after the Steer lane,
\* while this audit's staging arm also stages it into the running run.
AuditSteerLaneFifoStep ==
    \A later \in AuditSteers : \A earlier \in AuditSteers :
        ( /\ earlier # later
          /\ AuditWaitsInSteerLane(later)
          /\ ~AuditBound(later)
          /\ AuditBound(later)'
          /\ AuditWaitsInSteerLane(earlier)
          /\ AuditAdmittedBefore(earlier, later) )
            => AuditKeyOf(earlier) = AuditKeyOf(later)

AuditSteerLaneFifo == [][AuditSteerLaneFifoStep]_vars

\* Reachability goal, never a safety property: the late input joins the
\* running run (with the batch input's key; with an earlier input of another
\* key waiting, once that input left the Steer lane).
AuditLateNeverJoins == AuditLate \notin DOMAIN input_live_boundary_join_run

\* Reachability goal, never a safety property: with another key the late
\* input is staged into a run of its own.
AuditLateNeverRunsAlone == ~ AuditBoundTo(AuditLate, AuditFollowUpRun)

\* Reachability goal, never a safety property: the earlier input of another
\* key runs in the follow-up run, the run of its own it waited for.
AuditEarlyNeverRunsAlone == ~ AuditBoundTo(AuditEarly, AuditFollowUpRun)
====
