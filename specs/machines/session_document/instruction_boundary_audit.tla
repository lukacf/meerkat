---- MODULE instruction_boundary_audit ----
\* Hand-written audit of the operator-instruction region of the generated
\* SessionDocumentMachine model.
\*
\* The machine holds every operator-instruction fact it decides from, per
\* session and per slot: the owner and the mob a mob owner is, the target
\* generation, the recorded target and the slots in effect (revision,
\* effect, digest and body bytes per slot), and the entry-sequence
\* high-water. This audit drives the generated arms directly, as the shell
\* does (one session, two slots, revisions 1..2, a small and a budget-sized
\* body, two mobs, every run start, all three lowerings, folds at and after
\* a run's first request, session writes, mob records, rebinds and releases,
\* interleaved), and checks the machine's own state. The audit's few
\* variables model only the run around the session: whether a run is open,
\* whether its first request was sent, whether the transcript has
\* conversation rows, which slots an application revoked, and which mob
\* released the target.
\*
\* Invariants (each a property of the last step, through audit_breach, or of
\* the state):
\*
\*   AuditEffectiveChangesOnlyAtRunStart: the slots in effect change only by
\*     an application at a FreshRun start or by a fold at a run's first
\*     request; never mid-turn, never on ResumeRun or NoRun, never by a
\*     write, a record, a rebind or a release.
\*   AuditApplicationReachesTheTarget: after an application or a run-start
\*     fold, nothing is pending.
\*   AuditOneEntryPerRunStart: one run start mints at most one entry,
\*     whatever the number of writes and records before it (coalescing).
\*   AuditAppliedRevokeStaysAbsent: a slot an application took out of effect
\*     stays out until a later write, record, rebind or release changes its
\*     target slot.
\*   AuditMidRunFoldKeepsEffective: a fold after a run's first request
\*     leaves the slots in effect unchanged.
\*   AuditSessionWriteNeverLowersRevision: a session write never replaces a
\*     slot's record with a lower or equal revision.
\*   AuditPrefixOnlyAppliesInLeadingPrefix: an application on a
\*     leading-prefix-only model happens only while the transcript is a
\*     leading System prefix, and nothing applies without a lowering.
\*   AuditMobTargetBoundUntilReleased: a mob-owned target changes its owner
\*     only when its own mob releases it; another mob's record or release
\*     and a rebind leave it alone.
\*   AuditReleaseEmptiesTheTarget: the owning mob's release leaves the
\*     target unowned and empty.
\*   AuditUnownedRunStartRevokesWhatIsInEffect: an unowned target is empty,
\*     so a fresh run start that can lower an entry leaves nothing in
\*     effect: a released mob's instructions are revoked there.
\*   AuditRebindStartsFromTheRightTarget: an attach starts the mob from
\*     what is in effect; a new owner of an unowned target starts from an
\*     empty one.
\*   AuditTargetWithinBudget: the target's Active bodies never exceed the
\*     active budget.
\*
\* The witnesses (NotAuditWitness*, expected violations, checked in one TLC
\* run with -continue): an application happened, an applied revoke, a
\* mob-owned target in effect, a target another mob bound after its owning
\* mob released it, and a released mob's instructions revoked at a run
\* start. The runner's --mutants mode seeds nine defects into a copy of the
\* generated model and requires TLC to refuse each with the invariant it
\* targets: apply on ResumeRun, apply a mid-run fold from the target,
\* accept a stale session write, skip the leading-prefix check, accept
\* another mob's record, let a rebind take a mob-owned target, record a
\* session write over the active budget, keep a released mob's
\* instructions at a run start, and keep the target on a release.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* instruction_boundary_audit.sh (its constants, its invariants and its
\* step constraint, unchanged):
\*   specs/machines/session_document/instruction_boundary_audit.sh --mutants
EXTENDS model

