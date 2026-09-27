//! Re-link fork_off children after a host restart.
//!
//! A detached fork child's outcome is delivered by a process-local custodian.
//! If the host restarts while a child runs, that custodian is gone while the
//! child (and its durable [`meerkat_mob::ForkJobRecord`]) survives. This pass
//! runs once after restore and, for every seated child with a fork job:
//!
//! - job already over (its completion was admitted to the forker before the
//!   restart): left alone, whatever its limit, since the child may be doing
//!   later work that is not this job's; except a job that ended by its limit
//!   (its committed record is `max_run_elapsed`), whose child is retired if a
//!   crash or failure left it seated;
//! - already finished (its reply to the job is durable: the runtime's
//!   terminal receipt for the job turn's input, or, for a record without a
//!   turn delivery identity, the child's transcript after the fork prefix):
//!   delivers that result, however late the restart landed, and leaves the
//!   child seated; the opt-in `max_run` limit only bounds a run still going;
//! - still running: waits for the run to end, racing the opt-in `max_run`
//!   limit measured from the original start, then delivers the outcome; when
//!   the limit wins, the run is cancelled and `max_run_elapsed` delivered,
//!   and the child (with its descendants) is retired only once that outcome
//!   is settled, so a delivery that must wait keeps the job on record;
//! - idle with its durable reply: delivers that result;
//! - idle without one (the turn did not survive the restart): delivers a
//!   `restart_interrupted` outcome and leaves the child seated for its forker.
//!
//! A job whose turn was admitted under a stable delivery identity (every job
//! on a runtime-backed host) is settled from the runtime's terminal receipt
//! for that input alone ([`relink_by_receipt`]): a completed turn is
//! `completed`; the turn's own failure is `failed` with its typed error, and
//! the child is retired as the live custodian retires it; an end imposed from
//! outside (stop, destroy, cancel), an input no run answered, or one never
//! admitted is `restart_interrupted`. An input still owed a terminal is
//! watched, never settled from member status: after a restart it is requeued
//! and the child can read idle before the recovered run opens.
//!
//! A status read that does not observe the child (the mob's status
//! observation capacity stayed full past its admission wait, the actor did
//! not answer, or a read failed) says nothing about the child's state: the
//! pass reads again after a growing pause, and never takes it for an idle
//! child.
//! A read whose run state is unknown (the member was busy and the status
//! projection's bounded runtime read did not answer) is settled by reading
//! the child's runtime state directly.
//!
//! The owner is resolved from the job's owner session before anything can
//! retire the child, never from the child's roster entry. A member of the
//! child's mob is revived through its mob. A child forked in its forker's
//! turn (fork_off) is owned by a member of its own mob, so an owner session
//! no longer seated there (the forker was respawned or retired) is gone. Any
//! other owner (a job a library host bound) is looked up among the host's
//! managed mobs when delivering, read afresh, and is otherwise a plain
//! session, revived through the host's owner hook.
//!
//! Delivery is the same durable completion record the live custodian admits
//! ([`crate::detached_delivery`]), under the same idempotency key, so a job
//! already delivered before the restart is never recorded twice, and an idle
//! owner is woken to see it. A forker that cannot be revived yet, because
//! its mob is not running (a host that restores a stopped mob and activates
//! it later) or the mob's resume of it is still in progress, is reported as
//! [`ForkRelinkAction::AwaitingOwner`], and the automatic pass delivers again
//! once that clears.

use std::sync::Arc;
use std::time::Duration;

use std::collections::BTreeMap;

use meerkat_mob::{
    AgentIdentity, ForkJobRecord, MemberRunState, MobControlPrincipal, MobError, MobHandle, MobId,
};

use crate::MobMcpState;
use crate::agent_tools::{
    ForkOffCompletion, ForkOffCompletionStatus, RestartInterruptedReason, TOOL_FORK_OFF,
};
use crate::detached_delivery::{DetachedOwnerHost, OwnerRevivalDeferral};
#[cfg(target_arch = "wasm32")]
use crate::tokio;

/// What the re-link pass did for one child.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForkRelinkAction {
    /// The outcome was recorded in the forker's transcript now.
    Delivered,
    /// The outcome had already been recorded before the restart.
    AlreadyDelivered,
    /// The forker is gone (retired, or its session archived or deleted), so
    /// the outcome can never be delivered.
    OwnerGone,
    /// The owner, a member of mob `mob_id`, cannot be revived to receive the
    /// outcome yet, for a reason that clears on its own. The automatic pass
    /// delivers again once it has, waiting on that mob.
    AwaitingOwner {
        mob_id: MobId,
        reason: OwnerRevivalDeferral,
    },
    /// Delivery failed.
    Failed(String),
}

/// One child's re-link result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkRelinkReport {
    pub mob_id: MobId,
    pub child: AgentIdentity,
    pub job_id: String,
    pub action: ForkRelinkAction,
}

/// How a re-link delivers an outcome to a job's owner.
#[derive(Clone, Default)]
pub struct RelinkDelivery {
    /// The runtime that admits completions. Without one nothing can be
    /// delivered on this host.
    pub runtime: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    /// The host hook that makes an owner that is a plain session live (see
    /// [`DetachedOwnerHost`]).
    pub owner_host: Option<Arc<dyn DetachedOwnerHost>>,
    /// Mobs, besides the child's own, whose members may own a job bound
    /// outside the child's mob: such an owner is revived through its mob.
    pub owner_mobs: Vec<MobHandle>,
    /// The host's managed mobs, read afresh each time such an owner is
    /// looked up, so a mob inserted later is found.
    pub managed_mobs: Option<ManagedMobs>,
    /// Counts the deferred outcomes waiting on their owner's mob right now.
    pub waiting_owners: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl RelinkDelivery {
    /// The delivery of `state`: its runtime and owner hook, and its managed
    /// mobs as a live view.
    pub(crate) fn from_state(state: &MobMcpState) -> Self {
        Self {
            runtime: state.runtime_adapter_for_relink(),
            owner_host: state.detached_owner_host(),
            owner_mobs: Vec::new(),
            managed_mobs: Some(state.managed_mobs()),
            waiting_owners: Some(state.fork_relink_waiting_owners_gauge()),
        }
    }

    /// The handle of mob `mob_id`: the child's own (`child_handle`), or one
    /// of [`Self::owner_mobs`] or the managed mobs now.
    async fn mob_handle(&self, mob_id: &MobId, child_handle: &MobHandle) -> Option<MobHandle> {
        if child_handle.mob_id() == mob_id {
            return Some(child_handle.clone());
        }
        if let Some(found) = self
            .owner_mobs
            .iter()
            .find(|candidate| candidate.mob_id() == mob_id)
        {
            return Some(found.clone());
        }
        match &self.managed_mobs {
            Some(managed) => managed
                .handles()
                .await
                .into_iter()
                .find(|candidate| candidate.mob_id() == mob_id),
            None => None,
        }
    }

    /// The member seated on `owner_session_id` in a mob other than
    /// `child_mob`, among [`Self::owner_mobs`] and the managed mobs now.
    async fn member_elsewhere(
        &self,
        child_mob: &MobId,
        owner_session_id: &meerkat_core::SessionId,
    ) -> Option<(MobHandle, AgentIdentity)> {
        let mut candidates = self.owner_mobs.clone();
        if let Some(managed) = &self.managed_mobs {
            candidates.extend(managed.handles().await);
        }
        for candidate in candidates {
            if candidate.mob_id() == child_mob {
                continue;
            }
            let owner = candidate
                .roster()
                .await
                .find_by_bridge_session_id(owner_session_id)
                .map(|entry| entry.agent_identity.clone());
            if let Some(owner) = owner {
                return Some((candidate, owner));
            }
        }
        None
    }
}

/// A live view of the mobs a host manages (see
/// [`RelinkDelivery::managed_mobs`]).
#[derive(Clone)]
pub struct ManagedMobs {
    mobs: Arc<tokio::sync::RwLock<BTreeMap<MobId, crate::ManagedMob>>>,
    principal: MobControlPrincipal,
}

impl ManagedMobs {
    pub(crate) fn new(
        mobs: Arc<tokio::sync::RwLock<BTreeMap<MobId, crate::ManagedMob>>>,
        principal: MobControlPrincipal,
    ) -> Self {
        Self { mobs, principal }
    }

    /// The handles of every mob managed now.
    async fn handles(&self) -> Vec<MobHandle> {
        self.mobs
            .read()
            .await
            .values()
            .map(|managed| {
                managed.handle.clone().with_command_authority(
                    meerkat_mob::CommandAuthority::principal(self.principal.clone()),
                )
            })
            .collect()
    }
}

const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// How long the re-link keeps waiting on a child whose member status reads
/// settled while its last turn's boundary commit is not confirmed, before it
/// stops waiting for that commit.
///
/// The commit is unconfirmed while the runtime shows machine evidence that it
/// has not landed (a run input still `Staged`, `Applied` or
/// `AppliedPendingConsumption`, or degraded durability after a boundary
/// commit failed) or while its read is inconclusive (it timed out or failed).
/// A healthy boundary commit is one store transaction behind the run (23-76
/// ms measured on a loaded 4-core runner), so five minutes is far past any
/// commit that is going to land. The ceiling bounds the ones that do not,
/// for example a commit whose failure left durability degraded, which a job
/// without `max_run` would otherwise wait on for good.
///
/// At the ceiling the re-link delivers `restart_interrupted`. The typed
/// reason `commit_never_landed` is set only when the reading at the ceiling
/// is machine evidence; an inconclusive reading carries no reason. Only a
/// reading that shows the child running again restarts the wait; a status
/// read that did not observe the child does not. [`relink_child_within`]
/// takes the ceiling explicitly.
pub const COMMIT_PENDING_CEILING: Duration = Duration::from_secs(300);

/// Upper bound on one read of a child's run inputs. The read waits for the
/// child's session driver, which a boundary commit in progress holds; a read
/// that has not answered in this long is inconclusive (the driver was busy,
/// with a commit or with other work), not evidence of a pending commit.
const RUN_INPUT_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on one read of a child's member status in the receipt watch
/// (it reads the mob actor, and may read the runtime); a read that does not
/// answer in this long is inconclusive.
const STATUS_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How long one wait on a job turn's receipt lasts before the re-link looks
/// at the child's status again (the wait returns as soon as the receipt
/// exists). The last read of the receipt before a ceiling delivery waits
/// this long too.
const RECEIPT_WAIT_SLICE: Duration = Duration::from_secs(2);

/// Last receipt reads at the commit ceiling in a row that may say nothing
/// (the read did not observe the member or its runtime within its bound, or
/// it failed) before the receipt watch stops waiting. Such a read is not
/// evidence that no receipt exists: a read that waits for a driver the
/// receipt's own commit holds reads the same. Once this many are spent the
/// watch delivers `restart_interrupted` naming no cause; a reading that
/// shows the child running starts the count again.
const MAX_INCONCLUSIVE_CEILING_READS: u32 = 3;

/// First pause before reading a child's status again after a read that did
/// not observe it. Each further unobserved read in a row doubles the pause, up
/// to [`UNOBSERVED_RETRY_MAX_INTERVAL`]; an observed read resets it.
const UNOBSERVED_RETRY_INITIAL_INTERVAL: Duration = Duration::from_millis(250);

/// Longest pause between unobserved reads. An unobserved read already waited
/// out the status lane's own admission bound, so retrying faster only adds
/// load to a mob whose status capacity is saturated.
const UNOBSERVED_RETRY_MAX_INTERVAL: Duration = Duration::from_secs(5);

/// Pause schedule between status reads that did not observe a child.
#[derive(Debug, Default)]
struct UnobservedBackoff {
    consecutive: u32,
}

impl UnobservedBackoff {
    /// The pause before the next read after one more unobserved read.
    fn next_pause(&mut self) -> Duration {
        let pause = UNOBSERVED_RETRY_INITIAL_INTERVAL
            .saturating_mul(2_u32.saturating_pow(self.consecutive))
            .min(UNOBSERVED_RETRY_MAX_INTERVAL);
        self.consecutive = self.consecutive.saturating_add(1);
        pause
    }

