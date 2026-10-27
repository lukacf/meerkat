//! Error types, hook definitions, and report structures for the unified runtime.

use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::mob_handle_runtime::MobRuntimeError;
use crate::runtime::{
    NormalizationError, RuntimeRouteMutationError, RuntimeShutdownReport, SubscribeError,
};

use super::edge_types::{DesiredPeerEdge, EdgeReconcileFailure};

/// Report from dynamic edge reconciliation.
///
/// Best-effort: partial success is reported clearly. Apps decide whether
/// to treat failures as fatal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UnifiedRuntimeReconcileEdgesReport {
    pub desired_edges: Vec<DesiredPeerEdge>,
    pub wired_edges: Vec<DesiredPeerEdge>,
    pub unwired_edges: Vec<DesiredPeerEdge>,
    pub retained_edges: Vec<DesiredPeerEdge>,
    pub preexisting_edges: Vec<DesiredPeerEdge>,
    pub skipped_missing_members: Vec<DesiredPeerEdge>,
    pub pruned_stale_managed_edges: Vec<DesiredPeerEdge>,
    #[serde(default)]
    pub failures: Vec<EdgeReconcileFailure>,
}

impl UnifiedRuntimeReconcileEdgesReport {
    /// True if all desired edges were successfully applied or retained.
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty() && self.skipped_missing_members.is_empty()
    }
}

#[derive(Debug)]
pub enum UnifiedRuntimeBootstrapError {
    Mob(MobRuntimeError),
    Module(crate::runtime::MetaRuntimeError),
    ModuleStartupThreadPanicked,
    ModuleStartupRollbackFailed {
        startup_error: Box<UnifiedRuntimeBootstrapError>,
        rollback_error: MobRuntimeError,
    },
    PreSpawnHook(String),
    IdentityFirst(String),
    Topology(String),
}

impl Display for UnifiedRuntimeBootstrapError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mob(err) => write!(f, "failed to bootstrap mob runtime: {err}"),
            Self::Module(err) => write!(f, "failed to bootstrap module runtime: {err:?}"),
            Self::ModuleStartupThreadPanicked => {
                write!(
                    f,
                    "failed to bootstrap module runtime: startup thread panicked"
                )
            }
            Self::PreSpawnHook(err) => {
                write!(f, "pre-spawn hook failed: {err}")
            }
            Self::IdentityFirst(err) => {
                write!(f, "identity-first bootstrap failed: {err}")
            }
            Self::Topology(err) => write!(f, "topology-control bootstrap failed: {err}"),
            Self::ModuleStartupRollbackFailed {
                startup_error,
                rollback_error,
            } => {
                write!(
                    f,
                    "failed to bootstrap unified runtime: startup error ({startup_error}) and rollback failed: {rollback_error}"
                )
            }
        }
    }
}

impl std::error::Error for UnifiedRuntimeBootstrapError {}

