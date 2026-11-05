---- MODULE identity_entry_audit ----
\* Hand-written audit of MobMachine identity entries.
\*
\* An identity entry is an identity the host declared as its own durable
\* identity (identity_entry_origins); a member whose identity has no entry is
\* a worker. The generated ci profile has no identities, so the entry arms and
\* the entry invariants hold there only vacuously. This audit keeps the
\* generated model unchanged and drives, over three identities, two runtime
\* ids and two sessions:
\*
\*   identity 1: a member recovered by replay (its roster row and session
\*     binding), which the replay of an entry row (RecoverIdentityEntry) or
\*     the legacy adoption (AdoptLegacyIdentityEntry) can make an entry, and
\*     the replay of its retirement unseats (an entry stays, unseated and
\*     owed its seat);
\*   identity 2: a member seated by the spawn ladder (AuthorizeSpawnProfile,
\*     BeginSpawnExecFresh with or without entry custody, CommitSpawnMembership
\*     or AbortSpawnExec), and unseated by the replay of its retirement;
\*   identity 3: a name no member holds.
\*
\* Over them: the worker reconcile (Reconcile* in Running and Stopped, every
\* desired set, with and without retire_stale) and ensure (EnsureMember*),
\* the admission probe (ProbeMemberAdmission*, with and without entry
\* custody), the agent lane's member admission
\* (ResolveAgentLaneMemberAdmission*), every arm of the legacy adoption for
\* both generations and all sixteen combinations of the shell's raw
\* observations (the durable-identity marker, the own-identity label,
\* whether the roster lists the identity, whether a host's intent row names
\* it), the replay of an entry row for every identity and runtime id, the
\* build seed's composition authority, the seal of the boot plan, Stop and
\* Resume. Identity 1 is bound to no session until its binding's replay, so
\* the adoption meets a member bound to a session and one bound to none.
\*
\* The runner checks this space in two runs that split the adoption's
\* observations by the roster's listing (AuditListings): the listed run
\* drives the eight combinations of a listed identity, the unlisted run the
\* eight of an unlisted one, and each checks AuditAdoptionExactlyOne and
\* AuditRetireUnadoptedQualifies over its own eight. Under the VIEW every
\* adoption arm but the two that record an entry is a self-loop, and those
\* two (Adopted, AdoptedUnlisted) make the same update under the same guards
\* but their observation conjunct, so each run reaches every state of the
\* single run over all sixteen, and every adoption step is a step of the run
\* its listing belongs to: the two runs check every state and step the
\* single run checked (the runner checks that shape of the generated model).
\*
\* Each run explores its whole reachable space under a VIEW that leaves out
\* model_step_count and topology_epoch, which no guard reads, with TLC
\* coverage. Properties:
\*   every generated invariant of ci.cfg (kept), among them the entry ones:
\*     entry_is_seated_or_has_an_unseated_cause (an entry is seated, or
\*     unseated with a typed cause), unseated_entry_is_not_a_member,
\*     placed_entry_custody_is_hosted_on_first_use,
\*     worker_reconcile_spares_entries and entry_custody_ladder_is_open;
\*   AuditProbeExactlyOne, AuditAgentLaneExactlyOne and
\*     AuditAdoptionExactlyOne (invariants, ENABLED form): exactly one arm of
\*     the admission probe, of the agent lane's admission and of the legacy
\*     adoption answers every input, in every lifecycle phase of the run;
\*   AuditProbeRefusesEntryName (invariant): an entry's name, probed without
\*     entry custody, answers IdentityMemberRejected;
\*   AuditEntryOnlyWithCause (action property): an identity becomes an entry
\*     only by the declared apply (which records its unseated cause), the
\*     commit of a ladder that holds its entry custody, an adoption from
\*     qualifying legacy marks, or the replay of its row on its seated
\*     incarnation;
\*   AuditAdoptionNeedsObservation (action property): an adoption records an
\*     entry only from qualifying legacy marks (either mark for a listed
\*     identity; both, and no host's intent row, for an unlisted one);
\*   AuditRetireUnadoptedQualifies (action property): RetireUnadopted answers
\*     only an unlisted member with both marks and no intent row that is
\*     placed on a member host or bound to no session, and not retiring.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous. With --mutants the
\* runner also seeds six defects into a copy of the generated model and
\* requires TLC to refuse each: a reconcile that retires an entry, an
\* admission probe that answers an entry's name without custody as a plain
\* duplicate (the name reservation; the spawn ladder itself cannot open on a
\* seated entry's name, whose binding its current_binding_absent guard
\* already refuses), a listed adoption without its legacy marks, an unlisted
\* adoption that takes one mark (refused by AuditAdoptionNeedsObservation
\* alone), a RetireUnadopted that ignores the intent row (refused by
\* AuditRetireUnadoptedQualifies alone), and a replay that records an entry
\* on an unseated identity.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* identity_entry_audit.sh.
EXTENDS model

\* The positions of model_step_count and topology_epoch in the generated
\* `vars` tuple; the runner reads them from model.tla.
CONSTANTS AuditStepIndex, AuditEpochIndex

\* The values of the roster's listing (the adoption's `listed` observation)
\* this run drives the legacy adoption with: {TRUE} in the listed run and
\* {FALSE} in the unlisted run, which together drive all sixteen
\* combinations of the shell's raw observations (see the header).
CONSTANTS AuditListings

