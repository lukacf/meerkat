//! The entries of the mobs a host serves beside its default mob.
//!
//! Which mobs a host has, who created each, and whether a mob is loaded are
//! facts of the host's mob set (`meerkat_mob_mcp::MobMcpState`), which keeps
//! them in its per-store records. This table holds none of them. It is the
//! runtime's mechanics for the mobs the set built for the host and manages
//! now: one [`MobEntry`] per such mob, built when the set manages the mob
//! and dropped when the set no longer manages that mob's actor (an unload, a
//! destroy, the stand-down). Every question about a mob is put to the set.
//!
//! The producer is the set's own build of a mob, through the launch's
//! composition: each time the set builds a mob of the host that is not the
//! default mob (its create, and every load of its store), the composition
//! asks this table for that mob's member wiring
//! ([`HostMobEntryWiring`]). The wiring is made then, before the mob
//! exists, and the mob's entry is built around it once the set manages the
//! mob. A mob a member created is never given to the composition, so it
//! never has an entry here and resolves on no request.
//!
//! Nothing here starts before a host builds such a mob: a host of one mob
//! holds an empty table and no task.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use async_trait::async_trait;
use meerkat_mob::{MobError, MobHandle, MobId, MobProvenance};
use tokio::task::JoinHandle;

use super::mob_entry::MobEntry;
use super::mob_events::MobEventsStore;
use super::mob_identity_plane::HostMobIdentityComposer;
use super::{
    EdgeDiscovery, HealthCompletionLedger, SharedErrorHook, UnifiedRuntime, bootstrap_completion,
    edge_reconcile, workgraph_fact_hub_and_tail,
};
use crate::gateway_wiring::MemberBuildWiring;
use crate::identity_creation::LaunchIdentityPolicy;
use crate::mob_announcements_release::MobAnnouncementsStanding;
use crate::mob_handle_runtime::{HostMobEntryWiring, HostMobMemberWiring, MobRuntime};
use crate::mob_memory_release::MobMemoryStanding;
use crate::runtime::{PersistentMetadataStore, RuntimeMetadataTable};

/// What every entry of this table is built from: the launch's shared
/// services, known once the launch's own mob is built.
pub(super) struct HostOfEntries {
    /// The default mob's wrapper, observer side. Its process part is the
    /// same for every mob of the launch, and it carries the launch's mob
    /// set.
    pub(super) launch_mob: MobRuntime,
    pub(super) persistent_metadata: Arc<dyn PersistentMetadataStore>,
    pub(super) metadata_table: Arc<RuntimeMetadataTable>,
    pub(super) error_hook: SharedErrorHook,
    /// The host's console events: the store of a mob of this table ends
    /// when the mob's entry goes.
    pub(super) console_events: super::ConsoleEventStore,
    /// The runtime's console projection, once it built one: an entry's
    /// console events are registered with it, and that registration ends
    /// with the entry.
    pub(super) console_projection: super::mob_console_log::ConsoleProjectionSlot,
    /// The host's console log: what it keeps under a mob's id is removed
    /// before a mob is created under that id.
    pub(super) console_log: super::mob_console_log::HostConsoleLog,
    /// The host's module runtime: the routes, the routing resolutions and
    /// the deliveries it keeps under a mob's id go when a mob is created
    /// under that id. Held weakly: the runtime owns it, this table does not
    /// keep it alive, and a module runtime that is gone keeps nothing to
    /// forget.
    pub(super) module_runtime: Weak<tokio::sync::Mutex<crate::runtime::MetaRuntimeHandle>>,
    /// The writer of the gating owner's state, from when the runtime keeps
    /// that state across restarts ([`MobEntries::keep_gating_state`]): the
    /// approvals the host withdraws when a mob is created under an id are
    /// written before the create goes on.
    pub(super) gating_state_writer:
        OnceLock<Arc<crate::gating_persistence::GatingStatePersistence>>,
}

/// What becomes of the agent memory of a mob the host served beside its
/// launch mob when the mob set destroyed that mob.
///
/// THE ONE PLACE that says so: [`MobEntries::release_memory`] reads
/// [`MEMORY_OF_A_DESTROYED_MOB`], and nothing else does. Whichever stands, a
/// mob created under the id of an earlier mob starts with none of that
/// mob's memory: the release before a create does not read this.
#[derive(Clone, Copy)]
enum MemoryOfADestroyedMob {
    /// It goes with the mob: the destroy's release removes it.
    Removed,
    /// It stays on disk, read by nobody, until a mob is created under the
    /// id: the destroy's release removes nothing.
    #[allow(
        dead_code,
        reason = "the other answer to a product question that is open; this one word selects it"
    )]
    Kept,
}

/// See [`MemoryOfADestroyedMob`].
const MEMORY_OF_A_DESTROYED_MOB: MemoryOfADestroyedMob = MemoryOfADestroyedMob::Removed;

/// What a release of the memory of a destroyed mob answered where it did
/// not fail ([`MobEntries::release_memory`]).
///
/// A failure is an error and never one of these: what could not be read,
/// what could not be removed, a release that names the launch mob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestroyedMobMemory {
    /// Nothing is owed by this release: the memory under the id was
    /// released, there was none, the launch keeps none for the mobs it
    /// hosts, or a destroyed mob's memory stays by the one rule that says
    /// so ([`MemoryOfADestroyedMob`]).
    Done,
    /// The mob set hosts a mob under the id (it manages it, holds it before
    /// activation, keeps its store quarantined, or knows it and has not
    /// loaded it): that mob's memory is kept, and this call released
    /// nothing. Read from the set's own records under the hold of the
    /// table's passes, never from what a caller sends. It says that a mob
    /// stands under the id, not which one: that it is another mob than the
    /// one a caller destroyed is known to the caller that saw the set's
    /// destroy finish.
    KeptForTheMobUnderTheId,
}

/// What stands under a mob id ([`MobEntries::stands_under_the_hold`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum UnderTheId {
    /// Neither the mob set nor the table holds anything under the id.
    Nothing,
    /// The mob set hosts a mob under the id.
    AMobOfTheSet,
    /// The mob set hosts no mob under the id and the table still holds an
    /// entry of one: no pass ended it.
    AnEntryAlone,
}

/// How many mobs a mob's creation is followed through, from the mob a
/// member created to the mob of that member and on, before the question
/// whose reach a mob is in gives up ([`MobEntries::hosting`]).
const MOB_CREATION_LINEAGE_BOUND: usize = 16;

/// What the creation of a mob says of the reach the mob is in: one step of
/// [`MobEntries::hosting`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ReachOfACreation {
    /// The host created the mob: it is a mob the host hosts.
    Hosted,
    /// A member of this mob created it: it is in the reach that mob is in.
    CreatedIn(MobId),
    /// Neither the host nor a member created it: it is in no mob's reach.
    None,
}

/// The reach the creation `created_by` of the mob `mob` puts that mob in.
/// `Err` for a creation that does not decide it: one that names more than
/// one roster, and a kind of creation this judgement does not know (the
/// type grows with later releases).
pub(super) fn reach_of_a_creation(
    mob: &MobId,
    created_by: MobProvenance,
) -> Result<ReachOfACreation, String> {
    match created_by {
        MobProvenance::Host => Ok(ReachOfACreation::Hosted),
        MobProvenance::Member { parent_mob_id, .. }
        | MobProvenance::OperatorMember { parent_mob_id, .. } => {
            Ok(ReachOfACreation::CreatedIn(parent_mob_id))
        }
        MobProvenance::Session { .. }
        | MobProvenance::OperatorSession { .. }
        | MobProvenance::Unrecorded => Ok(ReachOfACreation::None),
        MobProvenance::Ambiguous { matching_rosters } => Err(format!(
            "the creation of mob '{mob}' names {matching_rosters} rosters"
        )),
        _ => Err(format!(
            "the creation of mob '{mob}' is of a kind this judgement does not know"
        )),
    }
}

/// Who created the mob `mob`, as the store file `store` records it, by
/// Core's read-only reader of a store file
/// (`MobStorage::read_stored_creator`): the file is opened read-only, and
/// nothing of it is created, brought forward to a later schema or repaired.
/// `None` when the file does not answer: it is missing or does not read, it
/// holds no creation record (a store an earlier release wrote before
/// creation records), its record is of a finished destroy, or its record
/// names another mob than `mob`.
pub(super) async fn creation_a_store_records(
    store: &std::path::Path,
    mob: &MobId,
) -> Option<MobProvenance> {
    meerkat_mob::MobStorage::read_stored_creator(store, mob)
        .await
        .ok()
        .flatten()
}

/// Whether the creation the mob set's record keeps of a mob says that
/// another than the host created it: a member, a session, a caller that
/// several rosters named, or a creation from before creators were
/// recorded. The host's own creation is not another's. Neither is a kind
/// this judgement does not name (the type grows with later releases): a
/// mob with such a record may be a mob of the host.
pub(super) fn another_created(creation: &MobProvenance) -> bool {
    matches!(
        creation,
        MobProvenance::Member { .. }
            | MobProvenance::OperatorMember { .. }
            | MobProvenance::Session { .. }
            | MobProvenance::OperatorSession { .. }
            | MobProvenance::Ambiguous { .. }
            | MobProvenance::Unrecorded
    )
}

/// The entries of the mobs a host serves beside its default mob: see the
/// module.
pub(crate) struct MobEntries {
    /// This table, for the task that follows the set.
    this: Weak<MobEntries>,
    host: OnceLock<HostOfEntries>,
    /// The member wiring of each mob the set is building, or has built, for
    /// the host, until that mob's entry takes it. It is kept under the
    /// mob's id and says nothing by itself: a pass takes it only for a mob
    /// the set hands a handle for, and the question whether a mob is served
    /// reads it only together with such a handle. A mob the set unloads or
    /// destroys before a pass made its entry leaves its wiring here. The
    /// same mob's next build puts a new one in its place, and a create
    /// under the id drops it before anything of the new mob exists
    /// ([`HostMobEntryWiring::release_before_create`]), so a wiring never
    /// meets a mob it was not made for.
    building: Mutex<BTreeMap<MobId, MemberBuildWiring>>,
    /// The mobs whose entry a pass is making now. A pass takes a mob's
    /// wiring out of `building` and puts the mob's entry into `entries`
    /// only after it built it, which waits (the mob's topology controller,
    /// the mob's links): for that time the mob stands here, entered before
    /// its wiring is taken and left after its entry stands, or when the
    /// pass is dropped half way. So a mob the host serves is at every
    /// moment in one of the three, for whoever reads them in that order.
    making: Mutex<BTreeSet<MobId>>,
    /// The passes that run now, each on a task of its own ([`Self::pass`]).
    /// The table keeps each task's handle until the task ended, and its
    /// close joins every one of them before it ends the entries: no pass
    /// outlives the table's close, with whatever it holds of a mob.
    passes: Mutex<Vec<JoinHandle<()>>>,
    /// The entry of each mob the set built for the host and manages now.
    entries: Mutex<BTreeMap<MobId, Arc<MobEntry>>>,
    /// One pass over the set at a time: the follower's, or a request's.
    pass: tokio::sync::Mutex<()>,
    /// The task that follows the set's changes, once a mob was built for
    /// the host.
    follower: Mutex<Option<JoinHandle<()>>>,
    /// Test builds only: a destroy asks for no pass
    /// ([`HostMobEntryWiring::after_destroy`]), once a row said so.
    #[cfg(test)]
    destroys_ask_for_no_pass_in_tests: AtomicBool,
    /// What the launch gives every other mob's topology controller: the
    /// host's one policy, and whether the host keeps topology state at all
    /// (the host part of the launch's default mob's controller, which holds
    /// no state file). Told once, when the launch has installed the
    /// controller its default mob keeps. No entry is built before it is
    /// told.
    launch_topology: OnceLock<crate::topology_control::HostTopologyControl>,
    /// The runtime began to shut down: no pass takes a mob's wiring once
    /// it reads this, so no entry is begun (a pass that is making one puts
    /// it in, and the close ends it).
    closed: AtomicBool,
    /// Whether this table ever held a mob of the host beside the launch
    /// mob: one was built for the host, a request was told that the host
    /// serves one, or a listing named one. It is never taken back and it
    /// outlives the close: it says what kind of host this is, not what the
    /// table answers now. It starts unset in every process. After a
    /// restart it is set before the first event of a stored mob of the
    /// host can be forwarded (a load builds the mob through
    /// `member_wiring`); a mob below such a mob is not built through this
    /// table, so the forwarder does not rest on this fact for it: it reads
    /// that mob's line ([`Self::lists_another_mob`]).
    hosted_another_mob: AtomicBool,
    /// How the launch composes the identity context of a mob of this
    /// table, said once by a launch that has an identity plane. Each entry
    /// whose mob has no identity context is asked for through it.
    identity_composer: OnceLock<Arc<dyn HostMobIdentityComposer>>,
    /// The launch's identity policy, said once by a launch that has one:
    /// the family of each mob of this table serves that mob's changes
    /// under it. A value, written once; it is no state of any mob.
    identity_policy: OnceLock<LaunchIdentityPolicy>,
    /// The access controllers the runtime was given, each once
    /// ([`Self::keep_access_controller`]): an admitted create has each of
    /// them forget what it holds of the id. They are the
    /// runtime's own; the table holds nothing of a mob here.
    access_controllers: Mutex<Vec<crate::access::AccessController>>,
    /// Test builds only: where a pass stands still, once a test asked for
    /// it, between taking a mob's wiring and putting its entry in.
    #[cfg(test)]
    standstill_in_tests: PassStandstill,
}

