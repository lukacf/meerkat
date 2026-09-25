---- MODULE durable_in_turn_steer_audit ----
\* Hand-written bounded audit of durable in-turn Steer delivery.
\*
\* The generated MeerkatMachine model has roughly nine hundred actions, so its
\* ci profile stops after one step and its deep profile does not finish on any
\* local or CI budget. This audit keeps the generated model's variables, Init,
\* actions and invariants unchanged and explores only the actions that touch the
\* input lifecycle of one runtime-loop run with one batch input and one late
\* Steer input: admission (every turn-append shape), acceptance, staging, the
\* turn phases that open and close model boundaries, the durable join and its
\* three terminal resolutions, live-boundary normalization and checkpoint
\* receipts, boundary application and consumption, staged rollback and
\* abandonment, every run-ending arm (commit, fail, cancel, rollback, service
\* turn commit, executor exit), and lane changes.
\*
\* Run with:
\*   JDK_JAVA_OPTIONS=-Xss1g JAVA_TOOL_OPTIONS='-Xss1g -XX:+UseParallelGC' \
\*     tlc -workers auto -config audit.cfg durable_in_turn_steer_audit.tla
EXTENDS model

AuditRun == "runid_1"
AuditSession == "sessionid_1"
AuditBatch == "input_batch"
AuditLate == "input_late"
AuditInputs == {AuditBatch, AuditLate}
AuditLanes == {"Queue", "Steer"}
AuditPrefixLength == 8

\* A single deterministic prefix reaches the interesting state: a registered
\* session running one runtime-loop run whose batch input is Staged and whose
\* turn is at a model boundary. Everything after it is explored exhaustively.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None)
    \/ model_step_count = 2 /\ ResolveAdmissionPlanDefaultQueueKindIdle(AuditBatch, "Prompt", None, "Ordinary", "Untyped", FALSE, None, FALSE, FALSE, FALSE)
    \/ model_step_count = 3 /\ QueueAcceptedIdle(AuditBatch)
    \/ model_step_count = 4 /\ PrepareIdle(AuditSession, AuditRun)
    \/ model_step_count = 5 /\ StageForRunRunning(AuditBatch, AuditRun)
    \/ model_step_count = 6 /\ StartConversationRunRunning(AuditRun, "ConversationTurn", "conversation", FALSE, FALSE, 0)
    \/ model_step_count = 7 /\ PrimitiveAppliedConversation(AuditRun)

\* The late Steer input under every turn-append shape the shell can report.
AuditLateAdmission ==
    \/ \E shape \in AdmissionTurnAppendShapeValues :
        ResolveAdmissionPlanRequestedSteerRunning(AuditLate, "Prompt", Some("Steer"), "Ordinary", shape, FALSE, None, TRUE, TRUE, FALSE)
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
    \/ AuditInputLifecycle
    \/ AuditRunEnding

AuditNext ==
    \/ AuditPrefix
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditSpec == Init /\ [][AuditNext]_vars

AuditStateConstraint == model_step_count <= 22

\* Exactly-once, checked on every explored state: an input whose join the
\* machine has resolved as Retained (its append survives in the run's image)
\* is never back in a work lane and never abandoned; a Published join is still
\* Staged and recoverable. These restate the generated invariants in the audit
\* vocabulary so a violation names the durable-steer rule directly.
AuditRetainedJoinNeverRequeued ==
    \A input \in DOMAIN input_live_boundary_join_phase :
        input_live_boundary_join_phase[input] = "Retained"
            => /\ input \notin DOMAIN input_lane
               /\ (input \in DOMAIN input_phases => input_phases[input] # "Abandoned")

AuditPublishedJoinIsStaged ==
    \A input \in DOMAIN input_live_boundary_join_phase :
        input_live_boundary_join_phase[input] = "Published"
            => /\ input \in DOMAIN input_phases
               /\ input_phases[input] = "Staged"
               /\ input \notin DOMAIN input_lane

\* A joined input is never stageable for a follow-up while its join is live.
AuditJoinedInputNotInAnyLane ==
    \A input \in DOMAIN input_live_boundary_join_run : input \notin DOMAIN input_lane
====
