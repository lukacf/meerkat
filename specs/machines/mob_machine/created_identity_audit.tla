---- MODULE created_identity_audit ----
\* Hand-written audit of the MobMachine created identity entries: the
\* recovery run of created_identity_audit.sh.
\*
\* A created identity entry (origin Created) is recorded by the boot's scan
\* of the identity store's created entry rows (RecoverCreatedIdentityEntry,
\* after the journal replay and before the boot plan's seal), one row at a
\* time, from the row's presence and the machine's replayed facts: a Present
\* row of a seated local member records it a Created entry; a Present row of
\* an identity the replay left without a member records the entry owed its
\* seat, with the session its member was last bound to as its revival
\* record; a row pending its host's preparation records it
\* TemplatePreparation; a removed row records the name's tombstone; and an
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
\* checks four runs over fixed domains (the recovery part of the audit):
\*
\*   scan (ScanSpec): member 1 recovered and bound (a seated local member),
\*     its Declared entry row replayed or not; member 2 with no member; the
\*     scan of a row of either with every presence and revival record, in a
\*     running and a stopped mob; the boot plan's seal.
\*   reseat (ReseatSpec): neither member recovered; the scan of their rows,
\*     without a revival record but member 2's Present row (session 2); a
\*     spawn ladder for member 1 without custody, seated on session 1, and
\*     one for member 2 with each pair of custodies, seated on its revival
\*     record (session 2) or a fresh session (session 3); their aborts; the
\*     worker reconcile of both members or of none; the forced destroy.
\*   names (NamesSpec): member 1 recovered and bound, its Declared entry
\*     row replayed or not; member 2 with no member; the scan of their rows,
\*     without a revival record; the declarations, the seal over every
\*     desired set, the legacy adoption, the agent lane and the admission
\*     probe with each pair of custodies, under the build seed's composition
\*     authority, in a running and a stopped mob.
\*   placed (PlacedScanSpec): one member host bound and member 2 seated
\*     there by the placed spawn ladder, its activation committed; the scan
\*     of its row at any point of the ladder (a row under the open ladder
\*     reserves the name, and the remote commit then refuses it).
\*
\* Each run explores its whole reachable space under the VIEW of
\* member_residency_audit, with TLC coverage. Properties: every generated
\* invariant of ci.cfg (the created entries' among them:
\* unseated_created_entry_is_preparing_or_owed, tombstone_is_never_an_entry,
\* entry_custodies_are_disjoint, worker_reconcile_spares_entries,
\* destroyed_mob_holds_no_member_lifecycle), and per run:
\*   scan and placed: AuditRecoverExactlyOne (invariant, ENABLED form:
\*     exactly one arm answers every row while the replay runs, and none
\*     once the boot plan is sealed).
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
Presences == {"Present", "PresentUnseated", "Tombstone", "Unreadable"}
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
       ENABLED RecoverCreatedIdentityEntryUnreadableStopped(id, presence, last) >>

RecoverScan == \E id \in CreatedIds, presence \in Presences, last \in Lasts : RecoverArm(id, presence, last)

\* Exactly one arm answers every row while the replay runs (a running or a
\* stopped mob whose boot plan is not sealed), and none once it is sealed.
\* No arm's guard reads the row's revival record (only the Retired arm's
\* update does), so the reseat run, whose states this costs most, counts
\* the arms of rows without one.
RecoverExactlyOneOver(lasts) ==
    \A id \in CreatedIds, presence \in Presences, last \in lasts :
        Cardinality({i \in 1..10 : RecoverVerdicts(id, presence, last)[i]})
            = IF boot_plan_sealed = FALSE /\ phase \in {"Running", "Stopped"} THEN 1 ELSE 0
AuditRecoverExactlyOne == RecoverExactlyOneOver(Lasts)

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
C1Declared == RecoverIdentityEntrySeatedRunning(Id1, Rid1, "Declared")
CStop == AuditQuiesceStop \/ StopRunning \/ ResumeStopped

(***************************************************************************)
(* The scan run.                                                           *)
(***************************************************************************)

ScanNext ==
    \/ C1Recover
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
    /\ UnseatedNext(Id2) = Some("TemplatePreparation")
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
    \/ AuditQuiesceDestroy
    \/ DestroyFromAny

ReseatSpec == Init /\ [][ReseatNext]_vars

