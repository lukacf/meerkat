//! Rows for the profiles a launch lists their own memory only, on a host of
//! two mobs that each keep agent memory.
//!
//! The fixture is the launch of the per-mob memory rows with two things
//! added: the launch lists profiles, and its roster seats what a row says
//! in each mob. Both mobs' agents are built by the one constructor a
//! gateway builds them with. An agent of ONE name, `desk`, stands in both
//! mobs: the launch's roster declares it in the launch mob, and in the
//! second mob an agent of a listed profile creates it, through the change
//! path that writes the mob's own record of who asked.

#![allow(clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use meerkat_mob::{MobDefinition, MobId, ProfileName};
use serde_json::json;

use super::UnifiedRuntime;
use super::mob_entries_tests::{launch_composed, second_mob, set_of, shut_down};
use super::mob_memory_tests::{a_note, empty_draft, instructions_of};
use crate::gateway_wiring::{HostMobIdentities, HostMobMemory, mob_memory_customizer};
use crate::identity_creation::{
    ChangeCommit, ChangeRequester, CoreChangeExecutor, CreateIdentitySpec, IdentityChangeExecutor,
    IdentityCreationPolicy, IdentityOutcome, OperationId, RequestContext, RequestEnvelope,
    RequesterFacts, consent_of_plan, normalize_create,
};
use crate::identity_first::agent_memory::{
    AgentMemoryConfig, AgentMemoryProvider, AgentMemorySelection,
};
use crate::identity_first::{
    AgentAddressability, AgentBuildContext, AgentCustomizer, AgentIdentity,
    AgentMemoryRuntimeInjector, AgentRuntimeServices, DurabilityPolicy, DurableAgentSpec,
    IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore,
    LocalLeaseProvider, RosterContext, RosterError, RosterProvider,
};
use crate::memory::own_memory_only::{
    LineageAbove, OwnMemoryOnly, RecordedLineage, TrustedLineage,
};
use crate::memory::records::{MemoryAuthor, MemoryKind, MemoryScope, NewMemoryRecord};
use crate::memory::sqlite_store::SqliteAgentMemoryStore;

/// The agent name both mobs hold.
const DESK: &str = "desk";
/// The agent of the listed profile that the second mob's roster declares.
const GREETER: &str = "greeter";
/// The agent of the unlisted profile that the second mob's roster declares.
const CLERK: &str = "clerk";
/// The profile the rows list: its agents may create agents.
const LISTED: &str = "keeper";
/// The profile no row lists.
const UNLISTED: &str = "worker";

const SHARED_BY_THE_LAUNCH_MOB: &str = "The launch mob files claims on Mondays";
const SHARED_BY_THE_SECOND_MOB: &str = "The second mob files claims on Fridays";
const OF_THE_SECOND_DESK: &str = "The second desk keeps the green ledger";

/// A definition with the two profiles of the rows, both keeping memory,
/// under an id of its own.
fn definition(tag: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "mob-own-memory-{tag}-{}"

[profiles.{UNLISTED}]
model = "gpt-5.5"

[profiles.{UNLISTED}.tools]
comms = true
memory = true

[profiles.{LISTED}]
model = "gpt-5.5"

[profiles.{LISTED}.tools]
comms = true
memory = true
mob = true
"#,
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with a profile that keeps memory and one that may create agents")
}

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("identity")
}

/// The row of an agent a host configures, or an agent creates.
fn agent_of(name: &str, profile: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: identity(name),
        profile: ProfileName::from(profile),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// The roster provider of the launch: one for every mob of the host, which
/// answers the agents a row seated in the mob it is asked for.
#[derive(Default)]
struct SeatedRoster {
    seats: Mutex<BTreeMap<MobId, Vec<DurableAgentSpec>>>,
}

impl SeatedRoster {
    fn seat(&self, mob_id: &MobId, agents: Vec<DurableAgentSpec>) {
        self.seats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(mob_id.clone(), agents);
    }
}

#[async_trait::async_trait]
impl RosterProvider for SeatedRoster {
    async fn roster(&self, context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(context
            .mob_definition
            .as_ref()
            .and_then(|definition| {
                self.seats
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&definition.id)
                    .cloned()
            })
            .unwrap_or_default())
    }
}

