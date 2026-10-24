//! Library owner of durable job delivery.
//!
//! One owner per runtime delivery inbox projects the job outbox into the inbox
//! and applies pending inbox rows through a host: job rows recipient by
//! recipient, continuation rows into the session serving their address. It
//! is woken only by typed signals: a job outbox commit, a runtime delivery commit, a runtime
//! attachment becoming serving, a run settlement and a source change of the
//! host's continuation services. Arming runs one
//! reconcile pass over the stores first. A row whose application fails stays
//! pending and is retried on the next wake that names its runtime, or on the
//! next arming.
//!
//! On a store shared by several processes (#1813) each recipient is applied
//! by the runtime owner that hosts the recipient's session; recipients no
//! process hosts are applied by the store's single cold-delivery owner, under
//! the session's claim taken before any delivery-authority input. See
//! [`crate::DeliveryRoute`]. Another process's commits and deaths raise no
//! signal in this process, so the owner also runs the shared SQLite change
//! watch over the store's database (the contract the mob event bus uses): a
//! coalesced file notification, or a bounded sweep when none arrived. A tick
//! is a HINT, never permission: the owner answers it with cheap reads (one
//! delivery-generation row, a try of the cold-delivery lock, the local
//! routes of the recipients it is waiting on) and reconciles only when one of
//! them moved; the cold-delivery owner also retries its waiting cold
//! recipients, whose claim attempt is the delivery itself. The sweep bounds
//! how long a missed notification or a dead peer's released claim can delay
//! a delivery; it is not a correctness mechanism (custody stays with the
//! hosting claims and the delivery CAS).
//!
//! A host whose producers owe continuations it commits for them (consent
//! notices) has them projected in rounds
//! ([`RuntimeDeliveryHost::project_owed_continuations`]): a reconcile or a
//! source change starts one, and it runs one finite page per pass, before
//! the pass collects the inbox, until its enumeration ends.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::future::{BoxFuture, FutureExt as _, Shared};
use meerkat_core::SessionId;
use meerkat_jobs::DetachedJobStore;
use meerkat_runtime::{
    ColdDeliveryOwnership, DeliveryGenerationReader, HostingCapability, HostingClaim,
    HostingRefused, KEPT_DELIVERY_READER_IDLE, LogicalRuntimeId, RetainedNotificationRegistration,
    RuntimeDeliveryError, RuntimeDeliveryInbox, RuntimeDeliveryKind,
    RuntimeDeliveryOwnerAlreadyArmed, RuntimeDeliveryOwnership,
};
use tokio::sync::watch;

use crate::{
    DeliveryRoute, JobDeliveryRouter, JobOutboxProjectionError, JobOutboxProjector,
    JobRuntimeDeliveryApplier,
};

/// Rows read per projection or application page.
const DELIVERY_PAGE: usize = 256;

/// The longest gap, in wakes, between two retries of a job projection that
/// keeps failing.
const PROJECTION_RETRY_MAX_WAKES: u32 = 64;

/// The retry schedule of a job projection that failed (the projector
/// skipped an entry, which stays pending in the outbox). The retry is owed
/// to later wakes and never driven by itself: the first one joins the next
/// wake, and while the projection keeps failing each further retry waits
/// for twice as many wakes, up to [`PROJECTION_RETRY_MAX_WAKES`]. A busy
/// runtime's stream of unrelated wakes (it settles a run on every loop
/// iteration) therefore does not retry it on every one. A job commit or a
/// reconcile still projects at once, and a projection that skips nothing
/// clears the schedule.
#[derive(Debug, Default)]
struct ProjectionRetry {
    /// Wakes left until the next retry; `None` when none is owed.
    wakes_left: Option<u32>,
    /// Projections that failed since the last one that skipped nothing.
    failures: u32,
}

impl ProjectionRetry {
    /// A projection skipped an entry: owe a retry after the next gap.
    fn failed(&mut self) {
        let gap = 1_u32
            .checked_shl(self.failures)
            .map_or(PROJECTION_RETRY_MAX_WAKES, |gap| {
                gap.min(PROJECTION_RETRY_MAX_WAKES)
            });
        self.failures = self.failures.saturating_add(1);
        self.wakes_left = Some(gap);
    }

    /// One wake arrived: whether the owed retry joins it.
    fn due_on_wake(&mut self) -> bool {
        let Some(left) = self.wakes_left.as_mut() else {
            return false;
        };
        *left = left.saturating_sub(1);
        if *left > 0 {
            return false;
        }
        self.wakes_left = None;
        true
    }
}

/// Default sweep of the store watch on a shared store: the bound on how long
/// a missed file notification, a dead peer's released claim or a released
/// cold-delivery lock can delay a delivery. The same 5 s the mob event bus
/// uses.
const DEFAULT_STORE_SWEEP: Duration = Duration::from_secs(5);

/// The host side of a [`RuntimeDeliveryOwner`]: where deliveries are applied.
#[async_trait::async_trait]
pub trait RuntimeDeliveryHost: Send + Sync {
    /// Where an application whose RECIPIENT is `session_id` goes, or `None`
    /// once the host is gone (the owner then stops). Routing is per
    /// recipient, never per origin row, and reads the host runtime owner's
    /// claim registry only (#1813).
    async fn delivery_route(&self, session_id: &SessionId) -> Option<DeliveryRoute>;

