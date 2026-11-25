//! The per-mob entry of a runtime.
//!
//! A [`UnifiedRuntimeCore`](super::UnifiedRuntimeCore) keeps what is one per
//! process whatever the number of mobs: the routers of its listener, the
//! console, sign-in, the access policy, the shared stores, the module
//! runtime, the cross-mob control channel. What is bound to one mob lives in
//! that mob's [`MobEntry`]: the mob wrapper, the steps of that mob's
//! bootstrap, the tasks that serve its actor and its event ledger, its part
//! of the link table between mobs of this process, and its identity plane.
//!
//! An entry is mechanics. It is built where its mob is activated and dropped
//! with it. It owns no lifecycle fact: a mob's phase, members and wiring are
//! its `MobMachine`'s, and which mobs a host has is the mob set's.
//!
//! A runtime holds the launch mob's entry as a field of its own on the
//! process part. That mob is the default mob: the mob that answers for a
//! caller that names none. Its entry lives exactly as long as the runtime,
//! so the accessors that hand out references into it borrow from the
//! runtime itself. The entries of the other mobs the launch's mob set
//! builds for the host live in a table beside it (`mob_entries`): each is
//! built whole when the set manages its mob, shared, never written in
//! place, and dropped when the set no longer manages that mob's actor.

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_mob::MobId;
use tokio::task::JoinHandle;

use super::mob_entries::{DestroyedMobMemory, HostedMobListing};
use super::mob_events::MobEventsStore;
use super::{
    Discovery, EdgeDiscovery, HealthCompletionLedger, UnifiedRuntime,
    UnifiedRuntimeReconcileEdgesReport, WorkGraphFactTailTask, bootstrap_completion,
};
use crate::mob_handle_runtime::{MobIdentityPlane, MobRuntime, PendingMobActivation};

/// What a runtime holds for one mob.
pub(super) struct MobEntry {
    /// Observer-side mob runtime: every internal holder (tasks, routers,
    /// bridges, tool state) is non-owning.
    pub(super) mob_runtime: MobRuntime,
    /// The one-use bootstrap completion, between its mint and its take.
    pub(super) bootstrap_completion: std::sync::Mutex<bootstrap_completion::CompletionSlot>,
    pub(super) bootstrap_edges_report:
        tokio::sync::RwLock<Option<UnifiedRuntimeReconcileEdgesReport>>,
    /// Set only for an identity-first resume of a persistent log, where the
    /// mob is deliberately left `Stopped` until continuity is registered.
    /// While this is `Some`, the mob is NOT ready: it cannot spawn and
    /// bootstrap edge reconciliation has been deferred.
    pub(super) pending_mob_activation: tokio::sync::Mutex<Option<PendingMobActivation>>,
    /// The member build wiring [`super::UnifiedRuntime::bootstrap`] installed
    /// on a mob whose spec brought no build preparer, bound at the identity
    /// bootstrap (`gateway_wiring::MemberBuildWiring::bind_at_identity_bootstrap`).
    pub(super) bootstrap_member_build_wiring: Option<crate::gateway_wiring::MemberBuildWiring>,
    pub(super) edge_discovery: Option<Arc<dyn EdgeDiscovery>>,
    pub(super) managed_dynamic_edges: Arc<tokio::sync::RwLock<BTreeSet<(String, String)>>>,
    /// This mob's part of the link table between mobs of this process
    /// (`cross_mob_routes`), set by the start-time read of the mob's rows,
    /// which runs once for the mob. Only the entry that started a part
    /// closes it.
    pub(super) cross_mob_routes:
        std::sync::OnceLock<Arc<super::cross_mob_routes::CrossMobWakeRoutes>>,
    pub(super) mob_events_subscriber_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// Actor-loop liveness probe: a periodic O(1) round trip through the
    /// serialized mob command loop that pages (`ErrorEvent::ActorLoopStalled`)
    /// when the loop stops draining. Observation only - aborted first during
    /// shutdown so the intentional actor stop cannot fire a false page.
    pub(super) actor_loop_probe_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// The probe's published verdict (live / stalled / terminated), shared
    /// with the identity-first delivery path so a send issued while a stall
    /// is open fails fast naming the `stall_id` instead of waiting the whole
    /// admission budget behind the wedged command.
    pub(super) actor_loop_health: Arc<crate::actor_loop_health::ActorLoopHealth>,
    /// Optional product-level topology authority. The controller always
    /// exists so query can remain available, but its policy defaults to
    /// disabled and mutation methods are then absent/denied.
    pub(super) topology_controller: crate::topology_control::TopologyController,
    /// Realm-scoped WorkGraph service backing the `meta/workgraph/*` RPC
    /// group and the console experience section. Seeded from the bootstrap
    /// spec and deliberately FIXED from then on: the admission guards
    /// (cross-process sidecar + agent tool-plane slots) freeze at
    /// `MobRuntime::bootstrap`, so a service wired in later would run
    /// guard-degraded. The spec (`MobBootstrapSpec::with_workgraph_service`
    /// plus admission slot/sidecar) is the only blessed wiring.
    pub(super) workgraph_service: Option<meerkat::WorkGraphService>,
    /// Runtime-owned lossy wake fan-out. It exists exactly when the
    /// authoritative WorkGraph service exists and is never an authority of
    /// its own.
    pub(super) workgraph_fact_hub: Option<crate::workgraph_events::WorkGraphFactHub>,
    /// Sole WorkGraph cursor tail feeding `workgraph_fact_hub`. It waits on a
    /// subscriber notification without reading the store while idle.
    pub(super) workgraph_fact_tail_task: tokio::sync::Mutex<WorkGraphFactTailTask>,
    /// This mob's identity plane: its id and the late-bound identity
    /// authority that the already-running idle retirement sweeper, member
    /// event monitor, health route and control listener observe. Gateways
    /// attach identity-first after base runtime bootstrap, so capturing
    /// `identity_runtime()` when those start would permanently capture
    /// `None`.
    pub(super) identity_plane: MobIdentityPlane,
    /// Woken (an epoch bump) whenever Core's binding of an identity changes
    /// in the installed identity runtime; the identity health monitor's typed
    /// wake for attaching a member that just went live.
    pub(super) identity_binding_wake: tokio::sync::watch::Sender<u64>,
    /// Per-session sequence through which the identity health monitor and
    /// the completion-cursor drain credited run completions, shared so each
    /// completion is credited exactly once.
    pub(super) identity_completion_ledger: Arc<HealthCompletionLedger>,
    /// Identity-first session bridge.
    pub(super) session_bridge: Option<Arc<dyn crate::identity_first::bridge::SessionBridge>>,
    /// The mob's identity-first context, in the entry's cell: a host
    /// installs it after the base bootstrap. The cell is shared with the
    /// owned form of the resolved mob ([`ResolvedMob`]), which reads it at
    /// each call.
    pub(super) identity_first_context: Arc<IdentityContextCell>,
    /// Identity-first console gateways: the mutable desired-identity roster
    /// that `meta/ensure_member` extends at runtime (ask K0). Set by the
    /// host beside `attach_identity_first_context`. The cell is shared
    /// with the owned form of the resolved mob ([`ResolvedMob`]), which
    /// reads it at each call: the roster lives here and nowhere else.
    pub(super) console_identity_roster: ConsoleIdentityRosterCell,
    /// The discovery this mob's roster is spawned from, and respawned from
    /// by a rediscover. A discovery is asked for members of one mob.
    pub(super) discovery: Option<Box<dyn Discovery>>,
    /// The structural events of this mob's ledger, as the mob's ledger
    /// subscriber (`mob_events_subscriber_task`) projects them. Each mob's
    /// ledger has its own cursors.
    pub(super) mob_events: MobEventsStore,
    /// The continuity mirror of this mob's identity runtime, when the host
    /// installed one. Shutdown stops it, with a last round, before the mob
    /// quiesces. Where it copies to is the host's configuration.
    pub(super) continuity_mirror:
        tokio::sync::Mutex<Option<super::continuity_mirror::ContinuityMirror>>,
    /// The monitor of this mob's identity streams: it watches this mob's
    /// identity plane, completion ledger and binding wake. The forwarder of
    /// member events beside it is the process's.
    pub(super) identity_stream_health_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// What a request that reaches this mob's identity plane needs beside
    /// the identity context itself, set by the host that composed the mob.
    pub(super) identity_request_services: std::sync::RwLock<Option<IdentityRequestServices>>,
    /// What serves this mob's agent memory while its identity plane lives,
    /// as the host that composed the mob handed it over: it holds the mob,
    /// and it ends with the plane ([`MobEntry::end_identity_plane`]).
    pub(super) served_memory: std::sync::Mutex<Option<crate::gateway_wiring::ServedMobMemory>>,
    /// One composition of this mob's identity plane at a time, for a mob
    /// the host serves beside its default mob: whoever asks while one runs
    /// waits for it. It records nothing: whether the mob has its identity
    /// context is the context's cell.
    pub(super) identity_composition: tokio::sync::Mutex<()>,
    /// Test builds only: each refusal a composition of this mob answered
    /// because the directory under the mob's name in the launch's agent
    /// memory is not proved the mob's, in order: one for each ask, written
    /// where the host's log line is.
    #[cfg(test)]
    pub(super) memory_refusals:
        std::sync::Mutex<Vec<crate::memory::sqlite_store::HostedMemoryRefusal>>,
    /// The refusal the host's log last carried for this mob's memory
    /// directory: a composition that is refused for the same reason again
    /// writes no second line, and one refused for another reason writes
    /// one. It is this entry's, so it lasts while the mob stays loaded: a
    /// mob that is unloaded and loaded again has a new entry, and its
    /// refusal is logged again. A logging fact only: every ask is answered
    /// on its own, and nothing is decided from this.
    pub(super) memory_refusal_logged:
        std::sync::Mutex<Option<crate::memory::sqlite_store::HostedMemoryRefusal>>,
    /// Test builds only: each refusal the host's log carried for this mob
    /// with the words of its line, in order: one for each line written.
    #[cfg(test)]
    pub(super) memory_refusal_lines: std::sync::Mutex<
        Vec<(
            crate::memory::sqlite_store::HostedMemoryRefusal,
            &'static str,
        )>,
    >,
    /// What this entry holds of its mob's console events with the host: the
    /// mob's own store as it stood when the entry was made, and the
    /// registration of that store with the runtime's console projection.
    /// The default mob's entry holds neither: its console events are the
    /// host's own store, and its runtime is registered as a runtime.
    pub(super) console_log: super::mob_console_log::EntryConsoleEvents,
}

/// The cell a mob's console roster lives in: set by the host after the
/// routers are built, read at each use.
pub(super) type ConsoleIdentityRosterCell =
    Arc<std::sync::RwLock<Option<Arc<crate::identity_first::MutableRosterProvider>>>>;

/// The cell a mob's identity context lives in. A host installs the context
/// after the base bootstrap, when routers may already be built, so the
/// entry shares the cell with the owned form of the resolved mob, which
/// reads it at each call: the context lives here and nowhere else.
///
/// The cell hands out a borrow of the installed context for as long as the
/// cell is borrowed (the runtime's `identity_runtime_ref` answers such a
/// borrow). A later install replaces the context for every read from
/// then on; the replaced one is kept until the cell is dropped, so a
/// borrow taken before stays what it was.
#[derive(Default)]
pub(super) struct IdentityContextCell {
    first: std::sync::OnceLock<Box<InstalledIdentityContext>>,
}

/// One install of an identity context, and the install that replaced it.
struct InstalledIdentityContext {
    context: Arc<crate::identity_first::IdentityFirstRuntimeContext>,
    replaced_by: std::sync::OnceLock<Box<InstalledIdentityContext>>,
}

impl IdentityContextCell {
    /// The identity context installed last, if one was installed.
    pub(super) fn get(&self) -> Option<&Arc<crate::identity_first::IdentityFirstRuntimeContext>> {
        let mut installed = self.first.get()?;
        while let Some(later) = installed.replaced_by.get() {
            installed = later;
        }
        Some(&installed.context)
    }

    /// Install `context`: it is the mob's identity context from now on.
    /// Installing the context that is installed already changes nothing.
    pub(super) fn install(&self, context: Arc<crate::identity_first::IdentityFirstRuntimeContext>) {
        let mut slot = &self.first;
        let mut install = Box::new(InstalledIdentityContext {
            context,
            replaced_by: std::sync::OnceLock::new(),
        });
        loop {
            match slot.get() {
                Some(installed) => {
                    if installed.replaced_by.get().is_none()
                        && Arc::ptr_eq(&installed.context, &install.context)
                    {
                        return;
                    }
                    slot = &installed.replaced_by;
                }
                None => match slot.set(install) {
                    Ok(()) => return,
                    // Another install took the slot: go on behind it.
                    Err(refused) => install = refused,
                },
            }
        }
    }
}

/// What a request that reaches a mob's identity plane needs beside the
/// mob's identity context: the services the host composed for that mob's
/// identity runtime. Each is `None` where the host composed none, and the
/// verbs that need it then refuse.
///
/// Nothing reads the three values from an entry yet: a request handler
/// takes them from here once it serves the mob a request resolved.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Default)]
pub(crate) struct IdentityRequestServices {
    /// The provider of the mob's agent memory.
    pub(crate) agent_memory_provider: Option<Arc<dyn crate::identity_first::AgentMemoryProvider>>,
    /// The session service through which a bound member's transcript is
    /// read and edited.
    pub(crate) transcript_edit_service:
        Option<Arc<dyn crate::memory::hygienist::TranscriptEditSessionService>>,
    /// The compaction floors of the mob's identity bridge.
    pub(crate) compaction_floors: Option<Arc<crate::identity_first::CompactionFloorRegistry>>,
}

impl MobEntry {
    /// The id of this entry's mob.
    pub(super) fn mob_id(&self) -> &MobId {
        self.mob_runtime.mob_id()
    }

    /// This mob's identity runtime, for the composition's own (non-owning)
    /// holders: they are back-edges and never keep the mob owned.
    pub(super) fn identity_runtime_ref(
        &self,
    ) -> Option<&Arc<crate::identity_first::IdentityRuntime>> {
        self.identity_first_context
            .get()
            .map(|context| &context.runtime)
    }

    /// This mob's identity runtime as an exported view carrying `owner`,
    /// the mob's public owner.
    pub(super) fn public_identity_runtime(
        &self,
        owner: &MobRuntime,
    ) -> Option<crate::identity_first::PublicIdentityRuntime> {
        self.identity_first_context.get().map(|context| {
            crate::identity_first::PublicIdentityRuntime::new(
                Arc::clone(&context.runtime),
                owner.clone(),
            )
        })
    }

    /// This mob's identity-first context as an exported view carrying
    /// `owner`, the mob's public owner.
    pub(super) fn public_identity_first_context(
        &self,
        owner: &MobRuntime,
    ) -> Option<crate::identity_first::PublicIdentityFirstContext> {
        self.identity_first_context.get().map(|context| {
            crate::identity_first::PublicIdentityFirstContext::new(
                Arc::clone(context),
                owner.clone(),
            )
        })
    }

    /// This mob's session bridge as an exported view carrying `owner`, the
    /// mob's public owner.
    pub(super) fn public_session_bridge(
        &self,
        owner: &MobRuntime,
    ) -> Option<crate::identity_first::PublicSessionBridge> {
        self.session_bridge.as_ref().map(|bridge| {
            crate::identity_first::PublicSessionBridge::new(Arc::clone(bridge), owner.clone())
        })
    }

    /// The desired-identity roster the console extends for this mob.
    pub(super) fn console_identity_roster(
        &self,
    ) -> Option<Arc<crate::identity_first::MutableRosterProvider>> {
        self.console_identity_roster
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The report of this mob's bootstrap edge reconciliation, if it ran.
    pub(super) async fn bootstrap_edges_report(
        &self,
    ) -> Option<UnifiedRuntimeReconcileEdgesReport> {
        self.bootstrap_edges_report.read().await.clone()
    }

    /// What a request that reaches this mob's identity plane needs beside
    /// the identity context, when the host composed it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn identity_request_services(&self) -> Option<IdentityRequestServices> {
        self.identity_request_services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The structural events of this mob's ledger that `query` selects, in
    /// cursor-ascending order.
    pub(super) async fn query_mob_events(
        &self,
        query: &super::EventQuery,
    ) -> Result<Vec<crate::types::MobStructuralEventEnvelope>, super::mob_events::MobEventsQueryError>
    {
        let events = self.mob_runtime.handle().events();
        super::mob_events::query_ledger_with_filter(&events, &self.mob_events, query).await
    }

    /// The cloneable topology seam of this mob, over `owner`'s handle.
    pub(super) fn topology_runtime_handle(
        &self,
        owner: &MobRuntime,
    ) -> crate::topology_control::TopologyRuntimeHandle {
        crate::topology_control::TopologyRuntimeHandle::new(
            owner.handle(),
            self.edge_discovery.clone(),
            Arc::clone(&self.managed_dynamic_edges),
            self.topology_controller.clone(),
            // An export: the handle goes to routers and the host.
            self.public_identity_first_context(owner),
            // A candidate launch's handle rewires nothing.
            !self.mob_runtime.speaks_for_composition(),
        )
    }
}

/// One mob of this host, resolved: what a handler that serves one mob works
/// on.
///
/// The only way to one is [`UnifiedRuntime::resolve_mob`], the seam where a
/// request that names no mob gets the default mob. A holder of this value
/// holds a mob: it has no unresolved state, and it does not say whether the
/// caller named the default mob or named none.
///
/// Its accessors answer for the resolved mob under the names the runtime's
/// own accessors have; those keep meaning the default mob. What is one per
/// process is reached through [`MobEntryRef::runtime`]; a method of the
/// runtime that means the default mob must not be called through it (the
/// names are listed in `default_mob_methods_tests`).
///
/// It borrows the runtime, so it lives for one request. A holder that
/// outlives the request (a stream, a spawned task) keeps the mob's id and
/// resolves again.
#[derive(Clone)]
pub(crate) struct MobEntryRef<'rt> {
    runtime: &'rt UnifiedRuntime,
    entry: EntryOf<'rt>,
}

/// Where a resolved mob's entry lives.
#[derive(Clone)]
enum EntryOf<'rt> {
    /// The default mob's entry: a field of the runtime state.
    Default(&'rt MobEntry),
    /// The entry of a mob the launch's mob set built for the host beside
    /// the default mob, from the runtime's table of such entries
    /// (`mob_entries`). The mob set owns the mob; the entry's own wrapper
    /// is what its exported views carry.
    Hosted(Arc<MobEntry>),
    /// A second entry beside the default mob's, lent by a test: the entry
    /// and the public owner of a mob another composition built. No
    /// production code builds one.
    #[cfg(test)]
    Lent {
        entry: &'rt MobEntry,
        owner: &'rt MobRuntime,
    },
}

