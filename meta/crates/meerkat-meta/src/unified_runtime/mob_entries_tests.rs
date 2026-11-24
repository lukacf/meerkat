//! Rows of the runtime's entries for the mobs its mob set builds for the
//! host beside the default mob.
//!
//! The fixture is a launch that declares its mob to its mob set, as the
//! gateways do: the set then builds a second mob of the host through the
//! launch's composition, and the runtime's seam is asked for that mob by its
//! id. Every launch lets a request resolve such a mob; no row opens
//! anything for it.

#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_mob::{MobDefinition, MobId, SpawnMemberSpec};

use super::{HostedMobListing, HostedMobPhase, MobResolveError, UnifiedRuntime};
use crate::blob_store::{Base64BlobStoreAdapter, ObjectStoreBlobStore};
use crate::identity_first::{
    AgentBuildContext, AgentBuildDraft, AgentCustomizer, AgentRuntimeServices, CustomizerError,
    DurabilityPolicy, DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime,
    IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, RosterContext, RosterError,
    RosterProvider,
};
use crate::mob_handle_runtime::{MobBootstrapOptions, MobBootstrapSpec};
use crate::runtime::{InMemoryMetadataStore, RuntimeOptions};
use crate::{DiscoverySpec, MetaConfig, MobRuntimeDelivery};

/// A definition with one profile under an id of its own.
pub(super) fn definition(tag: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        "[mob]\nid = \"mob-entries-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n[profiles.worker.tools]\ncomms = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with one profile")
}

/// The session service of a fixture launch: every mob of the host builds
/// its members' sessions in it.
pub(super) type Sessions = Arc<meerkat_session::PersistentSessionService<FactoryAgentBuilder>>;

/// A runtime whose launch declares the mob of `definition` to its mob set,
/// in memory: the set creates the launch's mob and can build further mobs
/// of the host. With it, the launch's session service.
pub(super) async fn launch(
    temp: &tempfile::TempDir,
    definition: MobDefinition,
) -> (UnifiedRuntime, Sessions) {
    let (runtime, service, _machine, ()) = launch_over(temp, definition, |_| ()).await;
    (runtime, service)
}

/// [`launch`], with what `prepare` makes of the launch's session builder
/// before any session is built on it (a gateway attaches its schedule tools
/// there), and with the launch's runtime machine.
pub(super) async fn launch_over<T>(
    temp: &tempfile::TempDir,
    definition: MobDefinition,
    prepare: impl FnOnce(&FactoryAgentBuilder) -> T,
) -> (
    UnifiedRuntime,
    Sessions,
    Arc<meerkat_runtime::MeerkatMachine>,
    T,
) {
    launch_composed(temp, definition, Config::default(), prepare, |spec| spec).await
}

/// [`launch_over`], with what `compose` makes of the launch's spec before
/// the bootstrap: a row that needs something a launch puts on its mobs
/// (a guard, a hook, a policy) says it there, as a gateway does.
pub(super) async fn launch_composed<T>(
    temp: &tempfile::TempDir,
    definition: MobDefinition,
    config: Config,
    prepare: impl FnOnce(&FactoryAgentBuilder) -> T,
    compose: impl FnOnce(MobBootstrapSpec) -> MobBootstrapSpec,
) -> (
    UnifiedRuntime,
    Sessions,
    Arc<meerkat_runtime::MeerkatMachine>,
    T,
) {
    launch_keeping(
        temp,
        definition,
        config,
        Arc::new(InMemoryMetadataStore::new()),
        prepare,
        compose,
    )
    .await
}

/// [`launch_composed`], keeping the host's records in `metadata`: a row
/// that starts a host again over the records an earlier launch left hands
/// both launches the same store.
pub(super) async fn launch_keeping<T>(
    temp: &tempfile::TempDir,
    definition: MobDefinition,
    config: Config,
    metadata: Arc<dyn crate::runtime::PersistentMetadataStore>,
    prepare: impl FnOnce(&FactoryAgentBuilder) -> T,
    compose: impl FnOnce(MobBootstrapSpec) -> MobBootstrapSpec,
) -> (
    UnifiedRuntime,
    Sessions,
    Arc<meerkat_runtime::MeerkatMachine>,
    T,
) {
    launch_stored(
        temp,
        definition,
        config,
        metadata,
        LaunchStore::Declared,
        prepare,
        compose,
    )
    .await
}

/// A launch that does NOT declare its mob to its mob set: it is built over
/// storage the launch opened itself, as a gateway builds a launch that
/// declares a definition update. Its mob set is given no composition, so
/// the launch hosts no mob beside its launch mob.
pub(super) async fn launch_over_opened_storage(
    temp: &tempfile::TempDir,
    definition: MobDefinition,
) -> UnifiedRuntime {
    launch_over_opened_storage_keeping(
        temp,
        definition,
        Arc::new(InMemoryMetadataStore::new()),
        |spec| spec,
    )
    .await
}

/// [`launch_over_opened_storage`] over the metadata store `metadata`, with
/// what the row composes onto the launch's spec.
pub(super) async fn launch_over_opened_storage_keeping(
    temp: &tempfile::TempDir,
    definition: MobDefinition,
    metadata: Arc<dyn crate::runtime::PersistentMetadataStore>,
    compose: impl FnOnce(MobBootstrapSpec) -> MobBootstrapSpec,
) -> UnifiedRuntime {
    let (runtime, _sessions, _machine, ()) = launch_stored(
        temp,
        definition,
        Config::default(),
        metadata,
        LaunchStore::Opened,
        |_| (),
        compose,
    )
    .await;
    runtime
}

/// How a row's launch holds its mob's store.
enum LaunchStore {
    /// Declared to the launch's mob set, which creates the mob there.
    Declared,
    /// Opened by the launch itself and handed to the bootstrap.
    Opened,
}

/// The launch every row's fixture is built from: [`launch_keeping`] over a
/// store the launch declares, [`launch_over_opened_storage`] over one it
/// opened.
async fn launch_stored<T>(
    temp: &tempfile::TempDir,
    definition: MobDefinition,
    config: Config,
    metadata: Arc<dyn crate::runtime::PersistentMetadataStore>,
    launch_store: LaunchStore,
    prepare: impl FnOnce(&FactoryAgentBuilder) -> T,
    compose: impl FnOnce(MobBootstrapSpec) -> MobBootstrapSpec,
) -> (
    UnifiedRuntime,
    Sessions,
    Arc<meerkat_runtime::MeerkatMachine>,
    T,
) {
    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blobs: Arc<dyn meerkat_core::BlobStore> = Arc::new(Base64BlobStoreAdapter::new(Arc::new(
        ObjectStoreBlobStore::memory(),
    )));
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(store.clone(), blobs.clone())
            .expect("the runtime machine"),
    );
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    let mut builder = FactoryAgentBuilder::new(
        AgentFactory::new(temp.path()).builtins(false).comms(true),
        config,
    );
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let prepared = prepare(&builder);
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        Arc::new(meerkat_store::MemoryStore::new()),
        store.clone(),
        blobs,
    ));
    let namespace = definition.id.to_string();
    let base = match launch_store {
        LaunchStore::Declared => MobBootstrapSpec::declared(
            definition,
            crate::mob_composition_manifest::DeclaredMobStore::in_memory(),
            service.clone(),
        ),
        LaunchStore::Opened => MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            service.clone(),
        ),
    };
    let spec = compose(
        base.with_session_runtime_adapter(Arc::clone(&machine))
            .expect("the launch adopts the runtime machine")
            .with_runtime_delivery(MobRuntimeDelivery::new(store, jobs))
            .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
            .expect("the agent mob tools")
            .with_child_mob_storage(crate::ChildMobStorage::ProcessBound)
            .with_options(MobBootstrapOptions {
                allow_ephemeral_sessions: false,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
            }),
    );
    let runtime = Box::pin(UnifiedRuntime::bootstrap_with_options(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace,
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        Duration::from_secs(2),
        RuntimeOptions::default(),
        metadata,
    ))
    .await
    .expect("a runtime over a declared store");
    (runtime, service, machine, prepared)
}

/// [`launch`], for a row that reads no session.
async fn host(temp: &tempfile::TempDir, definition: MobDefinition) -> UnifiedRuntime {
    launch(temp, definition).await.0
}

/// The launch's mob set.
pub(super) fn set_of(host: &UnifiedRuntime) -> Arc<meerkat_mob_mcp::MobMcpState> {
    host.mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set")
}

