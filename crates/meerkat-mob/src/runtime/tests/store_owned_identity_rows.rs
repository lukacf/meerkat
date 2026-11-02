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
    // The store's writes of an archive and an unarchive: a roster-owned
    // identity's are its host's.
    "ArchiveIdentityEntry",
    "UnarchiveIdentityEntry",
    "RemoveIdentityEntry",
];

const ORIGIN_GUARD_INVARIANTS: &[&str] = &[
    "entry_custodies_are_disjoint",
    "retiring_entry_is_a_declared_entry",
    "archived_entry_is_store_owned",
    "entry_ending_marks_are_seated_and_disjoint",
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

// ---------------------------------------------------------------------------
// R2.6: removal of either custody
// ---------------------------------------------------------------------------

/// A roster-owned spec of `name` on the worker profile, as a host declares
/// it.
fn roster_spec(name: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::host_root("worker", id(name))
}

/// A unique identity name for a row that holds a retirement stage: the
/// stage holds are keyed by name across the test process.
fn unique(name: &str) -> String {
    format!("{name}-{}", uuid::Uuid::new_v4().simple())
}

impl StoreOwnedMob {
    /// Apply a partial roster declaring `names`; the pass itself succeeds.
    async fn declare(&self, names: &[&str]) -> crate::runtime::DeclaredRosterOutcome {
        tokio::time::timeout(
            STEP,
            self.handle.apply_declared_roster(DeclaredRoster::partial(
                names
                    .iter()
                    .map(|name| DeclaredEntry::new(roster_spec(name))),
            )),
        )
        .await
        .expect("the apply answers")
        .expect("the apply runs")
    }

    /// A host surface's removal of `name` as `operation_id`.
    async fn remove(
        &self,
        operation_id: &str,
        name: &str,
    ) -> Result<IdentityChangeOutcome, MobError> {
        tokio::time::timeout(
            STEP,
            self.handle.remove_identity(
                crate::runtime::IdentityRemoval::new(
                    operation(operation_id),
                    IdentityChangeRequesterInput::host("console"),
                    name,
                ),
                ALLOW,
            ),
        )
        .await
        .expect("the removal answers")
    }

    /// A host surface's archive of `name` as `operation_id`.
    async fn archive(
        &self,
        operation_id: &str,
        name: &str,
    ) -> Result<IdentityChangeOutcome, MobError> {
        tokio::time::timeout(
            STEP,
            self.handle.archive_identity(
                crate::runtime::IdentityArchiveChange::new(
                    operation(operation_id),
                    IdentityChangeRequesterInput::host("console"),
                    name,
                ),
                ALLOW,
            ),
        )
        .await
        .expect("the archive answers")
    }

    /// A host surface's unarchive of `name` as `operation_id`.
    async fn unarchive(
        &self,
        operation_id: &str,
        name: &str,
    ) -> Result<IdentityChangeOutcome, MobError> {
        tokio::time::timeout(
            STEP,
            self.handle.unarchive_identity(
                crate::runtime::IdentityArchiveChange::new(
                    operation(operation_id),
                    IdentityChangeRequesterInput::host("console"),
                    name,
                ),
                ALLOW,
            ),
        )
        .await
        .expect("the unarchive answers")
    }

    /// Wait, within one step, until `name` reads removed.
    async fn removed(&self, name: &str) {
        let deadline = tokio::time::Instant::now() + STEP;
        while !matches!(self.read(name), IdentityEntryRead::Removed) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{name} never read removed: {:?}",
                self.read(name)
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// `name`'s standing, when it is an entry.
    fn standing(&self, name: &str) -> Option<EntryStanding> {
        match self.read(name) {
            IdentityEntryRead::Entry(view) => Some(view.standing),
            _ => None,
        }
    }

    /// Wait, within one step, until `name` stands unseated for `cause`.
    async fn unseated_for(&self, name: &str, cause: UnseatedEntryCause) {
        let deadline = tokio::time::Instant::now() + STEP;
        while self.standing(name) != Some(EntryStanding::Unseated(cause)) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{name} never stood unseated for {cause:?}: {:?}",
                self.read(name)
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// `name`'s readable store-owned row.
    async fn row(&self, name: &str) -> crate::identity::CreatedIdentityRecord {
        match self.rows().await.remove(&id(name)) {
            Some(CreatedIdentityObservation::Created(row)) => row,
            other => panic!("{name} has no readable row: {other:?}"),
        }
    }
}

/// Row (RED): removing a roster-owned identity retires its member and
/// tombstones its name: the store's first row of the name is its tombstone
/// (presence removed, the removal's record), and the roster's listing of it
/// is refused afterwards.
///
/// RED on the old tree: no removal exists.
#[tokio::test]
async fn removing_a_roster_owned_identity_tombstones_its_name() {
    let mob = StoreOwnedMob::new("remove-roster-owned").await;
    mob.declare(&["alpha"]).await;
    assert!(mob.is_member("alpha").await);
    let receipt = committed(mob.remove("remove-alpha", "alpha").await);
    assert_eq!(receipt.change, IdentityChangeKind::Remove);
    assert_eq!(
        receipt.identities[0].outcome,
        IdentityChangeEntryOutcome::Retiring
    );
    mob.removed("alpha").await;
    assert!(!mob.is_member("alpha").await);
    let row = mob.row("alpha").await;
    assert_eq!(row.presence, CreatedIdentityPresence::Removed);
    assert_eq!(row.spec, None);
    assert_eq!(
        row.provenance.entered,
        crate::identity::IdentityEntered::RosterRemoval
    );
    assert_eq!(
        row.removal
            .as_ref()
            .map(|removal| removal.operation_id.as_str()),
        Some("remove-alpha")
    );
    let relisted = mob.declare(&["alpha"]).await;
    assert!(matches!(
        relisted.entries.get(&id("alpha")),
        Some(crate::runtime::DeclaredEntryOutcome::Refused {
            refusal: crate::runtime::DeclaredEntryRefusal::NameTombstoned
        })
    ));
}

/// Row: a seated identity's removal reads retiring until its member's
/// retirement ends, then removed; its retirement's start row records the
/// cause `removed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removal_reads_retiring_until_the_terminal_then_removed() {
    let mob = StoreOwnedMob::new("remove-retiring").await;
    let name = unique("removed");
    committed(
        mob.create(host_create("create-held", &[&name]), ALLOW)
            .await,
    );
    mob.handle
        .ensure_member_live(&id(&name))
        .await
        .expect("wake the identity");
    let hold = super::actor::retirement_stage_hold::install(&id(&name), "retire-runtime-quiesce");
    let removal = {
        let handle = mob.handle.clone();
        let name = name.clone();
        tokio::spawn(async move {
            handle
                .remove_identity(
                    crate::runtime::IdentityRemoval::new(
                        operation("remove-held"),
                        IdentityChangeRequesterInput::host("console"),
                        name,
                    ),
                    ALLOW,
                )
                .await
        })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the retirement reaches its held stage");
    assert!(
        matches!(mob.standing(&name), Some(EntryStanding::Retiring(_))),
        "{:?}",
        mob.read(&name)
    );
    hold.release();
    committed(removal.await.expect("the removal task"));
    mob.removed(&name).await;
    let started = mob
        .journal()
        .await
        .into_iter()
        .find_map(|event| match event.kind {
            MobEventKind::MemberRetirementStarted {
                agent_identity,
                cause,
                ..
            } if agent_identity == id(&name) => Some(cause),
            _ => None,
        })
        .expect("the retirement's start row");
    assert_eq!(
        started,
        Some(crate::runtime::MemberRetirementCause::Removed)
    );
}

/// Row: a removal whose member's retirement a crash interrupted finishes
/// after the restart: the name reads removed, and no member holds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_during_a_removal_finishes_it_after_restart() {
    let mut mob = StoreOwnedMob::new("remove-crash").await;
    let name = unique("crashed");
    committed(
        mob.create(host_create("create-crashed", &[&name]), ALLOW)
            .await,
    );
    mob.handle
        .ensure_member_live(&id(&name))
        .await
        .expect("wake the identity");
    let hold = super::actor::retirement_stage_hold::install(&id(&name), "retire-runtime-quiesce");
    let removal = {
        let handle = mob.handle.clone();
        let name = name.clone();
        tokio::spawn(async move {
            handle
                .remove_identity(
                    crate::runtime::IdentityRemoval::new(
                        operation("remove-crashed"),
                        IdentityChangeRequesterInput::host("console"),
                        name,
                    ),
                    ALLOW,
                )
                .await
        })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the retirement reaches its held stage");
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    hold.release();
    removal.abort();
    mob.removed(&name).await;
    assert!(!mob.is_member(&name).await);
    assert_eq!(
        mob.row(&name).await.presence,
        CreatedIdentityPresence::Removed
    );
}

/// Row: a removed name is refused by everything that would take it again:
/// a create (`name_taken` by a removed identity), a declaration
/// (`NameTombstoned`), a spawn and a legacy adoption.
#[tokio::test]
async fn a_tombstoned_name_is_refused_by_create_declare_spawn_and_adopt() {
    let mob = StoreOwnedMob::new("remove-reserved").await;
    committed(
        mob.create(host_create("create-gone", &["gone"]), ALLOW)
            .await,
    );
    committed(mob.remove("remove-gone", "gone").await);
    mob.removed("gone").await;
    assert_eq!(
        refusal(
            mob.create(host_create("create-again", &["gone"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::NameTaken {
            identity: id("gone"),
            by: IdentityNameHolder::Removed
        }
    );
    let declared = mob.declare(&["gone"]).await;
    assert!(matches!(
        declared.entries.get(&id("gone")),
        Some(crate::runtime::DeclaredEntryOutcome::Refused {
            refusal: crate::runtime::DeclaredEntryRefusal::NameTombstoned
        })
    ));
    assert!(matches!(
        mob.handle
            .spawn_spec(SpawnMemberSpec::new("worker", id("gone")))
            .await,
        Err(MobError::IdentityMember { .. })
    ));
    assert!(!matches!(
        mob.handle.adopt_legacy_identity_entry(&id("gone")).await,
        Ok(crate::runtime::LegacyIdentityEntryAdoption::Adopted)
    ));
    assert!(!mob.is_member("gone").await);
}

/// Row: an unlisting is not a removal: a complete roster that leaves an
/// identity out retires its member and ends its entry, with no tombstone,
/// and listing it again seats it.
#[tokio::test]
async fn an_unlisting_is_not_a_removal_and_writes_no_tombstone() {
    let mob = StoreOwnedMob::new("unlist-not-removal").await;
    mob.declare(&["kept", "left"]).await;
    tokio::time::timeout(
        STEP,
        mob.handle
            .apply_declared_roster(DeclaredRoster::complete([DeclaredEntry::new(roster_spec(
                "kept",
            ))])),
    )
    .await
    .expect("the apply answers")
    .expect("the apply runs");
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.is_member("left").await {
        assert!(tokio::time::Instant::now() < deadline, "left never left");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(mob.rows().await.is_empty(), "no tombstone was written");
    assert!(!matches!(mob.read("left"), IdentityEntryRead::Removed));
    let relisted = mob.declare(&["left"]).await;
    assert!(matches!(
        relisted.entries.get(&id("left")),
        Some(crate::runtime::DeclaredEntryOutcome::Seated { .. })
    ));
}

// ---------------------------------------------------------------------------
// R2.15: archive and unarchive
// ---------------------------------------------------------------------------

/// Row (RED): archiving a store-owned identity retires its member and keeps
/// its entry (unseated, archived), its name and its record (presence
/// archived); a create of its name is still refused.
///
/// RED on the old tree: no archive exists.
#[tokio::test]
async fn archiving_a_store_owned_identity_retires_its_member_and_keeps_its_name_and_record() {
    let mob = StoreOwnedMob::new("archive-store-owned").await;
    committed(
        mob.create(host_create("create-shelf", &["shelf"]), ALLOW)
            .await,
    );
    let receipt = committed(mob.archive("archive-shelf", "shelf").await);
    assert_eq!(receipt.change, IdentityChangeKind::Archive);
    mob.unseated_for("shelf", UnseatedEntryCause::Archived)
        .await;
    assert!(!mob.is_member("shelf").await);
    assert!(matches!(
        mob.row("shelf").await.presence,
        CreatedIdentityPresence::Archived { .. }
    ));
    assert_eq!(
        refusal(
            mob.create(host_create("create-shelf-again", &["shelf"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::NameTaken {
            identity: id("shelf"),
            by: IdentityNameHolder::Identity
        }
    );
}

/// Row (RED): a delivery to an archived identity is refused
/// `identity_archived`, and nothing wakes.
///
/// RED on the old tree: no archive exists.
#[tokio::test]
async fn a_delivery_to_an_archived_identity_is_refused_identity_archived() {
    let mob = StoreOwnedMob::new("archive-delivery").await;
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    committed(mob.archive("archive-quiet", "quiet").await);
    mob.unseated_for("quiet", UnseatedEntryCause::Archived)
        .await;
    assert!(matches!(
        mob.handle.ensure_member_live(&id("quiet")).await,
        Err(MobError::IdentityArchived { member_id }) if member_id == id("quiet")
    ));
    assert!(!mob.is_member("quiet").await);
}

/// Row (RED): an archived identity stays archived across a restart.
///
/// RED on the old tree: no archive exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_archived_identity_stays_archived_across_restart() {
    let mut mob = StoreOwnedMob::new("archive-restart").await;
    committed(
        mob.create(host_create("create-kept", &["kept"]), ALLOW)
            .await,
    );
    committed(mob.archive("archive-kept", "kept").await);
    mob.unseated_for("kept", UnseatedEntryCause::Archived).await;
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    assert_eq!(
        mob.standing("kept"),
        Some(EntryStanding::Unseated(UnseatedEntryCause::Archived))
    );
    assert!(!mob.is_member("kept").await);
}

/// Row: removing an archived identity tombstones it at once.
#[tokio::test]
async fn removing_an_archived_identity_tombstones_it() {
    let mob = StoreOwnedMob::new("archive-remove").await;
    committed(mob.create(host_create("create-old", &["old"]), ALLOW).await);
    committed(mob.archive("archive-old", "old").await);
    mob.unseated_for("old", UnseatedEntryCause::Archived).await;
    let receipt = committed(mob.remove("remove-old", "old").await);
    assert_eq!(
        receipt.identities[0].outcome,
        IdentityChangeEntryOutcome::Removed
    );
    assert!(matches!(mob.read("old"), IdentityEntryRead::Removed));
    assert_eq!(
        mob.row("old").await.presence,
        CreatedIdentityPresence::Removed
    );
}

/// Row (RED): a roster-owned identity's archive is a one-entry unlist, and
/// its unarchive a re-list: it comes back on its own session.
///
/// RED on the old tree: no one-entry unlist exists.
#[tokio::test]
async fn a_one_entry_unlist_and_relist_keep_a_roster_owned_identitys_session() {
    let mob = StoreOwnedMob::new("unlist-one").await;
    mob.declare(&["listed", "other"]).await;
    let session = mob.session("listed").await;
    let unlisted = tokio::time::timeout(STEP, mob.handle.unlist_declared_identity(&id("listed")))
        .await
        .expect("the unlist answers")
        .expect("the unlist runs");
    assert!(unlisted.unlisted.contains_key(&id("listed")));
    assert!(!unlisted.unlisted.contains_key(&id("other")));
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.is_member("listed").await {
        assert!(tokio::time::Instant::now() < deadline, "listed never left");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(mob.is_member("other").await, "the other entry stays");
    assert!(mob.rows().await.is_empty(), "no tombstone");
    let relisted = mob.declare(&["listed"]).await;
    assert!(matches!(
        relisted.entries.get(&id("listed")),
        Some(crate::runtime::DeclaredEntryOutcome::Seated { session_id, .. })
            if *session_id == session
    ));
}

// ---------------------------------------------------------------------------
// R2.16: a roster listing of a store-owned name
// ---------------------------------------------------------------------------

/// Row (RED): a roster that lists a store-owned identity's name gets a typed
/// note, not a refusal, and nothing retires.
///
/// RED on the old tree: the apply answers `Refused { NameOwnedByCreated }`.
#[tokio::test]
async fn a_roster_listing_of_a_store_owned_name_is_a_typed_note() {
    let mob = StoreOwnedMob::new("listing-note").await;
    committed(
        mob.create(host_create("create-owned", &["owned"]), ALLOW)
            .await,
    );
    let outcome = mob.declare(&["owned"]).await;
    assert!(
        matches!(
            outcome.entries.get(&id("owned")),
            Some(crate::runtime::DeclaredEntryOutcome::Noted {
                note: crate::runtime::DeclaredEntryNote::OwnedByStore
            })
        ),
        "{outcome:?}"
    );
    assert!(mob.seated_dormant("owned"));
}

/// Row: a roster generation from before an identity became store-owned (one
/// that lists it, or one that omits it) retires nothing.
#[tokio::test]
async fn a_pre_adopt_roster_generation_retires_nothing() {
    let mob = StoreOwnedMob::new("pre-adopt-roster").await;
    committed(
        mob.create(host_create("create-owned", &["owned"]), ALLOW)
            .await,
    );
    for roster in [
        DeclaredRoster::complete([DeclaredEntry::new(roster_spec("owned"))]),
        DeclaredRoster::complete([DeclaredEntry::new(roster_spec("listed"))]),
    ] {
        tokio::time::timeout(STEP, mob.handle.apply_declared_roster(roster))
            .await
            .expect("the apply answers")
            .expect("the apply runs");
        assert!(mob.seated_dormant("owned"), "{:?}", mob.read("owned"));
    }
}

// ---------------------------------------------------------------------------
// R2.5: store-owned updates
// ---------------------------------------------------------------------------

impl StoreOwnedMob {
    /// A host surface's update of `name` to `spec` as `operation_id`,
    /// decided against the spec whose digest is `expected`.
    async fn update(
        &self,
        operation_id: &str,
        spec: CreatedIdentitySpec,
        expected: &str,
    ) -> Result<crate::runtime::IdentityUpdateOutcome, MobError> {
        tokio::time::timeout(
            STEP,
            self.handle.update_store_owned_identity(
                crate::runtime::StoreOwnedIdentityUpdate::new(
                    operation(operation_id),
                    IdentityChangeRequesterInput::host("console"),
                    spec,
                    expected,
                ),
                ALLOW,
            ),
        )
        .await
        .expect("the update answers")
    }

    async fn role(&self, name: &str) -> Option<ProfileName> {
        self.handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .map(|entry| entry.role)
    }

    async fn generation(&self, name: &str) -> Option<crate::ids::Generation> {
        self.handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .map(|entry| entry.generation)
    }
}

fn spec_digest(spec: &CreatedIdentitySpec) -> String {
    spec.spec_digest().expect("the spec's digest")
}

/// The answer's kind, for comparing a store-owned update's answer with the
/// declared apply's.
fn answer_kind(outcome: &crate::runtime::DeclaredEntryOutcome) -> &'static str {
    use crate::runtime::DeclaredEntryOutcome as Outcome;
    match outcome {
        Outcome::Seated { .. } => "seated",
        Outcome::Embodied { .. } => "embodied",
        Outcome::Kept => "kept",
        Outcome::Restamped { .. } => "restamped",
        Outcome::RoleMigrated { .. } => "role_migrated",
        Outcome::RoleRotated { .. } => "role_rotated",
        Outcome::FreshLife { .. } => "fresh_life",
        Outcome::RoleMigrationHeld { .. } => "role_migration_held",
        Outcome::AwaitingHeldCreation => "awaiting_held_creation",
        Outcome::PreparationPending => "preparation_pending",
        Outcome::HostedOnFirstUse => "hosted_on_first_use",
        Outcome::Deferred { .. } => "deferred",
        Outcome::Refused { .. } => "refused",
        Outcome::Noted { .. } => "noted",
        Outcome::Failed { .. } => "failed",
    }
}

/// Row (RED): a store-owned update and a roster-owned identity's change of
/// the same spec answer the same: a descriptive change, a label change and
/// a placement change keep the member, a runtime-mode change restamps it,
/// a profile change moves it to the new profile (the store-owned one's
/// migration declared by the update, the roster-owned one's by its roster).
///
/// RED on the old tree: no store-owned update exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_and_roster_updates_of_the_same_change_answer_the_same_verdict() {
    type Change = fn(CreatedIdentitySpec) -> CreatedIdentitySpec;
    let changes: [(&str, Change); 5] = [
        ("descriptive", |spec| spec.with_display_name("Renamed")),
        ("labels", |spec| {
            let mut labels = spec.labels.clone();
            labels.insert("tier".to_string(), "gold".to_string());
            spec.with_labels(labels)
        }),
        ("runtime_mode", |spec| {
            spec.with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
        }),
        ("profile", |mut spec| {
            spec.profile = ProfileName::from("lead");
            spec
        }),
        ("placement", |spec| {
            spec.with_placement(meerkat_contracts::WireHostRef("host-peer-1".to_string()))
        }),
    ];
    for (label, change) in changes {
        let mob = StoreOwnedMob::new(&format!("update-parity-{label}")).await;
        let base = |name: &str| {
            CreatedIdentitySpec::new(name, "worker")
                .with_runtime_mode_override(crate::MobRuntimeMode::TurnDriven)
        };
        // The roster-owned identity, declared with the base spec, then with
        // the changed one.
        let roster_entry = |spec: &CreatedIdentitySpec| {
            let mut lowered =
                super::super::identity_change::lower_created_spec(spec).expect("lower the spec");
            lowered.created_custody = false;
            DeclaredEntry::new(lowered)
        };
        let roster_base = base("roster");
        mob.handle
            .apply_declared_roster(DeclaredRoster::partial([roster_entry(&roster_base)]))
            .await
            .expect("declare the roster-owned identity");
        let roster_changed = change(roster_base.clone());
        let mut roster = DeclaredRoster::partial([roster_entry(&roster_changed)]);
        if roster_changed.profile != roster_base.profile {
            roster = roster.with_role_migration("roster", roster_base.profile.clone());
        }
        let roster_answer = mob
            .handle
            .apply_declared_roster(roster)
            .await
            .expect("change the roster-owned identity");
        let roster_kind = answer_kind(
            roster_answer
                .entries
                .get(&id("roster"))
                .expect("the roster-owned answer"),
        );

        let store_base = base("store");
        committed(
            mob.create(
                StoreOwnedIdentityCreate::new(
                    operation("create-store"),
                    IdentityChangeRequesterInput::host("console"),
                    vec![CreatedIdentityRequest::new(store_base.clone())],
                ),
                ALLOW,
            )
            .await,
        );
        let updated = mob
            .update(
                "update-store",
                change(store_base.clone()),
                &spec_digest(&store_base),
            )
            .await
            .expect("update the store-owned identity");
        let store_kind = answer_kind(updated.applied.as_ref().expect("the update's answer"));
        assert_eq!(store_kind, roster_kind, "{label}");
    }
}

/// Row (RED): a store-owned profile change restamps the identity's member on
/// its own session under the new profile, and its row holds the new spec at
/// the next revision with no pending migration once the restamp committed.
///
/// RED on the old tree: no store-owned update exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_owned_profile_change_restamps_on_the_same_session_under_the_new_profile() {
    let mob = StoreOwnedMob::new("update-profile").await;
    let before = spec("mover", "worker");
    committed(
        mob.create(host_create("create-mover", &["mover"]), ALLOW)
            .await,
    );
    mob.handle
        .ensure_member_live(&id("mover"))
        .await
        .expect("wake the identity");
    let session = mob.session("mover").await;
    let mut after = before.clone();
    after.profile = ProfileName::from("lead");
    let updated = mob
        .update("update-mover", after.clone(), &spec_digest(&before))
        .await
        .expect("update the profile");
    assert!(
        matches!(
            updated.applied,
            Some(crate::runtime::DeclaredEntryOutcome::RoleMigrated { .. })
        ),
        "{updated:?}"
    );
    assert_eq!(mob.role("mover").await, Some(ProfileName::from("lead")));
    assert_eq!(mob.session("mover").await, session, "the same session");
    let row = mob.row("mover").await;
    assert_eq!(row.revision, 2);
    assert_eq!(row.spec, Some(after));
    assert_eq!(row.pending_role_migration, None);
}

/// Row (RED): an approved store-owned profile change whose restamp a crash
/// prevented is carried out after the restart. The update's transaction
/// committed (the identity's row at its next revision under the new profile,
/// with the migration it declares from the old one, and its receipt), and
/// the process stopped before the restamp: the start restamps the member
/// under the new profile on its own session, and the row lets the migration
/// go once the restamp committed.
///
/// RED on the old tree: nothing reads a row's pending migration, and the
/// member keeps its old profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approved_store_owned_profile_change_survives_a_restart_before_its_restamp() {
    let mut mob = StoreOwnedMob::new("update-profile-crash").await;
    committed(
        mob.create(host_create("create-migrating", &["migrating"]), ALLOW)
            .await,
    );
    let session = mob.session("migrating").await;
    // The update's transaction as its commit step writes it, and no restamp
    // after it.
    let before = mob.row("migrating").await;
    let mut spec = before.spec.clone().expect("the row's spec");
    spec.profile = ProfileName::from("lead");
    let after = before
        .clone()
        .updated(
            spec.clone(),
            Some(crate::identity::PendingRoleMigration::new("worker")),
        )
        .expect("the updated row");
    let receipt = crate::IdentityChangeReceipt::sealed(
        mob.mob_id(),
        "update-migrating",
        digest("update-migrating"),
        IdentityChangeKind::Update,
        vec![
            crate::identity::IdentityChangeReceiptEntry::new(
                id("migrating"),
                IdentityChangeEntryOutcome::Committed,
            )
            .with_spec(spec_digest(&spec), Some(after.revision)),
        ],
        1,
    )
    .expect("the update's receipt");
    let store = mob
        .storage
        .identity_member
        .clone()
        .expect("the built-in identity store");
    let written = crate::store::MobIdentityMemberStore::commit_identity_change(
        store.as_ref(),
        &crate::store::IdentityChangeWrite {
            receipt,
            rows: vec![crate::store::CreatedIdentityRowWrite {
                record: after,
                expected: crate::store::CreatedIdentityRowExpectation::Current {
                    authority_digest: before.authority_digest.clone(),
                },
            }],
            events: Vec::new(),
        },
    )
    .await
    .expect("the update's transaction");
    assert!(matches!(
        written,
        crate::store::IdentityChangeCommit::Committed { .. }
    ));
    assert_eq!(
        mob.role("migrating").await,
        Some(ProfileName::from("worker"))
    );

    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.role("migrating").await != Some(ProfileName::from("lead")) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the restart never restamped the member under its new profile"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(mob.session("migrating").await, session, "the same session");
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.row("migrating").await.pending_role_migration.is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the row never let the migration go"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(mob.row("migrating").await.revision, 2);
}

/// Row: a descriptive store-owned update keeps the member: no restamp (the
/// generation stays), and the row holds the new spec at the next revision.
/// The member's next build applies it: the first wake of its seat builds
/// with the updated context and instructions.
#[tokio::test]
async fn a_descriptive_store_owned_update_applies_at_the_next_build_and_restamps_nothing() {
    let mob = StoreOwnedMob::new("update-descriptive").await;
    let before = spec("quiet", "worker");
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let generation = mob.generation("quiet").await;
    let session = mob.session("quiet").await;
    let after = before
        .clone()
        .with_display_name("Quieter")
        .with_context(serde_json::json!({"desk": "moved"}))
        .with_additional_instructions(vec!["Speak softly.".to_string()]);
    let updated = mob
        .update("update-quiet", after.clone(), &spec_digest(&before))
        .await
        .expect("update the descriptive facets");
    assert!(
        matches!(
            updated.applied,
            Some(crate::runtime::DeclaredEntryOutcome::Kept)
        ),
        "{updated:?}"
    );
    assert_eq!(mob.generation("quiet").await, generation, "no restamp");
    assert_eq!(mob.builds().await, 0, "nothing was built");
    let row = mob.row("quiet").await;
    assert_eq!(row.revision, 2);
    assert_eq!(row.spec, Some(after));

    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("quiet")))
        .await
        .expect("the wake answers")
        .expect("the wake builds the member");
    let builds = mob
        .service
        .recorded_create_requests()
        .await
        .into_iter()
        .filter(|record| record.session_id == session)
        .collect::<Vec<_>>();
    assert_eq!(builds.len(), 1, "one build of the seat's session");
    assert_eq!(
        builds[0].app_context,
        Some(serde_json::json!({"desk": "moved"})),
        "the next build applies the updated context"
    );
    assert_eq!(
        builds[0].additional_instructions,
        Some(vec!["Speak softly.".to_string()]),
        "the next build applies the updated instructions"
    );
}

/// Row: an update decided against a spec that is no longer current is
/// refused `stale_spec` with the current digest and revision, and nothing
/// is written.
#[tokio::test]
async fn a_stale_expected_revision_is_refused_with_the_current_one() {
    let mob = StoreOwnedMob::new("update-stale").await;
    let first = spec("stale", "worker");
    committed(
        mob.create(host_create("create-stale", &["stale"]), ALLOW)
            .await,
    );
    let second = first.clone().with_display_name("Second");
    mob.update("update-second", second.clone(), &spec_digest(&first))
        .await
        .expect("the first update");
    assert_eq!(
        refusal(
            mob.update(
                "update-third",
                first.clone().with_display_name("Third"),
                &spec_digest(&first)
            )
            .await
        ),
        IdentityChangeRefusal::StaleSpec {
            identity: id("stale"),
            current_digest: spec_digest(&second),
            current_revision: Some(2),
        }
    );
    assert_eq!(mob.row("stale").await.revision, 2);
}

/// Row: a retried update that committed replays its outcome, never a stale
/// spec, and writes nothing again.
#[tokio::test]
async fn a_retried_update_that_committed_replays_its_outcome() {
    let mob = StoreOwnedMob::new("update-replay").await;
    let first = spec("again", "worker");
    committed(
        mob.create(host_create("create-again", &["again"]), ALLOW)
            .await,
    );
    let second = first.clone().with_display_name("Again");
    let committed_update = mob
        .update("update-again", second.clone(), &spec_digest(&first))
        .await
        .expect("the update");
    let receipt = committed_update.change.receipt().clone();
    let retried = mob
        .update("update-again", second, &spec_digest(&first))
        .await
        .expect("the retry");
    assert!(
        matches!(
            &retried.change,
            IdentityChangeOutcome::Replayed { receipt: replayed } if *replayed == receipt
        ),
        "{retried:?}"
    );
    assert_eq!(mob.row("again").await.revision, 2);
}

// ---------------------------------------------------------------------------
// R2.9 and R2.10: changes without a restart; a roster-owned change's receipt
// ---------------------------------------------------------------------------

/// Row: a create, an update and a removal each take effect in the running
/// mob, with no restart.
#[tokio::test]
async fn create_update_and_remove_need_no_restart() {
    let mob = StoreOwnedMob::new("change-live").await;
    let first = spec("live", "worker");
    committed(
        mob.create(host_create("create-live", &["live"]), ALLOW)
            .await,
    );
    assert!(mob.seated_dormant("live"));
    let second = first.clone().with_display_name("Live");
    mob.update("update-live", second.clone(), &spec_digest(&first))
        .await
        .expect("the update");
    assert_eq!(mob.row("live").await.spec, Some(second));
    committed(mob.remove("remove-live", "live").await);
    mob.removed("live").await;
}

/// Row (RED): a roster-owned identity's change, applied by a partial
/// declared apply with its operation, writes its receipt with the apply's
/// declaration step; a change the apply answers `Kept` writes its receipt
/// only.
///
/// RED on the old tree: the apply writes no receipt.
#[tokio::test]
async fn a_roster_owned_change_writes_its_receipt_with_the_apply() {
    let mob = StoreOwnedMob::new("roster-receipt").await;
    let change = |operation_id: &str| {
        crate::runtime::IdentityRosterChange::new(
            operation(operation_id),
            IdentityChangeRequester::Host {
                surface: "console".to_string(),
            },
            IdentityChangeConsent::Policy {
                outcome: IdentityPolicyOutcome::Allow,
            },
        )
    };
    let seated = tokio::time::timeout(
        STEP,
        mob.handle.apply_declared_roster(
            DeclaredRoster::partial([DeclaredEntry::new(roster_spec("listed"))])
                .with_change(change("declare-listed")),
        ),
    )
    .await
    .expect("the apply answers")
    .expect("the apply runs");
    let receipt = seated.receipt.clone().expect("the apply's receipt");
    assert_eq!(receipt.change, IdentityChangeKind::RosterApply);
    assert_eq!(receipt.identities[0].identity, id("listed"));
    assert_eq!(
        mob.handle
            .identity_operation("declare-listed")
            .await
            .expect("read the receipt"),
        Some(receipt)
    );
    let kept = tokio::time::timeout(
        STEP,
        mob.handle.apply_declared_roster(
            DeclaredRoster::partial([DeclaredEntry::new(roster_spec("listed"))])
                .with_change(change("keep-listed")),
        ),
    )
    .await
    .expect("the apply answers")
    .expect("the apply runs");
    assert!(matches!(
        kept.entries.get(&id("listed")),
        Some(crate::runtime::DeclaredEntryOutcome::Kept)
    ));
    assert!(kept.receipt.is_some(), "a kept change writes its receipt");
    assert!(
        mob.rows().await.is_empty(),
        "a roster-owned change writes no row"
    );
}

// ---------------------------------------------------------------------------
// R2.11: recovery from the row's spec
// ---------------------------------------------------------------------------

/// Row (RED): a store-owned identity whose member was retired outside its
/// changes (an older build's reconcile, here the host's own retire) is seated
/// again at the next start from its row's spec, not from its last spawn row:
/// a label its row took since its seat is on the new seat.
///
/// RED on the old tree: the start seats it from the last spawn row, whose
/// labels predate the update.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_present_store_owned_entry_retired_by_an_older_build_is_reseated_with_its_v2_spec() {
    let mut mob = StoreOwnedMob::new("reseat-v2").await;
    let first = spec("phoenix", "worker");
    committed(
        mob.create(host_create("create-phoenix", &["phoenix"]), ALLOW)
            .await,
    );
    let mut labels = first.labels.clone();
    labels.insert("tier".to_string(), "gold".to_string());
    let second = first.clone().with_labels(labels.clone());
    mob.update("update-phoenix", second, &spec_digest(&first))
        .await
        .expect("the label update");
    mob.handle
        .retire(id("phoenix"))
        .await
        .expect("retire the member outside its changes");
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    let deadline = tokio::time::Instant::now() + STEP;
    while !mob.is_member("phoenix").await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "phoenix was never seated again"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        mob.handle
            .get_member(&id("phoenix"))
            .await
            .expect("roster read")
            .expect("the seated identity")
            .labels,
        labels,
        "the seat carries the row's labels"
    );
}

// ---------------------------------------------------------------------------
// R2.7: placement and held creation for store-owned identities
// ---------------------------------------------------------------------------

/// The admission verdict MobMachine answers a spawn of `name` holding the
/// given custodies, probed on a copy of the mob's published state (nothing
/// is applied to the mob).
fn probed_admission(
    mob: &StoreOwnedMob,
    name: &str,
    entry_custody: bool,
    created_custody: bool,
) -> crate::machines::mob_machine::MemberAdmissionVerdictKind {
    use crate::machines::mob_machine as dsl;
    let state = mob.handle.machine_state_watch_rx.borrow().clone();
    let mut authority =
        dsl::MobMachineAuthority::recover_from_state(state).expect("the published state recovers");
    let transition = dsl::MobMachineMutator::apply(
        &mut authority,
        dsl::MobMachineInput::ProbeMemberAdmission {
            agent_identity: dsl::AgentIdentity::from_domain(&id(name)),
            entry_custody,
            created_custody,
        },
    )
    .expect("the probe answers");
    transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            dsl::MobMachineEffect::MemberAdmissionProbed { verdict, .. } => Some(*verdict),
            _ => None,
        })
        .expect("the probe's verdict")
}

/// Row (RED): a placed store-owned identity is hosted on its first use. The
/// create commits its row and records it unseated, hosted on its first use,
/// and seats nothing; a dormant seat of it is refused; and the spawn that
/// creates it is its host's spawn holding its created custody, which
/// MobMachine admits for it (a placed spawn's admission among them), while a
/// spawn without custody is refused its name. The placed ladder's own steps
/// are the machine row
/// `a_placed_ladder_under_created_custody_takes_a_hosted_store_owned_entry`.
///
/// RED on the old tree: a placed store-owned create is refused unsupported.
#[tokio::test]
async fn a_placed_store_owned_identity_is_hosted_on_first_use_by_a_placed_spawn() {
    let mob = StoreOwnedMob::new("placed-store-owned").await;
    let placed = spec("placed", "worker")
        .with_placement(meerkat_contracts::WireHostRef("placed-host".to_string()));
    let receipt = committed(
        mob.create(
            StoreOwnedIdentityCreate::new(
                operation("create-placed"),
                IdentityChangeRequesterInput::host("console"),
                vec![CreatedIdentityRequest::new(placed)],
            ),
            ALLOW,
        )
        .await,
    );
    assert_eq!(
        receipt.identities[0].outcome,
        IdentityChangeEntryOutcome::HostedOnFirstUse
    );
    mob.unseated_for("placed", UnseatedEntryCause::HostedOnFirstUse)
        .await;
    assert!(!mob.is_member("placed").await, "nothing is seated");
    let refused = mob
        .handle
        .seat_dormant_members(vec![
            SpawnMemberSpec::host_root("worker", id("placed")).with_created_custody(true),
        ])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            refused[0],
            crate::runtime::DormantSeatOutcome::Refused { .. }
        ),
        "a dormant seat never creates a hosted identity: {refused:?}"
    );
    assert!(!mob.is_member("placed").await);
    use crate::machines::mob_machine::MemberAdmissionVerdictKind as Verdict;
    assert_eq!(
        probed_admission(&mob, "placed", false, true),
        Verdict::Admitted,
        "its host's spawn holding its created custody creates it"
    );
    assert_eq!(
        probed_admission(&mob, "placed", false, false),
        Verdict::IdentityMemberRejected,
        "a spawn without custody is refused its name"
    );
    assert_eq!(
        probed_admission(&mob, "placed", true, false),
        Verdict::IdentityMemberOfAnotherOrigin,
        "entry custody never takes a store-owned identity's name"
    );
}