/// Why a mob was not resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MobResolveError {
    /// This host does not have the mob `requested`. `default_mob` is the
    /// mob the host answers with for a caller that names none.
    NotHosted {
        requested: MobId,
        default_mob: MobId,
    },
    /// This host has the mob `requested`, and its mob set could not serve
    /// it for this request: the set's own error for the mob (a store that
    /// did not load is quarantined, and the error names the class of the
    /// failure).
    Unavailable {
        requested: MobId,
        error: MobUnavailable,
    },
}

/// The mob set's error for a mob of the host it could not serve. Two are
/// equal when they are one error, or read the same.
#[derive(Debug, Clone)]
pub(crate) struct MobUnavailable(Arc<meerkat_mob::MobError>);

impl MobUnavailable {
    /// The mob set's error.
    pub(crate) fn error(&self) -> &meerkat_mob::MobError {
        &self.0
    }
}

impl From<meerkat_mob::MobError> for MobUnavailable {
    fn from(error: meerkat_mob::MobError) -> Self {
        Self(Arc::new(error))
    }
}

impl PartialEq for MobUnavailable {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0.to_string() == other.0.to_string()
    }
}

impl Eq for MobUnavailable {}

/// The resolve error of a mob of the host that its mob set could not serve.
fn unavailable(requested: &MobId, error: meerkat_mob::MobError) -> MobResolveError {
    MobResolveError::Unavailable {
        requested: requested.clone(),
        error: error.into(),
    }
}

impl<'rt> MobEntryRef<'rt> {
    /// The resolved mob's entry.
    pub(super) fn entry(&self) -> &MobEntry {
        match &self.entry {
            EntryOf::Default(entry) => entry,
            EntryOf::Hosted(entry) => entry,
            #[cfg(test)]
            EntryOf::Lent { entry, .. } => entry,
        }
    }

    /// The resolved mob's public owner: what its exported views carry.
    fn owner(&self) -> &MobRuntime {
        match &self.entry {
            EntryOf::Default(_) => self.runtime.mob_runtime(),
            EntryOf::Hosted(entry) => &entry.mob_runtime,
            #[cfg(test)]
            EntryOf::Lent { owner, .. } => owner,
        }
    }

    /// Two entries in one runtime, for a test: `runtime` as the process
    /// part with its own default mob, and the entry of `second`'s mob
    /// resolved beside it. `second` is a composition of its own that lends
    /// its mob's entry and public owner; nothing in `runtime`'s state holds
    /// them, and no request resolves this mob.
    #[cfg(test)]
    pub(crate) fn second_entry_for_tests(
        runtime: &'rt UnifiedRuntime,
        second: &'rt UnifiedRuntime,
    ) -> Self {
        Self {
            runtime,
            entry: EntryOf::Lent {
                entry: second.default_mob(),
                owner: second.mob_runtime(),
            },
        }
    }

    /// The id of the resolved mob.
    pub(crate) fn mob_id(&self) -> &MobId {
        self.entry().mob_id()
    }

    /// Whether the resolved mob is the default mob: the mob the launch
    /// names.
    pub(crate) fn is_default(&self) -> bool {
        match &self.entry {
            EntryOf::Default(_) => true,
            EntryOf::Hosted(_) => false,
            #[cfg(test)]
            EntryOf::Lent { .. } => false,
        }
    }

    /// Attach `context` as the identity context of the resolved mob, for a
    /// mob the mob set built for the host beside the default mob: the mob's
    /// own identity runtime, composed by its host over this mob's handle
    /// ([`Self::mob_handle`]). The member wiring the mob was built with is
    /// bound to it, and it becomes the identity runtime the mob's plane
    /// answers; the default mob's and every other mob's are untouched.
    ///
    /// The default mob's identity context is installed by its runtime
    /// (`UnifiedRuntime::install_identity_first_context` and the bootstrap
    /// that follows): for the default mob this is refused.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "called by what composes the identity plane of a second mob; no launch does yet"
        )
    )]
    pub(crate) async fn attach_identity_first_context(
        &self,
        context: Arc<crate::identity_first::IdentityFirstRuntimeContext>,
    ) -> Result<(), crate::identity_first::IdentityRuntimeError> {
        match &self.entry {
            EntryOf::Hosted(entry) => entry.attach_identity_context(context).await,
            EntryOf::Default(_) => Err(crate::identity_first::IdentityRuntimeError::Internal(
                "the default mob's identity context is installed by its runtime".to_string(),
            )),
            #[cfg(test)]
            EntryOf::Lent { .. } => Err(crate::identity_first::IdentityRuntimeError::Internal(
                "a lent entry's identity context is installed by the runtime that lends it"
                    .to_string(),
            )),
        }
    }

    /// The runtime that hosts the resolved mob: what is one per process.
    pub(crate) fn runtime(&self) -> &'rt UnifiedRuntime {
        self.runtime
    }

    /// The resolved mob's runtime wrapper, as
    /// [`UnifiedRuntime::mob_runtime`] answers the default mob's.
    pub(crate) fn mob_runtime(&self) -> &MobRuntime {
        self.owner()
    }

    /// A public, owning handle to the resolved mob.
    pub(crate) fn mob_handle(&self) -> meerkat_mob::MobHandle {
        self.owner().handle()
    }

    /// The resolved mob's identity runtime as an exported view.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn identity_runtime(&self) -> Option<crate::identity_first::PublicIdentityRuntime> {
        self.entry().public_identity_runtime(self.owner())
    }

    /// The resolved mob's identity-first context as an exported view.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn identity_first_context(
        &self,
    ) -> Option<crate::identity_first::PublicIdentityFirstContext> {
        self.entry().public_identity_first_context(self.owner())
    }

    /// The resolved mob's session bridge as an exported view.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn session_bridge(&self) -> Option<crate::identity_first::PublicSessionBridge> {
        self.entry().public_session_bridge(self.owner())
    }

    /// The resolved mob's identity runtime, non-owning.
    pub(crate) fn identity_runtime_ref(
        &self,
    ) -> Option<&Arc<crate::identity_first::IdentityRuntime>> {
        self.entry().identity_runtime_ref()
    }

    /// Whether the launch speaks for the resolved mob's durable
    /// composition.
    pub(crate) fn speaks_for_composition(&self) -> bool {
        self.entry().mob_runtime.speaks_for_composition()
    }

    /// The storage durability resolution the resolved mob was composed
    /// with.
    pub(crate) fn resolved_storage(&self) -> Option<crate::storage_health::ResolvedStorageSummary> {
        self.entry().mob_runtime.resolved_storage()
    }

    /// The blob store the resolved mob's wrapper carries.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn binary_blob_store(&self) -> Option<Arc<dyn crate::blob_store::BinaryBlobStore>> {
        self.entry().mob_runtime.binary_blob_store()
    }

    /// The resolved mob's view of the WorkGraph store.
    pub(crate) fn workgraph_service(&self) -> Option<meerkat::WorkGraphService> {
        self.entry().workgraph_service.clone()
    }

    /// The admission that serializes the resolved mob's WorkGraph
    /// duplicate-binding guards: the request handlers and the agent tool
    /// plane of one mob take the same one, frozen at the mob's bootstrap
    /// with its WorkGraph service.
    pub(crate) fn workgraph_admission(
        &self,
    ) -> Arc<crate::workgraph_admission::WorkGraphAdmission> {
        self.entry().mob_runtime.workgraph_admission()
    }

    /// The report of the resolved mob's WorkGraph realm migration.
    pub(crate) fn workgraph_realm_migration(
        &self,
    ) -> Option<Arc<crate::workgraph_realm::WorkGraphRealmMigrationReport>> {
        self.entry().mob_runtime.workgraph_realm_migration()
    }

    /// The resolved mob's lossy WorkGraph fact hub.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn workgraph_fact_hub(&self) -> Option<crate::workgraph_events::WorkGraphFactHub> {
        self.entry().workgraph_fact_hub.clone()
    }

    /// The resolved mob's topology controller.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn topology_controller(&self) -> &crate::topology_control::TopologyController {
        &self.entry().topology_controller
    }

    /// The cloneable topology seam of the resolved mob.
    pub(crate) fn topology_runtime_handle(&self) -> crate::topology_control::TopologyRuntimeHandle {
        self.entry().topology_runtime_handle(self.owner())
    }

    /// The desired-identity roster the console extends for the resolved
    /// mob.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn console_identity_roster(
        &self,
    ) -> Option<Arc<crate::identity_first::MutableRosterProvider>> {
        self.entry().console_identity_roster()
    }

    /// The report of the resolved mob's bootstrap edge reconciliation.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn bootstrap_edges_report(
        &self,
    ) -> Option<UnifiedRuntimeReconcileEdgesReport> {
        self.entry().bootstrap_edges_report().await
    }

    /// The verdict of the resolved mob's actor-loop probe.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn actor_loop_health(&self) -> &Arc<crate::actor_loop_health::ActorLoopHealth> {
        &self.entry().actor_loop_health
    }

    /// The console events of the resolved mob: the host's own store for the
    /// default mob; for a mob the host serves beside it, THE ENTRY's own
    /// store, the one its console events are registered from. It is not
    /// asked of the host's stores by the mob's id: a request that resolved
    /// a mob records for that mob, whatever mob holds the id when its
    /// handler records.
    pub(crate) fn console_events(&self) -> super::ConsoleEventStore {
        let of_the_host = self.runtime.console_events();
        match &self.entry {
            EntryOf::Default(_) => of_the_host,
            EntryOf::Hosted(entry) => entry.console_log.store(&of_the_host, entry.mob_id()),
            // A lent entry is another composition's default mob: it holds
            // no store of its own, and its console events with this host
            // are the ones the id names.
            #[cfg(test)]
            EntryOf::Lent { entry, .. } => of_the_host.of_mob(Some(entry.mob_id())),
        }
    }

    /// The structural events of the resolved mob's ledger.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn mob_events_store(&self) -> MobEventsStore {
        self.entry().mob_events.clone()
    }

    /// The structural events of the resolved mob's ledger that `query`
    /// selects, as [`UnifiedRuntime::query_mob_events`] answers the default
    /// mob's.
    pub(crate) async fn query_mob_events(
        &self,
        query: &super::EventQuery,
    ) -> Result<Vec<crate::types::MobStructuralEventEnvelope>, super::mob_events::MobEventsQueryError>
    {
        self.entry().query_mob_events(query).await
    }

    /// The members of the resolved mob Core holds Broken, each with Core's
    /// reason, as [`UnifiedRuntime::broken_members`] answers the default
    /// mob's. `meta/status` reads the same members with their refusal codes
    /// (`crate::mob_handle_runtime::broken_member_facts_of`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn broken_members(&self) -> Vec<(meerkat_mob::AgentIdentity, String)> {
        crate::mob_handle_runtime::broken_members_of(&self.mob_handle()).await
    }

    /// The post-spawn hook that is told of the resolved mob's new members.
    /// The host's hook is the default mob's: it is told the members of the
    /// mob the launch names, and a member of another mob is none of its
    /// business, so another mob has no hook.
    pub(super) fn post_spawn_hook(&self) -> Option<&super::PostSpawnHook> {
        if self.is_default() {
            self.runtime.post_spawn_hook.as_ref()
        } else {
            None
        }
    }

    /// Spawn a member of the resolved mob, as [`UnifiedRuntime::spawn`]
    /// spawns a member of the default mob: the member is reserved under the
    /// resolved mob's identity runtime and joins its roster. The post-spawn
    /// hook is told only of a member of the default mob; a failure is
    /// reported on the host's error hook, which is one per process.
    pub(crate) async fn spawn(
        &self,
        spec: meerkat_mob::SpawnMemberSpec,
    ) -> Result<meerkat_mob::SpawnResult, crate::mob_handle_runtime::MobRuntimeError> {
        super::mob_ops::spawn_in(self, spec).await
    }

    /// Reset the resolved mob and run its own discovery again, as
    /// [`UnifiedRuntime::rediscover`] does for the default mob; `None` when
    /// the mob has no discovery. The discovered members are spawned as
    /// [`Self::spawn`] spawns one.
    pub(crate) async fn rediscover(
        &self,
    ) -> Result<Option<super::RediscoverReport>, crate::mob_handle_runtime::MobRuntimeError> {
        super::lifecycle::rediscover_in(self).await
    }

    /// Reconcile the resolved mob's dynamic peer edges over its own roster,
    /// edge policy, managed edges and topology controller, as
    /// [`UnifiedRuntime::reconcile_edges`] does for the default mob. An
    /// incomplete reconcile is reported on the host's error hook, which is
    /// one per process.
    pub(crate) async fn reconcile_edges(&self) -> UnifiedRuntimeReconcileEdgesReport {
        let report = self.topology_runtime_handle().reconcile().await;
        super::edge_reconcile::note_edge_reconcile(self.runtime, &report);
        report
    }

    /// What a peer needs to reach a member of the resolved mob, as
    /// [`UnifiedRuntime::local_member_peer_facts`] answers for a member of
    /// the default mob.
    pub(crate) async fn local_member_peer_facts(
        &self,
        member_id: &str,
    ) -> Result<super::cross_mob::LocalMemberPeerFacts, super::cross_mob::CrossMobError> {
        super::cross_mob::member_peer_facts_of(self, member_id).await
    }

    /// Ask the resolved mob's identity context for its roster again and
    /// reconcile the desired topology, as
    /// [`UnifiedRuntime::refresh_desired_topology`] does for the default
    /// mob; `None` when the mob has no identity context.
    pub(crate) async fn refresh_desired_topology(
        &self,
    ) -> Result<
        Option<crate::identity_first::RestoreFlowResult>,
        crate::identity_first::IdentityRuntimeError,
    > {
        match self.entry().identity_first_context.get() {
            Some(context) => context.refresh_desired_topology_tracked().await.map(Some),
            None => Ok(None),
        }
    }

    /// Wire a member of the resolved mob to a member of a peer mob, on both
    /// sides, as [`UnifiedRuntime::wire_cross_mob`] does for a member of
    /// the default mob. The local member is the resolved mob's own, and its
    /// link is held in that mob's own part of the link table; the peer mob
    /// is found in the host's contact directory and among the host's
    /// registered peer handles, which are one per process.
    pub(crate) async fn wire_cross_mob_with_identity_runtime(
        &self,
        local_member_id: &str,
        remote_member_id: &str,
        remote_mob_id: &str,
        local_identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<(), super::cross_mob::CrossMobError> {
        super::cross_mob::wire_cross_mob_in(
            self,
            local_member_id,
            remote_member_id,
            remote_mob_id,
            local_identity_runtime,
        )
        .await
    }

    /// Take back a wire between a member of the resolved mob and a member
    /// of a peer mob, on both sides, as [`UnifiedRuntime::unwire_cross_mob`]
    /// does for a member of the default mob.
    pub(crate) async fn unwire_cross_mob_with_identity_runtime(
        &self,
        local_member_id: &str,
        remote_member_id: &str,
        remote_mob_id: &str,
        local_identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<(), super::cross_mob::CrossMobError> {
        super::cross_mob::unwire_cross_mob_in(
            self,
            local_member_id,
            remote_member_id,
            remote_mob_id,
            local_identity_runtime,
        )
        .await
    }

    /// Send from a member of the resolved mob to a member of a peer mob, as
    /// [`UnifiedRuntime::send_cross_mob`] does for a member of the default
    /// mob: the sender is checked as a member of the resolved mob.
    pub(crate) async fn send_cross_mob_with_identity_runtime(
        &self,
        from_local_member: &str,
        remote_member_id: &str,
        remote_mob_id: &str,
        content: impl Into<meerkat_core::ContentInput>,
        local_identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<String, super::cross_mob::CrossMobError> {
        super::cross_mob::send_cross_mob_in(
            self,
            from_local_member,
            remote_member_id,
            remote_mob_id,
            content.into(),
            local_identity_runtime,
        )
        .await
    }

    /// Wire a member of the resolved mob to a peer, on the mob's side
    /// alone, as [`UnifiedRuntime::wire_local`] does for a member of the
    /// default mob. The member is looked up under `identity_runtime`, or
    /// else under the resolved mob's own identity runtime, and its link is
    /// held in that mob's own part of the link table.
    pub(crate) async fn wire_local_with_identity_runtime(
        &self,
        local_member_id: &str,
        remote_comms_name: &str,
        remote_peer_id: &str,
        remote_address: &str,
        remote_pubkey: Option<[u8; 32]>,
        identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<(), super::cross_mob::CrossMobError> {
        super::cross_mob::wire_local_in(
            self,
            local_member_id,
            remote_comms_name,
            remote_peer_id,
            remote_address,
            remote_pubkey,
            identity_runtime,
        )
        .await
    }

    /// Take back a one-sided wire of a member of the resolved mob, as
    /// [`UnifiedRuntime::unwire_local`] does for a member of the default
    /// mob.
    pub(crate) async fn unwire_local_with_identity_runtime(
        &self,
        local_member_id: &str,
        remote_comms_name: &str,
        remote_peer_id: &str,
        remote_address: &str,
        remote_pubkey: Option<[u8; 32]>,
        identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<(), super::cross_mob::CrossMobError> {
        super::cross_mob::unwire_local_in(
            self,
            local_member_id,
            remote_comms_name,
            remote_peer_id,
            remote_address,
            remote_pubkey,
            identity_runtime,
        )
        .await
    }

    /// What a request that reaches the resolved mob's identity plane needs
    /// beside the identity context: `None` until the host that composed the
    /// mob set it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn identity_request_services(&self) -> Option<IdentityRequestServices> {
        self.entry().identity_request_services()
    }

    /// The resolved mob as an owned value, for a holder that cannot borrow
    /// the runtime: the mob's id and owner handle, the values of the entry
    /// that are fixed once its runtime is composed, and the entry's shared
    /// cells. It copies no value that can change: what the owned form
    /// answers of those is read when it is asked.
    pub(crate) fn handles(&self) -> ResolvedMob {
        self.entry().resolved(self.is_default(), self.mob_runtime())
    }
}

impl MobEntry {
    /// This entry's mob as an owned value ([`ResolvedMob`]): `owner` is the
    /// mob's public owner, and `is_default_mob` says whether the mob is the
    /// one the launch names.
    pub(super) fn resolved(&self, is_default_mob: bool, owner: &MobRuntime) -> ResolvedMob {
        ResolvedMob {
            mob_id: self.mob_id().clone(),
            is_default_mob,
            mob_runtime: owner.clone(),
            console_identity_roster: Arc::clone(&self.console_identity_roster),
            identity_first_context: Arc::downgrade(&self.identity_first_context),
            workgraph_service: self.workgraph_service.clone(),
            workgraph_fact_hub: self.workgraph_fact_hub.clone(),
            mob_events: self.mob_events.clone(),
            edge_discovery: self.edge_discovery.clone(),
            managed_dynamic_edges: Arc::clone(&self.managed_dynamic_edges),
            topology_controller: self.topology_controller.clone(),
            console_events: self.console_log.kept(),
        }
    }
}

impl UnifiedRuntime {
    /// Resolve the mob a request addresses: the mob `named`, or the default
    /// mob when the request names none. This is the one seam where "no mob
    /// named" becomes the default mob; below it a mob is never optional.
    ///
    /// A mob this host does not have is [`MobResolveError::NotHosted`]. A
    /// mob the launch's mob set built for the host beside the default mob
    /// resolves to its own entry, and is loaded first when the set knows it
    /// and has not loaded it; a mob a member created is not a mob this host
    /// serves requests for. Every launch lets a request resolve the mobs
    /// its mob set built for the host (see `mob_entries`).
    pub(crate) async fn resolve_mob(
        &self,
        named: Option<&MobId>,
    ) -> Result<MobEntryRef<'_>, MobResolveError> {
        #[cfg(test)]
        {
            if let Some(second) = second_mob_in_tests::named(self, named) {
                return Ok(MobEntryRef::second_entry_for_tests(self, second));
            }
        }
        let default_mob = self.default_mob().mob_id();
        let Err(not_the_default) = default_mob_answers_for(default_mob, named) else {
            return Ok(self.default_mob_resolved());
        };
        let hosted = match named {
            Some(named) => self
                .other_mobs()
                .resolve(named)
                .await
                .map_err(|error| unavailable(named, error))?,
            None => None,
        };
        match hosted {
            Some(entry) => Ok(MobEntryRef {
                runtime: self,
                entry: EntryOf::Hosted(entry),
            }),
            None => Err(not_the_default),
        }
    }

    /// The default mob, resolved: what a method of the runtime that means
    /// the default mob hands to the form of its operation on a resolved
    /// mob.
    pub(super) fn default_mob_resolved(&self) -> MobEntryRef<'_> {
        MobEntryRef {
            runtime: self,
            entry: EntryOf::Default(self.default_mob()),
        }
    }

    /// The id of the mob this host serves for a request that names `named`:
    /// the default mob's for a request that names none. It builds nothing
    /// and loads nothing: it is the question of a caller that only needs to
    /// know that the mob is one this host serves, answered for another mob
    /// from what the mob set's records keep. It agrees with
    /// [`Self::resolve_mob`] outcome for outcome.
    pub(crate) async fn served_mob_id(
        &self,
        named: Option<&MobId>,
    ) -> Result<MobId, MobResolveError> {
        #[cfg(test)]
        {
            if let Some(second) = second_mob_in_tests::named(self, named) {
                return Ok(second.default_mob().mob_id().clone());
            }
        }
        let default_mob = self.default_mob().mob_id();
        let Err(not_the_default) = default_mob_answers_for(default_mob, named) else {
            return Ok(default_mob.clone());
        };
        if let Some(named) = named
            && self.other_mobs().serves(named).await
        {
            return Ok(named.clone());
        }
        Err(not_the_default)
    }

    /// The mob this host hosts beside its launch mob in whose reach `named`
    /// is: `named` itself, or the hosted mob whose member created it
    /// (through any number of mobs members created). `Ok(None)` for the
    /// launch mob and every mob in its own reach, and `Err` when the mob
    /// set's records do not decide it. Unlike [`Self::served_mob_id`] it
    /// does not ask whether requests resolve a hosted mob: it is the
    /// question of a judge that must hold whether or not they do. It loads
    /// no mob (`MobEntries::hosting`).
    pub(crate) async fn mob_hosting(&self, named: &MobId) -> Result<Option<MobId>, String> {
        self.other_mobs().hosting(named).await
    }

    /// The incarnation of the mob `served`, an id [`Self::served_mob_id`]
    /// answered, as that mob's events are kept under it. It loads nothing
    /// and builds no runtime: the default mob and a mob the mob set
    /// manages answer the incarnation their actor runs at, a mob that is
    /// not loaded answers the one the mob set's records keep for it, and
    /// `None` says that no record names it yet (the set has not read the
    /// record of a mob that is not loaded): a caller that selects by the
    /// incarnation answers nothing then.
    pub(crate) async fn served_mob_incarnation(
        &self,
        served: &MobId,
    ) -> Option<meerkat_core::service::MobIncarnationId> {
        #[cfg(test)]
        {
            if let Some(second) = second_mob_in_tests::named(self, Some(served)) {
                return Some(
                    MobEntryRef::second_entry_for_tests(self, second)
                        .mob_handle()
                        .incarnation(),
                );
            }
        }
        if self.default_mob().mob_id() == served {
            return Some(self.default_mob_resolved().mob_handle().incarnation());
        }
        self.other_mobs().incarnation_without_a_load(served).await
    }

    /// A resolver a request door keeps in its own state: it resolves the mob
    /// a request addresses without holding this runtime (see
    /// [`MobResolver`]). A door takes it when its router is built; what a
    /// mob resolved with it answers is read when it is asked.
    pub(crate) fn mob_resolver(&self) -> MobResolver {
        self.resolver_over(self.default_mob_resolved().handles())
    }

    /// [`Self::mob_resolver`] for what the runtime itself owns: a task of
    /// its own that serves requests (its control listener) and a part of
    /// its state that does (its keyed inject plane). It holds the default
    /// mob through the runtime's own wrapper, which is no public owner:
    /// what the runtime's state holds must not keep the mob publicly owned,
    /// or the composition would never see its last public owner go. It
    /// resolves the same mobs.
    pub(crate) fn mob_resolver_of_its_own_task(&self) -> MobResolver {
        let default_mob = self.default_mob();
        self.resolver_over(default_mob.resolved(true, &default_mob.mob_runtime))
    }

    /// A resolver that answers `default_resolved` for the default mob and
    /// this runtime's other mobs from its table.
    fn resolver_over(&self, default_resolved: ResolvedMob) -> MobResolver {
        let hosted_mobs = Arc::downgrade(self.other_mobs());
        // Test builds only: a runtime that serves a second mob hands its
        // doors a resolver that serves it too.
        #[cfg(test)]
        {
            if let Some(second) = second_mob_in_tests::of(self) {
                return MobResolver {
                    default_resolved,
                    hosted_mobs,
                    second_in_tests: None,
                }
                .serving_a_second_mob_in_tests(&second.mob_resolver());
            }
        }
        MobResolver {
            default_resolved,
            hosted_mobs,
            #[cfg(test)]
            second_in_tests: None,
        }
    }

    /// Give the default mob's entry what a request to its identity plane
    /// needs beside the identity context: the services the launch composed
    /// for the mob's identity runtime. The launch calls it once the mob's
    /// identity context is bootstrapped.
    pub(crate) fn set_identity_request_services(&self, services: IdentityRequestServices) {
        *self
            .default_mob()
            .identity_request_services
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(services);
    }
}

/// One mob of this host, resolved, as an owned value: what a request door
/// that holds no runtime works on.
///
/// It is the owned counterpart of [`MobEntryRef`]: the same mob under the
/// same names. A holder may keep it across any await and for as long as it
/// serves (a stream keeps one for its life), so it keeps only what cannot
/// change: the mob's id, whether it is the default mob, and the mob's
/// owner handle. What a host gives the mob later is read when it is asked
/// for, where it lives: the identity runtime from the mob's identity plane
/// (the one slot the mob's wrapper and its entry hold), the console roster
/// and the identity context its topology seam is built from, each from the
/// entry's own cell. So a door built before a host installed its identity
/// context answers the installed one.
///
/// It keeps nothing of the runtime's state alive. Once the mob is stopped
/// its owner handle answers as a stopped mob's does and it answers no
/// identity runtime: the stop detached it from the plane. (The borrowed
/// form reads the entry's identity context, which the entry holds until it
/// is dropped.)
///
/// It also holds what the mob's entry holds as a plain value: the mob's
/// WorkGraph service, its event store, its edge policy and its topology
/// controller. An entry is written in place only while its runtime is
/// composed, so those are fixed before any door exists.
#[derive(Clone)]
pub(crate) struct ResolvedMob {
    mob_id: MobId,
    #[cfg_attr(not(test), allow(dead_code))]
    is_default_mob: bool,
    /// The mob's public owner. It carries the mob's identity plane.
    mob_runtime: MobRuntime,
    /// The entry's roster cell, shared with the entry.
    console_identity_roster: ConsoleIdentityRosterCell,
    /// The entry's identity context cell. Held weakly: the context holds
    /// the mob's identity runtime, and a door that outlives its runtime
    /// keeps none of that alive.
    identity_first_context: std::sync::Weak<IdentityContextCell>,
    /// Fixed once the mob's runtime is composed.
    workgraph_service: Option<meerkat::WorkGraphService>,
    /// Fixed once the mob's runtime is composed: a handle to the hub the
    /// mob's one WorkGraph fact tail feeds. `None` for a mob that has no
    /// WorkGraph view.
    workgraph_fact_hub: Option<crate::workgraph_events::WorkGraphFactHub>,
    /// Fixed once the mob's runtime is composed: a handle to the one store.
    mob_events: MobEventsStore,
    /// Fixed once the mob's runtime is composed.
    edge_discovery: Option<Arc<dyn EdgeDiscovery>>,
    /// The entry's set of managed edges, shared with the entry.
    managed_dynamic_edges: Arc<tokio::sync::RwLock<BTreeSet<(String, String)>>>,
    /// A handle to the entry's topology controller.
    topology_controller: crate::topology_control::TopologyController,
    /// The entry's own console events, for a mob the host serves beside
    /// its default mob: a handle to the store the entry named when it was
    /// made. None for the default mob, whose console events are the host's
    /// own store.
    console_events: Option<super::ConsoleEventStore>,
}

impl ResolvedMob {
    /// The id of the resolved mob.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn mob_id(&self) -> &MobId {
        &self.mob_id
    }

    /// Whether the resolved mob is the default mob: the mob the launch
    /// names.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_default(&self) -> bool {
        self.is_default_mob
    }

    /// The resolved mob's runtime wrapper, as its public owner.
    pub(crate) fn mob_runtime(&self) -> &MobRuntime {
        &self.mob_runtime
    }

    /// A public, owning handle to the resolved mob.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn mob_handle(&self) -> meerkat_mob::MobHandle {
        self.mob_runtime.handle()
    }

    /// The resolved mob's identity runtime as an exported view: the one
    /// attached to the mob now.
    pub(crate) fn identity_runtime(&self) -> Option<crate::identity_first::PublicIdentityRuntime> {
        self.identity_runtime_ref().map(|identity_runtime| {
            crate::identity_first::PublicIdentityRuntime::new(
                identity_runtime,
                self.mob_runtime.clone(),
            )
        })
    }

    /// The resolved mob's identity runtime, non-owning: the one attached to
    /// the mob now.
    pub(crate) fn identity_runtime_ref(
        &self,
    ) -> Option<Arc<crate::identity_first::IdentityRuntime>> {
        self.mob_runtime.installed_identity_runtime()
    }

    /// The desired-identity roster the console extends for the resolved
    /// mob: the one the host gave the mob's entry, as it is now.
    pub(crate) fn console_identity_roster(
        &self,
    ) -> Option<Arc<crate::identity_first::MutableRosterProvider>> {
        self.console_identity_roster
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The resolved mob's view of the WorkGraph store. It is fixed at the
    /// mob's bootstrap and nothing replaces it afterwards, so the owned
    /// form holds it.
    pub(crate) fn workgraph_service(&self) -> Option<meerkat::WorkGraphService> {
        self.workgraph_service.clone()
    }

    /// The hub of the resolved mob's WorkGraph facts: the one its fact
    /// stream subscribes to. It is fixed at the mob's bootstrap and nothing
    /// replaces it afterwards, so the owned form holds a handle to it. A
    /// mob that has no WorkGraph view has none.
    #[allow(
        dead_code,
        reason = "read by the WorkGraph fact stream once it serves the mob a request names"
    )]
    pub(crate) fn workgraph_fact_hub(&self) -> Option<crate::workgraph_events::WorkGraphFactHub> {
        self.workgraph_fact_hub.clone()
    }

    /// The entry's own console events, for a mob the host serves beside its
    /// default mob: the store its entry was built with, never one asked of
    /// the host's stores by the mob's id. `None` for the default mob ALONE,
    /// whose console events are the host's own store: every entry of the
    /// table holds its store from its construction.
    #[allow(
        dead_code,
        reason = "read by the Console door once it takes a served mob's console events from the mob it resolved"
    )]
    pub(crate) fn console_events_of_its_entry(&self) -> Option<super::ConsoleEventStore> {
        self.console_events.clone()
    }

    /// The structural events of the resolved mob's ledger. The store is
    /// made with the mob's entry and nothing replaces it afterwards, so the
    /// owned form holds a handle to it.
    pub(crate) fn mob_events_store(&self) -> MobEventsStore {
        self.mob_events.clone()
    }

    /// The cloneable topology seam of the resolved mob, built when it is
    /// asked for from the identity context installed then, as the
    /// runtime's own seam is: a mob resolved before a host installed its
    /// identity context answers that context's topology. Once the mob's
    /// runtime is gone the seam carries no identity context.
    pub(crate) fn topology_runtime_handle(&self) -> crate::topology_control::TopologyRuntimeHandle {
        let identity_context = self
            .identity_first_context
            .upgrade()
            .and_then(|cell| cell.get().cloned())
            .map(|context| {
                crate::identity_first::PublicIdentityFirstContext::new(
                    context,
                    self.mob_runtime.clone(),
                )
            });
        crate::topology_control::TopologyRuntimeHandle::new(
            self.mob_runtime.handle(),
            self.edge_discovery.clone(),
            Arc::clone(&self.managed_dynamic_edges),
            self.topology_controller.clone(),
            identity_context,
            // A candidate launch's handle rewires nothing.
            !self.mob_runtime.speaks_for_composition(),
        )
    }
}