    /// Take the host runtime owner's hosting claim of `session_id` for a cold
    /// delivery by this process (#1813): `Ok(claim)` while no other runtime
    /// owner holds it, else [`HostingRefused`] (another owner holds it, or
    /// the claim is unavailable). `None` once the host is
    /// gone. The owner calls it only as the store's cold-delivery owner, for
    /// a [`DeliveryRoute::Unserved`] recipient, before any delivery-authority
    /// input.
    async fn claim_cold_delivery(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<HostingClaim, HostingRefused>>;

    /// The session serving a continuation delivery address now. The default
    /// serves a session address (`rt:session:{id}`) by that session and
    /// reports every other address as not served; hosts with mob members
    /// resolve member addresses through their roster.
    async fn resolve_address(&self, address: &LogicalRuntimeId) -> crate::AddressResolution {
        default_address_resolution(address)
    }

    /// Observe the incarnation behind a continuation delivery address
    /// without serving it, as a continuation status read does. The drain
    /// calls it after a row's reservation and before its admission: Retired
    /// releases the reservation and admits nothing, which is what makes a
    /// stranded status final. So it must agree on Retired with the status
    /// read of the same continuations
    /// ([`crate::ContinuationOwnerService::continuation_status`]): hosts with
    /// bound continuation services delegate to
    /// [`crate::ContinuationHostBindings::observe_address`]. The default is
    /// derived from [`Self::resolve_address`] (Retired stays Retired, and
    /// anything else may serve), which can revive a member. `Err` when the
    /// address cannot be observed now: the row stays blocked.
    async fn observe_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<crate::AddressObservation, String> {
        Ok(match self.resolve_address(address).await {
            crate::AddressResolution::Retired => crate::AddressObservation::Retired,
            crate::AddressResolution::Session(_) | crate::AddressResolution::NotServed => {
                crate::AddressObservation::MayServe
            }
        })
    }

    /// The sink that admits continuation deliveries into `session_id`, if
    /// this host admits continuations. Without one, continuation rows stay
    /// visibly blocked as an unsupported kind.
    async fn continuation_sink(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn crate::ContinuationDeliverySink>> {
        let _ = session_id;
        None
    }

    /// The committed owner of fork_off and council jobs, which confirms a
    /// retained completion before a governed runtime admits it. Without one,
    /// such a row stays pending (retryable), never settled.
    fn retained_job_source(&self) -> Option<Arc<dyn crate::RetainedJobSource>> {
        None
    }

    /// Whether this host's producers owe continuations it projects
    /// ([`Self::project_owed_continuations`]). For such a host a source
    /// change starts a projection round whether or not anything is blocked,
    /// so an obligation committed after the owner armed is projected on its
    /// hint. Run settlements and attachments never start one.
    fn projects_owed_continuations(&self) -> bool {
        false
    }

    /// Changes of the source behind [`Self::project_owed_continuations`],
    /// subscribed once when the owner arms. Every advance wakes the owner
    /// exactly as a continuation-source change does: it retries what is
    /// blocked and starts a round of the owed continuations (or asks for one
    /// more), so an obligation the source commits and announces after the
    /// owner armed is projected without a reconcile. Only a wake, never a
    /// fact. `None`, the default, for a source that announces nothing.
    fn subscribe_owed_continuation_changes(&self) -> Option<watch::Receiver<u64>> {
        None
    }

    /// Commit, idempotently by their continuation keys, the rows of the
    /// obligations on ONE page of the host's enumeration of what its
    /// producers owe: at most `limit` obligations, starting after `after`
    /// (from the first when `None`). Only rows this call commits count; an
    /// obligation whose row is already committed stays owed until its
    /// admission and counts nothing.
    ///
    /// The owner calls it once per pass, before it collects the inbox, so
    /// the rows committed here drain in the same pass. A round starts on the
    /// arming reconcile, on every reconcile and on every source change, and
    /// continues page by page, one per pass, until a page reports no
    /// [`OwedContinuationProjection::next`]; it is never repeated because
    /// a page committed rows. The enumeration must be in a stable order and
    /// each page's `next` past its start, so that a round reaches its end
    /// and visits every obligation owed throughout it.
    async fn project_owed_continuations(
        &self,
        after: Option<&OwedContinuationCursor>,
        limit: usize,
    ) -> OwedContinuationProjection {
        let _ = (after, limit);
        OwedContinuationProjection::default()
    }
}

/// Where a host's enumeration of owed continuations continues. Opaque to the
/// owner, which hands the one a page returned to the next page of the same
/// round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedContinuationCursor(String);

impl OwedContinuationCursor {
    pub fn new(position: impl Into<String>) -> Self {
        Self(position.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What one page of [`RuntimeDeliveryHost::project_owed_continuations`]
/// did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwedContinuationProjection {
    /// Rows this page committed; an obligation whose row was already
    /// committed is not counted.
    pub committed: usize,
    /// Obligations of this page that could not be committed now, or an
    /// enumeration that could not be read: each stays owed, and the next
    /// wake after the round starts another.
    pub failures: Vec<String>,
    /// Where the next page of this round starts, when the enumeration goes
    /// on past this page; `None` ends the round.
    pub next: Option<OwedContinuationCursor>,
}

/// Session addresses are served by their session; nothing else is.
pub fn default_address_resolution(address: &LogicalRuntimeId) -> crate::AddressResolution {
    match address
        .0
        .strip_prefix("rt:session:")
        .and_then(|raw| SessionId::parse(raw).ok())
    {
        Some(session) => crate::AddressResolution::Session(session),
        None => crate::AddressResolution::NotServed,
    }
}

/// Outcome of one owner pass, published after every pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeDeliveryPass {
    /// Passes completed since arming; the first is the reconcile pass.
    pub generation: u64,
    /// Outbox entries projected into the inbox this pass.
    pub projected: usize,
    /// Rows of owed continuations the host committed this pass: one page
    /// of a round ([`RuntimeDeliveryHost::project_owed_continuations`]).
    pub owed_projected: usize,
    /// Inbox rows applied this pass.
    pub applied: usize,
    /// Continuation rows settled as refused this pass: terminal policy
    /// outcomes, never applied, each also reported in `failures`.
    pub refused: usize,
    /// Sessions whose rows stayed pending after this pass, awaiting a wake
    /// that names them.
    pub blocked_sessions: Vec<SessionId>,
    /// Delivery runtimes whose rows stayed pending after this pass, including
    /// continuation addresses no session serves yet.
    pub blocked_runtimes: Vec<LogicalRuntimeId>,
    /// Failures observed this pass, in order.
    pub failures: Vec<String>,
    /// Origin runtimes whose next row waits on recipients this process does
    /// not serve (another process hosts them, or they are cold and another
    /// process is the cold-delivery owner). Not failures (#1813).
    pub awaiting_other_hosts: usize,
    /// Whether this process is the store's cold-delivery owner (or the only
    /// process).
    pub applies_cold_deliveries: bool,
    /// Set when rows committed by other processes cannot wake this owner (the
    /// store's database cannot be watched). Health must report it; explicit
    /// multi-process startup refuses it up front.
    pub cross_process_wake_unavailable: Option<String>,
}

/// The stores and signals one owner is armed over.
#[derive(Clone)]
pub struct RuntimeDeliveryOwner {
    job_store: Arc<dyn DetachedJobStore>,
    runtime_inbox: RuntimeDeliveryInbox,
    attachment_commits: Option<watch::Receiver<u64>>,
    run_settlements: Option<watch::Receiver<u64>>,
    source_changes: Option<watch::Receiver<u64>>,
    store_sweep: Duration,
}

impl std::fmt::Debug for RuntimeDeliveryOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDeliveryOwner")
            .finish_non_exhaustive()
    }
}

impl RuntimeDeliveryOwner {
    pub fn new(job_store: Arc<dyn DetachedJobStore>, runtime_inbox: RuntimeDeliveryInbox) -> Self {
        Self {
            job_store,
            runtime_inbox,
            attachment_commits: None,
            run_settlements: None,
            source_changes: None,
            store_sweep: DEFAULT_STORE_SWEEP,
        }
    }

    /// The job store this owner projects from.
    pub fn job_store(&self) -> Arc<dyn DetachedJobStore> {
        self.job_store.clone()
    }

    /// Retry blocked sessions when a runtime attachment becomes serving, for
    /// example from [`meerkat_runtime::MeerkatMachine::subscribe_attachment_commits`].
    /// When this signal closes (its runtime is gone) the owner stops.
    #[must_use]
    pub fn with_attachment_commits(mut self, attachment_commits: watch::Receiver<u64>) -> Self {
        self.attachment_commits = Some(attachment_commits);
        self
    }

    /// Also retry blocked sessions when a run may have ended, for example
    /// from [`meerkat_runtime::MeerkatMachine::subscribe_run_settlements`]: a
    /// session refuses a delivery while a callback tool batch awaits its
    /// results, and that batch resolves inside a run. When this signal closes
    /// the owner stops.
    #[must_use]
    pub fn with_run_settlements(mut self, run_settlements: watch::Receiver<u64>) -> Self {
        self.run_settlements = Some(run_settlements);
        self
    }

