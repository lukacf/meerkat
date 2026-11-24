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
\* One until-changed switch is pending at a time: a request that arrives
\* while another one is pending is refused, and the pending switch ends only
\* through its own request's completion or abort. A reconfigured switch
\* completes in whatever phase the session reached in the gap, a stop
\* included, and records its target as the routing baseline. No refusal
\* records a denial under the pending switch's id, and no accept stages a
\* request under an id that holds a denial, so a refused request stays
\* refused and the retry that finishes a pending switch is never denied.
\*
\* A request carries what its host found when it looked at the target for
\* the reasoning effort level the session holds, and the session identity
\* that was read for the look. A target that carries the transcript is
\* staged only on evidence that admits it, taken on the identity the session
\* holds; a target that refuses the level is denied for its capability, for
\* both durations, and only on that evidence.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and drives,
\* for one session, one input and two requests, the initialization, the
\* registration and the routing baseline, then in any order the accepted
\* until-changed request (with and without an approval, on every evidence
\* value and without evidence, with and without an identity witness the
\* session does not hold), its refusal while another request is pending, the
\* refusal of a finite request while a switch is pending, the denial of a
\* request of either duration whose target refuses the held level (on the
\* same evidence and witnesses), the input's admission and queueing, the
\* runtime stop
\* and the executor exit, the input's abandonment in Idle and Stopped, and
\* the completion and the abort in every phase each declares. Nothing here
\* attaches, runs or retires the session, so only their Idle and Stopped arms
\* fire; all phases of an arm share one generated body. Properties:
\*
\*   every generated invariant of ci.cfg (kept);
\*   AuditNoDenialUnderThePendingSwitch (invariant): the pending switch's id
\*     holds no denial;
\*   AuditAbortWithdrawsOnlyTheRequest (action property): an abort clears
\*     the pending switch and removes exactly its request's approval
\*     entries, and leaves the phase, the switch terminals and denials, the
\*     baseline and every input's facts unchanged;
\*   AuditAbortOnlyForThePendingRequest (action property): an abort fires
\*     only for the request that is the pending until-changed switch
\*     applying its persistent reconfigure;
\*   AuditPendingSwitchEndsOnlyWithItsRequest (action property): a pending
\*     switch ends only through its own request's completion or abort, so
\*     no other request replaces it;
\*   AuditCompleteSettlesTheRequest (action property): a completion clears
\*     the pending switch, records its request as applied and makes its
\*     target the routing baseline, and leaves the phase, the approvals and
\*     every input's facts unchanged;
\*   AuditStagedOnlyOnAdmittingEvidence (action property): a request is
\*     staged only on one of the four admitting evidence values and with
\*     no identity witness the session does not hold: never on a refusal,
\*     never without evidence, never on evidence taken on another identity;
\*   AuditEffortDenialOnlyOnARefusal (action property): a denial for the
\*     level fires only on a refusal and with no identity witness the
\*     session does not hold, records the capability denial under its
\*     request and leaves the pending switch and the approvals as they are.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous: an approved
\* request withdrawn in Idle while another caller's input is queued, a
\* request withdrawn in Stopped after a stop abandoned that input, a request
\* refused while another one is pending, a finite request refused while the
\* other one is pending, a request completed in Stopped, a request
\* denied for the level while another one is pending, and a request staged
\* on the evidence that the target's levels could not be read (which is not
\* a refusal). With --mutants
\* the runner also checks that fifteen seeded defects are refused: an abort
\* that records a terminal, an abort that erases the inputs' phases (as the
\* whole-state restore it replaced did), an abort without its
\* pending-request guard, an abort that keeps its request's approval entry,
\* an accept without its pending-switch guard, a completion that keeps the
\* old baseline, an accept without its denied-request guard, a finite
\* refusal without its pending-request guard, an accept without each of its
\* three evidence guards (not a refusal, evidence present, the identity
\* witness), and a level denial without its refusal guard, without its
\* identity witness guard, without its pending-request guard, and one that
\* fires on unreadable levels as on a refusal. The level
\* denial's guard that the target carries the transcript has no mutant
\* here: without it the arm writes what the transcript denial writes, so no
\* state tells them apart; the arm exclusivity ledger
\* (transition_arm_exclusivity.rs) refuses that overlap. The audit drives
\* the until-changed accept arm alone, and its session never holds an
\* identity, so a witness that matches is never admitted here: the finite
\* accept arm's evidence guards and the matching witness rest on the
\* runtime's rows.
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

\* What a request's host found when it looked at the target for the level
\* the session holds: one of the five verdicts, or nothing (nobody looked).
\* Every verdict but the refusal admits: unreadable levels are not a refusal.
AuditVerdicts == {"NoHeldEffort", "HeldEffortAccepted", "LegacyUnverified", "Refused", "LevelsUnreadable"}
AuditEvidence == {None} \cup {Some(verdict) : verdict \in AuditVerdicts}
AuditAdmitting == {Some("NoHeldEffort"), Some("HeldEffortAccepted"), Some("LegacyUnverified"), Some("LevelsUnreadable")}
\* The session identity a look was taken on: none named, or an identity the
\* session does not hold (nothing here hydrates the session, so the
\* identity it holds stays None).
AuditOtherIdentity == "sessionllmidentity_1"
AuditWitnesses == {None, Some(AuditOtherIdentity)}