/// Have `host`'s mob set create a second mob of the host from `definition`.
pub(super) async fn second_mob(host: &UnifiedRuntime, definition: MobDefinition) -> MobId {
    Box::pin(set_of(host).mob_create_definition(definition))
        .await
        .expect("the mob set creates a second mob of the host")
}

/// Shut `mob_id` down through the set, then the runtime.
pub(super) async fn shut_down(host: &UnifiedRuntime, others: &[&MobId]) {
    let set = set_of(host);
    for mob_id in others {
        if let Ok(handle) = set.handle_for(mob_id).await {
            let _ = handle.shutdown().await;
        }
    }
    tokio::time::timeout(Duration::from_secs(30), host.shutdown())
        .await
        .expect("the runtime shuts down");
}

/// The public names of the members `handle`'s mob holds.
async fn roster(handle: &meerkat_mob::MobHandle) -> std::collections::BTreeSet<String> {
    handle
        .list_members_including_retiring()
        .await
        .iter()
        .map(|member| {
            crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str()).into_owned()
        })
        .collect()
}

fn not_hosted(requested: &MobId, default_mob: &MobId) -> MobResolveError {
    MobResolveError::NotHosted {
        requested: requested.clone(),
        default_mob: default_mob.clone(),
    }
}

/// A mob the launch's mob set built for the host beside the default mob
/// resolves, by its id, to an entry of its own: not the default mob's, the
/// same one for every request while the set manages the mob, over the
/// handle the set serves. The question whether the mob is served and a
/// door's resolver answer the same mob, and a request that names no mob
/// keeps the default mob.
#[tokio::test]
async fn a_mob_the_set_built_for_the_host_resolves_to_an_entry_of_its_own() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    assert_eq!(second_id, second_definition.id);

    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        assert!(!mob.is_default());
        assert_eq!(mob.mob_id(), &second_id);
        assert_eq!(mob.mob_handle().mob_id(), &second_id);
        assert!(std::ptr::eq(mob.runtime(), &raw const host));
        let default_mob = host.resolve_mob(None).await.expect("the default mob");
        assert!(default_mob.is_default());
        assert_eq!(default_mob.mob_id(), &default_id);
        assert!(
            !std::ptr::eq(mob.entry(), default_mob.entry()),
            "the second mob's entry is its own"
        );
        let again = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob resolves again");
        assert!(
            std::ptr::eq(mob.entry(), again.entry()),
            "one entry for a mob while the set manages it"
        );
        let served = set_of(&host)
            .handle_for(&second_id)
            .await
            .expect("the set serves the second mob");
        assert_eq!(
            mob.mob_handle().incarnation(),
            served.incarnation(),
            "the entry is over the mob the set serves"
        );
    }
    assert_eq!(
        host.mob_entries_in_tests(),
        vec![second_id.clone()],
        "the runtime holds the second mob's entry, and no other"
    );
    assert_eq!(
        host.served_mob_id(Some(&second_id)).await,
        Ok(second_id.clone()),
        "the question whether the mob is served agrees"
    );
    let door = host.mob_resolver();
    assert_eq!(
        door.served_mob_id(Some(&second_id)).await,
        Ok(second_id.clone())
    );
    let through_a_door = door.resolve(Some(&second_id)).await;
    assert!(
        through_a_door
            .as_ref()
            .is_ok_and(|mob| mob.mob_id() == &second_id && !mob.is_default()),
        "a door's resolver answers the second mob: {:?}",
        through_a_door.as_ref().err()
    );
    assert_eq!(host.served_mob_id(None).await, Ok(default_id));
    shut_down(&host, &[&second_id]).await;
}

/// Two mobs of one host with a member of one name: a member seated through
/// the resolved second mob joins that mob's own roster, a member seated by
/// the runtime's own method joins the default mob's, and the two members
/// are two sessions.
#[tokio::test]
async fn two_mobs_of_one_host_seat_a_member_of_one_name_each_in_its_own_roster() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");

    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let seated = Box::pin(mob.spawn(SpawnMemberSpec::new("worker", "desk"))).await;
        assert!(
            seated.is_ok(),
            "the spawn through the resolved mob: {:?}",
            seated.as_ref().err()
        );
    }
    assert!(
        roster(&second).await.contains("desk"),
        "the member is seated in the second mob's own roster"
    );
    assert!(
        !roster(&host.mob_handle()).await.contains("desk"),
        "and not in the default mob's"
    );
    let of_the_default = Box::pin(host.spawn(SpawnMemberSpec::new("worker", "desk"))).await;
    assert!(
        of_the_default.is_ok(),
        "the runtime's own method seats a member of the same name in the default mob: {:?}",
        of_the_default.as_ref().err()
    );
    let desk = meerkat_mob::AgentIdentity::from("desk");
    let (in_the_default, in_the_second) = (
        host.mob_handle().resolve_bridge_session_id(&desk).await,
        second.resolve_bridge_session_id(&desk).await,
    );
    assert!(
        in_the_default.is_some() && in_the_second.is_some() && in_the_default != in_the_second,
        "two members, one name: {in_the_default:?}, {in_the_second:?}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A mob's entry goes when the mob set no longer manages the mob: after a
/// destroy through the set the id resolves no mob and the runtime holds no
/// entry for it. A mob created again under the same id gets an entry of
/// its own, over the new mob.
#[tokio::test]
async fn the_entry_of_a_mob_goes_when_the_mob_set_no_longer_manages_the_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    let first_incarnation = host
        .resolve_mob(Some(&second_id))
        .await
        .map(|mob| mob.mob_handle().incarnation());
    assert!(
        first_incarnation.is_ok(),
        "the second mob of the host resolves: {:?}",
        first_incarnation.as_ref().err()
    );
    assert_eq!(host.mob_entries_in_tests(), vec![second_id.clone()]);
    let part_of_the_first = match host.resolve_mob(Some(&second_id)).await {
        Ok(mob) => mob.entry().cross_mob_routes.get().cloned(),
        Err(_) => None,
    };
    assert!(
        part_of_the_first
            .as_ref()
            .is_some_and(super::cross_mob_routes::is_the_part_of_its_mob),
        "the running second mob holds its part of the link table"
    );

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        host.resolve_mob(Some(&second_id)).await.err(),
        Some(not_hosted(&second_id, &default_id)),
        "a destroyed mob resolves no mob"
    );
    assert_eq!(
        host.served_mob_id(Some(&second_id)).await,
        Err(not_hosted(&second_id, &default_id))
    );
    assert_eq!(
        host.mob_entries_in_tests(),
        Vec::<MobId>::new(),
        "the entry went with the mob"
    );
    assert!(
        part_of_the_first
            .as_ref()
            .is_some_and(|part| !super::cross_mob_routes::is_the_part_of_its_mob(part)),
        "and the mob's part of the link table went with its entry"
    );

    let again = second_mob(&host, second_definition).await;
    assert_eq!(again, second_id);
    let second_incarnation = host
        .resolve_mob(Some(&second_id))
        .await
        .map(|mob| mob.mob_handle().incarnation());
    assert!(
        second_incarnation.is_ok(),
        "a mob created again under the id resolves: {:?}",
        second_incarnation.as_ref().err()
    );
    assert_ne!(
        first_incarnation.ok(),
        second_incarnation.ok(),
        "its entry is over the new mob"
    );
    assert_eq!(host.mob_entries_in_tests(), vec![second_id.clone()]);
    shut_down(&host, &[&second_id]).await;
}

