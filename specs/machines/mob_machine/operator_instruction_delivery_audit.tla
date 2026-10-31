---- MODULE operator_instruction_delivery_audit ----
\* Hand-written audit of the MobMachine operator-instruction region: the
\* mob's desired records, the owed computation, and each member's delivery
\* facts (owed, recorded, blocked).
\*
\* The generated ci profile has no identities, so the region's member arms
\* and its member invariants hold there only vacuously. This audit keeps the
\* generated model unchanged and drives the region in two runs, each over a
\* small, fixed domain and its whole reachable space: a VIEW leaves out the
\* two counters no guard reads (model_step_count and topology_epoch), and a
\* state constraint bounds the mob's generation.
\*
\*   write (AuditWriteSpec): two members seated and bound (Dormant) under two
\*     profiles, then every write arm the domain reaches on one slot (a new
\*     record, a higher revision, an exact repeat, a stale revision, a
\*     conflict) for a mob, a profile and a member target, every revoke arm
\*     (an unknown slot, stale, repeated, conflicting, committed), the
\*     delivery's preparation (not owed, prepared), and every record and
\*     refusal arm for both sessions and every generation. A write is taken
\*     only once both members are bound (AuditWriteConstraint), so the run
\*     stays small; the seat run covers seats after writes.
\*   seat (AuditSeatSpec): two identities seated, bound, made unborn seats,
\*     born and unseated in any order around writes on one slot for a mob
\*     and a profile target, with the delivery's preparation (not owed,
\*     unborn, unbound, prepared) and the record of the current generation.
\*
\* Properties, in both runs, beside every generated invariant of ci.cfg (the
\* region's seven among them):
\*   AuditRecordedWithinGeneration (invariant): a member's session never
\*     recorded a generation the mob has not minted.
\*   AuditOwedIsSeatedAndUnblocked (invariant): an owed member is seated,
\*     never blocked, and owed a minted generation.
\*   AuditDeliversOnlyToBoundLocalBornMembers (invariant, ENABLED form): a
\*     delivery can be prepared only for an owed member that is bound to a
\*     session, whose session exists (not an unborn seat), and that is not
\*     placed.
\*   AuditWriteOwesExactlyTheSelected (action property): a committed write
\*     owes the new generation to exactly the seated members the slot's old
\*     or new target selects (an independent statement of the selector), and
\*     leaves every other member's obligation as it was.
\*   AuditSeatOwesTheGeneration (action property): a seat owes the member the
\*     mob's generation once it has one, and leaves it no recorded or blocked
\*     fact.
\*   AuditUnseatLeavesNoFact (action property): an unseat leaves the
\*     identity no delivery fact.
\*   AuditRegionWakesNothing (action property): a step that seats, binds,
\*     unbinds or unseats no member, or changes its seat session's unborn
\*     mark (every region step), changes no residency, wake attempt, wake
\*     binding, live runtime, materialization epoch or boot obligation.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations (a state goal as an invariant, a step goal as an
\* action property), so the properties are not vacuous:
\*   write: ProfileWriteOwesOne, RetargetOwesLeaver, RecordedReowed,
\*     BlockedThenUnblocked, RecordCloses (step goals), DormantDeliverable
\*     (state goal);
\*   seat: SeatAfterWriteOwed, UnseatDropsFacts (step goals),
\*     UnbornOwedDeferred, UnboundOwedDeferred (state goals).
\* With --mutants the runner seeds defects into a copy of the generated
\* model and requires TLC to refuse each with its target as the run's only
\* property (see the runner).
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* operator_instruction_delivery_audit.sh.
EXTENDS model

\* The positions of model_step_count and topology_epoch in the generated
\* `vars` tuple (the runner reads them from model.tla), and the generation
\* bound of each run.
CONSTANTS AuditStepIndex, AuditEpochIndex, AuditMaxWriteGeneration, AuditMaxSeatGeneration

AuditIds == {"agentidentity_1", "agentidentity_2"}
AuditSessions == {"sessionid_1", "sessionid_2"}
AuditRuntime(id) == IF id = "agentidentity_1" THEN "agentruntimeid_1" ELSE "agentruntimeid_2"
AuditSession(id) == IF id = "agentidentity_1" THEN "sessionid_1" ELSE "sessionid_2"
AuditProfile(id) == IF id = "agentidentity_1" THEN "profile_1" ELSE "profile_2"
AuditMode == "TurnDriven"
AuditSlot == "slot_a"
AuditUnknownSlot == "slot_b"
AuditDigest == "digest_1"
AuditBytes == 1
AuditRevisions == {1, 2}
AuditBlock == "OwnedByOtherMob"
AuditMob == [kind |-> "Mob", profile |-> "", member |-> None]
AuditProfileOne == [kind |-> "Profile", profile |-> "profile_1", member |-> None]
AuditMemberTwo == [kind |-> "Member", profile |-> "", member |-> Some("agentidentity_2")]
AuditWriteTargets == {AuditMob, AuditProfileOne, AuditMemberTwo}
AuditSeatTargets == {AuditMob, AuditProfileOne}

\* ---------------------------------------------------------------------------
\* Seats (the epoch replay's signals)
\* ---------------------------------------------------------------------------

AuditSeat ==
    \E id \in AuditIds :
        RecoverRosterMemberRunning(id, AuditRuntime(id), 1, 1, AuditProfile(id), AuditMode, FALSE)

AuditBind ==
    \E id \in AuditIds :
        RecoverMemberSessionBindingFreshRunning(id, AuditRuntime(id), AuditSession(id), None)

AuditUnborn ==
    \E id \in AuditIds :
        RecoverMemberSeatSessionUnbornRunning(id, AuditRuntime(id), TRUE)

AuditBorn ==
    \E id \in AuditIds :
        RecoverMemberSeatSessionBornRunning(id, AuditRuntime(id), FALSE)

AuditUnseat ==
    \E id \in AuditIds :
        RecoverRosterMemberRetiredRunning(id, AuditRuntime(id), 1, FALSE, FALSE, TRUE, FALSE)

\* ---------------------------------------------------------------------------
\* Writes and revokes
\* ---------------------------------------------------------------------------

AuditSetNew(targets) ==
    \E r \in AuditRevisions, t \in targets :
        SetOperatorInstructionNewRunning(AuditSlot, r, AuditDigest, AuditBytes, t.kind, t.profile, t.member)

AuditSetAdvance(targets) ==
    \E r \in AuditRevisions, t \in targets :
        SetOperatorInstructionAdvanceRunning(AuditSlot, r, AuditDigest, AuditBytes, t.kind, t.profile, t.member)

AuditSetDuplicate ==
    \E r \in AuditRevisions, t \in AuditWriteTargets :
        SetOperatorInstructionDuplicateRunning(AuditSlot, r, AuditDigest, AuditBytes, t.kind, t.profile, t.member)

AuditSetStale ==
    \E r \in AuditRevisions, t \in AuditWriteTargets :
        SetOperatorInstructionStaleRunning(AuditSlot, r, AuditDigest, AuditBytes, t.kind, t.profile, t.member)

AuditSetConflict ==
    \E r \in AuditRevisions, t \in AuditWriteTargets :
        SetOperatorInstructionConflictRunning(AuditSlot, r, AuditDigest, AuditBytes, t.kind, t.profile, t.member)

AuditRevokeUnknownSlot ==
    \E r \in AuditRevisions : RevokeOperatorInstructionUnknownSlotRunning(AuditUnknownSlot, r)

AuditRevokeStale ==
    \E r \in AuditRevisions : RevokeOperatorInstructionStaleRunning(AuditSlot, r)

AuditRevokeDuplicate ==
    \E r \in AuditRevisions : RevokeOperatorInstructionDuplicateRunning(AuditSlot, r)

AuditRevokeConflict ==
    \E r \in AuditRevisions : RevokeOperatorInstructionConflictRunning(AuditSlot, r)

AuditRevokeAdvance ==
    \E r \in AuditRevisions : RevokeOperatorInstructionAdvanceRunning(AuditSlot, r)

\* ---------------------------------------------------------------------------
\* Delivery and feedback
\* ---------------------------------------------------------------------------

AuditPrepareNotOwed == \E id \in AuditIds : PrepareMemberInstructionDeliveryNotOwedRunning(id)
AuditPrepareUnborn == \E id \in AuditIds : PrepareMemberInstructionDeliveryUnbornRunning(id)
AuditPrepareUnbound == \E id \in AuditIds : PrepareMemberInstructionDeliveryUnboundRunning(id)
AuditPrepareDeliver == \E id \in AuditIds : PrepareMemberInstructionDeliveryDeliverRunning(id)

AuditRecordNotOwed ==
    \E id \in AuditIds, s \in AuditSessions, g \in 1..AuditMaxWriteGeneration :
        RecordMemberInstructionTargetNotOwedRunning(id, s, g)
AuditRecordStaleSession ==
    \E id \in AuditIds, s \in AuditSessions, g \in 1..AuditMaxWriteGeneration :
        RecordMemberInstructionTargetStaleSessionRunning(id, s, g)
AuditRecordSuperseded ==
    \E id \in AuditIds, s \in AuditSessions, g \in 1..AuditMaxWriteGeneration :
        RecordMemberInstructionTargetSupersededRunning(id, s, g)
AuditRecordCurrent ==
    \E id \in AuditIds, s \in AuditSessions, g \in 1..AuditMaxWriteGeneration :
        RecordMemberInstructionTargetCurrentRunning(id, s, g)
AuditRecordUnminted ==
    \E id \in AuditIds, s \in AuditSessions, g \in 1..AuditMaxWriteGeneration :
        RecordMemberInstructionTargetUnmintedRunning(id, s, g)

AuditRefuseNotOwed ==
    \E id \in AuditIds, s \in AuditSessions, g \in 1..AuditMaxWriteGeneration :
        RefuseMemberInstructionTargetNotOwedRunning(id, s, g, AuditBlock)
AuditRefuseStaleSession ==
    \E id \in AuditIds, s \in AuditSessions, g \in 1..AuditMaxWriteGeneration :
        RefuseMemberInstructionTargetStaleSessionRunning(id, s, g, AuditBlock)
AuditRefuseSuperseded ==
    \E id \in AuditIds, s \in AuditSessions, g \in 1..AuditMaxWriteGeneration :
        RefuseMemberInstructionTargetSupersededRunning(id, s, g, AuditBlock)
AuditRefuseBlocked ==
    \E id \in AuditIds, s \in AuditSessions, g \in 1..AuditMaxWriteGeneration :
        RefuseMemberInstructionTargetBlockedRunning(id, s, g, AuditBlock)

\* The seat run records only the current generation, on the member's own
\* session.
AuditSeatRecordCurrent ==
    \E id \in AuditIds, g \in 1..AuditMaxSeatGeneration :
        RecordMemberInstructionTargetCurrentRunning(id, AuditSession(id), g)

\* ---------------------------------------------------------------------------
\* Runs
\* ---------------------------------------------------------------------------

AuditWriteNext ==
    \/ AuditSeat
    \/ AuditBind
    \/ AuditSetNew(AuditWriteTargets)
    \/ AuditSetAdvance(AuditWriteTargets)
    \/ AuditSetDuplicate
    \/ AuditSetStale
    \/ AuditSetConflict
    \/ AuditRevokeUnknownSlot
    \/ AuditRevokeStale
    \/ AuditRevokeDuplicate
    \/ AuditRevokeConflict
    \/ AuditRevokeAdvance
    \/ AuditPrepareNotOwed
    \/ AuditPrepareDeliver
    \/ AuditRecordNotOwed
    \/ AuditRecordStaleSession
    \/ AuditRecordSuperseded
    \/ AuditRecordCurrent
    \/ AuditRecordUnminted
    \/ AuditRefuseNotOwed
    \/ AuditRefuseStaleSession
    \/ AuditRefuseSuperseded
    \/ AuditRefuseBlocked

AuditWriteSpec == Init /\ [][AuditWriteNext]_vars

\* A state where a write committed before both members were bound is checked
\* but not explored further: the write run starts its writes from two bound
\* members.
AuditWriteConstraint ==
    /\ operator_instruction_generation <= AuditMaxWriteGeneration
    /\ operator_instruction_generation > 0 => DOMAIN member_session_bindings = AuditIds

AuditSeatNext ==
    \/ AuditSeat
    \/ AuditBind
    \/ AuditUnborn
    \/ AuditBorn
    \/ AuditUnseat
    \/ AuditSetNew(AuditSeatTargets)
    \/ AuditSetAdvance(AuditSeatTargets)
    \/ AuditPrepareNotOwed
    \/ AuditPrepareUnborn
    \/ AuditPrepareUnbound
    \/ AuditPrepareDeliver
    \/ AuditSeatRecordCurrent

AuditSeatSpec == Init /\ [][AuditSeatNext]_vars

AuditSeatConstraint == operator_instruction_generation <= AuditMaxSeatGeneration

\* Every state variable but the two counters (see the header).
AuditView == [vars EXCEPT ![AuditStepIndex] = 0, ![AuditEpochIndex] = 0]

\* ---------------------------------------------------------------------------
\* Properties
\* ---------------------------------------------------------------------------

\* `m` holds `g` for `id`.
AuditFactAt(m, id, g) == id \in DOMAIN m /\ m[id] = g

\* `id` has the same fact in `before` and `after`.
AuditSameFact(before, after, id) ==
    /\ (id \in DOMAIN before) = (id \in DOMAIN after)
    /\ id \in DOMAIN before => before[id] = after[id]

\* The slot `s` of the record maps `kinds`, `profiles` and `members` selects
\* the member `id`: an independent statement of the machine's selector.
AuditSelects(kinds, profiles, members, s, id) ==
    /\ s \in DOMAIN kinds
    /\ \/ kinds[s] = "Mob"
       \/ /\ kinds[s] = "Profile"
          /\ id \in DOMAIN member_profile_names
          /\ s \in DOMAIN profiles
          /\ member_profile_names[id] = profiles[s]
       \/ /\ kinds[s] = "Member"
          /\ s \in DOMAIN members
          /\ members[s] = id

AuditSelectedBefore(s, id) ==
    AuditSelects(operator_instruction_target_kind, operator_instruction_target_profile,
                 operator_instruction_target_member, s, id)

AuditSelectedAfter(s, id) ==
    AuditSelects(operator_instruction_target_kind', operator_instruction_target_profile',
                 operator_instruction_target_member', s, id)

\* The slots whose revision a step changed.
AuditChangedSlots ==
    {s \in DOMAIN operator_instruction_revision' :
        \/ s \notin DOMAIN operator_instruction_revision
        \/ operator_instruction_revision'[s] # operator_instruction_revision[s]}

\* A committed write: the generation advances by one and the roster stays.
AuditCommittedWrite ==
    /\ operator_instruction_generation' = operator_instruction_generation + 1
    /\ UNCHANGED identity_to_runtime

AuditRecordedWithinGeneration ==
    \A id \in DOMAIN member_instruction_recorded :
        member_instruction_recorded[id] <= operator_instruction_generation

AuditOwedIsSeatedAndUnblocked ==
    \A id \in DOMAIN member_instruction_owed :
        /\ id \in DOMAIN identity_to_runtime
        /\ id \notin DOMAIN member_instruction_blocked
        /\ member_instruction_owed[id] <= operator_instruction_generation

AuditDeliversOnlyToBoundLocalBornMembers ==
    \A id \in AuditIds :
        ENABLED PrepareMemberInstructionDeliveryDeliverRunning(id) =>
            /\ id \in DOMAIN member_instruction_owed
            /\ id \notin member_seat_session_unborn
            /\ id \in DOMAIN member_session_bindings
            /\ id \notin DOMAIN member_placement

AuditWriteOwesExactlyTheSelected ==
    [][AuditCommittedWrite =>
         \A id \in DOMAIN identity_to_runtime :
             IF \E s \in AuditChangedSlots : AuditSelectedBefore(s, id) \/ AuditSelectedAfter(s, id)
             THEN AuditFactAt(member_instruction_owed', id, operator_instruction_generation')
             ELSE AuditSameFact(member_instruction_owed, member_instruction_owed', id)
      ]_vars

AuditSeatOwesTheGeneration ==
    [][\A id \in AuditIds :
         (id \notin DOMAIN identity_to_runtime /\ id \in DOMAIN identity_to_runtime') =>
             /\ id \notin DOMAIN member_instruction_recorded'
             /\ id \notin DOMAIN member_instruction_blocked'
             /\ IF operator_instruction_generation' > 0
                THEN AuditFactAt(member_instruction_owed', id, operator_instruction_generation')
                ELSE id \notin DOMAIN member_instruction_owed'
      ]_vars

AuditUnseatLeavesNoFact ==
    [][\A id \in AuditIds :
         (id \in DOMAIN identity_to_runtime /\ id \notin DOMAIN identity_to_runtime') =>
             /\ id \notin DOMAIN member_instruction_owed'
             /\ id \notin DOMAIN member_instruction_recorded'
             /\ id \notin DOMAIN member_instruction_blocked'
      ]_vars

AuditRegionWakesNothing ==
    [][UNCHANGED <<identity_to_runtime, member_session_bindings, member_seat_session_unborn>> =>
         UNCHANGED <<member_materialization, member_wake_attempts, member_wake_bound,
                     live_runtime_ids, member_materialization_epoch, member_boot_obligation_owed>>
      ]_vars

\* ---------------------------------------------------------------------------
\* Reachability goals, each checked as an expected violation of its negation:
\* a state goal as an invariant, a step goal as an action property.
\* ---------------------------------------------------------------------------

\* Write run: a profile write owes the profile's member and not the other.
AuditWitnessProfileWriteOwesOneStep ==
    /\ AuditCommittedWrite
    /\ "agentidentity_2" \in DOMAIN identity_to_runtime
    /\ \E s \in AuditChangedSlots :
           /\ operator_instruction_target_kind'[s] = "Profile"
           /\ AuditFactAt(member_instruction_owed', "agentidentity_1", operator_instruction_generation')
           /\ ~AuditFactAt(member_instruction_owed', "agentidentity_2", operator_instruction_generation')

\* Write run: a retarget owes a member the new target no longer selects (its
\* snapshot revokes the slot).
AuditWitnessRetargetOwesLeaverStep ==
    /\ AuditCommittedWrite
    /\ \E s \in AuditChangedSlots, id \in DOMAIN identity_to_runtime :
           /\ AuditSelectedBefore(s, id)
           /\ ~AuditSelectedAfter(s, id)
           /\ AuditFactAt(member_instruction_owed', id, operator_instruction_generation')

\* Write run: a member whose session recorded a snapshot is owed again by a
\* later write, and keeps what its session recorded.
AuditWitnessRecordedReowedStep ==
    /\ AuditCommittedWrite
    /\ \E id \in DOMAIN member_instruction_recorded :
           /\ id \notin DOMAIN member_instruction_owed
           /\ id \in DOMAIN member_instruction_recorded'
           /\ AuditFactAt(member_instruction_owed', id, operator_instruction_generation')

\* Write run: a blocked member is unblocked and owed by a write that selects
\* it.
AuditWitnessBlockedThenUnblockedStep ==
    /\ AuditCommittedWrite
    /\ \E id \in DOMAIN member_instruction_blocked :
           /\ id \notin DOMAIN member_instruction_blocked'
           /\ AuditFactAt(member_instruction_owed', id, operator_instruction_generation')

\* Write run: a record closes an obligation.
AuditWitnessRecordClosesStep ==
    \E id \in DOMAIN member_instruction_owed :
        /\ id \notin DOMAIN member_instruction_owed'
        /\ id \in DOMAIN member_instruction_recorded'
        /\ id \in DOMAIN identity_to_runtime'

\* Write run: a Dormant member's delivery can be prepared (store-only, no
\* wake).
AuditWitnessDormantDeliverable ==
    \E id \in AuditIds :
        /\ id \in DOMAIN member_materialization
        /\ member_materialization[id] = "Dormant"
        /\ ENABLED PrepareMemberInstructionDeliveryDeliverRunning(id)

\* Seat run: a member seated after a write is owed the mob's generation.
AuditWitnessSeatAfterWriteOwedStep ==
    \E id \in AuditIds :
        /\ id \notin DOMAIN identity_to_runtime
        /\ id \in DOMAIN identity_to_runtime'
        /\ operator_instruction_generation > 0
        /\ AuditFactAt(member_instruction_owed', id, operator_instruction_generation)

\* Seat run: an unseat drops a member's delivery facts.
AuditWitnessUnseatDropsFactsStep ==
    \E id \in DOMAIN identity_to_runtime :
        /\ id \notin DOMAIN identity_to_runtime'
        /\ \/ id \in DOMAIN member_instruction_owed
           \/ id \in DOMAIN member_instruction_recorded

\* Seat run: an owed unborn seat's delivery is deferred until its session is
\* born.
AuditWitnessUnbornOwedDeferred ==
    \E id \in member_seat_session_unborn :
        /\ id \in DOMAIN member_instruction_owed
        /\ ENABLED PrepareMemberInstructionDeliveryUnbornRunning(id)

\* Seat run: an owed member without a session binding is deferred.
AuditWitnessUnboundOwedDeferred ==
    \E id \in DOMAIN member_instruction_owed :
        /\ id \notin DOMAIN member_session_bindings
        /\ ENABLED PrepareMemberInstructionDeliveryUnboundRunning(id)

NotAuditWitnessProfileWriteOwesOne == [][~AuditWitnessProfileWriteOwesOneStep]_vars
NotAuditWitnessRetargetOwesLeaver == [][~AuditWitnessRetargetOwesLeaverStep]_vars
NotAuditWitnessRecordedReowed == [][~AuditWitnessRecordedReowedStep]_vars
NotAuditWitnessBlockedThenUnblocked == [][~AuditWitnessBlockedThenUnblockedStep]_vars
NotAuditWitnessRecordCloses == [][~AuditWitnessRecordClosesStep]_vars
NotAuditWitnessDormantDeliverable == ~AuditWitnessDormantDeliverable
NotAuditWitnessSeatAfterWriteOwed == [][~AuditWitnessSeatAfterWriteOwedStep]_vars
NotAuditWitnessUnseatDropsFacts == [][~AuditWitnessUnseatDropsFactsStep]_vars
NotAuditWitnessUnbornOwedDeferred == ~AuditWitnessUnbornOwedDeferred
NotAuditWitnessUnboundOwedDeferred == ~AuditWitnessUnboundOwedDeferred
====
