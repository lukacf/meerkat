//! An identity's scoped tool grants end with the identity, whichever step
//! ends it; and the member grant gate's own branches: an identity's role
//! gate less the families its declaration disables, the material sealed
//! before its profile source was recorded, and a placed member's gate.

use super::*;
use crate::identity::{DesiredMemberMaterial, DesiredProfileSource, IdentityIntent};
use meerkat_core::tool_grant::{
    GrantGatedTools, ToolGrantIssuer, ToolGrantSubject, ToolGrantTarget,
};
use meerkat_runtime::tool_grant::{ToolGrantRequest, ToolGrantStatus, ToolGrantWindow};

/// The sample mob with its worker role gating `families`.
fn gated_definition(families: &[&str]) -> MobDefinition {
    let mut definition = sample_definition();
    let Some(ProfileBinding::Inline(worker)) =
        definition.profiles.get_mut(&ProfileName::from("worker"))
    else {
        panic!("the sample's worker role is an inline profile");
    };
    for family in families {
        worker.tools.grant_gated = worker.tools.grant_gated.clone().with_family(*family);
    }
    definition
}

/// An operator's grant of the shell family to `subject`.
fn shell_grant(subject: &ToolGrantSubject) -> ToolGrantRequest {
    ToolGrantRequest {
        subject: subject.clone(),
        target: ToolGrantTarget::Family {
            family: "shell".to_string(),
            names: std::iter::once(meerkat_core::types::ToolName::from("shell")).collect(),
        },
        issuer: ToolGrantIssuer::Operator {
            caller: None,
            decision_ref: None,
        },
        reason: None,
        window: ToolGrantWindow::For(Duration::from_mins(10)),
    }
}

/// The subject of `identity`'s grants in `handle`'s mob incarnation.
fn identity_subject(handle: &MobHandle, identity: &AgentIdentity) -> ToolGrantSubject {
    ToolGrantSubject::member(
        handle.mob_id().as_str(),
        &handle.live_incarnation(),
        identity.as_str(),
    )
}

/// A respawn holds its identity past the old member's archive: the archive
/// keeps the identity's grants for the successor. When the replacement
/// fails after that archive, the respawn is abandoned, which ends the
/// identity, and its grants end with it: a member spawned later under the
/// name inherits none.
#[tokio::test]
async fn a_respawn_abandoned_after_its_old_members_archive_ends_the_identitys_grants() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let (handle, service) = create_test_mob_with_events(sample_definition(), events.clone()).await;
    let runtime = service.enable_runtime_adapter();
    let identity = AgentIdentity::from("respawn-abandoned");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            identity.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn the member");
    let subject = identity_subject(&handle, &identity);
    let grant = runtime
        .grant_tool(shell_grant(&subject), None)
        .await
        .expect("the identity is granted");

    // The replacement's membership never commits, so the respawn fails
    // after the old member's archive.
    events.fail_appends_for("MemberSpawned").await;
    let respawned = handle.respawn(identity.clone(), None).await;
    assert!(respawned.is_err(), "the replacement fails: {respawned:?}");
    assert!(
        handle
            .get_member(&identity)
            .await
            .expect("the member lookup")
            .is_none(),
        "the abandoned respawn leaves no member under the name"
    );
    let listing = runtime
        .list_tool_grants(&subject, None)
        .await
        .expect("the identity's grants list");
    assert_eq!(listing.grants.len(), 1, "{listing:?}");
    assert_eq!(listing.grants[0].record.grant_id, grant.record.grant_id);
    assert_eq!(
        listing.grants[0].status,
        ToolGrantStatus::Revoked,
        "the abandoned respawn ended the identity's grant"
    );
    handle.shutdown().await.expect("shutdown test mob");
}

/// Bounds a hang only: a passing row never waits for it.
const STEP: Duration = Duration::from_secs(30);

/// A mob over a store-backed runtime, with every store kept, so a resume of
/// the mob reads what its first actor wrote.
struct RecoverableMob {
    events: Arc<FaultInjectedMobEventStore>,
    service: Arc<MockSessionService>,
    runtime_store: Arc<meerkat_runtime::InMemoryRuntimeStore>,
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    runs: Arc<InMemoryMobRunStore>,
    specs: Arc<InMemoryMobSpecStore>,
    runtime_metadata: Arc<InMemoryMobRuntimeMetadataStore>,
    identity_store: Arc<InMemoryMobIdentityStore>,
    identity_status: Arc<InMemoryMobIdentityStatusStore>,
}

