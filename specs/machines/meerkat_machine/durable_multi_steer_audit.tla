---- MODULE durable_multi_steer_audit ----
\* Hand-written bounded audit of several durable Steer inputs at one model
\* boundary of one run: two or three Steer inputs of the run's own requester
\* (none) are admitted, in a fixed order, while the run's turn is at its first
\* model boundary, and a further one may be admitted later.
\*
\* What the machine owns here. JoinLiveBoundaryDurableAppend joins one queued
\* Steer input into the running run per step, only while the turn is at a
\* model boundary (turn_at_model_boundary: turn_phase = CallingLlm), only from
\* the Queued phase and the Steer lane, only with a DurableAppend delivery and
\* only once (not_already_joined). AbandonQueuedInput (a recall) abandons an
\* input only from the Queued phase. The machine has no fact for the point at
\* which a model request's input is fixed: to the machine a boundary is one
\* stay of the turn in CallingLlm, and this audit counts those stays itself.
\*
\* The audit keeps the generated model's variables, Init, actions and
\* invariants unchanged and adds four history variables of its own, written
\* by one conjunct of its next-state relation and read only by its
\* properties:
\*   audit_visit      how many times the turn entered CallingLlm;
\*   audit_earliest   per Steer input, the first stay it may join: the stay
\*                    it was accepted in, or the next one when the turn was
\*                    not at a model boundary at its acceptance;
\*   audit_joined_at  per Steer input, the stay of its latest join (0: none);
\*   audit_retained   the Steer inputs whose join was ever resolved Retained.
\*
\* A deterministic prefix starts one runtime-loop run whose batch input is
\* Staged and whose turn is at its first model boundary, then admits and
\* accepts the early Steer inputs in order (input_a, input_b and, with
\* AuditSteerCount 3, input_c), each as a Prompt with an in-turn eligible
\* shape, so each waits in the Steer lane with a DurableAppend delivery and
\* their admission sequence is their order. TLC then explores, for every
\* Steer input, the durable join, its three resolutions (to either lane) and
\* live-boundary normalization; a tool round that leaves the model boundary
\* and comes back to it; the terminal model return; with AuditRecall a recall
\* of any Steer input; and with AuditHasAfter the admission and acceptance of
\* one more Steer input (input_after) at any explored point.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* durable_multi_steer_audit.sh (same constants and every generated
\* invariant, plus the audit values, properties and step bound). The runner
\* checks:
\*   - safety, for three early inputs with recall explored, and for two early
\*     inputs with recall and a later one: a join once resolved Retained is
\*     never undone, requeued or joined again (AuditRetainedOnce, with the
\*     three exactly-once invariants of durable_in_turn_steer_audit); a
\*     recalled input is in no lane, holds no join and was never retained
\*     (AuditRecalledNeverJoined) and stays abandoned (AuditRecalledIsFinal);
\*     a recall changes no other input's place in the Steer lane, delivery or
\*     join (AuditRecallKeepsTheOthers); and no input joins a stay earlier
\*     than the first it may join (AuditJoinNotBeforeItsBoundary);
\*   - witnesses, each a goal TLC must report violated: every early input,
\*     two and three, is joined in the first stay and retained
\*     (AuditEarlyNeverAllRetainedAtFirstBoundary); with the first of three
\*     recalled, and with the middle one recalled, the other two are
\*     (AuditHeadNeverRecalledWithOthersRetained,
\*     AuditMiddleNeverRecalledWithOthersRetained); of three joined in the
\*     first stay the first and third are retained and the second is
\*     returned to the queue, each join resolved on its own
\*     (AuditNeverMixedOutcomesAtFirstBoundary); an input accepted after the
\*     turn left the first stay joins the second, the early ones having
\*     joined the first (AuditAfterNeverJoinsTheNextBoundary);
\*   - mutants (--mutants), on a copy of the generated model: with the join's
\*     model-boundary guard removed, AuditJoinNotBeforeItsBoundary fails;
\*     with the recall's Queued guard removed, AuditRecalledNeverJoined
\*     fails; with a recall that drops every delivery,
\*     AuditRecallKeepsTheOthers fails.
\*
\* Scope limits. The audit admits the Steer inputs as Prompt only; the
\* reasoning that lets Prompt stand for every content kind is
\* durable_in_turn_steer_audit.sh's and is not repeated here. The run-ending
\* arms, cancellation, context receipts and the input lifecycle after a join
\* are explored by durable_in_turn_steer_audit with one late input and are
\* not part of this audit's next-state relation. A recall refused because the
\* input is no longer queued (AbandonQueuedInputNotQueued) changes no state
\* but the step count and is not explored. The generated model has no crash
\* action. "Admitted before the request's input was fixed" is not a machine
\* fact, so the audit cannot state that such an input joins THAT request: it
\* states once-only delivery, recall and the model-boundary guard, and
\* reaches the same-stay outcomes as witnesses. The generated join keeps no
\* order among the inputs of one requester (steer_lane_fifo_across_requesters
\* orders requesters only): that order is the runtime's, and this audit does
\* not state it. The machine also has no fact
\* for an input's registration with the core's boundary: to the machine an
\* accepted Steer input waiting with its delivery is eligible, registered
\* or not. An input whose join was returned to the queue has no delivery
\* left (the join removes it and no resolution restores it), so the machine
\* never joins it again in the same run.
EXTENDS model