/// A mob whose members are registered and finished by a selecting host, so
/// fresh members can be held, over in-memory storage with its identity store.
/// A restart resumes the same storage over a cold-restarted session service.
struct HeldMob {
    storage: MobStorage,
    handle: MobHandle,
    service: Arc<MockSessionService>,
}

impl HeldMob {
    fn compose(service: &MockSessionService) {
        let machine = Arc::new(
            meerkat_runtime::MeerkatMachine::ephemeral()
                .with_native_work_authorization_host(Arc::new(super::held_spawn::AdmitsNothing))
                .expect("install the governed work host"),
        );
        service.set_runtime_adapter(machine);
        service.decide_tool_surface_at_create();
    }

    async fn new(label: &str) -> Self {
        let storage = MobStorage::in_memory();
        let service = Arc::new(MockSessionService::new());
        Self::compose(&service);
        let handle = MobBuilder::new(store_owned_definition(label), storage.clone())
            .with_session_service(service.clone())
            .with_member_runtime_registration_hook(Arc::new(
                super::held_spawn::SelectingHost::default(),
            ))
            .create()
            .await
            .expect("create the mob");
        Self {
            storage,
            handle,
            service,
        }
    }

    async fn restart(&mut self) {
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        Self::compose(&restarted);
        crash_stop_and_release_routes(self.handle.clone()).await;
        self.service = restarted;
        self.handle = tokio::time::timeout(
            STEP,
            Box::pin(
                MobBuilder::for_resume(self.storage.clone())
                    .with_session_service(self.service.clone())
                    .with_member_runtime_registration_hook(Arc::new(
                        super::held_spawn::SelectingHost::default(),
                    ))
                    .notify_orchestrator_on_resume(false)
                    .resume(),
            ),
        )
        .await
        .expect("the restart answers")
        .expect("the restart succeeds");
    }