/// A launch that keeps agent memory and lists profiles, with a second mob
/// of the host.
struct Listing {
    host: UnifiedRuntime,
    /// The launch's own store, over the launch's memory root.
    of_the_launch: SqliteAgentMemoryStore,
    config: AgentMemoryConfig,
    /// The launch mob's identity context and identity runtime.
    launch_context: Arc<IdentityFirstRuntimeContext>,
    launch_runtime: Arc<IdentityRuntime>,
    second_id: MobId,
}

/// The profiles `names` as a launch's checked list.
fn listed(names: &[&str]) -> BTreeSet<ProfileName> {
    names.iter().copied().map(ProfileName::from).collect()
}

/// A launch that keeps agent memory in the bundled store under `temp` and
/// lists the profiles `names` their own memory only. Its launch mob's
/// roster declares the agent `desk` of the unlisted profile; the launch
/// mob's agents are built by the constructor a gateway builds them with,
/// over the launch mob's own records. It hands its memory, with the list,
/// over for the mobs it hosts beside its launch mob. A second mob of the
/// host has the agent `greeter` of the listed profile and the agent `clerk`
/// of the unlisted profile in its roster, and no agent `desk` there.
async fn launch_listing(temp: &tempfile::TempDir, names: &[&str]) -> Listing {
    let (mut host, _sessions, _machine, ()) = launch_composed(
        temp,
        definition("host"),
        meerkat::Config::default(),
        |_| (),
        |spec| spec,
    )
    .await;
    let launch_mob = host.mob_handle().mob_id().clone();
    let config = AgentMemoryConfig {
        selection: AgentMemorySelection::Always,
        ..AgentMemoryConfig::default()
    };
    let of_the_launch = SqliteAgentMemoryStore::open(temp.path().join("agent-memory"))
        .expect("the launch's memory store");
    let provider: Arc<dyn AgentMemoryProvider> = Arc::new(of_the_launch.clone());
    let handle = host.mob_runtime().handle().observer();

    // As the runtime builder's launch does: the customizer is made before
    // the identity runtime, over a lineage that is bound when it stands.
    let lineage = RecordedLineage::unbound();
    let customizer: Arc<dyn AgentCustomizer> = Arc::new(mob_memory_customizer(
        None,
        Arc::clone(&provider),
        config.clone(),
        BTreeMap::new(),
        Some(&handle),
        OwnMemoryOnly::listing(listed(names), lineage.clone()),
    ));
    let launch_runtime = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(
                LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "mob-own-memory-launch".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(crate::gateway_wiring::identity_session_bridge(
                host.mob_runtime(),
                handle.clone(),
            ))),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(handle.clone())),
    );
    assert!(
        lineage.bind(&launch_runtime),
        "the launch mob's lineage is bound once, to the launch mob's records"
    );
    launch_runtime
        .set_agent_memory(Some(AgentMemoryRuntimeInjector::new(
            Arc::clone(&provider),
            config.clone(),
        )))
        .await;
    launch_runtime
        .set_agent_customizer(Some(Arc::clone(&customizer)))
        .await;
    let roster = Arc::new(SeatedRoster::default());
    roster.seat(&launch_mob, vec![agent_of(DESK, UNLISTED)]);
    let launch_context = Arc::new(IdentityFirstRuntimeContext::new(
        Arc::clone(&launch_runtime),
        Arc::clone(&roster) as Arc<dyn RosterProvider>,
        None,
        Some(customizer),
        Some(handle.definition().clone()),
    ));
    Box::pin(host.install_and_bootstrap_identity_first_context(
        Arc::clone(&launch_context),
        &[agent_of(DESK, UNLISTED)],
    ))
    .await
    .expect("the launch mob's identity bootstrap");

    HostMobIdentities::new(Arc::clone(&roster) as Arc<dyn RosterProvider>, true)
        .with_agent_memory_per_mob(Some(
            HostMobMemory::new(of_the_launch.clone(), config.clone(), launch_mob)
                .with_own_memory_only_profiles(listed(names)),
        ))
        .hand_over_to(&host);

    let of_the_second = definition("second");
    roster.seat(
        &of_the_second.id,
        vec![agent_of(GREETER, LISTED), agent_of(CLERK, UNLISTED)],
    );
    let second_id = second_mob(&host, of_the_second).await;
    // The values a row makes for the second mob's store are made for the
    // mob as the set holds it, whoever touches its directory first.
    of_the_launch.hosted_mob_stands_at_in_tests(
        &second_id,
        set_of(&host)
            .handle_for(&second_id)
            .await
            .expect("the set serves the second mob")
            .incarnation(),
    );
    Listing {
        host,
        of_the_launch,
        config,
        launch_context,
        launch_runtime,
        second_id,
    }
}