/// A second mob's entry is built with a part of the link table of its own:
/// the start-time read of the second mob's links fills the second entry's
/// cell, and the default mob's cell is what it was.
#[tokio::test]
async fn the_start_time_read_of_a_second_mob_fills_its_own_entrys_part_of_the_link_table() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let of_the_default = host
        .resolve_mob(None)
        .await
        .expect("the default mob")
        .entry()
        .cross_mob_routes
        .get()
        .cloned();
    assert!(
        of_the_default.is_some(),
        "the default mob ran its start-time read"
    );
    let second_id = second_mob(&host, definition("second")).await;

    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let of_the_second = mob.entry().cross_mob_routes.get().cloned();
        assert!(
            of_the_second.is_some(),
            "the second mob's start-time read filled its own entry's cell"
        );
        assert!(
            !of_the_second
                .zip(of_the_default.clone())
                .is_some_and(|(second, default)| Arc::ptr_eq(&second, &default)),
            "the second mob's part is not the default mob's"
        );
    }
    let of_the_default_after = host
        .resolve_mob(None)
        .await
        .expect("the default mob")
        .entry()
        .cross_mob_routes
        .get()
        .cloned();
    assert!(
        of_the_default
            .zip(of_the_default_after)
            .is_some_and(|(before, after)| Arc::ptr_eq(&before, &after)),
        "the default mob's part is what it was"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A mob a member created is a mob of the member's, never one the host
/// serves requests for: its id resolves no mob, the question whether it is
/// served says so, and the runtime builds no entry for it. A guard: it
/// holds before and after the second mob of a host resolves.
#[tokio::test]
async fn a_mob_a_member_created_is_served_on_no_request() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let owner = meerkat_mob::AgentIdentity::from("owner");
    let seated = Box::pin(host.spawn(SpawnMemberSpec::new("worker", "owner"))).await;
    assert!(
        seated.is_ok(),
        "the owner member: {:?}",
        seated.as_ref().err()
    );
    let owner_session = host
        .mob_handle()
        .resolve_bridge_session_id(&owner)
        .await
        .expect("the owner member's session");
    let child_id = Box::pin(
        set_of(&host).mob_create_definition_with_owner_bridge_session(
            definition("child"),
            owner_session,
            true,
            false,
        ),
    )
    .await
    .expect("a mob the member owns");

    assert_eq!(
        host.resolve_mob(Some(&child_id)).await.err(),
        Some(not_hosted(&child_id, &default_id)),
        "a mob a member created resolves no mob"
    );
    assert_eq!(
        host.served_mob_id(Some(&child_id)).await,
        Err(not_hosted(&child_id, &default_id))
    );
    assert_eq!(
        host.mob_resolver().resolve(Some(&child_id)).await.err(),
        Some(not_hosted(&child_id, &default_id))
    );
    assert_eq!(host.mob_entries_in_tests(), Vec::<MobId>::new());
    shut_down(&host, &[&child_id]).await;
}

/// What the table keeps of a mob the host created, until a pass makes that
/// mob's entry, is that mob's and no later mob's. The host's mob is
/// destroyed before any pass made its entry (the row holds the table's
/// follower), and a member then creates a mob under its id. That mob is the
/// member's: a pass builds no entry for it, no request resolves it, the
/// question whether it is served says so, the listing of the host's mobs
/// leaves it out, and it has no console events of its own with the host,
/// which is how the console tells a mob a member made: the mob is among
/// the mobs below the launch's registration, where a console's walk over
/// the members of such mobs finds it.
#[tokio::test]
async fn a_members_mob_under_the_id_of_a_hosts_mob_that_got_no_entry_is_served_on_no_request() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let owner = meerkat_mob::AgentIdentity::from("owner");
    let seated = Box::pin(host.spawn(SpawnMemberSpec::new("worker", "owner"))).await;
    assert!(
        seated.is_ok(),
        "the owner member: {:?}",
        seated.as_ref().err()
    );
    let owner_session = host
        .mob_handle()
        .resolve_bridge_session_id(&owner)
        .await
        .expect("the owner member's session");
    host.hold_the_follower_of_host_mobs_in_tests().await;
    let of_the_host = definition("taken-again");
    let of_the_member = of_the_host.clone();
    let id = of_the_host.id.clone();
    let set = set_of(&host);

    let created = Box::pin(set.mob_create_definition(of_the_host))
        .await
        .map_err(|error| error.to_string());
    let entries_at_the_destroy = host.mob_entries_in_tests();
    // The handle goes before the destroy, as for every destroy.
    let incarnation = set
        .handle_for(&id)
        .await
        .map(|handle| handle.incarnation())
        .map_err(|error| error.to_string());
    let destroyed = match &incarnation {
        Ok(incarnation) => Box::pin(set.mob_destroy_incarnation(&id, *incarnation))
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}")),
        Err(error) => Err(error.clone()),
    };
    let anew = Box::pin(set.mob_create_definition_with_owner_bridge_session(
        of_the_member,
        owner_session,
        true,
        false,
    ))
    .await
    .map_err(|error| error.to_string());

    // The listing first: it loads nothing and runs no pass. So with the
    // mobs that have console events of their own with the host.
    let listed = host.mob_resolver().hosted_mobs().await;
    let with_console_events = host.console_events().mobs_with_a_store();
    let below_the_launchs_registration =
        crate::console_aggregator::mobs_below_a_host_registration_in_tests(&host).await;
    let served = host.served_mob_id(Some(&id)).await;
    let resolved = host.resolve_mob(Some(&id)).await.err();
    let at_a_door = host.mob_resolver().resolve(Some(&id)).await.err();
    let entries = host.mob_entries_in_tests();
    // What a request is answered that names an id no mob has.
    let no_mob = MobId::from("an-id-no-mob-of-this-host-has");
    let of_no_mob = host.resolve_mob(Some(&no_mob)).await.err();
    shut_down(&host, &[&id]).await;

    assert!(
        created.is_ok() && destroyed.is_ok() && anew.is_ok(),
        "the host creates a mob, destroys it, and a member creates one under its id: \
         {created:?}, {destroyed:?}, {anew:?}"
    );
    assert_eq!(
        entries_at_the_destroy,
        Vec::<MobId>::new(),
        "no pass made the entry of the host's mob"
    );
    assert_eq!(
        served,
        Err(not_hosted(&id, &default_id)),
        "the member's mob is not one the host serves"
    );
    assert_eq!(
        resolved,
        Some(not_hosted(&id, &default_id)),
        "no request resolves the member's mob"
    );
    assert_eq!(
        of_no_mob,
        Some(not_hosted(&no_mob, &default_id)),
        "and the answer is the one for an id no mob of the host has"
    );
    assert_eq!(at_a_door, Some(not_hosted(&id, &default_id)));
    assert_eq!(
        listed,
        Vec::new(),
        "the member's mob is not listed among the mobs the host serves"
    );
    assert_eq!(
        with_console_events,
        Vec::<MobId>::new(),
        "the member's mob has no console events of its own with the host"
    );
    assert!(
        below_the_launchs_registration.contains(&id),
        "the member's mob is among the mobs below the launch's registration: \
         {below_the_launchs_registration:?}"
    );
    assert_eq!(
        entries,
        Vec::<MobId>::new(),
        "the runtime built no entry for the member's mob"
    );
}

/// A pass takes a mob's wiring out of the table's record of builds and
/// puts the mob's entry in only after it built it. For that time the mob
/// is the host's as before and after: the row stands a request's pass
/// still there (the table's follower is held), and the question that loads
/// nothing, a door's and the listing of the host's mobs answer what the
/// request then finds.
#[tokio::test]
async fn a_mob_whose_entry_a_pass_is_making_is_served_and_listed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    host.hold_the_follower_of_host_mobs_in_tests().await;
    let named = second_mob(&host, definition("its-entry-being-made")).await;
    host.stand_the_next_pass_of_host_mobs_still_in_tests();

    let lookup = host.resolve_mob(Some(&named));
    tokio::pin!(lookup);
    let resolved_before_a_pass_stood_still = tokio::select! {
        biased;
        () = host.a_pass_of_host_mobs_stands_still_in_tests() => None,
        resolved = &mut lookup => Some(resolved.map(|mob| mob.mob_handle().mob_id().clone())),
    };
    let entries_while_it_stands = host.mob_entries_in_tests();
    let served = host.served_mob_id(Some(&named)).await;
    let at_a_door = host.mob_resolver().served_mob_id(Some(&named)).await;
    let listed: Vec<(MobId, HostedMobPhase)> = host
        .mob_resolver()
        .hosted_mobs()
        .await
        .into_iter()
        .map(|mob| (mob.mob_id, mob.phase))
        .collect();
    host.end_the_standstill_of_host_mobs_in_tests();
    let resolved = match resolved_before_a_pass_stood_still {
        Some(resolved) => resolved,
        None => lookup.await.map(|mob| mob.mob_handle().mob_id().clone()),
    };
    shut_down(&host, &[&named]).await;

    assert_eq!(
        entries_while_it_stands,
        Vec::<MobId>::new(),
        "the pass stands between the wiring it took and the entry it has not put in"
    );
    assert_eq!(resolved, Ok(named.clone()), "the request resolves the mob");
    assert_eq!(
        served,
        Ok(named.clone()),
        "the host answers that it serves the mob while a pass is making its entry"
    );
    assert_eq!(
        at_a_door,
        Ok(named.clone()),
        "and so does a door's resolver"
    );
    assert_eq!(
        listed,
        vec![(named.clone(), HostedMobPhase::Loaded)],
        "the mob is listed among the mobs the host serves meanwhile"
    );
}