    fn standing(&self, name: &str) -> Option<EntryStanding> {
        match self
            .handle
            .identity_entry(&id(name))
            .expect("the entry read answers")
        {
            IdentityEntryRead::Entry(view) => Some(view.standing),
            _ => None,
        }
    }

    fn origin(&self, name: &str) -> Option<IdentityEntryOrigin> {
        match self
            .handle
            .identity_entry(&id(name))
            .expect("the entry read answers")
        {
            IdentityEntryRead::Entry(view) => Some(view.origin),
            _ => None,
        }
    }
}

/// Row (RED): a store-owned identity whose host holds its creation is
/// created by the held protocol and never seated Dormant. The create
/// records it awaiting its held creation and seats nothing; a dormant seat
/// of it is refused; a restart keeps it awaiting its held creation (its row
/// keeps the fact); and its host's held spawn holding its created custody
/// creates it once activated, a store-owned entry.
///
/// RED on the old tree: the create seats the identity Dormant.
#[tokio::test]
async fn a_held_store_owned_identity_is_created_by_the_held_protocol_and_never_seated_dormant() {
    let mut mob = HeldMob::new("held-store-owned").await;
    let held = spec("held", "worker");
    committed(
        tokio::time::timeout(
            STEP,
            mob.handle.create_store_owned_identities(
                StoreOwnedIdentityCreate::new(
                    operation("create-held"),
                    IdentityChangeRequesterInput::host("console"),
                    vec![CreatedIdentityRequest::new(held).with_held_creation(true)],
                ),
                ALLOW,
            ),
        )
        .await
        .expect("the create answers"),
    );
    let awaiting = Some(EntryStanding::Unseated(UnseatedEntryCause::HeldCreation));
    assert_eq!(
        mob.standing("held"),
        awaiting,
        "it awaits its held creation"
    );
    let refused = mob
        .handle
        .seat_dormant_members(vec![
            SpawnMemberSpec::host_root("worker", id("held")).with_created_custody(true),
        ])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            refused[0],
            crate::runtime::DormantSeatOutcome::Refused { .. }
        ),
        "a dormant seat never creates a held identity: {refused:?}"
    );

    mob.restart().await;
    assert_eq!(
        mob.standing("held"),
        awaiting,
        "a restart keeps it awaiting its held creation"
    );

    let mut spec = SpawnMemberSpec::host_root("worker", id("held")).with_created_custody(true);
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    let spawn = tokio::time::timeout(
        STEP,
        mob.handle
            .spawn_held_within(spec, tokio::time::Instant::now() + STEP),
    )
    .await
    .expect("the held spawn reaches its hold point")
    .expect("the identity is held");
    tokio::time::timeout(STEP, spawn.activate())
        .await
        .expect("the activation answers")
        .expect("the activation publishes the member");
    assert!(
        matches!(mob.standing("held"), Some(EntryStanding::Seated(_))),
        "{:?}",
        mob.standing("held")
    );
    assert_eq!(mob.origin("held"), Some(IdentityEntryOrigin::Created));
}

