//! MCP attempt guards: a host-installed guard around each stdio MCP server
//! connect attempt.
//!
//! A host builds one [`McpAttemptGuards`] installation around its
//! [`McpAttemptGuardFactory`] and passes it to every router it builds
//! ([`crate::McpRouter::with_attempt_guards`]), with the owner those routers
//! act for. For every stdio server attempt, before secret resolution, the
//! attachment control's physical entry or any spawn, the router asks the
//! factory to begin a guard for that exact attempt ([`McpAttemptKey`]):
//!
//! - a refusal refuses the attempt, with nothing spawned and no guard;
//! - `Ok(None)` lets the attempt proceed unguarded;
//! - a guard is finished exactly once, by one async call, once both facts of
//!   its attempt are known: [`McpAttemptGuard::settle`] when the attempt's
//!   connection was installed, [`McpAttemptGuard::cancel`] for every other
//!   outcome. The [`McpAttemptDisposition`] it receives keeps the attempt's
//!   open outcome ([`McpAttemptOpenOutcome`]) apart from how its process
//!   ended ([`McpAttemptProcessEnd`]).
//!
//! A begin is never dropped. When the attempt stops waiting for it (its
//! bound, a router stop or drop, the attempt's task dropped, or the
//! installation closing), the attempt's [`McpAttemptCancellation`] is
//! requested with its cause and the attempt fails with nothing spawned; a
//! guard the begin returns later is cancelled in the begin's own task.
//!
//! Before the next begin for the same owner (mob member, else session) and
//! server, core awaits, bounded, every running begin and every started
//! finish for that pair; it never waits for a guard whose process is still
//! alive. A prior begin or finish still running past the settle bound
//! refuses the new attempt ([`McpAttemptRefusal::PriorGuardUnsettled`]).
//! Router shutdown awaits the router's own begins and finishes, bounded, and
//! reports them ([`crate::McpRouterShutdownReport::guards`]); the host's
//! [`McpAttemptGuards::close_and_join`] awaits the rest, those of dropped
//! routers included, and must run before the tokio runtime shuts down.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use futures::FutureExt as _;
use meerkat_core::McpServerConfig;
use meerkat_core::MobMemberBinding;
use meerkat_core::mcp_attachment::{McpAttachmentRefusal, McpStdioSetupFailure};
use meerkat_core::types::SessionId;

use crate::McpStdioCloseReceipt;

static NEXT_CONSTRUCTION: AtomicU64 = AtomicU64::new(1);

/// The identity of one router construction: unique per [`crate::McpRouter`]
/// in this process. A rebuilt or reset member gets a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct McpConstructionId(u64);

impl McpConstructionId {
    pub(crate) fn allocate() -> Self {
        Self(NEXT_CONSTRUCTION.fetch_add(1, Ordering::Relaxed))
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Who one router construction acts for: its session, and the mob member
/// it is built for, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpConstructionOwner {
    pub session_id: SessionId,
    pub member: Option<MobMemberBinding>,
}

impl McpConstructionOwner {
    #[must_use]
    pub fn new(session_id: SessionId, member: Option<MobMemberBinding>) -> Self {
        Self { session_id, member }
    }
}

/// One exact attempt: its router construction, its server and the router's
/// sequence for it (the same number as
/// [`McpStdioSetupFailure::attempt`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct McpAttemptKey {
    pub construction: McpConstructionId,
    pub server: String,
    pub attempt: u64,
}

/// What the attempt connects for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpAttemptOperation {
    Add,
    Reload,
}

/// What a guard is asked to begin for. Owned, so a begin may outlive the
/// attempt. `server` is the staged descriptor: it never carries a resolved
/// secret (secrets are resolved later, inside the attempt).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct McpAttemptGuardRequest {
    pub key: McpAttemptKey,
    pub owner: Arc<McpConstructionOwner>,
    pub server: McpServerConfig,
    pub operation: McpAttemptOperation,
}

/// Why an attempt stopped waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpAttemptCancelCause {
    /// The begin did not answer within its bound.
    BeginBound,
    /// The router began its shutdown.
    RouterClosed,
    /// The attempt's own task was dropped.
    AttemptDropped,
    /// The router was dropped without a shutdown.
    RouterDropped,
    /// The host closed the installation ([`McpAttemptGuards::close_and_join`]).
    InstallationClosed,
}

/// One attempt's cancellation: requested once, with its first cause.
#[derive(Clone)]
pub struct McpAttemptCancellation {
    inner: Arc<CancellationInner>,
}

struct CancellationInner {
    token: tokio_util::sync::CancellationToken,
    cause: OnceLock<McpAttemptCancelCause>,
}

impl std::fmt::Debug for McpAttemptCancellation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpAttemptCancellation")
            .field("cause", &self.cause())
            .finish()
    }
}

impl McpAttemptCancellation {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(CancellationInner {
                token: tokio_util::sync::CancellationToken::new(),
                cause: OnceLock::new(),
            }),
        }
    }

    /// Request the cancellation; the first cause stands.
    pub(crate) fn request(&self, cause: McpAttemptCancelCause) {
        let _ = self.inner.cause.set(cause);
        self.inner.token.cancel();
    }

    /// The cause, once requested.
    #[must_use]
    pub fn cause(&self) -> Option<McpAttemptCancelCause> {
        self.inner.cause.get().copied()
    }

    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.inner.token.is_cancelled()
    }

    /// Resolves once requested, with its cause.
    pub async fn requested(&self) -> McpAttemptCancelCause {
        self.inner.token.cancelled().await;
        self.cause()
            .unwrap_or(McpAttemptCancelCause::AttemptDropped)
    }

    fn downgrade(&self) -> Weak<CancellationInner> {
        Arc::downgrade(&self.inner)
    }

    fn upgrade(weak: &Weak<CancellationInner>) -> Option<Self> {
        weak.upgrade().map(|inner| Self { inner })
    }
}

/// A host's guard factory, asked once per stdio attempt before anything of
/// it runs. `Err` refuses the attempt with nothing spawned (the begin's
/// record carries [`McpAttemptRefusal::Guard`]); `Ok(None)` lets it proceed
/// unguarded (no finish is ever called for it). `cancel` is requested while
/// the attempt waits for this answer, when it stops waiting; the begin is
/// never dropped, and a guard it returns after that is cancelled at once.
///
/// Begin is called only for stdio attempts: remote (HTTP and SSE) attempts
/// are neither guarded nor refused by the factory.
#[async_trait::async_trait]
pub trait McpAttemptGuardFactory: Send + Sync {
    async fn begin(
        &self,
        request: McpAttemptGuardRequest,
        cancel: McpAttemptCancellation,
    ) -> Result<Option<Box<dyn McpAttemptGuard>>, McpAttachmentRefusal>;
}