/// A record shared by a mob's agents, written by the application.
fn a_shared_record(title: &str) -> NewMemoryRecord {
    NewMemoryRecord {
        kind: MemoryKind::Fact,
        title: title.to_string(),
        description: format!("{title}, in short."),
        body: format!("{title}, in full."),
        tags: Vec::new(),
        evidence: Vec::new(),
        verification: None,
    }
}

impl Listing {
    fn realm_of_the_second(&self) -> String {
        format!("mob.{}", self.second_id)
    }

    /// A store value over the second mob's own directory.
    fn of_the_second(&self) -> SqliteAgentMemoryStore {
        self.of_the_launch
            .of_hosted_mob_in_tests(&self.second_id)
            .expect("the second mob's store")
    }

    async fn second_handle(&self) -> meerkat_mob::MobHandle {
        set_of(&self.host)
            .handle_for(&self.second_id)
            .await
            .expect("the set serves the second mob")
    }

    /// The second mob's identity context and identity runtime, once the mob
    /// resolves: its roster's agents are seated then.
    async fn second_plane(
        &self,
    ) -> (
        crate::identity_first::PublicIdentityFirstContext,
        Arc<IdentityRuntime>,
    ) {
        let mob = self
            .host
            .resolve_mob(Some(&self.second_id))
            .await
            .expect("the second mob resolves");
        (
            mob.identity_first_context()
                .expect("the second mob has an identity plane"),
            mob.identity_runtime_ref()
                .cloned()
                .expect("the second mob has an identity runtime"),
        )
    }

    /// Each mob's shared record, in that mob's own store, and a note of the
    /// second mob's agent `desk` in its own scope.
    async fn seed(&self) {
        let launch_mob = self.host.mob_handle().mob_id().clone();
        self.of_the_launch
            .remember_authored(
                &MemoryScope::Mob {
                    realm: self.config.realm.clone(),
                    mob: launch_mob.to_string(),
                },
                a_shared_record(SHARED_BY_THE_LAUNCH_MOB),
                MemoryAuthor::Application,
            )
            .await
            .expect("the launch mob's shared record");
        let of_the_second = self.of_the_second();
        of_the_second
            .remember_authored(
                &MemoryScope::Mob {
                    realm: self.realm_of_the_second(),
                    mob: self.second_id.to_string(),
                },
                a_shared_record(SHARED_BY_THE_SECOND_MOB),
                MemoryAuthor::Application,
            )
            .await
            .expect("the second mob's shared record");
        of_the_second
            .remember(
                &self.realm_of_the_second(),
                &identity(DESK),
                a_note(OF_THE_SECOND_DESK),
            )
            .await
            .expect("the second desk's own record");
    }

