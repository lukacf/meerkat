//! Rows for the mobs a host has stored and not loaded.
//!
//! The fixture is a launch that declares its mob to its mob set over a state
//! directory, as the gateways do: the mobs the set builds for the host beside
//! the default mob are stored under that directory, so a later launch over
//! the same directory finds them. A mob the set knows and has not loaded is
//! loaded by the request that names it, once, and by nothing else.

#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_mob::{MobDefinition, MobId, MobProvenance};

use super::super::mob_entries_tests::{
    Sessions, definition, second_mob, set_of, shut_down, transcript_showing,
};
use super::super::{HostedMobPhase, MobResolveError, UnifiedRuntime};
use crate::blob_store::{Base64BlobStoreAdapter, ObjectStoreBlobStore};
use crate::mob_handle_runtime::{MobBootstrapOptions, MobBootstrapSpec};
use crate::runtime::{InMemoryMetadataStore, RuntimeOptions};
use crate::{DiscoverySpec, MetaConfig, MobRuntimeDelivery};

/// How long a row waits for something the mob set does on its own tasks.
const SETTLES_WITHIN: Duration = Duration::from_secs(20);

/// What outlives a launch: the state directory and the stores the sessions,
/// the runtime and the jobs are kept in. Each [`StoredHost::launch`] is a
/// launch of its own over them, as a restart of a gateway is.
pub(crate) struct StoredHost {
    dir: tempfile::TempDir,
    definition: MobDefinition,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    blobs: Arc<dyn meerkat_core::BlobStore>,
    sessions: Arc<meerkat_store::MemoryStore>,
    jobs: Arc<dyn meerkat::DetachedJobStore>,
    metadata: Arc<InMemoryMetadataStore>,
}

impl StoredHost {
    pub(crate) fn new() -> Self {
        Self::of(definition("stored-host"))
    }

    /// A host whose launch's mob is the mob of `definition`.
    pub(crate) fn of(definition: MobDefinition) -> Self {
        Self {
            dir: tempfile::tempdir().expect("a state directory"),
            definition,
            runtime_store: Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            blobs: Arc::new(Base64BlobStoreAdapter::new(Arc::new(
                ObjectStoreBlobStore::memory(),
            ))),
            sessions: Arc::new(meerkat_store::MemoryStore::new()),
            jobs: Arc::new(meerkat::MemoryDetachedJobStore::new()),
            metadata: Arc::new(InMemoryMetadataStore::new()),
        }
    }

    /// A launch over the state directory: its mob is declared to its mob
    /// set over a store in a file, and the mobs the set builds beside it
    /// are stored under the directory. Its requests resolve the host's
    /// other mobs.
    pub(crate) async fn launch(&self) -> UnifiedRuntime {
        self.bootstrap(self.spec()).await
    }

    /// [`Self::launch`], with what `compose` makes of the launch's spec
    /// before the bootstrap (a row that needs what a gateway declares on
    /// its launch says it there).
    pub(crate) async fn launch_composed(
        &self,
        compose: impl FnOnce(MobBootstrapSpec) -> MobBootstrapSpec,
    ) -> UnifiedRuntime {
        self.bootstrap(compose(self.spec())).await
    }

    /// [`Self::launch`], with what `prepare` makes of the launch's session
    /// builder before any session is built on it (a gateway attaches its
    /// schedule tools there), and with the launch's session service and
    /// runtime machine.
    pub(crate) async fn launch_over<T>(
        &self,
        prepare: impl FnOnce(&FactoryAgentBuilder) -> T,
    ) -> (
        UnifiedRuntime,
        Sessions,
        Arc<meerkat_runtime::MeerkatMachine>,
        T,
    ) {
        let (spec, sessions, machine, prepared) = self.spec_over(prepare);
        (self.bootstrap(spec).await, sessions, machine, prepared)
    }

    /// The runtime of a launch with `spec`, serving the host's other mobs.
    pub(crate) async fn bootstrap(&self, spec: MobBootstrapSpec) -> UnifiedRuntime {
        let metadata: Arc<InMemoryMetadataStore> = Arc::clone(&self.metadata);
        Box::pin(UnifiedRuntime::bootstrap_with_options(
            spec,
            MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec {
                    namespace: self.definition.id.to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            },
            Vec::new(),
            Duration::from_secs(10),
            RuntimeOptions::default(),
            metadata,
        ))
        .await
        .expect("a runtime over a declared store in a state directory")
    }

    /// The spec of a launch over the state directory, for a row that
    /// bootstraps the runtime itself: the mob declared over a store in a
    /// file, the sessions in the stores this value keeps, and the mobs the
    /// set builds beside the launch's stored under the directory.
    pub(crate) fn spec(&self) -> MobBootstrapSpec {
        self.spec_over(|_| ()).0
    }

    /// [`Self::spec`], with what `prepare` makes of the launch's session
    /// builder before any session is built on it, and with the launch's
    /// session service and runtime machine.
    pub(crate) fn spec_over<T>(
        &self,
        prepare: impl FnOnce(&FactoryAgentBuilder) -> T,
    ) -> (
        MobBootstrapSpec,
        Sessions,
        Arc<meerkat_runtime::MeerkatMachine>,
        T,
    ) {
        let machine = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(
                Arc::clone(&self.runtime_store),
                Arc::clone(&self.blobs),
            )
            .expect("the runtime machine"),
        );
        let mut builder = FactoryAgentBuilder::new(
            AgentFactory::new(self.dir.path())
                .builtins(false)
                .comms(true),
            Config::default(),
        );
        builder.default_detached_job_store = Some(Arc::clone(&self.jobs));
        let prepared = prepare(&builder);
        let sessions: Arc<meerkat_store::MemoryStore> = Arc::clone(&self.sessions);
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            8,
            sessions,
            Arc::clone(&self.runtime_store),
            Arc::clone(&self.blobs),
        ));
        let spec = MobBootstrapSpec::declared(
            self.definition.clone(),
            crate::mob_composition_manifest::declared_mob_store(
                crate::MetaStorageLayout::with_injected_roots(self.dir.path().to_path_buf(), None)
                    .event_log_db(),
            ),
            service.clone(),
        )
        .with_session_runtime_adapter(Arc::clone(&machine))
        .expect("the launch adopts the runtime machine")
        .with_runtime_delivery(MobRuntimeDelivery::new(
            Arc::clone(&self.runtime_store),
            Arc::clone(&self.jobs),
        ))
        .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
        .expect("the agent mob tools")
        .with_child_mob_storage(crate::ChildMobStorage::Durable(
            crate::MetaStorageLayout::with_injected_roots(self.dir.path().to_path_buf(), None),
        ))
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        (spec, service, machine, prepared)
    }

    /// The state directory the launches of this host share.
    pub(crate) fn state_dir(&self) -> &std::path::Path {
        self.dir.path()
    }
}

/// The mobs `host`'s mob set manages now, and the ones it knows and has not
/// loaded, with who created each of those as far as the set has read it.
pub(crate) struct Known {
    pub(crate) loaded: BTreeSet<MobId>,
    pub(crate) not_loaded: BTreeSet<MobId>,
    pub(crate) created_by_the_host: BTreeSet<MobId>,
}

pub(crate) async fn known(host: &UnifiedRuntime) -> Known {
    let inventory = set_of(host)
        .mob_inventory()
        .await
        .expect("the mob set's inventory");
    Known {
        loaded: inventory.managed.iter().map(|(id, _)| id.clone()).collect(),
        not_loaded: inventory.dormant.iter().cloned().collect(),
        created_by_the_host: inventory
            .dormant_creation
            .iter()
            .filter(|(_, creation)| matches!(creation, MobProvenance::Host))
            .map(|(id, _)| id.clone())
            .collect(),
    }
}

/// Have `host`'s mob set ask its idle mobs to unload, and require that each
/// of `mobs` is then known and not loaded.
pub(crate) async fn asleep(host: &UnifiedRuntime, mobs: &[&MobId]) {
    let unloaded = set_of(host).unload_idle_mobs_for_tests().await;
    let known = known(host).await;
    for mob_id in mobs {
        assert!(
            known.not_loaded.contains(*mob_id) && !known.loaded.contains(*mob_id),
            "the launch's mob set unloads the idle mob {mob_id}: it unloaded {unloaded:?}, \
             manages {:?} and knows as not loaded {:?}",
            known.loaded,
            known.not_loaded
        );
    }
}