\* Upper bound on model_step_count, including the deterministic prefix.
CONSTANT AuditMaxSteps

\* How many Steer inputs the prefix admits at the first model boundary: 2 or 3.
CONSTANT AuditSteerCount

\* Whether one more Steer input may be admitted by the explored steps.
CONSTANT AuditHasAfter

\* Whether a recall of a Steer input is explored.
CONSTANT AuditRecall

VARIABLES audit_visit, audit_earliest, audit_joined_at, audit_retained

AuditRun == "runid_1"
AuditSession == "sessionid_1"
AuditBatch == "input_batch"
AuditFirst == "input_a"
AuditSecond == "input_b"
AuditThird == "input_c"
AuditAfter == "input_after"
AuditEarlySteers ==
    IF AuditSteerCount = 3
        THEN {AuditFirst, AuditSecond, AuditThird}
        ELSE {AuditFirst, AuditSecond}
AuditSteers ==
    IF AuditHasAfter
        THEN AuditEarlySteers \cup {AuditAfter}
        ELSE AuditEarlySteers
AuditLanes == {"Queue", "Steer"}
AuditPrefixLength == 8 + (2 * AuditSteerCount)

\* The reason the runtime's recall gives the machine.
AuditRecallReason == "Cancelled"

\* A Steer input's admission while the run is going, as an ungoverned Prompt
\* without a requester and with an in-turn eligible shape: the admission
\* authorizes a DurableAppend delivery.
AuditSteerAdmission(input) ==
    ResolveAdmissionPlanRequestedSteerRunning(input, None, None, "Prompt", Some("Steer"), "Ordinary", "InTurnEligible", FALSE, None, TRUE, TRUE, FALSE)

AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ ResolveAdmissionPlanDefaultQueueKindIdle(AuditBatch, None, None, "Prompt", None, "Ordinary", "InTurnEligible", FALSE, None, FALSE, FALSE, FALSE)
    \/ model_step_count = 3 /\ QueueAcceptedIdle(AuditBatch)
    \/ model_step_count = 4 /\ PrepareIdle(AuditSession, AuditRun)
    \/ model_step_count = 5 /\ StageForRunRunning(AuditBatch, AuditRun)
    \/ model_step_count = 6 /\ StartConversationRunRunning(AuditRun, "ConversationTurn", "conversation", FALSE, FALSE, 0)
    \/ model_step_count = 7 /\ PrimitiveAppliedConversation(AuditRun)
    \/ model_step_count = 8 /\ AuditSteerAdmission(AuditFirst)
    \/ model_step_count = 9 /\ SteerAcceptedRunning(AuditFirst)
    \/ model_step_count = 10 /\ AuditSteerAdmission(AuditSecond)
    \/ model_step_count = 11 /\ SteerAcceptedRunning(AuditSecond)
    \/ AuditSteerCount = 3 /\ model_step_count = 12 /\ AuditSteerAdmission(AuditThird)
    \/ AuditSteerCount = 3 /\ model_step_count = 13 /\ SteerAcceptedRunning(AuditThird)

AuditAfterAdmission ==
    /\ AuditHasAfter
    /\ \/ AuditSteerAdmission(AuditAfter)
       \/ SteerAcceptedRunning(AuditAfter)