    /// The agent `greeter` of the second mob creates the agent `desk`
    /// there, of the unlisted profile, as itself: the second mob's own
    /// record of `desk` says an agent asked for it.
    async fn greeter_creates_desk_in_the_second_mob(&self) {
        let policy = IdentityCreationPolicy::parse(&json!({
            "enabled": true,
            "approval": {"rules": [{
                "change": ["create"],
                "outcome": {"kind": "allow"},
            }]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled");
        let facts = RequesterFacts::agent(GREETER, LISTED, BTreeMap::new());
        let profiles = BTreeSet::from([UNLISTED.to_owned(), LISTED.to_owned()]);
        let peers = BTreeMap::new();
        let normalized = normalize_create(
            &policy,
            &RequestContext {
                requester: &facts,
                profiles: &profiles,
                peers: &peers,
            },
            RequestEnvelope::default(),
            &[CreateIdentitySpec::new(agent_of(DESK, UNLISTED))],
        )
        .expect("the create is allowed");
        let requester = ChangeRequester::agent_recorded(GREETER);
        let operation_id = OperationId::for_request(&requester.recorded(), "greeter-creates-desk")
            .expect("a request id");
        let consent = consent_of_plan(&normalized.plan).expect("the policy allows the change");
        let mob = self.second_handle().await;
        let committed = CoreChangeExecutor::new()
            .commit(ChangeCommit::new(
                &mob,
                &operation_id,
                &requester,
                consent,
                &normalized.request,
            ))
            .await
            .expect("the change path answers a typed outcome");
        assert!(
            matches!(committed.outcome, IdentityOutcome::Done { .. }),
            "the second mob's agent of the listed profile creates an agent: {:?}",
            committed.outcome
        );
    }

    async fn shut_down(self) {
        shut_down(&self.host, &[&self.second_id]).await;
    }
}

/// What the customizer of `context` makes of a build of the agent `desk`,
/// of the unlisted profile, of the mob `handle` serves, as instructions.
async fn built_desk(
    context: &IdentityFirstRuntimeContext,
    handle: meerkat_mob::MobHandle,
) -> String {
    built(context, handle, DESK).await
}

/// [`built_desk`] of the agent `name` of the unlisted profile.
async fn built(
    context: &IdentityFirstRuntimeContext,
    handle: meerkat_mob::MobHandle,
    name: &str,
) -> String {
    let customizer = context
        .customizer
        .clone()
        .expect("the mob's identity context has a customizer");
    let mut draft = empty_draft();
    customizer
        .customize_build(
            &AgentBuildContext {
                identity: identity(name),
                active_peers: Vec::new(),
                managed_edges: Vec::new(),
                runtime_services: AgentRuntimeServices::new(handle),
            },
            &agent_of(name, UNLISTED),
            &mut draft,
        )
        .await
        .expect("the customizer builds the agent");
    instructions_of(&draft)
}

/// What `customizer` makes of a build of the agent `spec` of the mob
/// `handle` serves, as instructions.
async fn built_by(
    customizer: &dyn AgentCustomizer,
    handle: meerkat_mob::MobHandle,
    spec: &DurableAgentSpec,
) -> String {
    let mut draft = empty_draft();
    customizer
        .customize_build(
            &AgentBuildContext {
                identity: spec.identity.clone(),
                active_peers: Vec::new(),
                managed_edges: Vec::new(),
                runtime_services: AgentRuntimeServices::new(handle),
            },
            spec,
            &mut draft,
        )
        .await
        .expect("the customizer builds the agent");
    instructions_of(&draft)
}

/// A definition with the two profiles of the rows, whose agents run turn
/// by turn, so that no agent of a roster owes a turn at its start.
fn turn_driven_definition(tag: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "mob-own-memory-{tag}-{}"

[profiles.{UNLISTED}]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.{UNLISTED}.tools]
comms = true
memory = true

[profiles.{LISTED}]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.{LISTED}.tools]
comms = true
memory = true
"#,
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with two profiles that keep memory")
}

/// A launch by the runtime builder that keeps agent memory in `store` and
/// lists the listed profile its own memory only. Its roster declares the
/// agent `greeter` of the listed profile and the agent `clerk` of the
/// unlisted one.
async fn launched_by_the_builder(
    state: &std::path::Path,
    store: &SqliteAgentMemoryStore,
    config: &AgentMemoryConfig,
) -> UnifiedRuntime {
    Box::pin(
        UnifiedRuntime::builder()
            .definition(turn_driven_definition("builder"))
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("the state directory's identity substrate")
            .roster_provider(Arc::new(crate::identity_first::MutableRosterProvider::new(
                vec![agent_of(GREETER, LISTED), agent_of(CLERK, UNLISTED)],
            )))
            .identity_runtime_instance_id("mob-own-memory-builder")
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .agent_memory(Arc::new(store.clone()), config.clone())
            .agent_memory_own_memory_only_profiles([LISTED])
            .build(),
    )
    .await
    .expect("the builder's launch")
}

const SHARED_BY_THE_REALM: &str = "The realm closes its books in March";

/// The store of the launch rows, with a record of the realm and a record
/// of the mob `mob`, both written by the application.
async fn store_with_shared_records(
    root: &std::path::Path,
    config: &AgentMemoryConfig,
    mob: Option<&MobId>,
) -> SqliteAgentMemoryStore {
    let store = SqliteAgentMemoryStore::open(root).expect("the launch's memory store");
    store
        .remember_authored(
            &MemoryScope::Realm {
                realm: config.realm.clone(),
            },
            a_shared_record(SHARED_BY_THE_REALM),
            MemoryAuthor::Application,
        )
        .await
        .expect("the realm's record");
    if let Some(mob) = mob {
        store
            .remember_authored(
                &MemoryScope::Mob {
                    realm: config.realm.clone(),
                    mob: mob.to_string(),
                },
                a_shared_record(SHARED_BY_THE_LAUNCH_MOB),
                MemoryAuthor::Application,
            )
            .await
            .expect("the launch mob's shared record");
    }
    store
}

/// Two mobs hold an agent of one name. The launch's roster declares it in
/// the launch mob; in the second mob an agent of a listed profile created
/// it. The second mob's agent is listed its own memory only, by the second
/// mob's own record of who asked for it: under the launch mob's records
/// the name is a host's declaration, which is listed its mob's shared
/// memory. The launch mob's agent of the name is listed as before.
#[tokio::test]
async fn an_agent_a_listed_agent_created_in_a_second_mob_is_listed_its_own_memory_only_by_that_mobs_records()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let rows = launch_listing(&temp, &[LISTED]).await;
    rows.seed().await;
    let (second_context, second_runtime) = rows.second_plane().await;
    rows.greeter_creates_desk_in_the_second_mob().await;

    let of_the_second = built_desk(&second_context, rows.second_handle().await).await;
    assert!(
        of_the_second.contains(OF_THE_SECOND_DESK),
        "the second mob's agent is listed its own record: {of_the_second}"
    );
    assert!(
        !of_the_second.contains(SHARED_BY_THE_SECOND_MOB)
            && !of_the_second.contains(SHARED_BY_THE_LAUNCH_MOB),
        "an agent a listed agent created is listed no shared record: {of_the_second}"
    );
    // On the same host, the second mob's agent of the unlisted profile
    // that its host declared is listed its mob's shared record: the
    // second mob's records are read, and they say who is not covered.
    let of_the_clerk = built(&second_context, rows.second_handle().await, CLERK).await;
    assert!(
        of_the_clerk.contains(SHARED_BY_THE_SECOND_MOB)
            && !of_the_clerk.contains(SHARED_BY_THE_LAUNCH_MOB),
        "the second mob's agent its host declared is listed its mob's shared record: \
         {of_the_clerk}"
    );
    // The list holds by name in a mob hosted beside the launch mob: the
    // second mob's own agent of the listed profile, which its host
    // declared, is listed no shared record.
    let second_customizer = second_context
        .customizer
        .clone()
        .expect("the second mob's identity context has a customizer");
    let of_the_greeter = built_by(
        second_customizer.as_ref(),
        rows.second_handle().await,
        &agent_of(GREETER, LISTED),
    )
    .await;
    assert!(
        !of_the_greeter.contains(SHARED_BY_THE_SECOND_MOB)
            && !of_the_greeter.contains(SHARED_BY_THE_LAUNCH_MOB),
        "the second mob's agent of the listed profile is listed no shared record: \
         {of_the_greeter}"
    );

    let of_the_launch = built_desk(
        &rows.launch_context,
        rows.host.mob_runtime().handle().observer(),
    )
    .await;
    assert!(
        of_the_launch.contains(SHARED_BY_THE_LAUNCH_MOB)
            && !of_the_launch.contains(SHARED_BY_THE_SECOND_MOB),
        "the launch mob's agent of the name, which its host declared, is listed its mob's \
         shared record as before: {of_the_launch}"
    );

    // What each mob's records say is above the name.
    let desk = identity(DESK);
    let above_in_the_second = RecordedLineage::of(&second_runtime).above(&desk).await;
    assert_eq!(
        above_in_the_second,
        LineageAbove::CreatedBy {
            identity: identity(GREETER),
            profile: Some(ProfileName::from(LISTED)),
        },
        "the second mob's records say its agent of the listed profile created the name"
    );
    assert_eq!(
        RecordedLineage::of(&rows.launch_runtime).above(&desk).await,
        LineageAbove::HostDeclared,
        "the launch mob's records say its host declared the name"
    );
    rows.shut_down().await;
}

/// The same host with no profile listed: the agent the second mob's agent
/// created is listed its mob's shared record, as every agent is.
#[tokio::test]
async fn with_no_profile_listed_an_agent_created_in_a_second_mob_is_listed_its_mobs_shared_record()
{
    let temp = tempfile::tempdir().expect("tempdir");
    let rows = launch_listing(&temp, &[]).await;
    rows.seed().await;
    let (second_context, _second_runtime) = rows.second_plane().await;
    rows.greeter_creates_desk_in_the_second_mob().await;

    let of_the_second = built_desk(&second_context, rows.second_handle().await).await;
    assert!(
        of_the_second.contains(OF_THE_SECOND_DESK)
            && of_the_second.contains(SHARED_BY_THE_SECOND_MOB)
            && !of_the_second.contains(SHARED_BY_THE_LAUNCH_MOB),
        "with no profile listed nothing is withheld: {of_the_second}"
    );
    rows.shut_down().await;
}

/// A name the second mob's records do not hold is listed its own memory
/// only there, whatever the launch mob holds under that name: the launch
/// mob's roster declares `desk`, and the second mob's records are the ones
/// asked.
#[tokio::test]
async fn a_name_a_second_mobs_records_do_not_hold_is_listed_its_own_memory_only_whatever_the_launch_mob_holds()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let rows = launch_listing(&temp, &[LISTED]).await;
    rows.seed().await;
    let (second_context, second_runtime) = rows.second_plane().await;