/// Wait until the runtime holds exactly the entries of `mobs`.
async fn entries_are(host: &UnifiedRuntime, mobs: &[&MobId]) -> bool {
    let wanted: Vec<MobId> = mobs.iter().map(|id| (*id).clone()).collect();
    let deadline = tokio::time::Instant::now() + SETTLES_WITHIN;
    loop {
        if host.mob_entries_in_tests() == wanted {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A host with stored mobs loads none of them at its start: after a launch
/// over a state directory that holds five mobs beside the default mob, the
/// mob set manages the default mob alone and knows the five as not loaded,
/// also once its sweep has read who created each, and the runtime holds no
/// entry. A request that names one of them then loads that one.
#[tokio::test]
async fn a_host_with_many_stored_mobs_loads_none_but_its_default_at_start() {
    let stored = StoredHost::new();
    let first = stored.launch().await;
    let default_id = first.mob_handle().mob_id().clone();
    let mut others = BTreeSet::new();
    for tag in ["a", "b", "c", "d", "e"] {
        others.insert(second_mob(&first, definition(&format!("stored-{tag}"))).await);
    }
    shut_down(&first, &[]).await;
    drop(first);

    let host = stored.launch().await;
    assert_eq!(host.mob_handle().mob_id(), &default_id);
    // Until the set has read who created each stored mob, or has loaded one.
    let deadline = tokio::time::Instant::now() + SETTLES_WITHIN;
    let at_rest = loop {
        let now = known(&host).await;
        let read = others.is_subset(&now.created_by_the_host);
        let loaded_one = others.iter().any(|id| now.loaded.contains(id));
        if read || loaded_one || tokio::time::Instant::now() >= deadline {
            break now;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        at_rest.loaded,
        BTreeSet::from([default_id.clone()]),
        "a start loads the default mob and no stored mob"
    );
    assert!(
        others.is_subset(&at_rest.not_loaded),
        "every stored mob is known and not loaded: {:?}",
        at_rest.not_loaded
    );
    assert!(
        others.is_subset(&at_rest.created_by_the_host),
        "the set read who created each stored mob without loading it: {:?}",
        at_rest.created_by_the_host
    );
    assert_eq!(
        host.mob_entries_in_tests(),
        Vec::<MobId>::new(),
        "the runtime holds no entry of a mob that is not loaded"
    );

    let named = others.iter().next().cloned().expect("a stored mob");
    let resolved = host.resolve_mob(Some(&named)).await;
    assert!(
        resolved
            .as_ref()
            .is_ok_and(|mob| mob.mob_id() == &named && !mob.is_default()),
        "a request that names a stored mob resolves it: {:?}",
        resolved.as_ref().err()
    );
    let after = known(&host).await;
    assert_eq!(
        after.loaded,
        BTreeSet::from([default_id, named.clone()]),
        "the request loaded the mob it named and no other"
    );
    shut_down(&host, &[]).await;
}

/// Whether the host serves a mob beside its launch mob cannot be told
/// while the mob set knows a mob whose creation no record of it tells:
/// such a mob may be the host's. After a start over a state directory that
/// holds one stored mob beside the launch mob, whose store cannot be read,
/// the mob set knows the mob (as not loaded, and as quarantined once its
/// sweep met the store) and has read nothing of who created it. The answer
/// is "not known" from the start on and stays so; it is never "the launch
/// mob alone". The question opens no store and loads no mob.
#[tokio::test]
async fn a_stored_mob_whose_creation_is_not_told_leaves_the_hosts_other_mobs_not_known() {
    use super::super::OtherMobsServed;

    let stored = StoredHost::new();
    let first = stored.launch().await;
    let other = second_mob(&first, definition("creation-not-told")).await;
    shut_down(&first, &[]).await;
    drop(first);
    damage_the_store_of(&stored, &other);

    let host = stored.launch().await;
    let resolver = host.mob_resolver();
    let set = set_of(&host);
    let at_start = set.mob_inventory().await.expect("the mob set's inventory");
    assert!(
        at_start.dormant.contains(&other)
            || at_start.quarantined.iter().any(|kept| kept.mob_id == other),
        "the mob set knows the stored mob: not loaded {:?}, quarantined {:?}",
        at_start.dormant,
        at_start.quarantined
    );
    assert!(
        !at_start.dormant_creation.contains_key(&other),
        "nothing tells who created it: {:?}",
        at_start.dormant_creation
    );
    let answered_at_start = resolver.other_mobs_served().await;

    // Until the set's sweep met the store and keeps it quarantined.
    let deadline = tokio::time::Instant::now() + SETTLES_WITHIN;
    let quarantined = loop {
        let now = set.mob_inventory().await.expect("the mob set's inventory");
        let quarantined = now.quarantined.iter().any(|kept| kept.mob_id == other);
        if quarantined || tokio::time::Instant::now() >= deadline {
            break quarantined;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let answered_once_quarantined = resolver.other_mobs_served().await;
    let loaded = known(&host).await.loaded;
    drop(set);
    shut_down(&host, &[]).await;

    assert_eq!(
        answered_at_start,
        OtherMobsServed::NotKnown,
        "a stored mob whose creation is not told may be the host's"
    );
    assert!(
        quarantined,
        "the sweep keeps the unreadable store quarantined"
    );
    assert_eq!(
        answered_once_quarantined,
        OtherMobsServed::NotKnown,
        "a quarantined store whose creation was never read may be the host's"
    );
    assert!(
        !loaded.contains(&other),
        "the question loaded no mob: {loaded:?}"
    );
}

/// An upgrade of a host of one mob: its state directory holds the launch
/// mob's store as an earlier release wrote it (a schema with no table for
/// creation records) and no other mob store. A launch over it serves its
/// launch mob alone, and says so: whether the host serves another mob is
/// answered "the launch mob alone", not "not known".
#[tokio::test]
async fn a_host_of_one_mob_whose_store_predates_creation_records_serves_its_launch_mob_alone() {
    use super::super::OtherMobsServed;

    let stored = StoredHost::new();
    let launch_store =
        crate::MetaStorageLayout::with_injected_roots(stored.state_dir().to_path_buf(), None)
            .event_log_db();
    meerkat_mob::seed_store_that_predates_creation_records_for_test(&launch_store)
        .expect("the launch mob's store as an earlier release wrote it");
    assert!(
        crate::child_mob_storage::child_mob_databases(stored.state_dir()).is_empty(),
        "the state directory holds no other mob store"
    );

    let host = stored.launch().await;
    let answered = host.mob_resolver().other_mobs_served().await;
    shut_down(&host, &[]).await;

    assert_eq!(
        answered,
        OtherMobsServed::LaunchMobAlone,
        "a host of one mob over an earlier release's store serves its launch mob alone"
    );
}

/// A table that began to shut down cannot tell whether its host serves a
/// mob beside its launch mob: a host of one mob that answered "the launch
/// mob alone" answers "not known" from then on, and never "the launch mob
/// alone".
#[tokio::test]
async fn a_table_that_began_to_shut_down_leaves_the_hosts_other_mobs_not_known() {
    use super::super::OtherMobsServed;

    let stored = StoredHost::new();
    let host = stored.launch().await;
    let resolver = host.mob_resolver();
    let before = resolver.other_mobs_served().await;
    host.other_mobs().begin_close();
    let closing = resolver.other_mobs_served().await;
    shut_down(&host, &[]).await;

    assert_eq!(
        before,
        OtherMobsServed::LaunchMobAlone,
        "a host of one mob serves its launch mob alone"
    );
    assert_eq!(
        closing,
        OtherMobsServed::NotKnown,
        "a table that began to shut down cannot tell"
    );
}

/// A mob of the host whose actor stopped is still a mob of the host. The
/// host creates a second mob and a request resolves it: the host serves
/// another mob. The mob is then shut down through its handle: the table
/// holds no entry of it whose actor runs, and the mob set still manages
/// the mob, with the record that the host created it. The host does not
/// answer that it serves its launch mob alone: with no entry to list the
/// mob by, the answer is "not known".
#[tokio::test]
async fn a_hosts_mob_whose_actor_stopped_does_not_read_as_the_launch_mob_alone() {
    use super::super::OtherMobsServed;

    let stored = StoredHost::new();
    let host = stored.launch().await;
    let resolver = host.mob_resolver();
    let other = second_mob(&host, definition("actor-stopped")).await;
    let resolved = host.resolve_mob(Some(&other)).await;
    assert!(
        resolved.as_ref().is_ok_and(|mob| mob.mob_id() == &other),
        "a request resolves the host's second mob: {:?}",
        resolved.as_ref().err()
    );
    drop(resolved);
    let while_it_runs = resolver.other_mobs_served().await;

    let set = set_of(&host);
    let handle = set
        .handle_for(&other)
        .await
        .expect("the set hands out the second mob");
    let stopped = handle.shutdown().await;
    drop(handle);
    // The table lists a mob by an entry whose actor runs: it holds none of
    // this mob from here on (a pass ends the entry of a stopped actor).
    let no_running_entry = host
        .other_mobs()
        .held()
        .iter()
        .all(|entry| entry.mob_id() != &other || entry.actor_closed());
    let inventory = set.mob_inventory().await.expect("the mob set's inventory");
    let managed: BTreeSet<MobId> = inventory.managed.iter().map(|(id, _)| id.clone()).collect();
    let recorded = inventory.managed_creation.get(&other).cloned();
    let after_the_stop = resolver.other_mobs_served().await;
    drop(set);
    shut_down(&host, &[]).await;

    assert_eq!(
        while_it_runs,
        OtherMobsServed::AnotherMob,
        "the host serves its second mob while that mob runs"
    );
    assert!(stopped.is_ok(), "the second mob shuts down: {stopped:?}");
    assert!(
        no_running_entry,
        "the table holds no entry of the stopped mob whose actor runs"
    );
    assert!(
        managed.contains(&other) && recorded == Some(MobProvenance::Host),
        "the set still manages the stopped mob as one the host created: it manages \
         {managed:?} and records {recorded:?}"
    );
    assert_eq!(
        after_the_stop,
        OtherMobsServed::NotKnown,
        "a mob of the host with no entry does not read as no other mob"
    );
}

/// A request for a mob that is not loaded loads exactly that mob: of two
/// mobs the set unloaded, the one a request names is managed again with an
/// entry, and the other stays not loaded with none.
#[tokio::test]
async fn a_request_for_a_not_loaded_mob_loads_exactly_that_mob() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let default_id = host.mob_handle().mob_id().clone();
    let named = second_mob(&host, definition("named")).await;
    let other = second_mob(&host, definition("other")).await;
    asleep(&host, &[&named, &other]).await;
    assert!(
        entries_are(&host, &[]).await,
        "the entries of the unloaded mobs went: {:?}",
        host.mob_entries_in_tests()
    );

    let resolved = host.resolve_mob(Some(&named)).await;
    assert!(
        resolved
            .as_ref()
            .is_ok_and(|mob| mob.mob_id() == &named && !mob.is_default()),
        "a request that names a mob that is not loaded resolves it: {:?}",
        resolved.as_ref().err()
    );
    let after = known(&host).await;
    assert_eq!(
        after.loaded,
        BTreeSet::from([default_id, named.clone()]),
        "the request loaded the mob it named"
    );
    assert!(
        after.not_loaded.contains(&other),
        "and not the other: {:?}",
        after.not_loaded
    );
    assert_eq!(host.mob_entries_in_tests(), vec![named.clone()]);
    shut_down(&host, &[]).await;
}

/// Requests that name one mob that is not loaded wait on one load: eight
/// requests at once all resolve the same entry over one actor, and the
/// runtime holds that one entry.
#[tokio::test]
async fn requests_for_one_not_loaded_mob_wait_on_one_load() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let named = second_mob(&host, definition("waited-for")).await;
    asleep(&host, &[&named]).await;

    let answers = futures::future::join_all((0..8).map(|_| host.resolve_mob(Some(&named)))).await;
    let mut entries = BTreeSet::new();
    let mut actors = BTreeSet::new();
    for answer in &answers {
        assert!(
            answer.is_ok(),
            "each of the requests resolves the mob: {:?}",
            answer.as_ref().err()
        );
        if let Ok(mob) = answer {
            entries.insert(std::ptr::from_ref(mob.entry()).addr());
            actors.insert(format!("{:?}", mob.mob_handle().incarnation()));
        }
    }
    assert_eq!(entries.len(), 1, "one entry for all the requests");
    assert_eq!(actors.len(), 1, "over one actor: {actors:?}");
    drop(answers);
    assert_eq!(host.mob_entries_in_tests(), vec![named.clone()]);
    shut_down(&host, &[]).await;
}

/// A question that only needs to know that a mob is one the host serves
/// loads nothing: for a mob that is not loaded the host answers that it
/// serves it, a door lists it as not loaded and reads no loaded mob under
/// its id, and the mob is still not loaded afterwards.
#[tokio::test]
async fn a_host_scoped_call_that_names_a_not_loaded_mob_loads_nothing() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let default_id = host.mob_handle().mob_id().clone();
    let named = second_mob(&host, definition("only-asked-about")).await;
    asleep(&host, &[&named]).await;

    assert_eq!(
        host.served_mob_id(Some(&named)).await,
        Ok(named.clone()),
        "the host serves the mob"
    );
    let door = host.mob_resolver();
    assert_eq!(door.served_mob_id(Some(&named)).await, Ok(named.clone()));
    assert!(
        door.resolve_if_loaded(&named).is_none(),
        "a read of the loaded mobs finds none under the id"
    );
    let listed = door.hosted_mobs().await;
    assert!(
        listed
            .iter()
            .any(|mob| mob.mob_id == named && mob.phase == HostedMobPhase::NotLoaded),
        "a door lists the mob as not loaded: {listed:?}"
    );
    let after = known(&host).await;
    assert_eq!(
        after.loaded,
        BTreeSet::from([default_id]),
        "none of the questions loaded the mob"
    );
    assert!(after.not_loaded.contains(&named));
    assert_eq!(host.mob_entries_in_tests(), Vec::<MobId>::new());
    shut_down(&host, &[]).await;
}

/// The question that loads nothing answers what a request is answered,
/// also for a mob the set put to sleep before the table made its entry.
/// The row holds the table's follower, so no pass runs between the create
/// and the unload: what the table kept at the mob's build, for the entry a
/// pass makes, is still there when the mob is no longer managed. The host
/// serves the mob all the same, the question loads nothing, and the request
/// that names the mob then resolves it.
#[tokio::test]
async fn a_mob_put_to_sleep_before_its_entry_was_made_is_served_as_a_request_finds_it() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    host.hold_the_follower_of_host_mobs_in_tests().await;
    let named = second_mob(&host, definition("asleep-before-its-entry")).await;
    let entries_at_the_unload = host.mob_entries_in_tests();
    asleep(&host, &[&named]).await;

    let served = host.served_mob_id(Some(&named)).await;
    let at_a_door = host.mob_resolver().served_mob_id(Some(&named)).await;
    let not_loaded_after_the_question = known(&host).await.not_loaded.contains(&named);
    let resolved = host
        .resolve_mob(Some(&named))
        .await
        .map(|mob| mob.mob_handle().mob_id().clone());
    shut_down(&host, &[&named]).await;

    assert_eq!(
        entries_at_the_unload,
        Vec::<MobId>::new(),
        "no pass made the mob's entry before the mob was unloaded"
    );
    assert!(not_loaded_after_the_question, "the question loaded nothing");
    assert_eq!(
        resolved,
        Ok(named.clone()),
        "a request that names the mob resolves it"
    );
    assert_eq!(
        served,
        Ok(named.clone()),
        "the host answers that it serves the mob, as the request found"
    );
    assert_eq!(at_a_door, Ok(named), "and so does a door's resolver");
}

/// The entry of a mob the mob set destroys is ended without the task that
/// follows the set: the launch tells the table of each destroy, and the
/// table runs a pass of its own then. The row holds that task, so nothing
/// but the pass the destroy asked for ends the entry. The destroy, which
/// waits about a second for whatever still holds the mob's database in
/// this process, is answered its report, and the table then holds no
/// entry of the mob.
#[tokio::test]
async fn the_entry_of_a_destroyed_mob_is_ended_without_the_task_that_follows_the_set() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let named = second_mob(&host, definition("destroyed-while-nothing-follows")).await;
    let resolved = host
        .resolve_mob(Some(&named))
        .await
        .map(|mob| mob.mob_handle().mob_id().clone());
    let entries_at_the_destroy = host.mob_entries_in_tests();
    host.hold_the_follower_of_host_mobs_in_tests().await;

    let destroyed = Box::pin(set_of(&host).mob_destroy(&named))
        .await
        .map(|_| ())
        .map_err(|error| format!("{error:?}"));
    let ended = entries_are(&host, &[]).await;
    shut_down(&host, &[]).await;

    assert_eq!(resolved, Ok(named.clone()), "a request resolves the mob");
    assert_eq!(
        entries_at_the_destroy,
        vec![named],
        "the runtime holds the mob's entry when the destroy is asked"
    );
    assert_eq!(
        destroyed,
        Ok(()),
        "the destroy is answered its report: nothing of the runtime still holds the mob's \
         database"
    );
    assert!(ended, "the table holds no entry of the destroyed mob");
}

/// The pass a destroy asks for is the table's own: once the launch's hook
/// has told the table of a destroy, the table holds that pass's task, and
/// its close joins it. The mob set calls the hook on the destroy's own
/// task and waits for nothing; the row calls it as the set does and reads
/// the table's tasks before anything the hook started can have run (the
/// row's runtime has one thread, and nothing is awaited in between).
#[tokio::test]
async fn the_pass_a_destroy_asks_for_is_a_task_the_table_holds() {
    use crate::mob_handle_runtime::HostMobEntryWiring as _;

    let stored = StoredHost::new();
    let host = stored.launch().await;
    let named = second_mob(&host, definition("destroyed-and-told")).await;
    host.hold_the_follower_of_host_mobs_in_tests().await;
    let before = host.pass_tasks_of_host_mobs_running_in_tests();

    host.other_mobs().after_destroy(&named);
    let held_at_once = host.pass_tasks_of_host_mobs_running_in_tests();
    let mut left = held_at_once;
    for _ in 0..200 {
        if left == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        left = host.pass_tasks_of_host_mobs_running_in_tests();
    }
    shut_down(&host, &[&named]).await;

    assert_eq!(before, 0, "no pass runs before the table is told");
    assert_eq!(
        held_at_once, 1,
        "the table holds the task of the pass the destroy asked for as soon as it is told"
    );
    assert_eq!(left, 0, "that pass ran to its end");
}

/// A mob the set unloaded is loaded again by the next request that names
/// it, with an entry of its own over the new actor: the entry the first
/// request resolved went with the unload and its actor closed. It is the
/// same mob: its incarnation is what it was.
#[tokio::test]
async fn an_unloaded_mob_is_loaded_again_by_a_request_with_a_new_entry() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let named = second_mob(&host, definition("loaded-twice")).await;
    let first = host
        .resolve_mob(Some(&named))
        .await
        .map(|mob| format!("{:?}", mob.mob_handle().incarnation()));
    assert!(
        first.is_ok(),
        "the second mob of the host resolves: {:?}",
        first.as_ref().err()
    );
    let first_entry = host
        .mob_entry_in_tests(&named)
        .expect("the runtime holds the entry the request resolved");
    assert!(!first_entry.actor_closed());

    asleep(&host, &[&named]).await;
    assert!(
        entries_are(&host, &[]).await,
        "the entry went with the unload: {:?}",
        host.mob_entries_in_tests()
    );
    assert!(
        first_entry.actor_closed(),
        "the actor the first entry was built for closed"
    );

    let again = host
        .resolve_mob(Some(&named))
        .await
        .map(|mob| format!("{:?}", mob.mob_handle().incarnation()));
    assert!(
        again.is_ok(),
        "the next request that names the mob resolves it: {:?}",
        again.as_ref().err()
    );
    let second_entry = host
        .mob_entry_in_tests(&named)
        .expect("the runtime holds the entry of the mob loaded again");
    assert!(
        !Arc::ptr_eq(&first_entry, &second_entry) && !second_entry.actor_closed(),
        "the mob loaded again has an entry of its own over an actor that runs"
    );
    assert_eq!(first.ok(), again.ok(), "it is the same mob");
    assert_eq!(host.mob_entries_in_tests(), vec![named.clone()]);
    shut_down(&host, &[]).await;
}