/// A pass runs to its end whoever asked for it. A request that is dropped
/// while its lookup's pass is making a mob's entry does not take the mob's
/// wiring with it: the row drops the lookup while its pass stands still
/// between the wiring it took and the entry (the table's follower is
/// held), and the next request for the mob is served.
#[tokio::test]
async fn a_request_dropped_while_its_pass_makes_an_entry_leaves_the_mob_served() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    host.hold_the_follower_of_host_mobs_in_tests().await;
    let named = second_mob(&host, definition("its-request-dropped")).await;
    host.stand_the_next_pass_of_host_mobs_still_in_tests();

    let resolved_before_a_pass_stood_still = {
        let lookup = host.resolve_mob(Some(&named));
        tokio::pin!(lookup);
        tokio::select! {
            biased;
            () = host.a_pass_of_host_mobs_stands_still_in_tests() => false,
            _ = &mut lookup => true,
        }
        // The lookup is dropped here, while its pass stands still.
    };
    let entries_while_it_stands = host.mob_entries_in_tests();
    host.end_the_standstill_of_host_mobs_in_tests();
    let later = host
        .resolve_mob(Some(&named))
        .await
        .map(|mob| mob.mob_handle().mob_id().clone());
    let entries = host.mob_entries_in_tests();
    shut_down(&host, &[&named]).await;

    assert!(
        !resolved_before_a_pass_stood_still,
        "the lookup's pass stood still before the lookup resolved"
    );
    assert_eq!(
        entries_while_it_stands,
        Vec::<MobId>::new(),
        "the pass stood between the wiring it took and the entry it had not put in"
    );
    assert_eq!(
        later,
        Ok(named.clone()),
        "a later request for the mob is served"
    );
    assert_eq!(entries, vec![named], "and the mob has its entry");
}

/// A pass runs on a task of the table's own, and the table's close joins
/// it. The row begins the runtime's shutdown while a pass stands still
/// between the wiring it took and the entry (the request that asked for
/// it is dropped): the shutdown does not end while the pass stands, and
/// once the pass went on the shutdown ends with no pass left running and
/// no entry in the table: the entry the pass put in went with the close.
///
/// The row holds from the change that gives a pass its task; before it a
/// pass was part of the request that asked for it, and there was no task
/// to join.
#[tokio::test]
async fn a_shutdown_begun_while_a_pass_makes_an_entry_joins_the_pass() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    host.hold_the_follower_of_host_mobs_in_tests().await;
    let named = second_mob(&host, definition("its-pass-joined")).await;
    host.stand_the_next_pass_of_host_mobs_still_in_tests();

    let resolved_before_a_pass_stood_still = {
        let lookup = host.resolve_mob(Some(&named));
        tokio::pin!(lookup);
        tokio::select! {
            biased;
            () = host.a_pass_of_host_mobs_stands_still_in_tests() => false,
            _ = &mut lookup => true,
        }
    };
    let passes_while_it_stands = host.pass_tasks_of_host_mobs_running_in_tests();
    // The mob is shut down through the set first, as every row does before
    // the runtime's shutdown.
    if let Ok(handle) = set_of(&host).handle_for(&named).await {
        let _ = handle.shutdown().await;
    }
    let shutdown = host.shutdown();
    tokio::pin!(shutdown);
    let ended_while_the_pass_stood =
        tokio::time::timeout(Duration::from_millis(300), &mut shutdown)
            .await
            .is_ok();
    host.end_the_standstill_of_host_mobs_in_tests();
    let ended = tokio::time::timeout(Duration::from_secs(30), &mut shutdown)
        .await
        .is_ok();
    let passes_left = host.pass_tasks_of_host_mobs_running_in_tests();
    let entries_left = host.mob_entries_in_tests();

    assert!(
        !resolved_before_a_pass_stood_still,
        "the lookup's pass stood still before the lookup resolved"
    );
    assert_eq!(
        passes_while_it_stands, 1,
        "the pass runs on a task the table holds"
    );
    assert!(
        !ended_while_the_pass_stood,
        "the shutdown waits for the pass that is making an entry"
    );
    assert!(ended, "the shutdown ends once the pass went on");
    assert_eq!(passes_left, 0, "no pass is left running");
    assert_eq!(
        entries_left,
        Vec::<MobId>::new(),
        "the entry the pass put in went with the close"
    );
}

/// A pass that is under way when the runtime begins to shut down builds no
/// further entry: it puts in the one it is making, which the close ends,
/// and takes no other mob's wiring. The row lets two mobs of the host wait
/// for their entries (the table's follower is held), stands a request's
/// pass still in the first of them, begins the runtime's shutdown and lets
/// the pass go on: the table's passes put one entry in, not two.
#[tokio::test]
async fn a_pass_under_way_when_a_shutdown_begins_makes_no_further_entry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    host.hold_the_follower_of_host_mobs_in_tests().await;
    let one = second_mob(&host, definition("waits-for-its-entry")).await;
    let other = second_mob(&host, definition("waits-for-its-entry-too")).await;
    host.stand_the_next_pass_of_host_mobs_still_in_tests();

    let resolved_before_a_pass_stood_still = {
        let lookup = host.resolve_mob(Some(&one));
        tokio::pin!(lookup);
        tokio::select! {
            biased;
            () = host.a_pass_of_host_mobs_stands_still_in_tests() => false,
            _ = &mut lookup => true,
        }
    };
    let put_in_while_it_stands = host.entries_put_in_by_passes_of_host_mobs_in_tests();
    // The mobs are shut down through the set first, as every row does
    // before the runtime's shutdown.
    for named in [&one, &other] {
        if let Ok(handle) = set_of(&host).handle_for(named).await {
            let _ = handle.shutdown().await;
        }
    }
    let shutdown = host.shutdown();
    tokio::pin!(shutdown);
    let ended_while_the_pass_stood =
        tokio::time::timeout(Duration::from_millis(300), &mut shutdown)
            .await
            .is_ok();
    host.end_the_standstill_of_host_mobs_in_tests();
    let ended = tokio::time::timeout(Duration::from_secs(30), &mut shutdown)
        .await
        .is_ok();
    let put_in = host.entries_put_in_by_passes_of_host_mobs_in_tests();
    let entries_left = host.mob_entries_in_tests();

    assert!(
        !resolved_before_a_pass_stood_still,
        "the lookup's pass stood still before the lookup resolved"
    );
    assert_eq!(
        put_in_while_it_stands, 0,
        "both mobs wait for their entries while the pass stands in the first"
    );
    assert!(
        !ended_while_the_pass_stood,
        "the shutdown waits for the pass that is making an entry"
    );
    assert!(ended, "the shutdown ends once the pass went on");
    assert_eq!(
        put_in, 1,
        "the pass put in the entry it was making and made none for the mob that still \
         waited when the shutdown began"
    );
    assert_eq!(
        entries_left,
        Vec::<MobId>::new(),
        "the entry the pass put in went with the close"
    );
}