/// Test builds only: a standstill of one pass between the moment it took a
/// mob's wiring and the moment it puts that mob's entry in. A test arms
/// it, waits until a pass stands there, asks what it wants answered in
/// that state, and ends it. It also counts the entries the table's passes
/// put in, for a test that asks what a pass built.
#[cfg(test)]
#[derive(Default)]
struct PassStandstill {
    armed: AtomicBool,
    reached: tokio::sync::Notify,
    ended: tokio::sync::Notify,
    put_in: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl PassStandstill {
    /// The pass stands here when a test armed the standstill, once.
    async fn stand(&self) {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.reached.notify_one();
            self.ended.notified().await;
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A mob's place among the mobs whose entry a pass is making
/// (`MobEntries::making`), left when this is dropped: after the entry
/// stands, and when the pass that was making it is dropped half way.
struct EntryBeingMade<'a> {
    making: &'a Mutex<BTreeSet<MobId>>,
    mob_id: MobId,
}

impl Drop for EntryBeingMade<'_> {
    fn drop(&mut self) {
        locked(self.making).remove(&self.mob_id);
    }
}

/// One mob a host serves beside its default mob, as the mob set's records
/// know it at one read: what a door that lists the host's mobs shows of it.
/// It is a reading, kept nowhere: the mob set owns every fact in it.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "read by a door that lists the host's mobs; none reads it yet"
    )
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostedMobListing {
    pub(crate) mob_id: MobId,
    /// Who created the mob. Only a mob the host created is listed.
    pub(crate) provenance: MobProvenance,
    pub(crate) phase: HostedMobPhase,
    /// The incarnation of the mob, an opaque token, when it is known
    /// without opening the mob's store: the one its loaded actor runs.
    pub(crate) incarnation: Option<meerkat_core::service::MobIncarnationId>,
}

/// Where the mob set's record of a mob's store stands, reduced to what a
/// listing tells apart.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "read by a door that lists the host's mobs; none reads it yet"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostedMobPhase {
    /// The set manages the mob: its actor runs.
    Loaded,
    /// The set knows the mob and has not loaded it, or is loading it.
    NotLoaded,
    /// The set keeps the mob's store quarantined, with the class of the
    /// failure its last attempt met.
    CannotBeLoaded(meerkat_mob::MobQuarantineClass),
}

/// Whether a host serves a mob beside its launch mob, as far as that can
/// be told now ([`MobEntries::serves_beside_the_launch_mob`]). A door that
/// reads a record of no known mob as the launch mob's asks it, and takes
/// "not known" as it takes "another mob".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum OtherMobsServed {
    /// The host serves its launch mob alone: its mob set knows no mob
    /// beside it that is, or may be, a mob of the host.
    LaunchMobAlone,
    /// The host has another mob: loaded, not loaded, or one whose store
    /// cannot be loaded.
    AnotherMob,
    /// It cannot be told now: the runtime is going or gone, its table has
    /// no host yet, the mob set's inventory cannot be read, or the listing
    /// names no mob and the set knows one that may be the host's
    /// ([`MobEntries::lists_another_mob`]). Also what a door states that
    /// the host's resolver told nothing: such a door does not know which
    /// mobs its host has.
    #[default]
    NotKnown,
}

