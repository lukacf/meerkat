---- MODULE instruction_boundary_audit ----
\* Hand-written audit of the operator-instruction region of the generated
\* SessionDocumentMachine model.
\*
\* The machine holds the per-session facts it decides from (owner, target
\* generation, target slot count, effective active count, entry sequence);
\* the shell holds the typed sets (the recorded target, one record per slot,
\* and the effective set its own entry rows leave) and feeds the machine
\* mechanical observations of them. This audit composes the generated arms
\* with a model of that shell: one session, two slots, revisions 1..2, two
\* bodies, every run start (FreshRun, ResumeRun, NoRun), all three lowerings,
\* folds at and after a run's first request, mob target records, session
\* writes and owner rebinds, interleaved. The shell mirrors exactly what an
\* arm emits: a recorded write replaces its slot's record, a recorded mob
\* snapshot replaces the target, a rebind starts the target from what is in
\* effect, and an application or a run-start fold makes the target effective.
\*
\* Invariants (each a property of the last step, through audit_breach, or of
\* the state):
\*
\*   AuditEffectiveChangesOnlyAtRunStart: the effective set changes only by
\*     an application at a FreshRun start or by a fold at a run's first
\*     request; never mid-turn, never on ResumeRun or NoRun, never by a
\*     write, a record or a rebind.
\*   AuditApplicationReachesTheTarget: after an application or a run-start
\*     fold, no slot is pending.
\*   AuditOneEntryPerRunStart: one run start mints at most one entry,
\*     whatever the number of writes and records before it (coalescing).
\*   AuditAppliedRevokeStaysAbsent: a revoke, once applied, leaves the slot
\*     Absent until a later write or record names the slot again.
\*   AuditMidRunFoldKeepsEffective: a fold after a run's first request
\*     leaves the effective set unchanged.
\*   AuditSessionWriteNeverLowersRevision: a session write never replaces a
\*     slot's record with a lower or equal revision.
\*   AuditPrefixOnlyAppliesInLeadingPrefix: an application on a
\*     leading-prefix-only model happens only while the transcript is a
\*     leading System prefix, and nothing applies without a lowering.
\*   AuditMachineMirrorsShell: the machine's target slot count and effective
\*     active count are the shell's.
\*
\* The witnesses (NotAuditWitness*, expected violations, checked in one TLC
\* run with -continue): an application happened, an applied revoke, and a
\* mob-owned target in effect. The runner's --mutants mode seeds five defects
\* into a copy of the generated model and requires TLC to refuse each with
\* the invariant it targets: apply on ResumeRun, apply a mid-run fold from
\* the target, accept a stale session write, skip the leading-prefix check,
\* and let a rebind clear the target.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* instruction_boundary_audit.sh (its constants, its invariants and its
\* step constraint, unchanged):
\*   specs/machines/session_document/instruction_boundary_audit.sh --mutants
EXTENDS model

AuditSession == CHOOSE s \in SessionIdValues : TRUE
AuditSlots == {"a", "b"}
AuditRevisions == 1..2

\* A slot's record in the shell's target set; revision 0 is no record.
NoRecord == [rev |-> 0, effect |-> "Absent", body |-> "none"]
Record(r, e, b) == [rev |-> r, effect |-> e, body |-> b]
\* The writes a session host may send for one slot: a body at a revision, or
\* a revoke at a revision.
AuditWrites == { Record(r, "Active", b) : r \in AuditRevisions, b \in {"x", "y"} }
                   \cup { Record(r, "Absent", "none") : r \in AuditRevisions }
\* The member snapshots a mob may deliver.
AuditSnapshots ==
    { [s \in AuditSlots |-> NoRecord],
      [s \in AuditSlots |-> IF s = "a" THEN Record(1, "Active", "x") ELSE NoRecord],
      [s \in AuditSlots |-> IF s = "a" THEN Record(2, "Active", "y") ELSE Record(1, "Active", "x")],
      [s \in AuditSlots |-> IF s = "a" THEN Record(2, "Absent", "none") ELSE NoRecord] }

\* A slot as the session's entry rows leave it: in effect with a revision and
\* a body, or not in effect.
Inactive == [active |-> FALSE, rev |-> 0, body |-> "none"]

VARIABLES tgt, eff, in_run, run_start_open, conversed, revoked_applied, audit_breach

audit_vars == << tgt, eff, in_run, run_start_open, conversed, revoked_applied, audit_breach >>
audit_all_vars == << vars, audit_vars >>

Seeded == AuditSession \in DOMAIN session_instruction_owner

Present(t, s) == t[s].rev > 0
TargetActive(t, s) == Present(t, s) /\ t[s].effect = "Active"
\* A slot whose recorded target differs from what is in effect.
PendingIn(t, e, s) ==
    IF TargetActive(t, s)
    THEN ~e[s].active \/ e[s].rev # t[s].rev \/ e[s].body # t[s].body
    ELSE e[s].active
