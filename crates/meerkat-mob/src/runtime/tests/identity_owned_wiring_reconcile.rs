//! Identity reconciliation holds an identity to the wiring it owns.
//!
//! A desired edge has one owner, the lexicographically smaller endpoint, and
//! only that identity's declaration states it. The store also records, on
//! every identity, the union of the desired edges incident to it (its cleanup
//! targets), the other endpoint's edges included. The reconcile pass compares
//! an identity's realized wiring with what that identity owns: the far end of
//! another identity's edge owns nothing of it, so it converges and its
//! owner's changes become active, while the edge's owner stays held to the
//! edge.
//!
//! No reconcile step realizes an owned edge: an identity adopted with an edge
//! the mob does not have is repair-blocked for its wiring. The rows here use
//! such an adopter, so the far end's outcome never depends on the edge.

use super::shutdown_lease_release::adoption;
use super::*;
use crate::identity::{
    DesiredIdentityEdge, IdentityAdoptionOutcome, IdentityConvergenceStatus, IdentityIntent,
    IdentityReconcileDecision, IdentityRetirementPlan, IdentityStoredObservation,
    IdentityWiringCustody,
};

/// Bound for one step (a convergence, a settled decision); it only turns a
/// hang into a failure.
const STEP: Duration = Duration::from_secs(20);

/// Bound for the first settled decision after a start: longer than an
/// identity lease, which a start may have to wait out.
const AFTER_START: Duration = Duration::from_secs(60);

/// A mob over in-memory mob stores and a persistent session service, whose
/// members are turn-driven workers. The stores outlive the handle.
struct WiringFixture {
    handle: MobHandle,
    mob_id: MobId,
    events: Arc<dyn MobEventStore>,
    runs: Arc<InMemoryMobRunStore>,
    specs: Arc<InMemoryMobSpecStore>,
    runtime_metadata: Arc<InMemoryMobRuntimeMetadataStore>,
    identity: Arc<InMemoryMobIdentityStore>,
    identity_status: Arc<InMemoryMobIdentityStatusStore>,
    service: Arc<meerkat_session::PersistentSessionService<PersistentMockBuilder>>,
}

fn base_prompt() -> Arc<crate::StaticSpawnBasePromptSource> {
    Arc::new(crate::StaticSpawnBasePromptSource(
        "owned wiring reconcile test base prompt".to_string(),
    ))
}

impl WiringFixture {
    async fn create(label: &str) -> Self {
        let mut definition = with_unique_mob_id(sample_definition(), label);
        // The rows hand members messages the way an operator does.
        definition
            .profiles
            .get_mut(&ProfileName::from("worker"))
            .expect("worker profile")
            .as_inline_mut()
            .expect("an inline worker profile")
            .external_addressable = true;
        let mob_id = definition.id.clone();
        let event_store = InMemoryMobEventStore::new();
        let identity = Arc::new(InMemoryMobIdentityStore::paired_with_event_store(
            &event_store,
            Arc::new(crate::store::SystemMobIdentityStoreClock),
        ));
        let events: Arc<dyn MobEventStore> = Arc::new(event_store);
        let runs = Arc::new(InMemoryMobRunStore::new());
        let specs = Arc::new(InMemoryMobSpecStore::new());
        let runtime_metadata = Arc::new(InMemoryMobRuntimeMetadataStore::new());
        let identity_status = Arc::new(InMemoryMobIdentityStatusStore::new());
        let session_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            PersistentMockBuilder,
            16,
            session_store,
            runtime_store,
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        let storage = MobStorage {
            events: events.clone(),
            runs: runs.clone(),
            specs: specs.clone(),
            definition_projection_composition:
                crate::storage::DefinitionProjectionComposition::Independent,
            runtime_metadata: runtime_metadata.clone(),
            identity: identity.clone(),
            identity_member: Some(identity.clone()),
            identity_status: identity_status.clone(),
            identity_status_projection_order: Default::default(),
            realm_profiles: None,
            forked_participants: None,
        };
        let handle = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .with_spawn_base_prompt_source(base_prompt())
            .create()
            .await
            .expect("create the owned wiring test mob");
        Self {
            handle,
            mob_id,
            events,
            runs,
            specs,
            runtime_metadata,
            identity,
            identity_status,
            service,
        }
    }