/// One begun attempt's guard. Exactly one of its methods is called, exactly
/// once: `settle` when the attempt's connection was installed, `cancel` for
/// every other outcome, once both the open outcome and the process end are
/// known. The open outcome is fixed when the router processes the attempt's
/// result (its next poll, apply, wait or shutdown), so a failed attempt's
/// `cancel` follows that processing, not the failure itself.
#[async_trait::async_trait]
pub trait McpAttemptGuard: Send {
    async fn settle(
        self: Box<Self>,
        disposition: McpAttemptDisposition,
    ) -> Result<(), McpGuardFinishError>;

    async fn cancel(
        self: Box<Self>,
        disposition: McpAttemptDisposition,
    ) -> Result<(), McpGuardFinishError>;
}

/// A guard's own failure to finish.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{detail}")]
#[non_exhaustive]
pub struct McpGuardFinishError {
    pub detail: String,
}

impl McpGuardFinishError {
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

/// How one attempt ended: whether it opened (separately from) how its
/// process ended.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct McpAttemptDisposition {
    pub key: McpAttemptKey,
    pub open: McpAttemptOpenOutcome,
    pub process: McpAttemptProcessEnd,
}

/// Whether the attempt's connection opened and was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpAttemptOpenOutcome {
    /// The router installed the attempt's connection.
    Installed { connection_generation: u64 },
    /// The setup failed; `failure` is the typed stdio setup failure the
    /// transport established, when it established one.
    SetupFailed {
        failure: Option<McpStdioSetupFailure>,
    },
    /// The attempt was refused before it could open.
    Refused { refusal: McpAttemptRefusal },
    /// The connection opened but was not installed.
    NotInstalled { reason: McpAttemptNotInstalled },
    /// The attempt stopped: see the cause.
    Cancelled { cause: McpAttemptCancelCause },
}

/// Why an opened connection was not installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpAttemptNotInstalled {
    /// A newer intent for the server replaced the attempt.
    Superseded,
    /// The router refused to install it.
    InstallRefused,
}

/// Who refused the attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpAttemptRefusal {
    /// The guard factory (recorded on the begin; no guard is finished).
    Guard,
    /// The attachment control at the physical entry.
    Control,
    /// Process confinement.
    Confinement,
    /// The service-secret resolver.
    ServiceSecret,
    /// The server's configuration (including a configured working directory
    /// that is not an existing directory when the server is launched).
    InvalidConfig,
    /// A prior begin or finish of the same owner and server was still
    /// running past the settle bound.
    PriorGuardUnsettled,
}

/// How the attempt's process ended.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum McpAttemptProcessEnd {
    /// No process was spawned.
    NotSpawned,
    /// Its custody closed it; the receipt is the close's own.
    Closed(Arc<McpStdioCloseReceipt>),
    /// It was killed without its exit being observed (its custody or close
    /// was dropped).
    Abandoned,
}

/// Which call a completion record is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpGuardStage {
    Begin,
    Settle,
    Cancel,
}

/// How a call ended, as far as it was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpGuardCompletion {
    /// It returned (a begin with a guard, none or a refusal; a finish with
    /// `Ok`).
    Completed,
    /// A finish returned `Err`.
    Failed,
    /// It was still running when the bounded wait for it ended.
    TimedOut,
    /// It panicked.
    Panicked,
    /// A begin was not called (its router or the installation was closed,
    /// its attempt stopped waiting, or a prior guard was unsettled), or it
    /// returned after its attempt was cancelled.
    Cancelled,
    /// The guard's process was still alive, so its finish had not started.
    Held,
}

/// One observed guard call.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct McpGuardCompletionRecord {
    pub key: McpAttemptKey,
    pub stage: McpGuardStage,
    pub disposition: Option<McpAttemptDisposition>,
    pub completion: McpGuardCompletion,
}

/// The bounds of guard calls: how long an attempt waits for its begin, and
/// how long core waits for a cancel or a settle (and for prior guards, at
/// the settle bound). A bound limits the wait, never the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpAttemptGuardBounds {
    begin: Duration,
    cancel: Duration,
    settle: Duration,
}

/// One of the three [`McpAttemptGuardBounds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpAttemptGuardBound {
    Begin,
    Cancel,
    Settle,
}

impl std::fmt::Display for McpAttemptGuardBound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Begin => "begin",
            Self::Cancel => "cancel",
            Self::Settle => "settle",
        })
    }
}

/// A refused [`McpAttemptGuardBounds`] value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum McpAttemptGuardBoundsError {
    /// A zero bound: a zero begin bound would cancel every attempt, and a
    /// zero cancel or settle bound would never wait.
    #[error("MCP attempt guard {bound} bound must be greater than zero")]
    Zero { bound: McpAttemptGuardBound },
    /// A bound above [`McpAttemptGuardBounds::MAX`].
    #[error("MCP attempt guard {bound} bound of {value:?} exceeds the {max:?} maximum")]
    TooLong {
        bound: McpAttemptGuardBound,
        value: Duration,
        max: Duration,
    },
}

impl McpAttemptGuardBounds {
    /// The largest bound accepted.
    pub const MAX: Duration = Duration::from_secs(60);
    /// 10 s to begin, 5 s to cancel, 5 s to settle.
    pub const DEFAULT: Self = Self {
        begin: Duration::from_secs(10),
        cancel: Duration::from_secs(5),
        settle: Duration::from_secs(5),
    };

    /// Bounds with these durations. A zero bound, or one above
    /// [`Self::MAX`], is refused, not clamped.
    pub fn new(
        begin: Duration,
        cancel: Duration,
        settle: Duration,
    ) -> Result<Self, McpAttemptGuardBoundsError> {
        for (bound, value) in [
            (McpAttemptGuardBound::Begin, begin),
            (McpAttemptGuardBound::Cancel, cancel),
            (McpAttemptGuardBound::Settle, settle),
        ] {
            if value.is_zero() {
                return Err(McpAttemptGuardBoundsError::Zero { bound });
            }
            if value > Self::MAX {
                return Err(McpAttemptGuardBoundsError::TooLong {
                    bound,
                    value,
                    max: Self::MAX,
                });
            }
        }
        Ok(Self {
            begin,
            cancel,
            settle,
        })
    }

