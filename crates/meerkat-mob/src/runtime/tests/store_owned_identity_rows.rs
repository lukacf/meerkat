//! Store-owned identities (0.9.9): the commit of a create in Core's identity
//! store, its seats, its admission by the ordinary spawn grant and the
//! grant's recheck at the commit, the change's consent, holds, candidates
//! and storage compositions. They run on the combined candidate: the
//! machine, the actor, the identity store, the journal and the receipts
//! together.

use super::*;
use crate::identity::{
    CreatedIdentityObservation, CreatedIdentityPresence, CreatedIdentitySpec,
    IDENTITY_CHANGE_BATCH_MAX, IdentityChangeConsent, IdentityChangeEntryOutcome,
    IdentityChangeKind, IdentityChangeRequester, IdentityPolicyOutcome, IdentityPreparationMode,
};
use crate::runtime::{
    CreatedIdentityRequest, DeclaredEntry, DeclaredRoster, EntryStanding,
    IdentityChangeConsentInput, IdentityChangeOutcome, IdentityChangeRefusal,
    IdentityChangeRequesterInput, IdentityEntryOrigin, IdentityEntryRead, IdentityGrantRequest,
    IdentityNameHolder, IdentityOperation, MemberGrantMint, MemberGrantMintRequest,
    MobCompositionAuthority, StoreOwnedIdentityCreate, UnseatedEntryCause,
};
use meerkat_core::service::MobToolAuthorityContext;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const STEP: Duration = Duration::from_mins(1);

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

/// The rows' mob: the sample definition with mob tools on the worker
/// profile, every profile turn-driven, so a fresh seat owes no kickoff and
/// is seated Dormant.
fn store_owned_definition(label: &str) -> MobDefinition {
    let mut definition = with_unique_mob_id(sample_definition_with_mob_tools(), label);
    for binding in definition.profiles.values_mut() {
        if let Some(profile) = binding.as_inline_mut() {
            profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
        }
    }
    definition
}

/// A store-owned identity's spec of `name` on `profile`, with labels, a
/// context and instructions: everything a seat carries.
fn spec(name: &str, profile: &str) -> CreatedIdentitySpec {
    CreatedIdentitySpec::new(name, profile)
        .with_display_name(format!("The {name} identity"))
        .with_labels(BTreeMap::from([
            ("team".to_string(), "alpha".to_string()),
            ("desk".to_string(), name.to_string()),
        ]))
        .with_context(serde_json::json!({"desk": name}))
        .with_additional_instructions(vec![format!("You are {name}.")])
}

/// The canonical digest of `operation`'s request, as a host computes one.
fn digest(operation: &str) -> String {
    meerkat_core::approval::decision::Sha256Digest::of(operation.as_bytes())
        .as_str()
        .to_string()
}

fn operation(name: &str) -> IdentityOperation {
    IdentityOperation::new(name, digest(name))
}

/// A host surface's create of `names` on the worker profile as `operation`.
fn host_create(operation_id: &str, names: &[&str]) -> StoreOwnedIdentityCreate {
    StoreOwnedIdentityCreate::new(
        operation(operation_id),
        IdentityChangeRequesterInput::host("console"),
        names
            .iter()
            .map(|name| CreatedIdentityRequest::new(spec(name, "worker")))
            .collect(),
    )
}

const ALLOW: IdentityChangeConsentInput<'static> =
    IdentityChangeConsentInput::Policy(IdentityPolicyOutcome::Allow);

/// The typed refusal of a refused change.
fn refusal<T: std::fmt::Debug>(result: Result<T, MobError>) -> IdentityChangeRefusal {
    match result {
        Err(MobError::IdentityChangeRefused { refusal }) => refusal,
        other => panic!("expected an identity change refusal, got {other:?}"),
    }
}

/// The receipt of a change that committed now.
fn committed(result: Result<IdentityChangeOutcome, MobError>) -> crate::IdentityChangeReceipt {
    match result {
        Ok(IdentityChangeOutcome::Committed { receipt }) => receipt,
        other => panic!("expected a committed change, got {other:?}"),
    }
}

/// A running mob over the mock durable session service, with session-scoped
/// member comms identities, over in-memory storage or the storage a row
/// gives. A restart resumes the same storage over a cold-restarted session
/// service.
struct StoreOwnedMob {
    keys: tempfile::TempDir,
    storage: MobStorage,
    handle: MobHandle,
    service: Arc<MockSessionService>,
}

impl StoreOwnedMob {
    async fn new(label: &str) -> Self {
        Self::build(
            store_owned_definition(label),
            MobStorage::in_memory(),
            |builder| builder,
        )
        .await
    }

    async fn build(
        definition: MobDefinition,
        storage: MobStorage,
        configure: impl FnOnce(MobBuilder) -> MobBuilder,
    ) -> Self {
        let keys = tempfile::tempdir().expect("a comms identity root");
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let builder = MobBuilder::new(definition, storage.clone())
            .with_session_service(service.clone())
            .with_member_comms_identity_root(keys.path());
        let handle = configure(builder).create().await.expect("create the mob");
        Self {
            keys,
            storage,
            handle,
            service,
        }
    }

    /// Crash the mob and resume its storage as the composition `authority`
    /// over a cold-restarted session service.
    async fn restart_as(&mut self, authority: MobCompositionAuthority) {
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(self.handle.clone()).await;
        self.service = restarted;
        self.handle = tokio::time::timeout(
            STEP,
            Box::pin(
                MobBuilder::for_resume(self.storage.clone())
                    .with_session_service(self.service.clone())
                    .with_member_comms_identity_root(self.keys.path())
                    .with_composition_authority(authority)
                    .notify_orchestrator_on_resume(false)
                    .resume(),
            ),
        )
        .await
        .expect("the restart answers")
        .expect("the restart succeeds");
    }

    fn mob_id(&self) -> MobId {
        self.handle.mob_id().clone()
    }

    async fn create(
        &self,
        create: StoreOwnedIdentityCreate,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        tokio::time::timeout(
            STEP,
            self.handle.create_store_owned_identities(create, consent),
        )
        .await
        .expect("the change answers")
    }

    /// Every store-owned row of the mob, by identity.
    async fn rows(&self) -> BTreeMap<AgentIdentity, CreatedIdentityObservation> {
        let mut rows = BTreeMap::new();
        let mut after = None;
        loop {
            let page = self
                .storage
                .identity
                .scan_created_identities_page(
                    &self.mob_id(),
                    after.as_ref(),
                    crate::store::IDENTITY_INTENT_SCAN_PAGE_MAX,
                )
                .await
                .expect("scan the created rows");
            rows.extend(page.observations);
            match page.next {
                Some(next) => after = Some(next),
                None => return rows,
            }
        }
    }