    let of_the_second = built_desk(&second_context, rows.second_handle().await).await;
    assert!(
        of_the_second.contains(OF_THE_SECOND_DESK)
            && !of_the_second.contains(SHARED_BY_THE_SECOND_MOB),
        "a name the second mob's records do not hold is listed no shared record: {of_the_second}"
    );
    // On the same host, a name the second mob's records do hold as its
    // host's declaration is listed its mob's shared record.
    let of_the_clerk = built(&second_context, rows.second_handle().await, CLERK).await;
    assert!(
        of_the_clerk.contains(SHARED_BY_THE_SECOND_MOB),
        "the second mob's agent its host declared is listed its mob's shared record: \
         {of_the_clerk}"
    );
    assert_eq!(
        RecordedLineage::of(&second_runtime)
            .above(&identity(CLERK))
            .await,
        LineageAbove::HostDeclared,
        "the second mob's records say its host declared the clerk"
    );
    assert_eq!(
        RecordedLineage::of(&second_runtime)
            .above(&identity(DESK))
            .await,
        LineageAbove::Unreadable,
        "the second mob's records say nothing of the name"
    );
    assert_eq!(
        RecordedLineage::of(&rows.launch_runtime)
            .above(&identity(DESK))
            .await,
        LineageAbove::HostDeclared,
        "the launch mob's records say its host declared the name"
    );
    rows.shut_down().await;
}