    /// Stop the mob and start its next incarnation over the same stores.
    async fn restart(&mut self) {
        self.handle.shutdown().await.expect("shutdown the mob");
        self.start_again().await;
    }

    /// Start the next incarnation over the stores of a mob that was shut
    /// down.
    async fn start_again(&mut self) {
        let storage = MobStorage {
            events: self.events.clone(),
            runs: self.runs.clone(),
            specs: self.specs.clone(),
            definition_projection_composition:
                crate::storage::DefinitionProjectionComposition::Independent,
            runtime_metadata: self.runtime_metadata.clone(),
            identity: self.identity.clone(),
            identity_member: Some(self.identity.clone()),
            identity_status: self.identity_status.clone(),
            identity_status_projection_order: Default::default(),
            realm_profiles: None,
            forked_participants: None,
        };
        self.handle = tokio::time::timeout(
            STEP,
            MobBuilder::for_resume(storage)
                .with_session_service(self.service.clone())
                .with_spawn_base_prompt_source(base_prompt())
                .notify_orchestrator_on_resume(false)
                .resume(),
        )
        .await
        .expect("the resume answers")
        .expect("resume the owned wiring test mob");
    }

    /// Spawn a turn-driven worker and return its identity and session.
    async fn seat(&self, name: &str) -> (AgentIdentity, SessionId) {
        self.seat_on(name, None).await
    }

    /// [`Self::seat`] on a session that is built with a comms runtime on a
    /// key of its own: the wiring calls wire members over their comms
    /// runtimes.
    async fn seat_with_comms(&self, name: &str) -> (AgentIdentity, SessionId) {
        let session_id = SessionId::new();
        set_persistent_mock_comms_seed(&session_id, &format!("{}-{name}", self.mob_id));
        self.seat_on(name, Some(session_id)).await
    }

    async fn seat_on(&self, name: &str, session: Option<SessionId>) -> (AgentIdentity, SessionId) {
        let member = AgentIdentity::from(name);
        let mut spawn = SpawnMemberSpec::new(ProfileName::from("worker"), member.clone());
        spawn.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        spawn.fresh_session_id = session;
        self.handle
            .spawn_spec(spawn)
            .await
            .expect("spawn the member");
        let session_id = self
            .handle
            .resolve_bridge_session_id(&member)
            .await
            .expect("session-backed member");
        (member, session_id)
    }

    /// Adopt `member`'s existing session under `custody`, owning `owned`.
    async fn adopt(
        &self,
        member: &AgentIdentity,
        session_id: &SessionId,
        custody: IdentityWiringCustody,
        owned: &[DesiredIdentityEdge],
    ) {
        let mut declaration = adoption(&self.mob_id, member, session_id);
        declaration.wiring_custody = custody;
        declaration.owned_wiring = owned.iter().cloned().collect();
        let outcome = self
            .handle
            .adopt_member_identity_declaration(declaration)
            .await
            .expect("adopt the existing member")
            .adoption;
        assert!(
            matches!(
                outcome,
                IdentityAdoptionOutcome::Adopted {
                    desired_revision: 1
                }
            ),
            "{member}: the adoption is recorded: {outcome:?}"
        );
    }

    /// The latest convergence status, once the reconciler recorded one.
    async fn status(&self, member: &AgentIdentity) -> Option<IdentityConvergenceStatus> {
        match self
            .handle
            .identity_convergence_status(member)
            .await
            .expect("read the convergence status")
        {
            IdentityStoredObservation::Valid(status) => Some(status),
            IdentityStoredObservation::Missing => None,
            other => panic!("{member}: the convergence status is unreadable: {other:?}"),
        }
    }

    /// Wait until `member`'s latest reconcile decision at `revision` is one
    /// that no further pass changes by itself (converged or repair-blocked),
    /// and return that status.
    async fn settled(&self, member: &AgentIdentity, revision: u64) -> IdentityConvergenceStatus {
        self.settled_within(member, revision, 0, STEP).await
    }