    #[must_use]
    pub fn begin(&self) -> Duration {
        self.begin
    }

    #[must_use]
    pub fn cancel(&self) -> Duration {
        self.cancel
    }

    #[must_use]
    pub fn settle(&self) -> Duration {
        self.settle
    }
}

impl Default for McpAttemptGuardBounds {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A host-wide installation of one guard factory, shared by every router it
/// is passed to ([`crate::McpRouter::with_attempt_guards`]). Cloning shares
/// it. It tracks every begin and finish it started, those of dropped routers
/// included, until [`Self::close_and_join`] reports them (at most 1024
/// completed ones are kept; the oldest beyond that are forgotten).
///
/// Only stdio attempts are guarded: begin is called only for them. Remote
/// (HTTP and SSE) attempts are neither guarded nor refused by it.
#[derive(Clone)]
pub struct McpAttemptGuards(Arc<Installation>);

impl std::fmt::Debug for McpAttemptGuards {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpAttemptGuards")
            .field("bounds", &self.0.bounds)
            .finish_non_exhaustive()
    }
}

/// How many completed calls an installation keeps for
/// [`McpAttemptGuards::close_and_join`]; the oldest beyond it are forgotten.
const MAX_RETAINED_COMPLETED: usize = 1024;

struct Installation {
    factory: Arc<dyn McpAttemptGuardFactory>,
    bounds: McpAttemptGuardBounds,
    state: Mutex<InstallationState>,
    /// Holds each attempt between receiving its begin's answer and acting
    /// on it (tests only).
    #[cfg(test)]
    handover_hold: tokio::sync::watch::Sender<bool>,
    /// Observes, synchronously, each process end a record learns (tests
    /// only).
    #[cfg(test)]
    on_process_end: Mutex<Option<ProcessEndObserver>>,
}

#[cfg(test)]
type ProcessEndObserver = Arc<dyn Fn(&McpAttemptProcessEnd) + Send + Sync>;

#[derive(Default)]
struct InstallationState {
    closed: bool,
    routers: Vec<Weak<RouterGuards>>,
    tasks: Vec<Arc<GuardTask>>,
    records: Vec<Weak<AttemptGuardRecord>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl McpAttemptGuards {
    /// An installation of `factory` under `bounds`.
    #[must_use]
    pub fn new(factory: Arc<dyn McpAttemptGuardFactory>, bounds: McpAttemptGuardBounds) -> Self {
        Self(Arc::new(Installation {
            factory,
            bounds,
            state: Mutex::new(InstallationState::default()),
            #[cfg(test)]
            handover_hold: tokio::sync::watch::channel(false).0,
            #[cfg(test)]
            on_process_end: Mutex::new(None),
        }))
    }

    /// Observe, synchronously, each process end a record learns (tests
    /// only).
    #[cfg(test)]
    pub(crate) fn on_process_end_for_test(
        &self,
        observer: impl Fn(&McpAttemptProcessEnd) + Send + Sync + 'static,
    ) {
        *lock(&self.0.on_process_end) = Some(Arc::new(observer));
    }

    /// Hold every attempt between receiving its begin's answer and acting on
    /// it (tests only).
    #[cfg(test)]
    pub(crate) fn hold_handover_for_test(&self, hold: bool) {
        self.0.handover_hold.send_replace(hold);
    }

    #[must_use]
    pub fn bounds(&self) -> McpAttemptGuardBounds {
        self.0.bounds
    }

    /// Close the installation and join what it started, by `deadline`: new
    /// begins are refused, begins still waiting are cancelled
    /// ([`McpAttemptCancelCause::InstallationClosed`]), and every tracked
    /// begin and finish is awaited and reported once, those of dropped
    /// routers and those a router shutdown already reported included. A call
    /// that starts while it waits (a finish whose process ends meanwhile) is
    /// joined too. A call still running at `deadline` is reported
    /// [`McpGuardCompletion::TimedOut`] (it is not dropped, and a later call
    /// reports its end); a guard whose process is still alive is reported
    /// [`McpGuardCompletion::Held`]. The calls reported as ended are then
    /// forgotten. Run it before the tokio runtime shuts down: calls spawned
    /// on a runtime that has shut down never run.
    pub async fn close_and_join(
        &self,
        deadline: tokio::time::Instant,
    ) -> Vec<McpGuardCompletionRecord> {
        let routers = {
            let mut state = lock(&self.0.state);
            // Begins are tracked under this lock: every begin is either
            // tracked by now, and joined below, or refused.
            state.closed = true;
            state.routers.clone()
        };
        for router in routers.iter().filter_map(Weak::upgrade) {
            router.close(McpAttemptCancelCause::InstallationClosed);
        }
        let joined = self
            .0
            .join(&|_: &McpAttemptKey| true, deadline, JoinBy::At(deadline))
            .await;
        self.0.forget(&joined.ended);
        joined.records
    }
}

/// When a join stops waiting for finishes.
#[derive(Clone, Copy)]
enum JoinBy {
    /// At this instant.
    At(tokio::time::Instant),
    /// This long after the join's first begins were joined.
    Within(Duration),
}

/// What one join reported.
#[derive(Default)]
struct Joined {
    records: Vec<McpGuardCompletionRecord>,
    /// The calls reported as ended (any completion but `TimedOut`).
    ended: Vec<Arc<GuardTask>>,
}

impl Installation {
    /// Track a begin that is about to wait: refused once the installation
    /// closed.
    fn track_begin(&self, task: &Arc<GuardTask>) -> bool {
        let mut state = lock(&self.state);
        if state.closed {
            return false;
        }
        Self::push(&mut state, task);
        true
    }

    /// Track a call: a finish, or a begin already complete.
    fn track(&self, task: &Arc<GuardTask>) {
        Self::push(&mut lock(&self.state), task);
    }

    fn push(state: &mut InstallationState, task: &Arc<GuardTask>) {
        state.tasks.push(Arc::clone(task));
        // Completed calls of dropped routers are kept for close_and_join, up
        // to a bound.
        let completed = state
            .tasks
            .iter()
            .filter(|task| task.completion().is_some())
            .count();
        if completed > MAX_RETAINED_COMPLETED {
            let mut excess = completed - MAX_RETAINED_COMPLETED;
            state.tasks.retain(|task| {
                if excess > 0 && task.completion().is_some() {
                    excess -= 1;
                    false
                } else {
                    true
                }
            });
        }
    }

    fn is_closed(&self) -> bool {
        lock(&self.state).closed
    }

