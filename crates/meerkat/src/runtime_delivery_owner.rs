//! Library owner of durable job delivery.
//!
//! One owner per runtime delivery inbox projects the job outbox into the inbox
//! and applies pending inbox rows through a host: job rows recipient by
//! recipient, continuation rows into the session serving their address. It
//! is woken only by typed signals: a job outbox commit, a runtime delivery commit, a runtime
//! attachment becoming serving and a run settlement. Arming runs one
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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::SessionId;
use meerkat_jobs::DetachedJobStore;
use meerkat_runtime::{
    ColdDeliveryOwnership, HostingCapability, HostingClaim, HostingRefused, LogicalRuntimeId,
    RuntimeDeliveryInbox, RuntimeDeliveryKind, RuntimeDeliveryOwnerAlreadyArmed,
    RuntimeDeliveryOwnership,
};
use tokio::sync::watch;

use crate::{
    DeliveryRoute, JobDeliveryRouter, JobOutboxProjectionError, JobOutboxProjector,
    JobRuntimeDeliveryApplier,
};

/// Rows read per projection or application page.
const DELIVERY_PAGE: usize = 256;

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
        let store_watch = StoreWake::start(&capability, self.store_sweep);
        let cold = meerkat_runtime::try_cold_delivery_ownership(&capability);
        let task = tokio::spawn(run_owner(OwnerLoop {
            projector: JobOutboxProjector::new(self.job_store, self.runtime_inbox.clone()),
            ownership,
            router: Arc::new(HostRouter(Arc::clone(&host))),
            host,
            job_commits,
            inbox_commits,
            attachment_commits: self.attachment_commits,
            run_settlements: self.run_settlements,
            passes,
            capability,
            store_watch,
            last_delivery_generation: None,
            cold,
            awaiting: HashMap::new(),
        }));
        Ok(RuntimeDeliveryOwnerHandle {
            task: Some(task),
            passes: passes_rx,
        })
    }
}

/// A running owner. Dropping it stops the owner and releases the inbox,
/// unless it was [detached](Self::detach).
pub struct RuntimeDeliveryOwnerHandle {
    task: Option<tokio::task::JoinHandle<()>>,
    passes: watch::Receiver<RuntimeDeliveryPass>,
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
            .is_none_or(tokio::task::JoinHandle::is_finished)
    }

    /// Let the owner run on its own until its host or its runtime is gone.
    pub fn detach(mut self) {
        self.task = None;
    }
}

impl Drop for RuntimeDeliveryOwnerHandle {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
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
        _watch: meerkat_runtime::DeliveryStoreWatch,
        ticks: watch::Receiver<u64>,
    },
    Unavailable(String),
}