impl UnifiedRuntimeBootstrapError {
    /// The mob composition provenance refusal behind this bootstrap failure,
    /// if that is what stopped it, including a refusal whose startup rollback
    /// then failed too. Surfaces report these deliberate refusals with their
    /// own code instead of as an internal error.
    pub fn composition_provenance(
        &self,
    ) -> Option<&crate::mob_composition_manifest::MobCompositionProvenanceError> {
        match self {
            Self::Mob(MobRuntimeError::CompositionProvenance(refusal)) => Some(refusal),
            // Defensive: a provenance refusal fails mob preparation before any
            // module starts, so a rollback failure does not wrap one today.
            // Seeing through it keeps the refusal typed if that order changes.
            Self::ModuleStartupRollbackFailed { startup_error, .. } => {
                startup_error.composition_provenance()
            }
            Self::Mob(_)
            | Self::Module(_)
            | Self::ModuleStartupThreadPanicked
            | Self::PreSpawnHook(_)
            | Self::IdentityFirst(_)
            | Self::Topology(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifiedRuntimeBuilderField {
    MobSpec,
    ModuleConfig,
    Timeout,
}

#[derive(Debug)]
pub enum UnifiedRuntimeBuilderError {
    MissingRequiredField(UnifiedRuntimeBuilderField),
    Bootstrap(UnifiedRuntimeBootstrapError),
    /// Failed to read a definition TOML file or create a state directory.
    Io(String),
    /// Failed to parse a mob definition TOML.
    DefinitionLoad(String),
    /// Conflicting builder configuration (e.g., persistent_state + scratch_dir).
    ConflictingConfiguration(String),
    /// Storage layout refusal (file-name twins in the state directory).
    StorageLayout(crate::storage_layout::StorageLayoutError),
    /// A storage provider failed to open the realm's store set, or the
    /// fail-closed durability rule refused it (M4).
    StorageProvider(crate::storage_provider::MetaStorageProviderError),
    /// A live door (console voice or the external live channel) was
    /// registered but could not be composed.
    LiveCompose(super::live_compose::LiveComposeError),
    /// `require_incremental_session_persistence(true)` was set, and the
    /// session store does not advertise meerkat's incremental persistence:
    /// it would write the whole session document at every turn boundary.
    SessionStoreNotIncremental {
        store_kind: String,
    },
    /// The composition's runtime machine could not be built or acquired.
    Runtime(meerkat_runtime::RuntimeDriverError),
    Delivery(crate::mob_handle_runtime::MobRuntimeDeliveryError),
}

impl From<crate::storage_layout::StorageLayoutError> for UnifiedRuntimeBuilderError {
    fn from(error: crate::storage_layout::StorageLayoutError) -> Self {
        Self::StorageLayout(error)
    }
}

impl From<crate::storage_provider::MetaStorageProviderError> for UnifiedRuntimeBuilderError {
    fn from(error: crate::storage_provider::MetaStorageProviderError) -> Self {
        Self::StorageProvider(error)
    }
}

impl Display for UnifiedRuntimeBuilderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingRequiredField(UnifiedRuntimeBuilderField::MobSpec) => {
                write!(f, "missing required builder field: mob_spec or definition")
            }
            Self::MissingRequiredField(UnifiedRuntimeBuilderField::ModuleConfig) => {
                write!(f, "missing required builder field: module_config")
            }
            Self::MissingRequiredField(UnifiedRuntimeBuilderField::Timeout) => {
                write!(f, "missing required builder field: timeout")
            }
            Self::Bootstrap(err) => write!(f, "{err}"),
            Self::Io(msg) => write!(f, "{msg}"),
            Self::DefinitionLoad(msg) => write!(f, "{msg}"),
            Self::ConflictingConfiguration(msg) => write!(f, "conflicting configuration: {msg}"),
            Self::StorageLayout(err) => write!(f, "{err}"),
            Self::StorageProvider(err) => write!(f, "{err}"),
            Self::LiveCompose(err) => write!(f, "{err}"),
            Self::SessionStoreNotIncremental { store_kind } => write!(
                f,
                "require_incremental_session_persistence is set, but the session store \
                 ({store_kind}) does not advertise incremental persistence; implement \
                 ContinuityStore::as_incremental_sessions on the continuity store (or forward \
                 it from the store your wrapper wraps)"
            ),
            Self::Runtime(err) => write!(f, "{err}"),
            Self::Delivery(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for UnifiedRuntimeBuilderError {}

#[derive(Debug)]
pub enum UnifiedRuntimeError {
    Normalize(NormalizationError),
    Subscribe(SubscribeError),
    RuntimeShuttingDown,
}

impl Display for UnifiedRuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normalize(err) => write!(f, "failed to normalize unified event: {err:?}"),
            Self::Subscribe(err) => write!(f, "failed to subscribe to unified events: {err:?}"),
            Self::RuntimeShuttingDown => write!(f, "unified runtime is shutting down"),
        }
    }
}

impl std::error::Error for UnifiedRuntimeError {}

impl From<NormalizationError> for UnifiedRuntimeError {
    fn from(value: NormalizationError) -> Self {
        Self::Normalize(value)
    }
}

impl From<SubscribeError> for UnifiedRuntimeError {
    fn from(value: SubscribeError) -> Self {
        Self::Subscribe(value)
    }
}

/// What a teardown-time mob stop actually achieved.
///
/// The two non-failure outcomes are deliberately NOT collapsed.
/// [`Self::Stopped`] means the mob machine accepted `Stop`;
/// [`Self::ProceededWithoutInterrupt`] means teardown went ahead having failed
/// to interrupt a member. Reporting the second as the first would be a false
/// success the caller cannot detect at the call site - and a mid-attach member
/// is precisely the case where it would be a lie, because a turn has already
/// been admitted and its kickoff is about to bind.
#[derive(Debug)]
pub enum MobStopOutcome {
    /// The mob machine accepted `Stop`.
    Stopped,
    /// Teardown proceeded WITHOUT a successful interrupt: the stop kept being
    /// refused on runtime-attach readiness for the whole window. A turn may
    /// have been admitted on the named member and may still be running. The
    /// condition was reported through the error hook as
    /// [`ErrorEvent::MobStopProceededWithoutInterrupt`], never swallowed.
    ProceededWithoutInterrupt {
        waited_ms: u64,
        /// The session/member meerkat named in its refusal, when the refusal
        /// carried one. `None` means the text did not name a subject - the
        /// report omits what it did not observe rather than guessing.
        member: Option<String>,
        error: String,
    },
    /// Any other refusal, unchanged in meaning.
    Failed(crate::mob_handle_runtime::MobRuntimeError),
}

impl MobStopOutcome {
    /// Whether teardown may continue. NOT a success predicate: it is true for
    /// [`Self::ProceededWithoutInterrupt`], where nothing was interrupted.
    /// Callers that need to know the mob actually quiesced must test
    /// [`Self::stopped_cleanly`].
    pub fn teardown_may_proceed(&self) -> bool {
        matches!(self, Self::Stopped | Self::ProceededWithoutInterrupt { .. })
    }

    /// Whether the mob machine actually accepted `Stop`.
    pub fn stopped_cleanly(&self) -> bool {
        matches!(self, Self::Stopped)
    }
}

/// Exact disposition of identity-first lease authority during shutdown.
///
/// Meta holds no identity lease authority (Core owns every member
/// binding), so shutdown reports [`Self::NotConfigured`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityAuthorityReleaseOutcome {
    /// The runtime did not have identity-first authority to release.
    NotConfigured,
    /// Every retained grant was released from the configured provider.
    Released { grant_count: usize },
    /// The provider rejected or failed the exact release operation.
    Failed { error: String },
    /// A reset-superseded session/member cleanup obligation remained after
    /// physical shutdown retries, so provider grants were retained.
    SkippedResetCleanupFailed { error: String },
    /// Physical members did not quiesce, so their grants were deliberately retained.
    SkippedMobStopFailed,
}

#[derive(Debug)]
pub struct UnifiedRuntimeShutdownReport {
    pub drain: ShutdownDrainReport,
    pub module_shutdown: RuntimeShutdownReport,
    pub mob_stop: Result<(), MobRuntimeError>,
    pub identity_authority_release: IdentityAuthorityReleaseOutcome,
    /// Part of `cleanup_completed()`: see that method for why an unreleased
    /// supervisor is incompleteness rather than a detail. It is also the reason
    /// the gateway attaches diagnostics at all, and those attach only when
    /// cleanup did not complete, so excluding this outcome would have made it
    /// unreportable in exactly the case it exists for.
    pub retired_supervisor_cleanup: RetiredSupervisorCleanupOutcome,
    /// What the mob actor's terminal teardown after the mob stop reported.
    /// Best-effort, as before: it frees the mob's supervisor route, so it is
    /// not part of `cleanup_completed()`, and a refusal is logged loudly.
    pub mob_terminal_shutdown: MobTerminalShutdownOutcome,
    /// Durable child mobs. Part of `cleanup_completed()`: a runtime with
    /// durable child mobs attests complete cleanup only when their
    /// stand-down released the child root
    /// ([`crate::ChildMobStandDownOutcome::Released`]), which needs the agent
    /// mob-tool state's admission and owned work to settle and every child
    /// to shut down cleanly. Clean children alone are not enough.
    pub child_mob_stand_down: crate::child_mob_storage::ChildMobStandDownOutcome,
    /// The members' MCP servers, closed and observed after the member
    /// teardown. Part of `cleanup_completed()`: that teardown is bounded below
    /// a stdio close's own bound and is not part of it, and an agent's release
    /// of its MCP router is not a cleanup receipt.
    pub member_mcp_settlement: MemberMcpSettlement,
}

/// What the mob actor's terminal teardown (`MobHandle::shutdown_with_report`)
/// reported during [`UnifiedRuntime::shutdown`](crate::UnifiedRuntime).
#[derive(Debug)]
#[non_exhaustive]
pub enum MobTerminalShutdownOutcome {
    /// The mob stop did not quiesce members, so terminal teardown was not
    /// attempted.
    SkippedMobStopFailed,
    /// The mob actor shut down and reported each member; see
    /// [`meerkat_mob::MobShutdownReport::is_clean`].
    Completed(meerkat_mob::MobShutdownReport),
    /// The mob actor had already shut down: its command channel was closed.
    /// (A teardown that meets a Shutdown still draining joins it and reports
    /// that Shutdown's result instead.)
    AlreadyShutDown,
    /// The mob actor refused its terminal teardown; its supervisor in-proc
    /// route may stay registered until process exit.
    Refused { error: String },
}

/// What [`UnifiedRuntime::shutdown`](crate::UnifiedRuntime) observed of the
/// MCP servers its members' routers owned, through the gateway's member MCP
/// custody.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberMcpSettlement {
    /// No member router was in custody, or no custody was installed.
    NothingOwned,
    /// Every router in custody reported its shutdown, and every server it
    /// owned settled: a stdio server's process was reaped and every holder of
    /// its stdout closed it.
    Settled { routers: usize, servers: usize },
    /// At least one router or server did not settle. Never complete cleanup.
    Unsettled(MemberMcpUnsettled),
}