/// A lineage handed out before its binding reads, after it, the records of
/// the mob it was bound to; a second binding answers that it was bound
/// before, and the first stands.
#[tokio::test]
async fn a_lineage_handed_out_before_its_binding_reads_the_mob_it_is_bound_to_first() {
    let temp = tempfile::tempdir().expect("tempdir");
    let rows = launch_listing(&temp, &[LISTED]).await;
    let (_second_context, second_runtime) = rows.second_plane().await;
    rows.greeter_creates_desk_in_the_second_mob().await;
    let desk = identity(DESK);

    let lineage = RecordedLineage::unbound();
    let handed_out = lineage.clone();
    assert_eq!(
        handed_out.above(&desk).await,
        LineageAbove::Unreadable,
        "before the binding no link is read"
    );
    assert!(
        lineage.bind(&second_runtime),
        "the first binding is answered as the first"
    );
    let of_the_second = LineageAbove::CreatedBy {
        identity: identity(GREETER),
        profile: Some(ProfileName::from(LISTED)),
    };
    assert_eq!(
        handed_out.above(&desk).await,
        of_the_second,
        "the value handed out before the binding reads the second mob's records after it"
    );
    assert!(
        !lineage.bind(&rows.launch_runtime),
        "a second binding is answered as one that came after"
    );
    assert_eq!(
        handed_out.above(&desk).await,
        of_the_second,
        "the first binding stands: the launch mob's records, which say the host declared \
         the name, are not asked"
    );
    rows.shut_down().await;
}