    async fn journal(&self) -> Vec<MobEvent> {
        self.storage
            .events
            .replay_all()
            .await
            .expect("read the journal")
    }

    /// `name`'s last `MemberSpawned` row.
    async fn spawn_row(&self, name: &str) -> crate::event::MemberSpawnedEvent {
        self.journal()
            .await
            .into_iter()
            .rev()
            .find_map(|event| match event.kind {
                MobEventKind::MemberSpawned(spawned) if spawned.agent_identity == id(name) => {
                    Some(spawned)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} has no spawn row"))
    }

    async fn is_member(&self, name: &str) -> bool {
        self.handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .is_some()
    }

    async fn wired_to(&self, name: &str) -> BTreeSet<AgentIdentity> {
        self.handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .map(|entry| entry.wired_to)
            .unwrap_or_default()
    }

    async fn session(&self, name: &str) -> SessionId {
        self.handle
            .resolve_bridge_session_id(&id(name))
            .await
            .unwrap_or_else(|| panic!("{name} is bound to a session"))
    }

    fn read(&self, name: &str) -> IdentityEntryRead {
        self.handle
            .identity_entry(&id(name))
            .expect("the entry read answers")
    }

    /// Whether `name` is a store-owned identity seated Dormant.
    fn seated_dormant(&self, name: &str) -> bool {
        matches!(
            self.read(name),
            IdentityEntryRead::Entry(view)
                if view.origin == IdentityEntryOrigin::Created
                    && matches!(view.standing, EntryStanding::Seated(_))
        ) && self.handle.member_unmaterialized(&id(name))
    }

    async fn builds(&self) -> usize {
        self.service.recorded_create_requests().await.len()
    }

    /// Spawn `name`, live, on the worker profile (which has mob tools): an
    /// agent requester.
    async fn spawn_requester(&self, name: &str) -> SessionId {
        self.handle
            .spawn(ProfileName::from("worker"), id(name), None)
            .await
            .expect("spawn the requester");
        self.session(name).await
    }

    /// Admit `requester_session`'s change `operation_id` of `specs`.
    async fn admit(
        &self,
        requester_session: &SessionId,
        operation_id: &str,
        specs: &[CreatedIdentitySpec],
    ) -> Result<crate::runtime::IdentityChangeGrant, MobError> {
        self.handle
            .admit_identity_change_grant(
                requester_session,
                &operation(operation_id),
                &IdentityGrantRequest::for_specs(specs),
            )
            .await
    }

    /// An agent requester's create of `specs` as `operation_id`, under its
    /// admitted `grant`.
    fn agent_create(
        operation_id: &str,
        grant: crate::runtime::IdentityChangeGrant,
        specs: Vec<CreatedIdentitySpec>,
    ) -> StoreOwnedIdentityCreate {
        StoreOwnedIdentityCreate::new(
            operation(operation_id),
            IdentityChangeRequesterInput::Agent { grant },
            specs.into_iter().map(CreatedIdentityRequest::new).collect(),
        )
    }
}

/// The grants a row's host mints: each listed member's spawn grant narrowed
/// to its profiles, with manage scope over the mob when its flag is set.
type MemberGrants = Vec<(&'static str, Vec<&'static str>, bool)>;

/// A host that mints [`MemberGrants`] in its member build hook and, for the
/// reads of a grant that build nothing, in its grant mint: the same rule in
/// both, as a host passes it.
struct HostGrants(MemberGrants);

impl HostGrants {
    /// `member`'s grant over `authority`, when the host narrows it.
    fn narrow(
        &self,
        mob_id: &str,
        member: &str,
        incarnation: meerkat_core::service::LiveMobIncarnation,
        authority: &MobToolAuthorityContext,
    ) -> Option<MobToolAuthorityContext> {
        let (_, profiles, manage) = self.0.iter().find(|(name, _, _)| *name == member)?;
        let mut grant = meerkat_runtime::mob_operator_authority::restrict_spawn_profiles_in_mob(
            authority,
            mob_id,
            profiles.iter().copied(),
        )
        .expect("narrow the member's spawn grant");
        if *manage {
            grant = meerkat_runtime::mob_operator_authority::grant_manage_mob(
                &grant,
                mob_id.to_string(),
                incarnation,
            )
            .expect("grant manage scope");
        }
        Some(grant)
    }
}

impl MemberGrantMint for HostGrants {
    fn mint(&self, request: &MemberGrantMintRequest<'_>) -> Option<MobToolAuthorityContext> {
        let composed = request.composed?;
        Some(
            self.narrow(
                request.mob_id.as_str(),
                request.agent_identity.as_str(),
                request.incarnation,
                composed,
            )
            .unwrap_or_else(|| composed.clone()),
        )
    }
}

impl StoreOwnedMob {
    /// [`Self::new`] over a host that mints `grants` (its member build hook,
    /// and its grant mint).
    async fn with_grants(label: &str, grants: MemberGrants) -> Self {
        let host = Arc::new(HostGrants(grants));
        let mint: Arc<dyn MemberGrantMint> = host.clone();
        let mob = Self::build(
            store_owned_definition(label),
            MobStorage::in_memory(),
            move |builder| builder.with_member_grant_mint(mint),
        )
        .await;
        mob.service
            .set_member_build_hook(Arc::new(move |req: &mut CreateSessionRequest| {
                let Some(build) = req.build.as_mut() else {
                    return;
                };
                let Some(member) = build.mob_member_binding.clone() else {
                    return;
                };
                let (Some(authority), Some(witness)) = (
                    build.mob_tool_authority_context.as_ref(),
                    build.mob_incarnation,
                ) else {
                    return;
                };
                if let Some(grant) =
                    host.narrow(&member.mob_id, member.member.as_str(), witness, authority)
                {
                    build.mob_tool_authority_context = Some(grant);
                }
            }));
        mob
    }
}

// ---------------------------------------------------------------------------
// R2.1: the commit
// ---------------------------------------------------------------------------

/// Row (RED): a host's create of two identities commits in one change: each
/// identity's row (revision 1, its spec, its provenance), its seat's rows
/// and the receipt. Both are seated Dormant, store-owned.
///
/// RED on the old tree: no store-owned create exists (the commit answers an
/// internal error).
#[tokio::test]
async fn a_create_commits_store_owned_identities_seated_dormant_in_one_transaction() {
    let mob = StoreOwnedMob::new("create-commit").await;
    let receipt = committed(
        mob.create(host_create("create-1", &["alpha", "beta"]), ALLOW)
            .await,
    );
    assert_eq!(receipt.change, IdentityChangeKind::Create);
    assert_eq!(receipt.operation_id, "create-1");
    assert_eq!(receipt.request_digest, digest("create-1"));
    assert_eq!(
        receipt.requester,
        IdentityChangeRequester::Host {
            surface: "console".to_string()
        }
    );
    assert_eq!(
        receipt.consent,
        IdentityChangeConsent::Policy {
            outcome: IdentityPolicyOutcome::Allow
        }
    );
    let entries = receipt
        .identities
        .iter()
        .map(|entry| (entry.identity.clone(), entry.clone()))
        .collect::<BTreeMap<_, _>>();
    let rows = mob.rows().await;
    for name in ["alpha", "beta"] {
        let entry = &entries[&id(name)];
        assert_eq!(entry.outcome, IdentityChangeEntryOutcome::Committed);
        assert_eq!(entry.revision, Some(1));
        let Some(CreatedIdentityObservation::Created(row)) = rows.get(&id(name)) else {
            panic!("{name} has no readable row: {rows:?}");
        };
        assert_eq!(row.revision, 1);
        assert_eq!(row.presence, CreatedIdentityPresence::Present);
        assert_eq!(row.spec.as_ref(), Some(&spec(name, "worker")));
        assert_eq!(
            entry.spec_digest.as_deref(),
            Some(spec(name, "worker").spec_digest().expect("digest").as_str())
        );
        assert_eq!(row.provenance.operation_id, "create-1");
        assert!(mob.seated_dormant(name), "{name}: {:?}", mob.read(name));
        let seat = mob.spawn_row(name).await;
        assert_eq!(seat.identity_entry, Some(IdentityEntryOrigin::Created));
    }
    assert_eq!(
        mob.handle
            .identity_operation("create-1")
            .await
            .expect("read the receipt"),
        Some(receipt),
        "the receipt is stored with the change"
    );
}

/// Row: a create wakes nothing: no member session is built, and every
/// identity stays Dormant until its first delivery.
#[tokio::test]
async fn a_commit_wakes_nothing() {
    let mob = StoreOwnedMob::new("create-wakes-nothing").await;
    let builds = mob.builds().await;
    committed(
        mob.create(
            host_create("create-quiet", &["alpha", "beta", "gamma"]),
            ALLOW,
        )
        .await,
    );
    assert_eq!(mob.builds().await, builds, "the commit built no session");
    for name in ["alpha", "beta", "gamma"] {
        assert!(
            mob.handle.member_unmaterialized(&id(name)),
            "{name} is Dormant"
        );
    }
}

/// Row: a create of more identities than one change commits is refused
/// `batch_too_large`, and nothing is written.
#[tokio::test]
async fn a_batch_above_256_is_refused_and_writes_nothing() {
    let mob = StoreOwnedMob::new("create-too-large").await;
    let journal = mob.journal().await.len();
    let names = (0..=IDENTITY_CHANGE_BATCH_MAX)
        .map(|index| format!("member-{index}"))
        .collect::<Vec<_>>();
    let create = StoreOwnedIdentityCreate::new(
        operation("create-too-large"),
        IdentityChangeRequesterInput::host("console"),
        names
            .iter()
            .map(|name| CreatedIdentityRequest::new(spec(name, "worker")))
            .collect(),
    );
    assert_eq!(
        refusal(mob.create(create, ALLOW).await),
        IdentityChangeRefusal::BatchTooLarge {
            max: IDENTITY_CHANGE_BATCH_MAX
        }
    );
    assert!(mob.rows().await.is_empty(), "no row was written");
    assert_eq!(mob.journal().await.len(), journal, "no journal row");
    assert_eq!(
        mob.handle
            .identity_operation("create-too-large")
            .await
            .expect("read the receipt"),
        None
    );
}

// ---------------------------------------------------------------------------
// R2.2: the seat
// ---------------------------------------------------------------------------

/// Row (RED): a store-owned identity is seated exactly as a roster-owned
/// identity of the same spec: their `MemberSpawned` rows are equal field by
/// field, but for what names the identity itself (its name, runtime id,
/// fence, session and endpoint, its creation token) and the origin token.
/// Both are host roots.
///
/// RED on the old tree: no store-owned create exists.
#[tokio::test]
async fn a_store_owned_seat_equals_the_roster_owned_seat_of_the_same_spec() {
    let mob = StoreOwnedMob::new("seat-parity").await;
    committed(
        mob.create(host_create("create-alpha", &["alpha"]), ALLOW)
            .await,
    );
    let declared = spec("beta", "worker");
    let mut roster_spec = SpawnMemberSpec::host_root("worker", "beta");
    roster_spec.labels = Some(declared.labels.clone());
    roster_spec.context = declared.context.clone();
    roster_spec.additional_instructions = Some(declared.additional_instructions.clone());
    tokio::time::timeout(
        STEP,
        mob.handle
            .apply_declared_roster(DeclaredRoster::partial([DeclaredEntry::new(roster_spec)])),
    )
    .await
    .expect("the apply answers")
    .expect("the apply runs");

    let store_owned = mob.spawn_row("alpha").await;
    let roster_owned = mob.spawn_row("beta").await;
    assert_eq!(
        store_owned.creation.provenance,
        crate::MemberCreationProvenance::Root,
        "a store-owned identity is a host root"
    );
    assert_eq!(store_owned.spawned_by, None);
    let mut normalized = store_owned.clone();
    normalized.agent_identity = roster_owned.agent_identity.clone();
    normalized.agent_runtime_id = roster_owned.agent_runtime_id.clone();
    normalized.fence_token = roster_owned.fence_token;
    normalized.bridge_member_ref = roster_owned.bridge_member_ref.clone();
    normalized.member_peer_endpoint = roster_owned.member_peer_endpoint.clone();
    normalized.seat_session = roster_owned.seat_session.clone();
    normalized.creation.creation_id = roster_owned.creation.creation_id;
    normalized.identity_entry = roster_owned.identity_entry;
    normalized
        .labels
        .insert("desk".to_string(), "beta".to_string());
    if let Some(inputs) = normalized.seat_build_inputs.as_mut() {
        inputs.context = Some(serde_json::json!({"desk": "beta"}));
        inputs.additional_instructions = Some(vec!["You are beta.".to_string()]);
    }
    assert_eq!(
        normalized, roster_owned,
        "the store-owned seat equals the roster-owned seat of the same spec"
    );
}

/// Row: the definition's wiring applies to a store-owned identity by its
/// profile, as to any seat: with `auto_wire_orchestrator`, a created worker
/// is wired to the orchestrator.
#[tokio::test]
async fn definition_wiring_applies_to_a_store_owned_identity_by_its_profile() {
    let mut definition = store_owned_definition("seat-wiring");
    definition.wiring.auto_wire_orchestrator = true;
    let mob = StoreOwnedMob::build(definition, MobStorage::in_memory(), |builder| builder).await;
    mob.handle
        .spawn(ProfileName::from("lead"), id("boss"), None)
        .await
        .expect("spawn the orchestrator");
    committed(
        mob.create(host_create("create-wired", &["alpha"]), ALLOW)
            .await,
    );
    assert!(
        mob.wired_to("alpha").await.contains(&id("boss")),
        "the created worker is wired to the orchestrator: {:?}",
        mob.wired_to("alpha").await
    );
}

/// Row: an agent's created identity is no child of its requester: retiring
/// the requester retires nothing it created.
#[tokio::test]
async fn retiring_the_requester_never_retires_the_identity_it_created() {
    let mob = StoreOwnedMob::new("requester-retire").await;
    let session = mob.spawn_requester("requester").await;
    let child = spec("child", "worker");
    let grant = mob
        .admit(&session, "create-child", std::slice::from_ref(&child))
        .await
        .expect("the spawn grant admits the worker profile");
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-child", grant, vec![child]),
            ALLOW,
        )
        .await,
    );
    mob.handle
        .retire(id("requester"))
        .await
        .expect("retire the requester");
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.is_member("requester").await {
        assert!(tokio::time::Instant::now() < deadline, "the requester left");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(mob.is_member("child").await, "the created identity stays");
    assert!(mob.seated_dormant("child"));
}

/// Row: a create wires no peer its spec does not list: the requester and
/// its created identity are not wired to each other.
#[tokio::test]
async fn a_create_wires_no_peer_it_does_not_list() {
    let mob = StoreOwnedMob::new("create-no-parent-wire").await;
    let session = mob.spawn_requester("requester").await;
    let child = spec("child", "worker");
    let grant = mob
        .admit(&session, "create-child", std::slice::from_ref(&child))
        .await
        .expect("the spawn grant admits the worker profile");
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-child", grant, vec![child]),
            ALLOW,
        )
        .await,
    );
    assert!(mob.wired_to("child").await.is_empty());
    assert!(!mob.wired_to("requester").await.contains(&id("child")));
}