/// Source guard for what the table keeps of a mob between its build and
/// the pass that makes its entry (the `building` record). A mob that is
/// unloaded or destroyed before that pass leaves its part there, under its
/// id, and the table is told of neither: it learns them from a pass, to
/// which a part left behind looks as the part of a mob still being built
/// does. Two rules keep what is left from deciding anything.
///
/// It never meets another mob: the release every create passes drops it,
/// and ends the console events the build named with the host, before the
/// new mob exists.
///
/// For the mob it was made for it serves nothing by the id alone: the pass
/// and the question whether a mob is served take it only together with a
/// handle the mob set hands out now, and the listing reads it beside the
/// mobs the set manages or holds a store of. The two places that read it by
/// the id alone (the host's binding, which names the console events of the
/// mobs built before it; the end of an entry, which asks whether the mob
/// is being built again) decide only whether that mob of the host keeps
/// console events of its own.
///
/// A new place that reads the record is added here on purpose, with what
/// it reads beside the id.
#[test]
fn what_the_table_keeps_for_a_mobs_entry_serves_no_mob_by_its_id_alone() {
    let source = include_str!("mob_entries.rs");
    assert_eq!(
        source.matches("locked(&self.building)").count(),
        9,
        "the places that read or write the record: the launch's topology binding (is it \
         empty), the host's binding, the pass, the end of an entry, the question whether a \
         mob is served, the listing, the table's close, the release before a create and \
         the build"
    );
    assert!(
        source.contains("locked(&self.building).remove(mob_id);")
            && source.contains("host.console_events.end_of_mob(mob_id);"),
        "the release before a create drops what is left under the id, and ends the \
         console events named under it"
    );
    let start = source
        .find("\n    pub(super) async fn serves_read(")
        .expect("the question whether a mob is served");
    let rest = &source[start..];
    let end = rest.find("\n    }\n").expect("the question ends");
    let serves = &rest[..end];
    assert!(
        serves.contains("mob_handles_snapshot()") && !serves.contains("mob_list()"),
        "the question reads the record together with a handle the mob set hands out, as \
         the pass does, and not with the listing alone"
    );
    // The question holds nothing between its looks, so it looks for the
    // mob on both sides of the handles: a create that ran to its end in
    // between dropped what was left before its mob got a handle. And it
    // reads in the order a pass moves a mob, the mobs that wait for their
    // entry before the entries, so a mob whose entry a pass is making is
    // found in one of them.
    let handles_read = serves
        .find("mob_handles_snapshot()")
        .expect("the question reads the handles");
    assert!(
        serves[..handles_read].contains("waits_for_its_entry(named)")
            && serves[handles_read..].contains("waits_for_its_entry(named)"),
        "the question looks at the record before it reads the handles and again after"
    );
    let first_wait = serves.find("waits_for_its_entry(named)");
    let first_entry = serves.find("holds_the_entry_of(named)");
    assert!(
        first_wait.is_some() && first_wait < first_entry,
        "the question reads the mobs that wait for their entry before the entries: \
         {first_wait:?}, {first_entry:?}"
    );
    assert!(
        source.contains("locked(&self.making).insert(mob_id.clone());")
            && source.contains("locked(&self.making).contains(named)"),
        "a pass enters a mob among the entries being made, and the question reads them"
    );
    // Where the release's two removals stand: under the hold of the
    // passes, the wiring first, then the end of an earlier entry or of the
    // console events named under the id, all before the first release
    // that can refuse the create.
    let start = source
        .find("\n    async fn release_before_create(")
        .expect("the release before a create");
    let rest = &source[start..];
    let end = rest.find("\n    }\n").expect("the release ends");
    let release = &rest[..end];
    let place = |text: &str| release.find(text);
    let hold = place("self.pass.lock().await");
    let wiring_dropped = place("locked(&self.building).remove(mob_id);");
    let entry_ended = place("self.end(host, &entry).await");
    let console_events_ended = place("host.console_events.end_of_mob(mob_id);");
    let first_that_can_refuse = place(".release_mob_records(");
    assert!(
        hold.is_some()
            && hold < wiring_dropped
            && wiring_dropped < entry_ended
            && entry_ended < console_events_ended
            && console_events_ended < first_that_can_refuse,
        "the release takes the hold of the passes, drops the wiring, ends an earlier entry \
         or the console events named under the id, and only then asks a release that can \
         refuse: {hold:?}, {wiring_dropped:?}, {entry_ended:?}, {console_events_ended:?}, \
         {first_that_can_refuse:?}"
    );
}

/// Every launch lets a request resolve a mob of its host beside the default
/// mob: on a runtime as a launch builds it, where the row sets nothing, a
/// second mob of the host resolves as itself, at the runtime's seam and at
/// a door's resolver alike, and a request that names no mob keeps the
/// default mob.
#[tokio::test]
async fn a_request_resolves_a_second_mob_on_a_runtime_as_a_launch_builds_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let second_id = second_mob(&host, definition("second")).await;

    let at_the_seam = host.resolve_mob(Some(&second_id)).await;
    assert!(
        at_the_seam
            .as_ref()
            .is_ok_and(|mob| mob.mob_id() == &second_id && !mob.is_default()),
        "the second mob of the host resolves at the runtime's seam: {:?}",
        at_the_seam.as_ref().err()
    );
    assert_eq!(
        host.served_mob_id(Some(&second_id)).await,
        Ok(second_id.clone())
    );
    let door = host.mob_resolver();
    let at_a_door = door.resolve(Some(&second_id)).await;
    assert!(
        at_a_door
            .as_ref()
            .is_ok_and(|mob| mob.mob_id() == &second_id && !mob.is_default()),
        "the second mob of the host resolves at a door's resolver: {:?}",
        at_a_door.as_ref().err()
    );
    assert_eq!(
        door.served_mob_id(Some(&second_id)).await,
        Ok(second_id.clone())
    );
    assert_eq!(host.served_mob_id(None).await, Ok(default_id.clone()));
    assert_eq!(door.served_mob_id(None).await, Ok(default_id));
    shut_down(&host, &[&second_id]).await;
}

/// A host's roster provider that configures no agent.
struct NoAgents;

#[async_trait::async_trait]
impl RosterProvider for NoAgents {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(Vec::new())
    }
}

/// A host customizer that changes nothing: with one, an identity runtime
/// has a customizer tool registry.
struct PlainCustomizer;

#[async_trait::async_trait]
impl AgentCustomizer for PlainCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
}

/// An identity context a host composes for one mob: an identity runtime
/// over that mob's own handle (`mob`, the mob's wrapper), with a customizer.
pub(super) fn identity_context(
    mob: &crate::mob_handle_runtime::MobRuntime,
    tag: &str,
) -> Arc<IdentityFirstRuntimeContext> {
    let handle = mob.handle().observer();
    let identity_runtime = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(
                LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: format!("mob-entries-{tag}"),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(crate::gateway_wiring::identity_session_bridge(
                mob,
                handle.clone(),
            ))),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(handle.clone())),
    );
    Arc::new(IdentityFirstRuntimeContext::new(
        identity_runtime,
        Arc::new(NoAgents),
        None,
        Some(Arc::new(PlainCustomizer)),
        Some(handle.definition().clone()),
    ))
}