    /// An observed read restarts the schedule.
    fn reset(&mut self) {
        self.consecutive = 0;
    }
}

/// Times the automatic pass waits for a deferred forker revival to clear and
/// delivers again.
const MAX_OWNER_REVIVAL_WAITS: u32 = 16;

fn now_ms() -> u64 {
    u64::try_from(
        meerkat_core::time_compat::SystemTime::now()
            .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

/// `bound`, cut to the time left before the job's `max_run` deadline
/// (`deadline_ms`, unix milliseconds) when it has one, so that no pause or
/// read in a watch outlasts the limit.
fn within_limit(bound: Duration, deadline_ms: Option<u64>) -> Duration {
    deadline_ms.map_or(bound, |deadline| {
        bound.min(Duration::from_millis(deadline.saturating_sub(now_ms())))
    })
}

/// Re-link every fork child in every managed mob whose job started before
/// `restored_before_ms` (children forked by this process already have a live
/// custodian).
pub async fn relink_restored_fork_children(
    state: &Arc<MobMcpState>,
    restored_before_ms: u64,
    respect_claims: bool,
) -> Vec<ForkRelinkReport> {
    let mut reports = Vec::new();
    let Ok(handles) = state.mob_handles_snapshot().await else {
        return reports;
    };
    let delivery = RelinkDelivery::from_state(state);
    for (mob_id, handle) in handles {
        let claimed = state.claim_fork_relink(&mob_id);
        if claimed || !respect_claims {
            let mob_reports = relink_mob_fork_children(
                state.session_service(),
                delivery.clone(),
                &mob_id,
                &handle,
                restored_before_ms,
            )
            .await;
            let awaiting_owner = mob_reports
                .iter()
                .any(|report| matches!(report.action, ForkRelinkAction::AwaitingOwner { .. }));
            if claimed && awaiting_owner {
                // This call holds the mob's one automatic re-link, so it
                // also owns delivering again once a deferred forker
                // revival clears.
                let service = state.session_service();
                let delivery = delivery.clone();
                let pending = mob_reports.clone();
                let (mob_id, handle) = (mob_id.clone(), handle.clone());
                tokio::spawn(async move {
                    redeliver_when_owners_revivable(
                        service,
                        delivery,
                        &mob_id,
                        &handle,
                        restored_before_ms,
                        pending,
                    )
                    .await;
                });
            }
            reports.extend(mob_reports);
        }
    }
    reports
}

/// Deliver again the outcomes `reports` could not deliver because the
/// owner could not be revived yet ([`ForkRelinkAction::AwaitingOwner`]).
/// Each such job waits on its own owner's mob (which may not be the
/// child's) with its own budget of [`MAX_OWNER_REVIVAL_WAITS`] waits: when
/// its reason clears there (the mob runs, or the operation in progress on the
/// owner has had time to finish) that job alone is delivered again, and the
/// other jobs keep waiting. A job stops waiting when its owner's mob can run
/// no more (completed, destroyed, lost its actor, no longer managed) or its
/// budget is spent. Returns the final report of every child in `reports`.
/// Delivery is idempotent per job, so nothing is recorded twice.
pub(crate) async fn redeliver_when_owners_revivable(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    delivery: RelinkDelivery,
    mob_id: &MobId,
    handle: &MobHandle,
    restored_before_ms: u64,
    reports: Vec<ForkRelinkReport>,
) -> Vec<ForkRelinkReport> {
    let (awaiting, mut settled): (Vec<_>, Vec<_>) = reports
        .into_iter()
        .partition(|report| matches!(report.action, ForkRelinkAction::AwaitingOwner { .. }));
    let finished = redeliver_each(
        awaiting,
        |owner_mob, reason, attempt| {
            let delivery = &delivery;
            async move {
                let Some(owner_handle) = delivery.mob_handle(&owner_mob, handle).await else {
                    return false;
                };
                let _waiting = delivery.waiting_owners.as_deref().map(WaitingOwner::arm);
                reason.cleared(&owner_handle, attempt).await
            }
        },
        |child, job_id| {
            let (service, delivery) = (Arc::clone(&service), delivery.clone());
            async move {
                // Exactly this child's job: a job id is not unique across
                // children (bindings are the host's), and delivery dedup is
                // per owner session.
                relink_mob_fork_children_where(
                    service,
                    delivery,
                    mob_id,
                    handle,
                    restored_before_ms,
                    |candidate, job| candidate == &child && job.job_id == job_id,
                )
                .await
                .into_iter()
                .find(|report| report.child == child && report.job_id == job_id)
            }
        },
    )
    .await;
    let delivered = finished
        .iter()
        .filter(|report| report.action == ForkRelinkAction::Delivered)
        .count();
    if delivered > 0 {
        tracing::info!(
            mob_id = %mob_id,
            children = delivered,
            "fork_off re-link delivered outcomes once their owner could be revived"
        );
    }
    settled.extend(finished);
    settled
}

/// Drive every awaiting job to its own end, concurrently: each waits
/// (`wait(owner mob, reason, attempt)`, `true` once its owner may be
/// revivable) with its own budget, and a wake retries that job alone
/// (`retry(child, job_id)`, that child's new report, `None` when the child is
/// gone).
async fn redeliver_each<Wait, WaitFuture, Retry, RetryFuture>(
    awaiting: Vec<ForkRelinkReport>,
    wait: Wait,
    retry: Retry,
) -> Vec<ForkRelinkReport>
where
    Wait: Fn(MobId, OwnerRevivalDeferral, u32) -> WaitFuture,
    WaitFuture: std::future::Future<Output = bool>,
    Retry: Fn(AgentIdentity, String) -> RetryFuture,
    RetryFuture: std::future::Future<Output = Option<ForkRelinkReport>>,
{
    let (wait, retry) = (&wait, &retry);
    futures::future::join_all(awaiting.into_iter().map(|mut report| async move {
        for attempt in 0..MAX_OWNER_REVIVAL_WAITS {
            let ForkRelinkAction::AwaitingOwner { mob_id, reason } = &report.action else {
                break;
            };
            if !wait(mob_id.clone(), reason.clone(), attempt).await {
                break;
            }
            match retry(report.child.clone(), report.job_id.clone()).await {
                Some(next) => report = next,
                None => break,
            }
        }
        report
    }))
    .await
}

/// One deferred outcome waiting on its owner's mob, counted while it waits.
struct WaitingOwner<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> WaitingOwner<'a> {
    fn arm(gauge: &'a std::sync::atomic::AtomicUsize) -> Self {
        gauge.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(gauge)
    }
}

impl Drop for WaitingOwner<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Re-link the fork children of one mob (see the module docs).
pub async fn relink_mob_fork_children(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    delivery: RelinkDelivery,
    mob_id: &MobId,
    handle: &MobHandle,
    restored_before_ms: u64,
) -> Vec<ForkRelinkReport> {
    relink_mob_fork_children_where(
        service,
        delivery,
        mob_id,
        handle,
        restored_before_ms,
        |_, _| true,
    )
    .await
}

/// [`relink_mob_fork_children`] for the children `select` picks (by the
/// child's identity and its job).
async fn relink_mob_fork_children_where(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    delivery: RelinkDelivery,
    mob_id: &MobId,
    handle: &MobHandle,
    restored_before_ms: u64,
    select: impl Fn(&AgentIdentity, &ForkJobRecord) -> bool,
) -> Vec<ForkRelinkReport> {
    // One roster read lists the children and, before anything can retire a
    // child, resolves each job's owner among the mob's members.
    let roster = handle.roster().await;
    let mut children = Vec::new();
    for entry in roster.list() {
        let Some(job) = entry.fork_job.clone() else {
            continue;
        };
        if job.started_at_ms >= restored_before_ms || !select(&entry.agent_identity, &job) {
            continue;
        }
        let owner = match roster.find_by_bridge_session_id(&job.owner_session_id) {
            Some(owner) => JobOwner::Member(handle.clone(), owner.agent_identity.clone()),
            None => JobOwner::unseated(entry.spawned_by.is_some()),
        };
        children.push((entry.agent_identity.clone(), job, owner));
    }
    // Each child settles on its own task: observing a member waits for its
    // turn boundary, so one busy child must not hold up the others.
    let tasks = children.into_iter().map(|(child, job, owner)| {
        let service = Arc::clone(&service);
        let delivery = delivery.clone();
        let mob_id = mob_id.clone();
        let handle = handle.clone();
        tokio::spawn(async move {
            let action = relink_owned_child(
                service,
                &delivery,
                &mob_id,
                &handle,
                &child,
                &job,
                owner,
                COMMIT_PENDING_CEILING,
            )
            .await;
            ForkRelinkReport {
                mob_id,
                child,
                job_id: job.job_id.clone(),
                action,
            }
        })
    });
    futures::future::join_all(tasks)
        .await
        .into_iter()
        .filter_map(Result::ok)
        .collect()
}

/// Who receives a fork job's outcome. Resolved from the job's owner session
/// before anything can retire the child, never from the child's own roster
/// entry (a retired child has none).
#[derive(Clone)]
enum JobOwner {
    /// A member of the child's mob: revived through its mob.
    Member(MobHandle, AgentIdentity),
    /// The child was forked in its forker's turn (fork_off), so its owner is
    /// a member of its own mob; that session is not seated there any more
    /// (the forker was respawned or retired). The owner is gone.
    Gone,
    /// Bound to a session outside the child's mob (a job a library host
    /// bound): looked up among the host's mobs when delivering, and a plain
    /// session if it is a member of none.
    Elsewhere,
}

impl JobOwner {
    /// The owner of a job whose owner session is not seated in the child's
    /// mob. `forked_in_turn` is whether the child records its forker as its
    /// spawner.
    fn unseated(forked_in_turn: bool) -> Self {
        if forked_in_turn {
            Self::Gone
        } else {
            Self::Elsewhere
        }
    }

    /// The owner of `child`'s job, read from the child's mob (`handle`).
    async fn resolve(
        handle: &MobHandle,
        child: &AgentIdentity,
        owner_session_id: &meerkat_core::SessionId,
    ) -> Self {
        let roster = handle.roster().await;
        match roster.find_by_bridge_session_id(owner_session_id) {
            Some(owner) => Self::Member(handle.clone(), owner.agent_identity.clone()),
            None => Self::unseated(
                roster
                    .get_by_identity(child)
                    .is_some_and(|entry| entry.spawned_by.is_some()),
            ),
        }
    }
}

/// Settle one fork child and deliver its outcome to the job's owner.
///
/// A child whose reply to the job is already durable delivers that reply
/// first, before any limit is evaluated: the restart may land long after the
/// child finished within its limit, and its real outcome must not be replaced
/// by `max_run_elapsed`. Otherwise observing the child waits for its current
/// turn boundary. With an opt-in `max_run`, that wait is raced against the
/// limit measured from the job's original start. When the limit wins (with
/// still no durable reply) the child's run is cancelled and
/// `max_run_elapsed` delivered; the child and its descendants are retired
/// only once that outcome is settled (delivered, or its owner gone), so a
/// delivery that must wait leaves the job on record for the next pass.
///
/// The owner is resolved from the job's owner session first (see the module
/// docs); a plain-session owner is made live through
/// [`RelinkDelivery::owner_host`] when the runtime does not have it live.
pub async fn relink_child(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    delivery: &RelinkDelivery,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
) -> ForkRelinkAction {
    relink_child_within(
        service,
        delivery,
        mob_id,
        handle,
        child,
        job,
        COMMIT_PENDING_CEILING,
    )
    .await
}

/// [`relink_child`] with an explicit bound on how long a finished turn's
/// boundary commit is waited for (see [`COMMIT_PENDING_CEILING`]).
pub async fn relink_child_within(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    delivery: &RelinkDelivery,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
    commit_pending_ceiling: Duration,
) -> ForkRelinkAction {
    let owner = JobOwner::resolve(handle, child, &job.owner_session_id).await;
    relink_owned_child(
        service,
        delivery,
        mob_id,
        handle,
        child,
        job,
        owner,
        commit_pending_ceiling,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn relink_owned_child(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    delivery: &RelinkDelivery,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
    owner: JobOwner,
    commit_pending_ceiling: Duration,
) -> ForkRelinkAction {
    let runtime = delivery.runtime.as_deref();
    // A job whose completion the forker's runtime already admitted is over.
    // The child stays seated for further work that is no longer this job's,
    // so neither the job's limit nor another delivery applies to it. A job
    // whose delivered outcome retires its child (it ended by its limit, or
    // its own turn failed) is the exception: the child is retired, which a
    // crash or failure after delivery can have left undone (retiring is
    // idempotent). The delivered outcome is read typed from the committed
    // record: `restart_interrupted` shares the record's `failed` notice
    // status and must stay seated.
    if let Some(runtime) = runtime
        && crate::detached_delivery::detached_completion_admitted(
            runtime,
            &job.owner_session_id,
            TOOL_FORK_OFF,
            &job.job_id,
        )
        .await
    {
        if let Some(committed) = committed_completion(&service, mob_id, child, job).await
            && committed.retires_child()
            && let Err(error) = handle.retire_with_descendants(child.clone()).await
        {
            tracing::warn!(
                mob_id = %mob_id,
                child = %child,
                error = %error,
                "fork_off re-link could not retire a child whose delivered outcome retires it"
            );
        }
        return ForkRelinkAction::AlreadyDelivered;
    }
    if let Some(turn_delivery) = &job.turn_delivery {
        return relink_by_receipt(
            &service,
            delivery,
            &owner,
            mob_id,
            handle,
            child,
            job,
            turn_delivery,
            commit_pending_ceiling,
        )
        .await;
    }
    if let Some(completion) = durable_reply(&service, mob_id, handle, child, job).await {
        return deliver(delivery, &owner, mob_id, job, completion).await;
    }
    let deadline_ms = job
        .max_run_ms
        .map(|limit| job.started_at_ms.saturating_add(limit));
    let mut unobserved_backoff = UnobservedBackoff::default();
    let mut commit_watch = CommitWatch::new(commit_pending_ceiling);
    loop {
        // A limit already passed decides at once: the child's run is over
        // unless its reply is durable. Racing a status read against a
        // zero-length timer would decide by chance.
        let remaining_ms = deadline_ms.map(|deadline| deadline.saturating_sub(now_ms()));
        if remaining_ms == Some(0) {
            if let Some(completion) = durable_reply(&service, mob_id, handle, child, job).await {
                return deliver(delivery, &owner, mob_id, job, completion).await;
            }
            return limit_elapsed(&service, delivery, &owner, mob_id, handle, child, job).await;
        }
        let observe = observe_child(runtime, handle, child);
        let observed = match remaining_ms {
            None => Some(observe.await),
            Some(remaining_ms) => {
                tokio::select! {
                    observed = observe => Some(observed),
                    () = tokio::time::sleep(Duration::from_millis(remaining_ms)) => None,
                }
            }
        };
        let Some(observed) = observed else {
            // The limit ran down while the child was observed: the next
            // iteration decides it.
            continue;
        };
        if !matches!(observed, ChildObservation::Unobserved(_)) {
            unobserved_backoff.reset();
        }
        let (evidence, detail) = match observed {
            ChildObservation::Running => {
                commit_watch.progressed();
                tokio::time::sleep(within_limit(WATCH_INTERVAL, deadline_ms)).await;
                continue;
            }
            ChildObservation::Settled => {
                let completion = settled_outcome(&service, mob_id, handle, child, job).await;
                return deliver(delivery, &owner, mob_id, job, completion).await;
            }
            ChildObservation::Unobserved(detail) => {
                // The read says nothing about the child's state, so it
                // neither starts nor restarts the commit wait. A child that
                // finished meanwhile has a durable reply; otherwise read
                // again.
                if let Some(completion) = durable_reply(&service, mob_id, handle, child, job).await
                {
                    return deliver(delivery, &owner, mob_id, job, completion).await;
                }
                tracing::debug!(
                    mob_id = %mob_id,
                    child = %child,
                    detail = %detail,
                    "fork_off re-link could not observe the child; reading again"
                );
                tokio::time::sleep(within_limit(unobserved_backoff.next_pause(), deadline_ms))
                    .await;
                continue;
            }
            ChildObservation::CommitPending(evidence) => {
                // Degraded durability does not say the turn's own commit
                // failed: a reply that is durable is delivered now, not
                // held back to the ceiling.
                if evidence == CommitEvidence::DurabilityDegraded
                    && let Some(completion) =
                        durable_reply(&service, mob_id, handle, child, job).await
                {
                    return deliver(delivery, &owner, mob_id, job, completion).await;
                }
                (Some(evidence), None)
            }
            ChildObservation::CommitUnconfirmed(detail) => (None, Some(detail)),
        };
        let CommitWatchStep::Ceiling { reason } =
            commit_watch.unconfirmed(meerkat_core::time_compat::Instant::now(), evidence)
        else {
            tokio::time::sleep(within_limit(WATCH_INTERVAL, deadline_ms)).await;
            continue;
        };
        // A reply that did land wins over the bound.
        if let Some(completion) = durable_reply(&service, mob_id, handle, child, job).await {
            return deliver(delivery, &owner, mob_id, job, completion).await;
        }
        tracing::warn!(
            mob_id = %mob_id,
            child = %child,
            evidence = ?evidence,
            detail = ?detail,
            "fork_off re-link: the child's last turn never confirmed its commit; \
             delivering restart_interrupted"
        );
        let mut completion = ForkOffCompletion::empty(
            child.to_string(),
            member_ref(mob_id, child),
            ForkOffCompletionStatus::RestartInterrupted,
        );
        completion.restart_reason = reason;
        return deliver(delivery, &owner, mob_id, job, completion).await;
    }
}

/// Machine evidence that a settled child's last boundary commit has not
/// landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitEvidence {
    /// A run input is still `Staged`, `Applied` or
    /// `AppliedPendingConsumption`: the run's boundary has not consumed it.
    InputAwaitsBoundary,
    /// The child's runtime reports durability degraded: a durable write
    /// failed after the runtime changed its live state (a boundary commit
    /// that failed after the run consumed its inputs, or another operation's
    /// durable write), so the durable history may lag the live state. The
    /// turn's reply may still be durable, so the re-link checks for it.
    DurabilityDegraded,
}

/// The re-link's wait on a settled child whose commit is unconfirmed,
/// bounded by the commit ceiling.
struct CommitWatch {
    ceiling: Duration,
    since: Option<meerkat_core::time_compat::Instant>,
    /// Last receipt reads at the ceiling in a row that said nothing (see
    /// [`MAX_INCONCLUSIVE_CEILING_READS`]).
    inconclusive_ceiling_reads: u32,
}

/// What the commit wait decides after an unconfirmed reading.
#[derive(Debug, PartialEq, Eq)]
enum CommitWatchStep {
    KeepWatching,
    /// The ceiling passed. `reason` is `commit_never_landed` only when the
    /// reading at the ceiling was machine evidence.
    Ceiling {
        reason: Option<RestartInterruptedReason>,
    },
}

impl CommitWatch {
    fn new(ceiling: Duration) -> Self {
        Self {
            ceiling,
            since: None,
            inconclusive_ceiling_reads: 0,
        }
    }

    /// The child was read running again: the wait restarts, and so does the
    /// count of inconclusive reads at the ceiling.
    fn progressed(&mut self) {
        self.since = None;
        self.inconclusive_ceiling_reads = 0;
    }

    /// One more last receipt read at the ceiling said nothing. `true` once
    /// [`MAX_INCONCLUSIVE_CEILING_READS`] such reads in a row are spent: the
    /// watch stops waiting.
    fn inconclusive_ceiling_read(&mut self) -> bool {
        self.inconclusive_ceiling_reads = self.inconclusive_ceiling_reads.saturating_add(1);
        self.inconclusive_ceiling_reads >= MAX_INCONCLUSIVE_CEILING_READS
    }

    /// The child reads settled with its commit unconfirmed: by machine
    /// `evidence`, or inconclusively (`None`). The wait starts at the first
    /// such reading.
    fn unconfirmed(
        &mut self,
        now: meerkat_core::time_compat::Instant,
        evidence: Option<CommitEvidence>,
    ) -> CommitWatchStep {
        let since = *self.since.get_or_insert(now);
        if now.saturating_duration_since(since) < self.ceiling {
            return CommitWatchStep::KeepWatching;
        }
        CommitWatchStep::Ceiling {
            reason: evidence.map(|_| RestartInterruptedReason::CommitNeverLanded),
        }
    }
}

/// What reading a fork child's status says about it.
#[derive(Debug, PartialEq, Eq)]
enum ChildObservation {
    /// The child has a run open or work in flight.
    Running,
    /// The child's member status reads settled, but the runtime shows that
    /// its last turn's boundary commit (which makes its reply durable) has
    /// not landed. Its outcome is not readable yet.
    CommitPending(CommitEvidence),
    /// The child's member status reads settled, but the read of its run
    /// inputs was inconclusive: it timed out or failed. Not evidence either
    /// way.
    CommitUnconfirmed(String),
    /// The child is not running: idle with its commit landed, no longer
    /// seated, its runtime no longer holds it, or its mob's actor is gone
    /// (nothing runs it any more).
    Settled,
    /// The read did not observe the child, so it says nothing about the
    /// child's state: the mob's status observation capacity stayed full past
    /// its admission wait, the actor did not answer in time, or a read failed.
    Unobserved(String),
}

/// What a status read's run progress says, before any further read.
#[derive(Debug, PartialEq, Eq)]
enum ProgressVerdict {
    Running,
    Settled,
    /// The run state is unknown: the member was busy and the status
    /// projection's bounded runtime read did not answer. The runtime is
    /// asked directly.
    Undetermined,
}

impl ProgressVerdict {
    /// `progress` is the run state and in-flight work count of a status
    /// read, `None` when the read carries no run progress (a member placed
    /// on another host).
    fn of(progress: Option<(MemberRunState, u64)>) -> Self {
        match progress {
            Some((_, in_flight_work)) if in_flight_work > 0 => Self::Running,
            Some((MemberRunState::RunOpen, _)) => Self::Running,
            Some((MemberRunState::Unknown, _)) => Self::Undetermined,
            Some((MemberRunState::Idle, _)) | None => Self::Settled,
        }
    }
}

/// Read a fork child's status (see [`observe_child_run`]), where "settled"
/// also requires its last turn to be committed.
///
/// Member status reads the live agent, which is terminal before the turn's
/// boundary commit lands, while the outcome is read from the durable
/// transcript. The runtime owns the fact in between
/// ([`meerkat_runtime::MeerkatMachine::session_has_uncommitted_run_input`]):
/// a machine phase, not a comparison of transcripts, so a turn that compacted
/// inside the window reads the same. Its answer maps as follows:
/// - a run input awaiting its boundary, or degraded durability, is machine
///   evidence that the commit has not landed ([`ChildObservation::CommitPending`]);
/// - no such input, under healthy durability, settles the child;
/// - a runtime that no longer holds the child's session (not found,
///   destroyed, not ready) settles it too: for this workflow nothing is left
///   to wait for, and nothing more can be delivered from that runtime;
/// - a read that timed out or failed otherwise is inconclusive
///   ([`ChildObservation::CommitUnconfirmed`]).
async fn observe_child(
    runtime: Option<&meerkat_runtime::MeerkatMachine>,
    handle: &MobHandle,
    child: &AgentIdentity,
) -> ChildObservation {
    let observed = observe_child_run(runtime, handle, child).await;
    if observed != ChildObservation::Settled {
        return observed;
    }
    // Without a runtime nothing can be delivered on this host either.
    let Some(runtime) = runtime else {
        return ChildObservation::Settled;
    };
    let Some(session_id) = handle.resolve_bridge_session_id(child).await else {
        return ChildObservation::Settled;
    };
    commit_observation(
        tokio::time::timeout(
            RUN_INPUT_READ_TIMEOUT,
            runtime.session_has_uncommitted_run_input(&session_id),
        )
        .await
        .map_err(|_elapsed| ()),
    )
}

/// How the re-link reads the runtime's answer about a settled child's last
/// commit (see [`observe_child`]). `Err(())` is a read that timed out.
fn commit_observation(
    read: Result<Result<bool, meerkat_runtime::RuntimeDriverError>, ()>,
) -> ChildObservation {
    use meerkat_runtime::RuntimeDriverError;
    match read {
        Ok(Ok(true)) => ChildObservation::CommitPending(CommitEvidence::InputAwaitsBoundary),
        Ok(Ok(false)) => ChildObservation::Settled,
        Ok(Err(RuntimeDriverError::RecoveryRepairBlocked { .. })) => {
            ChildObservation::CommitPending(CommitEvidence::DurabilityDegraded)
        }
        Ok(Err(
            RuntimeDriverError::NotFound { .. }
            | RuntimeDriverError::Destroyed
            | RuntimeDriverError::NotReady { .. },
        )) => ChildObservation::Settled,
        Ok(Err(error)) => ChildObservation::CommitUnconfirmed(format!(
            "could not read the child's run inputs: {error}"
        )),
        Err(()) => ChildObservation::CommitUnconfirmed(format!(
            "the read of the child's run inputs did not answer within {RUN_INPUT_READ_TIMEOUT:?}"
        )),
    }
}

/// Read a fork child's run state: its member status, and when that leaves
/// the run state unknown, its runtime state.
async fn observe_child_run(
    runtime: Option<&meerkat_runtime::MeerkatMachine>,
    handle: &MobHandle,
    child: &AgentIdentity,
) -> ChildObservation {
    let snapshot = match handle.member_status(child).await {
        Ok(snapshot) => snapshot,
        Err(MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed) => {
            return ChildObservation::Settled;
        }
        Err(error) => return ChildObservation::Unobserved(error.to_string()),
    };
    let progress = snapshot
        .progress
        .as_ref()
        .map(|progress| (progress.run_state, progress.in_flight_work));
    match ProgressVerdict::of(progress) {
        ProgressVerdict::Running => ChildObservation::Running,
        ProgressVerdict::Settled => ChildObservation::Settled,
        ProgressVerdict::Undetermined => {
            let Some(runtime) = runtime else {
                // Nothing to ask; nothing can be delivered on this host
                // either.
                return ChildObservation::Settled;
            };
            let Some(session_id) = handle.resolve_bridge_session_id(child).await else {
                return ChildObservation::Settled;
            };
            use meerkat_runtime::SessionServiceRuntimeExt as _;
            let state = runtime.runtime_state(&session_id).await;
            let active_inputs = runtime
                .list_active_inputs(&session_id)
                .await
                .map(|inputs| inputs.len());
            from_runtime(state, active_inputs)
        }
    }
}

/// The child's runtime state, read directly: a run in progress, or a retired
/// runtime still draining admitted input, is running; a runtime that is idle,
/// attached with no run, stopped, destroyed or not held any more is settled;
/// a failed read or a state that says nothing yet is unobserved.
fn from_runtime(
    state: Result<meerkat_runtime::RuntimeState, meerkat_runtime::RuntimeDriverError>,
    active_inputs: Result<usize, meerkat_runtime::RuntimeDriverError>,
) -> ChildObservation {
    use meerkat_runtime::{RuntimeDriverError, RuntimeState};
    let gone = |error: &RuntimeDriverError| {
        matches!(
            error,
            RuntimeDriverError::NotFound { .. }
                | RuntimeDriverError::Destroyed
                | RuntimeDriverError::NotReady { .. }
        )
    };
    match state {
        Ok(RuntimeState::Running) => ChildObservation::Running,
        Ok(RuntimeState::Retired) => match active_inputs {
            Ok(0) => ChildObservation::Settled,
            Ok(_) => ChildObservation::Running,
            Err(error) if gone(&error) => ChildObservation::Settled,
            Err(error) => ChildObservation::Unobserved(error.to_string()),
        },
        Ok(
            RuntimeState::Idle
            | RuntimeState::Attached
            | RuntimeState::Stopped
            | RuntimeState::Destroyed,
        ) => ChildObservation::Settled,
        Ok(other) => ChildObservation::Unobserved(format!("the child's runtime is {other}")),
        Err(error) if gone(&error) => ChildObservation::Settled,
        Err(error) => ChildObservation::Unobserved(error.to_string()),
    }
}

/// The child's reply to the job, when its durable transcript already holds
/// it after the fork prefix, as a `completed` outcome. The child stays
/// seated.
///
/// This is the read for a record without a turn delivery identity (a host
/// without a runtime, or a record written before the field existed). A
/// record with one is settled from the runtime's receipt for the job turn
/// instead (see [`relink_by_receipt`]), which compacting the child's
/// transcript cannot move.
async fn durable_reply(
    service: &Arc<dyn meerkat_mob::MobSessionService>,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
) -> Option<ForkOffCompletion> {
    let session_id = handle.resolve_bridge_session_id(child).await?;
    let session = service
        .load_persisted_session(&session_id)
        .await
        .ok()
        .flatten()?;
    let result = job.durable_terminal_result(&session).ok().flatten()?;
    let mut completion = ForkOffCompletion::empty(
        child.to_string(),
        member_ref(mob_id, child),
        ForkOffCompletionStatus::Completed,
    );
    completion.bounded_result = Some(result.to_wire());
    Some(completion)
}

/// Re-link a job whose turn was admitted under a stable delivery identity:
/// its outcome is the runtime's terminal receipt for that exact input
/// ([`MobHandle::wait_bounded_work_for_identity_with_delivery_identity`]),
/// whatever the child's member status says in between.
///
/// - A receipt with a result delivers `completed`.
/// - A receipt of the turn's own failure (see [`receipt_failure`]) delivers
///   `failed` with the typed error and retires the child, as the live
///   custodian does.
/// - A receipt of an end the restart caused, an input no run answered, or an
///   input never admitted delivers `restart_interrupted`; the child stays
///   seated.
/// - An input still owed a terminal is watched, not settled: after a restart
///   the runtime requeues it and member status can read idle before the
///   recovered run opens. The watch waits on the receipt in slices, and is
///   bounded by `commit_pending_ceiling` only while the child is not seen
///   running. At the ceiling the receipt is read once more, as a real wait
///   ([`RECEIPT_WAIT_SLICE`]): a receipt found then is delivered. Only a
///   read that is evidence (the input still owed a terminal, never
///   admitted, no session, the member retired) delivers
///   `restart_interrupted`, typed `commit_never_landed` when the input's last
///   phase was taken up by a run (`Staged`, `Applied` or
///   `AppliedPendingConsumption`). A read that says nothing (not observed
///   within its bound, or failed) is not evidence: the watch goes on, and
///   delivers `restart_interrupted` naming no cause only after
///   [`MAX_INCONCLUSIVE_CEILING_READS`] such reads in a row.
/// - The opt-in `max_run` limit is measured from the original start, as for
///   every job; no read or pause in the watch outlasts it.
#[allow(clippy::too_many_arguments)]
async fn relink_by_receipt(
    service: &Arc<dyn meerkat_mob::MobSessionService>,
    delivery: &RelinkDelivery,
    owner: &JobOwner,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
    turn_delivery: &meerkat_mob::store::MobDeliveryIdentity,
    commit_pending_ceiling: Duration,
) -> ForkRelinkAction {
    let spec =
        match meerkat_mob::BoundedResultSpec::new(job.result_label.clone(), job.max_text_bytes) {
            Ok(spec) => spec,
            Err(error) => return ForkRelinkAction::Failed(error.to_string()),
        };
    let runtime = delivery.runtime.as_deref();
    let deadline_ms = job
        .max_run_ms
        .map(|limit| job.started_at_ms.saturating_add(limit));
    let mut stall_watch = CommitWatch::new(commit_pending_ceiling);
    let mut unreadable_backoff = UnobservedBackoff::default();
    loop {
        let remaining_ms = deadline_ms.map(|deadline| deadline.saturating_sub(now_ms()));
        let limit_passed = remaining_ms == Some(0);
        // With the limit passed, one read decides: a receipt that already
        // exists is the job's outcome, anything else loses to the limit.
        let slice = if limit_passed {
            Duration::ZERO
        } else {
            remaining_ms.map_or(RECEIPT_WAIT_SLICE, |remaining_ms| {
                RECEIPT_WAIT_SLICE.min(Duration::from_millis(remaining_ms))
            })
        };
        let read = handle
            .wait_bounded_work_for_identity_with_delivery_identity(
                child,
                turn_delivery,
                &spec,
                meerkat_core::time_compat::Instant::now() + slice,
            )
            .await;
        // The input's last phase while it is still owed a terminal; `None`
        // when the read was inconclusive.
        if read.is_ok() {
            unreadable_backoff.reset();
        }
        let phase = match read {
            Ok(report) => match report.into_parts().1 {
                meerkat_mob::DeliveryTerminalWait::Terminal(record) => {
                    // Past the limit only a completed turn wins: any other
                    // end loses to the limit, as it does for every job.
                    let completed = matches!(
                        record.resolution(),
                        meerkat_mob::DeliveryTerminalResolution::Receipt { result: Ok(_), .. }
                    );
                    if limit_passed && !completed {
                        return limit_elapsed(service, delivery, owner, mob_id, handle, child, job)
                            .await;
                    }
                    return deliver_receipt(
                        service, delivery, owner, mob_id, handle, child, job, *record,
                    )
                    .await;
                }
                // A reading from before a final read that ran out says
                // nothing about the input now, so its phase is not used.
                meerkat_mob::DeliveryTerminalWait::NotTerminal {
                    cause: meerkat_mob::DeliveryNotTerminalCause::EvidenceReadTimedOut,
                    ..
                } => None,
                meerkat_mob::DeliveryTerminalWait::NotTerminal { phase, .. } => Some(phase),
                meerkat_mob::DeliveryTerminalWait::Unknown {
                    cause: meerkat_mob::DeliveryUnknownCause::NotObservedByDeadline,
                } => None,
                // Never admitted (the host went down between seating the
                // child and admitting its turn), no session, or retired
                // without the input: the turn did not survive.
                _ => {
                    if limit_passed {
                        return limit_elapsed(service, delivery, owner, mob_id, handle, child, job)
                            .await;
                    }
                    return deliver(
                        delivery,
                        owner,
                        mob_id,
                        job,
                        restart_interrupted(mob_id, child),
                    )
                    .await;
                }
            },
            Err(error) => {
                tracing::debug!(
                    mob_id = %mob_id,
                    child = %child,
                    error = %error,
                    "fork_off re-link could not read the job turn's receipt"
                );
                if !limit_passed {
                    tokio::time::sleep(within_limit(unreadable_backoff.next_pause(), deadline_ms))
                        .await;
                }
                None
            }
        };
        if limit_passed {
            return limit_elapsed(service, delivery, owner, mob_id, handle, child, job).await;
        }
        // The status read is bounded, and never outlasts the limit: a read
        // that does not answer in time is inconclusive, and the deadline is
        // evaluated again before anything else.
        let status_bound = within_limit(STATUS_READ_TIMEOUT, deadline_ms);
        let status =
            match tokio::time::timeout(status_bound, observe_child_run(runtime, handle, child))
                .await
            {
                Ok(status) => status,
                Err(_elapsed) => ChildObservation::Unobserved(format!(
                    "the child's status read did not answer within {status_bound:?}"
                )),
            };
        if deadline_ms.is_some_and(|deadline| now_ms() >= deadline) {
            continue;
        }
        let CommitWatchStep::Ceiling { reason } = in_flight_step(
            phase,
            &status,
            &mut stall_watch,
            meerkat_core::time_compat::Instant::now(),
        ) else {
            continue;
        };
        // A receipt that landed while the status was read wins over the
        // bound, so the exact receipt is read once more. That read is a real
        // wait on the runtime, never outlasting the limit: a read given only
        // the waiter's evidence floor can end while the receipt's own commit
        // still holds the session driver, and then it says nothing.
        let last_read = handle
            .wait_bounded_work_for_identity_with_delivery_identity(
                child,
                turn_delivery,
                &spec,
                meerkat_core::time_compat::Instant::now()
                    + within_limit(RECEIPT_WAIT_SLICE, deadline_ms),
            )
            .await
            .map(|report| report.into_parts().1);
        // The read can end past the limit (the waiter keeps a 100 ms evidence
        // floor); the limit then decides, where only a completed turn wins.
        if deadline_ms.is_some_and(|deadline| now_ms() >= deadline) {
            continue;
        }
        let reason = match ceiling_receipt(last_read) {
            CeilingReceipt::Terminal(record) => {
                return deliver_receipt(
                    service, delivery, owner, mob_id, handle, child, job, *record,
                )
                .await;
            }
            CeilingReceipt::Absent => reason,
            CeilingReceipt::Inconclusive(detail) => {
                if !stall_watch.inconclusive_ceiling_read() {
                    tracing::debug!(
                        mob_id = %mob_id,
                        child = %child,
                        detail = %detail,
                        "fork_off re-link: the last receipt read at the ceiling said nothing; \
                         watching on"
                    );
                    tokio::time::sleep(within_limit(unreadable_backoff.next_pause(), deadline_ms))
                        .await;
                    continue;
                }
                // No read at the ceiling was evidence, so the outcome names
                // no cause.
                None
            }
        };
        tracing::warn!(
            mob_id = %mob_id,
            child = %child,
            phase = ?phase,
            "fork_off re-link: the job turn's input never reached a terminal \
             while the child sat idle; delivering restart_interrupted"
        );
        let mut completion = restart_interrupted(mob_id, child);
        completion.restart_reason = reason;
        return deliver(delivery, owner, mob_id, job, completion).await;
    }
}

/// What the last read of a job turn's receipt at the commit ceiling says.
#[derive(Debug)]
enum CeilingReceipt {
    /// The receipt exists: it is the job's outcome.
    Terminal(Box<meerkat_mob::DeliveryTerminalRecord>),
    /// Evidence that no receipt exists: the waiter's final read found the
    /// input still owed a terminal when the read ended (a reading it takes
    /// armed on the runtime's own terminal signal), or the runtime holds no
    /// input for it that can arrive (never admitted, the member has no
    /// session, or it is retired).
    Absent,
    /// The read says nothing: its final evidence read ran out (for example
    /// while the receipt's own commit held the session driver) and only an
    /// earlier pending reading remains, it observed neither the member nor
    /// its runtime within its bound, it answered in a way this build does
    /// not know, or it failed.
    Inconclusive(String),
}

/// Classify the last receipt read at the ceiling (see [`CeilingReceipt`]).
fn ceiling_receipt(
    read: Result<meerkat_mob::DeliveryTerminalWait, meerkat_mob::DeliveryTerminalWaitError>,
) -> CeilingReceipt {
    use meerkat_mob::{DeliveryNotTerminalCause, DeliveryTerminalWait, DeliveryUnknownCause};
    match read {
        Ok(DeliveryTerminalWait::Terminal(record)) => CeilingReceipt::Terminal(record),
        Ok(DeliveryTerminalWait::NotTerminal {
            cause: DeliveryNotTerminalCause::EvidenceReadTimedOut,
            ..
        }) => CeilingReceipt::Inconclusive(
            "the final evidence read ran out; the pending reading is from before it".to_string(),
        ),
        Ok(
            DeliveryTerminalWait::NotTerminal { .. }
            | DeliveryTerminalWait::Unknown {
                cause:
                    DeliveryUnknownCause::NotAdmittedByDeadline
                    | DeliveryUnknownCause::MemberHasNoSession
                    | DeliveryUnknownCause::MemberRetired,
            },
        ) => CeilingReceipt::Absent,
        Ok(other) => CeilingReceipt::Inconclusive(format!("{other:?}")),
        Err(error) => CeilingReceipt::Inconclusive(error.to_string()),
    }
}

/// What the receipt watch does with a job input still owed a terminal
/// (`phase` is its last phase, `None` when the receipt read was
/// inconclusive), given the child's status: a running child is making
/// progress and restarts the wait; a status read that did not observe the
/// child changes nothing; a child that is not running counts toward the
/// ceiling. It never settles the job before the ceiling, whatever the
/// input's phase: a requeued input (`Queued`) behind an idle child is the
/// normal state between a restart and the recovered run opening. At the
/// ceiling the reason is `commit_never_landed` only when a run had taken the
/// input up (`Staged`, `Applied` or `AppliedPendingConsumption`).
fn in_flight_step(
    phase: Option<meerkat_runtime::InputLifecycleState>,
    status: &ChildObservation,
    watch: &mut CommitWatch,
    now: meerkat_core::time_compat::Instant,
) -> CommitWatchStep {
    match status {
        ChildObservation::Running => {
            watch.progressed();
            CommitWatchStep::KeepWatching
        }
        ChildObservation::Unobserved(_) => CommitWatchStep::KeepWatching,
        _ => {
            let evidence = matches!(
                phase,
                Some(
                    meerkat_runtime::InputLifecycleState::Staged
                        | meerkat_runtime::InputLifecycleState::Applied
                        | meerkat_runtime::InputLifecycleState::AppliedPendingConsumption
                )
            )
            .then_some(CommitEvidence::InputAwaitsBoundary);
            watch.unconfirmed(now, evidence)
        }
    }
}

/// Deliver the outcome a job turn's terminal receipt records.
#[allow(clippy::too_many_arguments)]
async fn deliver_receipt(
    service: &Arc<dyn meerkat_mob::MobSessionService>,
    delivery: &RelinkDelivery,
    owner: &JobOwner,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
    record: meerkat_mob::DeliveryTerminalRecord,
) -> ForkRelinkAction {
    let terminal = record.terminal().clone();
    let failure = match record.into_resolution() {
        meerkat_mob::DeliveryTerminalResolution::Receipt {
            result: Ok(turn), ..
        } => {
            let mut completion = ForkOffCompletion::empty(
                child.to_string(),
                member_ref(mob_id, child),
                ForkOffCompletionStatus::Completed,
            );
            completion.record_completed_turn(&turn);
            return deliver(delivery, owner, mob_id, job, completion).await;
        }
        meerkat_mob::DeliveryTerminalResolution::Receipt {
            result: Err(failure),
            ..
        } => failure,
        // No run answered the input (superseded, coalesced, consumed on
        // accept, cancelled when the restarted host revived the child, or
        // abandoned before a run began), or a resolution this build does not
        // know: the turn did not survive.
        _ => {
            return deliver(
                delivery,
                owner,
                mob_id,
                job,
                restart_interrupted(mob_id, child),
            )
            .await;
        }
    };
    if receipt_failure(&failure, &terminal) == ReceiptFailure::RestartCaused {
        return deliver(
            delivery,
            owner,
            mob_id,
            job,
            restart_interrupted(mob_id, child),
        )
        .await;
    }
    let mut completion = ForkOffCompletion::empty(
        child.to_string(),
        member_ref(mob_id, child),
        ForkOffCompletionStatus::Failed,
    );
    completion.error = Some(failure.to_string());
    let action = deliver(delivery, owner, mob_id, job, completion).await;
    // The live custodian retires a child whose own turn failed; so does the
    // re-link, once that outcome is settled.
    if retires_after_delivery(&action, service, mob_id, child, job).await
        && let Err(error) = handle.retire_with_descendants(child.clone()).await
    {
        tracing::warn!(
            mob_id = %mob_id,
            child = %child,
            error = %error,
            "fork_off re-link delivered a failed job but could not retire its child"
        );
    }
    action
}

/// Whose end a job turn's failed receipt records.
#[derive(Debug, PartialEq, Eq)]
enum ReceiptFailure {
    /// The turn itself failed: the live custodian reports it `failed` and
    /// retires the child.
    TurnFailed,
    /// The turn was ended from outside (the runtime stopped, destroyed,
    /// retired or reset it, or cancelled it), which is what a restart does:
    /// it stays `restart_interrupted`, and the child stays seated.
    RestartCaused,
}

/// Classify a failed receipt by its typed variant and the input's typed
/// terminal. An integrity failure of the receipt itself (a missing or
/// mismatched attribution, closed or broken completion plumbing) says
/// nothing about how the turn ended and is treated as restart-caused, which
/// keeps the child seated.
fn receipt_failure(
    failure: &meerkat_mob::BoundedTurnFailure,
    terminal: &meerkat_runtime::InputTerminalOutcome,
) -> ReceiptFailure {
    use meerkat_mob::BoundedTurnFailure;
    use meerkat_runtime::{InputAbandonReason, InputTerminalOutcome};
    match failure {
        BoundedTurnFailure::AbandonedWithError { .. }
        | BoundedTurnFailure::ExtractionFailed { .. }
        | BoundedTurnFailure::CompletedWithFinalizationFailure { .. }
        | BoundedTurnFailure::CompletedWithoutResult { .. }
        | BoundedTurnFailure::CallbackPending { .. }
        | BoundedTurnFailure::CallbackBatchPending { .. }
        | BoundedTurnFailure::DirectSessionFailure { .. } => ReceiptFailure::TurnFailed,
        // Abandoned without an error of its own: its typed reason says who
        // ended it. Running out of stage attempts is the turn's own failure;
        // every other reason is an end imposed from outside.
        BoundedTurnFailure::Abandoned { .. } => match terminal {
            InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::MaxAttemptsExhausted { .. },
            } => ReceiptFailure::TurnFailed,
            _ => ReceiptFailure::RestartCaused,
        },
        _ => ReceiptFailure::RestartCaused,
    }
}

fn restart_interrupted(mob_id: &MobId, child: &AgentIdentity) -> ForkOffCompletion {
    ForkOffCompletion::empty(
        child.to_string(),
        member_ref(mob_id, child),
        ForkOffCompletionStatus::RestartInterrupted,
    )
}

/// The opt-in limit won: cancel the child's run and deliver
/// `max_run_elapsed`, then retire the child with its descendants once that
/// outcome is settled. A delivery that must wait (or failed) keeps the child,
/// cancelled, and its job record, so the next pass delivers it. A delivery
/// deduplicated against a record admitted earlier retires the child only when
/// that record's outcome retires it (see [`retires_after_delivery`]).
async fn limit_elapsed(
    service: &Arc<dyn meerkat_mob::MobSessionService>,
    delivery: &RelinkDelivery,
    owner: &JobOwner,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
) -> ForkRelinkAction {
    let _ = handle.force_cancel_member(child.clone()).await;
    let mut completion = ForkOffCompletion::empty(
        child.to_string(),
        member_ref(mob_id, child),
        ForkOffCompletionStatus::MaxRunElapsed,
    );
    completion.max_run_secs = job.max_run_ms.map(|limit| limit / 1000);
    let action = deliver(delivery, owner, mob_id, job, completion).await;
    if retires_after_delivery(&action, service, mob_id, child, job).await
        && let Err(error) = handle.retire_with_descendants(child.clone()).await
    {
        tracing::warn!(
            mob_id = %mob_id,
            child = %child,
            error = %error,
            "fork_off re-link delivered max_run_elapsed but could not retire the child; \
             it stays seated, cancelled, for its forker"
        );
    }
    action
}

/// Whether the child is retired after delivering it an outcome that retires
/// it (a failed turn, or `max_run_elapsed`), given what the delivery did
/// (`action`). An outcome delivered now, or one whose owner is gone, is
/// settled and retires the child. A delivery deduplicated against a record
/// admitted earlier delivered nothing now: that record may carry another
/// outcome (`restart_interrupted` or `completed`), so the child is retired
/// only when the record's committed outcome retires it, read typed as the
/// re-link's entry reads it ([`committed_completion`]). Nothing else is
/// settled.
async fn retires_after_delivery(
    action: &ForkRelinkAction,
    service: &Arc<dyn meerkat_mob::MobSessionService>,
    mob_id: &MobId,
    child: &AgentIdentity,
    job: &ForkJobRecord,
) -> bool {
    match action {
        ForkRelinkAction::Delivered | ForkRelinkAction::OwnerGone => true,
        ForkRelinkAction::AlreadyDelivered => committed_completion(service, mob_id, child, job)
            .await
            .is_some_and(|committed| committed.retires_child()),
        ForkRelinkAction::AwaitingOwner { .. } | ForkRelinkAction::Failed(_) => false,
    }
}

/// The outcome of an idle child: its durable reply, or
/// `restart_interrupted` when the turn left none.
async fn settled_outcome(
    service: &Arc<dyn meerkat_mob::MobSessionService>,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
) -> ForkOffCompletion {
    match durable_reply(service, mob_id, handle, child, job).await {
        Some(completion) => completion,
        None => ForkOffCompletion::empty(
            child.to_string(),
            member_ref(mob_id, child),
            ForkOffCompletionStatus::RestartInterrupted,
        ),
    }
}

fn member_ref(mob_id: &MobId, child: &AgentIdentity) -> meerkat_contracts::WireMemberRef {
    meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), child.as_str())
}