// ---------------------------------------------------------------------------
// R2.8 and R2.6: host preparation for either custody, held constructions
// ---------------------------------------------------------------------------

/// A host surface's settle of `name`'s preparation as `operation_id`,
/// activating `attempt`'s held construction when it names one.
async fn settle(
    handle: &MobHandle,
    operation_id: &str,
    name: &str,
    attempt: Option<u64>,
) -> Result<crate::runtime::IdentityPreparationSettled, MobError> {
    let mut settle = crate::runtime::IdentityPreparationSettle::new(
        operation(operation_id),
        IdentityChangeRequesterInput::host("console"),
        name,
    );
    if let Some(attempt) = attempt {
        settle = settle.with_attempt(attempt);
    }
    tokio::time::timeout(STEP, handle.settle_identity_preparation(settle, ALLOW))
        .await
        .expect("the settle answers")
}

/// A host surface's create of `name` on the worker profile, prepared by its
/// host as `preparation`.
fn prepared_create(
    operation_id: &str,
    spec: CreatedIdentitySpec,
    preparation: IdentityPreparationMode,
) -> StoreOwnedIdentityCreate {
    StoreOwnedIdentityCreate::new(
        operation(operation_id),
        IdentityChangeRequesterInput::host("console"),
        vec![CreatedIdentityRequest::new(spec).with_preparation(preparation)],
    )
}