/// The row of an agent a host configures: the name `name`, the fixture's
/// `worker` profile, nothing else.
pub(super) fn configured_agent(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: crate::identity_first::AgentIdentity::parse(name).expect("identity"),
        profile: meerkat_mob::ProfileName::from("worker"),
        addressability: crate::identity_first::AgentAddressability::Addressable,
        display_name: None,
        labels: std::collections::BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// Every mob of a host has an identity runtime of its own. A host attaches
/// an identity context it composed over the second mob's handle to the
/// resolved second mob: that mob then answers that identity runtime, at
/// the runtime's seam, on its wrapper and through a door's resolver, and
/// the default mob keeps answering the one its launch installed. Each
/// identity runtime has a customizer tool registry of its own. The default
/// mob's identity context is its runtime's to install: attaching one to
/// the resolved default mob is refused.
#[tokio::test]
async fn each_mob_of_a_host_has_an_identity_runtime_and_customizer_tools_of_its_own() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut host = host(&temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let of_the_default = identity_context(host.mob_runtime(), "default");
    let bootstrapped = Box::pin(
        host.install_and_bootstrap_identity_first_context(Arc::clone(&of_the_default), &[]),
    )
    .await;
    assert!(
        bootstrapped.is_ok(),
        "the default mob's identity bootstrap: {:?}",
        bootstrapped.as_ref().err()
    );

    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    if let Ok(mob) = resolved {
        let of_the_second = identity_context(mob.mob_runtime(), "second");
        let attached = mob
            .attach_identity_first_context(Arc::clone(&of_the_second))
            .await;
        assert!(
            attached.is_ok(),
            "a host attaches an identity context to its second mob: {:?}",
            attached.as_ref().err()
        );
        assert!(
            mob.identity_runtime_ref()
                .is_some_and(|runtime| Arc::ptr_eq(runtime, &of_the_second.runtime)),
            "the second mob answers the identity runtime attached to it"
        );
        assert!(
            mob.mob_runtime()
                .installed_identity_runtime()
                .is_some_and(|runtime| Arc::ptr_eq(&runtime, &of_the_second.runtime)),
            "and so does its wrapper's plane"
        );
        let through_a_door = host.mob_resolver().resolve(Some(&second_id)).await;
        assert!(
            through_a_door.is_ok_and(|mob| {
                mob.identity_runtime_ref()
                    .is_some_and(|runtime| Arc::ptr_eq(&runtime, &of_the_second.runtime))
            }),
            "and a door's resolver"
        );
        let default_mob = host.resolve_mob(None).await.expect("the default mob");
        assert!(
            default_mob
                .identity_runtime_ref()
                .is_some_and(|runtime| Arc::ptr_eq(runtime, &of_the_default.runtime)),
            "the default mob answers the identity runtime its launch installed"
        );
        assert!(
            host.mob_runtime()
                .installed_identity_runtime()
                .is_some_and(|runtime| Arc::ptr_eq(&runtime, &of_the_default.runtime))
        );
        let (tools_of_the_default, tools_of_the_second) = (
            of_the_default
                .runtime
                .installed_customizer_tool_registry()
                .await,
            of_the_second
                .runtime
                .installed_customizer_tool_registry()
                .await,
        );
        assert!(
            tools_of_the_default.is_some() && tools_of_the_second.is_some(),
            "each identity runtime has a customizer tool registry"
        );
        assert!(
            !tools_of_the_default
                .zip(tools_of_the_second)
                .is_some_and(|(default, second)| Arc::ptr_eq(&default, &second)),
            "and the two mobs do not share one"
        );
        let refused = default_mob
            .attach_identity_first_context(Arc::clone(&of_the_second))
            .await;
        assert!(
            refused.is_err(),
            "the default mob's identity context is its runtime's to install"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// The text of every message of the session `session`.
pub(super) async fn transcript(
    sessions: &Sessions,
    session: &meerkat_core::types::SessionId,
) -> String {
    use meerkat_core::service::{SessionHistoryQuery, SessionServiceHistoryExt};
    sessions
        .read_history(session, SessionHistoryQuery::default())
        .await
        .map(|page| format!("{:?}", page.messages))
        .unwrap_or_default()
}

/// [`transcript`] of `session`, read until it shows `shown` or twenty
/// seconds passed: a send is committed before its turn shows in the
/// session's history.
pub(super) async fn transcript_showing(
    sessions: &Sessions,
    session: &meerkat_core::types::SessionId,
    shown: &str,
) -> String {
    for _ in 0..200 {
        if transcript(sessions, session).await.contains(shown) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    transcript(sessions, session).await
}

/// Two mobs of one host with an agent of one name, and one send to each:
/// each send reaches the member of its own mob and not the other mob's,
/// and neither mob's identity runtime knows an agent of the other mob.
#[tokio::test]
async fn two_mobs_of_one_host_with_an_agent_of_one_name_each_get_their_own_send() {
    const TO_THE_DEFAULT: &str = "a word for the desk of the default mob";
    const TO_THE_SECOND: &str = "a word for the desk of the second mob";
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, sessions) = launch(&temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");
    let of_the_default = identity_context(host.mob_runtime(), "default");
    let bootstrapped = Box::pin(host.install_and_bootstrap_identity_first_context(
        Arc::clone(&of_the_default),
        &[configured_agent("desk"), configured_agent("front")],
    ))
    .await;
    assert!(
        bootstrapped.is_ok(),
        "the default mob's identity bootstrap: {:?}",
        bootstrapped.as_ref().err()
    );

    let resolved = host.resolve_mob(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    let Ok(mob) = resolved else {
        shut_down(&host, &[&second_id]).await;
        return;
    };
    let of_the_second = identity_context(mob.mob_runtime(), "second");
    let attached = mob
        .attach_identity_first_context(Arc::clone(&of_the_second))
        .await;
    assert!(
        attached.is_ok(),
        "a host attaches an identity context to its second mob: {:?}",
        attached.as_ref().err()
    );
    let seated = Box::pin(
        of_the_second.bootstrap_roster(&[configured_agent("desk"), configured_agent("back")]),
    )
    .await;
    assert!(
        seated.is_ok(),
        "the second mob's roster: {:?}",
        seated.as_ref().err()
    );

    let identity =
        |name: &str| crate::identity_first::AgentIdentity::parse(name).expect("identity");
    let text = |content: &str| meerkat_core::ContentInput::Text(content.to_string());
    let to_the_default = of_the_default
        .runtime
        .send(&identity("desk"), &text(TO_THE_DEFAULT))
        .await;
    assert!(
        to_the_default.is_ok(),
        "the send to the default mob's agent: {:?}",
        to_the_default.as_ref().err()
    );
    let to_the_second = of_the_second
        .runtime
        .send(&identity("desk"), &text(TO_THE_SECOND))
        .await;
    assert!(
        to_the_second.is_ok(),
        "the send to the second mob's agent: {:?}",
        to_the_second.as_ref().err()
    );

    let desk = meerkat_mob::AgentIdentity::from("desk");
    let (in_the_default, in_the_second) = (
        host.mob_handle().resolve_bridge_session_id(&desk).await,
        second.resolve_bridge_session_id(&desk).await,
    );
    assert!(
        in_the_default.is_some() && in_the_second.is_some() && in_the_default != in_the_second,
        "two members, one name: {in_the_default:?}, {in_the_second:?}"
    );
    if let (Some(in_the_default), Some(in_the_second)) = (in_the_default, in_the_second) {
        // Each member's own send first, so that both turns have begun
        // before either transcript is judged for the other mob's send.
        transcript_showing(&sessions, &in_the_default, TO_THE_DEFAULT).await;
        transcript_showing(&sessions, &in_the_second, TO_THE_SECOND).await;
        let (of_the_default_desk, of_the_second_desk) = (
            transcript(&sessions, &in_the_default).await,
            transcript(&sessions, &in_the_second).await,
        );
        assert!(
            of_the_default_desk.contains(TO_THE_DEFAULT)
                && !of_the_default_desk.contains(TO_THE_SECOND),
            "the default mob's member got its own send alone: {of_the_default_desk}"
        );
        assert!(
            of_the_second_desk.contains(TO_THE_SECOND)
                && !of_the_second_desk.contains(TO_THE_DEFAULT),
            "the second mob's member got its own send alone: {of_the_second_desk}"
        );
    }
    for (runtime, of_the_other_mob) in [
        (&of_the_default.runtime, "back"),
        (&of_the_second.runtime, "front"),
    ] {
        assert!(
            matches!(
                runtime.registered_spec(&identity(of_the_other_mob)).await,
                Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_))
            ),
            "an identity runtime knows no agent of the other mob: {of_the_other_mob}"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// What a door's listing shows of a mob of the host that is loaded.
fn listed_as_loaded(handle: &meerkat_mob::MobHandle) -> HostedMobListing {
    HostedMobListing {
        mob_id: handle.mob_id().clone(),
        provenance: meerkat_mob::MobProvenance::Host,
        phase: HostedMobPhase::Loaded,
        incarnation: Some(handle.incarnation()),
    }
}

/// A door's resolver lists the mobs the host serves beside its default mob
/// from the mob set's records, and answers a mob "if loaded" from the
/// entries the runtime holds: the second mob of the host is listed as
/// loaded, with its incarnation, and answered; the default mob is answered
/// and not listed; a mob a member created and an id no mob has are neither
/// listed nor answered; a destroyed mob is neither any more.
#[tokio::test]
async fn a_doors_resolver_lists_the_mobs_of_the_host_and_answers_the_loaded_ones() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let owner = meerkat_mob::AgentIdentity::from("owner");
    let seated = Box::pin(host.spawn(SpawnMemberSpec::new("worker", "owner"))).await;
    assert!(
        seated.is_ok(),
        "the owner member: {:?}",
        seated.as_ref().err()
    );
    let owner_session = host
        .mob_handle()
        .resolve_bridge_session_id(&owner)
        .await
        .expect("the owner member's session");
    let child_id = Box::pin(
        set_of(&host).mob_create_definition_with_owner_bridge_session(
            definition("child"),
            owner_session,
            true,
            false,
        ),
    )
    .await
    .expect("a mob the member owns");
    let second_id = second_mob(&host, definition("second")).await;
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );

    let door = host.mob_resolver();
    assert_eq!(
        door.hosted_mobs().await,
        vec![listed_as_loaded(&second)],
        "the second mob of the host, and neither the default mob nor a mob a member created"
    );
    assert!(
        door.resolve_if_loaded(&second_id)
            .is_some_and(|mob| mob.mob_id() == &second_id && !mob.is_default()),
        "a loaded mob of the host is answered"
    );
    assert!(
        door.resolve_if_loaded(&default_id)
            .is_some_and(|mob| mob.is_default()),
        "the default mob is loaded"
    );
    assert!(
        door.resolve_if_loaded(&child_id).is_none(),
        "a mob a member created is not answered"
    );
    assert!(
        door.resolve_if_loaded(&MobId::from("a-mob-this-host-does-not-have"))
            .is_none(),
        "an id no mob has is not answered"
    );

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert!(
        door.resolve_if_loaded(&second_id).is_none(),
        "a destroyed mob is not answered"
    );
    assert_eq!(
        door.hosted_mobs().await,
        Vec::new(),
        "and not listed any more"
    );
    shut_down(&host, &[&child_id]).await;
}

/// A host of one mob lists no mob. A runtime as a launch builds it, where
/// the row sets nothing, lists the second mob of its host from the mob
/// set's records, and answers that mob "if loaded" once a request resolved
/// it; its default mob is answered and not listed.
#[tokio::test]
async fn a_doors_resolver_lists_a_second_mob_on_a_runtime_as_a_launch_builds_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let of_one_mob = host(&temp, definition("alone")).await;
    assert_eq!(of_one_mob.mob_resolver().hosted_mobs().await, Vec::new());
    shut_down(&of_one_mob, &[]).await;

    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let second_id = second_mob(&host, definition("second")).await;
    let second = set_of(&host)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");
    let door = host.mob_resolver();
    let resolved = door.resolve(Some(&second_id)).await;
    assert!(
        resolved.is_ok(),
        "the second mob of the host resolves: {:?}",
        resolved.as_ref().err()
    );
    assert_eq!(door.hosted_mobs().await, vec![listed_as_loaded(&second)]);
    assert!(
        door.resolve_if_loaded(&second_id)
            .is_some_and(|mob| mob.mob_id() == &second_id && !mob.is_default())
    );
    assert!(
        door.resolve_if_loaded(&default_id)
            .is_some_and(|mob| mob.is_default())
    );
    shut_down(&host, &[&second_id]).await;
}

/// A fixture resolver that serves a second mob lists that mob as one
/// loaded mob of the host and answers it "if loaded", and neither is one
/// of the questions the fixture records.
#[tokio::test]
async fn a_fixture_resolver_that_serves_a_second_mob_lists_it_as_loaded() {
    let (temp_first, temp_second) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let first = host(&temp_first, definition("first")).await;
    let second = host(&temp_second, definition("second")).await;
    let second_id = second.mob_handle().mob_id().clone();
    let door = first
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver());

    assert_eq!(
        door.hosted_mobs().await,
        vec![listed_as_loaded(&second.mob_handle())]
    );
    assert!(
        door.resolve_if_loaded(&second_id)
            .is_some_and(|mob| mob.mob_id() == &second_id && !mob.is_default())
    );
    assert!(
        door.resolved_for_in_tests().is_empty() && door.asked_whether_served_in_tests().is_empty(),
        "neither is a question the fixture records"
    );
    shut_down(&first, &[]).await;
    shut_down(&second, &[]).await;
}

/// The tasks that serve a second mob's entry (its actor loop probe, the
/// monitor of its identity streams, its ledger subscriber) run while the
/// mob set manages the mob and have ended once the entry went.
#[tokio::test]
async fn the_tasks_that_serve_a_second_mobs_entry_end_when_the_entry_goes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let second_id = second_mob(&host, definition("second")).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    let entry = host
        .mob_entry_in_tests(&second_id)
        .expect("the runtime holds the second mob's entry");
    let running = |task: &Option<tokio::task::JoinHandle<()>>| {
        task.as_ref().map(tokio::task::JoinHandle::abort_handle)
    };
    let tasks = [
        (
            "the actor loop probe",
            running(&*entry.actor_loop_probe_task.lock().await),
        ),
        (
            "the monitor of the identity streams",
            running(&*entry.identity_stream_health_task.lock().await),
        ),
        (
            "the ledger subscriber",
            running(&*entry.mob_events_subscriber_task.lock().await),
        ),
    ];
    for (name, task) in &tasks {
        assert!(
            task.as_ref().is_some_and(|task| !task.is_finished()),
            "{name} runs while the mob set manages the mob"
        );
    }

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        host.resolve_mob(Some(&second_id)).await.err(),
        Some(not_hosted(&second_id, &default_id))
    );
    for (name, task) in &tasks {
        assert!(
            task.as_ref()
                .is_some_and(tokio::task::AbortHandle::is_finished),
            "{name} ended with the entry"
        );
    }
    shut_down(&host, &[]).await;
}

/// The close of an entry is final. An entry whose start-time read has not
/// run holds no part of the link table, so its close finds nothing to
/// close; a wire that was in flight when the mob went runs that read
/// afterwards. It then starts no part for a mob the host no longer has,
/// and the entry's cell stays empty.
#[tokio::test]
async fn the_start_time_read_of_an_entry_whose_mob_is_gone_starts_no_part() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let second_id = second_mob(&host, definition("second")).await;
    let entry = host
        .mob_entry_before_its_read_in_tests(&second_id)
        .await
        .expect("an entry over the second mob");
    assert!(
        entry.cross_mob_routes.get().is_none(),
        "the entry's start-time read has not run"
    );

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        host.resolve_mob(Some(&second_id)).await.err(),
        Some(not_hosted(&second_id, &default_id))
    );
    assert!(
        !super::cross_mob_routes::holds_a_part_in_tests(second_id.as_str()),
        "the mob's part of the link table went with the entry the runtime held"
    );

    entry.restore_cross_mob_wake_routes().await;
    assert!(
        entry.cross_mob_routes.get().is_none(),
        "the read of an entry whose mob is gone fills no cell"
    );
    assert!(
        !super::cross_mob_routes::holds_a_part_in_tests(second_id.as_str()),
        "and starts no part of the link table for a mob the host no longer has"
    );
    entry.close().await;
    shut_down(&host, &[]).await;
}