/// Damage the store of the stored mob `mob_id` of `stored` on disk: what a
/// store that cannot be read is.
fn damage_the_store_of(stored: &StoredHost, mob_id: &MobId) {
    let file = crate::child_mob_storage::child_mob_databases(stored.state_dir())
        .into_iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(mob_id.as_str()))
        })
        .expect("the stored mob's file under the state directory");
    std::fs::write(&file, b"not a sqlite database").expect("damage the stored mob's file");
}

/// A stored mob whose load fails is answered as a mob the host has and
/// cannot serve, with the mob set's own refusal, by every request that
/// names it, and no other mob is touched by it: the host still answers that
/// it serves the mob, a door lists it as one that cannot be loaded, the
/// other stored mob loads, and the default mob resolves. A door that has
/// callers answers its one form for a mob that did not resolve.
#[tokio::test]
async fn a_stored_mob_whose_load_fails_is_refused_as_the_mob_set_refuses_it_and_the_others_serve() {
    use crate::rpc::mob_addressing::{
        mob_not_served, mob_refusal_for_a_caller, mob_resolve_refusal,
    };

    let stored = StoredHost::new();
    let host = stored.launch().await;
    let broken = second_mob(&host, definition("cannot-load")).await;
    let healthy = second_mob(&host, definition("loads")).await;
    asleep(&host, &[&broken, &healthy]).await;
    damage_the_store_of(&stored, &broken);

    for request in ["the first request", "the next request"] {
        let refused = host.resolve_mob(Some(&broken)).await.err();
        assert!(
            refused
                .as_ref()
                .is_some_and(|error| !matches!(error, MobResolveError::NotHosted { .. })),
            "{request} for a mob the host has is not answered as a mob it does not have: \
             {refused:?}"
        );
        let Some(refused) = refused else {
            continue;
        };
        let to_the_host = mob_resolve_refusal(&refused);
        assert_eq!(
            to_the_host
                .data
                .as_ref()
                .and_then(|data| data.get("kind"))
                .and_then(serde_json::Value::as_str),
            Some("mob_quarantined"),
            "{request} is answered with the mob set's refusal: {to_the_host:?}"
        );
        assert_eq!(
            mob_refusal_for_a_caller(&refused),
            mob_not_served(broken.as_str()),
            "a door that has callers answers its one form"
        );
    }
    assert_eq!(
        host.served_mob_id(Some(&broken)).await,
        Ok(broken.clone()),
        "the host has the mob"
    );
    let listed = host.mob_resolver().hosted_mobs().await;
    assert!(
        listed
            .iter()
            .any(|mob| mob.mob_id == broken
                && matches!(mob.phase, HostedMobPhase::CannotBeLoaded(_))),
        "a door lists the mob as one that cannot be loaded: {listed:?}"
    );

    let other = host.resolve_mob(Some(&healthy)).await;
    assert!(
        other.as_ref().is_ok_and(|mob| mob.mob_id() == &healthy),
        "the other stored mob loads: {:?}",
        other.as_ref().err()
    );
    assert!(
        host.resolve_mob(None).await.is_ok(),
        "the default mob resolves"
    );
    assert_eq!(host.mob_entries_in_tests(), vec![healthy.clone()]);
    shut_down(&host, &[]).await;
}