/// The runtime builder's launch applies the list to its launch mob. The
/// customizer the launch composed and gave its identity context lists a
/// roster agent of the listed profile no shared record, and a roster agent
/// of the unlisted profile the realm's record: the record of who created an
/// agent was bound to the launch mob's identity runtime, which knows the
/// second as its host's declaration. (This launch lists its launch mob's
/// agents no mob records, with or without the list.)
#[tokio::test(flavor = "multi_thread")]
async fn the_runtime_builders_launch_lists_a_listed_profiles_agent_no_shared_record() {
    let state = tempfile::tempdir().expect("tempdir");
    let memory = tempfile::tempdir().expect("tempdir");
    let config = AgentMemoryConfig {
        selection: AgentMemorySelection::Always,
        ..AgentMemoryConfig::default()
    };
    let store = store_with_shared_records(memory.path(), &config, None).await;
    let runtime = launched_by_the_builder(state.path(), &store, &config).await;
    let context = runtime
        .identity_first_context()
        .expect("the launch has an identity context");
    let customizer = context
        .customizer
        .clone()
        .expect("the launch's identity context has a customizer");

    let of_the_clerk = built_by(
        customizer.as_ref(),
        runtime.mob_handle(),
        &agent_of(CLERK, UNLISTED),
    )
    .await;
    assert!(
        of_the_clerk.contains(SHARED_BY_THE_REALM),
        "the launch mob's agent of the unlisted profile is listed the realm's record: \
         {of_the_clerk}"
    );
    let of_the_greeter = built_by(
        customizer.as_ref(),
        runtime.mob_handle(),
        &agent_of(GREETER, LISTED),
    )
    .await;
    assert!(
        !of_the_greeter.contains(SHARED_BY_THE_REALM),
        "the launch mob's agent of the listed profile is listed no shared record: \
         {of_the_greeter}"
    );
    runtime.shutdown().await;
}

/// The stdio launch applies the list to its launch mob. The customizer the
/// stdio launch makes for its launch mob, over a real mob and its identity
/// runtime, lists a roster agent of the listed profile neither the mob's
/// shared record nor the realm's, and a roster agent of the unlisted
/// profile both; with no profile listed the first is listed both too.
#[tokio::test(flavor = "multi_thread")]
async fn the_stdio_launchs_customizer_lists_a_listed_profiles_agent_no_shared_record() {
    let state = tempfile::tempdir().expect("tempdir");
    let memory = tempfile::tempdir().expect("tempdir");
    let config = AgentMemoryConfig {
        selection: AgentMemorySelection::Always,
        ..AgentMemoryConfig::default()
    };
    // The runtime the customizer is made over: a launch with the same
    // roster. Its own customizer is not the one under test.
    let unlisting = SqliteAgentMemoryStore::open(memory.path().join("of-the-runtime"))
        .expect("the runtime's own memory store");
    let runtime = launched_by_the_builder(state.path(), &unlisting, &config).await;
    let launch_mob = runtime.mob_handle();
    let identities = Arc::clone(
        runtime
            .identity_runtime_ref()
            .expect("the launch has an identity runtime"),
    );
    let store = store_with_shared_records(
        &memory.path().join("of-the-launch"),
        &config,
        Some(launch_mob.mob_id()),
    )
    .await;
    let provider: Arc<dyn AgentMemoryProvider> = Arc::new(store);

    let listing = crate::rpc_gateway::launch_mob_memory_customizer(
        None,
        Arc::clone(&provider),
        config.clone(),
        listed(&[LISTED]),
        &launch_mob,
        &identities,
    );
    let of_the_clerk = built_by(&listing, launch_mob.clone(), &agent_of(CLERK, UNLISTED)).await;
    assert!(
        of_the_clerk.contains(SHARED_BY_THE_LAUNCH_MOB)
            && of_the_clerk.contains(SHARED_BY_THE_REALM),
        "the launch mob's agent of the unlisted profile is listed its mob's shared record \
         and the realm's: {of_the_clerk}"
    );
    let of_the_greeter = built_by(&listing, launch_mob.clone(), &agent_of(GREETER, LISTED)).await;
    assert!(
        !of_the_greeter.contains(SHARED_BY_THE_LAUNCH_MOB)
            && !of_the_greeter.contains(SHARED_BY_THE_REALM),
        "the launch mob's agent of the listed profile is listed no shared record: \
         {of_the_greeter}"
    );

    let listing_none = crate::rpc_gateway::launch_mob_memory_customizer(
        None,
        provider,
        config,
        listed(&[]),
        &launch_mob,
        &identities,
    );
    let unlisted_greeter = built_by(&listing_none, launch_mob, &agent_of(GREETER, LISTED)).await;
    assert!(
        unlisted_greeter.contains(SHARED_BY_THE_LAUNCH_MOB)
            && unlisted_greeter.contains(SHARED_BY_THE_REALM),
        "with no profile listed the same agent is listed both: {unlisted_greeter}"
    );
    runtime.shutdown().await;
}