Id1 == "agentidentity_1"
Id2 == "agentidentity_2"
Id3 == "agentidentity_3"
Rid1 == "agentruntimeid_1"
Rid2 == "agentruntimeid_2"
Sess1 == "sessionid_1"
Sess2 == "sessionid_2"
AuditProfile == "profile_1"
AuditModel == "model_1"
AuditDigest == "digest_1"
AuditMode == "TurnDriven"
AuditOrigin == "Declared"

AuditIds == {Id1, Id2, Id3}
AuditRids == {Rid1, Rid2}
AuditGenerations == {0, 1}

(***************************************************************************)
(* Arm families.                                                           *)
(***************************************************************************)

\* Identity 1: its replayed roster row and session binding.
R1Recover == RecoverRosterMemberRunning(Id1, Rid1, 1, 0, AuditProfile, AuditMode, FALSE)
R1Bind == RecoverMemberSessionBindingFreshRunning(Id1, Rid1, Sess1, None)

\* The replay of a retirement row: the identity is unseated.
RetiredReplay(id, rid) ==
    \/ RecoverRosterMemberRetiredRunning(id, rid, 0, FALSE, FALSE, TRUE, FALSE)
    \/ RecoverRosterMemberRetiredCurrentAlreadyAbsent(id, rid, 0, FALSE, FALSE, TRUE, FALSE)

\* Identity 2's spawn ladder, opened with or without entry custody.
L2Authorize ==
    AuthorizeSpawnProfileRunning(Id2, AuditProfile, AuditModel, AuditDigest, AuditDigest, AuditDigest,
                                 None, None, FALSE, None)
L2Open(custody) ==
    BeginSpawnExecFresh(Id2, Rid2, 2, 0, AuditDigest, FALSE, AuditMode, Some(Sess2), None, None,
                        FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE,
                        None, None, None, None, FALSE, FALSE, custody, FALSE)
L2Commit ==
    CommitSpawnMembershipFresh(Id2, Rid2, 2, 0, AuditDigest, FALSE, AuditMode, Some(Sess2), None,
                               None, None, None, None, None, FALSE)
L2Abort == AbortSpawnExecActiveRunning(Id2)

\* The replay of an entry row, in a running or a stopped mob.
ReplayArm(id, rid) ==
    \/ RecoverIdentityEntrySeatedRunning(id, rid, AuditOrigin)
    \/ RecoverIdentityEntrySeatedStopped(id, rid, AuditOrigin)