    /// [`Self::settled`] within `bound`, for a status under a lease epoch
    /// above `after_epoch` (a status of an earlier incarnation is not it).
    async fn settled_within(
        &self,
        member: &AgentIdentity,
        revision: u64,
        after_epoch: u64,
        bound: Duration,
    ) -> IdentityConvergenceStatus {
        let waited = tokio::time::timeout(bound, async {
            loop {
                if let Some(status) = self.status(member).await
                    && status.intent_revision == Some(revision)
                    && status.lease_epoch.is_some_and(|epoch| epoch > after_epoch)
                    && matches!(
                        status.decision,
                        Some(
                            IdentityReconcileDecision::Converged
                                | IdentityReconcileDecision::RepairBlocked
                        )
                    )
                {
                    break status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        match waited {
            Ok(status) => status,
            Err(_) => panic!(
                "{member}: no settled decision at revision {revision}; latest status: {:?}",
                self.status(member).await
            ),
        }
    }

    /// The stored intent record of an adopted member.
    async fn stored(&self, member: &AgentIdentity) -> crate::identity::IdentityIntentRecord {
        match self
            .identity
            .observe_identity_intent(&self.mob_id, member)
            .await
            .expect("read the identity intent")
        {
            IdentityStoredObservation::Valid(record) => record,
            other => panic!("{member}: the identity intent is unreadable: {other:?}"),
        }
    }

    /// The stored intent's owned edges and the stored plan's incident edges.
    async fn stored_wiring(
        &self,
        member: &AgentIdentity,
    ) -> (BTreeSet<DesiredIdentityEdge>, BTreeSet<DesiredIdentityEdge>) {
        let record = self.stored(member).await;
        let owned = match &record.intent {
            IdentityIntent::Present { owned_wiring, .. } => owned_wiring.clone(),
            IdentityIntent::Absent { .. } => panic!("{member}: the adopted intent is present"),
        };
        let incident = match &record.retirement_plan {
            IdentityRetirementPlan::Targets {
                incident_wiring, ..
            } => incident_wiring.clone(),
            IdentityRetirementPlan::NoKnownRealization => {
                panic!("{member}: a present intent carries its plan")
            }
        };
        (owned, incident)
    }

    /// The edge is still where the adoption recorded it: in its owner's
    /// declaration and, as an incident edge, in the far end's stored plan.
    async fn assert_edge_stays_stored(
        &self,
        edge: &DesiredIdentityEdge,
        owner: &AgentIdentity,
        far_end: &AgentIdentity,
    ) {
        let (owner_owned, owner_incident) = self.stored_wiring(owner).await;
        assert_eq!(owner_owned, BTreeSet::from([edge.clone()]));
        assert_eq!(owner_incident, BTreeSet::from([edge.clone()]));
        let (far_end_owned, far_end_incident) = self.stored_wiring(far_end).await;
        assert!(far_end_owned.is_empty(), "the far end owns no edge");
        assert_eq!(far_end_incident, BTreeSet::from([edge.clone()]));
    }

    /// The request that turns `member`'s shell tools on, against the stored
    /// revision `expected_revision`.
    async fn shell_change(
        &self,
        member: &AgentIdentity,
        expected_revision: u64,
    ) -> crate::identity::ApplyMemberToolDeclaration {
        let mut declaration = match &self.stored(member).await.intent {
            IdentityIntent::Present { member, .. } => member.material.member_tool_declaration(),
            IdentityIntent::Absent { .. } => panic!("{member}: the adopted intent is present"),
        };
        declaration.category_overrides.shell = ToolCategoryOverride::Enable;
        crate::identity::ApplyMemberToolDeclaration {
            mob_id: self.mob_id.clone(),
            agent_identity: member.clone(),
            request_id: crate::identity::MemberToolMutationId::new(format!(
                "enable-shell-{member}"
            ))
            .expect("valid mutation id"),
            expected_intent_revision: expected_revision,
            declaration,
            convergence: crate::identity::IdentityConvergenceMode::CancelActive,
        }
    }

    /// Whether the member's session was rebuilt with its shell tools on.
    async fn shell_is_on(&self, session_id: &SessionId) -> bool {
        MobSessionService::load_persisted_session(self.service.as_ref(), session_id)
            .await
            .expect("read the member's session")
            .and_then(|session| session.session_metadata())
            .map(|metadata| metadata.tooling.shell)
            == Some(ToolCategoryOverride::Enable)
    }

    /// Hand `member` a message.
    async fn send(&self, member: &AgentIdentity) -> Result<(), MobError> {
        self.handle
            .member(member)
            .await?
            .send("a message", HandlingMode::Queue)
            .await
            .map(|_| ())
    }

    async fn assert_takes_a_message(&self, member: &AgentIdentity) {
        if let Err(error) = self.send(member).await {
            panic!("{member} takes a message: {error}");
        }
    }

    /// Commit the change that turns `member`'s shell tools on, on the running
    /// mob, as desired revision 2.
    async fn change(&self, member: &AgentIdentity) {
        let update = self
            .handle
            .apply_member_tool_declaration(self.shell_change(member, 1).await)
            .await
            .expect("apply the tool declaration");
        assert!(
            matches!(
                update.commit,
                crate::identity::MemberToolCommitOutcome::Committed {
                    desired_revision: 2
                }
            ),
            "{member}: the change commits: {:?}",
            update.commit
        );
    }

    /// Commit the same change in the store of a mob that is shut down: the
    /// stored state of a change that committed and did not become active.
    async fn change_in_the_store(&self, member: &AgentIdentity) {
        let change = self.shell_change(member, 1).await;
        let commit = self
            .identity
            .apply_member_tool_declaration(&change)
            .await
            .expect("commit the change in the store");
        assert!(
            matches!(
                commit,
                crate::identity::MemberToolCommitOutcome::Committed {
                    desired_revision: 2
                }
            ),
            "{member}: the change commits: {commit:?}"
        );
        let carried = self.stored(member).await;
        assert_eq!(carried.intent_revision, 2);
        assert_eq!(
            carried
                .convergence_directive
                .as_ref()
                .map(|directive| directive.expected_active_revision),
            Some(1),
            "{member}: the stored change is not active"
        );
    }
}

fn edge(left: &AgentIdentity, right: &AgentIdentity) -> DesiredIdentityEdge {
    DesiredIdentityEdge::new(left.clone(), right.clone()).expect("a canonical edge")
}

fn assert_converged(member: &AgentIdentity, status: &IdentityConvergenceStatus) {
    assert_eq!(
        status.decision,
        Some(IdentityReconcileDecision::Converged),
        "{member} converges; latest status: {status:?}"
    );
    assert_eq!(
        status.active_intent_revision, status.intent_revision,
        "{member}: the desired revision is the active one: {status:?}"
    );
}

fn assert_blocked_for_its_wiring(member: &AgentIdentity, status: &IdentityConvergenceStatus) {
    assert_eq!(
        status.decision,
        Some(IdentityReconcileDecision::RepairBlocked),
        "{member} stays held to the wiring it owns; latest status: {status:?}"
    );
    assert!(
        status
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("ReconcileWiring")),
        "{member} is blocked for its wiring: {status:?}"
    );
}

/// An identity is adopted with an owned edge, then the edge's far end is
/// adopted under identity-owned custody, owning nothing: the store records
/// the edge as incident to the far end, and the far end converges and takes a
/// message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_far_end_of_another_identitys_owned_edge_converges() {
    let fixture = WiringFixture::create("owned-wiring-far-end").await;
    let (owner, owner_session) = fixture.seat("worker-a").await;
    let (far_end, far_end_session) = fixture.seat("worker-b").await;
    let owned_edge = edge(&owner, &far_end);
    assert_eq!(owned_edge.owner(), &owner);

    fixture
        .adopt(
            &owner,
            &owner_session,
            IdentityWiringCustody::IdentityOwned,
            std::slice::from_ref(&owned_edge),
        )
        .await;
    fixture
        .adopt(
            &far_end,
            &far_end_session,
            IdentityWiringCustody::IdentityOwned,
            &[],
        )
        .await;
    fixture
        .assert_edge_stays_stored(&owned_edge, &owner, &far_end)
        .await;

    assert_converged(&far_end, &fixture.settled(&far_end, 1).await);
    fixture.assert_takes_a_message(&far_end).await;
    fixture
        .assert_edge_stays_stored(&owned_edge, &owner, &far_end)
        .await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// The other order: the far end is adopted and converges, then another
/// identity is adopted with an edge to it. A later declaration change on the
/// far end commits, becomes active, and the far end takes a message again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_change_on_a_converged_far_end_becomes_active() {
    let fixture = WiringFixture::create("owned-wiring-far-end-change").await;
    let (owner, owner_session) = fixture.seat("worker-a").await;
    let (far_end, far_end_session) = fixture.seat("worker-b").await;
    let owned_edge = edge(&owner, &far_end);

    fixture
        .adopt(
            &far_end,
            &far_end_session,
            IdentityWiringCustody::IdentityOwned,
            &[],
        )
        .await;
    assert_converged(&far_end, &fixture.settled(&far_end, 1).await);
    fixture
        .adopt(
            &owner,
            &owner_session,
            IdentityWiringCustody::IdentityOwned,
            std::slice::from_ref(&owned_edge),
        )
        .await;
    fixture
        .assert_edge_stays_stored(&owned_edge, &owner, &far_end)
        .await;

    fixture.change(&far_end).await;
    let changed = fixture.settled(&far_end, 2).await;
    assert_converged(&far_end, &changed);
    assert_eq!(changed.active_intent_revision, Some(2));
    assert!(
        fixture.shell_is_on(&far_end_session).await,
        "the far end runs on its changed declaration"
    );
    fixture.assert_takes_a_message(&far_end).await;
    fixture
        .assert_edge_stays_stored(&owned_edge, &owner, &far_end)
        .await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// A store carried over a restart with two changes that committed and did not
/// become active (desired revision 2, active revision 1, each reported
/// repair-blocked): one on the far end of another identity's owned edge, one
/// on an identity that an ordinary wiring call joined to a larger-named
/// member. The next start makes the far end's change active by itself: the
/// far end converges and takes a message. The edge stays in the store, its
/// adopter stays blocked, and so does the identity held to its ordinary edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_far_end_stored_with_a_change_not_yet_active_converges_at_the_next_start() {
    let mut fixture = WiringFixture::create("owned-wiring-far-end-carried").await;
    let (owner, owner_session) = fixture.seat("worker-a").await;
    let (far_end, far_end_session) = fixture.seat("worker-b").await;
    let (wired, wired_session) = fixture.seat_with_comms("worker-c").await;
    let (larger, _larger_session) = fixture.seat_with_comms("worker-d").await;
    let owned_edge = edge(&owner, &far_end);

    fixture
        .adopt(
            &far_end,
            &far_end_session,
            IdentityWiringCustody::IdentityOwned,
            &[],
        )
        .await;
    let adopted = fixture.settled(&far_end, 1).await;
    assert_converged(&far_end, &adopted);
    fixture
        .adopt(
            &owner,
            &owner_session,
            IdentityWiringCustody::IdentityOwned,
            std::slice::from_ref(&owned_edge),
        )
        .await;
    assert_blocked_for_its_wiring(&owner, &fixture.settled(&owner, 1).await);
    fixture
        .adopt(
            &wired,
            &wired_session,
            IdentityWiringCustody::IdentityOwned,
            &[],
        )
        .await;
    let wired_adopted = fixture.settled(&wired, 1).await;
    assert_converged(&wired, &wired_adopted);
    fixture
        .handle
        .wire(wired.clone(), larger.clone())
        .await
        .expect("wire the member to a larger-named member");
    fixture.handle.shutdown().await.expect("shutdown the mob");

    // The stored state a host leaves when a change committed and the
    // reconcile pass then reported the identity repair-blocked for wiring:
    // the change at desired revision 2 with revision 1 active, and the status
    // that pass recorded.
    for (member, last) in [(&far_end, &adopted), (&wired, &wired_adopted)] {
        fixture.change_in_the_store(member).await;
        fixture
            .identity_status
            .replace_identity_convergence_status(
                &fixture.mob_id,
                &IdentityConvergenceStatus {
                    identity: member.clone(),
                    intent_revision: Some(2),
                    active_intent_revision: Some(1),
                    lease_epoch: last.lease_epoch,
                    decision: Some(IdentityReconcileDecision::RepairBlocked),
                    observed_at_ms: last.observed_at_ms,
                    detail: Some(
                        "generated obligation ReconcileWiring has no actuator in the retained sealed-intent reconciliation slice"
                            .to_string(),
                    ),
                },
            )
            .await
            .expect("record the carried status");
    }

    fixture.start_again().await;
    let after_epoch = adopted.lease_epoch.expect("the adoption held a lease");
    let started = fixture
        .settled_within(&far_end, 2, after_epoch, AFTER_START)
        .await;
    assert_converged(&far_end, &started);
    assert_eq!(started.active_intent_revision, Some(2));
    assert!(
        fixture.shell_is_on(&far_end_session).await,
        "the change committed before the start is active"
    );
    fixture.assert_takes_a_message(&far_end).await;
    fixture
        .assert_edge_stays_stored(&owned_edge, &owner, &far_end)
        .await;

    assert_blocked_for_its_wiring(
        &owner,
        &fixture
            .settled_within(&owner, 1, after_epoch, AFTER_START)
            .await,
    );
    let wired_after_epoch = wired_adopted
        .lease_epoch
        .expect("the adoption held a lease");
    let still_held = fixture
        .settled_within(&wired, 2, wired_after_epoch, AFTER_START)
        .await;
    assert_blocked_for_its_wiring(&wired, &still_held);
    assert_eq!(
        still_held.active_intent_revision,
        Some(1),
        "the change of the identity held to its ordinary edge is not active"
    );

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// The control: an identity adopted with an edge the mob does not have is
/// repair-blocked for its wiring, and stays so once the edge's far end has
/// been adopted and has settled, and after a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_adopter_whose_owned_edge_is_not_realized_stays_blocked() {
    let mut fixture = WiringFixture::create("owned-wiring-adopter-held").await;
    let (owner, owner_session) = fixture.seat("worker-a").await;
    let (far_end, far_end_session) = fixture.seat("worker-b").await;
    let owned_edge = edge(&owner, &far_end);

    fixture
        .adopt(
            &owner,
            &owner_session,
            IdentityWiringCustody::IdentityOwned,
            std::slice::from_ref(&owned_edge),
        )
        .await;
    let blocked = fixture.settled(&owner, 1).await;
    assert_blocked_for_its_wiring(&owner, &blocked);

    fixture
        .adopt(
            &far_end,
            &far_end_session,
            IdentityWiringCustody::IdentityOwned,
            &[],
        )
        .await;
    fixture.settled(&far_end, 1).await;
    // Whatever the far end settled on releases nothing of the adopter's
    // obligation.
    assert_blocked_for_its_wiring(&owner, &fixture.settled(&owner, 1).await);

    fixture.restart().await;
    let after_epoch = blocked.lease_epoch.expect("the adoption held a lease");
    assert_blocked_for_its_wiring(
        &owner,
        &fixture
            .settled_within(&owner, 1, after_epoch, AFTER_START)
            .await,
    );

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// The same when the host wired the edge first: two members joined by a
/// wiring call, then the smaller-named one adopted owning exactly that edge.
/// The adopter is repair-blocked for its wiring all the same, and the edge
/// stays recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_adopter_whose_owned_edge_was_wired_first_stays_blocked() {
    let fixture = WiringFixture::create("owned-wiring-adopter-wired-first").await;
    let (owner, owner_session) = fixture.seat_with_comms("worker-a").await;
    let (far_end, _far_end_session) = fixture.seat_with_comms("worker-b").await;
    fixture
        .handle
        .wire(owner.clone(), far_end.clone())
        .await
        .expect("wire the two members");
    let owned_edge = edge(&owner, &far_end);

    fixture
        .adopt(
            &owner,
            &owner_session,
            IdentityWiringCustody::IdentityOwned,
            std::slice::from_ref(&owned_edge),
        )
        .await;
    assert_blocked_for_its_wiring(&owner, &fixture.settled(&owner, 1).await);
    let (owner_owned, _) = fixture.stored_wiring(&owner).await;
    assert_eq!(owner_owned, BTreeSet::from([owned_edge]));

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// A far end under external-managed custody converges as before: the
/// reconciler abstains from its wiring whatever the store records as incident.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_far_end_under_external_managed_custody_converges_as_before() {
    let fixture = WiringFixture::create("owned-wiring-external-far-end").await;
    let (owner, owner_session) = fixture.seat("worker-a").await;
    let (far_end, far_end_session) = fixture.seat("worker-b").await;
    let owned_edge = edge(&owner, &far_end);

    fixture
        .adopt(
            &owner,
            &owner_session,
            IdentityWiringCustody::IdentityOwned,
            std::slice::from_ref(&owned_edge),
        )
        .await;
    fixture
        .adopt(
            &far_end,
            &far_end_session,
            IdentityWiringCustody::ExternalManaged,
            &[],
        )
        .await;
    fixture
        .assert_edge_stays_stored(&owned_edge, &owner, &far_end)
        .await;
    assert_converged(&far_end, &fixture.settled(&far_end, 1).await);
    fixture.assert_takes_a_message(&far_end).await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// Ordinary wiring, the larger-named end: an identity under identity-owned
/// custody that a wiring call joins to a smaller-named member owns nothing of
/// that edge. Its next declaration change becomes active and it converges.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_identity_wired_to_a_smaller_named_member_converges_at_its_next_change() {
    let fixture = WiringFixture::create("ordinary-wiring-larger-end").await;
    let (smaller, _smaller_session) = fixture.seat_with_comms("worker-c").await;
    let (adopted, adopted_session) = fixture.seat_with_comms("worker-d").await;

    fixture
        .adopt(
            &adopted,
            &adopted_session,
            IdentityWiringCustody::IdentityOwned,
            &[],
        )
        .await;
    assert_converged(&adopted, &fixture.settled(&adopted, 1).await);
    fixture
        .handle
        .wire(smaller.clone(), adopted.clone())
        .await
        .expect("wire the member to a smaller-named member");

    fixture.change(&adopted).await;
    let changed = fixture.settled(&adopted, 2).await;
    assert_converged(&adopted, &changed);
    assert_eq!(changed.active_intent_revision, Some(2));
    fixture.assert_takes_a_message(&adopted).await;

    fixture.handle.shutdown().await.expect("shutdown the mob");
}

/// Ordinary wiring, the smaller-named end: an identity under identity-owned
/// custody owns every edge toward a larger-named member, and its declaration
/// states none. A wiring call that joins it to a larger-named member is
/// accepted; at its next declaration change the identity is held to the edge
/// its declaration does not state: repair-blocked for its wiring, the change
/// not active.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_identity_wired_to_a_larger_named_member_is_held_at_its_next_change() {
    let fixture = WiringFixture::create("ordinary-wiring-smaller-end").await;
    let (adopted, adopted_session) = fixture.seat_with_comms("worker-c").await;
    let (larger, _larger_session) = fixture.seat_with_comms("worker-d").await;

    fixture
        .adopt(
            &adopted,
            &adopted_session,
            IdentityWiringCustody::IdentityOwned,
            &[],
        )
        .await;
    assert_converged(&adopted, &fixture.settled(&adopted, 1).await);
    fixture
        .handle
        .wire(adopted.clone(), larger.clone())
        .await
        .expect("wire the member to a larger-named member");

    fixture.change(&adopted).await;
    let held = fixture.settled(&adopted, 2).await;
    assert_blocked_for_its_wiring(&adopted, &held);
    assert_eq!(
        held.active_intent_revision,
        Some(1),
        "the change is not active"
    );

    fixture.handle.shutdown().await.expect("shutdown the mob");
}