\* One tool round leaves the model boundary and returns to it; the terminal
\* return ends the turn's model requests.
AuditTurn ==
    \/ LlmReturnedToolCallsPositive(AuditRun, 1)
    \/ ToolCallsResolvedToCalling(AuditRun)
    \/ LlmReturnedTerminal(AuditRun)

AuditLiveBoundary ==
    \E input \in AuditSteers :
        \/ JoinLiveBoundaryDurableAppendRunning(AuditRun, input)
        \/ \E lane \in AuditLanes : \E observation \in LiveBoundaryJoinObservationValues :
            \/ ResolveLiveBoundaryDurableAppendJoinNotAppliedRunning(AuditRun, input, lane, observation)
            \/ ResolveLiveBoundaryDurableAppendJoinAppliedDiscardedRunning(AuditRun, input, lane, observation)
            \/ ResolveLiveBoundaryDurableAppendJoinAppliedRetainedRunning(AuditRun, input, lane, observation)
        \/ LiveBoundaryUnavailableRunning(input)

AuditRecallStep ==
    /\ AuditRecall
    /\ \E input \in AuditSteers : AbandonQueuedInputRunning(input, AuditRecallReason)

AuditBody ==
    \/ AuditAfterAdmission
    \/ AuditTurn
    \/ AuditLiveBoundary
    \/ AuditRecallStep

AuditStep ==
    \/ AuditPrefix
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