/// A stored mob is resolved by the first request that names it after a
/// restart: the request does not wait for the mob set to have read, on its
/// own, who created each stored mob.
#[tokio::test]
async fn a_stored_mob_is_resolved_by_the_first_request_after_a_restart() {
    let stored = StoredHost::new();
    let first = stored.launch().await;
    let default_id = first.mob_handle().mob_id().clone();
    let mut others = Vec::new();
    for index in 0..12 {
        others.push(second_mob(&first, definition(&format!("after-restart-{index}"))).await);
    }
    shut_down(&first, &[]).await;
    drop(first);

    let host = stored.launch().await;
    let answers =
        futures::future::join_all(others.iter().map(|named| host.resolve_mob(Some(named)))).await;
    for (named, answer) in others.iter().zip(&answers) {
        assert!(
            answer.as_ref().is_ok_and(|mob| mob.mob_id() == named),
            "the first request after the restart resolves the stored mob {named}: {:?}",
            answer.as_ref().err()
        );
    }
    drop(answers);
    let mut loaded: BTreeSet<MobId> = others.iter().cloned().collect();
    loaded.insert(default_id);
    assert_eq!(known(&host).await.loaded, loaded);
    shut_down(&host, &[]).await;
}

/// The launch declares its default mob to its mob set in one place, and
/// that place selects the policy under which the set loads a stored mob
/// when something addresses it: no declaration of a default mob without it.
#[test]
fn every_declaration_of_a_default_mob_selects_loading_on_demand() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/mob_handle_runtime.rs");
    let source = std::fs::read_to_string(path).expect("the launch's source");
    let production = source
        .split("\n#[cfg(test)]\nmod tests {")
        .next()
        .expect("the production part");
    let code: Vec<&str> = production
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    let declarations: Vec<usize> = code
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(".with_default_mob("))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        declarations.len(),
        1,
        "one place declares the default mob to the mob set"
    );
    let Some(&declaration) = declarations.first() else {
        return;
    };
    let start = code
        .iter()
        .take(declaration)
        .rposition(|line| line.starts_with("fn "))
        .expect("the declaration is in a function of the module");
    let body = code
        .iter()
        .skip(start)
        .take_while(|line| **line != "}")
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains(".with_mob_restore_policy(meerkat_mob_mcp::MobRestorePolicy::OnDemand)"),
        "the function that declares the default mob selects loading on demand:\n{body}"
    );
}

/// The announcements' seam to the launch's mob set, as a gateway composes
/// it: the launch's mob is its own, and every other mob of the set is a mob
/// hosted here. Its identity runtime knows no identity: the rows ask about
/// the other mobs and their members only.
fn announcements_native(
    host: &UnifiedRuntime,
) -> crate::mob_announcements::native::MobNativeDelivery {
    let sessions = host
        .mob_runtime()
        .session_service()
        .cloned()
        .expect("the launch's session service");
    let machine = crate::mob_handle_runtime::observed_runtime_adapter(sessions.as_ref())
        .expect("the launch's runtime machine");
    let identities = Arc::new(crate::identity_first::IdentityRuntime::new(
        crate::identity_first::IdentityRuntimeConfig {
            continuity_store: Arc::new(
                crate::identity_first::LocalContinuityStore::in_memory()
                    .expect("a continuity store in memory"),
            ),
            lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
            runtime_instance_id: "lazy-start-rows".to_string(),
            has_runtime_store: true,
            durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        },
    ));
    crate::mob_announcements::native::MobNativeDelivery::new(
        announced(host.mob_handle().mob_id()),
        identities,
        set_of(host),
        machine,
        sessions,
    )
}

/// The mob `mob_id` as the announcements name it.
fn announced(mob_id: &MobId) -> crate::mob_announcements::MobId {
    crate::mob_announcements::MobId::new(mob_id.to_string()).expect("a mob id the store holds")
}

/// The member `identity` of the mob `mob_id` as the announcements name it.
fn announced_member(mob_id: &MobId, identity: &str) -> crate::mob_announcements::Member {
    crate::mob_announcements::Member::new(
        announced(mob_id),
        crate::mob_announcements::MemberIdentity::new(identity).expect("a member identity"),
    )
}

/// A mob the mob set unloaded is still one of the mobs hosted here for the
/// announcements, which end what belongs to a mob that is gone: it is in
/// their hosted set while it is not loaded, and once it is loaded again.
#[tokio::test]
async fn a_mob_that_is_not_loaded_is_in_the_announcements_hosted_set() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let named = second_mob(&host, definition("hosted-while-asleep")).await;
    let set = set_of(&host);
    let hosted = crate::mob_announcements::native::hosted_set(&*set).await;
    assert!(
        hosted
            .as_ref()
            .is_ok_and(|hosted| hosted.contains(named.as_str())),
        "a loaded mob is hosted: {hosted:?}"
    );

    asleep(&host, &[&named]).await;
    let hosted = crate::mob_announcements::native::hosted_set(&*set).await;
    assert!(
        hosted
            .as_ref()
            .is_ok_and(|hosted| hosted.contains(named.as_str())),
        "a mob that is not loaded is hosted, not gone: {hosted:?}"
    );
    assert!(
        known(&host).await.not_loaded.contains(&named),
        "and the question loaded nothing"
    );
    shut_down(&host, &[]).await;
}

/// No read the announcements make of a mob that is not loaded answers that
/// the mob is gone, and none of them loads it: the mob is hosted, and what
/// its roster and its log say cannot be read now. A subscription of one of
/// its members made before it was unloaded therefore ends by none of them.
/// Each of those reads says, typed, that the mob is not loaded: a pass that
/// meets it at every turn tells it from a read that failed.
#[tokio::test]
async fn the_announcements_read_a_mob_that_is_not_loaded_as_hosted_and_load_nothing() {
    use crate::mob_announcements::native::{MemberRoute, MobPresence, NativeDelivery};

    let stored = StoredHost::new();
    let host = stored.launch().await;
    let named = second_mob(&host, definition("read-while-asleep")).await;
    let native = announcements_native(&host);
    let (mob, member) = (announced(&named), announced_member(&named, "desk"));
    let subscribed = native.route_for(&member).await;
    assert!(
        subscribed
            .as_ref()
            .is_ok_and(|kept| kept.route == MemberRoute::Child),
        "a member of a loaded mob of the host is routed to that mob: {subscribed:?}"
    );
    let incarnation = subscribed.ok().and_then(|kept| kept.incarnation);

    asleep(&host, &[&named]).await;
    let presence = native.mob_presence(&BTreeSet::from([mob.clone()])).await;
    assert!(
        presence
            .as_ref()
            .is_ok_and(|presence| presence.get(&mob) == Some(&MobPresence::Hosted)),
        "a mob that is not loaded is hosted: {presence:?}"
    );
    let hosted = native.hosted_mob_ids().await;
    assert!(
        hosted.as_ref().is_ok_and(|hosted| hosted.contains(&mob)),
        "it is among the mobs hosted here: {hosted:?}"
    );
    let gone = native.member_mob_gone(&mob, incarnation.as_ref()).await;
    assert!(
        gone.as_ref()
            .is_err_and(|error| error.not_loaded_mob().is_some()),
        "whether the mob of a member is gone is not answered while it is not loaded: {gone:?}"
    );
    let ended = native.member_mob_end(&mob, incarnation.as_ref(), 0).await;
    assert!(
        ended
            .as_ref()
            .is_err_and(|error| error.not_loaded_mob().is_some()),
        "nor is what its log says after a cursor: {ended:?}"
    );
    let facts = native.member_facts(&member).await;
    assert!(
        facts
            .as_ref()
            .is_err_and(|error| error.not_loaded_mob().is_some()),
        "a member's roster facts cannot be read, which is not the answer that it has none: \
         {facts:?}"
    );
    let with_role = native.members_with_role(&mob, "worker").await;
    assert!(
        with_role
            .as_ref()
            .is_err_and(|error| error.not_loaded_mob().is_some()),
        "the members with a role cannot be read, which is not the answer that there are \
         none: {with_role:?}"
    );
    assert!(
        known(&host).await.not_loaded.contains(&named),
        "and none of the reads loaded the mob"
    );
    shut_down(&host, &[]).await;
}

