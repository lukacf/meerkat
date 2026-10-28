---- MODULE member_lifecycle_audit ----
\* Hand-written audit of the MobMachine member lifecycle.
\*
\* No generated profile model-checks MobMachine's member transitions to the
\* end: the ci profile has no identities (AgentIdentityValues = {}), and
\* deep.cfg, which has three, does not finish within the lane's per-run cap,
\* because the spawn arms (BeginSpawnExec*) bind 17 to 27 parameters, 12 of
\* them BOOLEAN, so a state with an enabled spawn has millions of
\* successors. This audit keeps the generated model unchanged and drives the
\* member lifecycle through every arm that does not spawn and that its
\* domains can enable. A member enters through the resume path's roster
\* recovery (RecoverRosterMemberRunning) and its session binding recovery
\* (fresh, replacing the bound session, or finding it current), then takes
\* progress observations, the local Retire arms (releasing the session,
\* preserving the binding, without a binding; in Running and Stopped), the
\* RetireMember signal (bound or peer-only), the session ingress detach (in
\* Running and Stopped), the runtime retirement and archive observations (in
\* Running and, where the machine has one, in Stopped, with and without
\* topology preservation), the runtime destroy observations (in Running,
\* Stopped and Completed, before and during the mob's own destroy), and the
\* resume path's replay arms, and a mob-level run started and finished
\* (StartRunRunning, FinishRunRunning; at most one at a time, so the run
\* count stays 0 or 1 and every member arm also meets a counted run): a
\* roster reset
\* that replaces a live incarnation in place, the Retiring and Retired
\* replays, the four unplaced retirement-start replays and the
\* addressability replay. A later roster recovery or reset mints the next
\* incarnation of the same identity, so the first incarnation's runtime id,
\* fence and session become stale. Domains: one identity, two runtime ids,
\* two fence tokens, generations 0..1, two sessions, observation times 0,
\* 60000 and 300000 (the degraded and wedged thresholds), two progress
\* tokens.
\*
\* Each run explores the whole reachable state space of these domains, with
\* no step bound. The TLC VIEW (AuditView) leaves out the two counters that
\* no guard, no other variable's update and no invariant reads:
\* model_step_count, which every arm increments, and topology_epoch, which
\* many arms increment. States that differ only in them are one state, so
\* the space is finite and TLC finishes it. The runner checks on every run
\* that the generated model still reads neither counter anywhere else.
\*
\* Input shapes: the audit gives each arm the shapes the shell sends, so a
\* guard refusal outside them is not explored. A Retire arm's (releasing,
\* session) pair is (Some(s), Some(s)) when it releases the session,
\* (None, Some(s)) when it preserves the binding and (None, None) without
\* one; RetireMember names the bound session and RetireMemberPeerOnly none.
\* The archive observations report disposal "Archived", with
\* preserve_machine_topology FALSE (TRUE for the PreservingTopology arms).
\* Roster recovery and the addressability replay use one profile and
\* runtime mode and external_addressable FALSE. The replay arms see an
\* unplaced member (host None) with no peer endpoint; the retirement-start
\* replays take topology preservation both ways and the Retired replays
\* both of their preservation flags, as a respawn's journal records them.
\* Progress observations report one in-flight count. In these shapes three
\* arms change no variable but the step count, so each returns to its
\* source state: RecoverMemberSessionBindingAlreadyCurrentRunning,
\* RecoverRosterMemberRetiringReplay and
\* RecoverRosterMemberAddressabilityRunning;
\* RecoverRosterMemberRetirementStartedReleasingAlreadyApplied does too
\* unless it records a topology preservation.
\*
\* A second, smaller run (AuditScopeSpec) checks the RetireAtScope
\* classifier: the same lifecycle arms without the progress observations,
\* plus the mob's completion (MarkCompleted) and destruction after their
\* lifecycle quiesce (DestroyMob, and the destroy admission
\* AdmitDestroyCleanup, after which a member runtime destroyed still removes
\* only that runtime), and every RetireAtScope verdict arm
\* over every scope the domains can name, current or stale. The progress
\* observations write only progress variables, and no guard of these arms,
\* nor of the Retire arms the coherence property enables, reads one; so the
\* smaller run loses no reachable value of the variables the verdicts and
\* the Retire arms read.
\*
\* Properties:
\*   every generated invariant of ci.cfg (kept);
\*   AuditObservationMonotonic (action property): while an identity stays
\*     recorded, its member_last_observed_at_ms never decreases. The guards
\*     that keep it so order optional values, which TLC evaluates only
\*     through the generated OptionLt / OptionLe: a renderer regression
\*     there fails this audit when TLC evaluates them.
\*   AuditMintClearsExecutionHealth (action property): a step that mints an
\*     incarnation (the identity's recorded generation is new or changes)
\*     leaves the identity no entry in the seven execution-health maps, so
\*     a new incarnation never starts from its predecessor's progress
\*     token, last observation, progress clock and event, health class, run
\*     state or in-flight work.
\*   AuditMintClearsFailureRecords (action property): a step that mints an
\*     incarnation leaves the identity no restore failure, restore failure
\*     code or materialization failure, so a new incarnation never reports
\*     its predecessor's failure.
\*   AuditRuntimeDestroyedRemovesOnlyItsRuntime (action property, in both
\*     runs): a member runtime destroyed, before or during the mob's own
\*     destroy, removes that runtime and nothing else: the mob keeps its
\*     phase, its destroy admission, every other runtime, its active runs and
\*     every member fact its own destroy has to drain, so only the Destroy
\*     input, behind its drains, ends the mob. With one identity no state
\*     holds two live runtimes when the arm fires, so "every other runtime"
\*     cannot fail here; the machine row
\*     a_member_runtime_destroyed_outside_the_mob_removes_only_that_runtime
\*     (two members) pins it.
\*   AuditDestroyedOnlyThroughAnAdmittedDestroy (action property, in both
\*     runs): the mob becomes Destroyed only through a destroy it admitted.
\*   AuditPlacementAbsent and AuditRetiringRuntimeMapped (invariants): no
\*     member is ever placed, and every Retiring runtime is the one its
\*     identity maps to. They check the reasons in the not-covered list
\*     below.
\*   In the scope run:
\*   AuditRetireAtScopeExactlyOne (invariant): for every scope, exactly one
\*     RetireAtScope verdict arm is enabled; the classifier is total and its
\*     arms never overlap.
\*   AuditRetireAtScopeCoherent (invariant): whenever a RetireAtScope
\*     Current arm is enabled for a scope, the scope names the identity's
\*     current runtime id and fence, and the Retire input the verdict admits
\*     can follow: a Retire arm for that incarnation is enabled, or the
\*     incarnation is already Retiring, or its runtime is no longer live (a
\*     state the unscoped Retire refuses as well).
\*
\* The runner requires every action it lists (covered_actions) to fire (TLC
\* coverage), a floor of distinct states, and every AuditWitness* goal to
\* be reached (as an expected violation of its negation): a member bound to
\* a session, a bound member's observation
\* time advancing, a Retiring member, an archived incarnation, the
\* recovery of an archived identity's next incarnation, and a delayed
\* Retired record for an older generation of an unmapped identity taking
\* RecoverRosterMemberRetiredStaleIncarnation; in the scope run, a
\* Retiring incarnation that is still Current for the session its
\* retirement releases, and a Current incarnation whose runtime is gone.
\* With --mutants it drops one observation_monotonic guard and requires
\* AuditObservationMonotonic to fail, renders the same guard as a plain
\* `<=` between option records and requires TLC to refuse to evaluate it,
\* drops the retirement_session_absent guard of
\* RecoverRosterMemberRetirementStartedPeerOnly and requires the generated
\* pending_session_ingress_detach_has_session_correlation to fail (the
\* replay then drops a releasing retirement's session while its ingress
\* detach is pending), restores the old guard of
\* RecoverRosterMemberRetiredStaleIncarnation, which also takes a record
\* naming the identity's current runtime with another generation, and
\* requires the generated pending_respawn_topology_has_no_current_runtime to
\* fail (the arm then prunes the Retiring runtime of an identity held for a
\* respawn while the identity still maps to it), drops the runtime and
\* fence guards of
\* RetireAtScopeCurrentRunning and requires AuditRetireAtScopeCoherent to
\* fail, and drops the mapped guard of RetireAtScopeStaleFenceTokenRunning
\* and requires AuditRetireAtScopeExactlyOne to fail, and keeps the
\* predecessor's execution-health entries in RecoverRosterMemberRunning and
\* requires AuditMintClearsExecutionHealth to fail, and makes
\* ObserveRuntimeDestroyed end the mob, leave its runtime live, and reset the
\* mob's run count, and requires AuditRuntimeDestroyedRemovesOnlyItsRuntime to
\* fail for each.
\*
\* Not covered:
\*   states reachable only through BeginSpawnExec* (spawn phases, spawn
\*     failures, provisioning), and flows and adaptive runs;
\*   placed members, which only the spawn and placement arms create
\*     (AuditPlacementAbsent): ObserveMemberRetirementArchivedPlaced*,
\*     RecoverRosterMemberRetirementStartedPlaced and the remote Retire arms
\*     (RetireMemberRemote, RetireMemberRemoteConfirmedRevoked,
\*     RetireRemoteReleasing*, RetireRemoteConfirmedRevoked*);
\*   ObserveMemberRetirementArchivedStaleRuntime and its Stopped twin: they
\*     need a Retiring runtime that its identity no longer maps to, and here
\*     every arm that unmaps an identity clears its runtime's Retiring
\*     marker in the same step, while a roster reset refuses a Retiring
\*     predecessor (AuditRetiringRuntimeMapped);
\*   RetireAllRunning, RetireAllStopped and RetireAllCompleted: they change
\*     only the phase and the step count.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* member_lifecycle_audit.sh.
EXTENDS model

\* The positions of model_step_count and topology_epoch in the generated
\* `vars` tuple; the runner reads them from model.tla.
CONSTANTS AuditStepIndex, AuditEpochIndex

AuditIdentity == "agentidentity_1"
AuditRuntimes == {"agentruntimeid_1", "agentruntimeid_2"}
AuditFences == {1, 2}
AuditGenerations == {0, 1}
AuditSessions == {"sessionid_1", "sessionid_2"}
\* 60000 and 300000 are the generated degraded and wedged thresholds.
AuditTimes == {0, 60000, 300000}
AuditTokens == {"token_1", "token_2"}
\* No guard reads in_flight_work; the arms only record it.
AuditInFlight == {1}
AuditMob == "mobid_1"
AuditProfile == "profile_1"
AuditMode == "TurnDriven"
AuditDisposal == "Archived"

AuditRecoverMember ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations :
        RecoverRosterMemberRunning(AuditIdentity, rid, fence, g, AuditProfile, AuditMode, FALSE)

AuditBindSession ==
    \E rid \in AuditRuntimes, s \in AuditSessions :
        RecoverMemberSessionBindingFreshRunning(AuditIdentity, rid, s, None)

AuditProgressChangedOpen ==
    \E w \in AuditInFlight, tok \in AuditTokens, t \in AuditTimes :
        ObserveMemberProgressChangedOpenRunning(AuditIdentity, TRUE, w, tok, t)

AuditProgressChangedIdle ==
    \E w \in AuditInFlight, tok \in AuditTokens, t \in AuditTimes :
        ObserveMemberProgressChangedIdleRunning(AuditIdentity, FALSE, w, tok, t)

AuditProgressUnchangedIdle ==
    \E w \in AuditInFlight, tok \in AuditTokens, t \in AuditTimes :
        ObserveMemberProgressUnchangedIdleRunning(AuditIdentity, FALSE, w, tok, t)

AuditProgressUnchangedOpenHealthy ==
    \E w \in AuditInFlight, tok \in AuditTokens, t \in AuditTimes :
        ObserveMemberProgressUnchangedOpenHealthyRunning(AuditIdentity, TRUE, w, tok, t)

AuditProgressUnchangedOpenDegraded ==
    \E w \in AuditInFlight, tok \in AuditTokens, t \in AuditTimes :
        ObserveMemberProgressUnchangedOpenDegradedRunning(AuditIdentity, TRUE, w, tok, t)

AuditProgressUnchangedOpenWedged ==
    \E w \in AuditInFlight, tok \in AuditTokens, t \in AuditTimes :
        ObserveMemberProgressUnchangedOpenWedgedRunning(AuditIdentity, TRUE, w, tok, t)

AuditProgressStale ==
    \E open \in BOOLEAN, w \in AuditInFlight, tok \in AuditTokens, t \in AuditTimes :
        ObserveMemberProgressStaleRunning(AuditIdentity, open, w, tok, t)

\* A recovered member is seated Dormant (it holds no registration), so its
\* retirement takes the archive-only twin; the routed arms need a Live
\* member, which member_residency_audit's retire run drives.
AuditRetireReleasing ==
    \E rid \in AuditRuntimes, g \in AuditGenerations, s \in AuditSessions :
        \/ RetireRunningReleasing(AuditMob, rid, AuditIdentity, g, Some(s), Some(s))
        \/ RetireRunningReleasingArchiveOnly(AuditMob, rid, AuditIdentity, g, Some(s), Some(s))

AuditRetirePreservingBinding ==
    \E rid \in AuditRuntimes, g \in AuditGenerations, s \in AuditSessions :
        \/ RetireRunningPreservingBinding(AuditMob, rid, AuditIdentity, g, None, Some(s))
        \/ RetireRunningPreservingBindingArchiveOnly(AuditMob, rid, AuditIdentity, g, None, Some(s))

AuditRetireNoBinding ==
    \E rid \in AuditRuntimes, g \in AuditGenerations :
        RetireRunningNoBinding(AuditMob, rid, AuditIdentity, g, None, None)

AuditRetireStoppedReleasing ==
    \E rid \in AuditRuntimes, g \in AuditGenerations, s \in AuditSessions :
        \/ RetireStoppedReleasing(AuditMob, rid, AuditIdentity, g, Some(s), Some(s))
        \/ RetireStoppedReleasingArchiveOnly(AuditMob, rid, AuditIdentity, g, Some(s), Some(s))

AuditRetireStoppedPreservingBinding ==
    \E rid \in AuditRuntimes, g \in AuditGenerations, s \in AuditSessions :
        \/ RetireStoppedPreservingBinding(AuditMob, rid, AuditIdentity, g, None, Some(s))
        \/ RetireStoppedPreservingBindingArchiveOnly(AuditMob, rid, AuditIdentity, g, None, Some(s))

AuditRetireStoppedNoBinding ==
    \E rid \in AuditRuntimes, g \in AuditGenerations :
        RetireStoppedNoBinding(AuditMob, rid, AuditIdentity, g, None, None)

AuditRetireAbsent == RetireAbsentRunning(AuditIdentity)

AuditRetireAbsentStopped == RetireAbsentStopped(AuditIdentity)

AuditRetireMember ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        \/ RetireMember(AuditIdentity, rid, fence, Some(s))
        \/ RetireMemberArchiveOnly(AuditIdentity, rid, fence, Some(s))

AuditDetachSessionIngress ==
    \E rid \in AuditRuntimes : SessionIngressDetachedForMobDestroyRunning(AuditMob, rid)

AuditRuntimeRetired ==
    \E rid \in AuditRuntimes, fence \in AuditFences : ObserveRuntimeRetired(rid, fence)

AuditRuntimeRetiredStopped ==
    \E rid \in AuditRuntimes, fence \in AuditFences : ObserveRuntimeRetiredStopped(rid, fence)

AuditArchivedLive ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations, s \in AuditSessions :
        ObserveMemberRetirementArchivedLive(AuditIdentity, rid, fence, g, Some(s), AuditDisposal, FALSE)

AuditArchivedRetired ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations, s \in AuditSessions :
        ObserveMemberRetirementArchivedRetired(AuditIdentity, rid, fence, g, Some(s), AuditDisposal, FALSE)

AuditArchivedLiveStopped ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations, s \in AuditSessions :
        ObserveMemberRetirementArchivedLiveStopped(AuditIdentity, rid, fence, g, Some(s), AuditDisposal, FALSE)

AuditArchivedRetiredStopped ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations, s \in AuditSessions :
        ObserveMemberRetirementArchivedRetiredStopped(AuditIdentity, rid, fence, g, Some(s), AuditDisposal, FALSE)

AuditArchivedAlreadyCleared ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations, s \in AuditSessions :
        ObserveMemberRetirementArchivedAlreadyCleared(AuditIdentity, rid, fence, g, Some(s), AuditDisposal)

AuditArchivedStaleRuntimeAlreadyCleared ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations, s \in AuditSessions :
        ObserveMemberRetirementArchivedStaleRuntimeAlreadyCleared(AuditIdentity, rid, fence, g, Some(s), AuditDisposal)

AuditArchivedAlreadyClearedStopped ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations, s \in AuditSessions :
        ObserveMemberRetirementArchivedAlreadyClearedStopped(AuditIdentity, rid, fence, g, Some(s), AuditDisposal)

AuditArchivedStaleRuntimeAlreadyClearedStopped ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations, s \in AuditSessions :
        ObserveMemberRetirementArchivedStaleRuntimeAlreadyClearedStopped(AuditIdentity, rid, fence, g, Some(s), AuditDisposal)

\* A member runtime destroyed, in each phase that has one, before or during
\* the mob's own destroy (in the scope run after AdmitDestroyCleanup): only
\* that runtime is removed.
AuditRuntimeDestroyedArm(rid, fence) ==
    \/ ObserveRuntimeDestroyed(rid, fence)
    \/ ObserveRuntimeDestroyedStopped(rid, fence)
    \/ ObserveRuntimeDestroyedCompleted(rid, fence)

AuditRuntimeDestroyed ==
    \E rid \in AuditRuntimes, fence \in AuditFences : AuditRuntimeDestroyedArm(rid, fence)

\* A mob-level run, at most one at a time: the destroyed and retired member
\* arms must leave it counted.
AuditStartRun == active_run_count = 0 /\ StartRunRunning

AuditFinishRun == FinishRunRunning

AuditQuiesceForStop == BeginPlacedCompletionLifecycleQuiesceFreshStop("Stop")

AuditQuiesceForComplete == BeginPlacedCompletionLifecycleQuiesceFresh("Complete")

AuditQuiesceForDestroy == BeginPlacedCompletionLifecycleQuiesceFresh("Destroy")

AuditComplete == MarkCompleted

AuditDestroy == \E s \in AuditSessions : DestroyMob(s)

\* The destroy admission, after the lifecycle quiesce for a destroy.
AuditAdmitDestroy == AdmitDestroyCleanup

AuditRetireAtScopeCurrentRunning ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        RetireAtScopeCurrentRunning(AuditIdentity, rid, fence, s)

AuditRetireAtScopeCurrentStopped ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        RetireAtScopeCurrentStopped(AuditIdentity, rid, fence, s)

AuditRetireAtScopeNoCurrentIncarnationRunning ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        RetireAtScopeNoCurrentIncarnationRunning(AuditIdentity, rid, fence, s)

AuditRetireAtScopeNoCurrentIncarnationStopped ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        RetireAtScopeNoCurrentIncarnationStopped(AuditIdentity, rid, fence, s)

AuditRetireAtScopeStaleFenceTokenRunning ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        RetireAtScopeStaleFenceTokenRunning(AuditIdentity, rid, fence, s)

AuditRetireAtScopeStaleFenceTokenStopped ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        RetireAtScopeStaleFenceTokenStopped(AuditIdentity, rid, fence, s)

AuditRetireAtScopeStaleSessionBindingRunning ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        RetireAtScopeStaleSessionBindingRunning(AuditIdentity, rid, fence, s)

AuditRetireAtScopeStaleSessionBindingStopped ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        RetireAtScopeStaleSessionBindingStopped(AuditIdentity, rid, fence, s)

AuditRetireAtScopeMobNotRunningCompleted ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        RetireAtScopeMobNotRunningCompleted(AuditIdentity, rid, fence, s)

AuditRetireAtScopeMobNotRunningDestroyed ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        RetireAtScopeMobNotRunningDestroyed(AuditIdentity, rid, fence, s)

\* A bound member's session binding recovery: the resume path replaces the
\* binding with another session, or finds the recorded session current.
AuditBindSessionReplacing ==
    \E rid \in AuditRuntimes, s \in AuditSessions, r \in AuditSessions :
        RecoverMemberSessionBindingReplacingRunning(AuditIdentity, rid, s, Some(r))

AuditBindSessionAlreadyCurrent ==
    \E rid \in AuditRuntimes, s \in AuditSessions :
        \/ RecoverMemberSessionBindingAlreadyCurrentRunning(AuditIdentity, rid, s, None)
        \/ RecoverMemberSessionBindingAlreadyCurrentRunning(AuditIdentity, rid, s, Some(s))

\* A roster reset: the next incarnation replaces a live one in place, with no
\* archive in between.
AuditResetMember ==
    \E prev \in AuditRuntimes, pg \in AuditGenerations, rid \in AuditRuntimes,
       fence \in AuditFences, g \in AuditGenerations :
        RecoverRosterMemberResetRunning(AuditIdentity, prev, pg, rid, fence, g)

AuditRetireMemberPeerOnly ==
    \E rid \in AuditRuntimes, fence \in AuditFences :
        RetireMemberPeerOnly(AuditIdentity, rid, fence, None)

AuditDetachSessionIngressStopped ==
    \E rid \in AuditRuntimes : SessionIngressDetachedForMobDestroyStopped(AuditMob, rid)

AuditArchivedLivePreservingTopology ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations, s \in AuditSessions :
        ObserveMemberRetirementArchivedLivePreservingTopology(AuditIdentity, rid, fence, g, Some(s), AuditDisposal, TRUE)

AuditArchivedRetiredPreservingTopology ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations, s \in AuditSessions :
        ObserveMemberRetirementArchivedRetiredPreservingTopology(AuditIdentity, rid, fence, g, Some(s), AuditDisposal, TRUE)

\* The replay arms: the resume path feeds a persisted retirement back for an
\* unplaced member (host None), without topology preservation or a peer
\* endpoint.
AuditRecoverRetiring ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations :
        \/ RecoverRosterMemberRetiringRunning(AuditIdentity, rid, fence, g, None, None)
        \/ \E s \in AuditSessions :
               RecoverRosterMemberRetiringRunning(AuditIdentity, rid, fence, g, Some(s), None)

AuditRecoverRetiringReplay ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations :
        \/ RecoverRosterMemberRetiringReplay(AuditIdentity, rid, fence, g, None, None)
        \/ \E s \in AuditSessions :
               RecoverRosterMemberRetiringReplay(AuditIdentity, rid, fence, g, Some(s), None)

AuditRecoverRetired ==
    \E rid \in AuditRuntimes, g \in AuditGenerations, p \in BOOLEAN, started \in BOOLEAN :
        RecoverRosterMemberRetiredRunning(AuditIdentity, rid, g, p, started)

AuditRecoverRetiredAlreadyAbsent ==
    \E rid \in AuditRuntimes, g \in AuditGenerations, p \in BOOLEAN, started \in BOOLEAN :
        RecoverRosterMemberRetiredCurrentAlreadyAbsent(AuditIdentity, rid, g, p, started)

AuditRecoverRetiredStaleGeneration ==
    \E rid \in AuditRuntimes : RecoverRosterMemberRetiredStaleGeneration(AuditIdentity, rid)

AuditRecoverRetiredStaleIncarnation ==
    \E rid \in AuditRuntimes, g \in AuditGenerations :
        RecoverRosterMemberRetiredStaleIncarnation(AuditIdentity, rid, g)

AuditRecoverRetirementStartedReleasing ==
    \E rid \in AuditRuntimes, g \in AuditGenerations, s \in AuditSessions, p \in BOOLEAN :
        RecoverRosterMemberRetirementStartedReleasing(AuditIdentity, rid, g, Some(s), Some(s), None, p)

AuditRecoverRetirementStartedReleasingAlreadyApplied ==
    \E rid \in AuditRuntimes, g \in AuditGenerations, s \in AuditSessions, p \in BOOLEAN :
        RecoverRosterMemberRetirementStartedReleasingAlreadyApplied(AuditIdentity, rid, g, Some(s), Some(s), None, p)

AuditRecoverRetirementStartedPreservingBinding ==
    \E rid \in AuditRuntimes, g \in AuditGenerations, s \in AuditSessions, p \in BOOLEAN :
        RecoverRosterMemberRetirementStartedPreservingBinding(AuditIdentity, rid, g, None, Some(s), None, p)

AuditRecoverRetirementStartedPeerOnly ==
    \E rid \in AuditRuntimes, g \in AuditGenerations, p \in BOOLEAN :
        RecoverRosterMemberRetirementStartedPeerOnly(AuditIdentity, rid, g, None, None, None, p)

AuditRecoverAddressability ==
    \E rid \in AuditRuntimes, fence \in AuditFences, g \in AuditGenerations :
        RecoverRosterMemberAddressabilityRunning(AuditIdentity, rid, fence, g, AuditProfile, AuditMode, FALSE)

AuditStop == StopRunning

AuditResume == ResumeStopped

\* The member lifecycle arms without the progress observations.
AuditLifecycleNext ==
    \/ AuditStartRun
    \/ AuditFinishRun
    \/ AuditRecoverMember
    \/ AuditBindSession
    \/ AuditBindSessionReplacing
    \/ AuditResetMember
    \/ AuditRetireReleasing
    \/ AuditRetirePreservingBinding
    \/ AuditRetireNoBinding
    \/ AuditRetireStoppedReleasing
    \/ AuditRetireStoppedPreservingBinding
    \/ AuditRetireStoppedNoBinding
    \/ AuditRetireAbsent
    \/ AuditRetireAbsentStopped
    \/ AuditRetireMember
    \/ AuditRetireMemberPeerOnly
    \/ AuditDetachSessionIngress
    \/ AuditDetachSessionIngressStopped
    \/ AuditRecoverRetiring
    \/ AuditRecoverRetired
    \/ AuditRecoverRetiredAlreadyAbsent
    \/ AuditRecoverRetiredStaleGeneration
    \/ AuditRecoverRetiredStaleIncarnation
    \/ AuditRecoverRetirementStartedReleasing
    \/ AuditRecoverRetirementStartedReleasingAlreadyApplied
    \/ AuditRecoverRetirementStartedPreservingBinding
    \/ AuditRecoverRetirementStartedPeerOnly
    \/ AuditRuntimeRetired
    \/ AuditRuntimeRetiredStopped
    \/ AuditRuntimeDestroyed
    \/ AuditArchivedLive
    \/ AuditArchivedRetired
    \/ AuditArchivedLiveStopped
    \/ AuditArchivedRetiredStopped
    \/ AuditArchivedAlreadyCleared
    \/ AuditArchivedStaleRuntimeAlreadyCleared
    \/ AuditArchivedAlreadyClearedStopped
    \/ AuditArchivedStaleRuntimeAlreadyClearedStopped
    \/ AuditArchivedLivePreservingTopology
    \/ AuditArchivedRetiredPreservingTopology
    \/ AuditQuiesceForStop
    \/ AuditStop
    \/ AuditResume

\* The arms that, in the shapes the audit gives them, change no variable but
\* the step count.
AuditStepCountOnlyNext ==
    \/ AuditBindSessionAlreadyCurrent
    \/ AuditRecoverRetiringReplay
    \/ AuditRecoverAddressability

AuditNext ==
    \/ AuditLifecycleNext
    \/ AuditStepCountOnlyNext
    \/ AuditProgressChangedOpen
    \/ AuditProgressChangedIdle
    \/ AuditProgressUnchangedIdle
    \/ AuditProgressUnchangedOpenHealthy
    \/ AuditProgressUnchangedOpenDegraded
    \/ AuditProgressUnchangedOpenWedged
    \/ AuditProgressStale

AuditSpec == Init /\ [][AuditNext]_vars

AuditScopeNext ==
    \/ AuditLifecycleNext
    \/ AuditStepCountOnlyNext
    \/ AuditQuiesceForComplete
    \/ AuditQuiesceForDestroy
    \/ AuditAdmitDestroy
    \/ AuditComplete
    \/ AuditDestroy
    \/ AuditRetireAtScopeCurrentRunning
    \/ AuditRetireAtScopeCurrentStopped
    \/ AuditRetireAtScopeNoCurrentIncarnationRunning
    \/ AuditRetireAtScopeNoCurrentIncarnationStopped
    \/ AuditRetireAtScopeStaleFenceTokenRunning
    \/ AuditRetireAtScopeStaleFenceTokenStopped
    \/ AuditRetireAtScopeStaleSessionBindingRunning
    \/ AuditRetireAtScopeStaleSessionBindingStopped
    \/ AuditRetireAtScopeMobNotRunningCompleted
    \/ AuditRetireAtScopeMobNotRunningDestroyed

AuditScopeSpec == Init /\ [][AuditScopeNext]_vars

\* Every state variable but the two counters (see the header).
AuditView == [vars EXCEPT ![AuditStepIndex] = 0, ![AuditEpochIndex] = 0]

AuditObservationMonotonic ==
    [][\A id \in DOMAIN member_last_observed_at_ms :
         id \in DOMAIN member_last_observed_at_ms' =>
             member_last_observed_at_ms'[id] >= member_last_observed_at_ms[id]
      ]_vars

\* A step that mints an incarnation leaves the identity no execution-health
\* entry: either its recorded generation is unchanged, or none of the seven
\* maps holds it after the step.
AuditMintClearsExecutionHealth ==
    [][\A id \in DOMAIN identity_runtime_generations' :
         \/ /\ id \in DOMAIN identity_runtime_generations
            /\ identity_runtime_generations'[id] = identity_runtime_generations[id]
         \/ /\ id \notin DOMAIN member_run_open'
            /\ id \notin DOMAIN member_in_flight_work'
            /\ id \notin DOMAIN member_progress_tokens'
            /\ id \notin DOMAIN member_last_observed_at_ms'
            /\ id \notin DOMAIN member_last_progress_at_ms'
            /\ id \notin DOMAIN member_last_progress_event'
            /\ id \notin DOMAIN member_health_class'
      ]_vars

\* A step that mints an incarnation leaves the identity no failure record:
\* either its recorded generation is unchanged, or none of the three
\* failure maps holds it after the step. It guards arms to come: the bounded
\* run reaches no failure record before a mint, so it has no mutant.
AuditMintClearsFailureRecords ==
    [][\A id \in DOMAIN identity_runtime_generations' :
         \/ /\ id \in DOMAIN identity_runtime_generations
            /\ identity_runtime_generations'[id] = identity_runtime_generations[id]
         \/ /\ id \notin DOMAIN member_restore_failures'
            /\ id \notin DOMAIN member_restore_failure_codes'
            /\ id \notin DOMAIN member_materialization_failures'
      ]_vars

\* A member runtime destroyed removes that runtime and nothing else: the mob
\* keeps its phase, its destroy admission, every other runtime, its active
\* runs and every member fact its own destroy has to drain.
AuditRuntimeDestroyedRemovesOnlyItsRuntime ==
    [][\A rid \in AuditRuntimes, fence \in AuditFences :
         AuditRuntimeDestroyedArm(rid, fence) =>
            /\ phase' = phase
            /\ destroy_admitted' = destroy_admitted
            /\ live_runtime_ids' = live_runtime_ids \ {rid}
            /\ identity_to_runtime' = identity_to_runtime
            /\ member_session_bindings' = member_session_bindings
            /\ member_state_markers' = member_state_markers
            /\ pending_respawn_topology' = pending_respawn_topology
            /\ wiring_edges' = wiring_edges
            /\ runtime_retire_pending_sessions' = runtime_retire_pending_sessions
            /\ pending_session_ingress_detach_runtime_ids' = pending_session_ingress_detach_runtime_ids
            /\ active_run_count' = active_run_count
      ]_vars

\* The mob reaches Destroyed only through a destroy it admitted
\* (AdmitDestroyCleanup or DestroyMob).
AuditDestroyedOnlyThroughAnAdmittedDestroy ==
    [][(phase # "Destroyed" /\ phase' = "Destroyed") => destroy_admitted' = TRUE]_vars

\* No arm here places a member: the placed archive, retirement-start and
\* remote Retire arms need a placement.
AuditPlacementAbsent ==
    /\ DOMAIN member_placement = {}
    /\ DOMAIN current_placed_spawn_ids = {}

\* Every Retiring runtime is the one its identity maps to, so the
\* StaleRuntime archive arms, which need a Retiring runtime its identity no
\* longer maps to, never fire here.
AuditRetiringRuntimeMapped ==
    \A rid \in DOMAIN member_state_markers :
        member_state_markers[rid] = "Retiring" =>
            /\ AuditIdentity \in DOMAIN identity_to_runtime
            /\ identity_to_runtime[AuditIdentity] = rid

AuditRetireAtScopeVerdicts(rid, fence, s) ==
    <<ENABLED RetireAtScopeCurrentRunning(AuditIdentity, rid, fence, s),
      ENABLED RetireAtScopeCurrentStopped(AuditIdentity, rid, fence, s),
      ENABLED RetireAtScopeNoCurrentIncarnationRunning(AuditIdentity, rid, fence, s),
      ENABLED RetireAtScopeNoCurrentIncarnationStopped(AuditIdentity, rid, fence, s),
      ENABLED RetireAtScopeStaleFenceTokenRunning(AuditIdentity, rid, fence, s),
      ENABLED RetireAtScopeStaleFenceTokenStopped(AuditIdentity, rid, fence, s),
      ENABLED RetireAtScopeStaleSessionBindingRunning(AuditIdentity, rid, fence, s),
      ENABLED RetireAtScopeStaleSessionBindingStopped(AuditIdentity, rid, fence, s),
      ENABLED RetireAtScopeMobNotRunningCompleted(AuditIdentity, rid, fence, s),
      ENABLED RetireAtScopeMobNotRunningDestroyed(AuditIdentity, rid, fence, s)>>

AuditRetireAtScopeExactlyOne ==
    \A rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        Cardinality({i \in 1..10 : AuditRetireAtScopeVerdicts(rid, fence, s)[i]}) = 1

AuditCurrentEnabled(rid, fence, s) ==
    \/ ENABLED RetireAtScopeCurrentRunning(AuditIdentity, rid, fence, s)
    \/ ENABLED RetireAtScopeCurrentStopped(AuditIdentity, rid, fence, s)

\* A Retire input for the incarnation `rid` of the audit identity is enabled,
\* in the shapes the audit's Retire arms take: the routed arms for a Live (or
\* phaseless) member, the archive-only twins for a Dormant or Waking one.
AuditRetireEnabledFor(rid) ==
    \E g \in AuditGenerations, s \in AuditSessions :
        \/ ENABLED RetireRunningReleasing(AuditMob, rid, AuditIdentity, g, Some(s), Some(s))
        \/ ENABLED RetireRunningReleasingArchiveOnly(AuditMob, rid, AuditIdentity, g, Some(s), Some(s))
        \/ ENABLED RetireRunningPreservingBinding(AuditMob, rid, AuditIdentity, g, None, Some(s))
        \/ ENABLED RetireRunningPreservingBindingArchiveOnly(AuditMob, rid, AuditIdentity, g, None, Some(s))
        \/ ENABLED RetireRunningNoBinding(AuditMob, rid, AuditIdentity, g, None, None)
        \/ ENABLED RetireStoppedReleasing(AuditMob, rid, AuditIdentity, g, Some(s), Some(s))
        \/ ENABLED RetireStoppedReleasingArchiveOnly(AuditMob, rid, AuditIdentity, g, Some(s), Some(s))
        \/ ENABLED RetireStoppedPreservingBinding(AuditMob, rid, AuditIdentity, g, None, Some(s))
        \/ ENABLED RetireStoppedPreservingBindingArchiveOnly(AuditMob, rid, AuditIdentity, g, None, Some(s))
        \/ ENABLED RetireStoppedNoBinding(AuditMob, rid, AuditIdentity, g, None, None)

AuditRetiring(rid) ==
    rid \in DOMAIN member_state_markers /\ member_state_markers[rid] = "Retiring"

AuditRetireAtScopeCoherent ==
    \A rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        AuditCurrentEnabled(rid, fence, s) =>
            /\ AuditIdentity \in DOMAIN identity_to_runtime
            /\ identity_to_runtime[AuditIdentity] = rid
            /\ AuditIdentity \in DOMAIN identity_runtime_fence_tokens
            /\ identity_runtime_fence_tokens[AuditIdentity] = fence
            /\ \/ AuditRetireEnabledFor(rid)
               \/ AuditRetiring(rid)
               \/ rid \notin live_runtime_ids

\* Reachability goals, each checked as an expected violation of its negation:
\* a state goal as an invariant, a step goal as an action property.
\* A member is bound to a session.
AuditWitnessBound ==
    /\ AuditIdentity \in DOMAIN identity_to_runtime
    /\ AuditIdentity \in DOMAIN member_session_bindings
\* A member is retiring.
AuditWitnessRetiring ==
    \E rid \in DOMAIN member_state_markers : member_state_markers[rid] = "Retiring"
\* An incarnation was archived: the identity keeps its history but no mapping.
AuditWitnessArchived ==
    /\ AuditIdentity \in DOMAIN identity_runtime_generations
    /\ AuditIdentity \notin DOMAIN identity_to_runtime
\* A bound member's recorded observation time advances: the progress guards
\* order Some(earlier) before Some(later) and admit the observation.
AuditWitnessObservationAdvancesStep ==
    /\ AuditWitnessBound
    /\ AuditIdentity \in DOMAIN member_last_observed_at_ms
    /\ AuditIdentity \in DOMAIN member_last_observed_at_ms'
    /\ member_last_observed_at_ms'[AuditIdentity] > member_last_observed_at_ms[AuditIdentity]
\* An archived identity's next incarnation is recovered at a higher
\* generation.
AuditWitnessSuccessorStep ==
    /\ AuditWitnessArchived
    /\ AuditIdentity \in DOMAIN identity_to_runtime'
    /\ AuditIdentity \in DOMAIN identity_runtime_generations'
    /\ identity_runtime_generations'[AuditIdentity] > identity_runtime_generations[AuditIdentity]
\* A delayed Retired record for an older generation of an unmapped identity
\* still takes RecoverRosterMemberRetiredStaleIncarnation: the control for
\* the arm's partition guards. Every arm here that unmaps an identity clears
\* its runtime's bookkeeping in the same step, so this step prunes nothing;
\* a meerkat-mob test pins the prune itself.
AuditWitnessStaleIncarnationStep ==
    /\ AuditIdentity \notin DOMAIN identity_to_runtime
    /\ AuditIdentity \in DOMAIN identity_runtime_generations
    /\ \E rid \in AuditRuntimes, g \in AuditGenerations :
           /\ g < identity_runtime_generations[AuditIdentity]
           /\ RecoverRosterMemberRetiredStaleIncarnation(AuditIdentity, rid, g)

\* Scope run: a Retiring incarnation whose session binding its retirement
\* released is still Current for the scope that names that session.
AuditWitnessRetiringScopeCurrent ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        /\ AuditRetiring(rid)
        /\ AuditIdentity \notin DOMAIN member_session_bindings
        /\ AuditCurrentEnabled(rid, fence, s)
\* Scope run: a Current incarnation that is not Retiring and whose runtime is
\* no longer live, the third case of AuditRetireAtScopeCoherent.
AuditWitnessCurrentRuntimeGone ==
    \E rid \in AuditRuntimes, fence \in AuditFences, s \in AuditSessions :
        /\ rid \notin live_runtime_ids
        /\ ~AuditRetiring(rid)
        /\ AuditCurrentEnabled(rid, fence, s)

NotAuditWitnessBound == ~AuditWitnessBound
NotAuditWitnessRetiring == ~AuditWitnessRetiring
NotAuditWitnessArchived == ~AuditWitnessArchived
NotAuditWitnessRetiringScopeCurrent == ~AuditWitnessRetiringScopeCurrent
NotAuditWitnessCurrentRuntimeGone == ~AuditWitnessCurrentRuntimeGone
NotAuditWitnessObservationAdvances == [][~AuditWitnessObservationAdvancesStep]_vars
NotAuditWitnessSuccessor == [][~AuditWitnessSuccessorStep]_vars
NotAuditWitnessStaleIncarnation == [][~AuditWitnessStaleIncarnationStep]_vars
====