impl MemberMcpSettlement {
    /// Settled, or nothing was owned.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        matches!(self, Self::NothingOwned | Self::Settled { .. })
    }

    /// `Settled` exactly when no cause counted.
    pub(crate) fn from_counts(counts: MemberMcpUnsettled) -> Self {
        if counts.deadline
            + counts.wait_failed
            + counts.not_reaped
            + counts.stdout_open
            + counts.stdout_unobserved
            + counts.not_held
            + counts.close_failed
            + counts.task_failed
            == 0
        {
            Self::Settled {
                routers: counts.routers,
                servers: counts.servers,
            }
        } else {
            Self::Unsettled(counts)
        }
    }
}

/// The member routers and servers in custody at shutdown, and how many of
/// them did not settle, by class. A server counts once, under one class.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemberMcpUnsettled {
    /// Routers in custody.
    pub routers: usize,
    /// Servers whose close reported a native outcome.
    pub servers: usize,
    /// Routers whose closes had not all reported by the deadline Meerkat
    /// derived from the closes it started: Meerkat broke its own close bound.
    /// Never complete. The runtime's own settlement leaves such closes
    /// pending; a gateway's executor-exit settlement waits for their native
    /// outcomes before it answers, counts those in their classes, and still
    /// counts the router here.
    pub deadline: usize,
    /// Servers whose reap failed.
    pub wait_failed: usize,
    /// Servers killed but not reaped by their close's backstop.
    pub not_reaped: usize,
    /// Servers reaped while a holder of their stdout (a process outside the
    /// server's group) still kept it open.
    pub stdout_open: usize,
    /// Servers reaped while nothing showed that every holder of their stdout
    /// closed it (no witness, or reading it failed).
    pub stdout_unobserved: usize,
    /// Servers whose close another owner held, without a receipt for this
    /// router.
    pub not_held: usize,
    /// Servers whose close failed, on any transport.
    pub close_failed: usize,
    /// Servers whose close task panicked or was cancelled.
    pub task_failed: usize,
}

/// The class of a server close that did not settle, by Meerkat's final
/// facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpCloseUnsettledClass {
    /// The server process was killed, but its reap failed.
    WaitFailed,
    /// The server process was killed, but not reaped within the close bound.
    NotReaped,
    /// A holder of the server's stdout (a process outside its group) kept it
    /// open.
    StdoutOpen,
    /// Nothing showed that every holder of the server's stdout closed it (no
    /// witness, or reading it failed).
    StdoutUnobserved,
    /// Another owner held the close, without a receipt for this one.
    NotHeld,
    /// The close itself failed, on any transport (including Meerkat's own
    /// bound on a process-free close).
    CloseFailed,
    /// The close task panicked or was cancelled.
    TaskFailed,
}

impl McpCloseUnsettledClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WaitFailed => "wait_failed",
            Self::NotReaped => "not_reaped",
            Self::StdoutOpen => "stdout_open",
            Self::StdoutUnobserved => "stdout_unobserved",
            Self::NotHeld => "not_held",
            Self::CloseFailed => "close_failed",
            Self::TaskFailed => "task_failed",
        }
    }
}

impl std::fmt::Display for McpCloseUnsettledClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl UnifiedRuntimeShutdownReport {
    /// True only when every shutdown phase that owns external authority or
    /// child-process state completed successfully.
    ///
    /// A retired supervisor cleanup counts: a lease renewal or repair pass that
    /// the process cannot attest reached its release boundary is an
    /// authority-owning phase that did not demonstrably complete. Reporting
    /// `shutdown=true` while one is still running, or ended without returning,
    /// would make this method's own contract false.
    pub fn cleanup_completed(&self) -> bool {
        self.authority_phases_completed() && self.child_mob_stand_down.cleanup_complete()
    }