impl RecoverableMob {
    fn new() -> Self {
        let runtime_store = Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let runtime = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent_without_blobs(
                Arc::clone(&runtime_store) as Arc<dyn meerkat_runtime::RuntimeStore>
            )
            .expect("construct runtime authority"),
        );
        let service = Arc::new(MockSessionService::new());
        service.set_runtime_adapter(Arc::clone(&runtime));
        Self {
            events: Arc::new(FaultInjectedMobEventStore::new()),
            service,
            runtime_store,
            runtime,
            runs: Arc::new(InMemoryMobRunStore::new()),
            specs: Arc::new(InMemoryMobSpecStore::new()),
            runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
            identity_store: Arc::new(InMemoryMobIdentityStore::new()),
            identity_status: Arc::new(InMemoryMobIdentityStatusStore::new()),
        }
    }

    fn storage(&self) -> MobStorage {
        MobStorage::custom_with_runtime_metadata(
            self.events.clone(),
            self.runs.clone(),
            self.specs.clone(),
            self.runtime_metadata.clone(),
            self.identity_store.clone(),
            self.identity_status.clone(),
        )
    }

    async fn create(&self) -> MobHandle {
        MobBuilder::new(sample_definition(), self.storage())
            .with_session_service(self.service.clone())
            .create()
            .await
            .expect("create mob")
    }

    async fn resume(&self) -> MobHandle {
        MobBuilder::for_resume(self.storage())
            .with_session_service(self.service.clone())
            .notify_orchestrator_on_resume(false)
            .resume()
            .await
            .expect("the resume")
    }

    /// Spawn `identity` in `handle`'s mob and grant it `shell`.
    async fn spawn_granted(
        &self,
        handle: &MobHandle,
        identity: &AgentIdentity,
    ) -> meerkat_runtime::tool_grant::StoredToolGrant {
        handle
            .spawn_with_options(
                ProfileName::from("worker"),
                identity.clone(),
                None,
                Some(crate::MobRuntimeMode::TurnDriven),
                None,
            )
            .await
            .expect("spawn the member");
        self.runtime
            .grant_tool(shell_grant(&identity_subject(handle, identity)), None)
            .await
            .expect("the identity is granted")
    }
}

/// A respawn whose replacement fails after the old member's archive, when
/// the live abandonment cannot end the identity's grants (the grant store
/// fails, a failure of no busy class), stops the mob's actor before the
/// abandonment's row is written: the mob waits for its resume. The resume
/// abandons the respawn itself, which ends the identity, and it ends the
/// identity's grants before the mob serves.
#[tokio::test]
async fn a_respawn_a_resume_abandons_ends_the_identitys_grants() {
    let mob = RecoverableMob::new();
    let handle = mob.create().await;
    let identity = AgentIdentity::from("respawn-recovered");
    let grant = mob.spawn_granted(&handle, &identity).await;
    let subject = identity_subject(&handle, &identity);

    mob.events.fail_appends_for("MemberSpawned").await;
    mob.runtime_store
        .fail_tool_grant_store_for_test(Some("grant store offline"));
    let respawned = tokio::time::timeout(STEP, handle.respawn(identity.clone(), None))
        .await
        .expect("the respawn answers within the bound");
    mob.runtime_store.fail_tool_grant_store_for_test(None);
    mob.events.allow_appends_for("MemberSpawned").await;
    assert!(respawned.is_err(), "the respawn fails: {respawned:?}");
    let before = mob
        .runtime
        .list_tool_grants(&subject, None)
        .await
        .expect("the identity's grants list");
    assert_eq!(
        before
            .grants
            .iter()
            .map(|listed| listed.status)
            .collect::<Vec<_>>(),
        vec![ToolGrantStatus::Active],
        "the stopped actor ended no grant"
    );

    let resumed = mob.resume().await;
    let journal = mob.events.replay_all().await.expect("the journal");
    assert_eq!(
        journal
            .iter()
            .filter(|event| matches!(event.kind, MobEventKind::RespawnTopologyAbandoned { .. }))
            .count(),
        1,
        "the resume wrote the respawn's abandonment"
    );
    assert!(
        resumed
            .get_member(&identity)
            .await
            .expect("the member lookup")
            .is_none(),
        "the resume abandoned the respawn: no member holds the name"
    );
    let listing = mob
        .runtime
        .list_tool_grants(&subject, None)
        .await
        .expect("the identity's grants list");
    assert_eq!(listing.grants.len(), 1, "{listing:?}");
    assert_eq!(listing.grants[0].record.grant_id, grant.record.grant_id);
    assert_eq!(
        listing.grants[0].status,
        ToolGrantStatus::Revoked,
        "the resume's abandonment ended the identity's grant"
    );
    resumed.shutdown().await.expect("shutdown resumed mob");
}