    /// Every tracked call that `select`s, in tracking order.
    fn tracked(&self, select: impl Fn(&Arc<GuardTask>) -> bool) -> Vec<Arc<GuardTask>> {
        lock(&self.state)
            .tasks
            .iter()
            .filter(|task| select(task))
            .cloned()
            .collect()
    }

    /// The guards still held (their finish not started) whose key `select`s.
    fn held(
        &self,
        select: &(dyn Fn(&McpAttemptKey) -> bool + Send + Sync),
    ) -> Vec<McpGuardCompletionRecord> {
        let records: Vec<Arc<AttemptGuardRecord>> = {
            let mut state = lock(&self.state);
            state.records.retain(|record| record.strong_count() > 0);
            state.records.iter().filter_map(Weak::upgrade).collect()
        };
        records
            .iter()
            .filter(|record| select(&record.key))
            .filter_map(|record| record.held())
            .collect()
    }

    /// Forget the calls a join reported as ended.
    fn forget(&self, ended: &[Arc<GuardTask>]) {
        if ended.is_empty() {
            return;
        }
        lock(&self.state)
            .tasks
            .retain(|task| !ended.iter().any(|ended| Arc::ptr_eq(ended, task)));
    }

    /// Wait, at most `bound`, for the running begins of `scope` tracked
    /// before `own` (this attempt's begin) and its started finishes; `false`
    /// if one is still running then. Begins tracked after `own` are not
    /// awaited, so two waiting attempts never wait for each other.
    async fn await_prior(&self, scope: &GuardScope, own: &Arc<GuardTask>, bound: Duration) -> bool {
        let running: Vec<Arc<GuardTask>> = {
            let state = lock(&self.state);
            let mut before_own = true;
            state
                .tasks
                .iter()
                .filter(|task| {
                    if Arc::ptr_eq(task, own) {
                        before_own = false;
                        return false;
                    }
                    &task.scope == scope
                        && task.completion().is_none()
                        && (before_own || task.stage != McpGuardStage::Begin)
                })
                .cloned()
                .collect()
        };
        let deadline = tokio::time::Instant::now() + bound;
        for task in running {
            if task.wait_until(deadline).await.is_none() {
                return false;
            }
        }
        true
    }

    /// Run one finish of `guard` (settle when the attempt was installed,
    /// cancel otherwise) on `runtime`, tracked first.
    fn spawn_finish(
        &self,
        runtime: &tokio::runtime::Handle,
        guard: Box<dyn McpAttemptGuard>,
        scope: GuardScope,
        disposition: McpAttemptDisposition,
    ) {
        let task = finish_task(&scope, &disposition);
        self.track(&task);
        spawn_finish_task(runtime, task, guard, disposition);
    }

    /// Await every tracked call `select` picks, begins by `begins_by` and
    /// finishes by `finishes_by`, and report each once. Calls tracked while
    /// it waits are joined in later passes; once a pass finds none, the
    /// guards still held are reported. A finish is tracked before its
    /// record lets go of the guard, so every guard is either held or
    /// tracked at that point. Past both deadlines the join ends; a call that
    /// starts later stays tracked for a later join.
    async fn join(
        &self,
        select: &(dyn Fn(&McpAttemptKey) -> bool + Send + Sync),
        begins_by: tokio::time::Instant,
        finishes_by: JoinBy,
    ) -> Joined {
        let mut joined = Joined::default();
        let mut seen: Vec<Arc<GuardTask>> = Vec::new();
        let mut finishes_at = match finishes_by {
            JoinBy::At(at) => Some(at),
            JoinBy::Within(_) => None,
        };
        loop {
            let pending = self.tracked(|task| {
                select(&task.key) && !seen.iter().any(|done| Arc::ptr_eq(done, task))
            });
            let found = !pending.is_empty();
            let (begins, finishes): (Vec<_>, Vec<_>) = pending
                .into_iter()
                .partition(|task| task.stage == McpGuardStage::Begin);
            for task in begins {
                report_joined(&mut joined, &task, begins_by).await;
                seen.push(task);
            }
            let finish_deadline = *finishes_at.get_or_insert_with(|| match finishes_by {
                JoinBy::At(at) => at,
                JoinBy::Within(bound) => tokio::time::Instant::now() + bound,
            });
            for task in finishes {
                report_joined(&mut joined, &task, finish_deadline).await;
                seen.push(task);
            }
            let past = tokio::time::Instant::now() >= begins_by.max(finish_deadline);
            if found && !past {
                continue;
            }
            let held = self.held(select);
            let late = self.tracked(|task| {
                select(&task.key) && !seen.iter().any(|done| Arc::ptr_eq(done, task))
            });
            if late.is_empty() || past {
                joined.records.extend(held);
                return joined;
            }
        }
    }
}

/// Wait for `task` until `by` and report it.
async fn report_joined(joined: &mut Joined, task: &Arc<GuardTask>, by: tokio::time::Instant) {
    let completion = task
        .wait_until(by)
        .await
        .unwrap_or(McpGuardCompletion::TimedOut);
    if completion != McpGuardCompletion::TimedOut {
        joined.ended.push(Arc::clone(task));
    }
    joined.records.push(task.record(completion));
}

/// Run a tracked finish on `runtime`.
fn spawn_finish_task(
    runtime: &tokio::runtime::Handle,
    task: Arc<GuardTask>,
    guard: Box<dyn McpAttemptGuard>,
    disposition: McpAttemptDisposition,
) {
    runtime.spawn(async move {
        let completion = run_finish(guard, disposition).await;
        task.complete(completion);
    });
}

fn finish_task(scope: &GuardScope, disposition: &McpAttemptDisposition) -> Arc<GuardTask> {
    GuardTask::new(
        disposition.key.clone(),
        scope.clone(),
        finish_stage(&disposition.open),
        Some(disposition.clone()),
    )
}

fn finish_stage(open: &McpAttemptOpenOutcome) -> McpGuardStage {
    if matches!(open, McpAttemptOpenOutcome::Installed { .. }) {
        McpGuardStage::Settle
    } else {
        McpGuardStage::Cancel
    }
}

/// The one finish call: settle for an installed attempt, cancel for any
/// other.
async fn run_finish(
    guard: Box<dyn McpAttemptGuard>,
    disposition: McpAttemptDisposition,
) -> McpGuardCompletion {
    let call = match finish_stage(&disposition.open) {
        McpGuardStage::Settle => guard.settle(disposition),
        McpGuardStage::Begin | McpGuardStage::Cancel => guard.cancel(disposition),
    };
    match AssertUnwindSafe(call).catch_unwind().await {
        Ok(Ok(())) => McpGuardCompletion::Completed,
        Ok(Err(_)) => McpGuardCompletion::Failed,
        Err(_) => McpGuardCompletion::Panicked,
    }
}

/// The owner and server whose guards are awaited before a new begin: the
/// mob member when there is one (so a rebuild or reset of that member
/// waits), else the session.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GuardScope {
    Member {
        mob_id: String,
        role: String,
        member: String,
        server: String,
    },
    Session {
        session_id: SessionId,
        server: String,
    },
}