// ---------------------------------------------------------------------------
// R2.3: admission by the ordinary spawn grant
// ---------------------------------------------------------------------------

/// Row (RED): an agent whose spawn grant does not hold a profile is refused
/// `profile_not_allowed` for it at admission, and nothing is written.
///
/// RED on the old tree: no admission exists.
#[tokio::test]
async fn an_agent_without_the_spawn_grant_for_the_profile_is_refused_profile_not_allowed() {
    let mob = StoreOwnedMob::with_grants(
        "grant-profile-refused",
        vec![("requester", vec!["worker"], false)],
    )
    .await;
    let session = mob.spawn_requester("requester").await;
    assert_eq!(
        refusal(
            mob.admit(&session, "create-lead", &[spec("boss", "lead")])
                .await
        ),
        IdentityChangeRefusal::ProfileNotAllowed {
            profile: ProfileName::from("lead")
        }
    );
    assert!(mob.rows().await.is_empty());
}

/// Row (RED): an agent whose spawn grant holds one profile creates
/// identities of that profile, and only of it.
///
/// RED on the old tree: no admission exists.
#[tokio::test]
async fn an_agent_with_a_spawn_profile_scope_creates_that_profile_only() {
    let mob = StoreOwnedMob::with_grants(
        "grant-profile-scope",
        vec![("requester", vec!["worker"], false)],
    )
    .await;
    let session = mob.spawn_requester("requester").await;
    let child = spec("child", "worker");
    let grant = mob
        .admit(&session, "create-child", std::slice::from_ref(&child))
        .await
        .expect("the grant holds the worker profile");
    assert_eq!(grant.requester(), &id("requester"));
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-child", grant, vec![child]),
            ALLOW,
        )
        .await,
    );
    assert!(mob.seated_dormant("child"));
    assert_eq!(
        refusal(
            mob.admit(&session, "create-boss", &[spec("boss", "lead")])
                .await
        ),
        IdentityChangeRefusal::ProfileNotAllowed {
            profile: ProfileName::from("lead")
        }
    );
}