PendingCount == Cardinality({s \in AuditSlots : PendingIn(tgt, eff, s)})
TargetSlotCount(t) == Cardinality({s \in AuditSlots : Present(t, s)})
TargetActiveCount == Cardinality({s \in AuditSlots : TargetActive(tgt, s)})
EffectiveActiveCount(e) == Cardinality({s \in AuditSlots : e[s].active})
\* The effective set an application or a run-start fold leaves: the target.
Applied ==
    [s \in AuditSlots |->
        IF TargetActive(tgt, s)
        THEN [active |-> TRUE, rev |-> tgt[s].rev, body |-> tgt[s].body]
        ELSE Inactive]
\* The slots an application revokes.
RevokedBy(next) == {s \in AuditSlots : eff[s].active /\ ~next[s].active}

AuditInit ==
    /\ Init
    /\ tgt = [s \in AuditSlots |-> NoRecord]
    /\ eff = [s \in AuditSlots |-> Inactive]
    /\ in_run = FALSE
    /\ run_start_open = FALSE
    /\ conversed = FALSE
    /\ revoked_applied = [s \in AuditSlots |-> FALSE]
    /\ audit_breach = {}

\* The shell seeds the authority from its durable projection once (here, a
\* session no target was ever recorded for).
AuditSeed ==
    /\ ~Seeded
    /\ RecoverSessionInstructionState(AuditSession, "Unowned", 0, 0, 0, 0)
    /\ UNCHANGED << tgt, eff, in_run, run_start_open, conversed, revoked_applied >>
    /\ audit_breach' = {}

\* A session host writes one slot. The shell observes the slot's record and
\* mirrors only a Recorded verdict.
AuditSessionWrite(s, w) ==
    /\ Seeded
    /\ LET present == Present(tgt, s)
           current == tgt[s].rev
           matches == present /\ tgt[s].effect = w.effect /\ tgt[s].body = w.body
       IN \/ /\ \/ SetSessionInstructionNew(AuditSession, w.rev, w.effect, present, current, matches)
                \/ SetSessionInstructionAdvance(AuditSession, w.rev, w.effect, present, current, matches)
             /\ tgt' = [tgt EXCEPT ![s] = w]
          \/ /\ \/ SetSessionInstructionDuplicate(AuditSession, w.rev, w.effect, present, current, matches)
                \/ SetSessionInstructionStale(AuditSession, w.rev, w.effect, present, current, matches)
                \/ SetSessionInstructionConflict(AuditSession, w.rev, w.effect, present, current, matches)
                \/ SetSessionInstructionOwnedByMob(AuditSession, w.rev, w.effect, present, current, matches)
                \/ SetSessionInstructionSlotLimitExceeded(AuditSession, w.rev, w.effect, present, current, matches)
                \/ SetSessionInstructionUnknownSlot(AuditSession, w.rev, w.effect, present, current, matches)
             /\ UNCHANGED tgt
    /\ revoked_applied' = IF tgt' # tgt THEN [revoked_applied EXCEPT ![s] = FALSE] ELSE revoked_applied
    /\ UNCHANGED << eff, in_run, run_start_open, conversed >>
    /\ audit_breach' =
           IF tgt'[s] # tgt[s] /\ tgt'[s].rev <= tgt[s].rev
           THEN {"SessionWriteLoweredRevision"}
           ELSE {}

\* A mob delivers a whole member snapshot at a generation.
AuditMobRecord(g, snapshot) ==
    /\ Seeded
    /\ LET count == TargetSlotCount(snapshot)
       IN \/ /\ RecordInstructionTargetAdvance(AuditSession, g, count)
             /\ tgt' = snapshot
          \/ /\ \/ RecordInstructionTargetAlreadyRecorded(AuditSession, g, count)
                \/ RecordInstructionTargetOwnedBySession(AuditSession, g, count)
             /\ UNCHANGED tgt
    /\ revoked_applied' = [s \in AuditSlots |-> revoked_applied[s] /\ tgt'[s] = tgt[s]]
    /\ UNCHANGED << eff, in_run, run_start_open, conversed >>
    /\ audit_breach' = {}

\* An explicit attach or a standalone adoption rebinds the owner; the new
\* owner's target starts from what is in effect.
AuditRebind(owner) ==
    /\ Seeded
    /\ RebindInstructionOwner(AuditSession, owner)
    /\ tgt' = [s \in AuditSlots |->
                  IF eff[s].active THEN Record(eff[s].rev, "Active", eff[s].body) ELSE NoRecord]
    /\ UNCHANGED << eff, in_run, run_start_open, conversed, revoked_applied >>
    /\ audit_breach' = {}