    /// Also retry blocked deliveries when the host's continuation sources
    /// change, from
    /// [`crate::ContinuationHostBindings::subscribe_source_changes`]: the
    /// services were bound or rebound, or the bound job owner announced that
    /// its custody may have changed. A retained completion stays blocked
    /// while its job owner cannot answer, and no attachment or run signal
    /// says when it can. Subscribe before arming, so a change after the
    /// arming reconcile is never missed. The signal is only a wake, never a
    /// fact: the retry asks the source again, and its revision is scheduling
    /// information only. When the bindings are gone the signal ends and the
    /// owner keeps running on its other signals.
    #[must_use]
    pub fn with_source_changes(mut self, source_changes: watch::Receiver<u64>) -> Self {
        self.source_changes = Some(source_changes);
        self
    }

    /// The sweep interval of the store watch on a shared store (default
    /// 5 s): the bound on how long another process's commit or death can go
    /// unnoticed when no file notification arrives. Ignored on a store
    /// without cross-process hosting.
    #[must_use]
    pub fn with_store_sweep(mut self, sweep: Duration) -> Self {
        self.store_sweep = sweep;
        self
    }

    /// Arm the owner: claim the inbox, run the reconcile pass, then apply on
    /// typed wakes until the returned handle is dropped or the host is gone.
    ///
    /// Must be called inside a tokio runtime. Fails with
    /// [`RuntimeDeliveryOwnerAlreadyArmed`] while another owner holds the
    /// inbox; there is never a second loop over one inbox.
    pub fn arm(
        self,
        host: Arc<dyn RuntimeDeliveryHost>,
    ) -> Result<RuntimeDeliveryOwnerHandle, RuntimeDeliveryOwnerAlreadyArmed> {
        let ownership = self.runtime_inbox.claim_delivery_ownership()?;
        let (passes, passes_rx) = watch::channel(RuntimeDeliveryPass::default());
        let job_commits = self.job_store.outbox_commit_signal().subscribe();
        let inbox_commits = self.runtime_inbox.subscribe_commits();
        let capability = self.runtime_inbox.hosting_capability();
        // Order matters (#1813): the watch starts BEFORE the reconcile pass
        // reads the delivery generation, so a commit landing in between is
        // seen by the pass or ticks the watch (at worst at the next sweep).
        let retained = Arc::new(OnceLock::new());
        let store_watch = StoreWake::start(&capability, self.store_sweep, &retained);
        // Each tick of a watched store reads the delivery generation: through
        // the store's kept reader when it keeps one. An unwatched
        // store reads it only when a pass reconciles, and keeps nothing.
        let generation_reader = if store_watch.is_watching() {
            self.runtime_inbox
                .delivery_generation_reader()
                .map(std::sync::Mutex::new)
        } else {
            None
        };
        let cold = meerkat_runtime::try_cold_delivery_ownership(&capability);
        let (stop, stop_rx) = watch::channel(false);
        let projects_owed = host.projects_owed_continuations();
        let owed_changes = host.subscribe_owed_continuation_changes();
        let task = tokio::spawn(run_owner(OwnerLoop {
            projector: JobOutboxProjector::new(self.job_store, self.runtime_inbox.clone()),
            ownership,
            router: Arc::new(HostRouter(Arc::clone(&host))),
            host,
            job_commits,
            inbox_commits,
            attachment_commits: self.attachment_commits,
            run_settlements: self.run_settlements,
            source_changes: self.source_changes,
            owed_changes,
            projects_owed,
            passes,
            capability,
            store_watch,
            last_delivery_generation: None,
            generation_reader,
            generation_reader_idle_at: None,
            answered_sweeps: 0,
            cold,
            awaiting: HashMap::new(),
            stop: Some(stop_rx),
            owed_round: None,
        }));
        Ok(RuntimeDeliveryOwnerHandle {
            task: Some(OwnerTask::new(task, retained)),
            passes: passes_rx,
            stop,
        })
    }
}

/// A running owner. Dropping it stops the owner and releases the inbox,
/// unless it was [detached](Self::detach).
pub struct RuntimeDeliveryOwnerHandle {
    task: Option<OwnerTask>,
    passes: watch::Receiver<RuntimeDeliveryPass>,
    /// Retirement's stop request. Dropping it (a detached owner) is not one.
    stop: watch::Sender<bool>,
}

/// The owner's task: aborted when its handle drops, and joined by every
/// retirement through one shared join of the task.
struct OwnerTask {
    abort: tokio::task::AbortHandle,
    joined: Shared<BoxFuture<'static, RuntimeDeliveryOwnerRetired>>,
}

impl OwnerTask {
    /// `retained` is where the task's store watch records what its stop
    /// left with the process.
    fn new(
        task: tokio::task::JoinHandle<()>,
        retained: Arc<OnceLock<RetainedNotificationRegistration>>,
    ) -> Self {
        let abort = task.abort_handle();
        let joined = async move {
            if let Err(error) = task.await
                && error.is_panic()
            {
                tracing::warn!(%error, "runtime delivery owner task panicked");
            }
            // However the task ended, its end dropped the store watch, and
            // with it recorded what the watch's stop left with the process.
            let retired = RuntimeDeliveryOwnerRetired {
                notification_registration_retained: retained.get().cloned(),
            };
            // The one trace of the report: this future runs once per task,
            // however many retirements join it.
            if let Some(registration) = &retired.notification_registration_retained {
                tracing::info!(
                    directory = %registration.directory.display(),
                    "runtime delivery owner retired; its store watch's file notification \
                     registration stays with the process until the platform call returns"
                );
            }
            retired
        }
        .boxed()
        .shared();
        Self { abort, joined }
    }
}

/// A wait on a retiring owner's task, from
/// [`RuntimeDeliveryOwnerHandle::retire`].
#[must_use = "a retirement is confirmed only once joined"]
pub struct RuntimeDeliveryOwnerRetirement {
    joined: Option<Shared<BoxFuture<'static, RuntimeDeliveryOwnerRetired>>>,
}

impl std::fmt::Debug for RuntimeDeliveryOwnerRetirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDeliveryOwnerRetirement")
            .finish_non_exhaustive()
    }
}

impl RuntimeDeliveryOwnerRetirement {
    /// Join the owner's task within `bound`. A joined owner has released the
    /// inbox, its store watch and any cold-delivery claim; the report says
    /// what the store watch's stop left with the process.
    pub async fn join(
        self,
        bound: Duration,
    ) -> Result<RuntimeDeliveryOwnerRetired, RuntimeDeliveryOwnerRetirementIncomplete> {
        let Some(joined) = self.joined else {
            return Ok(RuntimeDeliveryOwnerRetired::default());
        };
        tokio::time::timeout(bound, joined)
            .await
            .map_err(|_| RuntimeDeliveryOwnerRetirementIncomplete { bound })
    }
}

/// What a joined owner left with the process
/// ([`RuntimeDeliveryOwnerRetirement::join`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
#[must_use = "the report says what the retirement left with the process"]
pub struct RuntimeDeliveryOwnerRetired {
    /// The file notification registration of the owner's store watch, when
    /// the operating system still held it as the owner stopped the watch:
    /// at retirement, or earlier if the watch's thread ended and the owner
    /// went on without cross-process wake.
    /// Nothing can interrupt that platform call (macOS FSEvents can hold it
    /// for tens of seconds), so the stop cancels the registration without
    /// waiting for it: its thread stays with the process until the call
    /// returns, and never reads or writes the store. `None` when the
    /// registration had ended, or the store is not watched.
    pub notification_registration_retained: Option<RetainedNotificationRegistration>,
}