/// Whether `error` is the retryable refusal of a delivery to an identity
/// awaiting its host's preparation.
fn preparation_refusal(error: &MobError, name: &str) -> bool {
    matches!(
        error,
        MobError::MemberWakeRefused {
            member_id,
            refusal: crate::MemberWakeRefusal::HostPreparationPending,
        } if *member_id == id(name)
    )
}

/// Row (RED): a roster-owned identity whose host's rule requires its
/// preparation waits unseated for it: the apply answers it pending and
/// seats nothing, a delivery to it is refused retryable, an apply that
/// still derives the preparation keeps it pending, and the host's
/// declaration with no preparation (its settle) seats it Dormant.
///
/// RED on the old tree: the apply ignores the preparation and seats it.
#[tokio::test]
async fn a_roster_owned_identity_with_preparation_waits_unseated_until_settled() {
    let mob = StoreOwnedMob::new("roster-preparing").await;
    let declared = |preparation| {
        DeclaredRoster::partial([
            DeclaredEntry::new(roster_spec("prepared")).with_preparation(preparation)
        ])
    };
    for _ in 0..2 {
        let pending = tokio::time::timeout(
            STEP,
            mob.handle
                .apply_declared_roster(declared(IdentityPreparationMode::Required)),
        )
        .await
        .expect("the apply answers")
        .expect("the apply runs");
        assert!(
            matches!(
                pending.entries.get(&id("prepared")),
                Some(crate::runtime::DeclaredEntryOutcome::PreparationPending)
            ),
            "{pending:?}"
        );
        mob.unseated_for("prepared", UnseatedEntryCause::PreparationPending)
            .await;
        assert!(!mob.is_member("prepared").await);
        let refused = mob
            .handle
            .ensure_member_live(&id("prepared"))
            .await
            .expect_err("a delivery to an identity awaiting its preparation is refused");
        assert!(preparation_refusal(&refused, "prepared"), "{refused:?}");
    }
    let settled = tokio::time::timeout(
        STEP,
        mob.handle
            .apply_declared_roster(declared(IdentityPreparationMode::None)),
    )
    .await
    .expect("the apply answers")
    .expect("the apply runs");
    assert!(
        matches!(
            settled.entries.get(&id("prepared")),
            Some(crate::runtime::DeclaredEntryOutcome::Seated { .. })
        ),
        "{settled:?}"
    );
    assert!(mob.is_member("prepared").await);
}