impl GuardScope {
    fn of(owner: &McpConstructionOwner, server: &str) -> Self {
        match &owner.member {
            Some(member) => Self::Member {
                mob_id: member.mob_id.clone(),
                role: member.role.clone(),
                member: member.member.clone(),
                server: server.to_owned(),
            },
            None => Self::Session {
                session_id: owner.session_id.clone(),
                server: server.to_owned(),
            },
        }
    }
}

/// One tracked guard call.
struct GuardTask {
    key: McpAttemptKey,
    scope: GuardScope,
    stage: McpGuardStage,
    /// A finish's disposition, or a begin's own outcome once it is known (a
    /// begin not called, or refused by the factory).
    disposition: OnceLock<McpAttemptDisposition>,
    done: tokio::sync::watch::Sender<Option<McpGuardCompletion>>,
}

impl GuardTask {
    fn new(
        key: McpAttemptKey,
        scope: GuardScope,
        stage: McpGuardStage,
        disposition: Option<McpAttemptDisposition>,
    ) -> Arc<Self> {
        let recorded = OnceLock::new();
        if let Some(disposition) = disposition {
            let _ = recorded.set(disposition);
        }
        Arc::new(Self {
            key,
            scope,
            stage,
            disposition: recorded,
            done: tokio::sync::watch::channel(None).0,
        })
    }

    fn complete(&self, completion: McpGuardCompletion) {
        self.done.send_replace(Some(completion));
    }

    /// Record a begin's own outcome, nothing spawned.
    fn begin_outcome(&self, open: McpAttemptOpenOutcome) {
        let _ = self.disposition.set(McpAttemptDisposition {
            key: self.key.clone(),
            open,
            process: McpAttemptProcessEnd::NotSpawned,
        });
    }

    /// A begin that was not called: its outcome, completed `Cancelled`.
    fn not_called(&self, open: McpAttemptOpenOutcome) {
        self.begin_outcome(open);
        self.complete(McpGuardCompletion::Cancelled);
    }

    fn completion(&self) -> Option<McpGuardCompletion> {
        *self.done.borrow()
    }

    async fn wait_until(&self, deadline: tokio::time::Instant) -> Option<McpGuardCompletion> {
        let mut done = self.done.subscribe();
        match tokio::time::timeout_at(deadline, done.wait_for(Option::is_some)).await {
            Ok(Ok(completion)) => *completion,
            Ok(Err(_)) | Err(_) => self.completion(),
        }
    }

    fn record(&self, completion: McpGuardCompletion) -> McpGuardCompletionRecord {
        McpGuardCompletionRecord {
            key: self.key.clone(),
            stage: self.stage,
            disposition: self.disposition.get().cloned(),
            completion,
        }
    }
}

/// One router's part of an installation: its construction, its owner, and
/// the cancellations and records of its attempts.
pub(crate) struct RouterGuards {
    installation: Arc<Installation>,
    construction: McpConstructionId,
    owner: Arc<McpConstructionOwner>,
    /// Set once the router closed (or was dropped, or its installation
    /// closed): every later attempt is cancelled with it.
    closed: OnceLock<McpAttemptCancelCause>,
    attempts: Mutex<Vec<Weak<CancellationInner>>>,
    records: Mutex<Vec<Weak<AttemptGuardRecord>>>,
}

impl RouterGuards {
    pub(crate) fn new(
        guards: &McpAttemptGuards,
        construction: McpConstructionId,
        owner: McpConstructionOwner,
    ) -> Arc<Self> {
        let router = Arc::new(Self {
            installation: Arc::clone(&guards.0),
            construction,
            owner: Arc::new(owner),
            closed: OnceLock::new(),
            attempts: Mutex::new(Vec::new()),
            records: Mutex::new(Vec::new()),
        });
        let installation_closed = {
            let mut state = lock(&guards.0.state);
            state.routers.retain(|router| router.strong_count() > 0);
            state.routers.push(Arc::downgrade(&router));
            state.closed
        };
        if installation_closed {
            router.close(McpAttemptCancelCause::InstallationClosed);
        }
        router
    }

    /// Cancel every running attempt of this router with `cause`, and give
    /// every record whose open outcome is still unknown `Cancelled{cause}`.
    /// The first cause stands for later attempts too.
    pub(crate) fn close(&self, cause: McpAttemptCancelCause) {
        let cause = *self.closed.get_or_init(|| cause);
        let attempts: Vec<McpAttemptCancellation> = {
            let mut attempts = lock(&self.attempts);
            attempts.retain(|attempt| attempt.strong_count() > 0);
            attempts
                .iter()
                .filter_map(McpAttemptCancellation::upgrade)
                .collect()
        };
        for attempt in attempts {
            attempt.request(cause);
        }
        let records: Vec<Arc<AttemptGuardRecord>> = {
            let mut records = lock(&self.records);
            records.retain(|record| record.strong_count() > 0);
            records.iter().filter_map(Weak::upgrade).collect()
        };
        for record in records {
            record.open(McpAttemptOpenOutcome::Cancelled { cause });
        }
    }

    fn closed_cause(&self) -> Option<McpAttemptCancelCause> {
        self.closed.get().copied().or_else(|| {
            self.installation
                .is_closed()
                .then_some(McpAttemptCancelCause::InstallationClosed)
        })
    }

    /// Await this router's begins (by the cancel bound) and then its
    /// finishes (by the settle bound), and report them with the guards it
    /// still holds.
    pub(crate) async fn join(&self) -> Vec<McpGuardCompletionRecord> {
        let construction = self.construction;
        let bounds = self.installation.bounds;
        self.installation
            .join(
                &move |key: &McpAttemptKey| key.construction == construction,
                tokio::time::Instant::now() + bounds.cancel(),
                JoinBy::Within(bounds.settle()),
            )
            .await
            .records
    }

