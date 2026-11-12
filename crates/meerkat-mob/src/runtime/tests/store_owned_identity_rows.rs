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

// Adoption into store custody and owned edges (0.9.11): the rows of an
// adoption, and of an identity change's edges.
mod adoption;
// The downgrade cycle (0.9.11): export, purge, the reservation of a purged
// row's name, and import.
mod cycle;
mod owned_edges;

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

/// Whether `refused` is an `invalid_request` whose detail names `reason`.
fn invalid_request_naming(refused: &IdentityChangeRefusal, reason: &str) -> bool {
    matches!(
        refused,
        IdentityChangeRefusal::InvalidRequest { problem } if problem.contains(reason)
    )
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
        self.restart_configured(authority, |builder| builder).await;
    }

    /// [`Self::restart_as`], with the resumed mob's builder configured by
    /// `configure` (a host registers its grant mint again).
    async fn restart_configured(
        &mut self,
        authority: MobCompositionAuthority,
        configure: impl FnOnce(MobBuilder) -> MobBuilder,
    ) {
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(self.handle.clone()).await;
        self.service = restarted;
        self.handle = tokio::time::timeout(
            STEP,
            Box::pin(
                configure(
                    MobBuilder::for_resume(self.storage.clone())
                        .with_session_service(self.service.clone())
                        .with_member_comms_identity_root(self.keys.path())
                        .with_composition_authority(authority)
                        .notify_orchestrator_on_resume(false),
                )
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
        self.create_outcome(create, consent)
            .await
            .map(|created| created.change)
    }

    /// [`Self::create`], answering the whole outcome: the change and each
    /// post-commit kickoff spawn.
    async fn create_outcome(
        &self,
        create: StoreOwnedIdentityCreate,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<crate::runtime::IdentityCreateOutcome, MobError> {
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

    /// Admit `requester_session`'s change `operation_id` asking `request`
    /// of its grant.
    async fn admit_for(
        &self,
        requester_session: &SessionId,
        operation_id: &str,
        request: &IdentityGrantRequest,
    ) -> crate::runtime::IdentityChangeGrant {
        self.handle
            .admit_identity_change_grant(requester_session, &operation(operation_id), request)
            .await
            .unwrap_or_else(|error| panic!("the grant admits {operation_id}: {error:?}"))
    }

    /// The executor's re-admission of `requester`'s change `operation_id`
    /// asking `request`, rebuilt from the requester identity.
    async fn grant_for(
        &self,
        requester: &str,
        operation_id: &str,
        request: &IdentityGrantRequest,
    ) -> Result<crate::runtime::IdentityChangeGrant, MobError> {
        tokio::time::timeout(
            STEP,
            self.handle.identity_change_grant_for(
                &id(requester),
                &operation(operation_id),
                request,
            ),
        )
        .await
        .expect("the re-admission answers")
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

/// The number of turns `mob`'s session service started on `session` with
/// the prompt `prompt`.
async fn turns_with_prompt(mob: &StoreOwnedMob, session: &SessionId, prompt: &str) -> usize {
    mob.service
        .recorded_start_turn_prompts()
        .await
        .into_iter()
        .filter(|(ran_on, ran)| ran_on == session && ran == prompt)
        .count()
}

/// Wait, within one step, until `name` is a member whose session ran a turn
/// with `prompt`: its kickoff. Answers its session.
async fn kicked_off(mob: &StoreOwnedMob, name: &str, prompt: &str) -> SessionId {
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        if let Some(session) = mob.handle.resolve_bridge_session_id(&id(name)).await
            && turns_with_prompt(mob, &session, prompt).await > 0
        {
            return session;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{name}'s kickoff never ran"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Row (RED): a create whose spec owes a kickoff (an initial message)
/// answers its spawn per identity: spawned once after the commit, it is
/// answered embodied on its session, and its kickoff runs once. A spawn
/// that fails is answered failed, the identity owed its seat with no
/// member, and the mob's next start embodies it once, its kickoff then
/// running once.
///
/// RED on the old tree: the create answers no spawn (a failed one is only
/// logged).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_with_an_initial_message_answers_its_spawn_and_a_failed_spawn_is_owed_its_seat() {
    let mut mob = StoreOwnedMob::new("create-kickoff").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let kicked = |name: &str, prompt: &str| {
        spec(name, "worker")
            .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
            .with_initial_message(meerkat_core::types::ContentInput::from(prompt))
    };
    let create = |operation_id: &str, identity_spec: CreatedIdentitySpec| {
        StoreOwnedIdentityCreate::new(
            operation(operation_id),
            IdentityChangeRequesterInput::host("console"),
            vec![CreatedIdentityRequest::new(identity_spec)],
        )
    };

    let created = mob
        .create_outcome(create("create-greeter", kicked("greeter", "Begin.")), ALLOW)
        .await
        .expect("the create commits");
    assert!(matches!(
        created.change,
        IdentityChangeOutcome::Committed { .. }
    ));
    let session = match created.embodied.get(&id("greeter")) {
        Some(crate::runtime::DeclaredEntryOutcome::Embodied { session_id, .. }) => {
            session_id.clone()
        }
        other => panic!("the greeter's spawn is answered embodied: {other:?}"),
    };
    assert_eq!(created.embodied.len(), 1);
    assert_eq!(kicked_off(&mob, "greeter", "Begin.").await, session);
    assert_eq!(
        turns_with_prompt(&mob, &session, "Begin.").await,
        1,
        "the kickoff runs once"
    );

    super::super::identity_change::fail_next_kickoff_spawn_for_test(&mob.mob_id(), &id("waiter"));
    let created = mob
        .create_outcome(create("create-waiter", kicked("waiter", "Wait.")), ALLOW)
        .await
        .expect("the create commits");
    assert!(matches!(
        created.change,
        IdentityChangeOutcome::Committed { .. }
    ));
    assert!(
        matches!(
            created.embodied.get(&id("waiter")),
            Some(crate::runtime::DeclaredEntryOutcome::Failed { .. })
        ),
        "{:?}",
        created.embodied
    );
    mob.unseated_for("waiter", UnseatedEntryCause::SeatOwed)
        .await;
    assert!(!mob.is_member("waiter").await);

    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let session = kicked_off(&mob, "waiter", "Wait.").await;
    assert_eq!(
        turns_with_prompt(&mob, &session, "Wait.").await,
        1,
        "the next start embodies it once"
    );
}

/// A host build preparer that records each build it is asked about and
/// answers as the row scripts it, and each store-owned seat it is asked to
/// compose, which it composes as the row scripts it.
struct ScriptedPreparer {
    asked: std::sync::Mutex<Vec<(AgentIdentity, SessionId)>>,
    answer: std::sync::Mutex<
        Result<crate::runtime::MemberBuildPrepared, crate::runtime::MemberBuildPreparationRefusal>,
    >,
    seats: std::sync::Mutex<Vec<AgentIdentity>>,
    seat: std::sync::Mutex<SeatScript>,
}

/// What a scripted host does with a store-owned seat it is asked to compose.
#[derive(Clone)]
enum SeatScript {
    /// Leave the seat as Core lowered it.
    Keep,
    /// Set the host's build inputs on it.
    Compose(HostBuild),
    /// Refuse to compose it.
    Refuse(crate::runtime::MemberBuildPreparationRefusal),
    /// Change a fact of the seat Core refuses a change of.
    Change(SeatChange),
    /// Answer other labels than the seat's.
    Labels(LabelChange),
    /// Answer each listed identity's seat with the labels listed for it, as
    /// a host that reads the labels from the identity's row does.
    RowLabels(BTreeMap<AgentIdentity, BTreeMap<String, String>>),
}

/// How a scripted host's answer changes a store-owned seat's labels.
#[derive(Clone, Copy, Debug)]
enum LabelChange {
    /// Add a label the seat does not carry.
    Add,
    /// Drop a label the seat carries.
    Drop,
    /// Give a label the seat carries another value.
    Revalue,
    /// Set the seat's own labels again, key by key.
    Same,
}

impl LabelChange {
    /// The key of the seat's labels the change touches, and the value the
    /// scripted host uses for it: Core's refusal names the key, never the
    /// value.
    const ADDED: (&'static str, &'static str) = ("host_tier", "host-secret-gold");
    const REVALUED: (&'static str, &'static str) = ("team", "host-secret-omega");
    const DROPPED: &'static str = "desk";

    fn apply(self, seat: &mut SpawnMemberSpec) {
        let labels = seat.labels.get_or_insert_with(BTreeMap::new);
        match self {
            Self::Add => {
                labels.insert(Self::ADDED.0.to_string(), Self::ADDED.1.to_string());
            }
            Self::Drop => {
                labels.remove(Self::DROPPED);
            }
            Self::Revalue => {
                labels.insert(Self::REVALUED.0.to_string(), Self::REVALUED.1.to_string());
            }
            Self::Same => {
                let same = labels.clone();
                labels.clear();
                labels.extend(same);
            }
        }
    }

    /// How Core's refusal of the change names it: what the answer did, and
    /// the key.
    fn named(self) -> String {
        match self {
            Self::Add => format!("added [{}]", Self::ADDED.0),
            Self::Drop => format!("dropped [{}]", Self::DROPPED),
            Self::Revalue => format!("changed [{}]", Self::REVALUED.0),
            Self::Same => String::new(),
        }
    }
}

/// A fact of a store-owned seat its host may not change.
#[derive(Clone, Copy, Debug)]
enum SeatChange {
    Identity,
    Role,
    LaunchMode,
    Custody,
}

impl SeatChange {
    /// What Core's refusal of the change names.
    fn named(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Role => "role",
            Self::LaunchMode => "launch mode",
            Self::Custody => "custody",
        }
    }
}

/// The build inputs a scripted host sets on a store-owned seat: a model
/// override, a system prompt, a context and an instruction section.
#[derive(Clone, Debug)]
struct HostBuild {
    model: &'static str,
    prompt: &'static str,
    context: serde_json::Value,
    instruction: &'static str,
}

impl HostBuild {
    fn first() -> Self {
        Self {
            model: "claude-opus-4-8",
            prompt: "You keep the first desk.",
            context: serde_json::json!({"host": "first"}),
            instruction: "Answer for the first desk.",
        }
    }

    fn second() -> Self {
        Self {
            model: "claude-haiku-5-5",
            prompt: "You keep the second desk.",
            context: serde_json::json!({"host": "second"}),
            instruction: "Answer for the second desk.",
        }
    }

    fn apply(&self, seat: &mut SpawnMemberSpec) {
        seat.model_override = Some(self.model.to_string());
        seat.system_prompt_override = Some(crate::SpawnSystemPromptOverride::Replace(
            self.prompt.to_string(),
        ));
        seat.context = Some(self.context.clone());
        seat.additional_instructions
            .get_or_insert_with(Vec::new)
            .push(self.instruction.to_string());
    }

    /// Whether `build` took the model this host sets.
    fn gave_model(&self, build: &CreateSessionRecord) -> bool {
        build.model == self.model
    }

    /// Whether `build` took the system prompt, the context and the
    /// instruction section this host sets.
    fn gave_instructions(&self, build: &CreateSessionRecord) -> bool {
        matches!(
            &build.system_prompt,
            meerkat_core::SystemPromptOverride::Set(prompt) if prompt == self.prompt
        ) && build.app_context.as_ref() == Some(&self.context)
            && build
                .additional_instructions
                .as_ref()
                .is_some_and(|sections| sections.iter().any(|section| section == self.instruction))
    }

    /// Whether `build` took none of the system prompt, the context and the
    /// instruction section this host sets.
    fn gave_no_instructions(&self, build: &CreateSessionRecord) -> bool {
        !matches!(
            &build.system_prompt,
            meerkat_core::SystemPromptOverride::Set(prompt) if prompt == self.prompt
        ) && build.app_context.as_ref() != Some(&self.context)
            && !build
                .additional_instructions
                .as_ref()
                .is_some_and(|sections| sections.iter().any(|section| section == self.instruction))
    }
}

impl ScriptedPreparer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            asked: std::sync::Mutex::new(Vec::new()),
            answer: std::sync::Mutex::new(Ok(crate::runtime::MemberBuildPrepared::Prepared)),
            seats: std::sync::Mutex::new(Vec::new()),
            seat: std::sync::Mutex::new(SeatScript::Keep),
        })
    }

    fn answer(
        &self,
        answer: Result<
            crate::runtime::MemberBuildPrepared,
            crate::runtime::MemberBuildPreparationRefusal,
        >,
    ) {
        *self.answer.lock().expect("the scripted answer") = answer;
    }

    fn asked(&self) -> Vec<(AgentIdentity, SessionId)> {
        self.asked.lock().expect("the asked builds").clone()
    }

    /// Script what the host does with each seat it is asked to compose from
    /// now on.
    fn compose_seats(&self, script: SeatScript) {
        *self.seat.lock().expect("the seat script") = script;
    }

    /// The identities whose seat the host was asked to compose, in order.
    fn seats(&self) -> Vec<AgentIdentity> {
        self.seats.lock().expect("the composed seats").clone()
    }
}

#[async_trait::async_trait]
impl crate::runtime::MemberBuildPreparer for ScriptedPreparer {
    async fn prepare_member_build(
        &self,
        request: crate::runtime::MemberBuildPreparation,
    ) -> Result<crate::runtime::MemberBuildPrepared, crate::runtime::MemberBuildPreparationRefusal>
    {
        self.asked
            .lock()
            .expect("the asked builds")
            .push((request.agent_identity.clone(), request.session_id.clone()));
        self.answer.lock().expect("the scripted answer").clone()
    }

    async fn prepare_store_owned_seat(
        &self,
        _handle: crate::runtime::MobReadHandle,
        seat: &mut SpawnMemberSpec,
    ) -> Result<(), crate::runtime::MemberBuildPreparationRefusal> {
        self.seats
            .lock()
            .expect("the composed seats")
            .push(seat.identity.clone());
        let script = self.seat.lock().expect("the seat script").clone();
        match script {
            SeatScript::Keep => {}
            SeatScript::Compose(build) => build.apply(seat),
            SeatScript::Refuse(refusal) => return Err(refusal),
            SeatScript::Change(SeatChange::Identity) => seat.identity = id("impostor"),
            SeatScript::Change(SeatChange::Role) => seat.role_name = ProfileName::from("lead"),
            SeatScript::Change(SeatChange::LaunchMode) => {
                seat.launch_mode = crate::launch::MemberLaunchMode::Resume {
                    bridge_session_id: SessionId::new(),
                    resume_from_role: None,
                };
            }
            SeatScript::Change(SeatChange::Custody) => {
                *seat = seat.clone().with_created_custody(false);
            }
            SeatScript::Labels(change) => change.apply(seat),
            SeatScript::RowLabels(rows) => {
                if let Some(labels) = rows.get(&seat.identity) {
                    seat.labels = Some(labels.clone());
                }
            }
        }
        Ok(())
    }
}

/// Row (RED): the spawn of a store-owned identity that owes a kickoff is a
/// build the host prepares, as a wake's is: Core asks the host's build
/// preparer once, off the actor, before the member's provision, naming the
/// session the member is built on. A refusal is a failed kickoff spawn: the
/// create stays committed, the identity is owed its seat with no member, and
/// nothing is built. A hold the host asks for holds the member's run starts
/// until the host releases them, and its kickoff then runs once.
///
/// RED on the old tree: the preparer is never asked about the spawn, so a
/// member the host has prepared nothing for runs its kickoff.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_creates_kickoff_spawn_asks_the_hosts_build_preparer_before_its_build() {
    use crate::runtime::{MemberBuildPreparationRefusal, MemberBuildPrepared};
    let preparer = ScriptedPreparer::new();
    let mob = StoreOwnedMob::build(
        store_owned_definition("kickoff-prepared"),
        MobStorage::in_memory(),
        {
            let preparer = preparer.clone();
            move |builder| builder.with_member_build_preparer(preparer)
        },
    )
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let kicked = |name: &str, prompt: &str| {
        spec(name, "worker")
            .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
            .with_initial_message(meerkat_core::types::ContentInput::from(prompt))
    };
    let create = |operation_id: &str, identity_spec: CreatedIdentitySpec| {
        StoreOwnedIdentityCreate::new(
            operation(operation_id),
            IdentityChangeRequesterInput::host("console"),
            vec![CreatedIdentityRequest::new(identity_spec)],
        )
    };
    let embodied_on = |created: &crate::runtime::IdentityCreateOutcome, name: &str| match created
        .embodied
        .get(&id(name))
    {
        Some(crate::runtime::DeclaredEntryOutcome::Embodied { session_id, .. }) => {
            session_id.clone()
        }
        other => panic!("{name}'s spawn is answered embodied: {other:?}"),
    };

    // Prepared: asked once, about the session the member is built on.
    let created = mob
        .create_outcome(create("create-greeter", kicked("greeter", "Begin.")), ALLOW)
        .await
        .expect("the create commits");
    let session = embodied_on(&created, "greeter");
    assert_eq!(
        preparer.asked(),
        vec![(id("greeter"), session.clone())],
        "the host is asked once about the kickoff spawn's build, on its session"
    );
    assert_eq!(kicked_off(&mob, "greeter", "Begin.").await, session);
    assert_eq!(turns_with_prompt(&mob, &session, "Begin.").await, 1);

    // A refusal, for now or for good, is a failed kickoff spawn.
    for (name, refusal) in [
        (
            "waiter",
            MemberBuildPreparationRefusal::Retryable {
                reason: "the host is not ready for this member yet".to_string(),
            },
        ),
        (
            "refused",
            MemberBuildPreparationRefusal::Final {
                reason: "the host refuses to build this member".to_string(),
            },
        ),
    ] {
        preparer.answer(Err(refusal.clone()));
        let created = mob
            .create_outcome(
                create(&format!("create-{name}"), kicked(name, "Wait.")),
                ALLOW,
            )
            .await
            .expect("the create commits");
        assert!(matches!(
            created.change,
            IdentityChangeOutcome::Committed { .. }
        ));
        match created.embodied.get(&id(name)) {
            Some(crate::runtime::DeclaredEntryOutcome::Failed { error }) => assert!(
                error.to_string().contains(refusal.reason()),
                "{name}'s failed spawn names the host's reason: {error}"
            ),
            other => panic!("{name}'s spawn is answered failed: {other:?}"),
        }
        mob.unseated_for(name, UnseatedEntryCause::SeatOwed).await;
        assert!(!mob.is_member(name).await, "{name} has no member");
        assert!(
            preparer.asked().iter().any(|(asked, _)| asked == &id(name)),
            "the host was asked about {name}"
        );
    }
    assert!(
        mob.service
            .recorded_start_turn_prompts()
            .await
            .into_iter()
            .all(|(_, prompt)| prompt != "Wait."),
        "a refused build runs nothing"
    );

    // A hold: built, and no run starts until the host releases it.
    preparer.answer(Ok(MemberBuildPrepared::HoldUntilReleased));
    let created = mob
        .create_outcome(create("create-holder", kicked("holder", "Hold.")), ALLOW)
        .await
        .expect("the create commits");
    let session = embodied_on(&created, "holder");
    assert_eq!(
        mob.handle
            .members_holding_run_starts(crate::HostRunStartHoldReason::ToolsNotPublished)
            .expect("the held members read"),
        vec![id("holder")],
        "Core holds the member's run starts for its unpublished tools"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        turns_with_prompt(&mob, &session, "Hold.").await,
        0,
        "a held member starts no run"
    );
    mob.handle
        .release_member_run_starts(
            &id("holder"),
            crate::HostRunStartHoldReason::ToolsNotPublished,
        )
        .await
        .expect("the host releases the hold");
    assert_eq!(kicked_off(&mob, "holder", "Hold.").await, session);
    assert_eq!(
        turns_with_prompt(&mob, &session, "Hold.").await,
        1,
        "the released member runs its kickoff once"
    );
}

/// Row: a kickoff spawn the host refused to prepare is owed again at the
/// mob's next start. The start asks the host's build preparer again about
/// that identity, and once the host prepares, the identity is embodied and
/// its kickoff runs once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_asks_the_host_again_about_a_kickoff_spawn_it_refused_to_prepare() {
    use crate::runtime::{MemberBuildPreparationRefusal, MemberBuildPrepared};
    let preparer = ScriptedPreparer::new();
    let mut mob = StoreOwnedMob::build(
        store_owned_definition("kickoff-refused-then-prepared"),
        MobStorage::in_memory(),
        {
            let preparer = preparer.clone();
            move |builder| builder.with_member_build_preparer(preparer)
        },
    )
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    preparer.answer(Err(MemberBuildPreparationRefusal::Retryable {
        reason: "the host is not ready for this member yet".to_string(),
    }));
    let created = mob
        .create_outcome(
            StoreOwnedIdentityCreate::new(
                operation("create-waiter"),
                IdentityChangeRequesterInput::host("console"),
                vec![CreatedIdentityRequest::new(
                    spec("waiter", "worker")
                        .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
                        .with_initial_message(meerkat_core::types::ContentInput::from("Wait.")),
                )],
            ),
            ALLOW,
        )
        .await
        .expect("the create commits");
    assert!(matches!(
        created.change,
        IdentityChangeOutcome::Committed { .. }
    ));
    assert!(
        matches!(
            created.embodied.get(&id("waiter")),
            Some(crate::runtime::DeclaredEntryOutcome::Failed { .. })
        ),
        "{:?}",
        created.embodied
    );
    mob.unseated_for("waiter", UnseatedEntryCause::SeatOwed)
        .await;
    let asked_at_the_create = preparer.asked().len();
    assert_eq!(
        asked_at_the_create, 1,
        "the host was asked once at the create"
    );

    preparer.answer(Ok(MemberBuildPrepared::Prepared));
    mob.restart_configured(MobCompositionAuthority::Authoritative, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let session = kicked_off(&mob, "waiter", "Wait.").await;
    assert_eq!(
        turns_with_prompt(&mob, &session, "Wait.").await,
        1,
        "the next start embodies it once it is prepared"
    );
    let asked = preparer.asked();
    assert!(
        asked.len() > asked_at_the_create && asked.last() == Some(&(id("waiter"), session)),
        "the start asked the host again about the identity's spawn: {asked:?}"
    );
}

/// A store-owned mob whose host build preparer is `preparer`.
async fn seat_composing_mob(label: &str, preparer: &Arc<ScriptedPreparer>) -> StoreOwnedMob {
    let mob = StoreOwnedMob::build(store_owned_definition(label), MobStorage::in_memory(), {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    mob
}

/// The create of `name` on the worker profile with the first message
/// `prompt`: its spec owes a kickoff, so its create spawns it.
fn kickoff_create(name: &str, prompt: &str) -> StoreOwnedIdentityCreate {
    StoreOwnedIdentityCreate::new(
        operation(&format!("create-{name}")),
        IdentityChangeRequesterInput::host("console"),
        vec![CreatedIdentityRequest::new(
            spec(name, "worker")
                .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
                .with_initial_message(meerkat_core::types::ContentInput::from(prompt)),
        )],
    )
}

/// The builds `mob`'s session service recorded on `session`, in order.
async fn builds_on(mob: &StoreOwnedMob, session: &SessionId) -> Vec<CreateSessionRecord> {
    mob.service
        .recorded_create_requests()
        .await
        .into_iter()
        .filter(|record| &record.session_id == session)
        .collect()
}

/// Wake `name`, bounded.
async fn wake(mob: &StoreOwnedMob, name: &str) -> Result<(), MobError> {
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id(name)))
        .await
        .expect("the wake answers")
}

/// Row (RED): the host composes the seat of a store-owned identity before
/// its kickoff spawn. A refusal fails that spawn before it is sent: the
/// create stays committed, the identity is owed its seat with no member,
/// nothing is built, and the host's build preparation is not asked. The
/// mob's next start asks the host again, and the seat is built with what
/// the host composed: its model override, its system prompt, its context
/// and its instruction section. The member's roster labels stay its row's.
///
/// RED on the old tree: the host is never asked to compose the seat, so the
/// kickoff spawn of a seat the host refuses is built from the row alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_seat_composition_at_a_create_is_owed_and_the_next_start_composes_the_seat() {
    use crate::runtime::MemberBuildPreparationRefusal;
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("seat-refused-at-create", &preparer).await;
    let refusal = MemberBuildPreparationRefusal::Retryable {
        reason: "the host cannot compose this seat yet".to_string(),
    };
    preparer.compose_seats(SeatScript::Refuse(refusal.clone()));
    let created = mob
        .create_outcome(kickoff_create("waiter", "Wait."), ALLOW)
        .await
        .expect("the create commits");
    assert!(matches!(
        created.change,
        IdentityChangeOutcome::Committed { .. }
    ));
    match created.embodied.get(&id("waiter")) {
        Some(crate::runtime::DeclaredEntryOutcome::Failed { error }) => assert!(
            error.to_string().contains(refusal.reason()),
            "the failed spawn names the host's reason: {error}"
        ),
        other => panic!("the spawn of a seat its host refused is answered failed: {other:?}"),
    }
    mob.unseated_for("waiter", UnseatedEntryCause::SeatOwed)
        .await;
    assert!(!mob.is_member("waiter").await, "the identity has no member");
    assert_eq!(mob.builds().await, 0, "nothing was built");
    assert_eq!(
        preparer.seats(),
        vec![id("waiter")],
        "the host was asked once to compose the seat"
    );
    assert!(
        preparer.asked().is_empty(),
        "a spawn that was not sent asks for no build preparation"
    );

    let host = HostBuild::first();
    preparer.compose_seats(SeatScript::Compose(host.clone()));
    mob.restart_configured(MobCompositionAuthority::Authoritative, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let session = kicked_off(&mob, "waiter", "Wait.").await;
    assert_eq!(
        preparer.seats(),
        vec![id("waiter"), id("waiter")],
        "the start asked the host again to compose the seat"
    );
    let builds = builds_on(&mob, &session).await;
    assert_eq!(builds.len(), 1, "one build of the seat's session");
    assert!(
        host.gave_model(&builds[0]) && host.gave_instructions(&builds[0]),
        "the seat is built with what the host composed: {:?}",
        builds[0]
    );
    let row = mob.spawn_row("waiter").await;
    assert_eq!(
        row.labels,
        spec("waiter", "worker").labels,
        "the member's roster labels stay its row's"
    );
    assert_eq!(
        row.effective_model_override.as_deref(),
        Some(host.model),
        "the spawn records the host's model override"
    );
}

/// Row (RED): the host composes a Dormant store-owned seat at its first
/// wake, not before. A refusal there leaves the member Dormant with its
/// session unborn, as a refused build preparation does, with nothing built
/// and the host's build preparation not asked; once the host retries it,
/// the next wake asks the host again and builds the member with what it
/// composed. A refusal for good breaks the member, and nothing is built.
///
/// RED on the old tree: the host is never asked to compose the seat, so the
/// first wake builds the member from its row alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_seat_composition_at_a_first_wake_leaves_the_member_dormant_and_is_asked_again() {
    use crate::runtime::MemberBuildPreparationRefusal;
    let preparer = ScriptedPreparer::new();
    let mob = seat_composing_mob("seat-refused-at-wake", &preparer).await;
    committed(
        mob.create(host_create("create-seats", &["quiet", "closed"]), ALLOW)
            .await,
    );
    assert!(
        preparer.seats().is_empty(),
        "a Dormant seat asks the host nothing until it is built"
    );
    let session = mob.session("quiet").await;
    let unborn = |name: &str| {
        crate::runtime::boot_plan::domain_member_seat_session_unborn(
            &mob.handle.machine_state_watch_rx.borrow(),
            &id(name),
        )
    };

    preparer.compose_seats(SeatScript::Refuse(
        MemberBuildPreparationRefusal::Retryable {
            reason: "the host cannot compose this seat yet".to_string(),
        },
    ));
    let refused = wake(&mob, "quiet").await;
    assert!(
        super::member_wake_rows::refused_for_build_preparation_retry(&refused),
        "the wake of a seat its host refused to compose is refused for the retry: {refused:?}"
    );
    assert!(mob.seated_dormant("quiet"), "the member stays Dormant");
    assert!(unborn("quiet"), "its session stays unborn");
    assert_eq!(mob.builds().await, 0, "nothing was built");
    assert_eq!(preparer.seats(), vec![id("quiet")]);
    assert!(
        preparer.asked().is_empty(),
        "a build its seat composition refused asks for no build preparation"
    );

    let host = HostBuild::first();
    preparer.compose_seats(SeatScript::Compose(host.clone()));
    mob.handle
        .resolve_member_build_preparation(&id("quiet"))
        .await
        .expect("the host retries the build preparation");
    wake(&mob, "quiet")
        .await
        .expect("the next wake builds the member");
    assert_eq!(
        preparer.seats(),
        vec![id("quiet"), id("quiet")],
        "the next wake asked the host again to compose the seat"
    );
    let builds = builds_on(&mob, &session).await;
    assert_eq!(builds.len(), 1, "one build of the seat's session");
    assert!(
        host.gave_model(&builds[0]) && host.gave_instructions(&builds[0]),
        "the member is built with what the host composed: {:?}",
        builds[0]
    );
    let entry = mob
        .handle
        .get_member(&id("quiet"))
        .await
        .expect("roster read")
        .expect("quiet is a member");
    assert_eq!(
        entry.labels,
        spec("quiet", "worker").labels,
        "the member's roster labels stay its row's"
    );
    assert_eq!(
        entry.effective_model_override.as_deref(),
        Some(host.model),
        "the roster holds the host's model override"
    );

    // For good: the member is broken, and nothing is built.
    let closed_session = mob.session("closed").await;
    preparer.compose_seats(SeatScript::Refuse(MemberBuildPreparationRefusal::Final {
        reason: "the host refuses this seat".to_string(),
    }));
    let refused = wake(&mob, "closed").await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.to_string().contains("the host refuses this seat")),
        "the wake of a seat its host refused for good fails with the host's reason: {refused:?}"
    );
    assert!(
        builds_on(&mob, &closed_session).await.is_empty(),
        "a refused seat is not built"
    );
}