    /// Every phase except durable child-mob retention reached its boundary,
    /// and the durable child mobs are held until this process exits.
    ///
    /// Cleanup is still NOT complete: the root claim, the child state and
    /// any admitted child work stay with this process, and the same layout
    /// cannot be reopened in it. This only lets a host present expected
    /// process-exit retention differently from a failed phase; ending the
    /// process is what releases it.
    pub fn cleanup_retained_until_process_exit(&self) -> bool {
        self.authority_phases_completed()
            && matches!(
                self.child_mob_stand_down,
                crate::child_mob_storage::ChildMobStandDownOutcome::HeldUntilProcessExit { .. }
            )
    }

    fn authority_phases_completed(&self) -> bool {
        !self.drain.timed_out
            && self.mob_stop.is_ok()
            && matches!(
                &self.identity_authority_release,
                IdentityAuthorityReleaseOutcome::NotConfigured
                    | IdentityAuthorityReleaseOutcome::Released { .. }
            )
            && self.module_shutdown.orphan_processes == 0
            && self.module_shutdown.physical_cleanup.phase
                == crate::runtime::ModuleCleanupPhase::Settled
            && matches!(
                &self.retired_supervisor_cleanup,
                RetiredSupervisorCleanupOutcome::NothingPending
                    | RetiredSupervisorCleanupOutcome::Joined { .. }
            )
            && self.member_mcp_settlement.is_settled()
    }
}

#[derive(Debug)]
pub struct UnifiedRuntimeRunReport {
    pub serve_result: std::io::Result<()>,
    pub shutdown: UnifiedRuntimeShutdownReport,
}

/// Report from a rediscover operation (reset + re-run discovery + reconcile edges).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RediscoverReport {
    /// Number of members spawned by discovery.
    pub spawned: Vec<String>,
    /// Edge reconciliation report (if EdgeDiscovery is configured).
    pub edges: UnifiedRuntimeReconcileEdgesReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedRuntimeReconcileRoutingReport {
    pub router_module_loaded: bool,
    pub active_members: Vec<String>,
    pub added_route_keys: Vec<String>,
    pub removed_route_keys: Vec<String>,
}

/// Per-identity reconcile failure — re-export of the canonical
/// meerkat-contracts wire shape so SDK consumers see the same field
/// names whether they go through `mob/reconcile` or `meta/reconcile`.
pub use meerkat_contracts::MobReconcileFailureWire as MobReconcileFailure;

/// Roster half of a reconcile pass — re-export of meerkat-contracts'
/// canonical wire shape. `spawned: Vec<MobSpawnReceiptWire>` carries the
/// server-resolved `WireMemberRef` per receipt, replacing the
/// identity-string list meta projected before 0.6.
pub use meerkat_contracts::MobReconcileReportWire as MobReconcileReport;

/// Project meerkat's native `ReconcileReport` into the canonical wire shape.
///
/// Mirrors the `mob/reconcile` RPC handler's projection in
/// `meerkat-rpc/src/handlers/mob.rs`, with one meta-specific step: the
/// report's roster member ids are comms-safe encodings (meerkat 0.7
/// `MemberCommsName`), and this is a projection boundary, so every id is
/// decoded back to the public alias consoles/SDKs address members by.
pub fn meerkat_reconcile_report_to_wire(
    mob_id: &str,
    report: meerkat_mob::runtime::reconcile::ReconcileReport,
) -> MobReconcileReport {
    use meerkat_contracts::{MobSpawnReceiptWire, WireMemberRef};
    let alias_of =
        |id: &str| -> String { crate::member_comms_id::runtime_alias_str(id).into_owned() };
    MobReconcileReport {
        desired: report
            .desired
            .into_iter()
            .map(|id| alias_of(id.as_str()))
            .collect(),
        retained: report
            .retained
            .into_iter()
            .map(|id| alias_of(id.as_str()))
            .collect(),
        spawned: report
            .spawned
            .into_iter()
            .map(|receipt| {
                let identity_str = alias_of(receipt.agent_identity.as_str());
                MobSpawnReceiptWire {
                    member_ref: WireMemberRef::encode(mob_id, &identity_str),
                    agent_identity: identity_str,
                }
            })
            .collect(),
        retired: report
            .retired
            .into_iter()
            .map(|id| alias_of(id.as_str()))
            .collect(),
        failures: report
            .failures
            .into_iter()
            .map(|failure| MobReconcileFailure {
                agent_identity: alias_of(failure.agent_identity.as_str()),
                stage: match failure.stage {
                    meerkat_mob::runtime::reconcile::ReconcileStage::Spawn => {
                        meerkat_contracts::WireMobReconcileStage::Spawn
                    }
                    meerkat_mob::runtime::reconcile::ReconcileStage::Retire => {
                        meerkat_contracts::WireMobReconcileStage::Retire
                    }
                },
                error: meerkat_contracts::WireMobError {
                    code: meerkat_mob::mob_error_wire_code(&failure.error),
                    message: failure.error.to_string(),
                },
            })
            .collect(),
    }
}

// Eq is dropped because the canonical wire `MobReconcileReportWire` does
// not implement `Eq` (its nested types are PartialEq only).
#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedRuntimeReconcileReport {
    pub mob: MobReconcileReport,
    pub edges: UnifiedRuntimeReconcileEdgesReport,
    pub routing: UnifiedRuntimeReconcileRoutingReport,
}

#[derive(Debug)]
pub enum UnifiedRuntimeReconcileError {
    Mob(MobRuntimeError),
    RouteMutation(RuntimeRouteMutationError),
    /// Meerkat 0.6's `MobHandle::reconcile` collects per-identity failures
    /// into the returned report rather than returning `Err` on first failure.
    /// `UnifiedRuntime::reconcile` re-lifts that into an error variant so
    /// Rust callers using `?` still see failure propagation, while keeping
    /// the full report available for inspection.
    PartialFailure(Box<UnifiedRuntimeReconcileReport>),
}

impl Display for UnifiedRuntimeReconcileError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mob(err) => write!(f, "failed to reconcile mob roster: {err}"),
            Self::RouteMutation(err) => {
                write!(f, "failed to reconcile routing wiring: {err:?}")
            }
            Self::PartialFailure(report) => {
                write!(
                    f,
                    "reconcile completed with {} per-identity failure(s): {:?}",
                    report.mob.failures.len(),
                    report.mob.failures
                )
            }
        }
    }
}