/// Each call of the announcements that addresses one mob loads it when it
/// is not loaded: the route of a member that is being subscribed (the same
/// mob, with the incarnation it had: never a peer's member), the
/// preparation of a delivery to one of its members, the cursor read of a
/// new subscription, and the check of the member. After it nothing of a
/// member subscribed before reads as ended.
#[tokio::test]
async fn a_call_of_the_announcements_that_addresses_a_mob_that_is_not_loaded_loads_it() {
    use crate::mob_announcements::native::{MemberProblem, MemberRoute, NativeDelivery};

    let stored = StoredHost::new();
    let host = stored.launch().await;
    let named = second_mob(&host, definition("addressed-while-asleep")).await;
    let native = announcements_native(&host);
    let (mob, member) = (announced(&named), announced_member(&named, "desk"));
    let incarnation = native
        .route_for(&member)
        .await
        .ok()
        .and_then(|kept| kept.incarnation);
    assert!(incarnation.is_some(), "the loaded mob has an incarnation");

    asleep(&host, &[&named]).await;
    let routed = native.route_for(&member).await;
    assert!(
        routed
            .as_ref()
            .is_ok_and(|kept| kept.route == MemberRoute::Child && kept.incarnation == incarnation),
        "the member is routed to its mob, which is the mob it was: {routed:?}"
    );
    assert!(
        known(&host).await.loaded.contains(&named),
        "the route of a member that is being subscribed loaded its mob"
    );

    asleep(&host, &[&named]).await;
    let prepared = native.prepare_member(&member, MemberRoute::Child).await;
    assert!(
        prepared.is_ok(),
        "a delivery to a member of a mob that is not loaded is prepared: {prepared:?}"
    );
    assert!(
        known(&host).await.loaded.contains(&named),
        "the preparation loaded the mob"
    );

    asleep(&host, &[&named]).await;
    let cursor = native.member_mob_cursor(&mob).await;
    assert!(
        cursor.is_ok(),
        "the cursor of the mob's log is read for a new subscription: {cursor:?}"
    );
    assert!(
        known(&host).await.loaded.contains(&named),
        "the cursor read loaded the mob"
    );

    asleep(&host, &[&named]).await;
    let checked = native.check_member(&member, MemberRoute::Child).await;
    assert_eq!(
        checked,
        Err(MemberProblem::Unknown),
        "the mob was asked for the member, which it does not have"
    );
    assert!(
        known(&host).await.loaded.contains(&named),
        "the check loaded the mob"
    );

    let gone = native.member_mob_gone(&mob, incarnation.as_ref()).await;
    assert_eq!(
        gone,
        Ok(None),
        "the mob loaded again is the mob a member was subscribed in"
    );
    shut_down(&host, &[]).await;
}

/// The preparation of an announcement's delivery to a member of a stopped
/// mob is answered the same way whether or not the mob was loaded: a
/// stopped mob that is not loaded is loaded for it, as any call is that a
/// loaded stopped mob answers itself, and it is still stopped afterwards.
#[tokio::test]
async fn a_delivery_prepared_for_a_member_of_a_stopped_mob_is_answered_the_same_loaded_or_not() {
    use crate::mob_announcements::native::{MemberRoute, NativeDelivery};

    let stored = StoredHost::new();
    let host = stored.launch().await;
    let stays_loaded = second_mob(&host, definition("stopped-and-loaded")).await;
    let unloaded = second_mob(&host, definition("stopped-and-asleep")).await;
    let native = announcements_native(&host);
    for mob_id in [&stays_loaded, &unloaded] {
        set_of(&host)
            .mob_stop(mob_id)
            .await
            .expect("the mob is stopped");
    }
    // One stays loaded behind an owner lease, as a host service's work on
    // it holds it; the other is unloaded as idle.
    let lease = set_of(&host)
        .handle_for(&stays_loaded)
        .await
        .expect("the mob that stays loaded")
        .acquire_owner()
        .expect("an owner lease");
    asleep(&host, &[&unloaded]).await;
    assert!(
        known(&host).await.loaded.contains(&stays_loaded),
        "the leased mob stays loaded"
    );

    let mut answers = Vec::new();
    for mob_id in [&stays_loaded, &unloaded] {
        let prepared = native
            .prepare_member(&announced_member(mob_id, "desk"), MemberRoute::Child)
            .await;
        answers.push(format!("{prepared:?}").replace(mob_id.as_str(), "<the mob>"));
    }
    assert_eq!(
        answers[1], answers[0],
        "the preparation for the stopped mob that was not loaded is answered as the loaded one's"
    );
    assert!(
        known(&host).await.loaded.contains(&unloaded),
        "the preparation loaded the stopped mob to answer it"
    );
    for mob_id in [&stays_loaded, &unloaded] {
        assert_eq!(
            set_of(&host)
                .handle_for(mob_id)
                .await
                .expect("the stopped mob")
                .status_observation_snapshot(),
            meerkat_mob::MobState::Stopped,
            "the mob {mob_id} is still stopped"
        );
    }
    drop(lease);
    shut_down(&host, &[&stays_loaded, &unloaded]).await;
}

/// What the live lineage records hold of the mob the host created.
#[derive(Debug, PartialEq, Eq)]
enum HostsMobRecord {
    /// It has no record.
    Unrecorded,
    /// Its record names no parent.
    NoParent,
    /// Its record names a parent.
    UnderAParent,
}

/// The two mobs of the rows below and what the announcements recorded of
/// the one the host created.
struct JudgedMobs {
    by_a_member: MobId,
    by_the_host: MobId,
    record_of_the_hosts_mob: HostsMobRecord,
}