AuditSession == CHOOSE s \in SessionIdValues : TRUE
AuditSlots == {"a", "b"}
AuditRevisions == 1..2
AuditMobs == {"m1", "m2"}
AuditBudget == 1048576
\* The bodies a write or a snapshot may carry: a small one and one that
\* fills the whole active budget.
AuditBodies == {[digest |-> "dx", bytes |-> 1], [digest |-> "dy", bytes |-> AuditBudget]}

AuditNoSlots == [x \in {} |-> None]
AuditEmptySlots == [revision |-> AuditNoSlots, effect |-> AuditNoSlots, digest |-> AuditNoSlots, bytes |-> AuditNoSlots]
AuditActiveSlot(s, r, body) ==
    [revision |-> (s :> r), effect |-> (s :> "Active"), digest |-> (s :> body.digest),
     bytes |-> (s :> body.bytes)]
AuditAbsentSlot(s, r) ==
    [revision |-> (s :> r), effect |-> (s :> "Absent"), digest |-> (s :> ""), bytes |-> (s :> 0)]
AuditJoin(x, y) ==
    [revision |-> x.revision @@ y.revision, effect |-> x.effect @@ y.effect,
     digest |-> x.digest @@ y.digest, bytes |-> x.bytes @@ y.bytes]
\* The member snapshots a mob may deliver: empty, one Active slot, a revoke,
\* and two Active slots over the active budget.
AuditSnapshots ==
    { AuditEmptySlots,
      AuditActiveSlot("a", 1, [digest |-> "dx", bytes |-> 1]),
      AuditAbsentSlot("a", 2),
      AuditJoin(AuditActiveSlot("a", 2, [digest |-> "dy", bytes |-> AuditBudget]),
           AuditActiveSlot("b", 1, [digest |-> "dx", bytes |-> 1])) }

VARIABLES in_run, run_start_open, conversed, revoked_applied, released_from, rebound,
          revoked_after_release, audit_breach

audit_vars ==
    << in_run, run_start_open, conversed, revoked_applied, released_from, rebound,
       revoked_after_release, audit_breach >>
audit_all_vars == << vars, audit_vars >>

AuditSeeded == AuditSession \in DOMAIN session_instruction_owner

AuditOwner == session_instruction_owner[AuditSession]
AuditOwnerMob == session_instruction_owner_mob[AuditSession]
AuditOwnerNext == session_instruction_owner'[AuditSession]
AuditOwnerMobNext == session_instruction_owner_mob'[AuditSession]
AuditEntrySequence == session_instruction_entry_sequence[AuditSession]
AuditEntrySequenceNext == session_instruction_entry_sequence'[AuditSession]
AuditTarget ==
    [revision |-> session_instruction_target_revision[AuditSession],
     effect |-> session_instruction_target_effect[AuditSession],
     digest |-> session_instruction_target_digest[AuditSession],
     bytes |-> session_instruction_target_bytes[AuditSession]]