\* The history variables follow the step the model took: they read the
\* model's variables before and after it and constrain none of them.
AuditHistory ==
    /\ audit_visit' =
        IF turn_phase # "CallingLlm" /\ turn_phase' = "CallingLlm"
            THEN audit_visit + 1
            ELSE audit_visit
    /\ audit_earliest' = [steer \in AuditSteers |->
        IF steer \notin DOMAIN input_phases /\ steer \in DOMAIN input_phases'
            THEN (IF turn_phase = "CallingLlm" THEN audit_visit ELSE audit_visit + 1)
            ELSE audit_earliest[steer]]
    /\ audit_joined_at' = [steer \in AuditSteers |->
        IF steer \notin DOMAIN input_live_boundary_join_run /\ steer \in DOMAIN input_live_boundary_join_run'
            THEN audit_visit
            ELSE audit_joined_at[steer]]
    /\ audit_retained' = audit_retained \cup
        {steer \in AuditSteers :
            /\ steer \in DOMAIN input_live_boundary_join_phase'
            /\ input_live_boundary_join_phase'[steer] = "Retained"}

AuditNext == AuditStep /\ AuditHistory

AuditInit ==
    /\ audit_visit = 0
    /\ audit_earliest = [steer \in AuditSteers |-> 0]
    /\ audit_joined_at = [steer \in AuditSteers |-> 0]
    /\ audit_retained = {}

AuditSpec ==
    /\ Init
    /\ AuditInit
    /\ [][AuditNext]_<<vars, audit_visit, audit_earliest, audit_joined_at, audit_retained>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* The three exactly-once invariants of durable_in_turn_steer_audit, which
\* quantify over every joined input.
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

AuditJoinedInputNotInAnyLane ==
    \A input \in DOMAIN input_live_boundary_join_run : input \notin DOMAIN input_lane

AuditJoined(input) == input \in DOMAIN input_live_boundary_join_run

AuditWaitsInSteerLane(input) ==
    /\ input \in DOMAIN input_lane
    /\ input_lane[input] = "Steer"
    /\ input \in DOMAIN input_phases
    /\ input_phases[input] = "Queued"

\* An input the durable join may still take: it waits in the Steer lane with
\* the delivery the join needs.
AuditEligible(input) ==
    /\ AuditWaitsInSteerLane(input)
    /\ input \in DOMAIN input_live_boundary_delivery
    /\ input_live_boundary_delivery[input] = "DurableAppend"

AuditRecalled(input) ==
    /\ input \in DOMAIN input_phases
    /\ input_phases[input] = "Abandoned"

\* Safety: a join once resolved Retained stays Retained while the machine
\* keeps it; its input is never queued, in a lane or published again.
AuditRetainedOnce ==
    \A steer \in audit_retained :
        /\ steer \notin DOMAIN input_lane
        /\ (steer \in DOMAIN input_live_boundary_join_phase
                => input_live_boundary_join_phase[steer] = "Retained")
        /\ (steer \in DOMAIN input_phases => input_phases[steer] # "Queued")

\* Safety: a recalled input is in no lane and holds no join, and no recall
\* takes back an input whose join was retained.
AuditRecalledNeverJoined ==
    \A steer \in AuditSteers :
        AuditRecalled(steer)
            => /\ steer \notin DOMAIN input_live_boundary_join_run
               /\ steer \notin DOMAIN input_live_boundary_join_phase
               /\ steer \notin DOMAIN input_lane
               /\ steer \notin audit_retained

\* Safety (action property): a recalled input stays abandoned.
AuditRecalledIsFinalStep ==
    \A steer \in AuditSteers :
        AuditRecalled(steer)
            => /\ steer \in DOMAIN input_phases'
               /\ input_phases'[steer] = "Abandoned"

AuditRecalledIsFinal == [][AuditRecalledIsFinalStep]_vars

\* Safety (action property): a recall is its own input's alone. The step
\* that abandons one Steer input leaves every other Steer input where it
\* was: waiting with its delivery, or joined. So the recall of the input at
\* the head of the lane never takes the ones behind it out of this boundary.
AuditRecallKeepsTheOthersStep ==
    \A steer \in AuditSteers :
        (~AuditRecalled(steer) /\ AuditRecalled(steer)')
            => \A other \in AuditSteers \ {steer} :
                /\ AuditEligible(other)' = AuditEligible(other)
                /\ AuditJoined(other)' = AuditJoined(other)

AuditRecallKeepsTheOthers == [][AuditRecallKeepsTheOthersStep]_vars

\* Safety: an input never joins a stay earlier than the first it may join.
\* An input accepted while the turn was away from the model boundary joins,
\* at the earliest, the next stay.
AuditJoinNotBeforeItsBoundary ==
    \A steer \in AuditSteers :
        audit_joined_at[steer] # 0 => audit_joined_at[steer] >= audit_earliest[steer]

AuditRetainedAtFirstBoundary(steer) ==
    /\ steer \in audit_retained
    /\ audit_joined_at[steer] = 1

\* Reachability goal, never a safety property: every early input is joined
\* in the first stay at the model boundary and retained.
AuditEarlyNeverAllRetainedAtFirstBoundary ==
    ~ (\A steer \in AuditEarlySteers : AuditRetainedAtFirstBoundary(steer))

AuditOnlyRecalled(recalled) ==
    /\ AuditRecalled(recalled)
    /\ \A steer \in AuditEarlySteers \ {recalled} : AuditRetainedAtFirstBoundary(steer)

\* Reachability goal, never a safety property: with the first early input
\* recalled, the ones admitted after it are joined in the first stay and
\* retained.
AuditHeadNeverRecalledWithOthersRetained == ~ AuditOnlyRecalled(AuditFirst)

\* Reachability goal, never a safety property: with the middle early input
\* recalled, the others are joined in the first stay and retained.
AuditMiddleNeverRecalledWithOthersRetained == ~ AuditOnlyRecalled(AuditSecond)

\* An input joined in the first stay whose join was resolved back to the
\* queue (not applied, or applied and discarded) and never retained.
AuditReturnedAtFirstBoundary(steer) ==
    /\ audit_joined_at[steer] = 1
    /\ ~AuditJoined(steer)
    /\ steer \notin audit_retained
    /\ steer \in DOMAIN input_phases
    /\ input_phases[steer] = "Queued"

\* Reachability goal, never a safety property (three early inputs): each
\* join is resolved on its own. The first and third are retained and the
\* second is returned to the queue, all three joined in the first stay.
AuditNeverMixedOutcomesAtFirstBoundary ==
    ~ ( /\ AuditRetainedAtFirstBoundary(AuditFirst)
        /\ AuditReturnedAtFirstBoundary(AuditSecond)
        /\ AuditRetainedAtFirstBoundary(AuditThird) )

\* Reachability goal, never a safety property (runs with AuditHasAfter): an
\* input accepted after the turn left the first stay joins the second, the
\* early inputs having joined the first.
AuditAfterNeverJoinsTheNextBoundary ==
    ~ ( /\ AuditHasAfter
        /\ audit_earliest[AuditAfter] = 2
        /\ audit_joined_at[AuditAfter] = 2
        /\ \A steer \in AuditEarlySteers : audit_joined_at[steer] = 1 )
====