/// The delivery owner's task did not stop within its retirement bound. Its
/// handle keeps the task, so a later retirement joins the same task.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("the runtime delivery owner did not stop within {bound:?}")]
pub struct RuntimeDeliveryOwnerRetirementIncomplete {
    pub bound: Duration,
}

impl std::fmt::Debug for RuntimeDeliveryOwnerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDeliveryOwnerHandle")
            .finish_non_exhaustive()
    }
}

impl RuntimeDeliveryOwnerHandle {
    /// Observe the outcome of each pass.
    pub fn subscribe_passes(&self) -> watch::Receiver<RuntimeDeliveryPass> {
        self.passes.clone()
    }

    /// Whether the owner has stopped (its host or its runtime is gone).
    pub fn is_stopped(&self) -> bool {
        self.task
            .as_ref()
            .is_none_or(|task| task.abort.is_finished())
    }

    /// Let the owner run on its own until its host or its runtime is gone.
    pub fn detach(mut self) {
        self.task = None;
    }

    /// Ask the owner to stop, and wait on its task through the returned
    /// retirement.
    ///
    /// No pass starts after the request; a pass already running completes.
    /// Every retirement of one owner joins the same task, which this handle
    /// keeps: a wait that ends early, past its bound or cancelled, leaves
    /// the owner and its custody here for the next retirement.
    pub fn retire(&self) -> RuntimeDeliveryOwnerRetirement {
        self.stop.send_replace(true);
        RuntimeDeliveryOwnerRetirement {
            joined: self.task.as_ref().map(|task| task.joined.clone()),
        }
    }
}

impl Drop for RuntimeDeliveryOwnerHandle {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort.abort();
        }
    }
}

/// Routes each recipient through the owner's host.
struct HostRouter(Arc<dyn RuntimeDeliveryHost>);

#[async_trait::async_trait]
impl JobDeliveryRouter for HostRouter {
    async fn route(&self, recipient: &SessionId) -> Option<DeliveryRoute> {
        self.0.delivery_route(recipient).await
    }

    async fn claim_cold(
        &self,
        recipient: &SessionId,
    ) -> Option<Result<HostingClaim, HostingRefused>> {
        self.0.claim_cold_delivery(recipient).await
    }
}

/// The store watch of a shared store, or why it is unavailable.
enum StoreWake {
    /// The store has no cross-process hosting: nothing to watch.
    NotShared,
    #[cfg(not(target_arch = "wasm32"))]
    Watching {
        _watch: OwnedStoreWatch,
        ticks: watch::Receiver<u64>,
        /// The watch's sweeps, counted before the tick each moves.
        sweeps: watch::Receiver<u64>,
    },
    Unavailable(String),
}

/// The owner's store watch, stopped wherever the owner lets it go: when
/// its task ends (returned, panicked or aborted), or when the loop replaces
/// a watch whose thread ended. A file notification registration the stop
/// leaves with the process is recorded for the owner's retirement.
#[cfg(not(target_arch = "wasm32"))]
struct OwnedStoreWatch {
    watch: Option<meerkat_runtime::DeliveryStoreWatch>,
    retained: Arc<OnceLock<RetainedNotificationRegistration>>,
}

// Runs in the owner task's teardown, also while a panic unwinds, so it must
// never block or panic: the stop only requests the watch's stop and reads its
// registration slot, and dropping the watch does not join its threads.
#[cfg(not(target_arch = "wasm32"))]
impl Drop for OwnedStoreWatch {
    fn drop(&mut self) {
        if let Some(retained) = self
            .watch
            .take()
            .and_then(meerkat_runtime::DeliveryStoreWatch::stop)
        {
            // An owner starts one store watch, so this is the only record.
            let _ = self.retained.set(retained);
        }
    }
}

impl StoreWake {
    fn start(
        capability: &HostingCapability,
        sweep: Duration,
        retained: &Arc<OnceLock<RetainedNotificationRegistration>>,
    ) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match meerkat_runtime::watch_delivery_store(capability, sweep) {
                Ok(None) => Self::NotShared,
                Ok(Some(watch)) => Self::Watching {
                    ticks: watch.subscribe_ticks(),
                    sweeps: watch.subscribe_sweeps(),
                    _watch: OwnedStoreWatch {
                        watch: Some(watch),
                        retained: Arc::clone(retained),
                    },
                },
                Err(reason) => {
                    tracing::warn!(
                        %reason,
                        "delivery owner cannot watch for rows committed by other processes"
                    );
                    Self::Unavailable(reason)
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (sweep, retained);
            if capability.is_cross_process() {
                Self::Unavailable("a browser build has no SQLite store watch".to_string())
            } else {
                Self::NotShared
            }
        }
    }

    fn is_watching(&self) -> bool {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Watching { .. } => true,
            Self::NotShared | Self::Unavailable(_) => false,
        }
    }

    fn unavailable_reason(&self) -> Option<String> {
        match self {
            Self::Unavailable(reason) => Some(reason.clone()),
            Self::NotShared => None,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Watching { .. } => None,
        }
    }

    /// Whether the watch swept since the sweep count `answered`, which this
    /// moves on to the current count. The watch counts a sweep before it
    /// moves the tick, so a sweep among the ticks being answered is counted.
    fn swept_since(&self, answered: &mut u64) -> bool {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Watching { sweeps, .. } => {
                let sweeps = *sweeps.borrow();
                let swept = sweeps != *answered;
                *answered = sweeps;
                swept
            }
            Self::NotShared | Self::Unavailable(_) => {
                let _ = answered;
                false
            }
        }
    }

    /// Resolves on the next tick; never on a store that is not watched.
    async fn tick(&mut self) -> Result<(), watch::error::RecvError> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Watching { ticks, .. } => ticks.changed().await,
            Self::NotShared | Self::Unavailable(_) => std::future::pending().await,
        }
    }
}

struct OwnerLoop {
    projector: JobOutboxProjector,
    ownership: RuntimeDeliveryOwnership,
    host: Arc<dyn RuntimeDeliveryHost>,
    router: Arc<dyn JobDeliveryRouter>,
    job_commits: watch::Receiver<u64>,
    inbox_commits: watch::Receiver<u64>,
    attachment_commits: Option<watch::Receiver<u64>>,
    run_settlements: Option<watch::Receiver<u64>>,
    /// The host's continuation source hint; `None` once its bindings are
    /// gone.
    source_changes: Option<watch::Receiver<u64>>,
    /// The owing source's own change signal
    /// ([`RuntimeDeliveryHost::subscribe_owed_continuation_changes`]), read
    /// as a source change; `None` once the source is gone or when it
    /// announces nothing.
    owed_changes: Option<watch::Receiver<u64>>,
    /// The host projects owed continuations: a reconcile or a source change
    /// starts a round of them, whether or not anything is blocked.
    projects_owed: bool,
    passes: watch::Sender<RuntimeDeliveryPass>,
    capability: HostingCapability,
    store_watch: StoreWake,
    /// The store's durable delivery generation as last reconciled; a store
    /// tick reconciles only when it moved.
    last_delivery_generation: Option<u64>,
    /// The store's kept reader of its delivery generation: on a
    /// watched store whose store keeps one; `None` reads through the inbox.
    /// One per owner, so one kept connection per runtime delivery inbox. The
    /// loop reaches it only through `&mut self` (`Mutex::get_mut`, which never
    /// locks); the mutex only makes the loop `Sync` for the `&OwnerLoop`
    /// borrows its passes hold across awaits.
    generation_reader: Option<std::sync::Mutex<Box<dyn DeliveryGenerationReader>>>,
    /// When the kept reader closes its connection for quiet
    /// ([`KEPT_DELIVERY_READER_IDLE`] after its last read); `None` while it
    /// keeps nothing.
    generation_reader_idle_at: Option<tokio::time::Instant>,
    /// The store watch's sweep count when the owner last answered its ticks:
    /// a sweep since reads, while ticks that were file notifications only
    /// may be answered without opening the database.
    answered_sweeps: u64,
    cold: ColdDeliveryOwnership,
    /// The handle's retirement request. `None` once the handle is gone
    /// without one (detached): the owner then runs on its other signals.
    stop: Option<watch::Receiver<bool>>,
    /// Delivery runtimes whose next row waits on recipients this process
    /// did not apply. A store tick re-routes the recipients (registry reads
    /// only) and retries a runtime once one became this process's to apply:
    /// served here, or cold while this process is the cold-delivery owner
    /// (its claim attempt then decides).
    awaiting: HashMap<LogicalRuntimeId, AwaitingWait>,
    /// The round of the host's owed continuations in progress, if any.
    owed_round: Option<OwedRound>,
}

