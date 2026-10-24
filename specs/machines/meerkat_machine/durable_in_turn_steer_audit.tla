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
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* durable_in_turn_steer_audit.sh (same constants and every generated
\* invariant, plus the audit values, invariants and step bound below), so a
\* DSL change can never leave the audit on a stale constant block or a
\* narrower invariant set. The canonical TLC lane
\* (crates/xtask/tests/machine_verify_all_tlc_test.sh, `make machine-verify`,
\* the machine-codegen-verify pre-push hook and the nightly machine-verify
\* job) runs it at a lane-sized bound; run a deeper bound by hand with:
\*   specs/machines/meerkat_machine/durable_in_turn_steer_audit.sh 22
\*
\* The safety run admits the late input as a Prompt. That stands for every
\* content input kind: the admission writes the kind only into
\* input_is_prompt, whose values nothing in the generated model reads, so each
\* kind reaches the Prompt run's states up to that flag (at bound 16 a
\* PeerMessage-only run reaches exactly as many states as the Prompt run).
\* The runner checks both premises on the generated model and fails if either
\* stops holding; DURABLE_STEER_AUDIT_ALL_KINDS=1 runs the safety run over
\* every content kind. The runner then requires, once per content kind, that
\* TLC reaches a durable join of the late input (AuditLateNeverJoins violated
\* with AuditLateKinds narrowed to that kind), so the admission lets every
\* kind join and the exactly-once invariants are not vacuous for any of them.
\*
\* The runner also substitutes a small run contributor bound for the
\* generated 256 (one, then two), so TLC explores the durable join the
\* run_contributor_capacity guard refuses and the one it takes at the bound.
\*
\* Scope limit: the generated model has no crash action, so durable-row
\* recovery (RecoverInputLifecycle and durable-tail adoption after a process
\* dies mid-run) is outside this audit. The Rust test
\* persistent_crash_after_the_join_recovers_the_input_for_exactly_one_follow_up
\* covers that path.
EXTENDS model

\* Upper bound on model_step_count, including the deterministic prefix.
CONSTANT AuditMaxSteps

\* Admission input kinds the late Steer input may take: Prompt in the safety
\* run (every content kind on request), one kind per reachability goal run.
CONSTANT AuditLateKinds

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
\* Both admissions retain the ordinary unbound path: authority_binding and
\* authority_batch_key are None; this audit does not supply governed authority.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ ResolveAdmissionPlanDefaultQueueKindIdle(AuditBatch, None, None, "Prompt", None, "Ordinary", "InTurnEligible", FALSE, None, FALSE, FALSE, FALSE)
    \/ model_step_count = 3 /\ QueueAcceptedIdle(AuditBatch)
    \/ model_step_count = 4 /\ PrepareIdle(AuditSession, AuditRun)
    \/ model_step_count = 5 /\ StageForRunRunning(AuditBatch, AuditRun)
    \/ model_step_count = 6 /\ StartConversationRunRunning(AuditRun, "ConversationTurn", "conversation", FALSE, FALSE, 0)
    \/ model_step_count = 7 /\ PrimitiveAppliedConversation(AuditRun)

\* The late Steer input of the audited kinds under every turn-append shape
\* value; admission refuses Untyped for a content kind.
AuditLateAdmission ==
    \/ \E kind \in AuditLateKinds : \E shape \in AdmissionTurnAppendShapeValues :
        ResolveAdmissionPlanRequestedSteerRunning(AuditLate, None, None, kind, Some("Steer"), "Ordinary", shape, FALSE, None, TRUE, TRUE, FALSE)
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

AuditStateConstraint == model_step_count <= AuditMaxSteps

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

\* Reachability goal, never a safety property: the runner requires TLC to
\* report it violated for each content kind, proving the late input of that
\* kind joins the running run within the bound.
AuditLateNeverJoins == AuditLate \notin DOMAIN input_live_boundary_join_run

\* Run contributor bound. The generated join guard (run_contributor_capacity)
\* refuses a durable join once the run's contributors, its batch and its
\* joins (staged, or applied by a resumed run's committed round, which this
\* model never reaches: it has no crash), reach
\* meerkat_machine_run_contributor_limit: 256 in the
\* generated model, which no bounded run reaches. The contributor-bound runs
\* substitute a small bound for it in their config
\* (meerkat_machine_run_contributor_limit <- AuditOneContributor), so TLC
\* explores the join the guard refuses: with a bound of one the batch input
\* alone fills the run, and with two exactly one join fits.
AuditOneContributor == 1
AuditTwoContributors == 2

\* The audited run never holds more contributors than the bound in force.
AuditRunContributorsWithinBound ==
    meerkat_machine_run_contributor_count(input_run_associations, input_phases, AuditRun)
        <= meerkat_machine_run_contributor_limit

\* Reachability goal, never a safety property: the late Steer input waits at
\* a model boundary of the audited run, queued in the Steer lane with a
\* durable delivery, while the run already holds the bound's worth of
\* contributors, so the contributor guard is what keeps it out of the run.
\* The runner requires TLC to report it violated under a bound of one.
AuditCapacityRefusalNeverReached ==
    ~ ( /\ current_run_id = Some(AuditRun)
        /\ turn_phase = "CallingLlm"
        /\ IF AuditLate \in DOMAIN input_lane THEN input_lane[AuditLate] = "Steer" ELSE FALSE
        /\ IF AuditLate \in DOMAIN input_live_boundary_delivery
               THEN input_live_boundary_delivery[AuditLate] = "DurableAppend"
               ELSE FALSE
        /\ meerkat_machine_run_contributor_count(input_run_associations, input_phases, AuditRun)
               >= meerkat_machine_run_contributor_limit )
====