\* A turn starts: the shell resolves the run start before any input of the
\* run is appended, so the transcript is still a leading System prefix only
\* before the session's first run.
AuditRunStart(kind, lowering) ==
    /\ Seeded
    /\ ~in_run
    /\ LET prefix == ~conversed
           pending == PendingCount
           active == TargetActiveCount
       IN \/ /\ ResolveRunStartInstructionsApply(AuditSession, kind, lowering, prefix, pending, active)
             /\ eff' = Applied
             /\ audit_breach' =
                    (IF kind # "FreshRun" THEN {"EffectiveChangedOutsideRunStart"} ELSE {})
                    \cup (IF \E s \in AuditSlots : PendingIn(tgt, Applied, s)
                          THEN {"ApplicationLeftPending"} ELSE {})
                    \cup (IF \/ lowering = "Unavailable"
                             \/ (lowering = "LeadingPrefixOnly" /\ ~prefix)
                          THEN {"AppliedWithoutLowering"} ELSE {})
                    \cup (IF session_instruction_entry_sequence'[AuditSession]
                               > session_instruction_entry_sequence[AuditSession] + 1
                          THEN {"SeveralEntriesAtOneRunStart"} ELSE {})
          \/ /\ \/ ResolveRunStartInstructionsNoChange(AuditSession, kind, lowering, prefix, pending, active)
                \/ ResolveRunStartInstructionsDeferredLeadingPrefix(AuditSession, kind, lowering, prefix, pending, active)
                \/ ResolveRunStartInstructionsDeferredUnavailable(AuditSession, kind, lowering, prefix, pending, active)
             /\ UNCHANGED eff
             /\ audit_breach' = {}
    /\ revoked_applied' = [s \in AuditSlots |->
                              revoked_applied[s] \/ (s \in RevokedBy(eff') /\ Present(tgt, s))]
    /\ in_run' = (kind # "NoRun")
    /\ run_start_open' = (kind = "FreshRun")
    /\ conversed' = (conversed \/ kind # "NoRun")
    /\ UNCHANGED tgt

\* A compaction folds the entry rows at a request boundary of the open run;
\* it is a run-start fold only at the run's first request.
AuditFold(lowering) ==
    /\ Seeded
    /\ in_run
    /\ LET at_start == run_start_open
           active == TargetActiveCount
       IN \/ /\ AuthorizeInstructionFoldAtRunStart(AuditSession, at_start, lowering, active)
             /\ eff' = Applied
             /\ audit_breach' =
                    (IF ~at_start /\ eff' # eff
                     THEN {"MidRunFoldChangedEffective", "EffectiveChangedOutsideRunStart"}
                     ELSE {})
                    \cup (IF \E s \in AuditSlots : PendingIn(tgt, Applied, s)
                          THEN {"ApplicationLeftPending"} ELSE {})
          \/ /\ AuthorizeInstructionFoldEffective(AuditSession, at_start, lowering, active)
             /\ UNCHANGED eff
             /\ audit_breach' = {}
    /\ revoked_applied' = [s \in AuditSlots |->
                              revoked_applied[s] \/ (s \in RevokedBy(eff') /\ Present(tgt, s))]
    /\ UNCHANGED << tgt, in_run, run_start_open, conversed >>

\* The open run sends its first request; a later fold is mid-run.
AuditFirstRequest ==
    /\ in_run
    /\ run_start_open
    /\ run_start_open' = FALSE
    /\ UNCHANGED << vars, tgt, eff, in_run, conversed, revoked_applied >>
    /\ audit_breach' = {}

AuditRunEnd ==
    /\ in_run
    /\ in_run' = FALSE
    /\ run_start_open' = FALSE
    /\ UNCHANGED << vars, tgt, eff, conversed, revoked_applied >>
    /\ audit_breach' = {}

AuditNext ==
    \/ AuditSeed
    \/ \E s \in AuditSlots, w \in AuditWrites : AuditSessionWrite(s, w)
    \/ \E g \in 1..2, snapshot \in AuditSnapshots : AuditMobRecord(g, snapshot)
    \/ \E owner \in {"Session", "Mob"} : AuditRebind(owner)
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
    \A s \in AuditSlots : revoked_applied[s] => ~eff[s].active
AuditMidRunFoldKeepsEffective == "MidRunFoldChangedEffective" \notin audit_breach
AuditSessionWriteNeverLowersRevision == "SessionWriteLoweredRevision" \notin audit_breach
AuditPrefixOnlyAppliesInLeadingPrefix == "AppliedWithoutLowering" \notin audit_breach
AuditMachineMirrorsShell ==
    Seeded =>
        /\ session_instruction_target_slot_count[AuditSession] = TargetSlotCount(tgt)
        /\ session_instruction_effective_active_count[AuditSession] = EffectiveActiveCount(eff)

\* Reachability goals, checked as expected violations.
AuditWitnessApplied == \E s \in AuditSlots : eff[s].active
NotAuditWitnessApplied == ~AuditWitnessApplied
AuditWitnessRevokeApplied == \E s \in AuditSlots : revoked_applied[s]
NotAuditWitnessRevokeApplied == ~AuditWitnessRevokeApplied
AuditWitnessMobOwnedEffective ==
    /\ Seeded
    /\ session_instruction_owner[AuditSession] = "Mob"
    /\ \E s \in AuditSlots : eff[s].active
NotAuditWitnessMobOwnedEffective == ~AuditWitnessMobOwnedEffective
====
