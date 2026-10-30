---- MODULE identity_entry_lifecycle_audit ----
\* Hand-written audit of MobMachine identity entries through the member
\* lifecycle: the lifecycle run of identity_entry_audit.sh.
\*
\* identity_entry_audit drives the arms that record, refuse and replay an
\* entry, but no live unseat: its members leave only by the replay of their
\* retirement. This run takes member_residency_audit's retire run (one
\* identity recovered, woken, released, retired by the routed or the
\* archive-only start to its archive terminal, rotated, seated again on its
\* first seat's session and destroyed) and makes its member an entry:
\*
\*   the replay of its entry row, on any incarnation it is seated as (its
\*     first, its reseat's or its rotation's next), in Running and Stopped;
\*   the reseat's spawn ladder, opened with or without entry custody, so
\*     the dormant seat (CommitDormantSeatRunning) records the entry or a
\*     worker.
\*
\* With an entry present the run reaches the dormant seat, the live and the
\* archive-only retirement to every local archive terminal it drives, the
\* rotation's terminals in Running and Stopped, and the mob's destroy: its
\* admission, its member retirement and archive, DestroyMob and the forced
\* DestroyFromAny (with an open ladder's entry custody too). The run leaves
\* out the retire run's replay of the exempt row, which entries do not read.
\*
\* Properties: every generated invariant of ci.cfg (kept), among them
\* entry_is_seated_or_has_an_unseated_cause (every entry is seated, so every
\* unseat ends the entry), entry_custody_ladder_is_open and
\* destroyed_mob_holds_no_member_lifecycle (a destroyed mob holds no entry
\* and no entry custody); the retire run's own properties
\* (AuditAttemptEndsClosed, AuditRetireRoutesOnlyRegistered,
\* AuditDestroyFencesRotation, AuditReseatUnbornIffNeverBorn,
\* AuditRotationDecidesExemptionAnew, AuditWakingClosable and
\* AuditRetirementArchivable); and
\*   AuditSeatRecordsEntryFromCustody (action property): a dormant seat
\*     records its member an entry exactly when its ladder holds entry
\*     custody, and consumes the custody;
\*   AuditRotationKeepsEntry (action property): a rotation's terminal leaves
\*     the entries as they were (its next incarnation keeps the identity's
\*     entry);
\*   AuditLifecycleEntryOnlyWithCause (action property): an identity becomes
\*     an entry only by a seat that consumes its entry custody or the replay
\*     of its row.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous. With --mutants the
\* runner also seeds six defects into a copy of the generated model and
\* requires TLC to refuse each (see identity_entry_audit.sh).
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* identity_entry_audit.sh.
EXTENDS member_residency_audit

AuditEntryOrigin == "Declared"

\* The replay of member 1's entry row for the incarnation it is seated as: its
\* first (Rid1), its reseat (Rid2) or its rotation's next (Rid3), in a running
\* or a stopped mob. The replay applies only before the boot plan is sealed;
\* this run never seals it.
LEntryReplay ==
    \E rid \in {Rid1, Rid2, Rid3} :
        \/ RecoverIdentityEntrySeatedRunning(Id1, rid, AuditEntryOrigin)
        \/ RecoverIdentityEntrySeatedStopped(Id1, rid, AuditEntryOrigin)

\* The reseat's ladder (the retire run's RReseatOpen), opened with or without
\* entry custody.
LReseatOpenAs(custody) ==
    BeginSpawnExecFresh(Id1, Rid2, 2, 1, AuditDigest, FALSE, AuditMode, Some(Sess1), None, None,
                        FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE,
                        None, None, None, None, FALSE, FALSE, custody)
LReseatOpen == \E custody \in BOOLEAN : LReseatOpenAs(custody)