    /// Begin the guard of one stdio attempt, before anything of it runs.
    /// `Ok(Some)` carries its record (the attempt's custody holds it), and
    /// `Ok(None)` lets the attempt proceed unguarded. An `Err` refuses the
    /// attempt with nothing spawned: the factory's refusal, or
    /// `Unavailable` when the attempt stopped waiting (it is cancelled) or a
    /// prior guard of its owner and server was still finishing.
    pub(crate) async fn begin_attempt(
        self: &Arc<Self>,
        server: &McpServerConfig,
        attempt: u64,
        operation: McpAttemptOperation,
    ) -> Result<Option<Arc<AttemptGuardRecord>>, crate::McpError> {
        let key = McpAttemptKey {
            construction: self.construction,
            server: server.name.clone(),
            attempt,
        };
        let scope = GuardScope::of(&self.owner, &server.name);
        let installation = &self.installation;
        let bounds = installation.bounds;
        // The begin is tracked from here, before it waits for prior guards,
        // so a close that starts meanwhile waits for it and reports it. A
        // closed installation tracks no new begin.
        let task = GuardTask::new(key.clone(), scope.clone(), McpGuardStage::Begin, None);
        if !installation.track_begin(&task) {
            task.not_called(McpAttemptOpenOutcome::Cancelled {
                cause: McpAttemptCancelCause::InstallationClosed,
            });
            installation.track(&task);
            return Err(unavailable());
        }
        let cancel = McpAttemptCancellation::new();
        let pending = PendingBegin {
            task: Some(Arc::clone(&task)),
            cancel: cancel.clone(),
        };
        // While the attempt waits, its router's close requests the
        // cancellation (at once when the router is already closed).
        let _registered = self.register_attempt(&cancel);
        if let Some(cause) = cancel.cause() {
            pending.not_called(McpAttemptOpenOutcome::Cancelled { cause });
            return Err(unavailable());
        }
        let prior = tokio::select! {
            biased;
            cause = cancel.requested() => Err(cause),
            settled = installation.await_prior(&scope, &task, bounds.settle()) => Ok(settled),
        };
        match prior {
            Err(cause) => {
                pending.not_called(McpAttemptOpenOutcome::Cancelled { cause });
                return Err(unavailable());
            }
            Ok(false) => {
                pending.not_called(McpAttemptOpenOutcome::Refused {
                    refusal: McpAttemptRefusal::PriorGuardUnsettled,
                });
                return Err(unavailable());
            }
            Ok(true) => {}
        }
        if let Some(cause) = cancel.cause() {
            pending.not_called(McpAttemptOpenOutcome::Cancelled { cause });
            return Err(unavailable());
        }
        let Some(task) = pending.call() else {
            return Err(unavailable());
        };
        let runtime = tokio::runtime::Handle::current();
        let (handoff, delivered) = tokio::sync::oneshot::channel();
        spawn_begin(
            self,
            &runtime,
            McpAttemptGuardRequest {
                key,
                owner: Arc::clone(&self.owner),
                server: server.clone(),
                operation,
            },
            cancel.clone(),
            handoff,
            scope,
            task,
        );
        let mut waiting = BeginWait {
            delivered: Some(delivered),
            cancel: cancel.clone(),
        };
        let deadline = tokio::time::Instant::now() + bounds.begin();
        let answer = tokio::select! {
            biased;
            answer = waiting.delivered() => Some(answer),
            _ = cancel.requested() => None,
            () = tokio::time::sleep_until(deadline) => {
                cancel.request(McpAttemptCancelCause::BeginBound);
                None
            }
        };
        let Some(answer) = answer else {
            // Stopped waiting: a guard delivered meanwhile is cancelled.
            waiting.give_up();
            return Err(unavailable());
        };
        waiting.disarm();
        #[cfg(test)]
        {
            let mut held = installation.handover_hold.subscribe();
            let _ = held.wait_for(|held| !*held).await;
        }
        if let Some(cause) = cancel.cause() {
            // Answered, but the attempt was cancelled meanwhile: the guard is
            // late, and the attempt stops with nothing spawned.
            if let Ok(Ok(Some(record))) = answer {
                record.cancel_unspawned(cause);
            }
            return Err(unavailable());
        }
        match answer {
            // The record arrives registered (see `spawn_begin`).
            Ok(Ok(record)) => Ok(record),
            Ok(Err(refusal)) => Err(crate::McpError::AttachmentRefused(refusal)),
            // The begin ended without an answer (it panicked).
            Err(_) => Err(unavailable()),
        }
    }

    /// The record of a guard `begin` returned for `key`, registered with
    /// this router and the installation before it is handed to its attempt.
    fn record(
        &self,
        key: McpAttemptKey,
        scope: GuardScope,
        runtime: tokio::runtime::Handle,
        guard: Box<dyn McpAttemptGuard>,
    ) -> Arc<AttemptGuardRecord> {
        let record = Arc::new(AttemptGuardRecord {
            key,
            scope,
            installation: Arc::clone(&self.installation),
            runtime,
            state: Mutex::new(RecordState {
                guard: Some(guard),
                open: None,
                process: None,
            }),
        });
        self.register_record(&record);
        record
    }

    fn register_attempt(&self, cancel: &McpAttemptCancellation) -> AttemptRegistration<'_> {
        let attempt = cancel.downgrade();
        let closed = {
            let mut attempts = lock(&self.attempts);
            attempts.retain(|attempt| attempt.strong_count() > 0);
            attempts.push(attempt.clone());
            self.closed_cause()
        };
        if let Some(cause) = closed {
            cancel.request(cause);
        }
        AttemptRegistration {
            router: self,
            attempt,
        }
    }

    fn register_record(&self, record: &Arc<AttemptGuardRecord>) {
        let closed = {
            let mut records = lock(&self.records);
            records.retain(|record| record.strong_count() > 0);
            records.push(Arc::downgrade(record));
            self.closed_cause()
        };
        {
            let mut state = lock(&self.installation.state);
            state.records.retain(|record| record.strong_count() > 0);
            state.records.push(Arc::downgrade(record));
        }
        if let Some(cause) = closed {
            record.open(McpAttemptOpenOutcome::Cancelled { cause });
        }
    }
}

/// A waiting attempt's cancellation, registered with its router: dropped,
/// it deregisters, so a later close never requests the token of an attempt
/// that is no longer waiting for its begin.
struct AttemptRegistration<'a> {
    router: &'a RouterGuards,
    attempt: Weak<CancellationInner>,
}