\* No spawn ladder opens for a reserved name.
AuditNoLadderForReservedName ==
    [][\A id \in CreatedIds :
         (Reserved(id) /\ id \notin DOMAIN spawn_exec_phase) => id \notin DOMAIN spawn_exec_phase'
      ]_vars

\* A ladder holds entry custody only for a Declared entry's name or no
\* entry's, and created custody only for a Created entry owed its seat.
AuditLadderCustodyMatchesOrigin ==
    [][\A id \in CreatedIds :
         /\ (id \notin spawn_exec_entry_custody /\ id \in spawn_exec_entry_custody')
              => Origin(id) \in {None, Some("Declared")}
         /\ (id \notin spawn_exec_created_custody /\ id \in spawn_exec_created_custody')
              => (Created(id) /\ Unseated(id) = Some("SeatOwed"))
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
    \/ DeclareIdentityEntryRefusedNameOwnedByCreated(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRefusedNameTombstoned(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRefusedNameUnreadable(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
DeclareOneArmAll(id, s) ==
    Cardinality({i \in 1..18 :
        (DeclareEnabled(id, s) \o
         << ENABLED DeclareIdentityEntryRefusedNameOwnedByCreated(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
            ENABLED DeclareIdentityEntryRefusedNameTombstoned(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
            ENABLED DeclareIdentityEntryRefusedNameUnreadable(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]) >>)[i]}) = 1

\* Every arm of the legacy adoption, every phase of the run.
NamesAdoptVerdicts(id, observed) ==
    << ENABLED AdoptLegacyIdentityEntryAdopted(id, 0, observed),
       ENABLED AdoptLegacyIdentityEntryNameHeldByWorker(id, 0, observed),
       ENABLED AdoptLegacyIdentityEntryNotAdoptable(id, 0, observed),
       ENABLED AdoptLegacyIdentityEntryStaleObservation(id, 0, observed),
       ENABLED AdoptLegacyIdentityEntryNotSeated(id, 0, observed),
       ENABLED AdoptLegacyIdentityEntryAlreadyAnEntry(id, 0, observed),
       ENABLED AdoptLegacyIdentityEntryOtherCustody(id, 0, observed),
       ENABLED AdoptLegacyIdentityEntryNonAuthoritative(id, 0, observed),
       ENABLED AdoptLegacyIdentityEntryMobNotRunningStopped(id, 0, observed) >>

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
    \/ C1Recover
    \/ C1Bind
    \/ C1Declared
    \/ NamesScan
    \/ ASeed
    \/ CStop
    \/ \E id \in CreatedIds : DeclareArmAll(id, NamesShape)
    \/ \E d \in SUBSET CreatedIds : SealArm(d)
    \/ \E id \in CreatedIds, observed \in BOOLEAN : AdoptArm(id, 0, observed)
    \/ \E id \in CreatedIds : NamesLaneArm(id)
    \/ \E id \in CreatedIds, ec \in BOOLEAN, cc \in BOOLEAN : ProbeArm(id, ec, cc)

NamesSpec == Init /\ [][NamesNext]_vars

AuditNamesDeclareExactlyOne == \A id \in CreatedIds : DeclareOneArmAll(id, NamesShape)
AuditNamesAdoptionExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in CreatedIds, observed \in BOOLEAN :
            Cardinality({i \in 1..9 : NamesAdoptVerdicts(id, observed)[i]}) = 1
AuditNamesLaneExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in CreatedIds : Cardinality({i \in 1..6 : NamesLaneVerdicts(id)[i]}) = 1
AuditNamesProbeExactlyOne == ProbeExactlyOne

\* The seal never unlists, marks or removes a Created entry or an
\* unreadable name's entry.
AuditSealNeverTouchesCreated ==
    [][\A d \in SUBSET CreatedIds :
         SealDeclaredRosterSealed(d)
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
    /\ SealDeclaredRosterSealed({})
    /\ OriginNext(Id2) = Some("Created")
\* A complete seal that does not list an unreadable name's Declared entry
\* keeps it unmarked.
AuditWitnessSealSkipsUnreadable ==
    /\ Origin(Id1) = Some("Declared")
    /\ Id1 \in identity_entry_unreadable
    /\ Id1 \in DOMAIN identity_to_runtime
    /\ SealDeclaredRosterSealed({})
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
====