/// Row (RED): an agent with manage scope over the mob creates an identity
/// of any profile.
///
/// RED on the old tree: no admission exists.
#[tokio::test]
async fn an_agent_with_manage_scope_creates_any_profile() {
    let mob =
        StoreOwnedMob::with_grants("grant-manage-scope", vec![("requester", vec![], true)]).await;
    let session = mob.spawn_requester("requester").await;
    let boss = spec("boss", "lead");
    let grant = mob
        .admit(&session, "create-boss", std::slice::from_ref(&boss))
        .await
        .expect("manage scope admits every profile");
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-boss", grant, vec![boss]),
            ALLOW,
        )
        .await,
    );
    assert!(mob.seated_dormant("boss"));
}

/// Row (RED): a spec that sets a runtime binding or a placement needs manage
/// scope, as a spawn's privileged arguments do: refused `privileged_field`
/// for the field without it, admitted with it.
///
/// RED on the old tree: no admission exists, and the spawn admission
/// observed neither field.
#[tokio::test]
async fn a_binding_or_placement_in_the_spec_needs_manage_scope() {
    let mob = StoreOwnedMob::with_grants(
        "grant-privileged",
        vec![
            ("requester", vec!["worker"], false),
            ("manager", vec![], true),
        ],
    )
    .await;
    let requester = mob.spawn_requester("requester").await;
    let manager = mob.spawn_requester("manager").await;
    let bound =
        spec("bound", "worker").with_binding(meerkat_contracts::WireRuntimeBinding::Session);
    let placed = spec("placed", "worker")
        .with_placement(meerkat_contracts::WireHostRef("host-peer-1".to_string()));
    assert_eq!(
        refusal(
            mob.admit(&requester, "create-bound", std::slice::from_ref(&bound))
                .await
        ),
        IdentityChangeRefusal::PrivilegedField { field: "binding" }
    );
    assert_eq!(
        refusal(
            mob.admit(&requester, "create-placed", std::slice::from_ref(&placed))
                .await
        ),
        IdentityChangeRefusal::PrivilegedField { field: "placement" }
    );
    mob.admit(&manager, "create-bound", std::slice::from_ref(&bound))
        .await
        .expect("manage scope admits a binding");
    mob.admit(&manager, "create-placed", std::slice::from_ref(&placed))
        .await
        .expect("manage scope admits a placement");
}