/// Row: a seat composition its host refuses at a later build, of a member
/// whose session a build created, ends that build as it ends a first one.
/// After a restart the host refuses the seat for good: the wake fails with
/// the host's reason, the host's build preparation is not asked, and
/// nothing is built.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_seat_composition_at_a_later_build_of_a_built_session_builds_nothing() {
    use crate::runtime::MemberBuildPreparationRefusal;
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("seat-refused-at-later-build", &preparer).await;
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let session = mob.session("quiet").await;
    wake(&mob, "quiet")
        .await
        .expect("the first wake builds the member");
    assert_eq!(builds_on(&mob, &session).await.len(), 1, "one build");
    let asked = preparer.asked().len();

    mob.restart_configured(MobCompositionAuthority::Authoritative, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    assert_eq!(mob.session("quiet").await, session, "the same session");
    preparer.compose_seats(SeatScript::Refuse(MemberBuildPreparationRefusal::Final {
        reason: "the host refuses this seat's later build".to_string(),
    }));
    let refused = wake(&mob, "quiet").await;
    assert!(
        refused.as_ref().is_err_and(|error| error
            .to_string()
            .contains("the host refuses this seat's later build")),
        "the later build of a seat its host refused fails with the host's reason: {refused:?}"
    );
    assert_eq!(mob.builds().await, 0, "nothing was built after the restart");
    assert_eq!(
        preparer.asked().len(),
        asked,
        "a build its seat composition refused asks for no build preparation"
    );
}

/// A host build preparer that asks for a hold on each build and makes the
/// build's session fail to be created: the provision of a spawn it prepared
/// fails after its hold was staged.
struct HoldThenFailingProvision {
    service: Arc<MockSessionService>,
    held: std::sync::Mutex<Vec<SessionId>>,
}

#[async_trait::async_trait]
impl crate::runtime::MemberBuildPreparer for HoldThenFailingProvision {
    async fn prepare_member_build(
        &self,
        request: crate::runtime::MemberBuildPreparation,
    ) -> Result<crate::runtime::MemberBuildPrepared, crate::runtime::MemberBuildPreparationRefusal>
    {
        self.service
            .set_create_session_failure(&request.session_id)
            .await;
        self.held
            .lock()
            .expect("the held sessions")
            .push(request.session_id);
        Ok(crate::runtime::MemberBuildPrepared::HoldUntilReleased)
    }
}

/// Row: a kickoff spawn that fails after its host asked for a hold leaves
/// no hold staged for the session it named. The host answers
/// `HoldUntilReleased`, Core stages the hold on the member's next
/// registration, and the provision then fails: the spawn is answered
/// failed, the identity is owed its seat, and the staged hold is removed,
/// so the next spawn of the identity is prepared again and held only if
/// its host asks again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_kickoff_spawn_leaves_no_run_start_hold_staged() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let service = Arc::new(MockSessionService::new());
    let adapter = service.enable_runtime_adapter();
    let preparer = Arc::new(HoldThenFailingProvision {
        service: service.clone(),
        held: std::sync::Mutex::new(Vec::new()),
    });
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(
        store_owned_definition("kickoff-hold-unstaged"),
        storage.clone(),
    )
    .with_session_service(service.clone())
    .with_member_comms_identity_root(keys.path())
    .with_member_build_preparer(preparer.clone())
    .create()
    .await
    .expect("create the mob");
    let mob = StoreOwnedMob {
        keys,
        storage,
        handle,
        service,
    };
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let created = mob
        .create_outcome(kickoff_create("held", "Wait."), ALLOW)
        .await
        .expect("the create commits");
    assert!(
        matches!(
            created.embodied.get(&id("held")),
            Some(crate::runtime::DeclaredEntryOutcome::Failed { .. })
        ),
        "a spawn whose provision failed is answered failed: {created:?}"
    );
    mob.unseated_for("held", UnseatedEntryCause::SeatOwed).await;
    let held = preparer.held.lock().expect("the held sessions").clone();
    assert_eq!(
        held.len(),
        1,
        "the host was asked once and asked for a hold"
    );
    assert!(
        !adapter.registration_run_start_hold_staged(
            &held[0],
            meerkat_runtime::RunStartHoldReason::ToolsNotPublished
        ),
        "the failed spawn's staged hold is removed"
    );
}

/// A host build preparer that keeps the trait's default seat composition.
struct DefaultSeatPreparer;

#[async_trait::async_trait]
impl crate::runtime::MemberBuildPreparer for DefaultSeatPreparer {
    async fn prepare_member_build(
        &self,
        _request: crate::runtime::MemberBuildPreparation,
    ) -> Result<crate::runtime::MemberBuildPrepared, crate::runtime::MemberBuildPreparationRefusal>
    {
        Ok(crate::runtime::MemberBuildPrepared::Prepared)
    }
}

/// Row: a host whose build preparer keeps the default seat composition
/// changes nothing. A store-owned identity created with a first message and
/// one created Dormant and then woken are each built from their row alone:
/// the profile's model, the row's context and instructions, no system
/// prompt of the host's, and no override on their spawn rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_seat_composition_builds_a_store_owned_member_from_its_row_alone() {
    let mob = StoreOwnedMob::build(
        store_owned_definition("seat-default"),
        MobStorage::in_memory(),
        |builder| builder.with_member_build_preparer(Arc::new(DefaultSeatPreparer)),
    )
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let created = mob
        .create_outcome(kickoff_create("greeter", "Begin."), ALLOW)
        .await
        .expect("the create commits");
    assert!(
        matches!(
            created.embodied.get(&id("greeter")),
            Some(crate::runtime::DeclaredEntryOutcome::Embodied { .. })
        ),
        "{:?}",
        created.embodied
    );
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    wake(&mob, "quiet")
        .await
        .expect("the wake builds the member");

    for name in ["greeter", "quiet"] {
        let session = mob.session(name).await;
        let builds = builds_on(&mob, &session).await;
        assert_eq!(builds.len(), 1, "{name}: one build");
        let build = &builds[0];
        assert_eq!(
            build.model, "claude-sonnet-4-5",
            "{name}: the profile's model"
        );
        assert_eq!(
            build.app_context,
            Some(serde_json::json!({"desk": name})),
            "{name}: the row's context"
        );
        assert_eq!(
            build.additional_instructions,
            Some(vec![format!("You are {name}.")]),
            "{name}: the row's instructions"
        );
        assert!(
            !matches!(
                build.system_prompt,
                meerkat_core::SystemPromptOverride::Set(_)
            ),
            "{name}: no system prompt of a host's: {:?}",
            build.system_prompt
        );
        let row = mob.spawn_row(name).await;
        assert_eq!(
            row.labels,
            spec(name, "worker").labels,
            "{name}: the row's labels"
        );
        assert_eq!(
            (row.effective_model_override, row.effective_profile_override),
            (None, None),
            "{name}: no override"
        );
    }
}

/// Row (RED): Core refuses a seat composition that changed the seat's
/// identity, role, launch mode or custody. At a kickoff spawn the spawn
/// fails naming what changed, the identity is owed its seat with no member
/// and nothing is built under either name; at a Dormant seat's first wake
/// the wake fails naming it and nothing is built.
///
/// RED on the old tree: the host is never asked to compose the seat, so
/// nothing refuses the spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_composition_that_changed_identity_role_launch_mode_or_custody_is_refused() {
    let preparer = ScriptedPreparer::new();
    let mob = seat_composing_mob("seat-change-refused", &preparer).await;
    for change in [
        SeatChange::Identity,
        SeatChange::Role,
        SeatChange::LaunchMode,
        SeatChange::Custody,
    ] {
        let name = format!("changed-{}", change.named().replace(' ', "-"));
        preparer.compose_seats(SeatScript::Change(change));
        let created = mob
            .create_outcome(kickoff_create(&name, "Wait."), ALLOW)
            .await
            .expect("the create commits");
        match created.embodied.get(&id(&name)) {
            Some(crate::runtime::DeclaredEntryOutcome::Failed { error }) => assert!(
                error.to_string().contains(change.named()),
                "{change:?}: the failed spawn names what the host changed: {error}"
            ),
            other => panic!("{change:?}: the spawn is answered failed: {other:?}"),
        }
        mob.unseated_for(&name, UnseatedEntryCause::SeatOwed).await;
        assert!(!mob.is_member(&name).await, "{change:?}: no member");
    }
    assert!(
        !mob.is_member("impostor").await,
        "no member under another name"
    );
    assert_eq!(mob.builds().await, 0, "nothing was built");

    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    preparer.compose_seats(SeatScript::Change(SeatChange::Role));
    let refused = wake(&mob, "quiet").await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.to_string().contains(SeatChange::Role.named())),
        "the wake of a seat whose role its host changed fails naming it: {refused:?}"
    );
    assert_eq!(mob.builds().await, 0, "nothing was built");
    assert_eq!(
        mob.role("quiet").await,
        Some(ProfileName::from("worker")),
        "the member keeps its role"
    );
}

/// A store-owned member carries its row's labels and no others, so Core
/// refuses a seat composition that answers other labels than the seat's:
/// `change` is what the scripted host's answer does to them. The refusal is
/// for good, names the key and never a value, and names the remedy: the
/// label belongs in the identity's row. At a kickoff spawn the spawn fails,
/// the identity is owed its seat and nothing is built; at a Dormant seat's
/// first wake the wake fails and nothing is built.
async fn seat_label_change_is_refused(change: LabelChange) {
    let preparer = ScriptedPreparer::new();
    let label = format!("seat-labels-{change:?}").to_lowercase();
    let mob = seat_composing_mob(&label, &preparer).await;
    let names_the_key_only = |reason: &str| {
        reason.contains(&change.named())
            && reason.contains("row")
            && !reason.contains(LabelChange::ADDED.1)
            && !reason.contains(LabelChange::REVALUED.1)
            && !reason.contains("alpha")
    };
    preparer.compose_seats(SeatScript::Labels(change));
    let created = mob
        .create_outcome(kickoff_create("kicked", "Wait."), ALLOW)
        .await
        .expect("the create commits");
    match created.embodied.get(&id("kicked")) {
        Some(crate::runtime::DeclaredEntryOutcome::Failed { error }) => assert!(
            names_the_key_only(&error.to_string()),
            "{change:?}: the failed spawn names the label key and the remedy, and no value: \
             {error}"
        ),
        other => panic!("{change:?}: the spawn is answered failed: {other:?}"),
    }
    mob.unseated_for("kicked", UnseatedEntryCause::SeatOwed)
        .await;
    assert!(!mob.is_member("kicked").await, "{change:?}: no member");

    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let refused = wake(&mob, "quiet").await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| names_the_key_only(&error.to_string())),
        "{change:?}: the wake fails naming the label key and the remedy, and no value: \
         {refused:?}"
    );
    assert_eq!(mob.builds().await, 0, "{change:?}: nothing was built");
}

/// Row (RED): an answer that adds a label the seat does not carry is
/// refused ([`seat_label_change_is_refused`]): a member is never seated
/// with a label its row does not have, nor built without one its host
/// requires.
///
/// RED on the old tree: Core dropped the labels of the host's answer and
/// built the member with its row's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_composition_that_adds_a_label_the_seat_lacks_is_refused_naming_the_key() {
    seat_label_change_is_refused(LabelChange::Add).await;
}

/// Row (RED): an answer that drops a label the seat carries is refused
/// ([`seat_label_change_is_refused`]).
///
/// RED on the old tree, as the row above.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_composition_that_drops_a_label_of_the_seat_is_refused_naming_the_key() {
    seat_label_change_is_refused(LabelChange::Drop).await;
}

/// Row (RED): an answer that gives a label of the seat another value is
/// refused ([`seat_label_change_is_refused`]).
///
/// RED on the old tree, as the row above.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_composition_that_changes_a_label_of_the_seat_is_refused_naming_the_key() {
    seat_label_change_is_refused(LabelChange::Revalue).await;
}

/// Row: an answer that sets the seat's own labels again, key by key, is no
/// difference: the member is built, and its roster labels are its row's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_composition_that_answers_the_seats_own_labels_builds_the_member() {
    let preparer = ScriptedPreparer::new();
    let mob = seat_composing_mob("seat-labels-same", &preparer).await;
    committed(
        mob.create(host_create("create-same", &["same"]), ALLOW)
            .await,
    );
    preparer.compose_seats(SeatScript::Labels(LabelChange::Same));
    let session = mob.session("same").await;
    wake(&mob, "same")
        .await
        .expect("an answer with the seat's own labels builds the member");
    assert_eq!(builds_on(&mob, &session).await.len(), 1, "one build");
    let entry = mob
        .handle
        .get_member(&id("same"))
        .await
        .expect("roster read")
        .expect("same is a member");
    assert_eq!(
        entry.labels,
        spec("same", "worker").labels,
        "the member's roster labels are its row's"
    );
}

/// Row (RED): the host is asked at every build of a store-owned member,
/// and a later build takes less than the first. The first build of a
/// Dormant seat takes the host's model override, system prompt, context and
/// instruction section. After a restart the member's next build, on its
/// durable session, asks the host again and takes its model override anew,
/// and none of the system prompt, context or instruction section the host
/// answers then: what the session was first built with stands. The
/// profile's resume mask names the model, so the rebuild runs the model its
/// request names (without the mask a rebuild keeps its session's own): with
/// no answer from the host it would run the profile's.
///
/// RED on the old tree: the host is never asked to compose the seat.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_build_takes_the_hosts_model_again_and_keeps_its_first_builds_instructions() {
    let preparer = ScriptedPreparer::new();
    let mut definition = store_owned_definition("seat-later-build");
    for binding in definition.profiles.values_mut() {
        if let Some(profile) = binding.as_inline_mut() {
            profile.resume_overrides = vec![crate::profile::ResumeOverrideField::Model];
        }
    }
    let mut mob = StoreOwnedMob::build(definition, MobStorage::in_memory(), {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let session = mob.session("quiet").await;
    let first = HostBuild::first();
    preparer.compose_seats(SeatScript::Compose(first.clone()));
    wake(&mob, "quiet")
        .await
        .expect("the first wake builds the member");
    let builds = builds_on(&mob, &session).await;
    assert_eq!(builds.len(), 1, "one build of the seat's session");
    assert!(
        first.gave_model(&builds[0]) && first.gave_instructions(&builds[0]),
        "the first build takes everything the host composed: {:?}",
        builds[0]
    );

    let second = HostBuild::second();
    preparer.compose_seats(SeatScript::Compose(second.clone()));
    mob.restart_configured(MobCompositionAuthority::Authoritative, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    assert_eq!(mob.session("quiet").await, session, "the same session");
    wake(&mob, "quiet")
        .await
        .expect("the next wake builds the member");
    assert_eq!(
        preparer.seats(),
        vec![id("quiet"), id("quiet")],
        "the host is asked at each build"
    );
    let builds = builds_on(&mob, &session).await;
    let later = builds.last().expect("the member's later build");
    assert!(
        second.gave_model(later),
        "the later build takes the host's model override again: {later:?}"
    );
    assert!(
        second.gave_no_instructions(later),
        "the later build takes no system prompt, context or instructions of the host's: {later:?}"
    );
    let entry = mob
        .handle
        .get_member(&id("quiet"))
        .await
        .expect("roster read")
        .expect("quiet is a member");
    assert_eq!(
        entry.labels,
        spec("quiet", "worker").labels,
        "the member's roster labels stay its row's"
    );
    assert_eq!(
        entry.effective_model_override.as_deref(),
        Some(second.model),
        "the roster holds the override the host answered at this build"
    );
}

/// Row (RED): a label update of a seated store-owned identity seats its
/// row's labels at once, so the member's next build agrees with a host that
/// answers the row's labels. `quiet` is a Dormant seat no build created
/// yet; `built` was built before the update and is live and idle at it.
/// Each update restamps its member on its own session at the next
/// generation, the new seat row carries the row's labels, and no seat is
/// stale. The next build of each, and the one after a restart, is asked
/// about a seat with the row's labels and builds the member.
///
/// RED on the old tree: the update keeps the member with the labels it was
/// seated with, and the next build is refused for good, naming the changed
/// key, because the host answers the row's labels.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_update_seats_the_rows_labels_and_the_next_build_agrees_with_the_host() {
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("label-update-restamps", &preparer).await;
    let names = ["quiet", "built"];
    committed(mob.create(host_create("create-desks", &names), ALLOW).await);
    wake(&mob, "built")
        .await
        .expect("the first wake builds the member");
    let moved = |name: &str| {
        BTreeMap::from([
            ("team".to_string(), "beta".to_string()),
            ("desk".to_string(), name.to_string()),
        ])
    };
    let mut kinds = Vec::new();
    let mut seated = Vec::new();
    for name in names {
        let before = spec(name, "worker");
        seated.push((
            mob.generation(name).await.expect("a member"),
            mob.session(name).await,
        ));
        let updated = mob
            .update(
                &format!("relabel-{name}"),
                before.clone().with_labels(moved(name)),
                &spec_digest(&before),
            )
            .await
            .expect("the label update commits");
        kinds.push(answer_kind(
            updated.applied.as_ref().expect("the update's answer"),
        ));
    }

    // The host answers each seat from its identity's row.
    let rows = SeatScript::RowLabels(names.iter().map(|name| (id(name), moved(name))).collect());
    preparer.compose_seats(rows);
    let builds_with_the_rows_labels = |mob: &StoreOwnedMob, name: &'static str| {
        let handle = mob.handle.clone();
        async move {
            let woken = tokio::time::timeout(STEP, handle.ensure_member_live(&id(name)))
                .await
                .expect("the wake answers");
            assert!(
                woken.is_ok(),
                "{name}: the build after a label update agrees with a host that answers the \
                 row's labels: {woken:?}"
            );
            let entry = handle
                .get_member(&id(name))
                .await
                .expect("roster read")
                .expect("a member");
            assert_eq!(
                entry.labels,
                moved(name),
                "{name}: the member carries its row's labels"
            );
        }
    };
    for name in names {
        builds_with_the_rows_labels(&mob, name).await;
    }
    assert_eq!(kinds, ["restamped", "restamped"]);
    for (name, (generation, session)) in names.into_iter().zip(seated) {
        assert_eq!(mob.session(name).await, session, "{name}: the same session");
        assert_eq!(
            mob.generation(name).await,
            generation.next(),
            "{name}: the label update seated the next generation"
        );
        assert_eq!(
            mob.spawn_row(name).await.labels,
            moved(name),
            "{name}: the update's seat row carries the row's labels"
        );
    }
    assert_eq!(seats_stale(&mob).await, 0, "no seat is stale");
    mob.restart_configured(MobCompositionAuthority::Authoritative, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    for name in names {
        builds_with_the_rows_labels(&mob, name).await;
    }
}

/// Row (RED): the restamp a label update seats its row's labels with keeps
/// the member. `desk` was created with a first message and a peer, ran its
/// kickoff and was built; after the update it is the same store-owned
/// entry on the same session at the next generation, its edge to its peer
/// stands, its durable session holds what it held, and its kickoff is not
/// run again. Its next build resumes that session (it mints none), so its
/// next run continues the conversation.
///
/// RED on the old tree: the update restamps nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_updates_restamp_keeps_the_members_session_conversation_kickoff_and_edges() {
    use crate::identity::{OwnedEdgeAdmission, OwnedIdentityEdge};
    let mob = StoreOwnedMob::new("label-update-keeps-member").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    committed(
        mob.create(host_create("create-peer", &["peer"]), ALLOW)
            .await,
    );
    let before = spec("desk", "worker")
        .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
        .with_initial_message(meerkat_core::types::ContentInput::from("Begin."));
    let created = mob
        .create_outcome(
            StoreOwnedIdentityCreate::new(
                operation("create-desk"),
                IdentityChangeRequesterInput::host("console"),
                vec![CreatedIdentityRequest::new(before.clone()).with_peers(vec![
                    OwnedIdentityEdge::new(
                        "peer",
                        OwnedEdgeAdmission::rule("team-peers").expect("a peer rule id"),
                    ),
                ])],
            ),
            ALLOW,
        )
        .await
        .expect("the create commits");
    assert!(
        matches!(
            created.embodied.get(&id("desk")),
            Some(crate::runtime::DeclaredEntryOutcome::Embodied { .. })
        ),
        "{created:?}"
    );
    let session = kicked_off(&mob, "desk", "Begin.").await;
    let generation = mob.generation("desk").await.expect("desk is a member");
    let wired = mob.wired_to("desk").await;
    assert_eq!(
        wired,
        BTreeSet::from([id("peer")]),
        "desk is wired to its peer"
    );
    let conversation = |mob: &StoreOwnedMob| {
        let service = mob.service.clone();
        let session = session.clone();
        async move {
            crate::runtime::session_service::MobSessionService::load_persisted_session(
                service.as_ref(),
                &session,
            )
            .await
            .expect("the durable session reads")
            .map(|held| held.messages().len())
        }
    };
    let held = conversation(&mob).await;
    assert!(held.is_some(), "the member's session is durable");

    let moved = BTreeMap::from([("team".to_string(), "beta".to_string())]);
    let updated = mob
        .update(
            "relabel-desk",
            before.clone().with_labels(moved.clone()),
            &spec_digest(&before),
        )
        .await
        .expect("the label update commits");
    assert!(
        matches!(
            updated.applied,
            Some(crate::runtime::DeclaredEntryOutcome::Restamped { .. })
        ),
        "the label update restamps the member: {updated:?}"
    );
    assert_eq!(mob.session("desk").await, session, "the same session");
    assert_eq!(
        mob.generation("desk").await,
        generation.next(),
        "the next generation"
    );
    assert!(
        matches!(
            mob.read("desk"),
            IdentityEntryRead::Entry(view)
                if view.origin == IdentityEntryOrigin::Created
                    && matches!(view.standing, EntryStanding::Seated(_))
        ),
        "the same store-owned entry, seated"
    );
    assert_eq!(mob.wired_to("desk").await, wired, "its edge stands");
    assert_eq!(
        conversation(&mob).await,
        held,
        "its durable session holds what it held"
    );

    wake(&mob, "desk")
        .await
        .expect("the restamped member's next wake builds it");
    let builds = builds_on(&mob, &session).await;
    let rebuilt = builds.last().expect("the member's build after the restamp");
    assert!(
        builds.len() > 1,
        "the wake after the restamp built the member"
    );
    assert_eq!(
        rebuilt.session_build_intent,
        meerkat_core::SessionBuildIntent::Resume,
        "the build after the restamp resumes the member's session: {rebuilt:?}"
    );
    assert_eq!(
        rebuilt.resume_session_id.as_ref(),
        Some(&session),
        "on the session the member held"
    );
    assert_eq!(conversation(&mob).await, held, "the conversation is kept");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        turns_with_prompt(&mob, &session, "Begin.").await,
        1,
        "the kickoff is not run again"
    );
    assert_eq!(mob.wired_to("desk").await, wired, "its edge stands");
}

/// Row: a label update of a member that holds work commits and defers its
/// restamp: nothing interrupts the member's turn, its seat keeps the labels
/// it was seated with, and the record census counts the seat stale. A
/// retry of the update while the member still holds work replays and
/// leaves the seat as it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_update_of_a_member_that_holds_work_defers_its_restamp() {
    let mob = StoreOwnedMob::new("label-update-busy").await;
    // A first message whose turn the rows' session service leaves running.
    let before = spec("desk", "worker")
        .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
        .with_initial_message(meerkat_core::types::ContentInput::from("Keep working."));
    mob.create_outcome(
        StoreOwnedIdentityCreate::new(
            operation("create-desk"),
            IdentityChangeRequesterInput::host("console"),
            vec![CreatedIdentityRequest::new(before.clone())],
        ),
        ALLOW,
    )
    .await
    .expect("the create commits");
    let session = kicked_off(&mob, "desk", "Keep working.").await;
    let generation = mob.generation("desk").await;
    let seated_with = mob.spawn_row("desk").await.labels;
    let moved = BTreeMap::from([("team".to_string(), "beta".to_string())]);
    let after = before.clone().with_labels(moved.clone());
    for attempt in ["first", "retried"] {
        let updated = mob
            .update("relabel-desk", after.clone(), &spec_digest(&before))
            .await
            .expect("the label update answers");
        match attempt {
            "first" => assert!(
                matches!(
                    updated.applied,
                    Some(crate::runtime::DeclaredEntryOutcome::Deferred { .. })
                ),
                "a member that holds work keeps its seat, its restamp deferred: {updated:?}"
            ),
            _ => assert!(
                matches!(updated.change, IdentityChangeOutcome::Replayed { .. }),
                "{updated:?}"
            ),
        }
        assert_eq!(
            mob.row("desk").await.spec.map(|spec| spec.labels),
            Some(moved.clone()),
            "{attempt}: the row holds the new labels"
        );
        assert_eq!(
            mob.generation("desk").await,
            generation,
            "{attempt}: no restamp"
        );
        assert_eq!(
            mob.session("desk").await,
            session,
            "{attempt}: the same session"
        );
        assert_eq!(
            mob.spawn_row("desk").await.labels,
            seated_with,
            "{attempt}: the seat keeps the labels it was seated with"
        );
        assert_eq!(
            seats_stale(&mob).await,
            1,
            "{attempt}: the seat counts stale"
        );
    }
}

/// Row: a label restamp an update left owed (the update stopped after its
/// commit, here; a member that held work at the update defers it the same
/// way) is carried out by the update's retry once the member is idle: the
/// retry replays, the member is restamped with its row's labels on its own
/// session, and the row lets the owed restamp go.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owed_label_restamp_is_carried_out_by_the_updates_retry() {
    let mob = StoreOwnedMob::new("label-restamp-owed").await;
    committed(
        mob.create(host_create("create-desk", &["desk"]), ALLOW)
            .await,
    );
    let before = spec("desk", "worker");
    let moved = BTreeMap::from([("team".to_string(), "beta".to_string())]);
    let after = before.clone().with_labels(moved.clone());
    let session = mob.session("desk").await;
    let generation = mob.generation("desk").await.expect("desk is a member");
    mob.commit_update_transaction("relabel-desk", after.clone())
        .await;
    assert_eq!(seats_stale(&mob).await, 1, "the restamp is owed");

    let retried = mob
        .update("relabel-desk", after, &spec_digest(&before))
        .await
        .expect("the retry answers");
    assert!(
        matches!(retried.change, IdentityChangeOutcome::Replayed { .. }),
        "{retried:?}"
    );
    assert_eq!(
        mob.spawn_row("desk").await.labels,
        moved,
        "the retry seats the row's labels"
    );
    assert_eq!(mob.generation("desk").await, generation.next());
    assert_eq!(mob.session("desk").await, session, "the same session");
    assert_eq!(seats_stale(&mob).await, 0, "no seat is stale");
    assert!(!label_restamp_owed(&mob, "desk").await);
}

/// Row (RED for the roster entry): a member a same-session successor cannot
/// serve keeps its seat at a label update: on a session service that keeps
/// no durable sessions the update answers kept, the member keeps its
/// session, its generation and fence, the journal's seat row keeps the
/// labels it was seated with, and the record census counts that row stale.
/// A rotation onto a fresh session would cost the member its conversation.
/// Its roster entry reads the row's labels right after the update.
///
/// RED on the old tree: the roster entry keeps the labels the member was
/// seated with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_update_keeps_a_member_a_same_session_successor_cannot_serve() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let service = Arc::new(MockSessionService::new());
    service.keep_no_durable_sessions();
    service.set_keep_alive_turns_complete_immediately(true);
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(
        store_owned_definition("label-update-non-durable"),
        storage.clone(),
    )
    .with_session_service(service.clone())
    .with_member_comms_identity_root(keys.path())
    .allow_ephemeral_sessions(true)
    .create()
    .await
    .expect("create the mob");
    let mob = StoreOwnedMob {
        keys,
        storage,
        handle,
        service,
    };
    let before = spec("desk", "worker")
        .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
        .with_initial_message(meerkat_core::types::ContentInput::from("Begin."));
    mob.create_outcome(
        StoreOwnedIdentityCreate::new(
            operation("create-desk"),
            IdentityChangeRequesterInput::host("console"),
            vec![CreatedIdentityRequest::new(before.clone())],
        ),
        ALLOW,
    )
    .await
    .expect("the create commits");
    let session = kicked_off(&mob, "desk", "Begin.").await;
    let seated = seat(&mob, "desk").await;
    let seated_with = mob.spawn_row("desk").await.labels;
    let moved = BTreeMap::from([("team".to_string(), "beta".to_string())]);
    let updated = mob
        .update(
            "relabel-desk",
            before.clone().with_labels(moved.clone()),
            &spec_digest(&before),
        )
        .await
        .expect("the label update commits");
    assert!(
        matches!(
            updated.applied,
            Some(crate::runtime::DeclaredEntryOutcome::Kept)
        ),
        "a member no same-session successor can serve keeps its seat: {updated:?}"
    );
    let after = seat(&mob, "desk").await;
    assert_eq!(
        (after.generation, after.fence_token),
        (seated.generation, seated.fence_token),
        "no restamp"
    );
    assert_eq!(mob.session("desk").await, session, "the same session");
    assert_eq!(
        mob.spawn_row("desk").await.labels,
        seated_with,
        "the seat row keeps the labels the member was seated with"
    );
    assert_eq!(seats_stale(&mob).await, 1, "the seat row counts stale");
    assert_eq!(
        after.labels, moved,
        "the roster entry reads the row's labels right after the update"
    );
}