\* The accepted until-changed request, with and without an approval, on the
\* evidence and the identity witness it carries.
AuditRequestOn(r, approval, evidence, witness) ==
    RequestUntilChangedSwitchTurnAcceptedIdle(r, AuditTarget, TRUE, approval, TRUE, FALSE, None, FALSE, TRUE, evidence, witness)
AuditRequest(r, approval) ==
    \E evidence \in AuditEvidence : \E witness \in AuditWitnesses :
        AuditRequestOn(r, approval, evidence, witness)

\* The refusal of a request while another one is pending.
AuditConflict(r) ==
    RequestUntilChangedSwitchTurnPendingConflictIdle(r, AuditTarget, TRUE, FALSE, TRUE, FALSE, None, FALSE, TRUE, Some("NoHeldEffort"), None)

\* The refusal of a finite request while a switch is pending (the scoped
\* conflict: nothing here sets a turn or an operation override).
AuditFiniteConflict(r) ==
    RequestFiniteSwitchTurnScopedConflictIdle(r, AuditTarget, 1, TRUE, FALSE, TRUE, FALSE, None, FALSE, TRUE, Some("NoHeldEffort"), None)

\* The denial of a request whose target refuses the level the session
\* holds, for both durations, on the evidence and the identity witness the
\* request carries.
AuditEffortRefusedOn(r, evidence, witness) ==
    \/ RequestUntilChangedSwitchTurnReasoningEffortRefusedIdle(r, AuditTarget, TRUE, FALSE, TRUE, FALSE, None, FALSE, TRUE, evidence, witness)
    \/ RequestFiniteSwitchTurnReasoningEffortRefusedIdle(r, AuditTarget, 1, TRUE, FALSE, TRUE, FALSE, None, FALSE, TRUE, evidence, witness)
AuditEffortRefused(r) ==
    \E evidence \in AuditEvidence : \E witness \in AuditWitnesses :
        AuditEffortRefusedOn(r, evidence, witness)

\* The completion in every phase it declares.
AuditComplete(r) ==
    \/ CompleteUntilChangedSwitchTurnReconfigureIdle(r)
    \/ CompleteUntilChangedSwitchTurnReconfigureAttached(r)
    \/ CompleteUntilChangedSwitchTurnReconfigureRunning(r)
    \/ CompleteUntilChangedSwitchTurnReconfigureRetired(r)
    \/ CompleteUntilChangedSwitchTurnReconfigureStopped(r)

\* The abort in every phase it declares.
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
    \/ \E r \in AuditRequests : AuditConflict(r)
    \/ \E r \in AuditRequests : AuditFiniteConflict(r)
    \/ \E r \in AuditRequests : AuditEffortRefused(r)
    \/ /\ AuditInput \notin DOMAIN input_phases
       /\ ResolveAdmissionPlanDefaultQueueKindIdle(AuditInput, None, None, "Prompt", None, "Ordinary", "InTurnEligible", FALSE, None, FALSE, FALSE, FALSE)
    \/ QueueAcceptedIdle(AuditInput)
    \/ StopRuntimeExecutorIdle(AuditStopReason)
    \/ RuntimeExecutorExitedFromIdle
    \/ AbandonInputIdle(AuditInput, "Stopped", 0)
    \/ AbandonInputStopped(AuditInput, "Stopped", 0)
    \/ \E r \in AuditRequests : AuditComplete(r)
    \/ \E r \in AuditRequests : AuditAbort(r)

AuditNext ==
    \/ AuditPrefix
    \/ model_step_count >= 3 /\ AuditGap

AuditSpec == Init /\ [][AuditNext]_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* The pending switch's id holds no denial: no refusal records one under it,
\* and no accept stages an id that holds one, so the retry that finishes a
\* pending switch is never answered as denied.
AuditNoDenialUnderThePendingSwitch ==
    \A r \in AuditRequests :
        model_routing_pending_switch_request_id = Some(r)
            => r \notin DOMAIN model_routing_switch_denials

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
\* switch applying its persistent reconfigure: a request already completed
\* or withdrawn, or never accepted, has nothing to withdraw.
AuditAbortOnlyForThePendingRequest ==
    [][\A r \in AuditRequests :
         AuditAbort(r) =>
             /\ model_routing_pending_switch_request_id = Some(r)
             /\ model_routing_pending_switch_turns = None
             /\ model_routing_pending_switch_phase = Some("ApplyingPersistentReconfigure")
      ]_vars