\* The reseat itself (the retire run's RReseat).
LSeatAs(unborn, ex) ==
    CommitDormantSeatRunning(Id1, Rid2, 2, 1, FALSE, AuditMode, Sess1, None, FALSE, FALSE, unborn, FALSE, ex)

\* The destroy's member archives.
LDestroyArchives ==
    \/ ObserveDestroyMemberRetirementArchivedLiveRunning(Id1, Rid1, 1, 0, Some(Sess1), AuditDisposal)
    \/ ObserveDestroyMemberRetirementArchivedRetiredRunning(Id1, Rid1, 1, 0, Some(Sess1), AuditDisposal)

LifecycleNext ==
    \/ WRecover
    \/ WBind
    \/ WEndpoint
    \/ WSeatSession
    \/ LEntryReplay
    \/ RSeedHostPending
    \/ ResolveMemberHostPreparationRunning(Id1)
    \/ AuditOperatorWake(Id1)
    \/ AuditWakePath(Id1)
    \/ WReadyEpoch1
    \/ WReleaseEpoch1
    \/ RetireArms
    \/ RRetry
    \/ RRuntimeRetired
    \/ WDetach
    \/ PlainArchives
    \/ RRecoverStarted
    \/ RAuthorize
    \/ RotationArms
    \/ LReseatOpen
    \/ RReseat
    \/ AuditQuiesceStop
    \/ StopRunning
    \/ ResumeStopped
    \/ AuditQuiesceDestroy
    \/ AdmitDestroyCleanup
    \/ RDestroyMember
    \/ DestroyMob(Sess1)
    \/ DestroyFromAny

LifecycleSpec == Init /\ [][LifecycleNext]_vars

(***************************************************************************)
(* Properties.                                                             *)
(***************************************************************************)

LEntries == DOMAIN identity_entry_origins
LNewEntries == DOMAIN identity_entry_origins' \ DOMAIN identity_entry_origins

\* A dormant seat records its member an entry exactly when its ladder holds
\* entry custody, and consumes the custody.
AuditSeatRecordsEntryFromCustody ==
    [][\A unborn \in BOOLEAN, ex \in BOOLEAN :
         LSeatAs(unborn, ex)
           => /\ (Id1 \in DOMAIN identity_entry_origins' <=> Id1 \in spawn_exec_entry_custody)
              /\ Id1 \notin spawn_exec_entry_custody'
      ]_vars

\* A rotation's terminal leaves the entries as they were: the identity's next
\* incarnation keeps its entry.
AuditRotationKeepsEntry == [][RCommitRotation => identity_entry_origins' = identity_entry_origins]_vars

\* An identity becomes an entry only by a seat that consumes its entry
\* custody or the replay of its row.
AuditLifecycleEntryOnlyWithCause ==
    [][\A id \in LNewEntries :
         \/ id \in spawn_exec_entry_custody
         \/ \E rid \in {Rid1, Rid2, Rid3} :
                \/ RecoverIdentityEntrySeatedRunning(id, rid, AuditEntryOrigin)
                \/ RecoverIdentityEntrySeatedStopped(id, rid, AuditEntryOrigin)
      ]_vars

(***************************************************************************)
(* Reachability goals, each checked as an expected violation of its        *)
(* negation: a state goal joins the invariants, a step goal the properties.*)
(***************************************************************************)

\* A ladder opened with entry custody seats its member Dormant as an entry.
AuditWitnessEntrySeated == \E unborn \in BOOLEAN, ex \in BOOLEAN : LSeatAs(unborn, ex) /\ Id1 \in LNewEntries
\* An entry is woken Live (state goal).
AuditWitnessEntryLive == Id1 \in LEntries /\ Phase1 = Some("Live")
\* A Live entry's routed retirement starts.
AuditWitnessEntryRetiredLive == Id1 \in LEntries /\ Phase1 = Some("Live") /\ RetireArms
\* The archive terminal of an entry's retirement unseats it, which ends the
\* entry.
AuditWitnessEntryArchived == Id1 \in LEntries /\ PlainArchives /\ Id1 \notin DOMAIN identity_entry_origins'
\* An entry is rotated: its next incarnation keeps the entry.
AuditWitnessEntryRotated == Id1 \in LEntries /\ RCommitRotation
\* The destroy's member archive ends an entry.
AuditWitnessEntryDestroyArchived == Id1 \in LEntries /\ LDestroyArchives
\* DestroyMob, and the forced DestroyFromAny, with an entry seated.
AuditWitnessEntryDestroyedMob == LEntries # {} /\ DestroyMob(Sess1)
AuditWitnessEntryDestroyedForced == LEntries # {} /\ DestroyFromAny
\* The forced destroy while a ladder holds entry custody.
AuditWitnessCustodyDestroyed == spawn_exec_entry_custody # {} /\ DestroyFromAny

NotAuditWitnessEntrySeated == [][~AuditWitnessEntrySeated]_vars
NotAuditWitnessEntryLive == ~AuditWitnessEntryLive
NotAuditWitnessEntryRetiredLive == [][~AuditWitnessEntryRetiredLive]_vars
NotAuditWitnessEntryArchived == [][~AuditWitnessEntryArchived]_vars
NotAuditWitnessEntryRotated == [][~AuditWitnessEntryRotated]_vars
NotAuditWitnessEntryDestroyArchived == [][~AuditWitnessEntryDestroyArchived]_vars
NotAuditWitnessEntryDestroyedMob == [][~AuditWitnessEntryDestroyedMob]_vars
NotAuditWitnessEntryDestroyedForced == [][~AuditWitnessEntryDestroyedForced]_vars
NotAuditWitnessCustodyDestroyed == [][~AuditWitnessCustodyDestroyed]_vars
====