/// Row (RED): a label update whose member step fails reaches the member's
/// roster entry at once. `built`'s durable session does not decode, so the
/// same-session successor that would seat its row's labels cannot be
/// planned: the update commits and answers the failed member step. The
/// roster entry reads the row's labels right after the answer, on the same
/// generation, fence and session, and no seat row was written.
///
/// RED on the old tree: the roster entry keeps the labels the member was
/// seated with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_update_whose_member_step_fails_reaches_its_roster_entry_at_once() {
    let mob = StoreOwnedMob::new("label-restamp-fails").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    committed(
        mob.create(host_create("create-built", &["built"]), ALLOW)
            .await,
    );
    wake(&mob, "built")
        .await
        .expect("the first wake builds the member");
    let seated = seat(&mob, "built").await;
    let session = mob.session("built").await;
    mob.service
        .refuse_persisted_session_reads(&session, PersistedSessionReadRefusal::Corrupted);
    let before = spec("built", "worker");
    let updated = mob
        .update(
            "relabel-built",
            before.clone().with_labels(moved_team("built")),
            &spec_digest(&before),
        )
        .await
        .expect("the label update commits");
    assert_eq!(
        answer_kind(updated.applied.as_ref().expect("the update's answer")),
        "failed",
        "{updated:?}"
    );
    let after = seat(&mob, "built").await;
    assert_eq!(
        (after.generation, after.fence_token),
        (seated.generation, seated.fence_token),
        "no restamp"
    );
    assert_eq!(mob.session("built").await, session, "the same session");
    assert_eq!(
        mob.spawn_row("built").await.labels,
        before.labels,
        "no seat row was written"
    );
    assert_eq!(
        after.labels,
        moved_team("built"),
        "the roster entry reads the row's labels right after the update"
    );
}

/// A store-owned mob whose host build preparer is `preparer`, on a session
/// service that leaves an autonomous member's first turn running until the
/// row ends it.
async fn working_member_mob(label: &str, preparer: &Arc<ScriptedPreparer>) -> StoreOwnedMob {
    StoreOwnedMob::build(store_owned_definition(label), MobStorage::in_memory(), {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await
}

/// Create `desk` with a first message whose turn stays running: the member
/// holds work until the row ends that turn. Answers its spec and session.
async fn working_desk(mob: &StoreOwnedMob) -> (CreatedIdentitySpec, SessionId) {
    let before = spec("desk", "worker")
        .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
        .with_initial_message(meerkat_core::types::ContentInput::from("Keep working."));
    mob.create_outcome(
        StoreOwnedIdentityCreate::new(
            operation("create-desk"),
            IdentityChangeRequesterInput::host("console"),
            vec![CreatedIdentityRequest::new(before.clone())],
        ),
        ALLOW,
    )
    .await
    .expect("the create commits");
    let session = kicked_off(mob, "desk", "Keep working.").await;
    (before, session)
}

/// Update `desk`'s labels from `before`'s to `moved` while it holds work
/// ([`working_desk`]): the update commits, its restamp is deferred, and the
/// seat row keeps the labels the member was seated with.
async fn label_update_deferred_by_work(
    mob: &StoreOwnedMob,
    before: &CreatedIdentitySpec,
    moved: &BTreeMap<String, String>,
) {
    let updated = mob
        .update(
            "relabel-desk",
            before.clone().with_labels(moved.clone()),
            &spec_digest(before),
        )
        .await
        .expect("the label update commits");
    assert!(
        matches!(
            updated.applied,
            Some(crate::runtime::DeclaredEntryOutcome::Deferred { .. })
        ),
        "a member that holds work keeps its seat, its restamp deferred: {updated:?}"
    );
    assert_eq!(
        mob.spawn_row("desk").await.labels,
        spec("desk", "worker").labels,
        "the seat row keeps the labels the member was seated with"
    );
}

/// Release `name` once it is idle: it is Dormant, and its next wake is a
/// cold build.
async fn released(mob: &StoreOwnedMob, name: &str) {
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        match mob.handle.release_member(&id(name)).await {
            Ok(()) if mob.handle.member_unmaterialized(&id(name)) => return,
            Ok(()) | Err(MobError::LifecycleOperationPending { .. }) => {}
            Err(error) => panic!("the release of {name} failed: {error}"),
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{name} never went idle"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// End the turn `name` holds on `session`, then release the member
/// ([`released`]).
async fn released_once_idle(mob: &StoreOwnedMob, name: &str, session: &SessionId) {
    mob.service.set_keep_alive_turns_complete_immediately(true);
    tokio::time::timeout(STEP, mob.service.wait_keep_alive_turn_entered(session))
        .await
        .expect("the member's turn is running");
    mob.service.release_keep_alive_turn(session).await;
    released(mob, name).await;
}

/// `name`'s seat, as the mob's roster reads it.
async fn seat(mob: &StoreOwnedMob, name: &str) -> RosterEntry {
    mob.handle
        .get_member(&id(name))
        .await
        .expect("roster read")
        .expect("a member")
}

/// Whether `name` is a store-owned entry whose member is not Broken.
fn serves(mob: &StoreOwnedMob, name: &str) -> bool {
    matches!(
        mob.read(name),
        IdentityEntryRead::Entry(view)
            if view.origin == IdentityEntryOrigin::Created && view.broken.is_none()
    )
}

/// The labels the rows below move `name`'s row to: its team changes, its
/// desk stays.
fn moved_team(name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("team".to_string(), "beta".to_string()),
        ("desk".to_string(), name.to_string()),
    ])
}

/// A host that answers each of `names`' seats with [`moved_team`], as a
/// host that reads the labels from the identity's row does.
fn host_answers_moved_team(names: &[&str]) -> SeatScript {
    SeatScript::RowLabels(
        names
            .iter()
            .map(|name| (id(name), moved_team(name)))
            .collect(),
    )
}

/// Wake `name` and assert that its build agreed with its host: the wake
/// answers, the member is not Broken, its seat carries `labels`, and the
/// build the wake made on `session` carries them.
async fn builds_with_labels(
    mob: &StoreOwnedMob,
    name: &str,
    session: &SessionId,
    labels: &BTreeMap<String, String>,
) {
    let built = builds_on(mob, session).await.len();
    let woken = wake(mob, name).await;
    assert!(
        woken.is_ok(),
        "{name}: a build of a seat that trailed its row agrees with a host that answers the \
         row's labels: {woken:?}"
    );
    assert!(serves(mob, name), "{name}: the member is not Broken");
    assert_eq!(
        &seat(mob, name).await.labels,
        labels,
        "{name}: the seat carries its row's labels"
    );
    let builds = builds_on(mob, session).await;
    assert_eq!(builds.len(), built + 1, "{name}: the wake built the member");
    let rebuilt = builds.last().expect("the wake's build");
    // Every label of the row is on the build. A key the row dropped stays
    // in a resumed session's own peer metadata: a resume merges the build's
    // labels over the durable ones and deletes none.
    for (key, value) in labels {
        assert_eq!(
            rebuilt.peer_meta_labels.get(key),
            Some(value),
            "{name}: the build carries the row's labels: {rebuilt:?}"
        );
    }
}

/// Row (RED): a seat that trails its row and is owed its restamp takes its
/// row's labels at its next build. `quiet` is a Dormant seat no build
/// created; `built` was built and released. Each row's labels moved with no
/// member step after the update's commit, so each seat trails its row and
/// each row records the owed restamp. The host answers each seat from the
/// identity's row: the build is compared with the row's labels, and the
/// member is built with them on its own session and generation. No seat
/// row is written at a build, so a build settles nothing: the row keeps
/// the restamp owed, the journal's seat row follows at the member's next
/// restamp, and the census counts it until then.
///
/// RED on the old tree: the build is refused for good naming the changed
/// key, and the member is Broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_that_trails_its_row_takes_the_rows_labels_at_its_next_build() {
    let preparer = ScriptedPreparer::new();
    let mob = seat_composing_mob("trailing-seat-next-build", &preparer).await;
    let names = ["quiet", "built"];
    committed(mob.create(host_create("create-desks", &names), ALLOW).await);
    wake(&mob, "built")
        .await
        .expect("the first wake builds the member");
    released(&mob, "built").await;
    for name in names {
        mob.commit_update_transaction(
            &format!("relabel-{name}"),
            spec(name, "worker").with_labels(moved_team(name)),
        )
        .await;
    }
    assert_eq!(seats_stale(&mob).await, 2, "both seats trail their rows");
    preparer.compose_seats(host_answers_moved_team(&names));

    for name in names {
        let before = seat(&mob, name).await;
        let session = mob.session(name).await;
        builds_with_labels(&mob, name, &session, &moved_team(name)).await;
        let after = seat(&mob, name).await;
        assert_eq!(
            (after.generation, after.fence_token),
            (before.generation, before.fence_token),
            "{name}: the same incarnation"
        );
        assert_eq!(mob.session(name).await, session, "{name}: the same session");
        assert_eq!(
            mob.spawn_row(name).await.labels,
            spec(name, "worker").labels,
            "{name}: no seat row is written at a build"
        );
    }
    assert_eq!(
        seats_stale(&mob).await,
        2,
        "the census counts the seat rows until a restamp writes them"
    );
    for name in names {
        assert!(
            label_restamp_owed(&mob, name).await,
            "{name}: a build settles nothing: the restamp stays owed"
        );
    }
    assert_eq!(
        mob.handle
            .identity_record_census()
            .await
            .expect("the census answers")
            .label_restamps_owed,
        2,
        "the census counts the rows that owe a restamp"
    );
}

/// Row: a seat that trails its row with no restamp owed is compared with
/// its seat, as before this release. An earlier release's label update
/// moved `quiet`'s and `closed`'s rows and left their seats and no record
/// of it. A start leaves each roster entry with its seat's labels. A host
/// that leaves the seat as given builds `quiet` with the seat's labels; a
/// host that answers from the row is refused `closed`'s build for good,
/// naming the changed key: the seat is brought up to date first, by a
/// respawn with the row's labels or the identity's next update.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_that_trails_its_row_with_no_restamp_owed_is_compared_with_its_seat() {
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("trailing-seat-earlier-release", &preparer).await;
    let names = ["quiet", "closed"];
    committed(mob.create(host_create("create-desks", &names), ALLOW).await);
    for name in names {
        commit_update_transaction_of_an_earlier_release(
            &mob.handle,
            &format!("relabel-{name}"),
            spec(name, "worker").with_labels(moved_team(name)),
        )
        .await;
        assert!(!label_restamp_owed(&mob, name).await, "{name}");
    }
    assert_eq!(seats_stale(&mob).await, 2, "both seats trail their rows");
    mob.restart_configured(MobCompositionAuthority::Authoritative, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    for name in names {
        assert_eq!(
            seat(&mob, name).await.labels,
            spec(name, "worker").labels,
            "{name}: the start leaves the seat's labels on the roster"
        );
    }

    let session = mob.session("quiet").await;
    builds_with_labels(&mob, "quiet", &session, &spec("quiet", "worker").labels).await;

    preparer.compose_seats(host_answers_moved_team(&names));
    let refused = wake(&mob, "closed").await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.to_string().contains("changed [team]")),
        "a host that answers the row is refused, naming the changed key: {refused:?}"
    );
    assert!(!serves(&mob, "closed"), "the member is Broken");

    // The remedy: a respawn with the row's labels brings the seat up to
    // date, and the next incarnation builds.
    tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(
            id("closed"),
            crate::runtime::MemberRespawn::new().with_labels(moved_team("closed")),
        ),
    )
    .await
    .expect("the respawn answers")
    .expect("the respawn seats the row's labels");
    let session = mob.session("closed").await;
    builds_with_labels(&mob, "closed", &session, &moved_team("closed")).await;
}

/// Row (RED): a label update that found its member busy does not break the
/// member. `desk` holds work at the update, so the update commits and its
/// restamp is deferred. Once the work ended and the member was released,
/// its next wake is a cold build: the host answers the labels of the
/// identity's row, and the member is built with them on its own session.
///
/// RED on the old tree: the seat keeps the labels it was seated with, the
/// host's answer differs from them, the build is refused for good naming
/// the changed key, and the member is Broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_update_a_busy_member_deferred_does_not_refuse_its_next_build() {
    let preparer = ScriptedPreparer::new();
    let mob = working_member_mob("label-deferred-next-build", &preparer).await;
    let (before, session) = working_desk(&mob).await;
    label_update_deferred_by_work(&mob, &before, &moved_team("desk")).await;
    preparer.compose_seats(host_answers_moved_team(&["desk"]));
    released_once_idle(&mob, "desk", &session).await;

    builds_with_labels(&mob, "desk", &session, &moved_team("desk")).await;
    let builds = builds_on(&mob, &session).await;
    let rebuilt = builds.last().expect("the build after the update");
    assert_eq!(
        rebuilt.session_build_intent,
        meerkat_core::SessionBuildIntent::Resume,
        "the build resumes the member's own session: {rebuilt:?}"
    );
    assert_eq!(rebuilt.resume_session_id.as_ref(), Some(&session));
}

/// Row (RED): the same across restarts. The mob stops while `desk` still
/// holds the work that deferred its label update. After the restart its
/// first build agrees with a host that answers the row's labels; and after
/// a second restart, with no restamp between, its first build agrees
/// again: nothing but the row and the seat decides it, at every build.
///
/// RED on the old tree: the first build after the restart is refused for
/// good naming the changed key, and the member is Broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_update_a_busy_member_deferred_does_not_refuse_its_build_after_restarts() {
    let preparer = ScriptedPreparer::new();
    let mut mob = working_member_mob("label-deferred-restarts", &preparer).await;
    let (before, session) = working_desk(&mob).await;
    label_update_deferred_by_work(&mob, &before, &moved_team("desk")).await;
    preparer.compose_seats(host_answers_moved_team(&["desk"]));
    for restart in ["first", "second"] {
        mob.restart_configured(MobCompositionAuthority::Authoritative, {
            let preparer = preparer.clone();
            move |builder| builder.with_member_build_preparer(preparer)
        })
        .await;
        mob.service.set_keep_alive_turns_complete_immediately(true);
        assert_eq!(
            mob.session("desk").await,
            session,
            "{restart} restart: the same session"
        );
        let woken = wake(&mob, "desk").await;
        assert!(
            woken.is_ok(),
            "{restart} restart: the first build agrees with a host that answers the row's \
             labels: {woken:?}"
        );
        assert!(
            serves(&mob, "desk"),
            "{restart} restart: the member is not Broken"
        );
        assert_eq!(
            seat(&mob, "desk").await.labels,
            moved_team("desk"),
            "{restart} restart: the seat carries its row's labels"
        );
        let builds = builds_on(&mob, &session).await;
        let rebuilt = builds.last().expect("the build after the restart");
        assert_eq!(
            rebuilt.peer_meta_labels.get("team").map(String::as_str),
            Some("beta"),
            "{restart} restart: the build carries the row's labels: {rebuilt:?}"
        );
        assert_eq!(
            mob.spawn_row("desk").await.labels,
            spec("desk", "worker").labels,
            "{restart} restart: no restamp wrote a seat row"
        );
        assert!(
            label_restamp_owed(&mob, "desk").await,
            "{restart} restart: a build writes no seat row, so the restamp stays owed"
        );
    }
}

/// Row (RED): a start seats the row's labels on the roster entry of each
/// store-owned identity whose row records an owed label restamp. The rows
/// of `quiet` (a Dormant seat no build created) and `built` (built before
/// the restart) moved past their seats in an update's commit, which
/// recorded the restamp as owed. After the restart each roster entry reads
/// its row's labels at once, with nothing built, no member woken and the
/// host asked nothing: the start reads the rows anyway. The journal's seat
/// rows are as they were, and the rows still record the owed restamp.
///
/// RED on the old tree: the restarted roster reads the labels of the
/// journal's seat rows until a build or a restamp.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_seats_a_store_owned_identitys_row_labels_on_its_roster_entry() {
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("start-seats-row-labels", &preparer).await;
    let names = ["quiet", "built"];
    committed(mob.create(host_create("create-desks", &names), ALLOW).await);
    wake(&mob, "built")
        .await
        .expect("the first wake builds the member");
    for name in names {
        mob.commit_update_transaction(
            &format!("relabel-{name}"),
            spec(name, "worker").with_labels(moved_team(name)),
        )
        .await;
    }
    let (composed, prepared) = (preparer.seats().len(), preparer.asked().len());
    preparer.compose_seats(host_answers_moved_team(&names));
    mob.restart_configured(MobCompositionAuthority::Authoritative, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);

    for name in names {
        assert_eq!(
            seat(&mob, name).await.labels,
            moved_team(name),
            "{name}: the restarted roster entry reads its row's labels"
        );
        assert!(
            mob.seated_dormant(name),
            "{name}: the member is Dormant: nothing woke it"
        );
        assert_eq!(
            mob.spawn_row(name).await.labels,
            spec(name, "worker").labels,
            "{name}: the journal's seat row is as it was"
        );
        assert!(
            label_restamp_owed(&mob, name).await,
            "{name}: a start settles nothing: the restamp stays owed"
        );
    }
    assert_eq!(mob.builds().await, 0, "the restart built nothing");
    assert_eq!(
        (preparer.seats().len(), preparer.asked().len()),
        (composed, prepared),
        "the host was asked nothing"
    );
    assert_eq!(
        seats_stale(&mob).await,
        2,
        "the census counts the seat rows"
    );
    for name in names {
        let session = mob.session(name).await;
        builds_with_labels(&mob, name, &session, &moved_team(name)).await;
    }
}

/// Row (RED): a label update of a busy member reaches its roster entry at
/// once. `desk` holds work when its `team` label is removed: the update
/// commits, its restamp is deferred, and a roster read right after the
/// answer shows the row's labels. Nothing else of the member moved: the
/// same generation, fence and session, no build, and its running turn is
/// undisturbed. Its session keeps the labels it was built with until its
/// next build, which a host that answers the row's labels is not refused.
///
/// RED on the old tree: the roster entry keeps the labels the member was
/// seated with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_update_of_a_busy_member_reaches_its_roster_entry_at_once() {
    let preparer = ScriptedPreparer::new();
    let mob = working_member_mob("label-deferred-roster", &preparer).await;
    let without_team = BTreeMap::from([("desk".to_string(), "desk".to_string())]);
    let (spec_before, session) = working_desk(&mob).await;
    let before = seat(&mob, "desk").await;
    let built = builds_on(&mob, &session).await.len();
    label_update_deferred_by_work(&mob, &spec_before, &without_team).await;

    let after = seat(&mob, "desk").await;
    assert_eq!(
        after.labels, without_team,
        "the roster entry reads the row's labels right after the update"
    );
    assert_eq!(
        (after.generation, after.fence_token),
        (before.generation, before.fence_token),
        "the same incarnation"
    );
    assert_eq!(mob.session("desk").await, session, "the same session");
    assert!(
        !mob.handle.member_unmaterialized(&id("desk")),
        "the member stays live"
    );
    assert_eq!(
        builds_on(&mob, &session).await.len(),
        built,
        "nothing was built"
    );
    assert_eq!(
        turns_with_prompt(&mob, &session, "Keep working.").await,
        1,
        "its turn ran once and was not started again"
    );
    tokio::time::timeout(STEP, mob.service.wait_keep_alive_turn_entered(&session))
        .await
        .expect("its running turn is undisturbed");
    assert_eq!(
        mob.spawn_row("desk").await.labels,
        spec("desk", "worker").labels,
        "no seat row was written"
    );

    preparer.compose_seats(SeatScript::RowLabels(BTreeMap::from([(
        id("desk"),
        without_team.clone(),
    )])));
    released_once_idle(&mob, "desk", &session).await;
    builds_with_labels(&mob, "desk", &session, &without_team).await;
}

/// Row (RED): the roster follows the row whatever arm the member step
/// took. `desk` holds work when one update moves its labels and its
/// runtime mode: the member step is the runtime-mode restamp, which a busy
/// member defers. The roster entry reads the row's labels right after the
/// answer, on the same generation, fence and session; the runtime mode
/// waits for the restamp.
///
/// RED on the old tree: the roster entry keeps the labels the member was
/// seated with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_and_runtime_mode_update_of_a_busy_member_reaches_its_roster_entry_at_once() {
    let mob = StoreOwnedMob::new("label-and-mode-deferred-roster").await;
    let (spec_before, session) = working_desk(&mob).await;
    let before = seat(&mob, "desk").await;
    let after = spec_before
        .clone()
        .with_labels(moved_team("desk"))
        .with_runtime_mode_override(crate::MobRuntimeMode::TurnDriven);
    let updated = mob
        .update("relabel-desk", after, &spec_digest(&spec_before))
        .await
        .expect("the update commits");
    assert_eq!(
        answer_kind(updated.applied.as_ref().expect("the update's answer")),
        "deferred",
        "{updated:?}"
    );
    let now = seat(&mob, "desk").await;
    assert_eq!(
        (now.generation, now.fence_token, now.runtime_mode),
        (before.generation, before.fence_token, before.runtime_mode),
        "no restamp"
    );
    assert_eq!(mob.session("desk").await, session, "the same session");
    assert_eq!(
        now.labels,
        moved_team("desk"),
        "the roster entry reads the row's labels right after the update"
    );
}

/// Row (RED): a restamp a busy member deferred stays owed after the roster
/// followed the row. `desk` holds work at its label update: the update is
/// deferred, and the roster entry reads the row's labels at once. Once its
/// work ended, a retry of the same update replays and carries the restamp
/// out: the member is seated at the next generation on the same session,
/// its seat row carries the row's labels, and no seat row counts stale.
///
/// RED on the old tree: the restamp was decided by comparing the roster
/// entry with the declaration, so once the roster read the row's labels a
/// retry restamped nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deferred_label_restamp_is_carried_out_by_a_retry_once_the_member_is_idle() {
    let mob = StoreOwnedMob::new("label-deferred-retry-idle").await;
    let (spec_before, session) = working_desk(&mob).await;
    let before = seat(&mob, "desk").await;
    let moved = moved_team("desk");
    label_update_deferred_by_work(&mob, &spec_before, &moved).await;
    assert_eq!(
        seat(&mob, "desk").await.labels,
        moved,
        "the roster follows the row at once"
    );
    assert_eq!(seats_stale(&mob).await, 1, "the seat row trails the row");
    assert!(label_restamp_owed(&mob, "desk").await);

    // The member's work ends; the retry finds it idle.
    mob.service.set_keep_alive_turns_complete_immediately(true);
    tokio::time::timeout(STEP, mob.service.wait_keep_alive_turn_entered(&session))
        .await
        .expect("the member's turn is running");
    mob.service.release_keep_alive_turn(&session).await;
    let after = spec_before.clone().with_labels(moved.clone());
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        let retried = mob
            .update("relabel-desk", after.clone(), &spec_digest(&spec_before))
            .await
            .expect("the retry answers");
        assert!(
            matches!(retried.change, IdentityChangeOutcome::Replayed { .. }),
            "{retried:?}"
        );
        if mob.spawn_row("desk").await.labels == moved {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the retry never restamped the idle member"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let now = seat(&mob, "desk").await;
    assert_eq!(
        Some(now.generation),
        before.generation.next(),
        "the retry seated the next generation"
    );
    assert_eq!(mob.session("desk").await, session, "the same session");
    assert_eq!(now.labels, moved);
    assert_eq!(seats_stale(&mob).await, 0, "no seat row counts stale");
    assert!(
        !label_restamp_owed(&mob, "desk").await,
        "the restamp's seat row settles what was owed"
    );
}

/// Row (guard, passes before and after): the label rule stands on a build
/// of a seat owed its label restamp, which is compared with the row's
/// labels. `quiet`'s row moved past its seat (the update's transaction
/// with no member step after it, which records the owed restamp). A host
/// whose spawn customizer sets a label the row lacks, and a host whose seat
/// composition adds one, are each refused that build for good, naming the
/// added key and no other: nothing is built.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_the_row_lacks_is_refused_on_a_build_that_seats_the_rows_labels() {
    let relabel = |mob: &StoreOwnedMob| {
        let handle = mob.handle.clone();
        async move {
            commit_update_transaction_of(
                &handle,
                "relabel-quiet",
                spec("quiet", "worker").with_labels(moved_team("quiet")),
            )
            .await;
        }
    };

    // The host's spawn customizer sets a label the row lacks.
    let mob = LabellingCustomizer::mob("stale-seat-customizer-label").await;
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    relabel(&mob).await;
    assert_eq!(seats_stale(&mob).await, 1, "the seat trails its row");
    let refused = wake(&mob, "quiet").await;
    assert!(
        refused.as_ref().is_err_and(|error| {
            let reason = error.to_string();
            LabellingCustomizer::refused_by_key(&reason) && !reason.contains("changed [")
        }),
        "the customizer's label is refused, and nothing else: {refused:?}"
    );
    assert_eq!(mob.builds().await, 0, "nothing was built");

    // The host's seat composition adds a label the row lacks.
    let preparer = ScriptedPreparer::new();
    let mob = seat_composing_mob("stale-seat-composition-label", &preparer).await;
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    relabel(&mob).await;
    assert_eq!(seats_stale(&mob).await, 1, "the seat trails its row");
    preparer.compose_seats(SeatScript::Labels(LabelChange::Add));
    let refused = wake(&mob, "quiet").await;
    assert!(
        refused.as_ref().is_err_and(|error| {
            let reason = error.to_string();
            reason.contains(&LabelChange::Add.named())
                && !reason.contains("changed [")
                && !reason.contains(LabelChange::ADDED.1)
        }),
        "the composition's label is refused, and nothing else: {refused:?}"
    );
    assert_eq!(mob.builds().await, 0, "nothing was built");
}

/// Row: what gets a Broken store-owned member out. `quiet`'s first build
/// is refused for good, so the member is Broken. Its next wake is refused
/// with the recorded reason and the host is not asked again. A later label
/// update of it restamps it (the Broken member holds no work): the next
/// incarnation is not Broken, and its build is asked of the host again and
/// built. A restart also clears it: a Broken member's first wake after a
/// restart asks the host again, and `closed`, whose seat also trails its
/// row by then, is built with its row's labels.
///
/// RED on the old tree in its last step only: the build after the restart
/// is refused for good naming the changed key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_store_owned_member_is_built_again_after_a_label_update_or_a_restart() {
    use crate::runtime::MemberBuildPreparationRefusal;
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("broken-member-recovers", &preparer).await;
    let names = ["quiet", "closed"];
    committed(mob.create(host_create("create-desks", &names), ALLOW).await);
    let broken = |mob: &StoreOwnedMob, name: &str| {
        matches!(
            mob.read(name),
            IdentityEntryRead::Entry(view) if view.broken.is_some()
        )
    };
    preparer.compose_seats(SeatScript::Refuse(MemberBuildPreparationRefusal::Final {
        reason: "the host refuses this seat".to_string(),
    }));
    for name in names {
        assert!(
            wake(&mob, name).await.is_err(),
            "{name}: the build is refused"
        );
        assert!(broken(&mob, name), "{name}: the member is Broken");
    }
    let asked = preparer.seats().len();
    preparer.compose_seats(host_answers_moved_team(&names));
    let again = wake(&mob, "quiet").await;
    assert!(
        again
            .as_ref()
            .is_err_and(|error| error.to_string().contains("the host refuses this seat")),
        "a Broken member's wake is refused with the recorded reason: {again:?}"
    );
    assert_eq!(
        preparer.seats().len(),
        asked,
        "the host is not asked again about a Broken member"
    );

    // A later label update restamps the Broken member.
    let before = spec("quiet", "worker");
    let generation = mob.generation("quiet").await.expect("a member");
    let updated = mob
        .update(
            "relabel-quiet",
            before.clone().with_labels(moved_team("quiet")),
            &spec_digest(&before),
        )
        .await
        .expect("the label update commits");
    assert_eq!(
        answer_kind(updated.applied.as_ref().expect("the update's answer")),
        "restamped",
        "{updated:?}"
    );
    assert_eq!(mob.generation("quiet").await, generation.next());
    assert!(!broken(&mob, "quiet"), "the next incarnation is not Broken");
    let session = mob.session("quiet").await;
    builds_with_labels(&mob, "quiet", &session, &moved_team("quiet")).await;

    // A restart: the Broken member's first wake asks the host again.
    mob.commit_update_transaction(
        "relabel-closed",
        spec("closed", "worker").with_labels(moved_team("closed")),
    )
    .await;
    mob.restart_configured(MobCompositionAuthority::Authoritative, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    assert!(
        !broken(&mob, "closed"),
        "a restart does not keep a member Broken"
    );
    let session = mob.session("closed").await;
    builds_with_labels(&mob, "closed", &session, &moved_team("closed")).await;
}