impl std::error::Error for UnifiedRuntimeReconcileError {}

/// Which supervisor a replacement cleanup was joining. Meta runs
/// no lease-renewal or continuity-repair supervisor, so none is retired.
///
/// Replacement retires the previous supervisor, and the two are cancelled and
/// joined through different authority boundaries (a lease fencing-token
/// publication versus a restore pass's commit/rollback), so a cleanup that
/// fails to finish is worth naming rather than counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetiredSupervisorKind {
    LeaseRenewal,
    ContinuityRepair,
}

/// Outcome of joining the supervisor cleanups that replacement retired.
/// Nothing is retired and shutdown reports
/// [`Self::NothingPending`].
///
/// `start_identity_first_supervisors` may displace a running lease-renewal or
/// continuity-repair supervisor. Both are cancelled cooperatively, so the
/// process must still join them: their own doc comments make raw-aborting a
/// correctness error, because a lease renewal is joined through publication of
/// the provider's fencing token and a repair pass through its explicit
/// commit/rollback boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetiredSupervisorCleanupOutcome {
    /// Nothing was outstanding to join.
    ///
    /// Deliberately NOT called `NothingRetired`: finished cleanups are reaped at
    /// the next replacement, so an empty set means nothing remains pending, not
    /// that no supervisor was ever replaced. Claiming the latter would be
    /// unfalsifiable from this value.
    NothingPending,
    /// Every outstanding cleanup reached its release boundary, counted by which
    /// supervisor it was.
    Joined {
        lease_renewal: usize,
        continuity_repair: usize,
    },
    /// At least one cleanup did not reach its release boundary.
    ///
    /// One variant rather than separate failure and timeout cases so a run that
    /// hits both cannot report only one.
    ///
    /// `join_failed > 0` means a cleanup did not return normally, so the process
    /// cannot ATTEST that it reached its release boundary. Deliberately not
    /// called `panicked`: `JoinError` covers cancellation as well as panic, and
    /// even a panic can happen after the side effect but before returning, so
    /// "the fencing token was never published" would be a stronger causal claim
    /// than the evidence supports. What is certain is the absence of an
    /// attestation, which is enough to withhold one.
    ///
    /// `pending > 0` means the join budget expired with cleanups still running.
    ///
    /// Either count makes [`UnifiedRuntimeShutdownReport::cleanup_completed`]
    /// false. `join_failed` is a bare count because `JoinError` cannot recover
    /// which supervisor the task was carrying.
    Incomplete {
        joined: usize,
        join_failed: usize,
        pending: usize,
    },
}

#[derive(Debug)]
pub struct ShutdownDrainReport {
    pub drained_count: usize,
    pub timed_out: bool,
    pub drain_duration_ms: u64,
}

/// Whether the history a failed compaction preserved can still be sent to
/// the provider.
///
/// Meta-owned wire mirror of meerkat's
/// [`CompactionPreservedHistoryFit`](meerkat_core::event::CompactionPreservedHistoryFit)
/// discriminator, so `ErrorEvent`'s serialized shape does not track upstream's
/// `#[non_exhaustive]` growth: unknown future upstream verdicts degrade to
/// [`Self::Unclassified`] ("no verdict") at the drain seam instead of failing
/// deserialization downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionPreservedHistoryFit {
    /// No authority could answer at composed-request scope.
    Unclassified,
    /// The preserved history still composes a sendable request: the failed
    /// persistence costs a summary call and a retry, not progress.
    StillFits,
    /// The preserved history already exceeds the window or byte cap: every
    /// subsequent turn is refused until a compaction persists. Page.
    OverWindow,
}

impl From<meerkat_core::event::CompactionPreservedHistoryFit> for CompactionPreservedHistoryFit {
    fn from(fit: meerkat_core::event::CompactionPreservedHistoryFit) -> Self {
        use meerkat_core::event::CompactionPreservedHistoryFit as Upstream;
        match fit {
            Upstream::StillFits => Self::StillFits,
            Upstream::OverWindow => Self::OverWindow,
            // `Unclassified`, plus whatever `#[non_exhaustive]` upstream adds
            // later: an unknown verdict is still "no verdict this vocabulary
            // can route on".
            _ => Self::Unclassified,
        }
    }
}