/// On `host`: a member of the launch's mob creates a mob, the host creates
/// one, and each seats a member `desk`. Over the launch's mob set and the
/// announcements' owner as a gateway composes it: the member's mob is
/// recorded under the launch's mob and is below it, and the host's mob is
/// not below it. A caller the console keeps apart from other people, who
/// may address every mob, is shown the subscription of the desk of the
/// member's mob and is answered for the one of the host's mob as for an id
/// nobody holds; an administrator is shown it.
async fn the_desk_of_a_mob_the_host_created_is_judged_in_that_mob(
    host: &UnifiedRuntime,
) -> JudgedMobs {
    use crate::access::{AccessControlConfig, AccessController, AccessRule, AccessView};
    use crate::mob_announcements::owner::{AnnouncementConfig, AnnouncementService};
    use crate::mob_announcements::rpc::{
        handle_announcement_method, handle_announcement_method_seen_by,
    };
    use crate::mob_announcements::{AnnouncementStore, SqliteAnnouncementStore};
    use serde_json::json;

    const OF_A_MEMBERS_MOB: &str = "sub-of-a-members-mob";
    const OF_THE_HOSTS_MOB: &str = "sub-of-the-hosts-mob";
    const NOBODYS: &str = "sub-nobody-holds";

    Box::pin(host.spawn(meerkat_mob::SpawnMemberSpec::new("worker", "owner")))
        .await
        .expect("the owner is seated in the launch's mob");
    let owner = host
        .mob_handle()
        .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id("owner"))
        .await
        .expect("the owner's session");
    let set = set_of(host);
    let made = definition("made-by-a-member");
    let by_a_member = made.id.clone();
    set.mob_create_definition_with_owner_bridge_session(made, owner, true, false)
        .await
        .expect("the member's mob is created");
    let by_the_host = second_mob(host, definition("made-by-the-host")).await;
    for mob_id in [&by_a_member, &by_the_host] {
        set.mob_spawn(
            mob_id,
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::AgentIdentity::from("desk"),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("each of the two mobs seats its desk");
    }

    // The announcements' owner over the launch's mob set, as a gateway
    // composes it, with a store of its own.
    let dir = tempfile::tempdir().expect("a directory for the announcements' store");
    let store = Arc::new(
        SqliteAnnouncementStore::open(dir.path().join("mob_announcements.sqlite3"))
            .expect("the announcements' store"),
    );
    let native =
        announcements_native(host).with_members_apart(host.mob_runtime().private_owners().clone());
    let service = AnnouncementService::new(
        Arc::clone(&store) as Arc<dyn AnnouncementStore>,
        Arc::new(native),
        AnnouncementConfig::default(),
    )
    .expect("the announcements' owner");
    let own = announced(host.mob_handle().mob_id());
    let (of_a_member, of_the_host) = (announced(&by_a_member), announced(&by_the_host));

    // What the records say of the two mobs.
    service
        .reconcile_lineage()
        .await
        .expect("the origin of each mob held here is read");
    let records = store.lineage().await.expect("the lineage records");
    let parent_of = |mob: &crate::mob_announcements::MobId| {
        records
            .iter()
            .find(|record| record.mob_id == *mob)
            .map(|record| record.parent.clone())
    };
    assert_eq!(
        parent_of(&of_a_member),
        Some(Some(own.clone())),
        "the mob a member created is recorded under the launch's mob: {records:?}"
    );
    let record_of_the_hosts_mob = match parent_of(&of_the_host) {
        None => HostsMobRecord::Unrecorded,
        Some(None) => HostsMobRecord::NoParent,
        Some(Some(_)) => HostsMobRecord::UnderAParent,
    };
    let held = service
        .mobs_held_here()
        .await
        .expect("the mobs held here can be said");
    let below = service
        .mobs_below_own(&held)
        .await
        .expect("the mobs below the launch's mob can be said");
    assert!(
        below.contains(&of_a_member) && !below.contains(&of_the_host),
        "the member's mob is below the launch's mob and the host's mob is not: {below:?}"
    );

    // The host files one subscription of the desk of each mob.
    for (id, mob_id) in [
        (OF_A_MEMBERS_MOB, &of_a_member),
        (OF_THE_HOSTS_MOB, &of_the_host),
    ] {
        let put = handle_announcement_method(
            Some(&service),
            &own,
            "meta/mob_announcements/subscriptions/put",
            &json!({
                "operation_id": format!("put-{id}"),
                "subscription_id": id,
                "member": { "mob_id": mob_id.as_str(), "identity": "desk" },
                "topics": ["news"],
                "mobs": [own.as_str()],
            }),
        )
        .await;
        assert!(put.is_ok(), "the host files {id}: {put:?}");
    }

    // A console that identifies people: every signed-in caller may work
    // with every agent and address every mob, and one of them administers.
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![
            AccessRule {
                id: "everyone-works-with-every-agent".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec![
                    "agent.*".to_string(),
                    crate::access::ACTION_RUNTIME_ADMIN.to_string(),
                ],
                ..AccessRule::default()
            },
            AccessRule {
                id: "everyone-addresses-every-mob".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                mobs: vec!["*".to_string()],
                ..AccessRule::default()
            },
        ],
        ..AccessControlConfig::default()
    })
    .expect("an access configuration");
    let view_of = |subject: &str| access.view_for_subject(Some(subject)).on_console(true);
    let (person, admin) = (view_of("bob@example.test"), view_of("root@example.test"));
    assert!(
        person.kept_apart_from_other_people() && !admin.kept_apart_from_other_people(),
        "the console keeps the person apart from other people, and not its administrator"
    );
    let read = |view: &AccessView, id: &str| {
        let (view, request) = (view.clone(), json!({ "subscription_id": id }));
        let (service, own) = (service.clone(), own.clone());
        async move {
            handle_announcement_method_seen_by(
                Some(&service),
                &own,
                Some(&view),
                "meta/mob_announcements/subscriptions/get",
                &request,
            )
            .await
        }
    };
    let names_the_desk = |answer: &serde_json::Value, mob: &crate::mob_announcements::MobId| {
        answer["subscription"]["member"] == json!({ "mob_id": mob.as_str(), "identity": "desk" })
    };
    let spelled = |answer: &Result<serde_json::Value, crate::rpc::JsonRpcError>, id: &str| {
        format!("{answer:?}").replace(id, "<id>")
    };

    let shown = read(&person, OF_A_MEMBERS_MOB).await;
    assert!(
        shown
            .as_ref()
            .is_ok_and(|answer| names_the_desk(answer, &of_a_member)),
        "the person is shown the subscription of the desk of the member's mob: {shown:?}"
    );
    let hidden = read(&person, OF_THE_HOSTS_MOB).await;
    let absent = read(&person, NOBODYS).await;
    assert!(
        !hidden
            .as_ref()
            .is_ok_and(|answer| !answer["subscription"].is_null()),
        "the person is shown no subscription of the desk of the host's mob: {hidden:?}"
    );
    assert_eq!(
        spelled(&hidden, OF_THE_HOSTS_MOB),
        spelled(&absent, NOBODYS),
        "and is answered for it as for an id nobody holds"
    );
    let whole = read(&admin, OF_THE_HOSTS_MOB).await;
    assert!(
        whole
            .as_ref()
            .is_ok_and(|answer| names_the_desk(answer, &of_the_host)),
        "an administrator is shown the subscription of the desk of the host's mob: {whole:?}"
    );
    JudgedMobs {
        by_a_member,
        by_the_host,
        record_of_the_hosts_mob,
    }
}

/// The Console's mob announcement methods judge a member of a mob the host
/// itself created as a member of that mob. The announcements record no
/// parent for such a mob, so it is no mob below the launch's mob, while a
/// mob a member of the launch's mob created is one. A caller the console
/// keeps apart from other people, who may address every mob, is answered
/// for a subscription of a member of the host's mob, whose attributes the
/// console holds none of, as for an id nobody holds. The subscription of
/// the member of the same name in the member's mob is shown to that caller,
/// and an administrator is shown both.
#[tokio::test]
async fn the_console_judges_a_member_of_a_mob_the_host_created_in_that_mob() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let judged = the_desk_of_a_mob_the_host_created_is_judged_in_that_mob(&host).await;
    assert_eq!(
        judged.record_of_the_hosts_mob,
        HostsMobRecord::NoParent,
        "the mob the host created is recorded with no parent"
    );
    shut_down(&host, &[]).await;
}

/// The same on a launch that declares no store of its mob to its mob set
/// (a runtime the builder makes from a definition), where a mob the host
/// creates is one of the mob set's own: it is recorded with no parent, it
/// is no mob below the launch's mob, and its member is judged in that mob.
#[tokio::test]
async fn the_console_judges_a_hosts_mob_the_same_on_a_launch_without_a_declared_store() {
    let state = tempfile::tempdir().expect("a state directory");
    let host = UnifiedRuntime::builder()
        .definition(definition("host-without-a-declared-store"))
        .persistent_state(state.path())
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("a runtime the builder makes from a definition");
    let judged = the_desk_of_a_mob_the_host_created_is_judged_in_that_mob(&host).await;
    assert_eq!(
        judged.record_of_the_hosts_mob,
        HostsMobRecord::NoParent,
        "the mob the host created is recorded with no parent"
    );
    shut_down(&host, &[&judged.by_a_member, &judged.by_the_host]).await;
}

/// One turn of the member `name` of the mob behind `handle`, with `word` as
/// its input, to its end.
async fn takes_a_turn(handle: &meerkat_mob::MobHandle, name: &str, word: &str) {
    use meerkat_core::types::{ContentInput, HandlingMode};
    use meerkat_mob::{BoundedResultSpec, WorkOrigin, WorkSpec};

    let spec = BoundedResultSpec::new("turn", 4096).expect("a bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            meerkat_mob::AgentIdentity::from(name),
            WorkSpec::new(ContentInput::Text(word.to_string()), WorkOrigin::Internal),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("the member's turn starts");
    tokio::time::timeout(Duration::from_mins(1), work.wait_bounded(spec))
        .await
        .expect("the turn ends within a minute")
        .expect("the turn succeeds");
}

/// A mob a member created is asleep until something addresses it, and
/// nothing else changes for it. After the mob set unloaded it, and after a
/// restart of the gateway (which loads it no more than any other stored
/// mob), the lookup of the mob by its id loads it, its member is the member
/// it was, in the session it had, and that member takes a turn.
#[tokio::test]
async fn a_member_of_a_mob_a_member_created_takes_a_turn_after_an_unload_and_after_a_restart() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    Box::pin(host.spawn(meerkat_mob::SpawnMemberSpec::new("worker", "owner")))
        .await
        .expect("the owner is seated in the launch's mob");
    let owner = host
        .mob_handle()
        .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id("owner"))
        .await
        .expect("the owner's session");
    let made = definition("made-by-a-member");
    let child = made.id.clone();
    let set = set_of(&host);
    set.mob_create_definition_with_owner_bridge_session(made, owner, true, false)
        .await
        .expect("the member's mob is created");
    set.mob_spawn(
        &child,
        meerkat_mob::ProfileName::from("worker"),
        meerkat_mob::AgentIdentity::from("desk"),
        Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        None,
        None,
    )
    .await
    .expect("the member's mob seats its desk");
    let desk = meerkat_mob::AgentIdentity::from("desk");
    let seated_in = set
        .handle_for(&child)
        .await
        .expect("the member's mob")
        .resolve_bridge_session_id(&desk)
        .await;
    assert!(seated_in.is_some(), "the desk has a session");

    asleep(&host, &[&child]).await;
    let loaded = set
        .handle_for(&child)
        .await
        .expect("the lookup loads the mob a member created");
    assert_eq!(
        loaded.resolve_bridge_session_id(&desk).await,
        seated_in,
        "the member is in the session it had before the unload"
    );
    takes_a_turn(&loaded, "desk", "a word after the unload").await;
    drop(loaded);
    drop(set);
    shut_down(&host, &[]).await;
    drop(host);

    let host = stored.launch().await;
    let at_start = known(&host).await;
    assert!(
        at_start.not_loaded.contains(&child) && !at_start.loaded.contains(&child),
        "a restart loads no mob a member created until something addresses it: it manages \
         {:?} and knows as not loaded {:?}",
        at_start.loaded,
        at_start.not_loaded
    );
    let loaded = set_of(&host)
        .handle_for(&child)
        .await
        .expect("the lookup loads the mob after the restart");
    assert_eq!(
        loaded.resolve_bridge_session_id(&desk).await,
        seated_in,
        "the member is in the session it had before the restart"
    );
    takes_a_turn(&loaded, "desk", "a word after the restart").await;
    drop(loaded);
    shut_down(&host, &[]).await;
}