/// Whether `name`'s row records that its seat is owed a label restamp, read
/// from the row's stored form: a row that owes none carries no such key.
async fn label_restamp_owed(mob: &StoreOwnedMob, name: &str) -> bool {
    let row = serde_json::to_value(mob.row(name).await).expect("the row's stored form");
    row.get("label_restamp_owed")
        .is_some_and(|owed| owed.as_bool().expect("the marker is a flag"))
}

/// Seat `name` with a label its row never held: its host respawns it with
/// its row's labels and [`LabellingCustomizer::LABEL`]. No update made the
/// difference, so the row owes no restamp. Answers the seat's labels.
async fn seated_with_a_host_label(mob: &StoreOwnedMob, name: &str) -> BTreeMap<String, String> {
    let labels = LabellingCustomizer::labelled(spec(name, "worker").labels);
    tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(
            id(name),
            crate::runtime::MemberRespawn::new().with_labels(labels.clone()),
        ),
    )
    .await
    .expect("the respawn answers")
    .expect("the host's respawn seats its label");
    assert_eq!(mob.spawn_row(name).await.labels, labels);
    assert!(
        !label_restamp_owed(mob, name).await,
        "no update made the difference: nothing is owed"
    );
    labels
}

/// Retry the update `operation_id` to `after` (decided against `before`)
/// until its identity's seat row carries `after`'s labels: each retry
/// replays, and carries out what the update left owed once the member is
/// idle.
async fn retried_until_seated(
    mob: &StoreOwnedMob,
    operation_id: &str,
    before: &CreatedIdentitySpec,
    after: &CreatedIdentitySpec,
) {
    let name = after.identity.to_string();
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        let retried = mob
            .update(operation_id, after.clone(), &spec_digest(before))
            .await
            .expect("the retry answers");
        assert!(
            matches!(retried.change, IdentityChangeOutcome::Replayed { .. }),
            "{retried:?}"
        );
        if mob.spawn_row(&name).await.labels == after.labels {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the retry never carried out what the update left owed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// End the turn `desk` holds on `session` ([`working_desk`]).
async fn work_ended(mob: &StoreOwnedMob, session: &SessionId) {
    mob.service.set_keep_alive_turns_complete_immediately(true);
    tokio::time::timeout(STEP, mob.service.wait_keep_alive_turn_entered(session))
        .await
        .expect("the member's turn is running");
    mob.service.release_keep_alive_turn(session).await;
}

/// Row (RED): a label only a seat carries is not dropped at a build. Its
/// host seated `quiet` with a label its row never held, and its spawn
/// customizer sets that label at every build. No update moved the row, so
/// nothing is owed: the build is compared with the seat, agrees with it and
/// is built with the label, and the roster entry keeps it.
///
/// RED on the old tree: every difference between a seat and its row was
/// read as a seat that trails its row, so the build was compared with the
/// row's labels and refused for good naming the host's label as added.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_only_a_seat_carries_is_kept_at_its_builds() {
    let mob = LabellingCustomizer::mob("seat-only-label-builds").await;
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let labels = seated_with_a_host_label(&mob, "quiet").await;
    let session = mob.session("quiet").await;
    builds_with_labels(&mob, "quiet", &session, &labels).await;
    assert!(!label_restamp_owed(&mob, "quiet").await);
}

/// Row (RED): a label only a seat carries is not dropped at a start. After
/// a restart `quiet`'s roster entry still carries the label its host seated
/// it with (its row owes no restamp, so the start leaves its seat alone),
/// nothing was built and no member woken; and its first build after the
/// restart agrees with the seat, as before the restart.
///
/// RED on the old tree: the start set every store-owned roster entry's
/// labels from its row, so the label left the roster with no step and no
/// trace.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_only_a_seat_carries_is_kept_at_a_start() {
    let mut mob = LabellingCustomizer::mob("seat-only-label-start").await;
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let labels = seated_with_a_host_label(&mob, "quiet").await;
    let session = mob.session("quiet").await;
    mob.restart_configured(MobCompositionAuthority::Authoritative, |builder| {
        builder.with_spawn_member_customizer(Arc::new(LabellingCustomizer))
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    assert_eq!(
        seat(&mob, "quiet").await.labels,
        labels,
        "the restarted roster entry keeps the label its seat carries"
    );
    assert!(mob.seated_dormant("quiet"), "nothing woke the member");
    assert_eq!(mob.builds().await, 0, "the restart built nothing");
    builds_with_labels(&mob, "quiet", &session, &labels).await;
    assert!(!label_restamp_owed(&mob, "quiet").await);
}

/// Row (guard, passes before and after): an update of an idle identity
/// makes its seat equal its row. `relabelled` and `renamed` each carry a
/// label only their seat holds. A label update of the one and a descriptive
/// update of the other each restamp the idle member with the row's labels:
/// the seat-only label goes, by the update's own act, and nothing stays
/// owed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_of_an_idle_identity_drops_a_label_only_its_seat_carries() {
    let mob = StoreOwnedMob::new("seat-only-label-idle-update").await;
    let names = ["relabelled", "renamed"];
    committed(mob.create(host_create("create-desks", &names), ALLOW).await);
    for name in names {
        seated_with_a_host_label(&mob, name).await;
        let before = spec(name, "worker");
        let after = match name {
            "relabelled" => before.clone().with_labels(moved_team(name)),
            _ => before.clone().with_display_name("Renamed"),
        };
        let generation = mob.generation(name).await.expect("a member");
        let updated = mob
            .update(
                &format!("update-{name}"),
                after.clone(),
                &spec_digest(&before),
            )
            .await
            .expect("the update commits");
        assert_eq!(
            answer_kind(updated.applied.as_ref().expect("the update's answer")),
            "restamped",
            "{name}: {updated:?}"
        );
        assert_eq!(mob.generation(name).await, generation.next(), "{name}");
        assert_eq!(
            mob.spawn_row(name).await.labels,
            after.labels,
            "{name}: the seat row equals the row"
        );
        assert_eq!(seat(&mob, name).await.labels, after.labels, "{name}");
        assert!(
            !label_restamp_owed(&mob, name).await,
            "{name}: nothing stays owed"
        );
    }
}

/// Row (RED): a label restamp a busy member deferred is recorded in its
/// row, and the identity's next update carries it out. `desk` holds work at
/// its label update: the row records the owed restamp in the update's own
/// commit, and a retry while the member still holds work replays and
/// leaves it owed. Once the work ended, another update of the identity (a
/// descriptive one) restamps the idle member with the row's labels, and the
/// row lets the owed restamp go: a seat row now carries the row's labels.
///
/// RED on the old tree: the row keeps no fact that a restamp is owed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deferred_label_restamp_is_owed_in_the_row_until_the_next_update_carries_it_out() {
    let mob = StoreOwnedMob::new("label-deferred-next-update").await;
    let (before, session) = working_desk(&mob).await;
    let seated = seat(&mob, "desk").await;
    let moved = moved_team("desk");
    let after = before.clone().with_labels(moved.clone());
    assert!(!label_restamp_owed(&mob, "desk").await);
    label_update_deferred_by_work(&mob, &before, &moved).await;
    assert!(
        label_restamp_owed(&mob, "desk").await,
        "the update's commit records the owed restamp"
    );
    assert_eq!(seat(&mob, "desk").await.labels, moved);
    let retried = mob
        .update("relabel-desk", after.clone(), &spec_digest(&before))
        .await
        .expect("the retry answers");
    assert!(
        matches!(retried.change, IdentityChangeOutcome::Replayed { .. }),
        "{retried:?}"
    );
    assert!(
        label_restamp_owed(&mob, "desk").await,
        "a retry while the member holds work leaves the restamp owed"
    );
    assert_eq!(mob.spawn_row("desk").await.labels, before.labels);

    work_ended(&mob, &session).await;
    let deadline = tokio::time::Instant::now() + STEP;
    let mut expected = after.clone();
    let mut attempt = 0;
    loop {
        // Each attempt is an update of its own: one that still finds the
        // member finishing its turn commits and is deferred again.
        attempt += 1;
        let next = after
            .clone()
            .with_display_name(format!("Renamed {attempt}"));
        let updated = mob
            .update(
                &format!("rename-desk-{attempt}"),
                next.clone(),
                &spec_digest(&expected),
            )
            .await
            .expect("the next update commits");
        expected = next;
        if answer_kind(updated.applied.as_ref().expect("the update's answer")) == "restamped" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the next update never restamped the idle member: {updated:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(mob.spawn_row("desk").await.labels, moved);
    assert_eq!(mob.session("desk").await, session, "the same session");
    assert_eq!(
        Some(seat(&mob, "desk").await.generation),
        seated.generation.next(),
        "one restamp"
    );
    assert!(
        !label_restamp_owed(&mob, "desk").await,
        "a seat row carries the row's labels: nothing is owed"
    );
    assert_eq!(seats_stale(&mob).await, 0);
}

/// Row (RED): two label updates of a busy member owe one restamp, to the
/// row as it is. `desk` holds work at both: each commits and is deferred,
/// the roster entry reads the second row's labels, the seat row keeps the
/// labels the member was seated with, and the row records the owed restamp.
/// Once the work ended, a retry of the second update restamps the member
/// once, with the second row's labels.
///
/// RED on the old tree: the row keeps no fact that a restamp is owed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_label_updates_of_a_busy_member_owe_one_restamp_with_the_last_rows_labels() {
    let mob = StoreOwnedMob::new("label-deferred-twice").await;
    let (before, session) = working_desk(&mob).await;
    let seated = seat(&mob, "desk").await;
    let first = before.clone().with_labels(moved_team("desk"));
    let last_labels = BTreeMap::from([("team".to_string(), "gamma".to_string())]);
    let last = first.clone().with_labels(last_labels.clone());
    label_update_deferred_by_work(&mob, &before, &first.labels).await;
    let updated = mob
        .update("relabel-desk-again", last.clone(), &spec_digest(&first))
        .await
        .expect("the second label update commits");
    assert_eq!(
        answer_kind(updated.applied.as_ref().expect("the update's answer")),
        "deferred",
        "{updated:?}"
    );
    assert_eq!(seat(&mob, "desk").await.labels, last_labels);
    assert_eq!(mob.spawn_row("desk").await.labels, before.labels);
    assert!(label_restamp_owed(&mob, "desk").await);

    work_ended(&mob, &session).await;
    retried_until_seated(&mob, "relabel-desk-again", &first, &last).await;
    assert_eq!(
        Some(seat(&mob, "desk").await.generation),
        seated.generation.next(),
        "one restamp"
    );
    assert_eq!(mob.session("desk").await, session, "the same session");
    assert!(!label_restamp_owed(&mob, "desk").await);
}

/// Row: a restamp owed already stays owed across an update that returns the
/// row to the labels its seat carries, until a settle observes the seat.
/// `desk` holds work at a label update, so its row records the owed
/// restamp. A second update, the member still busy, moves the row back to
/// the labels the member was seated with. Its commit keeps the record (its
/// caller leaves here before anything reads the row again): the commit
/// compares the row with the seat row it read, and a restamp the earlier
/// update owes may be on its way to that seat, so the record is let go by
/// an observation made after the commit, never by the commit itself. A
/// retry of the update, the member still busy, observes that the seat row
/// carries the row's labels and lets the owed restamp go; no member step
/// ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restamp_owed_already_stays_owed_across_an_update_back_to_the_seats_labels() {
    let mut storage = MobStorage::in_memory();
    let store = FailingChangeCommit::wrap(&mut storage, |_| false);
    let mob = StoreOwnedMob::build(
        store_owned_definition("label-deferred-and-back"),
        storage,
        |builder| builder,
    )
    .await;
    let (before, _session) = working_desk(&mob).await;
    let seated = seat(&mob, "desk").await;
    let moved = before.clone().with_labels(moved_team("desk"));
    label_update_deferred_by_work(&mob, &before, &moved.labels).await;
    assert!(label_restamp_owed(&mob, "desk").await);

    let back = moved.clone().with_labels(before.labels.clone());
    store.park_row_reads_after_an_update();
    {
        let expected = spec_digest(&moved);
        let update = mob.update("relabel-desk-back", back.clone(), &expected);
        tokio::select! {
            answered = update => panic!("the update answered before its caller left: {answered:?}"),
            () = store.parked() => {}
        }
    }
    store.serve_row_reads();
    let row = mob.row("desk").await;
    assert_eq!(
        row.spec.as_ref().map(|spec| &spec.labels),
        Some(&before.labels),
        "the second update committed: the row carries the seat's own labels again"
    );
    assert_eq!(mob.spawn_row("desk").await.labels, before.labels);
    assert!(
        label_restamp_owed(&mob, "desk").await,
        "the commit keeps a restamp owed already, whatever the seat row it read carried"
    );
    assert_eq!(seat(&mob, "desk").await.labels, before.labels);

    let retried = mob
        .update("relabel-desk-back", back.clone(), &spec_digest(&moved))
        .await
        .expect("the retry answers");
    assert!(
        matches!(retried.change, IdentityChangeOutcome::Replayed { .. }),
        "{retried:?}"
    );
    assert!(
        !label_restamp_owed(&mob, "desk").await,
        "the seat row carries the row's labels: the settle lets the owed restamp go"
    );
    let now = seat(&mob, "desk").await;
    assert_eq!(
        (now.generation, now.fence_token),
        (seated.generation, seated.fence_token),
        "no member step ran"
    );
    assert_eq!(mob.spawn_row("desk").await.labels, before.labels);
}

/// Row (RED): the owed label restamp is let go by whatever seat row first
/// carries the row's labels. `desk` holds work when one update moves its
/// labels and its runtime mode, and `mover` when one update moves its
/// labels and its profile: each member step is deferred, the roster reads
/// the row's labels, and the row records the owed restamp (the profile
/// change keeps its role migration too). Once the work ended, a retry
/// carries out the runtime-mode restamp, or the role migration, which seats
/// the row's labels: the row lets the owed label restamp go.
///
/// RED on the old tree: the row keeps no fact that a restamp is owed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_mode_or_profile_restamp_seats_the_rows_labels_and_settles_the_owed_restamp() {
    for change in ["runtime_mode", "profile"] {
        let mob = StoreOwnedMob::new(&format!("label-and-{change}-deferred")).await;
        let (before, session) = working_desk(&mob).await;
        let moved = moved_team("desk");
        let mut after = before.clone().with_labels(moved.clone());
        match change {
            "runtime_mode" => {
                after = after.with_runtime_mode_override(crate::MobRuntimeMode::TurnDriven);
            }
            _ => after.profile = ProfileName::from("lead"),
        }
        let updated = mob
            .update("change-desk", after.clone(), &spec_digest(&before))
            .await
            .expect("the update commits");
        assert_eq!(
            answer_kind(updated.applied.as_ref().expect("the update's answer")),
            "deferred",
            "{change}: {updated:?}"
        );
        assert_eq!(seat(&mob, "desk").await.labels, moved, "{change}");
        assert!(
            label_restamp_owed(&mob, "desk").await,
            "{change}: the update's commit records the owed restamp"
        );
        assert_eq!(
            mob.row("desk").await.pending_role_migration.is_some(),
            change == "profile",
            "{change}"
        );

        work_ended(&mob, &session).await;
        retried_until_seated(&mob, "change-desk", &before, &after).await;
        let now = seat(&mob, "desk").await;
        match change {
            "runtime_mode" => {
                assert_eq!(now.runtime_mode, crate::MobRuntimeMode::TurnDriven);
            }
            _ => assert_eq!(now.role, ProfileName::from("lead")),
        }
        assert_eq!(
            mob.session("desk").await,
            session,
            "{change}: the same session"
        );
        let row = mob.row("desk").await;
        assert_eq!(row.pending_role_migration, None, "{change}");
        assert!(
            !label_restamp_owed(&mob, "desk").await,
            "{change}: a seat row carries the row's labels: nothing is owed"
        );
    }
}

/// Row (RED): a member no same-session successor can serve keeps its label
/// restamp owed, and its rotation settles it. On a session service that
/// keeps no durable sessions a label update of `desk` answers kept: the
/// roster reads the row's labels and the row records the owed restamp. A
/// later profile change rotates the member onto a fresh session under the
/// row's spec: that seat row carries the row's labels, and the row lets the
/// owed restamp go.
///
/// RED on the old tree: the row keeps no fact that a restamp is owed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_seats_the_rows_labels_and_settles_the_owed_restamp() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let service = Arc::new(MockSessionService::new());
    service.keep_no_durable_sessions();
    service.set_keep_alive_turns_complete_immediately(true);
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(
        store_owned_definition("label-owed-rotation"),
        storage.clone(),
    )
    .with_session_service(service.clone())
    .with_member_comms_identity_root(keys.path())
    .allow_ephemeral_sessions(true)
    .create()
    .await
    .expect("create the mob");
    let mob = StoreOwnedMob {
        keys,
        storage,
        handle,
        service,
    };
    let before = spec("desk", "worker")
        .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
        .with_initial_message(meerkat_core::types::ContentInput::from("Begin."));
    mob.create_outcome(
        StoreOwnedIdentityCreate::new(
            operation("create-desk"),
            IdentityChangeRequesterInput::host("console"),
            vec![CreatedIdentityRequest::new(before.clone())],
        ),
        ALLOW,
    )
    .await
    .expect("the create commits");
    kicked_off(&mob, "desk", "Begin.").await;
    let moved = moved_team("desk");
    let relabelled = before.clone().with_labels(moved.clone());
    let updated = mob
        .update("relabel-desk", relabelled.clone(), &spec_digest(&before))
        .await
        .expect("the label update commits");
    assert_eq!(
        answer_kind(updated.applied.as_ref().expect("the update's answer")),
        "kept",
        "{updated:?}"
    );
    assert!(
        label_restamp_owed(&mob, "desk").await,
        "the kept member's restamp stays owed"
    );
    assert_eq!(mob.spawn_row("desk").await.labels, before.labels);

    let mut moved_profile = relabelled.clone();
    moved_profile.profile = ProfileName::from("lead");
    let updated = mob
        .update("promote-desk", moved_profile, &spec_digest(&relabelled))
        .await
        .expect("the profile update commits");
    assert_eq!(
        answer_kind(updated.applied.as_ref().expect("the update's answer")),
        "role_rotated",
        "{updated:?}"
    );
    assert_eq!(
        mob.spawn_row("desk").await.labels,
        moved,
        "the rotation's seat row carries the row's labels"
    );
    assert!(
        !label_restamp_owed(&mob, "desk").await,
        "a seat row carries the row's labels: nothing is owed"
    );
}

/// Row (RED): an update whose caller went away after its commit leaves
/// nothing wrong. The caller of `quiet`'s label update is dropped right
/// after the update's transaction, before any member step. The roster
/// entry already reads the row's labels (the commit's own step set them),
/// the seat row keeps the labels the member was seated with, and the row
/// records the owed restamp. A retry of the update replays and restamps
/// the idle member, and the row lets the owed restamp go.
///
/// RED on the old tree: the row keeps no fact that a restamp is owed, and
/// no read of the row precedes the member step, so the caller here leaves
/// only after the restamp.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_whose_caller_is_dropped_after_its_commit_owes_its_restamp() {
    let mut storage = MobStorage::in_memory();
    let store = FailingChangeCommit::wrap(&mut storage, |_| false);
    let mob = StoreOwnedMob::build(
        store_owned_definition("label-update-caller-dropped"),
        storage,
        |builder| builder,
    )
    .await;
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let before = spec("quiet", "worker");
    let moved = moved_team("quiet");
    let after = before.clone().with_labels(moved.clone());
    let seated = seat(&mob, "quiet").await;
    store.park_row_reads_after_an_update();
    {
        let expected = spec_digest(&before);
        let update = mob.update("relabel-quiet", after.clone(), &expected);
        tokio::select! {
            answered = update => panic!("the update answered before its caller left: {answered:?}"),
            () = store.parked() => {}
        }
    }
    store.serve_row_reads();
    let now = seat(&mob, "quiet").await;
    assert_eq!(
        now.labels, moved,
        "the roster entry reads the row's labels: the commit's own step set them"
    );
    assert_eq!(
        (now.generation, now.fence_token),
        (seated.generation, seated.fence_token),
        "no member step ran"
    );
    assert_eq!(mob.spawn_row("quiet").await.labels, before.labels);
    assert!(
        label_restamp_owed(&mob, "quiet").await,
        "the update's commit records the owed restamp"
    );

    retried_until_seated(&mob, "relabel-quiet", &before, &after).await;
    assert_eq!(
        Some(seat(&mob, "quiet").await.generation),
        seated.generation.next(),
        "one restamp"
    );
    assert!(!label_restamp_owed(&mob, "quiet").await);
}

/// Row (RED): a host's own declaration of a store-owned identity restamps
/// no label its row lacks. A host declares `quiet` under created custody
/// with a label the identity's row does not hold. The row owes no restamp,
/// so the member is kept as it is: no seat row is written and the roster
/// entry keeps the row's labels. A label reaches a store-owned seat through
/// the identity's row, by an update.
///
/// RED on the old tree: the declaration's labels were compared with the
/// seat, and the member was restamped with a label its row lacks.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hosts_declaration_restamps_no_label_the_row_lacks() {
    let mob = StoreOwnedMob::new("declared-label-not-restamped").await;
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let seated = seat(&mob, "quiet").await;
    let row_labels = spec("quiet", "worker").labels;
    let declared =
        spec("quiet", "worker").with_labels(LabellingCustomizer::labelled(row_labels.clone()));
    let lowered =
        super::super::identity_change::lower_created_spec(&declared).expect("lower the spec");
    let answer = mob
        .handle
        .apply_declared_roster(DeclaredRoster::partial([DeclaredEntry::new(lowered)]))
        .await
        .expect("the declaration answers");
    assert_eq!(
        answer_kind(
            answer
                .entries
                .get(&id("quiet"))
                .expect("the declaration's answer")
        ),
        "kept",
        "{answer:?}"
    );
    let now = seat(&mob, "quiet").await;
    assert_eq!(now.generation, seated.generation, "no restamp");
    assert_eq!(now.labels, row_labels);
    assert_eq!(mob.spawn_row("quiet").await.labels, row_labels);
}

/// Row: a host's own declaration of a store-owned identity restamps no seat
/// whose row owes no restamp, whatever the seat carries. Its host seated
/// `quiet` with a label its row never held; no update moved the row, so
/// nothing is owed. The host then declares `quiet` under created custody
/// with the row's own spec: the member is kept as it is, no seat row is
/// written, and the seat keeps the label. Only a restamp the row records
/// as owed is carried out by a declaration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hosts_declaration_keeps_a_label_only_the_seat_carries() {
    let mob = StoreOwnedMob::new("declared-seat-label-kept").await;
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let labels = seated_with_a_host_label(&mob, "quiet").await;
    let seated = seat(&mob, "quiet").await;
    assert_eq!(seated.labels, labels);
    let lowered = super::super::identity_change::lower_created_spec(&spec("quiet", "worker"))
        .expect("lower the spec");
    let answer = mob
        .handle
        .apply_declared_roster(DeclaredRoster::partial([DeclaredEntry::new(lowered)]))
        .await
        .expect("the declaration answers");
    assert_eq!(
        answer_kind(
            answer
                .entries
                .get(&id("quiet"))
                .expect("the declaration's answer")
        ),
        "kept",
        "{answer:?}"
    );
    let now = seat(&mob, "quiet").await;
    assert_eq!(
        (now.generation, now.fence_token),
        (seated.generation, seated.fence_token),
        "no restamp"
    );
    assert_eq!(now.labels, labels, "the seat keeps the label");
    assert_eq!(mob.spawn_row("quiet").await.labels, labels);
    assert!(!label_restamp_owed(&mob, "quiet").await);
}

/// Row (RED): a row the store cannot serve at a build ends the build for a
/// retry of its own kind. `quiet`'s row read fails once at its first wake:
/// the wake is refused retryable as a store that cannot serve now, not as
/// its host's refusal; the host is asked nothing, nothing is built, and the
/// member is not Broken. Once the store serves again its next wake builds
/// it.
///
/// RED on the old tree: the failed read was recorded as the host's request
/// to retry its build preparation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_the_store_cannot_serve_at_a_build_ends_it_retryable_as_a_store_failure() {
    let preparer = ScriptedPreparer::new();
    let mut storage = MobStorage::in_memory();
    let store = FailingChangeCommit::wrap(&mut storage, |_| false);
    let mob = StoreOwnedMob::build(store_owned_definition("row-read-fails"), storage, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let (composed, prepared) = (preparer.seats().len(), preparer.asked().len());
    store.fail_row_reads(1);
    let refused = wake(&mob, "quiet").await;
    assert!(
        matches!(
            refused,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::ResumeGateUnavailable,
                ..
            })
        ),
        "a failed row read is a store that cannot serve now: {refused:?}"
    );
    assert_eq!(
        (preparer.seats().len(), preparer.asked().len()),
        (composed, prepared),
        "the host was asked nothing"
    );
    assert_eq!(mob.builds().await, 0, "nothing was built");
    assert!(serves(&mob, "quiet"), "the member is not Broken");
    assert!(mob.seated_dormant("quiet"), "the member stays Dormant");

    let deadline = tokio::time::Instant::now() + STEP;
    while let Err(error) = wake(&mob, "quiet").await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the member was never built once its row read again: {error}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(mob.builds().await, 1, "the next wake built the member");
    assert_eq!(preparer.seats().len(), composed + 1, "the host was asked");
}

/// Row: a row this build cannot read at a build ends the build for a retry,
/// as a row the store cannot serve does. `quiet`'s row reads unreadable (a
/// row a later release wrote, say): its wake is refused retryable as a
/// store that cannot serve the member now, never as its host's refusal,
/// and a second wake builds nothing either; the host is asked nothing,
/// nothing is built on labels the row may not hold, and the member is not
/// Broken and stays Dormant. Once the row reads again its next wake builds
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_this_build_cannot_read_at_a_build_ends_it_retryable_and_asks_the_host_nothing() {
    let preparer = ScriptedPreparer::new();
    let mut storage = MobStorage::in_memory();
    let store = FailingChangeCommit::wrap(&mut storage, |_| false);
    let mob = StoreOwnedMob::build(store_owned_definition("row-unreadable"), storage, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let (composed, prepared) = (preparer.seats().len(), preparer.asked().len());
    store.answer_row_reads_unreadable(true);
    let refused = wake(&mob, "quiet").await;
    assert!(
        matches!(
            refused,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::ResumeGateUnavailable,
                ..
            })
        ),
        "a row this build cannot read is a store that cannot serve the member now: {refused:?}"
    );
    let again = wake(&mob, "quiet").await;
    assert!(
        again.is_err(),
        "the row still reads unreadable: the next wake builds nothing either"
    );
    assert_eq!(
        (preparer.seats().len(), preparer.asked().len()),
        (composed, prepared),
        "the host was asked nothing"
    );
    assert_eq!(mob.builds().await, 0, "nothing was built");
    assert!(serves(&mob, "quiet"), "the member is not Broken");
    assert!(mob.seated_dormant("quiet"), "the member stays Dormant");

    store.answer_row_reads_unreadable(false);
    let deadline = tokio::time::Instant::now() + STEP;
    while let Err(error) = wake(&mob, "quiet").await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the member was never built once its row read again: {error}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(mob.builds().await, 1, "the next wake built the member");
    assert_eq!(preparer.seats().len(), composed + 1, "the host was asked");
}

/// Row: an archive and a removal let an owed label restamp go. `filed` and
/// `gone` each hold work at a label update, so each row records the owed
/// restamp. The archive of the one and the removal of the other retire the
/// member: no seat is owed anything, and the archived row and the
/// tombstone carry no such record. `filed`'s unarchive seats the row's
/// labels, with nothing owed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_archive_or_a_removal_lets_an_owed_label_restamp_go() {
    let mob = StoreOwnedMob::new("label-owed-archive-removal").await;
    committed(
        mob.create(host_create("create-desks", &["filed", "gone"]), ALLOW)
            .await,
    );
    for name in ["filed", "gone"] {
        mob.commit_update_transaction(
            &format!("relabel-{name}"),
            spec(name, "worker").with_labels(moved_team(name)),
        )
        .await;
        assert!(label_restamp_owed(&mob, name).await, "{name}");
    }
    mob.archive("archive-filed", "filed")
        .await
        .expect("the archive commits");
    mob.unseated_for("filed", UnseatedEntryCause::Archived)
        .await;
    assert!(
        !label_restamp_owed(&mob, "filed").await,
        "an archived identity's row owes no seat a restamp"
    );
    mob.remove("remove-gone", "gone")
        .await
        .expect("the removal commits");
    mob.removed("gone").await;
    assert!(
        !label_restamp_owed(&mob, "gone").await,
        "a tombstone owes no seat a restamp"
    );

    mob.unarchive("unarchive-filed", "filed")
        .await
        .expect("the unarchive commits");
    assert_eq!(mob.spawn_row("filed").await.labels, moved_team("filed"));
    assert!(!label_restamp_owed(&mob, "filed").await);
}