impl MobEntries {
    pub(super) fn new() -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            this: Weak::clone(this),
            host: OnceLock::new(),
            building: Mutex::default(),
            making: Mutex::default(),
            passes: Mutex::default(),
            entries: Mutex::default(),
            pass: tokio::sync::Mutex::new(()),
            follower: Mutex::default(),
            #[cfg(test)]
            destroys_ask_for_no_pass_in_tests: AtomicBool::new(false),
            launch_topology: OnceLock::new(),
            closed: AtomicBool::new(false),
            hosted_another_mob: AtomicBool::new(false),
            identity_composer: OnceLock::new(),
            identity_policy: OnceLock::new(),
            access_controllers: Mutex::default(),
            #[cfg(test)]
            standstill_in_tests: PassStandstill::default(),
        })
    }

    /// The runtime was given the access controller `controller`: an
    /// admitted create has it forget what it holds of the id
    /// ([`crate::access::AccessController::forget_mob`]). A runtime given
    /// another controller later keeps both: a console built with the
    /// earlier one still reads it.
    pub(super) fn keep_access_controller(&self, controller: &crate::access::AccessController) {
        let mut kept = locked(&self.access_controllers);
        if !kept
            .iter()
            .any(|known| known.shares_records_with(controller))
        {
            kept.push(controller.clone());
        }
    }

    /// Tell the table what the launch gives every other mob's topology
    /// controller, once the launch has installed the controller its
    /// default mob keeps (`controller`). The table keeps that controller's
    /// host part, not the controller: it holds nothing of the default mob's
    /// state file. From here on the table builds entries: a mob the set
    /// built for the host before gets its entry now.
    pub(super) async fn bind_launch_topology(
        &self,
        controller: &crate::topology_control::TopologyController,
    ) {
        if self.launch_topology.set(controller.host_part()).is_err() {
            return;
        }
        if !locked(&self.building).is_empty() {
            self.pass().await;
        }
    }

    /// Remove the topology state of the mob `mob_id` after the mob set
    /// destroyed it
    /// ([`MobResolver::release_topology_state_of_a_destroyed_mob`](super::mob_entry::MobResolver::release_topology_state_of_a_destroyed_mob)).
    /// One pass first: the entry of a mob the set no longer manages goes,
    /// and with it the hold of its controller on the state file. The pass,
    /// the check that the set no longer hosts the id and the removal run
    /// under the one hold of the table's passes: no entry is built in
    /// between, so a mob created under the id meanwhile opens its state
    /// file only after the removal ended.
    pub(super) async fn release_topology_state(&self, mob_id: &MobId) -> std::io::Result<()> {
        let Some(set) = self.set() else {
            return Ok(());
        };
        let _one_at_a_time = self.pass.lock().await;
        self.pass_under_the_hold().await;
        if self.hosts_under_the_hold(&set, mob_id).await? {
            return Err(std::io::Error::other(format!(
                "the mob {mob_id} is hosted: its topology state is kept"
            )));
        }
        match set.persistent_mob_store_path(mob_id) {
            Some(store) => super::mob_topology::remove_state_files_beside(&store),
            None => Ok(()),
        }
    }

    /// Whether the mob set hosts a mob under `mob_id` now (it manages it,
    /// holds it before activation, keeps its store quarantined, or knows it
    /// and has not loaded it), or the runtime still holds an entry of one.
    /// Read from the set's own records, after a pass, by a caller that
    /// holds the table's passes: no entry is built until that caller lets
    /// go. It is what says that a mob was destroyed: nothing a caller sends
    /// does.
    async fn hosts_under_the_hold(
        &self,
        set: &meerkat_mob_mcp::MobMcpState,
        mob_id: &MobId,
    ) -> std::io::Result<bool> {
        Ok(self.stands_under_the_hold(set, mob_id).await? != UnderTheId::Nothing)
    }

    /// What stands under `mob_id` now, read as
    /// [`Self::hosts_under_the_hold`] reads it and told apart: a mob the
    /// set hosts, an entry the runtime still holds while the set hosts no
    /// mob under the id, or nothing.
    async fn stands_under_the_hold(
        &self,
        set: &meerkat_mob_mcp::MobMcpState,
        mob_id: &MobId,
    ) -> std::io::Result<UnderTheId> {
        let inventory = set
            .mob_inventory()
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let hosted = inventory.managed.iter().any(|(id, _)| id == mob_id)
            || inventory.held.iter().any(|(id, _)| id == mob_id)
            || inventory.dormant.contains(mob_id)
            || inventory
                .quarantined
                .iter()
                .any(|quarantined| &quarantined.mob_id == mob_id);
        Ok(if hosted {
            UnderTheId::AMobOfTheSet
        } else if locked(&self.entries).contains_key(mob_id) {
            UnderTheId::AnEntryAlone
        } else {
            UnderTheId::Nothing
        })
    }

    /// Release the agent memory the launch keeps for the mob `mob_id` after
    /// the mob set destroyed it
    /// ([`MobResolver::release_memory_of_a_destroyed_mob`](super::mob_entry::MobResolver::release_memory_of_a_destroyed_mob)).
    ///
    /// As the topology state's release: one pass first, so the entry of a
    /// mob the set no longer manages goes, and with it what held that
    /// mob's memory open; then, under the one hold of the table's passes,
    /// the check that the set hosts no mob under the id, and the release. A
    /// mob the set hosts under that id (the destroy did not happen, a new
    /// mob took the id, the mob is only not loaded) keeps its memory:
    /// nothing is released, and the call says so as an answer of its own
    /// ([`DestroyedMobMemory::KeptForTheMobUnderTheId`]), which is neither
    /// "done" nor a failure. It loads no mob and opens no memory.
    ///
    /// It answers "done" only where it knows that nothing is left: the
    /// launch declared no memory for the mobs it hosts (none can be stored
    /// for one), or the bound release ran, or found nothing. What it cannot
    /// read is an error, never "done": a table no runtime is bound to (what
    /// the launch declared cannot be read), a launch that declared the
    /// memory and has not bound its release, and a launch with no mob set
    /// that hosts mobs for it (whether a mob stands under the id cannot be
    /// read). So is an entry the runtime still holds under an id the set
    /// hosts no mob for: no mob stands there, the entry holds the memory it
    /// served, and nothing was released.
    ///
    /// Whether a destroyed mob's memory goes with the mob is said in one
    /// place, [`MEMORY_OF_A_DESTROYED_MOB`], and read here alone.
    pub(super) async fn release_memory(
        &self,
        mob_id: &MobId,
    ) -> std::io::Result<DestroyedMobMemory> {
        match MEMORY_OF_A_DESTROYED_MOB {
            MemoryOfADestroyedMob::Removed => {}
            MemoryOfADestroyedMob::Kept => return Ok(DestroyedMobMemory::Done),
        }
        let Some(host) = self.host.get() else {
            return Err(std::io::Error::other(
                "the runtime that serves the launch's mobs is not bound to its table of \
                 entries: whether the launch keeps memory for the mob cannot be read, and \
                 nothing was released",
            ));
        };
        let memory = match host.launch_mob.mob_memory_release().standing() {
            MobMemoryStanding::NotDeclared => return Ok(DestroyedMobMemory::Done),
            MobMemoryStanding::Bound(memory) => memory,
            MobMemoryStanding::Owed => {
                return Err(std::io::Error::other(
                    "the memory the launch keeps for the mobs it hosts is not handed to the \
                     runtime yet: nothing of it can be released",
                ));
            }
        };
        let Some(set) = self.set() else {
            return Err(std::io::Error::other(
                "the launch has no mob set that hosts mobs for it: whether a mob stands \
                 under the id cannot be read, and nothing was released",
            ));
        };
        let _one_at_a_time = self.pass.lock().await;
        self.pass_under_the_hold().await;
        match self.stands_under_the_hold(&set, mob_id).await? {
            UnderTheId::Nothing => {}
            UnderTheId::AMobOfTheSet => {
                return Ok(DestroyedMobMemory::KeptForTheMobUnderTheId);
            }
            UnderTheId::AnEntryAlone => {
                return Err(std::io::Error::other(format!(
                    "the runtime still holds an entry under the id of the mob {mob_id}, which \
                     the mob set hosts no more: nothing of its memory was released"
                )));
            }
        }
        memory.release_mob(mob_id).map_err(std::io::Error::other)?;
        Ok(DestroyedMobMemory::Done)
    }

    /// The launch says how it composes the identity context of a mob of
    /// this table. The first composer stands. The entries the table holds
    /// already (a stored mob the set restored before the launch composed
    /// its own identity plane) are composed from here, without a request.
    pub(super) fn compose_identities(&self, composer: Arc<dyn HostMobIdentityComposer>) {
        if self.identity_composer.set(composer).is_err() {
            return;
        }
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let this = Weak::clone(&self.this);
        executor.spawn(async move {
            if let Some(table) = this.upgrade() {
                table.compose_held().await;
            }
        });
    }

    /// The launch says its identity policy for the mobs of this table,
    /// once. The first policy stands. From here on each entry's identity
    /// plane is composed with the family of its own mob's agents under it,
    /// and the entries composed before (with their reads alone) are given
    /// it from here, without a request. The policy is set before the
    /// entries are walked, and a composition reads it under its entry's
    /// composition lock: an entry composed while the walk runs gets the
    /// policy from one of the two.
    pub(super) fn say_identity_policy(&self, policy: LaunchIdentityPolicy) {
        if self.identity_policy.set(policy).is_err() {
            return;
        }
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let this = Weak::clone(&self.this);
        executor.spawn(async move {
            if let Some(table) = this.upgrade() {
                for entry in table.held() {
                    entry
                        .attach_identity_family(table.identity_policy.get())
                        .await;
                }
            }
        });
    }

    /// Give `entry`'s mob its identity plane when the launch composes
    /// identities and the mob has none: see
    /// [`MobEntry::compose_identity_plane`]. It waits for a composition of
    /// that entry under way.
    ///
    /// An entry whose plane is composed, with no composition under way, is
    /// answered at once. Otherwise the composition's future, which holds
    /// the roster pass, is built in a frame of its own and boxed: a resolve
    /// carries a pointer to it and none of its size.
    async fn compose(&self, entry: &MobEntry) {
        if let (Some(host), Some(composer)) = (self.host.get(), self.identity_composer.get())
            && !self.closed.load(Ordering::SeqCst)
            && !entry.identity_plane_is_composed()
        {
            meerkat_runtime::stack_relief::box_in_own_frame(|| {
                entry.compose_identity_plane(host, composer.as_ref(), &self.identity_policy)
            })
            .await;
        }
        // A policy the launch said after this entry was composed reaches
        // the entry here, before the request that resolved its mob goes on:
        // the walk that follows the saying may not have come to it yet.
        if let Some(policy) = self.identity_policy.get()
            && entry
                .identity_runtime_ref()
                .is_some_and(|identities| !identities.identity_changes_enabled())
        {
            entry.attach_identity_family(Some(policy)).await;
        }
    }

    /// Whether the launch said how it composes the identity planes of the
    /// mobs of this table, and the runtime is not shutting down: an entry
    /// without an identity context is then owed one.
    pub(super) fn composes_identities(&self) -> bool {
        self.identity_composer.get().is_some() && !self.closed.load(Ordering::SeqCst)
    }

    /// [`Self::compose`] for every entry the table holds.
    async fn compose_held(&self) {
        for entry in self.held() {
            self.compose(&entry).await;
        }
    }

    /// The runtime shuts down: the identity runtime of each mob of this
    /// table closes to requests and ends the work it owns for them, before
    /// any mob's actor is taken down, as the default mob's does.
    pub(super) async fn close_identity_requests(&self) {
        for entry in self.held() {
            entry.close_identity_requests().await;
        }
    }

    /// The runtime shuts down: the lifecycle projection of each mob of this
    /// table stops, by `deadline` at the latest, in the window the default
    /// mob's stops in.
    pub(super) async fn stop_identity_lifecycle_projections(&self, deadline: tokio::time::Instant) {
        for entry in self.held() {
            entry.stop_identity_lifecycle_projection(deadline).await;
        }
    }

    /// Give the table the launch's shared services, once the launch's own
    /// mob is built. A mob the set built for the host before (a stored mob
    /// its first verb restored) gets its entry from the follower started
    /// here, and its console events with the host are named here, as a
    /// later build's are when its wiring is asked.
    pub(super) fn bind_host(&self, host: HostOfEntries) {
        if self.host.set(host).is_err() {
            return;
        }
        let building: Vec<MobId> = locked(&self.building).keys().cloned().collect();
        if let Some(host) = self.host.get() {
            for mob_id in &building {
                let _named = host.console_events.of_mob(Some(mob_id));
            }
        }
        if !building.is_empty() {
            self.follow_the_set();
        }
    }

    /// The runtime keeps its gating state across restarts from now on,
    /// through `writer`: the release before a create writes that state
    /// after it withdrew an earlier mob's approvals. The first writer
    /// stands.
    pub(super) fn keep_gating_state(
        &self,
        writer: Arc<crate::gating_persistence::GatingStatePersistence>,
    ) {
        if let Some(host) = self.host.get() {
            let _ = host.gating_state_writer.set(writer);
        }
    }

    /// The launch's mob set, when the launch declared its default mob to
    /// it: only then are the mobs the set builds for the host this host's
    /// mobs.
    fn set(&self) -> Option<Arc<meerkat_mob_mcp::MobMcpState>> {
        let host = self.host.get()?;
        let set = host.launch_mob.agent_mob_mcp_state()?;
        (set.default_mob_id() == Some(host.launch_mob.mob_id())).then_some(set)
    }

    /// Whether the launch declared its default mob to its mob set
    /// (`MobEntries::set`): only then does this host have mobs of its own
    /// beside its launch mob. `None` while this table has no host yet.
    pub(super) fn launch_declared_its_mob_to_a_mob_set(&self) -> Option<bool> {
        let host = self.host.get()?;
        Some(
            host.launch_mob
                .agent_mob_mcp_state()
                .is_some_and(|set| set.default_mob_id() == Some(host.launch_mob.mob_id())),
        )
    }

    /// Start the task that follows the set's changes, once.
    fn follow_the_set(&self) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let Some(set) = self.set() else {
            return;
        };
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut follower = locked(&self.follower);
        if follower.is_some() {
            return;
        }
        *follower = Some(executor.spawn(follow(Weak::clone(&self.this), set.mob_set_changes())));
    }

    /// One pass over the set: an entry whose mob the set no longer manages,
    /// or whose actor closed, goes; a mob the set built for the host and
    /// manages now gets its entry. It loads no mob.
    ///
    /// The pass runs on a task of its own and its caller waits for it. A
    /// caller that is dropped while it waits (a request that was cancelled)
    /// leaves the pass to run to its end: a pass that took a mob's wiring
    /// always puts that mob's entry in, where a pass dropped between the
    /// two would have lost the wiring, and the mob its entry, until the
    /// set built the mob again. A panic of the pass reaches the caller as
    /// before.
    ///
    /// The task is the table's: its handle is kept in `passes` until it
    /// ended, and the table's close joins it ([`Self::close`]). Whether a
    /// pass gets a task is decided under the lock of `passes`
    /// ([`Self::start_a_pass`]), where the close takes the handles after
    /// it said that the table is closing: a pass asked for from then on
    /// runs in its caller, and builds no entry.
    async fn pass(&self) {
        let Some(pass_done) = self.start_a_pass() else {
            let _one_at_a_time = self.pass.lock().await;
            self.pass_under_the_hold().await;
            return;
        };
        if let Ok(Err(panic)) = pass_done.await {
            std::panic::resume_unwind(panic);
        }
    }

    /// Start one pass on a task of the table's own, in one step under the
    /// lock of `passes`: the table is not closing, it is alive and a
    /// runtime runs, and the task's handle is among the table's passes
    /// before this returns, so the close joins it. The answer is where the
    /// pass's outcome arrives, for a caller that waits for it; `None` when
    /// no task was started. It waits for nothing itself.
    ///
    /// A pass that panics while nobody waits for its outcome says so in
    /// the host's log: a caller that waits is handed the panic instead.
    fn start_a_pass(&self) -> Option<tokio::sync::oneshot::Receiver<std::thread::Result<()>>> {
        use futures::FutureExt as _;

        let (done, pass_done) = tokio::sync::oneshot::channel();
        let mut passes = locked(&self.passes);
        passes.retain(|pass| !pass.is_finished());
        match (
            self.closed.load(Ordering::SeqCst),
            self.this.upgrade(),
            tokio::runtime::Handle::try_current(),
        ) {
            (false, Some(table), Ok(executor)) => {
                passes.push(executor.spawn(async move {
                    let outcome = std::panic::AssertUnwindSafe(async {
                        let _one_at_a_time = table.pass.lock().await;
                        table.pass_under_the_hold().await;
                    })
                    .catch_unwind()
                    .await;
                    if let Err(Err(_panic)) = done.send(outcome) {
                        tracing::error!(
                            "a pass of the table of a host's other mobs panicked, and nobody \
                             waited for it"
                        );
                    }
                }));
                Some(pass_done)
            }
            _ => None,
        }
    }

    /// The body of [`Self::pass`], for a caller that holds the lock of the
    /// passes (`self.pass`) for longer than the pass itself.
    async fn pass_under_the_hold(&self) {
        let (Some(host), Some(set)) = (self.host.get(), self.set()) else {
            return;
        };
        // The listing names a mob until its actor closed; the handles are
        // withheld for a mob whose idle unload is asked and not answered.
        let (Ok(listed), Ok(handles)) = (set.mob_list().await, set.mob_handles_snapshot().await)
        else {
            return;
        };
        let managed: BTreeSet<MobId> = listed.into_iter().map(|(mob_id, _)| mob_id).collect();
        let gone: Vec<Arc<MobEntry>> = {
            let mut entries = locked(&self.entries);
            let ids: Vec<MobId> = entries
                .iter()
                .filter(|(mob_id, entry)| !managed.contains(*mob_id) || entry.actor_closed())
                .map(|(mob_id, _)| mob_id.clone())
                .collect();
            ids.iter().filter_map(|id| entries.remove(id)).collect()
        };
        let some_went = !gone.is_empty();
        for entry in gone {
            self.end(host, &entry).await;
        }
        // A mob that went may have been destroyed: the host's driver of
        // the held changes of agents makes a pass, which ends what is
        // still held for a mob the host no longer has. An unload ends
        // nothing: the set still lists the mob.
        if some_went && let Some(policy) = self.identity_policy.get() {
            policy.decisions.notify_work();
        }
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        // Until the launch has told its topology controller, no entry is
        // built: a mob's controller is made from the launch's.
        let Some(launch_topology) = self.launch_topology.get() else {
            return;
        };
        for (mob_id, handle) in handles {
            // The runtime may have begun to shut down since this pass
            // started: it then takes no further mob's wiring. The entry it
            // was making is in, and the close ends it.
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            if locked(&self.entries).contains_key(&mob_id) {
                continue;
            }
            // Only a mob the set built for the host has a wiring here. The
            // mob is entered among the entries being made before its wiring
            // leaves the record, under the record's own lock, so whoever
            // reads the record and then that list finds it in one of them.
            let (wiring, being_made) = {
                let mut building = locked(&self.building);
                let Some(wiring) = building.remove(&mob_id) else {
                    continue;
                };
                locked(&self.making).insert(mob_id.clone());
                (
                    wiring,
                    EntryBeingMade {
                        making: &self.making,
                        mob_id: mob_id.clone(),
                    },
                )
            };
            #[cfg(test)]
            self.standstill_in_tests.stand().await;
            // The mob's own controller: the host's policy, bound to this
            // mob, with its state beside the mob's store when the host
            // keeps topology state and the mob has a store file.
            let state_path = launch_topology
                .keeps_state()
                .then(|| super::mob_topology::state_path_of(&set, &handle))
                .flatten();
            let topology_controller = launch_topology
                .controller_of(mob_id.as_str(), state_path)
                .await;
            let entry = Arc::new(entry_of_hosted_mob(
                host,
                &set,
                &handle,
                wiring,
                topology_controller,
            ));
            if handle.status_observation_snapshot() == meerkat_mob::MobState::Running {
                // The mob runs: its links to members of other mobs of this
                // process are read from its rows, once, into its own part.
                entry.restore_cross_mob_wake_routes().await;
            }
            // The mob's attention bindings onto a member of it private to a
            // person are held paused, before the entry answers a request.
            entry.hold_bindings_onto_private_members(host).await;
            // The mob's console events are frames of the host's console log
            // from here on, when the runtime built its console projection.
            host.console_projection
                .register(&host.console_events, &entry);
            locked(&self.entries).insert(mob_id, entry);
            #[cfg(test)]
            self.standstill_in_tests
                .put_in
                .fetch_add(1, Ordering::SeqCst);
            // The entry stands: the mob leaves the entries being made.
            drop(being_made);
        }
    }

    /// The entry `entry` goes: the registration of its mob's console events
    /// with the runtime's console projection first (what the mob's store
    /// still holds is stored, then the registration ends), then its own
    /// close (its part of the link table, its tasks, its identity runtime's
    /// place on the plane), then the end of its mob's console events. The
    /// mob set said the mob is no longer managed, a mob is about to be
    /// created under the id, or the runtime shuts down; nothing here
    /// decides that.
    ///
    /// A mob the set is building again while its earlier entry ends (a
    /// load that follows an unload) keeps console events of its own with
    /// the host: they begin anew in the step in which the earlier ones go,
    /// so the launch mob's delegate scan never finds the mob without them.
    async fn end(&self, host: &HostOfEntries, entry: &MobEntry) {
        // Awaited in a box of its own: the last projection of the mob's
        // console events is a large future, and every function that ends,
        // closes or resolves an entry would otherwise hold it in its own.
        Box::pin(host.console_projection.end(&host.console_events, entry)).await;
        entry.close().await;
        if locked(&self.building).contains_key(entry.mob_id()) {
            host.console_events.renew_of_mob(entry.mob_id());
        } else {
            host.console_events.end_of_mob(entry.mob_id());
        }
    }

    /// The runtime holds another console log store from here on.
    pub(super) fn console_log_store_set(
        &self,
        store: Arc<dyn crate::console_aggregator::ConsoleLogStore>,
    ) {
        if let Some(host) = self.host.get() {
            host.console_log.set(store);
        }
    }

    /// How long a create under an id waits for the writes an earlier mob of
    /// that id still has in flight into the host's console log. At the end
    /// of a registration each of its writers has at most one store call in
    /// flight and starts no other, so a host whose console log answers does
    /// not reach it.
    const CONSOLE_WRITERS_REST_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

    /// Wait until nothing writes into the host's console log for a mob
    /// under `mob_id` any more, before a mob is created under that id:
    /// every registration of console events made under the id ended, and
    /// none of their writes is in flight
    /// ([`super::mob_console_log::ConsoleProjectionSlot::rest_of`]). The
    /// frames under the id are removed only after this
    /// ([`Self::release_console_records`]), so no write of an earlier mob
    /// is stored after the removal, where it would be the new mob's frame.
    ///
    /// REFUSES the create where a registration under the id has not ended
    /// (no wait ends it), or a write is still in flight at the bound
    /// ([`Self::CONSOLE_WRITERS_REST_BOUND`]): a console log call that does
    /// not return. Nothing under the id was released by then; a later
    /// create asks again.
    async fn console_writers_at_rest(host: &HostOfEntries, mob_id: &MobId) -> Result<(), MobError> {
        if host
            .console_projection
            .rest_of(mob_id, Self::CONSOLE_WRITERS_REST_BOUND)
            .await
        {
            return Ok(());
        }
        tracing::warn!(
            mob_id = %mob_id,
            "an earlier mob of this id still writes into the host's console log; the create is \
             refused"
        );
        Err(MobError::WiringError(format!(
            "the mob '{mob_id}' cannot be created: an earlier mob of that id still writes into \
             the host's console log"
        )))
    }

    /// Remove the console frames the host's console log keeps under
    /// `mob_id`, before a mob is created under that id: they are an earlier
    /// mob's (frames a destroy left, a frame written under the id after
    /// it), and the new mob shows none of them. A console log that cannot
    /// remove them refuses the create: the mob would show another mob's
    /// frames.
    ///
    /// Once they are removed the host's registry of console writers forgets
    /// the registrations that wrote them
    /// ([`super::mob_console_log::ConsoleProjectionSlot::released`]): a mob
    /// of another incarnation may be registered under the id from here on.
    async fn release_console_records(host: &HostOfEntries, mob_id: &MobId) -> Result<(), MobError> {
        match host.console_log.remove_frames_of_mob(mob_id).await {
            Ok(frames) => {
                host.console_projection.released(mob_id);
                if frames > 0 {
                    tracing::info!(
                        mob_id = %mob_id,
                        frames,
                        "removed the console frames an earlier mob of this id left in the \
                         host's console log, before the mob is created"
                    );
                }
                Ok(())
            }
            Err(error) => {
                // The store's own words stay with the host: they can name a
                // file of the store.
                tracing::warn!(
                    mob_id = %mob_id,
                    error = %error,
                    "the console frames an earlier mob of this id left in the host's console \
                     log could not be removed; the create is refused"
                );
                Err(MobError::WiringError(format!(
                    "the mob '{mob_id}' cannot be created: the console frames an earlier mob \
                     of that id left in the host's console log could not be removed"
                )))
            }
        }
    }

    /// The entry of the mob `named`, when the set manages that mob now.
    async fn entry_after_a_pass(&self, named: &MobId) -> Option<Arc<MobEntry>> {
        self.pass().await;
        locked(&self.entries).get(named).cloned()
    }

    /// Whether the set knows `named` as a mob the host created that it does
    /// not serve now: it is not loaded, its store did not load and is
    /// quarantined, or its idle unload is admitted and not answered yet
    /// (the set answers that one from its record, and a lookup of the mob
    /// then waits for the unload's answer). Answered from what the store's
    /// record keeps of who created the mob; for a mob that is not loaded
    /// and whose store this process has not read yet (after a restart,
    /// before the set's sweep reached it), the set reads that one store's
    /// creation record. No mob is loaded and no actor is built for the
    /// question.
    async fn is_a_mob_of_the_host_without_an_actor(
        set: &meerkat_mob_mcp::MobMcpState,
        named: &MobId,
    ) -> bool {
        matches!(
            set.unloaded_mob_creation(named).await,
            Ok(Some(MobProvenance::Host))
        )
    }

    /// Whether a request resolves a mob of this table at all: on every
    /// launch, until the runtime begins to shut down. Nothing switches it:
    /// a mob the launch's mob set built for the host is a mob its requests
    /// resolve.
    pub(super) fn answers_requests(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    /// The entry of the mob `named`, a mob the set built for the host:
    /// the one the set manages now, or the one built after this lookup
    /// loaded the mob. `Ok(None)` for an id that is no mob of the host: a
    /// mob a member created, an id no mob has. An error when the host has
    /// the mob and the set could not serve it: the set's own error (a
    /// store that did not load is quarantined, and the error says so).
    ///
    /// This is [`Self::resolve`], before the identity plane of the entry's
    /// mob is asked for.
    async fn entry_of_a_named_mob(&self, named: &MobId) -> Result<Option<Arc<MobEntry>>, MobError> {
        if !self.answers_requests() {
            return Ok(None);
        }
        let Some(set) = self.set() else {
            return Ok(None);
        };
        if let Some(entry) = self.entry_after_a_pass(named).await {
            return Ok(Some(entry));
        }
        if !Self::is_a_mob_of_the_host_without_an_actor(&set, named).await {
            return Ok(None);
        }
        // The one lookup that loads the mob: the set asks the launch's
        // composition for its builder, which asks this table for the mob's
        // member wiring. Requests that arrive while it runs wait on that
        // one load in the set.
        match set.handle_for(named).await {
            Ok(_) => Ok(self.entry_after_a_pass(named).await),
            // Destroyed since the set was asked: the host has no such mob.
            Err(MobError::MobNotFound(_)) => Ok(None),
            Err(error) => {
                tracing::warn!(mob_id = %named, %error, "a mob of the host did not load for a request");
                Err(error)
            }
        }
    }

    /// The entry of the mob a request names ([`Self::entry_of_a_named_mob`]).
    ///
    /// An entry is answered once its mob's identity plane was asked for,
    /// when the launch composes identities ([`Self::compose`]). A mob of
    /// the host that did not load is answered the set's error before any
    /// plane is asked for.
    pub(super) async fn resolve(&self, named: &MobId) -> Result<Option<Arc<MobEntry>>, MobError> {
        let entry = self.entry_of_a_named_mob(named).await?;
        if let Some(entry) = &entry {
            self.compose(entry).await;
        }
        Ok(entry)
    }

    /// Whether [`Self::resolve`] would answer the mob `named`, asked
    /// without loading the mob and without building its entry. What could
    /// not be read is answered no: a request is refused then.
    /// [`Self::serves_read`] holds the reads and their order.
    pub(super) async fn serves(&self, named: &MobId) -> bool {
        self.serves_read(named).await.unwrap_or(false)
    }

    /// [`Self::serves`] for a caller that must tell "no" from "not known".
    ///
    /// It reads what [`Self::entry_of_a_named_mob`] reads, in its order:
    /// first what a pass leaves (the entry the table holds, the one a pass
    /// is making now, or the one a pass would make now: a member wiring
    /// kept for the mob together with a handle the set hands out for it),
    /// then the record of who created the mob, which a request's lookup
    /// loads from. A wiring kept for a mob the set no longer manages
    /// decides nothing: such a mob is asked of its creation record, as a
    /// request's lookup asks it.
    ///
    /// `None` when a read failed and what could be read does not say yes:
    /// the set's handles could not be read for a mob that waits for its
    /// entry (a pass might make it, or is making it), or the mob's
    /// creation record could not be read.
    pub(super) async fn serves_read(&self, named: &MobId) -> Option<bool> {
        if !self.answers_requests() {
            return Some(false);
        }
        let Some(set) = self.set() else {
            return Some(false);
        };
        // Read in the order a pass moves a mob: the record of builds, the
        // entries being made, the entries. A pass sets each before it
        // clears the one before, so a mob the host serves is found in one
        // of them, also while a pass is making its entry.
        let waits = self.waits_for_its_entry(named);
        if self.holds_the_entry_of(named) {
            self.hosted_another_mob.store(true, Ordering::SeqCst);
            return Some(true);
        }
        // Built for the host and managed now: a pass makes its entry, is
        // making it, or has made it. `None`: the set's handles could not be
        // read.
        //
        // Nothing is held between the looks, so the mob is looked for on
        // both sides of the handles: a create that ran to its end in
        // between dropped what an earlier mob left under the id before its
        // own mob got a handle, and that mob has a wiring only when the
        // host created it; a pass that ran to its end in between left the
        // entry.
        let a_pass_makes_its_entry = if waits && self.launch_topology.get().is_some() {
            set.mob_handles_snapshot().await.ok().map(|handles| {
                handles.iter().any(|(mob_id, _)| mob_id == named)
                    && (self.waits_for_its_entry(named) || self.holds_the_entry_of(named))
            })
        } else {
            Some(false)
        };
        let served = if a_pass_makes_its_entry == Some(true) {
            Some(true)
        } else {
            let by_its_record = set
                .unloaded_mob_creation(named)
                .await
                .ok()
                .map(|creation| matches!(creation, Some(MobProvenance::Host)));
            match (a_pass_makes_its_entry, by_its_record) {
                (_, Some(true)) => Some(true),
                (Some(_), by_its_record) => by_its_record,
                (None, _) => None,
            }
        };
        if served == Some(true) {
            self.hosted_another_mob.store(true, Ordering::SeqCst);
        }
        served
    }

    /// Whether this table ever held a mob of the host beside the launch
    /// mob. It stays true once it is, also on a table that is closing.
    pub(super) fn hosted_another_mob(&self) -> bool {
        self.hosted_another_mob.load(Ordering::SeqCst)
    }

    /// Whether [`Self::listing`] names a mob: yes, no, or `None` for "not
    /// known". It is not known when the mob set's inventory could not be
    /// read, and while the listing names no mob but the set knows a mob it
    /// has not loaded whose creation it has not read, a store it keeps
    /// quarantined, or a mob it manages, with no entry or wiring in the
    /// table, that its records do not tell as another's: one of those may
    /// be a mob of the host. A caller that must not take "not known" for
    /// "none" asks here.
    pub(super) async fn lists_another_mob(&self) -> Option<bool> {
        let (listed, whole) = self.listing_read().await?;
        if !listed.is_empty() {
            Some(true)
        } else if whole {
            Some(false)
        } else {
            None
        }
    }

    /// Whether the mob `named` waits for its entry: its wiring is in the
    /// record of builds, or a pass is making its entry now. Read in that
    /// order, which is the order a pass moves a mob in.
    fn waits_for_its_entry(&self, named: &MobId) -> bool {
        if locked(&self.building).contains_key(named) {
            return true;
        }
        locked(&self.making).contains(named)
    }

    /// Whether the table holds an entry of the mob `named` whose actor has
    /// not closed.
    fn holds_the_entry_of(&self, named: &MobId) -> bool {
        locked(&self.entries)
            .get(named)
            .is_some_and(|entry| !entry.actor_closed())
    }

    /// The mob of the host in whose reach `named` is, asked of the mob
    /// set's own records whatever requests resolve
    /// ([`Self::answers_requests`] is not asked): `named` itself when the
    /// host created it, or the mob the host created whose member created
    /// it, through any number of mobs members created. `Ok(None)` for a mob
    /// in no such reach: the launch mob, a mob a member of the launch mob
    /// created (at any depth), a mob a session that is no member's created
    /// or one created before creations were recorded, and every mob of a
    /// runtime whose launch declared no mob to a mob set.
    ///
    /// It loads no mob. A mob the set lists as not loaded is judged by the
    /// record the set keeps of its creation; a mob the set runs is asked
    /// through the handle the set holds for it now
    /// (`MobMcpState::mob_handles_snapshot`, which loads nothing). `Err`
    /// when the records do not decide it: the set's inventory, its handles
    /// or a creation record cannot be read; the set has not read the record
    /// of a mob that is not loaded; the mob is neither listed as not loaded
    /// nor run by the set now (a mob it holds or keeps quarantined, a mob
    /// between a load and an unload, an id no mob has); or the creation
    /// itself does not decide it ([`reach_of_a_creation`]).
    pub(super) async fn hosting(&self, named: &MobId) -> Result<Option<MobId>, String> {
        let (Some(host), Some(set)) = (self.host.get(), self.set()) else {
            return Ok(None);
        };
        let launch_mob = host.launch_mob.mob_id();
        let inventory = set
            .mob_inventory()
            .await
            .map_err(|error| format!("the mob set's records cannot be read: {error}"))?;
        let running = set
            .mob_handles_snapshot()
            .await
            .map_err(|error| format!("the mob set's handles cannot be read: {error}"))?;
        Self::hosting_by(launch_mob, &inventory, &running, &BTreeMap::new(), named).await
    }

    /// [`Self::hosting`] of every mob the mob set's records list beside the
    /// launch mob (the mobs it runs, holds, keeps quarantined and has not
    /// loaded), in id order, each asked of one read of the set's inventory
    /// and of its handles. It loads no mob. `Err` when that inventory or
    /// those handles cannot be read; none for a runtime whose launch
    /// declared no mob to a mob set.
    ///
    /// One read is made here that [`Self::hosting`] does not make, so that
    /// a mob a member of the launch mob made is known to be in the launch
    /// mob's reach whatever state the set keeps it in. A mob the set does
    /// not run and whose creation its record does not keep (one that is
    /// not loaded and that the set has not read yet, one it holds before
    /// activation or keeps quarantined, one between a load and an unload)
    /// is asked of its store's own creation record
    /// ([`creation_a_store_records`]): the store's file is opened
    /// read-only, and nothing of it is created, brought forward or
    /// repaired. The set's own open of a store is not used here, and the
    /// set's record is not written. A store that read does not answer for
    /// stays undecided.
    pub(super) async fn hosting_of_every_mob(
        &self,
    ) -> Result<Vec<(MobId, Result<Option<MobId>, String>)>, String> {
        let (Some(host), Some(set)) = (self.host.get(), self.set()) else {
            return Ok(Vec::new());
        };
        let launch_mob = host.launch_mob.mob_id();
        let inventory = set
            .mob_inventory()
            .await
            .map_err(|error| format!("the mob set's records cannot be read: {error}"))?;
        let running = set
            .mob_handles_snapshot()
            .await
            .map_err(|error| format!("the mob set's handles cannot be read: {error}"))?;
        let mut mobs: BTreeSet<&MobId> = BTreeSet::new();
        mobs.extend(inventory.managed.iter().map(|(mob, _)| mob));
        mobs.extend(inventory.held.iter().map(|(mob, _)| mob));
        mobs.extend(inventory.quarantined.iter().map(|row| &row.mob_id));
        mobs.extend(inventory.dormant.iter());
        let mut stored: BTreeMap<MobId, MobProvenance> = BTreeMap::new();
        for mob in &mobs {
            // A mob the set runs answers through its handle, and a mob
            // whose creation the set's record keeps answers from it.
            if inventory.dormant_creation.contains_key(*mob)
                || running.iter().any(|(run, _)| run == *mob)
            {
                continue;
            }
            let Some(store) = set.persistent_mob_store_path(mob) else {
                continue;
            };
            // An answer the read does not give leaves the mob's reach
            // undecided below.
            if let Some(created_by) = creation_a_store_records(&store, mob).await {
                stored.insert((*mob).clone(), created_by);
            }
        }
        let mut hosting = Vec::with_capacity(mobs.len());
        for mob in mobs {
            if mob == launch_mob {
                continue;
            }
            hosting.push((
                mob.clone(),
                Self::hosting_by(launch_mob, &inventory, &running, &stored, mob).await,
            ));
        }
        Ok(hosting)
    }

    /// [`Self::hosting`] of the mob `named`, asked of one read of the mob
    /// set's inventory and of its handles. `stored` holds what the stores
    /// of mobs the set does not run and has no creation record of say of
    /// their creation, for a caller that read them
    /// ([`Self::hosting_of_every_mob`]); a mob the set's records do not
    /// answer for and that is not in `stored` is not decided.
    async fn hosting_by(
        launch_mob: &MobId,
        inventory: &meerkat_mob_mcp::MobInventory,
        running: &[(MobId, meerkat_mob::MobHandle)],
        stored: &BTreeMap<MobId, MobProvenance>,
        named: &MobId,
    ) -> Result<Option<MobId>, String> {
        let mut mob = named.clone();
        for _ in 0..MOB_CREATION_LINEAGE_BOUND {
            if &mob == launch_mob {
                return Ok(None);
            }
            let created_by = if inventory.dormant.contains(&mob) {
                inventory
                    .dormant_creation
                    .get(&mob)
                    .or_else(|| stored.get(&mob))
                    .cloned()
                    .ok_or_else(|| format!("the creation record of mob '{mob}' is not read"))?
            } else if let Some((_, handle)) = running.iter().find(|(run, _)| run == &mob) {
                handle.provenance().await.map_err(|error| {
                    format!("the creation record of mob '{mob}' cannot be read: {error}")
                })?
            } else if let Some(created_by) = stored.get(&mob) {
                created_by.clone()
            } else {
                return Err(format!(
                    "the mob set neither lists mob '{mob}' as not loaded nor runs it now"
                ));
            };
            match reach_of_a_creation(&mob, created_by)? {
                ReachOfACreation::Hosted => return Ok(Some(mob)),
                ReachOfACreation::CreatedIn(creating_mob) => mob = creating_mob,
                ReachOfACreation::None => return Ok(None),
            }
        }
        Err(format!(
            "the creation of mob '{named}' is not traced to a mob of the host in \
             {MOB_CREATION_LINEAGE_BOUND} steps"
        ))
    }

    /// Whether the mob set recorded that it destroyed the mob of the id
    /// `named` and hosts no mob under the id since
    /// (`MobMcpState::mob_destroyed_here`): the set's one record of a
    /// destroy, which is this process's own. An id the set merely does not
    /// list answers `false`: a store this launch does not see, and a mob an
    /// earlier process destroyed, are not destroyed mobs of this one. A
    /// read the set refuses answers `false` too: nothing is concluded from
    /// it.
    async fn set_destroyed_the_mob_of(set: &meerkat_mob_mcp::MobMcpState, named: &MobId) -> bool {
        set.mob_destroyed_here(named).await.unwrap_or(false)
    }

    /// The holder of the held changes of the mob `named`, as the host's
    /// one driver of those decisions asks it: the holder of the mob's own
    /// family when the table holds the mob's entry and its actor runs;
    /// gone when the mob set recorded that it destroyed the mob and hosts
    /// none under the id since; away otherwise (a mob that is not loaded,
    /// one whose identity plane is not composed, one whose launch said no
    /// policy, and a mob the set has no record of at all: it withdraws
    /// nothing of a mob it cannot say it destroyed). What a destroy that
    /// began and did not finish reads as is what the set's record says of
    /// it: the family keeps no fact of a destroy of its own. It is read
    /// from the table as it stands: no mob is loaded, and whether requests
    /// resolve a mob of the table is not asked (a held change is carried
    /// out on every launch that holds one).
    pub(super) async fn held_changes_of(
        &self,
        named: &MobId,
    ) -> crate::identity_creation::HeldFamily {
        use crate::identity_creation::HeldFamily;

        let entry = locked(&self.entries)
            .get(named)
            .filter(|entry| !entry.actor_closed())
            .cloned();
        if let Some(entry) = entry {
            return match entry
                .identity_runtime_ref()
                .and_then(|identities| identities.held_changes())
            {
                Some(holder) => HeldFamily::Here(Box::new(holder)),
                None => HeldFamily::Away,
            };
        }
        match self.set() {
            Some(set) if Self::set_destroyed_the_mob_of(&set, named).await => HeldFamily::Gone,
            _ => HeldFamily::Away,
        }
    }

    /// The identity runtime of the mob `named`, as an exported view, when
    /// the table holds the mob's entry, its actor runs and its identity
    /// plane is composed. Read from the table as it stands: no mob is
    /// loaded, and whether requests resolve a mob of the table is not
    /// asked. The notice of an ended held change reaches its requester
    /// through it.
    pub(super) fn identity_runtime_of(
        &self,
        named: &MobId,
    ) -> Option<crate::identity_first::PublicIdentityRuntime> {
        let entry = locked(&self.entries)
            .get(named)
            .filter(|entry| !entry.actor_closed())
            .cloned()?;
        entry.public_identity_runtime(&entry.mob_runtime)
    }

    /// The entries the table holds now.
    pub(super) fn held(&self) -> Vec<Arc<MobEntry>> {
        locked(&self.entries).values().cloned().collect()
    }

    /// The console events an event of an agent of the mob `mob_id` is
    /// projected into: those of the mob as the table holds it now, and only
    /// for an event that came from that very mob (`incarnation` is the
    /// incarnation of the mob the event's stream was attached to). None
    /// for a mob the table holds no entry of (a mob that is gone, or one
    /// whose entry is not built yet), for an event of an earlier mob of the
    /// id, and for an event that names no incarnation: such an event is
    /// projected into no console events, so a mob created under an id
    /// shows no run of the mob that had the id before it. Read from the
    /// table alone: no pass, no load.
    pub(super) fn console_events_of_event(
        &self,
        mob_id: &MobId,
        incarnation: Option<meerkat_core::service::MobIncarnationId>,
    ) -> Option<super::ConsoleEventStore> {
        let host = self.host.get()?;
        let entry = locked(&self.entries).get(mob_id).cloned()?;
        (incarnation == Some(entry.mob_runtime.handle().incarnation()))
            .then(|| entry.console_log.store(&host.console_events, mob_id))
    }

    /// The entry of the mob `named` when the set manages that mob now and
    /// its entry is built: read from the table alone. It asks the set
    /// nothing, so it loads no mob, waits behind no load and builds no
    /// entry: a mob that is not loaded, or is being loaded, has none.
    pub(super) fn loaded(&self, named: &MobId) -> Option<Arc<MobEntry>> {
        if !self.answers_requests() {
            return None;
        }
        locked(&self.entries)
            .get(named)
            .filter(|entry| !entry.actor_closed())
            .cloned()
    }

    /// The incarnation of the mob `named`, a mob [`Self::serves`] answers,
    /// asked without loading the mob: of a mob the set manages now, the
    /// one its actor runs at; of a mob that is not loaded, the one the
    /// set's records keep for it. `None` when neither says it: a mob whose
    /// load is under way and whose record was not read before it, a mob
    /// that is not loaded whose record the set has not read yet, and
    /// every id [`Self::serves`] refuses. A caller that selects by the
    /// incarnation answers nothing then; it never loads the mob to learn
    /// it. The pass it takes builds the entry of a mob the set manages
    /// and loads none.
    pub(super) async fn incarnation_without_a_load(
        &self,
        named: &MobId,
    ) -> Option<meerkat_core::service::MobIncarnationId> {
        if !self.answers_requests() {
            return None;
        }
        let set = self.set()?;
        if let Some(entry) = self.entry_after_a_pass(named).await
            && !entry.actor_closed()
        {
            return Some(entry.mob_runtime.handle().incarnation());
        }
        let inventory = set.mob_inventory().await.ok()?;
        incarnation_of_a_mob_not_loaded(
            named,
            &inventory.dormant,
            &inventory.dormant_creation,
            &inventory.dormant_incarnation,
        )
    }

    /// When the mob that holds the id `named` now was created, in
    /// milliseconds of the host's clock: the time the mob's journal keeps
    /// for its first event, which is the mob's creation. It is read without
    /// loading the mob (`MobMcpState::mob_events_without_load`): a mob the
    /// set manages answers through its own handle, and a mob that is known
    /// and not loaded is read from its store's file, read-only, so it is
    /// not loaded after the read either. `None` when no mob stands under
    /// the id in the set, when the set keeps the mob's store quarantined,
    /// and when the journal cannot be read or holds no event: a caller
    /// that decides by the time decides then as for an id under which no
    /// mob stands.
    ///
    /// The mob is any mob the launch's agent mob state holds: one the host
    /// created and one a member created alike. That state is asked whether
    /// or not the launch declared its default mob to it (`MobEntries::set`): a
    /// mob a member created stands on a host that serves its launch mob
    /// alone too.
    pub(super) async fn created_at_ms(&self, named: &MobId) -> Option<u64> {
        if !self.answers_requests() {
            return None;
        }
        let page = self
            .host
            .get()?
            .launch_mob
            .agent_mob_mcp_state()?
            .mob_events_without_load(named, 0, 1)
            .await
            .ok()?;
        let created = page.events().first()?;
        u64::try_from(created.timestamp.timestamp_millis()).ok()
    }

    /// Every mob the host serves beside its default mob, loaded or not, as
    /// the mob set's records know it now, in id order. It reads the set's
    /// inventory, which opens no store and loads no mob, and it builds no
    /// entry. A mob a member created is never listed.
    ///
    /// A mob the set manages is the host's when this table holds its entry
    /// or the wiring its entry waits for: the set gives only a mob it
    /// builds for the host to the launch's composition. A mob that is not
    /// loaded is the host's when the set's records keep its creation as the
    /// host's. A mob whose store is quarantined is listed when its last
    /// build was for the host (its wiring still waits here) or the set's
    /// record of the store keeps its creation as the host's; one whose
    /// creation was never read is not listed.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "read by a door that lists the host's mobs; none calls it yet"
        )
    )]
    pub(super) async fn listing(&self) -> Vec<HostedMobListing> {
        self.listing_read()
            .await
            .map(|(listed, _)| listed)
            .unwrap_or_default()
    }

    /// [`Self::listing`], with whether the listing is whole (the set's
    /// records say of every mob it has not loaded who created it, it
    /// keeps no store quarantined, and its records say of every mob it
    /// manages that the table holds no entry or wiring for that another
    /// than the host created it), and `None` when the mob set's
    /// inventory could not be read.
    async fn listing_read(&self) -> Option<(Vec<HostedMobListing>, bool)> {
        if !self.answers_requests() {
            return Some((Vec::new(), true));
        }
        let (Some(host), Some(set)) = (self.host.get(), self.set()) else {
            return Some((Vec::new(), true));
        };
        let Ok(inventory) = set.mob_inventory().await else {
            return None;
        };
        let default_mob = host.launch_mob.mob_id();
        // The mobs that wait for their entry are read before the entries,
        // in the order a pass moves a mob in: one whose entry a pass is
        // making is in one of the two.
        let mut waiting: BTreeSet<MobId> = locked(&self.building).keys().cloned().collect();
        waiting.extend(locked(&self.making).iter().cloned());
        let built: BTreeMap<MobId, meerkat_core::service::MobIncarnationId> = self
            .held()
            .into_iter()
            .filter(|entry| !entry.actor_closed())
            .map(|entry| {
                (
                    entry.mob_id().clone(),
                    entry.mob_runtime.handle().incarnation(),
                )
            })
            .collect();
        let listing = |mob_id: &MobId, phase: HostedMobPhase| HostedMobListing {
            mob_id: mob_id.clone(),
            provenance: MobProvenance::Host,
            phase,
            incarnation: built.get(mob_id).copied(),
        };
        let mut listed: Vec<HostedMobListing> = Vec::new();
        for (mob_id, _) in &inventory.managed {
            if mob_id != default_mob && (built.contains_key(mob_id) || waiting.contains(mob_id)) {
                listed.push(listing(mob_id, HostedMobPhase::Loaded));
            }
        }
        for mob_id in &inventory.dormant {
            if mob_id != default_mob
                && matches!(
                    inventory.dormant_creation.get(mob_id),
                    Some(MobProvenance::Host)
                )
            {
                listed.push(listing(mob_id, HostedMobPhase::NotLoaded));
            }
        }
        for quarantined in &inventory.quarantined {
            if &quarantined.mob_id == default_mob {
                continue;
            }
            // A quarantined store is never opened for the question: the
            // answer is what its record keeps.
            let of_the_host = waiting.contains(&quarantined.mob_id)
                || Self::is_a_mob_of_the_host_without_an_actor(&set, &quarantined.mob_id).await;
            if of_the_host {
                listed.push(listing(
                    &quarantined.mob_id,
                    HostedMobPhase::CannotBeLoaded(quarantined.class),
                ));
            }
        }
        listed.sort_by(|a, b| a.mob_id.cmp(&b.mob_id));
        if !listed.is_empty() {
            self.hosted_another_mob.store(true, Ordering::SeqCst);
        }
        // The listing is whole when the set's records tell of every mob it
        // does not list that another than the host created it
        // ([`another_created`]). A mob that is not loaded is told by the
        // record of its creation: the host's (the listing names it) or
        // another's. A mob the set manages for which the table holds
        // neither an entry whose actor runs nor a waiting wiring is told
        // by the set's record of who created it; one the record tells
        // nothing of (a handle a host inserted, a store with no creation
        // record that could be read) or tells as the host's keeps the
        // listing not whole for as long as the set manages it.
        let whole = inventory.dormant.iter().all(|mob_id| {
            mob_id == default_mob
                || inventory
                    .dormant_creation
                    .get(mob_id)
                    .is_some_and(|creation| {
                        matches!(creation, MobProvenance::Host) || another_created(creation)
                    })
        }) && inventory
            .quarantined
            .iter()
            .all(|quarantined| &quarantined.mob_id == default_mob)
            && inventory.managed.iter().all(|(mob_id, _)| {
                mob_id == default_mob
                    || built.contains_key(mob_id)
                    || waiting.contains(mob_id)
                    || inventory
                        .managed_creation
                        .get(mob_id)
                        .is_some_and(another_created)
            });
        Some((listed, whole))
    }

    /// Whether this host serves a mob beside its launch mob, in three
    /// answers ([`OtherMobsServed`]): what [`Self::lists_another_mob`]
    /// answers of the mob set's records, where "not known" is never read
    /// as "none". A table that began to shut down and a table that has no
    /// host yet cannot tell: both are answered here, before and after the
    /// listing is asked, which answers them as it answers a host with no
    /// other mob. A door that reads a record as the launch mob's because
    /// the host has no other mob asks this.
    ///
    /// Every launch lets its requests resolve the mobs of this table, from
    /// its start. Until the table has its host the answer is "not known",
    /// never "the launch mob alone": a store restored at a start can hold
    /// records of a second mob before the table says which mobs the host
    /// has.
    pub(super) async fn serves_beside_the_launch_mob(&self) -> OtherMobsServed {
        if self.closed.load(Ordering::SeqCst) {
            return OtherMobsServed::NotKnown;
        }
        if self.host.get().is_none() {
            return OtherMobsServed::NotKnown;
        }
        let another_is_listed = self.lists_another_mob().await;
        // A shutdown that began since: the listing was then asked of a
        // table that answers no request, and said "none" for it.
        if self.closed.load(Ordering::SeqCst) {
            return OtherMobsServed::NotKnown;
        }
        match another_is_listed {
            Some(true) => OtherMobsServed::AnotherMob,
            Some(false) => OtherMobsServed::LaunchMobAlone,
            None => OtherMobsServed::NotKnown,
        }
    }

    /// Whether the runtime's shutdown has begun ([`Self::begin_close`]):
    /// the table answers no request from then on, which says nothing about
    /// whose a mob is.
    pub(super) fn is_closing(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// The runtime begins to shut down: a pass asks this before it takes
    /// each mob's wiring and takes none once it is said, so a pass under
    /// way puts in at most the entry it is making (the close ends it) and
    /// begins no other; and every mob's part of the link table goes with
    /// the default mob's.
    pub(super) fn begin_close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(follower) = locked(&self.follower).take() {
            follower.abort();
        }
        for entry in self.held() {
            entry.close_cross_mob_links();
        }
    }

    /// The runtime's shutdown ran to its end: every entry goes.
    pub(super) async fn close(&self) {
        self.begin_close();
        // The passes that run on tasks of their own are joined first. The
        // table said above that it is closing, and a pass gets a task only
        // under the lock taken here, so none starts one after this. A pass
        // that is making an entry puts it in and begins no other, and that
        // entry goes with the rest below.
        let passes = std::mem::take(&mut *locked(&self.passes));
        for pass in passes {
            let _ = pass.await;
        }
        let _one_at_a_time = self.pass.lock().await;
        let entries = std::mem::take(&mut *locked(&self.entries));
        locked(&self.building).clear();
        for entry in entries.into_values() {
            match self.host.get() {
                Some(host) => self.end(host, &entry).await,
                None => entry.close().await,
            }
        }
    }

    /// The runtime built its console projection: the console events of
    /// every entry the table holds are registered with it, and those of
    /// each entry made later are.
    pub(super) fn console_projection_built(
        &self,
        projection: crate::console_aggregator::MetaConsoleAggregator,
        payload_policy: Arc<dyn crate::console_aggregator::ConsoleVisibilityPolicy>,
    ) {
        let Some(host) = self.host.get() else {
            return;
        };
        host.console_projection.set(projection, payload_policy);
        for entry in self.held() {
            host.console_projection
                .register(&host.console_events, &entry);
        }
    }

    /// The runtime's console projection is replaced: the registrations of
    /// the table's entries with it end, and nothing registers with it any
    /// more. The next projection the runtime builds gets them again.
    pub(super) fn console_projection_replaced(&self) {
        let Some(host) = self.host.get() else {
            return;
        };
        for entry in self.held() {
            host.console_projection.unregister(&entry);
        }
        host.console_projection.clear();
    }

    /// Test builds only: the next pass that takes a mob's wiring stands
    /// still before it puts that mob's entry in, until
    /// [`Self::end_the_standstill_in_tests`].
    #[cfg(test)]
    pub(super) fn stand_the_next_pass_still_in_tests(&self) {
        self.standstill_in_tests.armed.store(true, Ordering::SeqCst);
    }

    /// Test builds only: wait until a pass stands still
    /// ([`Self::stand_the_next_pass_still_in_tests`]).
    #[cfg(test)]
    pub(super) async fn a_pass_stands_still_in_tests(&self) {
        self.standstill_in_tests.reached.notified().await;
    }

    /// Test builds only: the pass that stands still goes on.
    #[cfg(test)]
    pub(super) fn end_the_standstill_in_tests(&self) {
        self.standstill_in_tests.ended.notify_one();
    }

    /// Test builds only: how many passes run on tasks of their own now.
    #[cfg(test)]
    pub(super) fn pass_tasks_running_in_tests(&self) -> usize {
        locked(&self.passes)
            .iter()
            .filter(|pass| !pass.is_finished())
            .count()
    }

    /// Test builds only: how many entries the table's passes put in so far.
    #[cfg(test)]
    pub(super) fn entries_put_in_by_passes_in_tests(&self) -> usize {
        self.standstill_in_tests.put_in.load(Ordering::SeqCst)
    }

    /// Test builds only: a destroy asks for no pass from here on. For a row
    /// that needs a destroyed mob's entry to stand at the next create: the
    /// state in which the pass a destroy asked for has not run yet, or the
    /// mob was destroyed where the mob set tells nobody.
    #[cfg(test)]
    pub(super) fn destroys_ask_for_no_pass_in_tests(&self) {
        self.destroys_ask_for_no_pass_in_tests
            .store(true, Ordering::SeqCst);
    }

    /// Test builds only: the task that follows the set stops, after a pass
    /// under way ended, and none starts in its place: from here on a pass
    /// runs only when a request asks for one.
    #[cfg(test)]
    pub(super) async fn hold_the_follower_in_tests(&self) {
        let _one_at_a_time = self.pass.lock().await;
        let mut follower = locked(&self.follower);
        if let Some(task) = follower.take() {
            task.abort();
        }
        *follower = Some(tokio::spawn(std::future::pending()));
    }

    /// Test builds only: the entry the table holds for the mob `named`.
    #[cfg(test)]
    pub(super) fn entry_in_tests(&self, named: &MobId) -> Option<Arc<MobEntry>> {
        locked(&self.entries).get(named).cloned()
    }

    /// Test builds only: a composition of `entry`'s identity plane as a
    /// caller begins it that passed this table's own checks
    /// ([`Self::compose`]) before the table ended the entry: the entry's
    /// own checks are the only ones left.
    #[cfg(test)]
    pub(super) async fn compose_begun_before_the_end_in_tests(&self, entry: &MobEntry) {
        if let (Some(host), Some(composer)) = (self.host.get(), self.identity_composer.get()) {
            entry
                .compose_identity_plane(host, composer.as_ref(), &self.identity_policy)
                .await;
        }
    }

    /// Test builds only: an entry over the mob `named` as the set manages
    /// it now, built as the table builds one and kept nowhere. Its
    /// start-time read has not run: its cell of the link table is empty.
    #[cfg(test)]
    pub(super) async fn entry_before_its_read_in_tests(
        &self,
        named: &MobId,
    ) -> Option<Arc<MobEntry>> {
        let (host, set) = (self.host.get()?, self.set()?);
        let handle = set.handle_for(named).await.ok()?;
        Some(Arc::new(entry_of_hosted_mob(
            host,
            &set,
            &handle,
            MemberBuildWiring::deferred(),
            crate::topology_control::TopologyController::default(),
        )))
    }
}