/// Operational error event for alerting.
///
/// Fired via the `on_error` hook when runtime operations fail. Apps
/// match on variants to decide alerting (Slack, PagerDuty, log, etc.).
///
/// Marked `#[non_exhaustive]` — new variants can be added without
/// breaking downstream match arms (use a `_` wildcard).
///
/// **Wired fire points:**
/// - `SpawnFailure` — `mob_ops.rs` spawn error path
/// - `ReconcileIncomplete` - `edge_reconcile.rs` after `reconcile_edges`, when
///   an edge operation failed; a reconcile that only skipped edges for an
///   endpoint that is not a member yet logs at INFO instead. The event names
///   the first five failed operations, each with its edge and its error,
///   beside the counts, and an ERROR line beside it names each of them too
///   (then a count of the rest)
/// - `RediscoverFailure` — `lifecycle.rs` rediscover error path
/// - `HostLoopCrash` - `lifecycle.rs` drain fires the alert the agent-event
///   forwarder extracted from a member `RunFailed` whose typed class is not
///   `Cancelled` (a cancelled run is not a crash)
/// - `CheckpointFailure` — via `run_periodic_gc_with_error_callback` in session store
/// - `CompactionPersistenceRejected` — `lifecycle.rs` drain fires the alert the
///   agent-event forwarder extracted from a member `CompactionFailed` event
/// - `ActorLoopStalled` — `mod.rs` actor-loop probe's round trip through the
///   serialized mob command loop went unanswered past its budget
/// - `ActorLoopRecovered` — the same probe's parked round trip completed;
///   the resolution half of the stall, correlated by `stall_id`
/// - `ActorLoopTerminated` - the probe's round trip resolved with the actor's
///   command or reply channel CLOSED: the loop did not recover, it is gone,
///   and the process must restart
/// - `IdentityMaterializationFailure` — identity-first peer/fleet hydration skipped a member
/// - `MobStopProceededWithoutInterrupt` — `lifecycle.rs` teardown gave up
///   waiting for runtime-attach readiness and continued without interrupting
/// - `RetiredMemberReleaseFailed` - `identity_first/runtime.rs`: a gateway
///   host's retirement hook (`GatewayHostComposition::identity_member_retired`)
///   answered an error for a retired incarnation; the retirement stands
/// - `MobAnnouncementEscalated` - the mob announcement owner's escalation
///   pass raised an escalation, or notified it again while it stays open
/// - `MobAnnouncementEscalationResolved` - the same pass resolved it; the
///   resolution half, correlated by `escalation_id`
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "category", rename_all = "snake_case")]
pub enum ErrorEvent {
    SpawnFailure {
        member_id: String,
        profile: String,
        error: String,
    },
    ReconcileIncomplete {
        /// How many edge operations failed.
        failures: usize,
        /// How many edges were skipped beside them, an endpoint not being a
        /// member yet.
        skipped: usize,
        /// The failed edge operations, the first five in the order the
        /// reconcile met them, each with its operation, its edge and its
        /// error. Empty on events recorded before the field existed.
        #[serde(default)]
        failed: Vec<EdgeReconcileFailure>,
    },
    CheckpointFailure {
        session_id: String,
        error: String,
    },
    CompactionPersistenceRejected {
        identity: String,
        session_id: String,
        error: String,
        /// Whether the request the preserved history composes can still be
        /// sent: the severity discriminator between a wedged member (page)
        /// and a costly-but-progressing one (log line). Carried only when
        /// meerkat reported the typed projection-handoff refusal; other
        /// compaction failure reasons (and events recorded before this field
        /// existed) carry `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preserved_history: Option<CompactionPreservedHistoryFit>,
        /// Discard entries the refused durable handoff would have projected.
        /// Populated alongside `preserved_history`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempted_entries: Option<usize>,
    },
    ActorLoopStalled {
        probe_waited_secs: u64,
        detail: String,
        /// Correlates this stall with the [`Self::ActorLoopRecovered`] that
        /// closes it. A receiver that opens an incident here closes it on the
        /// resolution carrying the same id; without the pairing it can only
        /// ever escalate. `None` on events recorded before the id existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stall_id: Option<u64>,
        /// How many earlier stalls on this loop already resolved.
        ///
        /// This is chronic-busyness evidence, NOT a wedged verdict, and the
        /// direction matters: the probe parks on the same round trip instead
        /// of starting a new one, so a genuinely wedged loop pages once and
        /// never increments again, while a merely slow loop recovers and
        /// stalls afresh. A high count therefore means "repeatedly late",
        /// and a wedged loop is the one that sits at zero priors with no
        /// resolution ever arriving.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prior_resolved_stalls: Option<u64>,
    },
    /// The stalled round trip finally completed: the loop drains again.
    ///
    /// The only non-failure variant, and it exists because a paging channel
    /// that can only ever open incidents is not decidable — a receiver must
    /// be able to close from the same hook it opened from. It is filtered out
    /// of error-level logging by the default sink.
    ActorLoopRecovered {
        /// The [`Self::ActorLoopStalled`] this resolves.
        stall_id: u64,
        /// Wall-clock time from the probe's send to the reply, which is the
        /// receiver's evidence of how bad the stall actually was.
        stalled_for_secs: u64,
    },
    /// The probe's round trip resolved with the actor's command or reply
    /// channel closed: the serialized mob loop TERMINATED.
    ///
    /// Terminal, and deliberately not [`Self::ActorLoopRecovered`]. A
    /// fail-stopped actor closes its command channel, which resolves a parked
    /// probe with a typed channel-closed error; before this variant existed
    /// the probe read any completion as "the loop drains" and emitted a
    /// recovery for a mob that was dead (a 2026-09-04 production incident: `actor_loop_recovered`
    /// while every subsequent send failed instantly). Nothing in this process
    /// can restart the actor; the operator must restart the process. The
    /// delivery path fails fast on this state with `ActorTerminated`.
    ActorLoopTerminated {
        /// The [`Self::ActorLoopStalled`] whose parked round trip observed the
        /// closed channel, when termination was seen on a stalled probe rather
        /// than on a fresh round trip.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stall_id: Option<u64>,
        /// The channel-closed error text the probe observed.
        detail: String,
    },
    HostLoopCrash {
        member_id: String,
        error: String,
    },
    RediscoverFailure {
        error: String,
    },
    EventLogFlushFailure {
        error: String,
    },
    IdentityMaterializationFailure {
        identity: String,
        initiator: Option<String>,
        operation: String,
        error: String,
    },
    /// Teardown proceeded WITHOUT interrupting a member: the stop kept being
    /// refused on runtime-attach readiness for its whole window.
    ///
    /// Says exactly that, and nothing stronger. It does NOT claim the member
    /// was interrupted, because a mid-attach member has already had a turn
    /// admitted and its kickoff is about to bind - calling that an interrupt
    /// would be a false success the caller cannot detect.
    MobStopProceededWithoutInterrupt {
        waited_ms: u64,
        member: Option<String>,
        error: String,
    },
    /// A durable identity's member incarnation was retired, and the gateway
    /// host's retirement hook (`GatewayHostComposition::identity_member_retired`)
    /// answered an error.
    ///
    /// The retirement itself stands: Core retired the incarnation, and the
    /// retire's caller saw its own outcome. Only the host's release of what
    /// it keeps for that incarnation failed, and Meta does not retry it.
    /// `cause` is `unlisted`, `retire` or `scoped`.
    RetiredMemberReleaseFailed {
        identity: String,
        agent_runtime_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        cause: String,
        error: String,
    },
    /// A mob announcement escalation was raised, or notified again while it
    /// stays open: `escalation` is the durable record, `event` "raised" or
    /// "renotified", and `cursor` its event cursor, from which
    /// `meta/mob_announcements/escalations/list` reads on after a
    /// reconnect.
    MobAnnouncementEscalated {
        escalation: crate::mob_announcements::Escalation,
        event: crate::mob_announcements::EscalationEventKind,
        renotify_count: u32,
        cursor: u64,
    },
    /// The [`Self::MobAnnouncementEscalated`] with the same
    /// `escalation_id` resolved: a receiver that opened an incident on it
    /// closes it here.
    MobAnnouncementEscalationResolved {
        escalation_id: String,
        cause: crate::mob_announcements::EscalationCause,
        subject: crate::mob_announcements::EscalationSubject,
        resolved_at_ms: u64,
        cursor: u64,
    },
}