/// Row: a store-owned identity whose own profile grants spawn creates
/// identities once it runs, by its own grant, as any member does.
#[tokio::test]
async fn a_created_identity_creates_when_its_own_profile_grants_spawn() {
    let mob = StoreOwnedMob::new("created-creates").await;
    committed(
        mob.create(host_create("create-creator", &["creator"]), ALLOW)
            .await,
    );
    mob.handle
        .ensure_member_live(&id("creator"))
        .await
        .expect("wake the created identity");
    let session = mob.session("creator").await;
    let child = spec("child", "worker");
    let grant = mob
        .admit(&session, "create-child", std::slice::from_ref(&child))
        .await
        .expect("its profile's grant admits the worker profile");
    assert_eq!(grant.requester(), &id("creator"));
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-child", grant, vec![child]),
            ALLOW,
        )
        .await,
    );
    assert!(mob.seated_dormant("child"));
}

// ---------------------------------------------------------------------------
// R2.4: no creator-derived state
// ---------------------------------------------------------------------------

/// Row: no limit of any kind applies to store-owned identities: three
/// hundred of them commit in two changes.
#[tokio::test]
async fn three_hundred_creations_in_two_batches_commit() {
    let mob = StoreOwnedMob::new("three-hundred").await;
    let names = (0..300)
        .map(|index| format!("member-{index:03}"))
        .collect::<Vec<_>>();
    let (first, second) = names.split_at(IDENTITY_CHANGE_BATCH_MAX);
    for (operation_id, batch) in [("create-first", first), ("create-second", second)] {
        let create = StoreOwnedIdentityCreate::new(
            operation(operation_id),
            IdentityChangeRequesterInput::host("console"),
            batch
                .iter()
                .map(|name| CreatedIdentityRequest::new(spec(name, "worker")))
                .collect(),
        );
        let receipt = committed(mob.create(create, ALLOW).await);
        assert_eq!(receipt.identities.len(), batch.len());
    }
    assert_eq!(mob.rows().await.len(), 300);
    assert_eq!(mob.handle.identity_census().expect("census").created, 300);
}

// ---------------------------------------------------------------------------
// R2.9: receipts, consent, candidates, holds and storage compositions
// ---------------------------------------------------------------------------