/// The console events of a second mob end with its entry. The store a
/// second mob's console events are kept in goes when the mob set no longer
/// manages the mob, and a mob created again under the same id gets a store
/// of its own, not the one the earlier mob left: what the earlier mob's
/// agents had pending, and their events, are not the new mob's. The launch
/// mob's store is the host's own and stays.
#[tokio::test]
async fn the_console_events_of_a_second_mob_end_with_its_entry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, definition("host")).await;
    let default_id = host.mob_handle().mob_id().clone();
    let second_definition = definition("second");
    let second_id = second_mob(&host, second_definition.clone()).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    let events = host.console_events();
    let of_the_first = events.of_mob(Some(&second_id));
    let of_the_launch_mob = events.of_mob(Some(&default_id));
    assert!(
        !of_the_first.is_the_store(&of_the_launch_mob) && of_the_launch_mob.is_the_store(&events),
        "the second mob's console events are a store of their own, the launch mob's the host's"
    );
    assert_eq!(events.mobs_with_a_store(), vec![second_id.clone()]);

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        host.resolve_mob(Some(&second_id)).await.err(),
        Some(not_hosted(&second_id, &default_id)),
        "a destroyed mob resolves no mob"
    );
    assert_eq!(
        events.mobs_with_a_store(),
        Vec::<MobId>::new(),
        "the mob's console event store went with its entry"
    );

    let again = second_mob(&host, second_definition).await;
    assert_eq!(again, second_id);
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "a mob created again under the id resolves"
    );
    let of_the_second = events.of_mob(Some(&second_id));
    assert!(
        !of_the_second.is_the_store(&of_the_first),
        "the mob created again under the id has a console event store of its own"
    );
    assert!(
        events
            .of_mob(Some(&default_id))
            .is_the_store(&of_the_launch_mob),
        "the launch mob's store is the host's own and stays"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The creation of a mob that a member of `parent` created: the create a
/// host makes for the member, or with `own_create` the member's own.
fn a_members_creation(parent: &MobId, own_create: bool) -> meerkat_mob::MobProvenance {
    let (parent_mob_id, parent_incarnation, creator_identity, creator_generation, owner_session) = (
        parent.clone(),
        meerkat_core::service::MobIncarnationId::mint(),
        meerkat_mob::AgentIdentity::from("creator"),
        meerkat_mob::Generation::INITIAL,
        meerkat_core::SessionId::new(),
    );
    if own_create {
        meerkat_mob::MobProvenance::OperatorMember {
            parent_mob_id,
            parent_incarnation,
            creator_identity,
            creator_generation,
            owner_session,
        }
    } else {
        meerkat_mob::MobProvenance::Member {
            parent_mob_id,
            parent_incarnation,
            creator_identity,
            creator_generation,
            owner_session,
        }
    }
}

/// The reach a mob is in, by the kind of its creation: the host's create
/// makes a mob the host hosts, a member's create (the host's for it, or its
/// own) puts the mob in the reach of the member's mob, and a create of a
/// session that is no member's, or one from before creations were
/// recorded, puts it in no mob's reach.
#[test]
fn a_creation_says_whose_reach_its_mob_is_in() {
    use super::mob_entries::{ReachOfACreation, reach_of_a_creation};
    use meerkat_mob::MobProvenance;

    let mob = MobId::from("a-mob");
    let parent = MobId::from("its-creators-mob");
    let session_id = meerkat_core::SessionId::new();
    assert_eq!(
        reach_of_a_creation(&mob, MobProvenance::Host),
        Ok(ReachOfACreation::Hosted)
    );
    for own_create in [false, true] {
        assert_eq!(
            reach_of_a_creation(&mob, a_members_creation(&parent, own_create)),
            Ok(ReachOfACreation::CreatedIn(parent.clone())),
            "a member's create (its own: {own_create})"
        );
    }
    for of_no_member in [
        MobProvenance::Session {
            session_id: session_id.clone(),
        },
        MobProvenance::OperatorSession { session_id },
        MobProvenance::Unrecorded,
    ] {
        assert_eq!(
            reach_of_a_creation(&mob, of_no_member.clone()),
            Ok(ReachOfACreation::None),
            "{of_no_member:?}"
        );
    }
}

/// A creation that names several rosters says nothing of its mob's reach:
/// it is not decided, and the question answers an error and no reach.
#[test]
fn a_creation_that_names_several_rosters_decides_no_reach() {
    use super::mob_entries::reach_of_a_creation;

    let mob = MobId::from("a-mob");
    let decided = reach_of_a_creation(
        &mob,
        meerkat_mob::MobProvenance::Ambiguous {
            matching_rosters: 2,
        },
    );
    assert!(
        decided.is_err(),
        "a creation several rosters name is not decided: {decided:?}"
    );
}

/// The question whose reach a mob is in loads no mob: it asks a mob that
/// runs through the handles the mob set holds now, and never through the
/// set's lookup of a handle (`handle_for`), which loads a mob that is known
/// and not loaded.
///
/// A source row, because no behaviour row can tell the two apart: they
/// answer the same for every mob that runs and for every mob that is not
/// loaded, and differ only when a mob that ran when the inventory was read
/// is gone when it is asked, which no fixture can place between the two
/// reads. So the row reads the body of `MobEntries::hosting`, comments
/// left out: it names the set's snapshot of handles and does not name the
/// loading lookup.
#[test]
fn the_question_of_a_mobs_reach_names_no_lookup_that_loads() {
    let source = include_str!("mob_entries.rs");
    let head = "pub(super) async fn hosting(";
    let start = source
        .find(head)
        .expect("the table's file has the question of a mob's reach");
    assert_eq!(source.matches(head).count(), 1, "one function of that name");
    let end = source[start..]
        .find("\n    }\n")
        .expect("the function ends at a closing brace of its own");
    let body: String = source[start..start + end]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains("mob_handles_snapshot"),
        "the row reads the function that asks the set's handles: {body}"
    );
    // The set's lookup of a handle, and the table's two functions that
    // reach a mob through it.
    for loads in ["handle_for", "resolve(", "entry_of_a_named_mob("] {
        assert!(
            !body.contains(loads),
            "the question of a mob's reach names `{loads}`, which loads a mob: {body}"
        );
    }
}