\* Every arm of the legacy adoption, over the shell's raw observations: the
\* spawn row's durable-identity marker (m) and own-identity label (l),
\* whether the roster lists the identity (li) and whether a host's intent
\* row names it (i).
AdoptArm(id, g, m, l, li, i) ==
    \/ AdoptLegacyIdentityEntryAdopted(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryAdoptedUnlisted(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryNameHeldByWorker(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryRetireUnadopted(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryNotAdoptable(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryStaleObservation(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryNotSeated(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryAlreadyAnEntry(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryOtherCustody(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryNonAuthoritative(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryMobNotRunningStopped(id, g, m, l, li, i)

\* The arms that record the member as an entry.
AdoptRecordArm(id, g, m, l, li, i) ==
    \/ AdoptLegacyIdentityEntryAdopted(id, g, m, l, li, i)
    \/ AdoptLegacyIdentityEntryAdoptedUnlisted(id, g, m, l, li, i)

\* The marks that make a member a legacy member: either one for an identity
\* the roster lists; both, and no host's intent row, for one it does not.
AuditLegacyMarks(m, l, li, i) == IF li THEN m \/ l ELSE m /\ l /\ ~i

\* The member's incarnation is retiring.
AuditRetiring(id) ==
    /\ id \in DOMAIN identity_to_runtime
    /\ identity_to_runtime[id] \in DOMAIN member_state_markers
    /\ member_state_markers[identity_to_runtime[id]] = "Retiring"

\* Every arm of the admission probe in the run's phases (the run's spawns
\* carry no created custody).
ProbeArm(id, custody) ==
    \/ ProbeMemberAdmissionDuplicateRunning(id, custody, FALSE)
    \/ ProbeMemberAdmissionAdmittedRunning(id, custody, FALSE)
    \/ ProbeMemberAdmissionIdentityMemberRunning(id, custody, FALSE)
    \/ ProbeMemberAdmissionDuplicateStopped(id, custody, FALSE)
    \/ ProbeMemberAdmissionAdmittedStopped(id, custody, FALSE)
    \/ ProbeMemberAdmissionIdentityMemberStopped(id, custody, FALSE)

\* Every arm of the agent lane's member admission in the run's phases.
LaneArm(id) ==
    \/ ResolveAgentLaneMemberAdmissionAdmittedRunning(id)
    \/ ResolveAgentLaneMemberAdmissionIdentityMemberRunning(id)
    \/ ResolveAgentLaneMemberAdmissionAdmittedStopped(id)
    \/ ResolveAgentLaneMemberAdmissionIdentityMemberStopped(id)

\* The worker reconcile and ensure.
ReconcileArm(desired, stale) ==
    \/ ReconcileRunning(desired, stale)
    \/ ReconcileStopped(desired, stale)
EnsureArm(id) ==
    \/ EnsureMemberRunningExisting(id)
    \/ EnsureMemberRunningMissing(id)

AuditSeed ==
    \E authority \in {"Authoritative", "NonAuthoritative"} :
        SeedMemberResidencyRunning("OnDemand", authority, {})

AuditQuiesceStop == BeginPlacedCompletionLifecycleQuiesceFreshStop("Stop")

AuditNext ==
    \/ R1Recover
    \/ R1Bind
    \/ \E id \in {Id1, Id2}, rid \in AuditRids : RetiredReplay(id, rid)
    \/ L2Authorize
    \/ \E custody \in BOOLEAN : L2Open(custody)
    \/ L2Commit
    \/ L2Abort
    \/ \E id \in AuditIds, rid \in AuditRids : ReplayArm(id, rid)
    \/ \E id \in AuditIds, g \in AuditGenerations, m, l, i \in BOOLEAN, li \in AuditListings :
           AdoptArm(id, g, m, l, li, i)
    \/ \E id \in AuditIds, custody \in BOOLEAN : ProbeArm(id, custody)
    \/ \E id \in AuditIds : LaneArm(id)
    \/ \E desired \in SUBSET AuditIds, stale \in BOOLEAN : ReconcileArm(desired, stale)
    \/ \E id \in AuditIds : EnsureArm(id)
    \/ AuditSeed
    \/ SealBootMaterializationPlan
    \/ AuditQuiesceStop
    \/ StopRunning
    \/ ResumeStopped

AuditSpec == Init /\ [][AuditNext]_vars

\* The space is finite under the VIEW: no further bound.
AuditStateConstraint == TRUE

\* Every state variable but the two counters (see the header).
AuditView == [vars EXCEPT ![AuditStepIndex] = 0, ![AuditEpochIndex] = 0]

(***************************************************************************)
(* Properties.                                                             *)
(***************************************************************************)

Entries == DOMAIN identity_entry_origins
NewEntries == DOMAIN identity_entry_origins' \ DOMAIN identity_entry_origins

AuditProbeVerdicts(id, custody) ==
    <<ENABLED ProbeMemberAdmissionDuplicateRunning(id, custody, FALSE),
      ENABLED ProbeMemberAdmissionAdmittedRunning(id, custody, FALSE),
      ENABLED ProbeMemberAdmissionIdentityMemberRunning(id, custody, FALSE),
      ENABLED ProbeMemberAdmissionDuplicateStopped(id, custody, FALSE),
      ENABLED ProbeMemberAdmissionAdmittedStopped(id, custody, FALSE),
      ENABLED ProbeMemberAdmissionIdentityMemberStopped(id, custody, FALSE)>>

\* The admission probe is total and its arms never overlap, in Running and
\* Stopped, for every identity and both entry custody flags of a spawn with
\* no created custody. (The run records no created entry and reserves no
\* name, so the other-origin and reserved-name arms answer nothing here;
\* created_identity_audit checks the probe with both custodies.)
AuditProbeExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in AuditIds, custody \in BOOLEAN :
            Cardinality({i \in 1..6 : AuditProbeVerdicts(id, custody)[i]}) = 1

\* An entry's name probed without entry custody is refused as an entry's.
AuditProbeRefusesEntryName ==
    phase = "Running" =>
        \A id \in Entries : ENABLED ProbeMemberAdmissionIdentityMemberRunning(id, FALSE, FALSE)

AuditLaneVerdicts(id) ==
    <<ENABLED ResolveAgentLaneMemberAdmissionAdmittedRunning(id),
      ENABLED ResolveAgentLaneMemberAdmissionIdentityMemberRunning(id),
      ENABLED ResolveAgentLaneMemberAdmissionAdmittedStopped(id),
      ENABLED ResolveAgentLaneMemberAdmissionIdentityMemberStopped(id),
      ENABLED ResolveAgentLaneMemberAdmissionUnreadableNameRunning(id),
      ENABLED ResolveAgentLaneMemberAdmissionUnreadableNameStopped(id)>>

\* The agent lane's member admission is total and its arms never overlap.
AuditAgentLaneExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in AuditIds : Cardinality({i \in 1..6 : AuditLaneVerdicts(id)[i]}) = 1

AuditAdoptionVerdicts(id, g, m, l, li, i) ==
    <<ENABLED AdoptLegacyIdentityEntryAdopted(id, g, m, l, li, i),
      ENABLED AdoptLegacyIdentityEntryAdoptedUnlisted(id, g, m, l, li, i),
      ENABLED AdoptLegacyIdentityEntryNameHeldByWorker(id, g, m, l, li, i),
      ENABLED AdoptLegacyIdentityEntryRetireUnadopted(id, g, m, l, li, i),
      ENABLED AdoptLegacyIdentityEntryNotAdoptable(id, g, m, l, li, i),
      ENABLED AdoptLegacyIdentityEntryStaleObservation(id, g, m, l, li, i),
      ENABLED AdoptLegacyIdentityEntryNotSeated(id, g, m, l, li, i),
      ENABLED AdoptLegacyIdentityEntryAlreadyAnEntry(id, g, m, l, li, i),
      ENABLED AdoptLegacyIdentityEntryOtherCustody(id, g, m, l, li, i),
      ENABLED AdoptLegacyIdentityEntryNonAuthoritative(id, g, m, l, li, i),
      ENABLED AdoptLegacyIdentityEntryMobNotRunningStopped(id, g, m, l, li, i)>>

\* The legacy adoption is total and its arms never overlap, for every one of
\* the run's eight observations (the sixteen over both runs).
AuditAdoptionExactlyOne ==
    phase \in {"Running", "Stopped"} =>
        \A id \in AuditIds, g \in AuditGenerations, m, l, i \in BOOLEAN, li \in AuditListings :
            Cardinality({k \in 1..11 : AuditAdoptionVerdicts(id, g, m, l, li, i)[k]}) = 1

\* An identity becomes an entry only by the declared apply (which records the
\* entry with its unseated cause), the commit of a ladder that holds its
\* entry custody, an adoption from qualifying legacy marks, or the replay of
\* its row on its seated incarnation.
AuditEntryOnlyWithCause ==
    [][\A id \in NewEntries :
         \/ id \in DOMAIN identity_entry_unseated'
         \/ id \in spawn_exec_entry_custody
         \/ \E g \in AuditGenerations, m, l, li, i \in BOOLEAN :
                AuditLegacyMarks(m, l, li, i) /\ AdoptRecordArm(id, g, m, l, li, i)
         \/ \E rid \in AuditRids : ReplayArm(id, rid)
      ]_vars

\* An adoption records an entry only from qualifying legacy marks: no step
\* that records an entry is an arm of the adoption answering marks that do
\* not qualify.
AuditAdoptionNeedsObservation ==
    [][\A id \in NewEntries :
         ~\E g \in AuditGenerations, m, l, li, i \in BOOLEAN :
              AdoptArm(id, g, m, l, li, i) /\ ~AuditLegacyMarks(m, l, li, i)
      ]_vars

\* RetireUnadopted answers only an unlisted member with both legacy marks and
\* no host's intent row naming it, placed on a member host or bound to no
\* session, and not retiring: the one member the complete roster's apply
\* retires without adopting it. It reads the run's eight observations: a step
\* that answers another observation is a step of the other run, which checks
\* it.
AuditRetireUnadoptedQualifies ==
    [][\A id \in AuditIds, g \in AuditGenerations, m, l, i \in BOOLEAN, li \in AuditListings :
         AdoptLegacyIdentityEntryRetireUnadopted(id, g, m, l, li, i)
           => /\ ~li /\ m /\ l /\ ~i
              /\ (id \in DOMAIN member_placement \/ id \notin DOMAIN member_session_bindings)
              /\ ~AuditRetiring(id)
      ]_vars

(***************************************************************************)
(* Reachability goals, each checked as an expected violation of its        *)
(* negation: a state goal joins the invariants, a step goal the properties.*)
(***************************************************************************)

\* A ladder opened with entry custody commits its member as an entry.
AuditWitnessCustodyCommit == L2Commit /\ Id2 \in NewEntries
\* A ladder opened with entry custody is aborted, and its custody with it.
AuditWitnessCustodyAborted == L2Abort /\ Id2 \in spawn_exec_entry_custody
\* The replay of a row records an entry, in Running and in Stopped.
AuditWitnessReplayed == \E rid \in AuditRids : RecoverIdentityEntrySeatedRunning(Id1, rid, AuditOrigin) /\ Id1 \in NewEntries
AuditWitnessReplayedStopped == \E rid \in AuditRids : RecoverIdentityEntrySeatedStopped(Id1, rid, AuditOrigin) /\ Id1 \in NewEntries
\* An observed adoption of a listed member records an entry.
AuditWitnessAdopted == \E g \in AuditGenerations : AdoptLegacyIdentityEntryAdopted(Id1, g, TRUE, TRUE, TRUE, FALSE) /\ Id1 \in NewEntries
\* A reconcile that retires stale workers runs while an entry it does not
\* desire is seated, and retires a worker beside it.
AuditWitnessReconcileSparesEntry ==
    /\ \E desired \in SUBSET AuditIds : ReconcileRunning(desired, TRUE) /\ \E id \in Entries : id \notin desired
    /\ members_to_retire' # {}
\* A reconcile that desires an entry and an unseated name spawns only the name.
AuditWitnessReconcileSpawnsWorkerOnly ==
    /\ \E desired \in SUBSET AuditIds : ReconcileRunning(desired, FALSE) /\ \E id \in Entries : id \in desired
    /\ members_to_spawn' # {}
\* The retirement replay unseats an entry, which keeps the entry, unseated
\* and owed its seat.
AuditWitnessUnseated ==
    \E id \in Entries :
        /\ id \notin DOMAIN identity_to_runtime'
        /\ id \in DOMAIN identity_entry_unseated'
        /\ identity_entry_unseated'[id] = "SeatOwed"
\* A seated entry's name is refused to a probe without custody (state goal).
AuditWitnessProbeRefused == \E id \in Entries : phase = "Running" /\ ENABLED ProbeMemberAdmissionIdentityMemberRunning(id, FALSE, FALSE)
\* The agent lane refuses an entry in a stopped mob (state goal).
AuditWitnessLaneRefusedStopped == \E id \in Entries : ENABLED ResolveAgentLaneMemberAdmissionIdentityMemberStopped(id)
\* Both members are entries, one by its ladder and one otherwise (state goal).
AuditWitnessTwoEntries == Entries = {Id1, Id2}

NotAuditWitnessCustodyCommit == [][~AuditWitnessCustodyCommit]_vars
NotAuditWitnessCustodyAborted == [][~AuditWitnessCustodyAborted]_vars
NotAuditWitnessReplayed == [][~AuditWitnessReplayed]_vars
NotAuditWitnessReplayedStopped == [][~AuditWitnessReplayedStopped]_vars
NotAuditWitnessAdopted == [][~AuditWitnessAdopted]_vars
NotAuditWitnessReconcileSparesEntry == [][~AuditWitnessReconcileSparesEntry]_vars
NotAuditWitnessReconcileSpawnsWorkerOnly == [][~AuditWitnessReconcileSpawnsWorkerOnly]_vars
NotAuditWitnessUnseated == [][~AuditWitnessUnseated]_vars
NotAuditWitnessProbeRefused == ~AuditWitnessProbeRefused
NotAuditWitnessLaneRefusedStopped == ~AuditWitnessLaneRefusedStopped
NotAuditWitnessTwoEntries == ~AuditWitnessTwoEntries
====