\* A pending switch ends only through its own request's completion or abort:
\* no other request replaces it.
AuditPendingSwitchEndsOnlyWithItsRequest ==
    [][\A r \in AuditRequests :
         (model_routing_pending_switch_request_id = Some(r) /\ model_routing_pending_switch_request_id' # Some(r))
             => (AuditComplete(r) \/ AuditAbort(r))
      ]_vars

\* A completion settles exactly its request, in whatever phase the session
\* reached: the pending switch clears, the request is recorded as applied
\* and its target becomes the routing baseline. The phase, the approvals and
\* every input's facts stay as they were.
AuditCompleteSettlesTheRequest ==
    [][\A r \in AuditRequests :
         AuditComplete(r) =>
             /\ model_routing_pending_switch_request_id' = None
             /\ model_routing_pending_switch_target_model' = None
             /\ model_routing_pending_switch_realtime' = None
             /\ model_routing_pending_switch_phase' = None
             /\ model_routing_switch_terminal' = MapSet(model_routing_switch_terminal, r, "PersistentReconfigureApplied")
             /\ model_routing_baseline_model' = model_routing_pending_switch_target_model
             /\ model_routing_baseline_realtime' = model_routing_pending_switch_realtime
             /\ UNCHANGED <<phase, registration_phase,
                            model_routing_approval_phases,
                            model_routing_approval_parent_kind,
                            model_routing_switch_denials,
                            input_phases, input_terminal_kind,
                            input_abandon_reason, input_lane,
                            input_admission_seq>>
      ]_vars

\* A request is staged only on evidence that admits it, taken on the
\* identity the session holds: never on a refusal, never without evidence,
\* never with a witness of an identity the session does not hold.
AuditStagedOnlyOnAdmittingEvidence ==
    [][\A r \in AuditRequests : \A approval \in BOOLEAN :
         \A evidence \in AuditEvidence : \A witness \in AuditWitnesses :
             AuditRequestOn(r, approval, evidence, witness) =>
                 /\ evidence \in AuditAdmitting
                 /\ witness = None
      ]_vars

\* A denial for the level fires only on a refusal, taken on the identity the
\* session holds. It records the capability denial under its request and
\* leaves the pending switch and the approvals as they are.
AuditEffortDenialOnlyOnARefusal ==
    [][\A r \in AuditRequests :
         \A evidence \in AuditEvidence : \A witness \in AuditWitnesses :
             AuditEffortRefusedOn(r, evidence, witness) =>
                 /\ evidence = Some("Refused")
                 /\ witness = None
                 /\ model_routing_switch_denials' = MapSet(model_routing_switch_denials, r, "CapabilityPolicy")
                 /\ model_routing_switch_terminal' = MapSet(model_routing_switch_terminal, r, "Denied")
                 /\ UNCHANGED <<phase, registration_phase,
                                model_routing_pending_switch_request_id,
                                model_routing_pending_switch_target_model,
                                model_routing_pending_switch_realtime,
                                model_routing_pending_switch_turns,
                                model_routing_pending_switch_phase,
                                model_routing_approval_phases,
                                model_routing_approval_parent_kind,
                                model_routing_switch_approval_reasons,
                                model_routing_baseline_model>>
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

\* A request is refused while another one is pending.
AuditWitnessConflictRefusedStep ==
    \E r \in AuditRequests : AuditConflict(r)
\* A finite request is refused while the other one is pending.
AuditWitnessFiniteConflictRefusedStep ==
    \E r \in AuditRequests : AuditFiniteConflict(r)
\* A request completes in Stopped after a stop in its gap.
AuditWitnessStoppedCompletedStep ==
    \E r \in AuditRequests : CompleteUntilChangedSwitchTurnReconfigureStopped(r)

\* A request is denied for the level while another one is pending.
AuditWitnessEffortRefusedWhilePendingStep ==
    /\ model_routing_pending_switch_request_id # None
    /\ \E r \in AuditRequests : AuditEffortRefused(r)

\* A request is staged on the evidence that the target's levels could not
\* be read: the accept arm's guards admit that value.
AuditWitnessUnreadableLevelsStagedStep ==
    \E r \in AuditRequests : \E approval \in BOOLEAN :
        AuditRequestOn(r, approval, Some("LevelsUnreadable"), None)

NotAuditWitnessQueuedInputKept == [][~AuditWitnessQueuedInputKeptStep]_vars
NotAuditWitnessStoppedWithdrawn == [][~AuditWitnessStoppedWithdrawnStep]_vars
NotAuditWitnessConflictRefused == [][~AuditWitnessConflictRefusedStep]_vars
NotAuditWitnessFiniteConflictRefused == [][~AuditWitnessFiniteConflictRefusedStep]_vars
NotAuditWitnessStoppedCompleted == [][~AuditWitnessStoppedCompletedStep]_vars
NotAuditWitnessEffortRefusedWhilePending == [][~AuditWitnessEffortRefusedWhilePendingStep]_vars
NotAuditWitnessUnreadableLevelsStaged == [][~AuditWitnessUnreadableLevelsStagedStep]_vars
====