/// A recorded creation says that another than the host created a mob by
/// its kind, and by a kind the judgement names: a member's create (the
/// host's for it, or its own), a session's (the host's for it, or its
/// own), a caller several rosters named, and a creation from before
/// creators were recorded. The host's own creation is not another's.
///
/// The type of a creation grows with later releases and a row cannot make
/// a kind it does not know, so the row also reads the judgement's source,
/// comments left out: it names each of those six kinds, it does not name
/// the host's, and it has no pattern that takes every kind.
#[test]
fn a_recorded_creation_is_anothers_by_a_kind_the_judgement_names() {
    use super::mob_entries::another_created;
    use meerkat_mob::MobProvenance;

    let parent = MobId::from("its-creators-mob");
    let session_id = meerkat_core::SessionId::new();
    for anothers in [
        a_members_creation(&parent, false),
        a_members_creation(&parent, true),
        MobProvenance::Session {
            session_id: session_id.clone(),
        },
        MobProvenance::OperatorSession { session_id },
        MobProvenance::Ambiguous {
            matching_rosters: 2,
        },
        MobProvenance::Unrecorded,
    ] {
        assert!(another_created(&anothers), "{anothers:?}");
    }
    assert!(
        !another_created(&MobProvenance::Host),
        "the host's own creation is not another's"
    );

    let source = include_str!("mob_entries.rs");
    let head = "pub(super) fn another_created(";
    let start = source
        .find(head)
        .expect("the table's file has the judgement of a recorded creation");
    assert_eq!(source.matches(head).count(), 1, "one function of that name");
    let end = source[start..]
        .find("\n}\n")
        .expect("the function ends at a closing brace of its own");
    let body: String = source[start..start + end]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for kind in [
        "MobProvenance::Member {",
        "MobProvenance::OperatorMember {",
        "MobProvenance::Session {",
        "MobProvenance::OperatorSession {",
        "MobProvenance::Ambiguous {",
        "MobProvenance::Unrecorded",
    ] {
        assert!(body.contains(kind), "the judgement names `{kind}`: {body}");
    }
    assert_eq!(
        body.matches("MobProvenance::").count(),
        6,
        "the judgement names those six kinds and no other: {body}"
    );
    for takes_every_kind in ["_ =>", "| _", "Some(_)"] {
        assert!(
            !body.contains(takes_every_kind),
            "the judgement has the pattern `{takes_every_kind}`, which takes a kind it does not \
             name: {body}"
        );
    }
}

/// A table that has no host yet cannot tell whether its host serves a mob
/// beside its launch mob: the answer is "not known", and never "the launch
/// mob alone". A store restored at a start can hold records of a second
/// mob before the table says which mobs the host has.
#[tokio::test]
async fn a_table_without_its_host_leaves_the_hosts_other_mobs_not_known() {
    use super::OtherMobsServed;
    use super::mob_entries::MobEntries;

    let table = MobEntries::new();
    assert_eq!(
        table.serves_beside_the_launch_mob().await,
        OtherMobsServed::NotKnown,
        "a table that has no host yet cannot tell"
    );
}

/// The schema version of the mob store file at `path`, read with a
/// read-only connection of the row's own: what the store's ledger says its
/// mob schema is.
fn mob_schema_version(path: &std::path::Path) -> i64 {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("the store file opens read-only");
    connection
        .query_row(
            "SELECT version FROM meerkat_schema WHERE domain = 'mob'",
            [],
            |row| row.get(0),
        )
        .expect("the store's ledger names its mob schema")
}

/// The names of the files in `dir`, sorted.
fn file_names_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("the directory reads")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The read of who created a store's mob, which the read of every mob's
/// reach makes of a store the mob set has no record of, changes nothing of
/// the store. The store here is one an earlier release wrote: its schema is
/// older than this build's and holds no creation record. After the read the
/// file is byte for byte what it was, its ledger names the schema it named,
/// and no file lies beside it that was not there; the read answers that the
/// store records no creator. The mob store's ordinary open, made of a copy
/// of the same file, brings that copy's schema forward: a read through it
/// would have changed the store.
#[tokio::test]
async fn the_read_of_a_stores_creation_changes_nothing_of_a_store_an_earlier_release_wrote() {
    use super::mob_entries::creation_a_store_records;

    let temp = tempfile::tempdir().expect("tempdir");
    let read = temp.path().join("read");
    let opened = temp.path().join("opened");
    for dir in [&read, &opened] {
        std::fs::create_dir_all(dir).expect("the directory is made");
    }
    let store = read.join("mob--earlier.db");
    meerkat_mob::seed_store_that_predates_creation_records_for_test(&store)
        .expect("a store an earlier release wrote");
    let copy = opened.join("mob--earlier.db");
    std::fs::copy(&store, &copy).expect("the store is copied");
    let before = (
        std::fs::read(&store).expect("the store reads"),
        mob_schema_version(&store),
        file_names_in(&read),
    );

    let answered = creation_a_store_records(&store, &MobId::from("earlier")).await;

    let after = (
        std::fs::read(&store).expect("the store reads"),
        mob_schema_version(&store),
        file_names_in(&read),
    );
    println!(
        "the store's mob schema before the read: {}; after it: {}; files: {:?}",
        before.1, after.1, after.2
    );
    assert_eq!(answered, None, "the store records no creator");
    assert!(
        after.0 == before.0,
        "the read changed the bytes of the store"
    );
    assert_eq!(after.1, before.1, "the read changed the store's schema");
    assert_eq!(after.2, before.2, "the read left a file beside the store");

    // The control: the store's ordinary open is the one that brings a
    // store's schema forward.
    drop(meerkat_mob::MobStorage::persistent(&copy).expect("the copy opens as a mob store"));
    let opened_version = mob_schema_version(&copy);
    println!("the copy's mob schema after the mob store's ordinary open: {opened_version}");
    assert!(
        opened_version > before.1,
        "the ordinary open brings the schema forward: {opened_version} from {}",
        before.1
    );
    assert!(
        std::fs::read(&copy).expect("the copy reads") != before.0,
        "the ordinary open changes the bytes of the store"
    );
}