/// A host's spawn customizer that names no labels on any build: it leaves
/// the seat's.
struct ClearingCustomizer;

impl crate::runtime::SpawnMemberCustomizer for ClearingCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &crate::runtime::SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        spec.labels = None;
        Ok(())
    }
}

/// Row: a build of a seat owed its label restamp carries the row's labels,
/// whatever its seat carries and whatever the host's customizer leaves.
/// `quiet`'s row moved in an update's commit, which recorded the restamp as
/// owed; its host then respawned it with labels of its own, so its seat
/// row and its roster entry carry those, and the restamp stays owed (no
/// seat row carries the row's labels). Its host's customizer names no
/// labels. The build is compared with the row's labels and carries them,
/// not the seat's; and the build settles nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_build_of_a_seat_owed_its_restamp_carries_the_rows_labels() {
    let mob = StoreOwnedMob::build(
        store_owned_definition("owed-seat-build-labels"),
        MobStorage::in_memory(),
        |builder| builder.with_spawn_member_customizer(Arc::new(ClearingCustomizer)),
    )
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    mob.commit_update_transaction(
        "relabel-quiet",
        spec("quiet", "worker").with_labels(moved_team("quiet")),
    )
    .await;
    let host_labels = BTreeMap::from([("team".to_string(), "gamma".to_string())]);
    tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(
            id("quiet"),
            crate::runtime::MemberRespawn::new().with_labels(host_labels.clone()),
        ),
    )
    .await
    .expect("the respawn answers")
    .expect("the host's respawn commits");
    assert_eq!(mob.spawn_row("quiet").await.labels, host_labels);
    assert!(
        label_restamp_owed(&mob, "quiet").await,
        "no seat row carries the row's labels: the restamp stays owed"
    );

    let session = mob.session("quiet").await;
    wake(&mob, "quiet").await.expect("the build is accepted");
    assert!(serves(&mob, "quiet"), "the member is not Broken");
    let builds = builds_on(&mob, &session).await;
    let built = builds.last().expect("the wake's build");
    for (key, value) in &moved_team("quiet") {
        assert_eq!(
            built.peer_meta_labels.get(key),
            Some(value),
            "the build carries the row's labels: {built:?}"
        );
    }
    assert!(
        label_restamp_owed(&mob, "quiet").await,
        "a build settles nothing"
    );
}

/// A host's spawn customizer that sets one label on every build it is asked
/// about, as a host that labels its members in its build step does.
struct LabellingCustomizer;

impl LabellingCustomizer {
    /// The label it sets: a refusal names the key, never the value.
    const LABEL: (&'static str, &'static str) = ("host_zone", "host-secret-zone");

    /// A spec's labels with the customizer's label among them.
    fn labelled(mut labels: BTreeMap<String, String>) -> BTreeMap<String, String> {
        labels.insert(Self::LABEL.0.to_string(), Self::LABEL.1.to_string());
        labels
    }

    /// A store-owned mob whose host installs the customizer and no build
    /// preparer.
    async fn mob(label: &str) -> StoreOwnedMob {
        let mob = StoreOwnedMob::build(
            store_owned_definition(label),
            MobStorage::in_memory(),
            |builder| builder.with_spawn_member_customizer(Arc::new(Self)),
        )
        .await;
        mob.service.set_keep_alive_turns_complete_immediately(true);
        mob
    }

    /// Whether `reason` names the customizer's label as added, by its key
    /// and with the remedy, and never its value.
    fn refused_by_key(reason: &str) -> bool {
        reason.contains(&format!("added [{}]", Self::LABEL.0))
            && reason.contains("row")
            && !reason.contains(Self::LABEL.1)
    }
}

impl crate::runtime::SpawnMemberCustomizer for LabellingCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &crate::runtime::SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        spec.labels
            .get_or_insert_with(BTreeMap::new)
            .insert(Self::LABEL.0.to_string(), Self::LABEL.1.to_string());
        Ok(())
    }
}

/// Row (RED): a store-owned member's labels are compared with its seat's on
/// the labels its build carries, after the host's spawn customizer. A
/// customizer that adds a label the identity's row lacks is refused for
/// good, naming the key and never the value: the kickoff spawn of a create
/// fails, the identity is owed its seat and nothing is built; the first
/// wake of a Dormant seat fails and nothing is built. No build preparer is
/// configured: the comparison does not depend on one.
///
/// RED on the old tree: the customizer's label reaches the kickoff spawn's
/// seat and the wake's build unchecked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spawn_customizer_label_a_store_owned_seat_lacks_is_refused_naming_the_key() {
    let mob = LabellingCustomizer::mob("customizer-label-refused").await;
    let created = mob
        .create_outcome(kickoff_create("kicked", "Wait."), ALLOW)
        .await
        .expect("the create commits");
    match created.embodied.get(&id("kicked")) {
        Some(crate::runtime::DeclaredEntryOutcome::Failed { error }) => assert!(
            LabellingCustomizer::refused_by_key(&error.to_string()),
            "the failed spawn names the label key and the remedy, and no value: {error}"
        ),
        other => panic!(
            "the kickoff spawn of a seat the customizer labelled is answered failed: {other:?}"
        ),
    }
    mob.unseated_for("kicked", UnseatedEntryCause::SeatOwed)
        .await;
    assert!(!mob.is_member("kicked").await, "no member");

    committed(
        mob.create(host_create("create-quiet", &["quiet"]), ALLOW)
            .await,
    );
    let refused = wake(&mob, "quiet").await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| LabellingCustomizer::refused_by_key(&error.to_string())),
        "the wake fails naming the label key and the remedy, and no value: {refused:?}"
    );
    assert_eq!(mob.builds().await, 0, "nothing was built");
    assert!(
        mob.service
            .recorded_start_turn_prompts()
            .await
            .into_iter()
            .all(|(_, prompt)| prompt != "Wait."),
        "a refused build runs nothing"
    );
}

/// Row: the twins of the row above build. A store-owned identity whose row
/// carries the label the customizer sets is built, at its kickoff spawn and
/// at a Dormant seat's first wake, and its member carries its row's labels.
/// A roster-declared member is not compared: the customizer labels its
/// build as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spawn_customizer_label_the_row_carries_builds_and_a_roster_member_is_not_compared() {
    let mob = LabellingCustomizer::mob("customizer-label-in-row").await;
    let labelled = |name: &str| {
        let base = spec(name, "worker");
        let labels = LabellingCustomizer::labelled(base.labels.clone());
        base.with_labels(labels)
    };
    let created = mob
        .create_outcome(
            StoreOwnedIdentityCreate::new(
                operation("create-kicked"),
                IdentityChangeRequesterInput::host("console"),
                vec![CreatedIdentityRequest::new(
                    labelled("kicked")
                        .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
                        .with_initial_message(meerkat_core::types::ContentInput::from("Begin.")),
                )],
            ),
            ALLOW,
        )
        .await
        .expect("the create commits");
    assert!(
        matches!(
            created.embodied.get(&id("kicked")),
            Some(crate::runtime::DeclaredEntryOutcome::Embodied { .. })
        ),
        "a seat whose row carries the customizer's label is spawned: {created:?}"
    );
    kicked_off(&mob, "kicked", "Begin.").await;
    assert_eq!(
        mob.spawn_row("kicked").await.labels,
        labelled("kicked").labels,
        "the member carries its row's labels"
    );

    committed(
        mob.create(
            StoreOwnedIdentityCreate::new(
                operation("create-quiet"),
                IdentityChangeRequesterInput::host("console"),
                vec![CreatedIdentityRequest::new(labelled("quiet"))],
            ),
            ALLOW,
        )
        .await,
    );
    let session = mob.session("quiet").await;
    wake(&mob, "quiet")
        .await
        .expect("a seat whose row carries the customizer's label is built");
    assert_eq!(builds_on(&mob, &session).await.len(), 1, "one build");

    // A roster-declared member: seated Dormant from its declaration, built
    // at its first wake with the customizer's label on its build.
    let declared = mob.declare(&["roster"]).await;
    assert!(
        matches!(
            declared.entries.get(&id("roster")),
            Some(crate::runtime::DeclaredEntryOutcome::Seated { .. })
        ),
        "{declared:?}"
    );
    let session = mob.session("roster").await;
    wake(&mob, "roster")
        .await
        .expect("a roster-declared member's build is not compared");
    let builds = builds_on(&mob, &session).await;
    assert_eq!(builds.len(), 1, "one build");
    assert_eq!(
        builds[0]
            .peer_meta_labels
            .get(LabellingCustomizer::LABEL.0)
            .map(String::as_str),
        Some(LabellingCustomizer::LABEL.1),
        "the customizer labels the roster-declared member's build as before: {:?}",
        builds[0]
    );
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