/// One enumeration of a host's owed continuations, a page per pass, from
/// its first page to a page that reports no next one. Started by a reconcile
/// or a source change; each following pass continues it without waiting for
/// a signal. Obligations stay owed until their admission, so a round is
/// never repeated because a page committed rows: it ends at the end of the
/// enumeration, and only a later reconcile or source change starts another.
#[derive(Debug, Default)]
struct OwedRound {
    /// Where the next page starts; `None` for the first.
    after: Option<OwedContinuationCursor>,
    /// A reconcile or source change arrived after this round started. What
    /// it announces may sit before the cursor, so one more round follows
    /// this one, from the first page.
    again: bool,
}

/// What a waiting delivery runtime waits on.
struct AwaitingWait {
    /// The sessions of the recipients the last pass skipped.
    recipients: Vec<SessionId>,
    /// The runtime's page session came from the host's address resolution
    /// (a continuation address). Its recipient is whichever session serves
    /// the address NOW: a member can repoint to another session without
    /// changing its address, so a retry re-resolves it instead of routing
    /// the cached session, and an attachment wake (the host's mapping
    /// change) retries it.
    via_address: bool,
}

impl OwnerLoop {
    /// The store's delivery generation, through the store's reader when there
    /// is one. A read that leaves a connection kept restarts its idle bound.
    async fn read_delivery_generation(
        &mut self,
        purpose: GenerationRead,
    ) -> Result<u64, RuntimeDeliveryError> {
        let Some(reader) = self.generation_reader.as_mut() else {
            return self.ownership.inbox().delivery_generation().await;
        };
        let reader = reader
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let read = match purpose {
            GenerationRead::Read => reader.read().await,
            GenerationRead::Notification => reader.read_on_notification().await,
        };
        self.generation_reader_idle_at = reader
            .keeps_connection()
            .then(|| tokio::time::Instant::now() + KEPT_DELIVERY_READER_IDLE);
        Ok(read?)
    }

    /// Close the kept reader's connection: once the store has been quiet for
    /// [`KEPT_DELIVERY_READER_IDLE`], and before the owner calls into the host
    /// (a delivery, a round of owed continuations, an address resolution),
    /// which can wait.
    fn release_generation_reader(&mut self) {
        if let Some(reader) = self.generation_reader.as_mut() {
            reader
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .release();
        }
        self.generation_reader_idle_at = None;
    }

    /// Answer a store tick with cheap reads: take cold-delivery ownership if
    /// its holder released it (or died), and return the awaiting delivery
    /// runtimes that have a recipient this process may now apply. Reconciles (sets
    /// `wake.reconcile`) when cold ownership was taken or the store's
    /// delivery generation moved. `None` once the host is gone.
    async fn answer_store_tick(&mut self, wake: &mut Wake) -> Option<Vec<LogicalRuntimeId>> {
        if !self.cold.applies_cold_deliveries() {
            let attempt = meerkat_runtime::try_cold_delivery_ownership(&self.capability);
            if attempt.applies_cold_deliveries() {
                self.cold = attempt;
                wake.reconcile = true;
            }
        }
        // Ticks that carried no sweep were file notifications only: the
        // reader may answer them without opening the database. A sweep reads.
        let swept = self.store_watch.swept_since(&mut self.answered_sweeps);
        if !wake.reconcile {
            // One row read. Writes by sessions and other stores tick the
            // watch too; they cost only this read.
            let purpose = if swept {
                GenerationRead::Read
            } else {
                GenerationRead::Notification
            };
            match self.read_delivery_generation(purpose).await {
                Ok(current) if self.last_delivery_generation == Some(current) => {}
                Ok(_) | Err(_) => wake.reconcile = true,
            }
        }
        let applies_cold = self.cold.applies_cold_deliveries();
        // Resolving an address calls into the host, which can wait (it can
        // revive a member): the kept reader's connection closes first. Routes
        // read the claim registry and keep it.
        if self.awaiting.values().any(|wait| wait.via_address) {
            self.release_generation_reader();
        }
        let mut due = Vec::new();
        for (runtime_id, wait) in &self.awaiting {
            let recipients = if wait.via_address {
                match self.host.resolve_address(runtime_id).await {
                    crate::AddressResolution::Session(session_id) => vec![session_id],
                    // Unserved now: the attachment wake that serves it
                    // retries it.
                    crate::AddressResolution::NotServed => Vec::new(),
                    // Retired: the pass settles its bookkeeping.
                    crate::AddressResolution::Retired => {
                        due.push(runtime_id.clone());
                        continue;
                    }
                }
            } else {
                wait.recipients.clone()
            };
            let mut ours = false;
            for recipient in &recipients {
                ours = match self.router.route(recipient).await? {
                    DeliveryRoute::ServedHere(_) => true,
                    // The pass's claim attempt is the delivery itself: it
                    // takes a session whose holder is gone, and disturbs no
                    // live holder.
                    DeliveryRoute::Unserved(_) => applies_cold,
                    DeliveryRoute::ServedElsewhere => false,
                };
                if ours {
                    break;
                }
            }
            if ours {
                due.push(runtime_id.clone());
            }
        }
        Some(due)
    }
}

/// Why the owner reads the store's delivery generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GenerationRead {
    /// A reconcile, or ticks among which the watch swept: the reader reads.
    Read,
    /// Ticks that were file notifications only: the reader may answer from
    /// its last read ([`DeliveryGenerationReader::read_on_notification`]).
    Notification,
}

/// Which signals fired since the previous pass.
#[derive(Debug, Clone, Copy, Default)]
struct Wake {
    reconcile: bool,
    job_commit: bool,
    inbox_commit: bool,
    /// Retry blocked deliveries: an attachment committed, a run may have
    /// ended, or the host's continuation sources may have changed (bound,
    /// rebound or an announced custody change). Scheduling only: each retry
    /// re-reads the facts it applies by.
    retry_blocked: bool,
    /// The host's continuation sources may have changed. Besides retrying
    /// blocked rows, it starts a round of a projecting host's owed
    /// continuations (or asks for one more), whether or not anything is
    /// blocked: a newly committed obligation announces itself this way.
    source_change: bool,
    /// The shared store's watch ticked: answer with cheap reads.
    store_tick: bool,
    /// A failed job projection's retry is due on this wake (see
    /// `ProjectionRetry`).
    retry_projection: bool,
}