/// The incarnation the mob set's records keep for `named`, a mob the host
/// created that is not loaded. `dormant`, `creation` and `incarnation` are
/// one read of the set's inventory: the mobs that are not loaded, who
/// created each, and the incarnation of each, as far as the set has read
/// them. `None` for a mob that is not among the mobs that are not loaded,
/// for a mob the host did not create (a mob a member created is served on
/// no request), and for a mob whose record the set has not read: nothing
/// is guessed, and nothing is loaded to learn it.
pub(super) fn incarnation_of_a_mob_not_loaded(
    named: &MobId,
    dormant: &[MobId],
    creation: &BTreeMap<MobId, MobProvenance>,
    incarnation: &BTreeMap<MobId, meerkat_core::service::MobIncarnationId>,
) -> Option<meerkat_core::service::MobIncarnationId> {
    if !dormant.contains(named) || !matches!(creation.get(named), Some(MobProvenance::Host)) {
        return None;
    }
    incarnation.get(named).copied()
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl HostMobEntryWiring for MobEntries {
    /// A mob is about to be created under `mob_id`: the records the host's
    /// metadata store keeps under that id, the console frames the host's
    /// console log keeps under it and the labels the host's memory holds
    /// under it are of an earlier mob of the same id (a destroy whose
    /// release never ran or failed, a write that came after it), and they
    /// go here, before anything of the mob exists. A store that cannot
    /// release them refuses the create: the new mob would read another
    /// mob's records.
    ///
    /// An entry of the earlier mob may still stand in the table: the
    /// follower ends the entry of a destroyed mob at its next pass, not at
    /// the destroy. It ends here first, when its actor closed, as a pass
    /// would end it: nothing that served the earlier mob (its events
    /// subscriber, its identity runtime and that runtime's lifecycle
    /// projection) writes under the id after the release, and the entry's
    /// end, which ends the id's console events, does not come after the
    /// new mob was given its own. An entry whose actor runs is not ended
    /// here: the mob set owns whether a mob stands under an id.
    ///
    /// The member wiring the table kept for an earlier mob of the id that
    /// no pass gave its entry goes in the same step, and with it the
    /// console events that mob's build named with the host, which only an
    /// entry's end would have ended. A mob created under the id is one the
    /// host serves only when the mob set builds it for the host, and it is
    /// then given a wiring and console events of its own: nothing an
    /// earlier mob left under the id makes an entry for it, or tells it as
    /// a mob of the host. Neither can fail, and both go first all the same,
    /// under the hold of the table's passes and before any release that
    /// can refuse: once no mob stands under the id they serve nobody, so a
    /// create that is then refused has lost nothing by it, and no pass
    /// meanwhile finds a wiring to build an entry from.
    ///
    /// Then the create WAITS until nothing writes into the host's console
    /// log for a mob under the id any more
    /// ([`Self::console_writers_at_rest`]): the end of an entry marks its
    /// console registration ended and does not wait for a write of it that
    /// is in flight, and such a write would otherwise be stored after the
    /// frames under the id were removed, as a frame of the new mob. The
    /// wait is the first thing here that can refuse the create after the
    /// earlier entry's end: a create refused at its bound has made none of
    /// the releases that follow. It is made outside the lock of the passes,
    /// so it holds up this id's create alone.
    ///
    /// The schedules the launch keeps under the id are not released here:
    /// the create admission asks their own release first, as another of
    /// the releases bound to the launch's record
    /// ([`crate::mob_schedules_release`]), so they have gone, or the create
    /// is refused, before this is asked.
    ///
    /// What the launch's announcements keep under the id goes as well,
    /// through the release bound where their owner is attached
    /// ([`crate::mob_announcements_release`]). A launch that declared
    /// announcements whose owner is not attached yet refuses the create
    /// before anything here is released.
    ///
    /// The agent memory the launch keeps for the mob of that id goes too,
    /// through the release the launch bound where it handed its memory over
    /// ([`crate::mob_memory_release`]), after the store's records: a mob
    /// created under the id is later given a memory of its own, and it
    /// must start with none of the earlier mob's. A launch that declared
    /// such memory and has not handed it over yet refuses the create
    /// before anything is released.
    ///
    /// The releases that can refuse the create come first: the store's
    /// records, the console frames, the memory of a hosted mob, the
    /// announcements. What the host keeps in its memory and cannot fail to
    /// forget (the labels) goes last, so a create one of those releases
    /// refused has forgotten none of it.
    ///
    /// What the host's delivery service keeps under the id (its routes,
    /// routing resolutions and deliveries, in the host's module runtime)
    /// is of that kind too: it is in memory, cannot fail to go and cannot
    /// be brought back. It goes after the labels.
    ///
    /// What the launch's record of private members holds under the id is
    /// not released here: the admission writes the new mob's own record in
    /// the place of the earlier mob's and forgets the members it held
    /// under the id, once every release passed; each member of the new mob
    /// is recorded by its own build.
    async fn release_before_create(&self, mob_id: &MobId) -> Result<(), MobError> {
        let Some(host) = self.host.get() else {
            return Err(MobError::WiringError(format!(
                "the mob '{mob_id}' cannot be created yet: the runtime that serves it is still \
                 being composed, so what an earlier mob of that id left with the host cannot be \
                 released"
            )));
        };
        // The launch's announcements: their owner is attached after the
        // runtime is built, and a create that came before it
        // could release nothing of them.
        let announcements = match host.launch_mob.mob_announcements_release().standing() {
            MobAnnouncementsStanding::NotDeclared => None,
            MobAnnouncementsStanding::Bound(announcements) => Some(announcements),
            MobAnnouncementsStanding::Owed => {
                return Err(MobError::WiringError(format!(
                    "the mob '{mob_id}' cannot be created yet: the launch's announcements are \
                     not attached to the runtime that serves it yet, so what an earlier mob of \
                     that id left with the announcements cannot be released"
                )));
            }
        };
        // So with the memory the launch keeps for the mobs it hosts: the
        // launch hands it over after its launch mob's agents are brought
        // up, and a create that came before could release nothing of it,
        // while the mob it creates would later be given that memory.
        let memory = match host.launch_mob.mob_memory_release().standing() {
            MobMemoryStanding::NotDeclared => None,
            MobMemoryStanding::Bound(memory) => Some(memory),
            MobMemoryStanding::Owed => {
                return Err(MobError::WiringError(format!(
                    "the mob '{mob_id}' cannot be created yet: the memory the launch keeps for \
                     the mobs it hosts is not handed to the runtime that serves it yet, so the \
                     memory an earlier mob of that id left cannot be released"
                )));
            }
        };
        {
            // A pass under way ends first: it may be ending this very entry.
            let _one_at_a_time = self.pass.lock().await;
            // What the table kept for an earlier mob of the id that no pass
            // gave its entry goes too. No mob stands under the id now; the
            // mob created under it is given its own wiring at its build
            // when the host creates it, and none when a member does, so a
            // pass makes no entry for a member's mob from what an earlier
            // mob of the host left.
            locked(&self.building).remove(mob_id);
            let earlier = {
                let mut entries = locked(&self.entries);
                if entries
                    .get(mob_id)
                    .is_some_and(|entry| entry.actor_closed())
                {
                    entries.remove(mob_id)
                } else {
                    None
                }
            };
            if let Some(entry) = earlier {
                self.end(host, &entry).await;
            } else if !locked(&self.entries).contains_key(mob_id) {
                // No entry stood under the id, so nothing ended the console
                // events an earlier mob's build named with the host. They
                // go here: a mob the host creates under the id is named
                // again at its build, and a mob a member creates is never
                // named, so it is told as a member's mob.
                host.console_events.end_of_mob(mob_id);
            }
        }
        // The earlier mob's writes into the host's console log come to
        // rest before anything under the id is released.
        Self::console_writers_at_rest(host, mob_id).await?;
        match host
            .persistent_metadata
            .release_mob_records(mob_id.as_str())
            .await
        {
            Ok(crate::runtime::MobRecordsRelease::Released { records }) => {
                if records > 0 {
                    tracing::info!(
                        mob_id = %mob_id,
                        records,
                        "released the records an earlier mob of this id left with the host, \
                         before the mob is created"
                    );
                }
            }
            Ok(crate::runtime::MobRecordsRelease::NotReleased) => {
                return Err(MobError::WiringError(format!(
                    "the mob '{mob_id}' cannot be created: the host's metadata store has no \
                     release of a mob's records, so the mob would read what an earlier mob of \
                     that id left"
                )));
            }
            Err(error) => {
                // The store's own words stay with the host: they can name a
                // file or a key of the store.
                tracing::warn!(
                    mob_id = %mob_id,
                    error = %error,
                    "the records an earlier mob of this id left with the host could not be \
                     released; the create is refused"
                );
                return Err(MobError::WiringError(format!(
                    "the mob '{mob_id}' cannot be created: the records an earlier mob of that \
                     id left with the host could not be released"
                )));
            }
        }
        // The console frames kept under the id are an earlier mob's too.
        Self::release_console_records(host, mob_id).await?;
        // The memory the launch keeps for the mob of this id. The mob set
        // holds no mob under the id now, and the entry of an earlier mob
        // ended above; whatever still holds that mob's store is refused by
        // the release, which closes the store before it removes its
        // directory.
        if let Some(memory) = memory
            && let Err(error) = memory.release_mob(mob_id)
        {
            // The release's own words stay with the host: they can name a
            // path of the memory root.
            tracing::warn!(
                mob_id = %mob_id,
                error = %error,
                "the memory an earlier mob of this id left with the host could not be \
                 released; the create is refused"
            );
            return Err(MobError::WiringError(format!(
                "the mob '{mob_id}' cannot be created: the memory an earlier mob of that id \
                 left with the host could not be released"
            )));
        }
        // What the launch's announcements keep under the mob's id: its
        // sources, its announcements, the subscriptions that name it. The
        // announcements' owner says in its own log what it released.
        if let Some(announcements) = announcements
            && let Err(error) = announcements.release_mob(mob_id).await
        {
            // The release's own words stay with the host: they can name a
            // path of the announcements' store.
            tracing::warn!(
                mob_id = %mob_id,
                error = %error,
                "what an earlier mob of this id left with the announcements could not be \
                 released; the create is refused"
            );
            return Err(MobError::WiringError(format!(
                "the mob '{mob_id}' cannot be created: what an earlier mob of that id left \
                 with the announcements could not be released"
            )));
        }
        // What cannot fail goes last, once every release above that can
        // refuse the create is done: a create one of them refused has
        // forgotten none of it. Where a release that is asked after this
        // one refuses the create, this one has already made its releases
        // under the id, and the next create under the id asks every
        // release again.
        // The labels of the mob and of its runs are kept in the host's
        // memory under the mob's id.
        let labels = host.metadata_table.release_mob(mob_id.as_str()).await;
        if labels > 0 {
            tracing::info!(
                mob_id = %mob_id,
                labels,
                "released the labels an earlier mob of this id left with the host, before the \
                 mob is created"
            );
        }
        // What the host's module runtime keeps under the id (routes, routing
        // resolutions, deliveries and their idempotency entries) is in
        // memory only and cannot fail to go. It goes after the releases
        // above that can refuse the create, so a create refused by a
        // release above has dropped none of it. A create refused later (by
        // the mob's wiring, its store, its build or its registration) has:
        // what went was kept under an id no mob holds. The table holds the
        // module runtime weakly: one that is gone keeps nothing to forget.
        //
        // The same forget withdraws, as the host, the approvals still
        // waiting for an earlier mob's members. Those are not in memory
        // only where the runtime keeps its gating state across restarts,
        // so that state is written here, before the create goes on: a
        // process that ends after this starts again without them. Neither
        // step refuses the create: a withdrawal that cannot be recorded
        // leaves its request pending, and a write that fails is logged, as
        // every write of that state is, and made again at the next gating
        // change.
        if let Some(module_runtime) = host.module_runtime.upgrade() {
            module_runtime.lock().await.forget_mob(mob_id);
            if let Some(writer) = host.gating_state_writer.get() {
                writer.persist(&module_runtime).await;
            }
        }
        // Last, what the launch's two memory slots keep in memory for a mob
        // it composed under the id: the marks of that mob's members and
        // what its side chats were bound from. It cannot fail to go, and a
        // mob created under the id starts with neither.
        host.launch_mob
            .dispatch_taint_slot()
            .forget_hosted_mob(mob_id.as_str());
        host.launch_mob
            .side_chat_memory_slot()
            .forget_hosted_mob(mob_id.as_str());
        Ok(())
    }

    /// The create under `mob_id` is admitted: what the runtime's access
    /// controllers hold of the id (the attributes and the current runtime
    /// aliases the console read of that mob's agents) is in memory only
    /// and is an earlier mob's, and it goes here
    /// ([`crate::access::AccessController::forget_mob`]). The mob created
    /// under the id is recorded by the reads of its own roster. A create
    /// that a release of its admission refuses, this table's or one asked
    /// after it, does not come here: the controllers hold what they held.
    fn forget_at_admission(&self, mob_id: &MobId) {
        let access_controllers = locked(&self.access_controllers).clone();
        for access in &access_controllers {
            access.forget_mob(mob_id);
        }
    }

    /// The mob set builds the mob `mob_id` for the host: its member wiring
    /// is made here, as the bootstrap makes the default mob's when the spec
    /// brings none, and kept for the entry the mob gets once the set
    /// manages it. A later build of the same mob (a load after an unload)
    /// makes it anew.
    ///
    /// The mob's console events with the host are named here too, before
    /// the mob exists for the set: the launch mob's delegate scan leaves
    /// out every mob that has console events of its own with the host, so
    /// the mob is at no moment scanned as a mob a member made.
    async fn member_wiring(&self, mob_id: &MobId) -> Result<HostMobMemberWiring, MobError> {
        let wiring = MemberBuildWiring::deferred();
        // A launch that builds an agent only of a composed mob (its recall
        // failure policy is `fail`) withholds the mob's member builds from
        // here: a member the mob materializes at its start is refused, to
        // be retried, until the mob's first composition succeeds. The
        // build asks for that composition itself: the table follows its
        // set from here, and a pass composes the entries it holds. A mob
        // built before the launch said how it composes (a launch says it
        // after its launch mob's identity bootstrap; a stored mob is built
        // by the request that names it) is not withheld here: it is from
        // the answer of its first composition that does not succeed.
        if self
            .identity_composer
            .get()
            .is_some_and(|composer| composer.builds_agents_only_of_composed_mobs())
        {
            wiring.withhold_member_builds(
                crate::memory::sqlite_store::HostedAgentsNotBuilt::MemoryNotComposedYet,
            );
        }
        let answer = wiring.host_mob_member_wiring();
        if let Some(host) = self.host.get() {
            let _named = host.console_events.of_mob(Some(mob_id));
        }
        locked(&self.building).insert(mob_id.clone(), wiring);
        self.hosted_another_mob.store(true, Ordering::SeqCst);
        self.follow_the_set();
        Ok(answer)
    }

    /// One ordinary pass, on a task the table holds: a pass ends the entry
    /// of every mob the set no longer manages, and of every actor that
    /// closed, under the hold of the table's passes, so the destroyed
    /// mob's entry goes with whatever a pass under way is building. The
    /// task that follows the set runs the same pass after each change, but
    /// only once it has composed the identity planes the table owed, which
    /// can outlast the second a destroy waits.
    ///
    /// The pass is started here and waited for by nobody: its task is
    /// among the table's passes before this returns, and the table's close
    /// joins it ([`MobEntries::start_a_pass`]). A table that is closing
    /// starts none: its close ends every entry.
    ///
    /// Nothing is done by the id: the entry is not taken out of the table
    /// here (a release that finds no entry under an id reads that as
    /// "ended", under the same hold), and a wiring left under the id stays
    /// for the release before a create, since the id may be another mob's
    /// by now. What a pass cannot shorten stays as it is: the entry's end
    /// waits for a composition of that mob's identity plane that is under
    /// way, and whoever still holds the entry holds the mob's handle.
    fn after_destroy(&self, _destroyed: &MobId) {
        #[cfg(test)]
        if self
            .destroys_ask_for_no_pass_in_tests
            .load(Ordering::SeqCst)
        {
            return;
        }
        drop(self.start_a_pass());
    }

    /// The identity plane of the mob of this table whose roster names the
    /// member of `session`: each entry the table holds whose actor runs is
    /// asked through its own mob's roster state, without waiting and
    /// without asking the mob's actor. It is read from the table as it
    /// stands: no mob is loaded, and whether requests resolve a mob of the
    /// table is not asked (an agent's tools are its own mob's on every
    /// launch).
    fn identity_plane_of_session(
        &self,
        session: &meerkat_core::types::SessionId,
    ) -> Option<crate::mob_handle_runtime::MobIdentityPlane> {
        self.held()
            .into_iter()
            .filter(|entry| !entry.actor_closed())
            .find(|entry| {
                futures::FutureExt::now_or_never(
                    entry.mob_runtime.handle().member_of_session(session),
                )
                .flatten()
                .is_some()
            })
            .map(|entry| entry.mob_runtime.identity_plane().clone())
    }
}

/// Follow the set's changes for as long as the table lives: one pass at the
/// start, and one after each change of the mobs the set manages.
async fn follow(entries: Weak<MobEntries>, mut changes: tokio::sync::watch::Receiver<u64>) {
    loop {
        let Some(table) = entries.upgrade() else {
            return;
        };
        table.pass().await;
        table.compose_held().await;
        drop(table);
        if changes.changed().await.is_err() {
            return;
        }
    }
}

/// The entry of a mob the set built for the host and manages as `handle`,
/// with `wiring`, the member wiring that mob was built with, and
/// `topology_controller`, the mob's own topology controller. The mob has no
/// identity context until its host attaches one, the edge policy its
/// definition declares, and its own realm of the launch's WorkGraph where
/// the launch has one: the service its wrapper holds
/// ([`MobRuntime::of_hosted_mob`]), a hub of that realm's facts and the one
/// tail that feeds it, as the launch mob's entry has its own.
fn entry_of_hosted_mob(
    host: &HostOfEntries,
    set: &Arc<meerkat_mob_mcp::MobMcpState>,
    handle: &MobHandle,
    wiring: MemberBuildWiring,
    topology_controller: crate::topology_control::TopologyController,
) -> MobEntry {
    let mob_runtime = host.launch_mob.of_hosted_mob(handle);
    let handle = mob_runtime.handle();
    let console_log = super::mob_console_log::EntryConsoleEvents::of(
        host.console_events.of_mob(Some(mob_runtime.mob_id())),
    );
    let mob_events = MobEventsStore::new().with_metadata_table(Arc::clone(&host.metadata_table));
    let identity_plane = mob_runtime.identity_plane().clone();
    let identity_binding_wake = tokio::sync::watch::channel(0_u64).0;
    let identity_completion_ledger = Arc::new(HealthCompletionLedger::default());
    let identity_stream_health_task = tokio::runtime::Handle::try_current().ok().map(|executor| {
        executor.spawn(super::run_identity_stream_health_monitor(
            handle.clone(),
            Some(Arc::clone(set)),
            identity_plane.clone(),
            Arc::clone(&identity_completion_ledger),
            identity_binding_wake.subscribe(),
        ))
    });
    let mob_events_task = UnifiedRuntime::spawn_mob_events_subscriber(
        handle.clone(),
        mob_events.clone(),
        Arc::clone(&host.persistent_metadata),
        mob_runtime.implicit_delegate_retirement_overrides(),
        mob_runtime.private_owners().clone(),
        mob_runtime.speaks_for_composition(),
    );
    let actor_loop_health = crate::actor_loop_health::ActorLoopHealth::shared();
    let actor_loop_probe_task = UnifiedRuntime::spawn_actor_loop_probe(
        handle.clone(),
        Arc::clone(&host.error_hook),
        Arc::clone(&actor_loop_health),
    );
    let edge_discovery =
        edge_reconcile::DefinitionWiringEdgeDiscovery::from_definition(handle.definition())
            .map(|policy| Arc::new(policy) as Arc<dyn EdgeDiscovery>);
    let workgraph_service = mob_runtime.workgraph_service();
    let (workgraph_fact_hub, workgraph_fact_tail_task) =
        workgraph_fact_hub_and_tail(workgraph_service.as_ref());
    MobEntry {
        mob_runtime,
        bootstrap_completion: Mutex::new(bootstrap_completion::CompletionSlot::Pending),
        bootstrap_edges_report: tokio::sync::RwLock::new(None),
        pending_mob_activation: tokio::sync::Mutex::new(None),
        bootstrap_member_build_wiring: Some(wiring),
        edge_discovery,
        managed_dynamic_edges: Arc::new(tokio::sync::RwLock::new(BTreeSet::new())),
        cross_mob_routes: OnceLock::new(),
        mob_events_subscriber_task: tokio::sync::Mutex::new(mob_events_task),
        actor_loop_probe_task: tokio::sync::Mutex::new(actor_loop_probe_task),
        actor_loop_health,
        topology_controller,
        workgraph_service,
        workgraph_fact_hub,
        workgraph_fact_tail_task: tokio::sync::Mutex::new(workgraph_fact_tail_task),
        identity_plane,
        identity_binding_wake,
        identity_completion_ledger,
        session_bridge: None,
        identity_first_context: Arc::default(),
        console_identity_roster: Arc::new(std::sync::RwLock::new(None)),
        discovery: None,
        mob_events,
        continuity_mirror: tokio::sync::Mutex::new(None),
        identity_stream_health_task: tokio::sync::Mutex::new(identity_stream_health_task),
        identity_request_services: std::sync::RwLock::new(None),
        served_memory: std::sync::Mutex::new(None),
        identity_composition: tokio::sync::Mutex::new(()),
        #[cfg(test)]
        memory_refusals: std::sync::Mutex::new(Vec::new()),
        memory_refusal_logged: std::sync::Mutex::new(None),
        #[cfg(test)]
        memory_refusal_lines: std::sync::Mutex::new(Vec::new()),
        console_log,
    }
}

impl MobEntry {
    /// What the launch mob's start does for the launch mob
    /// ([`WorkGraphAdmission::hold_bindings_onto_private_members`](crate::workgraph_admission::WorkGraphAdmission::hold_bindings_onto_private_members)),
    /// for this entry's mob, a mob the set built for the host beside the
    /// default mob, when its entry is made: at its create and at every
    /// load.
    ///
    /// First the launch mob's admission, which the member builds of every
    /// mob of the host ask, is told this mob's own
    /// ([`WorkGraphAdmission::hold_builds_of`](crate::workgraph_admission::WorkGraphAdmission::hold_builds_of)):
    /// from here on the build of a member of this mob private to a person
    /// holds the bindings that name it. Then every binding of this mob's
    /// realm whose target is a member of the mob private to a person is
    /// held paused. A member built before the entry was made is in the
    /// roster this hold reads.
    ///
    /// Only the launch that speaks for the composition writes. The hold
    /// fails open, as the launch mob's does: when the bindings cannot be
    /// read the fault is logged and the entry is made.
    async fn hold_bindings_onto_private_members(&self, host: &HostOfEntries) {
        let Some(service) = self.workgraph_service.as_ref() else {
            return;
        };
        let admission = self.mob_runtime.workgraph_admission();
        host.launch_mob
            .workgraph_admission()
            .hold_builds_of(&admission);
        if !self.mob_runtime.speaks_for_composition() {
            return;
        }
        if let Err(error) = admission.hold_bindings_onto_private_members(service).await {
            tracing::error!(
                mob_id = %self.mob_runtime.mob_id(),
                error = ?error,
                "the attention bindings of a mob hosted beside the launch mob could not be read \
                 when its entry was made; a binding onto a member private to a person may still \
                 be active"
            );
        }
    }

    /// Attach `context` as the identity context of this entry's mob, a mob
    /// the mob set built for the host beside the default mob. Its host
    /// composed the context over this mob's own handle: the identity
    /// runtime is this mob's and no other's.
    ///
    /// The member wiring the mob was built with is bound to the context's
    /// identity runtime first, as the default mob's identity bootstrap
    /// binds its own: with an agent customizer the runtime gets a customizer
    /// tool registry of its own, and without one the wiring is decided for
    /// none. A refusal of that binding attaches nothing. Then the identity
    /// runtime becomes the one attached to the mob's plane, with this
    /// entry's actor loop verdict, completion ledger and binding wake, and
    /// the context is installed in the entry's cell.
    ///
    /// Not installed here, for a mob other than the default mob: the flow
    /// target provisioner, the observer that carries an idle-retire opt-in
    /// across a rotation, and the console's family of identity reads.
    pub(super) async fn attach_identity_context(
        &self,
        context: Arc<crate::identity_first::IdentityFirstRuntimeContext>,
    ) -> Result<(), crate::identity_first::IdentityRuntimeError> {
        let session_service = self.mob_runtime.session_service().cloned();
        if let Some(wiring) = self.bootstrap_member_build_wiring.as_ref() {
            wiring
                .bind_at_identity_bootstrap(&context, session_service.clone())
                .await
                .map_err(crate::identity_first::IdentityRuntimeError::Internal)?;
        }
        context
            .runtime
            .install_actor_loop_health(Arc::clone(&self.actor_loop_health));
        // The identity runtime composes this mob's operator intent over
        // its declared topology, under this mob's controller's admission.
        context
            .runtime
            .set_topology_controller(self.topology_controller.clone());
        self.mob_runtime
            .install_identity_runtime_authority(Arc::clone(&context.runtime));
        if let Some(session_service) = session_service {
            context.runtime.install_pending_completion_drain(Arc::new(
                super::ReplayedCompletionDrain {
                    session_service,
                    ledger: Arc::clone(&self.identity_completion_ledger),
                },
            ));
        }
        context
            .runtime
            .install_member_binding_observer(self.identity_binding_wake.clone());
        self.identity_first_context.install(context);
        // The mob is composed, and everything a member's build and its
        // first run read is installed above: builds that were withheld
        // from the mob are withheld no longer, and Core is asked to retry
        // the preparation of each member whose build was refused
        // meanwhile, so its next wake asks the bound preparer. Until here
        // every build of the mob was still refused. A member whose refusal
        // Core had not recorded yet when it is asked to retry is retried
        // by the mob's own retry timer.
        if let Some(wiring) = self.bootstrap_member_build_wiring.as_ref() {
            for member in wiring.release_withheld_member_builds() {
                if let Err(error) = self
                    .mob_runtime
                    .handle()
                    .resolve_member_build_preparation(&member)
                    .await
                {
                    tracing::warn!(
                        %member,
                        %error,
                        "a member whose build was withheld could not be retried now; its \
                         mob's own retry timer retries it"
                    );
                }
            }
        }
        Ok(())
    }

    /// Whether the actor this entry was built for closed its admission: the
    /// set unloaded, destroyed or stood the mob down since.
    pub(super) fn actor_closed(&self) -> bool {
        self.mob_runtime
            .handle()
            .admission_closed()
            .borrow()
            .is_some()
    }

    /// The set no longer manages this entry's mob as the actor the entry
    /// was built for: the mob's part of the link table goes first, with the
    /// inbox routes of its links, then the tasks that served that actor
    /// (the tail of its WorkGraph facts among them), and the mob's identity
    /// runtime is detached from its plane. A composition of the mob's
    /// identity plane under way ends first, and none starts for an entry
    /// whose actor closed.
    pub(super) async fn close(&self) {
        self.clear_cross_mob_wake_routes();
        let _composed = self.identity_composition.lock().await;
        for task in [
            self.actor_loop_probe_task.lock().await.take(),
            self.identity_stream_health_task.lock().await.take(),
            self.mob_events_subscriber_task.lock().await.take(),
            self.workgraph_fact_tail_task.lock().await.take(),
        ]
        .into_iter()
        .flatten()
        {
            task.abort();
            let _ = task.await;
        }
        // The mob's identity runtime ends with the entry: it takes no
        // request from here on, and what it ran for requests is joined.
        self.end_identity_plane().await;
        self.mob_runtime.clear_identity_runtime_authority();
        // The mob's controller gives up its state file: the next load of
        // the mob opens it anew, and nothing writes it meanwhile.
        self.topology_controller.close().await;
    }
}

#[cfg(test)]
#[path = "mob_lazy_start_tests.rs"]
pub(super) mod lazy_start_tests;

#[cfg(test)]
#[path = "mob_creator_store_read_tests.rs"]
mod creator_store_read_tests;