/// The resolver of a request door that holds no runtime: the console's
/// request door and the stream routes keep a clone in their state.
///
/// It has the contract of [`UnifiedRuntime::resolve_mob`] and
/// [`UnifiedRuntime::served_mob_id`]: a request that names no mob and one
/// that names the default mob's id get the same mob, a mob the launch's
/// mob set built for the host beside it resolves as it does there, and any
/// other id is a mob this host does not have.
///
/// It holds the default mob as a [`ResolvedMob`]: no copy of a value that
/// can change, so it never disagrees with the runtime about what the mob
/// answers. Stopping the runtime does not wait for resolvers: afterwards
/// the default mob's owner handle answers as a stopped mob's does. It holds
/// the runtime's table of the other mobs' entries weakly: once the runtime
/// is gone, no other mob resolves.
#[derive(Clone)]
pub(crate) struct MobResolver {
    default_resolved: ResolvedMob,
    hosted_mobs: std::sync::Weak<super::mob_entries::MobEntries>,
    /// Test builds only: a second mob this resolver serves, with the record
    /// of what the resolver was asked
    /// ([`MobResolver::serving_a_second_mob_in_tests`]). A runtime's own
    /// resolver has none.
    #[cfg(test)]
    second_in_tests: Option<SecondMobInTests>,
}

impl MobResolver {
    /// Resolve the mob a request addresses to its handles: the mob `named`,
    /// or the default mob when the request names none.
    pub(crate) async fn resolve(
        &self,
        named: Option<&MobId>,
    ) -> Result<ResolvedMob, MobResolveError> {
        #[cfg(test)]
        {
            if let Some(second) = self.second_mob_in_tests(named, true) {
                return Ok(second);
            }
        }
        let Err(not_the_default) = default_mob_answers_for(&self.default_resolved.mob_id, named)
        else {
            return Ok(self.default_resolved.clone());
        };
        let hosted = match (named, self.hosted_mobs.upgrade()) {
            (Some(named), Some(other_mobs)) => other_mobs
                .resolve(named)
                .await
                .map_err(|error| unavailable(named, error))?,
            _ => None,
        };
        match hosted {
            Some(entry) => Ok(entry.resolved(false, &entry.mob_runtime)),
            None => Err(not_the_default),
        }
    }