/// Job `job_id`'s completion record, once committed in the owner's durable
/// transcript: its typed notice status and the outcome it delivered.
struct CommittedCompletion {
    status: meerkat_core::event::BackgroundJobTerminalStatus,
    /// The delivered outcome's typed `status`, read from the record's detail
    /// (the completion's own serialization). `None` when the detail does not
    /// carry one.
    outcome: Option<ForkOffCompletionStatus>,
}

impl CommittedCompletion {
    /// Whether the delivered outcome retires the child. A record whose detail
    /// carries no typed outcome falls back to its notice status, where only
    /// `terminated` (a limit autokill) is unambiguous.
    fn retires_child(&self) -> bool {
        match &self.outcome {
            Some(outcome) => outcome.retires_child(),
            None => self.status == meerkat_core::event::BackgroundJobTerminalStatus::Terminated,
        }
    }
}

/// The delivered outcome carried in a completion record's detail.
#[derive(serde::Deserialize)]
struct CommittedOutcome {
    status: ForkOffCompletionStatus,
}

/// Whose outcome a completion record's detail carries: the child fields of
/// the completion's own serialization ([`ForkOffCompletion`]), decoded apart
/// from its outcome so that an outcome this build cannot read does not hide
/// whose it is.
#[derive(serde::Deserialize)]
struct CommittedChild {
    agent_identity: Option<AgentIdentity>,
    member_ref: Option<meerkat_contracts::WireMemberRef>,
}