/// The host's answer that the approver who decided is still current.
fn approver_current() -> meerkat_core::approval::consent::ConsentEligibilityAnswer {
    meerkat_core::approval::consent::ConsentEligibilityAnswer::Eligible(std::sync::Arc::new(
        Eligible,
    ))
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
        .authorize_execution(
            &held.decision_id,
            &operation_id,
            &request_digest,
            &approver_current(),
        )
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

/// Row (RED): the removal of an unseated store-owned identity ends the
/// identity at once (its name is tombstoned), and its live scoped tool
/// grants end with it in the removal's step.
#[tokio::test]
async fn the_removal_of_an_unseated_identity_ends_its_grants() {
    let mob = StoreOwnedMob::new("remove-unseated-grants").await;
    let create = StoreOwnedIdentityCreate::new(
        operation("create-unseated"),
        IdentityChangeRequesterInput::host("console"),
        vec![
            CreatedIdentityRequest::new(spec("alpha", "worker"))
                .with_preparation(IdentityPreparationMode::Required),
        ],
    );
    committed(mob.create(create, ALLOW).await);
    mob.unseated_for("alpha", UnseatedEntryCause::PreparationPending)
        .await;
    let runtime = mob.service.enable_runtime_adapter();
    let subject = meerkat_core::tool_grant::ToolGrantSubject::member(
        mob.mob_id().as_str(),
        &mob.handle.live_incarnation(),
        "alpha",
    );
    let grant = runtime
        .grant_tool(
            meerkat_runtime::tool_grant::ToolGrantRequest {
                subject: subject.clone(),
                target: meerkat_core::tool_grant::ToolGrantTarget::Family {
                    family: "shell".to_string(),
                    names: std::iter::once(meerkat_core::types::ToolName::from("shell")).collect(),
                },
                issuer: meerkat_core::tool_grant::ToolGrantIssuer::Operator {
                    caller: None,
                    decision_ref: None,
                },
                reason: None,
                window: meerkat_runtime::tool_grant::ToolGrantWindow::For(Duration::from_mins(10)),
                run_scope: None,
            },
            None,
        )
        .await
        .expect("the identity is granted");

    committed(mob.remove("remove-alpha", "alpha").await);
    mob.removed("alpha").await;
    let listing = runtime
        .list_tool_grants(&subject, None)
        .await
        .expect("the identity's grants list");
    assert_eq!(listing.grants.len(), 1, "{listing:?}");
    assert_eq!(listing.grants[0].record.grant_id, grant.record.grant_id);
    assert_eq!(
        listing.grants[0].status,
        meerkat_runtime::tool_grant::ToolGrantStatus::Revoked,
        "the removal ended the identity's grant"
    );
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

/// Row: consent decided outside Meerkat commits only on the host
/// operator's import: a host surface that is not the operator is refused
/// `import_refused` with nothing written; the operator's change commits,
/// its row's provenance and its receipt sealing the imported consent.
#[tokio::test]
async fn imported_consent_commits_only_for_the_host_operator_and_is_sealed_in_the_row_and_receipt()
{
    use meerkat_core::approval::decision::{ConsentProvenance, DecisionReference};
    let mob = StoreOwnedMob::new("imported-consent").await;
    let imported = ConsentProvenance::Imported {
        source: DecisionReference::new("ticketing").expect("the import's source"),
        reference: DecisionReference::new("ticket-7").expect("the import's reference"),
        approver_id: None,
        decided_at_ms: None,
    };
    let import = |requester| {
        StoreOwnedIdentityCreate::new(
            operation("import-alpha"),
            requester,
            vec![CreatedIdentityRequest::new(spec("alpha", "worker"))],
        )
    };
    let journal = mob.journal().await.len();
    assert_eq!(
        refusal(
            mob.create(
                import(IdentityChangeRequesterInput::host("console")),
                IdentityChangeConsentInput::Imported(&imported),
            )
            .await
        ),
        IdentityChangeRefusal::ImportRefused
    );
    assert!(mob.rows().await.is_empty(), "no row");
    assert_eq!(mob.journal().await.len(), journal, "no journal row");
    assert_eq!(
        mob.handle
            .identity_operation("import-alpha")
            .await
            .expect("the receipt read answers"),
        None,
        "no receipt"
    );

    let receipt = committed(
        mob.create(
            import(IdentityChangeRequesterInput::host_operator("console")),
            IdentityChangeConsentInput::Imported(&imported),
        )
        .await,
    );
    let sealed = IdentityChangeConsent::Imported {
        provenance: imported.clone(),
    };
    assert_eq!(receipt.consent, sealed, "the receipt seals the import");
    assert_eq!(
        mob.row("alpha").await.provenance.consent,
        sealed,
        "the row seals the import"
    );
    assert!(mob.seated_dormant("alpha"));
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

/// A mob over a host grant mint the row revokes.
async fn revocable_mob(label: &str) -> (StoreOwnedMob, Arc<RevocableMint>) {
    let mint = Arc::new(RevocableMint::default());
    let mint_for_builder: Arc<dyn MemberGrantMint> = mint.clone();
    let mob = StoreOwnedMob::build(
        store_owned_definition(label),
        MobStorage::in_memory(),
        move |builder| builder.with_member_grant_mint(mint_for_builder),
    )
    .await;
    (mob, mint)
}

/// Row: the recheck of a decided change prepares its check again at the
/// commit (the change waited for its decision): a grant the host revoked
/// meanwhile ends the change `grant_revoked` under its permit, and nothing
/// is written.
///
/// A coverage row for behaviour that stands before this change (no row
/// drove the decided branch of the sink's check): it passes at its RED
/// commit.
#[tokio::test]
async fn a_grant_revoked_while_a_decided_change_waits_ends_it_grant_revoked_under_its_permit() {
    let (mob, mint) = revocable_mob("grant-revoked-decided").await;
    let session = mob.spawn_requester("requester").await;
    let child = spec("child", "worker");
    let grant = mob
        .admit(&session, "create-child", std::slice::from_ref(&child))
        .await
        .expect("the grant admits the change");
    let now = now_ms();
    let permit = decided_permit(&operation("create-child"), now, now + 600_000);
    mint.revoke();
    let journal = mob.journal().await.len();
    assert_eq!(
        refusal(
            mob.create(
                StoreOwnedMob::agent_create("create-child", grant, vec![child]),
                IdentityChangeConsentInput::Decided(&permit),
            )
            .await
        ),
        IdentityChangeRefusal::GrantRevoked
    );
    assert_eq!(mob.journal().await.len(), journal);
    assert!(mob.rows().await.is_empty());
    assert!(!mob.is_member("child").await);
}

/// Row: a dormant requester's revoked grant ends its change
/// `grant_revoked` without waking it: the requester stays unmaterialized,
/// no session is built, and nothing is written.
///
/// A coverage row for behaviour that stands before this change (the
/// dormant row's change commits, so it passes even with no recheck): it
/// passes at its RED commit.
#[tokio::test]
async fn a_dormant_requesters_revoked_grant_ends_its_change_without_waking_it() {
    let (mob, mint) = revocable_mob("grant-revoked-dormant").await;
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
    mint.revoke();
    let builds = mob.builds().await;
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
    assert!(
        mob.handle.member_unmaterialized(&id("requester")),
        "the recheck woke nothing"
    );
    assert_eq!(mob.builds().await, builds, "no session was built");
    assert!(mob.rows().await.is_empty());
}

/// Row: a grant the host revokes before the change commits ends an update,
/// a removal, an archive and a settle `grant_revoked`, as it ends a create,
/// with nothing written: each row keeps its revision and its presence, and
/// the journal is unchanged.
///
/// A coverage row for behaviour that stands before this change: it passes
/// at its RED commit.
#[tokio::test]
async fn a_grant_revoked_before_an_update_a_removal_an_archive_or_a_settle_ends_it_grant_revoked() {
    let (mob, mint) = revocable_mob("grant-revoked-changes").await;
    committed(
        mob.create(
            host_create("create-targets", &["updated", "removed", "archived"]),
            ALLOW,
        )
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
    let session = mob.spawn_requester("requester").await;
    let moved = spec("updated", "worker").with_display_name("The moved identity");
    let worker = IdentityGrantRequest::for_identity("worker");
    let update_grant = mob
        .admit_for(
            &session,
            "update-updated",
            &IdentityGrantRequest::for_update(&ProfileName::from("worker"), &moved),
        )
        .await;
    let removal_grant = mob.admit_for(&session, "remove-removed", &worker).await;
    let archive_grant = mob.admit_for(&session, "archive-archived", &worker).await;
    let settle_grant = mob.admit_for(&session, "settle-pending", &worker).await;
    mint.revoke();
    let journal = mob.journal().await.len();

    let update = crate::runtime::StoreOwnedIdentityUpdate::new(
        operation("update-updated"),
        IdentityChangeRequesterInput::Agent {
            grant: update_grant,
        },
        moved,
        spec_digest(&spec("updated", "worker")),
    );
    assert_eq!(
        refusal(mob.handle.update_store_owned_identity(update, ALLOW).await),
        IdentityChangeRefusal::GrantRevoked
    );
    let removal = crate::runtime::IdentityRemoval::new(
        operation("remove-removed"),
        IdentityChangeRequesterInput::Agent {
            grant: removal_grant,
        },
        "removed",
    );
    assert_eq!(
        refusal(mob.handle.remove_identity(removal, ALLOW).await),
        IdentityChangeRefusal::GrantRevoked
    );
    let archive = crate::runtime::IdentityArchiveChange::new(
        operation("archive-archived"),
        IdentityChangeRequesterInput::Agent {
            grant: archive_grant,
        },
        "archived",
    );
    assert_eq!(
        refusal(mob.handle.archive_identity(archive, ALLOW).await),
        IdentityChangeRefusal::GrantRevoked
    );
    let settle = crate::runtime::IdentityPreparationSettle::new(
        operation("settle-pending"),
        IdentityChangeRequesterInput::Agent {
            grant: settle_grant,
        },
        "pending",
    );
    assert_eq!(
        refusal(mob.handle.settle_identity_preparation(settle, ALLOW).await),
        IdentityChangeRefusal::GrantRevoked
    );

    assert_eq!(mob.journal().await.len(), journal);
    assert_eq!(mob.row("updated").await.revision, 1);
    for name in ["removed", "archived"] {
        assert_eq!(
            mob.row(name).await.presence,
            CreatedIdentityPresence::Present,
            "{name}"
        );
        assert!(mob.seated_dormant(name), "{name}");
    }
    assert!(matches!(
        mob.row("pending").await.presence,
        CreatedIdentityPresence::PendingPreparation { .. }
    ));
    mob.unseated_for("pending", UnseatedEntryCause::PreparationPending)
        .await;
}

/// Row (RED): an approved change of a requester a restart left dormant
/// executes without waking it: the executor rebuilds the change's grant
/// from the recorded requester identity
/// (`MobHandle::identity_change_grant_for`), reading no session, and
/// commits under the decision's permit. The requester stays
/// unmaterialized, and no session is built.
///
/// RED on the old tree: no grant can be rebuilt from an identity (the
/// rebuild answers an internal error).
#[tokio::test]
async fn an_approved_change_of_a_dormant_requester_executes_after_a_restart_without_waking_it() {
    let mut mob = StoreOwnedMob::new("grant-after-restart").await;
    let session = mob.spawn_requester("requester").await;
    let child = spec("child", "worker");
    let request = IdentityGrantRequest::for_specs(std::slice::from_ref(&child));
    // Admitted, the change waits for its decision; the grant is
    // process-local and goes with the restart.
    mob.admit_for(&session, "create-child", &request).await;
    let now = now_ms();
    let permit = decided_permit(&operation("create-child"), now, now + 600_000);

    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    assert!(
        mob.handle.member_unmaterialized(&id("requester")),
        "the restart left the requester dormant"
    );
    let builds = mob.builds().await;
    let grant = mob
        .grant_for("requester", "create-child", &request)
        .await
        .expect("the executor rebuilds the grant from the requester identity");
    assert_eq!(grant.requester(), &id("requester"));
    committed(
        mob.create(
            StoreOwnedMob::agent_create("create-child", grant, vec![child]),
            IdentityChangeConsentInput::Decided(&permit),
        )
        .await,
    );
    assert!(mob.seated_dormant("child"));
    assert!(
        mob.handle.member_unmaterialized(&id("requester")),
        "the re-admission and the commit woke nothing"
    );
    assert_eq!(mob.builds().await, builds, "no session was built");
}

/// Row (RED): the re-admission reads the requester identity's current
/// grant: one its host revoked while the change waited across a restart is
/// refused `grant_revoked`, the requester still dormant, no session built
/// and nothing written.
///
/// RED on the old tree: the rebuild answers an internal error.
#[tokio::test]
async fn a_revoked_grant_of_a_dormant_requester_is_refused_grant_revoked_after_a_restart() {
    let (mut mob, mint) = revocable_mob("grant-revoked-after-restart").await;
    let session = mob.spawn_requester("requester").await;
    let child = spec("child", "worker");
    let request = IdentityGrantRequest::for_specs(std::slice::from_ref(&child));
    mob.admit_for(&session, "create-child", &request).await;
    let mint_for_resume: Arc<dyn MemberGrantMint> = mint.clone();
    mob.restart_configured(MobCompositionAuthority::Authoritative, move |builder| {
        builder.with_member_grant_mint(mint_for_resume)
    })
    .await;
    mint.revoke();
    let builds = mob.builds().await;
    assert_eq!(
        refusal(mob.grant_for("requester", "create-child", &request).await),
        IdentityChangeRefusal::GrantRevoked
    );
    assert!(mob.handle.member_unmaterialized(&id("requester")));
    assert_eq!(mob.builds().await, builds, "no session was built");
    assert!(!mob.rows().await.contains_key(&id("child")));
}

/// Row (RED): a grant binds its operation's request: committed under the
/// same operation id with another request digest, the change is refused
/// `invalid_request`, and nothing is written.
///
/// RED on the old tree: the commit compares only the operation id, and
/// the change commits.
#[tokio::test]
async fn a_grant_admitted_for_another_request_digest_is_refused_invalid_request() {
    let mob = StoreOwnedMob::new("grant-other-digest").await;
    let session = mob.spawn_requester("requester").await;
    let child = spec("child", "worker");
    let grant = mob
        .admit(&session, "create-child", std::slice::from_ref(&child))
        .await
        .expect("the grant admits the change");
    let create = StoreOwnedIdentityCreate::new(
        IdentityOperation::new("create-child", digest("another request")),
        IdentityChangeRequesterInput::Agent { grant },
        vec![CreatedIdentityRequest::new(child)],
    );
    assert!(matches!(
        refusal(mob.create(create, ALLOW).await),
        IdentityChangeRefusal::InvalidRequest { .. }
    ));
    assert!(mob.rows().await.is_empty());
    assert!(!mob.is_member("child").await);
}

/// Row (RED): a create commits only what its grant's admission checked: a
/// spec of a profile the admitted request does not name is refused
/// `invalid_request`, and nothing is written.
///
/// RED on the old tree: the commit checks the grant against the admitted
/// request only, and the other profile's identity is created.
#[tokio::test]
async fn a_create_outside_its_admitted_request_is_refused_invalid_request() {
    let mob = StoreOwnedMob::new("grant-create-outside").await;
    let session = mob.spawn_requester("requester").await;
    let admitted = spec("child", "worker");
    let grant = mob
        .admit(&session, "create-child", std::slice::from_ref(&admitted))
        .await
        .expect("the grant admits the change");
    assert!(matches!(
        refusal(
            mob.create(
                StoreOwnedMob::agent_create("create-child", grant, vec![spec("child", "lead")]),
                ALLOW,
            )
            .await
        ),
        IdentityChangeRefusal::InvalidRequest { .. }
    ));
    assert!(mob.rows().await.is_empty());
    assert!(!mob.is_member("child").await);
}

/// Row (RED): an update commits only what its grant's admission checked
/// (the identity's current profile and its new spec): an update to a
/// profile the admitted request does not name is refused
/// `invalid_request`, the row at its revision and the member on its role.
///
/// RED on the old tree: the update commits, migrating the identity to the
/// other profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_outside_its_admitted_request_is_refused_invalid_request() {
    let mob = StoreOwnedMob::new("grant-update-outside").await;
    committed(
        mob.create(host_create("create-target", &["target"]), ALLOW)
            .await,
    );
    let session = mob.spawn_requester("requester").await;
    let admitted = spec("target", "worker").with_display_name("The renamed identity");
    let grant = mob
        .admit_for(
            &session,
            "update-target",
            &IdentityGrantRequest::for_update(&ProfileName::from("worker"), &admitted),
        )
        .await;
    let update = crate::runtime::StoreOwnedIdentityUpdate::new(
        operation("update-target"),
        IdentityChangeRequesterInput::Agent { grant },
        spec("target", "lead"),
        spec_digest(&spec("target", "worker")),
    );
    assert!(matches!(
        refusal(
            tokio::time::timeout(STEP, mob.handle.update_store_owned_identity(update, ALLOW))
                .await
                .expect("the update answers")
        ),
        IdentityChangeRefusal::InvalidRequest { .. }
    ));
    assert_eq!(mob.row("target").await.revision, 1);
    assert_eq!(mob.role("target").await, Some(ProfileName::from("worker")));
}

/// Row (RED): a removal and an archive commit only against the target's
/// current profile, as their grant's admission checked it: a grant
/// admitted for another profile is refused `invalid_request`, with the
/// identity's row and member untouched. An agent's removal of a
/// roster-owned identity no member holds is refused the same way: Core
/// holds no profile of it to check the grant against (its host's roster
/// holds its spec). Admitted for the target's profile, the removal
/// commits.
///
/// RED on the old tree: the commits check the grant against the admitted
/// request only, and each change commits.
#[tokio::test]
async fn a_removal_or_archive_of_another_profile_than_admitted_is_refused_invalid_request() {
    let mob = StoreOwnedMob::new("grant-target-profile").await;
    committed(
        mob.create(
            host_create("create-targets", &["removed", "archived"]),
            ALLOW,
        )
        .await,
    );
    tokio::time::timeout(
        STEP,
        mob.handle
            .apply_declared_roster(DeclaredRoster::partial([DeclaredEntry::new(roster_spec(
                "pending",
            ))
            .with_preparation(IdentityPreparationMode::Required)])),
    )
    .await
    .expect("the apply answers")
    .expect("the apply runs");
    mob.unseated_for("pending", UnseatedEntryCause::PreparationPending)
        .await;
    let session = mob.spawn_requester("requester").await;
    let lead = IdentityGrantRequest::for_identity("lead");
    let worker = IdentityGrantRequest::for_identity("worker");
    let agent = |grant| IdentityChangeRequesterInput::Agent { grant };

    let removal = crate::runtime::IdentityRemoval::new(
        operation("remove-removed"),
        agent(mob.admit_for(&session, "remove-removed", &lead).await),
        "removed",
    );
    assert!(matches!(
        refusal(mob.handle.remove_identity(removal, ALLOW).await),
        IdentityChangeRefusal::InvalidRequest { .. }
    ));
    let archive = crate::runtime::IdentityArchiveChange::new(
        operation("archive-archived"),
        agent(mob.admit_for(&session, "archive-archived", &lead).await),
        "archived",
    );
    assert!(matches!(
        refusal(mob.handle.archive_identity(archive, ALLOW).await),
        IdentityChangeRefusal::InvalidRequest { .. }
    ));
    for name in ["removed", "archived"] {
        assert_eq!(
            mob.row(name).await.presence,
            CreatedIdentityPresence::Present,
            "{name}"
        );
        assert!(mob.seated_dormant(name), "{name}");
    }
    let pending = crate::runtime::IdentityRemoval::new(
        operation("remove-pending"),
        agent(mob.admit_for(&session, "remove-pending", &worker).await),
        "pending",
    );
    let refused = refusal(mob.handle.remove_identity(pending, ALLOW).await);
    assert!(
        invalid_request_naming(&refused, "its host's roster holds its spec"),
        "the refusal names its reason: {refused:?}"
    );
    mob.unseated_for("pending", UnseatedEntryCause::PreparationPending)
        .await;
    assert!(!mob.rows().await.contains_key(&id("pending")));

    let removal = crate::runtime::IdentityRemoval::new(
        operation("remove-removed-again"),
        agent(
            mob.admit_for(&session, "remove-removed-again", &worker)
                .await,
        ),
        "removed",
    );
    committed(
        tokio::time::timeout(STEP, mob.handle.remove_identity(removal, ALLOW))
            .await
            .expect("the removal answers"),
    );
    mob.removed("removed").await;
}

/// Row (RED): a removal, an archive and an unarchive decided against a
/// spec the identity no longer holds are refused `stale_spec` with the
/// row's current digest and revision, and nothing is written; decided
/// against the current spec, they commit.
///
/// RED on the old tree: the changes carry no expected spec, and the stale
/// removal commits.
#[tokio::test]
async fn a_removal_archive_or_unarchive_decided_against_a_stale_spec_is_refused_with_the_current_one()
 {
    let mob = StoreOwnedMob::new("entry-change-stale").await;
    committed(
        mob.create(
            host_create("create-targets", &["removed", "archived"]),
            ALLOW,
        )
        .await,
    );
    let moved = |name: &str| spec(name, "worker").with_display_name(format!("The moved {name}"));
    for name in ["removed", "archived"] {
        mob.update(
            &format!("update-{name}"),
            moved(name),
            &spec_digest(&spec(name, "worker")),
        )
        .await
        .expect("the update commits");
    }
    let stale = |name: &str| spec_digest(&spec(name, "worker"));
    let stale_spec = |name: &str| IdentityChangeRefusal::StaleSpec {
        identity: id(name),
        current_digest: spec_digest(&moved(name)),
        current_revision: Some(2),
    };
    let host = || IdentityChangeRequesterInput::host("console");

    let removal =
        crate::runtime::IdentityRemoval::new(operation("remove-removed"), host(), "removed")
            .with_expected_spec_digest(stale("removed"));
    assert_eq!(
        refusal(mob.handle.remove_identity(removal, ALLOW).await),
        stale_spec("removed")
    );
    let archive = crate::runtime::IdentityArchiveChange::new(
        operation("archive-archived"),
        host(),
        "archived",
    )
    .with_expected_spec_digest(stale("archived"));
    assert_eq!(
        refusal(mob.handle.archive_identity(archive, ALLOW).await),
        stale_spec("archived")
    );
    for name in ["removed", "archived"] {
        assert_eq!(
            mob.row(name).await.presence,
            CreatedIdentityPresence::Present,
            "{name}"
        );
        assert!(mob.seated_dormant(name), "{name}");
    }

    let archive = crate::runtime::IdentityArchiveChange::new(
        operation("archive-archived-current"),
        host(),
        "archived",
    )
    .with_expected_spec_digest(spec_digest(&moved("archived")));
    committed(
        tokio::time::timeout(STEP, mob.handle.archive_identity(archive, ALLOW))
            .await
            .expect("the archive answers"),
    );
    let unarchive = crate::runtime::IdentityArchiveChange::new(
        operation("unarchive-archived"),
        host(),
        "archived",
    )
    .with_expected_spec_digest(stale("archived"));
    assert_eq!(
        refusal(mob.handle.unarchive_identity(unarchive, ALLOW).await),
        stale_spec("archived")
    );
    assert!(matches!(
        mob.row("archived").await.presence,
        CreatedIdentityPresence::Archived { .. }
    ));
    let removal = crate::runtime::IdentityRemoval::new(
        operation("remove-removed-current"),
        host(),
        "removed",
    )
    .with_expected_spec_digest(spec_digest(&moved("removed")));
    committed(
        tokio::time::timeout(STEP, mob.handle.remove_identity(removal, ALLOW))
            .await
            .expect("the removal answers"),
    );
}

/// Row (RED): an expected spec digest names a store-owned identity's row:
/// a roster-owned identity's removal naming one is refused
/// `invalid_request` (its host's roster checks its spec), its member kept
/// and no tombstone written.
///
/// RED on the old tree: the removal carries no expected spec, and it
/// commits.
#[tokio::test]
async fn a_roster_owned_removal_naming_a_spec_digest_is_refused_invalid_request() {
    let mob = StoreOwnedMob::new("roster-removal-digest").await;
    mob.declare(&["listed"]).await;
    let removal = crate::runtime::IdentityRemoval::new(
        operation("remove-listed"),
        IdentityChangeRequesterInput::host("console"),
        "listed",
    )
    .with_expected_spec_digest(digest("a roster spec"));
    let refused = refusal(mob.handle.remove_identity(removal, ALLOW).await);
    assert!(
        invalid_request_naming(&refused, "its host's roster checks its spec"),
        "the refusal names its reason: {refused:?}"
    );
    assert!(mob.is_member("listed").await);
    assert!(
        !mob.rows().await.contains_key(&id("listed")),
        "no tombstone was written"
    );
}

/// Row (RED): the receipt read needs the List scope, as the record reads
/// do: on the agent lane it is refused `ScopeDenied`, and the receipt (its
/// requester, its consent) is not read; the host reads it.
///
/// RED on the old tree: the read checks no scope and answers the receipt.
#[tokio::test]
async fn the_operation_read_is_refused_without_the_list_scope() {
    let mob = StoreOwnedMob::new("operation-read-scope").await;
    committed(
        mob.create(host_create("create-alpha", &["alpha"]), ALLOW)
            .await,
    );
    let denied = mob
        .handle
        .clone()
        .with_command_authority(crate::control_policy::CommandAuthority::agent_lane())
        .identity_operation("create-alpha")
        .await;
    assert!(
        matches!(
            &denied,
            Err(MobError::ScopeDenied(denial)) if denial.required == crate::ControlScope::List
        ),
        "{denied:?}"
    );
    assert!(
        mob.handle
            .identity_operation("create-alpha")
            .await
            .expect("the host reads the receipt")
            .is_some()
    );
}

// ---------------------------------------------------------------------------
// Source guards: origin is read only by the custody guards, and the grant
// recheck has no second mechanism
// ---------------------------------------------------------------------------

/// The shipped items that may read an identity entry's origin, by file:
/// the custody guards (the recovery source, the writers, the name guards,
/// the reservations and the host's seat composition) and the reads that
/// show it. Each item is named by
/// the start of its signature, and only its body may read the origin; a
/// method by its impl block's header and its signature, joined by ` > `,
/// so a new method of the impl (an `is_created()`) is not authorized in
/// advance. A file with no shipped read has no entry: a whole-file entry
/// would authorize in advance exactly the branch on custody this guard
/// forbids.
const ORIGIN_READERS: &[(&str, &[&str], &str)] = &[
    (
        "runtime/identity_entry.rs",
        &[
            "impl IdentityEntryOrigin { > pub(crate) fn from_machine(",
            "impl IdentityEntryOrigin { > pub(crate) fn to_machine(",
            "impl IdentityEntryOrigin { > pub(crate) fn as_stored(",
            "impl IdentityEntryOrigin { > pub(crate) fn from_stored(",
            "pub(crate) fn identity_census(",
        ],
        "the origin's machine conversions and journal token, and the census, which shows it",
    ),
    (
        "runtime/created_identity.rs",
        &[
            "fn owed_created_seats(",
            "pub(super) fn host_composes_seat(",
        ],
        "the recovery source: the store-owned entries owed their seat; and the builds whose \
         seat the host composes, a store-owned identity's, which Core seats from its row",
    ),
    (
        "runtime/handle.rs",
        &["pub(super) async fn adopt_listed_legacy_member("],
        "the legacy adoption's custody guard",
    ),
    (
        "runtime/actor/declared_apply.rs",
        &["pub(super) async fn declare_identity_entries("],
        "the roster writer's custody guard of a store-owned update",
    ),
    (
        "roster.rs",
        &["impl Roster { > pub fn apply("],
        "the roster's fold of an adoption's binding row: its Created origin makes the member a \
         host root, live and on replay",
    ),
];

/// The body of an origin reader in `code`: the item `signature` names, or,
/// for `impl header > method signature`, that method inside the impl block.
fn origin_reader_body(code: &str, item: &str) -> Option<std::ops::Range<usize>> {
    use super::fail_stop_cause::{brace_group_end, function_body};
    let Some((scope, method)) = item.split_once(" > ") else {
        return function_body(code, item);
    };
    let block = function_body(code, scope)?;
    let start = block.start + code[block.clone()].find(method)?;
    let open = start + code[start..block.end].find('{')?;
    Some(open..brace_group_end(code, open))
}

/// The byte offsets of every read of an origin value in `code`: a path to
/// `IdentityEntryOrigin::Created` or `::Declared`; `Self::Created` or
/// `Self::Declared` in the type's own impl blocks; a `use` that renames the
/// type or imports its variants (`as`, braces or a glob), which would hide
/// every later read from the name; and a stored token compared or matched
/// (`as_stored()` in an expression, between the statement or block
/// boundaries around it, that compares, matches or is a `match` scrutinee,
/// split across lines or not: the token's type is not visible to the scan,
/// so any such use counts).
fn origin_value_reads(code: &str) -> Vec<usize> {
    use super::fail_stop_cause::{brace_group_end, whole_identifier};
    const NAME: &str = "IdentityEntryOrigin";
    let mut reads = Vec::new();
    for (at, _) in code.match_indices(NAME) {
        if !whole_identifier(code, at, NAME) {
            continue;
        }
        let rest = code[at + NAME.len()..].trim_start();
        if rest.starts_with("::Created") || rest.starts_with("::Declared") {
            reads.push(at);
        }
        // The type's own impl block (`impl IdentityEntryOrigin` or
        // `impl ... for IdentityEntryOrigin`): its `Self::` variants.
        let before = code[..at].trim_end();
        if (before.ends_with("impl") || before.ends_with("for")) && rest.starts_with('{') {
            let open = code.len() - rest.len();
            let end = brace_group_end(code, open);
            for variant in ["Self::Created", "Self::Declared"] {
                reads.extend(
                    code[open..end]
                        .match_indices(variant)
                        .map(|(offset, _)| open + offset)
                        .filter(|index| whole_identifier(code, *index, variant)),
                );
            }
        }
    }
    for (use_at, _) in code.match_indices("use") {
        if !whole_identifier(code, use_at, "use") {
            continue;
        }
        let Some(end) = code[use_at..].find(';').map(|offset| use_at + offset) else {
            continue;
        };
        for (offset, _) in code[use_at..end].match_indices(NAME) {
            let at = use_at + offset;
            if !whole_identifier(code, at, NAME) {
                continue;
            }
            let rest = code[at + NAME.len()..end].trim_start();
            let renamed = rest
                .strip_prefix("as")
                .is_some_and(|after| after.starts_with(char::is_whitespace));
            if renamed || rest.starts_with("::{") || rest.starts_with("::*") {
                reads.push(at);
            }
        }
    }
    for (at, _) in code.match_indices(".as_stored()") {
        let start = code[..at]
            .rfind([';', '{', '}'])
            .map_or(0, |index| index + 1);
        let end = code[at..]
            .find([';', '{', '}'])
            .map_or(code.len(), |index| at + index);
        let expression = &code[start..end];
        let scrutinee = expression
            .match_indices("match")
            .any(|(offset, _)| whole_identifier(expression, offset, "match"));
        if expression.contains("==")
            || expression.contains("!=")
            || expression.contains("matches!")
            || scrutinee
        {
            reads.push(at);
        }
    }
    reads.sort_unstable();
    reads.dedup();
    reads
}

/// Row (source guard; RED leg: a scratch reader of
/// `IdentityEntryOrigin::Created`, `Self::Created` in its impl, a renamed or
/// glob import of it, or a compared stored token, added to any shipped item
/// that is not an origin reader, fails it): no code reads an identity's
/// origin outside the custody guards and the reads that show it. Custody
/// decides the recovery source, the writer, the name guards, the
/// reservations and whether the host composes a member's seat at its
/// builds, nothing else.
///
/// Hardened: readers are listed per item, not per file, and the origin's
/// impl per method; the files with no shipped read (the journal codec, the
/// store-owned writer and the machine's own module) have no entry; renamed,
/// brace and glob imports, `Self::` variants and stored tokens compared or
/// matched (on one line or across lines) count as reads.
/// `the_origin_scan_sees_each_read_form` pins each form. The DSL's reads
/// are guarded where the DSL lives, by meerkat-machine-schema's
/// `identity_entry_origin_is_read_only_by_the_dsl_custody_guards`
/// (`catalog/dsl/mob_machine.rs`), which reads its own source.
#[test]
fn identity_entry_origin_is_read_only_by_the_custody_guards() {
    use super::fail_stop_cause::{line_of, shipped_sources};
    let mut strays = Vec::new();
    for (path, code) in shipped_sources() {
        let reads = origin_value_reads(&code);
        if reads.is_empty() {
            continue;
        }
        let allowed: Vec<std::ops::Range<usize>> = ORIGIN_READERS
            .iter()
            .filter(|(reader, ..)| *reader == path)
            .flat_map(|(_, items, _)| items.iter())
            .map(|item| {
                origin_reader_body(&code, item)
                    .unwrap_or_else(|| panic!("{path}: the origin reader `{item}` is gone"))
            })
            .collect();
        strays.extend(
            reads
                .iter()
                .filter(|at| !allowed.iter().any(|body| body.contains(*at)))
                .map(|at| format!("{path}:{}", line_of(&code, *at))),
        );
    }
    assert!(
        strays.is_empty(),
        "an identity's origin is read outside the custody guards: {strays:?}"
    );
}

/// Row (the origin guard's own self-test): the scan finds each form of an
/// origin read, on exactly its line, and nothing else; and a reader's body
/// is its listed method's, not its impl block's. A scan branch that broke
/// would otherwise let the guard pass silently. The DSL scan's verdicts are
/// pinned where the DSL lives, by meerkat-machine-schema's
/// `the_dsl_origin_scan_allows_only_reads_inside_a_custody_guard`.
#[test]
fn the_origin_scan_sees_each_read_form() {
    use super::fail_stop_cause::{line_of, shipped_code};
    let code = shipped_code(
        "use crate::runtime::IdentityEntryOrigin;\n\
         use crate::runtime::IdentityEntryOrigin as Origin;\n\
         use crate::runtime::IdentityEntryOrigin::{Created};\n\
         use crate::runtime::IdentityEntryOrigin::*;\n\
         fn custody(origin: IdentityEntryOrigin) -> bool {\n\
             let token = origin.as_stored();\n\
             let created = origin == IdentityEntryOrigin::Created;\n\
             if origin.as_stored() == \"created\" {}\n\
             if origin.as_stored()\n\
                 != \"declared\"\n\
             {}\n\
             match origin.as_stored() {\n\
                 _ => {}\n\
             }\n\
             matches!(token, \"created\") && created\n\
         }\n\
         impl IdentityEntryOrigin {\n\
             fn listed(self) -> bool {\n\
                 self == Self::Declared\n\
             }\n\
             fn is_created(self) -> bool {\n\
                 matches!(self, Self::Created)\n\
             }\n\
         }\n",
    );
    let lines = |reads: Vec<usize>| -> Vec<usize> {
        reads.into_iter().map(|at| line_of(&code, at)).collect()
    };
    assert_eq!(
        lines(origin_value_reads(&code)),
        [2, 3, 4, 7, 8, 9, 12, 19, 22],
        "the renamed, brace and glob imports, a variant's path, a stored token \
         compared on one line, across lines and as a match scrutinee, and the \
         impl's Self:: variants; not the plain import, the parameter, the \
         token's binding or a match on a binding"
    );

    // A method listed by its impl is the reader: the impl's other methods
    // are not.
    let listed = origin_reader_body(&code, "impl IdentityEntryOrigin { > fn listed(")
        .expect("the listed method is found inside its impl");
    let reads_in = |body: &std::ops::Range<usize>| -> Vec<usize> {
        lines(
            origin_value_reads(&code)
                .into_iter()
                .filter(|at| body.contains(at))
                .collect(),
        )
    };
    assert_eq!(reads_in(&listed), [19]);
    assert!(
        origin_reader_body(&code, "impl IdentityEntryOrigin { > fn unlisted(").is_none(),
        "a method the impl does not hold is no reader"
    );
}

/// Row (source guard): the recheck of an agent requester's grant runs
/// through the change's prepared check at the commit, its final sink, and
/// through nothing else. The commit calls `current` on the check (a decided
/// change's `reprepared`); the runtime's receiver, which reads a session's
/// live grant, is read at the admission only; the identity's current grant
/// is computed only by the check's owner; and a grant is minted only by the
/// admission and the executor's re-admission
/// (`identity_change_grant_for`), both through the one preparation of the
/// change's check, whose owner reads the requester identity, never its
/// session.
///
/// Updated with the re-admission: the grant's mint sites and the
/// owner's one construction are now listed.
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
    let identity_change_grants = [
        "pub async fn admit_identity_change_grant(",
        "pub async fn identity_change_grant_for(",
    ];
    for (path, code) in &sources {
        for (mint, allowed_in) in [
            ("IdentityChangeGrant::new(", &identity_change_grants[..]),
            (
                "IdentityChangeGrantOwner {",
                &["async fn prepare_identity_change_check("][..],
            ),
        ] {
            for (at, _) in code.match_indices(mint) {
                let declaration = code[..at].trim_end().ends_with("struct")
                    || code[..at].trim_end().ends_with("impl")
                    || code[..at].trim_end().ends_with("for");
                if declaration {
                    continue;
                }
                let allowed = path == "runtime/identity_change.rs"
                    && allowed_in.iter().any(|function| {
                        function_body(code, function).is_some_and(|body| body.contains(&at))
                    });
                if !allowed {
                    strays.push(format!("{path}:{} {mint}", line_of(code, at)));
                }
            }
        }
    }
    assert!(
        strays.is_empty(),
        "a grant read outside the admission and the prepared check's owner, or minted \
         outside the admission and the re-admission: {strays:?}"
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

/// Row: while a roster-owned identity's removal retires
/// its member, the name's tombstone is already written (the removal's start
/// transaction), and a declared apply that lists the identity defers it,
/// seating and retiring nothing; once the removal ended, the roster's
/// listing of the name is refused as a removed name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declared_apply_during_a_roster_owned_removal_defers_the_identity_its_tombstone_written()
{
    let mob = StoreOwnedMob::new("remove-roster-deferred").await;
    let name = unique("listed");
    mob.declare(&[name.as_str()]).await;
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
                        operation("remove-listed"),
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
    assert_eq!(
        mob.row(&name).await.presence,
        CreatedIdentityPresence::Removed,
        "the tombstone is written at the removal's start"
    );
    let during = mob.declare(&[name.as_str()]).await;
    assert!(
        matches!(
            during.entries.get(&id(&name)),
            Some(crate::runtime::DeclaredEntryOutcome::Deferred { .. })
        ),
        "{during:?}"
    );
    hold.release();
    committed(
        tokio::time::timeout(STEP, removal)
            .await
            .expect("the removal answers")
            .expect("the removal task"),
    );
    mob.removed(&name).await;
    let after = mob.declare(&[name.as_str()]).await;
    assert!(
        matches!(
            after.entries.get(&id(&name)),
            Some(crate::runtime::DeclaredEntryOutcome::Refused { .. })
        ),
        "{after:?}"
    );
    assert!(!mob.is_member(&name).await);
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

/// Wait, within one step, until `name` is no member of `mob`.
async fn left(mob: &StoreOwnedMob, name: &str) {
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.is_member(name).await {
        assert!(tokio::time::Instant::now() < deadline, "{name} never left");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Row (RED): the one-entry unlist reads and changes no other entry: an
/// entry the legacy adoption records while the unlist waits for its step
/// stays listed, its member seated, and the unlist answers only its own
/// identity.
///
/// RED on the old tree: the unlist seals a roster built from the entries
/// published before its step, which omits the adopted one, so the seal
/// unlists it too and retires its member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entry_adopted_while_an_unlist_waits_is_not_unlisted() {
    let mob = StoreOwnedMob::new("unlist-concurrent").await;
    mob.declare(&["listed"]).await;
    let mut legacy = SpawnMemberSpec::new("worker", id("legacy"));
    legacy.labels = Some(BTreeMap::from([(
        crate::runtime::identity_entry::LEGACY_IDENTITY_LABEL.to_string(),
        "legacy".to_string(),
    )]));
    mob.handle
        .spawn_spec(legacy)
        .await
        .expect("a legacy member");

    let hold =
        super::super::identity_change::hold_next_change_step_for_test(&mob.mob_id(), "unlist");
    let handle = mob.handle.clone();
    let unlist = tokio::spawn(async move { handle.unlist_declared_identity(&id("listed")).await });
    tokio::time::timeout(STEP, hold.entered)
        .await
        .expect("the unlist reaches its step")
        .expect("the hold is armed");
    assert_eq!(
        mob.handle
            .adopt_legacy_identity_entry(&id("legacy"))
            .await
            .expect("the adoption answers"),
        crate::runtime::LegacyIdentityEntryAdoption::Adopted
    );
    let _ = hold.release.send(());
    let unlisted = tokio::time::timeout(STEP, unlist)
        .await
        .expect("the unlist answers")
        .expect("the unlist task completes")
        .expect("the unlist runs");
    assert!(
        unlisted.unlisted.contains_key(&id("listed")),
        "{unlisted:?}"
    );
    assert!(
        !unlisted.unlisted.contains_key(&id("legacy")),
        "the adopted entry is not unlisted: {unlisted:?}"
    );
    left(&mob, "listed").await;
    assert!(
        mob.is_member("legacy").await,
        "the adopted entry's member stays"
    );
    assert!(
        matches!(
            mob.read("legacy"),
            IdentityEntryRead::Entry(view)
                if view.origin == IdentityEntryOrigin::Declared
                    && matches!(view.standing, EntryStanding::Seated(_))
        ),
        "{:?}",
        mob.read("legacy")
    );
}

/// Row (RED): an unlisted roster-owned identity's archived session record
/// holds its name: a store-owned create of the name is refused
/// `name_taken {by: identity}` and writes nothing, and listing the identity
/// again seats it on its own session.
///
/// RED on the old tree: the create takes the name and lets the record go.
#[tokio::test]
async fn a_create_of_an_unlisted_roster_identitys_name_is_refused_name_taken() {
    let mob = StoreOwnedMob::new("unlisted-name").await;
    mob.declare(&["listed"]).await;
    let session = mob.session("listed").await;
    tokio::time::timeout(STEP, mob.handle.unlist_declared_identity(&id("listed")))
        .await
        .expect("the unlist answers")
        .expect("the unlist runs");
    left(&mob, "listed").await;
    assert_eq!(
        refusal(
            mob.create(host_create("create-listed", &["listed"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::NameTaken {
            identity: id("listed"),
            by: IdentityNameHolder::Identity,
        }
    );
    assert!(mob.rows().await.is_empty(), "nothing is written");
    assert_eq!(
        mob.handle
            .identity_operation("create-listed")
            .await
            .expect("read the receipt"),
        None
    );
    let relisted = mob.declare(&["listed"]).await;
    assert!(
        matches!(
            relisted.entries.get(&id("listed")),
            Some(crate::runtime::DeclaredEntryOutcome::Seated { session_id, .. })
                if *session_id == session
        ),
        "the re-list restores the identity on its own session: {relisted:?}"
    );
}

/// Row: an unarchive whose archived session is gone from the store
/// seats the identity on a fresh session with the notice: the seat's row
/// carries `history_unavailable` naming the lost session, and the entry
/// lets its archived session record go.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unarchive_whose_archived_session_is_gone_seats_a_fresh_session_with_the_notice() {
    let mob = StoreOwnedMob::new("unarchive-history-lost").await;
    committed(
        mob.create(host_create("create-kept", &["kept"]), ALLOW)
            .await,
    );
    mob.handle
        .ensure_member_live(&id("kept"))
        .await
        .expect("the wake builds the identity's session");
    let archived = mob.session("kept").await;
    committed(mob.archive("archive-kept", "kept").await);
    mob.unseated_for("kept", UnseatedEntryCause::Archived).await;
    // The store's retention purges the archived session.
    mob.service.delete_persisted_session(&archived).await;

    committed(mob.unarchive("unarchive-kept", "kept").await);
    assert!(mob.seated_dormant("kept"));
    let fresh = mob.session("kept").await;
    assert_ne!(fresh, archived, "a fresh session");
    assert_eq!(
        mob.spawn_row("kept").await.history_unavailable,
        Some(crate::runtime::HistoryUnavailable::new(archived)),
        "the seat's row carries the notice"
    );
    match mob.read("kept") {
        IdentityEntryRead::Entry(view) => assert_eq!(view.archived_session, None),
        other => panic!("kept reads {other:?}"),
    }
}

/// Row: an update of an archived identity changes its row only, and
/// its unarchive seats the identity from the updated spec: the seat carries
/// the update's labels.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_of_an_archived_identity_is_applied_at_its_unarchive() {
    let mob = StoreOwnedMob::new("unarchive-updated").await;
    let first = spec("resting", "worker");
    committed(
        mob.create(host_create("create-resting", &["resting"]), ALLOW)
            .await,
    );
    committed(mob.archive("archive-resting", "resting").await);
    mob.unseated_for("resting", UnseatedEntryCause::Archived)
        .await;
    let mut labels = first.labels.clone();
    labels.insert("tier".to_string(), "gold".to_string());
    let updated = mob
        .update(
            "update-resting",
            first.clone().with_labels(labels.clone()),
            &spec_digest(&first),
        )
        .await
        .expect("the update of the archived identity commits");
    assert!(matches!(
        updated.change,
        IdentityChangeOutcome::Committed { .. }
    ));
    assert!(updated.applied.is_none(), "the row alone changes");
    assert_eq!(
        mob.standing("resting"),
        Some(EntryStanding::Unseated(UnseatedEntryCause::Archived)),
        "it stays archived"
    );
    assert_eq!(mob.row("resting").await.revision, 2);

    committed(mob.unarchive("unarchive-resting", "resting").await);
    assert!(mob.seated_dormant("resting"));
    assert_eq!(
        mob.handle
            .get_member(&id("resting"))
            .await
            .expect("roster read")
            .expect("the seated identity")
            .labels,
        labels,
        "the seat carries the update's labels"
    );
}

/// Row: an archive whose member's retirement a crash interrupted finishes
/// after the restart: the identity reads archived, no member holds it, and
/// its row stays archived on its member's session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_during_an_archive_finishes_it_after_restart() {
    let mut mob = StoreOwnedMob::new("archive-crash").await;
    let name = unique("resting");
    committed(
        mob.create(host_create("create-resting", &[&name]), ALLOW)
            .await,
    );
    mob.handle
        .ensure_member_live(&id(&name))
        .await
        .expect("wake the identity");
    let session = mob.session(&name).await;
    let hold = super::actor::retirement_stage_hold::install(&id(&name), "retire-runtime-quiesce");
    let archive = {
        let handle = mob.handle.clone();
        let name = name.clone();
        tokio::spawn(async move {
            handle
                .archive_identity(
                    crate::runtime::IdentityArchiveChange::new(
                        operation("archive-resting"),
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
    archive.abort();
    mob.unseated_for(&name, UnseatedEntryCause::Archived).await;
    assert!(!mob.is_member(&name).await);
    assert_eq!(
        mob.row(&name).await.presence,
        CreatedIdentityPresence::Archived {
            archived_session: Some(session)
        }
    );
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
        Outcome::BuildRequired => "build_required",
        Outcome::Refused { .. } => "refused",
        Outcome::Noted { .. } => "noted",
        Outcome::Failed { .. } => "failed",
    }
}

/// Row (RED): a store-owned update and a roster-owned identity's change of
/// the same spec answer the same, and each answers its own kind: a
/// descriptive change and a placement change keep the member, a
/// runtime-mode change restamps it, a profile change moves it to the new
/// profile (the store-owned one's migration declared by the update, the
/// roster-owned one's by its roster). A label change is the one change the
/// two answer differently: a roster-owned member is kept (the apply stores
/// no labels, its host's build step labels it), and a store-owned member is
/// restamped with its row's labels (its seat carries its row's labels and
/// no others).
///
/// RED on the old tree: no store-owned update exists.
///
/// The kinds are pinned per change and per custody: two equal failures no
/// longer pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_and_roster_updates_of_the_same_change_answer_the_same_verdict() {
    type Change = fn(CreatedIdentitySpec) -> CreatedIdentitySpec;
    // Each change, what the roster-owned identity answers and what the
    // store-owned one answers.
    let changes: [(&str, &str, &str, Change); 5] = [
        ("descriptive", "kept", "kept", |spec| {
            spec.with_display_name("Renamed")
        }),
        ("labels", "kept", "restamped", |spec| {
            let mut labels = spec.labels.clone();
            labels.insert("tier".to_string(), "gold".to_string());
            spec.with_labels(labels)
        }),
        ("runtime_mode", "restamped", "restamped", |spec| {
            spec.with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
        }),
        ("profile", "role_migrated", "role_migrated", |mut spec| {
            spec.profile = ProfileName::from("lead");
            spec
        }),
        ("placement", "kept", "kept", |spec| {
            spec.with_placement(meerkat_contracts::WireHostRef("host-peer-1".to_string()))
        }),
    ];
    for (label, roster_expected, store_expected, change) in changes {
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
        assert_eq!(
            roster_kind, roster_expected,
            "{label}: the roster-owned change"
        );
        assert_eq!(
            store_kind, store_expected,
            "{label}: the store-owned update (a label change restamps a store-owned member \
             and keeps a roster-owned one; every other change answers the same for both)"
        );
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
            false,
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

impl StoreOwnedMob {
    /// Write the transaction of a store-owned update of `spec`'s identity to
    /// `spec` as the update's commit step writes it (the row at its next
    /// revision, with the label restamp the update owes a seat that carries
    /// other labels, and the receipt; the roster entry then reads the row's
    /// labels), with nothing after it.
    async fn commit_update_transaction(&self, operation_id: &str, spec: CreatedIdentitySpec) {
        commit_update_transaction_of(&self.handle, operation_id, spec).await;
    }
}

/// [`StoreOwnedMob::commit_update_transaction`] through `handle`'s own
/// identity store: the update of a mob that stopped after the update's
/// commit and before its member step (a label update's restamp, say), so
/// the seat keeps the labels it was seated with and the row records the
/// restamp as owed.
async fn commit_update_transaction_of(
    handle: &MobHandle,
    operation_id: &str,
    spec: CreatedIdentitySpec,
) {
    write_update_transaction(handle, operation_id, spec, true).await;
}

/// The row of an update an earlier release committed: the row at its next
/// revision and the receipt, with no record of what the seat is owed, and
/// the roster entry left as it was. Releases 0.9.9 through 0.9.14 left a
/// seat so at every label update.
async fn commit_update_transaction_of_an_earlier_release(
    handle: &MobHandle,
    operation_id: &str,
    spec: CreatedIdentitySpec,
) {
    write_update_transaction(handle, operation_id, spec, false).await;
}

/// The store transaction of an update of `spec`'s identity, and, with
/// `as_this_release`, what this release's commit step does with it: the row
/// records the label restamp a seat with other labels is owed, and the
/// roster entry reads the row's labels.
async fn write_update_transaction(
    handle: &MobHandle,
    operation_id: &str,
    spec: CreatedIdentitySpec,
    as_this_release: bool,
) {
    let store = handle
        .identity_member
        .clone()
        .expect("the built-in identity store");
    let before = match crate::store::MobIdentityMemberStore::observe_created_identity(
        store.as_ref(),
        handle.mob_id(),
        &spec.identity,
    )
    .await
    .expect("the row reads")
    {
        Some(CreatedIdentityObservation::Created(row)) => row,
        other => panic!("{} has no readable row: {other:?}", spec.identity),
    };
    {
        let seat_trails = as_this_release
            && handle
                .roster
                .read()
                .await
                .seat_row_labels(&spec.identity)
                .is_some_and(|seat| *seat != spec.labels);
        let after = before
            .clone()
            .updated(spec.clone(), None, before.label_restamp_owed || seat_trails)
            .expect("the updated row");
        let receipt = crate::IdentityChangeReceipt::sealed(
            handle.mob_id().clone(),
            operation_id,
            digest(operation_id),
            IdentityChangeKind::Update,
            vec![
                crate::identity::IdentityChangeReceiptEntry::new(
                    spec.identity.clone(),
                    IdentityChangeEntryOutcome::Committed,
                )
                .with_spec(spec_digest(&spec), Some(after.revision)),
            ],
            1,
        )
        .expect("the update's receipt");
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
        if as_this_release {
            handle
                .roster
                .write()
                .await
                .set_store_owned_seat_labels(&spec.identity, &spec.labels);
        }
    }
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

/// Row (RED): a store-owned update decided while its identity is archived,
/// whose identity is unarchived before the update's step, reaches the
/// seated member: MobMachine routes the update from the step's own state,
/// so the member moves to the new profile on its own session and the row
/// lets the migration go.
///
/// RED on the old tree: the update read the archived row before its step and
/// changed the row only, leaving the seated member under its old profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_racing_an_unarchive_reaches_the_seated_member() {
    let mob = StoreOwnedMob::new("update-unarchive-race").await;
    let before = spec("mover", "worker");
    committed(
        mob.create(host_create("create-mover", &["mover"]), ALLOW)
            .await,
    );
    committed(mob.archive("archive-mover", "mover").await);
    mob.unseated_for("mover", UnseatedEntryCause::Archived)
        .await;
    let mut after = before.clone();
    after.profile = ProfileName::from("lead");

    let hold =
        super::super::identity_change::hold_next_change_step_for_test(&mob.mob_id(), "update");
    let handle = mob.handle.clone();
    let expected = spec_digest(&before);
    let changed = after.clone();
    let update = tokio::spawn(async move {
        handle
            .update_store_owned_identity(
                crate::runtime::StoreOwnedIdentityUpdate::new(
                    operation("update-mover"),
                    IdentityChangeRequesterInput::host("console"),
                    changed,
                    &expected,
                ),
                ALLOW,
            )
            .await
    });
    tokio::time::timeout(STEP, hold.entered)
        .await
        .expect("the update reaches its step")
        .expect("the hold is armed");
    committed(mob.unarchive("unarchive-mover", "mover").await);
    assert!(
        mob.is_member("mover").await,
        "the unarchive seats the identity"
    );
    let session = mob.session("mover").await;
    let _ = hold.release.send(());
    let updated = tokio::time::timeout(STEP, update)
        .await
        .expect("the update answers")
        .expect("the update task completes")
        .expect("the update commits");
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
    assert_eq!(row.spec, Some(after));
    assert_eq!(row.pending_role_migration, None);
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
    // The label update's transaction, written as its commit step writes it
    // (the row's RED leg then fails at the recovery it tests, not at
    // an update stub).
    mob.commit_update_transaction("update-phoenix", second)
        .await;
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
        Self::with_storage(label, MobStorage::in_memory()).await
    }

    /// [`Self::new`] over `storage`.
    async fn with_storage(label: &str, storage: MobStorage) -> Self {
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
        .expect("the create answers")
        .map(|created| created.change),
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
            .expect("the create answers")
            .map(|created| created.change),
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

/// The built-in identity store of a row's storage, whose change commit
/// fails once, writing nothing, for the first change `fails` picks.
struct FailingChangeCommit {
    inner: Arc<dyn crate::store::MobIdentityMemberStore>,
    fails: fn(&crate::store::IdentityChangeWrite) -> bool,
    armed: AtomicBool,
    /// The store reads of one identity's row.
    row_reads: AtomicU64,
    /// The store reads of a page of identities' rows.
    page_reads: AtomicU64,
    /// How many of the next reads of one identity's row fail.
    row_read_failures: AtomicU64,
    /// Whether the reads of one identity's row answer a row this build
    /// cannot read.
    rows_unreadable: AtomicBool,
    /// Whether the reads of one identity's row park once an update
    /// committed, and whether they park now.
    parks_after_an_update: AtomicBool,
    parking: AtomicBool,
    parked: tokio::sync::Notify,
}

impl FailingChangeCommit {
    /// Fail the next `count` reads of one identity's row.
    fn fail_row_reads(&self, count: u64) {
        self.row_read_failures.store(count, Ordering::SeqCst);
    }

    /// Answer every read of one identity's row as a row this build cannot
    /// read (a row a later release wrote, say), or serve the rows again.
    fn answer_row_reads_unreadable(&self, unreadable: bool) {
        self.rows_unreadable.store(unreadable, Ordering::SeqCst);
    }

    /// From the next committed update on, park every read of one
    /// identity's row ([`Self::parked`] answers at the first).
    fn park_row_reads_after_an_update(&self) {
        self.parks_after_an_update.store(true, Ordering::SeqCst);
    }

    /// Answers once a read of one identity's row is parked.
    async fn parked(&self) {
        self.parked.notified().await;
    }

    /// Serve the reads of one identity's row again.
    fn serve_row_reads(&self) {
        self.parks_after_an_update.store(false, Ordering::SeqCst);
        self.parking.store(false, Ordering::SeqCst);
    }

    /// `storage` with its identity store wrapped, the failure armed for the
    /// first change `fails` picks.
    fn wrap(
        storage: &mut MobStorage,
        fails: fn(&crate::store::IdentityChangeWrite) -> bool,
    ) -> Arc<Self> {
        let failing = Arc::new(Self {
            inner: storage
                .identity_member
                .clone()
                .expect("the in-memory storage has the identity member store"),
            fails,
            armed: AtomicBool::new(true),
            row_reads: AtomicU64::new(0),
            page_reads: AtomicU64::new(0),
            row_read_failures: AtomicU64::new(0),
            rows_unreadable: AtomicBool::new(false),
            parks_after_an_update: AtomicBool::new(false),
            parking: AtomicBool::new(false),
            parked: tokio::sync::Notify::new(),
        });
        storage.identity_member =
            Some(Arc::clone(&failing) as Arc<dyn crate::store::MobIdentityMemberStore>);
        failing
    }

    /// A settle's receipt that accepts an activation (its entry
    /// `Activating`): a store write that fails after the activation was
    /// sent.
    fn accepts_an_activation(write: &crate::store::IdentityChangeWrite) -> bool {
        write.receipt.change == IdentityChangeKind::SettlePreparation
            && write
                .receipt
                .identities
                .iter()
                .any(|entry| entry.outcome == IdentityChangeEntryOutcome::Activating)
    }
}

#[async_trait::async_trait]
impl crate::store::MobIdentityMemberStore for FailingChangeCommit {
    async fn observe_identity_member_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<crate::store::IdentityMemberTargetObservation, MobStoreError> {
        self.inner
            .observe_identity_member_target(mob_id, identity)
            .await
    }

    async fn commit_identity_member_spawned(
        &self,
        permit: &crate::identity::IdentityActuationPermit,
        event: &crate::event::NewMobEvent,
    ) -> Result<crate::store::IdentityMemberEventCommitOutcome, MobStoreError> {
        self.inner
            .commit_identity_member_spawned(permit, event)
            .await
    }

    async fn observe_identity_wiring_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<crate::store::IdentityWiringTargetObservation, MobStoreError> {
        self.inner
            .observe_identity_wiring_target(mob_id, identity)
            .await
    }

    async fn commit_identity_wiring_event(
        &self,
        permit: &crate::identity::IdentityActuationPermit,
        event: &crate::event::NewMobEvent,
    ) -> Result<crate::store::IdentityWiringEventCommitOutcome, MobStoreError> {
        self.inner.commit_identity_wiring_event(permit, event).await
    }

    async fn read_member_binding_journal(
        &self,
        mob_id: &MobId,
    ) -> Result<crate::store::MemberBindingJournalRead, MobStoreError> {
        self.inner.read_member_binding_journal(mob_id).await
    }

    async fn read_member_binding_journal_from(
        &self,
        mob_id: &MobId,
        from_cursor: u64,
    ) -> Result<crate::store::MemberBindingJournalRead, MobStoreError> {
        self.inner
            .read_member_binding_journal_from(mob_id, from_cursor)
            .await
    }

    async fn commit_identity_change(
        &self,
        write: &crate::store::IdentityChangeWrite,
    ) -> Result<crate::store::IdentityChangeCommit, MobStoreError> {
        if (self.fails)(write) && self.armed.swap(false, Ordering::SeqCst) {
            return Err(MobStoreError::WriteFailed(
                "the row's store refused the change's write".to_string(),
            ));
        }
        let committed = self.inner.commit_identity_change(write).await;
        if committed.is_ok()
            && write.receipt.change == IdentityChangeKind::Update
            && self.parks_after_an_update.load(Ordering::SeqCst)
        {
            self.parking.store(true, Ordering::SeqCst);
        }
        committed
    }

    async fn settle_created_identity_row(
        &self,
        row: &crate::store::CreatedIdentityRowWrite,
    ) -> Result<bool, MobStoreError> {
        self.inner.settle_created_identity_row(row).await
    }

    async fn observe_identity_change_receipt(
        &self,
        mob_id: &MobId,
        operation_id: &str,
    ) -> Result<Option<crate::IdentityChangeReceipt>, MobStoreError> {
        self.inner
            .observe_identity_change_receipt(mob_id, operation_id)
            .await
    }

    async fn scan_created_identity_rows(
        &self,
        mob_id: &MobId,
        after: Option<&crate::store::IdentityIntentScanCursor>,
        limit: usize,
    ) -> Result<crate::store::CreatedIdentityScanPage, MobStoreError> {
        self.inner
            .scan_created_identity_rows(mob_id, after, limit)
            .await
    }

    async fn observe_created_identity(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<Option<CreatedIdentityObservation>, MobStoreError> {
        self.row_reads.fetch_add(1, Ordering::SeqCst);
        if self.parking.load(Ordering::SeqCst) {
            self.parked.notify_one();
            std::future::pending::<()>().await;
        }
        if self
            .row_read_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(MobStoreError::ReadFailed(
                "the row's store cannot serve the read now".to_string(),
            ));
        }
        if self.rows_unreadable.load(Ordering::SeqCst) {
            return Ok(Some(CreatedIdentityObservation::Unreadable {
                evidence_digest: digest("a row this build cannot read"),
                detail: "unknown field `a_later_releases_key`".to_string(),
            }));
        }
        self.inner.observe_created_identity(mob_id, identity).await
    }

    async fn observe_created_identities(
        &self,
        mob_id: &MobId,
        identities: &[AgentIdentity],
    ) -> Result<BTreeMap<AgentIdentity, CreatedIdentityObservation>, MobStoreError> {
        self.page_reads.fetch_add(1, Ordering::SeqCst);
        self.inner
            .observe_created_identities(mob_id, identities)
            .await
    }
}

/// Row (RED): many identities' rows are read in pages, one store read per
/// page of names, so a host that serves K store-owned identities reads
/// their rows with a number of store reads that does not grow with K beyond
/// pages. The answer is each identity's row as the one-row read answers it,
/// and a name the store holds no row of is absent.
///
/// RED on the old tree: no such read exists; a host reads one row per
/// identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_identities_rows_are_read_in_pages_not_one_store_read_each() {
    for count in [3_usize, crate::IDENTITY_ENTRY_PAGE_MAX + 4] {
        let mut storage = MobStorage::in_memory();
        let reads = FailingChangeCommit::wrap(&mut storage, |_| false);
        let mob = StoreOwnedMob::build(
            store_owned_definition(&format!("paged-rows-{count}")),
            storage,
            |builder| builder,
        )
        .await;
        let names = (0..count)
            .map(|index| format!("paged-{index:03}"))
            .collect::<Vec<_>>();
        for chunk in names.chunks(64) {
            let created = mob
                .create_outcome(
                    StoreOwnedIdentityCreate::new(
                        operation(&format!("create-{}", chunk[0])),
                        IdentityChangeRequesterInput::host("console"),
                        chunk
                            .iter()
                            .map(|name| CreatedIdentityRequest::new(spec(name, "worker")))
                            .collect(),
                    ),
                    ALLOW,
                )
                .await
                .expect("the create commits");
            assert!(matches!(
                created.change,
                IdentityChangeOutcome::Committed { .. }
            ));
        }
        let mut asked = names.iter().map(|name| id(name)).collect::<Vec<_>>();
        asked.push(id("paged-no-such-row"));

        reads.row_reads.store(0, Ordering::SeqCst);
        reads.page_reads.store(0, Ordering::SeqCst);
        let rows = mob
            .handle
            .identity_records(&asked)
            .await
            .expect("the rows read");
        assert_eq!(
            (
                reads.page_reads.load(Ordering::SeqCst),
                reads.row_reads.load(Ordering::SeqCst)
            ),
            (
                asked.len().div_ceil(crate::IDENTITY_ENTRY_PAGE_MAX) as u64,
                0
            ),
            "{count} identities are read in pages, with no read of one row"
        );
        assert_eq!(rows.len(), count, "a name with no row is absent");
        for name in [&names[0], &names[count - 1]] {
            assert_eq!(
                rows.get(&id(name)),
                mob.handle
                    .identity_record(&id(name))
                    .await
                    .expect("the row reads")
                    .as_ref(),
                "{name}'s row is the one the one-row read answers"
            );
        }
    }
}

/// Row: a store write that fails after the activation was sent
/// ends the attempt as a failed publication and commits nothing. The settle
/// answers the failure; MobMachine still holds the attempt unaccepted, so
/// the publication's membership commit is refused, and the hold's end (a
/// failed publication) ends the attempt. No member row is journaled, the
/// identity is still pending, no receipt is stored, and the row keeps
/// attempt 1. A retry naming attempt 1 is refused `stale_attempt` with no
/// current attempt, and a new construction holds attempt 2 and activates.
///
/// A row for the confirmed path (no RED leg: it pins behaviour the commits
/// before it built).
#[tokio::test]
async fn a_store_write_failing_after_the_activation_send_ends_a_failed_publication_and_commits_nothing()
 {
    let mut storage = MobStorage::in_memory();
    let failing =
        FailingChangeCommit::wrap(&mut storage, FailingChangeCommit::accepts_an_activation);
    let mob = HeldMob::with_storage("activation-write-fails", storage).await;
    mob.create_constructed("built").await;
    let held = mob
        .construct("construct-built", "built")
        .await
        .expect("the construction is held");
    assert_eq!(held.attempt(), 1);
    let settled = settle(&mob.handle, "settle-built", "built", Some(1)).await;
    assert!(settled.is_err(), "{settled:?}");
    assert!(
        !failing.armed.load(Ordering::SeqCst),
        "the store refused the settle's write"
    );

    // The hold's end: the publication fails, and the attempt ends.
    let dsl_identity = crate::machines::mob_machine::AgentIdentity::from_domain(&id("built"));
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        let open = {
            let state = mob.handle.machine_state_watch_rx.borrow();
            state
                .identity_entry_constructions_held
                .contains_key(&dsl_identity)
                || state
                    .identity_entry_constructions_activating
                    .contains_key(&dsl_identity)
        };
        if !open {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the attempt never ended"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let journal = mob
        .storage
        .events
        .replay_all()
        .await
        .expect("read the journal");
    assert!(
        !journals_a_member(&journal, "built"),
        "no member was published"
    );
    assert_eq!(
        mob.standing("built"),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::PreparationPending
        ))
    );
    assert_eq!(
        mob.handle
            .identity_operation("settle-built")
            .await
            .expect("the receipt read answers"),
        None
    );
    assert_eq!(
        row_presence(&mob.handle, "built").await,
        CreatedIdentityPresence::PendingPreparation {
            attempt: 1,
            constructed: true,
        }
    );
    assert_eq!(
        refusal(settle(&mob.handle, "settle-built-retry", "built", Some(1)).await),
        IdentityChangeRefusal::StaleAttempt {
            identity: id("built"),
            current: None,
        }
    );

    let again = mob
        .construct("construct-built-again", "built")
        .await
        .expect("a new construction is held");
    assert_eq!(again.attempt(), 2);
    let settled = settle(&mob.handle, "settle-built-again", "built", Some(2))
        .await
        .expect("the settle activates the new attempt");
    assert!(
        matches!(
            settled.settlement,
            Some(crate::runtime::PreparationSettlement::Activated { attempt: 2 })
        ),
        "{:?}",
        settled.settlement
    );
}

/// Row: a create whose store transaction fails writes nothing and
/// seats nothing: the change answers the store's failure, and no row, no
/// receipt, no journal row, no entry and no member exist. Once the store
/// serves again, the same create commits and seats both identities.
///
/// A row for behaviour that stands (no RED leg).
#[tokio::test]
async fn a_create_whose_store_write_fails_writes_nothing_and_seats_nothing() {
    let mut storage = MobStorage::in_memory();
    let failing = FailingChangeCommit::wrap(&mut storage, |write| {
        write.receipt.change == IdentityChangeKind::Create
    });
    let mob = StoreOwnedMob::build(
        store_owned_definition("create-write-fails"),
        storage,
        |builder| builder,
    )
    .await;
    let journal = mob.journal().await.len();
    let failed = mob
        .create(host_create("create-pair", &["alpha", "beta"]), ALLOW)
        .await;
    assert!(failed.is_err(), "{failed:?}");
    assert!(
        !failing.armed.load(Ordering::SeqCst),
        "the store refused the create's write"
    );
    assert!(mob.rows().await.is_empty(), "no row");
    assert_eq!(
        mob.handle
            .identity_operation("create-pair")
            .await
            .expect("the receipt read answers"),
        None,
        "no receipt"
    );
    assert_eq!(mob.journal().await.len(), journal, "no journal row");
    for name in ["alpha", "beta"] {
        assert!(!mob.is_member(name).await, "{name} has no member");
        assert!(
            matches!(
                mob.read(name),
                IdentityEntryRead::NotAnEntry { worker: None }
            ),
            "{name} is no entry"
        );
    }

    committed(
        mob.create(host_create("create-pair", &["alpha", "beta"]), ALLOW)
            .await,
    );
    assert!(mob.seated_dormant("alpha") && mob.seated_dormant("beta"));
}

/// Row (RED): an accepted activation that a crash left without its
/// publication ends `publication_interrupted` and keeps the identity
/// pending. The settle accepts the activation and journals its receipt;
/// the publication is parked, and the process crashes before it. After the
/// restart, a retry of the settle replays its receipt and answers the
/// interrupted publication, the identity is still pending, and a new held
/// attempt can be constructed and activated.
///
/// RED on the old tree: no settle exists.
///
/// Staged with a real crash: the settle's own step writes the
/// receipt, and the row waits until the receipt read answers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_between_acceptance_and_commit_ends_the_attempt_publication_interrupted_and_keeps_the_entry_pending()
 {
    let mut mob = HeldMob::new("construct-crash").await;
    // The publication hold is keyed by name across the test process.
    let name = unique("crashed");
    mob.create_constructed(&name).await;
    let held = mob
        .construct("construct-crashed", &name)
        .await
        .expect("the construction is held");
    let attempt = held.attempt();
    let (entered, release) = crate::runtime::held_spawn::arm_held_publication_for_test(&name);
    let handle = mob.handle.clone();
    let settled_name = name.clone();
    let settling = tokio::spawn(async move {
        settle(&handle, "settle-crashed", &settled_name, Some(attempt)).await
    });
    tokio::time::timeout(STEP, entered)
        .await
        .expect("the accepted publication parks")
        .expect("the publication hold");
    let deadline = tokio::time::Instant::now() + STEP;
    while mob
        .handle
        .identity_operation("settle-crashed")
        .await
        .expect("the receipt read answers")
        .is_none()
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the settle never journaled its acceptance"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The crash, with the publication still parked.
    settling.abort();
    mob.restart().await;
    let replayed = settle(&mob.handle, "settle-crashed", &name, Some(attempt))
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
        mob.standing(&name),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::PreparationPending
        )),
        "the identity stays pending"
    );
    let again = mob
        .construct("construct-again", &name)
        .await
        .expect("a new attempt is held");
    assert_eq!(again.attempt(), attempt + 1);
    settle(&mob.handle, "settle-again", &name, Some(again.attempt()))
        .await
        .expect("the new attempt activates");
    assert!(matches!(
        mob.standing(&name),
        Some(EntryStanding::Seated(_))
    ));
    // The parked publication of the crashed process is let go last.
    drop(release);
}

/// Row (RED): an accepted activation whose publication fails answers the
/// error that ended it, typed: the settle answers the failed publication
/// with its cause's failure class and wire error code (the seam's is a
/// membership commit MobMachine refused, as a ladder refused at its
/// publication is) and its text, and the identity stays pending.
///
/// RED on the old tree: the end's cause is discarded, and the settle
/// answers none; then the cause kept the text only, with no class.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_publication_answers_the_error_that_ended_it() {
    let mob = HeldMob::new("construct-publication-fails").await;
    // The publication failure is keyed by name across the test process.
    let name = unique("failing");
    mob.create_constructed(&name).await;
    let held = mob
        .construct("construct-failing", &name)
        .await
        .expect("the construction is held");
    crate::runtime::held_spawn::fail_next_held_publication_for_test(&name);
    let settled = settle(&mob.handle, "settle-failing", &name, Some(held.attempt()))
        .await
        .expect("the settle commits its acceptance");
    match settled.settlement {
        Some(crate::runtime::PreparationSettlement::PublicationEnded {
            attempt,
            end: crate::runtime::ConstructionEnd::PublicationFailed,
            cause: Some(cause),
        }) => {
            assert_eq!(attempt, held.attempt());
            assert_eq!(
                cause.class,
                crate::MobFailureClass::MobRejected,
                "the cause's class is the refusal's: {cause:?}"
            );
            assert_eq!(cause.error_code, None, "{cause:?}");
            assert!(
                cause
                    .detail
                    .contains(crate::runtime::held_spawn::HELD_PUBLICATION_FAILURE_FOR_TEST),
                "the cause's detail names the error that ended the publication: {cause:?}"
            );
        }
        other => panic!("the failed publication answers its cause: {other:?}"),
    }
    assert_eq!(
        mob.standing(&name),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::PreparationPending
        )),
        "the identity stays pending"
    );
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

/// A host surface's archive of `name` as `operation_id` through `handle`.
async fn archive_through(
    handle: &MobHandle,
    operation_id: &str,
    name: &str,
) -> Result<IdentityChangeOutcome, MobError> {
    tokio::time::timeout(
        STEP,
        handle.archive_identity(
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

/// `name`'s row's presence, read through `handle`.
async fn row_presence(handle: &MobHandle, name: &str) -> CreatedIdentityPresence {
    match handle
        .identity_record(&id(name))
        .await
        .expect("the record read answers")
    {
        Some(CreatedIdentityObservation::Created(row)) => row.presence,
        other => panic!("{name} has no readable row: {other:?}"),
    }
}

/// Assert `refused` is the archive's refusal of `name`, awaiting its host's
/// preparation with `attempt` open, retryable.
fn assert_preparation_pending(refused: &IdentityChangeRefusal, name: &str, attempt: Option<u64>) {
    assert_eq!(
        refused,
        &IdentityChangeRefusal::HostPreparationPending {
            identity: id(name),
            attempt,
        }
    );
    assert_eq!(refused.kind(), "host_preparation_pending");
    assert!(refused.is_retryable(), "{name}: the refusal is retryable");
}

/// Row (RED): an archive of an identity awaiting its host's preparation is
/// refused `host_preparation_pending`, retryable and naming its open
/// attempt, so the preparation is never dropped: its row still awaits the
/// preparation (and records the attempt), the identity stays pending, and no
/// receipt is written. Once the preparation is settled, the archive under
/// the same operation commits. Two legs: a `required` preparation with no
/// attempt, and a `constructed` one whose construction is held.
///
/// RED on the old tree: the `required` identity's archive commits and drops
/// its preparation (its unarchive would seat it without one), and the held
/// construction's answers a generic pending lifecycle operation.
#[tokio::test]
async fn archiving_an_identity_awaiting_its_preparation_is_refused_naming_it() {
    // The `required` leg.
    let mob = StoreOwnedMob::new("archive-pending").await;
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
    let refused = refusal(mob.archive("archive-pending", "pending").await);
    assert_preparation_pending(&refused, "pending", None);
    assert!(
        matches!(
            mob.row("pending").await.presence,
            CreatedIdentityPresence::PendingPreparation {
                constructed: false,
                ..
            }
        ),
        "the row still awaits the preparation"
    );
    assert_eq!(
        mob.standing("pending"),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::PreparationPending
        ))
    );
    assert_eq!(
        mob.handle
            .identity_operation("archive-pending")
            .await
            .expect("read the receipt"),
        None,
        "a refused archive writes no receipt"
    );
    settle(&mob.handle, "settle-pending", "pending", None)
        .await
        .expect("the settle commits");
    assert!(mob.seated_dormant("pending"));
    committed(mob.archive("archive-pending", "pending").await);
    mob.unseated_for("pending", UnseatedEntryCause::Archived)
        .await;

    // The `constructed` leg, its construction held.
    let mob = HeldMob::new("archive-constructed").await;
    mob.create_constructed("built").await;
    let held = mob
        .construct("construct-built", "built")
        .await
        .expect("the construction is held");
    let refused = refusal(archive_through(&mob.handle, "archive-built", "built").await);
    assert_preparation_pending(&refused, "built", Some(held.attempt()));
    assert!(
        matches!(
            row_presence(&mob.handle, "built").await,
            CreatedIdentityPresence::PendingPreparation {
                attempt,
                constructed: true,
            } if attempt == held.attempt()
        ),
        "the row still records the held attempt"
    );
    assert_eq!(
        mob.standing("built"),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::PreparationPending
        ))
    );
    assert_eq!(
        mob.handle
            .identity_operation("archive-built")
            .await
            .expect("read the receipt"),
        None,
        "a refused archive writes no receipt"
    );
    settle(&mob.handle, "settle-built", "built", Some(held.attempt()))
        .await
        .expect("the settle activates the held construction");
    assert!(matches!(
        mob.standing("built"),
        Some(EntryStanding::Seated(_))
    ));
    committed(archive_through(&mob.handle, "archive-built", "built").await);
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.standing("built") != Some(EntryStanding::Unseated(UnseatedEntryCause::Archived)) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "built never read archived: {:?}",
            mob.standing("built")
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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

/// Row (RED): the record read shows a store-owned identity's revision and
/// provenance, and the entry read its archive, without waking it. After an
/// update and an archive, the entry reads unseated, archived, on the
/// session its member was bound to; its record (`identity_record`, the
/// row's read) reads the revision the update committed, the provenance its
/// create wrote, and its archived presence; a name with no row reads none;
/// and the reads build no member. The entry view is a read of the published
/// machine state: revision and provenance are the row's, read from the
/// record (renamed from the entry read's name).
///
/// RED on the old tree: the entry view names no archived session and the
/// handle reads no record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_record_read_shows_revision_and_provenance_and_the_entry_read_its_archive_without_waking()
 {
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
/// past. A label change takes the row at once, and until its restamp runs
/// (here it did not: the update stopped after its commit) the seated
/// member's seat row keeps the old labels, so the identity counts
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
    // A label update whose restamp did not run: the seat keeps the labels
    // it was seated with.
    mob.commit_update_transaction(
        "relabel-desk",
        spec("desk", "worker")
            .with_labels(BTreeMap::from([("team".to_string(), "beta".to_string())])),
    )
    .await;
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

/// Row (RED): with no running mob, a store-level read counts the rows a
/// running mob's census counts, by the same rule. Over a persistent store,
/// before any change, both readers answer nothing. After a create of three
/// identities, an archive of one and the removal of another (its
/// tombstone), the census, the path reader of the store file (read
/// read-only beside the live mob) and the storage reader each count 3 rows:
/// 1 archived, 1 tombstone, none unreadable.
///
/// RED on the old tree: no store-level reader counts the rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_store_level_row_readers_count_what_the_census_counts() {
    let dir = tempfile::tempdir().expect("a store directory");
    let path = dir.path().join("mob.db");
    let mob = StoreOwnedMob::build(
        store_owned_definition("census-readers"),
        MobStorage::persistent(&path).expect("open the store"),
        |builder| builder,
    )
    .await;
    assert!(
        MobStorage::read_identity_record_rows(&path)
            .await
            .expect("read the store")
            .is_empty()
    );
    assert!(
        mob.storage
            .identity_record_rows()
            .await
            .expect("read the storage")
            .is_empty()
    );

    committed(
        mob.create(
            host_create("create-desks", &["kept", "shelved", "gone"]),
            ALLOW,
        )
        .await,
    );
    committed(mob.archive("archive-shelved", "shelved").await);
    mob.unseated_for("shelved", UnseatedEntryCause::Archived)
        .await;
    committed(mob.remove("remove-gone", "gone").await);
    mob.removed("gone").await;

    let census = mob
        .handle
        .identity_record_census()
        .await
        .expect("the census answers");
    let counted = crate::runtime::IdentityRecordRows {
        created_rows: census.created_rows,
        archived: census.archived,
        tombstones: census.tombstones,
        unreadable: census.unreadable,
        label_restamps_owed: census.label_restamps_owed,
    };
    assert_eq!(
        counted,
        crate::runtime::IdentityRecordRows {
            created_rows: 3,
            archived: 1,
            tombstones: 1,
            unreadable: 0,
            label_restamps_owed: 0,
        },
        "{census:?}"
    );
    let expected = BTreeMap::from([(mob.mob_id(), counted)]);
    assert_eq!(
        MobStorage::read_identity_record_rows(&path)
            .await
            .expect("read the live mob's store"),
        expected
    );
    assert_eq!(
        mob.storage
            .identity_record_rows()
            .await
            .expect("read the storage"),
        expected
    );
}

/// Row: with no running mob, the store-level readers count the rows that
/// record an owed label restamp, as the census of the mob over the store
/// counts them, so an operator reads the count before a rollback without
/// starting the mob. Over a persistent store, `owed`'s label update
/// committed while its seat row carried other labels: the census, the path
/// reader of the store file (read read-only beside the live mob) and the
/// storage reader each count 2 rows, 1 of them owed its label restamp. Once
/// a respawn seats the row's labels and the row lets the restamp go, each
/// counts none owed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_store_level_row_readers_count_the_owed_label_restamps() {
    let dir = tempfile::tempdir().expect("a store directory");
    let path = dir.path().join("mob.db");
    let mob = StoreOwnedMob::build(
        store_owned_definition("census-readers-owed"),
        MobStorage::persistent(&path).expect("open the store"),
        |builder| builder,
    )
    .await;
    committed(
        mob.create(host_create("create-desks", &["kept", "owed"]), ALLOW)
            .await,
    );
    mob.commit_update_transaction(
        "relabel-owed",
        spec("owed", "worker").with_labels(moved_team("owed")),
    )
    .await;
    assert!(label_restamp_owed(&mob, "owed").await);

    for owed in [1, 0] {
        let census = mob
            .handle
            .identity_record_census()
            .await
            .expect("the census answers");
        assert_eq!(census.label_restamps_owed, owed, "{census:?}");
        let counted = crate::runtime::IdentityRecordRows {
            created_rows: 2,
            archived: 0,
            tombstones: 0,
            unreadable: 0,
            label_restamps_owed: owed,
        };
        let expected = BTreeMap::from([(mob.mob_id(), counted)]);
        assert_eq!(
            MobStorage::read_identity_record_rows(&path)
                .await
                .expect("read the live mob's store"),
            expected,
            "the path reader, {owed} owed"
        );
        assert_eq!(
            mob.storage
                .identity_record_rows()
                .await
                .expect("read the storage"),
            expected,
            "the storage reader, {owed} owed"
        );
        if owed == 1 {
            tokio::time::timeout(
                STEP,
                mob.handle.respawn_member(
                    id("owed"),
                    crate::runtime::MemberRespawn::new().with_labels(moved_team("owed")),
                ),
            )
            .await
            .expect("the respawn answers")
            .expect("the respawn with the row's labels commits");
            assert!(
                !label_restamp_owed(&mob, "owed").await,
                "a seat row carries the row's labels: nothing is owed"
            );
        }
    }
}

/// Row (RED): the storage reader counts the rows of a mob storage kept in a
/// host store (Core's published reference host store) by the census's
/// rule, as the census of the mob over it counts them.
///
/// RED on the old tree: no store-level reader counts the rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_storage_row_reader_counts_a_host_store_storage() {
    let host = Arc::new(meerkat_store_conformance::InMemoryHostStore::new());
    let realm = meerkat_runtime::store::spi::SpiRealm::open(
        Arc::clone(&host) as Arc<dyn meerkat_core::host_store::HostStore>,
        meerkat_runtime::store::spi::SpiRealmConfig::default(),
    )
    .await
    .expect("open the host store realm");
    let mobs = crate::store::SpiMobStoreHost::for_realm(&realm)
        .await
        .expect("claim the mob families");
    let storage = mobs
        .open("root")
        .await
        .expect("open the mob's storage")
        .storage();
    assert!(
        storage
            .identity_record_rows()
            .await
            .expect("read the empty storage")
            .is_empty()
    );
    let mob = StoreOwnedMob::build(
        store_owned_definition("census-host-store"),
        storage,
        |builder| builder,
    )
    .await;
    committed(
        mob.create(host_create("create-desks", &["kept", "gone"]), ALLOW)
            .await,
    );
    committed(mob.remove("remove-gone", "gone").await);
    mob.removed("gone").await;

    let census = mob
        .handle
        .identity_record_census()
        .await
        .expect("the census answers");
    let counted = crate::runtime::IdentityRecordRows {
        created_rows: census.created_rows,
        archived: census.archived,
        tombstones: census.tombstones,
        unreadable: census.unreadable,
        label_restamps_owed: census.label_restamps_owed,
    };
    assert_eq!(
        counted,
        crate::runtime::IdentityRecordRows {
            created_rows: 2,
            archived: 0,
            tombstones: 1,
            unreadable: 0,
            label_restamps_owed: 0,
        },
        "{census:?}"
    );
    assert_eq!(
        mob.storage
            .identity_record_rows()
            .await
            .expect("read the host store storage"),
        BTreeMap::from([(mob.mob_id(), counted)])
    );
}

// ---------------------------------------------------------------------------
// Rollback (cl-design-v2 11.2): what a published predecessor finds of each
// 0.9.9 identity state. A predecessor replays the journal's member rows and
// reads a v2 row as unreadable for its identity alone; an in-process leg
// checks the facts it reads, and a lane-run leg writes the fixture the
// published pairs run (`lane:rollback`).
// ---------------------------------------------------------------------------

/// Whether the journal holds a member row of `name`: what a predecessor
/// would replay and run.
fn journals_a_member(journal: &[MobEvent], name: &str) -> bool {
    journal.iter().any(|event| {
        matches!(
            &event.kind,
            MobEventKind::MemberSpawned(spawned) if spawned.agent_identity == id(name)
        )
    })
}

/// Rollback row RB-CL2-1, in-process leg: an identity awaiting its host's
/// preparation, with no construction, leaves a predecessor nothing to run.
/// Its create journals no member row and builds no session; a restart of
/// this build keeps it pending with its row unchanged, and its settle then
/// seats it Dormant.
#[tokio::test]
async fn rb_cl2_1_a_pending_preparation_leaves_a_predecessor_nothing_to_run() {
    let mut mob = StoreOwnedMob::new("rb-cl2-1").await;
    let builds = mob.builds().await;
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
    assert!(!journals_a_member(&mob.journal().await, "pending"));
    assert_eq!(mob.builds().await, builds, "no session is built");
    let row = mob.row("pending").await;

    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    mob.unseated_for("pending", UnseatedEntryCause::PreparationPending)
        .await;
    assert_eq!(mob.row("pending").await, row, "the row is unchanged");
    settle(&mob.handle, "settle-pending", "pending", None)
        .await
        .expect("the settle commits");
    assert!(mob.seated_dormant("pending"));
}

/// Rollback row RB-CL2-1, lane-run leg:
/// `a_pending_preparation_under_<pair>_runs_nothing` for each published
/// pair.
///
/// This build writes the fixture: `pending`, a store-owned identity
/// awaiting its host's `required` preparation. The lane serves it with the
/// pair and checks: no member runs under the name, its row reads
/// unreadable to the predecessor for that identity alone and stays
/// byte-identical; rolled forward to this build, it reads awaiting its
/// preparation and its settle seats it Dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn rb_cl2_1_pending_preparation_fixture() {
    let name = "store-owned-pending";
    let dir = super::created_identity_rows::rollback_fixture_dir(name);
    super::identity_entry_rows::write_rollback_fixture(
        &dir,
        &format!("rollback-{name}"),
        |handle| async move {
            committed(
                handle
                    .create_store_owned_identities(
                        prepared_create(
                            "create-pending",
                            spec("pending", "worker")
                                .with_runtime_mode_override(crate::MobRuntimeMode::TurnDriven),
                            IdentityPreparationMode::Required,
                        ),
                        ALLOW,
                    )
                    .await
                    .map(|created| created.change),
            );
        },
    )
    .await;
}

/// Rollback row RB-CL2-3, Core's leg: an approved decision outlives a
/// rollback, since a predecessor never reads the decision store. Back on
/// this build (a restart here), the approved change re-executes under its
/// permit and commits once, a retry replays its receipt, and a decision
/// whose deadline passed meanwhile is refused `decision_closed` and writes
/// nothing. The executor's leg (settlement-first recovery, revalidation)
/// is Meta's.
#[tokio::test]
async fn rb_cl2_3_an_approved_change_re_executes_after_a_restart_and_an_expired_one_writes_nothing()
{
    let mut mob = StoreOwnedMob::new("rb-cl2-3").await;
    let now = now_ms();
    let approved = decided_permit(&operation("create-approved"), now, now + 600_000);
    let lapsed = decided_permit(&operation("create-lapsed"), now - 120_000, now - 60_000);

    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    committed(
        mob.create(
            host_create("create-approved", &["approved"]),
            IdentityChangeConsentInput::Decided(&approved),
        )
        .await,
    );
    assert!(matches!(
        mob.create(
            host_create("create-approved", &["approved"]),
            IdentityChangeConsentInput::Decided(&approved),
        )
        .await,
        Ok(IdentityChangeOutcome::Replayed { .. })
    ));
    assert_eq!(
        refusal(
            mob.create(
                host_create("create-lapsed", &["lapsed"]),
                IdentityChangeConsentInput::Decided(&lapsed),
            )
            .await
        ),
        IdentityChangeRefusal::DecisionClosed { state: "expired" }
    );
    let rows = mob.rows().await;
    assert!(rows.contains_key(&id("approved")));
    assert!(!rows.contains_key(&id("lapsed")), "nothing was written");
}

/// Rollback row RB-CL2-4a, lane-run leg:
/// `a_store_owned_member_under_<pair>_runs_as_a_member_of_its_profile` for
/// each published pair. The in-process leg is R2.2's
/// `a_store_owned_seat_equals_the_roster_owned_seat_of_the_same_spec`: the
/// seat row a predecessor replays is a roster-owned seat's.
///
/// This build writes the fixture: `desk`, a store-owned identity seated
/// Dormant on the worker profile. The lane serves it with the pair and
/// checks: `desk` is a plain member of its profile, built from the profile
/// and the host's build, and a delivery wakes it and runs a turn; rolled
/// forward, it is a store-owned identity again on the same session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn rb_cl2_4a_store_owned_member_fixture() {
    let name = "store-owned-member";
    let dir = super::created_identity_rows::rollback_fixture_dir(name);
    super::identity_entry_rows::write_rollback_fixture(
        &dir,
        &format!("rollback-{name}"),
        |handle| async move {
            committed(
                handle
                    .create_store_owned_identities(
                        StoreOwnedIdentityCreate::new(
                            operation("create-desk"),
                            IdentityChangeRequesterInput::host("console"),
                            vec![CreatedIdentityRequest::new(
                                spec("desk", "worker")
                                    .with_runtime_mode_override(crate::MobRuntimeMode::TurnDriven),
                            )],
                        ),
                        ALLOW,
                    )
                    .await
                    .map(|created| created.change),
            );
        },
    )
    .await;
}

/// Rollback row RB-CL2-4b, first leg (without Apply now), in-process: a
/// label change after the member's seat takes the row at once, and until
/// its restamp runs (here it did not: the update stopped after its commit,
/// as a member that holds work defers it) the seat row a predecessor
/// replays keeps the labels it was seated with, and the record census
/// counts the seat stale. The second leg (with Apply now)
/// is `rb_cl2_4b_apply_now_writes_the_rows_labels_into_the_seat_row`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rb_cl2_4b_a_label_change_leaves_the_seat_row_a_predecessor_reads_stale() {
    let mob = StoreOwnedMob::new("rb-cl2-4b").await;
    committed(
        mob.create(host_create("create-desk", &["desk"]), ALLOW)
            .await,
    );
    let seated_with = mob.spawn_row("desk").await.labels;
    let moved = BTreeMap::from([("team".to_string(), "beta".to_string())]);
    mob.commit_update_transaction("relabel-desk", spec("desk", "worker").with_labels(moved))
        .await;
    let row = mob.row("desk").await;
    assert_eq!(
        row.spec
            .expect("the row holds its spec")
            .labels
            .get("team")
            .map(String::as_str),
        Some("beta"),
        "the row takes the labels at once"
    );
    assert_eq!(
        mob.spawn_row("desk").await.labels,
        seated_with,
        "the seat row keeps its labels"
    );
    let census = mob
        .handle
        .identity_record_census()
        .await
        .expect("the census answers");
    assert_eq!(census.seats_stale, 1, "{census:?}");
}

/// Rollback row RB-CL2-4b, first leg, lane-run:
/// `a_stale_seat_under_<pair>_runs_with_its_seat_rows_labels` for each
/// published pair.
///
/// This build writes the fixture: `desk`, a store-owned identity seated
/// Dormant, whose labels an update moved after its seat and whose restamp
/// did not run (the fixture writes the update's transaction and nothing
/// after it: a label update restamps an idle member at once, so only an
/// update that stopped after its commit, or a member that holds work,
/// leaves such a seat). The lane serves it
/// with the pair and checks: `desk` runs with the labels of its seat row;
/// rolled forward, the seat keeps those labels, and the record census counts
/// it in `seats_stale`, until it is seated again or respawned with its row's
/// labels (Apply now, the second leg's fixture below).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn rb_cl2_4b_stale_seat_fixture() {
    let name = "store-owned-stale-seat";
    let dir = super::created_identity_rows::rollback_fixture_dir(name);
    super::identity_entry_rows::write_rollback_fixture(
        &dir,
        &format!("rollback-{name}"),
        |handle| async move {
            let seated = spec("desk", "worker")
                .with_runtime_mode_override(crate::MobRuntimeMode::TurnDriven);
            committed(
                handle
                    .create_store_owned_identities(
                        StoreOwnedIdentityCreate::new(
                            operation("create-desk"),
                            IdentityChangeRequesterInput::host("console"),
                            vec![CreatedIdentityRequest::new(seated.clone())],
                        ),
                        ALLOW,
                    )
                    .await
                    .map(|created| created.change),
            );
            commit_update_transaction_of(
                &handle,
                "relabel-desk",
                seated.with_labels(BTreeMap::from([("team".to_string(), "beta".to_string())])),
            )
            .await;
        },
    )
    .await;
}