AuditTargetNext ==
    [revision |-> session_instruction_target_revision'[AuditSession],
     effect |-> session_instruction_target_effect'[AuditSession],
     digest |-> session_instruction_target_digest'[AuditSession],
     bytes |-> session_instruction_target_bytes'[AuditSession]]
AuditEffective ==
    [revision |-> session_instruction_effective_revision[AuditSession],
     effect |-> session_instruction_effective_effect[AuditSession],
     digest |-> session_instruction_effective_digest[AuditSession],
     bytes |-> session_instruction_effective_bytes[AuditSession]]
AuditEffectiveNext ==
    [revision |-> session_instruction_effective_revision'[AuditSession],
     effect |-> session_instruction_effective_effect'[AuditSession],
     digest |-> session_instruction_effective_digest'[AuditSession],
     bytes |-> session_instruction_effective_bytes'[AuditSession]]

\* One slot of a slot set; the empty tuple when the set does not name it.
AuditSlotOf(set, s) ==
    IF s \in DOMAIN set.revision
    THEN << set.revision[s], set.effect[s], set.digest[s], set.bytes[s] >>
    ELSE << >>
AuditActiveIn(set, s) == s \in DOMAIN set.effect /\ set.effect[s] = "Active"
AuditPendingIn(t, e) ==
    instruction_change_pending(t.revision, t.effect, t.digest, e.revision, e.effect, e.digest)

AuditEffectiveChanged == AuditEffectiveNext # AuditEffective
\* A mob-owned target whose owner this step changes.
AuditMobTargetTaken == AuditOwner = "Mob" /\ (AuditOwnerNext # AuditOwner \/ AuditOwnerMobNext # AuditOwnerMob)
\* Slots a step took out of effect, kept until their target slot changes.
AuditRevokedNext ==
    [s \in AuditSlots |->
        IF AuditSlotOf(AuditTargetNext, s) # AuditSlotOf(AuditTarget, s)
        THEN FALSE
        ELSE revoked_applied[s] \/ (AuditActiveIn(AuditEffective, s) /\ ~AuditActiveIn(AuditEffectiveNext, s))]

\* A step at an unowned target, after a release, that leaves nothing in
\* effect of what was.
AuditRevokesAfterRelease ==
    /\ released_from # ""
    /\ AuditOwner = "Unowned"
    /\ \E s \in AuditSlots : AuditActiveIn(AuditEffective, s)
    /\ ~\E s \in AuditSlots : AuditActiveIn(AuditEffectiveNext, s)

AuditInit ==
    /\ Init
    /\ in_run = FALSE
    /\ run_start_open = FALSE
    /\ conversed = FALSE
    /\ revoked_applied = [s \in AuditSlots |-> FALSE]
    /\ released_from = ""
    /\ rebound = FALSE
    /\ revoked_after_release = FALSE
    /\ audit_breach = {}

\* The shell recovers the session once (here, a session no target was ever
\* recorded for: the first recovery step empties both slot sets).
AuditSeed ==
    /\ ~AuditSeeded
    /\ RecoverSessionInstructionState(AuditSession, "Unowned", "", 0, 0)
    /\ UNCHANGED << in_run, run_start_open, conversed, revoked_applied, released_from, rebound,
                    revoked_after_release >>
    /\ audit_breach' = {}

\* A session host writes one slot: an activation of a body at a revision,
\* or a revoke at a revision.
AuditWrite(s, r, eff, dig, byt) ==
    /\ AuditSeeded
    /\ \/ SetSessionInstructionNew(AuditSession, s, r, eff, dig, byt)
       \/ SetSessionInstructionAdvance(AuditSession, s, r, eff, dig, byt)
       \/ SetSessionInstructionActiveBudgetExceeded(AuditSession, s, r, eff, dig, byt)
       \/ SetSessionInstructionDuplicate(AuditSession, s, r, eff, dig, byt)
       \/ SetSessionInstructionStale(AuditSession, s, r, eff, dig, byt)
       \/ SetSessionInstructionConflict(AuditSession, s, r, eff, dig, byt)
       \/ SetSessionInstructionOwnedByMob(AuditSession, s, r, eff, dig, byt)
       \/ SetSessionInstructionSlotLimitExceeded(AuditSession, s, r, eff, dig, byt)
       \/ SetSessionInstructionUnknownSlot(AuditSession, s, r, eff, dig, byt)
    /\ revoked_applied' = AuditRevokedNext
    /\ UNCHANGED << in_run, run_start_open, conversed, released_from, rebound,
                    revoked_after_release >>
    /\ audit_breach' =
           (IF AuditEffectiveChanged THEN {"EffectiveChangedOutsideRunStart"} ELSE {})
           \cup (IF /\ AuditSlotOf(AuditTargetNext, s) # AuditSlotOf(AuditTarget, s)
                    /\ s \in DOMAIN AuditTarget.revision
                    /\ AuditTargetNext.revision[s] <= AuditTarget.revision[s]
                 THEN {"SessionWriteLoweredRevision"} ELSE {})
           \cup (IF AuditMobTargetTaken THEN {"MobTargetTakenWithoutRelease"} ELSE {})

\* A mob delivers a whole member snapshot at a generation.
AuditRecord(m, g, snap) ==
    /\ AuditSeeded
    /\ \/ RecordInstructionTargetAdvance(AuditSession, m, g, snap.revision, snap.effect, snap.digest, snap.bytes)
       \/ RecordInstructionTargetActiveBudgetExceeded(AuditSession, m, g, snap.revision, snap.effect, snap.digest, snap.bytes)
       \/ RecordInstructionTargetAlreadyRecorded(AuditSession, m, g, snap.revision, snap.effect, snap.digest, snap.bytes)
       \/ RecordInstructionTargetOwnedBySession(AuditSession, m, g, snap.revision, snap.effect, snap.digest, snap.bytes)
       \/ RecordInstructionTargetOwnedByOtherMob(AuditSession, m, g, snap.revision, snap.effect, snap.digest, snap.bytes)
    /\ revoked_applied' = AuditRevokedNext
    /\ rebound' = (rebound \/ (released_from \notin {"", m} /\ AuditOwnerNext = "Mob" /\ AuditOwnerMobNext = m))
    /\ UNCHANGED << in_run, run_start_open, conversed, released_from, revoked_after_release >>
    /\ audit_breach' =
           (IF AuditEffectiveChanged THEN {"EffectiveChangedOutsideRunStart"} ELSE {})
           \cup (IF AuditMobTargetTaken THEN {"MobTargetTakenWithoutRelease"} ELSE {})

\* An explicit attach or a standalone adoption rebinds the owner.
AuditRebind(owner, m) ==
    /\ AuditSeeded
    /\ \/ RebindInstructionOwner(AuditSession, owner, m)
       \/ RebindInstructionOwnerUnowned(AuditSession, owner, m)
    /\ revoked_applied' = AuditRevokedNext
    /\ UNCHANGED << in_run, run_start_open, conversed, released_from, rebound,
                    revoked_after_release >>
    /\ audit_breach' =
           (IF AuditEffectiveChanged THEN {"EffectiveChangedOutsideRunStart"} ELSE {})
           \cup (IF AuditMobTargetTaken THEN {"MobTargetTakenWithoutRelease"} ELSE {})
           \cup (IF \/ (AuditOwner = "Unowned" /\ AuditTargetNext # AuditEmptySlots)
                    \/ (AuditOwner # "Unowned" /\ AuditTargetNext # AuditEffectiveNext)
                 THEN {"RebindStartedFromTheWrongTarget"} ELSE {})

\* A mob releases a member's target (a retire, a destroy).
AuditRelease(m) ==
    /\ AuditSeeded
    /\ \/ ReleaseInstructionTarget(AuditSession, m)
       \/ ReleaseInstructionTargetAlreadyUnowned(AuditSession, m)
       \/ ReleaseInstructionTargetOwnedByOtherMob(AuditSession, m)
       \/ ReleaseInstructionTargetOwnedBySession(AuditSession, m)
    /\ released_from' = IF AuditOwner = "Mob" /\ AuditOwnerNext = "Unowned" THEN AuditOwnerMob ELSE released_from
    /\ revoked_applied' = AuditRevokedNext
    /\ UNCHANGED << in_run, run_start_open, conversed, rebound, revoked_after_release >>
    /\ audit_breach' =
           (IF AuditEffectiveChanged THEN {"EffectiveChangedOutsideRunStart"} ELSE {})
           \cup (IF AuditMobTargetTaken /\ AuditOwnerMob # m THEN {"MobTargetTakenWithoutRelease"} ELSE {})
           \cup (IF /\ AuditOwner = "Mob"
                    /\ AuditOwnerMob = m
                    /\ (AuditOwnerNext # "Unowned" \/ AuditTargetNext # AuditEmptySlots)
                 THEN {"ReleaseKeptTarget"} ELSE {})

\* A turn starts: the shell resolves the run start before any input of the
\* run is appended, so the transcript is still a leading System prefix only
\* before the session's first run.
AuditRunStart(kind, lowering) ==
    /\ AuditSeeded
    /\ ~in_run
    /\ LET prefix == ~conversed
           lowers == lowering = "MidConversation" \/ (lowering = "LeadingPrefixOnly" /\ prefix)
           \* An unowned target is empty: a fresh run start that can lower an
           \* entry leaves nothing in effect.
           kept == IF /\ AuditOwner = "Unowned"
                      /\ kind = "FreshRun"
                      /\ lowers
                      /\ \E s \in AuditSlots : AuditActiveIn(AuditEffectiveNext, s)
                   THEN {"ReleasedInstructionsKept"} ELSE {}
       IN \/ /\ ResolveRunStartInstructionsApply(AuditSession, kind, lowering, prefix)
             /\ audit_breach' =
                    (IF kind # "FreshRun" THEN {"EffectiveChangedOutsideRunStart"} ELSE {})
                    \cup (IF AuditPendingIn(AuditTargetNext, AuditEffectiveNext)
                          THEN {"ApplicationLeftPending"} ELSE {})
                    \cup (IF \/ lowering = "Unavailable"
                             \/ (lowering = "LeadingPrefixOnly" /\ ~prefix)
                          THEN {"AppliedWithoutLowering"} ELSE {})
                    \cup (IF AuditEntrySequenceNext > AuditEntrySequence + 1
                          THEN {"SeveralEntriesAtOneRunStart"} ELSE {})
                    \cup (IF AuditMobTargetTaken THEN {"MobTargetTakenWithoutRelease"} ELSE {})
                    \cup kept
          \/ /\ \/ ResolveRunStartInstructionsNoChange(AuditSession, kind, lowering, prefix)
                \/ ResolveRunStartInstructionsDeferredLeadingPrefix(AuditSession, kind, lowering, prefix)
                \/ ResolveRunStartInstructionsDeferredUnavailable(AuditSession, kind, lowering, prefix)
             /\ audit_breach' =
                    (IF AuditEffectiveChanged THEN {"EffectiveChangedOutsideRunStart"} ELSE {})
                    \cup (IF AuditMobTargetTaken THEN {"MobTargetTakenWithoutRelease"} ELSE {})
                    \cup kept
    /\ revoked_applied' = AuditRevokedNext
    /\ revoked_after_release' = (revoked_after_release \/ AuditRevokesAfterRelease)
    /\ in_run' = (kind # "NoRun")
    /\ run_start_open' = (kind = "FreshRun")
    /\ conversed' = (conversed \/ kind # "NoRun")
    /\ UNCHANGED << released_from, rebound >>

\* A compaction folds the entry rows at a request boundary of the open run;
\* it is a run-start fold only at the run's first request.
AuditFold(lowering) ==
    /\ AuditSeeded
    /\ in_run
    /\ LET at_start == run_start_open
       IN \/ /\ AuthorizeInstructionFoldAtRunStart(AuditSession, at_start, lowering)
             /\ audit_breach' =
                    (IF ~at_start /\ AuditEffectiveChanged
                     THEN {"MidRunFoldChangedEffective", "EffectiveChangedOutsideRunStart"}
                     ELSE {})
                    \cup (IF AuditPendingIn(AuditTargetNext, AuditEffectiveNext)
                          THEN {"ApplicationLeftPending"} ELSE {})
                    \cup (IF AuditMobTargetTaken THEN {"MobTargetTakenWithoutRelease"} ELSE {})
          \/ /\ AuthorizeInstructionFoldEffective(AuditSession, at_start, lowering)
             /\ audit_breach' =
                    (IF AuditEffectiveChanged
                     THEN {"MidRunFoldChangedEffective", "EffectiveChangedOutsideRunStart"}
                     ELSE {})
                    \cup (IF AuditMobTargetTaken THEN {"MobTargetTakenWithoutRelease"} ELSE {})
    /\ revoked_applied' = AuditRevokedNext
    /\ revoked_after_release' = (revoked_after_release \/ AuditRevokesAfterRelease)
    /\ UNCHANGED << in_run, run_start_open, conversed, released_from, rebound >>

\* The open run sends its first request; a later fold is mid-run.
AuditFirstRequest ==
    /\ in_run
    /\ run_start_open
    /\ run_start_open' = FALSE
    /\ UNCHANGED << vars, in_run, conversed, revoked_applied, released_from, rebound,
                    revoked_after_release >>
    /\ audit_breach' = {}

AuditRunEnd ==
    /\ in_run
    /\ in_run' = FALSE
    /\ run_start_open' = FALSE
    /\ UNCHANGED << vars, conversed, revoked_applied, released_from, rebound,
                    revoked_after_release >>
    /\ audit_breach' = {}

AuditNext ==
    \/ AuditSeed
    \/ \E s \in AuditSlots, r \in AuditRevisions, body \in AuditBodies :
           AuditWrite(s, r, "Active", body.digest, body.bytes)
    \/ \E s \in AuditSlots, r \in AuditRevisions : AuditWrite(s, r, "Absent", "", 0)
    \/ \E m \in AuditMobs, g \in 1..2, snap \in AuditSnapshots : AuditRecord(m, g, snap)
    \/ AuditRebind("Session", "")
    \/ \E m \in AuditMobs : AuditRebind("Mob", m)
    \/ \E m \in AuditMobs : AuditRelease(m)
    \/ \E kind \in InstructionRunStartValues, lowering \in InstructionLoweringValues :
           AuditRunStart(kind, lowering)
    \/ \E lowering \in InstructionLoweringValues : AuditFold(lowering)
    \/ AuditFirstRequest
    \/ AuditRunEnd

AuditSpec == AuditInit /\ [][AuditNext]_audit_all_vars

AuditEffectiveChangesOnlyAtRunStart ==
    "EffectiveChangedOutsideRunStart" \notin audit_breach
AuditApplicationReachesTheTarget == "ApplicationLeftPending" \notin audit_breach
AuditOneEntryPerRunStart == "SeveralEntriesAtOneRunStart" \notin audit_breach
AuditAppliedRevokeStaysAbsent ==
    AuditSeeded => \A s \in AuditSlots : revoked_applied[s] => ~AuditActiveIn(AuditEffective, s)
AuditMidRunFoldKeepsEffective == "MidRunFoldChangedEffective" \notin audit_breach
AuditSessionWriteNeverLowersRevision == "SessionWriteLoweredRevision" \notin audit_breach
AuditPrefixOnlyAppliesInLeadingPrefix == "AppliedWithoutLowering" \notin audit_breach
AuditMobTargetBoundUntilReleased == "MobTargetTakenWithoutRelease" \notin audit_breach
AuditReleaseEmptiesTheTarget == "ReleaseKeptTarget" \notin audit_breach
AuditUnownedRunStartRevokesWhatIsInEffect == "ReleasedInstructionsKept" \notin audit_breach
AuditRebindStartsFromTheRightTarget == "RebindStartedFromTheWrongTarget" \notin audit_breach
AuditTargetWithinBudget ==
    AuditSeeded =>
        session_document_instruction_active_bytes(AuditTarget.effect, AuditTarget.bytes) <= AuditBudget

\* Reachability goals, checked as expected violations.
AuditWitnessApplied == AuditSeeded /\ \E s \in AuditSlots : AuditActiveIn(AuditEffective, s)
NotAuditWitnessApplied == ~AuditWitnessApplied
AuditWitnessRevokeApplied == \E s \in AuditSlots : revoked_applied[s]
NotAuditWitnessRevokeApplied == ~AuditWitnessRevokeApplied
AuditWitnessMobOwnedEffective ==
    /\ AuditSeeded
    /\ AuditOwner = "Mob"
    /\ \E s \in AuditSlots : AuditActiveIn(AuditEffective, s)
NotAuditWitnessMobOwnedEffective == ~AuditWitnessMobOwnedEffective
AuditWitnessReboundAfterRelease == rebound
NotAuditWitnessReboundAfterRelease == ~AuditWitnessReboundAfterRelease
AuditWitnessReleasedInstructionsRevoked == revoked_after_release
NotAuditWitnessReleasedInstructionsRevoked == ~AuditWitnessReleasedInstructionsRevoked
====
