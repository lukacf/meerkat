---- MODULE operator_instruction_delivery_audit ----
\* Hand-written audit of the MobMachine operator-instruction region: the
\* mob's desired records, the owed computation, each member's delivery
\* facts (owed, recorded, blocked) and the fork lineage a record written for
\* a member and its forks follows.
\*
\* The generated ci profile has no identities, so the region's member arms
\* and its member invariants hold there only vacuously. This audit keeps the
\* generated model unchanged and drives the region in three runs, each over a
\* small, fixed domain and its whole reachable space: a VIEW leaves out the
\* four counters no guard reads (model_step_count, topology_epoch and the
\* identity life marker, identity_life_resets and identity_life_ends), and a
\* state constraint bounds the mob's generation.
\*
\*   write (AuditWriteSpec): two members seated and bound (Dormant) under two
\*     profiles, then every write arm the domain reaches on one slot (a new
\*     record, a higher revision, an exact repeat, a stale revision, a
\*     conflict) for a mob, a profile, each of the two members as a member
\*     target and each of them as a member-and-forks target (so a record
\*     moves from one member to the other, the arms that look the owed
\*     members up instead of searching the seats, and between a member and
\*     the member with its forks), the replay of a record or of its revoke
\*     at a higher revision and the next generation, every revoke arm
\*     (an unknown slot, stale, repeated, conflicting, committed), the
\*     delivery's preparation (not owed, prepared), and every record and
\*     refusal arm for both sessions and every generation. Either member's
\*     fork lineage is recorded with the other as its root at any time
\*     after its seat, before or after a write. A write is taken
\*     only once both members are bound (AuditWriteConstraint), so the run
\*     stays small; the seat run covers seats after writes.
\*   seat (AuditSeatSpec): two identities seated, bound, made unborn seats,
\*     born, retired (a releasing retire, archive-only for a Dormant member)
\*     and unseated in any order around writes on one slot for a mob, a
\*     profile and a member-and-forks target, with the delivery's
\*     preparation (not owed, unborn, unbound, retiring, prepared), the
\*     record of the current generation, the release verdict of a retiring
\*     or seated member's session, either member's fork lineage and the end
\*     of an unseated identity's life.
\*   fork (AuditForkSpec): three identities seated, unseated and seated
\*     again (a second incarnation of the same name) around a
\*     member target and two member-and-forks targets on one slot (a new
\*     record, a higher revision, a revoke, a replay), with any of them
\*     recorded as a fork of any other (so two forks of one member, as a
\*     fork and its own fork are recorded, a root that itself holds an
\*     entry, and a member that is neither the record's member nor its
\*     fork) and the end of an unseated identity's life.
\*
\* Properties, in every run, beside every generated invariant of ci.cfg (the
\* region's eight among them):
\*   AuditRecordedWithinGeneration (invariant): a member's session never
\*     recorded a generation the mob has not minted.
\*   AuditOwedIsSeatedAndUnblocked (invariant): an owed member is seated,
\*     never blocked, and owed a minted generation.
\*   AuditDeliversOnlyToBoundBornMembers (invariant, ENABLED form): a
\*     delivery can be prepared only for an owed member that is bound to a
\*     session and whose session exists (not an unborn seat). A placed
\*     member is prepared the same way (no arm of the region reads its
\*     placement); this audit seats no placed member.
\*   AuditLineageOnlyBeforeASnapshot (invariant, ENABLED form): a fork
\*     lineage can be recorded only for a seated member, with another name as
\*     its root, whose session has recorded no snapshot.
\*   AuditWriteOwesExactlyTheSelected (action property): a committed write
\*     (a new record, a higher revision, a revoke or a replayed record) owes
\*     the new generation to exactly the seated members the slot's old or new
\*     target selects (an independent statement of the selector: a record
\*     for a member and its forks selects the member it names and every
\*     member whose lineage root is that member), and leaves every other
\*     member's obligation as it was.
\*   AuditSeatOwesTheGeneration (action property): a seat owes the member the
\*     mob's generation once it has one, and leaves it no recorded or blocked
\*     fact.
\*   AuditUnseatLeavesNoFact (action property): an unseat leaves the
\*     identity no delivery fact.
\*   AuditLineageRecordWritesTheGivenRoot (action property): a lineage
\*     record writes the root it is given as the member's entry, whatever
\*     entry that root holds itself (the shell names the first member for a
\*     fork of a fork; the machine looks nothing up), changes no other
\*     member's entry, and repeated for a member that holds exactly that
\*     entry leaves the lineage as it was.
\*   AuditLineageRecordOwesTheGeneration (action property): a lineage record
\*     owes its member the mob's generation once the mob has one, leaves it
\*     unblocked, and changes no other member's obligation.
\*   AuditIdentityEndLeavesNoLineage (action property): a step that moves an
\*     identity's life marker (the identity ended) leaves it no lineage
\*     entry.
\*   AuditLineageKeptUnlessRecordedOrEnded (action property): a member's
\*     lineage entry changes only by its own lineage record or the end of
\*     its identity: a seat (a second one of the same name too), an unseat,
\*     a write and every other step keep it.
\*   AuditRegionWakesNothing (action property): a step that seats, binds,
\*     unbinds or unseats no member, or changes its seat session's unborn
\*     mark (every region step), changes no residency, wake attempt, wake
\*     binding, live runtime, materialization epoch or boot obligation.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations (a state goal as an invariant, a step goal as an
\* action property), so the properties are not vacuous:
\*   write: ProfileWriteOwesOne, RetargetOwesLeaver, MemberRetargetOwesBoth,
\*     RecordedReowed,
\*     BlockedThenUnblocked, RecordCloses, ForkWriteOwesFork,
\*     MemberWriteSkipsFork, LineageRecordOwes
\*     (step goals), DormantDeliverable (state goal);
\*   seat: SeatAfterWriteOwed, UnseatDropsFacts, IdentityEndDropsLineage,
\*     ForkOwedWithoutSeatedRoot (step goals),
\*     UnbornOwedDeferred, UnboundOwedDeferred, RetiringReleasable (state
\*     goals);
\*   fork: ForkWriteSkipsBystander, ForkOfForkFollowsTheFirstMember,
\*     ForkRecordSkipsSiblingFork, ReseatKeepsLineage,
\*     ReseatedForkOwedByRootRecord,
\*     LineageRecordRepeated (step goals), RootHoldsAnEntry (state goal).
\* With --mutants the runner seeds defects into a copy of the generated
\* model and requires TLC to refuse each with its target as the run's only
\* property (see the runner).
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* operator_instruction_delivery_audit.sh.
EXTENDS model

\* The positions of model_step_count, topology_epoch and the identity life
\* marker (identity_life_resets, identity_life_ends) in the generated `vars`
\* tuple (the runner reads them from model.tla), and the generation bound of
\* each run.
CONSTANTS AuditStepIndex, AuditEpochIndex, AuditLifeResetsIndex, AuditLifeEndsIndex,
          AuditMaxWriteGeneration, AuditMaxSeatGeneration, AuditMaxForkGeneration

AuditIds == {"agentidentity_1", "agentidentity_2"}
\* The fork run's identities: the two above and a third, under the second's
\* profile.
AuditForkIds == AuditIds \cup {"agentidentity_3"}
AuditSessions == {"sessionid_1", "sessionid_2"}
AuditRuntime(id) ==
    IF id = "agentidentity_1" THEN "agentruntimeid_1"
    ELSE IF id = "agentidentity_2" THEN "agentruntimeid_2" ELSE "agentruntimeid_3"
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
AuditMemberOne == [kind |-> "Member", profile |-> "", member |-> Some("agentidentity_1")]
AuditMemberTwo == [kind |-> "Member", profile |-> "", member |-> Some("agentidentity_2")]
AuditMemberOneAndForks ==
    [kind |-> "MemberAndForks", profile |-> "", member |-> Some("agentidentity_1")]
AuditMemberTwoAndForks ==
    [kind |-> "MemberAndForks", profile |-> "", member |-> Some("agentidentity_2")]
AuditWriteTargets == {AuditMob, AuditProfileOne, AuditMemberOne, AuditMemberTwo,
                      AuditMemberOneAndForks, AuditMemberTwoAndForks}
AuditSeatTargets == {AuditMob, AuditProfileOne, AuditMemberOneAndForks}
AuditForkTargets == {AuditMemberOne, AuditMemberOneAndForks, AuditMemberTwoAndForks}
AuditMobId == "mobid_1"
AuditDisposals == {"RuntimeReleasedOnlyHostOwned", "Archived"}
\* A replayed record is Active with the audit's body, or the record a revoke
\* left: Absent with no body.
AuditEffects == {"Active", "Absent"}
AuditReplayDigest(e) == IF e = "Active" THEN AuditDigest ELSE ""
AuditReplayBytes(e) == IF e = "Active" THEN AuditBytes ELSE 0

\* ---------------------------------------------------------------------------
\* Seats (the epoch replay's signals)
\* ---------------------------------------------------------------------------

AuditSeatOf(ids) ==
    \E id \in ids :
        RecoverRosterMemberRunning(id, AuditRuntime(id), 1, 1, AuditProfile(id), AuditMode, FALSE)

AuditSeat == AuditSeatOf(AuditIds)

\* The fork run's seat: a name's first seat is its first incarnation, and a
\* seat after an unseat (which kept the identity) is its second, so a member
\* of incarnation 2 was seated before.
AuditForkSeat ==
    \E id \in AuditForkIds, g \in {1, 2} :
        /\ g = IF id \in DOMAIN identity_runtime_generations THEN 2 ELSE 1
        /\ RecoverRosterMemberRunning(id, AuditRuntime(id), 1, g, AuditProfile(id), AuditMode, FALSE)

\* A retiring member is not bound, made unborn or born again here: after its
\* retire only its delivery, its records, its release verdict and its unseat
\* apply to it.
AuditNotRetiring(id) == AuditRuntime(id) \notin DOMAIN runtime_retire_pending_sessions

AuditBind ==
    \E id \in AuditIds :
        /\ AuditNotRetiring(id)
        /\ RecoverMemberSessionBindingFreshRunning(id, AuditRuntime(id), AuditSession(id), None)

AuditUnborn ==
    \E id \in AuditIds :
        /\ AuditNotRetiring(id)
        /\ RecoverMemberSeatSessionUnbornRunning(id, AuditRuntime(id), TRUE)

AuditBorn ==
    \E id \in AuditIds :
        /\ AuditNotRetiring(id)
        /\ RecoverMemberSeatSessionBornRunning(id, AuditRuntime(id), FALSE)

AuditUnseatOf(ids) ==
    \E id \in ids :
        RecoverRosterMemberRetiredRunning(id, AuditRuntime(id), 1, FALSE, FALSE, TRUE, FALSE)

AuditUnseat == AuditUnseatOf(AuditIds)

\* A releasing retire of a bound member: it unbinds the member and moves its
\* session to the runtime's pending retirement. A seated member is Dormant
\* here, so the retire is archive-only.
AuditRetire ==
    \E id \in AuditIds :
        RetireRunningReleasingArchiveOnly(AuditMobId, AuditRuntime(id), id, 1,
            Some(AuditSession(id)), Some(AuditSession(id)))

\* ---------------------------------------------------------------------------
\* Fork lineage
\* ---------------------------------------------------------------------------

\* The lineage record of a member as a fork of any other name of the run, at
\* any time the machine admits it (a fork seat records it with its seat
\* commit; the audit does not hold it to that moment). The root need not be
\* seated.
AuditLineage(ids) ==
    \E id \in ids :
        \E root \in ids :
            RecordMemberInstructionLineageRunning(id, root)

\* The end of an unseated identity's life (the end of a retirement that ends
\* its identity).
AuditEndLife(ids) ==
    \E id \in ids :
        /\ id \notin DOMAIN identity_to_runtime
        /\ EndIdentityLifeRunning(id)

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

\* The replay of a committed record, or of the record its revoke left, in
\* the journal's order: the slot's next revision at the mob's next
\* generation.
AuditReplay(targets, generations) ==
    \E r \in AuditRevisions, t \in targets, e \in AuditEffects, g \in generations :
        /\ g = operator_instruction_generation + 1
        /\ IF AuditSlot \in DOMAIN operator_instruction_revision
           THEN operator_instruction_revision[AuditSlot] < r
           ELSE TRUE
        /\ RecoverOperatorInstructionAppliedRunning(AuditSlot, r, e, AuditReplayDigest(e),
               AuditReplayBytes(e), t.kind, t.profile, t.member, g)

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

\* The release verdict of a member's own session, or of the other member's
\* session, for a host-kept and an archived session.
AuditRelease ==
    \E id \in AuditIds, s \in AuditSessions, d \in AuditDisposals :
        \/ ResolveMemberInstructionReleaseAuthorizedRunning(id, s, d)
        \/ ResolveMemberInstructionReleaseNotNeededRunning(id, s, d)

AuditPrepareRetiring == \E id \in AuditIds : PrepareMemberInstructionDeliveryRetiringRunning(id)

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
    \/ AuditLineage(AuditIds)
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
    \/ AuditReplay(AuditWriteTargets, 1..AuditMaxWriteGeneration)
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
    \/ AuditRetire
    \/ AuditLineage(AuditIds)
    \/ AuditEndLife(AuditIds)
    \/ AuditSetNew(AuditSeatTargets)
    \/ AuditSetAdvance(AuditSeatTargets)
    \/ AuditPrepareNotOwed
    \/ AuditPrepareUnborn
    \/ AuditPrepareUnbound
    \/ AuditPrepareRetiring
    \/ AuditPrepareDeliver
    \/ AuditSeatRecordCurrent
    \/ AuditRelease

AuditSeatSpec == Init /\ [][AuditSeatNext]_vars

AuditSeatConstraint == operator_instruction_generation <= AuditMaxSeatGeneration

\* The fork run binds no member: its members are owed and never prepared,
\* and the write and seat runs cover the delivery of a fork. A name is
\* seated as its first or its second incarnation, so an unseated member that
\* kept its identity is seated again.
AuditForkNext ==
    \/ AuditForkSeat
    \/ AuditUnseatOf(AuditForkIds)
    \/ AuditLineage(AuditForkIds)
    \/ AuditEndLife(AuditForkIds)
    \/ AuditSetNew(AuditForkTargets)
    \/ AuditSetAdvance(AuditForkTargets)
    \/ AuditRevokeAdvance
    \/ AuditReplay(AuditForkTargets, 1..AuditMaxForkGeneration)

AuditForkSpec == Init /\ [][AuditForkNext]_vars

AuditForkConstraint == operator_instruction_generation <= AuditMaxForkGeneration

\* Every state variable but the counters no guard reads (see the header):
\* the two above and the identity life marker.
AuditView == [vars EXCEPT ![AuditStepIndex] = 0, ![AuditEpochIndex] = 0,
                          ![AuditLifeResetsIndex] = 0, ![AuditLifeEndsIndex] = 0]

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
\* the member `id`, whose fork lineage root `lineage` holds when it is a
\* fork: an independent statement of the machine's selector. A record for a
\* member and its forks selects the member it names, as a member record
\* does, and every member whose lineage root is that member.
AuditSelects(kinds, profiles, members, lineage, s, id) ==
    /\ s \in DOMAIN kinds
    /\ \/ kinds[s] = "Mob"
       \/ /\ kinds[s] = "Profile"
          /\ id \in DOMAIN member_profile_names
          /\ s \in DOMAIN profiles
          /\ member_profile_names[id] = profiles[s]
       \/ /\ kinds[s] \in {"Member", "MemberAndForks"}
          /\ s \in DOMAIN members
          /\ members[s] = id
       \/ /\ kinds[s] = "MemberAndForks"
          /\ s \in DOMAIN members
          /\ id \in DOMAIN lineage
          /\ lineage[id] = members[s]

AuditSelectedBefore(s, id) ==
    AuditSelects(operator_instruction_target_kind, operator_instruction_target_profile,
                 operator_instruction_target_member, member_instruction_lineage, s, id)

AuditSelectedAfter(s, id) ==
    AuditSelects(operator_instruction_target_kind', operator_instruction_target_profile',
                 operator_instruction_target_member', member_instruction_lineage', s, id)

\* The slots whose revision a step changed.
AuditChangedSlots ==
    {s \in DOMAIN operator_instruction_revision' :
        \/ s \notin DOMAIN operator_instruction_revision
        \/ operator_instruction_revision'[s] # operator_instruction_revision[s]}

\* A committed write: the generation advances by one and the roster stays.
AuditCommittedWrite ==
    /\ operator_instruction_generation' = operator_instruction_generation + 1
    /\ UNCHANGED identity_to_runtime

\* `id` is seated, a fork of the member the slot `s` names after the step,
\* and owed the step's generation.
AuditForkOwedBy(s, id) ==
    /\ id \in DOMAIN identity_to_runtime
    /\ id \in DOMAIN member_instruction_lineage
    /\ s \in DOMAIN operator_instruction_target_member'
    /\ member_instruction_lineage[id] = operator_instruction_target_member'[s]
    /\ AuditFactAt(member_instruction_owed', id, operator_instruction_generation')

AuditRecordedWithinGeneration ==
    \A id \in DOMAIN member_instruction_recorded :
        member_instruction_recorded[id] <= operator_instruction_generation

AuditOwedIsSeatedAndUnblocked ==
    \A id \in DOMAIN member_instruction_owed :
        /\ id \in DOMAIN identity_to_runtime
        /\ id \notin DOMAIN member_instruction_blocked
        /\ member_instruction_owed[id] <= operator_instruction_generation

AuditDeliversOnlyToBoundBornMembers ==
    \A id \in AuditForkIds :
        ENABLED PrepareMemberInstructionDeliveryDeliverRunning(id) =>
            /\ id \in DOMAIN member_instruction_owed
            /\ id \notin member_seat_session_unborn
            /\ id \in DOMAIN member_session_bindings

AuditLineageOnlyBeforeASnapshot ==
    \A id \in AuditForkIds :
        \A root \in AuditForkIds :
            ENABLED RecordMemberInstructionLineageRunning(id, root) =>
                /\ id \in DOMAIN identity_to_runtime
                /\ id # root
                /\ id \notin DOMAIN member_instruction_recorded

AuditWriteOwesExactlyTheSelected ==
    [][AuditCommittedWrite =>
         \A id \in DOMAIN identity_to_runtime :
             IF \E s \in AuditChangedSlots : AuditSelectedBefore(s, id) \/ AuditSelectedAfter(s, id)
             THEN AuditFactAt(member_instruction_owed', id, operator_instruction_generation')
             ELSE AuditSameFact(member_instruction_owed, member_instruction_owed', id)
      ]_vars

AuditSeatOwesTheGeneration ==
    [][\A id \in AuditForkIds :
         (id \notin DOMAIN identity_to_runtime /\ id \in DOMAIN identity_to_runtime') =>
             /\ id \notin DOMAIN member_instruction_recorded'
             /\ id \notin DOMAIN member_instruction_blocked'
             /\ IF operator_instruction_generation' > 0
                THEN AuditFactAt(member_instruction_owed', id, operator_instruction_generation')
                ELSE id \notin DOMAIN member_instruction_owed'
      ]_vars

AuditUnseatLeavesNoFact ==
    [][\A id \in AuditForkIds :
         (id \in DOMAIN identity_to_runtime /\ id \notin DOMAIN identity_to_runtime') =>
             /\ id \notin DOMAIN member_instruction_owed'
             /\ id \notin DOMAIN member_instruction_recorded'
             /\ id \notin DOMAIN member_instruction_blocked'
      ]_vars

\* The step is the lineage record of `id` with `root` (the arm as the
\* generated model defines it); what it leaves is stated independently. The
\* entry is the root as given, whatever entry the root holds itself; a
\* record repeated for a member that holds exactly that entry changes
\* nothing in the lineage.
AuditLineageRecordWritesTheGivenRoot ==
    [][\A id \in AuditForkIds :
         \A root \in AuditForkIds :
             RecordMemberInstructionLineageRunning(id, root) =>
                 /\ AuditFactAt(member_instruction_lineage', id, root)
                 /\ \A other \in AuditForkIds \ {id} :
                        AuditSameFact(member_instruction_lineage, member_instruction_lineage', other)
                 /\ AuditFactAt(member_instruction_lineage, id, root) =>
                        member_instruction_lineage' = member_instruction_lineage
      ]_vars

AuditLineageRecordOwesTheGeneration ==
    [][\A id \in AuditForkIds :
         \A root \in AuditForkIds :
             RecordMemberInstructionLineageRunning(id, root) =>
                 /\ IF operator_instruction_generation > 0
                    THEN /\ AuditFactAt(member_instruction_owed', id, operator_instruction_generation)
                         /\ id \notin DOMAIN member_instruction_blocked'
                    ELSE AuditSameFact(member_instruction_owed, member_instruction_owed', id)
                 /\ \A other \in AuditForkIds \ {id} :
                        AuditSameFact(member_instruction_owed, member_instruction_owed', other)
      ]_vars

\* The identity life marker of `id` moved: its identity ended.
AuditIdentityEnded(id) ==
    IF id \in DOMAIN identity_life_ends
    THEN IF id \in DOMAIN identity_life_ends'
         THEN identity_life_ends'[id] # identity_life_ends[id]
         ELSE FALSE
    ELSE id \in DOMAIN identity_life_ends'

AuditIdentityEndLeavesNoLineage ==
    [][\A id \in AuditForkIds :
         AuditIdentityEnded(id) => id \notin DOMAIN member_instruction_lineage'
      ]_vars

\* Each disjunct is evaluated on its own (TLC evaluates every disjunct of an
\* action-level disjunction): none reads a map outside its domain.
AuditLineageKeptUnlessRecordedOrEnded ==
    [][\A id \in AuditForkIds :
         \/ AuditSameFact(member_instruction_lineage, member_instruction_lineage', id)
         \/ AuditIdentityEnded(id)
         \/ \E root \in AuditForkIds : RecordMemberInstructionLineageRunning(id, root)
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

\* Write run: a member-targeted record retargeted to the other member owes
\* both (the first member's snapshot loses the record, the second gains it).
\* The machine looks both members up instead of searching the seats.
AuditWitnessMemberRetargetOwesBothStep ==
    /\ AuditCommittedWrite
    /\ \E s \in AuditChangedSlots :
           /\ s \in DOMAIN operator_instruction_target_kind
           /\ operator_instruction_target_kind[s] = "Member"
           /\ operator_instruction_target_kind'[s] = "Member"
           /\ operator_instruction_target_member[s] # operator_instruction_target_member'[s]
           /\ AuditFactAt(member_instruction_owed', operator_instruction_target_member[s],
                          operator_instruction_generation')
           /\ AuditFactAt(member_instruction_owed', operator_instruction_target_member'[s],
                          operator_instruction_generation')

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

\* Write run: a record written for a member and its forks owes a seated fork
\* of that member (the record names the fork's root, never the fork).
AuditWitnessForkWriteOwesForkStep ==
    /\ AuditCommittedWrite
    /\ \E s \in AuditChangedSlots :
           /\ operator_instruction_target_kind'[s] = "MemberAndForks"
           /\ \E id \in DOMAIN member_instruction_lineage : AuditForkOwedBy(s, id)

\* Write run: a new record written for a member alone leaves a seated fork
\* of that member as it was.
AuditWitnessMemberWriteSkipsForkStep ==
    /\ AuditCommittedWrite
    /\ \E s \in AuditChangedSlots :
           /\ s \notin DOMAIN operator_instruction_revision
           /\ operator_instruction_target_kind'[s] = "Member"
           /\ \E id \in DOMAIN member_instruction_lineage :
                  /\ id \in DOMAIN identity_to_runtime
                  /\ member_instruction_lineage[id] = operator_instruction_target_member'[s]
                  /\ ~AuditFactAt(member_instruction_owed', id, operator_instruction_generation')

\* Write run: the lineage record of a member that was not owed owes it the
\* mob's generation.
AuditWitnessLineageRecordOwesStep ==
    \E id \in AuditForkIds :
        /\ id \notin DOMAIN member_instruction_lineage
        /\ id \in DOMAIN member_instruction_lineage'
        /\ id \notin DOMAIN member_instruction_owed
        /\ operator_instruction_generation > 0
        /\ AuditFactAt(member_instruction_owed', id, operator_instruction_generation)

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

\* Seat run: the end of a fork's identity drops its lineage entry, which its
\* unseat had kept.
AuditWitnessIdentityEndDropsLineageStep ==
    \E id \in DOMAIN member_instruction_lineage :
        /\ id \notin DOMAIN identity_to_runtime
        /\ id \notin DOMAIN member_instruction_lineage'

\* Seat run: a record written for a member and its forks owes a seated fork
\* while the member it names is not seated (a selector is a predicate, not a
\* reference).
AuditWitnessForkOwedWithoutSeatedRootStep ==
    /\ AuditCommittedWrite
    /\ \E s \in AuditChangedSlots :
           /\ operator_instruction_target_kind'[s] = "MemberAndForks"
           /\ operator_instruction_target_member'[s] \notin DOMAIN identity_to_runtime
           /\ \E id \in DOMAIN member_instruction_lineage : AuditForkOwedBy(s, id)

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

\* Seat run: a retiring member whose session recorded a snapshot (its
\* releasing retire unbound it) has the release of its session authorized
\* for a host that keeps the session.
AuditWitnessRetiringReleasable ==
    \E id \in AuditIds :
        /\ id \in DOMAIN identity_to_runtime
        /\ id \notin DOMAIN member_session_bindings
        /\ AuditRuntime(id) \in DOMAIN runtime_retire_pending_sessions
        /\ id \in DOMAIN member_instruction_recorded
        /\ ENABLED ResolveMemberInstructionReleaseAuthorizedRunning(
               id, AuditSession(id), "RuntimeReleasedOnlyHostOwned")

\* Fork run: a member may hold a root that itself holds an entry for a third
\* name (a root is a name: the name of a fork's root lives again as a fork
\* of another member). The machine keeps both entries as given.
AuditWitnessRootHoldsAnEntry ==
    \E id \in DOMAIN member_instruction_lineage :
        /\ member_instruction_lineage[id] \in DOMAIN member_instruction_lineage
        /\ member_instruction_lineage[member_instruction_lineage[id]] # id

\* Fork run: a fork of a fork is recorded with the first member, as the
\* fork between is, so a record written for the first member and its forks
\* owes both of them.
AuditWitnessForkOfForkFollowsTheFirstMemberStep ==
    /\ AuditCommittedWrite
    /\ \E s \in AuditChangedSlots :
           /\ operator_instruction_target_kind'[s] = "MemberAndForks"
           /\ \E id \in DOMAIN member_instruction_lineage :
                  \E other \in DOMAIN member_instruction_lineage \ {id} :
                      /\ AuditForkOwedBy(s, id)
                      /\ AuditForkOwedBy(s, other)

\* Fork run: a new record written for a member that is itself a fork, and
\* its forks, owes that member and leaves a seated member whose root is the
\* same first member as it was: no entry names the fork between as a root.
AuditWitnessForkRecordSkipsSiblingForkStep ==
    /\ AuditCommittedWrite
    /\ \E s \in AuditChangedSlots :
           /\ s \notin DOMAIN operator_instruction_revision
           /\ operator_instruction_target_kind'[s] = "MemberAndForks"
           /\ operator_instruction_target_member'[s] \in DOMAIN member_instruction_lineage
           /\ AuditFactAt(member_instruction_owed', operator_instruction_target_member'[s],
                          operator_instruction_generation')
           /\ \E other \in DOMAIN member_instruction_lineage \ {operator_instruction_target_member'[s]} :
                  /\ other \in DOMAIN identity_to_runtime
                  /\ member_instruction_lineage[other] =
                         member_instruction_lineage[operator_instruction_target_member'[s]]
                  /\ ~AuditFactAt(member_instruction_owed', other, operator_instruction_generation')

\* Fork run: a seat of a name that holds a lineage entry (its unseat kept
\* its identity, so the entry stayed) keeps the entry.
AuditWitnessReseatKeepsLineageStep ==
    \E id \in DOMAIN member_instruction_lineage :
        /\ id \notin DOMAIN identity_to_runtime
        /\ id \in DOMAIN identity_to_runtime'
        /\ AuditFactAt(member_instruction_lineage', id, member_instruction_lineage[id])

\* Fork run: a fork seated as its second incarnation is owed by a new
\* record written for its root and its forks.
AuditWitnessReseatedForkOwedByRootRecordStep ==
    /\ AuditCommittedWrite
    /\ \E s \in AuditChangedSlots :
           /\ s \notin DOMAIN operator_instruction_revision
           /\ operator_instruction_target_kind'[s] = "MemberAndForks"
           /\ \E id \in DOMAIN member_instruction_lineage :
                  /\ AuditForkOwedBy(s, id)
                  /\ id \in DOMAIN identity_runtime_generations
                  /\ identity_runtime_generations[id] = 2

\* Fork run: a lineage record repeated for a member that holds exactly that
\* entry.
AuditWitnessLineageRecordRepeatedStep ==
    \E id \in DOMAIN member_instruction_lineage :
        RecordMemberInstructionLineageRunning(id, member_instruction_lineage[id])

\* Fork run: a new record written for a seated member and its forks owes the
\* member's seated fork and leaves a third seated member, neither the
\* member nor its fork, as it was.
AuditWitnessForkWriteSkipsBystanderStep ==
    /\ AuditCommittedWrite
    /\ \E s \in AuditChangedSlots :
           /\ s \notin DOMAIN operator_instruction_revision
           /\ operator_instruction_target_kind'[s] = "MemberAndForks"
           /\ operator_instruction_target_member'[s] \in DOMAIN identity_to_runtime
           /\ \E id \in DOMAIN member_instruction_lineage : AuditForkOwedBy(s, id)
           /\ \E other \in DOMAIN identity_to_runtime :
                  /\ ~AuditSelectedAfter(s, other)
                  /\ ~AuditFactAt(member_instruction_owed', other, operator_instruction_generation')

NotAuditWitnessProfileWriteOwesOne == [][~AuditWitnessProfileWriteOwesOneStep]_vars
NotAuditWitnessRetargetOwesLeaver == [][~AuditWitnessRetargetOwesLeaverStep]_vars
NotAuditWitnessMemberRetargetOwesBoth == [][~AuditWitnessMemberRetargetOwesBothStep]_vars
NotAuditWitnessRecordedReowed == [][~AuditWitnessRecordedReowedStep]_vars
NotAuditWitnessBlockedThenUnblocked == [][~AuditWitnessBlockedThenUnblockedStep]_vars
NotAuditWitnessRecordCloses == [][~AuditWitnessRecordClosesStep]_vars
NotAuditWitnessForkWriteOwesFork == [][~AuditWitnessForkWriteOwesForkStep]_vars
NotAuditWitnessMemberWriteSkipsFork == [][~AuditWitnessMemberWriteSkipsForkStep]_vars
NotAuditWitnessLineageRecordOwes == [][~AuditWitnessLineageRecordOwesStep]_vars
NotAuditWitnessDormantDeliverable == ~AuditWitnessDormantDeliverable
NotAuditWitnessSeatAfterWriteOwed == [][~AuditWitnessSeatAfterWriteOwedStep]_vars
NotAuditWitnessUnseatDropsFacts == [][~AuditWitnessUnseatDropsFactsStep]_vars
NotAuditWitnessIdentityEndDropsLineage == [][~AuditWitnessIdentityEndDropsLineageStep]_vars
NotAuditWitnessForkOwedWithoutSeatedRoot == [][~AuditWitnessForkOwedWithoutSeatedRootStep]_vars
NotAuditWitnessUnbornOwedDeferred == ~AuditWitnessUnbornOwedDeferred
NotAuditWitnessUnboundOwedDeferred == ~AuditWitnessUnboundOwedDeferred
NotAuditWitnessRetiringReleasable == ~AuditWitnessRetiringReleasable
NotAuditWitnessRootHoldsAnEntry == ~AuditWitnessRootHoldsAnEntry
NotAuditWitnessForkOfForkFollowsTheFirstMember == [][~AuditWitnessForkOfForkFollowsTheFirstMemberStep]_vars
NotAuditWitnessForkRecordSkipsSiblingFork == [][~AuditWitnessForkRecordSkipsSiblingForkStep]_vars
NotAuditWitnessReseatKeepsLineage == [][~AuditWitnessReseatKeepsLineageStep]_vars
NotAuditWitnessReseatedForkOwedByRootRecord == [][~AuditWitnessReseatedForkOwedByRootRecordStep]_vars
NotAuditWitnessLineageRecordRepeated == [][~AuditWitnessLineageRecordRepeatedStep]_vars
NotAuditWitnessForkWriteSkipsBystander == [][~AuditWitnessForkWriteSkipsBystanderStep]_vars
====