/// The record census's count of seats whose row moved since their seat.
async fn seats_stale(mob: &StoreOwnedMob) -> usize {
    mob.handle
        .identity_record_census()
        .await
        .expect("the census answers")
        .seats_stale
}

/// Rollback row RB-CL2-4b, second leg (with Apply now), in-process: after
/// a label change left the seat stale (its restamp did not run), the host's
/// Apply now respawns the
/// member with its row's labels on its own session: the new seat row (the
/// last `MemberSpawned` row) carries the row's labels, the generation
/// advances by one, and the record census counts no stale seat.
///
/// RED on the old tree: the respawn keeps the member's labels.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rb_cl2_4b_apply_now_writes_the_rows_labels_into_the_seat_row() {
    let mob = StoreOwnedMob::new("rb-cl2-4b-apply-now").await;
    committed(
        mob.create(host_create("create-desk", &["desk"]), ALLOW)
            .await,
    );
    mob.commit_update_transaction(
        "relabel-desk",
        spec("desk", "worker")
            .with_labels(BTreeMap::from([("team".to_string(), "beta".to_string())])),
    )
    .await;
    assert_eq!(seats_stale(&mob).await, 1);
    let session = mob.session("desk").await;
    let generation = mob.generation("desk").await.expect("desk is a member");
    let row_labels = mob
        .row("desk")
        .await
        .spec
        .expect("the row holds its spec")
        .labels;

    let receipt = tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(
            id("desk"),
            crate::runtime::MemberRespawn::new().with_labels(row_labels.clone()),
        ),
    )
    .await
    .expect("the respawn answers")
    .expect("the respawn commits");
    assert_eq!(
        Some(receipt.runtime_id().generation),
        generation.next(),
        "the next generation"
    );
    assert_eq!(
        mob.spawn_row("desk").await.labels,
        row_labels,
        "the new seat row carries the row's labels"
    );
    assert_eq!(mob.session("desk").await, session, "the same session");
    assert_eq!(seats_stale(&mob).await, 0, "no seat is stale");
}

