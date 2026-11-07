---- MODULE created_identity_audit ----
\* Hand-written audit of the MobMachine created identity entries: the runs
\* of created_identity_audit.sh, in two parts (recovery and changes).
\*
\* A created identity entry (origin Created) is recorded by the boot's scan
\* of the identity store's created entry rows (RecoverCreatedIdentityEntry,
\* after the journal replay and before the boot plan's seal), one row at a
\* time, from the row's presence and the machine's replayed facts: a Present
\* row of a seated local member records it a Created entry; a Present row of
\* an identity the replay left without a member records the entry owed its
\* seat, with the session its member was last bound to as its revival
\* record; a row pending its host's preparation records it
\* PreparationPending; a removed row records the name's tombstone; and an
\* unreadable row, or one the replay contradicts (a Declared entry of the
\* name, a member where the row says none, a placed member, the name's
\* tombstone, a spawn ladder open for it), reserves the name as unreadable.
\* Reserved names take no spawn, no worker reconcile, no declaration,
\* adoption or agent-lane command, no ladder close seats a member under one
\* (a ladder opened before the reservation is aborted), and created custody
\* seats a Created entry owed its seat again on its revival record. The
\* generated ci profile has no identities, so these arms hold there only
\* vacuously.
\* This module keeps the generated model unchanged, extends
\* identity_entry_apply_audit for its constants and arm families, and
\* checks fifteen runs over fixed domains: the four of the recovery part
\* below, the ten of the changes part (0.9.9: admit, commit, update,
\* removal, archive, preparation, prepverdicts, placement, unlist and
\* recovered) and the doors run of the doors part (0.9.11: no external work
\* reaches an identity entry), which their own sections describe.
\* The recovery part:
\*
\*   scan (ScanSpec): member 1 recovered and bound (a seated local member),
\*     its Declared entry row replayed or not, before or after the scan of
\*     its rows (its own replay never after the scan of its row pending its
\*     host's preparation: the boot's order, C1RecoverInBootOrder); member 2
\*     with no member; the scan of a row of either with every presence and
\*     revival record, in a running and a stopped mob; the boot plan's seal.
\*   reseat (ReseatSpec): neither member recovered; the scan of their rows,
\*     without a revival record but member 2's Present row (session 2); a
\*     spawn ladder for member 1 without custody, seated on session 1, and
\*     one for member 2 with each pair of custodies, seated on its revival
\*     record (session 2) or a fresh session (session 3); their aborts; the
\*     worker ensure of either member, refused for a reserved name; the
\*     worker reconcile of both members or of none; the forced destroy.
\*   names (NamesSpec): member 1 recovered and bound (in the boot's order,
\*     as in the scan run), its Declared entry row replayed or not; member 2
\*     with no member; the scan of their rows, without a revival record; the
\*     declarations, the seal over every desired set, the legacy adoption,
\*     the agent lane and the admission probe with each pair of custodies,
\*     under the build seed's composition authority, in a running and a
\*     stopped mob.
\*   placed (PlacedScanSpec): one member host bound and member 2 seated
\*     there by the placed spawn ladder, its activation committed; the scan
\*     of its row at any point of the ladder (a row under the open ladder
\*     reserves the name, and the remote commit then refuses it).
\*
\* Each run explores its whole reachable space under the VIEW of
\* member_residency_audit, with TLC coverage. Properties: every generated
\* invariant of ci.cfg (the created entries' among them:
\* tombstone_is_never_an_entry, entry_custodies_are_disjoint,
\* worker_reconcile_spares_entries, destroyed_mob_holds_no_member_lifecycle),
\* and per run:
\*   scan and placed: AuditRecoverExactlyOne (invariant, ENABLED form:
\*     exactly one arm answers every row while the replay runs, and none
\*     once the boot plan is sealed).
\*   scan and names: AuditCreatedEntryNeverDeclared (action property: no
\*     step turns a Created entry Declared, in either order of the replay
\*     and the scan).
\*   scan: AuditScanUnseatedCauses (invariant: the scan records a Created
\*     entry unseated only owed its seat, awaiting its host's preparation
\*     or archived; a store-owned update's declaration, which no recovery
\*     run makes, is what may record one hosted on its first use or held).
\*   reseat and placed: AuditNoSeatUnderReservedName (action property: no
\*     ladder close seats a member under a reserved name).
\*   reseat: AuditReseatRecoverExactlyOne (the same, for rows without a
\*     revival record, ladders and reseated members included);
\*     AuditNoLadderForReservedName (no spawn ladder opens for a
\*     reserved name); AuditLadderCustodyMatchesOrigin (a ladder holds entry
\*     custody only for a Declared entry's name or no entry's, and created
\*     custody only for a Created entry owed its seat); AuditCreatedSeatTakesRecord
\*     (a seat under created custody binds the entry's revival record); all
\*     action properties; AuditReseatProbeExactlyOne (invariant, ENABLED
\*     form: exactly one arm answers every admission probe, ladders and
\*     reseated members included).
\*   names: AuditNamesDeclareExactlyOne, AuditSealExactlyOne,
\*     AuditNamesAdoptionExactlyOne, AuditNamesLaneExactlyOne and
\*     AuditNamesProbeExactlyOne (invariants, ENABLED form: exactly one arm
\*     answers every declaration, seal, adoption, agent-lane admission and
\*     admission probe, in a running and a stopped mob);
\*     AuditSealNeverTouchesCreated (action property: the seal never
\*     unlists, marks or removes a Created entry or an unreadable name's).
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* violated negations. The TLC configs are DERIVED from the generated ci.cfg
\* on every run by created_identity_audit.sh.
EXTENDS identity_entry_apply_audit

(***************************************************************************)
(* Shared helpers.                                                         *)
(***************************************************************************)

CreatedIds == {Id1, Id2}
Presences == {"Present", "PresentUnseated", "PendingConstruction", "Tombstone", "Unreadable", "Archived"}
\* The revival records a row's scan may carry: none, or member 2's session.
Lasts == {None, Some(Sess2)}

Origin(id) == IF id \in DOMAIN identity_entry_origins THEN Some(identity_entry_origins[id]) ELSE None
OriginNext(id) == IF id \in DOMAIN identity_entry_origins' THEN Some(identity_entry_origins'[id]) ELSE None
Created(id) == Origin(id) = Some("Created")
Reserved(id) == id \in identity_entry_tombstones \/ id \in identity_entry_unreadable

\* Every arm of the boot scan for one row, in the phases it answers.
RecoverArm(id, presence, last) ==
    \/ RecoverCreatedIdentityEntrySeatedRunning(id, presence, last)
    \/ RecoverCreatedIdentityEntrySeatedStopped(id, presence, last)
    \/ RecoverCreatedIdentityEntryRetiredRunning(id, presence, last)
    \/ RecoverCreatedIdentityEntryRetiredStopped(id, presence, last)
    \/ RecoverCreatedIdentityEntryPendingRunning(id, presence, last)
    \/ RecoverCreatedIdentityEntryPendingStopped(id, presence, last)
    \/ RecoverCreatedIdentityEntryTombstoneRunning(id, presence, last)
    \/ RecoverCreatedIdentityEntryTombstoneStopped(id, presence, last)
    \/ RecoverCreatedIdentityEntryUnreadableRunning(id, presence, last)
    \/ RecoverCreatedIdentityEntryUnreadableStopped(id, presence, last)
    \/ RecoverCreatedIdentityEntryRemovingRunning(id, presence, last)
    \/ RecoverCreatedIdentityEntryRemovingStopped(id, presence, last)
    \/ RecoverCreatedIdentityEntryArchivingRunning(id, presence, last)
    \/ RecoverCreatedIdentityEntryArchivingStopped(id, presence, last)
    \/ RecoverCreatedIdentityEntryArchivedRunning(id, presence, last)
    \/ RecoverCreatedIdentityEntryArchivedStopped(id, presence, last)

RecoverVerdicts(id, presence, last) ==
    << ENABLED RecoverCreatedIdentityEntrySeatedRunning(id, presence, last),
       ENABLED RecoverCreatedIdentityEntrySeatedStopped(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryRetiredRunning(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryRetiredStopped(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryPendingRunning(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryPendingStopped(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryTombstoneRunning(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryTombstoneStopped(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryUnreadableRunning(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryUnreadableStopped(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryRemovingRunning(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryRemovingStopped(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryArchivingRunning(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryArchivingStopped(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryArchivedRunning(id, presence, last),
       ENABLED RecoverCreatedIdentityEntryArchivedStopped(id, presence, last) >>

RecoverScan == \E id \in CreatedIds, presence \in Presences, last \in Lasts : RecoverArm(id, presence, last)

\* Exactly one arm answers every row while the replay runs (a running or a
\* stopped mob whose boot plan is not sealed), and none once it is sealed.
\* No arm's guard reads the row's revival record (only the Retired arm's
\* update does), so the reseat run, whose states this costs most, counts
\* the arms of rows without one.
RecoverExactlyOneOver(lasts) ==
    \A id \in CreatedIds, presence \in Presences, last \in lasts :
        Cardinality({i \in 1..16 : RecoverVerdicts(id, presence, last)[i]})
            = IF boot_plan_sealed = FALSE /\ phase \in {"Running", "Stopped"} THEN 1 ELSE 0
AuditRecoverExactlyOne == RecoverExactlyOneOver(Lasts)

\* A store-owned identity never turns roster-owned: no step replaces a
\* Created entry's origin with Declared, whatever order the journal replay
\* and the boot's scan come in. A Created row may replay over a stale
\* Declared origin (the journal records no end of an entry that ends
\* unseated), so the property names this one direction.
AuditCreatedEntryNeverDeclared ==
    [][\A id \in CreatedIds : Origin(id) = Some("Created") => OriginNext(id) # Some("Declared")]_vars

\* The causes the boot's scan records a Created entry unseated for: owed its
\* seat (a Present row of an identity no member holds), awaiting its host's
\* preparation (a row pending it), or archived (an archived row).
AuditScanUnseatedCauses ==
    \A id \in DOMAIN identity_entry_unseated :
        Created(id) => identity_entry_unseated[id] \in {"SeatOwed", "PreparationPending", "Archived"}

\* Every arm of the admission probe in a running and a stopped mob.
ProbeVerdicts(id, ec, cc) ==
    << ENABLED ProbeMemberAdmissionDuplicateRunning(id, ec, cc),
       ENABLED ProbeMemberAdmissionAdmittedRunning(id, ec, cc),
       ENABLED ProbeMemberAdmissionIdentityMemberRunning(id, ec, cc),
       ENABLED ProbeMemberAdmissionIdentityMemberOfAnotherOriginRunning(id, ec, cc),
       ENABLED ProbeMemberAdmissionIdentityNameReservedRunning(id, ec, cc),
       ENABLED ProbeMemberAdmissionDuplicateStopped(id, ec, cc),
       ENABLED ProbeMemberAdmissionAdmittedStopped(id, ec, cc),
       ENABLED ProbeMemberAdmissionIdentityMemberStopped(id, ec, cc),
       ENABLED ProbeMemberAdmissionIdentityMemberOfAnotherOriginStopped(id, ec, cc),
       ENABLED ProbeMemberAdmissionIdentityNameReservedStopped(id, ec, cc) >>
ProbeArm(id, ec, cc) ==
    \/ ProbeMemberAdmissionDuplicateRunning(id, ec, cc)
    \/ ProbeMemberAdmissionAdmittedRunning(id, ec, cc)
    \/ ProbeMemberAdmissionIdentityMemberRunning(id, ec, cc)
    \/ ProbeMemberAdmissionIdentityMemberOfAnotherOriginRunning(id, ec, cc)
    \/ ProbeMemberAdmissionIdentityNameReservedRunning(id, ec, cc)
    \/ ProbeMemberAdmissionDuplicateStopped(id, ec, cc)
    \/ ProbeMemberAdmissionAdmittedStopped(id, ec, cc)
    \/ ProbeMemberAdmissionIdentityMemberStopped(id, ec, cc)
    \/ ProbeMemberAdmissionIdentityMemberOfAnotherOriginStopped(id, ec, cc)
    \/ ProbeMemberAdmissionIdentityNameReservedStopped(id, ec, cc)
ProbeExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in CreatedIds, ec \in BOOLEAN, cc \in BOOLEAN :
            Cardinality({i \in 1..10 : ProbeVerdicts(id, ec, cc)[i]}) = 1

\* Member 1's replay: its roster row, its binding and its Declared entry's
\* seat row.
C1Recover == RecoverRosterMemberRunning(Id1, Rid1, 1, 0, AuditProfile, AuditMode, FALSE)
C1Bind == RecoverMemberSessionBindingFreshRunning(Id1, Rid1, Sess1, None)
\* The replay of member 1's Declared entry row, before or after the scan of
\* its created row (the boot replays first): the arm's own guard refuses it
\* over the Created entry a scan recorded first, so a store-owned identity
\* never turns roster-owned by a replay (AuditCreatedEntryNeverDeclared).
C1Declared == RecoverIdentityEntrySeatedRunning(Id1, Rid1, "Declared")
CStop == AuditQuiesceStop \/ StopRunning \/ ResumeStopped

\* A boot-order precondition the scan and names runs keep: the boot replays
\* the journal (runtime/builder.rs:9362, seed_mob_authority_sync_from_events,
\* then :9369, recover_current_committed_placed_carrier_signals) before it
\* scans the created entry rows (:9424, recover_created_identity_entries).
\* MobMachine relies on that order for a row pending its host's
\* preparation: scanned first, the row records an identity without a member
\* awaiting its preparation, and a replay that seats the identity after it
\* drops that cause, so the start would neither settle the row of a
\* published construction nor reserve the name a member contradicts. So
\* member 1's replay never follows the scan of its pending row; a named
\* wrapper of one arm, which the runner's alias table counts for it.
C1RecoverInBootOrder ==
    /\ Unseated(Id1) # Some("PreparationPending")
    /\ C1Recover

(***************************************************************************)
(* The scan run.                                                           *)
(***************************************************************************)

ScanNext ==
    \/ C1RecoverInBootOrder
    \/ C1Bind
    \/ C1Declared
    \/ RecoverScan
    \/ CStop
    \/ SealBootMaterializationPlan

ScanSpec == Init /\ [][ScanNext]_vars

\* Witnesses of the scan run: one per verdict and contradiction.
AuditWitnessScanSeated ==
    /\ RecoverCreatedIdentityEntrySeatedRunning(Id1, "Present", None)
    /\ OriginNext(Id1) = Some("Created")
AuditWitnessScanRetired ==
    /\ RecoverCreatedIdentityEntryRetiredRunning(Id2, "Present", Some(Sess2))
    /\ UnseatedNext(Id2) = Some("SeatOwed")
    /\ RecordNext(Id2) = Some(Sess2)
AuditWitnessScanPending ==
    /\ RecoverCreatedIdentityEntryPendingRunning(Id2, "PresentUnseated", None)
    /\ UnseatedNext(Id2) = Some("PreparationPending")
AuditWitnessScanTombstone ==
    /\ RecoverCreatedIdentityEntryTombstoneRunning(Id2, "Tombstone", None)
    /\ Id2 \in identity_entry_tombstones'
AuditWitnessScanUnreadable ==
    /\ RecoverCreatedIdentityEntryUnreadableRunning(Id2, "Unreadable", None)
    /\ Id2 \in identity_entry_unreadable'
\* Contradicted rows: a Declared entry of the name, and a member where the
\* row says none.
AuditWitnessScanContradictedDeclared ==
    /\ Origin(Id1) = Some("Declared")
    /\ RecoverCreatedIdentityEntryUnreadableRunning(Id1, "Present", None)
AuditWitnessScanContradictedMember ==
    /\ Origin(Id1) = None
    /\ RecoverCreatedIdentityEntryUnreadableRunning(Id1, "PresentUnseated", None)
\* A removed created entry's row after its Present row: the entry goes, the
\* name is tombstoned.
AuditWitnessScanEntryTombstoned ==
    /\ Created(Id2)
    /\ RecoverCreatedIdentityEntryTombstoneRunning(Id2, "Tombstone", None)
    /\ Id2 \notin DOMAIN identity_entry_origins'

NotAuditWitnessScanSeated == [][~AuditWitnessScanSeated]_vars
NotAuditWitnessScanRetired == [][~AuditWitnessScanRetired]_vars
NotAuditWitnessScanPending == [][~AuditWitnessScanPending]_vars
NotAuditWitnessScanTombstone == [][~AuditWitnessScanTombstone]_vars
NotAuditWitnessScanUnreadable == [][~AuditWitnessScanUnreadable]_vars
NotAuditWitnessScanContradictedDeclared == [][~AuditWitnessScanContradictedDeclared]_vars
NotAuditWitnessScanContradictedMember == [][~AuditWitnessScanContradictedMember]_vars
NotAuditWitnessScanEntryTombstoned == [][~AuditWitnessScanEntryTombstoned]_vars

(***************************************************************************)
(* The reseat run.                                                         *)
(***************************************************************************)

\* Member 1 on runtime 1 (fence 1) and session 1, by a ladder without
\* custody; member 2 on runtime 2 (fence 2) with each pair of custodies, on
\* its revival record (session 2, archived) or a fresh session (session 3);
\* each at generation 0. A ladder's session is not recorded before its seat.
CRSeedAuth == SeedMemberResidencyRunning("OnDemand", "Authoritative", {})
CRRid(id) == IF id = Id1 THEN Rid1 ELSE Rid2
CRFence(id) == IF id = Id1 THEN 1 ELSE 2
CRAuthorize(id) ==
    AuthorizeSpawnProfileRunning(id, AuditProfile, AuditModel, AuditDigest, AuditDigest, AuditDigest, None, None, FALSE, None)
CROpenAs(id, ec, cc) ==
    BeginSpawnExecFresh(id, CRRid(id), CRFence(id), 0, AuditDigest, FALSE, AuditMode, Some(Sess3), None, None,
                        FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE,
                        None, None, None, None, FALSE, FALSE, ec, cc)
CROpen ==
    \/ CROpenAs(Id1, FALSE, FALSE)
    \/ \E ec \in BOOLEAN, cc \in BOOLEAN : CROpenAs(Id2, ec, cc)
CRSeatAs(id, s, archived) ==
    CommitDormantSeatRunning(id, CRRid(id), CRFence(id), 0, FALSE, AuditMode, s, None, archived, FALSE, FALSE, FALSE, FALSE)
CRSeat == CRSeatAs(Id1, Sess1, FALSE) \/ CRSeatAs(Id2, Sess2, TRUE) \/ CRSeatAs(Id2, Sess3, FALSE)
CRAbort(id) == AbortSpawnExecActiveRunning(id)
CRScan ==
    \/ \E presence \in Presences : RecoverArm(Id1, presence, None)
    \/ RecoverArm(Id2, "Present", Some(Sess2))
    \/ \E presence \in Presences \ {"Present"} : RecoverArm(Id2, presence, None)

ReseatNext ==
    \/ CRScan
    \/ CRSeedAuth
    \/ \E id \in CreatedIds : CRAuthorize(id) \/ CRAbort(id)
    \/ CROpen
    \/ CRSeat
    \/ \E d \in {{}, CreatedIds} : ReconcileRunning(d, TRUE)
    \/ \E id \in CreatedIds : EnsureMemberRunningExisting(id) \/ EnsureMemberRunningMissing(id)
    \/ AuditQuiesceDestroy
    \/ DestroyFromAny

ReseatSpec == Init /\ [][ReseatNext]_vars

\* No spawn ladder opens for a reserved name.
AuditNoLadderForReservedName ==
    [][\A id \in CreatedIds :
         (Reserved(id) /\ id \notin DOMAIN spawn_exec_phase) => id \notin DOMAIN spawn_exec_phase'
      ]_vars

\* A ladder holds entry custody only for a Declared entry's name or no
\* entry's, and created custody only for a Created entry no member holds
\* that is owed its seat, hosted on its first use or held for its creation.
AuditLadderCustodyMatchesOrigin ==
    [][\A id \in CreatedIds :
         /\ (id \notin spawn_exec_entry_custody /\ id \in spawn_exec_entry_custody')
              => Origin(id) \in {None, Some("Declared")}
         /\ (id \notin spawn_exec_created_custody /\ id \in spawn_exec_created_custody')
              => (Created(id)
                  /\ Unseated(id) \in {Some("SeatOwed"), Some("HostedOnFirstUse"), Some("HeldCreation")})
      ]_vars

\* A seat under created custody binds the entry's revival record.
AuditCreatedSeatTakesRecord ==
    [][\A id \in CreatedIds :
         (id \in spawn_exec_created_custody /\ Record(id) # None
            /\ id \notin DOMAIN member_session_bindings /\ id \in DOMAIN member_session_bindings')
           => BindingNext(id) = Record(id)
      ]_vars

\* No ladder close seats a member under a reserved name: a ladder opened
\* before its name was reserved ends without one.
AuditNoSeatUnderReservedName ==
    [][\A id \in CreatedIds :
         (Reserved(id) /\ id \notin DOMAIN identity_to_runtime) => id \notin DOMAIN identity_to_runtime'
      ]_vars

AuditReseatRecoverExactlyOne == RecoverExactlyOneOver({None})
AuditReseatProbeExactlyOne == ProbeExactlyOne

\* Witnesses of the reseat run.
\* A Created entry owed its seat seated again on its revival record under
\* created custody: it stays a Created entry.
AuditWitnessReseatOnRecord ==
    /\ Id2 \in spawn_exec_created_custody
    /\ Record(Id2) = Some(Sess2)
    /\ CRSeatAs(Id2, Sess2, TRUE)
    /\ OriginNext(Id2) = Some("Created")
    /\ Id2 \notin spawn_exec_created_custody'
\* A created ladder aborted: its custody goes with it.
AuditWitnessCreatedCustodyAborted ==
    /\ Id2 \in spawn_exec_created_custody
    /\ CRAbort(Id2)
    /\ Id2 \notin spawn_exec_created_custody'
\* A row of a name a created ladder acts for is contradicted: the name is
\* reserved, its entry kept.
AuditWitnessScanContradictedLadder ==
    /\ Id2 \in spawn_exec_created_custody
    /\ RecoverCreatedIdentityEntryUnreadableRunning(Id2, "Present", Some(Sess2))
    /\ OriginNext(Id2) = Some("Created")
\* A ladder without custody opened before its name was reserved (a Present
\* row under it answered Unreadable): its seat is refused, and its abort
\* ends it with the name still reserved and no member.
AuditWitnessReservedLadderAborted ==
    /\ Id1 \in identity_entry_unreadable
    /\ Id1 \notin spawn_exec_entry_custody \cup spawn_exec_created_custody
    /\ ~ENABLED CRSeatAs(Id1, Sess1, FALSE)
    /\ CRAbort(Id1)
    /\ Id1 \notin DOMAIN spawn_exec_phase'
    /\ Id1 \in identity_entry_unreadable'
    /\ Id1 \notin DOMAIN identity_to_runtime'
\* The worker reconcile spares a reserved name it was asked for.
AuditWitnessReconcileSparesReserved ==
    /\ Id1 \in identity_entry_tombstones
    /\ ReconcileRunning(CreatedIds, TRUE)
    /\ Id1 \notin members_to_spawn'
\* The worker ensure refuses a reserved name that is no entry and no
\* member, while it records a plain name for a spawn in the same state: the
\* name's reservation is all that refuses it.
AuditWitnessEnsureSparesReserved ==
    /\ Reserved(Id1)
    /\ Origin(Id1) = None
    /\ Id1 \notin DOMAIN identity_to_runtime
    /\ ~ENABLED EnsureMemberRunningMissing(Id1)
    /\ EnsureMemberRunningMissing(Id2)
    /\ Id2 \in members_to_spawn'
\* The forced destroy clears the reserved names.
AuditWitnessDestroyClearsReserved ==
    /\ identity_entry_tombstones # {}
    /\ identity_entry_unreadable # {}
    /\ DestroyFromAny
    /\ identity_entry_tombstones' = {}
    /\ identity_entry_unreadable' = {}

NotAuditWitnessReseatOnRecord == [][~AuditWitnessReseatOnRecord]_vars
NotAuditWitnessCreatedCustodyAborted == [][~AuditWitnessCreatedCustodyAborted]_vars
NotAuditWitnessScanContradictedLadder == [][~AuditWitnessScanContradictedLadder]_vars
NotAuditWitnessReservedLadderAborted == [][~AuditWitnessReservedLadderAborted]_vars
NotAuditWitnessReconcileSparesReserved == [][~AuditWitnessReconcileSparesReserved]_vars
NotAuditWitnessEnsureSparesReserved == [][~AuditWitnessEnsureSparesReserved]_vars
NotAuditWitnessDestroyClearsReserved == [][~AuditWitnessDestroyClearsReserved]_vars

(***************************************************************************)
(* The names run.                                                          *)
(***************************************************************************)

\* The declarations of the run: local, the member's own profile and mode.
NamesShape == <<AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None">>

\* Every arm of the declared apply's verdict for one declaration: the
\* declared apply runs' arms and the refusals of a created, removed or
\* unreadable name.
DeclareArmAll(id, s) ==
    \/ DeclareArm(id, s)
    \/ DeclareIdentityEntryRefusedNameOwnedByCreated(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], FALSE, FALSE, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRefusedNameTombstoned(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], FALSE, FALSE, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRefusedNameUnreadable(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], FALSE, FALSE, FALSE, FALSE, 0)
DeclareOneArmAll(id, s) ==
    Cardinality({i \in 1..18 :
        (DeclareEnabled(id, s) \o
         << ENABLED DeclareIdentityEntryRefusedNameOwnedByCreated(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], FALSE, FALSE, FALSE, FALSE, 0),
            ENABLED DeclareIdentityEntryRefusedNameTombstoned(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], FALSE, FALSE, FALSE, FALSE, 0),
            ENABLED DeclareIdentityEntryRefusedNameUnreadable(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], FALSE, FALSE, FALSE, FALSE, 0) >>)[i]}) = 1

\* Every arm of the legacy adoption, every phase of the run, for each of the
\* sixteen combinations of the shell's raw observations (AdoptArm).
NamesAdoptVerdicts(id, m, l, li, i) ==
    << ENABLED AdoptLegacyIdentityEntryAdopted(id, 0, m, l, li, i),
       ENABLED AdoptLegacyIdentityEntryAdoptedUnlisted(id, 0, m, l, li, i),
       ENABLED AdoptLegacyIdentityEntryNameHeldByWorker(id, 0, m, l, li, i),
       ENABLED AdoptLegacyIdentityEntryRetireUnadopted(id, 0, m, l, li, i),
       ENABLED AdoptLegacyIdentityEntryNotAdoptable(id, 0, m, l, li, i),
       ENABLED AdoptLegacyIdentityEntryStaleObservation(id, 0, m, l, li, i),
       ENABLED AdoptLegacyIdentityEntryNotSeated(id, 0, m, l, li, i),
       ENABLED AdoptLegacyIdentityEntryAlreadyAnEntry(id, 0, m, l, li, i),
       ENABLED AdoptLegacyIdentityEntryOtherCustody(id, 0, m, l, li, i),
       ENABLED AdoptLegacyIdentityEntryNonAuthoritative(id, 0, m, l, li, i),
       ENABLED AdoptLegacyIdentityEntryMobNotRunningStopped(id, 0, m, l, li, i) >>

\* Every arm of the agent lane's member admission, every phase of the run.
NamesLaneVerdicts(id) ==
    << ENABLED ResolveAgentLaneMemberAdmissionAdmittedRunning(id),
       ENABLED ResolveAgentLaneMemberAdmissionIdentityMemberRunning(id),
       ENABLED ResolveAgentLaneMemberAdmissionUnreadableNameRunning(id),
       ENABLED ResolveAgentLaneMemberAdmissionAdmittedStopped(id),
       ENABLED ResolveAgentLaneMemberAdmissionIdentityMemberStopped(id),
       ENABLED ResolveAgentLaneMemberAdmissionUnreadableNameStopped(id) >>
NamesLaneArm(id) ==
    \/ ResolveAgentLaneMemberAdmissionAdmittedRunning(id)
    \/ ResolveAgentLaneMemberAdmissionIdentityMemberRunning(id)
    \/ ResolveAgentLaneMemberAdmissionUnreadableNameRunning(id)
    \/ ResolveAgentLaneMemberAdmissionAdmittedStopped(id)
    \/ ResolveAgentLaneMemberAdmissionIdentityMemberStopped(id)
    \/ ResolveAgentLaneMemberAdmissionUnreadableNameStopped(id)

\* The rows of the names run carry no revival record: no command it checks
\* reads one.
NamesScan == \E id \in CreatedIds, presence \in Presences : RecoverArm(id, presence, None)

NamesNext ==
    \/ C1RecoverInBootOrder
    \/ C1Bind
    \/ C1Declared
    \/ NamesScan
    \/ ASeed
    \/ CStop
    \/ \E id \in CreatedIds : DeclareArmAll(id, NamesShape)
    \/ \E d \in SUBSET CreatedIds : SealArm(d)
    \/ \E id \in CreatedIds, m, l, li, i \in BOOLEAN : AdoptArm(id, 0, m, l, li, i)
    \/ \E id \in CreatedIds : NamesLaneArm(id)
    \/ \E id \in CreatedIds, ec \in BOOLEAN, cc \in BOOLEAN : ProbeArm(id, ec, cc)

NamesSpec == Init /\ [][NamesNext]_vars

AuditNamesDeclareExactlyOne == \A id \in CreatedIds : DeclareOneArmAll(id, NamesShape)
AuditNamesAdoptionExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in CreatedIds, m, l, li, i \in BOOLEAN :
            Cardinality({k \in 1..11 : NamesAdoptVerdicts(id, m, l, li, i)[k]}) = 1
AuditNamesLaneExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in CreatedIds : Cardinality({i \in 1..6 : NamesLaneVerdicts(id)[i]}) = 1
AuditNamesProbeExactlyOne == ProbeExactlyOne

\* The seal never unlists, marks or removes a Created entry or an
\* unreadable name's entry.
AuditSealNeverTouchesCreated ==
    [][\A d \in SUBSET CreatedIds :
         SealDeclaredRosterSealed(d, {})
           => \A id \in CreatedIds :
                (Created(id) \/ id \in identity_entry_unreadable)
                  => /\ OriginNext(id) = Origin(id)
                     /\ UnseatedNext(id) = Unseated(id)
                     /\ ((id \in identity_entry_retiring') = (id \in identity_entry_retiring))
      ]_vars

\* Witnesses of the names run. The refusals of a created, removed or
\* unreadable name, the adoption's OtherCustody, the agent lane's
\* UnreadableName and the probe's IdentityNameReserved, running and stopped,
\* answer exactly the names their guards hold: the coverage check, which
\* requires each to fire in this run, is their witness.
\* A complete seal that does not list a Created entry keeps it.
AuditWitnessSealKeepsCreated ==
    /\ Created(Id2)
    /\ SealDeclaredRosterSealed({}, {})
    /\ OriginNext(Id2) = Some("Created")
\* A complete seal that does not list an unreadable name's Declared entry
\* keeps it unmarked.
AuditWitnessSealSkipsUnreadable ==
    /\ Origin(Id1) = Some("Declared")
    /\ Id1 \in identity_entry_unreadable
    /\ Id1 \in DOMAIN identity_to_runtime
    /\ SealDeclaredRosterSealed({}, {})
    /\ Id1 \notin identity_entry_retiring'
\* The probe refuses entry custody over a Created entry's name.
AuditWitnessProbeOtherOrigin ==
    /\ Created(Id2)
    /\ ProbeMemberAdmissionIdentityMemberOfAnotherOriginRunning(Id2, TRUE, FALSE)

NotAuditWitnessSealKeepsCreated == [][~AuditWitnessSealKeepsCreated]_vars
NotAuditWitnessSealSkipsUnreadable == [][~AuditWitnessSealSkipsUnreadable]_vars
NotAuditWitnessProbeOtherOrigin == [][~AuditWitnessProbeOtherOrigin]_vars

(***************************************************************************)
(* The placed run.                                                         *)
(***************************************************************************)

\* One member host bound and member 2 seated there by the placed spawn
\* ladder (the declared apply's placed run's steps) and its activation, then
\* its row scanned.
PlacedScanNext ==
    \/ POwner
    \/ PBeginHost
    \/ PCommitHost
    \/ PAuthorize(Id2)
    \/ POpen(Id2, Rid2, 2, FALSE)
    \/ PCommit
    \/ CommitSpawnActivationFinalRunning(Id2)
    \/ \E presence \in Presences, last \in Lasts : RecoverArm(Id2, presence, last)

PlacedScanSpec == Init /\ [][PlacedScanNext]_vars

\* A Present row of a placed member, its ladder ended, is contradicted: the
\* name is reserved.
AuditWitnessScanContradictedPlaced ==
    /\ Id2 \in DOMAIN member_placement
    /\ Id2 \notin DOMAIN spawn_exec_phase
    /\ RecoverCreatedIdentityEntryUnreadableRunning(Id2, "Present", None)

NotAuditWitnessScanContradictedPlaced == [][~AuditWitnessScanContradictedPlaced]_vars

(***************************************************************************)
(* The changes part (0.9.9): a change's admission (admit), a store-owned   *)
(* entry's record (commit), a store-owned update beside a roster's change  *)
(* of the same spec (update), the removal of either custody (removal), the *)
(* archive and unarchive (archive), the host's preparation and its held    *)
(* constructions (preparation) and their verdicts (prepverdicts), the      *)
(* placed and peer-only spawns of a store-owned identity (placement), the  *)
(* one-entry unlist of a roster-owned identity (unlist) and the replay of  *)
(* a removal or an archive in flight carried to its terminal (recovered).  *)
(***************************************************************************)

Receipts == {"Absent", "Same", "Other"}
ConsentKinds == {"Policy", "Decided", "Imported"}
Seated(id) == id \in DOMAIN identity_to_runtime

\* The facts a new store-owned entry is recorded from, <<preparation
\* required, placement kind, held creation>>: each cause, and each
\* precedence between two facts (a preparation over a placement and a held
\* creation, a placement over a held creation).
Facts ==
    {<<FALSE, "Local", FALSE>>, <<FALSE, "Local", TRUE>>, <<FALSE, "Placed", FALSE>>,
     <<FALSE, "External", TRUE>>, <<TRUE, "Placed", TRUE>>}
\* The facts of a request whose new entry is unseated for `cause`.
FactsFor(cause) ==
    CASE cause = "PreparationPending" -> <<TRUE, "Local", FALSE>>
      [] cause = "HostedOnFirstUse" -> <<FALSE, "Placed", FALSE>>
      [] cause = "HeldCreation" -> <<FALSE, "Local", TRUE>>
      [] OTHER -> <<FALSE, "Local", FALSE>>
\* The cause the declared apply's precedence gives an identity never
\* created, from its facts: its preparation, then a placed or externally
\* bound spec, then its held creation, and otherwise its seat owed.
ExpectedCause(f) ==
    IF f[1] THEN "PreparationPending"
    ELSE IF f[2] # "Local" THEN "HostedOnFirstUse"
    ELSE IF f[3] THEN "HeldCreation"
    ELSE "SeatOwed"

\* Every arm of a change's admission, in a running and a stopped mob.
AdmitArm(r, c, pb, pa, ho, gc, sc) ==
    \/ AdmitIdentityChangeAdmitted(r, c, pb, pa, ho, gc, sc)
    \/ AdmitIdentityChangeReplayed(r, c, pb, pa, ho, gc, sc)
    \/ AdmitIdentityChangeOperationReused(r, c, pb, pa, ho, gc, sc)
    \/ AdmitIdentityChangeNonAuthoritative(r, c, pb, pa, ho, gc, sc)
    \/ AdmitIdentityChangeChangesHeld(r, c, pb, pa, ho, gc, sc)
    \/ AdmitIdentityChangeDecisionUnbound(r, c, pb, pa, ho, gc, sc)
    \/ AdmitIdentityChangeDecisionExpired(r, c, pb, pa, ho, gc, sc)
    \/ AdmitIdentityChangeImportedNotHostOperator(r, c, pb, pa, ho, gc, sc)
    \/ AdmitIdentityChangeGrantRevoked(r, c, pb, pa, ho, gc, sc)
    \/ AdmitIdentityChangeStaleSpec(r, c, pb, pa, ho, gc, sc)
    \/ AdmitIdentityChangeMobNotRunningStopped(r, c, pb, pa, ho, gc, sc)
AdmitVerdicts(r, c, pb, pa, ho, gc, sc) ==
    << ENABLED AdmitIdentityChangeAdmitted(r, c, pb, pa, ho, gc, sc),
       ENABLED AdmitIdentityChangeReplayed(r, c, pb, pa, ho, gc, sc),
       ENABLED AdmitIdentityChangeOperationReused(r, c, pb, pa, ho, gc, sc),
       ENABLED AdmitIdentityChangeNonAuthoritative(r, c, pb, pa, ho, gc, sc),
       ENABLED AdmitIdentityChangeChangesHeld(r, c, pb, pa, ho, gc, sc),
       ENABLED AdmitIdentityChangeDecisionUnbound(r, c, pb, pa, ho, gc, sc),
       ENABLED AdmitIdentityChangeDecisionExpired(r, c, pb, pa, ho, gc, sc),
       ENABLED AdmitIdentityChangeImportedNotHostOperator(r, c, pb, pa, ho, gc, sc),
       ENABLED AdmitIdentityChangeGrantRevoked(r, c, pb, pa, ho, gc, sc),
       ENABLED AdmitIdentityChangeStaleSpec(r, c, pb, pa, ho, gc, sc),
       ENABLED AdmitIdentityChangeMobNotRunningStopped(r, c, pb, pa, ho, gc, sc) >>
AdmitAny ==
    \E r \in Receipts, c \in ConsentKinds, pb \in BOOLEAN, pa \in BOOLEAN, ho \in BOOLEAN,
       gc \in BOOLEAN, sc \in BOOLEAN : AdmitArm(r, c, pb, pa, ho, gc, sc)

\* Every arm of a new store-owned entry's record from the facts `f`, in a
\* running and a stopped mob.
RecordArm(id, f) ==
    \/ RecordCreatedIdentityEntryRecorded(id, f[1], f[2], f[3])
    \/ RecordCreatedIdentityEntryNameHeldByIdentity(id, f[1], f[2], f[3])
    \/ RecordCreatedIdentityEntryNameHeldByWorker(id, f[1], f[2], f[3])
    \/ RecordCreatedIdentityEntryNameTombstoned(id, f[1], f[2], f[3])
    \/ RecordCreatedIdentityEntryNameUnreadable(id, f[1], f[2], f[3])
    \/ RecordCreatedIdentityEntryNonAuthoritative(id, f[1], f[2], f[3])
    \/ RecordCreatedIdentityEntryChangesHeld(id, f[1], f[2], f[3])
    \/ RecordCreatedIdentityEntryMobNotRunningStopped(id, f[1], f[2], f[3])
RecordVerdicts(id, f) ==
    << ENABLED RecordCreatedIdentityEntryRecorded(id, f[1], f[2], f[3]),
       ENABLED RecordCreatedIdentityEntryNameHeldByIdentity(id, f[1], f[2], f[3]),
       ENABLED RecordCreatedIdentityEntryNameHeldByWorker(id, f[1], f[2], f[3]),
       ENABLED RecordCreatedIdentityEntryNameTombstoned(id, f[1], f[2], f[3]),
       ENABLED RecordCreatedIdentityEntryNameUnreadable(id, f[1], f[2], f[3]),
       ENABLED RecordCreatedIdentityEntryNonAuthoritative(id, f[1], f[2], f[3]),
       ENABLED RecordCreatedIdentityEntryChangesHeld(id, f[1], f[2], f[3]),
       ENABLED RecordCreatedIdentityEntryMobNotRunningStopped(id, f[1], f[2], f[3]) >>
\* The record of a new entry unseated for `cause`.
RecordAs(id, cause) ==
    RecordCreatedIdentityEntryRecorded(id, FactsFor(cause)[1], FactsFor(cause)[2], FactsFor(cause)[3])
HoldArm == \E held \in BOOLEAN : SetIdentityChangesHeldRunning(held) \/ SetIdentityChangesHeldStopped(held)

\* Every arm of one identity's removal, archive and unarchive, in a running
\* and a stopped mob.
RemoveArm(id) ==
    \/ RemoveIdentityEntryRetiring(id)
    \/ RemoveIdentityEntryRemoved(id)
    \/ RemoveIdentityEntryAlreadyRemoved(id)
    \/ RemoveIdentityEntryUnknown(id)
    \/ RemoveIdentityEntryInFlight(id)
    \/ RemoveIdentityEntryNameUnreadable(id)
    \/ RemoveIdentityEntryNonAuthoritative(id)
    \/ RemoveIdentityEntryChangesHeld(id)
    \/ RemoveIdentityEntryMobNotRunningStopped(id)
RemoveVerdicts(id) ==
    << ENABLED RemoveIdentityEntryRetiring(id),
       ENABLED RemoveIdentityEntryRemoved(id),
       ENABLED RemoveIdentityEntryAlreadyRemoved(id),
       ENABLED RemoveIdentityEntryUnknown(id),
       ENABLED RemoveIdentityEntryInFlight(id),
       ENABLED RemoveIdentityEntryNameUnreadable(id),
       ENABLED RemoveIdentityEntryNonAuthoritative(id),
       ENABLED RemoveIdentityEntryChangesHeld(id),
       ENABLED RemoveIdentityEntryMobNotRunningStopped(id) >>
ArchiveArm(id) ==
    \/ ArchiveIdentityEntryArchiving(id)
    \/ ArchiveIdentityEntryArchived(id)
    \/ ArchiveIdentityEntryAlreadyArchived(id)
    \/ ArchiveIdentityEntryOwnedByRoster(id)
    \/ ArchiveIdentityEntryUnknown(id)
    \/ ArchiveIdentityEntryInFlight(id)
    \/ ArchiveIdentityEntryNameUnreadable(id)
    \/ ArchiveIdentityEntryNonAuthoritative(id)
    \/ ArchiveIdentityEntryChangesHeld(id)
    \/ ArchiveIdentityEntryMobNotRunningStopped(id)
    \/ ArchiveIdentityEntryPreparationPending(id)
ArchiveVerdicts(id) ==
    << ENABLED ArchiveIdentityEntryArchiving(id),
       ENABLED ArchiveIdentityEntryArchived(id),
       ENABLED ArchiveIdentityEntryAlreadyArchived(id),
       ENABLED ArchiveIdentityEntryOwnedByRoster(id),
       ENABLED ArchiveIdentityEntryUnknown(id),
       ENABLED ArchiveIdentityEntryInFlight(id),
       ENABLED ArchiveIdentityEntryNameUnreadable(id),
       ENABLED ArchiveIdentityEntryNonAuthoritative(id),
       ENABLED ArchiveIdentityEntryChangesHeld(id),
       ENABLED ArchiveIdentityEntryMobNotRunningStopped(id),
       ENABLED ArchiveIdentityEntryPreparationPending(id) >>
UnarchiveArm(id) ==
    \/ UnarchiveIdentityEntryRestored(id)
    \/ UnarchiveIdentityEntryNotArchived(id)
    \/ UnarchiveIdentityEntryOwnedByRoster(id)
    \/ UnarchiveIdentityEntryUnknown(id)
    \/ UnarchiveIdentityEntryInFlight(id)
    \/ UnarchiveIdentityEntryNameUnreadable(id)
    \/ UnarchiveIdentityEntryNonAuthoritative(id)
    \/ UnarchiveIdentityEntryChangesHeld(id)
    \/ UnarchiveIdentityEntryMobNotRunningStopped(id)
UnarchiveVerdicts(id) ==
    << ENABLED UnarchiveIdentityEntryRestored(id),
       ENABLED UnarchiveIdentityEntryNotArchived(id),
       ENABLED UnarchiveIdentityEntryOwnedByRoster(id),
       ENABLED UnarchiveIdentityEntryUnknown(id),
       ENABLED UnarchiveIdentityEntryInFlight(id),
       ENABLED UnarchiveIdentityEntryNameUnreadable(id),
       ENABLED UnarchiveIdentityEntryNonAuthoritative(id),
       ENABLED UnarchiveIdentityEntryChangesHeld(id),
       ENABLED UnarchiveIdentityEntryMobNotRunningStopped(id) >>

\* Every arm of one roster-owned identity's unlist, in a running and a
\* stopped mob.
UnlistArm(id) ==
    \/ UnlistIdentityEntryRetiring(id)
    \/ UnlistIdentityEntryRemoved(id)
    \/ UnlistIdentityEntryDeferred(id)
    \/ UnlistIdentityEntryNotADeclaredEntry(id)
    \/ UnlistIdentityEntryNonAuthoritative(id)
    \/ UnlistIdentityEntryMobNotRunningStopped(id)
UnlistVerdicts(id) ==
    << ENABLED UnlistIdentityEntryRetiring(id),
       ENABLED UnlistIdentityEntryRemoved(id),
       ENABLED UnlistIdentityEntryDeferred(id),
       ENABLED UnlistIdentityEntryNotADeclaredEntry(id),
       ENABLED UnlistIdentityEntryNonAuthoritative(id),
       ENABLED UnlistIdentityEntryMobNotRunningStopped(id) >>

\* Every arm of the declared apply's verdict for one declaration of the
\* custody `cc` (TRUE: a store-owned identity's update) with the host's rule
\* requiring a preparation or not (`pr`), in a running and a stopped mob.
DeclareCustodyArm(id, s, cc, pr) ==
    \/ DeclareIdentityEntryKept(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntrySeatOwed(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryHostedOnFirstUse(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryAwaitHeldCreation(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryPreparationPending(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRuntimeModeRestamp(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRoleMigrate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRoleRotate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRoleMigrationHeld(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryLifecycleInFlight(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRefusedNameHeldByWorker(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRefusedHostAdopted(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRefusedNonAuthoritative(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRefusedNameOwnedByCreated(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRefusedNameTombstoned(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRefusedNameUnreadable(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryMobNotRunningStopped(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
    \/ DeclareIdentityEntryRecordOnly(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0)
DeclareCustodyVerdicts(id, s, cc, pr) ==
    << ENABLED DeclareIdentityEntryKept(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntrySeatOwed(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryHostedOnFirstUse(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryAwaitHeldCreation(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryPreparationPending(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRuntimeModeRestamp(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRoleMigrate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRoleRotate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRoleMigrationHeld(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryLifecycleInFlight(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRefusedNameHeldByWorker(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRefusedHostAdopted(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRefusedNonAuthoritative(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRefusedNameOwnedByCreated(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRefusedNameTombstoned(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRefusedNameUnreadable(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryMobNotRunningStopped(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0),
       ENABLED DeclareIdentityEntryRecordOnly(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], cc, pr, FALSE, FALSE, 0) >>
\* The arms a declaration answers, by their index in the tuple above.
DeclareAnswers(id, s, cc, pr) == {i \in 1..18 : DeclareCustodyVerdicts(id, s, cc, pr)[i]}

\* Member 2's ladder under created custody, its seat and its membership
\* commit: generation 0 on runtime 2 (fence 2), or generation 1 on runtime 3
\* (fence 3) once its first incarnation ended, on session 3.
CIncarnations == {<<Rid2, 2, 0>>, <<Rid3, 3, 1>>}
COpenAt(inc, ec, cc) ==
    BeginSpawnExecFresh(Id2, inc[1], inc[2], inc[3], AuditDigest, FALSE, AuditMode, Some(Sess3), None, None,
                        FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE,
                        None, None, None, None, FALSE, FALSE, ec, cc)
CSeatAt(inc, archived) ==
    CommitDormantSeatRunning(Id2, inc[1], inc[2], inc[3], FALSE, AuditMode, Sess3, None, archived, FALSE, FALSE, FALSE, FALSE)
CCommitAt(inc) ==
    CommitSpawnMembershipFresh(Id2, inc[1], inc[2], inc[3], AuditDigest, FALSE, AuditMode, Some(Sess3), None, None,
                               None, None, None, None, FALSE)
CRetireAt(inc) ==
    \/ RetireRunningReleasingArchiveOnly(AuditMob, inc[1], Id2, inc[3], Some(Sess3), Some(Sess3))
    \/ RetireRunningPreservingBindingArchiveOnly(AuditMob, inc[1], Id2, inc[3], None, Some(Sess3))
CArchiveAt(inc) == ObserveMemberRetirementArchivedLive(Id2, inc[1], inc[2], inc[3], Some(Sess3), AuditDisposal, FALSE)
\* Member 2's retirement by its archive only (the archive run retires it for
\* nothing else). Named wrappers of one arm each: TLC's coverage names an
\* action whose body is a conjunction after its operator, and the runner's
\* closed alias table counts each wrapper's coverage for the arm it wraps.
ArchiveRetireReleasing(inc) ==
    /\ Id2 \in identity_entry_archiving
    /\ RetireRunningReleasingArchiveOnly(AuditMob, inc[1], Id2, inc[3], Some(Sess3), Some(Sess3))
ArchiveRetirePreserving(inc) ==
    /\ Id2 \in identity_entry_archiving
    /\ RetireRunningPreservingBindingArchiveOnly(AuditMob, inc[1], Id2, inc[3], None, Some(Sess3))
\* Member 1's membership commit of its first incarnation (the apply run's
\* AFirst, its ladder AOpen holding entry custody).
ACommit ==
    CommitSpawnMembershipFresh(Id1, Rid1, 1, 0, AuditDigest, FALSE, AuditMode, Some(Sess1), None, None,
                               None, None, None, None, FALSE)
\* Member 1's archive terminal of its first incarnation in a stopped mob.
AArchiveStopped == ObserveMemberRetirementArchivedLiveStopped(Id1, Rid1, 1, 0, Some(Sess1), AuditDisposal, FALSE)

(***************************************************************************)
(* The commit run.                                                         *)
(***************************************************************************)

\* Member 1 recovered and bound as a worker (no entry row), member 2's name
\* reserved by its row or not; the record of either name from every
\* request's facts, the host's changes hold, and member 2's ladder under
\* created custody, its dormant seat (an entry owed its seat) or its
\* membership commit (one held for its creation, which the held protocol
\* publishes), in a running and a stopped mob. A change's admission is the
\* admit run's: a pure verdict, which changes nothing, so it adds no state
\* here, only 288 self-loops per state.
CommitNext ==
    \/ C1Recover
    \/ C1Bind
    \/ \E presence \in {"Tombstone", "Unreadable"} : RecoverArm(Id2, presence, None)
    \/ ASeed
    \/ CStop
    \/ HoldArm
    \/ \E id \in CreatedIds, f \in Facts : RecordArm(id, f)
    \/ CRAuthorize(Id2)
    \/ COpenAt(<<Rid2, 2, 0>>, FALSE, TRUE)
    \/ CSeatAt(<<Rid2, 2, 0>>, FALSE)
    \/ CCommitAt(<<Rid2, 2, 0>>)
    \/ CRAbort(Id2)
    \/ SealBootMaterializationPlan

CommitSpec == Init /\ [][CommitNext]_vars

\* The admit run: the admission of every change from every fact, over the
\* only state it reads (the build seed's composition authority, the host's
\* changes hold, a running and a stopped mob). It changes nothing.
AdmitNext ==
    \/ ASeed
    \/ HoldArm
    \/ CStop
    \/ AdmitAny

AdmitSpec == Init /\ [][AdmitNext]_vars

AuditAdmitExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A r \in Receipts, c \in ConsentKinds, pb \in BOOLEAN, pa \in BOOLEAN, ho \in BOOLEAN,
           gc \in BOOLEAN, sc \in BOOLEAN :
            Cardinality({i \in 1..11 : AdmitVerdicts(r, c, pb, pa, ho, gc, sc)[i]}) = 1
AuditRecordExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in CreatedIds, f \in Facts :
            Cardinality({i \in 1..8 : RecordVerdicts(id, f)[i]}) = 1

\* A new store-owned entry takes only a free name: no entry of either
\* custody, no member, no spawn under way, no reservation, and no unlisted
\* roster-owned identity's archived session record. Keyed on the record's
\* own step, so a record that replaces an entry is checked too.
AuditRecordTakesFreeNames ==
    [][\A id \in CreatedIds, f \in Facts :
         RecordCreatedIdentityEntryRecorded(id, f[1], f[2], f[3])
           => (/\ Origin(id) = None
               /\ ~Reserved(id)
               /\ ~Seated(id)
               /\ id \notin DOMAIN spawn_exec_phase
               /\ id \notin DOMAIN identity_entry_archived_sessions)
      ]_vars

\* A new entry's cause is the declared apply's precedence over its
\* request's facts.
AuditRecordCauseFollowsFacts ==
    [][\A id \in CreatedIds, f \in Facts :
         RecordCreatedIdentityEntryRecorded(id, f[1], f[2], f[3]) => UnseatedNext(id) = Some(ExpectedCause(f))
      ]_vars

\* An entry held for its creation is never seated Dormant: only the held
\* protocol's membership commit creates it.
AuditHeldNeverSeatedDormant ==
    [][Unseated(Id2) = Some("HeldCreation") => ~CSeatAt(<<Rid2, 2, 0>>, FALSE)]_vars

\* Witnesses of the commit run.
AuditWitnessCommitRecordedPending ==
    /\ RecordCreatedIdentityEntryRecorded(Id2, TRUE, "Placed", TRUE)
    /\ UnseatedNext(Id2) = Some("PreparationPending")
AuditWitnessCommitRecordedHosted ==
    /\ RecordCreatedIdentityEntryRecorded(Id2, FALSE, "External", TRUE)
    /\ UnseatedNext(Id2) = Some("HostedOnFirstUse")
AuditWitnessCommitRecordedHeld ==
    /\ RecordCreatedIdentityEntryRecorded(Id2, FALSE, "Local", TRUE)
    /\ UnseatedNext(Id2) = Some("HeldCreation")
AuditWitnessCommitStaleSpec ==
    AdmitIdentityChangeStaleSpec("Absent", "Policy", FALSE, FALSE, FALSE, TRUE, FALSE)
AuditWitnessCommitHeldRefused ==
    /\ identity_changes_held = TRUE
    /\ RecordCreatedIdentityEntryChangesHeld(Id2, FALSE, "Local", FALSE)
AuditWitnessCommitWorkerRefused ==
    /\ Seated(Id1)
    /\ RecordCreatedIdentityEntryNameHeldByWorker(Id1, FALSE, "Local", FALSE)
AuditWitnessCommitCreatedSeat ==
    /\ Id2 \in spawn_exec_created_custody
    /\ CSeatAt(<<Rid2, 2, 0>>, FALSE)
    /\ OriginNext(Id2) = Some("Created")
    /\ Id2 \in DOMAIN identity_to_runtime'
AuditWitnessCommitHeldCreated ==
    /\ Unseated(Id2) = Some("HeldCreation")
    /\ CCommitAt(<<Rid2, 2, 0>>)
    /\ OriginNext(Id2) = Some("Created")
    /\ UnseatedNext(Id2) = None

NotAuditWitnessCommitRecordedPending == [][~AuditWitnessCommitRecordedPending]_vars
NotAuditWitnessCommitRecordedHosted == [][~AuditWitnessCommitRecordedHosted]_vars
NotAuditWitnessCommitRecordedHeld == [][~AuditWitnessCommitRecordedHeld]_vars
NotAuditWitnessCommitStaleSpec == [][~AuditWitnessCommitStaleSpec]_vars
NotAuditWitnessCommitHeldRefused == [][~AuditWitnessCommitHeldRefused]_vars
NotAuditWitnessCommitWorkerRefused == [][~AuditWitnessCommitWorkerRefused]_vars
NotAuditWitnessCommitCreatedSeat == [][~AuditWitnessCommitCreatedSeat]_vars
NotAuditWitnessCommitHeldCreated == [][~AuditWitnessCommitHeldCreated]_vars

(***************************************************************************)
(* The update run.                                                         *)
(***************************************************************************)

\* Member 1 a roster-owned identity and member 2 a store-owned one, each
\* owed its seat and seated on its first incarnation under its custody (the
\* same profile and runtime mode); their declarations of the same change
\* (kept, another runtime mode, another profile with the migration from the
\* member's own declared or not), member 1's by its roster and member 2's
\* by its update, before and after their seats.
UpdateShapes ==
    {ApplyShape,
     <<AuditProfile, AuditModeAlt, "Local", FALSE, None, FALSE, FALSE, None, "None">>,
     <<AuditProfile2, AuditMode, "Local", FALSE, Some(AuditProfile), FALSE, FALSE, None, "None">>,
     <<AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None">>}

UpdateNext ==
    \/ DeclareCustodyArm(Id1, ApplyShape, FALSE, FALSE)
    \/ AAuthorize
    \/ AOpen(AFirst)
    \/ ASeat(AFirst)
    \/ RecordAs(Id2, "SeatOwed")
    \/ CRAuthorize(Id2)
    \/ COpenAt(<<Rid2, 2, 0>>, FALSE, TRUE)
    \/ CSeatAt(<<Rid2, 2, 0>>, FALSE)
    \/ \E s \in UpdateShapes : DeclareCustodyArm(Id1, s, FALSE, FALSE) \/ DeclareCustodyArm(Id2, s, TRUE, FALSE)

UpdateSpec == Init /\ [][UpdateNext]_vars

\* Exactly one arm answers every update of the store-owned identity.
AuditUpdateExactlyOne ==
    Created(Id2) => \A s \in UpdateShapes : Cardinality(DeclareAnswers(Id2, s, TRUE, FALSE)) = 1

\* A store-owned update and a roster's change of the same spec answer the
\* same verdict whenever both identities' facts are the same: both entries,
\* both seated (under the same profile and runtime mode) or both owed their
\* seats, neither with a role hold or a spawn ladder the other lacks.
UpdateSameFacts ==
    /\ Origin(Id1) = Some("Declared")
    /\ Origin(Id2) = Some("Created")
    /\ Seated(Id1) = Seated(Id2)
    /\ Unseated(Id1) = Unseated(Id2)
    /\ Profile(Id1) = Profile(Id2)
    /\ (IF Id1 \in DOMAIN member_runtime_modes THEN Some(member_runtime_modes[Id1]) ELSE None)
         = (IF Id2 \in DOMAIN member_runtime_modes THEN Some(member_runtime_modes[Id2]) ELSE None)
    /\ Cause(Id1) = Cause(Id2)
    /\ (Id1 \in DOMAIN spawn_exec_phase) = (Id2 \in DOMAIN spawn_exec_phase)
AuditUpdateParity ==
    UpdateSameFacts =>
        \A s \in UpdateShapes : DeclareAnswers(Id1, s, FALSE, FALSE) = DeclareAnswers(Id2, s, TRUE, FALSE)

\* Witnesses of the update run: the store-owned update's migration and
\* restamp verdicts, and its verdict for an identity owed its seat.
AuditWitnessUpdateMigrates ==
    DeclareIdentityEntryRoleMigrate(Id2, AuditProfile2, AuditMode, "Local", FALSE, Some(AuditProfile), FALSE, FALSE, None, "None", TRUE, FALSE, FALSE, FALSE, 0)
AuditWitnessUpdateRestamps ==
    DeclareIdentityEntryRuntimeModeRestamp(Id2, AuditProfile, AuditModeAlt, "Local", FALSE, None, FALSE, FALSE, None, "None", TRUE, FALSE, FALSE, FALSE, 0)
AuditWitnessUpdateOfUnseated ==
    /\ ~Seated(Id2)
    /\ DeclareIdentityEntrySeatOwed(Id2, AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None", TRUE, FALSE, FALSE, FALSE, 0)

NotAuditWitnessUpdateMigrates == [][~AuditWitnessUpdateMigrates]_vars
NotAuditWitnessUpdateRestamps == [][~AuditWitnessUpdateRestamps]_vars
NotAuditWitnessUpdateOfUnseated == [][~AuditWitnessUpdateOfUnseated]_vars

(***************************************************************************)
(* The removal run.                                                        *)
(***************************************************************************)

\* The mob's destroy of member 1's first incarnation, seated Dormant: its
\* archive-only retirement admitted, its session ingress detach closed and
\* its archive.
ADestroy ==
    \/ AuditQuiesceDestroy
    \/ AdmitDestroyCleanup
    \/ AdmitDestroyMemberRetireArchiveOnlyRunning(AuditMob, Id1, Rid1, 1, 0, Some(Sess1))
    \/ ADetach(Rid1)
    \/ ObserveDestroyMemberRetirementArchivedLiveRunning(Id1, Rid1, 1, 0, Some(Sess1), AuditDisposal)

\* Member 1 a roster-owned identity declared and seated on its first
\* incarnation, its removal retiring its member to the archive terminal in a
\* running or a stopped mob, or the mob's destroy ending it with its mark;
\* member 2 a store-owned identity recorded with every new entry's cause,
\* seated under created custody and retired to its terminal, or archived,
\* or its name reserved as unreadable; the removal of either, the
\* declarations of member 1 after its removal, the host's changes hold and
\* the build seed's composition authority, in a running and a stopped mob.
RemovalNext ==
    \/ ASeed
    \/ HoldArm
    \/ CStop
    \/ RecoverArm(Id2, "Unreadable", None)
    \/ DeclareCustodyArm(Id1, ApplyShape, FALSE, FALSE)
    \/ AAuthorize
    \/ AOpen(AFirst)
    \/ ASeat(AFirst)
    \/ ARetire(AFirst)
    \/ AArchive(AFirst)
    \/ AArchiveStopped
    \/ ADestroy
    \/ \E cause \in {"SeatOwed", "HostedOnFirstUse", "HeldCreation", "PreparationPending"} : RecordAs(Id2, cause)
    \/ CRAuthorize(Id2)
    \/ COpenAt(<<Rid2, 2, 0>>, FALSE, TRUE)
    \/ CSeatAt(<<Rid2, 2, 0>>, FALSE)
    \/ CRetireAt(<<Rid2, 2, 0>>)
    \/ CArchiveAt(<<Rid2, 2, 0>>)
    \/ ArchiveIdentityEntryArchived(Id2)
    \/ \E id \in CreatedIds : RemoveArm(id)

RemovalSpec == Init /\ [][RemovalNext]_vars

AuditRemovalExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in CreatedIds : Cardinality({i \in 1..9 : RemoveVerdicts(id)[i]}) = 1

\* The end of an entry its removal marked, and the removal of an unseated
\* entry, tombstone its name in the same step; the mob's destroy, which
\* ends every entry, is the one end that does not (the store keeps the
\* tombstone row the removal's start wrote).
AuditRemovalTombstones ==
    [][\A id \in CreatedIds :
         ((id \in identity_entry_removing \/ RemoveIdentityEntryRemoved(id))
            /\ destroy_admitted = FALSE
            /\ id \in DOMAIN identity_entry_origins /\ id \notin DOMAIN identity_entry_origins')
           => id \in identity_entry_tombstones'
      ]_vars

\* Witnesses of the removal run.
AuditWitnessRemovalRetiring ==
    /\ Seated(Id1)
    /\ RemoveIdentityEntryRetiring(Id1)
    /\ Id1 \in identity_entry_removing'
AuditWitnessRemovalEndsAtTerminal ==
    /\ Id1 \in identity_entry_removing
    /\ AArchive(AFirst)
    /\ Id1 \in identity_entry_tombstones'
    /\ Id1 \notin DOMAIN identity_entry_origins'
AuditWitnessRemovalEndsAtStoppedTerminal ==
    /\ Id1 \in identity_entry_removing
    /\ AArchiveStopped
    /\ Id1 \in identity_entry_tombstones'
    /\ Id1 \notin identity_entry_removing'
AuditWitnessRemovalStoreOwnedEndsAtTerminal ==
    /\ Id2 \in identity_entry_removing
    /\ CArchiveAt(<<Rid2, 2, 0>>)
    /\ Id2 \in identity_entry_tombstones'
    /\ Id2 \notin DOMAIN identity_entry_origins'
AuditWitnessRemovalDestroyClearsMark ==
    /\ Id1 \in identity_entry_removing
    /\ ObserveDestroyMemberRetirementArchivedLiveRunning(Id1, Rid1, 1, 0, Some(Sess1), AuditDisposal)
    /\ Id1 \notin identity_entry_removing'
    /\ Id1 \notin DOMAIN identity_entry_origins'
AuditWitnessRemovalAtOnce ==
    /\ Unseated(Id2) = Some("PreparationPending")
    /\ RemoveIdentityEntryRemoved(Id2)
    /\ Id2 \in identity_entry_tombstones'
AuditWitnessRemovalOfArchived ==
    /\ Unseated(Id2) = Some("Archived")
    /\ RemoveIdentityEntryRemoved(Id2)
AuditWitnessRemovalAlreadyRemoved ==
    /\ Id2 \in identity_entry_tombstones
    /\ RemoveIdentityEntryAlreadyRemoved(Id2)
AuditWitnessRemovalRefusesDeclaration ==
    /\ Id1 \in identity_entry_tombstones
    /\ DeclareIdentityEntryRefusedNameTombstoned(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None", FALSE, FALSE, FALSE, FALSE, 0)

NotAuditWitnessRemovalRetiring == [][~AuditWitnessRemovalRetiring]_vars
NotAuditWitnessRemovalEndsAtTerminal == [][~AuditWitnessRemovalEndsAtTerminal]_vars
NotAuditWitnessRemovalEndsAtStoppedTerminal == [][~AuditWitnessRemovalEndsAtStoppedTerminal]_vars
NotAuditWitnessRemovalStoreOwnedEndsAtTerminal == [][~AuditWitnessRemovalStoreOwnedEndsAtTerminal]_vars
NotAuditWitnessRemovalDestroyClearsMark == [][~AuditWitnessRemovalDestroyClearsMark]_vars
NotAuditWitnessRemovalAtOnce == [][~AuditWitnessRemovalAtOnce]_vars
NotAuditWitnessRemovalOfArchived == [][~AuditWitnessRemovalOfArchived]_vars
NotAuditWitnessRemovalAlreadyRemoved == [][~AuditWitnessRemovalAlreadyRemoved]_vars
NotAuditWitnessRemovalRefusesDeclaration == [][~AuditWitnessRemovalRefusesDeclaration]_vars

(***************************************************************************)
(* The archive run.                                                        *)
(***************************************************************************)

\* Member 2 a store-owned identity recorded owed its seat or awaiting its
\* host's preparation, seated under created custody, archived (its member
\* retired to the archive terminal; only an archive retires it here),
\* unarchived and seated again on its archived session, its update at any
\* point, its removal; a spawn ladder for it under created custody at any
\* point, committed or aborted; member 1 a roster-owned identity declared
\* and never seated, whose archive and unarchive are its roster's, or its
\* name reserved as unreadable; the host's changes hold and the build
\* seed's composition authority, in a running and a stopped mob.
ArchiveNext ==
    \/ ASeed
    \/ HoldArm
    \/ CStop
    \/ RecoverArm(Id1, "Unreadable", None)
    \/ DeclareCustodyArm(Id1, ApplyShape, FALSE, FALSE)
    \/ \E cause \in {"SeatOwed", "PreparationPending"} : RecordAs(Id2, cause)
    \/ CRAuthorize(Id2)
    \/ \E inc \in CIncarnations :
         \/ COpenAt(inc, FALSE, TRUE)
         \/ CSeatAt(inc, inc[3] = 1)
         \/ CCommitAt(inc)
         \/ ArchiveRetireReleasing(inc)
         \/ ArchiveRetirePreserving(inc)
         \/ CArchiveAt(inc)
    \/ CRAbort(Id2)
    \/ DeclareCustodyArm(Id2, ApplyShape, TRUE, FALSE)
    \/ \E id \in CreatedIds : ArchiveArm(id) \/ UnarchiveArm(id)
    \/ RemoveArm(Id2)

ArchiveSpec == Init /\ [][ArchiveNext]_vars

AuditArchiveExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        /\ \A id \in CreatedIds :
               /\ Cardinality({i \in 1..11 : ArchiveVerdicts(id)[i]}) = 1
               /\ Cardinality({i \in 1..9 : UnarchiveVerdicts(id)[i]}) = 1
        /\ Created(Id2) => Cardinality(DeclareAnswers(Id2, ApplyShape, TRUE, FALSE)) = 1

\* An archived identity keeps its name and its entry until its unarchive or
\* its removal: nothing else unseats its cause, and no member is seated
\* under it. A step leaves it archived under its origin, owes it its seat
\* again on its record in a running, authoritative mob with no changes hold
\* (its unarchive's change, under its unarchive's facts), or ends its entry
\* with its name tombstoned (its removal's). No disjunct names an arm: an
\* arm's action holds for any step with its successor, so it would pass
\* another input's step that changed as it does. Which input took the step
\* is the input's own property's to pin (AuditStoreUpdateKeepsStanding for
\* the update).
AuditArchivedNeverSeated ==
    [][\A id \in CreatedIds : Unseated(id) = Some("Archived") => ~(id \in DOMAIN identity_to_runtime')]_vars
AuditArchivedKeepsItsName ==
    [][\A id \in CreatedIds :
         Unseated(id) = Some("Archived")
           => \/ UnseatedNext(id) = Some("Archived") /\ OriginNext(id) = Origin(id)
              \/ /\ UnseatedNext(id) = Some("SeatOwed")
                 /\ OriginNext(id) = Origin(id)
                 /\ RecordNext(id) = Record(id)
                 /\ phase = "Running"
                 /\ composition_authority = "Authoritative"
                 /\ identity_changes_held = FALSE
              \/ /\ OriginNext(id) = None
                 /\ id \in identity_entry_tombstones'
      ]_vars
\* A store-owned update of an identity archived or awaiting its host's
\* preparation changes its row only (RecordOnly), or is refused: its entry
\* keeps its cause and its origin.
AuditStoreUpdateKeepsStanding ==
    [][(/\ Unseated(Id2) \in {Some("Archived"), Some("PreparationPending")}
        /\ DeclareCustodyArm(Id2, ApplyShape, TRUE, FALSE))
         => (UnseatedNext(Id2) = Unseated(Id2) /\ OriginNext(Id2) = Origin(Id2))
      ]_vars
\* An identity awaiting its host's preparation is never archived: its
\* preparation is settled first, or the identity removed.
AuditPendingNeverArchived ==
    [][\A id \in CreatedIds : Unseated(id) = Some("PreparationPending") => UnseatedNext(id) # Some("Archived")]_vars

\* Witnesses of the archive run.
AuditWitnessArchiveRetires ==
    /\ Seated(Id2)
    /\ ArchiveIdentityEntryArchiving(Id2)
    /\ Id2 \in identity_entry_archiving'
AuditWitnessArchiveAtTerminal ==
    /\ Id2 \in identity_entry_archiving
    /\ CArchiveAt(<<Rid2, 2, 0>>)
    /\ UnseatedNext(Id2) = Some("Archived")
    /\ RecordNext(Id2) = Some(Sess3)
AuditWitnessArchiveRestored ==
    /\ UnarchiveIdentityEntryRestored(Id2)
    /\ UnseatedNext(Id2) = Some("SeatOwed")
AuditWitnessArchiveReseated ==
    /\ Record(Id2) = Some(Sess3)
    /\ CSeatAt(<<Rid3, 3, 1>>, TRUE)
    /\ BindingNext(Id2) = Some(Sess3)
AuditWitnessArchiveOwnedByRoster == ArchiveIdentityEntryOwnedByRoster(Id1)
AuditWitnessArchivePendingRefused ==
    /\ Unseated(Id2) = Some("PreparationPending")
    /\ ArchiveIdentityEntryPreparationPending(Id2)
    /\ UnseatedNext(Id2) = Some("PreparationPending")
AuditWitnessArchiveUpdateRecordOnly ==
    /\ Unseated(Id2) = Some("Archived")
    /\ DeclareIdentityEntryRecordOnly(Id2, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None", TRUE, FALSE, FALSE, FALSE, 0)
AuditWitnessArchiveRemovalWaits ==
    /\ Id2 \in identity_entry_archiving
    /\ RemoveIdentityEntryInFlight(Id2)

NotAuditWitnessArchiveRetires == [][~AuditWitnessArchiveRetires]_vars
NotAuditWitnessArchiveAtTerminal == [][~AuditWitnessArchiveAtTerminal]_vars
NotAuditWitnessArchiveRestored == [][~AuditWitnessArchiveRestored]_vars
NotAuditWitnessArchiveReseated == [][~AuditWitnessArchiveReseated]_vars
NotAuditWitnessArchiveOwnedByRoster == [][~AuditWitnessArchiveOwnedByRoster]_vars
NotAuditWitnessArchivePendingRefused == [][~AuditWitnessArchivePendingRefused]_vars
NotAuditWitnessArchiveUpdateRecordOnly == [][~AuditWitnessArchiveUpdateRecordOnly]_vars
NotAuditWitnessArchiveRemovalWaits == [][~AuditWitnessArchiveRemovalWaits]_vars

(***************************************************************************)
(* The preparation run.                                                    *)
(***************************************************************************)

\* Every arm of a held construction's start, of a settle and of an
\* attempt's end, in a running and a stopped mob.
Attempts == {1, 2}
EndKinds == {"Aborted", "Expired", "Refused", "PublicationFailed"}
BeginArm(id, a, k) ==
    \/ BeginPreparationConstructionHeld(id, a, k)
    \/ BeginPreparationConstructionNotPending(id, a, k)
    \/ BeginPreparationConstructionNotConstructed(id, a, k)
    \/ BeginPreparationConstructionConstructionOpen(id, a, k)
    \/ BeginPreparationConstructionNonAuthoritative(id, a, k)
    \/ BeginPreparationConstructionMobNotRunningStopped(id, a, k)
BeginVerdicts(id, a, k) ==
    << ENABLED BeginPreparationConstructionHeld(id, a, k),
       ENABLED BeginPreparationConstructionNotPending(id, a, k),
       ENABLED BeginPreparationConstructionNotConstructed(id, a, k),
       ENABLED BeginPreparationConstructionConstructionOpen(id, a, k),
       ENABLED BeginPreparationConstructionNonAuthoritative(id, a, k),
       ENABLED BeginPreparationConstructionMobNotRunningStopped(id, a, k) >>
SettleArm(id, a, k, b, m) ==
    \/ SettleHostPreparationSeat(id, a, k, b, m)
    \/ SettleHostPreparationActivationAccepted(id, a, k, b, m)
    \/ SettleHostPreparationNotPending(id, a, k, b, m)
    \/ SettleHostPreparationConstructionRequired(id, a, k, b, m)
    \/ SettleHostPreparationConstructionOpen(id, a, k, b, m)
    \/ SettleHostPreparationStaleAttempt(id, a, k, b, m)
    \/ SettleHostPreparationExpired(id, a, k, b, m)
    \/ SettleHostPreparationMismatch(id, a, k, b, m)
    \/ SettleHostPreparationActivationPending(id, a, k, b, m)
    \/ SettleHostPreparationNonAuthoritative(id, a, k, b, m)
    \/ SettleHostPreparationMobNotRunningStopped(id, a, k, b, m)
SettleVerdicts(id, a, k, b, m) ==
    << ENABLED SettleHostPreparationSeat(id, a, k, b, m),
       ENABLED SettleHostPreparationActivationAccepted(id, a, k, b, m),
       ENABLED SettleHostPreparationNotPending(id, a, k, b, m),
       ENABLED SettleHostPreparationConstructionRequired(id, a, k, b, m),
       ENABLED SettleHostPreparationConstructionOpen(id, a, k, b, m),
       ENABLED SettleHostPreparationStaleAttempt(id, a, k, b, m),
       ENABLED SettleHostPreparationExpired(id, a, k, b, m),
       ENABLED SettleHostPreparationMismatch(id, a, k, b, m),
       ENABLED SettleHostPreparationActivationPending(id, a, k, b, m),
       ENABLED SettleHostPreparationNonAuthoritative(id, a, k, b, m),
       ENABLED SettleHostPreparationMobNotRunningStopped(id, a, k, b, m) >>
EndArm(id, a, e) ==
    \/ EndPreparationConstructionEndedRunning(id, a, e)
    \/ EndPreparationConstructionEndedStopped(id, a, e)
    \/ EndPreparationConstructionNotOpenRunning(id, a, e)
    \/ EndPreparationConstructionNotOpenStopped(id, a, e)
    \/ EndPreparationConstructionActivationPendingRunning(id, a, e)
    \/ EndPreparationConstructionActivationPendingStopped(id, a, e)
EndVerdicts(id, a, e) ==
    << ENABLED EndPreparationConstructionEndedRunning(id, a, e),
       ENABLED EndPreparationConstructionEndedStopped(id, a, e),
       ENABLED EndPreparationConstructionNotOpenRunning(id, a, e),
       ENABLED EndPreparationConstructionNotOpenStopped(id, a, e),
       ENABLED EndPreparationConstructionActivationPendingRunning(id, a, e),
       ENABLED EndPreparationConstructionActivationPendingStopped(id, a, e) >>
SettleAttempts == {None, Some(1), Some(2)}
Held(id) == IF id \in DOMAIN identity_entry_constructions_held THEN Some(identity_entry_constructions_held[id]) ELSE None
Activating(id) == IF id \in DOMAIN identity_entry_constructions_activating THEN Some(identity_entry_constructions_activating[id]) ELSE None

\* Member 1 a roster-owned identity whose host's rule requires its
\* preparation (its declaration requires it or no longer does), member 2 a
\* store-owned one recorded pending its preparation; the start, the settle
\* and the end of their held constructions with every fact, each one's
\* spawn ladder under its custody (member 1's first incarnation, member
\* 2's first), its dormant seat and its membership commit, its archive, its
\* removal, and member 2's update; in an authoritative composition (the
\* initial one: no step here needs a candidate one, whose refusals the
\* verdict run answers, and seeding one would double every state), in a
\* running and a stopped mob.
PrepNext ==
    \/ \E pr \in BOOLEAN : DeclareCustodyArm(Id1, ApplyShape, FALSE, pr)
    \/ RecordAs(Id2, "PreparationPending")
    \* The attempts' arms that change state, from every fact. Their refusals
    \* change nothing (each is a self-loop here): the verdict run fires them
    \* and checks that exactly one arm answers each.
    \/ \E id \in CreatedIds, a \in Attempts, k \in BOOLEAN : BeginPreparationConstructionHeld(id, a, k)
    \/ \E id \in CreatedIds, a \in SettleAttempts, k \in BOOLEAN, b \in BOOLEAN, m \in BOOLEAN :
         \/ SettleHostPreparationSeat(id, a, k, b, m)
         \/ SettleHostPreparationActivationAccepted(id, a, k, b, m)
    \/ \E id \in CreatedIds, a \in Attempts, e \in EndKinds :
         \/ EndPreparationConstructionEndedRunning(id, a, e)
         \/ EndPreparationConstructionEndedStopped(id, a, e)
    \/ AAuthorize
    \/ AOpen(AFirst)
    \/ ASeat(AFirst)
    \/ ACommit
    \/ AAbort
    \/ CRAuthorize(Id2)
    \/ COpenAt(<<Rid2, 2, 0>>, FALSE, TRUE)
    \/ CSeatAt(<<Rid2, 2, 0>>, FALSE)
    \/ CCommitAt(<<Rid2, 2, 0>>)
    \/ CRAbort(Id2)
    \/ DeclareCustodyArm(Id2, ApplyShape, TRUE, FALSE)
    \/ \E id \in CreatedIds : ArchiveArm(id) \/ RemoveArm(id)
    \/ CStop

PrepSpec == Init /\ [][PrepNext]_vars

\* The verdict run of the preparation: every arm of an attempt's start,
\* settle and end, from every fact, over the facts their verdicts read (each
\* identity's preparation, its held and its activating attempt, the build
\* seed's composition authority, a running and a stopped mob), with the
\* preparation run's declarations and records and no ladder.
PrepVerdictsNext ==
    \/ ASeed
    \/ \E pr \in BOOLEAN : DeclareCustodyArm(Id1, ApplyShape, FALSE, pr)
    \/ RecordAs(Id2, "PreparationPending")
    \/ \E id \in CreatedIds, a \in Attempts, k \in BOOLEAN : BeginArm(id, a, k)
    \/ \E id \in CreatedIds, a \in SettleAttempts, k \in BOOLEAN, b \in BOOLEAN, m \in BOOLEAN : SettleArm(id, a, k, b, m)
    \/ \E id \in CreatedIds, a \in Attempts, e \in EndKinds : EndArm(id, a, e)
    \/ CStop

PrepVerdictsSpec == Init /\ [][PrepVerdictsNext]_vars

AuditPrepExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in CreatedIds :
            /\ \A a \in Attempts, k \in BOOLEAN : Cardinality({i \in 1..6 : BeginVerdicts(id, a, k)[i]}) = 1
            /\ \A a \in SettleAttempts, k \in BOOLEAN, b \in BOOLEAN, m \in BOOLEAN :
                 Cardinality({i \in 1..11 : SettleVerdicts(id, a, k, b, m)[i]}) = 1
            /\ \A a \in Attempts, e \in EndKinds : Cardinality({i \in 1..6 : EndVerdicts(id, a, e)[i]}) = 1
            /\ Cardinality({i \in 1..11 : ArchiveVerdicts(id)[i]}) = 1

\* Exactly one arm answers every archive in the preparation run's states,
\* whose attempts open ladders and seat members (the verdict run has
\* neither): the archive's verdict reads both.
AuditPrepArchiveExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in CreatedIds : Cardinality({i \in 1..11 : ArchiveVerdicts(id)[i]}) = 1

\* An attempt is open only for an entry awaiting its preparation (the
\* recovery, which the run does not drive, runs before any attempt opens).
AuditConstructionAwaitsPreparation ==
    \A id \in CreatedIds :
        (Held(id) # None \/ Activating(id) # None) => Unseated(id) = Some("PreparationPending")

\* An entry awaiting its preparation is seated only by the membership
\* commit of the activation a settle accepted.
AuditPendingSeatedOnlyByItsActivation ==
    [][\A id \in CreatedIds :
         (Unseated(id) = Some("PreparationPending") /\ ~Seated(id) /\ id \in DOMAIN identity_to_runtime')
           => Activating(id) # None
      ]_vars

\* An accepted activation is final: while it is in flight, no end of its
\* attempt but its failed publication is enabled (its abort, its expiry and
\* a refused construction answer ActivationPending), so only its
\* publication's membership commit or its failed publication ends it.
AuditAcceptedActivationIsFinal ==
    \A id \in CreatedIds, a \in Attempts, e \in EndKinds \ {"PublicationFailed"} :
        Activating(id) = Some(a)
          => /\ ~ENABLED EndPreparationConstructionEndedRunning(id, a, e)
             /\ ~ENABLED EndPreparationConstructionEndedStopped(id, a, e)

\* A `constructed` preparation is never settled without an attempt.
AuditConstructedNeverSettledWithoutAttempt ==
    [][\A id \in CreatedIds, b \in BOOLEAN, m \in BOOLEAN : ~SettleHostPreparationSeat(id, None, TRUE, b, m)]_vars

\* Witnesses of the preparation run.
AuditWitnessPrepRosterPending ==
    DeclareIdentityEntryPreparationPending(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None", FALSE, TRUE, FALSE, FALSE, 0)
AuditWitnessPrepRosterSettledByDeclaration ==
    /\ Unseated(Id1) = Some("PreparationPending")
    /\ DeclareIdentityEntrySeatOwed(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None", FALSE, FALSE, FALSE, FALSE, 0)
    /\ UnseatedNext(Id1) = Some("SeatOwed")
AuditWitnessPrepStoreSettleSeats ==
    /\ SettleHostPreparationSeat(Id2, None, FALSE, FALSE, FALSE)
    /\ UnseatedNext(Id2) = Some("SeatOwed")
AuditWitnessPrepAccepted ==
    /\ SettleHostPreparationActivationAccepted(Id2, Some(1), TRUE, TRUE, TRUE)
    /\ Id2 \in DOMAIN identity_entry_constructions_activating'
AuditWitnessPrepPublished ==
    /\ Activating(Id2) # None
    /\ CCommitAt(<<Rid2, 2, 0>>)
    /\ OriginNext(Id2) = Some("Created")
    /\ UnseatedNext(Id2) = None
AuditWitnessPrepRosterPublished ==
    /\ Activating(Id1) # None
    /\ ACommit
    /\ OriginNext(Id1) = Some("Declared")
AuditWitnessPrepAbortRefusedWhileActivating ==
    /\ Activating(Id2) = Some(1)
    /\ EndPreparationConstructionActivationPendingRunning(Id2, 1, "Aborted")
AuditWitnessPrepExpired ==
    /\ Held(Id2) = Some(1)
    /\ EndPreparationConstructionEndedRunning(Id2, 1, "Expired")
    /\ Id2 \notin DOMAIN identity_entry_constructions_held'
    /\ UnseatedNext(Id2) = Some("PreparationPending")
\* A removal ends a held attempt with the entry, in one step, and waits
\* while an accepted activation's publication is in flight.
AuditWitnessPrepRemovalEndsHeld ==
    /\ Held(Id2) # None
    /\ RemoveIdentityEntryRemoved(Id2)
    /\ Id2 \notin DOMAIN identity_entry_constructions_held'
    /\ Id2 \in identity_entry_tombstones'
AuditWitnessPrepRemovalWaits ==
    /\ Activating(Id2) # None
    /\ RemoveIdentityEntryInFlight(Id2)
\* An archive of an identity whose attempt is held is refused, the attempt
\* kept.
AuditWitnessPrepArchiveRefusedWhileHeld ==
    /\ Held(Id2) # None
    /\ ArchiveIdentityEntryPreparationPending(Id2)
    /\ Held(Id2) = (IF Id2 \in DOMAIN identity_entry_constructions_held' THEN Some(identity_entry_constructions_held'[Id2]) ELSE None)
\* A construction's own spawn opens its ladder at its publication, after
\* the settle accepted its activation (the shell opens a spawn's ladder once
\* its provision returned), under either custody.
AuditWitnessPrepLadderOpensAfterAcceptance ==
    /\ Activating(Id2) # None
    /\ COpenAt(<<Rid2, 2, 0>>, FALSE, TRUE)
AuditWitnessPrepRosterLadderOpensAfterAcceptance ==
    /\ Activating(Id1) # None
    /\ AOpen(AFirst)
\* A store-owned update of an identity awaiting its preparation, with no
\* attempt open, is record only.
AuditWitnessPrepUpdateRecordOnly ==
    /\ Unseated(Id2) = Some("PreparationPending")
    /\ Held(Id2) = None
    /\ Activating(Id2) = None
    /\ DeclareIdentityEntryRecordOnly(Id2, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None", TRUE, FALSE, FALSE, FALSE, 0)

NotAuditWitnessPrepRosterPending == [][~AuditWitnessPrepRosterPending]_vars
NotAuditWitnessPrepRosterSettledByDeclaration == [][~AuditWitnessPrepRosterSettledByDeclaration]_vars
NotAuditWitnessPrepStoreSettleSeats == [][~AuditWitnessPrepStoreSettleSeats]_vars
NotAuditWitnessPrepAccepted == [][~AuditWitnessPrepAccepted]_vars
NotAuditWitnessPrepPublished == [][~AuditWitnessPrepPublished]_vars
NotAuditWitnessPrepRosterPublished == [][~AuditWitnessPrepRosterPublished]_vars
NotAuditWitnessPrepAbortRefusedWhileActivating == [][~AuditWitnessPrepAbortRefusedWhileActivating]_vars
NotAuditWitnessPrepExpired == [][~AuditWitnessPrepExpired]_vars
NotAuditWitnessPrepRemovalEndsHeld == [][~AuditWitnessPrepRemovalEndsHeld]_vars
NotAuditWitnessPrepRemovalWaits == [][~AuditWitnessPrepRemovalWaits]_vars
NotAuditWitnessPrepArchiveRefusedWhileHeld == [][~AuditWitnessPrepArchiveRefusedWhileHeld]_vars
NotAuditWitnessPrepUpdateRecordOnly == [][~AuditWitnessPrepUpdateRecordOnly]_vars
NotAuditWitnessPrepLadderOpensAfterAcceptance == [][~AuditWitnessPrepLadderOpensAfterAcceptance]_vars
NotAuditWitnessPrepRosterLadderOpensAfterAcceptance == [][~AuditWitnessPrepRosterLadderOpensAfterAcceptance]_vars

(***************************************************************************)
(* The placement run.                                                      *)
(***************************************************************************)

\* One member host bound; member 2 a store-owned identity recorded hosted on
\* its first use (placed, or externally bound and held), or owed its seat;
\* its placed spawn ladder and its peer-only one (no session: an externally
\* bound member) with each pair of custodies, their aborts, their commits
\* and its activation.
POpenAs(ec, cc) ==
    BeginSpawnExecRemote(Id2, Rid2, 2, 0, AuditDigest, FALSE, AuditMode, None, None, Some(Host1),
                         FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE,
                         None, Some(PlacedSpawn1), Some(AuditOperation), Some(Sess3), FALSE, FALSE, ec, cc)
PeerOpenAs(ec, cc) ==
    BeginSpawnExecFreshPeerOnly(Id2, Rid2, 2, 0, AuditDigest, FALSE, AuditMode, None, None, None,
                                FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE,
                                None, None, None, None, FALSE, FALSE, ec, cc)
PeerCommit ==
    CommitSpawnMembershipFreshPeerOnly(Id2, Rid2, 2, 0, AuditDigest, FALSE, AuditMode, None, None, None,
                                       None, None, None, None, FALSE)
PlacementNext ==
    \/ POwner
    \/ PBeginHost
    \/ PCommitHost
    \/ \E f \in {FactsFor("HostedOnFirstUse"), <<FALSE, "External", TRUE>>, FactsFor("SeatOwed")} :
         RecordCreatedIdentityEntryRecorded(Id2, f[1], f[2], f[3])
    \/ PAuthorize(Id2)
    \/ \E ec \in BOOLEAN, cc \in BOOLEAN : POpenAs(ec, cc) \/ PeerOpenAs(ec, cc)
    \/ PAbort(Id2)
    \/ CRAbort(Id2)
    \/ PCommit
    \/ PeerCommit
    \/ CommitSpawnActivationFinalRunning(Id2)

PlacementSpec == Init /\ [][PlacementNext]_vars

\* The placed opener holds created custody only for a store-owned entry
\* hosted on its first use.
AuditPlacedCreatedCustodyHosted ==
    [][\A ec \in BOOLEAN : POpenAs(ec, TRUE) => (Created(Id2) /\ Unseated(Id2) = Some("HostedOnFirstUse"))]_vars

\* The placed witness: a store-owned identity hosted on its first use is
\* seated by a placed spawn holding its created custody, and stays a
\* Created entry.
AuditWitnessPlacedCreated ==
    /\ Id2 \in spawn_exec_created_custody
    /\ PCommit
    /\ OriginNext(Id2) = Some("Created")
    /\ UnseatedNext(Id2) = None
\* The peer-only witness: an externally bound store-owned identity is
\* created by a peer-only spawn holding its created custody.
AuditWitnessPeerOnlyCreated ==
    /\ Id2 \in spawn_exec_created_custody
    /\ PeerCommit
    /\ OriginNext(Id2) = Some("Created")
    /\ UnseatedNext(Id2) = None

NotAuditWitnessPlacedCreated == [][~AuditWitnessPlacedCreated]_vars
NotAuditWitnessPeerOnlyCreated == [][~AuditWitnessPeerOnlyCreated]_vars

(***************************************************************************)
(* The unlist run.                                                         *)
(***************************************************************************)

\* Member 1 a roster-owned identity declared and seated on its first
\* incarnation, its ladder at any point; member 2 a store-owned identity
\* recorded owed its seat; the one-entry unlist of either; member 1's
\* retirement to its terminal, and the record of its name once its
\* unlisted end kept its archived session record; the build seed's
\* composition authority, in a running and a stopped mob.
UnlistNext ==
    \/ ASeed
    \/ CStop
    \/ DeclareCustodyArm(Id1, ApplyShape, FALSE, FALSE)
    \/ AAuthorize
    \/ AOpen(AFirst)
    \/ ASeat(AFirst)
    \/ AAbort
    \/ ARetire(AFirst)
    \/ AArchive(AFirst)
    \/ RecordAs(Id2, "SeatOwed")
    \/ RecordArm(Id1, FactsFor("SeatOwed"))
    \/ \E id \in CreatedIds : UnlistArm(id)

UnlistSpec == Init /\ [][UnlistNext]_vars

AuditUnlistExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in CreatedIds : Cardinality({i \in 1..6 : UnlistVerdicts(id)[i]}) = 1

\* An unlist decides on its identity alone: it changes no other identity's
\* entry, and never a Created entry.
AuditUnlistDecidesItsIdentityAlone ==
    [][\A id \in CreatedIds :
         UnlistArm(id)
           => \A other \in CreatedIds :
                (other # id \/ Created(other))
                  => /\ OriginNext(other) = Origin(other)
                     /\ UnseatedNext(other) = Unseated(other)
                     /\ ((other \in identity_entry_retiring') = (other \in identity_entry_retiring))
      ]_vars

\* Witnesses of the unlist run.
AuditWitnessUnlistMarksSeated ==
    /\ Seated(Id1)
    /\ UnlistIdentityEntryRetiring(Id1)
    /\ Id1 \in identity_entry_retiring'
AuditWitnessUnlistRemovesUnseated ==
    /\ Origin(Id1) = Some("Declared")
    /\ ~Seated(Id1)
    /\ UnlistIdentityEntryRemoved(Id1)
    /\ Id1 \notin DOMAIN identity_entry_origins'
AuditWitnessUnlistDefersLadder ==
    /\ Id1 \in DOMAIN spawn_exec_phase
    /\ UnlistIdentityEntryDeferred(Id1)
AuditWitnessUnlistEndsAtTerminal ==
    /\ Id1 \in identity_entry_retiring
    /\ AArchive(AFirst)
    /\ Id1 \notin DOMAIN identity_entry_origins'
    /\ RecordNext(Id1) # None
AuditWitnessUnlistedNameRefused ==
    /\ Origin(Id1) = None
    /\ Record(Id1) # None
    /\ RecordCreatedIdentityEntryNameHeldByIdentity(Id1, FALSE, "Local", FALSE)
AuditWitnessUnlistLeavesCreated ==
    /\ Created(Id2)
    /\ UnlistIdentityEntryNotADeclaredEntry(Id2)

NotAuditWitnessUnlistMarksSeated == [][~AuditWitnessUnlistMarksSeated]_vars
NotAuditWitnessUnlistRemovesUnseated == [][~AuditWitnessUnlistRemovesUnseated]_vars
NotAuditWitnessUnlistDefersLadder == [][~AuditWitnessUnlistDefersLadder]_vars
NotAuditWitnessUnlistEndsAtTerminal == [][~AuditWitnessUnlistEndsAtTerminal]_vars
NotAuditWitnessUnlistedNameRefused == [][~AuditWitnessUnlistedNameRefused]_vars
NotAuditWitnessUnlistLeavesCreated == [][~AuditWitnessUnlistLeavesCreated]_vars

(***************************************************************************)
(* The recovered run.                                                      *)
(***************************************************************************)

\* The retirement a recovered mark owes, of member 1's first incarnation
\* (AFirst), by named wrappers of one arm each, which the runner's alias
\* table counts for the arm they wrap. Their seal conjunct is a scope
\* restriction of the run, not a guard of MobMachine: the shell retires the
\* member once the start sealed its boot plan (an order the shell keeps),
\* so the run scans no row after the retirement.
RecoveredRetireReleasing ==
    /\ boot_plan_sealed = TRUE
    /\ RetireRunningReleasingArchiveOnly(AuditMob, Rid1, Id1, 0, Some(Sess1), Some(Sess1))
RecoveredRetirePreserving ==
    /\ boot_plan_sealed = TRUE
    /\ RetireRunningPreservingBindingArchiveOnly(AuditMob, Rid1, Id1, 0, None, Some(Sess1))

\* The replay of member 1's Created entry row: the spawn row of a
\* store-owned identity's seated incarnation records its entry.
C1Created == RecoverIdentityEntrySeatedRunning(Id1, Rid1, "Created")

\* A store precondition the run keeps (a scope restriction): the store holds
\* one created entry row per identity, keyed by (mob_id, agent_identity)
\* (store/sqlite.rs:217-225), and the boot's scan pages those rows on a
\* strictly advancing cursor, refusing any other
\* (runtime/created_identity.rs:115-195), so it sends one row of an
\* identity. Each row this run scans leaves its mark (a removal or an
\* archive in flight, an archived entry, a tombstone or an unreadable
\* name), so it scans a row of member 1 only while none is set. (The scan
\* run sends further rows of one identity on purpose, for the arms that
\* answer a contradicted row.)
C1Unscanned ==
    /\ Unseated(Id1) = None
    /\ Id1 \notin identity_entry_removing
    /\ Id1 \notin identity_entry_archiving
    /\ Id1 \notin identity_entry_tombstones
    /\ Id1 \notin identity_entry_unreadable

\* The scan of member 1's one row, a removal's or an archive's: a named
\* wrapper of one arm for each arm that answers such a row, in a running
\* and a stopped mob. The runner's alias table counts the Removing,
\* Archiving and Unreadable arms in a running mob for theirs; no Present
\* row is scanned here, so the Seated arm is the scan run's.
C1OneRowRemoving ==
    /\ C1Unscanned
    /\ RecoverCreatedIdentityEntryRemovingRunning(Id1, "Tombstone", None)
C1OneRowRemovingStopped ==
    /\ C1Unscanned
    /\ RecoverCreatedIdentityEntryRemovingStopped(Id1, "Tombstone", None)
C1OneRowTombstone ==
    /\ C1Unscanned
    /\ RecoverCreatedIdentityEntryTombstoneRunning(Id1, "Tombstone", None)
C1OneRowTombstoneStopped ==
    /\ C1Unscanned
    /\ RecoverCreatedIdentityEntryTombstoneStopped(Id1, "Tombstone", None)
C1OneRowArchiving ==
    /\ C1Unscanned
    /\ RecoverCreatedIdentityEntryArchivingRunning(Id1, "Archived", None)
C1OneRowArchivingStopped ==
    /\ C1Unscanned
    /\ RecoverCreatedIdentityEntryArchivingStopped(Id1, "Archived", None)
C1OneRowArchived ==
    /\ C1Unscanned
    /\ RecoverCreatedIdentityEntryArchivedRunning(Id1, "Archived", None)
C1OneRowArchivedStopped ==
    /\ C1Unscanned
    /\ RecoverCreatedIdentityEntryArchivedStopped(Id1, "Archived", None)
C1OneRowUnreadable(presence) ==
    /\ C1Unscanned
    /\ RecoverCreatedIdentityEntryUnreadableRunning(Id1, presence, None)
C1OneRowUnreadableStopped(presence) ==
    /\ C1Unscanned
    /\ RecoverCreatedIdentityEntryUnreadableStopped(Id1, presence, None)
C1OneRow ==
    \/ C1OneRowRemoving
    \/ C1OneRowRemovingStopped
    \/ C1OneRowTombstone
    \/ C1OneRowTombstoneStopped
    \/ C1OneRowArchiving
    \/ C1OneRowArchivingStopped
    \/ C1OneRowArchived
    \/ C1OneRowArchivedStopped
    \/ \E presence \in {"Tombstone", "Archived"} :
         \/ C1OneRowUnreadable(presence)
         \/ C1OneRowUnreadableStopped(presence)

\* Member 1 recovered and bound (a seated local member, Dormant) with its
\* peer endpoint, which the seal requires of a member it does not wake, and
\* its Created entry row replayed; its one row, a removal's or an archive's,
\* scanned before the boot plan's seal, in either order with the journal
\* replay (the shell replays first; MobMachine refuses a replay that would
\* seat a member under an archived entry or a tombstoned name); then the
\* retirement the recovered mark owes, to its terminal, in a running and a
\* stopped mob.
RecoveredNext ==
    \/ C1Recover
    \/ C1Bind
    \/ WEndpoint
    \/ C1Created
    \/ C1OneRow
    \/ SealBootMaterializationPlan
    \/ CStop
    \/ RecoveredRetireReleasing
    \/ RecoveredRetirePreserving
    \/ AArchive(AFirst)
    \/ AArchiveStopped

RecoveredSpec == Init /\ [][RecoveredNext]_vars

\* Witnesses of the recovered run: a recovered removal and a recovered
\* archive carried to their terminal.
AuditWitnessRecoveredRemovalEnds ==
    /\ Id1 \in identity_entry_removing
    /\ AArchive(AFirst) \/ AArchiveStopped
    /\ Id1 \in identity_entry_tombstones'
    /\ Id1 \notin DOMAIN identity_entry_origins'
AuditWitnessRecoveredArchiveEnds ==
    /\ Id1 \in identity_entry_archiving
    /\ AArchive(AFirst) \/ AArchiveStopped
    /\ UnseatedNext(Id1) = Some("Archived")

NotAuditWitnessRecoveredRemovalEnds == [][~AuditWitnessRecoveredRemovalEnds]_vars
NotAuditWitnessRecoveredArchiveEnds == [][~AuditWitnessRecoveredArchiveEnds]_vars

(***************************************************************************)
(* The doors part (0.9.11): the doors run.                                 *)
(***************************************************************************)

\* External work never reaches an identity entry, or a name reserved
\* unreadable: the entry's host delivers to it through its own door, as
\* internal work, and an external sender is refused as an identity member
\* (ResolveSubmitWorkRejectionIdentityMember and its unreadable-name twin),
\* whatever session the work names: a stale-session answer would invite a
\* retry that can only be refused again.
\* Member 1 recovered externally addressable and bound to its session, its
\* Declared entry row replayed or not; member 2 recovered externally
\* addressable or not, bound to its session or not, its peer registered or
\* not; the scan of either member's created entry row, Present (a Created
\* entry of a seated local member, or the reservation of a name a Declared
\* entry holds) or Unreadable (the name reserved); and work of either origin
\* to either member under either runtime and fence token, naming no
\* expected session, member 1's or member 2's: the four admissions and the
\* rejections of a running mob, the stale session's among them. The work
\* arms change nothing, so the run counts the arms that answer every piece
\* of work in every state (ENABLED form).

DoorsIds == {Id1, Id2}
DoorsRids == {Rid1, Rid2}
DoorsFences == {1, 2}
DoorsSessions == {None, Some(Sess1), Some(Sess2)}
DoorsOrigins == {"External", "Internal"}
\* No guard of the work arms reads the work id or its attribution.
DoorsWork == "workid_1"
DoorsAttribution == "Conversational"

D1Recover == RecoverRosterMemberRunning(Id1, Rid1, 1, 0, AuditProfile, AuditMode, TRUE)
D1Bind == RecoverMemberSessionBindingFreshRunning(Id1, Rid1, Sess1, None)
D1Declared == RecoverIdentityEntrySeatedRunning(Id1, Rid1, "Declared")
D2Recover ==
    \E addressable \in BOOLEAN :
        RecoverRosterMemberRunning(Id2, Rid2, 2, 0, AuditProfile, AuditMode, addressable)
D2Bind == RecoverMemberSessionBindingFreshRunning(Id2, Rid2, Sess2, None)
D2Peer == RegisterMemberPeerRunning(Id2, Rid2, 0, 2, E2Inproc)
DScan == \E id \in DoorsIds, presence \in {"Present", "Unreadable"} : RecoverArm(id, presence, None)

\* Every arm that answers a piece of work in a running mob: the admissions
\* (1-4), the refusals of a member that is not there, not live, not current
\* or retiring (5-8), the identity member refusals (9-10), the
\* addressability refusals (11-12) and the stale session refusal (13).
DoorsVerdicts(id, rid, ft, es, origin) ==
    << ENABLED SubmitWorkRunningExternal(id, rid, ft, es, DoorsWork, origin, DoorsAttribution),
       ENABLED SubmitWorkRunningExternalPeerOnly(id, rid, ft, es, DoorsWork, origin, DoorsAttribution),
       ENABLED SubmitWorkRunningInternal(id, rid, ft, es, DoorsWork, origin, DoorsAttribution),
       ENABLED SubmitWorkRunningInternalPeerOnly(id, rid, ft, es, DoorsWork, origin, DoorsAttribution),
       ENABLED ResolveSubmitWorkRejectionMemberNotFound(id, rid, ft, es, origin),
       ENABLED ResolveSubmitWorkRejectionCurrentRuntimeNotLive(id, rid, ft, es, origin),
       ENABLED ResolveSubmitWorkRejectionStaleFenceToken(id, rid, ft, es, origin),
       ENABLED ResolveSubmitWorkRejectionRetiringAsMemberNotFound(id, rid, ft, es, origin),
       ENABLED ResolveSubmitWorkRejectionIdentityMember(id, rid, ft, es, origin),
       ENABLED ResolveSubmitWorkRejectionIdentityMemberUnreadableName(id, rid, ft, es, origin),
       ENABLED ResolveSubmitWorkRejectionNotExternallyAddressable(id, rid, ft, es, origin),
       ENABLED ResolveSubmitWorkRejectionPeerOnlyNotExternallyAddressable(id, rid, ft, es, origin),
       ENABLED ResolveSubmitWorkRejectionStaleSessionBinding(id, rid, ft, es, origin) >>
DoorsArm(id, rid, ft, es, origin) ==
    \/ SubmitWorkRunningExternal(id, rid, ft, es, DoorsWork, origin, DoorsAttribution)
    \/ SubmitWorkRunningExternalPeerOnly(id, rid, ft, es, DoorsWork, origin, DoorsAttribution)
    \/ SubmitWorkRunningInternal(id, rid, ft, es, DoorsWork, origin, DoorsAttribution)
    \/ SubmitWorkRunningInternalPeerOnly(id, rid, ft, es, DoorsWork, origin, DoorsAttribution)
    \/ ResolveSubmitWorkRejectionMemberNotFound(id, rid, ft, es, origin)
    \/ ResolveSubmitWorkRejectionCurrentRuntimeNotLive(id, rid, ft, es, origin)
    \/ ResolveSubmitWorkRejectionStaleFenceToken(id, rid, ft, es, origin)
    \/ ResolveSubmitWorkRejectionRetiringAsMemberNotFound(id, rid, ft, es, origin)
    \/ ResolveSubmitWorkRejectionIdentityMember(id, rid, ft, es, origin)
    \/ ResolveSubmitWorkRejectionIdentityMemberUnreadableName(id, rid, ft, es, origin)
    \/ ResolveSubmitWorkRejectionNotExternallyAddressable(id, rid, ft, es, origin)
    \/ ResolveSubmitWorkRejectionPeerOnlyNotExternallyAddressable(id, rid, ft, es, origin)
    \/ ResolveSubmitWorkRejectionStaleSessionBinding(id, rid, ft, es, origin)

DoorsNext ==
    \/ D1Recover
    \/ D1Bind
    \/ D1Declared
    \/ D2Recover
    \/ D2Bind
    \/ D2Peer
    \/ DScan
    \/ \E id \in DoorsIds, rid \in DoorsRids, ft \in DoorsFences, es \in DoorsSessions,
          origin \in DoorsOrigins :
           DoorsArm(id, rid, ft, es, origin)

DoorsSpec == Init /\ [][DoorsNext]_vars

\* An identity entry of either origin, or a name reserved unreadable.
DoorsEntry(id) == id \in DOMAIN identity_entry_origins \/ id \in identity_entry_unreadable

\* No external work is admitted to an identity entry or to a name reserved
\* unreadable, by its session or by its peer, whatever session it names.
AuditDoorsNoExternalEntryAdmission ==
    \A id \in DoorsIds, rid \in DoorsRids, ft \in DoorsFences, es \in DoorsSessions :
        DoorsEntry(id) =>
            /\ ~ENABLED SubmitWorkRunningExternal(id, rid, ft, es, DoorsWork, "External", DoorsAttribution)
            /\ ~ENABLED SubmitWorkRunningExternalPeerOnly(id, rid, ft, es, DoorsWork, "External", DoorsAttribution)

\* At most one arm answers every piece of work: no admission and no two
\* refusals overlap. External work to an identity entry, or to a name
\* reserved unreadable, is checked whatever session it names: the identity
\* member refusals and the stale session refusal never both answer it.
\* Other work is checked naming no session: internal work naming a session
\* to a member bound to no session and no peer, and external work naming a
\* moved session to a worker whose runtime admits no external work, are
\* answered by an addressability refusal and the stale session refusal
\* together, overlaps that predate the doors and that the arm exclusivity
\* contract's baseline carries. (Some work answers no arm here: external
\* work to a member bound to no session and no peer, which the shell never
\* submits through these arms.)
AuditDoorsAtMostOne ==
    \A id \in DoorsIds, rid \in DoorsRids, ft \in DoorsFences, origin \in DoorsOrigins :
        \A es \in (IF origin = "External" /\ DoorsEntry(id) THEN DoorsSessions ELSE {None}) :
            Cardinality({i \in 1..13 : DoorsVerdicts(id, rid, ft, es, origin)[i]}) <= 1

\* External work to an identity entry, or to a name reserved unreadable, is
\* answered by exactly one of the identity member refusals whenever no
\* refusal of a member that is not there, not live, not current or retiring
\* answers it, whatever session it names.
AuditDoorsEntryExactlyOne ==
    \A id \in DoorsIds, rid \in DoorsRids, ft \in DoorsFences, es \in DoorsSessions :
        LET v == DoorsVerdicts(id, rid, ft, es, "External") IN
            DoorsEntry(id) =>
                Cardinality({i \in 9..10 : v[i]}) = IF v[5] \/ v[6] \/ v[7] \/ v[8] THEN 0 ELSE 1

\* Witnesses of the doors run. The refusals of an unreadable name, of a
\* member that is not externally addressable, of a peer-only member and of
\* a stale session answer exactly the work their guards name: the coverage
\* check, which requires each to fire in this run, is their witness.
\* External work to an externally addressable worker is admitted.
AuditWitnessDoorsExternalWorkerAdmitted ==
    /\ ~DoorsEntry(Id2)
    /\ SubmitWorkRunningExternal(Id2, Rid2, 2, None, DoorsWork, "External", DoorsAttribution)
\* Internal work to a Declared entry is admitted (its host's own door).
AuditWitnessDoorsInternalEntryAdmitted ==
    /\ Origin(Id1) = Some("Declared")
    /\ SubmitWorkRunningInternal(Id1, Rid1, 1, None, DoorsWork, "Internal", DoorsAttribution)
\* External work to a Declared entry whose runtime is externally
\* addressable is refused as an identity member.
AuditWitnessDoorsExternalEntryRefused ==
    /\ Origin(Id1) = Some("Declared")
    /\ ResolveSubmitWorkRejectionIdentityMember(Id1, Rid1, 1, None, "External")
\* External work to a Created entry is refused as an identity member.
AuditWitnessDoorsExternalCreatedRefused ==
    /\ Created(Id2)
    /\ ResolveSubmitWorkRejectionIdentityMember(Id2, Rid2, 2, None, "External")
\* External work to a Declared entry that names another session than the
\* entry's binding is refused as an identity member, not as a stale session.
AuditWitnessDoorsExternalEntryMovedSessionRefused ==
    /\ Origin(Id1) = Some("Declared")
    /\ ResolveSubmitWorkRejectionIdentityMember(Id1, Rid1, 1, Some(Sess2), "External")

NotAuditWitnessDoorsExternalWorkerAdmitted == [][~AuditWitnessDoorsExternalWorkerAdmitted]_vars
NotAuditWitnessDoorsInternalEntryAdmitted == [][~AuditWitnessDoorsInternalEntryAdmitted]_vars
NotAuditWitnessDoorsExternalEntryRefused == [][~AuditWitnessDoorsExternalEntryRefused]_vars
NotAuditWitnessDoorsExternalCreatedRefused == [][~AuditWitnessDoorsExternalCreatedRefused]_vars
NotAuditWitnessDoorsExternalEntryMovedSessionRefused ==
    [][~AuditWitnessDoorsExternalEntryMovedSessionRefused]_vars
====