/// Row (RED): a store-owned identity whose host's rule requires its
/// preparation is created pending and unseated; a delivery to it is refused
/// retryable; and its settle seats it Dormant from its row's spec, its row
/// present again, with the settle's receipt.
///
/// RED on the old tree: no settle exists.
#[tokio::test]
async fn a_store_owned_identity_with_preparation_waits_unseated_and_a_delivery_is_refused_retryable()
 {
    let mob = StoreOwnedMob::new("store-preparing").await;
    committed(
        mob.create(
            prepared_create(
                "create-pending",
                spec("pending", "worker"),
                IdentityPreparationMode::Required,
            ),
            ALLOW,
        )
        .await,
    );
    mob.unseated_for("pending", UnseatedEntryCause::PreparationPending)
        .await;
    let refused = mob
        .handle
        .ensure_member_live(&id("pending"))
        .await
        .expect_err("a delivery to an identity awaiting its preparation is refused");
    assert!(preparation_refusal(&refused, "pending"), "{refused:?}");
    assert!(!mob.is_member("pending").await);

    let settled = settle(&mob.handle, "settle-pending", "pending", None)
        .await
        .expect("the settle commits");
    let receipt = committed(Ok(settled.change));
    assert_eq!(receipt.change, IdentityChangeKind::SettlePreparation);
    assert!(
        matches!(
            settled.settlement,
            Some(crate::runtime::PreparationSettlement::Seated(
                crate::runtime::DeclaredEntryOutcome::Seated { .. }
            ))
        ),
        "{:?}",
        settled.settlement
    );
    assert!(mob.seated_dormant("pending"));
    assert_eq!(
        mob.row("pending").await.presence,
        CreatedIdentityPresence::Present
    );
}

/// Row (RED): a settle of an identity whose spec owes a kickoff embodies it
/// and runs the kickoff once; a retry of the settle replays its receipt and
/// runs nothing again, and a later settle finds no preparation pending.
///
/// RED on the old tree: no settle exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_settle_seats_dormant_and_runs_an_owed_kickoff_once() {
    let mob = StoreOwnedMob::new("settle-kickoff").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let kicked = spec("kicked", "worker")
        .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
        .with_initial_message(meerkat_core::types::ContentInput::from("Begin."));
    committed(
        mob.create(
            prepared_create("create-kicked", kicked, IdentityPreparationMode::Required),
            ALLOW,
        )
        .await,
    );
    let settled = settle(&mob.handle, "settle-kicked", "kicked", None)
        .await
        .expect("the settle commits");
    assert!(
        matches!(
            settled.settlement,
            Some(crate::runtime::PreparationSettlement::Seated(
                crate::runtime::DeclaredEntryOutcome::Embodied { .. }
            ))
        ),
        "{:?}",
        settled.settlement
    );
    mob.handle
        .wait_for_members_kickoff_complete(&[id("kicked")], Some(STEP))
        .await
        .expect("the kickoff resolves");
    let session = mob.session("kicked").await;
    let kickoffs = || async {
        mob.service
            .recorded_start_turn_prompts()
            .await
            .into_iter()
            .filter(|(ran_on, ran)| *ran_on == session && ran == "Begin.")
            .count()
    };
    let deadline = tokio::time::Instant::now() + STEP;
    while kickoffs().await == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the owed kickoff never ran"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(kickoffs().await, 1, "the kickoff runs once");
    let retried = settle(&mob.handle, "settle-kicked", "kicked", None)
        .await
        .expect("the retry answers");
    assert!(matches!(
        retried.change,
        IdentityChangeOutcome::Replayed { .. }
    ));
    assert_eq!(kickoffs().await, 1, "a retried settle runs nothing again");
    assert_eq!(
        refusal(settle(&mob.handle, "settle-kicked-again", "kicked", None).await),
        IdentityChangeRefusal::PreparationNotPending {
            identity: id("kicked")
        }
    );
}