impl Display for ErrorEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnFailure {
                member_id, error, ..
            } => {
                write!(f, "spawn_failure: {member_id}: {error}")
            }
            Self::ReconcileIncomplete {
                failures,
                skipped,
                failed,
            } => {
                write!(
                    f,
                    "reconcile_incomplete: {failures} failures, {skipped} skipped"
                )?;
                if let Some(first) = failed.first() {
                    let (a, b) = first.edge.endpoints();
                    write!(
                        f,
                        "; first: {} {a} <-> {b}: {}",
                        first.operation, first.error
                    )?;
                }
                Ok(())
            }
            Self::CheckpointFailure { session_id, error } => {
                write!(f, "checkpoint_failure: {session_id}: {error}")
            }
            Self::CompactionPersistenceRejected {
                identity,
                session_id,
                error,
                ..
            } => {
                // `error` is the upstream reason's Display, which already
                // renders the fit and attempted-entry facts for the typed
                // handoff-refusal case.
                write!(
                    f,
                    "compaction_persistence_rejected: {identity} ({session_id}): {error}"
                )
            }
            Self::ActorLoopStalled {
                probe_waited_secs,
                detail,
                ..
            } => {
                write!(
                    f,
                    "actor_loop_stalled: probe unanswered for {probe_waited_secs}s: {detail}"
                )
            }
            Self::ActorLoopRecovered {
                stall_id,
                stalled_for_secs,
            } => {
                write!(
                    f,
                    "actor_loop_recovered: stall {stall_id} resolved after {stalled_for_secs}s"
                )
            }
            Self::ActorLoopTerminated { stall_id, detail } => match stall_id {
                Some(stall_id) => write!(
                    f,
                    "actor_loop_terminated: the mob actor loop is gone (observed on stall \
                     {stall_id}); this is NOT a recovery, the process must restart: {detail}"
                ),
                None => write!(
                    f,
                    "actor_loop_terminated: the mob actor loop is gone; this is NOT a recovery, \
                     the process must restart: {detail}"
                ),
            },
            Self::HostLoopCrash { member_id, error } => {
                write!(f, "host_loop_crash: {member_id}: {error}")
            }
            Self::RediscoverFailure { error } => {
                write!(f, "rediscover_failure: {error}")
            }
            Self::EventLogFlushFailure { error } => {
                write!(f, "event_log_flush_failure: {error}")
            }
            Self::IdentityMaterializationFailure {
                identity,
                initiator,
                operation,
                error,
            } => {
                if let Some(initiator) = initiator {
                    write!(
                        f,
                        "identity_materialization_failure: {identity} for {initiator} during {operation}: {error}"
                    )
                } else {
                    write!(
                        f,
                        "identity_materialization_failure: {identity} during {operation}: {error}"
                    )
                }
            }
            Self::MobStopProceededWithoutInterrupt {
                waited_ms,
                member,
                error,
            } => {
                let subject = match member {
                    Some(member) => format!(" on {member}"),
                    None => String::new(),
                };
                write!(
                    f,
                    "mob_stop_proceeded_without_interrupt: teardown proceeded after {waited_ms}ms \
                     WITHOUT a successful interrupt{subject}; a turn may have been admitted and \
                     may still be running: {error}"
                )
            }
            Self::RetiredMemberReleaseFailed {
                identity,
                agent_runtime_id,
                session_id,
                cause,
                error,
            } => {
                let session = match session_id {
                    Some(session_id) => format!(" on session {session_id}"),
                    None => String::new(),
                };
                write!(
                    f,
                    "retired_member_release_failed: {identity} ({agent_runtime_id}{session}) \
                     was retired ({cause}), and the host's release failed: {error}"
                )
            }
            Self::MobAnnouncementEscalated {
                escalation,
                event,
                renotify_count,
                cursor,
            } => {
                write!(
                    f,
                    "mob_announcement_escalated: {event} {} {} on {} (notified again \
                     {renotify_count} times, cursor {cursor})",
                    escalation.escalation_id, escalation.cause, escalation.subject
                )
            }
            Self::MobAnnouncementEscalationResolved {
                escalation_id,
                cause,
                subject,
                cursor,
                ..
            } => {
                write!(
                    f,
                    "mob_announcement_escalation_resolved: {escalation_id} {cause} on {subject} \
                     (cursor {cursor})"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed_shutdown_report() -> UnifiedRuntimeShutdownReport {
        UnifiedRuntimeShutdownReport {
            drain: ShutdownDrainReport {
                drained_count: 1,
                timed_out: false,
                drain_duration_ms: 2,
            },
            module_shutdown: RuntimeShutdownReport {
                terminated_modules: vec!["router".to_string()],
                orphan_processes: 0,
                physical_cleanup: crate::runtime::ModuleCleanupObservation::settled(),
            },
            mob_stop: Ok(()),
            identity_authority_release: IdentityAuthorityReleaseOutcome::NotConfigured,
            retired_supervisor_cleanup: RetiredSupervisorCleanupOutcome::NothingPending,
            mob_terminal_shutdown: super::MobTerminalShutdownOutcome::AlreadyShutDown,
            child_mob_stand_down: crate::child_mob_storage::ChildMobStandDownOutcome::NotConfigured,
            member_mcp_settlement: MemberMcpSettlement::NothingOwned,
        }
    }

    /// An unsettled member MCP close is never complete cleanup; a settled one
    /// and an empty custody are.
    #[test]
    fn member_mcp_settlement_gates_cleanup_completed() {
        let mut report = completed_shutdown_report();
        assert!(report.cleanup_completed());
        report.member_mcp_settlement = MemberMcpSettlement::Settled {
            routers: 2,
            servers: 3,
        };
        assert!(report.cleanup_completed());
        for counts in [
            MemberMcpUnsettled {
                deadline: 1,
                ..MemberMcpUnsettled::default()
            },
            MemberMcpUnsettled {
                wait_failed: 1,
                ..MemberMcpUnsettled::default()
            },
            MemberMcpUnsettled {
                not_reaped: 1,
                ..MemberMcpUnsettled::default()
            },
            MemberMcpUnsettled {
                stdout_open: 1,
                ..MemberMcpUnsettled::default()
            },
            MemberMcpUnsettled {
                stdout_unobserved: 1,
                ..MemberMcpUnsettled::default()
            },
            MemberMcpUnsettled {
                not_held: 1,
                ..MemberMcpUnsettled::default()
            },
            MemberMcpUnsettled {
                close_failed: 1,
                ..MemberMcpUnsettled::default()
            },
            MemberMcpUnsettled {
                task_failed: 1,
                ..MemberMcpUnsettled::default()
            },
        ] {
            report.member_mcp_settlement = MemberMcpSettlement::from_counts(counts.clone());
            assert_eq!(
                report.member_mcp_settlement,
                MemberMcpSettlement::Unsettled(counts)
            );
            assert!(!report.cleanup_completed());
            assert!(!report.cleanup_retained_until_process_exit());
        }
        assert_eq!(
            MemberMcpSettlement::from_counts(MemberMcpUnsettled {
                routers: 1,
                servers: 2,
                ..MemberMcpUnsettled::default()
            }),
            MemberMcpSettlement::Settled {
                routers: 1,
                servers: 2
            }
        );
    }

    #[test]
    fn held_durable_child_mobs_keep_cleanup_incomplete() {
        let mut report = completed_shutdown_report();
        assert!(report.cleanup_completed());
        // Clean child actors do not drain Meerkat's state-owned child work.
        report.child_mob_stand_down =
            crate::child_mob_storage::ChildMobStandDownOutcome::HeldUntilProcessExit {
                child_mobs: 1,
            };
        assert!(!report.cleanup_completed());
        assert!(report.cleanup_retained_until_process_exit());
        // An empty child root whose state work did not settle stays claimed.
        report.child_mob_stand_down =
            crate::child_mob_storage::ChildMobStandDownOutcome::HeldUntilProcessExit {
                child_mobs: 0,
            };
        assert!(!report.cleanup_completed());
        assert!(report.cleanup_retained_until_process_exit());
        // A proven stand-down released the root: cleanup is complete.
        report.child_mob_stand_down =
            crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs: 0 };
        assert!(report.cleanup_completed());
        assert!(!report.cleanup_retained_until_process_exit());
        // A stand-down still running through another clone at this call's
        // deadline: incomplete, and not the process-exit retention class.
        report.child_mob_stand_down =
            crate::child_mob_storage::ChildMobStandDownOutcome::InProgress;
        assert!(!report.child_mob_stand_down.child_actors_settled());
        assert!(!report.cleanup_completed());
        assert!(!report.cleanup_retained_until_process_exit());
        report.child_mob_stand_down =
            crate::child_mob_storage::ChildMobStandDownOutcome::HeldUntilProcessExit {
                child_mobs: 0,
            };
        report.drain.timed_out = true;
        assert!(
            !report.cleanup_retained_until_process_exit(),
            "another blocked phase is not process-exit retention"
        );
        report.drain.timed_out = false;
        report.child_mob_stand_down =
            crate::child_mob_storage::ChildMobStandDownOutcome::Retained {
                unsettled: vec![crate::child_mob_storage::RetainedChildMob {
                    mob_id: "child".to_string(),
                    kind: crate::child_mob_storage::RetainedChildMobKind::PendingUnregister,
                    detail: "unsettled".to_string(),
                }],
            };
        assert!(!report.child_mob_stand_down.child_actors_settled());
        assert!(!report.cleanup_completed());
        assert!(!report.cleanup_retained_until_process_exit());
        // Children that could not be listed: incomplete, and not the
        // process-exit retention class.
        report.child_mob_stand_down =
            crate::child_mob_storage::ChildMobStandDownOutcome::EnumerationFailed {
                detail: "child mobs could not be enumerated".to_string(),
            };
        assert!(!report.child_mob_stand_down.child_actors_settled());
        assert!(!report.cleanup_completed());
        assert!(!report.cleanup_retained_until_process_exit());
    }

    #[test]
    fn shutdown_cleanup_attestation_requires_every_authority_boundary() {
        let mut report = completed_shutdown_report();
        assert!(report.cleanup_completed());

        report.identity_authority_release =
            IdentityAuthorityReleaseOutcome::Released { grant_count: 1 };
        assert!(report.cleanup_completed());

        let mut report = completed_shutdown_report();
        report.drain.timed_out = true;
        assert!(!report.cleanup_completed());

        let mut report = completed_shutdown_report();
        report.mob_stop = Err(MobRuntimeError::InvalidConfig(
            "mob stop failed".to_string(),
        ));
        assert!(!report.cleanup_completed());

        let mut report = completed_shutdown_report();
        report.identity_authority_release = IdentityAuthorityReleaseOutcome::Failed {
            error: "provider release failed".to_string(),
        };
        assert!(!report.cleanup_completed());

        let mut report = completed_shutdown_report();
        report.identity_authority_release = IdentityAuthorityReleaseOutcome::SkippedMobStopFailed;
        assert!(!report.cleanup_completed());

        let mut report = completed_shutdown_report();
        report.identity_authority_release =
            IdentityAuthorityReleaseOutcome::SkippedResetCleanupFailed {
                error: "superseded member retained".to_string(),
            };
        assert!(!report.cleanup_completed());

        let mut report = completed_shutdown_report();
        report.module_shutdown.orphan_processes = 1;
        assert!(!report.cleanup_completed());
    }
}
