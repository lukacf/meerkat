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
\* The recovery scene. The generated model has no crash action, so the scene
\* scripts what a start over the same store does: it registers the session,
\* marks the run the stored cursor names resumable, recovers the batch input
\* Staged and bound to that run and the late input Queued from one stored
\* row, and resumes the run. Each run of the scene is given the set of rows
\* the late input may be recovered from (its lane, its stage attempts, the
\* run it still names, the delivery class it stores) and whether the row is
\* governed, so each run asks one question:
\*   - the row of a steer that never left the Steer lane (no attempt, no run,
\*     the durable class) keeps its class and joins the resumed run (the
\*     runner requires AuditLateNeverJoins violated);
\*   - a row with a stage attempt, a row that still names a run, a row in the
\*     Queue lane and a row that stores no class hold no class after
\*     recovery and never join (one run over all four);
\*   - a governed row that never left loses its class at the binding
\*     recovery restores, and never joins.
\* With --mutants the runner seeds four defects into a copy of the generated
\* model (a recovery that ignores the attempts, the run or the lane, and a
\* binding that keeps the class) and requires TLC to refuse each.
\* The scene explores what the audit's main body explores after its prefix,
\* without a second admission of the late input.
\*
\* Scope limit of the scene: durable-tail adoption (which stored tail a start
\* adopts, and which appends it holds) is not modelled; the scene takes the
\* stored row as given. The Rust test
\* persistent_crash_after_the_join_recovers_the_input_for_exactly_one_follow_up
\* covers a join that a crash interrupts.
EXTENDS model

\* Upper bound on model_step_count, including the deterministic prefix.
CONSTANT AuditMaxSteps

\* Admission input kinds the late Steer input may take: Prompt in the safety
\* run (every content kind on request), one kind per reachability goal run.
CONSTANT AuditLateKinds

\* The stored rows the recovery scene may recover the late input from, and
\* whether the recovered row is governed. The main audit does not read them;
\* the runner gives every run a value.
CONSTANTS AuditRows, AuditRowGoverned

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
\* ---------------------------------------------------------------------------
\* The recovery scene.
\* ---------------------------------------------------------------------------

\* The row of a steer that was admitted with the durable class and never left
\* the Steer lane.
AuditRowsThatNeverLeft ==
    {[lane |-> "Steer", attempts |-> 0, run |-> None, class |-> Some("DurableAppend")]}

\* One row per way of having left it: a stage attempt (staged, then rolled
\* back, replayed or deferred), a run still named (a join whose applied image
\* was discarded), the Queue lane, and no stored class.
AuditRowsThatLeft ==
    {[lane |-> "Steer", attempts |-> 1, run |-> None, class |-> Some("DurableAppend")],
     [lane |-> "Steer", attempts |-> 0, run |-> Some(AuditRun), class |-> Some("DurableAppend")],
     [lane |-> "Queue", attempts |-> 0, run |-> None, class |-> Some("DurableAppend")],
     [lane |-> "Steer", attempts |-> 0, run |-> None, class |-> None]}

\* A governed row is bound again after its lifecycle, one step more.
AuditRecoveryBindSteps == IF AuditRowGoverned THEN 1 ELSE 0
AuditRecoveryPrefixLength == 11 + AuditRecoveryBindSteps

\* The generated lifecycle guard requires the row's lane to be the lane of the
\* admission witness, so the row is in effect chosen at the witness step.
AuditRecoveryPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ MarkRunResumableIdle(AuditRun)
    \/ model_step_count = 3
       /\ RecoverAdmittedInputIdle(AuditBatch, "Prompt", "RunStart", "ContentTurn", None, "Queue")
    \/ model_step_count = 4
       /\ RecoverInputLifecycleIdle(AuditBatch, "Staged", None, None, None, None, 0, 1,
             Some(AuditRun), None, Some(1), None, Some("Queue"), None, None, None, None,
             FALSE, None)
    \/ model_step_count = 5
       /\ \E row \in AuditRows :
            RecoverAdmittedInputIdle(AuditLate, "PeerMessage", "RunCheckpoint", "ContentTurn",
                None, row.lane)
    \/ model_step_count = 6
       /\ \E row \in AuditRows :
            RecoverInputLifecycleIdle(AuditLate, "Queued", None, None, None, None, 0,
                row.attempts, row.run, None, Some(2), None, Some(row.lane), Some(row.lane),
                Some("RunCheckpoint"), Some("ContentTurn"), None, FALSE, row.class)
    \/ model_step_count = 7 /\ AuditRowGoverned
       /\ BindInputAuthorityIdle(AuditLate, "binding_late", "batch_late")
    \/ model_step_count = 7 + AuditRecoveryBindSteps /\ PrepareIdle(AuditSession, AuditRun)
    \/ model_step_count = 8 + AuditRecoveryBindSteps /\ ResumeRunRunning(AuditRun)
    \/ model_step_count = 9 + AuditRecoveryBindSteps
       /\ StartConversationRunRunning(AuditRun, "ConversationTurn", "conversation", FALSE, FALSE, 0)
    \/ model_step_count = 10 + AuditRecoveryBindSteps /\ PrimitiveAppliedConversation(AuditRun)

AuditRecoveryBody ==
    \/ AuditTurn
    \/ AuditLiveBoundary
    \/ AuditInputLifecycle
    \/ AuditRunEnding

AuditRecoveryNext ==
    \/ AuditRecoveryPrefix
    \/ (model_step_count >= AuditRecoveryPrefixLength /\ AuditRecoveryBody)

AuditRecoverySpec == Init /\ [][AuditRecoveryNext]_vars

\* The recovered input holds no delivery class. Checked in the runs whose rows
\* all left the Steer lane: nothing in the scene admits the late input again,
\* so a class it held would be one recovery gave it. With AuditLateNeverJoins
\* in the same run this is exactly once across the restart: a row that names
\* a run (its applied append may sit in an adopted tail) or counts a stage
\* attempt never joins.
AuditLateHoldsNoClass == AuditLate \notin DOMAIN input_live_boundary_delivery

\* A governed recovered input holds no class once its binding is restored.
AuditGovernedRecoveredInputHoldsNoClass ==
    AuditLate \in DOMAIN input_authority_bindings
        => AuditLate \notin DOMAIN input_live_boundary_delivery
====