/// Row (RED): an identity whose preparation is constructed is never seated
/// Dormant: its settle without an attempt is refused `construction_required`,
/// a dormant seat of it is refused, and it stays pending.
///
/// RED on the old tree: no settle exists.
#[tokio::test]
async fn a_constructed_profile_refuses_a_settle_without_an_attempt_and_never_seats_dormant() {
    let mob = StoreOwnedMob::new("constructed-dormant").await;
    committed(
        mob.create(
            prepared_create(
                "create-built",
                spec("built", "worker"),
                IdentityPreparationMode::Constructed,
            ),
            ALLOW,
        )
        .await,
    );
    assert_eq!(
        refusal(settle(&mob.handle, "settle-built", "built", None).await),
        IdentityChangeRefusal::ConstructionRequired {
            identity: id("built")
        }
    );
    let refused = mob
        .handle
        .seat_dormant_members(vec![
            SpawnMemberSpec::host_root("worker", id("built")).with_created_custody(true),
        ])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            refused[0],
            crate::runtime::DormantSeatOutcome::Refused { .. }
        ),
        "{refused:?}"
    );
    mob.unseated_for("built", UnseatedEntryCause::PreparationPending)
        .await;
    assert!(matches!(
        mob.row("built").await.presence,
        CreatedIdentityPresence::PendingPreparation {
            constructed: true,
            ..
        }
    ));
}

impl HeldMob {
    async fn create_constructed(&self, name: &str) {
        committed(
            tokio::time::timeout(
                STEP,
                self.handle.create_store_owned_identities(
                    prepared_create(
                        &format!("create-{name}"),
                        spec(name, "worker")
                            .with_runtime_mode_override(crate::MobRuntimeMode::TurnDriven),
                        IdentityPreparationMode::Constructed,
                    ),
                    ALLOW,
                ),
            )
            .await
            .expect("the create answers"),
        );
    }

    async fn construct(
        &self,
        operation_id: &str,
        name: &str,
    ) -> Result<crate::runtime::HeldPreparation, MobError> {
        tokio::time::timeout(
            STEP,
            self.handle
                .construct_identity(crate::runtime::IdentityConstructionRequest::new(
                    operation(operation_id),
                    name,
                    tokio::time::Instant::now() + STEP,
                )),
        )
        .await
        .expect("the construction answers")
    }
}

/// Row (RED): a settle activates exactly the held attempt. The host's held
/// construction holds the identity's member unpublished; a settle naming
/// another attempt is refused `stale_attempt` with the current one; the
/// settle naming it publishes that very construction, a store-owned
/// member; and a later settle finds no preparation pending.
///
/// RED on the old tree: no held construction exists.
#[tokio::test]
async fn the_settle_activates_exactly_the_held_attempt_and_a_stale_attempt_is_refused() {
    let mob = HeldMob::new("construct-settle").await;
    mob.create_constructed("built").await;
    let held = mob
        .construct("construct-built", "built")
        .await
        .expect("the construction is held");
    assert_eq!(held.identity(), &id("built"));
    assert_eq!(
        mob.standing("built"),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::PreparationPending
        )),
        "a held construction is not published"
    );
    let stale = held.attempt() + 1;
    assert_eq!(
        refusal(settle(&mob.handle, "settle-stale", "built", Some(stale)).await),
        IdentityChangeRefusal::StaleAttempt {
            identity: id("built"),
            current: Some(held.attempt()),
        }
    );
    let settled = settle(&mob.handle, "settle-built", "built", Some(held.attempt()))
        .await
        .expect("the settle activates the held construction");
    assert!(
        matches!(
            settled.settlement,
            Some(crate::runtime::PreparationSettlement::Activated { attempt })
                if attempt == held.attempt()
        ),
        "{:?}",
        settled.settlement
    );
    assert!(matches!(
        mob.standing("built"),
        Some(EntryStanding::Seated(_))
    ));
    assert_eq!(mob.origin("built"), Some(IdentityEntryOrigin::Created));
    assert_eq!(
        refusal(settle(&mob.handle, "settle-again", "built", Some(held.attempt())).await),
        IdentityChangeRefusal::PreparationNotPending {
            identity: id("built")
        }
    );
}

/// Row (RED): an accepted activation that a crash left without its
/// publication ends `publication_interrupted` and keeps the identity
/// pending. The settle's receipt journals the acceptance (written here as
/// the settle's step writes it, and the process stops before the
/// publication); after the restart, a retry of the settle replays its
/// receipt and answers the interrupted publication, the identity is still
/// pending, and a new held attempt can be constructed and activated.
///
/// RED on the old tree: no settle exists.
#[tokio::test]
async fn a_crash_between_acceptance_and_commit_ends_the_attempt_publication_interrupted_and_keeps_the_entry_pending()
 {
    let mut mob = HeldMob::new("construct-crash").await;
    mob.create_constructed("built").await;
    let mob_id = mob.handle.mob_id().clone();
    let receipt = crate::IdentityChangeReceipt::sealed(
        mob_id,
        "settle-built",
        digest("settle-built"),
        IdentityChangeKind::SettlePreparation,
        vec![crate::identity::IdentityChangeReceiptEntry::new(
            id("built"),
            IdentityChangeEntryOutcome::Activating,
        )],
        1,
    )
    .expect("the acceptance's receipt");
    let store = mob
        .storage
        .identity_member
        .clone()
        .expect("the built-in identity store");
    let written = crate::store::MobIdentityMemberStore::commit_identity_change(
        store.as_ref(),
        &crate::store::IdentityChangeWrite {
            receipt,
            rows: Vec::new(),
            events: Vec::new(),
        },
    )
    .await
    .expect("the acceptance's transaction");
    assert!(matches!(
        written,
        crate::store::IdentityChangeCommit::Committed { .. }
    ));

    mob.restart().await;
    let replayed = settle(&mob.handle, "settle-built", "built", Some(1))
        .await
        .expect("the retry answers");
    assert!(matches!(
        replayed.change,
        IdentityChangeOutcome::Replayed { .. }
    ));
    assert!(
        matches!(
            replayed.settlement,
            Some(crate::runtime::PreparationSettlement::PublicationEnded {
                end: crate::runtime::ConstructionEnd::PublicationInterrupted,
                ..
            })
        ),
        "{:?}",
        replayed.settlement
    );
    assert_eq!(
        mob.standing("built"),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::PreparationPending
        )),
        "the identity stays pending"
    );
    let held = mob
        .construct("construct-again", "built")
        .await
        .expect("a new attempt is held");
    settle(&mob.handle, "settle-again", "built", Some(held.attempt()))
        .await
        .expect("the new attempt activates");
    assert!(matches!(
        mob.standing("built"),
        Some(EntryStanding::Seated(_))
    ));
}

/// Row (RED): a removal of an identity whose construction is held aborts
/// the construction and tombstones the name: no member is published, the
/// name reads removed, and the held attempt settles nothing.
///
/// RED on the old tree: no held construction exists.
#[tokio::test]
async fn a_removal_during_a_held_construction_aborts_it_and_tombstones() {
    let mob = HeldMob::new("construct-remove").await;
    mob.create_constructed("built").await;
    let held = mob
        .construct("construct-built", "built")
        .await
        .expect("the construction is held");
    let removed = tokio::time::timeout(
        STEP,
        mob.handle.remove_identity(
            crate::runtime::IdentityRemoval::new(
                operation("remove-built"),
                IdentityChangeRequesterInput::host("console"),
                "built",
            ),
            ALLOW,
        ),
    )
    .await
    .expect("the removal answers");
    committed(removed);
    let deadline = tokio::time::Instant::now() + STEP;
    while !matches!(
        mob.handle
            .identity_entry(&id("built"))
            .expect("the entry read answers"),
        IdentityEntryRead::Removed
    ) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the identity never read removed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        mob.handle
            .get_member(&id("built"))
            .await
            .expect("roster read")
            .is_none(),
        "the held construction was never published"
    );
    assert!(matches!(
        settle(&mob.handle, "settle-built", "built", Some(held.attempt())).await,
        Err(MobError::IdentityChangeRefused { .. })
    ));
}

