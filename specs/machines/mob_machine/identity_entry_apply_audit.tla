---- MODULE identity_entry_apply_audit ----
\* Hand-written audit of the MobMachine declared apply: the runs apply,
\* legacy, roles, hold, resume, held and placed of identity_entry_audit.sh.
\*
\* The declared apply records a host's roster as identity entries
\* (DeclareIdentityEntry, one arm per verdict of identity_entry_verdict),
\* seals a complete roster (SealDeclaredRoster: an unlisted Declared entry is
\* marked while its member is seated, or removed when it is not), and keeps
\* every entry seated or unseated with a typed cause. The end of a
\* retirement the seal did not start keeps the entry unseated, owed a seat
\* (hosted on its first use when its incarnation was hosted elsewhere), with
\* a local member's archived session recorded
\* (identity_entry_archived_sessions), and a later seat of the entry revives
\* it on that session; the end of an unlisted retirement removes the entry.
\* A seat that would resume a session built under another role needs the
\* migration from that role declared, and is held (RoleMigrationHeld)
\* otherwise; a role hold ends only when the member's own role is declared
\* or a rotation terminal commits another role, and never outlives its
\* entry (every end of the entry ends it: its member's retirement end, live,
\* replayed or in a destroy, the seal's removal of an unseated entry and a
\* reset). The generated ci profile has no identities, so these arms
\* hold there only vacuously. This module keeps
\* the generated model unchanged, extends member_residency_audit for its
\* constants and arm families, and checks seven runs over fixed domains:
\*
\*   apply (ApplySpec): member 1 declared local and seated Dormant by the
\*     spawn ladder holding its entry custody, retired (archive-only) to its
\*     archive terminal, unlisted or not, declared again (under the role its
\*     archived session was built under, or another with the migration from
\*     it declared or not) and seated on its archived session (the revive)
\*     or, its record let go (ForgetArchivedSession), on a fresh one; the seal
\*     over every desired set, the replay of an unlisted retirement's mark,
\*     the listing of a marked member, the build seed's composition authority
\*     (a candidate composition declares and seals nothing) and the mob's
\*     forced destroy. The run holds no reset, so an entry is seated on a
\*     fresh session here only once a forget let its record go.
\*   legacy (LegacySpec): member 2 a legacy member recovered by replay, a
\*     worker until its adoption, then unlisted and retired to every copy of
\*     the plain end: live or after its runtime retired, in a running or a
\*     stopped mob, or the replay of its retirement (its runtime recovered or
\*     already gone, its session archived or not, its incarnation local or
\*     hosted elsewhere); the replay of its start row (marking it, or stale);
\*     its declarations, the seal, the build seed's composition authority,
\*     Stop, Resume and Complete, and the mob's reset from each phase it
\*     leaves (its intent, its preflight with member 2 seated or not, and
\*     its commit on an empty roster).
\*   roles (RolesSpec): member 1 recovered and made an entry by the replay of
\*     its row, declared under its own role and mode, another role (with the
\*     migration declared from its role or not) or another mode, on a durable
\*     session service or one that keeps none: the role hold, its end, the
\*     restamp and the rotation verdicts; the host's first-boot preparation
\*     seeded at build and carried out; every WakeMember cause; the wake,
\*     release and the same-session rotation that carries out a declared
\*     migration (its grant under the new role, its start and its terminal);
\*     the archive-only retirement to its terminal.
\*   hold (HoldSpec): member 1 recovered, made an entry by its row, its
\*     first-boot preparation owed or not, declared under another role with
\*     no migration (held; with a first-boot Resume request too), with the
\*     migration, on a session kept in memory only, or under its own role;
\*     its grant under either role, its same-session rotation and its
\*     fresh-session rotation (after its archive-only retirement started).
\*   resume (ResumeSpec): member 1 recovered Dormant and made an entry by
\*     its row, its first-boot preparation owed or not, woken to Live by one
\*     operator wake or not, and declared under another role with no
\*     migration (its role hold) or not, in a stopped mob the explicit resume
\*     brings back (its member probed while the mob is still stopped, at
\*     its candidate selection, and again once it runs).
\*   held (HeldSpec): member 2 never created, declared local, placed or
\*     externally bound, held or not; the held spawn (the spawn ladder with
\*     entry custody and its membership commit) and its abort; the dormant
\*     seat's refusal of a held or hosted entry; member 1 recovered and
\*     retired by replay (ever created), declared held; the seal; Stop,
\*     Resume, Complete and the forced destroy (which ends the held spawn's
\*     ladder, its membership committed or not), where the declaration and
\*     the seal refuse; the declared apply's open fact (open, busy, refused,
\*     closed).
\*   placed (PlacedSpec): one member host bound and the owner bridge, member 2
\*     declared placed (hosted on its first use) and member 1 declared local
\*     (owed its seat): the placed spawn ladder opened with or without entry
\*     custody for either, and its abort; member 2's remote commit, its
\*     retirement and its placed end.
\*
\* Each run explores its whole reachable space under a VIEW that leaves out
\* model_step_count and topology_epoch, which no guard reads, with TLC
\* coverage. Properties: every generated invariant of ci.cfg (the declared
\* apply's among them: entry_is_seated_or_has_an_unseated_cause,
\* unseated_entry_is_not_a_member, retiring_entry_is_a_declared_entry,
\* held_creation_entry_was_never_created, archived_session_record_is_unseated,
\* placed_entry_custody_is_hosted_on_first_use, entry_custody_ladder_is_open,
\* role_hold_belongs_to_an_entry, destroyed_mob_holds_no_member_lifecycle),
\* and per run:
\*   apply and legacy: AuditDeclareExactlyOne and AuditSealExactlyOne
\*     (invariants, ENABLED form: exactly one arm answers every declaration
\*     and seal, in every lifecycle phase of the run);
\*     AuditEntryLeavesBySealDestroyOrReset (action property: a Declared
\*     entry leaves only at the end of the retirement the seal marked, at the
\*     seal itself when it is unseated, at a destroy or at a mob reset);
\*     AuditCandidateChangesNothing (action property: under a candidate
\*     composition no declaration, seal, record release or adoption step
\*     changes the state);
\*     AuditApplyEntryOnlyWithCause (action property: an identity becomes an
\*     entry only by a declaration, the commit of a ladder holding its
\*     custody, an observed adoption or the replay of its row).
\*   apply: AuditSeatTakesArchivedSession (a seat holding an entry's custody
\*     binds the identity's archived session when it has a record);
\*     AuditSeatBindsArchivedOnlyAsRecord (a seat binds an archived session
\*     only as the identity's record); AuditForgetNamesRecord (a forget lets
\*     go only the record naming its session);
\*     AuditResumeRoleMoveNeedsMigration (a seat resuming a session built
\*     under another role is owed only with the migration from it declared);
\*     AuditListingUnmarks (a seated member listed again loses its mark);
\*     AuditSeatKeepsRoleHold (the dormant seat keeps a role hold); all
\*     action properties.
\*   legacy: AuditMarkReplayExactlyOne (invariant, ENABLED form: exactly one
\*     arm answers the replay of a start row, while the replay runs);
\*     AuditResetPreflightAgrees (invariant, ENABLED form: on an empty
\*     roster the reset's preflight admits exactly the resets the mob
\*     commits); AuditReplayRecordsOnlyArchived (the replay records an
\*     archived session only when its row archived it);
\*     AuditHostedReplayKeepsHosted (the replay of a hosted incarnation's
\*     retirement keeps the entry hosted on its first use, with no record);
\*     AuditResetOnEmptyRoster (the reset commits only on an empty roster);
\*     AuditResetPreflightChangesNothing (the preflight changes nothing);
\*     the last four action properties.
\*   roles: AuditRolesDeclareExactlyOne; AuditHostPreparationNeverWoken
\*     (action property: no wake opens an attempt for a member with a host
\*     preparation cause; the run drives WakeMember and the boot plan, and no
\*     explicit resume or drift revival, which are outside the hold);
\*     AuditHostStepKeepsRoleHold (the host's carry-out never ends a role
\*     hold); AuditRoleHoldKeepsFirstBootRow (a declaration with no first-boot
\*     request never replaces a first-boot preparation).
\*   hold: AuditHoldDeclareExactlyOne; AuditRoleMoveKeepsRoleHold (a
\*     declaration under another role than the member's own keeps a role
\*     hold); AuditRoleHoldEndsAtNewRole (a rotation terminal ends a role hold
\*     exactly when it commits another role); AuditSeatedResumeWritesRoleHold
\*     (a seated member's first-boot Resume request ends its first-boot
\*     preparation in the declaration's step, which writes the role hold it
\*     masked); all action properties but the first.
\*   resume: AuditResumeNeverRebuildsHeldMember (the explicit resume never
\*     authorizes a member whose host preparation is owed);
\*     AuditResumeHeldChangesNothing (its held answer, stopped or running,
\*     changes nothing); AuditResumeHeldOnlyWhenOwed (it answers held only a
\*     member whose host preparation is owed); all action properties.
\*   held: AuditHeldDeclareExactlyOne and AuditSealExactlyOne (in the
\*     Stopped, Completed and Destroyed phases too); AuditApplyOpenExactlyOne
\*     (invariant, ENABLED form: exactly one arm answers an apply's opening);
\*     AuditDormantSeatTakesOnlyOwedEntries (action property: the dormant seat
\*     never seats an entry awaiting its held creation or hosted on its
\*     first use).
\*   placed: AuditPlacedDeclareExactlyOne; AuditLadderReservesEntryName (a
\*     spawn ladder opened for an entry's name holds its custody);
\*     AuditRemoteCommitSeatsEntry (the remote commit of a ladder holding an
\*     entry's custody seats the entry); AuditPlacedEndKeepsHosted (the
\*     placed end keeps the entry hosted on its first use, with no record);
\*     all action properties but the first.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations. With --mutants the runner also seeds defects into a
\* copy of the generated model and requires TLC to refuse each (see
\* identity_entry_audit.sh). These runs record no Created entry and reserve
\* no name, so the declared apply's refusals of a Created entry's, a removed
\* or an unreadable name are checked by created_identity_audit's names run,
\* with a mutant of the origin check.
\*
\* The TLC configs are DERIVED from the generated ci.cfg on every run by
\* identity_entry_audit.sh.
EXTENDS member_residency_audit

AuditProfile2 == "profile_2"
AuditModeAlt == "AutonomousHost"
Host1 == "hostid_1"
PlacedSpawn1 == "placedspawnid_1"
AuditOperation == "operation_1"
AuditEngine == "engine_1"

(***************************************************************************)
(* Arm families shared by the runs.                                        *)
(***************************************************************************)

\* Every arm of the declared apply's verdict for one declaration
\* <<profile, mode, placement, held, migration, intent managed, memory only,
\* resume session role, first-boot request>>, in every lifecycle phase.
DeclareArm(id, s) ==
    \/ DeclareIdentityEntryKept(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntrySeatOwed(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryHostedOnFirstUse(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryAwaitHeldCreation(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRuntimeModeRestamp(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRoleMigrate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRoleRotate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRoleMigrationHeld(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryLifecycleInFlight(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRefusedNameHeldByWorker(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRefusedHostAdopted(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRefusedNonAuthoritative(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryMobNotRunningStopped(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryMobNotRunningCompleted(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryMobNotRunningDestroyed(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])

DeclareEnabled(id, s) ==
    << ENABLED DeclareIdentityEntryKept(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntrySeatOwed(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryHostedOnFirstUse(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryAwaitHeldCreation(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryRuntimeModeRestamp(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryRoleMigrate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryRoleRotate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryRoleMigrationHeld(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryLifecycleInFlight(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryRefusedNameHeldByWorker(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryRefusedHostAdopted(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryRefusedNonAuthoritative(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryMobNotRunningStopped(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryMobNotRunningCompleted(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]),
       ENABLED DeclareIdentityEntryMobNotRunningDestroyed(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]) >>

\* (The declared apply's refusals of a created, removed or unreadable name
\* answer only a name these runs never record; created_identity_audit counts
\* them beside these arms.)
DeclareOneArm(id, s) == Cardinality({i \in 1..15 : DeclareEnabled(id, s)[i]}) = 1

\* Every arm of the seal of one desired set, in every lifecycle phase.
SealArm(d) ==
    \/ SealDeclaredRosterSealed(d)
    \/ SealDeclaredRosterNonAuthoritative(d)
    \/ SealDeclaredRosterMobNotRunningStopped(d)
    \/ SealDeclaredRosterMobNotRunningCompleted(d)
    \/ SealDeclaredRosterMobNotRunningDestroyed(d)

SealOneArm(d) ==
    Cardinality({i \in 1..5 :
        << ENABLED SealDeclaredRosterSealed(d),
           ENABLED SealDeclaredRosterNonAuthoritative(d),
           ENABLED SealDeclaredRosterMobNotRunningStopped(d),
           ENABLED SealDeclaredRosterMobNotRunningCompleted(d),
           ENABLED SealDeclaredRosterMobNotRunningDestroyed(d) >>[i]}) = 1

\* Every arm of the legacy adoption, in the run's phases.
AdoptArm(id, g, observed) ==
    \/ AdoptLegacyIdentityEntryAdopted(id, g, observed)
    \/ AdoptLegacyIdentityEntryNameHeldByWorker(id, g, observed)
    \/ AdoptLegacyIdentityEntryNotAdoptable(id, g, observed)
    \/ AdoptLegacyIdentityEntryStaleObservation(id, g, observed)
    \/ AdoptLegacyIdentityEntryNotSeated(id, g, observed)
    \/ AdoptLegacyIdentityEntryAlreadyAnEntry(id, g, observed)
    \/ AdoptLegacyIdentityEntryOtherCustody(id, g, observed)
    \/ AdoptLegacyIdentityEntryNonAuthoritative(id, g, observed)
    \/ AdoptLegacyIdentityEntryMobNotRunningStopped(id, g, observed)

\* The replay of an entry's row, on the incarnation it is seated as.
EntryReplay(id, rid) ==
    \/ RecoverIdentityEntrySeatedRunning(id, rid, "Declared")
    \/ RecoverIdentityEntrySeatedStopped(id, rid, "Declared")

\* Every state variable but the two counters.
StateView == [vars EXCEPT ![AuditStepIndex] = 0, ![AuditEpochIndex] = 0]

Cause(id) == IF id \in DOMAIN member_host_preparation_causes THEN Some(member_host_preparation_causes[id]) ELSE None
Unseated(id) == IF id \in DOMAIN identity_entry_unseated THEN Some(identity_entry_unseated[id]) ELSE None
UnseatedNext(id) == IF id \in DOMAIN identity_entry_unseated' THEN Some(identity_entry_unseated'[id]) ELSE None
Record(id) == IF id \in DOMAIN identity_entry_archived_sessions THEN Some(identity_entry_archived_sessions[id]) ELSE None
Binding(id) == IF id \in DOMAIN member_session_bindings THEN Some(member_session_bindings[id]) ELSE None
Profile(id) == IF id \in DOMAIN member_profile_names THEN Some(member_profile_names[id]) ELSE None
ProfileNext(id) == IF id \in DOMAIN member_profile_names' THEN Some(member_profile_names'[id]) ELSE None
BindingNext(id) == IF id \in DOMAIN member_session_bindings' THEN Some(member_session_bindings'[id]) ELSE None
Entries == DOMAIN identity_entry_origins
NewEntries == DOMAIN identity_entry_origins' \ DOMAIN identity_entry_origins
LeftEntries == DOMAIN identity_entry_origins \ DOMAIN identity_entry_origins'
CauseNext(id) ==
    IF id \in DOMAIN member_host_preparation_causes' THEN Some(member_host_preparation_causes'[id]) ELSE None
RecordNext(id) ==
    IF id \in DOMAIN identity_entry_archived_sessions' THEN Some(identity_entry_archived_sessions'[id]) ELSE None

(***************************************************************************)
(* The apply run.                                                          *)
(***************************************************************************)

\* Member 1's declarations: local, its own profile and mode, neither held nor
\* migrating, on a durable session service; managed by no intent row, or
\* by one (refused HostAdopted); and under another role than the one its
\* archived session was built under (profile 1), with the migration from
\* that role declared or not.
ApplyShape == <<AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None">>
ApplyShapeMoved == <<AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, FALSE, Some(AuditProfile), "None">>
ApplyShapeMigrated ==
    <<AuditProfile2, AuditMode, "Local", FALSE, Some(AuditProfile), FALSE, FALSE, Some(AuditProfile), "None">>
ApplyShapes ==
    {ApplyShape, <<AuditProfile, AuditMode, "Local", FALSE, None, TRUE, FALSE, None, "None">>,
     ApplyShapeMoved, ApplyShapeMigrated}

\* Member 1's incarnations <<runtime id, fence, generation, session>>: its
\* first seat, and its next one at generation 1, revived on its archived
\* session 1 or, its record let go, seated on a fresh session 3.
AFirst == <<Rid1, 1, 0, Sess1>>
ARevived == <<Rid3, 3, 1, Sess1>>
AFresh == <<Rid3, 3, 1, Sess3>>
AIncarnations == {AFirst, ARevived, AFresh}

AAuthorize ==
    AuthorizeSpawnProfileRunning(Id1, AuditProfile, AuditModel, AuditDigest, AuditDigest, AuditDigest, None, None, FALSE, None)
AOpen(inc) ==
    BeginSpawnExecFresh(Id1, inc[1], inc[2], inc[3], AuditDigest, FALSE, AuditMode, Some(inc[4]), None, None,
                        FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE,
                        None, None, None, None, FALSE, FALSE, TRUE, FALSE)
ASeatAs(inc, archived, unborn, fresh) ==
    CommitDormantSeatRunning(Id1, inc[1], inc[2], inc[3], FALSE, AuditMode, inc[4], None, archived, FALSE, unborn, fresh, FALSE)
ASeat(inc) == \E archived \in BOOLEAN, unborn \in BOOLEAN, fresh \in BOOLEAN : ASeatAs(inc, archived, unborn, fresh)
AAbort == AbortSpawnExecActiveRunning(Id1)
ARetire(inc) ==
    \/ RetireRunningReleasingArchiveOnly(AuditMob, inc[1], Id1, inc[3], Some(inc[4]), Some(inc[4]))
    \/ RetireRunningPreservingBindingArchiveOnly(AuditMob, inc[1], Id1, inc[3], None, Some(inc[4]))
AArchive(inc) == ObserveMemberRetirementArchivedLive(Id1, inc[1], inc[2], inc[3], Some(inc[4]), AuditDisposal, FALSE)
ADetach(rid) == SessionIngressDetachedForMobDestroyRunning(AuditMob, rid)
AForgetAs(s) == \E reason \in {"SessionAbsent", "FreshLifeRequested"} : ForgetArchivedSession(Id1, s, reason)
AForget == \E s \in {Sess1, Sess3} : AForgetAs(s)
AMark == \E rid \in {Rid1, Rid3} : RecoverIdentityEntryRetiringMarkedRunning(Id1, rid)

ASeed == \E authority \in {"Authoritative", "NonAuthoritative"} : SeedMemberResidencyRunning("OnDemand", authority, {})
AQuiesceReset == BeginPlacedCompletionLifecycleQuiesceFresh("Reset")
\* The reset's intent in a stopped and a completed mob (each takes over the
\* quiesce its Stop or Complete began), and the reset's preflight in each
\* phase a reset leaves.
AQuiesceResetStopped == BeginPlacedCompletionLifecycleQuiesceStoppedReplay("Reset")
AQuiesceResetCompleted == BeginPlacedCompletionLifecycleQuiesceCompletedReplay("Reset")
AdmitReset == AdmitResetRunning \/ AdmitResetStopped \/ AdmitResetCompleted

\* Member 2 (the legacy run): a legacy member the replay recovered, a
\* worker until adopted, retired (archive-only) in a running mob and ended
\* by any copy of the plain end: live or after its runtime retired, in a
\* running or a stopped mob, or by the replay of its retirement (the runtime
\* recovered or already gone, its session archived or not, the incarnation
\* local or hosted elsewhere: the generic replay of a placed or externally
\* bound incarnation whose carrier is gone restores no placement).
A2Recover == RecoverRosterMemberRunning(Id2, Rid2, 2, 0, AuditProfile, AuditMode, FALSE)
A2Bind == RecoverMemberSessionBindingFreshRunning(Id2, Rid2, Sess2, None)
A2Dormant == PlanBootMaterializationStayDormant(Id2, "None", FALSE, FALSE)
A2Adopt == \E observed \in BOOLEAN : AdoptArm(Id2, 0, observed)
A2Retire ==
    \/ RetireRunningReleasingArchiveOnly(AuditMob, Rid2, Id2, 0, Some(Sess2), Some(Sess2))
    \/ RetireRunningPreservingBindingArchiveOnly(AuditMob, Rid2, Id2, 0, None, Some(Sess2))
A2RuntimeRetired == ObserveRuntimeRetired(Rid2, 2) \/ ObserveRuntimeRetiredStopped(Rid2, 2)
A2ArchiveLive == ObserveMemberRetirementArchivedLive(Id2, Rid2, 2, 0, Some(Sess2), AuditDisposal, FALSE)
A2ArchiveLiveStopped == ObserveMemberRetirementArchivedLiveStopped(Id2, Rid2, 2, 0, Some(Sess2), AuditDisposal, FALSE)
A2ArchiveRetired == ObserveMemberRetirementArchivedRetired(Id2, Rid2, 2, 0, Some(Sess2), AuditDisposal, FALSE)
A2ArchiveRetiredStopped == ObserveMemberRetirementArchivedRetiredStopped(Id2, Rid2, 2, 0, Some(Sess2), AuditDisposal, FALSE)
A2Archive == A2ArchiveLive \/ A2ArchiveLiveStopped \/ A2ArchiveRetired \/ A2ArchiveRetiredStopped
A2ReplayEndAs(archived, hosted) ==
    \/ RecoverRosterMemberRetiredRunning(Id2, Rid2, 0, FALSE, FALSE, archived, hosted)
    \/ RecoverRosterMemberRetiredCurrentAlreadyAbsent(Id2, Rid2, 0, FALSE, FALSE, archived, hosted)
A2ReplayEnd == \E archived \in BOOLEAN, hosted \in BOOLEAN : A2ReplayEndAs(archived, hosted)
\* Member 2 seated as the incarnation the replayed row names.
A2Current == Id2 \in DOMAIN identity_to_runtime /\ identity_to_runtime[Id2] = Rid2
A2End == A2Archive \/ A2ReplayEnd
\* The replay of an unlisted retirement's start row, for the incarnation
\* the identity is seated as (it marks a Declared entry) or another one,
\* or a worker's (it marks nothing).
A2MarkFor(rid) ==
    \/ RecoverIdentityEntryRetiringMarkedRunning(Id2, rid)
    \/ RecoverIdentityEntryRetiringStaleRunning(Id2, rid)
A2Mark == \E rid \in {Rid1, Rid2} : A2MarkFor(rid)

\* The declaration, seal, record release and adoption steps of both runs.
ApplyIdentitySteps ==
    \/ \E id \in {Id1, Id2}, s \in ApplyShapes : DeclareArm(id, s)
    \/ \E d \in SUBSET {Id1, Id2} : SealArm(d)
    \/ AForget
    \/ A2Adopt

\* The run holds no reset: an entry is seated on a fresh session here only
\* once its record is let go (the legacy run drives the reset).
ApplyNext ==
    \/ ASeed
    \/ \E s \in ApplyShapes : DeclareArm(Id1, s)
    \/ \E d \in SUBSET {Id1} : SealArm(d)
    \/ AAuthorize
    \/ \E inc \in AIncarnations : AOpen(inc) \/ ASeat(inc) \/ ARetire(inc) \/ AArchive(inc)
    \/ AAbort
    \/ \E rid \in {Rid1, Rid3} : ADetach(rid)
    \/ AForget
    \/ AMark
    \/ SealBootMaterializationPlan
    \/ AuditQuiesceDestroy
    \/ DestroyFromAny

ApplySpec == Init /\ [][ApplyNext]_vars

\* The legacy run: member 2 recovered by replay, a worker until its
\* adoption, then unlisted and retired to every copy of the plain end; its
\* declarations, the seal, the replay of its unlisted retirement's mark, the
\* build seed's composition authority, Stop, Resume and Complete, and the
\* mob's reset from each phase it leaves: its intent, its preflight (with
\* member 2 seated or not) and its commit, which needs the roster empty.
LegacyNext ==
    \/ ASeed
    \/ \E s \in ApplyShapes : DeclareArm(Id2, s)
    \/ \E d \in SUBSET {Id2} : SealArm(d)
    \/ A2Recover
    \/ A2Bind
    \/ A2Dormant
    \/ A2Adopt
    \/ A2Retire
    \/ A2RuntimeRetired
    \/ A2End
    \/ ADetach(Rid2)
    \/ A2Mark
    \/ SealBootMaterializationPlan
    \/ AuditQuiesceStop
    \/ StopRunning
    \/ ResumeStopped
    \/ AuditQuiesceComplete
    \/ CompleteRunning
    \/ AQuiesceReset
    \/ AQuiesceResetStopped
    \/ AQuiesceResetCompleted
    \/ AdmitReset
    \/ ResetToRunning

LegacySpec == Init /\ [][LegacyNext]_vars

\* Exactly one arm answers every declaration and every seal, in every phase.
AuditDeclareExactlyOne == \A id \in {Id1, Id2}, s \in ApplyShapes : DeclareOneArm(id, s)
AuditSealExactlyOne == \A d \in SUBSET {Id1, Id2} : SealOneArm(d)

\* Exactly one arm answers the replay of an unlisted retirement's start row
\* for every incarnation, while the replay runs (a running mob whose boot
\* plan is not sealed).
AuditMarkReplayExactlyOne ==
    (phase = "Running" /\ boot_plan_sealed = FALSE)
        => \A rid \in {Rid1, Rid2} :
             Cardinality({i \in 1..2 :
                 << ENABLED RecoverIdentityEntryRetiringMarkedRunning(Id2, rid),
                    ENABLED RecoverIdentityEntryRetiringStaleRunning(Id2, rid) >>[i]}) = 1

\* A Declared entry leaves the entries only at the end of the
\* retirement the seal marked, at the seal itself when it is unseated, at a
\* destroy or at a mob reset.
AuditEntryLeavesBySealDestroyOrReset ==
    [][\A id \in LeftEntries :
         \/ id \in identity_entry_retiring
         \/ \E d \in SUBSET {Id1, Id2} : SealDeclaredRosterSealed(d)
         \/ destroy_admitted
         \/ phase' = "Destroyed"
         \/ ResetToRunning
      ]_vars

\* Under a candidate composition no declaration, seal, record release or
\* adoption step changes the state.
AuditCandidateChangesNothing ==
    [][(composition_authority = "NonAuthoritative" /\ ApplyIdentitySteps) => StateView' = StateView]_vars

\* An identity becomes an entry only by a declaration (which records its
\* unseated cause), the commit of a ladder holding its custody, an observed
\* adoption or the replay of its row.
AuditApplyEntryOnlyWithCause ==
    [][\A id \in NewEntries :
         \/ id \in DOMAIN identity_entry_unseated'
         \/ id \in spawn_exec_entry_custody
         \/ AdoptLegacyIdentityEntryAdopted(id, 0, TRUE)
      ]_vars

\* A seat holding an entry's custody binds the identity's archived session
\* when it has a record: the entry comes back with its history.
AuditSeatTakesArchivedSession ==
    [][\A inc \in AIncarnations :
         (ASeat(inc) /\ Id1 \in spawn_exec_entry_custody /\ Record(Id1) # None)
           => BindingNext(Id1) = Record(Id1)
      ]_vars

\* A dormant seat that binds an archived session binds the identity's own
\* record of it, never another archived session.
AuditSeatBindsArchivedOnlyAsRecord ==
    [][\A inc \in AIncarnations, unborn \in BOOLEAN, fresh \in BOOLEAN :
         ASeatAs(inc, TRUE, unborn, fresh) => Record(Id1) = Some(inc[4])
      ]_vars

\* A forget lets go only the record that names its session.
AuditForgetNamesRecord ==
    [][\A s \in {Sess1, Sess3} : AForgetAs(s) => Record(Id1) = Some(s)]_vars

\* An unseated identity whose seat would resume a session built under
\* another role is owed its seat only with the migration from that role
\* declared.
AuditResumeRoleMoveNeedsMigration ==
    [][\A s \in ApplyShapes :
         (DeclareIdentityEntrySeatOwed(Id1, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
            /\ s[8] # None /\ s[8] # Some(s[1]))
           => s[5] = s[8]
      ]_vars

\* A seated member listed again before its unlisted retirement started
\* loses its mark, so a later retirement keeps its entry.
SeatedListed(id, s) ==
    \/ DeclareIdentityEntryKept(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRuntimeModeRestamp(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRoleMigrate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ DeclareIdentityEntryRoleRotate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9])
    \/ (id \in DOMAIN identity_to_runtime
          /\ DeclareIdentityEntryRoleMigrationHeld(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9]))
AuditListingUnmarks ==
    [][\A s \in ApplyShapes : SeatedListed(Id1, s) => Id1 \notin identity_entry_retiring']_vars

\* The dormant seat ends a first-boot preparation only: a role hold
\* survives the seat.
AuditSeatKeepsRoleHold ==
    [][\A inc \in AIncarnations :
         (ASeat(inc) /\ Cause(Id1) = Some("RoleMigrationUndeclared"))
           => CauseNext(Id1) = Some("RoleMigrationUndeclared")
      ]_vars

\* The replay of a retirement whose row records no archived session records
\* none.
AuditReplayRecordsOnlyArchived ==
    [][(\E hosted \in BOOLEAN : A2ReplayEndAs(FALSE, hosted)) => RecordNext(Id2) = Record(Id2)]_vars

\* The replay of a retirement whose row records the incarnation hosted
\* elsewhere keeps an entry the seal did not unlist hosted on its first use,
\* with no archived session recorded, as its live end did.
AuditHostedReplayKeepsHosted ==
    [][((\E archived \in BOOLEAN : A2ReplayEndAs(archived, TRUE))
          /\ Id2 \in Entries /\ Id2 \notin identity_entry_retiring /\ A2Current)
         => (UnseatedNext(Id2) = Some("HostedOnFirstUse") /\ RecordNext(Id2) = Record(Id2))
      ]_vars

\* The reset commits only on an empty roster: its retirement batch ended
\* every member's retirement first, so no member before it is a member
\* after it.
AuditResetOnEmptyRoster == [][ResetToRunning => DOMAIN identity_to_runtime = {}]_vars

\* The reset's preflight is a verdict: it changes nothing.
AuditResetPreflightChangesNothing == [][AdmitReset => StateView' = StateView]_vars

\* On an empty roster the preflight admits exactly the resets the mob
\* commits: every other guard of the reset is the preflight's.
AuditResetPreflightAgrees ==
    DOMAIN identity_to_runtime = {} => (ENABLED AdmitReset <=> ENABLED ResetToRunning)

\* Witnesses of the apply and legacy runs (the legacy run's name member 2).
\* A legacy member adopted, then unlisted by the seal and retired: the end
\* of its retirement removes the entry.
AuditWitnessAdoptedThenUnlisted ==
    /\ Id2 \in identity_entry_retiring
    /\ A2ArchiveLive
    /\ Id2 \notin DOMAIN identity_entry_origins'
\* The same unlisted end on the stopped copy, on the copies after the
\* runtime retired, and on the replay of the retirement.
AuditWitnessUnlistedEndStopped ==
    Id2 \in identity_entry_retiring /\ A2ArchiveLiveStopped /\ Id2 \notin DOMAIN identity_entry_origins'
AuditWitnessUnlistedEndRetired ==
    Id2 \in identity_entry_retiring /\ (A2ArchiveRetired \/ A2ArchiveRetiredStopped)
        /\ Id2 \notin DOMAIN identity_entry_origins'
AuditWitnessUnlistedReplayEnd ==
    Id2 \in identity_entry_retiring /\ A2ReplayEnd /\ Id2 \notin DOMAIN identity_entry_origins'
\* The replay of a retirement whose session was not archived keeps the entry
\* with no record.
AuditWitnessReplayKeepsUnarchived ==
    /\ Id2 \in Entries
    /\ Id2 \notin identity_entry_retiring
    /\ A2Current
    /\ A2ReplayEndAs(FALSE, FALSE)
    /\ UnseatedNext(Id2) = Some("SeatOwed")
    /\ RecordNext(Id2) = None
\* The replay of a hosted incarnation's retirement keeps the entry hosted on
\* its first use, with no record, even when its row records the session
\* archived.
AuditWitnessHostedReplayKept ==
    /\ Id2 \in Entries
    /\ Id2 \notin identity_entry_retiring
    /\ A2Current
    /\ A2ReplayEndAs(TRUE, TRUE)
    /\ UnseatedNext(Id2) = Some("HostedOnFirstUse")
    /\ RecordNext(Id2) = None
\* The replay of a stale start row marks nothing.
AuditWitnessStaleMarkIgnored ==
    /\ Id2 \in DOMAIN identity_to_runtime
    /\ \E rid \in {Rid1, Rid2} : RecoverIdentityEntryRetiringStaleRunning(Id2, rid)
\* An entry removed by its unlisted retirement is declared again: it is owed
\* its seat on its archived session.
AuditWitnessRelistedAfterUnlist ==
    /\ Id1 \notin Entries
    /\ Record(Id1) = Some(Sess1)
    /\ DeclareIdentityEntrySeatOwed(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None")
\* The revive: the entry seated Dormant on its archived session, at its next
\* generation.
AuditWitnessRevived ==
    /\ Record(Id1) = Some(Sess1)
    /\ ASeatAs(ARevived, TRUE, FALSE, FALSE)
\* A retirement the seal did not start keeps the entry, unseated and owed its
\* seat, with its archived session recorded.
AuditWitnessRetiredEntryKept ==
    /\ Id1 \in Entries
    /\ Id1 \notin identity_entry_retiring
    /\ AArchive(AFirst)
    /\ UnseatedNext(Id1) = Some("SeatOwed")
    /\ Id1 \in DOMAIN identity_entry_archived_sessions'
\* The record let go, the entry owed its seat is seated on a fresh session
\* (with no reset in the run, only a forget lets the record go).
AuditWitnessFreshAfterForget ==
    /\ Id1 \in Entries
    /\ Unseated(Id1) = Some("SeatOwed")
    /\ Record(Id1) = None
    /\ ASeat(AFresh)
\* The seal removes an unseated entry it does not list.
AuditWitnessSealRemovesUnseated ==
    /\ Id1 \in Entries
    /\ Id1 \in DOMAIN identity_entry_unseated
    /\ SealDeclaredRosterSealed({})
    /\ Id1 \notin DOMAIN identity_entry_origins'
\* The replay of an unlisted retirement marks the entry.
AuditWitnessReplayMarked ==
    /\ Id1 \notin identity_entry_retiring
    /\ \E rid \in {Rid1, Rid3} : RecoverIdentityEntryRetiringMarkedRunning(Id1, rid)
\* A marked member listed again before its retirement started loses its
\* mark.
AuditWitnessListedAgainUnmarked ==
    /\ Id1 \in identity_entry_retiring
    /\ DeclareIdentityEntryKept(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None")
    /\ Id1 \notin identity_entry_retiring'
\* An unseated identity whose seat would resume a session built under
\* another role, with no migration from it declared, is recorded owed its
\* seat and held; declared with the migration, it is owed its seat.
AuditWitnessResumeRoleHeld ==
    /\ Id1 \notin DOMAIN identity_to_runtime
    /\ DeclareIdentityEntryRoleMigrationHeld(Id1, AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, FALSE, Some(AuditProfile), "None")
    /\ UnseatedNext(Id1) = Some("SeatOwed")
    /\ CauseNext(Id1) = Some("RoleMigrationUndeclared")
AuditWitnessResumeRoleMigrationOwed ==
    /\ Id1 \notin DOMAIN identity_to_runtime
    /\ DeclareIdentityEntrySeatOwed(Id1, AuditProfile2, AuditMode, "Local", FALSE, Some(AuditProfile), FALSE, FALSE, Some(AuditProfile), "None")
\* A seat of a held identity keeps the role hold.
AuditWitnessSeatKeepsRoleHold ==
    /\ Cause(Id1) = Some("RoleMigrationUndeclared")
    /\ \E inc \in AIncarnations : ASeat(inc)
    /\ CauseNext(Id1) = Some("RoleMigrationUndeclared")
\* Refusals: a candidate composition, a name a worker holds, an intent row,
\* and a declaration while the member's ladder is open.
AuditWitnessCandidateRefused == DeclareIdentityEntryRefusedNonAuthoritative(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None")
AuditWitnessNameHeldByWorker == DeclareIdentityEntryRefusedNameHeldByWorker(Id2, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None")
AuditWitnessHostAdopted == DeclareIdentityEntryRefusedHostAdopted(Id1, AuditProfile, AuditMode, "Local", FALSE, None, TRUE, FALSE, None, "None")
AuditWitnessInFlight == DeclareIdentityEntryLifecycleInFlight(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None")
\* A mob reset and a destroy with entries present.
AuditWitnessResetEndsEntries == Entries # {} /\ ResetToRunning
AuditWitnessDestroyEndsEntries == Entries # {} /\ DestroyFromAny
\* The reset's preflight admits a mob with member 2 seated, whose
\* retirement the reset's batch then ends.
AuditWitnessPreflightAdmitsMembers == Id2 \in DOMAIN identity_to_runtime /\ AdmitReset
\* The reset after member 2's retirement ended with its entry kept unseated
\* and its archived session recorded: the reset ends the entry.
AuditWitnessResetEndsRetiredEntry == Id2 \in Entries /\ Record(Id2) # None /\ ResetToRunning
\* A role hold never outlives its entry: a complete seal that does not list
\* a held unseated entry removes the entry and ends its role hold, and so
\* does a reset over one.
AuditWitnessSealEndsRoleHold ==
    /\ Id1 \in DOMAIN identity_entry_unseated
    /\ Cause(Id1) = Some("RoleMigrationUndeclared")
    /\ SealDeclaredRosterSealed({})
    /\ Id1 \notin DOMAIN identity_entry_origins'
    /\ CauseNext(Id1) = None
AuditWitnessResetEndsRoleHold ==
    /\ Id2 \in DOMAIN identity_entry_unseated
    /\ Cause(Id2) = Some("RoleMigrationUndeclared")
    /\ ResetToRunning
    /\ CauseNext(Id2) = None
\* The replayed end of an unlisted retirement removes a role-held entry and
\* ends its role hold.
AuditWitnessReplayEndsRoleHold ==
    /\ Id2 \in identity_entry_retiring
    /\ Cause(Id2) = Some("RoleMigrationUndeclared")
    /\ A2ReplayEnd
    /\ Id2 \notin DOMAIN identity_entry_origins'
    /\ CauseNext(Id2) = None

NotAuditWitnessAdoptedThenUnlisted == [][~AuditWitnessAdoptedThenUnlisted]_vars
NotAuditWitnessUnlistedEndStopped == [][~AuditWitnessUnlistedEndStopped]_vars
NotAuditWitnessUnlistedEndRetired == [][~AuditWitnessUnlistedEndRetired]_vars
NotAuditWitnessUnlistedReplayEnd == [][~AuditWitnessUnlistedReplayEnd]_vars
NotAuditWitnessReplayKeepsUnarchived == [][~AuditWitnessReplayKeepsUnarchived]_vars
NotAuditWitnessStaleMarkIgnored == [][~AuditWitnessStaleMarkIgnored]_vars
NotAuditWitnessHostedReplayKept == [][~AuditWitnessHostedReplayKept]_vars
NotAuditWitnessRelistedAfterUnlist == [][~AuditWitnessRelistedAfterUnlist]_vars
NotAuditWitnessRevived == [][~AuditWitnessRevived]_vars
NotAuditWitnessRetiredEntryKept == [][~AuditWitnessRetiredEntryKept]_vars
NotAuditWitnessFreshAfterForget == [][~AuditWitnessFreshAfterForget]_vars
NotAuditWitnessSealRemovesUnseated == [][~AuditWitnessSealRemovesUnseated]_vars
NotAuditWitnessReplayMarked == [][~AuditWitnessReplayMarked]_vars
NotAuditWitnessListedAgainUnmarked == [][~AuditWitnessListedAgainUnmarked]_vars
NotAuditWitnessResumeRoleHeld == [][~AuditWitnessResumeRoleHeld]_vars
NotAuditWitnessResumeRoleMigrationOwed == [][~AuditWitnessResumeRoleMigrationOwed]_vars
NotAuditWitnessSeatKeepsRoleHold == [][~AuditWitnessSeatKeepsRoleHold]_vars
NotAuditWitnessCandidateRefused == [][~AuditWitnessCandidateRefused]_vars
NotAuditWitnessNameHeldByWorker == [][~AuditWitnessNameHeldByWorker]_vars
NotAuditWitnessHostAdopted == [][~AuditWitnessHostAdopted]_vars
NotAuditWitnessInFlight == [][~AuditWitnessInFlight]_vars
NotAuditWitnessResetEndsEntries == [][~AuditWitnessResetEndsEntries]_vars
NotAuditWitnessDestroyEndsEntries == [][~AuditWitnessDestroyEndsEntries]_vars
NotAuditWitnessPreflightAdmitsMembers == [][~AuditWitnessPreflightAdmitsMembers]_vars
NotAuditWitnessResetEndsRetiredEntry == [][~AuditWitnessResetEndsRetiredEntry]_vars
NotAuditWitnessSealEndsRoleHold == [][~AuditWitnessSealEndsRoleHold]_vars
NotAuditWitnessResetEndsRoleHold == [][~AuditWitnessResetEndsRoleHold]_vars
NotAuditWitnessReplayEndsRoleHold == [][~AuditWitnessReplayEndsRoleHold]_vars

(***************************************************************************)
(* The roles run.                                                          *)
(***************************************************************************)

\* Member 1's declarations: its own role or another (with the migration
\* declared from its role or not), its own mode or another, on a durable
\* session service or one that keeps none.
RolesShapes ==
    { <<p, m, "Local", FALSE, mig, FALSE, mem, None, "None">> :
        p \in {AuditProfile, AuditProfile2},
        m \in {AuditMode, AuditModeAlt},
        mig \in {None, Some(AuditProfile)},
        mem \in BOOLEAN }

\* The grant of a same-session rotation under member 1's own or its new
\* role.
RAuthorizeAs(p) ==
    AuthorizeSpawnProfileRunning(Id1, p, AuditModel, AuditDigest, AuditDigest, AuditDigest, None, None, FALSE, None)
RSeedFirstBoot == \E hp \in {{}, {Id1}} : SeedMemberResidencyRunning("OnDemand", "Authoritative", hp)
RWake == \E cause \in AuditWakeCauses : WakeArm(Id1, cause, FALSE, "Content", FALSE)

RolesNext ==
    \/ WRecover
    \/ WBind
    \/ WEndpoint
    \/ QEntry
    \/ RSeedFirstBoot
    \/ QHostPrepared
    \/ \E s \in RolesShapes : DeclareArm(Id1, s)
    \/ AuditPlanDefault(Id1)
    \/ RWake
    \/ AuditWakePath(Id1)
    \/ QReady
    \/ QRelease
    \/ \E p \in {AuditProfile, AuditProfile2} : RAuthorizeAs(p)
    \/ QBegin
    \/ QCommit
    \/ QRetire
    \/ WDetach
    \/ QArchive

RolesSpec == Init /\ [][RolesNext]_vars

AuditRolesDeclareExactlyOne == \A s \in RolesShapes : DeclareOneArm(Id1, s)

\* No wake opens an attempt for a member with a host preparation
\* cause (the run drives WakeMember and the boot plan, no explicit resume and
\* no drift revival).
AuditHostPreparationNeverWoken ==
    [][\A id \in DOMAIN member_host_preparation_causes :
         id \in DOMAIN member_wake_attempts' => id \in DOMAIN member_wake_attempts
      ]_vars

\* The host's carry-out ends a first-boot preparation, never a role hold.
AuditHostStepKeepsRoleHold ==
    [][QHostPrepared /\ Cause(Id1) = Some("RoleMigrationUndeclared")
         => member_host_preparation_causes' = member_host_preparation_causes]_vars

\* A declaration with no first-boot request never replaces a first-boot
\* preparation.
AuditRoleHoldKeepsFirstBootRow ==
    [][(Cause(Id1) = Some("FirstBootRow") /\ \E s \in RolesShapes : DeclareArm(Id1, s))
         => member_host_preparation_causes' = member_host_preparation_causes]_vars

\* Witnesses of the roles run.
\* The role hold refuses a peer's delivery wake.
AuditWitnessRoleHoldRefusesPeerWake ==
    /\ Cause(Id1) = Some("RoleMigrationUndeclared")
    /\ WakeMemberRefusedRunning(Id1, "PeerDelivery", FALSE, "Content", FALSE)
\* A declared migration restamps the member on its own session under its
\* new role.
AuditWitnessMigrationVerdict ==
    DeclareIdentityEntryRoleMigrate(Id1, AuditProfile2, AuditMode, "Local", FALSE, Some(AuditProfile), FALSE, FALSE, None, "None")
AuditWitnessMigrationRestamped ==
    /\ Profile(Id1) = Some(AuditProfile)
    /\ QCommit
    /\ ProfileNext(Id1) = Some(AuditProfile2)
    /\ BindingNext(Id1) = Binding(Id1)
\* A runtime-mode change is restamped; a memory-only session rotates.
AuditWitnessRestampVerdict ==
    DeclareIdentityEntryRuntimeModeRestamp(Id1, AuditProfile, AuditModeAlt, "Local", FALSE, None, FALSE, FALSE, None, "None")
AuditWitnessRotateVerdict ==
    DeclareIdentityEntryRoleRotate(Id1, AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, TRUE, None, "None")
\* The host restores the role: the hold ends.
AuditWitnessRoleHoldEnded ==
    /\ Cause(Id1) = Some("RoleMigrationUndeclared")
    /\ DeclareIdentityEntryKept(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None")
    /\ Id1 \notin DOMAIN member_host_preparation_causes'
\* A first-boot preparation stands over a role move, and holds the wakes.
AuditWitnessFirstBootKeptOverRoleHold ==
    /\ Cause(Id1) = Some("FirstBootRow")
    /\ DeclareIdentityEntryRoleMigrationHeld(Id1, AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None")
\* The end of a role-held member's retirement drops the hold.
AuditWitnessRoleHoldDroppedAtArchive ==
    /\ Cause(Id1) = Some("RoleMigrationUndeclared")
    /\ QArchive
    /\ Id1 \notin DOMAIN member_host_preparation_causes'

NotAuditWitnessRoleHoldRefusesPeerWake == [][~AuditWitnessRoleHoldRefusesPeerWake]_vars
NotAuditWitnessMigrationVerdict == [][~AuditWitnessMigrationVerdict]_vars
NotAuditWitnessMigrationRestamped == [][~AuditWitnessMigrationRestamped]_vars
NotAuditWitnessRestampVerdict == [][~AuditWitnessRestampVerdict]_vars
NotAuditWitnessRotateVerdict == [][~AuditWitnessRotateVerdict]_vars
NotAuditWitnessRoleHoldEnded == [][~AuditWitnessRoleHoldEnded]_vars
NotAuditWitnessFirstBootKeptOverRoleHold == [][~AuditWitnessFirstBootKeptOverRoleHold]_vars
NotAuditWitnessRoleHoldDroppedAtArchive == [][~AuditWitnessRoleHoldDroppedAtArchive]_vars

(***************************************************************************)
(* The held run.                                                           *)
(***************************************************************************)

\* Member 2's declarations: local, placed or externally bound, held or not.
HeldShapes2 ==
    { <<AuditProfile, AuditMode, pl, held, None, FALSE, FALSE, None, "None">> :
        pl \in {"Local", "Placed", "External"}, held \in BOOLEAN }
\* Member 1's declarations: local, held or not (it was created before).
HeldShapes1 == { <<AuditProfile, AuditMode, "Local", held, None, FALSE, FALSE, None, "None">> : held \in BOOLEAN }

H1Recover == RecoverRosterMemberRunning(Id1, Rid1, 1, 0, AuditProfile, AuditMode, FALSE)
H1Retired == \E started \in BOOLEAN : RecoverRosterMemberRetiredRunning(Id1, Rid1, 0, FALSE, started, TRUE, FALSE)
H2Authorize ==
    AuthorizeSpawnProfileRunning(Id2, AuditProfile, AuditModel, AuditDigest, AuditDigest, AuditDigest, None, None, FALSE, None)
H2Open(custody) ==
    BeginSpawnExecFresh(Id2, Rid2, 2, 0, AuditDigest, FALSE, AuditMode, Some(Sess2), None, None,
                        FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE,
                        None, None, None, None, FALSE, FALSE, custody, FALSE)
H2Spawn ==
    CommitSpawnMembershipFresh(Id2, Rid2, 2, 0, AuditDigest, FALSE, AuditMode, Some(Sess2), None,
                               None, None, None, None, None, FALSE)
H2Seat == \E unborn \in BOOLEAN : CommitDormantSeatRunning(Id2, Rid2, 2, 0, FALSE, AuditMode, Sess2, None, FALSE, FALSE, unborn, TRUE, FALSE)
H2Abort == AbortSpawnExecActiveRunning(Id2)
\* The declared apply's open fact: an apply opens it, a second open is
\* refused busy, and the close is total, in every phase the run reaches.
ApplyOpenArm ==
    \/ OpenDeclaredApplyOpened
    \/ OpenDeclaredApplyBusy
    \/ OpenDeclaredApplyMobNotRunningStopped
    \/ OpenDeclaredApplyMobNotRunningCompleted
    \/ OpenDeclaredApplyMobNotRunningDestroyed
ApplyCloseArm ==
    \/ CloseDeclaredApplyRunning
    \/ CloseDeclaredApplyStopped
    \/ CloseDeclaredApplyCompleted
    \/ CloseDeclaredApplyDestroyed

HeldNext ==
    \/ H1Recover
    \/ H1Retired
    \/ \E s \in HeldShapes1 : DeclareArm(Id1, s)
    \/ \E s \in HeldShapes2 : DeclareArm(Id2, s)
    \/ \E d \in SUBSET {Id1, Id2} : SealArm(d)
    \/ H2Authorize
    \/ \E custody \in BOOLEAN : H2Open(custody)
    \/ H2Spawn
    \/ H2Seat
    \/ H2Abort
    \/ AuditQuiesceStop
    \/ StopRunning
    \/ ResumeStopped
    \/ AuditQuiesceComplete
    \/ CompleteRunning
    \/ AuditQuiesceDestroy
    \/ DestroyFromAny
    \/ ApplyOpenArm
    \/ ApplyCloseArm

HeldSpec == Init /\ [][HeldNext]_vars

AuditHeldDeclareExactlyOne ==
    /\ \A s \in HeldShapes1 : DeclareOneArm(Id1, s)
    /\ \A s \in HeldShapes2 : DeclareOneArm(Id2, s)

\* Exactly one arm answers an apply's opening, in every phase: one apply of
\* the mob is open at a time.
AuditApplyOpenExactlyOne ==
    Cardinality({i \in 1..5 :
        << ENABLED OpenDeclaredApplyOpened,
           ENABLED OpenDeclaredApplyBusy,
           ENABLED OpenDeclaredApplyMobNotRunningStopped,
           ENABLED OpenDeclaredApplyMobNotRunningCompleted,
           ENABLED OpenDeclaredApplyMobNotRunningDestroyed >>[i]}) = 1

\* The dormant seat never seats an entry awaiting its held creation or
\* hosted on its first use.
AuditDormantSeatTakesOnlyOwedEntries ==
    [][H2Seat => Unseated(Id2) \in {None, Some("SeatOwed")}]_vars

\* Witnesses of the held run.
\* Held creation waits, then the held spawn (the ladder with entry custody
\* and its membership commit) seats the entry.
AuditWitnessHeldWaiting == Unseated(Id2) = Some("HeldCreation") /\ Id2 \notin DOMAIN identity_runtime_generations
AuditWitnessHeldSpawnSeats ==
    /\ Unseated(Id2) = Some("HeldCreation")
    /\ H2Spawn
    /\ Id2 \in DOMAIN identity_to_runtime'
    /\ UnseatedNext(Id2) = None
\* A placed entry is hosted on its first use; so is an externally bound one.
AuditWitnessHosted == Unseated(Id2) = Some("HostedOnFirstUse")
\* A second apply while one is open is refused busy.
AuditWitnessApplyBusy == OpenDeclaredApplyBusy
\* A held declaration of an identity created before is owed its seat.
AuditWitnessEverCreatedOwedSeat ==
    /\ Id1 \notin DOMAIN identity_to_runtime
    /\ Id1 \in DOMAIN identity_runtime_generations
    /\ DeclareIdentityEntrySeatOwed(Id1, AuditProfile, AuditMode, "Local", TRUE, None, FALSE, FALSE, None, "None")
\* The forced destroy ends a ladder whose membership committed, and the
\* startup binding request its commit recorded: no ladder or request
\* outlives the members it named.
AuditWitnessDestroyEndsCommittedLadder ==
    /\ Id2 \in DOMAIN spawn_exec_phase
    /\ spawn_exec_phase[Id2] = "MembershipCommitted"
    /\ member_startup_binding_requested # {}
    /\ DestroyFromAny
    /\ DOMAIN spawn_exec_phase' = {}
    /\ member_startup_binding_requested' = {}

NotAuditWitnessHeldWaiting == ~AuditWitnessHeldWaiting
NotAuditWitnessHeldSpawnSeats == [][~AuditWitnessHeldSpawnSeats]_vars
NotAuditWitnessHosted == ~AuditWitnessHosted
NotAuditWitnessEverCreatedOwedSeat == [][~AuditWitnessEverCreatedOwedSeat]_vars
NotAuditWitnessApplyBusy == [][~AuditWitnessApplyBusy]_vars
NotAuditWitnessDestroyEndsCommittedLadder == [][~AuditWitnessDestroyEndsCommittedLadder]_vars

(***************************************************************************)
(* The placed run.                                                         *)
(***************************************************************************)

PlacedShape1 == <<AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None">>
PlacedShape2 == <<AuditProfile, AuditMode, "Placed", FALSE, None, FALSE, FALSE, None, "None">>

POwner == BindOwnerBridgeSessionRunning(Sess3, FALSE, FALSE)
PBeginHost == BeginHostBindFreshUnplaced(Host1, "peeraddress_1", 1)
PCommitHost ==
    CommitHostBind(Host1, "peersigningkey_1", "peeraddress_1", 1, 1, 1, 4, AuditEngine,
                   TRUE, TRUE, TRUE, TRUE, TRUE, TRUE, {}, TRUE, None)
PAuthorize(id) ==
    AuthorizeSpawnProfileRunning(id, AuditProfile, AuditModel, AuditDigest, AuditDigest, AuditDigest, None, None, FALSE, Some(AuditDigest))
\* The placed spawn ladder of member id (runtime rid, fence f), with or
\* without entry custody.
POpen(id, rid, f, custody) ==
    BeginSpawnExecRemote(id, rid, f, 0, AuditDigest, FALSE, AuditMode, None, None, Some(Host1),
                         FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE,
                         None, Some(PlacedSpawn1), Some(AuditOperation), Some(Sess3), FALSE, FALSE, custody, FALSE)
PAbort(id) == AbortSpawnExecMaterializePendingRunning(id)
\* The placed member's commit (member 2, on the session its host created,
\* endpoint 2), its retirement (releasing its binding) and the placed end.
PCommit ==
    CommitSpawnMembershipRemote(Id2, Rid2, 2, 0, AuditDigest, FALSE, AuditMode, Some(Sess2), None, Some(E2Inproc),
                                Some(AuditDigest), Some(AuditEngine), Some(PlacedSpawn1), Some(AuditOperation), FALSE)
PRetire == RetireRemoteReleasingRunning(AuditMob, Rid2, Id2, 0, Some(Sess2), Some(Sess2))
PArchive == ObserveMemberRetirementArchivedPlacedLiveRunning(Id2, Rid2, 2, 0, Some(Sess2), AuditDisposal, FALSE)

PlacedNext ==
    \/ POwner
    \/ PBeginHost
    \/ PCommitHost
    \/ DeclareArm(Id1, PlacedShape1)
    \/ DeclareArm(Id2, PlacedShape2)
    \/ PAuthorize(Id1)
    \/ PAuthorize(Id2)
    \/ \E custody \in BOOLEAN : POpen(Id1, Rid1, 1, custody) \/ POpen(Id2, Rid2, 2, custody)
    \/ PAbort(Id1)
    \/ PAbort(Id2)
    \/ PCommit
    \/ PRetire
    \/ PArchive

PlacedSpec == Init /\ [][PlacedNext]_vars

AuditPlacedDeclareExactlyOne == DeclareOneArm(Id1, PlacedShape1) /\ DeclareOneArm(Id2, PlacedShape2)

\* A spawn ladder opened for an entry's name holds its custody.
AuditLadderReservesEntryName ==
    [][\A id \in Entries :
         (id \notin DOMAIN spawn_exec_phase /\ id \in DOMAIN spawn_exec_phase') => id \in spawn_exec_entry_custody'
      ]_vars

\* The remote commit of a ladder holding an entry's custody seats the entry:
\* a Declared entry, no longer unseated.
AuditRemoteCommitSeatsEntry ==
    [][(PCommit /\ Id2 \in spawn_exec_entry_custody)
         => (Id2 \in DOMAIN identity_entry_origins'
               /\ identity_entry_origins'[Id2] = "Declared"
               /\ UnseatedNext(Id2) = None)
      ]_vars

\* The placed end of a retirement the seal did not start keeps the entry
\* hosted on its first use, with no archived session recorded.
AuditPlacedEndKeepsHosted ==
    [][(PArchive /\ Id2 \in Entries /\ Id2 \notin identity_entry_retiring)
         => (UnseatedNext(Id2) = Some("HostedOnFirstUse") /\ RecordNext(Id2) = Record(Id2))
      ]_vars

\* Witnesses of the placed run.
\* The placed ladder holds the custody of an entry hosted on its first use.
AuditWitnessPlacedCustody ==
    /\ Unseated(Id2) = Some("HostedOnFirstUse")
    /\ POpen(Id2, Rid2, 2, TRUE)
\* The placed ladder of a worker name opens without custody.
AuditWitnessPlacedWorker ==
    /\ Id1 \notin Entries
    /\ POpen(Id1, Rid1, 1, FALSE)
\* The abort of a placed ladder drops the custody it held.
AuditWitnessPlacedCustodyAborted == Id2 \in spawn_exec_entry_custody /\ PAbort(Id2)
\* The remote commit seats the placed entry.
AuditWitnessPlacedEntrySeated ==
    /\ Unseated(Id2) = Some("HostedOnFirstUse")
    /\ PCommit
    /\ UnseatedNext(Id2) = None
\* The placed end keeps the entry hosted on its first use.
AuditWitnessPlacedEndHosted ==
    /\ Id2 \in Entries
    /\ PArchive
    /\ UnseatedNext(Id2) = Some("HostedOnFirstUse")

NotAuditWitnessPlacedCustody == [][~AuditWitnessPlacedCustody]_vars
NotAuditWitnessPlacedWorker == [][~AuditWitnessPlacedWorker]_vars
NotAuditWitnessPlacedCustodyAborted == [][~AuditWitnessPlacedCustodyAborted]_vars
NotAuditWitnessPlacedEntrySeated == [][~AuditWitnessPlacedEntrySeated]_vars
NotAuditWitnessPlacedEndHosted == [][~AuditWitnessPlacedEndHosted]_vars

(***************************************************************************)
(* The hold run.                                                           *)
(***************************************************************************)

\* Member 1's declarations while seated under profile 1: under profile 2
\* with no migration declared (held), with a first-boot Resume request too,
\* with the migration (migrate), on a session kept in memory only (rotate),
\* and under its own role (kept).
HoldMoved == <<AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None">>
HoldMovedResume == <<AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "Resume">>
HoldMigrate == <<AuditProfile2, AuditMode, "Local", FALSE, Some(AuditProfile), FALSE, FALSE, None, "None">>
HoldRotate == <<AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, TRUE, None, "None">>
HoldOwn == <<AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "None">>
HoldShapes == {HoldMoved, HoldMovedResume, HoldMigrate, HoldRotate, HoldOwn}

\* The rotation terminals of the run: the same-session one (a declared
\* migration's restamp, whose retirement starts in its own step) and the
\* fresh-session ones (a rotation onto a new session after its archive-only
\* retirement started, as a first-boot Fresh request's fresh life).
HoldCommit == QCommit \/ RCommitRotation

HoldNext ==
    \/ WRecover
    \/ WBind
    \/ QEntry
    \/ RSeedHostPending
    \/ AuditPlanDefault(Id1)
    \/ \E s \in HoldShapes : DeclareArm(Id1, s)
    \/ \E p \in {AuditProfile, AuditProfile2} : RAuthorizeAs(p)
    \/ QBegin
    \/ QRetire
    \/ RBeginRotation
    \/ HoldCommit

HoldSpec == Init /\ [][HoldNext]_vars

AuditHoldDeclareExactlyOne == \A s \in HoldShapes : DeclareOneArm(Id1, s)

\* A declaration under another role than the member's own keeps a role
\* hold (a role move holds the member until its terminal commits the new
\* role).
AuditRoleMoveKeepsRoleHold ==
    [][\A s \in HoldShapes :
         (DeclareArm(Id1, s) /\ Cause(Id1) = Some("RoleMigrationUndeclared") /\ Some(s[1]) # Profile(Id1))
           => CauseNext(Id1) = Some("RoleMigrationUndeclared")
      ]_vars

\* A rotation terminal ends a role hold exactly when it seats the successor
\* under another role.
AuditRoleHoldEndsAtNewRole ==
    [][(HoldCommit /\ Cause(Id1) = Some("RoleMigrationUndeclared"))
         => IF ProfileNext(Id1) # Profile(Id1)
            THEN CauseNext(Id1) = None
            ELSE CauseNext(Id1) = Some("RoleMigrationUndeclared")
      ]_vars

\* A seated member's first-boot Resume request ends its first-boot
\* preparation in the declaration's own step, so a role move with no
\* migration writes the role hold there.
AuditSeatedResumeWritesRoleHold ==
    [][(Id1 \in DOMAIN identity_to_runtime
          /\ DeclareIdentityEntryRoleMigrationHeld(Id1, AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "Resume"))
         => CauseNext(Id1) = Some("RoleMigrationUndeclared")
      ]_vars

\* Witnesses of the hold run.
\* A declared migration keeps the hold; the same-session terminal that
\* commits the new role ends it, and so does a fresh-session terminal.
AuditWitnessMigrateKeepsHold ==
    /\ Cause(Id1) = Some("RoleMigrationUndeclared")
    /\ DeclareIdentityEntryRoleMigrate(Id1, AuditProfile2, AuditMode, "Local", FALSE, Some(AuditProfile), FALSE, FALSE, None, "None")
AuditWitnessHoldEndsAtSameSessionCommit ==
    /\ Cause(Id1) = Some("RoleMigrationUndeclared")
    /\ QCommit
    /\ ProfileNext(Id1) # Profile(Id1)
    /\ CauseNext(Id1) = None
AuditWitnessHoldEndsAtFreshCommit ==
    /\ Cause(Id1) = Some("RoleMigrationUndeclared")
    /\ RCommitRotation
    /\ ProfileNext(Id1) # Profile(Id1)
    /\ CauseNext(Id1) = None
\* A rotation that keeps the role keeps the hold.
AuditWitnessHoldKeptOnSameRole ==
    /\ Cause(Id1) = Some("RoleMigrationUndeclared")
    /\ HoldCommit
    /\ ProfileNext(Id1) = Profile(Id1)
\* A seated member's Resume request ends its first-boot preparation and
\* reveals the role hold in the same step.
AuditWitnessSeatedResumeRevealsHold ==
    /\ Cause(Id1) = Some("FirstBootRow")
    /\ DeclareIdentityEntryRoleMigrationHeld(Id1, AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, FALSE, None, "Resume")
    /\ CauseNext(Id1) = Some("RoleMigrationUndeclared")

NotAuditWitnessMigrateKeepsHold == [][~AuditWitnessMigrateKeepsHold]_vars
NotAuditWitnessHoldEndsAtSameSessionCommit == [][~AuditWitnessHoldEndsAtSameSessionCommit]_vars
NotAuditWitnessHoldEndsAtFreshCommit == [][~AuditWitnessHoldEndsAtFreshCommit]_vars
NotAuditWitnessHoldKeptOnSameRole == [][~AuditWitnessHoldKeptOnSameRole]_vars
NotAuditWitnessSeatedResumeRevealsHold == [][~AuditWitnessSeatedResumeRevealsHold]_vars

(***************************************************************************)
(* The resume run.                                                         *)
(***************************************************************************)

\* Member 1 recovered Dormant and made an entry by its row, its first-boot
\* preparation owed or not, woken to Live by one operator wake or not, and
\* declared under another role with no migration (its role hold) or not, in
\* a stopped mob the explicit resume brings back: the resume rebuilds no
\* member whose host preparation is owed, Dormant or Live, and answers it
\* held, while the mob is still stopped (its candidate selection, before
\* anything prepares the member) as once it runs.
ResumeAuthorize ==
    \/ AuthorizeExplicitResumeMemberRunning(AuditResumeAttempt, Id1, AuditResumeBinding)
    \/ AuthorizeExplicitResumeMemberLiveRunning(AuditResumeAttempt, Id1, AuditResumeBinding)
    \/ AuthorizeExplicitResumeMemberJoinRunning(AuditResumeAttempt, Id1, AuditResumeBinding)
ResumeHeld == AuthorizeExplicitResumeMemberHeldRunning(AuditResumeAttempt, Id1, AuditResumeBinding)
ResumeHeldStopped == AuthorizeExplicitResumeMemberHeldStopped(AuditResumeAttempt, Id1, AuditResumeBinding)
ResumeHeldAny == ResumeHeld \/ ResumeHeldStopped

ResumeNext ==
    \/ WRecover
    \/ WBind
    \/ REndpoint
    \/ QEntry
    \/ RSeedHostPending
    \/ AuditPlan(Id1)
    \/ AuditOperatorWake(Id1)
    \/ AuditWakePath(Id1)
    \/ RReady
    \/ DeclareArm(Id1, HoldMoved)
    \/ AuditQuiesceStop
    \/ StopRunning
    \/ ResumeStopped
    \/ RResumeArms
    \/ ResumeHeld
    \/ ResumeHeldStopped

ResumeSpec == Init /\ [][ResumeNext]_vars

\* The explicit resume never rebuilds a member whose host preparation is
\* owed.
AuditResumeNeverRebuildsHeldMember ==
    [][Cause(Id1) # None => ~ResumeAuthorize]_vars

\* The resume's held answer, stopped or running, changes nothing.
AuditResumeHeldChangesNothing == [][ResumeHeldAny => StateView' = StateView]_vars

\* The resume answers held only a member whose host preparation is owed.
AuditResumeHeldOnlyWhenOwed == [][ResumeHeldAny => Cause(Id1) # None]_vars

\* Witnesses of the resume run.
AuditWitnessResumeAnswersHeld == ResumeHeld
\* A held member probed while the mob is still stopped, in an open attempt.
AuditWitnessResumeAnswersHeldStopped == ResumeHeldStopped
AuditWitnessResumeAuthorizesSettled == Cause(Id1) = None /\ ResumeAuthorize

NotAuditWitnessResumeAnswersHeld == [][~AuditWitnessResumeAnswersHeld]_vars
NotAuditWitnessResumeAnswersHeldStopped == [][~AuditWitnessResumeAnswersHeldStopped]_vars
NotAuditWitnessResumeAuthorizesSettled == [][~AuditWitnessResumeAuthorizesSettled]_vars
====