async fn run_owner(mut owner: OwnerLoop) {
    // Blocked delivery runtimes, with the session that serves each when known.
    let mut blocked: HashMap<LogicalRuntimeId, Option<SessionId>> = HashMap::new();
    let mut wake = Wake {
        reconcile: true,
        ..Wake::default()
    };
    let mut generation = 0_u64;
    let mut reported_failures: Vec<String> = Vec::new();
    // A reconcile read that failed during a round of owed continuations:
    // owed to the first wake after the round, like any failed reconcile.
    let mut reconcile_owed = false;
    let mut projection_retry = ProjectionRetry::default();
    loop {
        // A retired owner starts no pass, whichever signal woke it.
        if owner.stop.as_ref().is_some_and(|stop| *stop.borrow()) {
            return;
        }
        // Capture and mark the job, attachment and settlement signals in one
        // step each, BEFORE reading the authority they announce: a commit
        // published before the mark is read by this pass, and one published
        // after it moves the signal again and wakes the next, so none is
        // swallowed. Each borrow ends with its statement. The inbox signal is
        // marked inside the pass.
        wake.job_commit |= owner.job_commits.borrow_and_update().has_changed();
        if let Some(attachments) = owner.attachment_commits.as_mut() {
            wake.retry_blocked |= attachments.borrow_and_update().has_changed();
        }
        if let Some(settlements) = owner.run_settlements.as_mut() {
            wake.retry_blocked |= settlements.borrow_and_update().has_changed();
        }
        // Capture and mark the source revision in one step, before this
        // pass reads any fact: a change published before the mark is read
        // by this pass, and one published after it moves the revision again
        // and wakes the next, so none is swallowed. The borrow ends with
        // this statement, before any lock or await.
        if let Some(sources) = owner.source_changes.as_mut() {
            let changed = sources.borrow_and_update().has_changed();
            wake.retry_blocked |= changed;
            wake.source_change |= changed;
        }
        if let Some(owed) = owner.owed_changes.as_mut() {
            let changed = owed.borrow_and_update().has_changed();
            wake.retry_blocked |= changed;
            wake.source_change |= changed;
        }
        wake.inbox_commit |= owner.inbox_commits.has_changed().unwrap_or(false);
        #[cfg(not(target_arch = "wasm32"))]
        if let StoreWake::Watching { ticks, .. } = &mut owner.store_watch {
            // Captured and marked in one step, as above.
            wake.store_tick |= ticks.borrow_and_update().has_changed();
        }
        let due = if wake.store_tick {
            match owner.answer_store_tick(&mut wake).await {
                Some(due) => due,
                // The host is gone: the owner stops.
                None => return,
            }
        } else {
            Vec::new()
        };

        // A reconcile (the arming one first) or a source change starts a
        // round of the host's owed continuations, or asks for one more after
        // the round in progress. Run settlements and attachments never do.
        if owner.projects_owed && (wake.reconcile || wake.source_change) {
            match owner.owed_round.as_mut() {
                Some(round) => round.again = true,
                None => owner.owed_round = Some(OwedRound::default()),
            }
        }

        // A retry signal with nothing blocked has nothing to do: run
        // settlements fire on every runtime loop iteration.
        let has_work = wake.reconcile
            || wake.job_commit
            || wake.inbox_commit
            || (wake.retry_blocked
                && (!blocked.is_empty() || owner.awaiting.values().any(|wait| wait.via_address)))
            || wake.retry_projection
            || owner.owed_round.is_some()
            || !due.is_empty();
        let mut reconcile_failed = false;
        if has_work {
            generation = generation.wrapping_add(1);
            let mut pass = RuntimeDeliveryPass {
                generation,
                cross_process_wake_unavailable: owner.store_watch.unavailable_reason(),
                ..RuntimeDeliveryPass::default()
            };
            let Some(outcome) = run_pass(&mut owner, wake, due, &mut blocked, &mut pass).await
            else {
                return;
            };
            reconcile_failed = outcome.reconcile_failed;
            match outcome.projection_skipped {
                Some(true) => projection_retry.failed(),
                Some(false) => projection_retry = ProjectionRetry::default(),
                None => {}
            }
            pass.blocked_sessions = blocked.values().flatten().cloned().collect();
            pass.blocked_runtimes = blocked.keys().cloned().collect();
            pass.applies_cold_deliveries = owner.cold.applies_cold_deliveries();
            // Report a failure set once, not on every retry of the same rows.
            if !pass.failures.is_empty() && pass.failures != reported_failures {
                tracing::warn!(
                    failures = ?pass.failures,
                    "durable job delivery left rows pending until the next delivery wake"
                );
            }
            reported_failures.clone_from(&pass.failures);
            owner.passes.send_replace(pass);
        }

        if owner.owed_round.is_some() {
            // The round goes on with its next page in the next pass, which
            // no signal announces. Every page moves the cursor on and the
            // round ends with its enumeration, so this never spins; a failed
            // reconcile read waits for the first wake after the round.
            reconcile_owed |= reconcile_failed;
            wake = Wake::default();
            continue;
        }
        // A failed reconcile read stays owed to the next wake.
        wake = Wake {
            reconcile: reconcile_failed || std::mem::take(&mut reconcile_owed),
            ..Wake::default()
        };
        // Waits for the next wake. The kept generation reader's idle bound
        // also ends a wait, but only closes the reader's connection.
        loop {
            tokio::select! {
                changed = owner.job_commits.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    wake.job_commit = true;
                }
                changed = owner.inbox_commits.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    wake.inbox_commit = true;
                }
                changed = optional_changed(&mut owner.attachment_commits) => {
                    // A closed attachment signal means the runtime is gone.
                    if changed.is_err() {
                        return;
                    }
                    wake.retry_blocked = true;
                }
                changed = optional_changed(&mut owner.run_settlements) => {
                    if changed.is_err() {
                        return;
                    }
                    wake.retry_blocked = true;
                }
                changed = optional_changed(&mut owner.source_changes) => {
                    if changed.is_ok() {
                        wake.retry_blocked = true;
                        wake.source_change = true;
                    } else {
                        // The bindings are gone: their hint ends here, once, and
                        // is not polled again. It never stops the owner: the
                        // host and the runtime signals keep their lifetime
                        // checks, and the other wakes keep serving.
                        owner.source_changes = None;
                    }
                }
                changed = optional_changed(&mut owner.owed_changes) => {
                    if changed.is_ok() {
                        wake.retry_blocked = true;
                        wake.source_change = true;
                    } else {
                        // The owing source is gone: its hint ends here, once.
                        owner.owed_changes = None;
                    }
                }
                changed = owner.store_watch.tick() => {
                    // The watch's sender lives as long as the watch this loop
                    // owns; a closed one means the watch thread ended, and the
                    // store is then reported unwatched.
                    if changed.is_ok() {
                        wake.store_tick = true;
                    } else {
                        owner.store_watch = StoreWake::Unavailable(
                            "the store watch stopped".to_string(),
                        );
                        wake.reconcile = true;
                    }
                }
                changed = optional_changed(&mut owner.stop) => {
                    // The loop's top returns on a retirement request. A handle
                    // gone without one was detached: its owner runs on.
                    if changed.is_err() {
                        owner.stop = None;
                    }
                }
                () = generation_reader_idle(owner.generation_reader_idle_at) => {
                    // The store has been quiet for the reader's idle bound:
                    // close its connection. Nothing changed, so this is no
                    // wake and runs no pass.
                    owner.release_generation_reader();
                    continue;
                }
            }
            break;
        }
        // A wake arrived: a failed projection's retry owed to it joins the
        // pass it starts.
        wake.retry_projection = projection_retry.due_on_wake();
    }
}

