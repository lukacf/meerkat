//! Unified runtime — combines mob lifecycle, module management, and operational subsystems.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use futures::stream::{BoxStream, SelectAll, StreamExt};
use meerkat_core::comms::EventStream;
use meerkat_core::event::{AgentEvent, agent_event_type};
use meerkat_mob::{
    AgentIdentity, AgentRuntimeId, AttributedEvent, FenceToken, MobError, MobHandle,
    MobMemberStatus, MobState, ProfileName, SpawnMemberSpec,
};
use serde_json::json;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;

pub(crate) use self::console_events::ConsoleEventStore;
use self::mob_events::MobEventsStore;
use crate::console_aggregator::{ConsoleLogStore, InMemoryConsoleLogStore};
use crate::mob_handle_runtime::{MobBootstrapSpec, MobRuntime, MobRuntimeError};
use crate::runtime::{
    InMemoryMetadataStore, MetaRuntimeHandle, MetadataScope, PersistentMetadataStore,
    RuntimeMetadataTable, RuntimeOptions, start_meta_runtime_with_options,
};
use crate::types::{
    AgentDiscoverySpec, EventEnvelope, MetaConfig, MobStructuralEventEnvelope, UnifiedEvent,
};

mod bootstrap_completion;
#[cfg(test)]
mod bootstrap_completion_tests;
pub mod builder;
pub mod continuity_mirror;
pub mod continuity_mirror_bigquery;
/// Completes an application from a held identity bootstrap's held members,
/// with the pass's [`crate::identity_first::HeldCompletion`]; see
/// [`UnifiedRuntime::bootstrap_installed_identity_first_context_held_until`].
pub type HeldIdentityCompletion = Box<
    dyn FnOnce(
            Vec<crate::identity_first::HeldIdentityMember>,
            crate::identity_first::HeldCompletion,
        ) -> futures::future::BoxFuture<'static, Result<(), String>>
        + Send,
>;

pub(crate) mod console_events;
pub mod cross_mob;
pub mod edge_reconcile;
pub mod edge_types;
pub mod event_log;
#[cfg(test)]
mod held_identity_add_tests;
#[cfg(test)]
mod held_identity_race_tests;
#[cfg(test)]
pub(crate) mod held_identity_test_support;
pub mod http;
#[cfg(test)]
mod identity_install_phase_tests;
pub(crate) mod implicit_delegate_retirement;
pub mod in_flight_drain;
#[cfg(test)]
pub(crate) mod in_flight_drain_tests;
pub mod lifecycle;
pub mod live_compose;
pub mod mob_events;
pub mod mob_ops;
pub(crate) mod mob_stop;
pub mod module_ops;
#[cfg(test)]
mod retired_member_hook_tests;
#[cfg(test)]
mod shutdown_settlement_tests;
pub mod types;

pub use crate::identity_first::IdentityBootstrapMode;
pub use bootstrap_completion::{
    BootstrapCompletionRefused, BootstrapCompletionUnavailable, RuntimeBootstrapCompleted,
};
pub use builder::UnifiedRuntimeBuilder;
pub use edge_types::{
    DesiredPeerEdge, DesiredPeerEdgeError, Discovery, EdgeDiscovery, EdgeReconcileFailure,
    PreSpawnContext, PreSpawnHook,
};
pub use event_log::{
    EventLogConfig, EventLogError, EventLogStore, EventQuery, NullEventLogStore, PersistedEvent,
};
pub use http::DEFAULT_REFERENCE_APP_MAX_CONCURRENT_REQUESTS;
pub use in_flight_drain::{InFlightDrainOutcome, InFlightDrainReport};
pub use mob_ops::MemberTurnAdmission;
pub use mob_stop::{MOB_STOP_FLOW_SETTLE_BUDGET, MobStopFlowRunsUnsettled};
pub use types::{
    CompactionPreservedHistoryFit, ErrorEvent, IdentityAuthorityReleaseOutcome,
    McpCloseUnsettledClass, MemberMcpSettlement, MemberMcpUnsettled, MobStopOutcome,
    MobTerminalShutdownOutcome, RediscoverReport, ShutdownDrainReport,
    UnifiedRuntimeBootstrapError, UnifiedRuntimeBuilderError, UnifiedRuntimeBuilderField,
    UnifiedRuntimeError, UnifiedRuntimeReconcileEdgesReport, UnifiedRuntimeReconcileError,
    UnifiedRuntimeReconcileReport, UnifiedRuntimeReconcileRoutingReport, UnifiedRuntimeRunReport,
    UnifiedRuntimeShutdownReport,
};

/// Called after members are spawned. Receives the list of spawned member IDs.
pub type PostSpawnHook =
    Arc<dyn Fn(Vec<String>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Called after reconcile completes. Receives the reconcile report.
pub type PostReconcileHook = Arc<
    dyn Fn(UnifiedRuntimeReconcileReport) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

/// Called when a runtime operation fails. Fire-and-forget — the hook's
/// result is not checked and a failing hook cannot break the runtime.
pub type ErrorHook =
    Arc<dyn Fn(ErrorEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Late-bound shared error hook slot (the same pattern as the identity
/// authority and `gateway_peer_keys`): gateways install the hook via
/// [`UnifiedRuntime::set_error_hook`] AFTER construction, while runtime-owned
/// background tasks that must fire it (the actor-loop probe) start AT
/// construction. Tasks hold the slot and read the hook at fire time.
type SharedErrorHook = Arc<std::sync::RwLock<Option<ErrorHook>>>;

fn current_error_hook(slot: &SharedErrorHook) -> Option<ErrorHook> {
    slot.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The default sink every [`ErrorEvent`] passes through, whether or not a
/// host registered an [`ErrorHook`].
///
/// A paging channel that discards when nobody wired it is indistinguishable
/// from a healthy fleet. A host with zero `on_error` call sites used to get
/// silence for every operational failure meta detected — 136 compaction
/// rejections in one observed fleet, found only by reading a console table
/// by hand. Logging here costs a wired host nothing (the hook still fires,
/// exactly once) and gives an unwired one somewhere an operator or a
/// log-shipper can see the event.
///
/// Emitted with the typed event (`Debug`, so the variant name and its fields
/// both land) rather than only the `Display` rendering, and synchronously on
/// the caller's thread so the record is not stranded on a detached task.
pub(crate) fn log_error_event(event: &ErrorEvent, hook_registered: bool) {
    // `ActorLoopRecovered` is the one variant that reports a failure ENDING.
    // Logging a recovery at ERROR would make the level a lie and would put a
    // second scary line in the log for every stall that resolved fine.
    if matches!(
        event,
        ErrorEvent::ActorLoopRecovered { .. }
            | ErrorEvent::MobAnnouncementEscalationResolved { .. }
    ) {
        tracing::info!(
            error_event = ?event,
            hook_registered,
            "meta runtime error event resolved: {event}"
        );
    } else {
        tracing::error!(
            error_event = ?event,
            hook_registered,
            "meta runtime error event: {event}"
        );
    }
    if !hook_registered {
        warn_error_hook_absent_once();
    }
}

/// State the missing-hook condition plainly: nobody is listening, and here is
/// where to fix it. Emitted at build time by `UnifiedRuntimeBuilder::build`
/// for hosts that never called `on_error`, and once per process from the fire
/// path for hosts that bootstrap directly and install the hook afterwards.
pub(crate) fn emit_error_hook_absent_notice() {
    tracing::warn!(
        "no error hook is registered, so runtime error events reach logs only; \
         register one with UnifiedRuntimeBuilder::on_error (or \
         UnifiedRuntime::set_error_hook) to route them to paging"
    );
}

/// The fire-path guard for the notice: the condition is per-process, and
/// repeating it on every event would bury the events themselves.
fn warn_error_hook_absent_once() {
    static NOTICED: std::sync::Once = std::sync::Once::new();
    NOTICED.call_once(emit_error_hook_absent_notice);
}

/// Fire an error event on the hook currently installed in `slot`, if any.
/// Truly fire-and-forget — spawns a detached task so slow hooks (HTTP to
/// Slack, PagerDuty) never block the caller.
///
/// The event reaches [`log_error_event`] either way: an unregistered hook
/// must not be the difference between an operator seeing a failure and not.
fn fire_error_hook(slot: &SharedErrorHook, event: ErrorEvent) {
    let hook = current_error_hook(slot);
    log_error_event(&event, hook.is_some());
    if let Some(hook) = hook {
        tokio::spawn(async move {
            let () = hook(event).await;
        });
    }
}

const ROSTER_ROUTE_PREFIX: &str = "mob.member.";
const ROSTER_ROUTE_CHANNEL: &str = "notification";
const ROSTER_ROUTE_SINK: &str = "mob_member";
const ROSTER_ROUTE_TARGET_MODULE: &str = "delivery";

const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Map an [`AgentDiscoverySpec`] to a [`SpawnMemberSpec`] for spawning.
///
/// `additional_instructions` maps directly to `SpawnMemberSpec.additional_instructions`,
/// which flows through Meerkat's build pipeline to `AgentBuildConfig.additional_instructions`.
pub fn discovery_spec_to_spawn_spec(spec: &AgentDiscoverySpec) -> SpawnMemberSpec {
    let resume_session_id = spec
        .resume_session_id
        .as_deref()
        .and_then(|s| meerkat_core::types::SessionId::parse(s).ok());
    let additional_instructions = if spec.additional_instructions.is_empty() {
        None
    } else {
        Some(spec.additional_instructions.clone())
    };
    let mut spawn = SpawnMemberSpec::new(
        meerkat_mob::ProfileName::from(spec.profile.as_str()),
        // The spec stays in the public alias space: the hook-aware
        // `UnifiedRuntime::spawn`/`spawn_many` own the encode to the
        // comms-safe roster id (meerkat 0.7 MemberCommsName), and the encode
        // is deliberately not idempotent (`mk--` is a reserved marker), so
        // encoding here too would double-encode `:`-bearing identities.
        meerkat_mob::ids::AgentIdentity::from(spec.meerkat_id.as_str()),
    );
    if let Some(context) = spec.context.clone() {
        spawn = spawn.with_context(context);
    }
    if let Some(labels) = spec.labels.clone() {
        spawn = spawn.with_labels(labels);
    }
    if let Some(sid) = resume_session_id {
        spawn = spawn.with_resume_bridge_session_id(sid);
    }
    if let Some(instructions) = additional_instructions {
        spawn = spawn.with_additional_instructions(instructions);
    }
    spawn
}

/// A startup phase of [`UnifiedRuntime::install_and_bootstrap_identity_first_context`]
/// or [`UnifiedRuntime::activate_without_identity_context`], reported to the
/// installed [`UnifiedRuntimeBootstrapPhaseObserver`] as the phase begins.
///
/// Hosts use it to report startup progress (the stdio gateway forwards it as
/// `meta/init_progress`); it never changes what bootstrap does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnifiedRuntimeBootstrapPhase {
    /// Registering each roster identity's customizer-tool dispatcher before
    /// the mob is lifted. The name is kept for the init-progress wire.
    RegisterPersistedOwners,
    /// Lifting the prepared mob and reconciling bootstrap edges.
    Activate,
    /// Applying the initial identity roster (eager restore embodies members).
    RestoreRoster,
    /// Bootstrap failed and the runtime is shutting down, which can await
    /// external cleanup before the error is returned.
    FailureCleanup,
}

/// Receives [`UnifiedRuntimeBootstrapPhase`] transitions. Called inline on
/// the bootstrap task, so it must not block.
pub type UnifiedRuntimeBootstrapPhaseObserver =
    Arc<dyn Fn(UnifiedRuntimeBootstrapPhase) + Send + Sync>;

/// The public lifecycle owner of a composition.
///
/// It holds the mob's public owner (the owning [`MobRuntime`], whose handle
/// carries the native public-owner token) and the runtime state
/// ([`UnifiedRuntimeCore`], reached through `Deref`). The state's own mob
/// tasks, routers, tool state and back-edges hold non-owning observers, so
/// only this value, the handles and runtimes exported from it, and the
/// clones its holders take keep the mob publicly owned. The composition's
/// own identity runtime and session bridge are non-owning back-edges; their
/// exports (`identity_runtime()`, `identity_first_context()`,
/// `session_bridge()`) are separate views that carry the public owner, and a
/// tracked identity foreground operation owns the mob for its whole life.
///
/// Dropping the last public owner without [`UnifiedRuntime::shutdown`] hands
/// the runtime state to a retained teardown (see [`RetainedTeardown`]); the
/// native lifecycle owner decides atomically whether the mob is idle and
/// closes its admission, and only then does the existing awaited shutdown
/// run. A composition with entered work keeps its lifetime.
pub struct UnifiedRuntime {
    core: Arc<UnifiedRuntimeCore>,
    public_owner: Option<MobRuntime>,
    /// False for the retained teardown's own view and for a value whose
    /// state was moved into a rebuilt runtime: neither hands anything over.
    hand_over_on_drop: bool,
}

/// The runtime state of a [`UnifiedRuntime`]. Not constructed or held on its
/// own outside this module; it is public only as the `Deref` target.
#[doc(hidden)]
pub struct UnifiedRuntimeCore {
    #[cfg(feature = "extension-state")]
    extension_authority: Option<Arc<crate::extension_state::NativeAuthorityRegistry>>,
    // Immutable after construction — &self access
    /// Observer-side mob runtime: every internal holder (tasks, routers,
    /// bridges, tool state) is non-owning.
    mob_runtime: MobRuntime,
    /// Set when an explicit shutdown has run to its end; the drop path then
    /// has nothing to hand over.
    teardown_settled: AtomicBool,
    /// This runtime's claims on its state directories (see
    /// [`crate::state_dir_claim`]), held until a shutdown whose mob runtime
    /// stood down cleanly has run to its end, or until the runtime drops: the
    /// shutdown still drains console frames and stops modules after the mob
    /// runtime has released its own share.
    state_dir_claims: std::sync::Mutex<Vec<Arc<crate::state_dir_claim::StateDirClaim>>>,
    /// The report of the shutdown a [`RetainedTeardown`] ran, once it has
    /// run: the observable outcome of a dropped runtime's settlement.
    retained_teardown_report: Arc<std::sync::OnceLock<types::UnifiedRuntimeShutdownReport>>,
    post_spawn_hook: Option<PostSpawnHook>,
    post_reconcile_hook: Option<PostReconcileHook>,
    error_hook: SharedErrorHook,
    drain_timeout: Duration,
    discovery: Option<Box<dyn Discovery>>,
    edge_discovery: Option<Arc<dyn EdgeDiscovery>>,

    // Fine-grained interior mutability
    module_runtime: Arc<tokio::sync::Mutex<MetaRuntimeHandle>>,
    managed_dynamic_edges: Arc<tokio::sync::RwLock<BTreeSet<(String, String)>>>,
    shutting_down: AtomicBool,
    /// The runtime's close begins under this gate, serialized against a
    /// running publication of its bootstrap completion.
    close_gate: Arc<bootstrap_completion::RuntimeCloseGate>,
    /// The one-use bootstrap completion, between its mint and its take.
    bootstrap_completion: std::sync::Mutex<bootstrap_completion::CompletionSlot>,
    mob_event_ingress: tokio::sync::Mutex<Option<MobEventIngress>>,
    bootstrap_edges_report: tokio::sync::RwLock<Option<UnifiedRuntimeReconcileEdgesReport>>,
    /// Optional observer for bootstrap phase transitions (set once).
    bootstrap_phase_observer: std::sync::OnceLock<UnifiedRuntimeBootstrapPhaseObserver>,
    /// Set only for an identity-first resume of a persistent log, where the
    /// mob is deliberately left `Stopped` until continuity is registered.
    /// While this is `Some`, the runtime is NOT ready: the mob cannot spawn
    /// and bootstrap edge reconciliation has been deferred.
    pending_mob_activation:
        tokio::sync::Mutex<Option<crate::mob_handle_runtime::PendingMobActivation>>,
    event_log: Option<event_log::EventLogHandle>,
    console_log_store: Arc<dyn ConsoleLogStore>,
    console_projection: std::sync::OnceLock<crate::console_aggregator::MetaConsoleAggregator>,
    /// What the builder asked the live doors to be (console voice and/or
    /// the external `meta/live/*` channel) plus the typed inputs retained
    /// from a persistent session service. `None` when the builder registered
    /// no live door; the runtime then behaves exactly as before.
    live_plan: Option<live_compose::LivePlan>,
    /// The one live composition per runtime, bound by
    /// [`UnifiedRuntime::compose_live`] after the runtime is shared.
    live_composition: tokio::sync::OnceCell<live_compose::LiveComposition>,
    console_events: ConsoleEventStore,
    mob_events: MobEventsStore,
    mob_events_subscriber_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// Actor-loop liveness probe: a periodic O(1) round trip through the
    /// serialized mob command loop that pages (`ErrorEvent::ActorLoopStalled`)
    /// when the loop stops draining. Observation only — aborted first during
    /// shutdown so the intentional actor stop cannot fire a false page.
    actor_loop_probe_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// The continuity mirror the builder installed. Shutdown stops it, with a
    /// last round, before the mob quiesces; dropping the runtime aborts it.
    continuity_mirror: tokio::sync::Mutex<Option<continuity_mirror::ContinuityMirror>>,
    /// The probe's published verdict (live / stalled / terminated), shared
    /// with the identity-first delivery path so a send issued while a stall
    /// is open fails fast naming the `stall_id` instead of waiting the whole
    /// admission budget behind the wedged command.
    actor_loop_health: Arc<crate::actor_loop_health::ActorLoopHealth>,
    implicit_delegate_retirement_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// Late-bound identity authority observed by the already-running idle
    /// retirement sweeper. Gateways attach identity-first after base runtime
    /// bootstrap, so capturing `identity_runtime()` when the task starts would
    /// permanently capture `None`.
    implicit_delegate_identity_runtime:
        Arc<std::sync::RwLock<Option<Arc<crate::identity_first::IdentityRuntime>>>>,
    /// Woken (an epoch bump) whenever Core's binding of an identity changes
    /// in the installed identity runtime; the identity health monitor's typed
    /// wake for attaching a member that just went live.
    identity_binding_wake: tokio::sync::watch::Sender<u64>,
    /// Per-session sequence through which the identity health monitor and
    /// the completion-cursor drain credited run completions, shared so each
    /// completion is credited exactly once.
    identity_completion_ledger: Arc<HealthCompletionLedger>,
    agent_memory_observer_task:
        tokio::sync::Mutex<Option<crate::memory::taint::TaintObserverGuard>>,
    agent_memory_steward_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,

    // Cross-mob communication
    contact_directory: Option<crate::contact_directory::ContactDirectory>,
    peer_mob_handles: tokio::sync::RwLock<BTreeMap<String, cross_mob::PeerMobAuthority>>,
    /// Serve task of the cross-mob control listener, when one was started
    /// via [`UnifiedRuntime::start_control_listener`]. Aborted on shutdown
    /// like the other runtime-owned background tasks.
    cross_mob_control_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// The dialable address the control listener actually bound
    /// (`tcp://ip:port` with the real port for `host:0` binds, or
    /// `uds:///path`). This is the address remote peers are told to put in
    /// their contact directories, and the address stamped into outbound
    /// remote wire requests as this gateway's control endpoint.
    cross_mob_control_advertised: std::sync::RwLock<Option<String>>,
    /// Long-lived Ed25519 signing identity for cross-process peering.
    /// `None` is the default for inproc-only deployments and tests;
    /// production gateways set this via
    /// [`UnifiedRuntime::set_gateway_peer_keys`] during bootstrap so the
    /// `meta/peer_pubkey` RPC can advertise it and the cross-mob control
    /// listener can sign its responses. A shared late-bound slot (the same
    /// pattern as the identity authority): the control listener may start
    /// before the host installs keys, and its serve task re-reads this
    /// slot per request.
    gateway_peer_keys: crate::runtime::cross_mob_control::ControlSignerSlot,
    /// Late-bound read-only host projection paired with `gateway_peer_keys`.
    /// The listener re-reads both slots, so listener-first and keys-first
    /// bootstrap orders converge on the same authenticated host identity.
    remote_host_facts: crate::runtime::cross_mob_control::HostFactsProviderSlot,
    /// Controller-owned durable endpoint-identity pins plus transient
    /// authenticated reachability. Installed from the same durable state root
    /// as `gateway_peer_keys`; absent for explicitly ephemeral gateways.
    remote_host_lifecycle:
        std::sync::RwLock<Option<Arc<crate::runtime::remote_host::RemoteHostLifecycle>>>,
    /// A corrupt/unreadable pairing file is retained as a typed refusal. The
    /// runtime may continue serving local work, but remote placement cannot
    /// silently start from an empty pin set.
    remote_host_lifecycle_error:
        std::sync::RwLock<Option<crate::runtime::remote_host::HostPairingError>>,
    /// Sole reconnect task. Probes are observations only and are aborted and
    /// joined before the control listener and mob authority shut down.
    remote_host_reconnect_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,

    // Identity-first session bridge
    session_bridge: Option<Arc<dyn crate::identity_first::bridge::SessionBridge>>,
    identity_first_context: Option<Arc<crate::identity_first::IdentityFirstRuntimeContext>>,

    // Optional ABAC enforcement shared by the console/SSE surfaces.
    access_controller: Option<crate::access::AccessController>,

    // The Console's session fence record, shared by every console and SSE
    // route; it exists whether or not access control is installed.
    console_session_fences: crate::auth::session_fence::ConsoleSessionFences,

    // The host's opt-in to Flow Editor host deploys for callers that no
    // enforced access controller governs. Off by default.
    flow_editor_host_deploy: bool,

    // Whether the agent factories hold a provider credential store the host
    // chose instead of the process default, which a host deploy's `rkat`
    // process could not carry. Off by default.
    explicit_provider_credentials: bool,

    // Optional product-level topology authority. The controller always
    // exists so query can remain available, but its policy defaults to
    // disabled and mutation methods are then absent/denied.
    topology_controller: crate::topology_control::TopologyController,

    // Optional panel-capable store handle backing the console Memory
    // panel's read-only RPCs (§9.3). Any provider advertising
    // `MemoryPanelStore` serves it (M4 de-weld). Interior-mutable so
    // gateways can wire it after the runtime is shared (`Arc`), wherever
    // the store is constructed.
    memory_panel_store:
        std::sync::RwLock<Option<Arc<dyn crate::memory::capabilities::MemoryPanelStore>>>,
    /// Rebuildable detached-job health/status projection supplied by the
    /// host that owns the canonical Meerkat job service.
    job_health_projection: Arc<std::sync::RwLock<Option<serde_json::Value>>>,
    // Realm-scoped WorkGraph service backing the `meta/workgraph/*` RPC
    // group and the console experience section. Seeded from the bootstrap
    // spec and deliberately FIXED from then on: the admission guards
    // (cross-process sidecar + agent tool-plane slots) freeze at
    // `MobRuntime::bootstrap`, so a service wired in later would run
    // guard-degraded. The spec (`MobBootstrapSpec::with_workgraph_service`
    // plus admission slot/sidecar) is the only blessed wiring.
    workgraph_service: Option<meerkat::WorkGraphService>,
    /// Runtime-owned lossy wake fan-out. It exists exactly when the
    /// authoritative WorkGraph service exists and is never an authority of
    /// its own.
    workgraph_fact_hub: Option<crate::workgraph_events::WorkGraphFactHub>,
    /// Sole WorkGraph cursor tail feeding `workgraph_fact_hub`. It waits on a
    /// subscriber notification without reading the store while idle.
    workgraph_fact_tail_task: tokio::sync::Mutex<WorkGraphFactTailTask>,
    /// Identity-first console gateways: the mutable desired-identity roster
    /// that `meta/ensure_member` extends at runtime (ask K0). Set by the
    /// host beside `attach_identity_first_context`.
    console_identity_roster:
        std::sync::RwLock<Option<Arc<crate::identity_first::MutableRosterProvider>>>,
    /// Mob announcements: attached once by the host after boot, absent
    /// until then and in every host that did not enable them.
    mob_announcements: crate::mob_announcements::AnnouncementSlot,
    /// The serving side of the keyed inject plane on the cross-mob control
    /// channel. The control listener starts before the receiver is composed,
    /// so it re-reads this per request.
    keyed_inject_slot: crate::runtime::keyed_inject::KeyedInjectSlot,
    /// §16 Q1 provisional operator keying: the console-principal resolver,
    /// shared between the memory coordinator (reads) and the console send
    /// path (notes interactions). `&self`-settable like the panel store.
    console_operator_resolver: std::sync::RwLock<
        Option<Arc<crate::memory::coordinator::ConsolePrincipalOperatorResolver>>,
    >,
    /// The trusted host's governance of Console sends, shared with every
    /// Console router this runtime builds.
    console_governed_ingress: crate::console_governed_ingress::ConsoleGovernedIngressSlot,

    // Meta-side label sidecar for mob- and run-scoped metadata
    metadata_table: Arc<RuntimeMetadataTable>,

    // Persistent metadata adapter (currently used for the structural-events
    // subscription cursor). Falls back to `InMemoryMetadataStore` when not
    // explicitly configured — see `UnifiedRuntimeBuilder::persistent_metadata`.
    persistent_metadata: Arc<dyn PersistentMetadataStore>,
}

enum MobEventIngress {
    Forwarder(MobEventForwarder),
}

struct MobEventForwarder {
    event_rx: Receiver<ForwardedMemberEvent>,
    task: JoinHandle<()>,
    identity_stream_health_task: JoinHandle<()>,
}

/// A forwarded member event: the wire-facing unified envelope plus a
/// drain-side alert extracted at ingest, while the member `AgentEvent` was
/// still typed. The alert never reaches the wire — the drain fires it on
/// the error hook (which gateways install after construction, so the
/// forwarder task cannot capture it) and forwards only the envelope.
struct ForwardedMemberEvent {
    envelope: EventEnvelope<UnifiedEvent>,
    alert: Option<ErrorEvent>,
}

/// Provider-free input to the real runtime drain for composition tests.
#[cfg(test)]
pub(crate) struct TestConsoleEventIngress(Sender<ForwardedMemberEvent>);

#[cfg(test)]
impl TestConsoleEventIngress {
    pub(crate) async fn send(
        &self,
        envelope: EventEnvelope<UnifiedEvent>,
    ) -> Result<(), &'static str> {
        self.0
            .send(ForwardedMemberEvent {
                envelope,
                alert: None,
            })
            .await
            .map_err(|_| "test console ingress is closed")
    }
}

struct WorkGraphFactTailTask(Option<JoinHandle<()>>);

impl WorkGraphFactTailTask {
    fn take(&mut self) -> Option<JoinHandle<()>> {
        self.0.take()
    }
}

impl UnifiedRuntime {
    fn assemble(core: UnifiedRuntimeCore, public_owner: MobRuntime) -> Self {
        Self {
            core: Arc::new(core),
            public_owner: Some(public_owner),
            hand_over_on_drop: true,
        }
    }

    /// Take the runtime state out of this value to rebuild a runtime from it;
    /// this value then hands nothing over when it drops.
    #[allow(
        clippy::expect_used,
        reason = "the state Arc is shared only with a retained teardown, which exists only after drop"
    )]
    pub(crate) fn into_parts(mut self) -> (UnifiedRuntimeCore, MobRuntime) {
        self.hand_over_on_drop = false;
        let public_owner = self
            .public_owner
            .take()
            .expect("a live UnifiedRuntime has its public owner");
        let core = Arc::clone(&self.core);
        drop(self);
        let core = Arc::try_unwrap(core)
            .ok()
            .expect("a live UnifiedRuntime owns its runtime state alone");
        (core, public_owner)
    }

    pub(crate) fn from_parts_unchecked(core: UnifiedRuntimeCore, public_owner: MobRuntime) -> Self {
        Self::assemble(core, public_owner)
    }
}

/// A held bootstrap's prepared-mob activation, run first in its tracked
/// operation (see [`UnifiedRuntime::bootstrap_installed_identity_first_context_held_until`])
/// in the ruled order of the unheld path: register each roster identity's
/// customizer-tool dispatcher, publish the roster's customizer tools, then
/// lift the mob. Each step runs to its end once it is
/// entered, and its failure is the setup's settlement. No step starts once
/// the setup's terminal decision is `Abort` (its deadline passed, or its
/// caller went away): the activation obligation is then dropped unconsumed,
/// as a failed registration leaves it, and the setup settles with that
/// refusal.
fn held_setup_prelude(
    pending: crate::mob_handle_runtime::PendingMobActivation,
    context: Arc<crate::identity_first::IdentityFirstRuntimeContext>,
    roster: Vec<crate::identity_first::DurableAgentSpec>,
    observer: Option<UnifiedRuntimeBootstrapPhaseObserver>,
    terminal: Arc<crate::identity_first::held_bootstrap::HeldTerminal>,
) -> crate::identity_first::held_bootstrap::HeldSetupPrelude {
    Box::pin(async move {
        let report = |phase: UnifiedRuntimeBootstrapPhase| {
            if let Some(observer) = observer.as_ref() {
                observer(phase);
            }
        };
        let ended = || {
            crate::identity_first::IdentityRuntimeError::HeldBootstrapRefused(
                terminal.abort_refusal(),
            )
        };
        if terminal.is_aborted() {
            return Err(ended());
        }
        report(UnifiedRuntimeBootstrapPhase::RegisterPersistedOwners);
        // Core's restart restore attaches each member's customizer tools, so
        // their dispatchers exist before the lift.
        context
            .runtime
            .ensure_customizer_tool_dispatchers(&roster)
            .await;
        // The caller may have been lost meanwhile: no customizer publication
        // starts after `Abort`.
        if terminal.is_aborted() {
            return Err(ended());
        }
        let release_owed = context
            .runtime
            .prepublish_customizer_tools(&roster, context.customizer.as_deref())
            .await;
        if terminal.is_aborted() {
            return Err(ended());
        }
        report(UnifiedRuntimeBootstrapPhase::Activate);
        pending.activate().await.map_err(|error| {
            crate::identity_first::IdentityRuntimeError::Internal(format!(
                "activating the prepared mob: {error}"
            ))
        })?;
        context
            .runtime
            .release_customizer_tool_holds(&release_owed)
            .await;
        report(UnifiedRuntimeBootstrapPhase::RestoreRoster);
        Ok(())
    })
}

/// No lift is staged when the mob resumed Running (an unclean stop left its
/// log Running): meerkat's own resume revived the persisted members while the
/// mob was built, before any activation step, so the publication that
/// precedes a lift never runs. The mob build's early publication ran before
/// the mob handle existed, and meerkat holds the run starts of each restored
/// member it could not publish for. Publish the `owed` members' tools now, as
/// before a lift and with the identity runtime's services, and release the
/// holds of those that publish; the others stay held until their wake or
/// delivery publishes.
async fn publish_customizer_tools_after_resume(
    context: &crate::identity_first::IdentityFirstRuntimeContext,
    owed: &[crate::identity_first::DurableAgentSpec],
) {
    if owed.is_empty() {
        return;
    }
    // Pending before the pass: the mob build's early publication failed, so
    // meerkat holds the member's run starts.
    let held_before = owed
        .iter()
        .map(|spec| {
            context
                .runtime
                .customizer_tools_pending_for(&spec.identity)
                .is_some()
        })
        .collect::<Vec<_>>();
    context
        .runtime
        .ensure_customizer_tool_dispatchers(owed)
        .await;
    let release_owed = context
        .runtime
        .prepublish_customizer_tools(owed, context.customizer.as_deref())
        .await;
    context
        .runtime
        .release_customizer_tool_holds(&release_owed)
        .await;
    // A publication and a release that succeed log nothing per identity, so
    // the pass reports itself once, counting each owed member once:
    // held_released (its hold is released), not_held (Core held it live
    // without a hold and its tools are republished) or still_pending. A
    // member still pending stays held until its wake or delivery publishes:
    // that is a WARN, naming them (each failure's reason is logged above).
    let mut held_released = 0_usize;
    let mut not_held = 0_usize;
    let mut still_pending = Vec::new();
    for (spec, held) in owed.iter().zip(held_before) {
        if context
            .runtime
            .customizer_tools_pending_for(&spec.identity)
            .is_some()
        {
            still_pending.push(spec.identity.to_string());
        } else if held {
            held_released += 1;
        } else {
            not_held += 1;
        }
    }
    if still_pending.is_empty() {
        tracing::info!(
            held_released,
            not_held,
            "published the customizer tools of the members restored while the mob resumed \
             Running"
        );
    } else {
        tracing::warn!(
            held_released,
            not_held,
            still_pending_count = still_pending.len(),
            still_pending = %still_pending.join(","),
            "the customizer tools of members restored while the mob resumed Running are not \
             published; each starts no run until its wake or delivery publishes them"
        );
    }
}

/// [`publish_customizer_tools_after_resume`] as a held bootstrap's prelude,
/// run in its tracked operation like [`held_setup_prelude`]: it starts no
/// publication once the setup's terminal decision is `Abort`.
fn held_resume_prelude(
    context: Arc<crate::identity_first::IdentityFirstRuntimeContext>,
    owed: Vec<crate::identity_first::DurableAgentSpec>,
    observer: Option<UnifiedRuntimeBootstrapPhaseObserver>,
    terminal: Arc<crate::identity_first::held_bootstrap::HeldTerminal>,
) -> crate::identity_first::held_bootstrap::HeldSetupPrelude {
    Box::pin(async move {
        if terminal.is_aborted() {
            return Err(
                crate::identity_first::IdentityRuntimeError::HeldBootstrapRefused(
                    terminal.abort_refusal(),
                ),
            );
        }
        publish_customizer_tools_after_resume(&context, &owed).await;
        if let Some(observer) = observer.as_ref() {
            observer(UnifiedRuntimeBootstrapPhase::RestoreRoster);
        }
        Ok(())
    })
}

impl std::ops::Deref for UnifiedRuntime {
    type Target = UnifiedRuntimeCore;

    fn deref(&self) -> &UnifiedRuntimeCore {
        &self.core
    }
}

impl std::ops::DerefMut for UnifiedRuntime {
    #[allow(
        clippy::expect_used,
        reason = "the state Arc is shared only with a retained teardown, which exists only after drop"
    )]
    fn deref_mut(&mut self) -> &mut UnifiedRuntimeCore {
        Arc::get_mut(&mut self.core).expect("a live UnifiedRuntime owns its runtime state alone")
    }
}

/// The retained teardown of a runtime whose last public owner was dropped
/// without [`UnifiedRuntime::shutdown`].
///
/// Ownership of the whole runtime state transfers here at drop; the drop
/// path is disarmed only by this transfer (or by an explicit shutdown that
/// ran to its end), never by shutdown entry. The teardown waits for the
/// native lifecycle owner to close the mob's admission (it does so
/// atomically, only when the mob is idle, once its last public owner is
/// gone); a mob with entered work keeps everything, held here. After the
/// close it runs the existing awaited [`UnifiedRuntime::shutdown`] over the
/// same participants and reports its real outcome, including the terminal
/// mob teardown. Settlement is bounded by the executor that carries this
/// task: if that executor is gone, nothing has been settled and nothing is
/// claimed.
struct RetainedTeardown {
    core: Arc<UnifiedRuntimeCore>,
}

impl RetainedTeardown {
    async fn run(self) {
        let mut closed = self.core.mob_runtime.handle().admission_closed();
        let reason = loop {
            if let Some(reason) = *closed.borrow_and_update() {
                break Some(reason);
            }
            if closed.changed().await.is_err() {
                // The actor is gone: its own teardown already ran.
                break None;
            }
        };
        let runtime = UnifiedRuntime {
            core: self.core,
            public_owner: None,
            hand_over_on_drop: false,
        };
        let report = runtime.shutdown().await;
        let observed = Arc::clone(&runtime.retained_teardown_report);
        drop(runtime);
        let report = observed.get_or_init(|| report);
        let terminal = &report.mob_terminal_shutdown;
        if report.cleanup_completed()
            && matches!(
                terminal,
                types::MobTerminalShutdownOutcome::Completed(native) if native.is_clean()
            )
        {
            tracing::info!(
                ?reason,
                "a dropped idle composition closed its admission and shut down"
            );
        } else {
            tracing::warn!(
                ?reason,
                cleanup_completed = report.cleanup_completed(),
                mob_terminal_shutdown = ?terminal,
                "a dropped idle composition shut down without complete cleanup"
            );
        }
    }
}

impl Drop for UnifiedRuntime {
    fn drop(&mut self) {
        let public_owner = self.public_owner.take();
        if !self.hand_over_on_drop
            || self
                .core
                .teardown_settled
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let core = Arc::clone(&self.core);
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            // Nothing can settle without an executor; the state goes as it
            // would have before, and the mob keeps whatever it holds.
            return;
        };
        // This runtime's own public owner is released here; the native
        // lifecycle owner closes admission only once no public owner remains
        // and the mob is idle.
        drop(public_owner);
        executor.spawn(RetainedTeardown { core }.run());
    }
}

impl Drop for WorkGraphFactTailTask {
    fn drop(&mut self) {
        // A JoinHandle detaches on drop. Failed construction paths do not get
        // an async shutdown boundary, so abort here; graceful shutdown takes
        // the handle first and also joins it.
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

impl UnifiedRuntime {
    pub fn builder() -> UnifiedRuntimeBuilder {
        UnifiedRuntimeBuilder::default()
    }

    #[allow(
        unknown_lints,
        clippy::unused_async_trait_impl,
        reason = "preserve the async construction seam used by runtime bootstrap"
    )]
    pub(crate) async fn from_parts(
        mob_runtime: MobRuntime,
        module_runtime: MetaRuntimeHandle,
        persistent_metadata: Arc<dyn PersistentMetadataStore>,
    ) -> Self {
        // The given runtime is the mob's public owner and stays with the
        // returned value; everything built below holds the observer side.
        let public_owner = mob_runtime.clone();
        let mob_runtime = mob_runtime.observer();
        // Construct the metadata table first so the structural-events store
        // can be wired with it — every projected envelope picks up the
        // matching mob/run labels at projection time.
        let metadata_table = Arc::new(RuntimeMetadataTable::new());
        let mob_events_store = MobEventsStore::new().with_metadata_table(metadata_table.clone());
        let identity_runtime_authority = Arc::new(std::sync::RwLock::new(None));
        let identity_binding_wake = tokio::sync::watch::channel(0_u64).0;
        let identity_completion_ledger = Arc::new(HealthCompletionLedger::default());
        let mob_event_ingress = Some(Self::create_event_ingress(
            mob_runtime.handle(),
            mob_runtime.agent_mob_mcp_state(),
            mob_events_store.clone(),
            Arc::clone(&identity_runtime_authority),
            Arc::clone(&identity_completion_ledger),
            identity_binding_wake.subscribe(),
        ));
        let mob_events_task = Self::spawn_mob_events_subscriber(
            mob_runtime.handle(),
            mob_events_store.clone(),
            persistent_metadata.clone(),
            mob_runtime.implicit_delegate_retirement_overrides(),
        );
        // The probe starts at construction while the error hook is installed
        // later (`set_error_hook`), so it holds the late-bound slot and reads
        // the hook at fire time.
        let error_hook: SharedErrorHook = Arc::new(std::sync::RwLock::new(None));
        let actor_loop_health = crate::actor_loop_health::ActorLoopHealth::shared();
        let actor_loop_probe_task = Self::spawn_actor_loop_probe(
            mob_runtime.handle(),
            Arc::clone(&error_hook),
            Arc::clone(&actor_loop_health),
        );
        let console_events = ConsoleEventStore::new();
        if let Some(binary) = mob_runtime.binary_blob_store() {
            // The session store's own adapter: content-addressed image ids
            // match the ones persisted history carries.
            console_events.install_blob_store(Arc::new(
                crate::blob_store::Base64BlobStoreAdapter::new(binary),
            ));
        }
        // Agent-tool spawns (mob_spawn_member/delegate) project their members
        // into this runtime's console event store so spawned workers are
        // visible in the console without embedder-side workarounds.
        mob_runtime.install_console_spawn_sink(crate::console_spawn::ConsoleSpawnSink::new(
            console_events.clone(),
        ));
        let workgraph_service = mob_runtime.workgraph_service();
        let (workgraph_fact_hub, workgraph_fact_tail_task) =
            if let Some(service) = workgraph_service.clone() {
                let hub = crate::workgraph_events::WorkGraphFactHub::new();
                let task = crate::workgraph_events::spawn_workgraph_fact_tail(
                    service,
                    hub.clone(),
                    crate::workgraph_events::WorkGraphFactTailOptions::default(),
                );
                (Some(hub), Some(task))
            } else {
                (None, None)
            };
        let definition_edge_discovery =
            edge_reconcile::DefinitionWiringEdgeDiscovery::from_definition(
                mob_runtime.handle().definition(),
            )
            .map(|policy| Arc::new(policy) as Arc<dyn EdgeDiscovery>);
        let state_dir_claims = mob_runtime.state_dir_claim_refs();
        Self::assemble(
            UnifiedRuntimeCore {
                mob_runtime,
                #[cfg(feature = "extension-state")]
                extension_authority: None,
                teardown_settled: AtomicBool::new(false),
                state_dir_claims: std::sync::Mutex::new(state_dir_claims),
                retained_teardown_report: Arc::new(std::sync::OnceLock::new()),
                post_spawn_hook: None,
                post_reconcile_hook: None,
                error_hook,
                drain_timeout: DEFAULT_DRAIN_TIMEOUT,
                discovery: None,
                // Default the edge policy to the definition's declared wiring
                // (auto_wire_orchestrator / role_wiring): upstream applies those
                // rules only at spawn time and only from the non-orchestrator
                // side, so bring-up order and restarts leave declared crews
                // unwired (a downstream app, 2026-07-09). With the default installed,
                // `reconcile_edges` converges the roster onto the declaration;
                // embedder-supplied policies (builder) override it.
                edge_discovery: definition_edge_discovery,
                module_runtime: Arc::new(tokio::sync::Mutex::new(module_runtime)),
                managed_dynamic_edges: Arc::new(tokio::sync::RwLock::new(BTreeSet::new())),
                shutting_down: AtomicBool::new(false),
                close_gate: bootstrap_completion::RuntimeCloseGate::new(),
                bootstrap_completion: std::sync::Mutex::new(
                    bootstrap_completion::CompletionSlot::Pending,
                ),
                mob_event_ingress: tokio::sync::Mutex::new(mob_event_ingress),
                bootstrap_edges_report: tokio::sync::RwLock::new(None),
                bootstrap_phase_observer: std::sync::OnceLock::new(),
                pending_mob_activation: tokio::sync::Mutex::new(None),
                event_log: None,
                console_log_store: Arc::new(InMemoryConsoleLogStore::new()),
                console_projection: std::sync::OnceLock::new(),
                live_plan: None,
                live_composition: tokio::sync::OnceCell::new(),
                console_events,
                mob_events: mob_events_store,
                mob_events_subscriber_task: tokio::sync::Mutex::new(mob_events_task),
                actor_loop_probe_task: tokio::sync::Mutex::new(actor_loop_probe_task),
                continuity_mirror: tokio::sync::Mutex::new(None),
                actor_loop_health,
                implicit_delegate_retirement_task: tokio::sync::Mutex::new(None),
                implicit_delegate_identity_runtime: identity_runtime_authority,
                identity_binding_wake,
                identity_completion_ledger,
                agent_memory_observer_task: tokio::sync::Mutex::new(None),
                agent_memory_steward_task: tokio::sync::Mutex::new(None),
                contact_directory: None,
                peer_mob_handles: tokio::sync::RwLock::new(BTreeMap::new()),
                cross_mob_control_task: tokio::sync::Mutex::new(None),
                cross_mob_control_advertised: std::sync::RwLock::new(None),
                gateway_peer_keys: crate::runtime::cross_mob_control::unsigned_control_signer(),
                remote_host_facts: crate::runtime::cross_mob_control::empty_host_facts_provider(),
                remote_host_lifecycle: std::sync::RwLock::new(None),
                remote_host_lifecycle_error: std::sync::RwLock::new(None),
                remote_host_reconnect_task: tokio::sync::Mutex::new(None),
                session_bridge: None,
                identity_first_context: None,
                access_controller: None,
                console_session_fences: crate::auth::session_fence::ConsoleSessionFences::detached(
                ),
                flow_editor_host_deploy: false,
                explicit_provider_credentials: false,
                topology_controller: crate::topology_control::TopologyController::default(),
                memory_panel_store: std::sync::RwLock::new(None),
                job_health_projection: Arc::new(std::sync::RwLock::new(None)),
                workgraph_service,
                workgraph_fact_hub,
                workgraph_fact_tail_task: tokio::sync::Mutex::new(WorkGraphFactTailTask(
                    workgraph_fact_tail_task,
                )),
                console_identity_roster: std::sync::RwLock::new(None),
                mob_announcements: Default::default(),
                keyed_inject_slot: crate::runtime::keyed_inject::pending_keyed_inject_slot(),
                console_operator_resolver: std::sync::RwLock::new(None),
                console_governed_ingress: Default::default(),
                metadata_table,
                persistent_metadata,
            },
            public_owner,
        )
    }

    /// Spawn a background task that opens a streaming subscription to
    /// the meerkat mob event ledger and projects each [`MobEvent`] into
    /// the runtime's [`MobEventsStore`]. The task resumes from the
    /// last-projected cursor recorded in `persistent_metadata`, so the
    /// SDK-side cursor is durable across meta restarts on
    /// SQLite-backed deployments.
    ///
    /// Returns `None` when there is no current tokio runtime (e.g. unit
    /// tests outside an async context); in that case the store is still
    /// usable via direct projection.
    fn spawn_mob_events_subscriber(
        handle: MobHandle,
        store: MobEventsStore,
        persistent_metadata: Arc<dyn PersistentMetadataStore>,
        idle_retire_overrides: Option<
            crate::mob_handle_runtime::ImplicitDelegateRetirementOverrides,
        >,
    ) -> Option<JoinHandle<()>> {
        let runtime_handle = tokio::runtime::Handle::try_current().ok()?;
        Some(runtime_handle.spawn(run_mob_events_subscription(
            handle,
            store,
            persistent_metadata,
            idle_retire_overrides,
        )))
    }

    /// Spawn the actor-loop liveness probe (see [`run_actor_loop_probe`]).
    ///
    /// The probe round trip is `MobHandle::status()` → `MobCommand::QueryPhase`:
    /// the cheapest command the handle exposes — the actor's handler is a pure
    /// in-memory phase read (`reply_tx.send(Ok(self.state()))`), read-only and
    /// O(1) — while still riding the same serialized command loop whose stall
    /// it exists to detect.
    ///
    /// Returns `None` when there is no current tokio runtime (e.g. unit
    /// tests outside an async context), like the events subscriber above.
    fn spawn_actor_loop_probe(
        handle: MobHandle,
        error_hook: SharedErrorHook,
        health: Arc<crate::actor_loop_health::ActorLoopHealth>,
    ) -> Option<JoinHandle<()>> {
        let runtime_handle = tokio::runtime::Handle::try_current().ok()?;
        Some(runtime_handle.spawn(run_actor_loop_probe(
            move || {
                let handle = handle.clone();
                async move { handle.status().await }
            },
            error_hook,
            health,
            actor_loop_probe_interval(),
            actor_loop_probe_budget(),
        )))
    }

    /// The actor-loop probe's shared verdict. Read by the identity-first
    /// delivery path (fail fast while a stall is open) and by
    /// `meta/member_health`.
    pub fn actor_loop_health(&self) -> &Arc<crate::actor_loop_health::ActorLoopHealth> {
        &self.actor_loop_health
    }

    pub async fn bootstrap(
        mob_spec: MobBootstrapSpec,
        module_config: MetaConfig,
        timeout: Duration,
    ) -> Result<Self, UnifiedRuntimeBootstrapError> {
        crate::boxed_future::boxed(|| {
            Self::bootstrap_with_options(
                mob_spec,
                module_config,
                Vec::new(),
                timeout,
                RuntimeOptions::default(),
                Arc::new(InMemoryMetadataStore::new()),
            )
        })
        .await
    }

    pub async fn bootstrap_with_options(
        mob_spec: MobBootstrapSpec,
        module_config: MetaConfig,
        module_agent_events: Vec<EventEnvelope<UnifiedEvent>>,
        timeout: Duration,
        options: RuntimeOptions,
        persistent_metadata: Arc<dyn PersistentMetadataStore>,
    ) -> Result<Self, UnifiedRuntimeBootstrapError> {
        crate::boxed_future::boxed(|| {
            Self::bootstrap_with_options_and_topology(
                mob_spec,
                module_config,
                module_agent_events,
                timeout,
                options,
                persistent_metadata,
                crate::topology_control::TopologyBootstrapConfig::default(),
            )
        })
        .await
    }

    /// Bootstrap the legacy runtime with the ordinary defaults plus an
    /// explicit optional topology-control configuration.
    ///
    /// This is the concise opt-in for embedders that do not otherwise need
    /// custom module events, runtime options, or metadata storage.
    pub async fn bootstrap_with_topology(
        mob_spec: MobBootstrapSpec,
        module_config: MetaConfig,
        timeout: Duration,
        topology: crate::topology_control::TopologyBootstrapConfig,
    ) -> Result<Self, UnifiedRuntimeBootstrapError> {
        crate::boxed_future::boxed(|| {
            Self::bootstrap_with_options_and_topology(
                mob_spec,
                module_config,
                Vec::new(),
                timeout,
                RuntimeOptions::default(),
                Arc::new(InMemoryMetadataStore::new()),
                topology,
            )
        })
        .await
    }

    /// Legacy bootstrap with an explicit optional topology-control seam.
    ///
    /// The default remains query-only with mutation disabled. Supplying an
    /// editable policy does not bypass console authentication or ABAC; every
    /// RPC mutation is still authorized against both endpoint resources.
    /// Supplying `state_path` makes desired additions, suppression tombstones,
    /// revisions, idempotency records, and recovery journals durable.
    #[allow(clippy::too_many_arguments)]
    pub async fn bootstrap_with_options_and_topology(
        mob_spec: MobBootstrapSpec,
        module_config: MetaConfig,
        module_agent_events: Vec<EventEnvelope<UnifiedEvent>>,
        timeout: Duration,
        options: RuntimeOptions,
        persistent_metadata: Arc<dyn PersistentMetadataStore>,
        topology: crate::topology_control::TopologyBootstrapConfig,
    ) -> Result<Self, UnifiedRuntimeBootstrapError> {
        let topology_authority = mob_spec.definition.id.to_string();
        let topology_controller = match topology.state_path {
            Some(path) => {
                crate::topology_control::TopologyController::load_or_default(topology.policy, path)
            }
            None => crate::topology_control::TopologyController::new(topology.policy),
        }
        .map_err(|error| UnifiedRuntimeBootstrapError::Topology(error.to_string()))?;
        topology_controller
            .bind_authority(topology_authority)
            .await
            .map_err(|error| UnifiedRuntimeBootstrapError::Topology(error.to_string()))?;
        // `prepare`, not `bootstrap`: an identity-first resume of a persistent
        // log must stay Stopped until its continuity owners are registered,
        // because the lift is what triggers session revival.
        let (mob_runtime, pending_mob_activation) =
            crate::boxed_future::boxed(|| MobRuntime::prepare(mob_spec))
                .await
                .map_err(UnifiedRuntimeBootstrapError::Mob)?;
        let runtime_options = options.clone();
        let module_start_result = std::thread::spawn(move || {
            start_meta_runtime_with_options(module_config, module_agent_events, timeout, options)
        })
        .join();

        match module_start_result {
            Ok(Ok(module_runtime)) => {
                let mut runtime = crate::boxed_future::boxed(|| {
                    Self::from_parts(mob_runtime, module_runtime, persistent_metadata)
                })
                .await;
                runtime.topology_controller = topology_controller;
                // Configuration-only; dispatches nothing into the mob.
                runtime
                    .configure_implicit_delegate_retirement(&runtime_options)
                    .await;
                let staged = pending_mob_activation.is_some();
                *runtime.pending_mob_activation.lock().await = pending_mob_activation;
                if staged {
                    // DEFERRED, not skipped. Reconciliation here would take the
                    // non-identity authority path (no identity context exists
                    // yet) and wire or unwire through a Stopped mob. It runs
                    // after activation, through the identity context, and
                    // `bootstrap_edges_report` stays None until then so an
                    // observer cannot read an absent report as "reconciled, no
                    // changes".
                    tracing::info!(
                        "identity-first resume: mob left Stopped pending continuity registration; \
                         bootstrap edge reconciliation deferred until activation"
                    );
                } else {
                    crate::boxed_future::boxed(|| {
                        runtime.reconcile_bootstrap_edges_if_configured()
                    })
                    .await;
                }
                Ok(runtime)
            }
            Ok(Err(error)) => {
                let startup_error = UnifiedRuntimeBootstrapError::Module(error);
                crate::boxed_future::boxed(|| {
                    Self::rollback_mob_runtime(mob_runtime, startup_error)
                })
                .await
            }
            Err(_) => {
                let startup_error = UnifiedRuntimeBootstrapError::ModuleStartupThreadPanicked;
                crate::boxed_future::boxed(|| {
                    Self::rollback_mob_runtime(mob_runtime, startup_error)
                })
                .await
            }
        }
    }

    /// Complete bootstrap when the host declares that no identity context
    /// will be installed. Agent tools can retain an identity slot even for
    /// this composition, so slot presence alone cannot discharge activation.
    ///
    /// Identity compositions must instead use
    /// [`Self::install_and_bootstrap_identity_first_context`] to register
    /// continuity owners before resumed sessions become runnable.
    ///
    /// On `Ok` it mints the runtime's bootstrap completion
    /// ([`Self::take_bootstrap_completion`]), also when nothing was pending:
    /// a runtime built by a legacy constructor ([`Self::bootstrap`] and the
    /// `bootstrap_with_*` family) is already active, but it has no
    /// completion until this returns.
    pub async fn activate_without_identity_context(&mut self) -> Result<(), MobRuntimeError> {
        if self.identity_first_context.is_some() {
            return Err(MobRuntimeError::InvalidConfig(
                "cannot activate without identity authority after an identity context is installed"
                    .to_string(),
            ));
        }
        let pending = self.pending_mob_activation.lock().await.take();
        if let Some(pending) = pending {
            self.report_bootstrap_phase(UnifiedRuntimeBootstrapPhase::Activate);
            if let Err(error) = pending.activate().await {
                self.report_bootstrap_phase(UnifiedRuntimeBootstrapPhase::FailureCleanup);
                self.shutdown().await;
                return Err(error);
            }
            self.reconcile_bootstrap_edges_if_configured().await;
        }
        self.mint_bootstrap_completion();
        Ok(())
    }

    /// Install the observer for [`UnifiedRuntimeBootstrapPhase`] transitions.
    /// Returns `false` (and keeps the first) when one is already installed.
    pub fn set_bootstrap_phase_observer(
        &self,
        observer: UnifiedRuntimeBootstrapPhaseObserver,
    ) -> bool {
        self.bootstrap_phase_observer.set(observer).is_ok()
    }

    fn report_bootstrap_phase(&self, phase: UnifiedRuntimeBootstrapPhase) {
        if let Some(observer) = self.bootstrap_phase_observer.get() {
            observer(phase);
        }
    }

    /// Run bootstrap edge reconciliation if this runtime was configured for it.
    ///
    /// One rule, shared by the immediate path and the deferred identity-first
    /// path, so the two cannot drift on WHEN reconciliation is owed.
    async fn reconcile_bootstrap_edges_if_configured(&self) {
        if self.edge_discovery.is_some()
            || self.topology_controller.revision().await > 0
            || self.topology_controller.has_pending().await
        {
            let report = self.reconcile_edges().await;
            *self.bootstrap_edges_report.write().await = Some(report);
        }
    }

    /// Bootstrap edge reconciliation report, if edge discovery was configured.
    ///
    /// Inspect after `build()` to detect incomplete startup topology.
    /// Returns `None` if no edge discovery was configured.
    pub async fn bootstrap_edges_report(&self) -> Option<UnifiedRuntimeReconcileEdgesReport> {
        self.bootstrap_edges_report.read().await.clone()
    }

    /// Register an error hook after construction. Useful when the runtime
    /// is built via `bootstrap()` rather than the builder.
    pub fn set_error_hook(&mut self, hook: ErrorHook) {
        *self
            .error_hook
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook.clone());
        if let Some(identity_runtime) = self.identity_runtime_ref() {
            identity_runtime.set_error_hook(Some(hook));
        }
    }

    /// Start the event log ingestion engine. Must be called after
    /// construction (the builder calls this automatically when event_log
    /// config is provided).
    pub fn start_event_log(&mut self, config: EventLogConfig) {
        let handle = event_log::start_event_log(config, current_error_hook(&self.error_hook));
        self.event_log = Some(handle);
    }

    pub(crate) fn console_events(&self) -> ConsoleEventStore {
        self.console_events.clone()
    }

    /// A §9.3 memory-event sink projecting typed memory-plane events onto
    /// the console timeline (standard `ConsoleIdentityEventEnvelope`,
    /// `event_type = "memory.*"`). Must be called from async context — the
    /// sink captures the current runtime handle so sync emitters
    /// (store/taint/guard code) can fire-and-forget.
    pub fn memory_event_sink(&self) -> Arc<dyn crate::memory::events::MemoryEventSink> {
        Arc::new(ConsoleMemoryEventSink {
            store: self.console_events(),
            handle: tokio::runtime::Handle::current(),
        })
    }

    /// Register an observer for gating pending-entry resolutions
    /// (decisions and timeout fallbacks) — the seam the memory steward's
    /// gated promotions commit through (§10.2).
    pub async fn register_gating_resolution_observer(
        &self,
        observer: Arc<dyn crate::runtime::GatingResolutionObserver>,
    ) {
        self.module_runtime
            .lock()
            .await
            .register_gating_resolution_observer(observer);
    }

    /// Internal accessor used by console-facing RPC routers to share the
    /// in-memory structural mob events store without holding a full
    /// runtime reference.
    pub(crate) fn mob_events_store(&self) -> MobEventsStore {
        self.mob_events.clone()
    }

    pub fn binary_blob_store(&self) -> Option<Arc<dyn crate::blob_store::BinaryBlobStore>> {
        self.mob_runtime.binary_blob_store()
    }

    /// Publish the latest rebuildable detached-job observability projection.
    ///
    /// Lifecycle remains owned by Meerkat's generated job machine; this slot
    /// exists only so status, capability, console, and health surfaces can
    /// expose the host-owned projection without a parallel semantic store.
    pub fn set_job_health_projection(&self, projection: Option<serde_json::Value>) {
        *self
            .job_health_projection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = projection;
    }

    pub fn job_health_projection(&self) -> Option<serde_json::Value> {
        self.job_health_projection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn module_runtime_handle(&self) -> Arc<tokio::sync::Mutex<MetaRuntimeHandle>> {
        Arc::clone(&self.module_runtime)
    }

    pub(crate) fn mobpack_runtime_catalog_state_snapshot(
        &self,
    ) -> crate::mobpack::MobpackRuntimeCatalogState {
        let loaded_modules = self
            .module_runtime
            .try_lock()
            .map(|runtime| runtime.loaded_modules())
            .unwrap_or_default();
        let has_peer_mob_handles = self
            .peer_mob_handles
            .try_read()
            .map(|handles| !handles.is_empty())
            .unwrap_or(false);
        let mut runtime_methods = vec![
            "meta/capabilities".to_string(),
            "meta/models/catalog".to_string(),
            "meta/spawn_member".to_string(),
            "meta/list_members".to_string(),
            "meta/get_member".to_string(),
            "meta/run_flow".to_string(),
            "meta/list_flows".to_string(),
            "meta/list_runs".to_string(),
        ];
        runtime_methods.extend(
            crate::rpc::MOBPACK_AUTHORING_METHODS
                .iter()
                .map(std::string::ToString::to_string),
        );
        if self.has_contact_directory() {
            runtime_methods.push("meta/cross_mob/directory".to_string());
        }
        if (has_peer_mob_handles && self.has_inproc_contacts()) || self.has_remote_contacts() {
            runtime_methods.extend([
                "meta/cross_mob/wire".to_string(),
                "meta/cross_mob/unwire".to_string(),
                "meta/cross_mob/send".to_string(),
            ]);
        }
        let mob_handle = self.mob_handle();
        let definition = mob_handle.definition();
        // The runtime projects its own definition, as the host.
        let (runtime_flow_rows, runtime_agent_definition_sources, runtime_skill_realms) =
            crate::mobpack::with_host_path_skill_reads(|| {
                (
                    crate::mobpack::runtime_flow_registry_rows_from_definition(definition),
                    crate::mobpack::runtime_agent_definition_sources_from_definition(definition),
                    crate::mobpack::runtime_skill_realms_from_definition(definition),
                )
            });
        crate::mobpack::MobpackRuntimeCatalogState {
            loaded_modules,
            runtime_methods,
            has_contact_directory: self.has_contact_directory(),
            has_peer_mob_handles,
            has_inproc_contacts: self.has_inproc_contacts(),
            runtime_flow_rows,
            runtime_agent_definition_sources,
            runtime_skill_realms,
        }
    }

    /// The session bridge for identity-first operations, if configured, as
    /// an exported view carrying the mob's public owner (D67).
    pub fn session_bridge(&self) -> Option<crate::identity_first::PublicSessionBridge> {
        self.session_bridge.as_ref().map(|bridge| {
            crate::identity_first::PublicSessionBridge::new(
                Arc::clone(bridge),
                self.mob_runtime().clone(),
            )
        })
    }

    /// The identity-first context as an exported view carrying the mob's
    /// public owner (D67).
    pub fn identity_first_context(
        &self,
    ) -> Option<crate::identity_first::PublicIdentityFirstContext> {
        self.identity_first_context.as_ref().map(|context| {
            crate::identity_first::PublicIdentityFirstContext::new(
                Arc::clone(context),
                self.mob_runtime().clone(),
            )
        })
    }

    /// The identity runtime as an exported view carrying the mob's public
    /// owner (D67).
    pub fn identity_runtime(&self) -> Option<crate::identity_first::PublicIdentityRuntime> {
        self.identity_first_context.as_ref().map(|context| {
            crate::identity_first::PublicIdentityRuntime::new(
                Arc::clone(&context.runtime),
                self.mob_runtime().clone(),
            )
        })
    }

    /// The composition's own (non-owning) identity runtime, for its internal
    /// holders: they are back-edges and never keep the mob owned.
    pub(crate) fn identity_runtime_ref(
        &self,
    ) -> Option<&Arc<crate::identity_first::IdentityRuntime>> {
        self.identity_first_context.as_ref().map(|ctx| &ctx.runtime)
    }

    pub async fn remember_agent_memory(
        &self,
        realm: &str,
        identity: &crate::identity_first::AgentIdentity,
        memory: crate::identity_first::NewAgentMemory,
    ) -> Result<crate::identity_first::AgentMemoryRecord, crate::identity_first::AgentMemoryError>
    {
        let runtime = self.identity_runtime_ref().ok_or_else(|| {
            crate::identity_first::AgentMemoryError::InvalidConfig(
                "identity-first runtime is not configured".to_string(),
            )
        })?;
        runtime.remember_agent_memory(realm, identity, memory).await
    }

    pub async fn recall_agent_memory(
        &self,
        request: crate::identity_first::AgentMemoryRecallRequest,
    ) -> Result<
        Vec<crate::identity_first::AgentMemoryRecord>,
        crate::identity_first::AgentMemoryError,
    > {
        let runtime = self.identity_runtime_ref().ok_or_else(|| {
            crate::identity_first::AgentMemoryError::InvalidConfig(
                "identity-first runtime is not configured".to_string(),
            )
        })?;
        runtime.recall_agent_memory(request).await
    }

    pub async fn forget_agent_memory(
        &self,
        realm: &str,
        identity: &crate::identity_first::AgentIdentity,
        memory_id: &str,
    ) -> Result<
        crate::identity_first::AgentMemoryForgetResult,
        crate::identity_first::AgentMemoryError,
    > {
        let runtime = self.identity_runtime_ref().ok_or_else(|| {
            crate::identity_first::AgentMemoryError::InvalidConfig(
                "identity-first runtime is not configured".to_string(),
            )
        })?;
        runtime
            .forget_agent_memory(realm, identity, memory_id)
            .await
    }

    pub fn attach_identity_first_context(
        &mut self,
        context: Arc<crate::identity_first::IdentityFirstRuntimeContext>,
    ) {
        self.install_identity_first_context_authority(context);
    }

    /// Install identity authority before applying the initial roster.
    ///
    /// The gateway builds its base [`UnifiedRuntime`] before callback-backed
    /// identity providers are available. Identity bootstrap can partially
    /// materialize a roster before a later member fails, so the context must be
    /// visible to [`Self::shutdown`] before bootstrap starts. On failure this
    /// method drives the complete runtime shutdown order before returning the
    /// error.
    pub async fn install_and_bootstrap_identity_first_context(
        &mut self,
        context: Arc<crate::identity_first::IdentityFirstRuntimeContext>,
        roster: &[crate::identity_first::DurableAgentSpec],
    ) -> Result<crate::identity_first::RestoreFlowResult, crate::identity_first::IdentityRuntimeError>
    {
        self.install_identity_first_context(context);
        self.bootstrap_installed_identity_first_context(roster)
            .await
    }

    /// The first phase of [`Self::install_and_bootstrap_identity_first_context`]:
    /// install the identity authority so [`Self::identity_runtime`] answers
    /// and [`Self::shutdown`] finds the context, without consuming a pending
    /// mob activation, registering any persisted continuity owner, restoring
    /// the identity roster or starting an identity supervisor. (The mob and
    /// the runtime's base modules may already be running.) A host binds to
    /// the installed identity view between this and
    /// [`Self::bootstrap_installed_identity_first_context`]; if it refuses,
    /// the ordinary [`Self::shutdown`] cleans up the installed context.
    ///
    /// Installing withdraws a bootstrap completion not yet taken (see
    /// [`Self::take_bootstrap_completion`]): the runtime's bootstrap is then
    /// the identity bootstrap that follows, which mints again when it
    /// succeeds. [`Self::attach_identity_first_context`] leaves it as it is.
    pub fn install_identity_first_context(
        &mut self,
        context: Arc<crate::identity_first::IdentityFirstRuntimeContext>,
    ) {
        // The runtime's bootstrap is now its identity bootstrap: a classic
        // activation's completion not yet taken no longer attests it.
        self.withdraw_bootstrap_completion();
        self.install_identity_first_context_authority(context);
    }

    /// The second phase of [`Self::install_and_bootstrap_identity_first_context`]
    /// for the context [`Self::install_identity_first_context`] installed:
    /// register each roster identity's customizer-tool dispatcher and
    /// activate the pending mob, then restore the roster. On failure it
    /// drives the complete runtime shutdown order before returning.
    pub async fn bootstrap_installed_identity_first_context(
        &mut self,
        roster: &[crate::identity_first::DurableAgentSpec],
    ) -> Result<crate::identity_first::RestoreFlowResult, crate::identity_first::IdentityRuntimeError>
    {
        self.bootstrap_installed_identity_first_context_inner(roster)
            .await
    }

    /// [`Self::bootstrap_installed_identity_first_context_held_until`],
    /// waiting for an aborted pass's settlement and this runtime's failure
    /// shutdown ([`Self::finish_held_identity_bootstrap`]) before it returns:
    /// this call is not bounded by the setup deadline.
    pub async fn bootstrap_installed_identity_first_context_held(
        &mut self,
        roster: &[crate::identity_first::DurableAgentSpec],
        deadline: meerkat_mob::HeldSpawnDeadline,
        complete: HeldIdentityCompletion,
    ) -> Result<crate::identity_first::RestoreFlowResult, crate::identity_first::IdentityRuntimeError>
    {
        match self
            .bootstrap_installed_identity_first_context_held_until(roster, deadline, complete)
            .await?
        {
            crate::identity_first::HeldBootstrapStart::Activated(restored) => Ok(restored),
            crate::identity_first::HeldBootstrapStart::Unsettled(settlement) => {
                Err(self.finish_held_identity_bootstrap(settlement).await)
            }
        }
    }

    /// [`Self::bootstrap_installed_identity_first_context`] holding the
    /// roster's freshly created members before their publication, with the
    /// whole setup bounded by `deadline`: a prepared mob's activation
    /// (registering the persisted owners, publishing the customizer tools,
    /// converging the persisted authority, lifting the mob, in that order)
    /// and then the roster pass. Both run in the identity runtime's tracked
    /// operation, never in this caller's future, so a blocked step neither
    /// delays this call past `deadline` nor is cancelled by it.
    ///
    /// Once every roster identity is held, `complete` receives the held
    /// members, their finished surfaces and the pass's
    /// [`crate::identity_first::HeldCompletion`]: an `Ok` recorded before the
    /// deadline is the pass's choice `Activate`, and Core then accepts or
    /// expires each member's own activation against the same deadline (per
    /// member, not atomic). It returns
    /// [`crate::identity_first::HeldBootstrapStart::Activated`] once the
    /// roster pass completed, with each member's actual outcome: never a
    /// blanket success.
    ///
    /// Anything else returns
    /// [`crate::identity_first::HeldBootstrapStart::Unsettled`], at the
    /// setup's terminal `Abort` at the latest: the tracked operation still
    /// owns whatever setup it entered, or the settlement is already resolved
    /// with the refusal or failure. This runtime is not shut down yet: the
    /// caller first closes whatever admission it owns, then calls
    /// [`Self::finish_held_identity_bootstrap`]. `Err` is returned only when
    /// no identity-first context is installed.
    pub async fn bootstrap_installed_identity_first_context_held_until(
        &mut self,
        roster: &[crate::identity_first::DurableAgentSpec],
        deadline: meerkat_mob::HeldSpawnDeadline,
        complete: HeldIdentityCompletion,
    ) -> Result<
        crate::identity_first::HeldBootstrapStart,
        crate::identity_first::IdentityRuntimeError,
    > {
        use crate::identity_first::{HeldBootstrapSettlement, HeldBootstrapStart};
        // The setup's one terminal decision exists before anything of the
        // setup awaits, so the deadline bounds all of it.
        let terminal = Arc::new(crate::identity_first::held_bootstrap::HeldTerminal::new(
            deadline,
        ));
        let Some(context) = self.identity_first_context.clone() else {
            return Err(crate::identity_first::IdentityRuntimeError::Internal(
                "no identity-first context is installed to bootstrap".to_string(),
            ));
        };
        // The ruled activation order is unchanged (see the unheld path): a
        // held setup runs it first in its tracked operation.
        let pending = self.pending_mob_activation.get_mut().take();
        let reconcile_after_materialization =
            pending.is_some() && !context.bootstrap_mode().is_lazy();
        // The mob resumed Running: no lift, but the restored members still
        // owe their customizer tools.
        let owed = if pending.is_none() {
            context
                .runtime
                .roster_owing_customizer_tools_after_resume(roster)
                .await
        } else {
            Vec::new()
        };
        let prelude = match pending {
            Some(pending) => Some(held_setup_prelude(
                pending,
                Arc::clone(&context),
                roster.to_vec(),
                self.bootstrap_phase_observer.get().cloned(),
                Arc::clone(&terminal),
            )),
            None if !owed.is_empty() => Some(held_resume_prelude(
                Arc::clone(&context),
                owed,
                self.bootstrap_phase_observer.get().cloned(),
                Arc::clone(&terminal),
            )),
            None => {
                self.report_bootstrap_phase(UnifiedRuntimeBootstrapPhase::RestoreRoster);
                None
            }
        };
        match context
            .bootstrap_roster_held_after(roster, terminal, prelude, complete)
            .await
        {
            Ok(HeldBootstrapStart::Activated(result)) => {
                if reconcile_after_materialization {
                    self.reconcile_bootstrap_edges_if_configured().await;
                }
                self.mint_bootstrap_completion();
                Ok(HeldBootstrapStart::Activated(result))
            }
            // The setup did not activate: whatever it entered still settles
            // in its tracked operation, and this runtime's failure shutdown
            // waits for `finish_held_identity_bootstrap`, after the caller has
            // closed its own admission.
            Ok(unsettled @ HeldBootstrapStart::Unsettled(_)) => Ok(unsettled),
            // Refused before its admission, or refused or failed before its
            // terminal decision: nothing is left to settle, and the caller
            // still closes its admission before this runtime shuts down.
            Err(error) => Ok(HeldBootstrapStart::Unsettled(
                HeldBootstrapSettlement::resolved(error),
            )),
        }
    }

    /// Settle a held bootstrap that did not activate: await its settlement
    /// (not bounded: whatever setup it entered, the members' aborts until Core
    /// has settled each and the host callback's join), then drive this
    /// runtime's complete failure shutdown, and return the bootstrap's error.
    pub async fn finish_held_identity_bootstrap(
        &mut self,
        settlement: crate::identity_first::HeldBootstrapSettlement,
    ) -> crate::identity_first::IdentityRuntimeError {
        let error = match settlement.settled().await {
            Err(error) => error,
            Ok(_) => crate::identity_first::IdentityRuntimeError::Internal(
                "an aborted held identity bootstrap settled as a success".to_string(),
            ),
        };
        self.report_bootstrap_phase(UnifiedRuntimeBootstrapPhase::FailureCleanup);
        self.shutdown().await;
        error
    }

    async fn bootstrap_installed_identity_first_context_inner(
        &mut self,
        roster: &[crate::identity_first::DurableAgentSpec],
    ) -> Result<crate::identity_first::RestoreFlowResult, crate::identity_first::IdentityRuntimeError>
    {
        let Some(context) = self.identity_first_context.clone() else {
            return Err(crate::identity_first::IdentityRuntimeError::Internal(
                "no identity-first context is installed to bootstrap".to_string(),
            ));
        };
        // The ruled activation order. The customizer-tool dispatchers and the
        // published tools must precede the lift, because the lift is what
        // makes Core restore persisted members and each restored member is
        // built with them; and materialization must follow the lift, because
        // a Stopped mob cannot spawn. So this sits between installing the
        // context and rostering.
        let pending = self.pending_mob_activation.lock().await.take();
        let reconcile_after_materialization =
            pending.is_some() && !context.bootstrap_mode().is_lazy();
        if let Some(pending) = pending {
            self.report_bootstrap_phase(UnifiedRuntimeBootstrapPhase::RegisterPersistedOwners);
            context
                .runtime
                .ensure_customizer_tool_dispatchers(roster)
                .await;
            // Publish every roster identity's customizer tools before the lift,
            // so the restore builds each member with them (#563).
            let release_owed = context
                .runtime
                .prepublish_customizer_tools(roster, context.customizer.as_deref())
                .await;
            self.report_bootstrap_phase(UnifiedRuntimeBootstrapPhase::Activate);
            if let Err(error) = pending.activate().await {
                self.report_bootstrap_phase(UnifiedRuntimeBootstrapPhase::FailureCleanup);
                self.shutdown().await;
                return Err(crate::identity_first::IdentityRuntimeError::Internal(
                    format!("activating the prepared mob: {error}"),
                ));
            }
            // A publication above that succeeded after the mob build's early
            // one failed leaves the build's run-start hold staged; the mob
            // is live now, so the release reaches it.
            context
                .runtime
                .release_customizer_tool_holds(&release_owed)
                .await;
            // Lazy bootstrap keeps its pending-edge observation without
            // forcing materialization. Eager restore must first attach the
            // persisted members to the fresh bridge so actual wires can be
            // resolved to their authoritative runtime identities.
            if !reconcile_after_materialization {
                self.reconcile_bootstrap_edges_if_configured().await;
            }
        } else {
            let owed = context
                .runtime
                .roster_owing_customizer_tools_after_resume(roster)
                .await;
            publish_customizer_tools_after_resume(&context, &owed).await;
        }
        self.report_bootstrap_phase(UnifiedRuntimeBootstrapPhase::RestoreRoster);
        match context.bootstrap_roster(roster).await {
            Ok(result) => {
                if reconcile_after_materialization {
                    self.reconcile_bootstrap_edges_if_configured().await;
                }
                self.mint_bootstrap_completion();
                Ok(result)
            }
            Err(error) => {
                self.report_bootstrap_phase(UnifiedRuntimeBootstrapPhase::FailureCleanup);
                self.shutdown().await;
                Err(error)
            }
        }
    }

    fn install_identity_first_context_authority(
        &mut self,
        context: Arc<crate::identity_first::IdentityFirstRuntimeContext>,
    ) {
        self.install_identity_first_flow_target_provisioner(&context.runtime);
        // The probe's verdict reaches the identity delivery path through the
        // identity runtime (and from there its session bridge): a send issued
        // while a stall is open fails fast instead of queueing behind it.
        context
            .runtime
            .install_actor_loop_health(Arc::clone(&self.actor_loop_health));
        self.mob_runtime
            .install_identity_runtime_authority(Arc::clone(&context.runtime));
        // A respawned or delivery-repaired identity member keeps its
        // idle-retire opt-in: the continuity rebind carries it to the new
        // session.
        if let Some(overrides) = self.mob_runtime.implicit_delegate_retirement_overrides() {
            context.runtime.install_session_rotation_observer(Arc::new(
                crate::mob_handle_runtime::IdleRetireRotationObserver::new(
                    overrides,
                    self.mob_runtime.handle().mob_id().to_string(),
                ),
            ));
        }
        *self
            .implicit_delegate_identity_runtime
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::clone(&context.runtime));
        // Let completion-cursor reads credit the completions a member's actor
        // already published but the health monitor has not credited yet,
        // replayed from meerkat's retained session events. Installed before
        // the binding observer, so the monitor's first binding-woken attach
        // already knows it may replay.
        if let Some(session_service) = self.mob_runtime.session_service().cloned() {
            context
                .runtime
                .install_pending_completion_drain(Arc::new(ReplayedCompletionDrain {
                    session_service,
                    ledger: Arc::clone(&self.identity_completion_ledger),
                }));
        }
        // After the authority is visible: the install itself wakes the
        // monitor once, for bindings that predate it.
        context
            .runtime
            .install_member_binding_observer(self.identity_binding_wake.clone());
        self.identity_first_context = Some(context);
        if let Some(projection) = self.console_projection.get() {
            projection.update_identity_authority("default", self.identity_runtime_ref().cloned());
        }
    }

    pub(crate) fn install_identity_first_flow_target_provisioner(
        &self,
        runtime: &Arc<crate::identity_first::IdentityRuntime>,
    ) {
        let identity_runtime = Arc::downgrade(runtime);
        self.mob_runtime
            .handle()
            .install_flow_target_provisioner(Arc::new(move || {
                let identity_runtime = identity_runtime.clone();
                Box::pin(async move {
                    let runtime = identity_runtime.upgrade().ok_or_else(|| {
                        MobError::Internal(
                            "identity-first flow provisioner is no longer available".to_string(),
                        )
                    })?;
                    // ATTRIBUTION. This barrier materializes the whole
                    // registered identity set, and its spawns are recorded as
                    // ordinary fleet materialization - they carry no source
                    // naming the flow trigger that caused them. A consumer
                    // measuring a slow flow trigger therefore sees a
                    // materialization burst in their telemetry with nothing
                    // linking the two, which is how a 358-second trigger read
                    // as a hang rather than as work (a 2026-08-31 production incident: 144
                    // spawns, 17 -> 161, the entire identity count).
                    //
                    // The scope itself is not fixable here: `FlowTargetProvisioner`
                    // is `Fn() -> ...` with no flow argument, so this callback
                    // cannot know which flow, which roles, or which identities
                    // are actually required. Narrowing it needs the upstream
                    // hook to carry flow context. Until then the least this can
                    // do is SAY what it is about to do, and how much.
                    tracing::info!(
                        cause = "flow_target_provisioning",
                        "identity-first flow barrier is materializing every registered \
                         identity before the flow run id is minted; cost is O(fleet), \
                         not O(flow targets)"
                    );
                    let started = std::time::Instant::now();
                    let outcome = runtime
                        .materialize_all_required_for_entered_flow()
                        .await
                        .map(|records| records.len())
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "identity-first flow materialization failed: {error}"
                            ))
                        });
                    match &outcome {
                        Ok(materialized) => tracing::info!(
                            materialized = *materialized,
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            cause = "flow_target_provisioning",
                            "identity-first flow barrier complete"
                        ),
                        Err(error) => tracing::warn!(
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            cause = "flow_target_provisioning",
                            %error,
                            "identity-first flow barrier failed"
                        ),
                    }
                    outcome.map(|_| ())
                })
            }));
    }

    pub async fn refresh_desired_topology(
        &self,
    ) -> Result<
        Option<crate::identity_first::RestoreFlowResult>,
        crate::identity_first::IdentityRuntimeError,
    > {
        match self.identity_first_context.as_ref() {
            Some(ctx) => ctx.refresh_desired_topology_tracked().await.map(Some),
            None => Ok(None),
        }
    }

    /// Hydrate identity-first lazy members before handing control to concrete
    /// mob APIs that operate on already-materialized runtime members.
    pub async fn materialize_identity_first_for_flow(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberDeliveryScope>, crate::identity_first::IdentityRuntimeError>
    {
        match self.identity_runtime_ref() {
            Some(runtime) => runtime.materialize_all_required_tracked().await,
            None => Ok(Vec::new()),
        }
    }

    /// Return the mob/run label sidecar table.
    ///
    /// Meta owns this table — meerkat-mob has no concept of mob- or
    /// run-level labels. Apps use it to attach external context (repo,
    /// branch, customer, deployment, environment) to a mob or a flow run.
    pub fn metadata_table(&self) -> &Arc<RuntimeMetadataTable> {
        &self.metadata_table
    }

    /// Install the shared access controller. Console routers built after
    /// this call enforce (and live-serve) the ABAC configuration.
    pub fn set_access_controller(&mut self, controller: crate::access::AccessController) {
        self.access_controller = Some(controller);
    }

    /// Install the Console's session fence record, conventionally
    /// [`crate::ConsoleSessionFences::open_in_state_dir`] over the gateway's
    /// state directory. Console and SSE routers built after this call refuse
    /// every token issued before its principal's fence, and end the streams
    /// and voice sessions such tokens opened when a fence is set. Without
    /// one the runtime holds a record with no file, which refuses to set a
    /// fence. On a candidate launch the record writes nothing: the
    /// principals it accepts a token without `exp` for are kept in this
    /// process only.
    pub fn set_console_session_fences(
        &mut self,
        fences: crate::auth::session_fence::ConsoleSessionFences,
    ) {
        if !self.speaks_for_composition() {
            fences.keep_non_expiring_notes_in_process();
        }
        self.console_session_fences = fences;
    }

    /// The Console's session fence record.
    pub fn console_session_fences(&self) -> &crate::auth::session_fence::ConsoleSessionFences {
        &self.console_session_fences
    }

    /// Whether the Flow Editor on the reference app router may run `rkat` on
    /// this host for callers that no enforced access controller governs:
    /// `meta/mobpacks/deploy` with `execute: true` and
    /// `meta/mobpacks/validate` with `rkat_validate: true`. Off by default,
    /// so without an access controller that grants `mobpack.deploy` both are
    /// refused (deploys stay plans, validation stays structural).
    ///
    /// Allowed, every caller the route admits may deploy: on an open console
    /// that is every process that can reach the listener, whichever user
    /// runs it. An enforced access controller keeps deciding per caller
    /// either way. Routers built after this call use it. The bundled
    /// gateways set it from `--allow-flow-editor-host-deploy`.
    pub fn allow_flow_editor_host_deploy(&mut self, allowed: bool) {
        self.flow_editor_host_deploy = allowed;
    }

    /// Record whether this runtime's agent factories hold a provider
    /// credential store the host chose instead of the process default: the
    /// builder records its own `provider_auth_persistence`, and the bundled
    /// gateways record a launch's store. A host deploy cannot carry that
    /// store: `meta/mobpacks/deploy` with `execute: true` runs `rkat mob run`
    /// in a process of its own, which would resolve provider credentials in
    /// the process default store. While this is set, the runtime's RPC and
    /// its Flow Editor refuse such a deploy with `-32004`
    /// (`data.kind = "provider_credentials_not_carried"`), whatever grant or
    /// opt-in the caller has; deploy plans still answer. Routers built after
    /// this call use it.
    pub fn set_explicit_provider_credentials(&mut self, explicit: bool) {
        self.explicit_provider_credentials = explicit;
    }

    /// Whether this runtime's agent factories hold a host-chosen provider
    /// credential store (see [`Self::set_explicit_provider_credentials`]).
    pub(crate) fn has_explicit_provider_credentials(&self) -> bool {
        self.explicit_provider_credentials
    }

    /// Wire the bundled sqlite memory store into the console Memory panel
    /// (§9.3). `&self` deliberately: gateways construct the store next to
    /// the memory subsystem wiring, which may run after the runtime is
    /// `Arc`-shared. Routers built *after* this call serve the panel RPCs.
    pub fn set_console_identity_roster(
        &self,
        roster: Arc<crate::identity_first::MutableRosterProvider>,
    ) {
        *self
            .console_identity_roster
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(roster);
    }

    pub fn console_identity_roster(
        &self,
    ) -> Option<Arc<crate::identity_first::MutableRosterProvider>> {
        self.console_identity_roster
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Attach the mob announcement owner. Once per runtime: `false` when one
    /// is already attached.
    pub fn attach_mob_announcements(
        &self,
        service: crate::mob_announcements::AnnouncementService,
    ) -> bool {
        self.mob_announcements.set(service).is_ok()
    }

    /// The attached mob announcement owner, when the host enabled them.
    pub fn mob_announcements(&self) -> Option<crate::mob_announcements::AnnouncementService> {
        self.mob_announcements.get().cloned()
    }

    /// The slot the mob announcement owner attaches to, for a surface built
    /// before it attaches: the console reads it at each request.
    pub(crate) fn mob_announcements_slot(&self) -> crate::mob_announcements::AnnouncementSlot {
        std::sync::Arc::clone(&self.mob_announcements)
    }

    /// Seal what the keyed inject plane of the cross-mob control channel
    /// serves. Once per runtime, from pending: `false` when it is already
    /// sealed. Gateways seal it through
    /// [`crate::mob_announcements::compose_keyed_inject_plane`].
    pub fn seal_keyed_inject(
        &self,
        serving: crate::runtime::keyed_inject::KeyedInjectServing,
    ) -> bool {
        crate::runtime::keyed_inject::seal_keyed_inject_slot(&self.keyed_inject_slot, serving)
            .is_ok()
    }

    /// The contact directory and the signing-key slot that delivery to
    /// members on peer gateways uses.
    pub(crate) fn peer_delivery_access(
        &self,
    ) -> Option<(
        crate::contact_directory::ContactDirectory,
        crate::runtime::cross_mob_control::ControlSignerSlot,
    )> {
        self.contact_directory
            .clone()
            .map(|contacts| (contacts, Arc::clone(&self.gateway_peer_keys)))
    }

    pub fn set_memory_panel_store(
        &self,
        store: Arc<dyn crate::memory::capabilities::MemoryPanelStore>,
    ) {
        *self
            .memory_panel_store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(store);
    }

    pub fn memory_panel_store(
        &self,
    ) -> Option<Arc<dyn crate::memory::capabilities::MemoryPanelStore>> {
        self.memory_panel_store
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The realm-scoped WorkGraph service backing the `meta/workgraph/*`
    /// RPC group and the console experience section, seeded from the
    /// bootstrap spec. There is deliberately NO late setter (round-5 S3): the
    /// admission guards — the cross-process sidecar and the agent tool-plane
    /// slots — freeze at `MobRuntime::bootstrap`, so a service wired in
    /// after the fact would silently run guard-degraded. Wire workgraph
    /// through `MobBootstrapSpec::with_workgraph_service` (plus
    /// `with_workgraph_admission_slot`/`with_workgraph_admission_sidecar`)
    /// or the stock spec constructors, which do all three.
    pub fn workgraph_service(&self) -> Option<meerkat::WorkGraphService> {
        self.workgraph_service.clone()
    }

    /// Report of the bootstrap-time migration of member-bound attention
    /// bindings into their mob realm (`None` without a WorkGraph service).
    pub fn workgraph_realm_migration(
        &self,
    ) -> Option<std::sync::Arc<crate::workgraph_realm::WorkGraphRealmMigrationReport>> {
        self.mob_runtime.workgraph_realm_migration()
    }

    /// Clone the runtime's lossy WorkGraph fact hub when WorkGraph is
    /// configured. Every subscriber must begin with a durable pull; the hub
    /// has no replay or state authority.
    pub fn workgraph_fact_hub(&self) -> Option<crate::workgraph_events::WorkGraphFactHub> {
        self.workgraph_fact_hub.clone()
    }

    /// Composition-time storage durability resolution (H1/H2) carried from
    /// the bootstrap spec, reported by `meta/status` /
    /// `meta/capabilities`. `None` when the spec was composed externally
    /// without a declaration.
    pub fn resolved_storage(&self) -> Option<crate::storage_health::ResolvedStorageSummary> {
        self.mob_runtime.resolved_storage()
    }

    /// The runtime-wide admission authority serializing the workgraph
    /// duplicate-binding guards' check-then-act windows (RPC arms + agent
    /// tool plane). Lives on the mob runtime so console routers (which
    /// capture the mob runtime by value) and the unified stdin dispatch
    /// reach the SAME instance, frozen at bootstrap alongside the service.
    pub(crate) fn workgraph_admission(
        &self,
    ) -> std::sync::Arc<crate::workgraph_admission::WorkGraphAdmission> {
        self.mob_runtime.workgraph_admission()
    }

    /// Wire the §16 Q1 console-principal operator resolver (set by the
    /// gateway's memory wiring when `operator_scope = "provisional"`); the
    /// console send path notes authenticated interactions through it.
    pub fn set_console_operator_resolver(
        &self,
        resolver: Arc<crate::memory::coordinator::ConsolePrincipalOperatorResolver>,
    ) {
        *self
            .console_operator_resolver
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(resolver);
    }

    /// Install the trusted host's governance of Console sends
    /// ([`crate::console_governed_ingress`]). A Console send to an identity
    /// it governs then reaches the member only through its binder, for the
    /// authenticated Console human who sent it. Installing it also closes
    /// the Console's member-only send path for every target (the module's
    /// compatibility restriction). Routers already built see it from their
    /// next send.
    pub fn set_console_governed_ingress(
        &self,
        ingress: Arc<dyn crate::console_governed_ingress::ConsoleGovernedIngress>,
    ) {
        self.console_governed_ingress.install(ingress);
    }

    pub fn console_operator_resolver(
        &self,
    ) -> Option<Arc<crate::memory::coordinator::ConsolePrincipalOperatorResolver>> {
        self.console_operator_resolver
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Borrow the shared access controller if one was installed.
    pub fn access_controller(&self) -> Option<&crate::access::AccessController> {
        self.access_controller.as_ref()
    }

    /// Optional topology control-plane policy and durable intent store.
    pub fn topology_controller(&self) -> &crate::topology_control::TopologyController {
        &self.topology_controller
    }

    /// Cloneable topology seam for HTTP/RPC routers.
    pub fn topology_runtime_handle(&self) -> crate::topology_control::TopologyRuntimeHandle {
        crate::topology_control::TopologyRuntimeHandle::new(
            self.mob_handle(),
            self.edge_discovery.clone(),
            Arc::clone(&self.managed_dynamic_edges),
            self.topology_controller.clone(),
            // An export: the handle goes to routers and the host.
            self.identity_first_context(),
            // A candidate launch's handle rewires nothing.
            !self.speaks_for_composition(),
        )
    }

    /// Replace the topology policy at runtime. Existing additions and
    /// suppression tombstones remain authoritative; disabling hides/denies
    /// mutation rather than silently discarding desired state.
    pub fn set_topology_control_policy(
        &self,
        policy: crate::topology_control::TopologyControlPolicy,
    ) -> Result<(), crate::topology_control::TopologyControlError> {
        self.topology_controller.set_policy(policy)
    }

    /// Return the persistent metadata adapter — used by the
    /// structural-events subscription to checkpoint its last-projected
    /// cursor. Tests and integration code that need to inspect the
    /// persisted cursor reach through this accessor.
    pub fn persistent_metadata(&self) -> &Arc<dyn PersistentMetadataStore> {
        &self.persistent_metadata
    }

    /// Replace the label set associated with this mob.
    ///
    /// An empty `labels` map clears the entry. Replacement is wholesale —
    /// existing labels not present in `labels` are dropped. To merge,
    /// read first via [`Self::get_mob_labels`] and combine.
    pub async fn set_mob_labels(&self, labels: BTreeMap<String, String>) {
        self.metadata_table
            .set_labels(MetadataScope::Mob(self.mob_id()), labels)
            .await;
    }

    /// Return the label set associated with this mob, or an empty map.
    pub async fn get_mob_labels(&self) -> BTreeMap<String, String> {
        self.metadata_table
            .get_labels(&MetadataScope::Mob(self.mob_id()))
            .await
    }

    /// Remove the label set associated with this mob.
    pub async fn delete_mob_labels(&self) {
        let _ = self
            .metadata_table
            .delete_labels(&MetadataScope::Mob(self.mob_id()))
            .await;
    }

    /// Replace the label set for `run_id` under this mob.
    pub async fn set_run_labels(&self, run_id: &str, labels: BTreeMap<String, String>) {
        self.metadata_table
            .set_labels(
                MetadataScope::Run(self.mob_id(), run_id.to_string()),
                labels,
            )
            .await;
    }

    /// Return the label set for `run_id` under this mob, or an empty map.
    pub async fn get_run_labels(&self, run_id: &str) -> BTreeMap<String, String> {
        self.metadata_table
            .get_labels(&MetadataScope::Run(self.mob_id(), run_id.to_string()))
            .await
    }

    /// Remove the label set for `run_id` under this mob.
    pub async fn delete_run_labels(&self, run_id: &str) {
        let _ = self
            .metadata_table
            .delete_labels(&MetadataScope::Run(self.mob_id(), run_id.to_string()))
            .await;
    }

    /// Return the underlying event log store if one is configured.
    ///
    /// Used to share the store with sub-handlers (e.g. console RPC) that
    /// don't hold a full `UnifiedRuntime` reference.
    pub fn event_log_store(&self) -> Option<std::sync::Arc<dyn event_log::EventLogStore>> {
        self.event_log
            .as_ref()
            .map(event_log::EventLogHandle::store)
    }

    pub fn console_log_store(&self) -> Arc<dyn ConsoleLogStore> {
        self.console_log_store.clone()
    }

    pub fn set_console_log_store(&mut self, store: Arc<dyn ConsoleLogStore>) {
        self.console_log_store = store;
        if let Some(projection) = self.console_projection.take() {
            projection.unregister_runtime("default");
        }
    }

    /// Query structural mob events from the meerkat ledger.
    ///
    /// Returns events filtered by [`EventQuery`] in cursor-ascending
    /// order. `EventQuery::after_seq` acts as the pagination cursor: the
    /// caller passes the highest `cursor` seen so far to receive only
    /// strictly-newer events. Without `after_seq` the call returns the
    /// **latest** matching events up to `limit` (default 256), scanning
    /// the ledger backwards from `latest_cursor`.
    ///
    /// Errors propagate the typed [`mob_events::MobEventsQueryError`]
    /// so the JSON-RPC handler can surface `StaleEventCursor` as code
    /// `-32010`.
    pub async fn query_mob_events(
        &self,
        query: &EventQuery,
    ) -> Result<Vec<MobStructuralEventEnvelope>, mob_events::MobEventsQueryError> {
        let events = self.mob_runtime.handle().events();
        mob_events::query_ledger_with_filter(&events, &self.mob_events, query).await
    }

    /// Subscribe to live structural mob events. Returns a broadcast
    /// receiver that yields each newly-projected envelope. The receiver
    /// will report `RecvError::Lagged` if it falls behind the in-memory
    /// channel cap.
    pub fn subscribe_mob_events(
        &self,
    ) -> tokio::sync::broadcast::Receiver<MobStructuralEventEnvelope> {
        self.mob_events.subscribe()
    }

    /// Ingest an event into the event log (if configured). Non-blocking.
    pub(crate) fn ingest_event(&self, event: &EventEnvelope<UnifiedEvent>) {
        if let Some(ref log) = self.event_log {
            log.ingest(event.clone());
        }
    }

    pub(crate) async fn record_console_lifecycle(
        &self,
        identity: &str,
        event_type: &str,
        data: serde_json::Value,
    ) {
        self.console_events
            .record_lifecycle(identity, event_type, data)
            .await;
    }

    pub async fn reserve_identity_interaction(
        &self,
        identity: &str,
        runtime_member_id: Option<&str>,
        interaction_id: &str,
        origin: &str,
        content: serde_json::Value,
    ) -> Result<(), &'static str> {
        self.console_events
            .reserve_interaction_value(identity, runtime_member_id, interaction_id, origin, content)
            .await
    }

    /// Reserve a caller-supplied interaction id for `meta/interact`,
    /// refusing one still in flight for the identity (see
    /// [`console_events::InteractionIdInFlight`]).
    pub(crate) async fn reserve_caller_identity_interaction(
        &self,
        identity: &str,
        runtime_member_id: Option<&str>,
        interaction_id: &str,
        origin: &str,
        content: serde_json::Value,
    ) -> Result<(), console_events::InteractionIdInFlight> {
        self.console_events
            .reserve_caller_interaction_value(
                identity,
                runtime_member_id,
                interaction_id,
                origin,
                content,
            )
            .await
    }

    pub(crate) async fn project_console_event_from_unified(
        &self,
        event: &EventEnvelope<UnifiedEvent>,
    ) {
        self.console_events.project_unified_event(event).await;
    }

    /// Fire an error event to the registered hook, if any.
    /// Truly fire-and-forget — spawns a detached task so slow hooks
    /// (HTTP to Slack, PagerDuty) never block the runtime operation.
    pub(crate) fn fire_error(&self, event: ErrorEvent) {
        fire_error_hook(&self.error_hook, event);
    }

    /// Fire [`ErrorEvent`]s through this runtime's error hook from a
    /// component that outlives no runtime borrow (the mob announcement
    /// owner): the hook installed when each fires is the one used.
    pub(crate) fn error_sink(&self) -> crate::mob_announcements::ErrorSink {
        let slot = Arc::clone(&self.error_hook);
        Arc::new(move |event| fire_error_hook(&slot, event))
    }

    fn create_event_ingress(
        mob_handle: MobHandle,
        agent_mob_mcp_state: Option<Arc<meerkat_mob_mcp::MobMcpState>>,
        mob_events: MobEventsStore,
        identity_runtime: Arc<
            std::sync::RwLock<Option<Arc<crate::identity_first::IdentityRuntime>>>,
        >,
        completion_ledger: Arc<HealthCompletionLedger>,
        identity_binding_changes: tokio::sync::watch::Receiver<u64>,
    ) -> MobEventIngress {
        // Keep forwarding bounded to avoid unbounded memory growth under sustained ingress.
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(256);
        // Identity lifecycle repair must remain live even when an embedding
        // application does not drain the bounded console/event channel.
        // A dedicated subscription monitor drains its streams independently
        // and owns only permanent-loss detection; the ordinary forwarder
        // retains lossless backpressure for user-visible events.
        let identity_stream_health_task = tokio::spawn(run_identity_stream_health_monitor(
            mob_handle.clone(),
            agent_mob_mcp_state.clone(),
            identity_runtime,
            completion_ledger,
            identity_binding_changes,
        ));
        let task = tokio::spawn(run_resilient_mob_agent_event_forwarder(
            mob_handle,
            agent_mob_mcp_state,
            event_tx,
            mob_events,
        ));
        MobEventIngress::Forwarder(MobEventForwarder {
            event_rx,
            task,
            identity_stream_health_task,
        })
    }

    /// Test seam: replace the live ingress with a caller-owned channel so
    /// tests can push forwarded member events through the real drain path.
    #[cfg(test)]
    async fn install_test_event_ingress(&self) -> Sender<ForwardedMemberEvent> {
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(16);
        let replaced = self
            .mob_event_ingress
            .lock()
            .await
            .replace(MobEventIngress::Forwarder(MobEventForwarder {
                event_rx,
                task: tokio::spawn(async {}),
                identity_stream_health_task: tokio::spawn(async {}),
            }));
        if let Some(MobEventIngress::Forwarder(forwarder)) = replaced {
            forwarder.task.abort();
            forwarder.identity_stream_health_task.abort();
            // Settle the aborted tasks' drops before the test takes over.
            let _ = forwarder.task.await;
            let _ = forwarder.identity_stream_health_task.await;
        }
        event_tx
    }

    #[cfg(test)]
    pub(crate) async fn install_test_console_event_ingress(&self) -> TestConsoleEventIngress {
        TestConsoleEventIngress(self.install_test_event_ingress().await)
    }

    async fn rollback_mob_runtime(
        mob_runtime: MobRuntime,
        startup_error: UnifiedRuntimeBootstrapError,
    ) -> Result<Self, UnifiedRuntimeBootstrapError> {
        // A candidate launch's rollback leaves the store as it found it.
        match mob_runtime.stop_after_failed_startup().await {
            Ok(_report) => Err(startup_error),
            Err(err) => Err(UnifiedRuntimeBootstrapError::ModuleStartupRollbackFailed {
                startup_error: Box::new(startup_error),
                rollback_error: MobRuntimeError::from(err),
            }),
        }
    }
}

// The trailing `Option<Arc<str>>` is the member's durable identity label,
// present only for identity-first owned members of the primary mob. The
// identity health monitor needs it to attribute a run completion to the
// durable identity; the console forwarder ignores it.
type TaggedAgentEvent = (
    AgentRuntimeId,
    FenceToken,
    ProfileName,
    meerkat_core::event::EventEnvelope<AgentEvent>,
    Option<Arc<str>>,
    // Sequence space of the stream the envelope came from.
    Option<meerkat_core::comms::SessionEventEpoch>,
);

enum ForwardedAgentEvent {
    Event(Box<TaggedAgentEvent>),
    /// The stream ended. Carries the key it was tracked under at that moment
    /// and its attachment generation, so a superseded stream ending cannot
    /// untrack the stream that replaced it under the same key.
    Closed(TrackedAgentEventStream, u64),
    /// The reconciler stopped waiting for a revoked predecessor actor's
    /// stream after [`PREDECESSOR_DRAIN_DEADLINE`] and cut it off. Yielded in
    /// stream order, after the last event forwarded from it and before its
    /// `Closed`, so the console learns of the gap before any successor event.
    Abandoned {
        key: TrackedAgentEventStream,
        session_id: meerkat_core::types::SessionId,
        generation: u64,
    },
}

/// How long a revoked predecessor actor's still-open stream may hold back
/// its member's next attachment. The stream normally closes right after the
/// actor is removed, but only once the actor's task exits: a discard does not
/// interrupt a turn stuck in a tool or provider call. Past the deadline the
/// predecessor's stream is cut off with an explicit
/// [`ForwarderStreamGap::PredecessorStreamAbandoned`] marker and the member's
/// successor attaches.
const PREDECESSOR_DRAIN_DEADLINE: Duration = Duration::from_secs(10);

/// A gap the console forwarder itself introduced into a member's timeline,
/// surfaced as a `stream_truncated` frame (the console's typed gap: it ends
/// inherited run lineage and triggers a committed-history refresh).
#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ForwarderStreamGap {
    /// A revoked predecessor actor's stream stayed open past
    /// [`PREDECESSOR_DRAIN_DEADLINE`]; whatever it published afterwards is
    /// not on the live timeline.
    PredecessorStreamAbandoned { drain_deadline_ms: u64 },
}

fn predecessor_stream_gap_event(
    key: &TrackedAgentEventStream,
    session_id: &meerkat_core::types::SessionId,
    generation: u64,
) -> ForwardedMemberEvent {
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default();
    let gap = ForwarderStreamGap::PredecessorStreamAbandoned {
        drain_deadline_ms: u64::try_from(PREDECESSOR_DRAIN_DEADLINE.as_millis())
            .unwrap_or(u64::MAX),
    };
    ForwardedMemberEvent {
        envelope: EventEnvelope {
            event_id: format!("evt-agent-gap-{session_id}-{timestamp_ms}-{generation}"),
            source: "agent".to_string(),
            timestamp_ms,
            event: UnifiedEvent::Agent {
                agent_id: crate::member_comms_id::runtime_event_alias(&key.runtime_id),
                event_type: "stream_truncated".to_string(),
                payload: Some(json!({
                    "reason": gap,
                    "session_id": session_id,
                })),
            },
        },
        alert: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TrackedAgentEventStream {
    mob_id: String,
    /// Trusted durable identity stamped by the identity-first spawn bridge.
    /// Ordinary/child mobs do not carry this label and are never handed to
    /// the primary identity authority.
    durable_identity: Option<String>,
    /// Concrete roster identity used to subscribe to Meerkat events.
    member_identity: AgentIdentity,
    runtime_id: AgentRuntimeId,
    fence_token: FenceToken,
}
type TaggedAgentEventStream = BoxStream<'static, ForwardedAgentEvent>;

/// Every member event stream a reconciler has attached.
#[derive(Default)]
struct AttachedStreams {
    /// The attachment serving each current roster binding.
    current: HashMap<TrackedAgentEventStream, StreamAttachment>,
    /// Streams of known actor incarnations (the console forwarder's
    /// subscriptions name theirs) whose binding left the roster while they
    /// are still open, by attachment generation. A live one is re-keyed when its member is
    /// rebound onto the same actor; a revoked one is a predecessor still
    /// draining, and holds back its member's next attachment until it closes.
    departed: HashMap<u64, DepartedStream>,
    /// Event ids each member's streams already forwarded (console forwarder
    /// only). A replaying subscription can overlap what an earlier stream of
    /// the same member forwarded; those envelopes are not repeated.
    forwarded: HashMap<MemberStreamOwner, Arc<std::sync::Mutex<ForwardedEventIds>>>,
}

/// Most recent event ids remembered per member: meerkat's per-actor replay
/// window (1024 envelopes) with headroom.
const FORWARDED_EVENT_ID_WINDOW: usize = 2048;

/// A bounded window of the event ids one member's streams forwarded.
#[derive(Default)]
struct ForwardedEventIds {
    order: std::collections::VecDeque<uuid::Uuid>,
    seen: HashSet<uuid::Uuid>,
}

impl ForwardedEventIds {
    /// Record `event_id`; `false` when it was already forwarded.
    fn first_sighting(&mut self, event_id: uuid::Uuid) -> bool {
        if !self.seen.insert(event_id) {
            return false;
        }
        self.order.push_back(event_id);
        while self.order.len() > FORWARDED_EVENT_ID_WINDOW {
            if let Some(evicted) = self.order.pop_front() {
                self.seen.remove(&evicted);
            }
        }
        true
    }
}

struct DepartedStream {
    owner: MemberStreamOwner,
    actor: meerkat_session::LiveSessionActorWitness,
    attachment: StreamAttachment,
}

/// The member a stream serves, across rebindings of that member.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct MemberStreamOwner {
    mob_id: String,
    member_identity: AgentIdentity,
}

impl MemberStreamOwner {
    fn of(key: &TrackedAgentEventStream) -> Self {
        Self {
            mob_id: key.mob_id.clone(),
            member_identity: key.member_identity.clone(),
        }
    }
}

impl AttachedStreams {
    /// Record a stream's close. Returns whether it was the attachment
    /// serving `key`: a superseded stream (re-keyed away, or a departed
    /// predecessor) closing never untracks what serves the key now.
    fn close(&mut self, key: &TrackedAgentEventStream, generation: u64) -> bool {
        self.departed.remove(&generation);
        if self
            .current
            .get(key)
            .is_some_and(|attachment| attachment.generation == generation)
        {
            self.current.remove(key);
            true
        } else {
            false
        }
    }

    /// Stop serving a binding that left the roster. A stream of a known
    /// actor stays accounted for until it closes; an ordinary subscription's
    /// actor is unknown, so it simply ages out, unless `cut_unknown_actor`
    /// asks to cut it off: the identity health monitor does, because a Core
    /// fence rotation re-keys the binding, the monitor opens a fresh
    /// subscription to the same actor, and the aging one would count every
    /// completion a second time.
    fn depart(&mut self, key: &TrackedAgentEventStream, cut_unknown_actor: bool) {
        let Some(attachment) = self.current.remove(key) else {
            return;
        };
        if attachment.actor.is_none() && cut_unknown_actor {
            attachment.abort.abort();
            return;
        }
        if let Some(actor) = attachment.actor.clone() {
            self.departed.insert(
                attachment.generation,
                DepartedStream {
                    owner: MemberStreamOwner::of(key),
                    actor,
                    attachment,
                },
            );
        }
    }

    /// Whether a predecessor stream still holds back `owner`'s next
    /// attachment: a departed stream whose actor was revoked, or whose live
    /// actor serves a different session than the member is bound to now (a
    /// rebind that lands before the old actor's revocation). Its remaining
    /// events must reach the console before any successor's, or a
    /// predecessor delta could inherit the successor run's lineage.
    ///
    /// Bounded: a predecessor held for [`PREDECESSOR_DRAIN_DEADLINE`] is cut
    /// off (see [`StreamAttachment::hold`]). Until its `Closed` arrives the
    /// member stays held, so the gap marker still precedes the successor.
    fn predecessor_draining(
        &mut self,
        owner: &MemberStreamOwner,
        bound_session: Option<&meerkat_core::types::SessionId>,
        now: tokio::time::Instant,
    ) -> bool {
        let mut draining = false;
        for departed in self.departed.values_mut() {
            if departed.owner != *owner {
                continue;
            }
            let predecessor = !departed.actor.is_live()
                || bound_session
                    .is_some_and(|session_id| departed.actor.session_id() != session_id);
            if predecessor {
                departed.attachment.hold(now);
                draining = true;
            }
        }
        draining
    }

    /// The departed stream of `owner`'s live actor for `session_id`, if any.
    fn live_departed_for(
        &self,
        owner: &MemberStreamOwner,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<u64> {
        self.departed
            .iter()
            .find(|(_, departed)| {
                departed.owner == *owner
                    && departed.actor.is_live()
                    && departed.actor.session_id() == session_id
            })
            .map(|(generation, _)| *generation)
    }

    /// The forwarded-event window shared by `owner`'s streams.
    fn forwarded_for(
        &mut self,
        owner: &MemberStreamOwner,
    ) -> Arc<std::sync::Mutex<ForwardedEventIds>> {
        Arc::clone(self.forwarded.entry(owner.clone()).or_default())
    }

    /// Forget the forwarded-event windows of members that left the roster
    /// and hold no open stream.
    fn retain_forwarded_for(&mut self, roster: &HashSet<TrackedAgentEventStream>) {
        let Self {
            current,
            departed,
            forwarded,
        } = self;
        forwarded.retain(|owner, _| {
            roster
                .iter()
                .any(|key| MemberStreamOwner::of(key) == *owner)
                || current
                    .keys()
                    .any(|key| MemberStreamOwner::of(key) == *owner)
                || departed.values().any(|departed| departed.owner == *owner)
        });
    }

    fn holds_departed(&self, owner: &MemberStreamOwner) -> bool {
        self.departed
            .values()
            .any(|departed| departed.owner == *owner)
    }

    /// When the earliest held predecessor reaches its drain deadline.
    fn earliest_drain_deadline(&self) -> Option<tokio::time::Instant> {
        self.current
            .values()
            .chain(self.departed.values().map(|departed| &departed.attachment))
            .filter_map(StreamAttachment::drain_deadline)
            .min()
    }
}

/// The earliest instant the reconciler must run again on its own.
fn earliest_reconcile_deadline(
    subscribe_failures: &HashMap<TrackedAgentEventStream, SubscribeBackoff>,
    tracked: &AttachedStreams,
) -> Option<tokio::time::Instant> {
    earliest_backoff_attempt(subscribe_failures)
        .into_iter()
        .chain(tracked.earliest_drain_deadline())
        .min()
}

/// One member event stream a reconciler has attached.
struct StreamAttachment {
    /// Unique per attached stream; matched against [`ForwardedAgentEvent::Closed`].
    generation: u64,
    /// What the stream's events are attributed to. Shared with the stream's
    /// own mapping so a same-actor rebinding re-keys it in place instead of
    /// opening a second stream to the same actor.
    attribution: Arc<std::sync::Mutex<StreamAttribution>>,
    /// The exact actor incarnation the stream belongs to, when its
    /// subscription named it (the console forwarder's local members).
    actor: Option<meerkat_session::LiveSessionActorWitness>,
    /// Cuts the stream off; it then yields [`ForwardedAgentEvent::Abandoned`]
    /// and closes.
    abort: futures::stream::AbortHandle,
    /// Since when this stream, as a predecessor, holds back its member's
    /// next attachment.
    held_since: Option<tokio::time::Instant>,
}

impl StreamAttachment {
    /// Record that this predecessor stream holds back its member's next
    /// attachment at `now`, and cut it off once it has done so for
    /// [`PREDECESSOR_DRAIN_DEADLINE`].
    fn hold(&mut self, now: tokio::time::Instant) {
        let since = *self.held_since.get_or_insert(now);
        if now >= since + PREDECESSOR_DRAIN_DEADLINE && !self.abort.is_aborted() {
            tracing::warn!(
                identity = %lock_attribution(&self.attribution).key.member_identity,
                deadline = ?PREDECESSOR_DRAIN_DEADLINE,
                "meta agent event forwarder: a revoked predecessor's stream stayed open past \
                 its drain deadline; cutting it off with an explicit gap so the successor attaches"
            );
            self.abort.abort();
        }
    }

    fn drain_deadline(&self) -> Option<tokio::time::Instant> {
        self.held_since
            .filter(|_| !self.abort.is_aborted())
            .map(|since| since + PREDECESSOR_DRAIN_DEADLINE)
    }
}

struct StreamAttribution {
    key: TrackedAgentEventStream,
    role: ProfileName,
    durable_identity: Option<Arc<str>>,
}

impl StreamAttribution {
    fn new(key: TrackedAgentEventStream, role: ProfileName) -> Self {
        let durable_identity = key.durable_identity.as_deref().map(Arc::<str>::from);
        Self {
            key,
            role,
            durable_identity,
        }
    }
}

fn lock_attribution(
    attribution: &std::sync::Mutex<StreamAttribution>,
) -> std::sync::MutexGuard<'_, StreamAttribution> {
    attribution
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Push `stream` into the reconciler's stream set under `key`, attributed
/// through a shared cell, and return its tracking record.
fn attach_member_event_stream(
    streams: &mut SelectAll<TaggedAgentEventStream>,
    key: TrackedAgentEventStream,
    role: ProfileName,
    stream: EventStream,
    actor: Option<meerkat_session::LiveSessionActorWitness>,
    epoch: Option<meerkat_core::comms::SessionEventEpoch>,
    forwarded: Option<Arc<std::sync::Mutex<ForwardedEventIds>>>,
) -> StreamAttachment {
    static NEXT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let generation = NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let attribution = Arc::new(std::sync::Mutex::new(StreamAttribution::new(key, role)));
    let events = Arc::clone(&attribution);
    let closing = Arc::clone(&attribution);
    let stream = match forwarded {
        Some(forwarded) => stream
            .filter(move |envelope| {
                let first = forwarded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .first_sighting(envelope.event_id);
                futures::future::ready(first)
            })
            .boxed(),
        None => stream,
    };
    let (stream, abort) = futures::stream::abortable(stream);
    let cut_off = abort.clone();
    let session_id = actor.as_ref().map(|actor| actor.session_id().clone());
    let mapped = stream
        .map(move |envelope| {
            let attribution = lock_attribution(&events);
            ForwardedAgentEvent::Event(Box::new((
                attribution.key.runtime_id.clone(),
                attribution.key.fence_token,
                attribution.role.clone(),
                envelope,
                attribution.durable_identity.clone(),
                epoch,
            )))
        })
        .chain(
            futures::stream::once(async move {
                let key = lock_attribution(&closing).key.clone();
                let abandoned = match session_id {
                    Some(session_id) if cut_off.is_aborted() => {
                        Some(ForwardedAgentEvent::Abandoned {
                            key: key.clone(),
                            session_id,
                            generation,
                        })
                    }
                    _ => None,
                };
                futures::stream::iter(
                    abandoned
                        .into_iter()
                        .chain([ForwardedAgentEvent::Closed(key, generation)]),
                )
            })
            .flatten(),
        )
        .boxed();
    streams.push(mapped);
    StreamAttachment {
        generation,
        attribution,
        actor,
        abort,
        held_since: None,
    }
}

/// Per-member subscribe-failure backoff for agent-event subscriptions.
/// The forwarder and independent identity-health monitor reconcile on
/// machine-state/mob-set change signals (plus a slow safety tick); without
/// backoff a member that keeps failing `subscribe_agent_events` is retried
/// on every wake indefinitely and floods the log (observed: ~49k "failed to
/// subscribe" warnings over 3.4h on a single wedged-retiring alias). Both
/// retry transient failures with exponential backoff; only the independent
/// health monitor reports persistent loss, once, at the threshold. A failure while
/// the member's mob is stopping or stopped is its teardown, not a transient
/// failure: it parks the member instead ([`mob_is_stopping_or_stopped`]).
struct SubscribeBackoff {
    /// When the next subscribe may be attempted. `None` parks the member
    /// while its mob is stopping or stopped: no deadline ends that, only the
    /// mob running again, so no retry is scheduled for it.
    next_attempt: Option<tokio::time::Instant>,
    consecutive_failures: u32,
}

/// First retry waits one backoff quantum; subsequent retries double up to a
/// cap so a persistently-unsubscribable member costs at most ~1 attempt per
/// `SUBSCRIBE_BACKOFF_MAX`.
const SUBSCRIBE_BACKOFF_BASE: Duration = Duration::from_millis(250);
const SUBSCRIBE_BACKOFF_MAX: Duration = Duration::from_secs(30);
const PERMANENT_STREAM_FAILURE_THRESHOLD: u32 = 4;

/// Upper bound between reconcile passes when no change signal fires. The
/// reconcilers are event-driven (machine-state watches + managed-mob-set
/// epoch + stream closures + backoff deadlines, plus identity binding changes
/// for the health monitor); this tick only bounds drift
/// from signals they cannot observe - a binding change that reaches the
/// identity runtime without a machine transition, and membership changes that
/// land inside the unwatched window while a brand-new mob's watcher is being
/// bound. It must
/// stay slow: the historical 250ms tick made every pass's full member
/// projection an idle-CPU driver on restore-scale mobs.
const RECONCILE_SAFETY_INTERVAL: Duration = Duration::from_secs(30);

/// Wakes the stream reconcilers when membership/binding truth may have moved,
/// replacing the historical 250ms polling tick.
///
/// Wake sources, in no priority order:
/// - any tracked mob's [`meerkat_mob::MobMachineStateChanges`] firing (the
///   mob actor publishes on every applied machine input),
/// - the managed mob-set epoch changing (child mob created/removed),
/// - Core's binding of an identity changing (identity health monitor only),
/// - the earliest pending subscribe-backoff deadline,
/// - the [`RECONCILE_SAFETY_INTERVAL`] safety tick.
///
/// Watchers are keyed by mob id and RETAINED across rebinds so their
/// internally-tracked seen-version survives: a state change landing while a
/// reconcile pass runs still wakes the next wait. Only a mob first seen by
/// the previous pass starts a fresh watcher (its pre-bind changes are covered
/// by that same pass's subscription attempt and by the safety tick). Closed
/// watchers (actor gone) are dropped on rebind and on wake so a destroyed mob
/// cannot busy-wake the loop.
struct ReconcileCadence {
    machine_watchers: BTreeMap<String, meerkat_mob::MobMachineStateChanges>,
    mob_set_changes: Option<tokio::sync::watch::Receiver<u64>>,
    /// Identity binding changes (identity health monitor only): the monitor
    /// attaches a member only once the identity runtime has registered it,
    /// and a registration can land with no mob machine transition after it.
    binding_changes: Option<tokio::sync::watch::Receiver<u64>>,
    /// Absolute deadline for the next safety reconcile, anchored at the last
    /// completed reconcile pass ([`Self::rebind`]). Persisting it here is
    /// load-bearing: the callers' outer `select!` drops and recreates the
    /// [`Self::wait`] future on every forwarded member event, so a deadline
    /// computed inside `wait` would reset under sustained event traffic and
    /// the safety reconcile would never fire.
    next_safety_deadline: tokio::time::Instant,
}

impl ReconcileCadence {
    fn new(agent_mob_mcp_state: &Option<Arc<meerkat_mob_mcp::MobMcpState>>) -> Self {
        Self {
            machine_watchers: BTreeMap::new(),
            mob_set_changes: agent_mob_mcp_state
                .as_ref()
                .map(|state| state.mob_set_changes()),
            binding_changes: None,
            next_safety_deadline: tokio::time::Instant::now() + RECONCILE_SAFETY_INTERVAL,
        }
    }

    /// Also wake on identity binding changes.
    fn with_binding_changes(mut self, binding_changes: tokio::sync::watch::Receiver<u64>) -> Self {
        self.binding_changes = Some(binding_changes);
        self
    }

    /// Rebind the watcher set to the exact handles the reconcile pass just
    /// enumerated, keeping existing watchers (and their seen-versions) alive.
    /// Every reconcile pass ends here, so this is also where the safety
    /// deadline is re-armed: drift from watch-invisible signals is bounded
    /// relative to the last reconcile, not the last wake attempt.
    fn rebind(&mut self, handles: &[MobHandle]) {
        let mut next = BTreeMap::new();
        for handle in handles {
            let key = handle.mob_id().to_string();
            let watcher = self
                .machine_watchers
                .remove(&key)
                .unwrap_or_else(|| handle.machine_state_changes());
            if !watcher.is_closed() {
                next.insert(key, watcher);
            }
        }
        self.machine_watchers = next;
        self.next_safety_deadline = tokio::time::Instant::now() + RECONCILE_SAFETY_INTERVAL;
    }

    /// Wait for the next reconcile trigger. `next_backoff_attempt` is the
    /// earliest pending subscribe retry, if any.
    async fn wait(&mut self, next_backoff_attempt: Option<tokio::time::Instant>) {
        let now = tokio::time::Instant::now();
        let mut deadline = self.next_safety_deadline;
        if let Some(attempt) = next_backoff_attempt {
            deadline = deadline.min(attempt.max(now));
        }

        let Self {
            machine_watchers,
            mob_set_changes,
            binding_changes,
            ..
        } = self;

        // Await "any machine watcher fired". A closed watcher is removed
        // in-place so it cannot immediately re-wake the caller.
        let machine_change = async {
            if machine_watchers.is_empty() {
                std::future::pending::<()>().await;
                return;
            }
            let keys: Vec<String> = machine_watchers.keys().cloned().collect();
            let closed_key = {
                let futures: Vec<_> = machine_watchers
                    .values_mut()
                    .map(|watcher| Box::pin(watcher.changed()))
                    .collect();
                let (result, index, rest) = futures::future::select_all(futures).await;
                drop(rest);
                result.is_err().then(|| keys[index].clone())
            };
            if let Some(key) = closed_key {
                machine_watchers.remove(&key);
            }
        };

        let mob_set_change = async {
            match mob_set_changes.as_mut() {
                Some(rx) => rx.changed().await,
                None => std::future::pending().await,
            }
        };

        let binding_change = async {
            match binding_changes.as_mut() {
                Some(rx) => rx.changed().await,
                None => std::future::pending().await,
            }
        };

        let (mob_set_closed, binding_closed) = tokio::select! {
            () = machine_change => (false, false),
            result = mob_set_change => (result.is_err(), false),
            result = binding_change => (false, result.is_err()),
            () = tokio::time::sleep_until(deadline) => (false, false),
        };
        if binding_closed {
            self.binding_changes = None;
        }
        if mob_set_closed {
            // The dispatcher state is gone; a closed watch completes
            // immediately, so it must not stay selectable.
            self.mob_set_changes = None;
        }
    }
}

/// Earliest pending subscribe-backoff deadline, if any member is waiting.
fn earliest_backoff_attempt(
    subscribe_failures: &HashMap<TrackedAgentEventStream, SubscribeBackoff>,
) -> Option<tokio::time::Instant> {
    subscribe_failures
        .values()
        .filter_map(|backoff| backoff.next_attempt)
        .min()
}

fn subscribe_backoff_delay(consecutive_failures: u32) -> Duration {
    SUBSCRIBE_BACKOFF_BASE
        .saturating_mul(1u32 << consecutive_failures.min(7))
        .min(SUBSCRIBE_BACKOFF_MAX)
}

/// Whether the console forwarder should hold a live agent-event subscription
/// for a member in this lifecycle state. Only `Active` members have a live
/// runtime delta stream; subscribing a `Retiring`/`Broken`/`Completed` member
/// (which can still carry stale binding atoms) fails every reconcile tick.
fn forwarder_should_subscribe(status: MobMemberStatus) -> bool {
    matches!(status, MobMemberStatus::Active)
}

/// Whether `handle`'s mob is stopping or stopped, so a member subscribe that
/// failed is the mob tearing its members down rather than a spawn or resume
/// race: the mob's admission is closed (it never reopens), or its last
/// published phase is Stopped, Completed or Destroyed. Each teardown Meta
/// drives says so before the members' sessions end: its shutdown stops the
/// mob before the terminal teardown (Meerkat's own `MobHandle::shutdown`
/// publishes Stopped only at its end), its child stand-down closes each
/// child's admission first, and a destroy publishes Destroyed when its
/// durable destroy marker is written, before the members are disposed. A
/// destroy also marks every member Retiring, and the forwarder subscribes
/// only Active members ([`forwarder_should_subscribe`]). What the actor
/// answered does not count: an actor that refuses with a closed channel
/// while its mob runs and admits died, and its members keep the warning,
/// the backoff and the failure count.
fn mob_is_stopping_or_stopped(handle: &MobHandle) -> bool {
    handle.admission_closed().borrow().is_some()
        || matches!(
            handle.status_observation_snapshot(),
            MobState::Stopped | MobState::Completed | MobState::Destroyed
        )
}

fn durable_identity_label(labels: &BTreeMap<String, String>) -> Option<String> {
    labels.get("agent_identity").cloned()
}

async fn identity_member_registered(
    primary_mob_id: &str,
    mob_id: &str,
    durable_identity: Option<&str>,
    identity_runtime: Option<
        &Arc<std::sync::RwLock<Option<Arc<crate::identity_first::IdentityRuntime>>>>,
    >,
) -> bool {
    if mob_id != primary_mob_id {
        return false;
    }
    let (Some(durable_identity), Some(identity_runtime)) = (durable_identity, identity_runtime)
    else {
        return false;
    };
    let authority = identity_runtime
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let (Some(authority), Ok(identity)) = (
        authority,
        crate::identity_first::AgentIdentity::parse(durable_identity),
    ) else {
        return false;
    };
    authority.contains(&identity).await
}

/// The per-session envelope sequence through which run terminals were
/// credited to the identity completion cursor.
///
/// Meerkat numbers a session's events monotonically within one sequence
/// space (a session service for a local member, an event pump residency for
/// a placed one; [`meerkat_core::comms::SessionEventEpoch`]) and replays
/// retained events from a cursor. The identity health monitor's replaying
/// streams and the completion-cursor drain ([`ReplayedCompletionDrain`]) both
/// read those events; each claims an envelope's sequence before crediting it,
/// and a claim succeeds only above the session's high-water in the same
/// space, so every envelope is credited by exactly one of them. Both read in
/// sequence order, so an envelope skipped by one was already claimed by the
/// other. A terminal is claimed and credited under the identity's
/// completion-credit lock ([`crate::identity_first::IdentityRuntime::credit_observed_terminal`]),
/// which a cursor read holds across its drain, so a read never sees a claim
/// without its credit. A new sequence space (a placed member's host restart)
/// restarts the session's high-water.
#[derive(Default)]
struct HealthCompletionLedger {
    sessions: std::sync::Mutex<HashMap<meerkat_core::types::SessionId, SessionCreditLedger>>,
}

/// One session's credited positions, per sequence space. A space's
/// high-water is kept even after a newer space appears, so a late stream of
/// an older space (a replaced placed residency) re-attaching claims nothing
/// it already credited.
#[derive(Default)]
struct SessionCreditLedger {
    /// The newest space observed; the drain resumes in it.
    current: ResumeSpace,
    high_water: HashMap<Option<meerkat_core::comms::SessionEventEpoch>, u64>,
}

/// The sequence space a session's drain resumes in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ResumeSpace {
    /// Nothing claimed yet: read from the earliest retained event.
    #[default]
    Unclaimed,
    /// The newest space claimed in is unnamed (a live-only source): no
    /// cursor can resume in it.
    Unnamed,
    /// Resume after this space's high-water.
    Named(meerkat_core::comms::SessionEventEpoch),
}

impl HealthCompletionLedger {
    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<meerkat_core::types::SessionId, SessionCreditLedger>>
    {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Claim `seq` of `session_id` in sequence space `epoch` for crediting.
    /// `false` when an envelope at or past it was already claimed in that
    /// space. Synthetic gap markers carry sequence 0 and are never claimed.
    fn claim(
        &self,
        session_id: &meerkat_core::types::SessionId,
        epoch: Option<meerkat_core::comms::SessionEventEpoch>,
        seq: u64,
    ) -> bool {
        if seq == 0 {
            return false;
        }
        let mut sessions = self.lock();
        let session = sessions.entry(session_id.clone()).or_default();
        if !session.high_water.contains_key(&epoch) {
            // A space not seen before is the newest one.
            session.current = match epoch {
                Some(epoch) => ResumeSpace::Named(epoch),
                None => ResumeSpace::Unnamed,
            };
        }
        let high_water = session.high_water.entry(epoch).or_default();
        if seq <= *high_water {
            return false;
        }
        *high_water = seq;
        true
    }

    /// Where the drain resumes reading `session_id`'s events: after the
    /// newest space's high-water, or from the earliest retained event.
    fn resume_cursor(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> meerkat_core::comms::SessionEventCursor {
        let sessions = self.lock();
        let Some(session) = sessions.get(session_id) else {
            return meerkat_core::comms::SessionEventCursor::Earliest;
        };
        match session.current {
            ResumeSpace::Named(epoch) => meerkat_core::comms::SessionEventCursor::After {
                epoch,
                seq: session.high_water.get(&Some(epoch)).copied().unwrap_or(0),
            },
            _ => meerkat_core::comms::SessionEventCursor::Earliest,
        }
    }
}

/// Credits the run terminals a member's actor already published but the
/// identity health monitor has not credited yet, before every completion
/// cursor read ([`crate::identity_first::runtime::PendingCompletionDrain`]).
///
/// It replays the session's retained events after the ledger's high-water.
/// Meerkat captures the replayed prefix atomically with publication, so
/// every terminal published before the read is in it; only that prefix is
/// consumed (the subscription's live tail is left to the monitor). The
/// caller holds the identity's completion-credit lock throughout.
struct ReplayedCompletionDrain {
    session_service: Arc<dyn meerkat_mob::MobSessionService>,
    ledger: Arc<HealthCompletionLedger>,
}

#[async_trait::async_trait]
impl crate::identity_first::runtime::PendingCompletionDrain for ReplayedCompletionDrain {
    async fn drain(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> crate::identity_first::runtime::PendingRunTerminals {
        use futures::FutureExt as _;
        use meerkat_core::comms::{SessionEventCursorRejection, StreamError};
        let mut terminals = crate::identity_first::runtime::PendingRunTerminals::default();
        let mut cursor = self.ledger.resume_cursor(session_id);
        let subscription = loop {
            match self
                .session_service
                .subscribe_agent_session_events_from(session_id, cursor)
                .await
            {
                Ok(subscription) => break subscription,
                // A new sequence space: read it from its earliest retained
                // event. Its envelopes are claimed in their own space's
                // high-water, so nothing it holds was credited before.
                Err(StreamError::CursorRejected {
                    reason: SessionEventCursorRejection::EpochMismatch { .. },
                    ..
                }) if cursor != meerkat_core::comms::SessionEventCursor::Earliest => {
                    cursor = meerkat_core::comms::SessionEventCursor::Earliest;
                }
                // No live actor serves the session here (a placed member, or
                // one not materialized yet): nothing is retained to credit.
                Err(_) => return terminals,
            }
        };
        let epoch = subscription.epoch;
        let mut stream = subscription.stream;
        while let Some(Some(envelope)) = stream.next().now_or_never() {
            // Claim every envelope so the high-water tracks the read
            // position; only a claimed terminal is credited.
            let claimed = self.ledger.claim(session_id, epoch, envelope.seq);
            match envelope.payload {
                AgentEvent::RunCompleted { .. } if claimed => terminals.completed += 1,
                AgentEvent::RunFailed { .. } if claimed => terminals.failed += 1,
                _ => {}
            }
        }
        terminals
    }
}

/// Advance an identity's completion cursor when meerkat reports that one of
/// its turns finished.
///
/// This is the only place the cursor moves in production, and it is driven by
/// the run-completion EVENT rather than by polling a projection on purpose: a
/// poll cannot distinguish "new turn, byte-identical output" from "no new
/// turn", which is exactly the defect the cursor closes. The identity health
/// monitor is the right host because it drains its own subscription set — a
/// full console channel cannot starve it.
///
/// Losing the subscription mid-turn means a missed completion, so the cursor
/// under-counts rather than over-counts: a waiter times out instead of being
/// told a turn finished that did not. A completion that happened before a
/// cursor read is never credited after it: every cursor read first credits
/// the completions the actor already published
/// ([`ReplayedCompletionDrain`]), and the [`HealthCompletionLedger`] lets
/// each envelope be credited once, by the drain or by the monitor's
/// replaying stream, claimed and credited under the identity's
/// completion-credit lock. Returns whether this envelope credited a
/// completion.
async fn record_identity_turn_completion(
    identity_runtime: &Arc<std::sync::RwLock<Option<Arc<crate::identity_first::IdentityRuntime>>>>,
    ledger: &HealthCompletionLedger,
    durable_identity: Option<&str>,
    epoch: Option<meerkat_core::comms::SessionEventEpoch>,
    envelope: &meerkat_core::event::EventEnvelope<AgentEvent>,
) -> bool {
    let claim = || {
        envelope
            .source_session_id()
            .is_some_and(|session_id| ledger.claim(session_id, epoch, envelope.seq))
    };
    let failed = match envelope.payload {
        AgentEvent::RunCompleted { .. } => false,
        AgentEvent::RunFailed { .. } => true,
        // Claim every envelope, not only terminals, so the ledger's
        // high-water tracks the stream's position.
        _ => {
            claim();
            return false;
        }
    };
    let authority = identity_runtime
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let identity = durable_identity.and_then(|label| {
        crate::identity_first::AgentIdentity::parse(label)
            .inspect_err(|error| {
                tracing::debug!(
                    identity = %label,
                    error = %error,
                    "meta identity health monitor: run completion carried an unparseable durable identity"
                );
            })
            .ok()
    });
    let (Some(authority), Some(identity)) = (authority, identity) else {
        claim();
        return false;
    };
    authority
        .credit_observed_terminal(&identity, failed, claim)
        .await
}

fn note_identity_stream_closed(
    primary_mob_id: &str,
    tracked_key: &TrackedAgentEventStream,
    identity_runtime: &Arc<std::sync::RwLock<Option<Arc<crate::identity_first::IdentityRuntime>>>>,
    detail: &str,
) {
    if tracked_key.mob_id != primary_mob_id {
        return;
    }
    let Some(durable_identity) = tracked_key.durable_identity.as_deref() else {
        return;
    };
    let authority = identity_runtime
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(authority) = authority else {
        return;
    };
    let identity = match crate::identity_first::AgentIdentity::parse(durable_identity) {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(
                identity = %durable_identity,
                error = %error,
                "meta agent event forwarder: roster identity cannot be mapped to identity authority"
            );
            return;
        }
    };
    // A closed stream is a fact about Meta's subscription, not about the
    // member: Core owns the member's liveness and restore, so nothing
    // is marked broken or repaired here. The note wakes the views and the
    // completion waiters; the reconcile loop re-attaches the stream.
    authority.note_member_stream_closed(&identity, detail);
}

async fn run_resilient_mob_agent_event_forwarder(
    handle: MobHandle,
    agent_mob_mcp_state: Option<Arc<meerkat_mob_mcp::MobMcpState>>,
    event_tx: Sender<ForwardedMemberEvent>,
    mob_events: MobEventsStore,
) {
    let mut streams: SelectAll<TaggedAgentEventStream> = SelectAll::new();
    let mut tracked = AttachedStreams::default();
    let mut subscribe_failures: HashMap<TrackedAgentEventStream, SubscribeBackoff> = HashMap::new();
    let mut cadence = ReconcileCadence::new(&agent_mob_mcp_state);

    let handles = Box::pin(reconcile_agent_event_streams(
        &handle,
        &agent_mob_mcp_state,
        &mut tracked,
        &mut subscribe_failures,
        &mut streams,
        None,
    ))
    .await;
    cadence.rebind(&handles);
    // Only the watchers outlive the pass. A child mob handle kept for the
    // forwarder's life would keep that child's database observer alive, and
    // the child's destroy would leave its storage cleanup outstanding.
    drop(handles);

    loop {
        tokio::select! {
            Some(forwarded) = streams.next() => {
                match forwarded {
                    ForwardedAgentEvent::Event(event) => {
                        let (source, source_fence_token, role, envelope, _durable_identity, _epoch) = *event;
                        let attributed_event = AttributedEvent {
                            source,
                            source_fence_token,
                            role,
                            envelope,
                        };
                        // Fan out to the structural mob events store. Today this is a
                        // no-op for attributed agent events (they don't carry mob/run/
                        // step fields), but the projection seam keeps the surface
                        // symmetric with the structural `MobEvent` subscriber and lets
                        // future code add attribution without touching this shape.
                        let _ = mob_events.project_attributed_event(&attributed_event).await;
                        if event_tx
                            .send(forwarded_member_event(attributed_event))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    ForwardedAgentEvent::Abandoned { key, session_id, generation } => {
                        if event_tx
                            .send(predecessor_stream_gap_event(&key, &session_id, generation))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    ForwardedAgentEvent::Closed(tracked_key, generation) => {
                        if tracked.close(&tracked_key, generation) {
                            subscribe_failures.remove(&tracked_key);
                        }
                        // A closure is itself the re-subscribe trigger: the
                        // member may still be live (stream lag/teardown race),
                        // and no machine transition is guaranteed to follow.
                        let handles = Box::pin(reconcile_agent_event_streams(&handle, &agent_mob_mcp_state, &mut tracked, &mut subscribe_failures, &mut streams, None)).await;
                        cadence.rebind(&handles);
                    }
                }
            }
            () = cadence.wait(earliest_reconcile_deadline(&subscribe_failures, &tracked)) => {
                let handles = Box::pin(reconcile_agent_event_streams(&handle, &agent_mob_mcp_state, &mut tracked, &mut subscribe_failures, &mut streams, None)).await;
                cadence.rebind(&handles);
            }
        }
    }
}

/// Drain a second subscription set dedicated to identity health. Keeping this
/// task separate from console/event projection ensures a full user-facing
/// output channel cannot suppress permanent stream-loss detection.
async fn run_identity_stream_health_monitor(
    handle: MobHandle,
    agent_mob_mcp_state: Option<Arc<meerkat_mob_mcp::MobMcpState>>,
    identity_runtime: Arc<std::sync::RwLock<Option<Arc<crate::identity_first::IdentityRuntime>>>>,
    completion_ledger: Arc<HealthCompletionLedger>,
    binding_changes: tokio::sync::watch::Receiver<u64>,
) {
    let mut streams: SelectAll<TaggedAgentEventStream> = SelectAll::new();
    let mut tracked = AttachedStreams::default();
    let mut subscribe_failures: HashMap<TrackedAgentEventStream, SubscribeBackoff> = HashMap::new();
    // One typed wake beyond machine changes: an identity binding change. A
    // member is attached only once the identity runtime registered it, and
    // its subscription replays the actor's retained events from the first,
    // so the completion of a turn that ran before the attach is still read,
    // once, in order: credited by the next cursor read's drain or by this
    // stream, whichever claims it first in the completion ledger.
    let mut cadence =
        ReconcileCadence::new(&agent_mob_mcp_state).with_binding_changes(binding_changes);

    let handles = Box::pin(reconcile_agent_event_streams(
        &handle,
        &agent_mob_mcp_state,
        &mut tracked,
        &mut subscribe_failures,
        &mut streams,
        Some(&identity_runtime),
    ))
    .await;
    cadence.rebind(&handles);
    // As in the forwarder: no child mob handle outlives the pass.
    drop(handles);

    loop {
        tokio::select! {
            Some(forwarded) = streams.next() => {
                match forwarded {
                    ForwardedAgentEvent::Event(event) => {
                        let (_, _, _, envelope, durable_identity, epoch) = *event;
                        let _credited = record_identity_turn_completion(
                            &identity_runtime,
                            &completion_ledger,
                            durable_identity.as_deref(),
                            epoch,
                            &envelope,
                        ).await;
                    }
                    // A predecessor actor cut off after the drain deadline:
                    // its unread completions are lost to the cursor, which
                    // under-counts (a waiter times out) rather than guesses.
                    ForwardedAgentEvent::Abandoned { .. } => {}
                    ForwardedAgentEvent::Closed(tracked_key, generation) => {
                        if tracked.close(&tracked_key, generation) {
                            subscribe_failures.remove(&tracked_key);
                        }
                        note_identity_stream_closed(
                            handle.mob_id().as_str(),
                            &tracked_key,
                            &identity_runtime,
                            "live agent event stream closed permanently",
                        );
                        // Re-attach promptly after a closure; the attach must
                        // not wait for an unrelated machine transition.
                        let handles = Box::pin(reconcile_agent_event_streams(
                            &handle,
                            &agent_mob_mcp_state,
                            &mut tracked,
                            &mut subscribe_failures,
                            &mut streams,
                            Some(&identity_runtime),
                        )).await;
                        cadence.rebind(&handles);
                    }
                }
            }
            () = cadence.wait(earliest_reconcile_deadline(&subscribe_failures, &tracked)) => {
                let handles = Box::pin(reconcile_agent_event_streams(
                    &handle,
                    &agent_mob_mcp_state,
                    &mut tracked,
                    &mut subscribe_failures,
                    &mut streams,
                    Some(&identity_runtime),
                )).await;
                cadence.rebind(&handles);
            }
        }
    }
}

/// Returns the handles it enumerated (primary + child mobs) so the caller can
/// rebind its [`ReconcileCadence`] watchers to the same set.
async fn reconcile_agent_event_streams(
    handle: &MobHandle,
    agent_mob_mcp_state: &Option<Arc<meerkat_mob_mcp::MobMcpState>>,
    tracked: &mut AttachedStreams,
    subscribe_failures: &mut HashMap<TrackedAgentEventStream, SubscribeBackoff>,
    streams: &mut SelectAll<TaggedAgentEventStream>,
    identity_runtime: Option<
        &Arc<std::sync::RwLock<Option<Arc<crate::identity_first::IdentityRuntime>>>>,
    >,
) -> Vec<MobHandle> {
    let primary_mob_id = handle.mob_id().to_string();
    let mut handles = vec![handle.clone()];
    if let Some(state) = agent_mob_mcp_state {
        handles.extend(
            Box::pin(state.mob_handles_snapshot())
                .await
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(mob_id, child_handle)| {
                    if mob_id.as_str() == primary_mob_id {
                        None
                    } else {
                        Some(child_handle)
                    }
                }),
        );
    }

    let mut current: HashSet<TrackedAgentEventStream> = HashSet::new();
    for handle in &handles {
        let mob_id = handle.mob_id().to_string();
        for entry in handle.list_members_including_retiring().await {
            // Members without current machine-supplied binding atoms have no
            // live runtime stream to track; their stale streams age out.
            let Some((runtime_id, fence_token)) = entry.binding_atoms() else {
                continue;
            };
            let durable_identity = durable_identity_label(&entry.labels);
            let identity_registered = identity_member_registered(
                &primary_mob_id,
                &mob_id,
                durable_identity.as_deref(),
                identity_runtime,
            )
            .await;
            // The health monitor serves the identity runtime's members only.
            // Avoid duplicating every ordinary/child-mob event stream.
            if identity_runtime.is_some() && !identity_registered {
                continue;
            }
            current.insert(TrackedAgentEventStream {
                mob_id: mob_id.clone(),
                durable_identity,
                member_identity: entry.agent_identity.clone(),
                runtime_id,
                fence_token,
            });
        }
    }

    // Keys that left the roster stop being served. A stream of a known
    // actor stays accounted for until it closes (`AttachedStreams::depart`):
    // re-keyed below if its member was rebound onto that same live actor,
    // otherwise holding back the member's successor until it drained.
    let departed: Vec<TrackedAgentEventStream> = tracked
        .current
        .keys()
        .filter(|tracked_key| !current.contains(*tracked_key))
        .cloned()
        .collect();
    for tracked_key in departed {
        tracked.depart(&tracked_key, identity_runtime.is_some());
    }
    // Drop backoff bookkeeping for members that have left the roster so the
    // map can't grow without bound across the runtime's lifetime.
    subscribe_failures.retain(|key, _| current.contains(key));
    tracked.retain_forwarded_for(&current);
    // The console forwarder tracks the exact actor incarnation so a revoked
    // predecessor's stream drains before its successor's; the identity
    // health monitor keeps its ordinary untracked streams. Both replay each
    // actor's retained events from the first, so a run that started before
    // the attach is still read from `RunStarted`.
    let console = identity_runtime.is_none();

    for handle in &handles {
        let mob_id = handle.mob_id().to_string();
        for entry in handle.list_members_including_retiring().await {
            let identity = entry.agent_identity.clone();
            // No binding atoms means no live runtime to subscribe to.
            let Some((runtime_id, fence_token)) = entry.binding_atoms() else {
                continue;
            };
            let durable_identity = durable_identity_label(&entry.labels);
            let identity_registered = identity_member_registered(
                &primary_mob_id,
                &mob_id,
                durable_identity.as_deref(),
                identity_runtime,
            )
            .await;
            if identity_runtime.is_some() && !identity_registered {
                // An identity member the runtime has not registered yet is
                // attached by the binding wake's pass.
                continue;
            }
            let tracked_key = TrackedAgentEventStream {
                mob_id: mob_id.clone(),
                durable_identity,
                member_identity: identity.clone(),
                runtime_id,
                fence_token,
            };
            if let Some(attachment) = tracked.current.get_mut(&tracked_key) {
                // Served. A served stream whose actor was revoked (a
                // replacement kept the same binding atoms) keeps serving until
                // its close re-runs this pass: the successor is subscribed
                // only after the predecessor's last event, so no predecessor
                // event can follow the successor's first on the console,
                // which attributes lineage-less events to the identity's
                // current run. Bounded by the drain deadline.
                if attachment
                    .actor
                    .as_ref()
                    .is_some_and(|actor| !actor.is_live())
                {
                    attachment.hold(tokio::time::Instant::now());
                }
                continue;
            }

            // Only Active members have a live runtime delta stream to attach
            // to. A Retiring/Broken/Completed member can still carry stale
            // binding atoms (so `binding_atoms()` is Some) while its session
            // injector is already gone, which makes `subscribe_agent_events`
            // fail every reconcile tick — the source of the 4×/s forwarder
            // hot-loop. Such members are skipped here; their final events
            // arrive via the structural ledger / session-history backfill and
            // their streams age out once their binding leaves the roster.
            if !forwarder_should_subscribe(entry.status) {
                subscribe_failures.remove(&tracked_key);
                continue;
            }

            // Same ordering rule across a rebinding: a revoked predecessor's
            // stream for this member drains first, and its close re-runs
            // this pass.
            let owner = MemberStreamOwner::of(&tracked_key);
            if tracked.holds_departed(&owner) {
                let current_session = handle.resolve_bridge_session_id(&identity).await;
                if tracked.predecessor_draining(
                    &owner,
                    current_session.as_ref(),
                    tokio::time::Instant::now(),
                ) {
                    continue;
                }
            }

            // The member's session, observed together with this exact
            // binding, only when a departed stream could answer for it.
            let bound_session = if tracked.holds_departed(&owner) {
                match handle.resolve_bridge_session_id(&identity).await {
                    Some(candidate) if tracked.live_departed_for(&owner, &candidate).is_some() => {
                        match observe_bound_session(handle, &tracked_key).await {
                            Some(session_id) => Some(session_id),
                            // This member's binding or session moved while
                            // observing; the machine change that moved it
                            // re-runs this pass. Nothing is attached
                            // meanwhile, so a successor's stream never lands
                            // under a predecessor's binding.
                            None => continue,
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

            // Rebound onto the same live actor: move its stream to the new
            // binding, attributing its events from now on to it.
            if let Some(generation) = bound_session
                .as_ref()
                .and_then(|session_id| tracked.live_departed_for(&owner, session_id))
                && let Some(DepartedStream { attachment, .. }) =
                    tracked.departed.remove(&generation)
            {
                *lock_attribution(&attachment.attribution) =
                    StreamAttribution::new(tracked_key.clone(), entry.role.clone());
                subscribe_failures.remove(&tracked_key);
                tracked.current.insert(tracked_key, attachment);
                continue;
            }

            // Back off an Active member that keeps failing to subscribe (a
            // genuinely stuck injector), so even that case can't spin the log.
            // A member parked by its mob's teardown waits for the mob to run
            // again.
            let now = tokio::time::Instant::now();
            if let Some(backoff) = subscribe_failures.get(&tracked_key) {
                let held = match backoff.next_attempt {
                    Some(next_attempt) => now < next_attempt,
                    None => mob_is_stopping_or_stopped(handle),
                };
                if held {
                    continue;
                }
            }

            let role = entry.role.clone();

            // The health monitor replays only while a completion-cursor
            // drain credits what happened before its attach; without one a
            // replayed completion could satisfy a baseline read before it.
            let replay = console
                || identity_runtime.is_some_and(|slot| {
                    slot.read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .as_ref()
                        .is_some_and(|runtime| runtime.has_pending_completion_drain())
                });
            let subscription =
                subscribe_agent_events_for_console_forwarder(handle, &identity, console, replay)
                    .await;
            match subscription {
                Ok((stream, actor, epoch)) => {
                    subscribe_failures.remove(&tracked_key);
                    let forwarded = console.then(|| tracked.forwarded_for(&owner));
                    let attachment = attach_member_event_stream(
                        streams,
                        tracked_key.clone(),
                        role,
                        stream,
                        actor,
                        epoch,
                        forwarded,
                    );
                    tracked.current.insert(tracked_key, attachment);
                }
                Err(error) => {
                    // The member's mob is tearing its members down (a
                    // shutdown stops the mob, then ends their sessions and
                    // its actor): the failure is the teardown. Park the
                    // member with no retry and nothing above debug until the
                    // mob runs again; identity repair does not count it.
                    if mob_is_stopping_or_stopped(handle) {
                        if identity_runtime.is_none() {
                            tracing::debug!(
                                mob_id = %mob_id,
                                identity = %identity,
                                error = %error,
                                "meta agent event forwarder: subscribe failed while the member's \
                                 mob is stopping or stopped; not retrying until it runs again"
                            );
                        }
                        subscribe_failures
                            .entry(tracked_key)
                            .and_modify(|backoff| backoff.next_attempt = None)
                            .or_insert(SubscribeBackoff {
                                next_attempt: None,
                                consecutive_failures: 0,
                            });
                        continue;
                    }
                    // Usually a short-lived spawn/resume race while Meerkat
                    // finishes installing the session event injector. Retry
                    // with exponential backoff and warn only on the first
                    // failure. A bounded number of misses remains a spawn
                    // race; persistent loss is reported once, at the
                    // threshold, and Core owns the member, so nothing here
                    // repairs it.
                    let lost_key = tracked_key.clone();
                    let backoff =
                        subscribe_failures
                            .entry(tracked_key)
                            .or_insert(SubscribeBackoff {
                                next_attempt: Some(now),
                                consecutive_failures: 0,
                            });
                    // The identity health monitor runs this pass too: a
                    // member it cannot watch is logged like one the console
                    // forwarder cannot.
                    let watcher = if identity_runtime.is_some() {
                        "identity_health_monitor"
                    } else {
                        "console_forwarder"
                    };
                    if backoff.consecutive_failures == 0 {
                        tracing::warn!(
                            mob_id = %mob_id,
                            identity = %identity,
                            watcher,
                            error = %error,
                            "meta agent event forwarder: failed to subscribe; will retry with backoff"
                        );
                    } else {
                        tracing::debug!(
                            mob_id = %mob_id,
                            identity = %identity,
                            watcher,
                            error = %error,
                            consecutive_failures = backoff.consecutive_failures,
                            "meta agent event forwarder: subscribe still failing; backing off"
                        );
                    }
                    backoff.next_attempt =
                        Some(now + subscribe_backoff_delay(backoff.consecutive_failures));
                    backoff.consecutive_failures = backoff.consecutive_failures.saturating_add(1);
                    if backoff.consecutive_failures == PERMANENT_STREAM_FAILURE_THRESHOLD
                        && let Some(identity_runtime) = identity_runtime
                    {
                        note_identity_stream_closed(
                            &primary_mob_id,
                            &lost_key,
                            identity_runtime,
                            "live agent event stream remained unavailable after bounded retries",
                        );
                    }
                }
            }
        }
    }
    handles
}

/// The session `key`'s member is bound to, observed while the member's
/// Active binding is still `key`: the member's session is read before and
/// after its binding, and must not have moved in between. `None` when the
/// binding is no longer `key` or the member's session moved; the machine
/// change that moved it re-runs the reconciler. Only this member's own
/// state is compared, so unrelated machine traffic cannot void it.
/// Adoption and re-keying go through this, so a stream is never attributed
/// to a binding its session might not belong to.
async fn observe_bound_session(
    handle: &MobHandle,
    key: &TrackedAgentEventStream,
) -> Option<meerkat_core::types::SessionId> {
    let before = handle
        .resolve_bridge_session_id(&key.member_identity)
        .await?;
    let bound = handle
        .list_members_including_retiring()
        .await
        .into_iter()
        .any(|entry| {
            entry.agent_identity == key.member_identity
                && forwarder_should_subscribe(entry.status)
                && entry
                    .binding_atoms()
                    .is_some_and(|(runtime_id, fence_token)| {
                        runtime_id == key.runtime_id && fence_token == key.fence_token
                    })
        });
    let after = handle.resolve_bridge_session_id(&key.member_identity).await;
    (bound && after.as_ref() == Some(&before)).then_some(before)
}

/// A mob whose member event subscriptions fail, for the tests of a
/// subscription that cannot attach.
#[cfg(test)]
static FAIL_MEMBER_EVENT_SUBSCRIPTIONS_OF_MOB: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);

async fn subscribe_agent_events_for_console_forwarder(
    handle: &MobHandle,
    identity: &AgentIdentity,
    console: bool,
    replay: bool,
) -> Result<
    (
        EventStream,
        Option<meerkat_session::LiveSessionActorWitness>,
        Option<meerkat_core::comms::SessionEventEpoch>,
    ),
    meerkat_mob::MobError,
> {
    #[cfg(test)]
    if FAIL_MEMBER_EVENT_SUBSCRIPTIONS_OF_MOB
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_deref()
        == Some(handle.mob_id().as_str())
    {
        return Err(meerkat_mob::MobError::Internal(
            "member event subscription failed (test)".to_string(),
        ));
    }
    // Keep the console forwarder on the same authoritative subscription path
    // as `/agents/{id}/events`. The observation shortcut can lag the actor's
    // runtime-member projection in identity-first/runtime-backed packs, which
    // leaves the console with only session-history backfill while direct agent
    // SSE streams live deltas correctly.
    //
    // Meerkat retains each actor incarnation's recent events, so replaying
    // from the earliest retained one covers a run that started before this
    // attach (a restored member's first run included); a gap beyond the
    // retained window arrives as meerkat's typed `StreamTruncated` marker.
    let cursor = if replay {
        meerkat_core::comms::SessionEventCursor::Earliest
    } else {
        meerkat_core::comms::SessionEventCursor::Live
    };
    let subscription = handle.subscribe_agent_events_from(identity, cursor).await?;
    // The identity health monitor does not hold successors back, so it
    // leaves the actor untracked (its departed streams are cut off instead).
    let actor = if console { subscription.actor } else { None };
    Ok((subscription.stream, actor, subscription.epoch))
}

/// Streaming subscription against the meerkat mob event ledger. Each
/// projected envelope's cursor is the upstream `MobEvent.cursor`; after
/// projection the cursor is checkpointed via `persistent_metadata` so
/// the next runtime instance can resume from where this one left off.
///
/// Resume semantics on startup:
/// - persisted cursor present → `subscribe_after(cursor)`. On
///   `MobError::StaleEventCursor` (the ledger has been truncated past
///   our checkpoint) the task logs a warning and falls through to a
///   fresh `subscribe()` at the current latest.
/// - no persisted cursor → `subscribe()` (latest, no replay).
///
/// Exits when the upstream `event_rx` closes (machine destroyed) or
/// when subscription setup fails after a stale-cursor fallback.
async fn run_mob_events_subscription(
    handle: MobHandle,
    store: MobEventsStore,
    persistent_metadata: Arc<dyn PersistentMetadataStore>,
    idle_retire_overrides: Option<crate::mob_handle_runtime::ImplicitDelegateRetirementOverrides>,
) {
    let mob_id = handle.mob_id().as_str().to_string();
    let resume_cursor = match persistent_metadata.get_subscription_cursor(&mob_id).await {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                mob_id = %mob_id,
                error = %err,
                "mob_events subscription: failed to read persisted cursor; resuming from latest"
            );
            None
        }
    };

    let events = handle.events();
    let mut subscription = match resume_cursor {
        Some(cursor) => match events.subscribe_after(cursor).await {
            Ok(sub) => sub,
            Err(MobError::StaleEventCursor {
                after_cursor,
                latest_cursor,
            }) => {
                tracing::warn!(
                    mob_id = %mob_id,
                    after_cursor,
                    latest_cursor,
                    "mob_events subscription: persisted cursor is past ledger frontier; resuming at latest"
                );
                match events.subscribe().await {
                    Ok(sub) => sub,
                    Err(err) => {
                        tracing::warn!(
                            mob_id = %mob_id,
                            error = %err,
                            "mob_events subscription: failed to subscribe at latest after stale-cursor recovery"
                        );
                        return;
                    }
                }
            }
            Err(err) => {
                tracing::warn!(
                    mob_id = %mob_id,
                    error = %err,
                    "mob_events subscription: failed to resume from persisted cursor"
                );
                return;
            }
        },
        None => match events.subscribe().await {
            Ok(sub) => sub,
            Err(err) => {
                tracing::warn!(
                    mob_id = %mob_id,
                    error = %err,
                    "mob_events subscription: initial subscribe failed"
                );
                return;
            }
        },
    };

    while let Some(event) = subscription.event_rx.recv().await {
        let envelope = store.project_mob_event(&event).await;
        // Idle-retire opt-ins follow the members they were set for: a
        // retirement from ANY surface (hand retire, operator retire, reset,
        // destroy) releases the opt-in bound to exactly the retired session.
        if let Some(overrides) = idle_retire_overrides.as_ref() {
            overrides.observe_mob_event(&mob_id, &event).await;
        }
        if let Err(err) = persistent_metadata
            .set_subscription_cursor(&mob_id, envelope.cursor)
            .await
        {
            tracing::warn!(
                mob_id = %mob_id,
                cursor = envelope.cursor,
                error = %err,
                "mob_events subscription: failed to persist cursor; continuing"
            );
        }
    }
}

/// How often the actor-loop probe sends its round trip.
const ACTOR_LOOP_PROBE_INTERVAL: Duration = Duration::from_mins(1);

/// How long one probe round trip may go unanswered before the stall pages.
///
/// Unlike the delivery path's admission budget (`identity_first/bridge.rs`,
/// 600s), which must stay wide because firing it drops a delivery, a probe
/// timeout drops nothing — it only names the stall — so it can afford to be
/// aggressive. The probe's handler is a pure in-memory phase read whose
/// healthy latency is microseconds; 30s therefore cannot fire because the
/// READ is slow, only because the read is queued behind a handler that has
/// not yet RETURNED.
///
/// That is deliberately weaker than "blocked". A handler that has not
/// returned may be wedged, or may be doing legitimate long work — a member
/// revival, a large replay, a compaction, a storage migration. The probe
/// cannot tell those apart, because `QueryPhase` rides the same serialized
/// loop it is watching, so its round trip measures "time to drain everything
/// queued ahead", not health. Note the scale this sits at: the same system
/// treats a 600s in-flight admission as normal (`BRIDGE_ACTOR_ADMISSION_BUDGET`),
/// which is 20x this budget, so a busy loop can cross 30s without anything
/// being wrong. Widening the budget is NOT the fix — it would only move the
/// same ambiguity — and one such long command has already been suppressed by
/// hand (`lifecycle.rs` aborts the probe on shutdown so an intentional
/// shutdown stall cannot page). The real discriminator is whether the loop
/// made PROGRESS while the probe waited, which needs a monotonic
/// command-completion counter on meerkat's mob actor; meta cannot observe
/// it from here. Until that exists, the stall is an OPEN INCIDENT rather
/// than a verdict: it is closed by the correlated
/// [`ErrorEvent::ActorLoopRecovered`], and a receiver decides severity from
/// how long the incident stays open on its own clock.
const ACTOR_LOOP_PROBE_BUDGET: Duration = Duration::from_secs(30);

/// Effective probe interval: `META_ACTOR_LOOP_PROBE_INTERVAL_SECS`
/// overrides the default, clamped to [1, 3600] seconds (the same knob idiom
/// as `META_BRIDGE_ACTOR_ADMISSION_SECS`).
fn actor_loop_probe_interval() -> Duration {
    parse_probe_secs(
        std::env::var("META_ACTOR_LOOP_PROBE_INTERVAL_SECS")
            .ok()
            .as_deref(),
        ACTOR_LOOP_PROBE_INTERVAL,
    )
}

/// Effective probe budget: `META_ACTOR_LOOP_PROBE_BUDGET_SECS` overrides
/// the default, clamped to [1, 3600] seconds.
fn actor_loop_probe_budget() -> Duration {
    parse_probe_secs(
        std::env::var("META_ACTOR_LOOP_PROBE_BUDGET_SECS")
            .ok()
            .as_deref(),
        ACTOR_LOOP_PROBE_BUDGET,
    )
}

fn parse_probe_secs(raw: Option<&str>, default: Duration) -> Duration {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .map(|secs| Duration::from_secs(secs.clamp(1, 3600)))
        .unwrap_or(default)
}

/// Which observations on a probe round trip mean "the actor terminated".
///
/// A fail-stopped actor closes `command_rx`; the parked probe then resolves
/// with `ActorCommandChannelClosed` (or `ActorReplyChannelClosed` when the
/// reply side was dropped). meerkat 0.8.33's `MobHandle::status` additionally
/// launders both variants into `MobError::Internal("<variant text>; last
/// actor-published phase is Running, ...")` whenever the phase watch is not
/// terminal, so the typed variant is matched first and the laundered text
/// second. Anything else (a typed scope denial, a lifecycle refusal) is a
/// LIVE loop that answered, and stays out of this classification.
fn actor_terminated_detail(result: &Result<MobState, MobError>) -> Option<String> {
    let error = match result {
        Ok(_) => return None,
        Err(error) => error,
    };
    match error {
        MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed => {
            Some(error.to_string())
        }
        MobError::Internal(text)
            if text.contains(&MobError::ActorCommandChannelClosed.to_string())
                || text.contains(&MobError::ActorReplyChannelClosed.to_string()) =>
        {
            Some(text.clone())
        }
        _ => None,
    }
}

/// Actor-loop liveness probe.
///
/// meerkat's mob actor is ONE serialized command loop: a handler that blocks
/// freezes every member's dispatch behind it, and the only existing witness
/// is the delivery path's admission budget — which fires only if a delivery
/// happens to be waiting. If nobody waits, nobody learns. This probe is the
/// unconditional waiter: every `interval` it sends the cheapest round trip
/// the handle exposes (`QueryPhase`, an O(1) in-memory phase read) and pages
/// `ErrorEvent::ActorLoopStalled` when the reply does not arrive within
/// `budget`.
///
/// At most ONE probe is ever outstanding: on timeout the task stays parked
/// on the SAME round trip until the loop drains it, rather than stacking
/// further commands onto a stalled actor (uncapped retry loops already
/// amplify channel pressure there; the probe must never join them). When the
/// parked round trip finally resolves, recovery is emitted as
/// [`ErrorEvent::ActorLoopRecovered`], correlated to the stall by `stall_id`.
///
/// That variant amends a previously stated invariant — `ErrorEvent` used to
/// have no recovery precedent, on the reasoning that every variant names a
/// failure and the hook is a paging channel. An open-only paging channel is
/// not decidable: a receiver that pages on a stall can never close the
/// incident it opened, so it can only escalate. The resolution is filtered
/// out of error-level logging by the default sink, and unaware consumers see
/// it fall into the `_` arm `#[non_exhaustive]` already forces.
///
/// THE CORRELATED PAIR PLUS THE RECEIVER'S OWN CLOCK IS THE DISCRIMINATOR.
/// An open `stall_id` with no matching resolution after ten minutes is a
/// wedged loop; one closed after fifty seconds was a busy loop. That is a
/// complete decision procedure using only what this emitter already sends,
/// which is why the missing progress counter is an IMPROVEMENT rather than a
/// missing piece: it would let the probe skip PAGING for the busy case at
/// all, whereas today the receiver pages first and decides after. Better,
/// but strictly an optimization of a decision the receiver can already make.
///
/// Note what the counters can and cannot say. Because the probe parks on the
/// SAME round trip rather than starting a new one, a genuinely wedged loop
/// pages exactly once and never gets another cycle, so no counter can climb;
/// a merely slow loop recovers and stalls again, so it is the slow case that
/// accumulates. `prior_resolved_stalls` is therefore chronic-busyness
/// evidence, and "wedged" is the absence of a resolution, measured by the
/// receiver's own clock — not by any count this emitter could produce.
///
/// The probe treats a completion within budget, `Ok` or a typed error the
/// actor ANSWERED with, as a live loop: it measures whether the loop drains,
/// not whether the mob is healthy. The one exception is a channel-closed
/// error ([`actor_terminated_detail`]): that is not the loop answering, it is
/// the loop being gone. It is emitted as [`ErrorEvent::ActorLoopTerminated`],
/// never as a recovery (a 2026-09-04 production incident read exactly such a resolution as
/// `actor_loop_recovered` while every later send failed instantly), the
/// shared [`ActorLoopHealth`] is marked terminated so deliveries fail fast,
/// and the probe ends: there is no loop left to watch and nothing in this
/// process can restart it. A terminal phase reply
/// (`Stopped`/`Completed`/`Destroyed`) also ends the probe.
///
/// Every verdict is mirrored into `health`, the seam the delivery path reads:
/// stalled on page, live on the correlated resolution, terminated on a closed
/// channel.
///
/// [`ActorLoopHealth`]: crate::actor_loop_health::ActorLoopHealth
async fn run_actor_loop_probe<P, F>(
    mut probe: P,
    error_hook: SharedErrorHook,
    health: Arc<crate::actor_loop_health::ActorLoopHealth>,
    interval: Duration,
    budget: Duration,
) where
    P: FnMut() -> F,
    F: Future<Output = Result<MobState, MobError>>,
{
    // Correlates each stall with the resolution that closes it, and counts
    // the stalls that have already resolved. Task-local: one probe per
    // runtime, so no shared counter is needed.
    let mut next_stall_id: u64 = 1;
    let mut resolved_stalls: u64 = 0;
    loop {
        tokio::time::sleep(interval).await;
        let started = tokio::time::Instant::now();
        let round_trip = probe();
        tokio::pin!(round_trip);
        let result = match tokio::time::timeout(budget, &mut round_trip).await {
            Ok(result) => result,
            Err(_) => {
                let stall_id = next_stall_id;
                next_stall_id += 1;
                health.mark_stalled(stall_id);
                fire_error_hook(
                    &error_hook,
                    ErrorEvent::ActorLoopStalled {
                        // Measured, not the configured budget echoed back: a
                        // field that looks like data must be data. At page
                        // time this necessarily reads ~= the budget (we page
                        // the instant it expires), so it is a truthfulness
                        // fix rather than new information — the informative
                        // elapsed is `ActorLoopRecovered::stalled_for_secs`.
                        probe_waited_secs: started.elapsed().as_secs(),
                        detail: format!(
                            "QueryPhase probe round trip unanswered after {}s; the mob actor \
                             is one serialized command loop, so every member's dispatch is \
                             queued behind whatever is blocking it; the probe stays parked on \
                             this round trip and will not stack another. THIS STALL PAGES \
                             ONCE: no further stall events will be emitted for it, so silence \
                             is NOT recovery — hold the incident open until an \
                             actor_loop_recovered arrives with stall_id {}",
                            budget.as_secs(),
                            stall_id
                        ),
                        stall_id: Some(stall_id),
                        prior_resolved_stalls: Some(resolved_stalls),
                    },
                );
                // Park on the SAME round trip until the loop drains it.
                let result = round_trip.await;
                if let Some(detail) = actor_terminated_detail(&result) {
                    // The parked command did not drain; the actor died under
                    // it. Say so, and never call it a recovery.
                    health.mark_terminated(Some(stall_id), detail.clone());
                    tracing::error!(
                        stall_id,
                        stalled_for_secs = started.elapsed().as_secs(),
                        detail = %detail,
                        "mob actor loop TERMINATED while stall {stall_id} was open: the actor \
                         command channel closed, so the loop did not recover and cannot \
                         recover in this process; every delivery will now fail fast; the \
                         process must restart"
                    );
                    fire_error_hook(
                        &error_hook,
                        ErrorEvent::ActorLoopTerminated {
                            stall_id: Some(stall_id),
                            detail,
                        },
                    );
                    break;
                }
                resolved_stalls += 1;
                health.mark_recovered(stall_id);
                // The receiver opened an incident on the stall above; this is
                // the only thing that lets it close that incident rather than
                // escalate forever.
                fire_error_hook(
                    &error_hook,
                    ErrorEvent::ActorLoopRecovered {
                        stall_id,
                        stalled_for_secs: started.elapsed().as_secs(),
                    },
                );
                result
            }
        };
        if let Some(detail) = actor_terminated_detail(&result) {
            // A closed channel on a fresh round trip: the loop is gone
            // without ever having stalled from this probe's point of view.
            health.mark_terminated(None, detail.clone());
            tracing::error!(
                detail = %detail,
                "mob actor loop TERMINATED: the actor command channel closed; every delivery \
                 will now fail fast; the process must restart"
            );
            fire_error_hook(
                &error_hook,
                ErrorEvent::ActorLoopTerminated {
                    stall_id: None,
                    detail,
                },
            );
            break;
        }
        if matches!(
            result,
            Ok(MobState::Stopped | MobState::Completed | MobState::Destroyed)
        ) {
            break;
        }
    }
}

/// Project a forwarded member event for the drain: the wire envelope plus
/// any drain-side alert extracted while the member event is still typed.
fn forwarded_member_event(attributed: AttributedEvent) -> ForwardedMemberEvent {
    let compaction_rejection = compaction_rejection_alert(&attributed);
    let failed_run_pages = run_failure_pages(&attributed.envelope.payload);
    let envelope = attributed_event_to_unified(attributed);
    let alert = match compaction_rejection {
        Some(alert) => Some(alert),
        None if failed_run_pages => host_loop_crash_alert(&envelope),
        None => None,
    };
    ForwardedMemberEvent { envelope, alert }
}

/// Whether a member event is a run failure that pages as `HostLoopCrash`: a
/// `RunFailed` whose typed failure class is anything but `Cancelled`. A
/// cancelled run ended because it was asked to (an interrupt, a mob stop, a
/// shutdown cancelling in-flight work), not because the member's loop
/// failed; its `run_failed` event still reaches the console and event log.
fn run_failure_pages(payload: &AgentEvent) -> bool {
    matches!(
        payload,
        AgentEvent::RunFailed { error_report, .. }
            if error_report.class != meerkat_core::event::AgentErrorClass::Cancelled
    )
}

/// The `HostLoopCrash` alert for a failed member run, named by the event id
/// and member alias the console and event log key the run by, with the
/// failure the projected payload carries.
fn host_loop_crash_alert(envelope: &EventEnvelope<UnifiedEvent>) -> Option<ErrorEvent> {
    let UnifiedEvent::Agent {
        agent_id, payload, ..
    } = &envelope.event
    else {
        return None;
    };
    Some(ErrorEvent::HostLoopCrash {
        member_id: agent_id.clone(),
        error: lifecycle::run_failed_crash_error(&envelope.event_id, payload.as_ref()),
    })
}

/// Compaction persistence rejections must page rather than pass as ordinary
/// console traffic — meerkat emits `CompactionFailed` on every rejected
/// compaction commit, and fleets otherwise discover wedged members by
/// silence. Extracted here, where the member event and its session source
/// identity are still typed.
fn compaction_rejection_alert(attributed: &AttributedEvent) -> Option<ErrorEvent> {
    let AgentEvent::CompactionFailed { reason } = &attributed.envelope.payload else {
        return None;
    };
    // Live member session streams stamp session source identity; a
    // non-session source has no session to attribute.
    let session_id = attributed
        .envelope
        .source
        .session_id()
        .map(ToString::to_string)
        .unwrap_or_default();
    // The projection-handoff refusal carries the severity facts typed: the
    // preserved-history fit is the wedged-member (page) vs still-progressing
    // (log line) discriminator, and hosts must not fish it out of the message
    // string. Other compaction failure reasons have no fit verdict to carry.
    let (preserved_history, attempted_entries) = match reason {
        meerkat_core::event::CompactionFailureReason::ProjectionHandoffRefused {
            preserved_history,
            attempted_entries,
            ..
        } => (Some((*preserved_history).into()), Some(*attempted_entries)),
        _ => (None, None),
    };
    Some(ErrorEvent::CompactionPersistenceRejected {
        identity: crate::member_comms_id::runtime_event_alias(&attributed.source),
        session_id,
        error: reason.to_string(),
        preserved_history,
        attempted_entries,
    })
}

/// Identifies this process's member event streams. Meerkat numbers a
/// member's stream from the start again whenever the stream is recreated, so
/// sequence numbers only order events within one stream lifetime.
static SOURCE_EVENT_PROCESS_EPOCH: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| uuid::Uuid::new_v4().simple().to_string());

/// The stream lifetime `attributed.envelope.seq` belongs to: this process and
/// the member incarnation (generation and fence) that emitted it. Sequences
/// from different epochs are not comparable; consumers order them by arrival.
fn source_event_epoch(attributed: &AttributedEvent) -> String {
    format!(
        "{}.{}.{}",
        *SOURCE_EVENT_PROCESS_EPOCH,
        attributed.source.generation.get(),
        attributed.source_fence_token.get()
    )
}

fn attributed_event_to_unified(attributed: AttributedEvent) -> EventEnvelope<UnifiedEvent> {
    let mut payload =
        crate::mob_handle_runtime::console_agent_event_payload(&attributed.envelope.payload);
    if let Some(object) = payload.as_object_mut() {
        // The event publisher owns source scope, including replay from an older
        // session. Never substitute the member's current session at read time.
        if let Some(session_id) = attributed.envelope.source.session_id() {
            object.insert("session_id".to_string(), json!(session_id));
            object.insert(
                "source_sequence".to_string(),
                json!(attributed.envelope.seq),
            );
            object.insert(
                "source_epoch".to_string(),
                json!(source_event_epoch(&attributed)),
            );
        }
    }
    EventEnvelope {
        event_id: format!("evt-agent-{}", attributed.envelope.event_id),
        source: "agent".to_string(),
        timestamp_ms: attributed.envelope.timestamp_ms,
        event: UnifiedEvent::Agent {
            // The runtime id's member component is the comms-safe roster
            // encoding (meerkat 0.7 `MemberCommsName`); decode back to the
            // public alias space here so console replay resolution, the
            // `meta/events/subscribe` buffer, and the event log all key
            // events by the same ids that spawn/reserve paths register.
            agent_id: crate::member_comms_id::runtime_event_alias(&attributed.source),
            event_type: agent_event_type(&attributed.envelope.payload).to_string(),
            // Project through the console wire shape (not the raw 0.7 event)
            // so downstream surfaces — console timeline frames, the
            // `meta/events/subscribe` replay buffer, and the event-log
            // query — keep the `result`/`tool_call_id` keys the SDKs parse.
            payload: Some(payload),
        },
    }
}

/// Projects [`crate::memory::events::MemoryTimelineEvent`]s onto the
/// console timeline. Sync fire-and-forget: the async append is spawned on
/// the captured runtime handle, so emitters inside mutexes or blocking
/// threads never wait on the event surface.
struct ConsoleMemoryEventSink {
    store: ConsoleEventStore,
    handle: tokio::runtime::Handle,
}

impl crate::memory::events::MemoryEventSink for ConsoleMemoryEventSink {
    fn emit(&self, event: crate::memory::events::MemoryTimelineEvent) {
        let store = self.store.clone();
        let identity = event.identity().map(str::to_string);
        let event_type = event.event_type().to_string();
        let data = event.data();
        self.handle.spawn(async move {
            match identity {
                // A member's memory event: the store refuses it under a
                // runtime-plane identity.
                Some(identity) => {
                    store.append(identity, None, event_type, data).await;
                }
                None => {
                    store.append_runtime_plane_event(event_type, data).await;
                }
            }
        });
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use meerkat_mob::ids::Generation;

    /// The memory sink keeps `_system` for identity-less memory events and
    /// never writes a member's memory event under a runtime-plane identity.
    #[tokio::test]
    async fn memory_sink_uses_runtime_plane_identity_only_for_identity_less_events() {
        use crate::memory::events::{MemoryEventSink as _, MemoryTimelineEvent};

        let store = ConsoleEventStore::new();
        let sink = ConsoleMemoryEventSink {
            store: store.clone(),
            handle: tokio::runtime::Handle::current(),
        };
        sink.emit(MemoryTimelineEvent::HarvestCompleted {
            realm: "realm".to_string(),
            identity: "__console__".to_string(),
            promoted: 1,
            tombstoned: 0,
        });
        sink.emit(MemoryTimelineEvent::DreamSkipped {
            realm: "realm".to_string(),
            reason: "idle".to_string(),
        });
        // The sink appends on spawned tasks, in emit order on this runtime.
        let replay = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let replay = store.replay_all(None).await.expect("replay");
                if replay
                    .iter()
                    .any(|event| event.event_type == "memory.dream.skipped")
                {
                    return replay;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the identity-less memory event lands");
        assert!(
            replay
                .iter()
                .any(|event| event.event_type == "memory.dream.skipped"
                    && event.identity == "_system"),
            "an identity-less memory event keeps the system identity"
        );
        assert!(
            !replay.iter().any(|event| event.identity == "__console__"),
            "a member memory event never lands under a runtime-plane identity"
        );
    }

    /// Console attribution of a member named with a runtime-plane identity:
    /// its agent events are refused whatever spelling its roster id uses.
    #[tokio::test]
    async fn agent_events_of_runtime_plane_roster_ids_are_refused() {
        let store = ConsoleEventStore::new();
        for (index, member_id) in ["_system", "__console__", "mk--__system", "mk--rt_c__system"]
            .into_iter()
            .enumerate()
        {
            let mut attributed = attributed_text_delta(member_id, 1);
            attributed.envelope.event_id = uuid::Uuid::from_u128(index as u128 + 1);
            store
                .project_unified_event(&attributed_event_to_unified(attributed))
                .await;
        }
        let replay = store.replay_all(None).await.expect("replay");
        assert!(
            replay
                .iter()
                .all(|event| event.event_type == "runtime_bootstrapped"
                    || (event.identity != "_system" && event.identity != "__console__")),
            "no member event lands under a runtime-plane identity: {replay:#?}"
        );
    }

    fn attributed_text_delta(member_id: &str, generation: u64) -> AttributedEvent {
        AttributedEvent {
            source: AgentRuntimeId::new(
                AgentIdentity::from(member_id),
                Generation::new(generation),
            ),
            source_fence_token: FenceToken::new(1),
            role: ProfileName::from("worker"),
            envelope: meerkat_core::event::EventEnvelope {
                event_id: Default::default(),
                source: meerkat_core::event::EventSourceIdentity::runtime("test"),
                seq: 0,
                mob_id: None,
                timestamp_ms: 1,
                payload: AgentEvent::TextDelta {
                    assistant_message_id: None,
                    delta: "hello".to_string(),
                },
            },
        }
    }

    #[test]
    fn attributed_projection_preserves_exact_source_session_and_sequence() {
        let session_id = meerkat_core::SessionId::new();
        let mut attributed = attributed_text_delta("router", 1);
        attributed.envelope.source =
            meerkat_core::event::EventSourceIdentity::session(session_id.clone());
        attributed.envelope.seq = 41;
        let projected = attributed_event_to_unified(attributed);
        let UnifiedEvent::Agent {
            payload: Some(payload),
            ..
        } = projected.event
        else {
            panic!("expected agent payload");
        };
        assert_eq!(payload["session_id"], json!(session_id));
        assert_eq!(payload["source_sequence"], json!(41));
        assert_eq!(payload["delta"], "hello");
        // The sequence is scoped to this process and member incarnation.
        let epoch = payload["source_epoch"].as_str().expect("source epoch");
        assert!(epoch.ends_with(".1.1"), "{epoch}");
        let mut respawned = attributed_text_delta("router", 2);
        respawned.envelope.source = meerkat_core::event::EventSourceIdentity::session(session_id);
        let UnifiedEvent::Agent {
            payload: Some(respawned),
            ..
        } = attributed_event_to_unified(respawned).event
        else {
            panic!("expected respawned agent payload");
        };
        assert_ne!(respawned["source_epoch"], payload["source_epoch"]);
        let legacy = attributed_event_to_unified(attributed_text_delta("router", 1));
        let UnifiedEvent::Agent {
            payload: Some(payload),
            ..
        } = legacy.event
        else {
            panic!("expected legacy agent payload");
        };
        assert!(payload.get("session_id").is_none());
        assert!(payload.get("source_sequence").is_none());
        assert!(payload.get("source_epoch").is_none());
    }

    #[test]
    fn identity_stream_tracking_uses_trusted_durable_identity_label() {
        let labels =
            BTreeMap::from([("agent_identity".to_string(), "review:singleton".to_string())]);
        assert_eq!(
            durable_identity_label(&labels).as_deref(),
            Some("review:singleton")
        );
        assert_eq!(
            durable_identity_label(&BTreeMap::new()),
            None,
            "ordinary mobs must not be guessed into identity authority"
        );
    }

    /// Regression: identity-first members spawn under comms-safe encoded
    /// roster ids (`mk--…`); the agent-event ingest must decode the member
    /// component back to the public alias space before console/SDK
    /// projection, or events project under junk identities and reserved
    /// interactions never complete.
    #[test]
    fn attributed_event_ingest_decodes_encoded_roster_member_ids() {
        let encoded = crate::member_comms_id::mob_member_id_str("rt:review:singleton:0");
        assert!(encoded.starts_with("mk--"), "precondition: alias encodes");
        let unified = attributed_event_to_unified(attributed_text_delta(&encoded, 1));
        let UnifiedEvent::Agent { agent_id, .. } = unified.event else {
            panic!("expected agent event");
        };
        assert_eq!(agent_id, "rt:review:singleton:0:1");
    }

    #[test]
    fn attributed_event_ingest_passes_plain_member_ids_through() {
        let unified = attributed_event_to_unified(attributed_text_delta("worker-one", 0));
        let UnifiedEvent::Agent { agent_id, .. } = unified.event else {
            panic!("expected agent event");
        };
        assert_eq!(agent_id, "worker-one:0");
    }

    /// The console drops an agent event by its Core event id, never by its
    /// content. Two Core events with the same member, payload, sequence and
    /// timestamp still carry distinct UUIDv7 ids, so both are kept; only a
    /// redelivered copy of one of them (the same id) is dropped.
    #[tokio::test]
    async fn console_projection_keeps_identical_events_and_drops_a_redelivered_copy() {
        let core_event = || {
            let mut envelope = meerkat_core::event::EventEnvelope::new_with_source(
                meerkat_core::event::EventSourceIdentity::runtime("test"),
                7,
                None,
                AgentEvent::TextDelta {
                    assistant_message_id: None,
                    delta: "same words".to_string(),
                },
            );
            envelope.timestamp_ms = 1;
            let mut attributed = attributed_text_delta("worker-one", 0);
            attributed.envelope = envelope;
            attributed
        };
        let first = core_event();
        let second = core_event();
        assert_ne!(
            first.envelope.event_id, second.envelope.event_id,
            "Core mints a fresh id for every event"
        );
        assert_eq!(
            serde_json::to_value(&first.envelope.payload).expect("payload serializes"),
            serde_json::to_value(&second.envelope.payload).expect("payload serializes")
        );
        assert_eq!(first.envelope.seq, second.envelope.seq);

        let store = console_events::ConsoleEventStore::new();
        store
            .register_runtime_identity("worker-one:0", "worker-one")
            .await;
        let first_id = format!("evt-agent-{}", first.envelope.event_id);
        let second_id = format!("evt-agent-{}", second.envelope.event_id);
        store
            .project_unified_event(&attributed_event_to_unified(first.clone()))
            .await;
        store
            .project_unified_event(&attributed_event_to_unified(second))
            .await;
        // The same Core event delivered again.
        store
            .project_unified_event(&attributed_event_to_unified(first))
            .await;

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        // Registration records its own console event; keep the agent events.
        let agent_events = replay
            .iter()
            .filter(|event| event.event_id.starts_with("evt-agent-"))
            .collect::<Vec<_>>();
        let ids = agent_events
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![first_id.as_str(), second_id.as_str()]);
        assert_eq!(agent_events[0].data, agent_events[1].data);
    }

    /// Regression: the callers' outer `select!` drops and recreates the
    /// `wait()` future on every forwarded member event, so a safety deadline
    /// computed inside `wait` would reset under sustained event traffic and
    /// the safety reconcile would never fire. The deadline must persist in
    /// the cadence and eventually complete a recreated `wait`.
    #[tokio::test(start_paused = true)]
    async fn reconcile_cadence_safety_deadline_survives_recreated_waits() {
        let mut cadence = ReconcileCadence::new(&None);
        let mut fired = false;
        // Seven 5s rounds = 35s of simulated event churn; the 30s deadline
        // anchored at construction must fire within them.
        for _ in 0..7 {
            tokio::select! {
                () = cadence.wait(None) => {
                    fired = true;
                    break;
                }
                () = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
        assert!(
            fired,
            "safety reconcile starved: recreated wait futures reset the deadline"
        );
    }

    /// The safety bound is "at most 30s since the last reconcile pass", not
    /// "since construction": `rebind` (which ends every reconcile pass) must
    /// re-arm the deadline.
    #[tokio::test(start_paused = true)]
    async fn reconcile_cadence_rebind_rearms_safety_deadline() {
        let mut cadence = ReconcileCadence::new(&None);
        tokio::time::sleep(Duration::from_secs(20)).await;
        cadence.rebind(&[]);
        tokio::select! {
            () = cadence.wait(None) => {
                panic!("deadline fired 30s after construction despite rebind re-arm")
            }
            () = tokio::time::sleep(Duration::from_secs(29)) => {}
        }
        tokio::select! {
            () = cadence.wait(None) => {}
            () = tokio::time::sleep(Duration::from_secs(2)) => {
                panic!("re-armed deadline did not fire 30s after rebind")
            }
        }
    }

    /// Regression: the console forwarder must only hold a live subscription
    /// for Active members. A Retiring member can keep stale binding atoms
    /// while its session injector is gone, so subscribing it fails every
    /// 250ms reconcile tick — the 4×/s "failed to subscribe" hot-loop
    /// (observed ~49k warnings over 3.4h on one wedged-retiring alias).
    #[test]
    fn forwarder_only_subscribes_active_members() {
        assert!(forwarder_should_subscribe(MobMemberStatus::Active));
        assert!(!forwarder_should_subscribe(MobMemberStatus::Retiring));
        assert!(!forwarder_should_subscribe(MobMemberStatus::Broken));
        assert!(!forwarder_should_subscribe(MobMemberStatus::Completed));
        assert!(!forwarder_should_subscribe(MobMemberStatus::Unknown));
    }

    /// The backoff for a persistently-failing Active subscribe must grow from
    /// one reconcile tick and cap, so even a genuinely stuck member retries at
    /// most ~once per cap instead of 4×/s.
    #[test]
    fn subscribe_backoff_grows_and_caps() {
        const { assert!(PERMANENT_STREAM_FAILURE_THRESHOLD > 1) };
        assert_eq!(subscribe_backoff_delay(0), SUBSCRIBE_BACKOFF_BASE);
        assert_eq!(subscribe_backoff_delay(1), SUBSCRIBE_BACKOFF_BASE * 2);
        assert_eq!(subscribe_backoff_delay(3), SUBSCRIBE_BACKOFF_BASE * 8);
        assert_eq!(subscribe_backoff_delay(7), SUBSCRIBE_BACKOFF_MAX);
        // Saturates at the cap for arbitrarily many failures (no shift overflow).
        assert_eq!(subscribe_backoff_delay(50), SUBSCRIBE_BACKOFF_MAX);
        assert!(subscribe_backoff_delay(2) > subscribe_backoff_delay(1));
    }

    async fn bootstrap_minimal_runtime(temp_dir: &tempfile::TempDir) -> UnifiedRuntime {
        let session_path = temp_dir.path().join("sessions");
        std::fs::create_dir_all(&session_path).expect("session path");
        let factory = meerkat::AgentFactory::new(&session_path);
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = Arc::new(
            meerkat::build_ephemeral_service(factory, meerkat::Config::default(), 16),
        );

        // A mob id per call: it names process-wide comms participants, and
        // the suite runs the tests that bootstrap this runtime concurrently.
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"alert-mob-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            uuid::Uuid::new_v4().simple()
        ))
        .expect("parse mob definition");
        let mob_spec = MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            session_service,
        )
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::for_provider(
                meerkat_core::Provider::OpenAI,
            ))),
        });
        let module_config = MetaConfig {
            modules: vec![],
            discovery: crate::types::DiscoverySpec {
                namespace: "compaction-alert".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        };
        UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
            .await
            .expect("bootstrap unified runtime")
    }

    /// Push member `CompactionFailed` events carrying `reasons` through the
    /// real drain path and return the alerts the error hook received.
    ///
    /// Every reason rides ONE bootstrapped runtime. The mapping is only worth
    /// anything if the fields survive the forwarder, the drain, and the
    /// detached hook fire, so this goes the whole way to the hook — but the
    /// lib suite runs ~1500 tests in one process and `bootstrap`'s 2s budget
    /// is a wall-clock allowance, so a second concurrent bootstrap is load
    /// the suite should not have to carry.
    async fn compaction_alerts_through_drain(
        session_id: &meerkat_core::types::SessionId,
        reasons: Vec<meerkat_core::event::CompactionFailureReason>,
    ) -> Vec<ErrorEvent> {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut runtime = bootstrap_minimal_runtime(&temp_dir).await;

        let captured: Arc<tokio::sync::Mutex<Vec<ErrorEvent>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let hook_captured = captured.clone();
        let hook: ErrorHook = Arc::new(move |event| {
            let hook_captured = hook_captured.clone();
            Box::pin(async move {
                hook_captured.lock().await.push(event);
            })
        });
        runtime.set_error_hook(hook);

        let event_tx = runtime.install_test_event_ingress().await;
        let expected = reasons.len();
        for (seq, reason) in reasons.into_iter().enumerate() {
            let attributed = AttributedEvent {
                source: AgentRuntimeId::new(
                    AgentIdentity::from("compaction-worker"),
                    Generation::new(0),
                ),
                source_fence_token: FenceToken::new(1),
                role: ProfileName::from("worker"),
                envelope: meerkat_core::event::EventEnvelope {
                    event_id: Default::default(),
                    source: meerkat_core::event::EventSourceIdentity::session(session_id.clone()),
                    seq: seq as u64,
                    mob_id: None,
                    timestamp_ms: 9,
                    payload: AgentEvent::CompactionFailed { reason },
                },
            };
            event_tx
                .send(forwarded_member_event(attributed))
                .await
                .expect("send forwarded event");
        }
        runtime
            .drain_mob_agent_events()
            .await
            .expect("drain member events");

        // `fire_error` spawns a detached task; wait bounded for every hook.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if captured.lock().await.len() >= expected {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "error hook did not receive all {expected} compaction persistence rejections"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let alerts = captured.lock().await.clone();
        runtime.shutdown().await;
        alerts
    }

    /// Compaction persistence rejections must page, not pass as ordinary
    /// console traffic: member `CompactionFailed` agent events pushed through
    /// the drain path fire the error hook with the typed
    /// `CompactionPersistenceRejected` alert carrying the member identity,
    /// the emitting session, and the rejection detail.
    ///
    /// meerkat's typed `ProjectionHandoffRefused` reason additionally lands
    /// its severity facts on the hook typed: the preserved-history fit
    /// discriminator (page vs log line) and the attempted entry count arrive
    /// as fields, not as message substrings, while `error` keeps the full
    /// human rendering for catch-all adopters. `StillFits` is the
    /// discriminating value — an "always page" default would say
    /// `OverWindow`. Every other failure reason carries no verdict at all.
    #[tokio::test]
    async fn compaction_failures_page_error_hook_with_typed_fit_through_drain() {
        let session_id = meerkat_core::types::SessionId::new();
        let alerts = compaction_alerts_through_drain(
            &session_id,
            vec![
                meerkat_core::event::CompactionFailureReason::TranscriptRewriteFailed {
                    message: "runtime epoch mismatch".to_string(),
                },
                meerkat_core::event::CompactionFailureReason::ProjectionHandoffRefused {
                    refusal: meerkat_core::memory::CompactionHandoffRefusal::RuntimeEpochRotated,
                    preserved_history:
                        meerkat_core::event::CompactionPreservedHistoryFit::StillFits,
                    attempted_entries: 12,
                    message: "runtime epoch rotated under the coordinator".to_string(),
                },
            ],
        )
        .await;

        // The hook fires from detached tasks, so arrival order is not the
        // send order: pick each alert out by the fact under test.
        let mut untyped = None;
        let mut typed = None;
        for alert in &alerts {
            match alert {
                ErrorEvent::CompactionPersistenceRejected {
                    identity,
                    session_id: rejected_session,
                    error,
                    preserved_history,
                    attempted_entries,
                } => {
                    assert_eq!(identity, "compaction-worker:0");
                    assert_eq!(rejected_session, &session_id.to_string());
                    if preserved_history.is_some() {
                        typed = Some((error.clone(), *preserved_history, *attempted_entries));
                    } else {
                        untyped = Some((error.clone(), *attempted_entries));
                    }
                }
                other => panic!("expected CompactionPersistenceRejected, got {other:?}"),
            }
        }

        let (untyped_error, untyped_entries) =
            untyped.unwrap_or_else(|| panic!("the non-handoff failure must page too: {alerts:?}"));
        assert!(
            untyped_error.contains("runtime epoch mismatch"),
            "error must carry the rejection detail: {untyped_error}"
        );
        assert_eq!(
            untyped_entries, None,
            "a non-handoff compaction failure carries no fit verdict"
        );

        let (typed_error, typed_fit, typed_entries) = typed.unwrap_or_else(|| {
            panic!("the projection-handoff refusal must page with its fit: {alerts:?}")
        });
        assert_eq!(
            typed_fit,
            Some(CompactionPreservedHistoryFit::StillFits),
            "the wedged/progressing discriminator must cross typed"
        );
        assert_eq!(typed_entries, Some(12));
        assert!(
            typed_error.contains("runtime epoch rotated under the coordinator"),
            "error must keep the human rendering: {typed_error}"
        );
    }

    /// Push one member `RunFailed` event per `(class, message)` through the
    /// real drain path and return the alerts the error hook received by the
    /// time the last event's alert arrived. `#[tokio::test]` runs one thread,
    /// so the detached hook tasks run in the order the drain fired them: once
    /// the last event's alert is in, every earlier one is in too.
    async fn run_failure_alerts_through_drain(
        failures: Vec<(meerkat_core::event::AgentErrorClass, &str)>,
    ) -> Vec<ErrorEvent> {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut runtime = bootstrap_minimal_runtime(&temp_dir).await;

        let captured: Arc<tokio::sync::Mutex<Vec<ErrorEvent>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let hook_captured = captured.clone();
        let hook: ErrorHook = Arc::new(move |event| {
            let hook_captured = hook_captured.clone();
            Box::pin(async move {
                hook_captured.lock().await.push(event);
            })
        });
        runtime.set_error_hook(hook);

        let event_tx = runtime.install_test_event_ingress().await;
        let session_id = meerkat_core::types::SessionId::new();
        let last_message = failures
            .last()
            .map(|(_, message)| (*message).to_string())
            .expect("at least one failed run");
        for (seq, (class, message)) in failures.into_iter().enumerate() {
            let attributed = AttributedEvent {
                source: AgentRuntimeId::new(AgentIdentity::from("run-worker"), Generation::new(0)),
                source_fence_token: FenceToken::new(1),
                role: ProfileName::from("worker"),
                envelope: meerkat_core::event::EventEnvelope {
                    event_id: Default::default(),
                    source: meerkat_core::event::EventSourceIdentity::session(session_id.clone()),
                    seq: seq as u64,
                    mob_id: None,
                    timestamp_ms: 9,
                    payload: AgentEvent::RunFailed {
                        session_id: session_id.clone(),
                        identity: Default::default(),
                        error_report: meerkat_core::event::AgentErrorReport {
                            class,
                            reason: None,
                            message: message.to_string(),
                        },
                        terminal_cause_kind: None,
                    },
                },
            };
            event_tx
                .send(forwarded_member_event(attributed))
                .await
                .expect("send forwarded event");
        }
        runtime
            .drain_mob_agent_events()
            .await
            .expect("drain member events");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let last_paged = captured.lock().await.iter().any(|alert| {
                matches!(
                    alert,
                    ErrorEvent::HostLoopCrash { error, .. } if error.contains(&last_message)
                )
            });
            if last_paged {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the last failed run did not page: {:?}",
                captured.lock().await
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let alerts = captured.lock().await.clone();
        runtime.shutdown().await;
        alerts
    }

    /// A cancelled member run is not a host loop crash: an interrupt, a stop
    /// or a shutdown cancels in-flight work on purpose. Only a run that
    /// failed pages `HostLoopCrash`, decided on the typed failure class.
    #[tokio::test]
    async fn a_cancelled_member_run_does_not_page_as_a_host_loop_crash() {
        use meerkat_core::event::AgentErrorClass;

        let alerts = run_failure_alerts_through_drain(vec![
            (AgentErrorClass::Cancelled, "Run was cancelled"),
            (
                AgentErrorClass::Llm,
                "LLM error: authentication failed (401)",
            ),
        ])
        .await;

        let crashes: Vec<(&str, &str)> = alerts
            .iter()
            .filter_map(|alert| match alert {
                ErrorEvent::HostLoopCrash { member_id, error } => {
                    Some((member_id.as_str(), error.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(crashes.len(), 1, "only the failed run pages: {alerts:?}");
        let (member_id, error) = crashes[0];
        assert_eq!(member_id, "run-worker:0");
        assert!(
            error.starts_with("agent run failed (event_id: evt-agent-")
                && error.ends_with("class=llm: LLM error: authentication failed (401)"),
            "the alert keeps its detail: {error}"
        );
    }

    /// Captures formatted tracing output so the default-sink tests can assert
    /// on the emitted records. `with_default` is thread-local, so everything
    /// asserted here must be logged synchronously on the calling thread.
    #[derive(Clone, Default)]
    struct CaptureWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl CaptureWriter {
        fn contents(&self) -> String {
            String::from_utf8_lossy(
                &self
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
            .into_owned()
        }
    }

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    pub(super) fn capture_tracing<T>(body: impl FnOnce() -> T) -> (T, String) {
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            // INFO rather than WARN because the default sink deliberately
            // drops the resolution variant to INFO, and a test that capped at
            // WARN could not tell "logged at INFO" from "not logged at all".
            .with_max_level(tracing::Level::INFO)
            // Without this the formatter wraps every field name and `=` in
            // ANSI escapes, so `contains("hook_registered=false")` reads a
            // string that is never literally present.
            .with_ansi(false)
            .finish();
        // A second registered Dispatch keeps callsite interest from being
        // cached `never` by threads without a subscriber.
        let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        let value = tracing::subscriber::with_default(subscriber, body);
        (value, writer.contents())
    }

    fn sample_alert() -> ErrorEvent {
        ErrorEvent::CompactionPersistenceRejected {
            identity: "compaction-worker:0".to_string(),
            session_id: "sess-1".to_string(),
            error: "runtime refused the durable compaction projection handoff".to_string(),
            preserved_history: Some(CompactionPreservedHistoryFit::OverWindow),
            attempted_entries: Some(12),
        }
    }

    /// An `ErrorEvent` fired with NO hook registered must still reach the log,
    /// with its typed variant and fields — a paging channel that discards when
    /// nobody wired it is indistinguishable from a healthy fleet. Fired
    /// through `fire_error_hook` (not the log helper directly) so the test
    /// covers the branch that used to drop the event on the floor.
    #[test]
    fn error_event_without_hook_still_reaches_the_log() {
        let slot: SharedErrorHook = Arc::new(std::sync::RwLock::new(None));
        // No hook means no `tokio::spawn`, so this needs no runtime — which is
        // also what keeps the record on this thread where the capture sees it.
        let ((), logged) = capture_tracing(|| fire_error_hook(&slot, sample_alert()));

        assert!(
            logged.contains("CompactionPersistenceRejected"),
            "the typed variant must be named in the record: {logged}"
        );
        assert!(
            logged.contains("OverWindow") && logged.contains("compaction-worker:0"),
            "typed fields must ride the record, not just the Display string: {logged}"
        );
        assert!(
            logged.contains("hook_registered=false"),
            "the record must say nobody was listening: {logged}"
        );
    }

    /// A wired host loses nothing: the record is still emitted, marked as
    /// delivered. Split from the delivery assertion below so that a single
    /// mutation of either behaviour fails its own test.
    #[test]
    fn error_event_with_hook_is_logged_as_delivered() {
        let hook: ErrorHook = Arc::new(move |_event| Box::pin(async move {}));
        let slot: SharedErrorHook = Arc::new(std::sync::RwLock::new(Some(hook)));

        // The dispatch spawns, so this needs a runtime; the log record itself
        // is still written synchronously on this thread.
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let _guard = runtime.enter();
        let ((), logged) = capture_tracing(|| fire_error_hook(&slot, sample_alert()));

        assert!(
            logged.contains("hook_registered=true"),
            "a wired host still gets the record, marked as delivered: {logged}"
        );
    }

    /// The default sink is an addition to the hook, not a second delivery
    /// path through it: a registered hook must fire EXACTLY once per event.
    #[tokio::test]
    async fn registered_error_hook_fires_exactly_once() {
        let captured: Arc<tokio::sync::Mutex<Vec<ErrorEvent>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let hook_captured = captured.clone();
        let hook: ErrorHook = Arc::new(move |event| {
            let hook_captured = hook_captured.clone();
            Box::pin(async move {
                hook_captured.lock().await.push(event);
            })
        });
        let slot: SharedErrorHook = Arc::new(std::sync::RwLock::new(Some(hook)));

        fire_error_hook(&slot, sample_alert());

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if !captured.lock().await.is_empty() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "registered hook never received the event"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Settle any second delivery a double-dispatch bug would produce.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            captured.lock().await.len(),
            1,
            "the hook must fire exactly once per event"
        );
    }

    /// The unwired notice must name the condition AND the fix. This is the
    /// same function `UnifiedRuntimeBuilder::build` calls when a host never
    /// registered a hook, so the text is pinned in one place.
    #[test]
    fn error_hook_absent_notice_points_at_the_registration_call() {
        let ((), logged) = capture_tracing(emit_error_hook_absent_notice);

        assert!(
            logged.contains("no error hook is registered"),
            "the notice must state the condition plainly: {logged}"
        );
        assert!(
            logged.contains("on_error"),
            "the notice must point at the registration call: {logged}"
        );
    }

    /// The additive fields must not move the wire shape under an adopter
    /// that predates them: a rejection with no fit verdict serializes
    /// without the new keys, and a payload written before the fields
    /// existed still deserializes.
    #[test]
    fn compaction_rejection_wire_shape_stays_additive() {
        let legacy_json = serde_json::json!({
            "category": "compaction_persistence_rejected",
            "identity": "compaction-worker:0",
            "session_id": "sess-1",
            "error": "compaction curator failed: no summary",
        });

        let alert: ErrorEvent =
            serde_json::from_value(legacy_json.clone()).expect("pre-field payload must load");
        assert_eq!(
            alert,
            ErrorEvent::CompactionPersistenceRejected {
                identity: "compaction-worker:0".to_string(),
                session_id: "sess-1".to_string(),
                error: "compaction curator failed: no summary".to_string(),
                preserved_history: None,
                attempted_entries: None,
            }
        );
        assert_eq!(
            serde_json::to_value(&alert).expect("serialize"),
            legacy_json,
            "an alert with no fit verdict must not emit the new keys"
        );

        let typed = ErrorEvent::CompactionPersistenceRejected {
            identity: "compaction-worker:0".to_string(),
            session_id: "sess-1".to_string(),
            error: "runtime refused the durable compaction projection handoff".to_string(),
            preserved_history: Some(CompactionPreservedHistoryFit::OverWindow),
            attempted_entries: Some(12),
        };
        let encoded = serde_json::to_value(&typed).expect("serialize typed");
        assert_eq!(
            encoded["preserved_history"],
            serde_json::json!("over_window")
        );
        assert_eq!(encoded["attempted_entries"], serde_json::json!(12));
        assert_eq!(
            serde_json::from_value::<ErrorEvent>(encoded).expect("round trip"),
            typed
        );
    }

    fn capturing_error_hook_slot() -> (SharedErrorHook, Arc<tokio::sync::Mutex<Vec<ErrorEvent>>>) {
        let captured: Arc<tokio::sync::Mutex<Vec<ErrorEvent>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let hook_captured = captured.clone();
        let hook: ErrorHook = Arc::new(move |event| {
            let hook_captured = hook_captured.clone();
            Box::pin(async move {
                hook_captured.lock().await.push(event);
            })
        });
        let slot: SharedErrorHook = Arc::new(std::sync::RwLock::new(Some(hook)));
        (slot, captured)
    }

    /// A probe round trip that goes unanswered past its budget must page
    /// `ActorLoopStalled` through the error hook with the waited budget.
    #[tokio::test(start_paused = true)]
    async fn stalled_actor_loop_pages_error_hook() {
        let (slot, captured) = capturing_error_hook_slot();
        let probe_task = tokio::spawn(run_actor_loop_probe(
            std::future::pending::<Result<MobState, MobError>>,
            slot,
            crate::actor_loop_health::ActorLoopHealth::shared(),
            Duration::from_mins(1),
            Duration::from_secs(30),
        ));

        // One interval (60s) + one budget (30s), plus slack for the
        // detached hook task.
        tokio::time::sleep(Duration::from_secs(95)).await;
        tokio::task::yield_now().await;

        let events = captured.lock().await;
        assert_eq!(events.len(), 1, "exactly one stall page: {events:?}");
        match &events[0] {
            ErrorEvent::ActorLoopStalled {
                probe_waited_secs,
                detail,
                stall_id,
                prior_resolved_stalls,
            } => {
                assert_eq!(*probe_waited_secs, 30);
                assert!(
                    detail.contains("QueryPhase"),
                    "detail must name the probe round trip: {detail}"
                );
                // A human reading this page must not conclude "it stopped
                // complaining, so it recovered" — the probe pages once per
                // stall by design, so only the correlated resolution can
                // close it.
                assert!(
                    detail.contains("PAGES ONCE") && detail.contains("silence is NOT recovery"),
                    "detail must say the absence of further pages is not recovery: {detail}"
                );
                assert!(
                    detail.contains("actor_loop_recovered") && detail.contains("stall_id 1"),
                    "detail must name the resolution to wait for, by id: {detail}"
                );
                assert_eq!(
                    *stall_id,
                    Some(1),
                    "the stall must carry the id its resolution will echo"
                );
                assert_eq!(*prior_resolved_stalls, Some(0));
            }
            other => panic!("expected ActorLoopStalled, got {other:?}"),
        }
        drop(events);
        probe_task.abort();
    }

    /// The wedged case, and the reason no counter can name it: the probe
    /// parks on the SAME round trip, so a loop that never answers pages
    /// exactly once and then goes silent — `prior_resolved_stalls` cannot
    /// climb, and "wedged" is the ABSENCE of a resolution on the receiver's
    /// clock. This pins the direction so a future change cannot quietly turn
    /// the count into a wedged proxy.
    #[tokio::test(start_paused = true)]
    async fn wedged_actor_loop_pages_once_and_never_resolves() {
        let (slot, captured) = capturing_error_hook_slot();
        let probe_task = tokio::spawn(run_actor_loop_probe(
            std::future::pending::<Result<MobState, MobError>>,
            slot,
            crate::actor_loop_health::ActorLoopHealth::shared(),
            Duration::from_mins(1),
            Duration::from_secs(30),
        ));

        // Long enough for several more probe cycles had any been possible.
        tokio::time::sleep(Duration::from_mins(10)).await;
        tokio::task::yield_now().await;

        let events = captured.lock().await;
        assert_eq!(
            events.len(),
            1,
            "a wedged loop pages once and cannot page again: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ErrorEvent::ActorLoopRecovered { .. })),
            "a wedged loop must never emit a resolution: {events:?}"
        );
        drop(events);
        probe_task.abort();
    }

    /// A stall that later drains must emit a resolution the receiver can
    /// PAIR with the incident it opened. The correlation is the point: an
    /// unpaired "something recovered" cannot close a specific incident, so
    /// the assertion is on matching ids, not merely on both events firing.
    #[tokio::test(start_paused = true)]
    async fn resolved_stall_emits_recovery_correlated_by_stall_id() {
        let (slot, captured) = capturing_error_hook_slot();
        // Answers after 50s: past the 30s budget, so it stalls, then drains.
        let probe_task = tokio::spawn(run_actor_loop_probe(
            || async {
                tokio::time::sleep(Duration::from_secs(50)).await;
                Ok(MobState::Running)
            },
            slot,
            crate::actor_loop_health::ActorLoopHealth::shared(),
            Duration::from_mins(1),
            Duration::from_secs(30),
        ));

        // One interval (60s) + the 50s answer, plus slack for the hook task.
        tokio::time::sleep(Duration::from_mins(2)).await;
        tokio::task::yield_now().await;

        let events = captured.lock().await;
        let stall_id = events
            .iter()
            .find_map(|event| match event {
                ErrorEvent::ActorLoopStalled { stall_id, .. } => Some(*stall_id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected a stall page: {events:?}"));
        let (recovered_id, stalled_for_secs) = events
            .iter()
            .find_map(|event| match event {
                ErrorEvent::ActorLoopRecovered {
                    stall_id,
                    stalled_for_secs,
                } => Some((*stall_id, *stalled_for_secs)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected a resolution: {events:?}"));

        assert_eq!(
            Some(recovered_id),
            stall_id,
            "the resolution must name the stall it closes, or a receiver \
             cannot close the incident it opened: {events:?}"
        );
        assert!(
            stalled_for_secs >= 50,
            "the resolution must carry how long the loop was stalled, got \
             {stalled_for_secs}s"
        );
        drop(events);
        probe_task.abort();
    }

    /// The resolution is the one variant that reports a failure ending, so
    /// the default sink must not log it at ERROR — a recovery rendered as an
    /// error is a lie about severity and doubles the scary lines per stall.
    #[test]
    fn resolved_stall_is_not_logged_as_an_error() {
        let recovered = ErrorEvent::ActorLoopRecovered {
            stall_id: 7,
            stalled_for_secs: 42,
        };
        let ((), logged) = capture_tracing(|| log_error_event(&recovered, true));

        assert!(
            logged.contains("INFO"),
            "a resolution must log at INFO: {logged}"
        );
        assert!(
            !logged.contains("ERROR"),
            "a resolution must not log at ERROR: {logged}"
        );
        assert!(
            logged.contains("actor_loop_recovered") && logged.contains("42"),
            "the record must still carry the resolution facts: {logged}"
        );
    }

    /// Additivity: an `ActorLoopStalled` payload written before the
    /// correlation fields existed must still load, and a stall carrying them
    /// must round-trip. The operator deployment pages on this enum today.
    #[test]
    fn actor_loop_stall_wire_shape_stays_additive() {
        let legacy_json = serde_json::json!({
            "category": "actor_loop_stalled",
            "probe_waited_secs": 30,
            "detail": "QueryPhase probe round trip unanswered",
        });

        let alert: ErrorEvent =
            serde_json::from_value(legacy_json.clone()).expect("pre-field payload must load");
        assert_eq!(
            alert,
            ErrorEvent::ActorLoopStalled {
                probe_waited_secs: 30,
                detail: "QueryPhase probe round trip unanswered".to_string(),
                stall_id: None,
                prior_resolved_stalls: None,
            }
        );
        assert_eq!(
            serde_json::to_value(&alert).expect("serialize"),
            legacy_json,
            "a stall with no correlation must not emit the new keys"
        );

        let correlated = ErrorEvent::ActorLoopStalled {
            probe_waited_secs: 30,
            detail: "stalled".to_string(),
            stall_id: Some(3),
            prior_resolved_stalls: Some(2),
        };
        let encoded = serde_json::to_value(&correlated).expect("serialize correlated");
        assert_eq!(encoded["stall_id"], serde_json::json!(3));
        assert_eq!(
            serde_json::from_value::<ErrorEvent>(encoded).expect("round trip"),
            correlated
        );
    }

    /// A responsive command loop must never page across many probe cycles.
    #[tokio::test(start_paused = true)]
    async fn healthy_actor_loop_never_pages() {
        let (slot, captured) = capturing_error_hook_slot();
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = probes.clone();
        let probe_task = tokio::spawn(run_actor_loop_probe(
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(MobState::Running))
            },
            slot,
            crate::actor_loop_health::ActorLoopHealth::shared(),
            Duration::from_mins(1),
            Duration::from_secs(30),
        ));

        tokio::time::sleep(Duration::from_secs(60 * 5 + 5)).await;
        tokio::task::yield_now().await;

        assert!(
            probes.load(Ordering::SeqCst) >= 4,
            "probe must keep its cadence on a healthy loop: {}",
            probes.load(Ordering::SeqCst)
        );
        assert!(
            captured.lock().await.is_empty(),
            "a healthy loop must not page"
        );
        probe_task.abort();
    }

    /// A stalled probe must never stack another round trip onto the blocked
    /// actor: one page, one parked command, no matter how many cycles pass.
    /// When the parked round trip finally drains, the cadence resumes and
    /// recovery does not page.
    #[tokio::test(start_paused = true)]
    async fn stalled_probe_never_stacks_and_resumes_after_recovery() {
        let (slot, captured) = capturing_error_hook_slot();
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate = Arc::new(tokio::sync::Notify::new());
        let counted = probes.clone();
        let released_probe = released.clone();
        let gate_probe = gate.clone();
        let probe_task = tokio::spawn(run_actor_loop_probe(
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
                let released = released_probe.clone();
                let gate = gate_probe.clone();
                async move {
                    if !released.load(Ordering::SeqCst) {
                        gate.notified().await;
                    }
                    Ok(MobState::Running)
                }
            },
            slot,
            crate::actor_loop_health::ActorLoopHealth::shared(),
            Duration::from_mins(1),
            Duration::from_secs(30),
        ));

        // Many full interval+budget cycles while the first round trip is
        // parked: no second probe, no second page.
        tokio::time::sleep(Duration::from_mins(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "a stalled probe must not stack another round trip"
        );
        assert_eq!(
            captured.lock().await.len(),
            1,
            "a persisting stall pages exactly once"
        );

        // Drain the parked round trip; the probe resumes its cadence.
        released.store(true, Ordering::SeqCst);
        gate.notify_one();
        tokio::time::sleep(Duration::from_secs(61)).await;
        tokio::task::yield_now().await;
        assert!(
            probes.load(Ordering::SeqCst) >= 2,
            "probe must resume after the stalled round trip drains: {}",
            probes.load(Ordering::SeqCst)
        );
        // Recovery used to be an info log only. It is now an event, because a
        // receiver that can open an incident but never close it can only
        // escalate — so the drained round trip must produce exactly one
        // resolution, correlated to the stall it closes.
        let events = captured.lock().await;
        assert_eq!(
            events.len(),
            2,
            "the drained stall must produce its resolution: {events:?}"
        );
        let stall_id = match &events[0] {
            ErrorEvent::ActorLoopStalled { stall_id, .. } => *stall_id,
            other => panic!("expected the stall first, got {other:?}"),
        };
        match &events[1] {
            ErrorEvent::ActorLoopRecovered {
                stall_id: closed, ..
            } => {
                assert_eq!(
                    Some(*closed),
                    stall_id,
                    "the resolution must close the stall that opened: {events:?}"
                );
            }
            other => panic!("expected the resolution second, got {other:?}"),
        }
        drop(events);
        probe_task.abort();
    }

    /// The wedge that the operator deployment misread as a recovery: the parked probe resolves
    /// because the actor's command channel CLOSED, not because the loop
    /// drained. That must page `ActorLoopTerminated`, never
    /// `ActorLoopRecovered`, must mark the shared health terminated so every
    /// delivery fails fast, and must end the probe (nothing is left to watch).
    #[tokio::test(start_paused = true)]
    async fn channel_closed_on_parked_probe_is_termination_not_recovery() {
        let (slot, captured) = capturing_error_hook_slot();
        let health = crate::actor_loop_health::ActorLoopHealth::shared();
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = probes.clone();
        let probe_task = tokio::spawn(run_actor_loop_probe(
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
                async {
                    // Past the 30s budget, then the actor dies under the
                    // parked command.
                    tokio::time::sleep(Duration::from_secs(50)).await;
                    Err(MobError::ActorCommandChannelClosed)
                }
            },
            slot,
            Arc::clone(&health),
            Duration::from_mins(1),
            Duration::from_secs(30),
        ));

        tokio::time::sleep(Duration::from_mins(2)).await;
        tokio::task::yield_now().await;

        let events = captured.lock().await;
        assert_eq!(
            events.len(),
            2,
            "one stall page then one termination: {events:?}"
        );
        assert!(
            matches!(
                &events[0],
                ErrorEvent::ActorLoopStalled {
                    stall_id: Some(1),
                    ..
                }
            ),
            "the stall opens first: {events:?}"
        );
        match &events[1] {
            ErrorEvent::ActorLoopTerminated { stall_id, detail } => {
                assert_eq!(*stall_id, Some(1), "termination names the stall it closed");
                assert!(
                    detail.contains("command channel closed"),
                    "detail carries the channel-closed evidence: {detail}"
                );
            }
            other => panic!("expected ActorLoopTerminated, got {other:?}"),
        }
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ErrorEvent::ActorLoopRecovered { .. })),
            "a dead actor must NEVER be reported as recovered: {events:?}"
        );
        drop(events);
        assert!(
            matches!(
                health.snapshot(),
                crate::actor_loop_health::ActorLoopHealthState::Terminated {
                    stall_id: Some(1),
                    ..
                }
            ),
            "shared health must read terminated: {:?}",
            health.snapshot()
        );
        // The probe ended: many more intervals produce no further round trips.
        tokio::time::sleep(Duration::from_mins(10)).await;
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "a terminated loop has nothing left to probe"
        );
        assert!(
            probe_task.is_finished(),
            "the probe task must end on termination"
        );
    }

    /// meerkat 0.8.33's `MobHandle::status` launders the typed channel-closed
    /// variants into `MobError::Internal(..)` when the phase watch is not
    /// terminal. The classification must see through that, or a laundered
    /// termination reads as a live loop answering with an error.
    #[test]
    fn laundered_channel_closed_text_classifies_as_termination() {
        let laundered = MobError::Internal(format!(
            "{}; last actor-published phase is Running, so the phase watch is not terminal \
             lifecycle authority",
            MobError::ActorCommandChannelClosed
        ));
        assert!(actor_terminated_detail(&Err(laundered)).is_some());
        assert!(actor_terminated_detail(&Err(MobError::ActorReplyChannelClosed)).is_some());
        assert!(actor_terminated_detail(&Err(MobError::ActorCommandChannelClosed)).is_some());
        // A typed refusal the actor ANSWERED with is a live loop.
        assert!(
            actor_terminated_detail(&Err(MobError::Internal(
                "scope denied for this command".to_string()
            )))
            .is_none()
        );
        assert!(actor_terminated_detail(&Ok(MobState::Running)).is_none());
    }

    /// A closed channel on a FRESH round trip (no stall open) is still a
    /// termination with no stall to correlate, and still fails deliveries.
    #[tokio::test(start_paused = true)]
    async fn channel_closed_on_fresh_probe_terminates_without_a_stall_id() {
        let (slot, captured) = capturing_error_hook_slot();
        let health = crate::actor_loop_health::ActorLoopHealth::shared();
        let probe_task = tokio::spawn(run_actor_loop_probe(
            || std::future::ready(Err(MobError::ActorReplyChannelClosed)),
            slot,
            Arc::clone(&health),
            Duration::from_mins(1),
            Duration::from_secs(30),
        ));
        tokio::time::sleep(Duration::from_secs(65)).await;
        tokio::task::yield_now().await;
        let events = captured.lock().await;
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(
            matches!(
                &events[0],
                ErrorEvent::ActorLoopTerminated { stall_id: None, .. }
            ),
            "{events:?}"
        );
        drop(events);
        assert!(health.snapshot().refuses_admission());
        assert!(probe_task.is_finished());
    }

    /// The health seam mirrors the probe's verdict exactly: stalled while the
    /// round trip is parked, live again on the correlated resolution.
    #[tokio::test(start_paused = true)]
    async fn health_reads_stalled_while_parked_and_live_after_recovery() {
        let (slot, _captured) = capturing_error_hook_slot();
        let health = crate::actor_loop_health::ActorLoopHealth::shared();
        let probe_task = tokio::spawn(run_actor_loop_probe(
            || async {
                tokio::time::sleep(Duration::from_secs(50)).await;
                Ok(MobState::Running)
            },
            slot,
            Arc::clone(&health),
            Duration::from_mins(1),
            Duration::from_secs(30),
        ));

        // Interval (60s) + budget (30s) + slack: the stall is open.
        tokio::time::sleep(Duration::from_secs(95)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            health.snapshot().open_stall_id(),
            Some(1),
            "the delivery path must see the open stall: {:?}",
            health.snapshot()
        );

        // The parked round trip answers at 110s.
        tokio::time::sleep(Duration::from_secs(20)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            health.snapshot(),
            crate::actor_loop_health::ActorLoopHealthState::Live,
            "recovery must reopen the delivery path"
        );
        probe_task.abort();
    }

    /// The default sink must log a termination at ERROR with the restart
    /// instruction, never at INFO like a recovery.
    #[test]
    fn termination_is_logged_as_an_error_naming_the_restart() {
        let terminated = ErrorEvent::ActorLoopTerminated {
            stall_id: Some(4),
            detail: "mob actor command channel closed".to_string(),
        };
        let ((), logged) = capture_tracing(|| log_error_event(&terminated, true));
        assert!(logged.contains("ERROR"), "{logged}");
        assert!(
            logged.contains("process must restart") && logged.contains("NOT a recovery"),
            "the operator must be told the loop is gone, not recovered: {logged}"
        );
    }

    #[test]
    fn probe_env_knob_parses_and_clamps() {
        assert_eq!(
            parse_probe_secs(None, ACTOR_LOOP_PROBE_INTERVAL),
            Duration::from_mins(1)
        );
        assert_eq!(
            parse_probe_secs(Some("120"), ACTOR_LOOP_PROBE_INTERVAL),
            Duration::from_mins(2)
        );
        assert_eq!(
            parse_probe_secs(Some(" 15 "), ACTOR_LOOP_PROBE_BUDGET),
            Duration::from_secs(15)
        );
        assert_eq!(
            parse_probe_secs(Some("0"), ACTOR_LOOP_PROBE_BUDGET),
            Duration::from_secs(1)
        );
        assert_eq!(
            parse_probe_secs(Some("999999"), ACTOR_LOOP_PROBE_BUDGET),
            Duration::from_hours(1)
        );
        assert_eq!(
            parse_probe_secs(Some("junk"), ACTOR_LOOP_PROBE_BUDGET),
            ACTOR_LOOP_PROBE_BUDGET
        );
    }

    const IDLE_WORKER: &str = "worker";

    /// A runtime-backed mob (the builder's ephemeral shape) with one idle
    /// turn-driven member, seated while no console forwarder exists yet: the
    /// post-restore window in which the forwarder has not attached to a live
    /// member.
    async fn mob_with_idle_worker(
        mob_id: &str,
        temp_dir: &tempfile::TempDir,
    ) -> (MobRuntime, meerkat_core::types::SessionId) {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
             external_addressable = true\n\n[profiles.worker.tools]\ncomms = true\n"
        ))
        .expect("mob definition");
        let spec = MobBootstrapSpec::ephemeral_runtime_backed_inner(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            temp_dir.path().to_path_buf(),
            16,
            None,
            None,
            None,
            None,
            crate::mob_handle_runtime::CapabilityFlags::default(),
            None,
            None,
        )
        .expect("bootstrap spec")
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let mob_runtime = MobRuntime::bootstrap(spec)
            .await
            .expect("bootstrap mob runtime");
        let handle = mob_runtime.handle();
        let mut member = SpawnMemberSpec::new(
            ProfileName::from("worker"),
            AgentIdentity::from(IDLE_WORKER),
        );
        member.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
        handle.ensure_member(member).await.expect("seat worker");
        let session_id = handle
            .resolve_bridge_session_id(&AgentIdentity::from(IDLE_WORKER))
            .await
            .expect("worker session binding");
        (mob_runtime, session_id)
    }

    /// Run one worker turn and return meerkat's own events for it, observed
    /// on a test-owned subscription opened before the send.
    async fn run_worker_turn(
        handle: &MobHandle,
        content: &str,
    ) -> Vec<meerkat_core::event::EventEnvelope<AgentEvent>> {
        let mut probe = handle
            .subscribe_agent_events(&AgentIdentity::from(IDLE_WORKER))
            .await
            .expect("probe subscription");
        crate::mob_handle_runtime::send_message_on_mob(handle, IDLE_WORKER, content)
            .await
            .expect("send to worker");
        let mut events = Vec::new();
        tokio::time::timeout(crate::test_wait::STRUCTURAL_BACKSTOP, async {
            while let Some(event) = probe.next().await {
                let terminal = matches!(
                    event.payload,
                    AgentEvent::RunCompleted { .. } | AgentEvent::RunFailed { .. }
                );
                events.push(event);
                if terminal {
                    break;
                }
            }
        })
        .await
        .expect("worker turn reaches a terminal");
        assert!(
            matches!(
                events.last().map(|event| &event.payload),
                Some(AgentEvent::RunCompleted { .. })
            ),
            "worker turn completes: {events:?}"
        );
        events
    }

    /// A run that started, and finished, before the console forwarder
    /// existed still reaches the console timeline with meerkat's own
    /// sequence, and so does the next run: the forwarder replays the actor's
    /// retained events from its first.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn console_forwarder_delivers_runs_that_started_before_it_attached() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (mob_runtime, session_id) =
            mob_with_idle_worker("console-replay-first-run", &temp_dir).await;
        let handle = mob_runtime.handle();
        let first_run = run_worker_turn(&handle, "probe-1").await;
        let first_terminal_seq = first_run.last().expect("first run events").seq;

        let module_runtime = std::thread::spawn(|| {
            start_meta_runtime_with_options(
                MetaConfig {
                    modules: vec![],
                    discovery: crate::types::DiscoverySpec {
                        namespace: "console-replay-first-run".to_string(),
                        modules: vec![],
                    },
                    pre_spawn: vec![],
                },
                Vec::new(),
                Duration::from_secs(2),
                RuntimeOptions::default(),
            )
        })
        .join()
        .expect("module runtime thread")
        .expect("module runtime");
        let runtime = UnifiedRuntime::from_parts(
            mob_runtime,
            module_runtime,
            Arc::new(InMemoryMetadataStore::new()),
        )
        .await;

        let second_run = run_worker_turn(&handle, "probe-2").await;
        let second_terminal_seq = second_run.last().expect("second run events").seq;

        let worker_frames =
            async || -> Vec<crate::console_contracts::ConsoleIdentityEventEnvelope> {
                runtime
                    .drain_mob_agent_events()
                    .await
                    .expect("drain member events");
                runtime
                    .console_events()
                    .replay_all(None)
                    .await
                    .expect("console replay")
                    .into_iter()
                    .filter(|frame| frame.identity == IDLE_WORKER)
                    .collect()
            };
        crate::test_wait::poll_until(
            "probe-2's terminal reaches the console timeline",
            crate::test_wait::STRUCTURAL_BACKSTOP,
            async || {
                worker_frames().await.iter().any(|frame| {
                    frame.event_type == "interaction_complete"
                        && frame.data.get("session_id") == Some(&json!(session_id))
                        && frame.data.get("source_sequence") == Some(&json!(second_terminal_seq))
                })
            },
        )
        .await;
        let frames = worker_frames().await;

        let run_started_prompts: Vec<Option<String>> = frames
            .iter()
            .filter(|frame| frame.event_type == "run_started")
            .map(|frame| {
                frame
                    .data
                    .get("input")
                    .cloned()
                    .and_then(|input| {
                        serde_json::from_value::<meerkat_core::types::RunInput>(input).ok()
                    })
                    .and_then(|input| input.prompt_text())
            })
            .collect();
        assert_eq!(
            run_started_prompts,
            vec![Some("probe-1".to_string()), Some("probe-2".to_string())],
            "both runs start on the console timeline, in order"
        );

        let mut first_run_sequences: Vec<u64> = frames
            .iter()
            .filter(|frame| frame.data.get("session_id") == Some(&json!(session_id)))
            .filter_map(|frame| {
                frame
                    .data
                    .get("source_sequence")
                    .and_then(serde_json::Value::as_u64)
            })
            .filter(|seq| *seq <= first_terminal_seq)
            .collect();
        first_run_sequences.sort_unstable();
        assert_eq!(
            first_run_sequences,
            (1..=first_terminal_seq).collect::<Vec<_>>(),
            "the pre-attach run arrives complete from meerkat's first sequence"
        );

        runtime.shutdown().await;
    }

    /// The forwarder's subscription, opened after the member's first run
    /// already finished, names the member's actor and starts at that run's
    /// `RunStarted`; a lapsed subscribe backoff is cleared by it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reconcile_attaches_from_the_actors_first_event_after_a_finished_run() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (mob_runtime, _session_id) =
            mob_with_idle_worker("console-replay-backoff", &temp_dir).await;
        let handle = mob_runtime.handle();
        run_worker_turn(&handle, "probe-1").await;

        let entry = handle
            .list_members_including_retiring()
            .await
            .into_iter()
            .find(|entry| entry.agent_identity == IDLE_WORKER)
            .expect("worker roster entry");
        let (runtime_id, fence_token) = entry.binding_atoms().expect("worker binding atoms");
        let key = TrackedAgentEventStream {
            mob_id: handle.mob_id().to_string(),
            durable_identity: durable_identity_label(&entry.labels),
            member_identity: entry.agent_identity.clone(),
            runtime_id,
            fence_token,
        };
        let mut tracked = AttachedStreams::default();
        let mut subscribe_failures = HashMap::from([(
            key.clone(),
            SubscribeBackoff {
                next_attempt: Some(tokio::time::Instant::now()),
                consecutive_failures: 1,
            },
        )]);
        let mut streams: SelectAll<TaggedAgentEventStream> = SelectAll::new();

        Box::pin(reconcile_agent_event_streams(
            &handle,
            &None,
            &mut tracked,
            &mut subscribe_failures,
            &mut streams,
            None,
        ))
        .await;

        assert!(
            tracked
                .current
                .get(&key)
                .is_some_and(|attachment| attachment.actor.is_some()),
            "the subscription names the member's actor"
        );
        assert!(
            subscribe_failures.is_empty(),
            "the subscription clears the lapsed backoff"
        );
        let first = tokio::time::timeout(Duration::from_secs(5), streams.next())
            .await
            .expect("attached stream yields")
            .expect("attached stream open");
        let ForwardedAgentEvent::Event(event) = first else {
            panic!("attached stream closed before yielding the first run");
        };
        let (_, _, _, envelope, _, _) = *event;
        assert!(
            matches!(envelope.payload, AgentEvent::RunStarted { .. }),
            "the stream starts at the finished run's start, got {:?}",
            envelope.payload
        );
        assert_eq!(envelope.seq, 1);

        drop(streams);
        let _ = handle.shutdown().await;
    }

    /// A member rebound (new binding atoms) onto the same live actor keeps
    /// its stream: the reconciler re-keys it instead of opening a
    /// second stream to that actor, and attributes its events to the new
    /// binding from then on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reconcile_rekeys_a_stream_onto_a_same_actor_rebinding() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (mob_runtime, _session_id) =
            mob_with_idle_worker("console-replay-rekey", &temp_dir).await;
        let handle = mob_runtime.handle();
        let mut tracked = AttachedStreams::default();
        let mut subscribe_failures = HashMap::new();
        let mut streams: SelectAll<TaggedAgentEventStream> = SelectAll::new();
        Box::pin(reconcile_agent_event_streams(
            &handle,
            &None,
            &mut tracked,
            &mut subscribe_failures,
            &mut streams,
            None,
        ))
        .await;
        let (key, attachment) = tracked
            .current
            .drain()
            .next()
            .expect("the worker's stream attached");
        assert!(attachment.actor.is_some());
        let generation = attachment.generation;

        // The same stream, tracked under atoms the roster no longer
        // lists: the member's previous binding onto this actor.
        let mut previous = key.clone();
        previous.fence_token = FenceToken::new(key.fence_token.get() + 1_000);
        {
            let mut attribution = lock_attribution(&attachment.attribution);
            let role = attribution.role.clone();
            *attribution = StreamAttribution::new(previous.clone(), role);
        }
        tracked.current.insert(previous, attachment);

        Box::pin(reconcile_agent_event_streams(
            &handle,
            &None,
            &mut tracked,
            &mut subscribe_failures,
            &mut streams,
            None,
        ))
        .await;

        assert_eq!(tracked.current.len(), 1);
        let rekeyed = tracked
            .current
            .get(&key)
            .expect("the live binding tracks the stream");
        assert_eq!(
            rekeyed.generation, generation,
            "the same stream was re-keyed, not a second subscription opened"
        );
        assert_eq!(streams.len(), 1, "one stream to the actor");

        run_worker_turn(&handle, "probe-1").await;
        let first = tokio::time::timeout(crate::test_wait::STRUCTURAL_BACKSTOP, streams.next())
            .await
            .expect("re-keyed stream yields")
            .expect("re-keyed stream open");
        let ForwardedAgentEvent::Event(event) = first else {
            panic!("re-keyed stream closed");
        };
        let (runtime_id, fence_token, _, envelope, _, _) = *event;
        assert!(matches!(envelope.payload, AgentEvent::RunStarted { .. }));
        assert_eq!(
            (runtime_id, fence_token),
            (key.runtime_id.clone(), key.fence_token),
            "events carry the new binding"
        );

        drop(streams);
        let _ = handle.shutdown().await;
    }

    /// A member whose mob is shutting down is neither retried nor warned
    /// about: its stream closing and its re-subscribe failing are the
    /// teardown, not a spawn race. The closed stream re-runs the pass, as in
    /// the forwarder task; the failed subscribe parks the member with no
    /// retry deadline, a later pass does not try again, and nothing is
    /// logged above debug.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reconcile_neither_retries_nor_warns_for_a_member_of_a_mob_shutting_down() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (mob_runtime, _session_id) =
            mob_with_idle_worker("console-teardown-member", &temp_dir).await;
        let handle = mob_runtime.handle();
        let mut forwarder = DrivenForwarder::new();
        forwarder.reconcile(&handle, &None).await;
        assert_eq!(
            forwarder.tracked.current.len(),
            1,
            "the worker's stream attached"
        );

        // The teardown the runtime's shutdown drives: Stop, then the mob's
        // terminal shutdown, which ends the worker's session and its stream.
        handle.stop().await.expect("stop the mob");
        let shutdown_handle = handle.clone();
        let shutdown = tokio::spawn(async move { shutdown_handle.shutdown().await });
        let (closed_key, generation) = loop {
            if let ForwardedAgentEvent::Closed(key, generation) = forwarder.next().await {
                break (key, generation);
            }
        };
        assert!(forwarder.tracked.close(&closed_key, generation));

        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .finish();
        // A second registered dispatcher keeps callsite interest computed for
        // every dispatcher, so a callsite another test's thread reached first
        // still records here.
        let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        let guard = tracing::subscriber::set_default(subscriber);
        forwarder.reconcile(&handle, &None).await;
        let parked_after_first_pass = forwarder.subscribe_failures.contains_key(&closed_key);
        forwarder.reconcile(&handle, &None).await;
        drop(guard);
        let logged = writer.contents();
        let _ = shutdown.await;

        let retry = earliest_backoff_attempt(&forwarder.subscribe_failures);
        assert!(
            retry.is_none() && !logged.contains("will retry with backoff"),
            "a member of a mob shutting down is neither retried (next attempt \
             {retry:?}) nor warned about: {logged}"
        );
        assert!(
            forwarder.tracked.current.is_empty(),
            "nothing attaches to a member being torn down"
        );
        assert!(
            parked_after_first_pass,
            "the failed subscribe parks the member: {logged}"
        );
        assert_eq!(
            logged.matches("not retrying").count(),
            1,
            "one debug line, and the second pass does not try again: {logged}"
        );
    }

    /// A member parked by its mob's teardown is tried again once the mob
    /// runs: the park holds only while the mob is stopping or stopped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reconcile_tries_a_parked_member_again_once_its_mob_runs() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (mob_runtime, _session_id) =
            mob_with_idle_worker("console-parked-member", &temp_dir).await;
        let handle = mob_runtime.handle();
        let entry = handle
            .list_members_including_retiring()
            .await
            .into_iter()
            .find(|entry| entry.agent_identity == IDLE_WORKER)
            .expect("worker roster entry");
        let (runtime_id, fence_token) = entry.binding_atoms().expect("worker binding atoms");
        let key = TrackedAgentEventStream {
            mob_id: handle.mob_id().to_string(),
            durable_identity: durable_identity_label(&entry.labels),
            member_identity: entry.agent_identity.clone(),
            runtime_id,
            fence_token,
        };
        let mut forwarder = DrivenForwarder::new();
        forwarder.subscribe_failures.insert(
            key.clone(),
            SubscribeBackoff {
                next_attempt: None,
                consecutive_failures: 0,
            },
        );
        assert_eq!(
            earliest_backoff_attempt(&forwarder.subscribe_failures),
            None,
            "a parked member schedules no retry"
        );

        forwarder.reconcile(&handle, &None).await;

        assert!(
            forwarder.tracked.current.contains_key(&key),
            "the running mob's member attached"
        );
        assert!(forwarder.subscribe_failures.is_empty());
        drop(forwarder);
        let _ = handle.shutdown().await;
    }

    /// Only a mob that says it stops parks its members: a closed admission,
    /// or a Stopped, Completed or Destroyed phase. A mob that runs and admits
    /// is no teardown, whatever its actor answered.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn only_a_mob_that_stops_or_closes_admission_parks_its_members() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (standing_down, _) = mob_with_idle_worker("console-park-stand-down", &temp_dir).await;
        let handle = standing_down.handle();
        assert!(
            !mob_is_stopping_or_stopped(&handle),
            "a mob that runs and admits"
        );
        handle
            .close_admission(meerkat_mob::MobAdmissionCloseReason::StandDown)
            .await
            .expect("close admission");
        assert!(mob_is_stopping_or_stopped(&handle), "a mob standing down");
        let _ = handle.shutdown().await;

        let other_dir = tempfile::tempdir().expect("temp dir");
        let (stopped, _) = mob_with_idle_worker("console-park-stopped", &other_dir).await;
        let handle = stopped.handle();
        handle.stop().await.expect("stop the mob");
        assert!(mob_is_stopping_or_stopped(&handle), "a stopped mob");
        let _ = handle.shutdown().await;
    }

    /// A member of a destroyed mob leaves the subscribe bookkeeping once the
    /// roster drops it: the pass prunes every entry whose member it no
    /// longer lists, a parked one included.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reconcile_prunes_a_destroyed_mobs_parked_member() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (mob_runtime, _session_id) =
            mob_with_idle_worker("console-destroyed-member", &temp_dir).await;
        let handle = mob_runtime.handle();
        let entry = handle
            .list_members_including_retiring()
            .await
            .into_iter()
            .find(|entry| entry.agent_identity == IDLE_WORKER)
            .expect("worker roster entry");
        let (runtime_id, fence_token) = entry.binding_atoms().expect("worker binding atoms");
        let key = TrackedAgentEventStream {
            mob_id: handle.mob_id().to_string(),
            durable_identity: durable_identity_label(&entry.labels),
            member_identity: entry.agent_identity.clone(),
            runtime_id,
            fence_token,
        };
        let mut forwarder = DrivenForwarder::new();
        forwarder.subscribe_failures.insert(
            key,
            SubscribeBackoff {
                next_attempt: None,
                consecutive_failures: 0,
            },
        );

        handle.destroy().await.expect("destroy the mob");
        assert_eq!(handle.status_observation_snapshot(), MobState::Destroyed);
        forwarder.reconcile(&handle, &None).await;

        assert!(
            forwarder.subscribe_failures.is_empty(),
            "the destroyed mob's member was pruned ({} entries left)",
            forwarder.subscribe_failures.len()
        );
        assert!(forwarder.tracked.current.is_empty());
    }

    const RESTART_MEMBER: &str = "lead-1";

    /// One gateway-shaped boot against a durable store: a persistent spec,
    /// `UnifiedRuntime::bootstrap` (which starts the console forwarder), then
    /// identity-first activation and restore, which revives the persisted
    /// member.
    async fn boot_identity_first_persistent(
        mob_id: &str,
        mob_path: &std::path::Path,
        state_root: &std::path::Path,
    ) -> (UnifiedRuntime, Arc<crate::identity_first::IdentityRuntime>) {
        let boot = boot_persistent_before_activation(mob_id, mob_path, state_root).await;
        let identity_runtime = boot.identity_runtime.clone();
        let runtime = boot.activate().await;
        (runtime, identity_runtime)
    }

    /// A booted persistent runtime whose identity-first restore has not run
    /// yet: the window in which the console forwarder already exists and the
    /// persisted member is listed Active with its restored binding, but no
    /// live actor serves its session.
    struct PersistentBootBeforeActivation {
        runtime: UnifiedRuntime,
        identity_runtime: Arc<crate::identity_first::IdentityRuntime>,
        context: Arc<crate::identity_first::IdentityFirstRuntimeContext>,
        roster: Vec<crate::identity_first::DurableAgentSpec>,
    }

    impl PersistentBootBeforeActivation {
        async fn activate(self) -> UnifiedRuntime {
            let Self {
                mut runtime,
                context,
                roster,
                ..
            } = self;
            runtime
                .install_and_bootstrap_identity_first_context(context, &roster)
                .await
                .expect("activate and restore identity-first runtime");
            runtime
        }
    }

    async fn boot_persistent_before_activation(
        mob_id: &str,
        mob_path: &std::path::Path,
        state_root: &std::path::Path,
    ) -> PersistentBootBeforeActivation {
        use crate::identity_first::{
            AgentAddressability, AgentRuntimeServices, ContinuityStore, DurabilityPolicy,
            DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig,
            LocalContinuityStore, LocalLeaseProvider, MobSessionBridge, MutableRosterProvider,
        };

        std::fs::create_dir_all(state_root).expect("state root");
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n\n[profiles.lead]\nmodel = \"gpt-5.5\"\n\
             external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
             [profiles.lead.tools]\ncomms = true\n"
        ))
        .expect("mob definition");
        let (storage, provenance) =
            crate::mob_composition_manifest::persistent_mob_storage(mob_path.to_path_buf())
                .expect("open persistent mob storage");
        let spec = MobBootstrapSpec::persistent(
            definition,
            storage,
            provenance,
            state_root.to_path_buf(),
            16,
        )
        .await
        .expect("compose persistent Meta stores")
        // These restarts happen in one process. A durable child-mob default
        // would hold `<state>/mobs` for the process lifetime and refuse the
        // successor, so child mobs stay process-bound here.
        .with_child_mob_storage(crate::ChildMobStorage::ProcessBound)
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let runtime = UnifiedRuntime::bootstrap(
            spec,
            MetaConfig {
                modules: Vec::new(),
                discovery: crate::types::DiscoverySpec {
                    namespace: mob_id.to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            },
            Duration::from_secs(2),
        )
        .await
        .expect("bootstrap unified runtime");
        let roster = vec![DurableAgentSpec {
            identity: crate::identity_first::AgentIdentity::parse(RESTART_MEMBER)
                .expect("identity"),
            profile: ProfileName::from("lead"),
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
        }];
        let continuity_store = Arc::new(
            LocalContinuityStore::open(state_root.join("identity-continuity.sqlite3"))
                .expect("open identity continuity store"),
        );
        let identity_runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: continuity_store as Arc<dyn ContinuityStore>,
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: format!("{mob_id}-instance"),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(
                    MobSessionBridge::with_session_service(
                        // The gateways' raw bootstrap wiring: the composition's
                        // own bridge and services are non-owning back-edges (D67).
                        crate::gateway_wiring::identity_mob_handle(&runtime),
                        runtime
                            .mob_runtime()
                            .session_service()
                            .cloned()
                            .expect("persistent runtime has a session service"),
                    )
                    // As the gateways do: the concrete session owner's actor
                    // operations.
                    .with_controller_pin_source(
                        runtime
                            .mob_runtime()
                            .controller_pin_source()
                            .expect("the persistent runtime records its session owner"),
                    ),
                )),
                default_timeout: None,
            })
            .with_runtime_services(AgentRuntimeServices::new(
                crate::gateway_wiring::identity_mob_handle(&runtime),
            )),
        );
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            identity_runtime.clone(),
            Arc::new(MutableRosterProvider::new(roster.clone())),
            None,
            None,
            Some(runtime.mob_handle().definition().clone()),
        ));
        PersistentBootBeforeActivation {
            runtime,
            identity_runtime,
            context,
            roster,
        }
    }

    async fn commit_restart_member_turn(
        identity_runtime: &crate::identity_first::IdentityRuntime,
        prompt: &str,
    ) -> meerkat_core::types::SessionId {
        let identity =
            crate::identity_first::AgentIdentity::parse(RESTART_MEMBER).expect("identity");
        identity_runtime
            .send_awaiting_commit(
                &identity,
                &meerkat_core::ContentInput::Text(prompt.to_string()),
            )
            .await
            .expect("complete member turn");
        identity_runtime
            .status(&identity)
            .await
            .expect("member status")
            .session_id
            .expect("member session")
    }

    /// How the first boot leaves the member before the restart.
    #[derive(Clone, Copy)]
    enum RestartShape {
        /// Live at shutdown: the restore resumes it (the Resume route).
        Live,
        /// Retired on the mob plane (session archived, snapshot intact): the
        /// restore revives the archived session (the Revivable route).
        Retired,
    }

    /// The reported bug end to end: a gateway-shaped restart against the
    /// same durable store revives the member through the restore route while
    /// the console forwarder already runs, and the revived actor's first run
    /// still reaches the console timeline from `run_started` (seq 1) and
    /// `turn_started` (seq 2), exactly once.
    ///
    /// The real forwarder races the restore here, so this asserts only what
    /// holds under every legal interleaving: whenever the forwarder's
    /// subscription lands (a backoff retry, or a machine-change pass), it
    /// replays the revived actor's retained events from its first, so the
    /// head arrives exactly once. The late-attach schedule is fixed
    /// deterministically by
    /// `restored_members_first_run_reaches_the_console_after_stream_not_found_backoff`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn console_timeline_carries_a_revived_members_first_run_after_restart() {
        console_timeline_after_restart("console-replay-restart", RestartShape::Live).await;
    }

    /// Same, for a member the restore revives from its archived session.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn console_timeline_carries_an_archived_members_first_run_after_restart() {
        console_timeline_after_restart("console-replay-restart-archived", RestartShape::Retired)
            .await;
    }

    /// The state directory contract (`crate::state_dir_claim`): after a
    /// clean shutdown, a successor in the same process composes the
    /// directory and serves a turn while the predecessor's handles are still
    /// alive. They keep Core's shared execution custody until they drop, and
    /// the custody probe attributes that lock to this process
    /// (`crate::native_startup`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_same_process_successor_composes_while_the_shut_down_predecessor_is_alive() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mob_id = "same-process-successor";
        let mob_path = temp.path().join("mob.sqlite3");
        let state_root = temp.path().join("state");
        let (first, first_identity) =
            boot_identity_first_persistent(mob_id, &mob_path, &state_root).await;
        let session_id = commit_restart_member_turn(&first_identity, "before restart").await;
        first.shutdown().await;

        let (second, second_identity) =
            boot_identity_first_persistent(mob_id, &mob_path, &state_root).await;
        assert_eq!(
            commit_restart_member_turn(&second_identity, "after restart").await,
            session_id,
            "the successor resumed the same session"
        );
        second.shutdown().await;
        drop((first, first_identity));
    }

    /// Boot twice against one durable store; the second boot's runtime has
    /// its own event ingress stopped (so a test drives the health monitor)
    /// and is activated (the revived member's actor exists and Core holds it
    /// live). Returns the runtime's completion ledger, which its installed
    /// completion drain shares.
    async fn health_restart(
        mob_id: &str,
        temp: &tempfile::TempDir,
    ) -> (
        UnifiedRuntime,
        Arc<crate::identity_first::IdentityRuntime>,
        Arc<HealthCompletionLedger>,
        crate::identity_first::AgentIdentity,
    ) {
        let mob_path = temp.path().join("mob.sqlite3");
        let state_root = temp.path().join("state");
        let (first, first_identity) =
            boot_identity_first_persistent(mob_id, &mob_path, &state_root).await;
        commit_restart_member_turn(&first_identity, "before restart").await;
        first.shutdown().await;

        let boot = boot_persistent_before_activation(mob_id, &mob_path, &state_root).await;
        // The runtime's own forwarder and health monitor stop; the test
        // drives the monitor.
        let _ingress = boot.runtime.install_test_event_ingress().await;
        let identity_runtime = boot.identity_runtime.clone();
        let runtime = boot.activate().await;
        let ledger = Arc::clone(&runtime.identity_completion_ledger);
        let identity =
            crate::identity_first::AgentIdentity::parse(RESTART_MEMBER).expect("identity");
        assert_eq!(
            identity_runtime
                .status(&identity)
                .await
                .expect("status")
                .state,
            crate::identity_first::IdentityLifecycleState::Active,
            "Core restored the member live"
        );
        (runtime, identity_runtime, ledger, identity)
    }

    /// Drain the monitor's streams for the rest of the test, crediting
    /// completions exactly as the monitor does.
    fn drive_health_streams(
        mut streams: SelectAll<TaggedAgentEventStream>,
        slot: Arc<std::sync::RwLock<Option<Arc<crate::identity_first::IdentityRuntime>>>>,
        ledger: Arc<HealthCompletionLedger>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(forwarded) = streams.next().await {
                if let ForwardedAgentEvent::Event(event) = forwarded {
                    let (_, _, _, envelope, durable_identity, epoch) = *event;
                    let _credited = record_identity_turn_completion(
                        &slot,
                        &ledger,
                        durable_identity.as_deref(),
                        epoch,
                        &envelope,
                    )
                    .await;
                }
            }
        })
    }

    /// A drain that credits nothing, so only the monitor can count a turn.
    struct NoPendingCompletions;

    #[async_trait::async_trait]
    impl crate::identity_first::runtime::PendingCompletionDrain for NoPendingCompletions {
        async fn drain(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> crate::identity_first::runtime::PendingRunTerminals {
            crate::identity_first::runtime::PendingRunTerminals::default()
        }
    }

    /// Activation installs the completion drain before the binding observer,
    /// so the health monitor's first binding-woken attach may already replay.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_completion_drain_is_installed_before_the_binding_observer() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime, _ledger, _identity) =
            health_restart("identity-health-drain-order", &temp).await;
        assert!(identity_runtime.has_pending_completion_drain());
        assert!(
            identity_runtime.binding_observer_installed_after_drain(),
            "the binding observer saw the drain installed"
        );
        runtime.shutdown().await;
    }

    /// A drain that reports one failed run once armed.
    struct FailingOnceDrain {
        armed: std::sync::atomic::AtomicBool,
        calls: tokio::sync::watch::Sender<u64>,
    }

    #[async_trait::async_trait]
    impl crate::identity_first::runtime::PendingCompletionDrain for FailingOnceDrain {
        async fn drain(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> crate::identity_first::runtime::PendingRunTerminals {
            self.calls.send_modify(|calls| *calls += 1);
            crate::identity_first::runtime::PendingRunTerminals {
                completed: 0,
                failed: u64::from(self.armed.swap(false, std::sync::atomic::Ordering::AcqRel)),
            }
        }
    }

    /// K4: a failed run the drain consumes is recorded, so a failure-aware
    /// completion wait ends typed instead of running to its deadline.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_drained_failed_run_ends_a_completion_wait_typed() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime, _ledger, identity) =
            health_restart("identity-health-drained-failure", &temp).await;
        let (calls, mut observed) = tokio::sync::watch::channel(0_u64);
        let drain = Arc::new(FailingOnceDrain {
            armed: std::sync::atomic::AtomicBool::new(false),
            calls,
        });
        identity_runtime.install_pending_completion_drain(drain.clone());
        let baseline = identity_runtime.completion_cursor(&identity).await;
        let calls_before_wait = *observed.borrow_and_update();
        let wait = {
            let identity_runtime = identity_runtime.clone();
            let identity = identity.clone();
            tokio::spawn(async move {
                identity_runtime
                    .await_completion(&identity, Some(baseline), Duration::from_secs(10))
                    .await
            })
        };
        // The wait drained twice (before its failure baseline, then its first
        // read), so the baseline is taken.
        observed
            .wait_for(|calls| *calls >= calls_before_wait + 2)
            .await
            .expect("drain calls observed");
        drain
            .armed
            .store(true, std::sync::atomic::Ordering::Release);
        identity_runtime.completion_cursor(&identity).await;
        let outcome = tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .expect("the wait ends promptly")
            .expect("wait task");
        assert!(
            matches!(outcome, crate::identity_first::CompletionWait::RunFailed(_)),
            "{outcome:?}"
        );
        runtime.shutdown().await;
    }

    #[test]
    fn health_completion_ledger_claims_each_sequence_once_per_space() {
        use meerkat_core::comms::{SessionEventCursor, SessionEventEpoch};
        let ledger = HealthCompletionLedger::default();
        let session = meerkat_core::types::SessionId::new();
        let space_epoch = SessionEventEpoch::new();
        let space = Some(space_epoch);
        assert_eq!(ledger.resume_cursor(&session), SessionEventCursor::Earliest);
        assert!(
            !ledger.claim(&session, space, 0),
            "gap markers are never claimed"
        );
        assert!(ledger.claim(&session, space, 1));
        assert!(ledger.claim(&session, space, 3));
        assert!(!ledger.claim(&session, space, 2), "below the high-water");
        assert!(!ledger.claim(&session, space, 3), "claimed once");
        assert_eq!(
            ledger.resume_cursor(&session),
            SessionEventCursor::After {
                epoch: space_epoch,
                seq: 3,
            }
        );
        // A new sequence space (a placed member's host restart) starts its
        // own high-water instead of refusing its first sequences, and becomes
        // the space the drain resumes in.
        let restarted_epoch = SessionEventEpoch::new();
        let restarted = Some(restarted_epoch);
        assert!(ledger.claim(&session, restarted, 1));
        assert!(!ledger.claim(&session, restarted, 1));
        assert_eq!(
            ledger.resume_cursor(&session),
            SessionEventCursor::After {
                epoch: restarted_epoch,
                seq: 1,
            }
        );
        // A late stream of the older space re-attaching claims nothing it
        // already credited, and does not move the drain back to that space.
        assert!(
            !ledger.claim(&session, space, 3),
            "credited in its own space"
        );
        assert!(!ledger.claim(&session, space, 1));
        assert!(
            ledger.claim(&session, space, 4),
            "only past its own high-water"
        );
        assert_eq!(
            ledger.resume_cursor(&session),
            SessionEventCursor::After {
                epoch: restarted_epoch,
                seq: 1,
            }
        );
    }

    /// The drain (under a cursor read) and the monitor's replaying stream
    /// race over the same events while turns complete and cursor reads
    /// keep draining: every turn is credited exactly once, and no read ever
    /// runs ahead of the turns that actually completed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn identity_health_drain_and_monitor_race_credits_each_turn_once() {
        const TURNS: u64 = 4;
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime, ledger, identity) =
            health_restart("identity-health-race", &temp).await;
        let slot = Arc::new(std::sync::RwLock::new(Some(identity_runtime.clone())));
        let handle = runtime.mob_handle();
        let mut tracked = AttachedStreams::default();
        let mut failures = HashMap::new();
        let mut streams = SelectAll::new();
        Box::pin(reconcile_agent_event_streams(
            &handle,
            &None,
            &mut tracked,
            &mut failures,
            &mut streams,
            Some(&slot),
        ))
        .await;
        assert!(!tracked.current.is_empty(), "the monitor attached");
        let before = identity_runtime.completion_cursor(&identity).await;
        let monitor = drive_health_streams(streams, Arc::clone(&slot), Arc::clone(&ledger));
        let committed = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let reader = {
            let identity_runtime = identity_runtime.clone();
            let identity = identity.clone();
            let committed = Arc::clone(&committed);
            tokio::spawn(async move {
                let mut last = before;
                loop {
                    let cursor = identity_runtime.completion_cursor(&identity).await;
                    assert!(cursor >= last, "the cursor never rewinds");
                    // Turns run one at a time, so at most one completed turn
                    // is not yet counted in `committed`.
                    let done = committed.load(std::sync::atomic::Ordering::Acquire);
                    assert!(
                        cursor <= (0..=done).fold(before, |cursor, _| cursor.advanced()),
                        "a read never runs ahead of completed turns"
                    );
                    last = cursor;
                    tokio::task::yield_now().await;
                }
            })
        };
        for turn in 0..TURNS {
            commit_restart_member_turn(&identity_runtime, &format!("race turn {turn}")).await;
            committed.fetch_add(1, std::sync::atomic::Ordering::Release);
        }
        let expected = (0..TURNS).fold(before, |cursor, _| cursor.advanced());
        identity_runtime
            .wait_for_completion(
                &identity,
                (0..TURNS - 1).fold(before, |cursor, _| cursor.advanced()),
                Duration::from_secs(10),
            )
            .await
            .expect("the last turn is credited");
        assert_eq!(
            identity_runtime.completion_cursor(&identity).await,
            expected,
            "each turn credited exactly once"
        );
        reader.abort();
        assert!(
            reader.await.err().is_some_and(|error| error.is_cancelled()),
            "the reader's invariants held"
        );
        monitor.abort();
        runtime.shutdown().await;
    }

    /// A Core fence rotation re-keys the member's binding: the monitor cuts
    /// its previous stream off and opens a new one, which replays the actor's
    /// retained events. The replay credits nothing twice, and the
    /// next turn is counted once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn identity_health_fence_rotation_replay_credits_nothing_twice() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime, ledger, identity) =
            health_restart("identity-health-rotation", &temp).await;
        commit_restart_member_turn(&identity_runtime, "before rotation").await;
        let slot = Arc::new(std::sync::RwLock::new(Some(identity_runtime.clone())));
        let handle = runtime.mob_handle();
        let mut tracked = AttachedStreams::default();
        let mut failures = HashMap::new();
        let mut streams = SelectAll::new();
        Box::pin(reconcile_agent_event_streams(
            &handle,
            &None,
            &mut tracked,
            &mut failures,
            &mut streams,
            Some(&slot),
        ))
        .await;
        let (key, attachment) = tracked
            .current
            .drain()
            .next()
            .expect("the monitor attached");
        // The same stream under the member's previous Core fence: the pass
        // sees the binding re-keyed (a fence rotation).
        let mut previous = key.clone();
        previous.fence_token = FenceToken::new(key.fence_token.get() + 1_000);
        tracked.current.insert(previous, attachment);
        Box::pin(reconcile_agent_event_streams(
            &handle,
            &None,
            &mut tracked,
            &mut failures,
            &mut streams,
            Some(&slot),
        ))
        .await;
        assert!(
            tracked.current.contains_key(&key),
            "a new stream serves the rotated fence"
        );

        let baseline = identity_runtime.completion_cursor(&identity).await;
        let monitor = drive_health_streams(streams, Arc::clone(&slot), Arc::clone(&ledger));
        assert!(
            identity_runtime
                .wait_for_completion(&identity, baseline, Duration::from_millis(300))
                .await
                .is_err(),
            "neither the cut-off stream nor the new stream's replay credits a turn again"
        );
        commit_restart_member_turn(&identity_runtime, "after rotation").await;
        let completed = identity_runtime
            .wait_for_completion(&identity, baseline, Duration::from_secs(5))
            .await
            .expect("the next turn completes the wait");
        assert_eq!(completed, baseline.advanced(), "counted exactly once");

        monitor.abort();
        runtime.shutdown().await;
    }

    /// The identity health monitor's window (the Meta operator-method
    /// seed-turn timeouts), and the late-replay over-count it must not
    /// cause. A turn runs and completes while the monitor is not attached.
    /// It is credited before the next completion-cursor read (credit before
    /// read, replayed from meerkat's retained session events), so a baseline
    /// taken now already includes it. The monitor attaching later replays
    /// the same events and credits nothing twice; a wait on that baseline is
    /// satisfied only by a newer turn, counted exactly once whether the
    /// monitor's stream or a cursor read's drain claims it first.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn identity_health_credits_a_window_turn_before_any_later_baseline() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime, ledger, identity) =
            health_restart("identity-health-window", &temp).await;

        let before = identity_runtime.completion_cursor(&identity).await;
        // The window: a turn runs and completes with no monitor attached.
        commit_restart_member_turn(&identity_runtime, "in the window").await;
        let baseline = identity_runtime.completion_cursor(&identity).await;
        assert_eq!(
            baseline,
            before.advanced(),
            "the read credited the window turn first, exactly once"
        );

        // The monitor attaches now and replays the actor's retained events.
        let slot = Arc::new(std::sync::RwLock::new(Some(identity_runtime.clone())));
        let handle = runtime.mob_handle();
        let mut tracked = AttachedStreams::default();
        let mut failures = HashMap::new();
        let mut streams = SelectAll::new();
        Box::pin(reconcile_agent_event_streams(
            &handle,
            &None,
            &mut tracked,
            &mut failures,
            &mut streams,
            Some(&slot),
        ))
        .await;
        assert!(!tracked.current.is_empty(), "the monitor attached");
        let monitor = drive_health_streams(streams, Arc::clone(&slot), Arc::clone(&ledger));
        assert!(
            identity_runtime
                .wait_for_completion(&identity, baseline, Duration::from_millis(300))
                .await
                .is_err(),
            "the replayed window turn is not credited twice, and never satisfies a newer baseline"
        );

        // A newer turn is counted once.
        commit_restart_member_turn(&identity_runtime, "after the attach").await;
        let completed = identity_runtime
            .wait_for_completion(&identity, baseline, Duration::from_secs(5))
            .await
            .expect("the newer turn completes the wait");
        assert_eq!(completed, baseline.advanced(), "counted exactly once");
        assert_eq!(
            identity_runtime.completion_cursor(&identity).await,
            baseline.advanced(),
            "the monitor's replay and the drain never both credit it"
        );

        monitor.abort();
        runtime.shutdown().await;
    }

    /// The wiring: the real health monitor loop (its `ReconcileCadence`),
    /// not a hand-driven reconcile, attaches a member because its lease wake
    /// fired. The monitor starts with no identity authority installed, so its
    /// first passes skip the member; installing the authority and its lease
    /// observer is the only wake, far inside the 30 s safety tick. The
    /// completion drain is replaced by one that credits nothing, so only the
    /// attached monitor can count the turn.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_health_monitor_attaches_on_the_binding_wake() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime, ledger, identity) =
            health_restart("identity-health-wake", &temp).await;
        identity_runtime.install_pending_completion_drain(Arc::new(NoPendingCompletions));

        let slot = Arc::new(std::sync::RwLock::new(None));
        let (lease_wake, binding_changes) = tokio::sync::watch::channel(0_u64);
        let monitor = tokio::spawn(run_identity_stream_health_monitor(
            runtime.mob_handle(),
            None,
            Arc::clone(&slot),
            ledger,
            binding_changes,
        ));
        let baseline = identity_runtime.completion_cursor(&identity).await;
        // With no authority the monitor cannot attach.
        tokio::task::yield_now().await;

        *slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(identity_runtime.clone());
        identity_runtime.install_member_binding_observer(lease_wake);
        commit_restart_member_turn(&identity_runtime, "after the wake").await;
        identity_runtime
            .wait_for_completion(&identity, baseline, Duration::from_secs(10))
            .await
            .expect("the lease wake attached the monitor, well inside the safety tick");

        monitor.abort();
        runtime.shutdown().await;
    }

    /// The identity health monitor logs a member it fails to subscribe to,
    /// as the console forwarder does: a warning on the first failure, then
    /// debug while it keeps failing. Both were skipped for the monitor, so a
    /// member it could not watch, and whose turns it then could not count,
    /// left no trace until the stream repair.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_identity_health_monitor_logs_a_failed_subscription() {
        let mob_id = "identity-health-subscribe-log";
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime, _ledger, _identity) = health_restart(mob_id, &temp).await;
        let slot = Arc::new(std::sync::RwLock::new(Some(identity_runtime.clone())));
        let handle = runtime.mob_handle();
        let mut tracked = AttachedStreams::default();
        let mut failures = HashMap::new();
        let mut streams = SelectAll::new();
        *FAIL_MEMBER_EVENT_SUBSCRIPTIONS_OF_MOB
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(mob_id.to_string());
        // A second registered dispatch keeps the capture's interest in the
        // warning when another thread reached its callsite first.
        let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        let (_, log) =
            crate::test_diagnostics::capture_warnings(Box::pin(reconcile_agent_event_streams(
                &handle,
                &None,
                &mut tracked,
                &mut failures,
                &mut streams,
                Some(&slot),
            )))
            .await;
        *FAIL_MEMBER_EVENT_SUBSCRIPTIONS_OF_MOB
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        assert_eq!(
            (
                tracked.current.len(),
                failures.len(),
                log.matches("failed to subscribe; will retry with backoff")
                    .count(),
                log.contains("identity_health_monitor"),
            ),
            (0, 1, 1, true),
            "{log}"
        );
        runtime.shutdown().await;
    }

    async fn console_timeline_after_restart(mob_id: &str, shape: RestartShape) {
        let temp = tempfile::tempdir().expect("temp dir");
        let mob_path = temp.path().join("mob.sqlite3");
        let state_root = temp.path().join("state");

        let (first, first_identity) =
            boot_identity_first_persistent(mob_id, &mob_path, &state_root).await;
        let session_id = commit_restart_member_turn(&first_identity, "before restart").await;
        if let RestartShape::Retired = shape {
            let handle = first.mob_handle();
            let members = handle.list_members().await;
            assert_eq!(members.len(), 1, "one durable member");
            let member = members[0].agent_identity.clone();
            handle
                .retire(member.clone())
                .await
                .expect("mob-plane retire archives the session");
            crate::test_wait::poll_until(
                "the mob-plane retire finalizes",
                crate::test_wait::STRUCTURAL_BACKSTOP,
                async || {
                    !handle
                        .list_members()
                        .await
                        .iter()
                        .any(|entry| entry.agent_identity == member && !entry.is_final)
                },
            )
            .await;
        }
        first.shutdown().await;

        let (second, second_identity) =
            boot_identity_first_persistent(mob_id, &mob_path, &state_root).await;
        let revived_session = commit_restart_member_turn(&second_identity, "after restart").await;
        assert_eq!(
            revived_session, session_id,
            "the restore resumed the same session"
        );

        let session_frames =
            async || -> Vec<crate::console_contracts::ConsoleIdentityEventEnvelope> {
                second
                    .drain_mob_agent_events()
                    .await
                    .expect("drain member events");
                second
                    .console_events()
                    .replay_all(None)
                    .await
                    .expect("console replay")
                    .into_iter()
                    .filter(|frame| frame.data.get("session_id") == Some(&json!(session_id)))
                    .collect()
            };
        let count_at = |frames: &[crate::console_contracts::ConsoleIdentityEventEnvelope],
                        event_type: &str,
                        seq: u64| {
            frames
                .iter()
                .filter(|frame| {
                    frame.event_type == event_type
                        && frame.data.get("source_sequence") == Some(&json!(seq))
                })
                .count()
        };
        crate::test_wait::poll_until(
            "the revived member's first run starts on the console timeline",
            crate::test_wait::STRUCTURAL_BACKSTOP,
            async || {
                let frames = session_frames().await;
                count_at(&frames, "run_started", 1) > 0 && count_at(&frames, "turn_started", 2) > 0
            },
        )
        .await;
        // The head arrives once, however the subscription raced the run.
        let frames = session_frames().await;
        assert_eq!(count_at(&frames, "run_started", 1), 1, "{frames:#?}");
        assert_eq!(count_at(&frames, "turn_started", 2), 1, "{frames:#?}");

        second.shutdown().await;
    }

    /// The console forwarder's own steps, driven by a test: the same
    /// reconcile pass and stream set the forwarder task runs, so a test can
    /// fix when the forwarder attaches relative to a member's runs.
    struct DrivenForwarder {
        tracked: AttachedStreams,
        subscribe_failures: HashMap<TrackedAgentEventStream, SubscribeBackoff>,
        streams: SelectAll<TaggedAgentEventStream>,
    }

    impl DrivenForwarder {
        fn new() -> Self {
            Self {
                tracked: AttachedStreams::default(),
                subscribe_failures: HashMap::new(),
                streams: SelectAll::new(),
            }
        }

        async fn reconcile(
            &mut self,
            handle: &MobHandle,
            agent_mob_mcp_state: &Option<Arc<meerkat_mob_mcp::MobMcpState>>,
        ) {
            Box::pin(reconcile_agent_event_streams(
                handle,
                agent_mob_mcp_state,
                &mut self.tracked,
                &mut self.subscribe_failures,
                &mut self.streams,
                None,
            ))
            .await;
        }

        async fn next(&mut self) -> ForwardedAgentEvent {
            tokio::time::timeout(crate::test_wait::STRUCTURAL_BACKSTOP, self.streams.next())
                .await
                .expect("the forwarder's streams yield")
                .expect("the forwarder holds a stream")
        }

        /// Forward events to the console until `terminals` run terminals
        /// went through, the way the forwarder task does. A close is handled
        /// as the task handles it: record it, then reconcile.
        async fn forward_terminals(
            &mut self,
            runtime: &UnifiedRuntime,
            ingress: &Sender<ForwardedMemberEvent>,
            terminals: usize,
        ) -> Vec<ForwarderStep> {
            let mut seen = 0;
            self.forward_until(runtime, ingress, |payload| {
                if matches!(
                    payload,
                    AgentEvent::RunCompleted { .. } | AgentEvent::RunFailed { .. }
                ) {
                    seen += 1;
                }
                seen == terminals
            })
            .await
        }

        /// Forward until an event for which `done` holds went through.
        async fn forward_until(
            &mut self,
            runtime: &UnifiedRuntime,
            ingress: &Sender<ForwardedMemberEvent>,
            mut done: impl FnMut(&AgentEvent) -> bool,
        ) -> Vec<ForwarderStep> {
            let handle = runtime.mob_handle();
            let mut steps = Vec::new();
            let mut finished = false;
            while !finished {
                match self.next().await {
                    ForwardedAgentEvent::Event(event) => {
                        let (source, source_fence_token, role, envelope, _, _) = *event;
                        finished = done(&envelope.payload);
                        steps.push(ForwarderStep::Event(Box::new(envelope.clone())));
                        ingress
                            .send(forwarded_member_event(AttributedEvent {
                                source,
                                source_fence_token,
                                role,
                                envelope,
                            }))
                            .await
                            .expect("console ingress open");
                        runtime
                            .drain_mob_agent_events()
                            .await
                            .expect("drain member events");
                    }
                    ForwardedAgentEvent::Abandoned {
                        key,
                        session_id,
                        generation,
                    } => {
                        steps.push(ForwarderStep::Abandoned { generation });
                        ingress
                            .send(predecessor_stream_gap_event(&key, &session_id, generation))
                            .await
                            .expect("console ingress open");
                        runtime
                            .drain_mob_agent_events()
                            .await
                            .expect("drain member events");
                    }
                    ForwardedAgentEvent::Closed(key, generation) => {
                        let served = self.tracked.close(&key, generation);
                        if served {
                            self.subscribe_failures.remove(&key);
                        }
                        steps.push(ForwarderStep::Closed { generation, served });
                        self.reconcile(&handle, &None).await;
                    }
                }
            }
            steps
        }
    }

    #[derive(Debug)]
    enum ForwarderStep {
        Event(Box<meerkat_core::event::EventEnvelope<AgentEvent>>),
        Closed { generation: u64, served: bool },
        Abandoned { generation: u64 },
    }

    fn restart_member_key(
        entry: &meerkat_mob::runtime::MobMemberListEntry,
        mob_id: &str,
    ) -> TrackedAgentEventStream {
        let (runtime_id, fence_token) = entry.binding_atoms().expect("member binding atoms");
        TrackedAgentEventStream {
            mob_id: mob_id.to_string(),
            durable_identity: durable_identity_label(&entry.labels),
            member_identity: entry.agent_identity.clone(),
            runtime_id,
            fence_token,
        }
    }

    async fn restart_member_entry(handle: &MobHandle) -> meerkat_mob::runtime::MobMemberListEntry {
        let mut entries = handle.list_members_including_retiring().await;
        assert_eq!(entries.len(), 1, "one durable member");
        entries.remove(0)
    }

    /// One run's console frames for `session_id`, in timeline order, with
    /// the typed prompt of its `run_started`.
    async fn session_timeline(
        runtime: &UnifiedRuntime,
        session_id: &meerkat_core::types::SessionId,
    ) -> Vec<crate::console_contracts::ConsoleIdentityEventEnvelope> {
        runtime
            .console_events()
            .replay_all(None)
            .await
            .expect("console replay")
            .into_iter()
            .filter(|frame| frame.data.get("session_id") == Some(&json!(session_id)))
            .collect()
    }

    fn run_started_prompt(
        frame: &crate::console_contracts::ConsoleIdentityEventEnvelope,
    ) -> Option<String> {
        frame
            .data
            .get("input")
            .cloned()
            .and_then(|input| serde_json::from_value::<meerkat_core::types::RunInput>(input).ok())
            .and_then(|input| input.prompt_text())
    }

    fn assert_one_attributed_run(
        frames: &[crate::console_contracts::ConsoleIdentityEventEnvelope],
        prompt: &str,
    ) {
        let types: Vec<&str> = frames
            .iter()
            .map(|frame| frame.event_type.as_str())
            .collect();
        assert_eq!(types.first(), Some(&"run_started"), "timeline: {types:?}");
        assert_eq!(types.get(1), Some(&"turn_started"), "timeline: {types:?}");
        assert_eq!(
            types.last(),
            Some(&"interaction_complete"),
            "timeline: {types:?}"
        );
        assert_eq!(
            types.iter().filter(|kind| **kind == "run_started").count(),
            1,
            "one run: {types:?}"
        );
        assert_eq!(run_started_prompt(&frames[0]).as_deref(), Some(prompt));
        let sequences: Vec<u64> = frames
            .iter()
            .map(|frame| {
                frame
                    .data
                    .get("source_sequence")
                    .and_then(serde_json::Value::as_u64)
                    .expect("source sequence")
            })
            .collect();
        let first = sequences.first().copied().expect("frames carry sequences");
        assert_eq!(
            sequences,
            (first..first + frames.len() as u64).collect::<Vec<_>>(),
            "meerkat's own contiguous sequence for the run, none missing or repeated"
        );
        let run_id = frames[0]
            .data
            .get("run_id")
            .filter(|run_id| !run_id.is_null())
            .expect("run_started carries its typed run id")
            .clone();
        for frame in frames {
            if let Some(frame_run) = frame.data.get("run_id") {
                assert_eq!(
                    frame_run, &run_id,
                    "{} is attributed to the run it belongs to",
                    frame.event_type
                );
            }
        }
        for kind in ["turn_started", "interaction_complete"] {
            let frame = frames
                .iter()
                .find(|frame| frame.event_type == kind)
                .expect("frame present");
            assert_eq!(frame.data.get("run_id"), Some(&run_id), "{kind} lineage");
        }
    }

    /// The reported restore path with its failing schedule forced: after a
    /// restart the persisted member is listed Active with its restored
    /// binding while no actor serves its session, so the forwarder's
    /// subscription fails with "stream not found" and the member is parked
    /// in backoff; meerkat reports the missing actor as the typed
    /// `MemberSessionNotLive`. The restore then revives the actor, and its
    /// first run completes before the forwarder attaches again. When the
    /// backoff lapses the forwarder's subscription replays the actor's
    /// retained events, and the console timeline carries that run exactly:
    /// `run_started` (seq 1, its own prompt), `turn_started` (seq 2),
    /// through its terminal, all with the run's typed lineage.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn restored_members_first_run_reaches_the_console_after_stream_not_found_backoff() {
        let mob_id = "console-replay-restore-backoff";
        let temp = tempfile::tempdir().expect("temp dir");
        let mob_path = temp.path().join("mob.sqlite3");
        let state_root = temp.path().join("state");
        let (first, first_identity) =
            boot_identity_first_persistent(mob_id, &mob_path, &state_root).await;
        let session_id = commit_restart_member_turn(&first_identity, "before restart").await;
        first.shutdown().await;

        let boot = boot_persistent_before_activation(mob_id, &mob_path, &state_root).await;
        // The test takes over the forwarder's steps; the runtime's own
        // forwarder is stopped and the console drain is fed from here.
        let ingress = boot.runtime.install_test_event_ingress().await;
        let handle = boot.runtime.mob_handle();
        let mut forwarder = DrivenForwarder::new();

        let restored = restart_member_entry(&handle).await;
        assert_eq!(restored.status, MobMemberStatus::Active);
        let key = restart_member_key(&restored, mob_id);
        let not_live = match handle
            .subscribe_agent_events(&restored.agent_identity)
            .await
        {
            Ok(_) => panic!("no actor serves the restored member's session yet"),
            Err(error) => error,
        };
        assert!(
            matches!(
                &not_live,
                meerkat_mob::MobError::MemberSessionNotLive { session_id: missing, .. }
                    if missing == &session_id
            ),
            "{not_live:?}"
        );
        forwarder.reconcile(&handle, &None).await;
        assert!(
            forwarder.tracked.current.is_empty(),
            "no actor to attach to yet"
        );
        // The restored mob stays Stopped until activation resumes it, so the
        // failed subscription parks the member: no retry deadline, no count.
        assert_eq!(handle.status_observation_snapshot(), MobState::Stopped);
        let backoff = forwarder
            .subscribe_failures
            .get_mut(&key)
            .expect("the failed subscription parked the member");
        assert_eq!(
            (backoff.next_attempt, backoff.consecutive_failures),
            (None, 0)
        );
        // Keep the member parked until its run finished unobserved,
        // whatever the scheduling.
        backoff.next_attempt = Some(tokio::time::Instant::now() + Duration::from_hours(1));

        let identity_runtime = boot.identity_runtime.clone();
        let runtime = boot.activate().await;
        assert_eq!(
            restart_member_key(&restart_member_entry(&handle).await, mob_id),
            key,
            "the Resume route kept the restored binding"
        );
        let revived = commit_restart_member_turn(&identity_runtime, "after restart").await;
        assert_eq!(revived, session_id);
        assert!(
            forwarder.tracked.current.is_empty(),
            "the run finished unobserved"
        );

        forwarder.reconcile(&handle, &None).await;
        assert!(
            forwarder.tracked.current.is_empty(),
            "the pending backoff still parks the member"
        );
        // The backoff lapses.
        forwarder
            .subscribe_failures
            .get_mut(&key)
            .expect("parked member")
            .next_attempt = Some(tokio::time::Instant::now());
        forwarder.reconcile(&handle, &None).await;
        assert!(
            forwarder
                .tracked
                .current
                .get(&key)
                .is_some_and(|attachment| attachment.actor.is_some()),
            "the late subscription attached to the revived actor"
        );
        assert!(forwarder.subscribe_failures.is_empty());

        forwarder.forward_terminals(&runtime, &ingress, 1).await;
        assert_one_attributed_run(
            &session_timeline(&runtime, &session_id).await,
            "after restart",
        );

        runtime.shutdown().await;
    }

    /// Stage resume outcomes on the pending activation a restart left, ahead
    /// of the real `MobHandle::resume`.
    async fn inject_activation_outcomes(
        runtime: &UnifiedRuntime,
        max_consecutive_stalls: u32,
        outcomes: Vec<meerkat_mob::MobError>,
    ) {
        let mut slot = runtime.pending_mob_activation.lock().await;
        let pending = slot
            .as_mut()
            .expect("restarting a Stopped persistent mob stages an activation");
        pending.set_stall_policy_for_test(
            crate::mob_activation_retry::ActivationStallPolicy::with_max_consecutive_stalls(
                max_consecutive_stalls,
            ),
        );
        pending.inject_resume_outcomes_for_test(outcomes);
    }

    fn restart_member_activation_stall() -> meerkat_mob::MobError {
        meerkat_mob::MobError::LifecycleOperationProgressStalled {
            intent: "explicit_resume".to_string(),
            member_id: Some(meerkat_mob::AgentIdentity::from(RESTART_MEMBER)),
            stage: "resume_member",
        }
    }

    /// Boot, commit a turn, shut down, and boot again up to the activation.
    async fn restart_before_activation(
        mob_id: &str,
        temp: &tempfile::TempDir,
    ) -> (
        PersistentBootBeforeActivation,
        meerkat_core::types::SessionId,
    ) {
        let mob_path = temp.path().join("mob.sqlite3");
        let state_root = temp.path().join("state");
        let (first, first_identity) =
            boot_identity_first_persistent(mob_id, &mob_path, &state_root).await;
        let session_id = commit_restart_member_turn(&first_identity, "before restart").await;
        first.shutdown().await;
        (
            boot_persistent_before_activation(mob_id, &mob_path, &state_root).await,
            session_id,
        )
    }

    /// Wait (bounded) until the mob's native lifecycle owner has closed its
    /// admission.
    async fn admission_closes(
        mut closed: tokio::sync::watch::Receiver<Option<meerkat_mob::MobAdmissionCloseReason>>,
    ) -> bool {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while closed.borrow_and_update().is_none() {
                if closed.changed().await.is_err() {
                    return;
                }
            }
        })
        .await
        .is_ok()
            && closed.borrow().is_some()
    }

    /// D67: a stopped resume whose staged activation is never consumed (the
    /// identity roster failed before the lift, so the runtime is dropped
    /// with the activation still in its state) does not keep the otherwise
    /// idle mob publicly owned: dropping the runtime lets the native owner
    /// close its admission instead of waiting forever.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_unconsumed_stopped_resume_activation_lets_the_idle_mob_close() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (boot, _) = restart_before_activation("activation-never-consumed", &temp).await;
        assert!(
            boot.runtime.pending_mob_activation.lock().await.is_some(),
            "the restart staged an activation"
        );
        let closed = boot.runtime.mob_handle().observer().admission_closed();
        drop(boot);
        assert!(
            admission_closes(closed).await,
            "the dropped runtime's idle mob closes despite its unconsumed activation"
        );
    }

    /// D67: the exported identity-first context is a public owner. Kept
    /// across dropping the runtime, it keeps the mob owned and stays usable
    /// (a tracked operation runs through it); releasing it lets the idle mob
    /// close.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_retained_identity_first_context_export_keeps_the_mob_owned() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime) = boot_identity_first_persistent(
            "retained-context-export",
            &temp.path().join("mob.sqlite3"),
            &temp.path().join("state"),
        )
        .await;
        let exported = runtime
            .identity_first_context()
            .expect("identity-first context export");
        let closed = runtime.mob_handle().observer().admission_closed();
        drop(identity_runtime);
        drop(runtime);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        assert!(
            closed.borrow().is_none(),
            "the retained export keeps the mob publicly owned"
        );
        exported
            .refresh_desired_topology_tracked()
            .await
            .expect("the retained export stays usable");
        drop(exported);
        assert!(
            admission_closes(closed).await,
            "releasing the export lets the idle mob close"
        );
    }

    /// A reviewed attachment control for exactly one server that allows its
    /// live apply and refuses persistence (the requests below are live-only).
    struct LiveOnlyControl(meerkat_core::mcp_config::McpServerConfig);

    impl meerkat_core::mcp_attachment::McpAttachmentAuthority for LiveOnlyControl {
        fn server(&self) -> &meerkat_core::mcp_config::McpServerConfig {
            &self.0
        }

        fn authorize_apply(
            &self,
        ) -> Result<(), meerkat_core::mcp_attachment::McpAttachmentRefusal> {
            Ok(())
        }

        fn authorize_persist(
            &self,
            _scope: meerkat_core::mcp_config::McpScope,
        ) -> Result<(), meerkat_core::mcp_attachment::McpAttachmentRefusal> {
            Err(meerkat_core::mcp_attachment::McpAttachmentRefusal::Denied)
        }
    }

    fn live_only_attachment() -> meerkat_core::mcp_attachment::LiveMcpAttachment {
        live_only_attachment_of(meerkat_core::mcp_config::McpServerConfig::stdio(
            "member-attached",
            "/bin/true",
            Vec::new(),
            std::collections::HashMap::new(),
        ))
    }

    fn live_only_attachment_of(
        server: meerkat_core::mcp_config::McpServerConfig,
    ) -> meerkat_core::mcp_attachment::LiveMcpAttachment {
        meerkat_core::mcp_attachment::LiveMcpAttachment {
            control: Arc::new(LiveOnlyControl(server.clone())),
            server,
            persist: None,
        }
    }

    /// Ask a non-waiting attach entry until it answers anything but `Busy`.
    ///
    /// The entry answers `Busy` while anyone holds the session's turn
    /// boundary, and a committed turn does not prove that nobody does: the
    /// post-commit checkpoint and every observation read take it too.
    /// `Busy` grants nothing and leaves the decision to the caller's owner;
    /// this owner waits on the boundary itself, never on a timer, and asks
    /// again, within the structural backstop.
    async fn attach_between_turns<F, Fut>(
        sessions: &dyn meerkat_mob::MobSessionService,
        session_id: &meerkat_core::types::SessionId,
        mut attempt: F,
    ) -> Result<
        meerkat_core::mcp_attachment::LiveMcpAttachmentReceipt,
        meerkat_core::mcp_attachment::LiveMcpAttachError,
    >
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<
                Output = Result<
                    meerkat_core::mcp_attachment::LiveMcpAttachmentReceipt,
                    meerkat_core::mcp_attachment::LiveMcpAttachError,
                >,
            >,
    {
        let deadline = tokio::time::Instant::now() + crate::test_wait::STRUCTURAL_BACKSTOP;
        loop {
            let answer = attempt().await;
            if !matches!(
                answer,
                Err(meerkat_core::mcp_attachment::LiveMcpAttachError::Busy)
            ) {
                return answer;
            }
            match tokio::time::timeout_at(
                deadline,
                sessions.acquire_runtime_turn_finalization_guard(session_id),
            )
            .await
            {
                Ok(guard) => drop(guard.expect("the session's turn boundary")),
                Err(_) => return answer,
            }
        }
    }

    /// The member's live MCP attachment reaches its actual session owner's
    /// non-waiting entry. The owner refuses a session with no live actor as
    /// not current (its own answer, not the trait default), answers `Busy`
    /// at once while the session's turn boundary is held, and a live
    /// ungoverned member (no governed construction built it a router) has
    /// no attachment surface. Through the public identity entry an unknown
    /// identity is not current and the member gets its owner's answer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_member_live_mcp_attachment_reaches_its_session_owner() {
        use meerkat_core::mcp_attachment::{LiveMcpAttachError, McpAttachmentRefusal};
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime) = boot_identity_first_persistent(
            "member-live-mcp",
            &temp.path().join("mob.sqlite3"),
            &temp.path().join("state"),
        )
        .await;
        let session_id = commit_restart_member_turn(&identity_runtime, "before attach").await;
        let owner = runtime
            .mob_runtime()
            .controller_pin_source()
            .expect("the persistent runtime records its session owner");
        let sessions = Arc::clone(
            runtime
                .mob_runtime()
                .session_service()
                .expect("the persistent runtime has a session service"),
        );

        assert_eq!(
            owner
                .try_attach_live_mcp_server(
                    &meerkat_core::types::SessionId::new(),
                    live_only_attachment(),
                )
                .await
                .map(|receipt| receipt.outcome),
            Err(LiveMcpAttachError::Refused(
                McpAttachmentRefusal::NotCurrent
            )),
            "a session without a live actor is the owner's not-current refusal"
        );
        // The entry never waits: under a held turn boundary it answers
        // `Busy` at once and attaches nothing.
        let held = sessions
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await
            .expect("the session's turn boundary");
        assert_eq!(
            owner
                .try_attach_live_mcp_server(&session_id, live_only_attachment())
                .await
                .map(|receipt| receipt.outcome),
            Err(LiveMcpAttachError::Busy),
            "a held turn boundary answers Busy at once"
        );
        drop(held);
        let owner_entry = owner.as_ref();
        let session = &session_id;
        assert_eq!(
            attach_between_turns(sessions.as_ref(), session, move || {
                owner_entry.try_attach_live_mcp_server(session, live_only_attachment())
            })
            .await
            .map(|receipt| receipt.outcome),
            Err(LiveMcpAttachError::Unsupported),
            "the live ungoverned member's actor has no attachment surface"
        );

        let view = runtime.identity_runtime().expect("identity runtime export");
        let unknown =
            crate::identity_first::AgentIdentity::parse("unknown:live-mcp").expect("identity");
        assert_eq!(
            view.attach_live_mcp_server(&unknown, live_only_attachment())
                .await
                .map(|receipt| receipt.outcome),
            Err(LiveMcpAttachError::Refused(
                McpAttachmentRefusal::NotCurrent
            ))
        );
        let member = crate::identity_first::AgentIdentity::parse(RESTART_MEMBER).expect("identity");
        let (view_entry, member_identity) = (&view, &member);
        assert_eq!(
            attach_between_turns(sessions.as_ref(), session, move || {
                view_entry.attach_live_mcp_server(member_identity, live_only_attachment())
            })
            .await
            .map(|receipt| receipt.outcome),
            Err(LiveMcpAttachError::Unsupported)
        );
    }

    /// Always the same readiness, for the identity-bound attempt below.
    struct FixedReadiness(meerkat_core::mcp_attachment::McpAttachmentReadiness);

    #[async_trait::async_trait]
    impl meerkat_core::mcp_attachment::McpAttachmentAttemptObserver for FixedReadiness {
        async fn observe(
            &self,
            _tool: &str,
        ) -> meerkat_core::mcp_attachment::McpAttachmentReadiness {
            self.0.clone()
        }

        async fn wait(
            &self,
            _tool: &str,
            _timeout: Duration,
        ) -> meerkat_core::mcp_attachment::McpAttachmentReadiness {
            self.0.clone()
        }
    }

    /// The public entry's attempt answers only while its identity is still
    /// on the attached session: another session, an unknown identity or a
    /// released identity runtime is not current.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_identity_bound_attempt_answers_only_for_the_attached_session() {
        use crate::identity_first::IdentityBoundAttempt;
        use meerkat_core::mcp_attachment::{McpAttachmentAttemptObserver, McpAttachmentReadiness};
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime) = boot_identity_first_persistent(
            "identity-bound-attempt",
            &temp.path().join("mob.sqlite3"),
            &temp.path().join("state"),
        )
        .await;
        let session_id = commit_restart_member_turn(&identity_runtime, "attached").await;
        let member = crate::identity_first::AgentIdentity::parse(RESTART_MEMBER).expect("identity");
        let usable = McpAttachmentReadiness::Usable {
            connection_generation: 3,
        };
        let bound = |runtime: std::sync::Weak<crate::identity_first::IdentityRuntime>,
                     identity: crate::identity_first::AgentIdentity,
                     session: meerkat_core::types::SessionId| {
            IdentityBoundAttempt::new(
                runtime,
                identity,
                session,
                Arc::new(FixedReadiness(usable.clone())),
            )
        };
        let weak = Arc::downgrade(&identity_runtime);

        let attached = bound(weak.clone(), member.clone(), session_id.clone());
        assert_eq!(attached.observe("echo").await, usable);
        // The wait's two identity checks read the identity table, which can
        // take more than a few milliseconds on a busy host. This row is about
        // currency, so its budget is one those reads cannot exhaust; the wait
        // still answers as soon as they finish. The held-table row below pins
        // the deadline itself.
        assert_eq!(attached.wait("echo", Duration::from_secs(30)).await, usable);
        assert_eq!(
            bound(
                weak.clone(),
                member.clone(),
                meerkat_core::types::SessionId::new()
            )
            .observe("echo")
            .await,
            McpAttachmentReadiness::NotCurrent
        );
        let unknown =
            crate::identity_first::AgentIdentity::parse("unknown:bound").expect("identity");
        assert_eq!(
            bound(weak, unknown, session_id.clone())
                .observe("echo")
                .await,
            McpAttachmentReadiness::NotCurrent
        );
        assert_eq!(
            bound(std::sync::Weak::new(), member, session_id)
                .observe("echo")
                .await,
            McpAttachmentReadiness::NotCurrent
        );
        drop(runtime);
    }

    /// Answers Usable from wait() only after the identity table is held, so
    /// the identity check that follows it cannot complete in time.
    struct UsableAfterHolding {
        runtime: Arc<crate::identity_first::IdentityRuntime>,
        release: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::mcp_attachment::McpAttachmentAttemptObserver for UsableAfterHolding {
        async fn observe(
            &self,
            _tool: &str,
        ) -> meerkat_core::mcp_attachment::McpAttachmentReadiness {
            meerkat_core::mcp_attachment::McpAttachmentReadiness::Usable {
                connection_generation: 3,
            }
        }

        async fn wait(
            &self,
            _tool: &str,
            _timeout: Duration,
        ) -> meerkat_core::mcp_attachment::McpAttachmentReadiness {
            let release = self
                .release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("one wait");
            let (acquired_tx, acquired) = tokio::sync::oneshot::channel();
            let runtime = Arc::clone(&self.runtime);
            tokio::spawn(async move {
                runtime
                    .hold_identity_table_until(acquired_tx, release)
                    .await;
            });
            acquired.await.expect("the table is held");
            meerkat_core::mcp_attachment::McpAttachmentReadiness::Usable {
                connection_generation: 3,
            }
        }
    }

    /// One deadline bounds the identity-bound wait, its identity checks
    /// included: with the identity table held, a short wait returns on time
    /// and never Usable, whether the table is held before the native wait
    /// or only for the check after it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_identity_bound_wait_returns_on_time_without_usable_while_identity_is_held() {
        use crate::identity_first::IdentityBoundAttempt;
        use meerkat_core::mcp_attachment::{McpAttachmentAttemptObserver, McpAttachmentReadiness};
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime) = boot_identity_first_persistent(
            "identity-bound-deadline",
            &temp.path().join("mob.sqlite3"),
            &temp.path().join("state"),
        )
        .await;
        let session_id = commit_restart_member_turn(&identity_runtime, "attached").await;
        let member = crate::identity_first::AgentIdentity::parse(RESTART_MEMBER).expect("identity");

        // Held before the wait: the precheck cannot complete.
        let (acquired_tx, acquired) = tokio::sync::oneshot::channel();
        let (release_tx, release) = tokio::sync::oneshot::channel();
        let holder = Arc::clone(&identity_runtime);
        let held = tokio::spawn(async move {
            holder.hold_identity_table_until(acquired_tx, release).await;
        });
        acquired.await.expect("the table is held");
        let bound = IdentityBoundAttempt::new(
            Arc::downgrade(&identity_runtime),
            member.clone(),
            session_id.clone(),
            Arc::new(FixedReadiness(McpAttachmentReadiness::Usable {
                connection_generation: 3,
            })),
        );
        let answer = tokio::time::timeout(
            Duration::from_secs(5),
            bound.wait("echo", Duration::from_millis(200)),
        )
        .await
        .expect("the wait returns at its own deadline");
        assert_eq!(answer, McpAttachmentReadiness::Pending);
        let _ = release_tx.send(());
        held.await.expect("released");

        // Held only after the native wait: the final check cannot complete,
        // and the native Usable does not get through.
        let (release_tx, release) = tokio::sync::oneshot::channel();
        let bound = IdentityBoundAttempt::new(
            Arc::downgrade(&identity_runtime),
            member,
            session_id,
            Arc::new(UsableAfterHolding {
                runtime: Arc::clone(&identity_runtime),
                release: std::sync::Mutex::new(Some(release)),
            }),
        );
        let answer = tokio::time::timeout(
            Duration::from_secs(5),
            bound.wait("echo", Duration::from_millis(200)),
        )
        .await
        .expect("the wait returns at its own deadline");
        assert_eq!(answer, McpAttachmentReadiness::Pending);
        let _ = release_tx.send(());
        drop(runtime);
    }

    fn diagnosed_setup_failure() -> meerkat_core::mcp_attachment::McpStdioSetupFailure {
        meerkat_core::mcp_attachment::McpStdioSetupFailure::new(
            "tools",
            3,
            meerkat_core::mcp_attachment::McpStdioSetupPhase::Initialize,
            meerkat_core::mcp_attachment::McpStdioSetupFailureClass::Closed { exit: None },
            true,
        )
    }

    fn diagnosed_stderr_tail() -> meerkat_core::mcp_attachment::McpStdioStderrTail {
        meerkat_core::mcp_attachment::McpStdioStderrTail::new(
            "server stderr\n".to_string(),
            14,
            false,
            true,
        )
    }

    /// An attempt that failed its stdio setup: always the same typed failure
    /// and stderr tail. With `leaving`, that identity leaves the identity
    /// runtime while either diagnostic is read.
    struct DiagnosedFailure {
        leaving: Option<(
            Arc<crate::identity_first::IdentityRuntime>,
            crate::identity_first::AgentIdentity,
        )>,
    }

    impl DiagnosedFailure {
        fn readiness() -> meerkat_core::mcp_attachment::McpAttachmentReadiness {
            meerkat_core::mcp_attachment::McpAttachmentReadiness::Failed {
                detail: "the server closed during setup".to_string(),
            }
        }

        async fn read(&self) {
            if let Some((runtime, identity)) = &self.leaving {
                let _ = runtime.remove_unrecorded_for_test(identity).await;
            }
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::mcp_attachment::McpAttachmentAttemptObserver for DiagnosedFailure {
        async fn observe(
            &self,
            _tool: &str,
        ) -> meerkat_core::mcp_attachment::McpAttachmentReadiness {
            Self::readiness()
        }

        async fn wait(
            &self,
            _tool: &str,
            _timeout: Duration,
        ) -> meerkat_core::mcp_attachment::McpAttachmentReadiness {
            Self::readiness()
        }

        async fn setup_failure(
            &self,
        ) -> Option<meerkat_core::mcp_attachment::McpStdioSetupFailure> {
            self.read().await;
            Some(diagnosed_setup_failure())
        }

        async fn stderr_tail(&self) -> Option<meerkat_core::mcp_attachment::McpStdioStderrTail> {
            self.read().await;
            Some(diagnosed_stderr_tail())
        }
    }

    /// The public entry's attempt forwards its stdio setup diagnostics, the
    /// typed failure and the host-only stderr tail, only while its identity
    /// is still on the attached session: another session, an unknown
    /// identity or a released identity runtime gets `None`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_identity_bound_attempt_forwards_setup_diagnostics_only_for_the_attached_session() {
        use crate::identity_first::IdentityBoundAttempt;
        use meerkat_core::mcp_attachment::McpAttachmentAttemptObserver;
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime) = boot_identity_first_persistent(
            "identity-bound-diagnostics",
            &temp.path().join("mob.sqlite3"),
            &temp.path().join("state"),
        )
        .await;
        let session_id = commit_restart_member_turn(&identity_runtime, "attached").await;
        let member = crate::identity_first::AgentIdentity::parse(RESTART_MEMBER).expect("identity");
        let bound = |runtime: std::sync::Weak<crate::identity_first::IdentityRuntime>,
                     identity: crate::identity_first::AgentIdentity,
                     session: meerkat_core::types::SessionId| {
            IdentityBoundAttempt::new(
                runtime,
                identity,
                session,
                Arc::new(DiagnosedFailure { leaving: None }),
            )
        };
        let weak = Arc::downgrade(&identity_runtime);

        let attached = bound(weak.clone(), member.clone(), session_id.clone());
        assert_eq!(
            attached.setup_failure().await,
            Some(diagnosed_setup_failure())
        );
        assert_eq!(attached.stderr_tail().await, Some(diagnosed_stderr_tail()));

        let unknown =
            crate::identity_first::AgentIdentity::parse("unknown:diagnostics").expect("identity");
        for stale in [
            bound(
                weak.clone(),
                member.clone(),
                meerkat_core::types::SessionId::new(),
            ),
            bound(weak, unknown, session_id.clone()),
            bound(std::sync::Weak::new(), member, session_id),
        ] {
            assert_eq!(stale.setup_failure().await, None);
            assert_eq!(stale.stderr_tail().await, None);
        }
        drop(runtime);
    }

    /// An identity that leaves its runtime while a diagnostic is read gets
    /// `None`: the identity check after the read catches it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_identity_leaving_during_a_diagnostic_read_gets_none() {
        use crate::identity_first::IdentityBoundAttempt;
        use meerkat_core::mcp_attachment::McpAttachmentAttemptObserver;
        for read_tail in [false, true] {
            let temp = tempfile::tempdir().expect("temp dir");
            let (runtime, identity_runtime) = boot_identity_first_persistent(
                if read_tail {
                    "identity-leaves-tail-read"
                } else {
                    "identity-leaves-failure-read"
                },
                &temp.path().join("mob.sqlite3"),
                &temp.path().join("state"),
            )
            .await;
            let session_id = commit_restart_member_turn(&identity_runtime, "attached").await;
            let member =
                crate::identity_first::AgentIdentity::parse(RESTART_MEMBER).expect("identity");
            let leaving = IdentityBoundAttempt::new(
                Arc::downgrade(&identity_runtime),
                member.clone(),
                session_id,
                Arc::new(DiagnosedFailure {
                    leaving: Some((Arc::clone(&identity_runtime), member.clone())),
                }),
            );
            if read_tail {
                assert_eq!(leaving.stderr_tail().await, None);
            } else {
                assert_eq!(leaving.setup_failure().await, None);
            }
            assert!(
                !identity_runtime.contains(&member).await,
                "the identity left during the read"
            );
            drop(runtime);
        }
    }

    /// What the failing stdio server below writes to its stderr: it reaches
    /// the host in the attempt's tail and nothing else.
    const STDERR_SENTINEL: &str = "META-STDERR-SENTINEL-4b9d";

    /// A stdio MCP server that writes [`STDERR_SENTINEL`] to its stderr and
    /// exits before it answers `initialize`.
    fn exits_during_initialize(name: &str) -> meerkat_core::mcp_config::McpServerConfig {
        meerkat_core::mcp_config::McpServerConfig::stdio(
            name,
            "/bin/sh",
            vec![
                "-c".to_string(),
                format!("printf 'setup failed: %s\\n' '{STDERR_SENTINEL}' >&2\nexit 3\n"),
            ],
            std::collections::HashMap::new(),
        )
    }

    /// A governed identity's live stdio server that exits during
    /// `initialize` reports its typed setup failure and its stderr tail to
    /// the host through the public identity entry's attempt. The tail stays
    /// with the host: the receipt, the readiness, the failure and the tail's
    /// own `Debug` never show the server's stderr, nor does any event of the
    /// member or the mob. A retired identity's attempt forwards neither.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_identity_attempt_reports_its_failed_stdio_setup_to_the_host_only() {
        use meerkat_core::mcp_attachment::{
            McpAttachmentReadiness, McpStdioSetupFailureClass, McpStdioSetupPhase,
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let mut runtime =
            super::held_identity_test_support::governed_runtime(&dir, "identity-mcp-diagnostics")
                .await;
        let identity =
            crate::identity_first::AgentIdentity::parse("mcp-diagnostics").expect("identity");
        let roster = vec![super::held_identity_test_support::member(&identity)];
        let sessions = Arc::clone(
            runtime
                .mob_runtime()
                .session_service()
                .expect("the governed runtime has a session service"),
        );
        // As the gateways wire it: the identity bridge reaches the session
        // owner's live attachment entry.
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()
                        .expect("continuity store"),
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "identity-mcp-diagnostics".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(
                    crate::identity_first::MobSessionBridge::with_session_service(
                        crate::gateway_wiring::identity_mob_handle(&runtime),
                        Arc::clone(&sessions),
                    )
                    .with_controller_pin_source(
                        runtime
                            .mob_runtime()
                            .controller_pin_source()
                            .expect("the governed runtime records its session owner"),
                    ),
                )),
                default_timeout: None,
            },
        ));
        super::held_identity_test_support::install_roster(&mut runtime, identity_runtime, &roster);
        runtime
            .bootstrap_installed_identity_first_context(&roster)
            .await
            .expect("the governed member is created");
        let view = runtime.identity_runtime().expect("identity runtime export");
        let session_id = view
            .status(&identity)
            .await
            .expect("identity status")
            .session_id
            .expect("the member's session");
        let mut mob_events = runtime.subscribe_mob_events();

        let (view_entry, member) = (&view, &identity);
        let receipt = attach_between_turns(sessions.as_ref(), &session_id, move || {
            view_entry.attach_live_mcp_server(
                member,
                live_only_attachment_of(exits_during_initialize("exits-during-initialize")),
            )
        })
        .await
        .expect("the governed member attaches");
        let readiness = receipt.attempt.wait("any", Duration::from_secs(40)).await;
        assert!(
            matches!(readiness, McpAttachmentReadiness::Failed { .. }),
            "{readiness:?}"
        );
        let failure = receipt
            .attempt
            .setup_failure()
            .await
            .expect("the attempt's typed setup failure reaches the host");
        assert_eq!(failure.server, "exits-during-initialize");
        assert_eq!(failure.phase, McpStdioSetupPhase::Initialize);
        assert!(
            matches!(failure.class, McpStdioSetupFailureClass::Closed { .. }),
            "{failure:?}"
        );
        assert!(failure.stderr_tail_retained, "{failure:?}");
        // The drain of the server's stderr can trail the attempt's failure.
        let tail = tokio::time::timeout(crate::test_wait::STRUCTURAL_BACKSTOP, async {
            loop {
                if let Some(tail) = receipt.attempt.stderr_tail().await
                    && tail.text.contains(STDERR_SENTINEL)
                {
                    return tail;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the attempt's stderr tail reaches the host");

        let mut rendered = vec![
            format!("{receipt:?}"),
            format!("{readiness:?}"),
            format!("{:?}", receipt.attempt.observe("any").await),
            format!("{failure:?}"),
            format!("{tail:?}"),
        ];
        let mut member_events = runtime
            .mob_handle()
            .subscribe_agent_events_from(
                &crate::member_comms_id::mob_member_id(identity.as_str()),
                meerkat_core::comms::SessionEventCursor::Earliest,
            )
            .await
            .expect("the member's retained events")
            .stream;
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(500), member_events.next()).await
        {
            rendered.push(serde_json::to_string(&event).expect("member event json"));
            rendered.push(format!("{event:?}"));
        }
        loop {
            match mob_events.try_recv() {
                Ok(event) => {
                    rendered.push(serde_json::to_string(&event).expect("mob event json"));
                    rendered.push(format!("{event:?}"));
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
        for event in runtime
            .query_mob_events(&EventQuery::default())
            .await
            .expect("the mob event ledger")
        {
            rendered.push(serde_json::to_string(&event).expect("ledger event json"));
        }
        for text in &rendered {
            assert!(
                !text.contains(STDERR_SENTINEL),
                "the server's stderr escaped the host-only tail: {text}"
            );
        }

        view.retire(&identity).await.expect("the identity retires");
        assert_eq!(receipt.attempt.setup_failure().await, None);
        assert_eq!(receipt.attempt.stderr_tail().await, None);
        drop(view);
        let _ = runtime.shutdown().await;
    }

    /// A session owner that records each live attachment and answers
    /// `Busy`, the non-waiting refusal of a member in its turn.
    #[derive(Default)]
    struct BusyAttachOwner(std::sync::Mutex<Vec<(meerkat_core::types::SessionId, String)>>);

    #[async_trait::async_trait]
    impl crate::identity_first::bridge::ControllerPinSource for BusyAttachOwner {
        async fn live_actor(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Option<meerkat_session::LiveSessionActorWitness> {
            None
        }

        async fn pin_actor(
            &self,
            _actor: &meerkat_session::LiveSessionActorWitness,
        ) -> Result<meerkat_core::ControllerModelClient, String> {
            Err("not pinned here".to_string())
        }

        async fn try_attach_live_mcp_server(
            &self,
            session_id: &meerkat_core::types::SessionId,
            attachment: meerkat_core::mcp_attachment::LiveMcpAttachment,
        ) -> Result<
            meerkat_core::mcp_attachment::LiveMcpAttachmentReceipt,
            meerkat_core::mcp_attachment::LiveMcpAttachError,
        > {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((session_id.clone(), attachment.server.name));
            Err(meerkat_core::mcp_attachment::LiveMcpAttachError::Busy)
        }
    }

    /// The bridge hands the exact session and attachment to its session
    /// owner and returns the owner's answer unchanged (`Busy` here: retry
    /// later); a bridge without a session owner is unsupported.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_bridge_forwards_a_live_mcp_attachment_to_its_session_owner() {
        use crate::identity_first::bridge::SessionBridge as _;
        use meerkat_core::mcp_attachment::LiveMcpAttachError;
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, _identity_runtime) = boot_identity_first_persistent(
            "member-live-mcp-bridge",
            &temp.path().join("mob.sqlite3"),
            &temp.path().join("state"),
        )
        .await;
        let owner = Arc::new(BusyAttachOwner::default());
        let bridge = crate::identity_first::MobSessionBridge::new(runtime.mob_handle().observer())
            .with_controller_pin_source(owner.clone());
        let session_id = meerkat_core::types::SessionId::new();
        assert_eq!(
            bridge
                .attach_live_mcp_server(&session_id, live_only_attachment())
                .await
                .map(|receipt| receipt.outcome),
            Err(LiveMcpAttachError::Busy)
        );
        assert_eq!(
            *owner
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [(session_id.clone(), "member-attached".to_string())]
        );
        let ownerless =
            crate::identity_first::MobSessionBridge::new(runtime.mob_handle().observer());
        assert_eq!(
            ownerless
                .attach_live_mcp_server(&session_id, live_only_attachment())
                .await
                .map(|receipt| receipt.outcome),
            Err(LiveMcpAttachError::Unsupported)
        );
    }

    /// A binder that must never be reached: the sends below are refused
    /// before any binding.
    struct UnreachedBinder(std::sync::atomic::AtomicUsize);

    #[async_trait::async_trait]
    impl crate::identity_first::bridge::GovernedInputBinder for UnreachedBinder {
        async fn bind(
            &self,
            _context: &crate::identity_first::bridge::GovernedBindContext,
            _input: meerkat_runtime::Input,
        ) -> Result<crate::identity_first::bridge::BoundGovernedInput, String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("never bound".to_string())
        }
        fn retain_unresolved(
            &self,
            _settlement: Box<dyn crate::identity_first::bridge::GovernedSettlement>,
            _detail: &str,
        ) {
        }
    }

    /// D67: an identity runtime installed with the gateways' wiring
    /// (`gateway_wiring::identity_mob_handle`) lives inside the composition,
    /// so its services carry no public owner, and dropping the last external
    /// owner lets the idle mob close instead of the installed runtime
    /// holding admission open.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_gateway_installed_identity_runtime_holds_no_public_owner() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime) = boot_identity_first_persistent(
            "gateway-installed-identity",
            &temp.path().join("mob.sqlite3"),
            &temp.path().join("state"),
        )
        .await;
        let services_handle = identity_runtime
            .runtime_services()
            .mob_handle()
            .expect("the installed services carry the mob handle");
        assert!(
            !services_handle.is_public_owner(),
            "the installed identity runtime's services hold no public owner"
        );
        drop(services_handle);

        let closed = runtime.mob_handle().observer().admission_closed();
        drop(identity_runtime);
        drop(runtime);
        assert!(
            admission_closes(closed).await,
            "dropping the last external owner closes admission"
        );
    }

    /// The weak back-link to an exported identity runtime: it never keeps
    /// the mob publicly owned nor the runtime alive. While the mob is owned,
    /// upgrade returns the owner-bound view over the same runtime, with the
    /// same member lookup and the same governed-send checks; once the
    /// runtime is released and the idle mob closed, upgrade is `None` and
    /// the runtime frees.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_weak_identity_runtime_upgrades_only_while_the_mob_is_owned() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime) = boot_identity_first_persistent(
            "weak-identity-runtime",
            &temp.path().join("mob.sqlite3"),
            &temp.path().join("state"),
        )
        .await;
        let view = runtime.identity_runtime().expect("identity runtime export");
        let weak = view.downgrade();
        drop(view);

        let upgraded = weak.upgrade().expect("upgrades while the mob is owned");
        assert!(Arc::ptr_eq(upgraded.runtime(), &identity_runtime));
        for alias in ["domain:home", "rt:missing:singleton:0", "not-an-alias"] {
            assert_eq!(
                upgraded.owned_identity_for_member_alias(alias).await,
                identity_runtime
                    .owned_identity_for_member_alias(alias)
                    .await,
                "same member lookup for {alias}"
            );
        }
        let binder = Arc::new(UnreachedBinder(std::sync::atomic::AtomicUsize::new(0)));
        let unknown =
            crate::identity_first::AgentIdentity::parse("unknown:weak-upgrade").expect("identity");
        let send = |view: crate::identity_first::PublicIdentityRuntime| {
            let unknown = unknown.clone();
            let binder: Arc<dyn crate::identity_first::bridge::GovernedInputBinder> =
                binder.clone();
            async move {
                view.send_governed_with_turn_ticket(
                    &unknown,
                    None,
                    meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new("weak", None)),
                    binder,
                )
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
            }
        };
        let through_weak = send(upgraded.clone()).await;
        let through_owner = send(runtime.identity_runtime().expect("export")).await;
        assert!(through_weak.is_err(), "an unknown identity is refused");
        assert_eq!(through_weak, through_owner, "same governed-send checks");
        assert_eq!(
            binder.0.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "refused before binding"
        );

        let closed = runtime.mob_handle().observer().admission_closed();
        drop(upgraded);
        drop(identity_runtime);
        drop(runtime);
        assert!(
            admission_closes(closed).await,
            "the weak link does not keep the mob publicly owned"
        );
        assert!(weak.upgrade().is_none(), "no owner after the close");
        let freed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while weak.runtime_strong_count() > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(
            freed.is_ok(),
            "the weak link keeps no strong reference: the runtime frees ({} left)",
            weak.runtime_strong_count()
        );
    }

    /// D67: `shutdown` shuts the mob down, and a shut-down mob admits no
    /// public owner, so a weak identity runtime link that outlives the
    /// shutdown upgrades to `None` ("`None` once it closed"), even while the
    /// identity runtime itself is still held.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_weak_identity_runtime_does_not_upgrade_after_shutdown() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime) = boot_identity_first_persistent(
            "weak-identity-after-shutdown",
            &temp.path().join("mob.sqlite3"),
            &temp.path().join("state"),
        )
        .await;
        let weak = runtime
            .identity_runtime()
            .expect("identity runtime export")
            .downgrade();
        assert!(
            weak.upgrade().is_some(),
            "upgrades while the mob admits owners"
        );
        let closed = runtime.mob_handle().observer().admission_closed();

        runtime.shutdown().await;
        assert!(
            weak.runtime_strong_count() > 0,
            "the identity runtime is still held, so only the mob decides"
        );
        assert!(
            weak.upgrade().is_none(),
            "a shut-down mob grants the weak link no public owner"
        );
        assert!(
            closed.borrow().is_some(),
            "the shutdown closed the mob's admission"
        );
        drop(identity_runtime);
    }

    /// D67: the session bridge exported from an identity runtime view is a
    /// public owner. Kept after dropping the view and the runtime, it keeps
    /// the mob owned and stays usable; releasing it lets the idle mob close.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_retained_bridge_export_keeps_the_mob_owned() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (runtime, identity_runtime) = boot_identity_first_persistent(
            "retained-bridge-export",
            &temp.path().join("mob.sqlite3"),
            &temp.path().join("state"),
        )
        .await;
        let bridge = runtime
            .identity_runtime()
            .expect("identity runtime export")
            .bridge()
            .expect("bridge export");
        let closed = runtime.mob_handle().observer().admission_closed();
        drop(identity_runtime);
        drop(runtime);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        assert!(
            closed.borrow().is_none(),
            "the retained bridge export keeps the mob publicly owned"
        );
        bridge
            .current_member_wires()
            .await
            .expect("the retained bridge export stays usable");
        drop(bridge);
        assert!(
            admission_closes(closed).await,
            "releasing the bridge export lets the idle mob close"
        );
    }

    /// The downstream app's crash loop: an explicit resume that stalls past Meerkat's
    /// patience window is re-joined in-process, and bootstrap completes on
    /// the same runtime instead of shutting down.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_stalled_activation_that_later_resumes_completes_bootstrap() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (boot, session_id) =
            restart_before_activation("activation-stall-recovers", &temp).await;
        inject_activation_outcomes(
            &boot.runtime,
            3,
            vec![
                restart_member_activation_stall(),
                restart_member_activation_stall(),
            ],
        )
        .await;
        let identity_runtime = boot.identity_runtime.clone();
        let runtime = boot.activate().await;
        assert_eq!(
            runtime.mob_handle().status().await.expect("mob status"),
            meerkat_mob::MobState::Running,
            "the re-joined resume lifted the mob"
        );
        let revived = commit_restart_member_turn(&identity_runtime, "after restart").await;
        assert_eq!(revived, session_id, "the restore resumed the same session");
        runtime.shutdown().await;
    }

    /// K consecutive stalls at the same member and stage still fail the
    /// bootstrap, and the runtime is shut down as before.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn consecutive_activation_stalls_at_one_point_fail_bootstrap() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (boot, _) = restart_before_activation("activation-stall-exhausts", &temp).await;
        inject_activation_outcomes(
            &boot.runtime,
            2,
            vec![
                restart_member_activation_stall(),
                restart_member_activation_stall(),
            ],
        )
        .await;
        let PersistentBootBeforeActivation {
            mut runtime,
            context,
            roster,
            ..
        } = boot;
        let error = runtime
            .install_and_bootstrap_identity_first_context(context, &roster)
            .await
            .expect_err("K consecutive stalls exhaust the activation");
        assert!(
            matches!(
                &error,
                crate::identity_first::IdentityRuntimeError::Internal(message)
                    if message.starts_with("activating the prepared mob")
            ),
            "{error:?}"
        );
        assert_ne!(
            runtime.mob_handle().status_observation_snapshot(),
            meerkat_mob::MobState::Running,
            "the failed activation never lifted the mob"
        );
        assert!(
            runtime.pending_mob_activation.lock().await.is_none(),
            "the obligation was consumed, not re-staged"
        );
    }

    /// Any other activation error stays fatal on the first attempt: the
    /// queued stall behind it is never reached.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_non_stall_activation_error_fails_bootstrap_immediately() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (boot, _) = restart_before_activation("activation-error-fatal", &temp).await;
        inject_activation_outcomes(
            &boot.runtime,
            10,
            vec![
                meerkat_mob::MobError::Internal("injected activation failure".to_string()),
                restart_member_activation_stall(),
            ],
        )
        .await;
        let PersistentBootBeforeActivation {
            mut runtime,
            context,
            roster,
            ..
        } = boot;
        let error = runtime
            .install_and_bootstrap_identity_first_context(context, &roster)
            .await
            .expect_err("a non-stall activation error is fatal");
        assert!(
            matches!(
                &error,
                crate::identity_first::IdentityRuntimeError::Internal(message)
                    if message.contains("injected activation failure")
            ),
            "{error:?}"
        );
    }

    /// Discard the member's live actor, as a durability or lifecycle path
    /// does; the member keeps its binding and the next send revives a
    /// successor actor for the same session.
    async fn discard_restart_member_actor(
        runtime: &UnifiedRuntime,
        session_id: &meerkat_core::types::SessionId,
    ) {
        let service = runtime
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("persistent runtime has a session service");
        meerkat_mob::MobSessionService::discard_live_session(service.as_ref(), session_id)
            .await
            .expect("discard the live actor");
    }

    /// A same-binding successor actor: the predecessor's stream is attached,
    /// its run is still unread when the actor is discarded, and a successor
    /// for the same session and binding atoms runs to completion. The
    /// successor is subscribed only after the predecessor's stream has
    /// drained and closed, so every predecessor event reaches the console
    /// before the successor's first, the successor's replay still starts at
    /// its own `run_started`, its sequence continues the predecessor's, and
    /// each run's frames carry their own run's lineage.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn same_binding_successor_attaches_only_after_its_predecessor_drained() {
        let mob_id = "console-replay-successor";
        let temp = tempfile::tempdir().expect("temp dir");
        let mob_path = temp.path().join("mob.sqlite3");
        let state_root = temp.path().join("state");
        let (first, first_identity) =
            boot_identity_first_persistent(mob_id, &mob_path, &state_root).await;
        let session_id = commit_restart_member_turn(&first_identity, "before restart").await;
        first.shutdown().await;

        let boot = boot_persistent_before_activation(mob_id, &mob_path, &state_root).await;
        let ingress = boot.runtime.install_test_event_ingress().await;
        let identity_runtime = boot.identity_runtime.clone();
        let runtime = boot.activate().await;
        let handle = runtime.mob_handle();
        let key = restart_member_key(&restart_member_entry(&handle).await, mob_id);
        let mut forwarder = DrivenForwarder::new();
        forwarder.reconcile(&handle, &None).await;
        let predecessor = forwarder
            .tracked
            .current
            .get(&key)
            .map(|attachment| attachment.generation)
            .expect("the revived actor's stream attached");

        commit_restart_member_turn(&identity_runtime, "predecessor run").await;
        discard_restart_member_actor(&runtime, &session_id).await;
        commit_restart_member_turn(&identity_runtime, "successor run").await;
        assert_eq!(
            restart_member_key(&restart_member_entry(&handle).await, mob_id),
            key,
            "the successor kept the binding atoms"
        );

        forwarder.reconcile(&handle, &None).await;
        assert_eq!(
            forwarder
                .tracked
                .current
                .get(&key)
                .map(|attachment| attachment.generation),
            Some(predecessor),
            "the predecessor's stream still serves the binding; the successor waits"
        );
        assert_eq!(forwarder.streams.len(), 1);

        let steps = forwarder.forward_terminals(&runtime, &ingress, 2).await;
        let close = steps
            .iter()
            .position(|step| matches!(step, ForwarderStep::Closed { .. }))
            .expect("the predecessor's stream closed");
        assert!(matches!(
            steps[close],
            ForwarderStep::Closed { generation, served: true } if generation == predecessor
        ));
        let events = |steps: &[ForwarderStep]| -> Vec<(u64, &'static str)> {
            steps
                .iter()
                .filter_map(|step| match step {
                    ForwarderStep::Event(envelope) => Some((
                        envelope.seq,
                        meerkat_core::event::agent_event_type(&envelope.payload),
                    )),
                    ForwarderStep::Closed { .. } | ForwarderStep::Abandoned { .. } => None,
                })
                .collect()
        };
        let (before, after) = (events(&steps[..close]), events(&steps[close + 1..]));
        assert_eq!(before.first(), Some(&(1, "run_started")), "{before:?}");
        assert_eq!(
            before.last().map(|event| event.1),
            Some("run_completed"),
            "{before:?}"
        );
        let predecessor_tail = before.last().map_or(0, |event| event.0);
        assert_eq!(
            after.first().map(|event| event.1),
            Some("run_started"),
            "{after:?}"
        );
        assert!(
            after
                .first()
                .is_some_and(|event| event.0 > predecessor_tail),
            "the successor's sequence continues the predecessor's: {after:?}"
        );
        assert_eq!(
            after.last().map(|event| event.1),
            Some("run_completed"),
            "{after:?}"
        );
        assert!(
            forwarder
                .tracked
                .current
                .get(&key)
                .is_some_and(
                    |attachment| attachment.generation != predecessor && attachment.actor.is_some()
                ),
            "the successor's stream serves the binding now"
        );

        let timeline = session_timeline(&runtime, &session_id).await;
        let second_start = timeline
            .iter()
            .rposition(|frame| frame.event_type == "run_started")
            .expect("successor run_started");
        assert_one_attributed_run(&timeline[..second_start], "predecessor run");
        assert_one_attributed_run(&timeline[second_start..], "successor run");
        assert_ne!(
            timeline[0].data.get("run_id"),
            timeline[second_start].data.get("run_id")
        );

        runtime.shutdown().await;
    }

    /// A member of a running, admitting mob whose subscribe fails keeps the
    /// transient handling: the first-failure warning, a retry deadline, and
    /// the failure counted toward the stream-loss threshold.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reconcile_keeps_the_warning_backoff_and_count_for_a_running_mobs_member() {
        let mob_id = "console-running-failure";
        let temp = tempfile::tempdir().expect("temp dir");
        let mob_path = temp.path().join("mob.sqlite3");
        let state_root = temp.path().join("state");
        let (first, first_identity) =
            boot_identity_first_persistent(mob_id, &mob_path, &state_root).await;
        let session_id = commit_restart_member_turn(&first_identity, "before restart").await;
        first.shutdown().await;

        let boot = boot_persistent_before_activation(mob_id, &mob_path, &state_root).await;
        let _ingress = boot.runtime.install_test_event_ingress().await;
        let identity_runtime = boot.identity_runtime.clone();
        let runtime = boot.activate().await;
        let handle = runtime.mob_handle();
        let key = restart_member_key(&restart_member_entry(&handle).await, mob_id);
        commit_restart_member_turn(&identity_runtime, "revoked run").await;
        discard_restart_member_actor(&runtime, &session_id).await;
        assert_eq!(handle.status_observation_snapshot(), MobState::Running);
        assert!(handle.admission_closed().borrow().is_none());

        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .finish();
        // A second registered dispatcher keeps callsite interest computed for
        // every dispatcher, so a callsite another test's thread reached first
        // still records here.
        let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        let guard = tracing::subscriber::set_default(subscriber);
        let mut forwarder = DrivenForwarder::new();
        forwarder.reconcile(&handle, &None).await;
        drop(guard);
        let logged = writer.contents();

        let backoff = forwarder
            .subscribe_failures
            .get(&key)
            .expect("the failed subscription is held");
        assert!(backoff.next_attempt.is_some(), "a retry is scheduled");
        assert_eq!(
            backoff.consecutive_failures, 1,
            "the failure counts toward the stream-loss threshold"
        );
        assert!(
            logged.contains("failed to subscribe; will retry with backoff"),
            "{logged}"
        );

        runtime.shutdown().await;
    }

    /// An actor discarded before the forwarder attached is never attached:
    /// no stream is attributed to the member from a revoked actor. The
    /// successor actor's stream attaches next, from its own first event.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn discarded_actor_is_never_attributed_and_its_successor_attaches_from_its_first_event() {
        let mob_id = "console-replay-discarded";
        let temp = tempfile::tempdir().expect("temp dir");
        let mob_path = temp.path().join("mob.sqlite3");
        let state_root = temp.path().join("state");
        let (first, first_identity) =
            boot_identity_first_persistent(mob_id, &mob_path, &state_root).await;
        let session_id = commit_restart_member_turn(&first_identity, "before restart").await;
        first.shutdown().await;

        let boot = boot_persistent_before_activation(mob_id, &mob_path, &state_root).await;
        let ingress = boot.runtime.install_test_event_ingress().await;
        let identity_runtime = boot.identity_runtime.clone();
        let runtime = boot.activate().await;
        let handle = runtime.mob_handle();
        let key = restart_member_key(&restart_member_entry(&handle).await, mob_id);
        commit_restart_member_turn(&identity_runtime, "revoked run").await;
        discard_restart_member_actor(&runtime, &session_id).await;

        let mut forwarder = DrivenForwarder::new();
        forwarder.reconcile(&handle, &None).await;
        assert!(
            forwarder.tracked.current.is_empty(),
            "nothing is attached from a revoked actor, and no actor serves the session"
        );
        assert!(forwarder.subscribe_failures.contains_key(&key));

        commit_restart_member_turn(&identity_runtime, "successor run").await;
        // The backoff lapses.
        forwarder
            .subscribe_failures
            .get_mut(&key)
            .expect("parked member")
            .next_attempt = Some(tokio::time::Instant::now());
        forwarder.reconcile(&handle, &None).await;
        assert!(
            forwarder
                .tracked
                .current
                .get(&key)
                .is_some_and(|attachment| attachment.actor.is_some()),
            "the successor's stream attached"
        );
        forwarder.forward_terminals(&runtime, &ingress, 1).await;
        assert_one_attributed_run(
            &session_timeline(&runtime, &session_id).await,
            "successor run",
        );

        runtime.shutdown().await;
    }

    /// A superseded stream's close carries its own generation, so it never
    /// untracks the stream that serves its key now.
    #[tokio::test]
    async fn stale_close_never_untracks_the_stream_serving_its_key() {
        let key = TrackedAgentEventStream {
            mob_id: "stale-close".to_string(),
            durable_identity: None,
            member_identity: AgentIdentity::from("worker"),
            runtime_id: AgentRuntimeId::new(AgentIdentity::from("worker"), Generation::new(0)),
            fence_token: FenceToken::new(1),
        };
        let mut streams: SelectAll<TaggedAgentEventStream> = SelectAll::new();
        let superseded = attach_member_event_stream(
            &mut streams,
            key.clone(),
            ProfileName::from("worker"),
            Box::pin(futures::stream::empty()),
            None,
            None,
            None,
        );
        let superseded_generation = superseded.generation;
        let serving = attach_member_event_stream(
            &mut streams,
            key.clone(),
            ProfileName::from("worker"),
            Box::pin(futures::stream::pending()),
            None,
            None,
            None,
        );
        let serving_generation = serving.generation;
        let mut tracked = AttachedStreams::default();
        tracked.current.insert(key.clone(), serving);

        let Some(ForwardedAgentEvent::Closed(closed_key, generation)) = streams.next().await else {
            panic!("the superseded stream closes");
        };
        assert_eq!(
            (closed_key.clone(), generation),
            (key.clone(), superseded_generation)
        );
        assert!(!tracked.close(&closed_key, generation));
        assert_eq!(
            tracked
                .current
                .get(&key)
                .map(|attachment| attachment.generation),
            Some(serving_generation)
        );
        assert!(tracked.close(&key, serving_generation));
        assert!(tracked.current.is_empty());
    }

    /// Child mobs are built on the agent mob tools' session service: a
    /// child-mob member's first run, finished before any forwarder attached,
    /// is replayed under the child mob.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn child_mob_members_first_run_is_replayed_under_the_child_mob() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (mob_runtime, _) = mob_with_idle_worker("console-replay-parent", &temp_dir).await;
        let state = mob_runtime
            .agent_mob_mcp_state()
            .expect("stock constructor installs agent mob tools");
        let child_definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"console-replay-child\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
             external_addressable = true\n\n[profiles.worker.tools]\ncomms = true\n",
        )
        .expect("child mob definition");
        state
            .mob_create_definition(child_definition)
            .await
            .expect("create child mob");
        let child = Box::pin(state.mob_handles_snapshot())
            .await
            .expect("managed mobs")
            .into_iter()
            .find(|(mob_id, _)| mob_id.as_str() == "console-replay-child")
            .map(|(_, handle)| handle)
            .expect("child mob handle");
        let mut member = SpawnMemberSpec::new(
            ProfileName::from("worker"),
            AgentIdentity::from(IDLE_WORKER),
        );
        member.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
        child
            .ensure_member(member)
            .await
            .expect("seat child worker");
        run_worker_turn(&child, "child probe").await;

        let mut forwarder = DrivenForwarder::new();
        forwarder
            .reconcile(&mob_runtime.handle(), &Some(state))
            .await;
        let ForwardedAgentEvent::Event(event) = forwarder.next().await else {
            panic!("the child stream yields its first run");
        };
        let (runtime_id, _, _, envelope, _, _) = *event;
        assert!(matches!(envelope.payload, AgentEvent::RunStarted { .. }));
        assert_eq!(envelope.seq, 1);
        assert!(
            forwarder
                .tracked
                .current
                .keys()
                .any(|key| key.mob_id == "console-replay-child"
                    && key.runtime_id == runtime_id
                    && forwarder.tracked.current[key].actor.is_some()),
            "attributed to the child mob's member, with its actor"
        );

        drop(forwarder);
        let _ = mob_runtime.handle().shutdown().await;
    }

    /// The drain deadline bounds how long a revoked predecessor holds back
    /// its member. The predecessor actor's stream is attached, then the
    /// member is respawned (new binding, new session) and its successor runs
    /// to completion. The predecessor's stream is kept open past
    /// its actor's revocation, as it stays when the actor's task never exits
    /// (a turn stuck in a tool or provider call that a discard does not
    /// interrupt, or a teardown that parks); in-process, meerkat's retire
    /// otherwise closes it at once. The successor is held while the
    /// predecessor may still drain. Once the predecessor has held it for the
    /// deadline (the test moves the hold's clock instead of waiting), the
    /// predecessor's stream is cut off with an explicit `stream_truncated`
    /// gap, and only then does the successor attach, replaying from its own
    /// `run_started` (seq 1) through its terminal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stuck_predecessor_is_cut_off_at_its_drain_deadline() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (mob_runtime, _) =
            mob_with_idle_worker("console-replay-stuck-predecessor", &temp_dir).await;
        let handle = mob_runtime.handle();
        let module_runtime = std::thread::spawn(|| {
            start_meta_runtime_with_options(
                MetaConfig {
                    modules: vec![],
                    discovery: crate::types::DiscoverySpec {
                        namespace: "console-replay-stuck-predecessor".to_string(),
                        modules: vec![],
                    },
                    pre_spawn: vec![],
                },
                Vec::new(),
                Duration::from_secs(2),
                RuntimeOptions::default(),
            )
        })
        .join()
        .expect("module runtime thread")
        .expect("module runtime");
        let runtime = UnifiedRuntime::from_parts(
            mob_runtime,
            module_runtime,
            Arc::new(InMemoryMetadataStore::new()),
        )
        .await;
        let ingress = runtime.install_test_event_ingress().await;
        // This test drives the forwarder's steps itself, from a fresh actor
        // the exited forwarder never saw.
        handle
            .respawn(AgentIdentity::from(IDLE_WORKER), None)
            .await
            .expect("respawn onto a fresh actor");
        let predecessor_session = handle
            .resolve_bridge_session_id(&AgentIdentity::from(IDLE_WORKER))
            .await
            .expect("predecessor session");
        let entry = handle
            .list_members_including_retiring()
            .await
            .into_iter()
            .find(|entry| entry.agent_identity == IDLE_WORKER)
            .expect("worker roster entry");
        let predecessor_key = restart_member_key(&entry, handle.mob_id().as_str());
        let subscription = handle
            .subscribe_agent_events_from(
                &AgentIdentity::from(IDLE_WORKER),
                meerkat_core::comms::SessionEventCursor::Earliest,
            )
            .await
            .expect("subscribe to the fresh actor");
        assert!(subscription.actor.is_some(), "the fresh actor is named");
        let mut forwarder = DrivenForwarder::new();
        let attachment = attach_member_event_stream(
            &mut forwarder.streams,
            predecessor_key.clone(),
            entry.role.clone(),
            Box::pin(subscription.stream.chain(futures::stream::pending())),
            subscription.actor,
            subscription.epoch,
            None,
        );
        let predecessor = attachment.generation;
        forwarder
            .tracked
            .current
            .insert(predecessor_key.clone(), attachment);
        run_worker_turn(&handle, "predecessor run").await;
        forwarder.forward_terminals(&runtime, &ingress, 1).await;

        handle
            .respawn(AgentIdentity::from(IDLE_WORKER), None)
            .await
            .expect("respawn the member");
        let successor_session = handle
            .resolve_bridge_session_id(&AgentIdentity::from(IDLE_WORKER))
            .await
            .expect("successor session");
        assert_ne!(successor_session, predecessor_session);
        run_worker_turn(&handle, "successor run").await;

        forwarder.reconcile(&handle, &None).await;
        assert!(
            forwarder.tracked.current.is_empty(),
            "the successor's new binding waits for the predecessor"
        );
        let held_since = forwarder
            .tracked
            .departed
            .get(&predecessor)
            .filter(|departed| !departed.actor.is_live())
            .and_then(|departed| departed.attachment.held_since)
            .expect("the revoked predecessor is draining, and its hold is recorded");
        assert_eq!(
            earliest_reconcile_deadline(&forwarder.subscribe_failures, &forwarder.tracked),
            Some(held_since + PREDECESSOR_DRAIN_DEADLINE),
            "the forwarder wakes itself at the drain deadline"
        );
        forwarder.reconcile(&handle, &None).await;
        assert!(
            forwarder.tracked.current.is_empty(),
            "the successor still waits before the deadline"
        );
        {
            use futures::FutureExt;
            assert!(
                forwarder.streams.next().now_or_never().is_none(),
                "the predecessor's stream is still open"
            );
        }

        // The deadline passes.
        forwarder
            .tracked
            .departed
            .get_mut(&predecessor)
            .expect("draining predecessor")
            .attachment
            .held_since = Some(held_since - PREDECESSOR_DRAIN_DEADLINE);
        forwarder.reconcile(&handle, &None).await;
        assert!(
            forwarder.tracked.current.is_empty(),
            "the successor attaches only after the gap marker went out"
        );

        let steps = forwarder.forward_terminals(&runtime, &ingress, 1).await;
        assert!(
            matches!(
                steps.as_slice(),
                [
                    ForwarderStep::Abandoned { generation: abandoned },
                    ForwarderStep::Closed { generation: closed, served: false },
                    ..
                ] if *abandoned == predecessor && *closed == predecessor
            ),
            "the predecessor is cut off first: {steps:?}"
        );
        let successor_events: Vec<(u64, &'static str)> = steps[2..]
            .iter()
            .filter_map(|step| match step {
                ForwarderStep::Event(envelope) => Some((
                    envelope.seq,
                    meerkat_core::event::agent_event_type(&envelope.payload),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(successor_events.first(), Some(&(1, "run_started")));
        assert_eq!(
            successor_events.last().map(|event| event.1),
            Some("run_completed")
        );
        let successor_key = forwarder
            .tracked
            .current
            .keys()
            .next()
            .expect("the successor's stream serves its binding")
            .clone();
        assert_ne!(successor_key, predecessor_key);

        let replay = runtime
            .console_events()
            .replay_all(None)
            .await
            .expect("console replay");
        let gap = replay
            .iter()
            .position(|frame| frame.event_type == "stream_truncated")
            .expect("the gap is on the console timeline");
        assert_eq!(
            replay[gap].data["reason"]["kind"],
            json!("predecessor_stream_abandoned")
        );
        assert_eq!(replay[gap].data["session_id"], json!(predecessor_session));
        let session_frames = |session: &meerkat_core::types::SessionId| {
            replay
                .iter()
                .enumerate()
                .filter(|(_, frame)| frame.data.get("session_id") == Some(&json!(session)))
                .filter(|(_, frame)| frame.event_type != "stream_truncated")
                .map(|(index, frame)| (index, frame.clone()))
                .collect::<Vec<_>>()
        };
        let predecessor_frames = session_frames(&predecessor_session);
        let successor_frames = session_frames(&successor_session);
        assert!(predecessor_frames.iter().all(|(index, _)| *index < gap));
        assert!(
            successor_frames.iter().all(|(index, _)| *index > gap),
            "every successor frame follows the gap"
        );
        let frames = |indexed: Vec<(
            usize,
            crate::console_contracts::ConsoleIdentityEventEnvelope,
        )>| {
            indexed
                .into_iter()
                .map(|(_, frame)| frame)
                .collect::<Vec<_>>()
        };
        assert_one_attributed_run(&frames(predecessor_frames), "predecessor run");
        assert_one_attributed_run(&frames(successor_frames), "successor run");

        runtime.shutdown().await;
    }

    /// A departed stream whose actor is still live but serves a different
    /// session than the member is bound to now is a predecessor too (a
    /// rebind that landed before the old actor's revocation): it holds the
    /// member back, until its drain deadline cuts it off. On the member's
    /// current session the same stream is a re-key candidate instead.
    #[tokio::test]
    async fn departed_live_actor_on_another_session_drains_until_its_deadline() {
        let dir = tempfile::tempdir().expect("temp dir");
        let raw = Arc::new(meerkat::build_ephemeral_service(
            meerkat::AgentFactory::new(dir.path()),
            meerkat::Config::default(),
            16,
        )) as Arc<dyn meerkat_mob::MobSessionService>;
        let request = meerkat_core::service::CreateSessionRequest {
            model: "gpt-5.5".to_string(),
            prompt: meerkat_core::ContentInput::Text("probe".to_string()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    Arc::new(meerkat_client::TestClient::default()),
                )),
                ..Default::default()
            }),
            labels: None,
            injected_context: Vec::new(),
        };
        let old_session = raw
            .create_session(request)
            .await
            .expect("deferred create")
            .session_id;
        let actor = raw
            .subscribe_agent_session_events_from(
                &old_session,
                meerkat_core::comms::SessionEventCursor::Live,
            )
            .await
            .expect("subscribe")
            .actor
            .expect("the ephemeral service names its actor");
        let new_session = meerkat_core::types::SessionId::new();
        let key = TrackedAgentEventStream {
            mob_id: "departed-other-session".to_string(),
            durable_identity: None,
            member_identity: AgentIdentity::from("worker"),
            runtime_id: AgentRuntimeId::new(AgentIdentity::from("worker"), Generation::new(0)),
            fence_token: FenceToken::new(1),
        };
        let owner = MemberStreamOwner::of(&key);
        let mut streams: SelectAll<TaggedAgentEventStream> = SelectAll::new();
        let attachment = attach_member_event_stream(
            &mut streams,
            key.clone(),
            ProfileName::from("worker"),
            Box::pin(futures::stream::pending()),
            Some(actor),
            None,
            None,
        );
        let generation = attachment.generation;
        let mut tracked = AttachedStreams::default();
        tracked.current.insert(key.clone(), attachment);
        tracked.depart(&key, false);

        let now = tokio::time::Instant::now();
        assert!(!tracked.predecessor_draining(&owner, Some(&old_session), now));
        assert_eq!(
            tracked.live_departed_for(&owner, &old_session),
            Some(generation)
        );
        assert!(tracked.predecessor_draining(&owner, Some(&new_session), now));
        assert_eq!(
            tracked.earliest_drain_deadline(),
            Some(now + PREDECESSOR_DRAIN_DEADLINE)
        );

        assert!(tracked.predecessor_draining(
            &owner,
            Some(&new_session),
            now + PREDECESSOR_DRAIN_DEADLINE
        ));
        assert_eq!(tracked.earliest_drain_deadline(), None, "cut off");
        let Some(ForwardedAgentEvent::Abandoned {
            session_id,
            generation: abandoned,
            ..
        }) = streams.next().await
        else {
            panic!("the cut-off stream reports its gap first");
        };
        assert_eq!((session_id, abandoned), (old_session, generation));
        let Some(ForwardedAgentEvent::Closed(closed_key, closed)) = streams.next().await else {
            panic!("then closes");
        };
        assert!(!tracked.close(&closed_key, closed));
        assert!(!tracked.predecessor_draining(&owner, Some(&new_session), now));
    }
}