/// Row: a custom storage composition (no built-in identity and member store)
/// refuses every change `storage_unsupported`; nothing else of the mob
/// changes.
#[tokio::test]
async fn a_custom_storage_composition_refuses_storage_unsupported_and_nothing_else_changes() {
    let storage = MobStorage {
        identity_member: None,
        ..MobStorage::in_memory()
    };
    let mob = StoreOwnedMob::build(
        store_owned_definition("custom-storage"),
        storage,
        |builder| builder,
    )
    .await;
    let journal = mob.journal().await.len();
    assert_eq!(
        refusal(
            mob.create(host_create("create-custom", &["alpha"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::StorageUnsupported
    );
    assert_eq!(mob.journal().await.len(), journal);
    assert!(!mob.is_member("alpha").await);
    assert!(matches!(
        mob.read("alpha"),
        IdentityEntryRead::NotAnEntry { worker: None }
    ));
    mob.handle
        .spawn(ProfileName::from("worker"), id("worker"), None)
        .await
        .expect("the mob still spawns");
}

/// A decision clock at a fixed time.
struct FixedClock(u64);

impl meerkat_runtime::decision::DecisionClock for FixedClock {
    fn now_ms(&self) -> Result<u64, meerkat_runtime::decision::DecisionClockError> {
        Ok(self.0)
    }
}

struct Eligible;

impl meerkat_core::approval::consent::ConsentEligibility for Eligible {
    fn check_current(&self) -> Result<(), meerkat_core::approval::consent::ConsentIneligible> {
        Ok(())
    }
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock reads after the epoch")
            .as_millis(),
    )
    .expect("milliseconds fit")
}

/// An approved decision's execution permit for `operation`, held at
/// `held_at_ms` until `deadline_at_ms` and authorized then.
fn decided_permit(
    operation: &IdentityOperation,
    held_at_ms: u64,
    deadline_at_ms: u64,
) -> meerkat_runtime::decision::DecisionExecutionPermit {
    use meerkat_core::approval::decision;
    let principal = |kind, name: &str| {
        meerkat_core::PrincipalRef::in_domain(
            kind,
            name,
            meerkat_core::TrustDomainId::new("example.org").expect("trust domain"),
        )
        .expect("qualified principal")
    };
    let service = meerkat_runtime::decision::DurableDecisionService::with_clock(
        Arc::new(decision::InMemoryDecisionStore::default()),
        Arc::new(FixedClock(held_at_ms)),
    );
    let rendering = decision::DecisionRendering::new("Create the identity.").expect("rendering");
    let operation_id =
        decision::DecisionOperationId::new(operation.operation_id.clone()).expect("operation id");
    let request_digest =
        decision::Sha256Digest::new(operation.request_digest.clone()).expect("request digest");
    let approver = decision::ApproverId::new("team:owners").expect("approver id");
    let held = service
        .hold(
            decision::DecisionHoldRequest::new(
                decision::DecisionKind::new("identity.create").expect("kind"),
                decision::DecisionSubjectKey::new(format!("mob/{}", operation.operation_id))
                    .expect("subject key"),
                operation_id.clone(),
                request_digest.clone(),
                decision::DecisionRequestBody::from_value(&serde_json::json!({"create": 1}))
                    .expect("request body"),
                decision::DecisionPrincipalKey::of(&principal(
                    meerkat_core::PrincipalKind::PersonalAgent,
                    "requester",
                ))
                .expect("requester key"),
                approver.clone(),
                deadline_at_ms,
                rendering.clone(),
                decision::DecisionRenderer::new("render.v1").expect("renderer"),
            ),
            true,
        )
        .expect("hold");
    let reference = |value: &str| decision::DecisionReference::new(value).expect("reference");
    service
        .present(
            &held.decision_id,
            &decision::PresentationRecord::complete(
                decision::PresentationAudience::Approver {
                    approver_id: approver.clone(),
                },
                &rendering,
                None,
                reference("console"),
                reference("message-1"),
                held_at_ms,
            ),
        )
        .expect("present");
    let decider = meerkat_core::approval::consent::AuthenticatedApprover::new(
        principal(meerkat_core::PrincipalKind::Human, "alice"),
        meerkat_core::approval::ApprovalAssurance::PersonAuthenticated,
    )
    .expect("authenticated human");
    service
        .decide(
            &held.decision_id,
            &decision::AttestedDecision::new(
                decider,
                decision::DecisionIngress::new("console").expect("ingress"),
                reference("event-1"),
                held_at_ms + 1,
                approver,
                rendering.subject_digest(),
                true,
            )
            .expect("attested decision"),
            &Eligible,
        )
        .expect("approve");
    service
        .authorize_execution(&held.decision_id, &operation_id, &request_digest)
        .expect("authorize the execution")
}

/// Row (RED): a decided change commits only under its permit, while the
/// permit binds its operation and request digest and admits a commit now:
/// a permit of another request is refused `subject_widened`, one past its
/// deadline `decision_closed`, and nothing is written either way. The
/// committed row seals the permit's consent.
///
/// RED on the old tree: no store-owned create exists.
#[tokio::test]
async fn a_decided_commit_needs_its_permit_binding_and_deadline() {
    let mob = StoreOwnedMob::new("decided-commit").await;
    let now = now_ms();
    let other = decided_permit(&operation("another-request"), now, now + 600_000);
    assert_eq!(
        refusal(
            mob.create(
                host_create("create-decided", &["alpha"]),
                IdentityChangeConsentInput::Decided(&other)
            )
            .await
        ),
        IdentityChangeRefusal::SubjectWidened
    );
    let expired = decided_permit(&operation("create-decided"), now - 120_000, now - 60_000);
    assert_eq!(
        refusal(
            mob.create(
                host_create("create-decided", &["alpha"]),
                IdentityChangeConsentInput::Decided(&expired)
            )
            .await
        ),
        IdentityChangeRefusal::DecisionClosed { state: "expired" }
    );
    assert!(mob.rows().await.is_empty(), "nothing was written");

    let permit = decided_permit(&operation("create-decided"), now, now + 600_000);
    let receipt = committed(
        mob.create(
            host_create("create-decided", &["alpha"]),
            IdentityChangeConsentInput::Decided(&permit),
        )
        .await,
    );
    let sealed = IdentityChangeConsent::Decided {
        provenance: permit.consent().clone(),
    };
    assert_eq!(receipt.consent, sealed);
    let rows = mob.rows().await;
    let Some(CreatedIdentityObservation::Created(row)) = rows.get(&id("alpha")) else {
        panic!("alpha has no readable row: {rows:?}");
    };
    assert_eq!(row.provenance.consent, sealed);
}

/// Row: a candidate composition commits no change: refused
/// `candidate_launch`, and nothing is written.
#[tokio::test]
async fn a_commit_under_a_candidate_writes_nothing() {
    let mut mob = StoreOwnedMob::new("candidate").await;
    mob.restart_as(MobCompositionAuthority::NonAuthoritative)
        .await;
    let journal = mob.journal().await.len();
    assert_eq!(
        refusal(
            mob.create(host_create("create-candidate", &["alpha"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::CandidateLaunch
    );
    assert_eq!(mob.journal().await.len(), journal);
    assert!(mob.rows().await.is_empty());
}

/// Row: while the mob's identity changes are held, a change is refused
/// `changes_held`, retryable, and nothing is written; once the hold is
/// released, the same change commits. A mob held from its start refuses
/// too.
#[tokio::test]
async fn held_changes_refuse_retryable_and_a_release_admits() {
    let mob = StoreOwnedMob::new("changes-held").await;
    mob.handle
        .set_identity_changes_held(true)
        .await
        .expect("hold the changes");
    let held = refusal(
        mob.create(host_create("create-held", &["alpha"]), ALLOW)
            .await,
    );
    assert_eq!(held, IdentityChangeRefusal::ChangesHeld);
    assert!(held.is_retryable());
    assert!(mob.rows().await.is_empty());
    mob.handle
        .set_identity_changes_held(false)
        .await
        .expect("release the changes");
    committed(
        mob.create(host_create("create-held", &["alpha"]), ALLOW)
            .await,
    );
    assert!(mob.seated_dormant("alpha"));

    let held_from_start = StoreOwnedMob::build(
        store_owned_definition("changes-held-at-start"),
        MobStorage::in_memory(),
        |builder| builder.with_identity_changes_held(true),
    )
    .await;
    assert_eq!(
        refusal(
            held_from_start
                .create(host_create("create-held", &["alpha"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::ChangesHeld
    );
}

/// Row: a preparation the host owes keeps the identity unseated: its row is
/// pending its preparation, its receipt says so, and nothing is seated.
#[tokio::test]
async fn a_create_with_preparation_records_the_identity_pending_and_seats_nothing() {
    let mob = StoreOwnedMob::new("create-preparing").await;
    let create = StoreOwnedIdentityCreate::new(
        operation("create-preparing"),
        IdentityChangeRequesterInput::host("console"),
        vec![
            CreatedIdentityRequest::new(spec("alpha", "worker"))
                .with_preparation(IdentityPreparationMode::Required),
        ],
    );
    let receipt = committed(mob.create(create, ALLOW).await);
    assert_eq!(
        receipt.identities[0].outcome,
        IdentityChangeEntryOutcome::PendingPreparation
    );
    let rows = mob.rows().await;
    let Some(CreatedIdentityObservation::Created(row)) = rows.get(&id("alpha")) else {
        panic!("alpha has no readable row: {rows:?}");
    };
    assert!(matches!(
        row.presence,
        CreatedIdentityPresence::PendingPreparation {
            attempt: 0,
            constructed: false
        }
    ));
    assert!(!mob.is_member("alpha").await);
    assert!(matches!(
        mob.read("alpha"),
        IdentityEntryRead::Entry(view)
            if view.standing == EntryStanding::Unseated(UnseatedEntryCause::PreparationPending)
    ));
}

/// Row (guard): a retry of a committed create with the same request replays
/// its receipt and writes nothing again; another request under the same
/// operation is refused `operation_reused`.
#[tokio::test]
async fn a_retried_create_replays_and_another_request_under_its_operation_is_refused() {
    let mob = StoreOwnedMob::new("create-replay").await;
    let receipt = committed(mob.create(host_create("create-1", &["alpha"]), ALLOW).await);
    let journal = mob.journal().await.len();
    match mob.create(host_create("create-1", &["alpha"]), ALLOW).await {
        Ok(IdentityChangeOutcome::Replayed { receipt: replayed }) => {
            assert_eq!(replayed, receipt);
        }
        other => panic!("expected a replay, got {other:?}"),
    }
    let other_request = StoreOwnedIdentityCreate::new(
        IdentityOperation::new("create-1", digest("another request")),
        IdentityChangeRequesterInput::host("console"),
        vec![CreatedIdentityRequest::new(spec("beta", "worker"))],
    );
    assert_eq!(
        refusal(mob.create(other_request, ALLOW).await),
        IdentityChangeRefusal::OperationReused {
            operation_id: "create-1".to_string()
        }
    );
    assert_eq!(mob.journal().await.len(), journal, "nothing written again");
    assert!(!mob.is_member("beta").await);
}

/// Row (guard): a create of a name any member, identity or reservation holds
/// is refused `name_taken` with the holder, for the whole change: a worker,
/// an identity of either custody, and nothing of the change is written.
#[tokio::test]
async fn a_create_of_a_taken_name_is_refused_and_writes_nothing() {
    let mob = StoreOwnedMob::new("create-name-taken").await;
    mob.handle
        .spawn(ProfileName::from("worker"), id("worker"), None)
        .await
        .expect("spawn a worker");
    committed(
        mob.create(host_create("create-alpha", &["alpha"]), ALLOW)
            .await,
    );
    let journal = mob.journal().await.len();
    assert_eq!(
        refusal(
            mob.create(host_create("create-taken", &["fresh", "worker"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::NameTaken {
            identity: id("worker"),
            by: IdentityNameHolder::Worker
        }
    );
    assert_eq!(
        refusal(
            mob.create(host_create("create-taken-again", &["alpha"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::NameTaken {
            identity: id("alpha"),
            by: IdentityNameHolder::Identity
        }
    );
    assert_eq!(mob.journal().await.len(), journal);
    assert!(!mob.is_member("fresh").await, "the change wrote nothing");
}

// ---------------------------------------------------------------------------
// R2.14: the grant recheck
// ---------------------------------------------------------------------------

/// A host grant mint the rows revoke: until then it mints the authority the
/// runtime composes for each member's profile; once revoked, none.
#[derive(Default)]
struct RevocableMint {
    revoked: AtomicBool,
    revision: AtomicU64,
}

impl RevocableMint {
    fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
        self.revision.fetch_add(1, Ordering::SeqCst);
    }
}

impl MemberGrantMint for RevocableMint {
    fn mint(&self, request: &MemberGrantMintRequest<'_>) -> Option<MobToolAuthorityContext> {
        if self.revoked.load(Ordering::SeqCst) {
            None
        } else {
            request.composed.cloned()
        }
    }

    fn revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }
}

/// Row (RED): a grant the host revokes while the change waits ends the
/// change `grant_revoked` at its commit, and nothing is written.
///
/// RED on the old tree: no admission or recheck exists.
#[tokio::test]
async fn a_grant_revoked_while_a_change_waits_ends_it_grant_revoked_and_commits_nothing() {
    let mint = Arc::new(RevocableMint::default());
    let mint_for_builder: Arc<dyn MemberGrantMint> = mint.clone();
    let mob = StoreOwnedMob::build(
        store_owned_definition("grant-revoked"),
        MobStorage::in_memory(),
        move |builder| builder.with_member_grant_mint(mint_for_builder),
    )
    .await;
    let session = mob.spawn_requester("requester").await;
    let child = spec("child", "worker");
    let grant = mob
        .admit(&session, "create-child", std::slice::from_ref(&child))
        .await
        .expect("the grant admits the change");
    mint.revoke();
    let journal = mob.journal().await.len();
    assert_eq!(
        refusal(
            mob.create(
                StoreOwnedMob::agent_create("create-child", grant, vec![child]),
                ALLOW,
            )
            .await
        ),
        IdentityChangeRefusal::GrantRevoked
    );
    assert_eq!(mob.journal().await.len(), journal);
    assert!(mob.rows().await.is_empty());
    assert!(!mob.is_member("child").await);
}

/// Row (RED): the recheck reads the requester identity's grant, never its
/// session: a change admitted before the requester's new activation, or
/// before a reset onto a new session, still commits under its approval.
///
/// RED on the old tree: no admission or recheck exists.
#[tokio::test]
async fn a_requesters_session_reset_or_new_activation_keeps_its_pending_approval() {
    let mob = StoreOwnedMob::new("grant-reset").await;
    let session = mob.spawn_requester("requester").await;
    let first = spec("first", "worker");
    let second = spec("second", "worker");
    let first_grant = mob
        .admit(&session, "create-first", std::slice::from_ref(&first))
        .await
        .expect("the grant admits the first change");
    let second_grant = mob
        .admit(&session, "create-second", std::slice::from_ref(&second))
        .await
        .expect("the grant admits the second change");

    // A new activation: released, then woken again.
    mob.handle
        .release_member(&id("requester"))
        .await
        .expect("release the requester");
    mob.handle
        .ensure_member_live(&id("requester"))
        .await
        .expect("wake the requester again");
    let now = now_ms();
    let permit = decided_permit(&operation("create-first"), now, now + 600_000);
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-first", first_grant, vec![first]),
            IdentityChangeConsentInput::Decided(&permit),
        )
        .await,
    );

    // A reset: the requester's fresh life, on a new session.
    let mut reset = SpawnMemberSpec::new("worker", id("requester"));
    reset.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    let reset_session = mob
        .handle
        .rotate_member(reset)
        .await
        .expect("reset the requester onto a new session");
    assert_ne!(reset_session, session);
    let permit = decided_permit(&operation("create-second"), now, now + 600_000);
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-second", second_grant, vec![second]),
            IdentityChangeConsentInput::Decided(&permit),
        )
        .await,
    );
    assert!(mob.seated_dormant("first") && mob.seated_dormant("second"));
}

/// Row (RED): a dormant requester's grant is read without waking it: its
/// change commits, and it stays Dormant, with no session built.
///
/// RED on the old tree: no admission or recheck exists.
#[tokio::test]
async fn a_dormant_requesters_grant_is_read_without_waking_it() {
    let mob = StoreOwnedMob::new("grant-dormant").await;
    let session = mob.spawn_requester("requester").await;
    let child = spec("child", "worker");
    let grant = mob
        .admit(&session, "create-child", std::slice::from_ref(&child))
        .await
        .expect("the grant admits the change");
    mob.handle
        .release_member(&id("requester"))
        .await
        .expect("release the requester");
    assert!(mob.handle.member_unmaterialized(&id("requester")));
    let builds = mob.builds().await;
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-child", grant, vec![child]),
            ALLOW,
        )
        .await,
    );
    assert!(
        mob.handle.member_unmaterialized(&id("requester")),
        "the recheck woke nothing"
    );
    assert_eq!(mob.builds().await, builds, "no session was built");
}

// ---------------------------------------------------------------------------
// Source guards: origin is read only by the custody guards, and the grant
// recheck has no second mechanism
// ---------------------------------------------------------------------------

/// The shipped files that may read an identity entry's origin, each for a
/// custody guard (the recovery source, the writer, the name guards and the
/// reservations) or for the reads that show it:
const ORIGIN_READERS: &[(&str, &str)] = &[
    ("event.rs", "the journal codec of a seat row's origin token"),
    (
        "runtime/identity_entry.rs",
        "the entry reads and the census, which show the origin",
    ),
    (
        "runtime/created_identity.rs",
        "the recovery source: the store-owned entries owed their seat",
    ),
    (
        "runtime/handle.rs",
        "the legacy adoption's custody guard (adopt_legacy_identity_entry only)",
    ),
    ("runtime/identity_change.rs", "the store-owned writer"),
    (
        "runtime/actor/declared_apply.rs",
        "the roster writer, the declared apply",
    ),
    ("machines/mob_machine.rs", "the machine's own types"),
];

/// The DSL transitions (by name prefix) and invariants that may read an
/// identity entry's origin: the custody guards of a spawn ladder and its
/// commit, the admission probe, the declared apply and its seal (the roster
/// writer), the recovery of entries (the recovery source), the legacy
/// adoption, and the store-owned writer's record of a new entry.
const ORIGIN_GUARD_TRANSITIONS: &[&str] = &[
    "BeginSpawnExec",
    "CommitSpawnMembership",
    "CommitDormantSeat",
    "ProbeMemberAdmission",
    "DeclareIdentityEntry",
    "SealDeclaredRoster",
    "RecoverCreatedIdentityEntry",
    "RecoverIdentityEntry",
    "AdoptLegacyIdentityEntry",
    "RecordCreatedIdentityEntry",
];

const ORIGIN_GUARD_INVARIANTS: &[&str] = &[
    "entry_custodies_are_disjoint",
    "retiring_entry_is_a_declared_entry",
    "unseated_created_entry_is_preparing_or_owed",
];

/// The byte offsets of every read of an origin value
/// (`IdentityEntryOrigin::Created` or `::Declared`) in `code`.
fn origin_value_reads(code: &str) -> Vec<usize> {
    use super::fail_stop_cause::whole_identifier;
    const NAME: &str = "IdentityEntryOrigin";
    code.match_indices(NAME)
        .map(|(at, _)| at)
        .filter(|at| whole_identifier(code, *at, NAME))
        .filter(|at| {
            let rest = code[at + NAME.len()..].trim_start();
            rest.starts_with("::Created") || rest.starts_with("::Declared")
        })
        .collect()
}

/// Row (source guard; RED leg: a scratch reader of
/// `IdentityEntryOrigin::Created` added to any other shipped file, or to a
/// DSL transition outside the custody guards, fails it): no code reads an
/// identity's origin outside the custody guards and the reads that show it.
/// Custody decides the recovery source, the writer, the name guards and the
/// reservations, nothing else.
#[test]
fn identity_entry_origin_is_read_only_by_the_custody_guards() {
    use super::fail_stop_cause::{function_body, line_of, shipped_code, shipped_sources};
    let mut strays = Vec::new();
    for (path, code) in shipped_sources() {
        let reads = origin_value_reads(&code);
        if reads.is_empty() {
            continue;
        }
        if !ORIGIN_READERS.iter().any(|(reader, _)| *reader == path) {
            strays.extend(
                reads
                    .iter()
                    .map(|at| format!("{path}:{}", line_of(&code, *at))),
            );
            continue;
        }
        if path == "runtime/handle.rs" {
            let adoption = function_body(&code, "pub async fn adopt_legacy_identity_entry(")
                .expect("the legacy adoption");
            strays.extend(
                reads
                    .iter()
                    .filter(|at| !adoption.contains(at))
                    .map(|at| format!("{path}:{}", line_of(&code, *at))),
            );
        }
    }

    let dsl_path = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set for test processes"),
    )
    .join("../meerkat-machine-schema/src/catalog/dsl/mob_machine.rs");
    let dsl = shipped_code(&std::fs::read_to_string(&dsl_path).expect("read the DSL"));
    for at in origin_value_reads(&dsl) {
        let before = &dsl[..at];
        let item = ["transition ", "invariant ", "helper "]
            .iter()
            .filter_map(|keyword| before.rfind(keyword).map(|start| (start, *keyword)))
            .max_by_key(|(start, _)| *start);
        let allowed = item.is_some_and(|(start, keyword)| {
            let name = dsl[start + keyword.len()..]
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .next()
                .unwrap_or_default();
            match keyword {
                "transition " => ORIGIN_GUARD_TRANSITIONS
                    .iter()
                    .any(|prefix| name.starts_with(prefix)),
                "invariant " => ORIGIN_GUARD_INVARIANTS.contains(&name),
                _ => false,
            }
        });
        if !allowed {
            strays.push(format!("mob_machine DSL:{}", line_of(&dsl, at)));
        }
    }
    assert!(
        strays.is_empty(),
        "an identity's origin is read outside the custody guards: {strays:?}"
    );
}

/// Row (source guard): the recheck of an agent requester's grant runs
/// through the change's prepared check at the commit, its final sink, and
/// through nothing else. The commit calls `current` on the check (a decided
/// change's `reprepared`); the runtime's receiver, which reads a session's
/// live grant, is read at the admission only; and the identity's current
/// grant is computed only by the check's owner.
#[test]
fn the_recheck_runs_through_the_prepared_check_at_the_final_sink() {
    use super::fail_stop_cause::{function_body, line_of, shipped_sources};
    let sources = shipped_sources();
    let identity_change = &sources
        .iter()
        .find(|(path, _)| path == "runtime/identity_change.rs")
        .expect("the identity change module")
        .1;
    let sink = function_body(identity_change, "fn grant_current_at_the_sink(")
        .map(|range| &identity_change[range])
        .expect("the commit's grant check");
    assert!(
        sink.contains(".reprepared()") && sink.contains(".current()"),
        "the commit checks the prepared check current, or re-prepares a decided one"
    );
    let mut strays = Vec::new();
    for (path, code) in &sources {
        for read in [
            "member_spawn_profile_scope_contains(",
            "member_manage_scope_contains(",
            "current_identity_grant(",
        ] {
            for (at, _) in code.match_indices(read) {
                let declaration = code[..at].trim_end().ends_with("fn");
                if declaration {
                    continue;
                }
                let allowed = match read {
                    "current_identity_grant(" => {
                        path == "runtime/identity_change.rs"
                            && function_body(code, "fn prepare(")
                                .is_some_and(|body| body.contains(&at))
                    }
                    _ => {
                        path == "runtime/identity_change.rs"
                            && function_body(code, "pub async fn admit_identity_change_grant(")
                                .is_some_and(|body| body.contains(&at))
                    }
                };
                if !allowed {
                    strays.push(format!("{path}:{} {read}", line_of(code, at)));
                }
            }
        }
    }
    assert!(
        strays.is_empty(),
        "a grant read outside the admission and the prepared check's owner: {strays:?}"
    );
}