impl CommittedChild {
    /// Whether the record names a child other than `child` of mob `mob_id`:
    /// by its identity, or by its member ref (mob and identity). A field the
    /// record lacks, or a member ref that does not decode, names no one.
    fn names_another_child(&self, mob_id: &MobId, child: &AgentIdentity) -> bool {
        let other_identity = self
            .agent_identity
            .as_ref()
            .is_some_and(|recorded| recorded != child);
        let other_member = self
            .member_ref
            .as_ref()
            .and_then(|member_ref| member_ref.decode().ok())
            .is_some_and(|(recorded_mob, recorded_identity)| {
                recorded_mob != mob_id.as_str() || recorded_identity != child.as_str()
            });
        other_identity || other_member
    }
}

/// `child`'s completion record for its job in the owner's durable
/// transcript, once the record is committed there.
///
/// A job id is not unique across children (a host binds it), and records are
/// keyed by owner session and job id, so a record whose detail names another
/// child (see [`CommittedChild`]) is not this child's. A record whose detail
/// names no child (it does not decode) is matched by its job id alone.
async fn committed_completion(
    service: &Arc<dyn meerkat_mob::MobSessionService>,
    mob_id: &MobId,
    child: &AgentIdentity,
    job: &ForkJobRecord,
) -> Option<CommittedCompletion> {
    let session = service
        .load_persisted_session(&job.owner_session_id)
        .await
        .ok()
        .flatten()?;
    session.messages().iter().find_map(|message| {
        let meerkat_core::Message::SystemNotice(notice) = message else {
            return None;
        };
        notice.blocks.iter().find_map(|block| match block {
            meerkat_core::types::SystemNoticeBlock::BackgroundJob {
                job_id: recorded,
                display_name: Some(tool),
                status,
                detail,
                persisted: true,
                ..
            } if recorded == &job.job_id && tool == TOOL_FORK_OFF => {
                let detail = detail.as_deref();
                let names_another_child = detail
                    .and_then(|detail| serde_json::from_str::<CommittedChild>(detail).ok())
                    .is_some_and(|recorded| recorded.names_another_child(mob_id, child));
                if names_another_child {
                    return None;
                }
                Some(CommittedCompletion {
                    status: *status,
                    outcome: detail
                        .and_then(|detail| serde_json::from_str::<CommittedOutcome>(detail).ok())
                        .map(|committed| committed.status),
                })
            }
            _ => None,
        })
    })
}