/// Row (RED): a respawn of a store-owned member through the plain respawn
/// (`MobHandle::respawn`) settles an owed label restamp, as a respawn that
/// names its labels does. `owed`'s label update committed while its seat
/// row carried other labels, so its row records the owed restamp and its
/// roster entry reads the row's labels. The respawn seats the labels the
/// entry reads, the row's, on the member's own session: a seat row now
/// carries the row's labels, the row lets the owed restamp go, and the
/// census counts none.
///
/// RED on the old tree: no settle follows this respawn, so the row keeps a
/// record of a restamp nothing owes: the census counts it, and an earlier
/// release reads the row as unreadable for no reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_respawn_settles_an_owed_label_restamp() {
    let mob = StoreOwnedMob::new("plain-respawn-settles").await;
    committed(
        mob.create(host_create("create-owed", &["owed"]), ALLOW)
            .await,
    );
    let moved = moved_team("owed");
    mob.commit_update_transaction(
        "relabel-owed",
        spec("owed", "worker").with_labels(moved.clone()),
    )
    .await;
    assert!(label_restamp_owed(&mob, "owed").await);
    let session = mob.session("owed").await;
    tokio::time::timeout(STEP, mob.handle.respawn(id("owed"), None))
        .await
        .expect("the respawn answers")
        .expect("the respawn commits");
    assert_eq!(
        mob.spawn_row("owed").await.labels,
        moved,
        "the respawn seats the labels the roster entry reads: the row's"
    );
    assert_eq!(mob.session("owed").await, session, "the same session");
    assert!(
        !label_restamp_owed(&mob, "owed").await,
        "a seat row carries the row's labels: nothing is owed"
    );
    let census = mob
        .handle
        .identity_record_census()
        .await
        .expect("the census answers");
    assert_eq!(census.label_restamps_owed, 0, "{census:?}");
}

/// Row: a respawn that names no labels keeps the member's own, the labels
/// its roster entry reads. After an earlier release's label update (the
/// row moved, the entry did not) the respawn keeps the labels the member
/// was seated with: Apply now is the respawn with the row's labels, never
/// a default, so the seat stays stale. After this release's label update
/// the entry reads the row's labels from the update's commit on, so the
/// same respawn seats them, and the row lets the owed restamp go.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_respawn_without_labels_keeps_the_members_labels() {
    let mob = StoreOwnedMob::new("respawn-keeps-labels").await;
    committed(
        mob.create(host_create("create-desks", &["desk", "owed"]), ALLOW)
            .await,
    );
    let moved = BTreeMap::from([("team".to_string(), "beta".to_string())]);
    mob.commit_update_transaction(
        "relabel-owed",
        spec("owed", "worker").with_labels(moved.clone()),
    )
    .await;
    assert!(label_restamp_owed(&mob, "owed").await);
    tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_member(id("owed"), crate::runtime::MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers")
    .expect("the respawn commits");
    assert_eq!(
        mob.spawn_row("owed").await.labels,
        moved,
        "the respawn seats the labels the roster entry reads: the row's"
    );
    assert!(
        !label_restamp_owed(&mob, "owed").await,
        "a seat row carries the row's labels: the row lets the owed restamp go"
    );

    let seated_with = mob.spawn_row("desk").await.labels;
    assert_ne!(moved, seated_with, "the update moves the labels");
    commit_update_transaction_of_an_earlier_release(
        &mob.handle,
        "relabel-desk",
        spec("desk", "worker").with_labels(moved),
    )
    .await;
    assert_eq!(
        seats_stale(&mob).await,
        1,
        "the label change whose restamp did not run left the seat stale"
    );
    tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_member(id("desk"), crate::runtime::MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers")
    .expect("the respawn commits");
    assert_eq!(
        mob.spawn_row("desk").await.labels,
        seated_with,
        "the respawn keeps the member's labels, not the row's"
    );
    assert_eq!(seats_stale(&mob).await, 1, "the seat stays stale");
}

/// Row (RED): a store-owned update that restamps its member (another
/// runtime mode) and moves its labels seats the next incarnation with the
/// row's labels: the restamp's seat row carries them, and no seat is stale.
///
/// RED on the old tree: the restamp keeps the member's labels, and the
/// census counts the seat stale.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_owned_restamp_seats_the_rows_labels() {
    let mob = StoreOwnedMob::new("restamp-labels").await;
    let before = spec("desk", "worker");
    committed(
        mob.create(host_create("create-desk", &["desk"]), ALLOW)
            .await,
    );
    let moved = BTreeMap::from([("team".to_string(), "gamma".to_string())]);
    let after = before
        .clone()
        .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
        .with_labels(moved.clone());
    let updated = mob
        .update("restamp-desk", after, &spec_digest(&before))
        .await
        .expect("the update commits");
    assert!(
        matches!(
            updated.applied,
            Some(crate::runtime::DeclaredEntryOutcome::Restamped { .. })
        ),
        "{updated:?}"
    );
    assert_eq!(mob.spawn_row("desk").await.labels, moved);
    assert_eq!(seats_stale(&mob).await, 0, "no seat is stale");
}

/// Rollback row RB-CL2-4b, second leg, lane-run:
/// `a_stale_seat_after_apply_now_under_<pair>_runs_with_the_rows_labels`
/// for each published pair.
///
/// This build writes the fixture: `desk`, a store-owned identity whose
/// labels an update moved after its seat and whose restamp did not run (the
/// update's transaction and nothing after it), respawned by the host's Apply
/// now with its row's labels. The lane serves it with the pair and checks:
/// `desk` runs with the row's labels (its last seat row's); rolled forward,
/// the record census counts no stale seat.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn rb_cl2_4b_apply_now_fixture() {
    let name = "store-owned-apply-now";
    let dir = super::created_identity_rows::rollback_fixture_dir(name);
    super::identity_entry_rows::write_rollback_fixture(
        &dir,
        &format!("rollback-{name}"),
        |handle| async move {
            let seated = spec("desk", "worker")
                .with_runtime_mode_override(crate::MobRuntimeMode::TurnDriven);
            committed(
                handle
                    .create_store_owned_identities(
                        StoreOwnedIdentityCreate::new(
                            operation("create-desk"),
                            IdentityChangeRequesterInput::host("console"),
                            vec![CreatedIdentityRequest::new(seated.clone())],
                        ),
                        ALLOW,
                    )
                    .await
                    .map(|created| created.change),
            );
            let labels = BTreeMap::from([("team".to_string(), "beta".to_string())]);
            commit_update_transaction_of(
                &handle,
                "relabel-desk",
                seated.with_labels(labels.clone()),
            )
            .await;
            handle
                .respawn_member(
                    id("desk"),
                    crate::runtime::MemberRespawn::new().with_labels(labels),
                )
                .await
                .expect("Apply now respawns the member with its row's labels");
        },
    )
    .await;
}

/// Rollback row RB-CL2-5, in-process leg: a held construction open at a
/// stop leaves no member. Its member was never published, so the journal
/// holds no member row for the identity (what a predecessor would run);
/// after the restart the identity still awaits its preparation, its row
/// keeps the attempt, and a new attempt is held and activated.
#[tokio::test]
async fn rb_cl2_5_a_held_construction_open_at_a_stop_leaves_no_member() {
    let mut mob = HeldMob::new("rb-cl2-5").await;
    mob.create_constructed("built").await;
    let held = mob
        .construct("construct-built", "built")
        .await
        .expect("the construction is held");

    mob.restart().await;
    let journal = mob
        .storage
        .events
        .replay_all()
        .await
        .expect("read the journal");
    assert!(
        !journals_a_member(&journal, "built"),
        "no member was published"
    );
    assert_eq!(
        mob.standing("built"),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::PreparationPending
        ))
    );
    let again = mob
        .construct("construct-again", "built")
        .await
        .expect("a new attempt is held");
    assert!(again.attempt() > held.attempt(), "the row kept the attempt");
    settle(&mob.handle, "settle-built", "built", Some(again.attempt()))
        .await
        .expect("the new attempt activates");
    assert!(matches!(
        mob.standing("built"),
        Some(EntryStanding::Seated(_))
    ));
}

/// Rollback row RB-CL2-5, lane-run leg:
/// `a_held_construction_under_<pair>_leaves_no_member` for each published
/// pair.
///
/// This build writes the fixture: `built`, a store-owned identity whose
/// `constructed` preparation holds an open construction when the process
/// stops. The lane serves it with the pair and checks: no member runs under
/// the name and its row reads unreadable for that identity alone; rolled
/// forward, the identity awaits its preparation, and a new attempt is held
/// and activated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn rb_cl2_5_held_construction_fixture() {
    let name = "store-owned-held-construction";
    let dir = super::created_identity_rows::rollback_fixture_dir(name);
    let service = Arc::new(MockSessionService::new());
    HeldMob::compose(&service);
    let mut definition = store_owned_definition(name);
    definition.id = MobId::from(format!("rollback-{name}").as_str());
    let handle = MobBuilder::new(
        definition,
        MobStorage::persistent(dir.join("mob.sqlite")).expect("open the fixture store"),
    )
    .with_session_service(service)
    .with_member_runtime_registration_hook(Arc::new(super::held_spawn::SelectingHost::default()))
    .create()
    .await
    .expect("create the fixture mob");
    committed(
        handle
            .create_store_owned_identities(
                prepared_create(
                    "create-built",
                    spec("built", "worker")
                        .with_runtime_mode_override(crate::MobRuntimeMode::TurnDriven),
                    IdentityPreparationMode::Constructed,
                ),
                ALLOW,
            )
            .await
            .map(|created| created.change),
    );
    handle
        .construct_identity(crate::runtime::IdentityConstructionRequest::new(
            operation("construct-built"),
            "built",
            tokio::time::Instant::now() + STEP,
        ))
        .await
        .expect("the construction is held");
    crash_stop_and_release_routes(handle).await;
}