impl Drop for AttemptRegistration<'_> {
    fn drop(&mut self) {
        lock(&self.router.attempts)
            .retain(|attempt| !attempt.ptr_eq(&self.attempt) && attempt.strong_count() > 0);
    }
}

/// One attempt's tracked begin until it is called. Dropped first (its
/// attempt's task dropped while it waits for prior guards), it requests the
/// attempt's cancellation and records the begin not called.
struct PendingBegin {
    task: Option<Arc<GuardTask>>,
    cancel: McpAttemptCancellation,
}

impl PendingBegin {
    fn not_called(mut self, open: McpAttemptOpenOutcome) {
        if let Some(task) = self.task.take() {
            task.not_called(open);
        }
    }

    /// The begin is called: its task is the begin's from here.
    fn call(mut self) -> Option<Arc<GuardTask>> {
        self.task.take()
    }
}

impl Drop for PendingBegin {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            self.cancel.request(McpAttemptCancelCause::AttemptDropped);
            task.not_called(McpAttemptOpenOutcome::Cancelled {
                cause: self
                    .cancel
                    .cause()
                    .unwrap_or(McpAttemptCancelCause::AttemptDropped),
            });
        }
    }
}

fn unavailable() -> crate::McpError {
    crate::McpError::AttachmentRefused(McpAttachmentRefusal::Unavailable)
}

/// Whether an attempt of `config` is guarded: only stdio attempts are.
pub(crate) fn guards_transport(config: &McpServerConfig) -> bool {
    matches!(
        config.transport,
        meerkat_core::mcp_config::McpTransportConfig::Stdio(_)
    )
}

/// The guard operation of a surface completion obligation.
pub(crate) fn attempt_operation(
    operation: meerkat_core::ExternalToolSurfaceDeltaOperation,
) -> McpAttemptOperation {
    match operation {
        meerkat_core::ExternalToolSurfaceDeltaOperation::Reload => McpAttemptOperation::Reload,
        _ => McpAttemptOperation::Add,
    }
}

/// The open outcome of a failed attempt: who refused it, or its setup
/// failure with the attempt's own classified stdio failure, if any.
pub(crate) fn open_for_error(
    error: &crate::McpError,
    failure: Option<McpStdioSetupFailure>,
) -> McpAttemptOpenOutcome {
    use crate::McpError as E;
    let refused = |refusal| McpAttemptOpenOutcome::Refused { refusal };
    match error {
        E::AttachmentRefused(_) => refused(McpAttemptRefusal::Control),
        E::Confinement(_) => refused(McpAttemptRefusal::Confinement),
        E::ServiceSecretRefused { .. } => refused(McpAttemptRefusal::ServiceSecret),
        E::InvalidAccessPolicy(_) | E::InvalidSecretEnv(_) | E::WorkingDirectory(_) => {
            refused(McpAttemptRefusal::InvalidConfig)
        }
        E::RouterShutDown => McpAttemptOpenOutcome::Cancelled {
            cause: McpAttemptCancelCause::RouterClosed,
        },
        _ => McpAttemptOpenOutcome::SetupFailed { failure },
    }
}

/// What a begin hands its attempt: its guard's record, already registered,
/// or no guard, or the factory's refusal.
type BeginAnswer = Result<Option<Arc<AttemptGuardRecord>>, McpAttachmentRefusal>;

/// Run the attempt's tracked begin (`task`) for `router` on `runtime`, never
/// dropped. A guard it returns is recorded and registered here, before the
/// begin is complete, so a join that sees the begin complete sees the guard
/// held (or its finish tracked); the record then goes to the waiting
/// attempt. When the attempt is gone, the record is cancelled here, nothing
/// spawned. A refusal is recorded on the begin as `Refused{Guard}`.
fn spawn_begin(
    router: &Arc<RouterGuards>,
    runtime: &tokio::runtime::Handle,
    request: McpAttemptGuardRequest,
    cancel: McpAttemptCancellation,
    handoff: tokio::sync::oneshot::Sender<BeginAnswer>,
    scope: GuardScope,
    task: Arc<GuardTask>,
) {
    let key = request.key.clone();
    let factory = Arc::clone(&router.installation.factory);
    let router = Arc::clone(router);
    let record_runtime = runtime.clone();
    runtime.spawn(async move {
        let began = AssertUnwindSafe(factory.begin(request, cancel.clone()))
            .catch_unwind()
            .await;
        let completion = match began {
            Err(_) => McpGuardCompletion::Panicked,
            Ok(answer) => {
                let answer = match answer {
                    Ok(Some(guard)) => Ok(Some(router.record(key, scope, record_runtime, guard))),
                    Ok(None) => Ok(None),
                    Err(refusal) => {
                        task.begin_outcome(McpAttemptOpenOutcome::Refused {
                            refusal: McpAttemptRefusal::Guard,
                        });
                        Err(refusal)
                    }
                };
                match handoff.send(answer) {
                    Ok(()) if cancel.is_requested() => McpGuardCompletion::Cancelled,
                    Ok(()) => McpGuardCompletion::Completed,
                    Err(answer) => {
                        // The attempt is gone: its guard is cancelled, and
                        // its finish tracked before this begin completes.
                        if let Ok(Some(record)) = answer {
                            record.cancel_unspawned(
                                cancel
                                    .cause()
                                    .unwrap_or(McpAttemptCancelCause::AttemptDropped),
                            );
                        }
                        McpGuardCompletion::Cancelled
                    }
                }
            }
        };
        task.complete(completion);
    });
}

/// An attempt waiting for its begin. Dropped while still waiting (its task
/// dropped), it cancels the attempt and cancels a guard already delivered.
struct BeginWait {
    delivered: Option<tokio::sync::oneshot::Receiver<BeginAnswer>>,
    cancel: McpAttemptCancellation,
}

impl BeginWait {
    async fn delivered(&mut self) -> Result<BeginAnswer, tokio::sync::oneshot::error::RecvError> {
        match self.delivered.as_mut() {
            Some(delivered) => delivered.await,
            None => std::future::pending().await,
        }
    }

    /// The answer was taken: nothing is left to rescue.
    fn disarm(&mut self) {
        self.delivered = None;
    }

    /// Stop waiting: no later answer can be delivered, and a guard already
    /// delivered is cancelled.
    fn give_up(&mut self) {
        let Some(mut delivered) = self.delivered.take() else {
            return;
        };
        delivered.close();
        if let Ok(Ok(Some(record))) = delivered.try_recv() {
            record.cancel_unspawned(
                self.cancel
                    .cause()
                    .unwrap_or(McpAttemptCancelCause::AttemptDropped),
            );
        }
    }
}