async fn deliver(
    delivery: &RelinkDelivery,
    owner: &JobOwner,
    child_mob: &MobId,
    job: &ForkJobRecord,
    completion: ForkOffCompletion,
) -> ForkRelinkAction {
    if let JobOwner::Gone = owner {
        return ForkRelinkAction::OwnerGone;
    }
    let Some(runtime) = delivery.runtime.as_deref() else {
        return ForkRelinkAction::Failed(
            "no runtime to admit the completion on this host".to_string(),
        );
    };
    let status = completion.status.terminal_status();
    let value = match serde_json::to_value(&completion) {
        Ok(value) => value,
        Err(error) => return ForkRelinkAction::Failed(error.to_string()),
    };
    let member = match owner {
        JobOwner::Member(owner_mob, owner_identity) => {
            Some((owner_mob.clone(), owner_identity.clone()))
        }
        JobOwner::Elsewhere => {
            delivery
                .member_elsewhere(child_mob, &job.owner_session_id)
                .await
        }
        JobOwner::Gone => None,
    };
    let delivered = match member {
        Some((owner_mob, owner_identity)) => {
            crate::detached_delivery::deliver_detached_completion_to_member(
                runtime,
                &owner_mob,
                &owner_identity,
                &job.owner_session_id,
                TOOL_FORK_OFF,
                &job.job_id,
                status,
                value,
            )
            .await
        }
        None => {
            crate::detached_delivery::deliver_detached_completion_to_session(
                runtime,
                delivery.owner_host.as_deref(),
                &job.owner_session_id,
                TOOL_FORK_OFF,
                &job.job_id,
                status,
                value,
            )
            .await
        }
    };
    match delivered {
        Ok(crate::detached_delivery::DetachedCompletionDelivered::Delivered) => {
            ForkRelinkAction::Delivered
        }
        Ok(_) => ForkRelinkAction::AlreadyDelivered,
        Err(crate::detached_delivery::DetachedCompletionError::OwnerGone { .. }) => {
            ForkRelinkAction::OwnerGone
        }
        Err(crate::detached_delivery::DetachedCompletionError::OwnerRevivalDeferred {
            mob_id,
            reason,
            ..
        }) => ForkRelinkAction::AwaitingOwner { mob_id, reason },
        Err(error) => ForkRelinkAction::Failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CeilingReceipt, ChildObservation, CommitEvidence, CommitWatch, CommitWatchStep,
        CommittedChild, CommittedCompletion, ForkRelinkAction, ForkRelinkReport,
        MAX_INCONCLUSIVE_CEILING_READS, MAX_OWNER_REVIVAL_WAITS, ProgressVerdict, ReceiptFailure,
        UNOBSERVED_RETRY_INITIAL_INTERVAL, UNOBSERVED_RETRY_MAX_INTERVAL, UnobservedBackoff,
        ceiling_receipt, commit_observation, from_runtime, in_flight_step, now_ms, receipt_failure,
        redeliver_each, within_limit,
    };
    use crate::agent_tools::RestartInterruptedReason;
    use crate::detached_delivery::OwnerRevivalDeferral;
    use meerkat_mob::{AgentIdentity, MemberRunState, MobId, MobState};
    use meerkat_runtime::{RuntimeDriverError, RuntimeState};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    /// Each deferred job waits on its own owner's mob with its own budget,
    /// and a wake retries that job alone. Job X's owner mob B stays stopped
    /// while job Y's owner mob C keeps waking with its owner still deferred
    /// (an operation in progress): Y spends its own budget, X is not retried
    /// and keeps waiting, and when B alone resumes X is delivered once
    /// (lifecycle review: C's wakes spent a budget shared with X and dropped
    /// the wait on B).
    #[tokio::test]
    async fn each_deferred_job_waits_on_its_owner_mob_with_its_own_budget() {
        let (mob_b, mob_c) = (MobId::from("owner-mob-b"), MobId::from("owner-mob-c"));
        let report = |job: &str, action: ForkRelinkAction| ForkRelinkReport {
            mob_id: MobId::from("child-mob-a"),
            child: AgentIdentity::from(job),
            job_id: job.to_string(),
            action,
        };
        let x_waits = || ForkRelinkAction::AwaitingOwner {
            mob_id: mob_b.clone(),
            reason: OwnerRevivalDeferral::MobNotRunning {
                phase: MobState::Stopped,
            },
        };
        let y_waits = || ForkRelinkAction::AwaitingOwner {
            mob_id: mob_c.clone(),
            reason: OwnerRevivalDeferral::LifecycleOperationPending {
                intent: "explicit_resume member owner-c".to_string(),
            },
        };
        let (b_runs, b_runs_rx) = tokio::sync::watch::channel(false);
        let (x_retries, y_retries) = (AtomicU32::new(0), AtomicU32::new(0));

        let driver = redeliver_each(
            vec![report("job-x", x_waits()), report("job-y", y_waits())],
            |owner_mob, _reason, _attempt| {
                let mut b_runs = b_runs_rx.clone();
                let on_b = owner_mob == mob_b;
                async move {
                    if on_b {
                        b_runs.wait_for(|runs| *runs).await.is_ok()
                    } else {
                        tokio::task::yield_now().await;
                        true
                    }
                }
            },
            |_child, job_id| {
                let b_running = *b_runs_rx.borrow();
                let (x_retries, y_retries) = (&x_retries, &y_retries);
                let (x_waits, y_waits) = (&x_waits, &y_waits);
                async move {
                    if job_id == "job-x" {
                        x_retries.fetch_add(1, Ordering::SeqCst);
                        Some(report(
                            "job-x",
                            if b_running {
                                ForkRelinkAction::Delivered
                            } else {
                                x_waits()
                            },
                        ))
                    } else {
                        y_retries.fetch_add(1, Ordering::SeqCst);
                        Some(report("job-y", y_waits()))
                    }
                }
            },
        );
        let control = async {
            while y_retries.load(Ordering::SeqCst) < MAX_OWNER_REVIVAL_WAITS {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                x_retries.load(Ordering::SeqCst),
                0,
                "X is not retried while only C wakes"
            );
            let _ = b_runs.send(true);
        };
        let (finished, ()) = tokio::join!(driver, control);

        let action = |job: &str| {
            finished
                .iter()
                .find(|report| report.job_id == job)
                .map(|report| report.action.clone())
        };
        assert_eq!(action("job-x"), Some(ForkRelinkAction::Delivered));
        assert_eq!(action("job-y"), Some(y_waits()), "Y spent its own budget");
        assert_eq!(x_retries.load(Ordering::SeqCst), 1, "X delivered once");
        assert_eq!(y_retries.load(Ordering::SeqCst), MAX_OWNER_REVIVAL_WAITS);
    }

    /// Unobserved reads back off: the pause doubles from the initial interval
    /// up to the cap, and an observed read restarts it. A read that did not
    /// observe the child already waited out the status lane's admission
    /// bound, so a fixed short retry only loads a saturated mob.
    #[test]
    fn unobserved_reads_back_off_and_an_observed_read_resets_the_pause() {
        let mut backoff = UnobservedBackoff::default();
        let pauses: Vec<Duration> = (0..8).map(|_| backoff.next_pause()).collect();
        assert_eq!(pauses[0], UNOBSERVED_RETRY_INITIAL_INTERVAL);
        assert_eq!(pauses[1], UNOBSERVED_RETRY_INITIAL_INTERVAL * 2);
        assert_eq!(pauses[2], UNOBSERVED_RETRY_INITIAL_INTERVAL * 4);
        assert!(
            pauses.windows(2).all(|pair| pair[0] <= pair[1]),
            "the pause never shrinks while reads stay unobserved: {pauses:?}"
        );
        assert_eq!(pauses[7], UNOBSERVED_RETRY_MAX_INTERVAL);
        assert!(
            pauses
                .iter()
                .all(|pause| *pause <= UNOBSERVED_RETRY_MAX_INTERVAL)
        );
        for _ in 0..64 {
            assert_eq!(backoff.next_pause(), UNOBSERVED_RETRY_MAX_INTERVAL);
        }
        backoff.reset();
        assert_eq!(backoff.next_pause(), UNOBSERVED_RETRY_INITIAL_INTERVAL);
    }

    /// An unknown run state (the member was busy and the projection's
    /// bounded runtime read did not answer) is never taken for a settled
    /// child; the runtime is asked (lifecycle review: it was read as idle).
    #[test]
    fn an_unknown_run_state_is_not_settled() {
        assert_eq!(
            ProgressVerdict::of(Some((MemberRunState::Unknown, 0))),
            ProgressVerdict::Undetermined
        );
        assert_eq!(
            ProgressVerdict::of(Some((MemberRunState::Idle, 0))),
            ProgressVerdict::Settled
        );
        assert_eq!(
            ProgressVerdict::of(Some((MemberRunState::RunOpen, 0))),
            ProgressVerdict::Running
        );
        assert_eq!(
            ProgressVerdict::of(Some((MemberRunState::Unknown, 1))),
            ProgressVerdict::Running
        );
        assert_eq!(ProgressVerdict::of(None), ProgressVerdict::Settled);
    }

    /// The direct runtime read: a run in progress, or a retired runtime still
    /// draining admitted input, is running; no run, or a runtime that no
    /// longer holds the child (the turn did not survive a restart), is
    /// settled; a failed read or an initializing runtime is unobserved.
    #[test]
    fn the_runtime_state_settles_an_unknown_run_state() {
        let gone = || RuntimeDriverError::Destroyed;
        assert_eq!(
            from_runtime(Ok(RuntimeState::Running), Ok(0)),
            ChildObservation::Running
        );
        assert_eq!(
            from_runtime(Ok(RuntimeState::Retired), Ok(1)),
            ChildObservation::Running
        );
        assert_eq!(
            from_runtime(Ok(RuntimeState::Retired), Ok(0)),
            ChildObservation::Settled
        );
        for settled in [
            RuntimeState::Idle,
            RuntimeState::Attached,
            RuntimeState::Stopped,
            RuntimeState::Destroyed,
        ] {
            assert_eq!(from_runtime(Ok(settled), Ok(0)), ChildObservation::Settled);
        }
        assert_eq!(
            from_runtime(Err(gone()), Err(gone())),
            ChildObservation::Settled
        );
        assert!(matches!(
            from_runtime(Ok(RuntimeState::Initializing), Ok(0)),
            ChildObservation::Unobserved(_)
        ));
        assert!(matches!(
            from_runtime(
                Err(RuntimeDriverError::Internal(
                    "store unavailable".to_string()
                )),
                Ok(0)
            ),
            ChildObservation::Unobserved(_)
        ));
        assert!(matches!(
            from_runtime(
                Ok(RuntimeState::Retired),
                Err(RuntimeDriverError::Internal(
                    "store unavailable".to_string()
                ))
            ),
            ChildObservation::Unobserved(_)
        ));
    }

    /// The runtime's answer about a settled child's commit: a run input
    /// awaiting its boundary and degraded durability are machine evidence;
    /// a gone runtime settles the child for this workflow; a timeout or any
    /// other error is inconclusive, never settled.
    #[test]
    fn commit_observation_maps_the_owner_read() {
        assert_eq!(
            commit_observation(Ok(Ok(true))),
            ChildObservation::CommitPending(CommitEvidence::InputAwaitsBoundary)
        );
        assert_eq!(commit_observation(Ok(Ok(false))), ChildObservation::Settled);
        assert_eq!(
            commit_observation(Ok(Err(RuntimeDriverError::RecoveryRepairBlocked {
                evidence_digest: None,
                reason: "reload required".to_string(),
            }))),
            ChildObservation::CommitPending(CommitEvidence::DurabilityDegraded)
        );
        for gone in [
            RuntimeDriverError::Destroyed,
            RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            },
        ] {
            assert_eq!(commit_observation(Ok(Err(gone))), ChildObservation::Settled);
        }
        for inconclusive in [
            Err(()),
            Ok(Err(RuntimeDriverError::StaleAuthority {
                reason: "driver replaced".to_string(),
            })),
            Ok(Err(RuntimeDriverError::Internal(
                "generated input lifecycle phase missing".to_string(),
            ))),
        ] {
            assert!(matches!(
                commit_observation(inconclusive),
                ChildObservation::CommitUnconfirmed(_)
            ));
        }
    }

    /// The wait starts at the first unconfirmed reading and only a running
    /// reading restarts it. At the ceiling, `commit_never_landed` is set only
    /// when that reading is machine evidence.
    #[test]
    fn commit_watch_names_commit_never_landed_only_on_machine_evidence() {
        let ceiling = Duration::from_secs(300);
        let start = meerkat_core::time_compat::Instant::now();
        let at = |secs| start + Duration::from_secs(secs);

        let mut watch = CommitWatch::new(ceiling);
        assert_eq!(
            watch.unconfirmed(at(0), Some(CommitEvidence::InputAwaitsBoundary)),
            CommitWatchStep::KeepWatching
        );
        // Inconclusive readings (a timeout) in between do not restart it.
        assert_eq!(
            watch.unconfirmed(at(200), None),
            CommitWatchStep::KeepWatching
        );
        assert_eq!(
            watch.unconfirmed(at(300), Some(CommitEvidence::DurabilityDegraded)),
            CommitWatchStep::Ceiling {
                reason: Some(RestartInterruptedReason::CommitNeverLanded)
            }
        );

        // Timeouts alone reach the ceiling without the typed reason.
        let mut watch = CommitWatch::new(ceiling);
        assert_eq!(
            watch.unconfirmed(at(0), None),
            CommitWatchStep::KeepWatching
        );
        assert_eq!(
            watch.unconfirmed(at(301), None),
            CommitWatchStep::Ceiling { reason: None }
        );

        // A running reading restarts the wait.
        let mut watch = CommitWatch::new(ceiling);
        assert_eq!(
            watch.unconfirmed(at(0), Some(CommitEvidence::InputAwaitsBoundary)),
            CommitWatchStep::KeepWatching
        );
        watch.progressed();
        assert_eq!(
            watch.unconfirmed(at(301), Some(CommitEvidence::InputAwaitsBoundary)),
            CommitWatchStep::KeepWatching
        );
        assert_eq!(
            watch.unconfirmed(at(601), Some(CommitEvidence::InputAwaitsBoundary)),
            CommitWatchStep::Ceiling {
                reason: Some(RestartInterruptedReason::CommitNeverLanded)
            }
        );
    }

    /// A job input still owed its receipt never settles the job before the
    /// ceiling. A requeued input behind an idle child (the state between a
    /// restart and the recovered run opening) keeps the watch going; a
    /// running child restarts it; at the ceiling only an input a run had
    /// taken up names `commit_never_landed`.
    #[test]
    fn an_in_flight_job_input_is_watched_not_settled() {
        use meerkat_runtime::InputLifecycleState;
        let ceiling = Duration::from_secs(300);
        let start = meerkat_core::time_compat::Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        let idle = ChildObservation::Settled;

        let mut watch = CommitWatch::new(ceiling);
        for secs in [0, 100, 299] {
            assert_eq!(
                in_flight_step(
                    Some(InputLifecycleState::Queued),
                    &idle,
                    &mut watch,
                    at(secs)
                ),
                CommitWatchStep::KeepWatching,
                "a requeued input behind an idle child is watched"
            );
        }
        assert_eq!(
            in_flight_step(
                Some(InputLifecycleState::Queued),
                &ChildObservation::Unobserved("lane busy".to_string()),
                &mut watch,
                at(299)
            ),
            CommitWatchStep::KeepWatching
        );
        assert_eq!(
            in_flight_step(None, &idle, &mut watch, at(300)),
            CommitWatchStep::Ceiling { reason: None },
            "an inconclusive read at the ceiling names no cause"
        );

        let mut watch = CommitWatch::new(ceiling);
        assert_eq!(
            in_flight_step(Some(InputLifecycleState::Queued), &idle, &mut watch, at(0)),
            CommitWatchStep::KeepWatching
        );
        assert_eq!(
            in_flight_step(
                Some(InputLifecycleState::Staged),
                &ChildObservation::Running,
                &mut watch,
                at(250)
            ),
            CommitWatchStep::KeepWatching,
            "the recovered run opened"
        );
        assert_eq!(
            in_flight_step(
                Some(InputLifecycleState::AppliedPendingConsumption),
                &idle,
                &mut watch,
                at(301)
            ),
            CommitWatchStep::KeepWatching,
            "the running reading restarted the wait"
        );
        assert_eq!(
            in_flight_step(
                Some(InputLifecycleState::AppliedPendingConsumption),
                &idle,
                &mut watch,
                at(601)
            ),
            CommitWatchStep::Ceiling {
                reason: Some(RestartInterruptedReason::CommitNeverLanded)
            }
        );
    }

    /// A failed job-turn receipt is classified by its typed variant and the
    /// input's typed terminal: the turn's own failures are `failed`, ends
    /// imposed from outside (what a restart does) stay `restart_interrupted`.
    #[test]
    fn a_failed_receipt_is_classified_by_its_typed_variant() {
        use meerkat_mob::BoundedTurnFailure;
        use meerkat_runtime::{InputAbandonReason, InputTerminalOutcome};
        let session_id = meerkat_core::SessionId::new();
        let error = || Box::new(meerkat_core::TurnErrorMetadata::runtime_apply_failure("x"));
        let abandoned = |reason| InputTerminalOutcome::Abandoned { reason };
        let consumed = InputTerminalOutcome::Consumed;

        for failure in [
            BoundedTurnFailure::AbandonedWithError {
                session_id: session_id.clone(),
                reason: "provider failed after retries".to_string(),
                error: error(),
            },
            BoundedTurnFailure::CompletedWithoutResult {
                session_id: session_id.clone(),
            },
            BoundedTurnFailure::CompletedWithFinalizationFailure {
                session_id: session_id.clone(),
                error: error(),
            },
        ] {
            assert_eq!(
                receipt_failure(&failure, &consumed),
                ReceiptFailure::TurnFailed,
                "{failure:?}"
            );
        }
        let abandoned_turn = || BoundedTurnFailure::Abandoned {
            session_id: session_id.clone(),
            reason: "abandoned".to_string(),
            error: error(),
        };
        assert_eq!(
            receipt_failure(
                &abandoned_turn(),
                &abandoned(InputAbandonReason::MaxAttemptsExhausted { attempts: 3 })
            ),
            ReceiptFailure::TurnFailed,
            "running out of stage attempts is the turn's own failure"
        );

        for reason in [
            InputAbandonReason::Stopped,
            InputAbandonReason::Destroyed,
            InputAbandonReason::Retired,
            InputAbandonReason::Reset,
            InputAbandonReason::Cancelled,
        ] {
            assert_eq!(
                receipt_failure(&abandoned_turn(), &abandoned(reason.clone())),
                ReceiptFailure::RestartCaused,
                "{reason:?}"
            );
        }
        for failure in [
            BoundedTurnFailure::RuntimeTerminated {
                session_id: session_id.clone(),
                reason: "runtime stopped".to_string(),
                error: error(),
            },
            BoundedTurnFailure::Cancelled {
                session_id: session_id.clone(),
            },
            BoundedTurnFailure::CompletionAuthorityClosed {
                admitted_session_id: session_id.clone(),
            },
        ] {
            assert_eq!(
                receipt_failure(&failure, &consumed),
                ReceiptFailure::RestartCaused,
                "{failure:?}"
            );
        }
    }

    /// The last receipt read at the ceiling: a receipt is delivered; a
    /// pending input or a typed not-admitted, no-session or retired cause is
    /// evidence; a read that did not observe the member or its runtime within
    /// its bound, or a failed read, says nothing (review: at the evidence
    /// floor a read held by the receipt's own commit counted as absence).
    #[test]
    fn only_evidence_at_the_ceiling_lets_restart_interrupted_through() {
        use meerkat_mob::{
            DeliveryNotTerminalCause, DeliveryTerminalWait, DeliveryTerminalWaitError,
            DeliveryUnknownCause,
        };
        for cause in [
            DeliveryNotTerminalCause::DeadlineElapsed,
            DeliveryNotTerminalCause::RuntimeDetached,
        ] {
            assert!(matches!(
                ceiling_receipt(Ok(DeliveryTerminalWait::NotTerminal {
                    input_id: meerkat_core::lifecycle::InputId::new(),
                    phase: meerkat_runtime::InputLifecycleState::Applied,
                    terminal: None,
                    last_run_id: None,
                    attempt_count: 1,
                    cause,
                })),
                CeilingReceipt::Absent
            ));
        }
        for cause in [
            DeliveryUnknownCause::NotAdmittedByDeadline,
            DeliveryUnknownCause::MemberHasNoSession,
            DeliveryUnknownCause::MemberRetired,
        ] {
            assert!(
                matches!(
                    ceiling_receipt(Ok(DeliveryTerminalWait::Unknown { cause })),
                    CeilingReceipt::Absent
                ),
                "{cause:?}"
            );
        }
        assert!(matches!(
            ceiling_receipt(Ok(DeliveryTerminalWait::Unknown {
                cause: DeliveryUnknownCause::NotObservedByDeadline,
            })),
            CeilingReceipt::Inconclusive(_)
        ));
        assert!(matches!(
            ceiling_receipt(Err(DeliveryTerminalWaitError::RuntimeAdapterUnavailable)),
            CeilingReceipt::Inconclusive(_)
        ));
        // A pending reading from before a final read that ran out is not
        // evidence (review: a commit taking the driver mid-read left a stale
        // pending reading that counted as absence).
        assert!(matches!(
            ceiling_receipt(Ok(DeliveryTerminalWait::NotTerminal {
                input_id: meerkat_core::lifecycle::InputId::new(),
                phase: meerkat_runtime::InputLifecycleState::Applied,
                terminal: None,
                last_run_id: None,
                attempt_count: 1,
                cause: DeliveryNotTerminalCause::EvidenceReadTimedOut,
            })),
            CeilingReceipt::Inconclusive(_)
        ));
    }

    /// Inconclusive last reads at the ceiling are bounded: the watch stops on
    /// the last of `MAX_INCONCLUSIVE_CEILING_READS` in a row, and a reading of
    /// the child running starts the count again.
    #[test]
    fn inconclusive_ceiling_reads_are_bounded_and_progress_restarts_them() {
        let mut watch = CommitWatch::new(Duration::ZERO);
        for _ in 1..MAX_INCONCLUSIVE_CEILING_READS {
            assert!(!watch.inconclusive_ceiling_read());
        }
        watch.progressed();
        for _ in 1..MAX_INCONCLUSIVE_CEILING_READS {
            assert!(!watch.inconclusive_ceiling_read());
        }
        assert!(watch.inconclusive_ceiling_read(), "the extra wait is spent");
    }

    /// Pauses and reads are cut to the time left before a job's `max_run`
    /// deadline; a job without one keeps the full bound.
    #[test]
    fn a_pause_never_outlasts_the_limit() {
        let bound = Duration::from_secs(5);
        assert_eq!(within_limit(bound, None), bound);
        assert_eq!(within_limit(bound, Some(0)), Duration::ZERO);
        assert!(within_limit(bound, Some(now_ms() + 1_000)) <= Duration::from_secs(1));
        assert_eq!(within_limit(bound, Some(now_ms() + 60_000)), bound);
    }

    /// A completion record is another child's when its detail names another
    /// identity, or a member ref of another mob or identity; a detail that
    /// names no child (a field missing, or a member ref that does not decode)
    /// excludes no one (review: a job id is not unique across children).
    #[test]
    fn a_committed_record_is_matched_to_its_child_by_typed_fields() {
        let (mob, child) = (MobId::from("mob-a"), AgentIdentity::from("child-a"));
        // `None` when the child fields do not decode.
        let names_another = |detail: &serde_json::Value| {
            serde_json::from_value::<CommittedChild>(detail.clone())
                .ok()
                .map(|record| record.names_another_child(&mob, &child))
        };
        let member_ref =
            |mob: &str, identity: &str| meerkat_contracts::WireMemberRef::encode(mob, identity);
        for own in [
            serde_json::json!({
                "agent_identity": "child-a",
                "member_ref": member_ref("mob-a", "child-a"),
            }),
            serde_json::json!({ "agent_identity": "child-a" }),
            serde_json::json!({ "member_ref": member_ref("mob-a", "child-a") }),
            serde_json::json!({
                "agent_identity": "child-a",
                "member_ref": "not-a-member-ref",
            }),
            serde_json::json!({ "status": "failed" }),
        ] {
            assert_eq!(names_another(&own), Some(false), "{own}");
        }
        for other in [
            serde_json::json!({ "agent_identity": "child-b" }),
            serde_json::json!({
                "agent_identity": "child-a",
                "member_ref": member_ref("mob-b", "child-a"),
            }),
            serde_json::json!({ "member_ref": member_ref("mob-a", "child-b") }),
        ] {
            assert_eq!(names_another(&other), Some(true), "{other}");
        }
    }

    /// A record whose detail carries no typed outcome falls back to its
    /// notice status: only `terminated` (a limit autokill) retires the child.
    #[test]
    fn a_record_without_a_typed_outcome_retires_only_on_terminated() {
        use meerkat_core::event::BackgroundJobTerminalStatus;
        let untyped = |status| CommittedCompletion {
            status,
            outcome: None,
        };
        assert!(untyped(BackgroundJobTerminalStatus::Terminated).retires_child());
        assert!(!untyped(BackgroundJobTerminalStatus::Failed).retires_child());
        assert!(!untyped(BackgroundJobTerminalStatus::Completed).retires_child());
    }
}