impl StoreWake {
    fn start(capability: &HostingCapability, sweep: Duration) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match meerkat_runtime::watch_delivery_store(capability, sweep) {
                Ok(None) => Self::NotShared,
                Ok(Some(watch)) => Self::Watching {
                    ticks: watch.subscribe_ticks(),
                    _watch: watch,
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
            let _ = sweep;
            if capability.is_cross_process() {
                Self::Unavailable("a browser build has no SQLite store watch".to_string())
            } else {
                Self::NotShared
            }
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
    passes: watch::Sender<RuntimeDeliveryPass>,
    capability: HostingCapability,
    store_watch: StoreWake,
    /// The store's durable delivery generation as last reconciled; a store
    /// tick reconciles only when it moved.
    last_delivery_generation: Option<u64>,
    cold: ColdDeliveryOwnership,
    /// Delivery runtimes whose next row waits on recipients this process
    /// did not apply. A store tick re-routes the recipients (registry reads
    /// only) and retries a runtime once one became this process's to apply:
    /// served here, or cold while this process is the cold-delivery owner
    /// (its claim attempt then decides).
    awaiting: HashMap<LogicalRuntimeId, AwaitingWait>,
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
        if !wake.reconcile {
            // One row read. Writes by sessions and other stores tick the
            // watch too; they cost only this read.
            match self.ownership.inbox().delivery_generation().await {
                Ok(current) if self.last_delivery_generation == Some(current) => {}
                Ok(_) | Err(_) => wake.reconcile = true,
            }
        }
        let applies_cold = self.cold.applies_cold_deliveries();
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

/// Which signals fired since the previous pass.
#[derive(Debug, Clone, Copy, Default)]
struct Wake {
    reconcile: bool,
    job_commit: bool,
    inbox_commit: bool,
    /// An attachment committed or a run may have ended: retry blocked
    /// sessions.
    attachment_commit: bool,
    /// The shared store's watch ticked: answer with cheap reads.
    store_tick: bool,
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
    loop {
        // Mark the job and attachment signals seen BEFORE reading the
        // authority they announce, so a commit that lands during this pass
        // wakes the next one. The inbox signal is marked inside the pass.
        wake.job_commit |= owner.job_commits.has_changed().unwrap_or(false);
        owner.job_commits.borrow_and_update();
        if let Some(attachments) = owner.attachment_commits.as_mut() {
            wake.attachment_commit |= attachments.has_changed().unwrap_or(false);
            attachments.borrow_and_update();
        }
        if let Some(settlements) = owner.run_settlements.as_mut() {
            wake.attachment_commit |= settlements.has_changed().unwrap_or(false);
            settlements.borrow_and_update();
        }
        wake.inbox_commit |= owner.inbox_commits.has_changed().unwrap_or(false);
        #[cfg(not(target_arch = "wasm32"))]
        if let StoreWake::Watching { ticks, .. } = &mut owner.store_watch {
            wake.store_tick |= ticks.has_changed().unwrap_or(false);
            ticks.borrow_and_update();
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

        // A retry signal with nothing blocked has nothing to do: run
        // settlements fire on every runtime loop iteration.
        let has_work = wake.reconcile
            || wake.job_commit
            || wake.inbox_commit
            || (wake.attachment_commit
                && (!blocked.is_empty() || owner.awaiting.values().any(|wait| wait.via_address)))
            || !due.is_empty();
        let mut reconcile_failed = false;
        if has_work {
            generation = generation.wrapping_add(1);
            let mut pass = RuntimeDeliveryPass {
                generation,
                cross_process_wake_unavailable: owner.store_watch.unavailable_reason(),
                ..RuntimeDeliveryPass::default()
            };
            let Some(failed) = run_pass(&mut owner, wake, due, &mut blocked, &mut pass).await
            else {
                return;
            };
            reconcile_failed = failed;
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

        // A failed reconcile read stays owed to the next wake.
        wake = Wake {
            reconcile: reconcile_failed,
            ..Wake::default()
        };
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
                wake.attachment_commit = true;
            }
            changed = optional_changed(&mut owner.run_settlements) => {
                if changed.is_err() {
                    return;
                }
                wake.attachment_commit = true;
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
        }
    }
}

async fn optional_changed(
    signal: &mut Option<watch::Receiver<u64>>,
) -> Result<(), watch::error::RecvError> {
    match signal {
        Some(signal) => signal.changed().await,
        None => std::future::pending().await,
    }
}

/// One pass. Returns `None` when the host is gone, otherwise whether a
/// reconcile read failed and must be repeated on the next wake.
async fn run_pass(
    owner: &mut OwnerLoop,
    wake: Wake,
    due: Vec<LogicalRuntimeId>,
    blocked: &mut HashMap<LogicalRuntimeId, Option<SessionId>>,
    pass: &mut RuntimeDeliveryPass,
) -> Option<bool> {
    let mut reconcile_failed = false;
    if wake.reconcile || wake.job_commit {
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
                    if projection.projected.is_empty() {
                        break;
                    }
                }
                Err(error) => {
                    reconcile_failed = true;
                    pass.failures
                        .push(format!("job outbox could not be read: {error}"));
                    break;
                }
            }
        }
    }

    let mut runtimes: Vec<LogicalRuntimeId> = Vec::new();
    if wake.reconcile {
        // Record the generation BEFORE reading the backlog, so a commit
        // landing during the read moves it again and wakes the next pass.
        if let Ok(current) = owner.ownership.inbox().delivery_generation().await {
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
    if wake.attachment_commit {
        runtimes.extend(blocked.keys().cloned());
        // A continuation address waiting on another host may now be served
        // here by another session (an attachment is the host's mapping
        // change): retry it with a fresh resolution.
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

    for runtime_id in runtimes {
        let (session_id, via_address) = match serving_session(owner, &runtime_id).await {
            Ok(ServingSession::Session(session_id)) => (session_id, false),
            Ok(ServingSession::Address(session_id)) => (session_id, true),
            Ok(ServingSession::NotServed) => {
                owner.awaiting.remove(&runtime_id);
                blocked.insert(runtime_id, None);
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
        if let Some(continuations) = owner.host.continuation_sink(&session_id).await {
            applier = applier.with_continuations(
                continuations,
                session_id.clone(),
                owner.host.retained_job_source(),
            );
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
    Some(reconcile_failed)
}

enum ServingSession {
    /// A job row's provenance session.
    Session(SessionId),
    /// The session serving a continuation address now, from the host's
    /// address resolution.
    Address(SessionId),
    /// A live owner no session serves at the moment: retried on the next
    /// attachment commit or run settlement.
    NotServed,
    /// No pending row this owner drains: empty, foreign, or a retired owner
    /// whose rows are stranded.
    NothingToDrain,
}

/// The session that drains `runtime_id` now, decided by its first pending
/// row: a job row by its provenance, a continuation row by the host's address
/// resolution.
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
    if first.submission.kind() == RuntimeDeliveryKind::Continuation {
        return Ok(match owner.host.resolve_address(runtime_id).await {
            crate::AddressResolution::Session(session_id) => ServingSession::Address(session_id),
            crate::AddressResolution::NotServed => ServingSession::NotServed,
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