/// A source text without its white space, so that a search does not depend
/// on how the formatter breaks a call.
fn without_white_space(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Both launches give their launch mob's customizer the list their start
/// checked, read through their own identity runtime, and hand the same
/// list over for the mobs they host. The stdio launch has no launch a row
/// can run in this process: this row reads its text. It makes its launch
/// mob's customizer by the one function the row above runs, called once,
/// with the list its start checked and its own identity runtime, and it
/// calls the constructor of a mob's memory customizer nowhere else.
#[test]
fn both_launches_give_their_launch_mobs_customizer_the_list_their_start_checked() {
    let gateway = without_white_space(include_str!("../rpc_gateway.rs"));
    let checked = gateway.find(
        "letown_memory_only_profiles=matchgateway_options.agent_memory.as_ref(){\
         Some(agent_memory)=>crate::memory::own_memory_only::own_memory_only_profiles_of(\
         &mob_definition,&agent_memory.own_memory_only_profiles,)",
    );
    let called = gateway.find(
        "launch_mob_memory_customizer(base_customizer.clone(),provider,\
         agent_memory.config.clone(),own_memory_only_profiles.clone(),\
         &runtime.mob_handle(),&irt,)",
    );
    assert!(
        matches!((checked, called), (Some(checked), Some(called)) if checked < called),
        "rpc_gateway.rs: the stdio launch checks the list against its launch mob's definition, \
         then makes its launch mob's customizer with that list, the launch mob's handle and \
         its own identity runtime"
    );
    assert_eq!(
        gateway.matches("launch_mob_memory_customizer(").count(),
        2,
        "rpc_gateway.rs: the function and its one call"
    );
    assert!(
        gateway.contains(
            "crate::gateway_wiring::mob_memory_customizer(base,provider,config,\
             std::collections::BTreeMap::new(),Some(launch_mob),\
             crate::memory::own_memory_only::OwnMemoryOnly::listing(own_memory_only_profiles,\
             crate::memory::own_memory_only::RecordedLineage::of(identities),),)"
        ),
        "rpc_gateway.rs: the function gives the constructor the rule over the list it was \
         given and the records of the runtime it was given"
    );
    assert_eq!(
        gateway
            .matches("gateway_wiring::mob_memory_customizer(")
            .count(),
        1,
        "rpc_gateway.rs calls the constructor of a mob's memory customizer in that function \
         alone"
    );

    let builder = without_white_space(include_str!("builder.rs"));
    let composed = builder.find(
        "self.composed_agent_customizer(agent_memory_provider.clone(),\
         crate::memory::own_memory_only::OwnMemoryOnly::listing(\
         own_memory_only_profiles.clone(),own_memory_only_lineage.clone(),),)",
    );
    let bound = builder.find("if!own_memory_only_lineage.bind(&identity_runtime){");
    let given = builder.find(".set_agent_customizer(agent_customizer.clone())");
    assert!(
        matches!((composed, bound, given), (Some(composed), Some(bound), Some(given))
            if composed < bound && bound < given),
        "unified_runtime/builder.rs: the launch composes its customizer with the rule over \
         its list, binds the rule's record to its identity runtime, and only then gives the \
         customizer to that runtime"
    );

    for (file, source) in [
        ("rpc_gateway.rs", &gateway),
        ("unified_runtime/builder.rs", &builder),
    ] {
        assert_eq!(
            source
                .matches(".with_own_memory_only_profiles(own_memory_only_profiles.clone())")
                .count(),
            1,
            "{file}: the launch hands its checked list over for the mobs it hosts"
        );
    }
}