/// Row (RED): an activation accepted before the attempt's deadline publishes
/// after it, and nothing replaces it meanwhile. The settle's acceptance
/// holds while the publication is parked past the deadline and the host's
/// observer is dropped: the identity is not published before its commit,
/// an abort is refused `activation_pending`, a new attempt
/// `construction_open`, and the deadline ends nothing. Released, the
/// publication seats exactly that construction, and the settle answers it
/// activated.
///
/// RED on the old tree: no held construction exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_activation_accepted_before_the_deadline_publishes_after_it_and_nothing_replaces_it() {
    let mob = HeldMob::new("construct-deadline").await;
    // The publication hold is keyed by name across the test process.
    let name = unique("parked");
    mob.create_constructed(&name).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let held = tokio::time::timeout(
        STEP,
        mob.handle
            .construct_identity(crate::runtime::IdentityConstructionRequest::new(
                operation("construct-parked"),
                name.as_str(),
                deadline,
            )),
    )
    .await
    .expect("the construction answers")
    .expect("the construction is held");
    let attempt = held.attempt();
    let (entered, release) = crate::runtime::held_spawn::arm_held_publication_for_test(&name);
    let handle = mob.handle.clone();
    let settled_name = name.clone();
    let settling = tokio::spawn(async move {
        settle(&handle, "settle-parked", &settled_name, Some(attempt)).await
    });
    tokio::time::timeout(STEP, entered)
        .await
        .expect("the accepted publication parks")
        .expect("the publication hold");
    drop(held);
    tokio::time::sleep_until(deadline + Duration::from_millis(200)).await;
    assert_eq!(
        mob.standing(&name),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::PreparationPending
        )),
        "nothing is published before its commit"
    );
    assert_eq!(
        refusal(
            mob.handle
                .abort_identity_construction(&id(&name), attempt)
                .await
        ),
        IdentityChangeRefusal::ActivationPending {
            identity: id(&name),
            attempt,
        }
    );
    assert_eq!(
        refusal(mob.construct("construct-parked-again", &name).await),
        IdentityChangeRefusal::ConstructionOpen {
            identity: id(&name),
            attempt,
        }
    );
    let _ = release.send(());
    let settled = tokio::time::timeout(STEP, settling)
        .await
        .expect("the settle answers")
        .expect("the settle task")
        .expect("the settle commits");
    assert!(
        matches!(
            settled.settlement,
            Some(crate::runtime::PreparationSettlement::Activated { attempt: activated })
                if activated == attempt
        ),
        "{:?}",
        settled.settlement
    );
    assert!(matches!(
        mob.standing(&name),
        Some(EntryStanding::Seated(_))
    ));
    assert_eq!(mob.origin(&name), Some(IdentityEntryOrigin::Created));
}

// ---------------------------------------------------------------------------
// R2.12: the reads
// ---------------------------------------------------------------------------

/// Row (RED): the entry read shows a store-owned identity's revision, its
/// provenance and its archive without waking it. After an update and an
/// archive, the entry reads unseated, archived, on the session its member
/// was bound to; its record reads the revision the update committed, the
/// provenance its create wrote, and its archived presence; a name with no
/// row reads none; and the reads build no member.
///
/// RED on the old tree: the entry view names no archived session and the
/// handle reads no record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_entry_read_shows_revision_provenance_and_archive_without_waking() {
    let mob = StoreOwnedMob::new("read-record").await;
    committed(
        mob.create(host_create("create-read", &["read"]), ALLOW)
            .await,
    );
    let session = mob.session("read").await;
    mob.update(
        "update-read",
        spec("read", "worker").with_display_name("Renamed"),
        &spec_digest(&spec("read", "worker")),
    )
    .await
    .expect("the update commits");
    committed(mob.archive("archive-read", "read").await);
    mob.unseated_for("read", UnseatedEntryCause::Archived).await;
    let builds = mob.builds().await;
    match mob.read("read") {
        IdentityEntryRead::Entry(view) => {
            assert_eq!(
                view.standing,
                EntryStanding::Unseated(UnseatedEntryCause::Archived)
            );
            assert_eq!(view.archived_session, Some(session.clone()));
        }
        other => panic!("read reads {other:?}"),
    }
    let record = match mob
        .handle
        .identity_record(&id("read"))
        .await
        .expect("the record read answers")
    {
        Some(CreatedIdentityObservation::Created(record)) => record,
        other => panic!("read has no readable record: {other:?}"),
    };
    assert_eq!(record.revision, 2);
    assert_eq!(record.provenance.operation_id, "create-read");
    assert!(matches!(
        record.provenance.entered,
        crate::identity::IdentityEntered::Created
    ));
    assert_eq!(
        record.presence,
        CreatedIdentityPresence::Archived {
            archived_session: Some(session)
        }
    );
    assert!(
        mob.handle
            .identity_record(&id("no-such-name"))
            .await
            .expect("the record read answers")
            .is_none(),
        "a name with no row reads none"
    );
    assert_eq!(mob.builds().await, builds, "the reads build no member");
}

/// Row (RED): the tombstone page lists the removed names of either custody,
/// in identity order, page by page, and no name that was not removed.
///
/// RED on the old tree: no tombstone page exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tombstone_page_lists_removed_names_of_either_custody() {
    let mob = StoreOwnedMob::new("tombstone-page").await;
    mob.declare(&["listed"]).await;
    committed(
        mob.create(host_create("create-made", &["made", "kept"]), ALLOW)
            .await,
    );
    committed(mob.remove("remove-listed", "listed").await);
    committed(mob.remove("remove-made", "made").await);
    mob.removed("listed").await;
    mob.removed("made").await;
    let first = mob
        .handle
        .identity_tombstones(None, 1)
        .expect("the page answers");
    assert_eq!(first.removed, vec![id("listed")]);
    assert_eq!(first.next, Some(id("listed")));
    let second = mob
        .handle
        .identity_tombstones(first.next.as_ref(), 1)
        .expect("the page answers");
    assert_eq!(second.removed, vec![id("made")]);
    assert_eq!(second.next, None);
    let all = mob
        .handle
        .identity_tombstones(None, crate::runtime::IDENTITY_ENTRY_PAGE_MAX)
        .expect("the page answers");
    assert_eq!(all.removed, vec![id("listed"), id("made")]);
}

/// Row (RED): the record census counts a seat whose labels its row moved
/// past. A label change takes the row at once while the seated member's
/// seat row keeps the old labels until its restamp, so the identity counts
/// in `seats_stale`; the census also counts the mob's rows (present,
/// pending, removed), its tombstones and its open preparations.
///
/// RED on the old tree: the census counts nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_census_counts_a_stale_seat_after_a_label_change() {
    let mob = StoreOwnedMob::new("census-stale").await;
    committed(
        mob.create(host_create("create-desks", &["desk", "gone"]), ALLOW)
            .await,
    );
    committed(
        mob.create(
            prepared_create(
                "create-pending",
                spec("pending", "worker"),
                IdentityPreparationMode::Required,
            ),
            ALLOW,
        )
        .await,
    );
    let before = mob
        .handle
        .identity_record_census()
        .await
        .expect("the census answers");
    assert_eq!(before.seats_stale, 0, "{before:?}");
    mob.update(
        "relabel-desk",
        spec("desk", "worker")
            .with_labels(BTreeMap::from([("team".to_string(), "beta".to_string())])),
        &spec_digest(&spec("desk", "worker")),
    )
    .await
    .expect("the update commits");
    committed(mob.remove("remove-gone", "gone").await);
    mob.removed("gone").await;
    let census = mob
        .handle
        .identity_record_census()
        .await
        .expect("the census answers");
    assert_eq!(census.seats_stale, 1, "{census:?}");
    assert_eq!(census.created_rows, 3, "{census:?}");
    assert_eq!(census.tombstones, 1, "{census:?}");
    assert_eq!(census.preparations_open, 1, "{census:?}");
    assert_eq!(census.archived, 0, "{census:?}");
    assert_eq!(census.constructions_held, 0, "{census:?}");
}