    /// Every mob this host serves other than the default mob, loaded or
    /// not, as the mob set's records know it now, in id order
    /// ([`HostedMobListing`]). It loads no mob, opens no mob store and
    /// builds nothing, and it is no resolve: a door that lists the host's
    /// mobs reads it. A mob a member created is never listed. A host of one
    /// mob answers an empty list.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "read by a door that lists the host's mobs; none calls it yet"
        )
    )]
    pub(crate) async fn hosted_mobs(&self) -> Vec<HostedMobListing> {
        #[cfg(test)]
        {
            if let Some(second) = self.second_in_tests.as_ref() {
                return vec![HostedMobListing {
                    mob_id: second.resolved.mob_id.clone(),
                    provenance: meerkat_mob::MobProvenance::Host,
                    phase: if second.loaded {
                        super::mob_entries::HostedMobPhase::Loaded
                    } else {
                        super::mob_entries::HostedMobPhase::NotLoaded
                    },
                    incarnation: second
                        .loaded
                        .then(|| second.resolved.mob_runtime.handle().incarnation()),
                }];
            }
        }
        match self.hosted_mobs.upgrade() {
            Some(other_mobs) => other_mobs.listing().await,
            None => Vec::new(),
        }
    }

    /// Every mob the host's mob set lists beside its launch mob, each with
    /// the mob the host created in whose reach it is: itself, or the mob
    /// the host created whose member made it, through any number of mobs
    /// members made. `Ok(None)` for a mob in the launch mob's own reach (a
    /// mob a member of the launch mob made, at any depth) and for one no
    /// member and no host made; `Err` for a mob whose reach the set's
    /// records do not decide. It loads no mob
    /// (`MobEntries::hosting_of_every_mob`).
    ///
    /// The outer `Err` says that the set's records could not be read at
    /// all, or that the runtime behind this resolver is gone: a door that
    /// reads the host's records for a caller then knows of no mob whose
    /// reach it could judge.
    pub(crate) async fn hosting_of_every_mob(
        &self,
    ) -> Result<Vec<(MobId, Result<Option<MobId>, String>)>, String> {
        match self.hosted_mobs.upgrade() {
            Some(other_mobs) => other_mobs.hosting_of_every_mob().await,
            None => Err("the runtime of this resolver is gone".to_string()),
        }
    }

    /// Whether the launch of this host declared its default mob to its mob
    /// set (`MobEntries::launch_declared_its_mob_to_a_mob_set`). A host
    /// whose launch did not has no mob of its own beside its launch mob,
    /// and [`Self::hosting_of_every_mob`] lists none for it: `Some(false)`
    /// tells that host from one whose mob set lists no mob now. `None`
    /// where it cannot be told: the runtime behind this resolver is gone,
    /// or its table has no host yet.
    pub(crate) fn launch_declared_its_mob_to_a_mob_set(&self) -> Option<bool> {
        self.hosted_mobs
            .upgrade()?
            .launch_declared_its_mob_to_a_mob_set()
    }

    /// Whether the host serves a mob beside its default mob, in three
    /// answers ([`super::mob_entries::OtherMobsServed`]): another mob where
    /// [`Self::hosted_mobs`] lists one; "not known" where that cannot be
    /// told (a runtime that is going or gone, a table with no host yet, an
    /// inventory that cannot be read) and where the listing names none but
    /// the mob set knows a mob that may be the host's (one whose creation
    /// none of its records tells as another's, or a store it keeps
    /// quarantined); else the default mob alone. It loads no mob and opens
    /// no mob store. A door that reads a record of no known mob as the
    /// default mob's asks it first.
    pub(crate) async fn other_mobs_served(&self) -> super::mob_entries::OtherMobsServed {
        #[cfg(test)]
        {
            if self.second_in_tests.is_some() {
                return super::mob_entries::OtherMobsServed::AnotherMob;
            }
        }
        match self.hosted_mobs.upgrade() {
            Some(other_mobs) => other_mobs.serves_beside_the_launch_mob().await,
            None => super::mob_entries::OtherMobsServed::NotKnown,
        }
    }

    /// Whether a request resolves a mob the mob set built for the host
    /// beside the default mob: the runtime lives and has not begun to shut
    /// down. Where it answers `false` every id but the default mob's is
    /// refused, whatever mobs the set holds. It asks the mob set nothing
    /// and loads nothing.
    pub(crate) fn resolves_host_mobs(&self) -> bool {
        self.hosted_mobs
            .upgrade()
            .is_some_and(|other_mobs| other_mobs.answers_requests())
    }

    /// Where the identity plane of the resolved mob `mob` stands now, read
    /// without waiting: it asks the mob set nothing, loads nothing and
    /// waits for no composition. A door that must not act on a mob's
    /// members while the mob's identity plane is still owed reads it here.
    pub(crate) fn identity_plane_standing(
        &self,
        mob: &ResolvedMob,
    ) -> super::IdentityPlaneStanding {
        if mob.identity_runtime_ref().is_some() {
            return super::IdentityPlaneStanding::Attached;
        }
        let owed = !mob.is_default_mob
            && self
                .hosted_mobs
                .upgrade()
                .is_some_and(|other_mobs| other_mobs.composes_identities());
        if owed {
            super::IdentityPlaneStanding::Owed
        } else {
            super::IdentityPlaneStanding::NotOwed
        }
    }

    /// The mob `mob_id` when the mob set manages it now: the default mob,
    /// or a mob the set built for the host whose entry the runtime holds.
    /// `None` for a mob the set knows and has not loaded, for one it is
    /// loading, and for an id no mob of the host has. It asks the mob set
    /// nothing: it never loads a mob and never waits behind a load, and it
    /// is no resolve.
    pub(crate) fn resolve_if_loaded(&self, mob_id: &MobId) -> Option<ResolvedMob> {
        if mob_id == &self.default_resolved.mob_id {
            return Some(self.default_resolved.clone());
        }
        #[cfg(test)]
        {
            if let Some(second) = self
                .second_in_tests
                .as_ref()
                .filter(|second| &second.resolved.mob_id == mob_id)
            {
                return second.loaded.then(|| second.resolved.clone());
            }
        }
        let entry = self.hosted_mobs.upgrade()?.loaded(mob_id)?;
        Some(entry.resolved(false, &entry.mob_runtime))
    }

    /// When the mob that stands under the id `mob_id` was created, in
    /// milliseconds of the host's clock: the time its journal keeps for its
    /// first event (`MobEntries::created_at_ms`). It loads no mob: a mob
    /// that is known and not loaded is read from its store's file. `None`
    /// when no mob stands under the id, and when the mob's journal cannot
    /// be read. The default mob stands for as long as its host does and is
    /// never created under its id again: it answers the earliest time.
    ///
    /// A door that shows a record made for a mob by the mob's id asks it:
    /// the record is the standing mob's when it was made at or after this
    /// time.
    pub(crate) async fn standing_mob_created_at_ms(&self, mob_id: &MobId) -> Option<u64> {
        if mob_id == &self.default_resolved.mob_id {
            return Some(0);
        }
        #[cfg(test)]
        {
            if let Some(second) = self
                .second_in_tests
                .as_ref()
                .filter(|second| &second.resolved.mob_id == mob_id)
            {
                let first = second
                    .resolved
                    .mob_runtime
                    .handle()
                    .poll_events(0, 1)
                    .await
                    .ok()?;
                return u64::try_from(first.first()?.timestamp.timestamp_millis()).ok();
            }
        }
        self.hosted_mobs.upgrade()?.created_at_ms(mob_id).await
    }

    /// The incarnation of the mob that stands under the id `mob_id` now,
    /// read without loading it: the default mob and a mob the mob set
    /// manages answer the one their actor runs at, and a mob that is not
    /// loaded answers the one the set's records keep for it
    /// (`MobEntries::incarnation_without_a_load`). `None` where no mob of
    /// the host stands under the id, where the set's records do not say
    /// its incarnation (a mob that is not loaded whose record the set has
    /// not read, a load under way), and once the runtime is going or gone.
    /// It is no resolve and never loads a mob to learn it: a door that
    /// serves a record only to the mob it was written for compares the
    /// record's incarnation with this one, and serves nothing on `None`.
    pub(crate) async fn standing_incarnation(
        &self,
        mob_id: &MobId,
    ) -> Option<meerkat_core::service::MobIncarnationId> {
        if mob_id == &self.default_resolved.mob_id {
            return Some(self.default_resolved.mob_handle().incarnation());
        }
        #[cfg(test)]
        {
            if let Some(second) = self
                .second_in_tests
                .as_ref()
                .filter(|second| &second.resolved.mob_id == mob_id)
            {
                return Some(second.resolved.mob_handle().incarnation());
            }
        }
        self.hosted_mobs
            .upgrade()?
            .incarnation_without_a_load(mob_id)
            .await
    }

    /// The id of the mob this host serves for a request that names `named`,
    /// without building or loading anything.
    pub(crate) async fn served_mob_id(
        &self,
        named: Option<&MobId>,
    ) -> Result<MobId, MobResolveError> {
        #[cfg(test)]
        {
            if let Some(second) = self.second_mob_in_tests(named, false) {
                return Ok(second.mob_id().clone());
            }
        }
        let Err(not_the_default) = default_mob_answers_for(&self.default_resolved.mob_id, named)
        else {
            return Ok(self.default_resolved.mob_id.clone());
        };
        if let (Some(named), Some(other_mobs)) = (named, self.hosted_mobs.upgrade())
            && other_mobs.serves(named).await
        {
            return Ok(named.clone());
        }
        Err(not_the_default)
    }

    /// Remove the topology state of the mob `mob_id`, a mob this host
    /// served beside its default mob and its mob set destroyed: the state
    /// file of every incarnation of that id and its lock, beside where the
    /// mob's store was. What destroys a mob of the host calls it after the
    /// mob set's destroy answered, with the resolver its door keeps.
    ///
    /// The entry the runtime held for the mob goes first, so its controller
    /// has given the file up and writes nothing after the removal. A mob
    /// the mob set still hosts under that id (the destroy did not happen,
    /// or a new mob took the id) keeps its state: nothing is removed and
    /// the call fails. A host that keeps no topology state, a mob it kept
    /// in memory and a runtime that is gone have nothing to remove; a
    /// second call finds nothing. None of these is a failure.
    pub(crate) async fn release_topology_state_of_a_destroyed_mob(
        &self,
        mob_id: &MobId,
    ) -> std::io::Result<()> {
        match self.hosted_mobs.upgrade() {
            Some(other_mobs) => other_mobs.release_topology_state(mob_id).await,
            None => Ok(()),
        }
    }

    /// Release the agent memory of the mob `mob_id`, a mob this host served
    /// beside its default mob and its mob set destroyed: the one directory
    /// of that mob below the launch's memory root, with every file in it.
    /// What destroys a mob of the host calls it after the mob set's destroy
    /// answered, beside [`Self::release_topology_state_of_a_destroyed_mob`],
    /// with the resolver its door keeps.
    ///
    /// The mob set's records decide, not the caller: a mob the set hosts
    /// under that id (the destroy did not happen, a new mob took the id, or
    /// the mob is only not loaded) keeps its memory, nothing is released,
    /// and the call answers exactly that
    /// ([`DestroyedMobMemory::KeptForTheMobUnderTheId`]): no failure, and
    /// not "done". A call that names the default mob, whose memory is the
    /// launch's own, fails. A release that cannot be finished fails
    /// too: what it left is removed by the release before the next create
    /// under the id, or at the launch's next start. A launch that keeps no
    /// memory for the mobs it hosts and a mob that never used its memory
    /// have nothing to release, and a second call finds nothing: none of
    /// these is a failure. What cannot be read is one: a runtime that is
    /// gone released nothing, and the call says so instead of "done", as
    /// does a launch that declared the memory and has not bound its
    /// release. It loads no mob and opens no memory.
    pub(crate) async fn release_memory_of_a_destroyed_mob(
        &self,
        mob_id: &MobId,
    ) -> std::io::Result<DestroyedMobMemory> {
        if mob_id == &self.default_resolved.mob_id {
            return Err(std::io::Error::other(format!(
                "the mob {mob_id} is the default mob: its memory is the launch's own"
            )));
        }
        match self.hosted_mobs.upgrade() {
            Some(other_mobs) => other_mobs.release_memory(mob_id).await,
            None => Err(std::io::Error::other(format!(
                "the runtime that served the mob {mob_id} is gone: nothing of its memory \
                 was released"
            ))),
        }
    }
}

/// Test builds only: the second mob a fixture resolver serves, and the mob
/// each question of that resolver named.
#[cfg(test)]
#[derive(Clone)]
struct SecondMobInTests {
    /// The second mob's handles, as a mob other than the default mob.
    resolved: ResolvedMob,
    /// Whether the fixture's second mob stands as loaded. A mob that is
    /// served and not loaded resolves (a resolve is what loads a mob) and
    /// is served, and is no mob "if loaded"
    /// ([`MobResolver::not_loaded_in_tests`]).
    loaded: bool,
    /// The mob each `resolve` call named, in call order.
    resolved_for: Arc<std::sync::Mutex<Vec<Option<MobId>>>>,
    /// The mob each `served_mob_id` call named, in call order.
    asked_whether_served: Arc<std::sync::Mutex<Vec<Option<MobId>>>>,
}

#[cfg(test)]
impl MobResolver {
    /// Test builds only: this resolver as a door holds it once the runtime
    /// it was taken from is gone: its table of the host's other mobs is no
    /// longer there to ask.
    pub(crate) fn whose_runtime_is_gone_in_tests(mut self) -> Self {
        self.hosted_mobs = std::sync::Weak::new();
        self
    }

    /// Test builds only: this resolver, serving the default mob of `second`
    /// (another runtime's resolver) too, as a mob other than the default
    /// mob, and recording the mob each of its questions names. Its clones
    /// share the record. No host can build one: a runtime's own resolver
    /// serves its default mob alone.
    pub(crate) fn serving_a_second_mob_in_tests(mut self, second: &MobResolver) -> Self {
        let mut resolved = second.default_resolved.clone();
        resolved.is_default_mob = false;
        self.second_in_tests = Some(SecondMobInTests {
            resolved,
            loaded: true,
            resolved_for: Arc::default(),
            asked_whether_served: Arc::default(),
        });
        self
    }

    /// Test builds only: this fixture resolver with its second mob served
    /// and NOT LOADED, as the mob set knows a mob of the host it has not
    /// loaded: the listing shows it not loaded, "if loaded" answers no mob
    /// for it, and a resolve, which is what loads a mob, still answers it
    /// and is recorded.
    pub(crate) fn not_loaded_in_tests(mut self) -> Self {
        if let Some(second) = self.second_in_tests.as_mut() {
            second.loaded = false;
        }
        self
    }

    /// Test builds only: the mob each `resolve` call of this resolver
    /// named, in call order (`None` for a call that named no mob). Empty
    /// for a resolver that serves no second mob.
    pub(crate) fn resolved_for_in_tests(&self) -> Vec<Option<MobId>> {
        self.second_in_tests
            .as_ref()
            .map(|second| {
                second
                    .resolved_for
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            })
            .unwrap_or_default()
    }

    /// Test builds only: the mob each `served_mob_id` call of this resolver
    /// named, in call order (`None` for a call that named no mob). Empty
    /// for a resolver that serves no second mob.
    pub(crate) fn asked_whether_served_in_tests(&self) -> Vec<Option<MobId>> {
        self.second_in_tests
            .as_ref()
            .map(|second| {
                second
                    .asked_whether_served
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            })
            .unwrap_or_default()
    }

    /// Records the mob a question named and answers the second mob when
    /// the question named its id. `resolves`: the question was `resolve`.
    fn second_mob_in_tests(&self, named: Option<&MobId>, resolves: bool) -> Option<ResolvedMob> {
        let second = self.second_in_tests.as_ref()?;
        let record = if resolves {
            &second.resolved_for
        } else {
            &second.asked_whether_served
        };
        record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(named.cloned());
        (named == Some(&second.resolved.mob_id)).then(|| second.resolved.clone())
    }
}

#[cfg(test)]
impl UnifiedRuntime {
    /// Test builds only: the task that follows the mob set for the
    /// runtime's other mobs stops, so their table changes only when a
    /// request asks for a pass.
    pub(super) async fn hold_the_follower_of_host_mobs_in_tests(&self) {
        self.other_mobs().hold_the_follower_in_tests().await;
    }

    /// Test builds only: a destroy of one of the runtime's other mobs asks
    /// their table for no pass from here on.
    pub(super) fn destroys_of_host_mobs_ask_for_no_pass_in_tests(&self) {
        self.other_mobs().destroys_ask_for_no_pass_in_tests();
    }

    /// Test builds only: the next pass of the runtime's table of other mobs
    /// that takes a mob's wiring stands still before it puts that mob's
    /// entry in.
    pub(super) fn stand_the_next_pass_of_host_mobs_still_in_tests(&self) {
        self.other_mobs().stand_the_next_pass_still_in_tests();
    }

    /// Test builds only: wait until a pass of that table stands still.
    pub(super) async fn a_pass_of_host_mobs_stands_still_in_tests(&self) {
        self.other_mobs().a_pass_stands_still_in_tests().await;
    }

    /// Test builds only: the pass that stands still goes on.
    pub(super) fn end_the_standstill_of_host_mobs_in_tests(&self) {
        self.other_mobs().end_the_standstill_in_tests();
    }

    /// Test builds only: how many passes of that table run on tasks of
    /// their own now.
    pub(super) fn pass_tasks_of_host_mobs_running_in_tests(&self) -> usize {
        self.other_mobs().pass_tasks_running_in_tests()
    }

    /// Test builds only: how many entries the passes of that table put in
    /// so far.
    pub(super) fn entries_put_in_by_passes_of_host_mobs_in_tests(&self) -> usize {
        self.other_mobs().entries_put_in_by_passes_in_tests()
    }

    /// Test builds only: the ids of the mobs whose entries the runtime's
    /// table of other mobs holds now.
    pub(crate) fn mob_entries_in_tests(&self) -> Vec<MobId> {
        self.other_mobs()
            .held()
            .iter()
            .map(|entry| entry.mob_id().clone())
            .collect()
    }

    /// Test builds only: the entry the runtime holds for the mob `named`.
    pub(super) fn mob_entry_in_tests(&self, named: &MobId) -> Option<Arc<MobEntry>> {
        self.other_mobs().entry_in_tests(named)
    }

    /// Test builds only: an entry over the mob `named` as the mob set
    /// manages it now, built as the runtime builds one and kept nowhere:
    /// its start-time read has not run.
    pub(super) async fn mob_entry_before_its_read_in_tests(
        &self,
        named: &MobId,
    ) -> Option<Arc<MobEntry>> {
        self.other_mobs()
            .entry_before_its_read_in_tests(named)
            .await
    }
}

/// Test builds only: a runtime that serves a second mob.
///
/// A second composition, built from its own definition, is lent to a host
/// runtime. The host's seam then answers that mob by its id as a mob other
/// than the default mob, from the lent composition's own entry (its
/// handle, its identity context, its identity request services); a request
/// that names no mob keeps the host's default mob. Nothing in the host's
/// state holds the lent entry, and no build outside tests has this module:
/// a second mob resolves on no host.
#[cfg(test)]
mod second_mob_in_tests {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use meerkat_mob::MobId;

    use super::UnifiedRuntime;

    /// The lent runtimes, by the id of the host's default mob. A lent
    /// runtime is leaked: it lives to the end of the test process, so an
    /// entry borrowed from it outlives every host.
    static LENT: Mutex<BTreeMap<String, &'static UnifiedRuntime>> = Mutex::new(BTreeMap::new());

    fn lent() -> MutexGuard<'static, BTreeMap<String, &'static UnifiedRuntime>> {
        LENT.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The runtime lent to `host`, if one was.
    pub(super) fn of(host: &UnifiedRuntime) -> Option<&'static UnifiedRuntime> {
        lent().get(host.default_mob().mob_id().as_str()).copied()
    }