/// A create under an id withdraws the waiting requests that name that id
/// and no other. On a host of one mob a request the stdio door evaluated
/// waits for a person, and the audit, which keeps its newest rows, holds
/// none of the rows its evaluation wrote any more. A member of the launch
/// mob then creates a mob of its own: the request still waits, and the
/// audit holds no withdrawal.
#[tokio::test]
async fn a_members_create_withdraws_no_request_that_names_no_mob() {
    fn request(action: &str, risk_tier: &str) -> crate::runtime::GatingEvaluateRequest {
        serde_json::from_value(serde_json::json!({
            "action": action,
            "actor_id": "host-job",
            "risk_tier": risk_tier,
            "approval_timeout_ms": 600_000,
        }))
        .expect("a gating request")
    }
    async fn waiting(host: &UnifiedRuntime) -> Vec<String> {
        host.list_gating_pending()
            .await
            .into_iter()
            .map(|entry| entry.pending_id)
            .collect()
    }

    let stored = StoredHost::new();
    let host = stored.launch().await;
    Box::pin(host.spawn(meerkat_mob::SpawnMemberSpec::new("worker", "owner")))
        .await
        .expect("the owner is seated in the launch's mob");
    let owner = host
        .mob_handle()
        .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id("owner"))
        .await
        .expect("the owner's session");

    // As the stdio door evaluates a request for the launch mob: no origin.
    let asked = host
        .evaluate_gating_action_of_mob(request("publish_report", "r3"), None)
        .await;
    let pending_id = asked
        .pending_id
        .clone()
        .expect("the action waits for an approval");
    // Newer rows, until the audit holds none of that evaluation's.
    let mut newer = 0_u32;
    while host
        .gating_audit_entries(usize::MAX)
        .await
        .iter()
        .any(|entry| entry.action_id == asked.action_id)
    {
        assert!(
            newer < 2_000,
            "the audit keeps its newest rows: {newer} newer rows were written"
        );
        host.evaluate_gating_action_of_mob(request("read_report", "r0"), None)
            .await;
        newer += 1;
    }
    let waiting_before = waiting(&host).await;

    let made = definition("made-by-a-member");
    let child = made.id.clone();
    set_of(&host)
        .mob_create_definition_with_owner_bridge_session(made, owner, true, false)
        .await
        .expect("the member's mob is created");
    let waiting_after = waiting(&host).await;
    let withdrawals: Vec<serde_json::Value> = host
        .gating_audit_entries(usize::MAX)
        .await
        .into_iter()
        .filter(|entry| entry.event_type == "request_withdrawn")
        .map(|entry| entry.detail)
        .collect();
    shut_down(&host, &[&child]).await;

    let the_request = vec![pending_id];
    assert_eq!(
        waiting_before, the_request,
        "the request waits before the create"
    );
    assert_eq!(
        waiting_after, the_request,
        "the request still waits after a member created a mob"
    );
    assert_eq!(
        withdrawals,
        Vec::<serde_json::Value>::new(),
        "the create withdrew nothing"
    );
}

