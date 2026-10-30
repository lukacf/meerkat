---- MODULE identity_entry_apply_audit ----
\* Hand-written audit of the MobMachine declared apply: the runs apply,
\* legacy, roles, held and placed of identity_entry_audit.sh.
\*
\* The declared apply records a host's roster as identity entries
\* (DeclareIdentityEntry, one arm per verdict of identity_entry_verdict),
\* seals a complete roster (SealDeclaredRoster: an unlisted Declared entry is
\* marked while its member is seated, or removed when it is not), and keeps
\* every entry seated or unseated with a typed cause. The end of a
\* retirement the seal did not start keeps the entry unseated, owed a seat,
\* with its archived session recorded (identity_entry_archived_sessions),
\* and a later seat of the entry revives it on that session; the end of an
\* unlisted retirement removes the entry. The generated ci profile has no
\* identities, so these arms hold there only vacuously. This module keeps the
\* generated model unchanged, extends member_residency_audit for its
\* constants and arm families, and checks five runs over fixed domains:
\*
\*   apply (ApplySpec): member 1 declared local and seated Dormant by the
\*     spawn ladder holding its entry custody, retired (archive-only) to its
\*     archive terminal, unlisted or not, declared again and seated on its
\*     archived session (the revive) or, its record let go
\*     (ForgetArchivedSession), on a fresh one; the seal over every desired
\*     set, the replay of an unlisted retirement's mark, the build seed's
\*     composition authority (a candidate composition declares and seals
\*     nothing), the mob's reset and its forced destroy.
\*   legacy (LegacySpec): member 2 a legacy member recovered by replay, a
\*     worker until its adoption, then unlisted and retired; its
\*     declarations, the seal over every desired set, the replay of its
\*     unlisted retirement's mark and the build seed's composition authority.
\*   roles (RolesSpec): member 1 recovered and made an entry by the replay of
\*     its row, declared under its own role and mode, another role (with the
\*     migration declared from its role or not) or another mode, on a durable
\*     session service or one that keeps none: the role hold, its end, the
\*     restamp and the rotation verdicts; the host's first-boot preparation
\*     seeded at build and carried out; every WakeMember cause; the wake,
\*     release and the same-session rotation that carries out a declared
\*     migration (its grant under the new role, its start and its terminal);
\*     the archive-only retirement to its terminal.
\*   held (HeldSpec): member 2 never created, declared local, placed or
\*     externally bound, held or not; the held spawn (the spawn ladder with
\*     entry custody and its membership commit) and its abort; the dormant
\*     seat's refusal of a held or hosted entry; member 1 recovered and
\*     retired by replay (ever created), declared held; the seal; Stop,
\*     Resume, Complete and the forced destroy (which ends the held spawn's
\*     ladder, its membership committed or not), where the declaration and
\*     the seal refuse.
\*   placed (PlacedSpec): one member host bound and the owner bridge, member 2
\*     declared placed (hosted on its first use) and member 1 declared local
\*     (owed its seat): the placed spawn ladder opened with or without entry
\*     custody for either, and its abort.
\*
\* Each run explores its whole reachable space under a VIEW that leaves out
\* model_step_count and topology_epoch, which no guard reads, with TLC
\* coverage. Properties: every generated invariant of ci.cfg (the declared
\* apply's among them: entry_is_seated_or_has_an_unseated_cause,
\* unseated_entry_is_not_a_member, retiring_entry_is_a_declared_entry,
\* held_creation_entry_was_never_created, archived_session_record_is_unseated,
\* placed_entry_custody_is_hosted_on_first_use, entry_custody_ladder_is_open,
\* destroyed_mob_holds_no_member_lifecycle), and per run:
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
\*     custody, an observed adoption or the replay of its row);
\*     AuditSeatTakesArchivedSession (action property: a seat holding an
\*     entry's custody binds the identity's archived session when it has a
\*     record).
\*   roles: AuditRolesDeclareExactlyOne; AuditHostPreparationNeverWoken
\*     (action property: no wake opens an attempt for a member with a host
\*     preparation cause; the run drives WakeMember and the boot plan, and no
\*     explicit resume or drift revival, which are outside the hold);
\*     AuditHostStepKeepsRoleHold (the host's carry-out never ends a role
\*     hold); AuditRoleHoldKeepsFirstBootRow (a declaration never replaces a
\*     first-boot preparation).
\*   held: AuditHeldDeclareExactlyOne and AuditSealExactlyOne (in the
\*     Stopped, Completed and Destroyed phases too);
\*     AuditDormantSeatTakesOnlyOwedEntries
\*     (action property: the dormant seat never seats an entry awaiting its
\*     held creation or hosted on its first use).
\*   placed: AuditPlacedDeclareExactlyOne; AuditLadderReservesEntryName (action
\*     property: a spawn ladder opened for an entry's name holds its
\*     custody).
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations. With --mutants the runner also seeds defects into a
\* copy of the generated model and requires TLC to refuse each (see
\* identity_entry_audit.sh). No origin other than Declared exists yet, so the
\* declared apply's explicit origin check has no refusal to reach here; the
\* release that adds an origin adds its row and mutant.
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
\* <<profile, mode, placement, held, migration, intent managed, memory only>>,
\* in every lifecycle phase.
DeclareArm(id, s) ==
    \/ DeclareIdentityEntryKept(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntrySeatOwed(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryHostedOnFirstUse(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryAwaitHeldCreation(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryRuntimeModeRestamp(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryRoleMigrate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryRoleRotate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryRoleMigrationHeld(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryLifecycleInFlight(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryRefusedNameHeldByWorker(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryRefusedHostAdopted(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryRefusedNonAuthoritative(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryMobNotRunningStopped(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryMobNotRunningCompleted(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])
    \/ DeclareIdentityEntryMobNotRunningDestroyed(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7])

DeclareEnabled(id, s) ==
    << ENABLED DeclareIdentityEntryKept(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntrySeatOwed(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryHostedOnFirstUse(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryAwaitHeldCreation(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryRuntimeModeRestamp(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryRoleMigrate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryRoleRotate(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryRoleMigrationHeld(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryLifecycleInFlight(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryRefusedNameHeldByWorker(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryRefusedHostAdopted(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryRefusedNonAuthoritative(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryMobNotRunningStopped(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryMobNotRunningCompleted(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]),
       ENABLED DeclareIdentityEntryMobNotRunningDestroyed(id, s[1], s[2], s[3], s[4], s[5], s[6], s[7]) >>

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

(***************************************************************************)
(* The apply run.                                                          *)
(***************************************************************************)

\* Member 1's declarations: local, its own profile and mode, neither held nor
\* migrating, on a durable session service; managed by no intent row, or
\* by one (refused HostAdopted).
ApplyShape == <<AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE>>
ApplyShapes == {ApplyShape, <<AuditProfile, AuditMode, "Local", FALSE, None, TRUE, FALSE>>}

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
                        None, None, None, None, FALSE, FALSE, TRUE)
ASeatAs(inc, archived, unborn, fresh) ==
    CommitDormantSeatRunning(Id1, inc[1], inc[2], inc[3], FALSE, AuditMode, inc[4], None, archived, FALSE, unborn, fresh, FALSE)
ASeat(inc) == \E archived \in BOOLEAN, unborn \in BOOLEAN, fresh \in BOOLEAN : ASeatAs(inc, archived, unborn, fresh)
AAbort == AbortSpawnExecActiveRunning(Id1)
ARetire(inc) ==
    \/ RetireRunningReleasingArchiveOnly(AuditMob, inc[1], Id1, inc[3], Some(inc[4]), Some(inc[4]))
    \/ RetireRunningPreservingBindingArchiveOnly(AuditMob, inc[1], Id1, inc[3], None, Some(inc[4]))
AArchive(inc) == ObserveMemberRetirementArchivedLive(Id1, inc[1], inc[2], inc[3], Some(inc[4]), AuditDisposal, FALSE)
ADetach(rid) == SessionIngressDetachedForMobDestroyRunning(AuditMob, rid)
AForget ==
    \E s \in {Sess1, Sess3}, reason \in {"SessionAbsent", "FreshLifeRequested"} : ForgetArchivedSession(Id1, s, reason)
AMark == \E rid \in {Rid1, Rid3} : RecoverIdentityEntryRetiringMarkedRunning(Id1, rid)

ASeed == \E authority \in {"Authoritative", "NonAuthoritative"} : SeedMemberResidencyRunning("OnDemand", authority, {})
AQuiesceReset == BeginPlacedCompletionLifecycleQuiesceFresh("Reset")

\* Member 2 (the legacy run): a legacy member the replay recovered, a
\* worker until adopted.
A2Recover == RecoverRosterMemberRunning(Id2, Rid2, 2, 0, AuditProfile, AuditMode, FALSE)
A2Bind == RecoverMemberSessionBindingFreshRunning(Id2, Rid2, Sess2, None)
A2Dormant == PlanBootMaterializationStayDormant(Id2, "None", FALSE, FALSE)
A2Adopt == \E observed \in BOOLEAN : AdoptArm(Id2, 0, observed)
A2Retire ==
    \/ RetireRunningReleasingArchiveOnly(AuditMob, Rid2, Id2, 0, Some(Sess2), Some(Sess2))
    \/ RetireRunningPreservingBindingArchiveOnly(AuditMob, Rid2, Id2, 0, None, Some(Sess2))
A2Archive == ObserveMemberRetirementArchivedLive(Id2, Rid2, 2, 0, Some(Sess2), AuditDisposal, FALSE)
A2Mark == RecoverIdentityEntryRetiringMarkedRunning(Id2, Rid2)

\* The declaration, seal, record release and adoption steps of both runs.
ApplyIdentitySteps ==
    \/ \E id \in {Id1, Id2}, s \in ApplyShapes : DeclareArm(id, s)
    \/ \E d \in SUBSET {Id1, Id2} : SealArm(d)
    \/ AForget
    \/ A2Adopt

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
    \/ AQuiesceReset
    \/ ResetToRunning
    \/ AuditQuiesceDestroy
    \/ DestroyFromAny

ApplySpec == Init /\ [][ApplyNext]_vars

\* The legacy run: member 2 recovered by replay, a worker until its
\* adoption, then unlisted and retired; its declarations and the seal.
LegacyNext ==
    \/ ASeed
    \/ \E s \in ApplyShapes : DeclareArm(Id2, s)
    \/ \E d \in SUBSET {Id2} : SealArm(d)
    \/ A2Recover
    \/ A2Bind
    \/ A2Dormant
    \/ A2Adopt
    \/ A2Retire
    \/ A2Archive
    \/ ADetach(Rid2)
    \/ A2Mark
    \/ SealBootMaterializationPlan

LegacySpec == Init /\ [][LegacyNext]_vars

\* Exactly one arm answers every declaration and every seal, in every phase.
AuditDeclareExactlyOne == \A id \in {Id1, Id2}, s \in ApplyShapes : DeclareOneArm(id, s)
AuditSealExactlyOne == \A d \in SUBSET {Id1, Id2} : SealOneArm(d)

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

\* Witnesses of the apply and legacy runs (the legacy run's name member 2).
\* A legacy member adopted, then unlisted by the seal and retired: the end
\* of its retirement removes the entry.
AuditWitnessAdoptedThenUnlisted ==
    /\ Id2 \in identity_entry_retiring
    /\ A2Archive
    /\ Id2 \notin DOMAIN identity_entry_origins'
\* An entry removed by its unlisted retirement is declared again: it is owed
\* its seat on its archived session.
AuditWitnessRelistedAfterUnlist ==
    /\ Id1 \notin Entries
    /\ Record(Id1) = Some(Sess1)
    /\ DeclareIdentityEntrySeatOwed(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE)
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
\* The record let go, the entry is seated on a fresh session.
AuditWitnessFreshAfterForget ==
    /\ Id1 \in Entries
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
\* Refusals: a candidate composition, a name a worker holds, an intent row,
\* and a declaration while the member's ladder is open.
AuditWitnessCandidateRefused == DeclareIdentityEntryRefusedNonAuthoritative(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE)
AuditWitnessNameHeldByWorker == DeclareIdentityEntryRefusedNameHeldByWorker(Id2, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE)
AuditWitnessHostAdopted == DeclareIdentityEntryRefusedHostAdopted(Id1, AuditProfile, AuditMode, "Local", FALSE, None, TRUE, FALSE)
AuditWitnessInFlight == DeclareIdentityEntryLifecycleInFlight(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE)
\* A mob reset and a destroy with entries present.
AuditWitnessResetEndsEntries == Entries # {} /\ ResetToRunning
AuditWitnessDestroyEndsEntries == Entries # {} /\ DestroyFromAny

NotAuditWitnessAdoptedThenUnlisted == [][~AuditWitnessAdoptedThenUnlisted]_vars
NotAuditWitnessRelistedAfterUnlist == [][~AuditWitnessRelistedAfterUnlist]_vars
NotAuditWitnessRevived == [][~AuditWitnessRevived]_vars
NotAuditWitnessRetiredEntryKept == [][~AuditWitnessRetiredEntryKept]_vars
NotAuditWitnessFreshAfterForget == [][~AuditWitnessFreshAfterForget]_vars
NotAuditWitnessSealRemovesUnseated == [][~AuditWitnessSealRemovesUnseated]_vars
NotAuditWitnessReplayMarked == [][~AuditWitnessReplayMarked]_vars
NotAuditWitnessCandidateRefused == [][~AuditWitnessCandidateRefused]_vars
NotAuditWitnessNameHeldByWorker == [][~AuditWitnessNameHeldByWorker]_vars
NotAuditWitnessHostAdopted == [][~AuditWitnessHostAdopted]_vars
NotAuditWitnessInFlight == [][~AuditWitnessInFlight]_vars
NotAuditWitnessResetEndsEntries == [][~AuditWitnessResetEndsEntries]_vars
NotAuditWitnessDestroyEndsEntries == [][~AuditWitnessDestroyEndsEntries]_vars

(***************************************************************************)
(* The roles run.                                                          *)
(***************************************************************************)

\* Member 1's declarations: its own role or another (with the migration
\* declared from its role or not), its own mode or another, on a durable
\* session service or one that keeps none.
RolesShapes ==
    { <<p, m, "Local", FALSE, mig, FALSE, mem>> :
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

\* A declaration never replaces a first-boot preparation.
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
    DeclareIdentityEntryRoleMigrate(Id1, AuditProfile2, AuditMode, "Local", FALSE, Some(AuditProfile), FALSE, FALSE)
AuditWitnessMigrationRestamped ==
    /\ Profile(Id1) = Some(AuditProfile)
    /\ QCommit
    /\ ProfileNext(Id1) = Some(AuditProfile2)
    /\ BindingNext(Id1) = Binding(Id1)
\* A runtime-mode change is restamped; a memory-only session rotates.
AuditWitnessRestampVerdict ==
    DeclareIdentityEntryRuntimeModeRestamp(Id1, AuditProfile, AuditModeAlt, "Local", FALSE, None, FALSE, FALSE)
AuditWitnessRotateVerdict ==
    DeclareIdentityEntryRoleRotate(Id1, AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, TRUE)
\* The host restores the role: the hold ends.
AuditWitnessRoleHoldEnded ==
    /\ Cause(Id1) = Some("RoleMigrationUndeclared")
    /\ DeclareIdentityEntryKept(Id1, AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE)
    /\ Id1 \notin DOMAIN member_host_preparation_causes'
\* A first-boot preparation stands over a role move, and holds the wakes.
AuditWitnessFirstBootKeptOverRoleHold ==
    /\ Cause(Id1) = Some("FirstBootRow")
    /\ DeclareIdentityEntryRoleMigrationHeld(Id1, AuditProfile2, AuditMode, "Local", FALSE, None, FALSE, FALSE)
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
    { <<AuditProfile, AuditMode, pl, held, None, FALSE, FALSE>> :
        pl \in {"Local", "Placed", "External"}, held \in BOOLEAN }
\* Member 1's declarations: local, held or not (it was created before).
HeldShapes1 == { <<AuditProfile, AuditMode, "Local", held, None, FALSE, FALSE>> : held \in BOOLEAN }

H1Recover == RecoverRosterMemberRunning(Id1, Rid1, 1, 0, AuditProfile, AuditMode, FALSE)
H1Retired == \E started \in BOOLEAN : RecoverRosterMemberRetiredRunning(Id1, Rid1, 0, FALSE, started)
H2Authorize ==
    AuthorizeSpawnProfileRunning(Id2, AuditProfile, AuditModel, AuditDigest, AuditDigest, AuditDigest, None, None, FALSE, None)
H2Open(custody) ==
    BeginSpawnExecFresh(Id2, Rid2, 2, 0, AuditDigest, FALSE, AuditMode, Some(Sess2), None, None,
                        FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE,
                        None, None, None, None, FALSE, FALSE, custody)
H2Spawn ==
    CommitSpawnMembershipFresh(Id2, Rid2, 2, 0, AuditDigest, FALSE, AuditMode, Some(Sess2), None,
                               None, None, None, None, None, FALSE)
H2Seat == \E unborn \in BOOLEAN : CommitDormantSeatRunning(Id2, Rid2, 2, 0, FALSE, AuditMode, Sess2, None, FALSE, FALSE, unborn, TRUE, FALSE)
H2Abort == AbortSpawnExecActiveRunning(Id2)

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

HeldSpec == Init /\ [][HeldNext]_vars

AuditHeldDeclareExactlyOne ==
    /\ \A s \in HeldShapes1 : DeclareOneArm(Id1, s)
    /\ \A s \in HeldShapes2 : DeclareOneArm(Id2, s)

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
\* A held declaration of an identity created before is owed its seat.
AuditWitnessEverCreatedOwedSeat ==
    /\ Id1 \notin DOMAIN identity_to_runtime
    /\ Id1 \in DOMAIN identity_runtime_generations
    /\ DeclareIdentityEntrySeatOwed(Id1, AuditProfile, AuditMode, "Local", TRUE, None, FALSE, FALSE)
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
NotAuditWitnessDestroyEndsCommittedLadder == [][~AuditWitnessDestroyEndsCommittedLadder]_vars

(***************************************************************************)
(* The placed run.                                                         *)
(***************************************************************************)

PlacedShape1 == <<AuditProfile, AuditMode, "Local", FALSE, None, FALSE, FALSE>>
PlacedShape2 == <<AuditProfile, AuditMode, "Placed", FALSE, None, FALSE, FALSE>>

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
                         None, Some(PlacedSpawn1), Some(AuditOperation), Some(Sess3), FALSE, FALSE, custody)
PAbort(id) == AbortSpawnExecMaterializePendingRunning(id)

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

PlacedSpec == Init /\ [][PlacedNext]_vars

AuditPlacedDeclareExactlyOne == DeclareOneArm(Id1, PlacedShape1) /\ DeclareOneArm(Id2, PlacedShape2)

\* A spawn ladder opened for an entry's name holds its custody.
AuditLadderReservesEntryName ==
    [][\A id \in Entries :
         (id \notin DOMAIN spawn_exec_phase /\ id \in DOMAIN spawn_exec_phase') => id \in spawn_exec_entry_custody'
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

NotAuditWitnessPlacedCustody == [][~AuditWitnessPlacedCustody]_vars
NotAuditWitnessPlacedWorker == [][~AuditWitnessPlacedWorker]_vars
NotAuditWitnessPlacedCustodyAborted == [][~AuditWitnessPlacedCustodyAborted]_vars
====