/// When the abandonment's grant end meets a concurrent writer that changed a
/// grant twice (the busy class), the abandonment retries it within its
/// bound: the identity's grant ends, the respawn answers its own failure,
/// and the mob keeps serving.
#[tokio::test]
async fn an_abandonments_busy_grant_end_is_retried_and_the_mob_keeps_serving() {
    let mob = RecoverableMob::new();
    let handle = mob.create().await;
    let identity = AgentIdentity::from("respawn-busy");
    let grant = mob.spawn_granted(&handle, &identity).await;
    let subject = identity_subject(&handle, &identity);

    mob.events.fail_appends_for("MemberSpawned").await;
    // One attempt meets two conflicts, so the grant service answers the busy
    // class (it retries a conflict once); the next attempt meets one and
    // commits.
    mob.runtime_store.conflict_tool_grant_writes_for_test(3);
    let respawned = tokio::time::timeout(STEP, handle.respawn(identity.clone(), None))
        .await
        .expect("the respawn answers within the bound");
    mob.runtime_store.conflict_tool_grant_writes_for_test(0);
    mob.events.allow_appends_for("MemberSpawned").await;
    assert!(respawned.is_err(), "the replacement fails: {respawned:?}");
    let listing = mob
        .runtime
        .list_tool_grants(&subject, None)
        .await
        .expect("the identity's grants list");
    assert_eq!(listing.grants.len(), 1, "{listing:?}");
    assert_eq!(listing.grants[0].record.grant_id, grant.record.grant_id);
    assert_eq!(
        listing.grants[0].status,
        ToolGrantStatus::Revoked,
        "the retried end ended the identity's grant"
    );
    tokio::time::timeout(
        STEP,
        handle.spawn_with_options(
            ProfileName::from("worker"),
            AgentIdentity::from("after-busy"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        ),
    )
    .await
    .expect("the spawn answers within the bound")
    .expect("the mob keeps serving after a busy grant end");
    handle.shutdown().await.expect("shutdown test mob");
}

/// `identity`'s sealed intent in a fresh identity store of `definition`'s
/// mob, from the adoption fixture's material changed by `edit`.
async fn store_with_intent(
    definition: &MobDefinition,
    identity: &AgentIdentity,
    edit: impl FnOnce(&mut DesiredMemberMaterial),
) -> InMemoryMobIdentityStore {
    let (request, record) = crate::identity::identity_adoption_fixture_with(
        definition.id.clone(),
        identity.clone(),
        "grant-gate-intent",
        SessionId::new(),
        edit,
    );
    let store = InMemoryMobIdentityStore::new();
    store
        .adopt_member_identity_declaration(&request, &record)
        .await
        .expect("the identity is adopted");
    store
}

/// An identity declared with its role's named profile takes its role's
/// gate as its own, less the families its declaration's category overrides
/// disable: an explicit disable means no such tools, where the gate would
/// mount a gated family closed.
#[tokio::test]
async fn an_identitys_role_gate_leaves_out_the_families_its_declaration_disables() {
    let definition = gated_definition(&["shell", "memory"]);
    let identity = AgentIdentity::from("disables-shell");
    let store = store_with_intent(&definition, &identity, |material| {
        material.overlay.tool_category_overrides.shell = ToolCategoryOverride::Disable;
    })
    .await;
    let gate = crate::runtime::member_grant_gate::identity_role_gate(
        &store,
        &definition,
        None,
        &identity,
        &ProfileName::from("worker"),
    )
    .await
    .expect("the identity's gate resolves");
    assert_eq!(
        gate,
        Some(GrantGatedTools::default().with_family("memory")),
        "the disabled shell family leaves the identity's own gate"
    );
}

/// Material sealed before it recorded where its profile came from keeps the
/// rule of the release that sealed it: its role's gate joins as role entries
/// only, never as the identity's own gate.
#[tokio::test]
async fn material_with_no_recorded_profile_source_keeps_role_entries_only() {
    let definition = gated_definition(&["shell"]);
    let identity = AgentIdentity::from("legacy-material");
    let store = store_with_intent(&definition, &identity, |material| {
        *material = legacy_material(material);
    })
    .await;
    let gate = crate::runtime::member_grant_gate::identity_role_gate(
        &store,
        &definition,
        None,
        &identity,
        &ProfileName::from("worker"),
    )
    .await
    .expect("the identity's gate resolves");
    assert_eq!(
        gate, None,
        "material with no recorded source takes no role gate as its own"
    );
}

/// `material` as a build that did not record its profile source wrote it:
/// its serialized form without the `profile_source` field, read back.
fn legacy_material(material: &DesiredMemberMaterial) -> DesiredMemberMaterial {
    let mut value = serde_json::to_value(material).expect("the material serializes");
    value
        .as_object_mut()
        .expect("the material is an object")
        .remove("profile_source");
    serde_json::from_value(value).expect("a material with no source reads")
}

/// New material records where its profile came from, the role's named
/// profile included, so material with no recorded source is told apart
/// from it.
#[test]
fn a_materials_profile_source_is_recorded_and_its_absence_is_no_definition() {
    let (_, record) = crate::identity::identity_adoption_fixture(
        MobId::from("profile-source-mob"),
        AgentIdentity::from("worker-1"),
        "profile-source",
        SessionId::new(),
    );
    let IdentityIntent::Present { member, .. } = &record.intent else {
        panic!("the fixture's intent is present");
    };
    let material = member.material.clone();
    assert_eq!(material.profile_source, DesiredProfileSource::Definition);
    let value = serde_json::to_value(&material).expect("the material serializes");
    assert_eq!(
        value["profile_source"], "definition",
        "a material records that it took the role's named profile"
    );
    assert_ne!(
        legacy_material(&material).profile_source,
        DesiredProfileSource::Definition,
        "a material with no recorded source is not read as the role's named profile"
    );
}

/// A member placed on another host carries no gate (its placement refuses
/// one), so its gate answers empty even when its role gates a family: a
/// grant to it is refused as gating nothing.
#[tokio::test]
async fn a_placed_member_answers_the_empty_gate_whatever_its_role_gates() {
    let (handle, _service) = create_test_mob(gated_definition(&["shell"])).await;
    let template = AgentIdentity::from("local-worker");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            template.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn a local worker");
    assert!(
        !handle
            .member_grant_gate(&template)
            .await
            .expect("the local worker's gate")
            .gated
            .is_empty(),
        "control: a local member of the role is gated"
    );

    // `member-a`, a worker placed on `host-a`, and its roster entry.
    seed_operator_execution_residency(&handle, "placed-member-session").await;
    let placed = AgentIdentity::from("member-a");
    let original = handle.roster.read().await.snapshot();
    {
        let mut entry = original
            .get(&template)
            .cloned()
            .expect("the local worker's roster entry");
        entry.agent_identity = placed.clone();
        entry.agent_runtime_id = AgentRuntimeId::new(placed.clone(), Generation::new(0));
        entry.member_ref = crate::event::MemberRef::from_bridge_session_id(SessionId::new());
        let entries = original.list_all().cloned().chain(std::iter::once(entry));
        *handle.roster.write().await = super::super::roster_authority::RosterAuthority::from_roster(
            crate::roster::Roster::from_projected_entries(entries),
        );
    }
    assert!(
        handle
            .machine_state_watch_rx
            .borrow()
            .member_placement
            .contains_key(&crate::machines::mob_machine::AgentIdentity::from_domain(
                &placed
            )),
        "control: the member is placed"
    );
    assert_eq!(
        handle
            .member_grant_gate(&placed)
            .await
            .expect("the placed member's gate"),
        crate::runtime::MemberGrantGate::default(),
        "a placed member answers the empty gate"
    );
    *handle.roster.write().await =
        super::super::roster_authority::RosterAuthority::from_roster(original);
    handle.shutdown().await.expect("shutdown test mob");
}

/// A grant end that met a concurrent writer twice answers the busy class
/// (`SESSION_BUSY` on the wire); any other failure to end the grants stays
/// internal. The row pins the mapping; the abandonment's retry of the busy
/// class has its own row
/// (`an_abandonments_busy_grant_end_is_retried_and_the_mob_keeps_serving`).
#[test]
fn a_grant_end_that_meets_a_concurrent_writer_answers_the_busy_class() {
    use meerkat_runtime::tool_grant::ToolGrantServiceError;
    let identity = AgentIdentity::from("busy-identity");
    let busy = super::super::actor::tool_grant_end_failure(
        &identity,
        ToolGrantServiceError::Conflict {
            grant_id: "grant-1".to_string(),
        },
    );
    assert!(
        matches!(busy, MobError::LifecycleOperationPending { .. }),
        "a concurrent writer answers the busy class: {busy:?}"
    );
    assert_eq!(
        busy.wire_error_code(),
        Some(meerkat_contracts::ErrorCode::SessionBusy)
    );
    let internal =
        super::super::actor::tool_grant_end_failure(&identity, ToolGrantServiceError::Clock);
    assert!(
        matches!(internal, MobError::Internal(_)),
        "any other failure stays internal: {internal:?}"
    );
}