/// A schedule for a member of a mob of the host that is not loaded fires at
/// that member: the fire loads the mob, completes at its delivery, and the
/// member holds its word.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_a_member_of_a_mob_that_is_not_loaded_fires_at_that_member() {
    use meerkat::{
        CreateScheduleRequest, MobTargetBinding, OccurrencePhase, ScheduledMobAction,
        TargetBinding, TriggerSpec,
    };

    use crate::gateway_composition::{
        GatewayScheduleHostInputs, ScheduleFiringRole, adopt_schedule_mob_targets,
        attach_gateway_schedule_tools, spawn_gateway_schedule_host,
    };

    const WORD: &str = "the scheduled word for the desk of a mob that is not loaded";
    let stored = StoredHost::new();
    let schedule_state = stored.state_dir().join("schedule-state");
    std::fs::create_dir_all(&schedule_state).expect("the schedule state directory");
    let (host, sessions, machine, attached) = stored
        .launch_over(|builder| {
            attach_gateway_schedule_tools(builder, &schedule_state, ScheduleFiringRole::Executor)
                .expect("the schedule tools attach")
        })
        .await;
    let second = second_mob(&host, definition("scheduled-while-asleep")).await;
    let desk = crate::member_comms_id::mob_member_id("desk");
    // Seated turn-driven: a member that runs a turn of its own at its spawn
    // keeps its mob busy for the idle unload while that turn is under way.
    set_of(&host)
        .mob_spawn(
            &second,
            meerkat_mob::ProfileName::from("worker"),
            desk.clone(),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("the second mob's desk");
    let desk_session = set_of(&host)
        .handle_for(&second)
        .await
        .expect("the second mob")
        .resolve_bridge_session_id(&desk)
        .await
        .expect("the desk's session");
    let mob_state =
        adopt_schedule_mob_targets(&host, &attached.service, &attached.mob_target_registry).await;
    let (schedule_host, watchdog) = spawn_gateway_schedule_host(
        &host,
        mob_state,
        GatewayScheduleHostInputs {
            schedule_service: attached.service.clone(),
            session_service: Arc::clone(&sessions),
            runtime_adapter: machine,
            schedule_store_path: schedule_state.join(crate::schedule_wiring::SCHEDULE_STORE_FILE),
            firing_host_binding: attached.firing_host_binding.clone(),
            runnable_host: None,
            workgraph_service: None,
            owner_id: format!("lazy-start-{}", uuid::Uuid::new_v4().simple()),
        },
    )
    .await;
    assert!(
        schedule_host.is_some(),
        "the schedule host spawns over the launch"
    );

    asleep(&host, &[&second]).await;
    let schedule = attached
        .service
        .create(CreateScheduleRequest {
            name: Some("a word for the desk of a mob that is not loaded".to_string()),
            description: None,
            trigger: TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::seconds(1),
            },
            target: TargetBinding::Mob(Box::new(MobTargetBinding::Member {
                mob_id: second.to_string(),
                member_id: desk.to_string(),
                action: ScheduledMobAction::Send {
                    content: meerkat_core::ContentInput::Text(WORD.to_string()),
                    render_metadata: None,
                },
            })),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: std::collections::BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await
        .expect("the schedule is created");
    let deadline = tokio::time::Instant::now() + SETTLES_WITHIN;
    let fired = loop {
        let occurrence = attached
            .service
            .list_occurrences(&schedule.schedule_id)
            .await
            .expect("the occurrences are listed")
            .into_iter()
            .next();
        let terminal = occurrence.as_ref().is_some_and(|occurrence| {
            matches!(
                occurrence.phase,
                OccurrencePhase::Completed
                    | OccurrencePhase::Skipped
                    | OccurrencePhase::Misfired
                    | OccurrencePhase::Superseded
                    | OccurrencePhase::DeliveryFailed
            )
        });
        if terminal || tokio::time::Instant::now() >= deadline {
            break occurrence;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(
        fired.map(|occurrence| format!("{:?}", occurrence.phase)),
        Some(format!("{:?}", OccurrencePhase::Completed)),
        "the fire for a member of a mob that is not loaded completes at its delivery"
    );
    assert!(
        transcript_showing(&sessions, &desk_session, WORD)
            .await
            .contains(WORD),
        "the member of the mob that was not loaded holds the scheduled word"
    );
    assert!(
        known(&host).await.loaded.contains(&second),
        "the fire loaded the mob"
    );

    if let Some(schedule_host) = schedule_host {
        schedule_host.shutdown().await;
    }
    watchdog.abort();
    let _ = watchdog.await;
    shut_down(&host, &[&second]).await;
}

/// A schedule that fires for a member of a stopped mob ends the same way
/// whether or not the mob was loaded. A stopped mob that is not loaded is
/// loaded by the fire, as any call is answered that a loaded stopped mob
/// answers itself: the loaded mob refuses the send as a stopped mob does,
/// the occurrence ends as the one for the stopped mob that stayed loaded
/// ends, with the same record of its delivery, and the mob is still
/// stopped. Neither member holds the scheduled word.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_a_member_of_a_stopped_mob_ends_the_same_loaded_or_not() {
    use meerkat::{
        CreateScheduleRequest, MobTargetBinding, OccurrencePhase, ScheduledMobAction,
        TargetBinding, TriggerSpec,
    };

    use crate::gateway_composition::{
        GatewayScheduleHostInputs, ScheduleFiringRole, adopt_schedule_mob_targets,
        attach_gateway_schedule_tools, spawn_gateway_schedule_host,
    };

    const WORD: &str = "the scheduled word for the desk of a stopped mob";
    let stored = StoredHost::new();
    let schedule_state = stored.state_dir().join("schedule-state");
    std::fs::create_dir_all(&schedule_state).expect("the schedule state directory");
    let (host, sessions, machine, attached) = stored
        .launch_over(|builder| {
            attach_gateway_schedule_tools(builder, &schedule_state, ScheduleFiringRole::Executor)
                .expect("the schedule tools attach")
        })
        .await;
    let stays_loaded = second_mob(&host, definition("stopped-and-loaded")).await;
    let unloaded = second_mob(&host, definition("stopped-and-asleep")).await;
    let desk = crate::member_comms_id::mob_member_id("desk");
    let mut desk_sessions = Vec::new();
    for mob_id in [&stays_loaded, &unloaded] {
        set_of(&host)
            .mob_spawn(
                mob_id,
                meerkat_mob::ProfileName::from("worker"),
                desk.clone(),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("the mob's desk");
        desk_sessions.push(
            set_of(&host)
                .handle_for(mob_id)
                .await
                .expect("the mob")
                .resolve_bridge_session_id(&desk)
                .await
                .expect("the desk's session"),
        );
    }
    let mob_state =
        adopt_schedule_mob_targets(&host, &attached.service, &attached.mob_target_registry).await;
    let (schedule_host, watchdog) = spawn_gateway_schedule_host(
        &host,
        mob_state,
        GatewayScheduleHostInputs {
            schedule_service: attached.service.clone(),
            session_service: Arc::clone(&sessions),
            runtime_adapter: machine,
            schedule_store_path: schedule_state.join(crate::schedule_wiring::SCHEDULE_STORE_FILE),
            firing_host_binding: attached.firing_host_binding.clone(),
            runnable_host: None,
            workgraph_service: None,
            owner_id: format!("lazy-start-{}", uuid::Uuid::new_v4().simple()),
        },
    )
    .await;
    assert!(
        schedule_host.is_some(),
        "the schedule host spawns over the launch"
    );

    // Both mobs are stopped. One stays loaded behind an owner lease, as a
    // host service's work on it holds it; the other is unloaded as idle.
    for mob_id in [&stays_loaded, &unloaded] {
        set_of(&host)
            .mob_stop(mob_id)
            .await
            .expect("the mob is stopped");
    }
    let lease = set_of(&host)
        .handle_for(&stays_loaded)
        .await
        .expect("the mob that stays loaded")
        .acquire_owner()
        .expect("an owner lease");
    asleep(&host, &[&unloaded]).await;
    assert!(
        known(&host).await.loaded.contains(&stays_loaded),
        "the leased mob stays loaded"
    );

    // What an occurrence's end says, without what names its own mob,
    // schedule or hour.
    let said = |occurrence: &meerkat::Occurrence, mob_id: &MobId| {
        format!(
            "{:?} class {:?} outcome {:?} detail {:?} attempts {} receipt {} correlation {}",
            occurrence.phase,
            occurrence.failure_class,
            occurrence.runtime_outcome,
            occurrence
                .failure_detail
                .as_deref()
                .map(|detail| detail.replace(mob_id.as_str(), "<the mob>")),
            occurrence.attempt_count,
            occurrence.last_receipt.is_some(),
            occurrence.delivery_correlation_id.is_some(),
        )
    };
    let mut ends = Vec::new();
    for mob_id in [&stays_loaded, &unloaded] {
        let schedule = attached
            .service
            .create(CreateScheduleRequest {
                name: Some(format!("a word for the desk of the stopped mob {mob_id}")),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: chrono::Utc::now() + chrono::Duration::seconds(1),
                },
                target: TargetBinding::Mob(Box::new(MobTargetBinding::Member {
                    mob_id: mob_id.to_string(),
                    member_id: desk.to_string(),
                    action: ScheduledMobAction::Send {
                        content: meerkat_core::ContentInput::Text(WORD.to_string()),
                        render_metadata: None,
                    },
                })),
                misfire_policy: meerkat::MisfirePolicy::default(),
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: std::collections::BTreeMap::new(),
                planning_horizon_days: None,
                planning_horizon_occurrences: None,
            })
            .await
            .expect("the schedule is created");
        let deadline = tokio::time::Instant::now() + SETTLES_WITHIN;
        let ended = loop {
            let occurrence = attached
                .service
                .list_occurrences(&schedule.schedule_id)
                .await
                .expect("the occurrences are listed")
                .into_iter()
                .next();
            let terminal = occurrence.as_ref().is_some_and(|occurrence| {
                matches!(
                    occurrence.phase,
                    OccurrencePhase::Completed
                        | OccurrencePhase::Skipped
                        | OccurrencePhase::Misfired
                        | OccurrencePhase::Superseded
                        | OccurrencePhase::DeliveryFailed
                )
            });
            if terminal || tokio::time::Instant::now() >= deadline {
                break occurrence;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        ends.push(ended.map(|occurrence| said(&occurrence, mob_id)));
    }

    let for_the_loaded = ends[0].clone().expect("the loaded mob's occurrence");
    assert!(
        for_the_loaded.starts_with(&format!("{:?} ", OccurrencePhase::DeliveryFailed))
            && for_the_loaded.contains("Stopped"),
        "the fire for a member of a stopped mob that is loaded ends with its delivery failed, \
         refused as a stopped mob refuses a send: {for_the_loaded}"
    );
    assert_eq!(
        ends[1].as_deref(),
        Some(for_the_loaded.as_str()),
        "the fire for a member of a stopped mob that was not loaded ends as the loaded one's"
    );
    assert!(
        known(&host).await.loaded.contains(&unloaded),
        "the fire loaded the stopped mob to answer it"
    );
    for mob_id in [&stays_loaded, &unloaded] {
        assert_eq!(
            set_of(&host)
                .handle_for(mob_id)
                .await
                .expect("the stopped mob")
                .status_observation_snapshot(),
            meerkat_mob::MobState::Stopped,
            "the mob {mob_id} is still stopped"
        );
    }
    // Both occurrences ended: a word that was delivered is in a transcript
    // by now.
    for session in &desk_sessions {
        assert!(
            !super::super::mob_entries_tests::transcript(&sessions, session)
                .await
                .contains(WORD),
            "no member of a stopped mob holds the scheduled word"
        );
    }
    drop(lease);

    if let Some(schedule_host) = schedule_host {
        schedule_host.shutdown().await;
    }
    watchdog.abort();
    let _ = watchdog.await;
    shut_down(&host, &[&stays_loaded, &unloaded]).await;
}

/// A gateway that puts the announcements' member tools on its launch's
/// spec has said, before its runtime is built, that the launch composes
/// announcements: the spec's release of what the announcements keep under
/// a mob's id is owed from then on, so a create that comes before the
/// announcements' owner is attached is refused and not made over what an
/// earlier mob of its id left. A spec without them declares nothing.
#[tokio::test]
async fn a_spec_that_takes_the_announcements_member_tools_owes_their_release() {
    use crate::mob_announcements_release::MobAnnouncementsStanding;

    let standing = |spec: &MobBootstrapSpec| match spec.mob_announcements_release.standing() {
        MobAnnouncementsStanding::NotDeclared => "not declared",
        MobAnnouncementsStanding::Owed => "owed",
        MobAnnouncementsStanding::Bound(_) => "bound",
    };
    let stored = StoredHost::new();
    let spec = stored.spec();
    assert_eq!(standing(&spec), "not declared");
    let composition = crate::mob_announcements::AnnouncementComposition::new(
        crate::mob_announcements::AnnouncementConfig::default(),
    )
    .expect("the default announcement configuration");
    let spec = composition.register_member_tools(spec);
    assert_eq!(
        standing(&spec),
        "owed",
        "the spec that takes the member tools declares the announcements"
    );
}

/// The question whose reach a mob is in is answered for a mob of the host
/// that is not loaded from the record the mob set keeps of its creation,
/// and loads nothing: the mob the host created is its own reach, it stays
/// not loaded, and the runtime builds no entry for it. A judge of a request
/// asks this question; it must not be what loads a mob.
#[tokio::test]
async fn the_reach_of_a_mob_that_is_not_loaded_is_answered_without_a_load() {
    let stored = StoredHost::new();
    let host = stored.launch().await;
    let named = second_mob(&host, definition("reach-not-loaded")).await;
    asleep(&host, &[&named]).await;
    assert!(
        entries_are(&host, &[]).await,
        "setup: the entry went with the unload: {:?}",
        host.mob_entries_in_tests()
    );

    let reach = host.mob_hosting(&named).await;

    let after = known(&host).await;
    assert_eq!(
        reach,
        Ok(Some(named.clone())),
        "a mob the host created is its own reach, by the set's record of its creation"
    );
    assert!(
        after.not_loaded.contains(&named) && !after.loaded.contains(&named),
        "the mob stays not loaded: the set manages {:?} and knows as not loaded {:?}",
        after.loaded,
        after.not_loaded
    );
    assert_eq!(
        host.mob_entries_in_tests(),
        Vec::<MobId>::new(),
        "the runtime builds no entry for it"
    );
    shut_down(&host, &[]).await;
}

/// A wake of the session of a member of a mob of the host that is not
/// loaded, asked by a road that holds only the session's id, loads that
/// mob and is answered by it: the launch states to its mob set that the set
/// answers such a wake. A candidate launch over the same directory acts
/// nowhere: it states nothing, the same wake finds no owner, and the mob
/// stays not loaded.
#[tokio::test]
async fn a_wake_by_session_loads_the_members_mob_and_a_candidate_launch_answers_none() {
    use meerkat_core::session_owner_wake::{SessionOwnerWakeOutcome, SessionOwnerWakeRegistry};

    let stored = StoredHost::new();
    let host = stored.launch().await;
    let mob = second_mob(&host, definition("woken-by-session")).await;
    let set = set_of(&host);
    let desk = meerkat_mob::AgentIdentity::from("desk");
    set.mob_spawn(
        &mob,
        meerkat_mob::ProfileName::from("worker"),
        desk.clone(),
        Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        None,
        None,
    )
    .await
    .expect("the second mob seats its desk");
    let session = set
        .handle_for(&mob)
        .await
        .expect("the second mob")
        .resolve_bridge_session_id(&desk)
        .await
        .expect("the desk has a session");
    asleep(&host, &[&mob]).await;

    let woken = SessionOwnerWakeRegistry::global().wake(&session).await;
    assert!(
        matches!(woken, Some(Ok(SessionOwnerWakeOutcome::Ready))),
        "the wake of the desk's session is answered by its mob, loaded for it: {woken:?}"
    );
    let after = known(&host).await;
    assert!(
        after.loaded.contains(&mob),
        "the wake loaded the desk's mob: the set manages {:?} and knows as not loaded {:?}",
        after.loaded,
        after.not_loaded
    );
    drop(set);
    // The launch ends as the rows that launch again end theirs: the
    // runtime's own shutdown asks every mob of the host, so the state
    // directory is free for the next launch.
    shut_down(&host, &[]).await;
    drop(host);

    let candidate = stored
        .bootstrap(stored.spec().with_composition_authority(
            crate::mob_composition_manifest::CompositionAuthority::NonAuthoritative,
        ))
        .await;
    let before = known(&candidate).await;
    assert!(
        before.not_loaded.contains(&mob) && !before.loaded.contains(&mob),
        "the candidate launch knows the mob and has not loaded it: it manages {:?} and knows \
         as not loaded {:?}",
        before.loaded,
        before.not_loaded
    );
    let woken = SessionOwnerWakeRegistry::global().wake(&session).await;
    assert!(
        woken.is_none(),
        "a candidate launch's mob set answers no wake: {woken:?}"
    );
    let after = known(&candidate).await;
    assert!(
        after.not_loaded.contains(&mob) && !after.loaded.contains(&mob),
        "the wake loaded nothing on the candidate launch: it manages {:?} and knows as not \
         loaded {:?}",
        after.loaded,
        after.not_loaded
    );
    shut_down(&candidate, &[]).await;
}