async fn optional_changed<T>(
    signal: &mut Option<watch::Receiver<T>>,
) -> Result<(), watch::error::RecvError> {
    match signal {
        Some(signal) => signal.changed().await,
        None => std::future::pending().await,
    }
}

/// Resolves when the kept generation reader's idle bound passes; never while
/// it keeps nothing.
async fn generation_reader_idle(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// What a pass leaves owed to later wakes.
struct PassOutcome {
    /// A reconcile read failed and is repeated on the next wake.
    reconcile_failed: bool,
    /// Whether the job projection ran and skipped an entry; `None` when it
    /// did not run or the outbox could not be read.
    projection_skipped: Option<bool>,
}

/// One pass. Returns `None` when the host is gone, otherwise what it leaves
/// owed to later wakes.
async fn run_pass(
    owner: &mut OwnerLoop,
    wake: Wake,
    due: Vec<LogicalRuntimeId>,
    blocked: &mut HashMap<LogicalRuntimeId, Option<SessionId>>,
    pass: &mut RuntimeDeliveryPass,
) -> Option<PassOutcome> {
    let mut reconcile_failed = false;
    let mut projection_skipped = None;
    if wake.reconcile || wake.job_commit || wake.retry_projection {
        let mut skipped_any = Some(false);
        loop {
            match owner.projector.project_pending(DELIVERY_PAGE).await {
                Ok(projection) => {
                    pass.projected += projection.projected.len();
                    for skipped in &projection.skipped {
                        pass.failures.push(format!(
                            "projection of job {} delivery {} failed: {}",
                            skipped.job_id, skipped.delivery_sequence, skipped.error
                        ));
                    }
                    if !projection.skipped.is_empty() {
                        skipped_any = Some(true);
                    }
                    if projection.projected.is_empty() {
                        break;
                    }
                }
                Err(error) => {
                    reconcile_failed = true;
                    skipped_any = None;
                    pass.failures
                        .push(format!("job outbox could not be read: {error}"));
                    break;
                }
            }
        }
        projection_skipped = skipped_any;
    }
    // One page of the round of owed continuations in progress (see
    // `OwedRound`), before the inbox is collected, so the rows it commits
    // drain in this pass.
    if let Some(round) = owner.owed_round.take() {
        // Projecting owed continuations calls into the host, which can wait:
        // the kept reader's connection closes first.
        owner.release_generation_reader();
        let projection = owner
            .host
            .project_owed_continuations(round.after.as_ref(), DELIVERY_PAGE)
            .await;
        pass.owed_projected += projection.committed;
        if !projection.failures.is_empty() {
            reconcile_failed = true;
        }
        pass.failures.extend(projection.failures);
        owner.owed_round = match projection.next {
            Some(next) => Some(OwedRound {
                after: Some(next),
                again: round.again,
            }),
            None if round.again => Some(OwedRound::default()),
            None => None,
        };
    }

    let mut runtimes: Vec<LogicalRuntimeId> = Vec::new();
    if wake.reconcile {
        // Record the generation BEFORE reading the backlog, so a commit
        // landing during the read moves it again and wakes the next pass.
        if let Ok(current) = owner.read_delivery_generation(GenerationRead::Read).await {
            owner.last_delivery_generation = Some(current);
        }
        match owner
            .ownership
            .inbox()
            .runtimes_with_pending_deliveries()
            .await
        {
            Ok(found) => {
                // The pass below re-records every runtime still waiting.
                owner
                    .awaiting
                    .retain(|runtime_id, _| found.contains(runtime_id));
                runtimes.extend(found);
            }
            Err(error) => {
                reconcile_failed = true;
                pass.failures.push(format!(
                    "runtime delivery backlog could not be read: {error}"
                ));
            }
        }
    }
    // Marked seen only after this pass's own projection, so the rows it just
    // committed are drained below instead of waking a redundant pass. A
    // runtime is recorded before the generation advances, so any commit not
    // in the set taken here wakes the next pass.
    owner.inbox_commits.borrow_and_update();
    runtimes.extend(owner.ownership.take_committed_runtimes());
    if wake.retry_blocked {
        runtimes.extend(blocked.keys().cloned());
        // A continuation address waiting on another host may now be served
        // here by another session (an attachment is the host's mapping
        // change, and a rebinding replaces its address resolver): retry it
        // with a fresh resolution.
        runtimes.extend(
            owner
                .awaiting
                .iter()
                .filter(|(_, wait)| wait.via_address)
                .map(|(runtime_id, _)| runtime_id.clone()),
        );
    }
    runtimes.extend(due);
    let mut seen = HashSet::new();
    runtimes.retain(|runtime_id| seen.insert(runtime_id.clone()));

    // Delivering calls into the host, which can wait: the kept reader's
    // connection closes first, so maintenance never waits on a delivery. The
    // next store tick opens it again.
    if !runtimes.is_empty() {
        owner.release_generation_reader();
    }
    for runtime_id in runtimes {
        let (session_id, via_address) = match serving_session(owner, &runtime_id).await {
            Ok(ServingSession::Session(session_id)) => (session_id, false),
            Ok(ServingSession::Address(session_id)) => (session_id, true),
            Ok(ServingSession::NotServed) => {
                owner.awaiting.remove(&runtime_id);
                blocked.insert(runtime_id, None);
                continue;
            }
            Ok(ServingSession::Retired(first)) => {
                owner.awaiting.remove(&runtime_id);
                blocked.remove(&runtime_id);
                match settle_retired_reservation(owner, &runtime_id, &first).await {
                    Ok(None) => {}
                    Ok(Some(failure)) => pass.failures.push(failure),
                    Err(_) => return None,
                }
                continue;
            }
            Ok(ServingSession::NothingToDrain) => {
                owner.awaiting.remove(&runtime_id);
                blocked.remove(&runtime_id);
                continue;
            }
            Err(error) => {
                reconcile_failed = true;
                pass.failures.push(format!(
                    "delivery runtime {runtime_id} could not be resolved: {error}"
                ));
                continue;
            }
        };
        // Job rows are applied recipient by recipient, each routed by its own
        // session's hosting; a continuation row's one recipient is the
        // serving session, routed the same way (#1813).
        let mut applier = JobRuntimeDeliveryApplier::routed(
            owner.ownership.inbox().clone(),
            Arc::clone(&owner.router),
            owner.cold.applies_cold_deliveries(),
        );
        if via_address {
            applier = applier.with_address_session(session_id.clone());
        }
        if let Some(continuations) = owner.host.continuation_sink(&session_id).await {
            applier = applier
                .with_continuations(
                    continuations,
                    session_id.clone(),
                    owner.host.retained_job_source(),
                )
                .with_address_observer(Arc::clone(&owner.host));
        }
        loop {
            match applier.apply_pending(&runtime_id, DELIVERY_PAGE).await {
                Ok(drain) => {
                    let completed =
                        drain.applied.len() + drain.locally_settled.len() + drain.refused.len();
                    pass.applied += drain.applied.len();
                    pass.refused += drain.refused.len();
                    for row in &drain.refused {
                        pass.failures.push(format!(
                            "delivery {} (sequence {}) for session {session_id} is refused ({:?})",
                            row.delivery_id, row.runtime_sequence, row.reason
                        ));
                    }
                    if let Some(row) = drain.blocked {
                        if row.reason == crate::BlockedDeliveryReason::AddressRetired {
                            // The address retired after this pass resolved it:
                            // the drain released its reservation and admitted
                            // nothing, and nothing drains the address again.
                            blocked.remove(&runtime_id);
                            owner.awaiting.remove(&runtime_id);
                            break;
                        }
                        blocked.insert(runtime_id.clone(), Some(session_id.clone()));
                        owner.awaiting.remove(&runtime_id);
                        pass.failures.push(format!(
                            "delivery {} (sequence {}) for session {session_id} is blocked ({:?}): {}",
                            row.delivery_id, row.runtime_sequence, row.reason, row.error
                        ));
                        break;
                    }
                    if let Some(awaiting) = drain.awaiting_other_hosts {
                        // Not this process's to finish, and not a failure.
                        blocked.remove(&runtime_id);
                        owner.awaiting.insert(
                            runtime_id.clone(),
                            AwaitingWait {
                                recipients: awaiting.skipped_recipients,
                                via_address,
                            },
                        );
                        pass.awaiting_other_hosts += 1;
                        break;
                    }
                    if completed < DELIVERY_PAGE {
                        blocked.remove(&runtime_id);
                        owner.awaiting.remove(&runtime_id);
                        break;
                    }
                }
                Err(JobOutboxProjectionError::DeliveryHostGone) => return None,
                Err(error) => {
                    blocked.insert(runtime_id.clone(), Some(session_id.clone()));
                    pass.failures.push(format!(
                        "delivery drain for session {session_id} failed: {error}"
                    ));
                    break;
                }
            }
        }
    }
    Some(PassOutcome {
        reconcile_failed,
        projection_skipped,
    })
}

/// Settle the reservation `first`, the first pending row of the retired
/// continuation address `runtime_id`, holds, from the reserved session's
/// input ledger. Only the first row can hold one: the drain reserves rows in
/// order and stops at the first it cannot finish. The owner settles it only
/// under that session's custody (it routes here, or this process holds its
/// cold claim), so no admission into the session is in flight; a session
/// another runtime owner serves is that owner's to settle. Present: applied,
/// and the delivery reads admitted. Absent: released, and it reads
/// stranded. Another input under the key: the reservation stays, reported.
///
/// `Ok(Some)` names a failure to report; `Err` once the host is gone.
async fn settle_retired_reservation(
    owner: &OwnerLoop,
    runtime_id: &LogicalRuntimeId,
    first: &meerkat_runtime::RuntimeDeliveryRecord,
) -> Result<Option<String>, JobOutboxProjectionError> {
    let delivery_id = first.submission.delivery_id();
    let (session_id, input_id) = match owner
        .ownership
        .inbox()
        .continuation_admission(runtime_id, delivery_id)
        .await
    {
        Ok(Some(meerkat_runtime::ContinuationAdmission::Reserved {
            session_id,
            input_id,
        })) => (session_id, input_id),
        Ok(_) => return Ok(None),
        Err(error) => {
            return Ok(Some(format!(
                "the admission of delivery {delivery_id} on retired address {runtime_id} \
                 could not be read: {error}"
            )));
        }
    };
    let _cold_claim = match owner
        .router
        .route(&session_id)
        .await
        .ok_or(JobOutboxProjectionError::DeliveryHostGone)?
    {
        DeliveryRoute::ServedHere(_) => None,
        DeliveryRoute::Unserved(_) if owner.cold.applies_cold_deliveries() => {
            match owner
                .router
                .claim_cold(&session_id)
                .await
                .ok_or(JobOutboxProjectionError::DeliveryHostGone)?
            {
                Ok(claim) => Some(claim),
                Err(_) => return Ok(None),
            }
        }
        DeliveryRoute::Unserved(_) | DeliveryRoute::ServedElsewhere => return Ok(None),
    };
    let Some(sink) = owner.host.continuation_sink(&session_id).await else {
        return Ok(None);
    };
    match crate::continuation::settle_retired_reservation(
        owner.ownership.inbox(),
        runtime_id,
        first,
        &*sink,
        &session_id,
        &input_id,
    )
    .await
    {
        Ok(_) => Ok(None),
        Err(error) => Ok(Some(format!(
            "the reservation of delivery {delivery_id} on retired address {runtime_id} in \
             session {session_id} stays: {error}"
        ))),
    }
}

enum ServingSession {
    /// A job row's provenance session.
    Session(SessionId),
    /// The session serving a continuation address, or a member's address,
    /// now, from the host's address resolution.
    Address(SessionId),
    /// A live owner no session serves at the moment: retried on the next
    /// attachment commit, run settlement or source change.
    NotServed,
    /// No pending row this owner drains: empty, foreign, or a retired owner
    /// whose rows are stranded.
    NothingToDrain,
    /// A retired continuation address whose first pending row is a
    /// continuation: nothing drains it, and the owner settles a reservation
    /// that row holds ([`settle_retired_reservation`]).
    Retired(Box<meerkat_runtime::RuntimeDeliveryRecord>),
}

/// The session that drains `runtime_id` now: a member's delivery address and
/// a continuation row by the host's address resolution, any other job row by
/// its provenance. A member's address carries its continuations and, since
/// #1790, the rows of the jobs its sessions submitted; a retired incarnation
/// (its identity respawned under a new generation, or its mob gone) is
/// served by no session, so its rows are never applied.
async fn serving_session(
    owner: &OwnerLoop,
    runtime_id: &LogicalRuntimeId,
) -> Result<ServingSession, String> {
    let Some(first) = owner
        .ownership
        .inbox()
        .list_pending(runtime_id, 1)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
    else {
        return Ok(ServingSession::NothingToDrain);
    };
    if first.submission.kind() == RuntimeDeliveryKind::Continuation
        || matches!(runtime_id.member_address(), Ok(Some(_)))
    {
        return Ok(match owner.host.resolve_address(runtime_id).await {
            crate::AddressResolution::Session(session_id) => ServingSession::Address(session_id),
            crate::AddressResolution::NotServed => ServingSession::NotServed,
            crate::AddressResolution::Retired
                if first.submission.kind() == RuntimeDeliveryKind::Continuation =>
            {
                ServingSession::Retired(Box::new(first))
            }
            crate::AddressResolution::Retired => ServingSession::NothingToDrain,
        });
    }
    let sessions = owner
        .projector
        .sessions_for_runtimes(std::slice::from_ref(runtime_id))
        .await
        .map_err(|error| error.to_string())?;
    Ok(sessions
        .into_iter()
        .next()
        .map_or(ServingSession::NothingToDrain, ServingSession::Session))
}