    /// The runtime lent to `host`, when `named` is the id of its mob.
    pub(super) fn named(
        host: &UnifiedRuntime,
        named: Option<&MobId>,
    ) -> Option<&'static UnifiedRuntime> {
        let named = named?;
        of(host).filter(|second| second.default_mob().mob_id() == named)
    }

    impl UnifiedRuntime {
        /// Test builds only: make this runtime serve the mob of `second`
        /// too, as a mob other than its default mob. From then on
        /// [`UnifiedRuntime::resolve_mob`] and
        /// [`UnifiedRuntime::served_mob_id`] answer that mob by its id, and
        /// a resolver taken afterwards ([`UnifiedRuntime::mob_resolver`],
        /// so every router built afterwards) serves it too. `second` is a
        /// whole composition with its own definition: give it its identity
        /// context and its identity request services before it is lent.
        /// The reference handed back is the lent runtime, which the test
        /// shuts down itself.
        pub(crate) fn serve_a_second_mob_in_tests(
            &self,
            second: UnifiedRuntime,
        ) -> &'static UnifiedRuntime {
            let second: &'static UnifiedRuntime = Box::leak(Box::new(second));
            lent().insert(self.default_mob().mob_id().to_string(), second);
            second
        }
    }
}

/// Whether the default mob answers a request that names `named`: it does
/// for a request that names no mob and for one that names its id. Any other
/// id is a mob this host does not have.
fn default_mob_answers_for(
    default_mob: &MobId,
    named: Option<&MobId>,
) -> Result<(), MobResolveError> {
    match named {
        Some(requested) if requested != default_mob => Err(MobResolveError::NotHosted {
            requested: requested.clone(),
            default_mob: default_mob.clone(),
        }),
        Some(_) | None => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use super::super::UnifiedRuntime;
    use super::super::identity_install_phase_tests::runtime;

    /// A resolver that serves a second mob in a test build answers that mob
    /// by its id as a mob other than the default mob, with the second
    /// runtime's handles; it answers the default mob and an id no mob has
    /// as a runtime's own resolver does; and it records the mob each
    /// question named, for its clones too. A runtime's own resolver serves
    /// its default mob alone and records nothing.
    #[tokio::test]
    async fn a_resolver_that_serves_a_second_mob_in_tests_answers_it_and_records_its_questions() {
        let temp = tempfile::tempdir().expect("tempdir");
        let second_temp = tempfile::tempdir().expect("tempdir");
        let first = runtime(&temp).await;
        let second = runtime(&second_temp).await;
        let first_id = first.mob_handle().mob_id().clone();
        let second_id = second.mob_handle().mob_id().clone();
        let absent = meerkat_mob::MobId::from("a-mob-this-host-does-not-have");
        let plain = first.mob_resolver();
        let resolver = first
            .mob_resolver()
            .serving_a_second_mob_in_tests(&second.mob_resolver());
        let door = resolver.clone();

        let by_no_id = door.resolve(None).await.ok();
        assert!(
            by_no_id
                .as_ref()
                .is_some_and(|mob| mob.is_default() && mob.mob_id() == &first_id),
            "a request that names no mob resolves the default mob"
        );
        let by_its_id = door.resolve(Some(&first_id)).await.ok();
        assert!(
            by_its_id
                .as_ref()
                .is_some_and(|mob| mob.is_default() && mob.mob_id() == &first_id),
            "the default mob resolves by its id"
        );
        let the_second = door.resolve(Some(&second_id)).await.ok();
        assert!(
            the_second.as_ref().is_some_and(|mob| {
                !mob.is_default()
                    && mob.mob_id() == &second_id
                    && mob.mob_handle().mob_id() == &second_id
            }),
            "the second mob resolves by its id, as a mob other than the default mob, with \
             the second runtime's handles"
        );
        assert!(
            matches!(
                door.resolve(Some(&absent)).await,
                Err(super::MobResolveError::NotHosted { .. })
            ),
            "an id no mob has does not resolve"
        );
        assert_eq!(
            door.served_mob_id(None).await.ok(),
            Some(first_id.clone()),
            "a request that names no mob is served by the default mob"
        );
        assert_eq!(
            door.served_mob_id(Some(&second_id)).await.ok(),
            Some(second_id.clone()),
            "the second mob is served by its id"
        );
        assert!(
            door.served_mob_id(Some(&absent)).await.is_err(),
            "an id no mob has is not served"
        );

        // The records, in call order.
        let resolved_for = resolver.resolved_for_in_tests();
        assert_eq!(
            resolved_for.iter().map(Option::as_ref).collect::<Vec<_>>(),
            [None, Some(&first_id), Some(&second_id), Some(&absent)],
            "the record of the resolve calls, shared with the clone that was asked"
        );
        let asked_whether_served = resolver.asked_whether_served_in_tests();
        assert_eq!(
            asked_whether_served
                .iter()
                .map(Option::as_ref)
                .collect::<Vec<_>>(),
            [None, Some(&second_id), Some(&absent)],
            "the record of the served-mob questions"
        );

        assert!(
            plain.resolve(Some(&second_id)).await.is_err(),
            "a runtime's own resolver does not serve another runtime's mob"
        );
        assert!(
            plain.resolved_for_in_tests().is_empty()
                && plain.asked_whether_served_in_tests().is_empty(),
            "a runtime's own resolver records nothing"
        );

        first.shutdown().await;
        second.shutdown().await;
    }

    /// The default mob's entry is the launch mob's: its wrapper and its
    /// identity plane are that mob's, and the default accessors answer from
    /// it.
    #[tokio::test]
    async fn a_runtime_s_default_mob_entry_is_its_launch_mob_s() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = runtime(&temp).await;
        let mob_id = runtime.mob_handle().mob_id().clone();

        let entry = runtime.default_mob();
        assert_eq!(entry.mob_runtime.mob_id(), &mob_id);
        assert_eq!(
            entry.identity_plane.mob_id(),
            &mob_id,
            "the entry's identity plane is its own mob's"
        );
        assert!(entry.identity_plane.is_of_mob(mob_id.as_str()));
        assert!(entry.identity_plane.attached().is_none());
        assert_eq!(runtime.mob_id(), mob_id.to_string());
        assert_eq!(runtime.mob_runtime().mob_id(), &mob_id);
        runtime.shutdown().await;
    }

    /// A request that names no mob is answered by the default mob: the
    /// resolver hands out that mob's own entry.
    #[tokio::test]
    async fn a_request_that_names_no_mob_resolves_the_default_mobs_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = runtime(&temp).await;
        let default_id = runtime.mob_handle().mob_id().clone();

        let resolved = runtime.resolve_mob(None).await;
        assert!(
            resolved.is_ok(),
            "a request that names no mob resolves the default mob, got {:?}",
            resolved.as_ref().err()
        );
        if let Ok(mob) = resolved {
            assert_eq!(mob.mob_id(), &default_id);
            assert!(mob.is_default());
            assert!(
                std::ptr::eq(mob.entry(), runtime.default_mob()),
                "the resolved entry is the default mob's own entry"
            );
        }
        runtime.shutdown().await;
    }

    /// Naming the default mob by its id and naming no mob are one value:
    /// both resolve the same entry.
    #[tokio::test]
    async fn the_default_mobs_own_id_resolves_the_same_entry_as_no_mob() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = runtime(&temp).await;
        let default_id = runtime.mob_handle().mob_id().clone();

        let named = runtime.resolve_mob(Some(&default_id)).await;
        assert!(
            named.is_ok(),
            "the default mob's own id resolves, got {:?}",
            named.as_ref().err()
        );
        let unnamed = runtime.resolve_mob(None).await;
        assert!(
            unnamed.is_ok(),
            "a request that names no mob resolves, got {:?}",
            unnamed.as_ref().err()
        );
        if let (Ok(named), Ok(unnamed)) = (named, unnamed) {
            assert!(
                std::ptr::eq(named.entry(), unnamed.entry()),
                "one entry, however the default mob was named"
            );
            assert_eq!(named.mob_id(), unnamed.mob_id());
            assert!(named.is_default());
        }
        runtime.shutdown().await;
    }

    /// A resolved default mob answers what the runtime's own accessors
    /// answer for it, from the same values, and names the runtime it
    /// belongs to.
    #[tokio::test]
    async fn a_resolved_default_mob_answers_what_the_runtime_answers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = runtime(&temp).await;
        let context = super::super::identity_install_phase_tests::context(&runtime);
        runtime.install_identity_first_context(Arc::clone(&context));
        let resolved = runtime.resolve_mob(None).await;
        assert!(resolved.is_ok(), "the default mob resolves");
        if let Ok(mob) = resolved {
            let hosting: &UnifiedRuntime = &runtime;
            assert!(std::ptr::eq(mob.runtime(), hosting));
            assert!(std::ptr::eq(mob.mob_runtime(), runtime.mob_runtime()));
            assert_eq!(mob.mob_handle().mob_id(), runtime.mob_handle().mob_id());
            assert_eq!(mob.mob_id().as_str(), runtime.mob_id());
            assert!(
                mob.identity_runtime_ref()
                    .is_some_and(|identity| Arc::ptr_eq(identity, &context.runtime)),
                "the installed identity runtime"
            );
            assert!(
                runtime
                    .identity_runtime_ref()
                    .is_some_and(|identity| Arc::ptr_eq(identity, &context.runtime))
            );
            assert_eq!(
                mob.identity_runtime().is_some(),
                runtime.identity_runtime().is_some()
            );
            assert!(mob.identity_runtime().is_some());
            assert!(mob.identity_first_context().is_some());
            assert_eq!(
                mob.session_bridge().is_some(),
                runtime.session_bridge().is_some()
            );
            assert_eq!(
                mob.speaks_for_composition(),
                runtime.speaks_for_composition()
            );
            assert_eq!(
                mob.resolved_storage().is_some(),
                runtime.resolved_storage().is_some()
            );
            assert_eq!(
                mob.binary_blob_store().is_some(),
                runtime.binary_blob_store().is_some()
            );
            assert_eq!(
                mob.workgraph_service().is_some(),
                runtime.workgraph_service().is_some()
            );
            assert!(Arc::ptr_eq(
                &mob.workgraph_admission(),
                &runtime.mob_runtime().workgraph_admission()
            ));
            assert_eq!(
                mob.workgraph_realm_migration().is_some(),
                runtime.workgraph_realm_migration().is_some()
            );
            assert_eq!(
                mob.workgraph_fact_hub().is_some(),
                runtime.workgraph_fact_hub().is_some()
            );
            assert!(std::ptr::eq(
                mob.topology_controller(),
                runtime.topology_controller()
            ));
            assert!(Arc::ptr_eq(
                mob.actor_loop_health(),
                runtime.actor_loop_health()
            ));
            assert_eq!(
                mob.console_identity_roster().is_some(),
                runtime.console_identity_roster().is_some()
            );
            assert_eq!(
                mob.bootstrap_edges_report().await.is_some(),
                runtime.bootstrap_edges_report().await.is_some()
            );
            // The topology seam is built over this mob's handle, and the
            // event store is the entry's.
            let _topology = mob.topology_runtime_handle();
            let _events = mob.mob_events_store();
            // The two operations a resolved mob has a form of answer what
            // the runtime answers for the default mob.
            assert_eq!(mob.broken_members().await, runtime.broken_members().await);
            let query = super::super::EventQuery::default();
            assert_eq!(
                mob.query_mob_events(&query)
                    .await
                    .ok()
                    .map(|events| events.len()),
                runtime
                    .query_mob_events(&query)
                    .await
                    .ok()
                    .map(|events| events.len())
            );
        }
        tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown())
            .await
            .expect("the runtime shuts down");
    }

    /// The question "which mob does this host serve for this request" has
    /// the resolver's answer for every way of naming a mob: the same id
    /// where the resolver hands out an entry, the same refusal where it
    /// refuses.
    #[tokio::test]
    async fn the_question_which_mob_is_served_agrees_with_the_resolver() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = runtime(&temp).await;
        let default_id = runtime.mob_handle().mob_id().clone();

        for named in [
            None,
            Some(default_id.clone()),
            Some(meerkat_mob::MobId::from("a-mob-this-host-does-not-have")),
            Some(meerkat_mob::MobId::from("")),
        ] {
            let served = runtime.served_mob_id(named.as_ref()).await;
            let resolved = runtime
                .resolve_mob(named.as_ref())
                .await
                .map(|mob| mob.mob_id().clone());
            assert_eq!(served, resolved, "asked for {named:?}");
        }
        assert_eq!(runtime.served_mob_id(None).await, Ok(default_id));
        runtime.shutdown().await;
    }

    /// The resolver a request door keeps answers what the runtime's own
    /// resolver answers, for every way of naming a mob, with the default
    /// mob's values as the runtime answers them when the door asks. A door
    /// takes its resolver when its router is built, and a host gives the
    /// runtime the mob's identity context and console roster after that:
    /// the resolver taken first, and a mob resolved with it then, answer
    /// both once they are given. A clone answers the same, and the
    /// resolver goes on answering after the runtime stopped, without the
    /// identity runtime the stop detached.
    #[tokio::test]
    async fn a_doors_resolver_answers_what_the_runtimes_resolver_answers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = runtime(&temp).await;
        let default_id = runtime.mob_handle().mob_id().clone();
        // The door's resolver, and a mob resolved with it, come first.
        let resolver = runtime.mob_resolver();
        let cloned = resolver.clone();
        let early = resolver.resolve(None).await.ok();
        assert!(
            early.as_ref().is_some_and(|mob| {
                mob.identity_runtime().is_none() && mob.console_identity_roster().is_none()
            }),
            "the default mob resolves, with no identity context and no roster yet"
        );

        let context = super::super::identity_install_phase_tests::context(&runtime);
        runtime.install_identity_first_context(Arc::clone(&context));
        let roster = Arc::new(crate::identity_first::MutableRosterProvider::new(Vec::new()));
        runtime.set_console_identity_roster(Arc::clone(&roster));
        let installed = |view: Option<crate::identity_first::PublicIdentityRuntime>| {
            view.is_some_and(|view| {
                std::ptr::eq(std::ptr::from_ref(&*view), Arc::as_ptr(&context.runtime))
            })
        };
        assert!(
            early
                .as_ref()
                .is_some_and(|mob| installed(mob.identity_runtime())),
            "a mob resolved before the identity context was installed answers the installed \
             identity runtime"
        );
        assert!(
            early.as_ref().is_some_and(|mob| {
                mob.console_identity_roster()
                    .is_some_and(|given| Arc::ptr_eq(&given, &roster))
            }),
            "a mob resolved before the console roster was given answers the roster"
        );
        let late = resolver.resolve(None).await.ok();
        assert!(
            late.as_ref()
                .is_some_and(|mob| installed(mob.identity_runtime())),
            "a resolver taken before the identity context was installed resolves a mob that \
             answers the installed identity runtime"
        );
        drop(late);

        for named in [
            None,
            Some(default_id.clone()),
            Some(meerkat_mob::MobId::from("a-mob-this-host-does-not-have")),
            Some(meerkat_mob::MobId::from("")),
        ] {
            let expected = runtime
                .resolve_mob(named.as_ref())
                .await
                .map(|mob| mob.mob_id().clone());
            for door in [&resolver, &cloned] {
                let resolved = door
                    .resolve(named.as_ref())
                    .await
                    .map(|mob| mob.mob_id().clone());
                assert_eq!(resolved, expected, "asked for {named:?}");
                assert_eq!(
                    door.served_mob_id(named.as_ref()).await,
                    expected,
                    "asked which mob is served for {named:?}"
                );
            }
        }

        let owned = resolver.resolve(None).await;
        let borrowed = runtime.resolve_mob(None).await;
        assert!(
            owned.is_ok() && borrowed.is_ok(),
            "the default mob resolves"
        );
        if let (Ok(owned), Ok(borrowed)) = (owned, borrowed) {
            assert_eq!(owned.mob_id(), borrowed.mob_id());
            assert_eq!(owned.is_default(), borrowed.is_default());
            assert!(owned.is_default());
            assert_eq!(
                owned.mob_runtime().mob_id(),
                borrowed.mob_runtime().mob_id()
            );
            assert_eq!(owned.mob_handle().mob_id(), borrowed.mob_handle().mob_id());
            assert!(
                owned
                    .identity_runtime_ref()
                    .is_some_and(|identity| Arc::ptr_eq(&identity, &context.runtime)),
                "the identity runtime the runtime answers"
            );
            assert!(
                borrowed
                    .identity_runtime_ref()
                    .is_some_and(|identity| Arc::ptr_eq(identity, &context.runtime)),
                "the borrowed form answers the same identity runtime"
            );
            assert_eq!(
                owned.identity_runtime().is_some(),
                borrowed.identity_runtime().is_some()
            );
            assert!(
                owned
                    .console_identity_roster()
                    .zip(borrowed.console_identity_roster())
                    .is_some_and(|(owned, borrowed)| Arc::ptr_eq(&owned, &borrowed)),
                "both forms answer the roster the host gave"
            );
            // What the entry holds as a plain value is fixed once the
            // runtime is composed: the owned form holds the same.
            assert_eq!(
                owned.workgraph_service().is_some(),
                borrowed.workgraph_service().is_some()
            );
            let _events = owned.mob_events_store();
        }

        tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown())
            .await
            .expect("the runtime shuts down");
        // Between the stop and the drop the entry still holds its identity
        // context, and the runtime's own read answers it. The owned form
        // reads the plane, which the stop detached.
        assert!(
            runtime
                .identity_runtime_ref()
                .is_some_and(|identity| Arc::ptr_eq(identity, &context.runtime)),
            "the stopped runtime still answers the identity runtime its entry holds"
        );
        assert!(
            early.as_ref().is_some_and(|mob| {
                mob.identity_runtime().is_none() && mob.identity_runtime_ref().is_none()
            }),
            "after the stop, with the runtime still there, a door's resolved mob answers no \
             identity runtime"
        );
        drop(runtime);
        assert_eq!(
            resolver.resolve(None).await.map(|mob| mob.mob_id().clone()),
            Ok(default_id.clone()),
            "a door's resolver still answers after the runtime stopped"
        );
        assert!(
            early.as_ref().is_some_and(|mob| {
                mob.identity_runtime().is_none() && mob.identity_runtime_ref().is_none()
            }),
            "after the runtime is gone a door's resolved mob answers no identity runtime"
        );
        let missing = meerkat_mob::MobId::from("a-mob-this-host-does-not-have");
        assert_eq!(
            cloned.served_mob_id(Some(&missing)).await,
            Err(super::MobResolveError::NotHosted {
                requested: missing.clone(),
                default_mob: default_id,
            })
        );
    }

    /// The cell of a mob's identity context answers the context installed
    /// last. Installing the installed context again changes nothing; a
    /// later install replaces it for every read from then on; and a
    /// borrow taken before stays the context it borrowed.
    #[tokio::test]
    async fn a_mobs_identity_context_cell_answers_the_context_installed_last() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = runtime(&temp).await;
        let first = super::super::identity_install_phase_tests::context(&runtime);
        let second = super::super::identity_install_phase_tests::context(&runtime);

        let cell = super::IdentityContextCell::default();
        assert!(cell.get().is_none(), "nothing installed yet");
        cell.install(Arc::clone(&first));
        let borrowed = cell.get();
        assert!(borrowed.is_some_and(|context| Arc::ptr_eq(context, &first)));
        cell.install(Arc::clone(&first));
        assert_eq!(
            Arc::strong_count(&first),
            2,
            "installing the installed context again keeps one install of it"
        );
        cell.install(Arc::clone(&second));
        assert!(
            cell.get()
                .is_some_and(|context| Arc::ptr_eq(context, &second)),
            "a later install replaces the context"
        );
        assert!(
            borrowed.is_some_and(|context| Arc::ptr_eq(context, &first)),
            "a borrow taken before the later install is still the first context"
        );
        cell.install(Arc::clone(&first));
        assert!(
            cell.get()
                .is_some_and(|context| Arc::ptr_eq(context, &first)),
            "the first context installed again, after another, is the context again"
        );
        runtime.shutdown().await;
    }

    /// The identities the topology `seam` answers as nodes; `None` when its
    /// query refuses.
    async fn topology_nodes(
        seam: crate::topology_control::TopologyRuntimeHandle,
    ) -> Option<Vec<String>> {
        seam.query().await.ok().map(|snapshot| {
            snapshot
                .nodes
                .into_iter()
                .map(|node| node.endpoint.identity)
                .collect()
        })
    }

    /// A door's resolved mob builds the mob's topology seam when it is
    /// asked for, from the identity context installed then. Resolved
    /// before a host installed its identity context, it answers the
    /// topology of that context once it is installed, as the runtime's own
    /// seam does: here, the agent the context's roster configures.
    #[tokio::test]
    async fn a_doors_resolved_mob_answers_the_topology_of_the_installed_identity_context() {
        use super::super::identity_install_phase_tests::{configured_agent, context_with_roster};

        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = runtime(&temp).await;
        let resolver = runtime.mob_resolver();
        let early = resolver.resolve(None).await.ok();
        assert!(early.is_some(), "the default mob resolves");
        if let Some(early) = early.as_ref() {
            assert_eq!(
                topology_nodes(early.topology_runtime_handle()).await,
                Some(Vec::new()),
                "no identity context and no member: a topology with no node"
            );
        }

        let context = context_with_roster(
            &runtime,
            Arc::new(crate::identity_first::MutableRosterProvider::new(vec![
                configured_agent("desk"),
            ])),
        );
        runtime.install_identity_first_context(context);
        let configured = Some(vec!["desk".to_string()]);
        assert_eq!(
            topology_nodes(runtime.topology_runtime_handle()).await,
            configured,
            "the runtime's own seam answers the installed context's roster"
        );
        if let Some(early) = early.as_ref() {
            assert_eq!(
                topology_nodes(early.topology_runtime_handle()).await,
                configured,
                "a mob resolved before the identity context was installed answers the \
                 context's topology"
            );
        }

        tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown())
            .await
            .expect("the runtime shuts down");
    }

    /// A door's resolved mob that outlives its runtime keeps no identity
    /// context alive: once the runtime is stopped and dropped, the context
    /// a host installed is held by the host alone, though the resolver and
    /// the mob resolved with it are still there and still build a seam.
    #[tokio::test]
    async fn a_doors_resolved_mob_that_outlives_its_runtime_keeps_no_identity_context_alive() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = runtime(&temp).await;
        let resolver = runtime.mob_resolver();
        let early = resolver.resolve(None).await.ok();
        assert!(early.is_some(), "the default mob resolves");

        let context = super::super::identity_install_phase_tests::context(&runtime);
        runtime.install_identity_first_context(Arc::clone(&context));
        assert!(
            Arc::strong_count(&context) > 1,
            "the runtime's entry holds the installed context"
        );

        tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown())
            .await
            .expect("the runtime shuts down");
        // Between the stop and the drop the two reads of the owned form
        // differ, and both are pinned as they are. It answers no identity
        // runtime, from the plane the stop detached
        // (`a_doors_resolver_answers_what_the_runtimes_resolver_answers`).
        // Its seam is built from the entry's cell, which holds the installed
        // context until the entry is dropped: the seam carries it.
        if let Some(mob) = early.as_ref() {
            let seam = mob.topology_runtime_handle();
            let held_with_the_seam = Arc::strong_count(&context);
            drop(seam);
            assert!(
                held_with_the_seam > Arc::strong_count(&context),
                "the seam of a stopped mob whose entry is still there carries the installed \
                 identity context"
            );
        }
        drop(runtime);
        assert_eq!(
            Arc::strong_count(&context),
            1,
            "a door's resolver and resolved mob keep the dropped runtime's identity context \
             alive"
        );
        let late = resolver.resolve(None).await.ok();
        for mob in early.iter().chain(late.iter()) {
            let seam = mob.topology_runtime_handle();
            assert_eq!(
                Arc::strong_count(&context),
                1,
                "a seam built once the runtime is gone carries no identity context"
            );
            drop(seam);
        }
    }

    /// What a request to a mob's identity plane needs beside the identity
    /// context is the mob's entry's: nothing until the host that composed
    /// the mob gives it, then exactly what it gave.
    #[tokio::test]
    async fn a_mobs_identity_request_services_are_its_entrys() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = runtime(&temp).await;
        let before = runtime.resolve_mob(None).await;
        assert!(
            before.is_ok_and(|mob| mob.identity_request_services().is_none()),
            "no services before the host gave any"
        );

        let floors = Arc::new(crate::identity_first::CompactionFloorRegistry::default());
        runtime.set_identity_request_services(super::IdentityRequestServices {
            compaction_floors: Some(Arc::clone(&floors)),
            ..Default::default()
        });
        let services = runtime
            .resolve_mob(None)
            .await
            .ok()
            .and_then(|mob| mob.identity_request_services());
        assert!(services.is_some(), "the services the host gave");
        if let Some(services) = services {
            assert!(
                services
                    .compaction_floors
                    .as_ref()
                    .is_some_and(|given| Arc::ptr_eq(given, &floors))
            );
            assert!(services.agent_memory_provider.is_none());
            assert!(services.transcript_edit_service.is_none());
        }
        runtime.shutdown().await;
    }

    /// The monitor of a mob's identity streams is its entry's, beside the
    /// process's forwarder of member events: both run from the start, and
    /// shutdown stops both.
    #[tokio::test]
    async fn the_monitor_of_a_mobs_identity_streams_is_its_entrys() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = runtime(&temp).await;
        assert!(
            runtime
                .default_mob()
                .identity_stream_health_task
                .lock()
                .await
                .is_some(),
            "the entry holds its mob's identity stream monitor"
        );
        assert!(runtime.mob_event_ingress.lock().await.is_some());

        tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown())
            .await
            .expect("the runtime shuts down");
        assert!(
            runtime
                .default_mob()
                .identity_stream_health_task
                .lock()
                .await
                .is_none(),
            "shutdown stops the mob's identity stream monitor"
        );
        assert!(runtime.mob_event_ingress.lock().await.is_none());
    }

    /// The monitor of a mob's identity streams stops with its entry, whatever
    /// became of the process's forwarder of member events: a drain that
    /// found the forwarder's channel closed dropped the ingress, and the
    /// shutdown after it still stops the monitor.
    #[tokio::test]
    async fn the_monitor_of_a_mobs_identity_streams_stops_without_the_forwarder() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = runtime(&temp).await;
        let ingress = runtime.mob_event_ingress.lock().await.take();
        assert!(ingress.is_some(), "the forwarder runs from the start");
        if let Some(super::super::MobEventIngress::Forwarder(forwarder)) = ingress {
            forwarder.task.abort();
            let _ = forwarder.task.await;
        }
        let monitor = runtime
            .default_mob()
            .identity_stream_health_task
            .lock()
            .await
            .as_ref()
            .map(tokio::task::JoinHandle::abort_handle);
        assert!(
            monitor
                .as_ref()
                .is_some_and(|monitor| !monitor.is_finished()),
            "the monitor runs although the forwarder is gone"
        );

        tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown())
            .await
            .expect("the runtime shuts down");
        assert!(
            runtime
                .default_mob()
                .identity_stream_health_task
                .lock()
                .await
                .is_none(),
            "shutdown stops the mob's identity stream monitor although the forwarder was gone"
        );
        assert!(
            monitor.is_some_and(|monitor| monitor.is_finished()),
            "and the monitor's task has ended: it was stopped, not let go"
        );
    }

    /// Where the forwarder is present, the close of the event router stops
    /// the forwarder first, then the monitor of the mob's identity streams,
    /// then the subscriber of the mob's ledger: the order of the source.
    #[test]
    fn the_forwarder_stops_before_the_monitor_of_a_mobs_identity_streams() {
        let source = include_str!("lifecycle.rs");
        let close = source
            .find("async fn close_event_router")
            .map(|start| &source[start..]);
        assert!(close.is_some(), "the close of the event router is read");
        if let Some(close) = close {
            let forwarder = close.find("task.abort()");
            let monitor = close.find("identity_stream_health_task");
            let ledger = close.find("mob_events_subscriber_task");
            assert!(
                forwarder.is_some() && forwarder < monitor && monitor < ledger,
                "forwarder {forwarder:?}, monitor {monitor:?}, ledger subscriber {ledger:?}"
            );
        }
    }

    /// The one edge the fixture's edge policy desires, between two names no
    /// member has.
    fn declared_edge() -> super::super::DesiredPeerEdge {
        super::super::DesiredPeerEdge::new("north", "south").expect("an edge")
    }

    /// An edge policy that desires [`declared_edge`] whatever the roster.
    struct OneDeclaredEdge;

    impl super::super::EdgeDiscovery for OneDeclaredEdge {
        fn discover_edges(
            &self,
            _active_members: Vec<super::super::edge_types::EdgeMemberView>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Vec<super::super::DesiredPeerEdge>> + Send + '_>,
        > {
            Box::pin(async { vec![declared_edge()] })
        }
    }

    /// A runtime of one mob with a `worker` profile and no members, built
    /// by the builder; with `edge_policy`, its entry's edge policy is
    /// [`OneDeclaredEdge`].
    async fn runtime_of_one_mob(tag: &str, edge_policy: bool) -> UnifiedRuntime {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"entry-form-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            uuid::Uuid::new_v4().simple()
        ))
        .expect("mob definition");
        let mut builder = UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()));
        if edge_policy {
            builder = builder.edge_discovery(OneDeclaredEdge);
        }
        builder.build().await.expect("runtime builds")
    }

    async fn shut_down(runtime: &UnifiedRuntime) {
        tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown())
            .await
            .expect("the runtime shuts down");
    }

    /// What a handler that resolved `mob` calls to reconcile the mob's
    /// edges.
    async fn reconcile_edges_as_a_handler(
        mob: &super::MobEntryRef<'_>,
    ) -> super::UnifiedRuntimeReconcileEdgesReport {
        mob.reconcile_edges().await
    }

    /// What a handler that resolved `mob` calls for the peer facts of the
    /// mob's member `member`.
    async fn peer_facts_as_a_handler(
        mob: &super::MobEntryRef<'_>,
        member: &str,
    ) -> Option<super::super::cross_mob::LocalMemberPeerFacts> {
        mob.local_member_peer_facts(member).await.ok()
    }

    /// What a handler that resolved `mob` calls to ask the mob's roster
    /// again: whether the mob's identity context was asked.
    async fn refresh_as_a_handler(mob: &super::MobEntryRef<'_>) -> bool {
        !matches!(mob.refresh_desired_topology().await, Ok(None))
    }

    /// Two entries in one runtime: the edge reconcile of a resolved mob
    /// reads that mob's own edge policy, and the runtime's own method reads
    /// the default mob's.
    #[tokio::test]
    async fn a_resolved_mob_reconciles_the_edges_of_its_own_entry() {
        let host = runtime_of_one_mob("host", false).await;
        let second = runtime_of_one_mob("second", true).await;
        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);
        assert!(!mob.is_default());
        assert_eq!(mob.mob_id(), second.mob_handle().mob_id());
        assert!(std::ptr::eq(mob.runtime(), &raw const host));

        let of_the_second = reconcile_edges_as_a_handler(&mob).await;
        assert_eq!(
            of_the_second.desired_edges,
            vec![declared_edge()],
            "the second mob's edge policy desires its edge"
        );
        assert_eq!(
            of_the_second.skipped_missing_members,
            vec![declared_edge()],
            "neither endpoint is a member of the second mob"
        );
        let of_the_default = host.reconcile_edges().await;
        assert!(
            of_the_default.desired_edges.is_empty(),
            "the runtime's own method reconciles the default mob, which has no edge policy"
        );
        let resolved_default = host.resolve_mob(None).await;
        assert!(resolved_default.is_ok(), "the default mob resolves");
        if let Ok(default_mob) = resolved_default {
            assert!(
                reconcile_edges_as_a_handler(&default_mob)
                    .await
                    .desired_edges
                    .is_empty(),
                "the default mob resolved answers as the runtime's own method"
            );
        }
        shut_down(&host).await;
        shut_down(&second).await;
    }

    /// The builder of a runtime of one mob whose `worker` members have
    /// comms, so that a member can be seated and has peer facts: its
    /// members are built by a session service with comms.
    fn builder_of_one_mob_with_comms(
        temp: &tempfile::TempDir,
        tag: &str,
    ) -> super::super::UnifiedRuntimeBuilder {
        let session_path = temp.path().join("sessions");
        std::fs::create_dir_all(&session_path).expect("session directory");
        let factory = meerkat::AgentFactory::new(&session_path).comms(true);
        let session_service = Arc::new(meerkat::build_ephemeral_service(
            factory,
            meerkat::Config::default(),
            16,
        ));
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"entry-form-{tag}-{}\"\n\n[profiles.worker]\nmodel = \
             \"gpt-5.5\"\nexternal_addressable = true\n\n[profiles.worker.tools]\ncomms = true\n",
            uuid::Uuid::new_v4().simple()
        ))
        .expect("mob definition");
        let spec = crate::MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            session_service,
        )
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(crate::MetaConfig {
                modules: Vec::new(),
                discovery: crate::DiscoverySpec {
                    namespace: format!("entry-form-{tag}"),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .timeout(std::time::Duration::from_secs(1))
    }

    /// A runtime of one mob whose `worker` members have comms and that has
    /// no members, built by [`builder_of_one_mob_with_comms`].
    async fn runtime_of_one_mob_with_comms(temp: &tempfile::TempDir, tag: &str) -> UnifiedRuntime {
        Box::pin(builder_of_one_mob_with_comms(temp, tag).build())
            .await
            .expect("runtime builds")
    }

    /// The members of `runtime`'s mob, by their public names.
    async fn roster(runtime: &UnifiedRuntime) -> std::collections::BTreeSet<String> {
        runtime
            .mob_handle()
            .list_members_including_retiring()
            .await
            .iter()
            .map(|member| {
                crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str())
                    .into_owned()
            })
            .collect()
    }

    /// What a handler that resolved `mob` calls to seat a member of the
    /// mob: the error's text when the spawn failed.
    async fn spawn_as_a_handler(
        mob: &super::MobEntryRef<'_>,
        spec: meerkat_mob::SpawnMemberSpec,
    ) -> Result<(), String> {
        Box::pin(mob.spawn(spec))
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    /// What a handler that resolved `mob` calls to reset the mob and run
    /// its discovery again: the members the discovery seated.
    async fn rediscover_as_a_handler(
        mob: &super::MobEntryRef<'_>,
    ) -> Result<Option<Vec<String>>, String> {
        Box::pin(mob.rediscover())
            .await
            .map(|report| report.map(|report| report.spawned))
            .map_err(|error| format!("{error:?}"))
    }

    /// A discovery that answers one member, `scout`, of the fixture's
    /// `worker` profile.
    struct OneScout;

    impl super::super::Discovery for OneScout {
        fn discover(
            &self,
            _context: super::super::PreSpawnContext,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Vec<crate::types::AgentDiscoverySpec>> + Send + '_,
            >,
        > {
            Box::pin(async {
                vec![crate::types::AgentDiscoverySpec {
                    profile: "worker".to_string(),
                    meerkat_id: "scout".to_string(),
                    labels: None,
                    context: None,
                    additional_instructions: Vec::new(),
                    resume_session_id: None,
                }]
            })
        }
    }

    /// Two entries in one runtime: a spawn through a resolved mob seats the
    /// member in that mob's own roster, and the runtime's own method seats
    /// a member of the default mob.
    #[tokio::test]
    async fn a_resolved_mob_spawns_into_its_own_roster() {
        let (temp_host, temp_second) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let host = runtime_of_one_mob_with_comms(&temp_host, "host").await;
        let second = runtime_of_one_mob_with_comms(&temp_second, "second").await;
        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);

        let seated =
            spawn_as_a_handler(&mob, meerkat_mob::SpawnMemberSpec::new("worker", "desk")).await;
        assert_eq!(seated, Ok(()), "the spawn through the resolved mob");
        assert!(
            roster(&second).await.contains("desk"),
            "the member is seated in the resolved mob's own roster"
        );
        assert!(
            !roster(&host).await.contains("desk"),
            "and not in the default mob's"
        );

        let front =
            Box::pin(host.spawn(meerkat_mob::SpawnMemberSpec::new("worker", "front"))).await;
        assert!(
            front.is_ok(),
            "the runtime's own method: {:?}",
            front.as_ref().err()
        );
        assert!(
            roster(&host).await.contains("front") && !roster(&second).await.contains("front"),
            "the runtime's own method seats a member of the default mob"
        );
        shut_down(&host).await;
        shut_down(&second).await;
    }

    /// The post-spawn hook is the default mob's: it is told the members
    /// spawned in the mob the launch names, and nothing of a member spawned
    /// through another resolved mob. The error hook is the host's one alert
    /// sink: a failed spawn of the other mob fires it.
    #[tokio::test]
    async fn the_post_spawn_hook_is_told_of_the_default_mobs_members_alone() {
        let (temp_host, temp_second) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let told: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let sink = Arc::clone(&told);
        let hook: super::super::PostSpawnHook = Arc::new(move |members| {
            sink.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(members);
            Box::pin(async {})
        });
        let told_so_far = || {
            told.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };
        let mut host = Box::pin(
            builder_of_one_mob_with_comms(&temp_host, "host")
                .post_spawn_hook(hook)
                .build(),
        )
        .await
        .expect("runtime builds");
        let (alerts, mut alerted) = tokio::sync::mpsc::unbounded_channel::<String>();
        let error_hook: super::super::ErrorHook = Arc::new(move |event| {
            let _ = alerts.send(event.to_string());
            Box::pin(async {})
        });
        host.set_error_hook(error_hook);
        let second = runtime_of_one_mob_with_comms(&temp_second, "second").await;

        let front =
            Box::pin(host.spawn(meerkat_mob::SpawnMemberSpec::new("worker", "front"))).await;
        assert!(
            front.is_ok(),
            "the default mob seats a member: {:?}",
            front.as_ref().err()
        );
        assert_eq!(
            told_so_far(),
            ["front"],
            "the hook is told of the default mob's member"
        );

        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);
        let seated =
            spawn_as_a_handler(&mob, meerkat_mob::SpawnMemberSpec::new("worker", "desk")).await;
        assert_eq!(seated, Ok(()), "the spawn through the resolved mob");
        assert_eq!(
            told_so_far(),
            ["front"],
            "the hook is told nothing of a member of another mob"
        );

        let refused = spawn_as_a_handler(
            &mob,
            meerkat_mob::SpawnMemberSpec::new("no-such-profile", "ghost"),
        )
        .await;
        assert!(refused.is_err(), "a profile the mob does not have");
        let alert = tokio::time::timeout(std::time::Duration::from_secs(10), alerted.recv()).await;
        assert!(
            alert
                .as_ref()
                .is_ok_and(|alert| alert.as_ref().is_some_and(|text| text.contains("ghost"))),
            "the host's error hook is told of the failed spawn: {alert:?}"
        );
        assert_eq!(told_so_far(), ["front"]);
        shut_down(&host).await;
        shut_down(&second).await;
    }

    /// Two entries in one runtime, a discovery on the second mob alone: a
    /// rediscover through a resolved mob resets that mob and runs its own
    /// discovery again, and the runtime's own method rediscovers the
    /// default mob, which has no discovery.
    #[tokio::test]
    async fn a_resolved_mob_rediscovers_its_own_members() {
        let (temp_host, temp_second) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let host = runtime_of_one_mob_with_comms(&temp_host, "host").await;
        let second = Box::pin(
            builder_of_one_mob_with_comms(&temp_second, "second")
                .discovery(OneScout)
                .build(),
        )
        .await
        .expect("runtime builds");
        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);

        let rediscovered = tokio::time::timeout(
            std::time::Duration::from_mins(1),
            rediscover_as_a_handler(&mob),
        )
        .await;
        assert_eq!(
            rediscovered.ok(),
            Some(Ok(Some(vec!["scout".to_string()]))),
            "the resolved mob's own discovery runs again"
        );
        assert!(
            roster(&second).await.contains("scout"),
            "the discovered member is seated in the resolved mob"
        );
        assert!(
            matches!(Box::pin(host.rediscover()).await, Ok(None)),
            "the runtime's own method rediscovers the default mob, which has no discovery"
        );
        assert!(
            roster(&host).await.is_empty(),
            "the default mob is untouched"
        );
        shut_down(&host).await;
        shut_down(&second).await;
    }

    /// The second mob of the rediscover rows: a discovery that answers
    /// `scout`, and members with comms.
    async fn second_mob_with_a_discovery(temp: &tempfile::TempDir) -> UnifiedRuntime {
        Box::pin(
            builder_of_one_mob_with_comms(temp, "second")
                .discovery(OneScout)
                .build(),
        )
        .await
        .expect("runtime builds")
    }

    /// The edge the rediscover rows plant among a mob's managed edges.
    fn planted_edge() -> (String, String) {
        ("north".to_string(), "south".to_string())
    }

    /// Two entries in one runtime, each with a member and a managed edge: a
    /// rediscover through a resolved mob resets THAT mob and clears ITS
    /// managed edges. The default mob keeps its member and its managed
    /// edge.
    #[tokio::test]
    async fn a_rediscover_of_a_resolved_mob_resets_that_mob_alone() {
        let (temp_host, temp_second) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let host = runtime_of_one_mob_with_comms(&temp_host, "host").await;
        let second = second_mob_with_a_discovery(&temp_second).await;
        seat(&host, "front").await;
        seat(&second, "desk").await;
        for runtime in [&host, &second] {
            runtime
                .default_mob()
                .managed_dynamic_edges
                .write()
                .await
                .insert(planted_edge());
        }
        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);

        let rediscovered = tokio::time::timeout(
            std::time::Duration::from_mins(1),
            rediscover_as_a_handler(&mob),
        )
        .await;
        assert_eq!(
            rediscovered.ok(),
            Some(Ok(Some(vec!["scout".to_string()]))),
            "the resolved mob's own discovery runs again"
        );
        let of_the_second = roster(&second).await;
        assert!(
            of_the_second.contains("scout") && !of_the_second.contains("desk"),
            "the resolved mob was reset: {of_the_second:?}"
        );
        assert!(
            roster(&host).await.contains("front"),
            "the default mob keeps its member: the reset was the resolved mob's"
        );
        assert!(
            second
                .default_mob()
                .managed_dynamic_edges
                .read()
                .await
                .is_empty(),
            "the resolved mob's managed edges were cleared"
        );
        assert!(
            host.default_mob()
                .managed_dynamic_edges
                .read()
                .await
                .contains(&planted_edge()),
            "the default mob keeps its managed edge"
        );
        shut_down(&host).await;
        shut_down(&second).await;
    }

    /// A rediscover is unavailable to a mob with an identity runtime, and
    /// it is the RESOLVED mob's identity runtime that is asked. Where only
    /// the default mob has one, the second mob's rediscover runs; where
    /// only the second mob has one, its rediscover is refused.
    #[tokio::test]
    async fn a_rediscover_of_a_resolved_mob_asks_that_mobs_own_identity_authority() {
        const REFUSAL: &str = "unavailable with identity-first authority";
        let (temp_host, temp_second) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let mut host = runtime_of_one_mob_with_comms(&temp_host, "host").await;
        let context = super::super::identity_install_phase_tests::context(&host);
        host.install_identity_first_context(context);
        let second = second_mob_with_a_discovery(&temp_second).await;
        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);
        let rediscovered = tokio::time::timeout(
            std::time::Duration::from_mins(1),
            rediscover_as_a_handler(&mob),
        )
        .await;
        assert_eq!(
            rediscovered.ok(),
            Some(Ok(Some(vec!["scout".to_string()]))),
            "only the default mob has an identity runtime: the second mob's rediscover runs"
        );
        shut_down(&host).await;
        shut_down(&second).await;

        let (temp_host, temp_second) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let host = runtime_of_one_mob_with_comms(&temp_host, "host").await;
        let mut second = second_mob_with_a_discovery(&temp_second).await;
        let context = super::super::identity_install_phase_tests::context(&second);
        second.install_identity_first_context(context);
        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);
        let refused = tokio::time::timeout(
            std::time::Duration::from_mins(1),
            rediscover_as_a_handler(&mob),
        )
        .await;
        assert!(
            refused
                .as_ref()
                .is_ok_and(|answer| answer.as_ref().is_err_and(|text| text.contains(REFUSAL))),
            "only the second mob has an identity runtime: its rediscover is refused: {refused:?}"
        );
        shut_down(&host).await;
        shut_down(&second).await;
    }

    /// Two entries in one runtime, a member of one name in both mobs: the
    /// peer facts a resolved mob answers are its own member's, and the
    /// runtime's own method answers the default mob's member.
    #[tokio::test]
    async fn a_resolved_mob_answers_the_peer_facts_of_its_own_member() {
        let (temp_host, temp_second) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let host = runtime_of_one_mob_with_comms(&temp_host, "host").await;
        let second = runtime_of_one_mob_with_comms(&temp_second, "second").await;
        for runtime in [&host, &second] {
            let seated =
                Box::pin(runtime.spawn(meerkat_mob::SpawnMemberSpec::new("worker", "desk"))).await;
            assert!(
                seated.is_ok(),
                "each mob seats a member named desk: {:?}",
                seated.as_ref().err()
            );
        }
        let of_the_default = host.local_member_peer_facts("desk").await;
        let of_the_second_mob = second.local_member_peer_facts("desk").await;
        assert!(
            of_the_default.is_ok() && of_the_second_mob.is_ok(),
            "each member has peer facts: {:?}, {:?}",
            of_the_default.as_ref().err(),
            of_the_second_mob.as_ref().err()
        );
        let (of_the_default, of_the_second_mob) = (of_the_default.ok(), of_the_second_mob.ok());
        let peer_id = |facts: &Option<super::super::cross_mob::LocalMemberPeerFacts>| {
            facts.as_ref().map(|facts| facts.peer_id.clone())
        };
        assert_ne!(
            peer_id(&of_the_default),
            peer_id(&of_the_second_mob),
            "two members, one name"
        );

        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);
        let answered = peer_facts_as_a_handler(&mob, "desk").await;
        assert_eq!(
            peer_id(&answered),
            peer_id(&of_the_second_mob),
            "the resolved mob answers its own member"
        );
        assert_eq!(
            answered.map(|facts| facts.comms_name),
            of_the_second_mob.map(|facts| facts.comms_name)
        );
        let resolved_default = host.resolve_mob(None).await;
        assert!(resolved_default.is_ok(), "the default mob resolves");
        if let Ok(default_mob) = resolved_default {
            assert_eq!(
                peer_id(&peer_facts_as_a_handler(&default_mob, "desk").await),
                peer_id(&of_the_default),
                "the default mob resolved answers as the runtime's own method"
            );
        }
        shut_down(&host).await;
        shut_down(&second).await;
    }

    /// Two entries in one runtime, an identity context on the second mob
    /// alone: the roster pass of a resolved mob asks that mob's own
    /// identity context, and the runtime's own method asks the default
    /// mob's, which has none.
    #[tokio::test]
    async fn a_resolved_mob_refreshes_the_topology_of_its_own_identity_context() {
        let (temp_host, temp_second) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let host = runtime(&temp_host).await;
        let mut second = runtime(&temp_second).await;
        let context = super::super::identity_install_phase_tests::context(&second);
        second.install_identity_first_context(context);

        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);
        let asked = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            refresh_as_a_handler(&mob),
        )
        .await;
        assert_eq!(
            asked.ok(),
            Some(true),
            "the second mob's identity context is asked for its roster"
        );
        assert!(
            matches!(host.refresh_desired_topology().await, Ok(None)),
            "the runtime's own method asks the default mob, which has no identity context"
        );
        shut_down(&host).await;
        shut_down(&second).await;
    }

    type PeerFacts = super::super::cross_mob::LocalMemberPeerFacts;

    /// Seat a `worker` named `member` in `runtime`'s mob, through the
    /// runtime's own method.
    async fn seat(runtime: &UnifiedRuntime, member: &str) {
        let seated =
            Box::pin(runtime.spawn(meerkat_mob::SpawnMemberSpec::new("worker", member))).await;
        assert!(
            seated.is_ok(),
            "the mob seats {member}: {:?}",
            seated.as_ref().err()
        );
    }

    /// What a peer needs to reach the member `member` of `runtime`'s mob.
    async fn facts_of(runtime: &UnifiedRuntime, member: &str) -> PeerFacts {
        runtime
            .local_member_peer_facts(member)
            .await
            .expect("the member has peer facts")
    }

    /// The transport key of the member `peer` describes.
    fn key_of(peer: &PeerFacts) -> [u8; 32] {
        crate::auth::peer_keys::decode_pubkey_b64(&peer.pubkey_b64).expect("the member's key")
    }

    /// The peers the row of `member` in `runtime`'s mob is wired to, by
    /// their comms names.
    async fn wired_to(
        runtime: &UnifiedRuntime,
        member: &str,
    ) -> std::collections::BTreeSet<String> {
        let member = crate::member_comms_id::mob_member_id(member);
        runtime
            .mob_handle()
            .get_member(&member)
            .await
            .ok()
            .flatten()
            .map(|row| {
                row.wired_to
                    .iter()
                    .map(|peer| peer.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the part of the link table of `runtime`'s mob holds a link
    /// to the member `peer` describes: the mob's namespace then has a wake
    /// route under that member's key.
    fn holds_a_link_to(runtime: &UnifiedRuntime, peer: &PeerFacts) -> bool {
        let namespace = super::super::cross_mob::mob_inproc_namespace_for_id(&runtime.mob_id())
            .expect("the mob's namespace");
        meerkat_comms::InprocRegistry::global()
            .has_wake_route_in_namespace(&namespace, &meerkat_comms::PubKey::new(key_of(peer)))
    }

    /// A contact directory that names the mob of `peer` as a mob of this
    /// process.
    fn directory_naming(peer: &UnifiedRuntime) -> crate::contact_directory::ContactDirectory {
        crate::contact_directory::ContactDirectory::from_toml(&format!(
            "[mobs]\n{} = \"inproc\"\n",
            peer.mob_id()
        ))
        .expect("a contact directory")
    }

    /// What a handler that resolved `mob` calls to wire the mob's member
    /// `member` to the peer `peer` describes, on the mob's side alone, for
    /// a request that brings no identity runtime of its own.
    async fn wire_local_as_a_handler(
        mob: &super::MobEntryRef<'_>,
        member: &str,
        peer: &PeerFacts,
    ) -> Result<(), String> {
        Box::pin(mob.wire_local_with_identity_runtime(
            member,
            &peer.comms_name,
            &peer.peer_id,
            &peer.address,
            Some(key_of(peer)),
            None,
        ))
        .await
        .map_err(|error| error.to_string())
    }

    /// What a handler that resolved `mob` calls to take that wire back.
    async fn unwire_local_as_a_handler(
        mob: &super::MobEntryRef<'_>,
        member: &str,
        peer: &PeerFacts,
    ) -> Result<(), String> {
        Box::pin(mob.unwire_local_with_identity_runtime(
            member,
            &peer.comms_name,
            &peer.peer_id,
            &peer.address,
            Some(key_of(peer)),
            None,
        ))
        .await
        .map_err(|error| error.to_string())
    }

    /// What a handler that resolved `mob` calls to wire the mob's member
    /// `member` and the member `remote_member` of the peer mob
    /// `remote_mob`, on both sides.
    async fn wire_cross_mob_as_a_handler(
        mob: &super::MobEntryRef<'_>,
        member: &str,
        remote_member: &str,
        remote_mob: &str,
    ) -> Result<(), String> {
        Box::pin(mob.wire_cross_mob_with_identity_runtime(member, remote_member, remote_mob, None))
            .await
            .map_err(|error| error.to_string())
    }

    /// What a handler that resolved `mob` calls to take that wire back.
    async fn unwire_cross_mob_as_a_handler(
        mob: &super::MobEntryRef<'_>,
        member: &str,
        remote_member: &str,
        remote_mob: &str,
    ) -> Result<(), String> {
        Box::pin(mob.unwire_cross_mob_with_identity_runtime(
            member,
            remote_member,
            remote_mob,
            None,
        ))
        .await
        .map_err(|error| error.to_string())
    }

    /// What a handler that resolved `mob` calls to send from the mob's
    /// member `from` to the member `remote_member` of the peer mob
    /// `remote_mob`.
    async fn send_cross_mob_as_a_handler(
        mob: &super::MobEntryRef<'_>,
        from: &str,
        remote_member: &str,
        remote_mob: &str,
    ) -> Result<String, String> {
        Box::pin(mob.send_cross_mob_with_identity_runtime(
            from,
            remote_member,
            remote_mob,
            "a message",
            None,
        ))
        .await
        .map_err(|error| error.to_string())
    }

    /// Two entries in one runtime, a member of one name in both mobs, and a
    /// member of a third mob of this process: a one-sided wire through a
    /// resolved mob changes the row of that mob's own member and is held in
    /// that mob's own part of the link table; the unwire takes both back.
    /// The runtime's own methods wire and unwire the default mob's member.
    #[tokio::test]
    async fn a_resolved_mob_wires_and_unwires_its_own_member_locally() {
        let (temp_host, temp_second, temp_far) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let host = runtime_of_one_mob_with_comms(&temp_host, "host").await;
        let second = runtime_of_one_mob_with_comms(&temp_second, "second").await;
        let far = runtime_of_one_mob_with_comms(&temp_far, "far").await;
        seat(&host, "desk").await;
        seat(&second, "desk").await;
        seat(&far, "front").await;
        let front = facts_of(&far, "front").await;
        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);

        let wired = wire_local_as_a_handler(&mob, "desk", &front).await;
        assert_eq!(wired, Ok(()), "the wire through the resolved mob");
        assert!(
            wired_to(&second, "desk").await.contains(&front.comms_name),
            "the resolved mob's own member holds the edge"
        );
        assert!(
            !wired_to(&host, "desk").await.contains(&front.comms_name),
            "and the default mob's member of that name does not"
        );
        assert!(
            holds_a_link_to(&second, &front) && !holds_a_link_to(&host, &front),
            "the link is held in the resolved mob's own part of the link table"
        );

        let unwired = unwire_local_as_a_handler(&mob, "desk", &front).await;
        assert_eq!(unwired, Ok(()), "the unwire through the resolved mob");
        assert!(
            !wired_to(&second, "desk").await.contains(&front.comms_name),
            "the resolved mob's own member lost the edge"
        );
        assert!(
            !holds_a_link_to(&second, &front),
            "and the resolved mob's part lost the link"
        );

        let of_the_default = Box::pin(host.wire_local(
            "desk",
            &front.comms_name,
            &front.peer_id,
            &front.address,
            Some(key_of(&front)),
        ))
        .await;
        assert!(
            of_the_default.is_ok(),
            "the runtime's own method: {:?}",
            of_the_default.as_ref().err()
        );
        assert!(
            wired_to(&host, "desk").await.contains(&front.comms_name)
                && !wired_to(&second, "desk").await.contains(&front.comms_name),
            "the runtime's own method wires the default mob's member"
        );
        assert!(holds_a_link_to(&host, &front) && !holds_a_link_to(&second, &front));
        let of_the_default = Box::pin(host.unwire_local(
            "desk",
            &front.comms_name,
            &front.peer_id,
            &front.address,
            Some(key_of(&front)),
        ))
        .await;
        assert!(
            of_the_default.is_ok() && !wired_to(&host, "desk").await.contains(&front.comms_name),
            "the runtime's own method unwires the default mob's member: {:?}",
            of_the_default.as_ref().err()
        );
        shut_down(&host).await;
        shut_down(&second).await;
        shut_down(&far).await;
    }

    /// Two entries in one runtime, a member of one name in both mobs, and a
    /// peer mob the HOST knows (its contact directory and its registered
    /// peer handles are the process's): a wire through a resolved mob joins
    /// that mob's own member and the peer mob's member, each row naming the
    /// other and each mob's part of the link table holding its link; the
    /// unwire takes all of it back. The runtime's own methods wire and
    /// unwire the default mob's member.
    #[tokio::test]
    async fn a_resolved_mob_wires_and_unwires_its_own_member_with_a_peer_mob() {
        let (temp_host, temp_second, temp_far) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let mut host = runtime_of_one_mob_with_comms(&temp_host, "host").await;
        let second = runtime_of_one_mob_with_comms(&temp_second, "second").await;
        let far = runtime_of_one_mob_with_comms(&temp_far, "far").await;
        host.set_contact_directory(directory_naming(&far));
        host.register_peer_runtime(&far).await;
        seat(&host, "desk").await;
        seat(&second, "desk").await;
        seat(&far, "front").await;
        let front = facts_of(&far, "front").await;
        let desk_of_the_default = facts_of(&host, "desk").await;
        let desk_of_the_second = facts_of(&second, "desk").await;
        let far_id = far.mob_id();
        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);

        let wired = wire_cross_mob_as_a_handler(&mob, "desk", "front", &far_id).await;
        assert_eq!(wired, Ok(()), "the wire through the resolved mob");
        assert!(
            wired_to(&second, "desk").await.contains(&front.comms_name),
            "the resolved mob's own member holds the edge"
        );
        assert!(
            !wired_to(&host, "desk").await.contains(&front.comms_name),
            "and the default mob's member of that name does not"
        );
        let front_names = wired_to(&far, "front").await;
        assert!(
            front_names.contains(&desk_of_the_second.comms_name)
                && !front_names.contains(&desk_of_the_default.comms_name),
            "the peer mob's member names the resolved mob's member: {front_names:?}"
        );
        assert!(
            holds_a_link_to(&second, &front)
                && holds_a_link_to(&far, &desk_of_the_second)
                && !holds_a_link_to(&host, &front),
            "each of the two mobs holds its link in its own part of the link table"
        );

        let unwired = unwire_cross_mob_as_a_handler(&mob, "desk", "front", &far_id).await;
        assert_eq!(unwired, Ok(()), "the unwire through the resolved mob");
        assert!(
            !wired_to(&second, "desk").await.contains(&front.comms_name)
                && !wired_to(&far, "front")
                    .await
                    .contains(&desk_of_the_second.comms_name),
            "both rows lost the edge"
        );
        assert!(
            !holds_a_link_to(&second, &front) && !holds_a_link_to(&far, &desk_of_the_second),
            "and both parts lost their link"
        );

        let of_the_default = Box::pin(host.wire_cross_mob("desk", "front", &far_id)).await;
        assert!(
            of_the_default.is_ok(),
            "the runtime's own method: {:?}",
            of_the_default.as_ref().err()
        );
        assert!(
            wired_to(&host, "desk").await.contains(&front.comms_name)
                && wired_to(&far, "front")
                    .await
                    .contains(&desk_of_the_default.comms_name)
                && !wired_to(&second, "desk").await.contains(&front.comms_name),
            "the runtime's own method wires the default mob's member"
        );
        let of_the_default = Box::pin(host.unwire_cross_mob("desk", "front", &far_id)).await;
        assert!(
            of_the_default.is_ok() && !wired_to(&host, "desk").await.contains(&front.comms_name),
            "the runtime's own method unwires the default mob's member: {:?}",
            of_the_default.as_ref().err()
        );
        shut_down(&host).await;
        shut_down(&second).await;
        shut_down(&far).await;
    }

    /// Two entries in one runtime: a send through a resolved mob is a send
    /// of that mob's member. A generated alias no identity runtime owns is
    /// refused before any peer mob is looked up, and the refusal names the
    /// mob the member was looked for in: the resolved mob through the
    /// resolved mob, the default mob through the runtime's own method.
    #[tokio::test]
    async fn a_resolved_mobs_send_to_a_peer_mob_is_its_own_members() {
        let host = runtime_of_one_mob("host", false).await;
        let second = runtime_of_one_mob("second", false).await;
        let in_the_mob_of = |runtime: &UnifiedRuntime| format!("in mob '{}'", runtime.mob_id());
        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);

        let refused = send_cross_mob_as_a_handler(&mob, "rt:ghost:1", "front", "a-peer-mob").await;
        assert!(
            refused
                .as_ref()
                .is_err_and(|refusal| refusal.contains(&in_the_mob_of(&second))),
            "the sender is looked for in the resolved mob: {refused:?}"
        );
        let of_the_default =
            Box::pin(host.send_cross_mob("rt:ghost:1", "front", "a-peer-mob", "a message"))
                .await
                .map_err(|error| error.to_string());
        assert!(
            of_the_default
                .as_ref()
                .is_err_and(|refusal| refusal.contains(&in_the_mob_of(&host))),
            "the runtime's own method looks for the sender in the default mob: {of_the_default:?}"
        );
        shut_down(&host).await;
        shut_down(&second).await;
    }

    /// Two entries in one runtime, an identity context on the second mob
    /// alone: a one-sided wire or unwire through a resolved mob, for a
    /// request that brings no identity runtime of its own, asks that mob's
    /// own identity runtime about the member. A generated alias is refused
    /// by the second mob's identity runtime, which does not know it; the
    /// default mob has no identity runtime, and the runtime's own method
    /// says so.
    #[tokio::test]
    async fn a_resolved_mobs_local_wire_asks_its_own_identity_runtime() {
        const NO_IDENTITY_RUNTIME: &str = "require the owning IdentityRuntime";
        let (temp_host, temp_second, temp_far) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let host = runtime(&temp_host).await;
        let mut second = runtime(&temp_second).await;
        let context = super::super::identity_install_phase_tests::context(&second);
        second.install_identity_first_context(context);
        let far = runtime_of_one_mob_with_comms(&temp_far, "far").await;
        seat(&far, "front").await;
        let front = facts_of(&far, "front").await;
        let in_the_second_mob = format!("in mob '{}'", second.mob_id());
        let mob = super::MobEntryRef::second_entry_for_tests(&host, &second);

        for (operation, refused) in [
            (
                "wire",
                wire_local_as_a_handler(&mob, "rt:ghost:1", &front).await,
            ),
            (
                "unwire",
                unwire_local_as_a_handler(&mob, "rt:ghost:1", &front).await,
            ),
        ] {
            assert!(
                refused.as_ref().is_err_and(|refusal| {
                    refusal.contains(&in_the_second_mob) && !refusal.contains(NO_IDENTITY_RUNTIME)
                }),
                "the {operation} asks the resolved mob's own identity runtime: {refused:?}"
            );
        }
        let of_the_default = Box::pin(host.wire_local(
            "rt:ghost:1",
            &front.comms_name,
            &front.peer_id,
            &front.address,
            Some(key_of(&front)),
        ))
        .await
        .map_err(|error| error.to_string());
        assert!(
            of_the_default
                .as_ref()
                .is_err_and(|refusal| refusal.contains(NO_IDENTITY_RUNTIME)),
            "the default mob has no identity runtime: {of_the_default:?}"
        );
        shut_down(&host).await;
        shut_down(&second).await;
        shut_down(&far).await;
    }

    /// The answer of `host`'s stdio dispatch to `method` with `params`:
    /// with `owned`, through the entry that hands the handlers an owner of
    /// the runtime; without, through the one that lends it.
    async fn stdio(
        host: &Arc<UnifiedRuntime>,
        owned: bool,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        })
        .to_string();
        let within = std::time::Duration::from_secs(10);
        let answer = if owned {
            crate::rpc::handle_unified_rpc_json_arc(host, &request, within, None, None)
        } else {
            crate::rpc::handle_unified_rpc_json(host, &request, within, None, None)
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), answer)
            .await
            .ok()
            .and_then(|answer| serde_json::from_str(&answer).ok())
            .unwrap_or(serde_json::Value::Null)
    }

    /// A host that serves a second mob in a test build, a member of one
    /// name in both mobs, and a peer mob the host knows: each of the five
    /// wire requests that names the second mob acts on that mob's own
    /// member, whether the dispatch lends the runtime to the handler or
    /// hands it an owner of it, and never on the default mob's member of
    /// that name.
    #[tokio::test]
    async fn a_request_that_names_a_second_mob_wires_that_mobs_own_member() {
        let (temp_host, temp_second, temp_far) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let mut host = runtime_of_one_mob_with_comms(&temp_host, "host").await;
        let second = runtime_of_one_mob_with_comms(&temp_second, "second").await;
        let far = runtime_of_one_mob_with_comms(&temp_far, "far").await;
        host.set_contact_directory(directory_naming(&far));
        host.register_peer_runtime(&far).await;
        seat(&host, "desk").await;
        seat(&second, "desk").await;
        seat(&far, "front").await;
        let front = facts_of(&far, "front").await;
        let desk_of_the_second = facts_of(&second, "desk").await;
        let (second_id, far_id) = (second.mob_id(), far.mob_id());
        let host = Arc::new(host);
        let second = host.serve_a_second_mob_in_tests(second);
        let accepted =
            |answer: &serde_json::Value| answer["result"]["accepted"].as_bool() == Some(true);

        for owned in [false, true] {
            let path = if owned {
                "an owner of the runtime"
            } else {
                "a lent runtime"
            };
            let one_side = serde_json::json!({
                "mob_id": second_id,
                "local_member_id": "desk",
                "remote_comms_name": front.comms_name,
                "remote_peer_id": front.peer_id,
                "remote_address": front.address,
                "remote_pubkey_b64": front.pubkey_b64,
            });
            let wired = stdio(&host, owned, "meta/cross_mob/wire_local", one_side.clone()).await;
            assert!(accepted(&wired), "{path}: {wired}");
            assert!(
                wired_to(second, "desk").await.contains(&front.comms_name),
                "{path}: the one-sided wire reached the member of the mob the request names"
            );
            assert!(
                !wired_to(&host, "desk").await.contains(&front.comms_name),
                "{path}: and not the default mob's member of that name"
            );
            let unwired = stdio(&host, owned, "meta/cross_mob/unwire_local", one_side).await;
            assert!(accepted(&unwired), "{path}: {unwired}");
            assert!(
                !wired_to(second, "desk").await.contains(&front.comms_name),
                "{path}: the one-sided unwire reached the member of the mob the request names"
            );

            let both_sides = serde_json::json!({
                "mob_id": second_id,
                "local_member_id": "desk",
                "remote_member_id": "front",
                "remote_mob_id": far_id,
            });
            let wired = stdio(&host, owned, "meta/cross_mob/wire", both_sides.clone()).await;
            assert!(accepted(&wired), "{path}: {wired}");
            assert!(
                wired_to(second, "desk").await.contains(&front.comms_name)
                    && wired_to(&far, "front")
                        .await
                        .contains(&desk_of_the_second.comms_name),
                "{path}: the wire joined the member of the mob the request names and the peer"
            );
            assert!(
                !wired_to(&host, "desk").await.contains(&front.comms_name),
                "{path}: and not the default mob's member of that name"
            );
            let unwired = stdio(&host, owned, "meta/cross_mob/unwire", both_sides).await;
            assert!(accepted(&unwired), "{path}: {unwired}");
            assert!(
                !wired_to(second, "desk").await.contains(&front.comms_name)
                    && !wired_to(&far, "front")
                        .await
                        .contains(&desk_of_the_second.comms_name),
                "{path}: the unwire parted the member of the mob the request names and the peer"
            );

            let sent = stdio(
                &host,
                owned,
                "meta/cross_mob/send",
                serde_json::json!({
                    "mob_id": second_id,
                    "from_member_id": "rt:ghost:1",
                    "remote_member_id": "front",
                    "remote_mob_id": far_id,
                    "message": "a message",
                }),
            )
            .await;
            assert!(
                sent["error"]["message"]
                    .as_str()
                    .is_some_and(|refusal| refusal.contains(&format!("in mob '{second_id}'"))),
                "{path}: the sender is looked for in the mob the request names: {sent}"
            );
        }
        shut_down(&host).await;
        shut_down(second).await;
        shut_down(&far).await;
    }

    /// An id this host does not have resolves no mob, the default mob least
    /// of all: the refusal names the id asked for and the default mob. An
    /// empty id is an id like any other.
    #[tokio::test]
    async fn an_id_the_host_does_not_have_is_not_hosted_and_names_both_mobs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = runtime(&temp).await;
        let default_id = runtime.mob_handle().mob_id().clone();

        for requested in ["a-mob-this-host-does-not-have", ""] {
            let requested = meerkat_mob::MobId::from(requested);
            assert_ne!(requested, default_id);
            let refused = runtime.resolve_mob(Some(&requested)).await.err();
            assert_eq!(
                refused,
                Some(super::MobResolveError::NotHosted {
                    requested: requested.clone(),
                    default_mob: default_id.clone(),
                }),
                "an id the host does not have is not hosted"
            );
        }
        runtime.shutdown().await;
    }

    /// A runtime built by the builder, with wiring declared between two
    /// profiles when `wiring` says so and no edge policy of the host's own.
    async fn built(wiring: &str) -> UnifiedRuntime {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"entry-edge-policy-{}\"\n\n{wiring}[profiles.builder]\nmodel = \
             \"gpt-5.5\"\n\n[profiles.reviewer]\nmodel = \"gpt-5.5\"\n",
            uuid::Uuid::new_v4().simple()
        ))
        .expect("mob definition");
        UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build()
            .await
            .expect("runtime builds")
    }

    /// A builder that brings no edge policy of its own keeps the one the
    /// mob's definition declares, in the mob's entry; a definition that
    /// declares no wiring leaves the entry without one.
    #[tokio::test]
    async fn a_builder_without_an_edge_policy_keeps_the_definition_s() {
        let runtime = built("[[wiring.role_wiring]]\na = \"builder\"\nb = \"reviewer\"\n\n").await;
        assert!(
            runtime.default_mob().edge_discovery.is_some(),
            "the definition's declared wiring is the entry's edge policy"
        );
        runtime.shutdown().await;

        let runtime = built("").await;
        assert!(runtime.default_mob().edge_discovery.is_none());
        runtime.shutdown().await;
    }

    /// A runtime that serves a second mob in a test build answers that mob
    /// by its id, as a mob other than its default mob, from the lent
    /// composition's own entry: its handle, its identity context and its
    /// identity request services. A request that names no mob, or the
    /// default mob's id, keeps the default mob; an id no mob has is not
    /// hosted; the resolver a door takes afterwards serves the second mob
    /// too. Before the mob is lent the runtime does not serve it, and the
    /// lent runtime never serves the host's mob.
    #[tokio::test]
    async fn a_runtime_that_serves_a_second_mob_in_tests_resolves_it_from_its_own_entry() {
        let (temp_host, temp_second) = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let host = runtime(&temp_host).await;
        let mut second = runtime(&temp_second).await;
        let context = super::super::identity_install_phase_tests::context(&second);
        second.install_identity_first_context(Arc::clone(&context));
        let floors = Arc::new(crate::identity_first::CompactionFloorRegistry::default());
        second.set_identity_request_services(super::IdentityRequestServices {
            compaction_floors: Some(Arc::clone(&floors)),
            ..Default::default()
        });
        let host_id = host.mob_handle().mob_id().clone();
        let second_id = second.mob_handle().mob_id().clone();
        let absent = meerkat_mob::MobId::from("a-mob-this-host-does-not-have");

        assert!(
            host.resolve_mob(Some(&second_id)).await.is_err()
                && host.served_mob_id(Some(&second_id)).await.is_err(),
            "a runtime serves its default mob alone until a second mob is lent"
        );
        let second = host.serve_a_second_mob_in_tests(second);

        assert_eq!(
            host.served_mob_id(Some(&second_id)).await.ok(),
            Some(second_id.clone()),
            "the second mob is served by its id"
        );
        for named in [None, Some(&host_id)] {
            assert_eq!(
                host.served_mob_id(named).await.ok(),
                Some(host_id.clone()),
                "a request that names no mob, or the default mob, is served by the default mob"
            );
            assert!(
                host.resolve_mob(named).await.is_ok_and(|mob| {
                    mob.is_default()
                        && mob.mob_id() == &host_id
                        && std::ptr::eq(mob.entry(), host.default_mob())
                }),
                "a request that names no mob, or the default mob, resolves the default mob's entry"
            );
        }
        assert!(
            host.served_mob_id(Some(&absent)).await.is_err()
                && matches!(
                    host.resolve_mob(Some(&absent)).await,
                    Err(super::MobResolveError::NotHosted { .. })
                ),
            "an id no mob has is not hosted"
        );

        let resolved = host.resolve_mob(Some(&second_id)).await;
        assert!(resolved.is_ok(), "the second mob resolves by its id");
        if let Ok(mob) = resolved {
            assert!(!mob.is_default(), "as a mob other than the default mob");
            assert_eq!(mob.mob_id(), &second_id);
            assert_eq!(mob.mob_handle().mob_id(), &second_id);
            assert!(
                std::ptr::eq(mob.runtime(), &raw const host),
                "the process part is the host's"
            );
            assert!(
                std::ptr::eq(mob.entry(), second.default_mob()),
                "the entry is the second mob's own"
            );
            assert!(
                mob.identity_runtime_ref()
                    .is_some_and(|identity| Arc::ptr_eq(identity, &context.runtime)),
                "the second mob's own identity context"
            );
            assert!(
                mob.identity_request_services()
                    .and_then(|services| services.compaction_floors)
                    .is_some_and(|given| Arc::ptr_eq(&given, &floors)),
                "the second mob's own identity request services"
            );
        }

        let door = host.mob_resolver();
        assert!(
            door.resolve(Some(&second_id))
                .await
                .is_ok_and(|mob| !mob.is_default() && mob.mob_id() == &second_id),
            "a door's resolver taken afterwards serves the second mob"
        );
        assert!(
            door.resolve(None)
                .await
                .is_ok_and(|mob| mob.is_default() && mob.mob_id() == &host_id),
            "and keeps the default mob for a request that names none"
        );
        assert!(
            second.resolve_mob(Some(&host_id)).await.is_err(),
            "the lent runtime does not serve the host's mob"
        );

        tokio::time::timeout(std::time::Duration::from_secs(30), host.shutdown())
            .await
            .expect("the host shuts down");
        tokio::time::timeout(std::time::Duration::from_secs(30), second.shutdown())
            .await
            .expect("the lent runtime shuts down");
    }
}
