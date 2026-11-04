---- MODULE switch_turn_withdrawal_audit ----
\* Hand-written bounded audit of the withdrawal of a failed until-changed
\* switch-turn (AbortUntilChangedSwitchTurnReconfigure).
\*
\* An accepted until-changed request stages a pending switch (and, when it
\* needs approval, an approval entry), then the shell releases the session
\* mutation gate while the LLM reconfigure runs. Other callers may commit
\* meanwhile: an input can be admitted and queued, and the runtime can be
\* stopped, which abandons the queued input and leaves the session Stopped.
\* When the reconfigure fails, the shell withdraws the request through the
\* abort arm, which must withdraw exactly what the request staged and nothing
\* another caller committed: it records no terminal, leaves the lifecycle
\* phase and every input's facts as they are, and fires only for the request
\* that is still the pending until-changed switch.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and drives,
\* for one session, one input and two requests, the initialization, the
\* registration and the routing baseline, then in any order the accepted
\* until-changed request (with and without an approval), the input's
\* admission and queueing, the runtime stop and the executor exit, the
\* input's abandonment in Idle and Stopped, the completion arm, and the abort
\* arm in every phase it declares. A second accepted request replaces a
\* pending one, so the abort of the replaced request is refused there.
\* Properties:
\*
\*   every generated invariant of ci.cfg (kept);
\*   AuditAbortWithdrawsOnlyTheRequest (action property): an abort clears
\*     the pending switch and removes exactly its request's approval
\*     entries, and leaves the phase, the switch terminals and denials, the
\*     baseline and every input's facts unchanged;
\*   AuditAbortOnlyForThePendingRequest (action property): an abort fires
\*     only for the request that is the pending until-changed switch
\*     applying its persistent reconfigure.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous: an approved
\* request withdrawn in Idle while another caller's input is queued, and a
\* request withdrawn in Stopped after a stop abandoned that input. With
\* --mutants the runner also checks that four seeded defects are refused:
\* an abort that records a terminal, an abort that erases the inputs' phases
\* (as the whole-state restore it replaced did), an abort without its
\* pending-request guard, and an abort that keeps its request's approval
\* entry.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* switch_turn_withdrawal_audit.sh.
EXTENDS model

CONSTANT AuditMaxSteps

AuditSession == "sessionid_1"
AuditInput == "input_1"
AuditRequests == {"request_1", "request_2"}
AuditBaseline == "model_baseline"
AuditTarget == "model_target"
AuditStopReason == "stop"

\* The accepted until-changed request, with and without an approval.
AuditRequest(r, approval) ==
    RequestUntilChangedSwitchTurnAcceptedIdle(r, AuditTarget, TRUE, approval, TRUE, FALSE, None, FALSE, TRUE)

\* The abort arm in every phase it declares.
AuditAbort(r) ==
    \/ AbortUntilChangedSwitchTurnReconfigureIdle(r)
    \/ AbortUntilChangedSwitchTurnReconfigureAttached(r)
    \/ AbortUntilChangedSwitchTurnReconfigureRunning(r)
    \/ AbortUntilChangedSwitchTurnReconfigureRetired(r)
    \/ AbortUntilChangedSwitchTurnReconfigureStopped(r)

AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {})
    \/ model_step_count = 2 /\ SetModelRoutingBaselineIdle(AuditBaseline, TRUE)

\* What the request, the reconfigure gap and the withdrawal may interleave.
\* The input is admitted once, as the shell admits it.
AuditGap ==
    \/ \E r \in AuditRequests, approval \in BOOLEAN : AuditRequest(r, approval)
    \/ /\ AuditInput \notin DOMAIN input_phases
       /\ ResolveAdmissionPlanDefaultQueueKindIdle(AuditInput, None, None, "Prompt", None, "Ordinary", "InTurnEligible", FALSE, None, FALSE, FALSE, FALSE)
    \/ QueueAcceptedIdle(AuditInput)
    \/ StopRuntimeExecutorIdle(AuditStopReason)
    \/ RuntimeExecutorExitedFromIdle
    \/ AbandonInputIdle(AuditInput, "Stopped", 0)
    \/ AbandonInputStopped(AuditInput, "Stopped", 0)
    \/ \E r \in AuditRequests : CompleteUntilChangedSwitchTurnReconfigureIdle(r)
    \/ \E r \in AuditRequests : AuditAbort(r)

AuditNext ==
    \/ AuditPrefix
    \/ model_step_count >= 3 /\ AuditGap

AuditSpec == Init /\ [][AuditNext]_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* An abort withdraws exactly what its request staged: the pending switch and
\* the request's approval entries. It records no terminal and leaves the
\* phase, the rest of the routing state and every input's facts as they were.
AuditAbortWithdrawsOnlyTheRequest ==
    [][\A r \in AuditRequests :
         AuditAbort(r) =>
             /\ model_routing_pending_switch_request_id' = None
             /\ model_routing_pending_switch_target_model' = None
             /\ model_routing_pending_switch_realtime' = None
             /\ model_routing_pending_switch_phase' = None
             /\ model_routing_approval_phases' = MapRemove(model_routing_approval_phases, r)
             /\ model_routing_approval_parent_kind' = MapRemove(model_routing_approval_parent_kind, r)
             /\ UNCHANGED <<phase, registration_phase,
                            model_routing_pending_switch_turns,
                            model_routing_switch_terminal,
                            model_routing_switch_denials,
                            model_routing_switch_approval_reasons,
                            model_routing_baseline_model,
                            model_routing_baseline_realtime,
                            model_routing_topology_epoch,
                            input_phases, input_terminal_kind,
                            input_abandon_reason, input_lane,
                            input_admission_seq>>
      ]_vars

\* An abort fires only for the request that is still the pending until-changed
\* switch applying its persistent reconfigure: a request a newer one
\* replaced, or one already completed or withdrawn, has nothing to withdraw.
AuditAbortOnlyForThePendingRequest ==
    [][\A r \in AuditRequests :
         AuditAbort(r) =>
             /\ model_routing_pending_switch_request_id = Some(r)
             /\ model_routing_pending_switch_turns = None
             /\ model_routing_pending_switch_phase = Some("ApplyingPersistentReconfigure")
      ]_vars

\* Reachability goals (step goals), each checked as an expected violation of
\* its negation.
\* An approved request is withdrawn in Idle while another caller's input is
\* queued.
AuditWitnessQueuedInputKeptStep ==
    /\ AuditInput \in DOMAIN input_phases
    /\ input_phases[AuditInput] = "Queued"
    /\ \E r \in AuditRequests :
         /\ r \in DOMAIN model_routing_approval_phases
         /\ AbortUntilChangedSwitchTurnReconfigureIdle(r)
\* A request is withdrawn in Stopped after a stop abandoned that input.
AuditWitnessStoppedWithdrawnStep ==
    /\ AuditInput \in DOMAIN input_phases
    /\ input_phases[AuditInput] = "Abandoned"
    /\ \E r \in AuditRequests : AbortUntilChangedSwitchTurnReconfigureStopped(r)

NotAuditWitnessQueuedInputKept == [][~AuditWitnessQueuedInputKeptStep]_vars
NotAuditWitnessStoppedWithdrawn == [][~AuditWitnessStoppedWithdrawnStep]_vars
====