impl Drop for BeginWait {
    fn drop(&mut self) {
        if self.delivered.is_some() {
            self.cancel.request(McpAttemptCancelCause::AttemptDropped);
            self.give_up();
        }
    }
}

/// The guard of one begun attempt, with its two facts. Its finish starts
/// once both are known; dropped first, it fills the unknown ones
/// (`Cancelled{RouterDropped}`, `Abandoned`) and starts it then.
pub(crate) struct AttemptGuardRecord {
    key: McpAttemptKey,
    scope: GuardScope,
    installation: Arc<Installation>,
    runtime: tokio::runtime::Handle,
    state: Mutex<RecordState>,
}

struct RecordState {
    guard: Option<Box<dyn McpAttemptGuard>>,
    open: Option<McpAttemptOpenOutcome>,
    process: Option<McpAttemptProcessEnd>,
}

impl std::fmt::Debug for AttemptGuardRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = lock(&self.state);
        f.debug_struct("AttemptGuardRecord")
            .field("key", &self.key)
            .field("open", &state.open)
            .field("finished", &state.guard.is_none())
            .finish_non_exhaustive()
    }
}

impl AttemptGuardRecord {
    /// The attempt's open outcome; the first one stands.
    pub(crate) fn open(&self, outcome: McpAttemptOpenOutcome) {
        let mut state = lock(&self.state);
        if state.open.is_none() {
            state.open = Some(outcome);
        }
        self.finish_if_known(state);
    }

    /// The attempt stopped before anything of it ran: cancelled with
    /// `cause` (unless its outcome is already known), nothing spawned.
    fn cancel_unspawned(&self, cause: McpAttemptCancelCause) {
        let mut state = lock(&self.state);
        if state.open.is_none() {
            state.open = Some(McpAttemptOpenOutcome::Cancelled { cause });
        }
        if state.process.is_none() {
            state.process = Some(McpAttemptProcessEnd::NotSpawned);
        }
        self.finish_if_known(state);
    }

    /// How the attempt's process ended; the first one stands.
    pub(crate) fn process_ended(&self, end: McpAttemptProcessEnd) {
        #[cfg(test)]
        if let Some(observer) = lock(&self.installation.on_process_end).clone() {
            observer(&end);
        }
        let mut state = lock(&self.state);
        if state.process.is_none() {
            state.process = Some(end);
        }
        self.finish_if_known(state);
    }

    fn finish_if_known(&self, mut state: std::sync::MutexGuard<'_, RecordState>) {
        let (Some(open), Some(process)) = (state.open.clone(), state.process.clone()) else {
            return;
        };
        let Some(guard) = state.guard.take() else {
            return;
        };
        let disposition = McpAttemptDisposition {
            key: self.key.clone(),
            open,
            process,
        };
        // Tracked before the record lets go of its guard: a join sees the
        // guard either held or tracked, never neither.
        let task = finish_task(&self.scope, &disposition);
        self.installation.track(&task);
        drop(state);
        spawn_finish_task(&self.runtime, task, guard, disposition);
    }

    /// A record of this guard while it is still held (its finish not
    /// started).
    fn held(&self) -> Option<McpGuardCompletionRecord> {
        let state = lock(&self.state);
        state.guard.as_ref()?;
        let stage = state
            .open
            .as_ref()
            .map_or(McpGuardStage::Cancel, finish_stage);
        Some(McpGuardCompletionRecord {
            key: self.key.clone(),
            stage,
            disposition: None,
            completion: McpGuardCompletion::Held,
        })
    }
}

impl Drop for AttemptGuardRecord {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(guard) = state.guard.take() else {
            return;
        };
        let disposition = McpAttemptDisposition {
            key: self.key.clone(),
            open: state
                .open
                .take()
                .unwrap_or(McpAttemptOpenOutcome::Cancelled {
                    cause: McpAttemptCancelCause::RouterDropped,
                }),
            process: state
                .process
                .take()
                .unwrap_or(McpAttemptProcessEnd::Abandoned),
        };
        self.installation
            .spawn_finish(&self.runtime, guard, self.scope.clone(), disposition);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The one predicate that decides which attempts are guarded: stdio
    /// only.
    #[test]
    fn only_stdio_attempts_are_guarded() {
        let stdio = McpServerConfig::stdio(
            "local",
            "/bin/true",
            vec![],
            std::collections::HashMap::new(),
        );
        let http = McpServerConfig::streamable_http(
            "remote",
            "https://example.com/mcp",
            std::collections::HashMap::new(),
        );
        let sse = McpServerConfig::sse(
            "events",
            "https://example.com/sse",
            std::collections::HashMap::new(),
        );
        assert!(guards_transport(&stdio));
        assert!(!guards_transport(&http));
        assert!(!guards_transport(&sse));
    }

    /// Every typed refusal maps to its refuser; anything else is a setup
    /// failure carrying the attempt's own failure.
    #[test]
    fn errors_map_to_their_open_outcome() {
        use crate::McpError as E;
        let invalid =
            McpServerConfig::stdio("bad", "/bin/true", vec![], std::collections::HashMap::new());
        let mut invalid = invalid;
        if let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut invalid.transport {
            stdio.secret_env.insert("1BAD".to_string());
        }
        let secret_env = invalid.validate_secret_env().unwrap_err();
        let cases = [
            (
                E::AttachmentRefused(McpAttachmentRefusal::Denied),
                McpAttemptOpenOutcome::Refused {
                    refusal: McpAttemptRefusal::Control,
                },
            ),
            (
                E::Confinement(
                    meerkat_core::confinement::ConfinementRefusal::UnsupportedRequirement,
                ),
                McpAttemptOpenOutcome::Refused {
                    refusal: McpAttemptRefusal::Confinement,
                },
            ),
            (
                E::InvalidSecretEnv(secret_env),
                McpAttemptOpenOutcome::Refused {
                    refusal: McpAttemptRefusal::InvalidConfig,
                },
            ),
            (
                E::RouterShutDown,
                McpAttemptOpenOutcome::Cancelled {
                    cause: McpAttemptCancelCause::RouterClosed,
                },
            ),
            (
                E::ConnectionFailed {
                    reason: "closed".into(),
                },
                McpAttemptOpenOutcome::SetupFailed { failure: None },
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(open_for_error(&error, None), expected, "{error}");
        }
    }
}
