---- MODULE requester_batch_audit ----
\* Hand-written bounded audit of the requester batch rule: inputs of different
\* requesters never share a run.
\*
\* An admitted input's batch key (input_authority_batch_keys) is its governed
\* binding's batch identity or, for an ungoverned input whose host supplied a
\* requester, that requester's key; an ungoverned input without a requester
\* has none, and absence is a key value too. The machine keeps one key per
\* run: StageForRun's homogeneous_run_authority stages only inputs of the key
\* the run staged first, and JoinLiveBoundaryDurableAppend's
\* live_join_same_requester lets an input join a running run only when its
\* key is the key that run staged.
\*
\* The audit keeps the generated model's variables, Init, actions and
\* invariants unchanged. A deterministic prefix starts one runtime-loop run
\* whose batch input carries the key named by AuditBatchKeyName; then TLC
\* explores one late Steer input of the key named by AuditLateKeyName: its
\* admission (every turn-append shape), acceptance, durable join and its
\* three resolutions, live-boundary normalization, staging into the running
\* run, the turn phases and every run-ending arm, and a follow-up run that
\* the late input may be staged into.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* requester_batch_audit.sh (same constants and every generated invariant,
\* plus the audit values, invariants and step bound), so a DSL change never
\* leaves the audit on a stale constant block. The runner checks:
\*   - safety, for each pair of different keys (key_a and key_b, key_a and
\*     none, none and key_a): the late input is never associated with the
\*     batch input's run, and the bound inputs of each run share one key;
\*   - witnesses: a late input of the batch input's key joins the running run
\*     (AuditLateNeverJoins violated), and a late input of another key is
\*     staged into a run of its own (AuditLateNeverRunsAlone violated);
\*   - mutants, on a copy of the generated model: with the join guard
\*     live_join_same_requester removed, or with admission no longer
\*     recording an ungoverned input's key, the key_a and key_b safety run
\*     fails.
\* The canonical TLC lane (crates/xtask/tests/machine_verify_all_tlc_test.sh)
\* runs it with --mutants at 14 steps; run a deeper bound by hand with:
\*   specs/machines/meerkat_machine/requester_batch_audit.sh 16 --mutants
\*
\* Scope limit: the generated model has no crash action, so the recovered
\* rows' keys (BindInputRequesterBatchKey) are outside this audit. The Rust
\* test host_requester_survives_restart covers that path.
EXTENDS model

\* Upper bound on model_step_count, including the deterministic prefix.
CONSTANT AuditMaxSteps

\* The keys of the batch input and of the late input: "none" for no key.
CONSTANT AuditBatchKeyName
CONSTANT AuditLateKeyName

AuditRun == "runid_1"
AuditFollowUpRun == "runid_2"
AuditSession == "sessionid_1"
AuditBatch == "input_batch"
AuditLate == "input_late"
AuditInputs == {AuditBatch, AuditLate}
AuditRuns == {AuditRun, AuditFollowUpRun}
AuditLanes == {"Queue", "Steer"}
AuditPrefixLength == 8

AuditKey(name) == IF name = "none" THEN None ELSE Some(name)

\* A single deterministic prefix reaches the interesting state: a registered
\* session running one runtime-loop run whose batch input, admitted with its
\* key and no governed binding, is Staged and whose turn is at a model
\* boundary. Everything after it is explored exhaustively.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ ResolveAdmissionPlanDefaultQueueKindIdle(AuditBatch, None, AuditKey(AuditBatchKeyName), "Prompt", None, "Ordinary", "InTurnEligible", FALSE, None, FALSE, FALSE, FALSE)
    \/ model_step_count = 3 /\ QueueAcceptedIdle(AuditBatch)
    \/ model_step_count = 4 /\ PrepareIdle(AuditSession, AuditRun)
    \/ model_step_count = 5 /\ StageForRunRunning(AuditBatch, AuditRun)
    \/ model_step_count = 6 /\ StartConversationRunRunning(AuditRun, "ConversationTurn", "conversation", FALSE, FALSE, 0)
    \/ model_step_count = 7 /\ PrimitiveAppliedConversation(AuditRun)

\* The late Steer input under every turn-append shape value; admission
\* refuses Untyped for a content kind.
AuditLateAdmission ==
    \/ \E shape \in AdmissionTurnAppendShapeValues :
        ResolveAdmissionPlanRequestedSteerRunning(AuditLate, None, AuditKey(AuditLateKeyName), "Prompt", Some("Steer"), "Ordinary", shape, FALSE, None, TRUE, TRUE, FALSE)
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
    \/ JoinLiveBoundaryDurableAppendRunning(AuditRun, AuditLate)
    \/ \E lane \in AuditLanes : \E observation \in LiveBoundaryJoinObservationValues :
        \/ ResolveLiveBoundaryDurableAppendJoinNotAppliedRunning(AuditRun, AuditLate, lane, observation)
        \/ ResolveLiveBoundaryDurableAppendJoinAppliedDiscardedRunning(AuditRun, AuditLate, lane, observation)
        \/ ResolveLiveBoundaryDurableAppendJoinAppliedRetainedRunning(AuditRun, AuditLate, lane, observation)
    \/ LiveBoundaryUnavailableRunning(AuditLate)
    \/ \E input \in AuditInputs : ResolveLiveBoundaryContextReceiptRunning(AuditRun, input)
    \/ \E sequence \in 1..2 : CommitTerminalBoundarySequenceRunning(AuditRun, sequence)

\* Staging the late input into a running run: the request-only live-boundary
\* realization stages a Steer input into the current run, and the follow-up
\* run stages it into a run of its own.
AuditStaging ==
    \/ \E run \in AuditRuns : StageForRunRunning(AuditLate, run)
    \/ PrepareIdle(AuditSession, AuditFollowUpRun)

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

\* Safety, in the machine's own vocabulary: the inputs bound to one run share
\* one key (absence included).
AuditRunsShareOneKey ==
    \A run \in AuditRuns : \A left, right \in AuditInputs :
        (AuditBoundTo(left, run) /\ AuditBoundTo(right, run))
            => AuditKeyOf(left) = AuditKeyOf(right)

\* Safety, in the audit's own vocabulary (the keys the inputs were admitted
\* with, which the mutants cannot hide): the late input, whose key differs
\* from the batch input's in every safety run, is never associated with the
\* batch input's run, neither joined nor staged.
AuditLateNeverInBatchRun ==
    ~ ( /\ AuditLate \in DOMAIN input_run_associations
        /\ input_run_associations[AuditLate] = AuditRun )

\* Reachability goal, never a safety property: with the batch input's key the
\* late input joins the running run.
AuditLateNeverJoins == AuditLate \notin DOMAIN input_live_boundary_join_run

\* Reachability goal, never a safety property: with another key the late
\* input is staged into a run of its own.
AuditLateNeverRunsAlone == ~ AuditBoundTo(AuditLate, AuditFollowUpRun)
====
