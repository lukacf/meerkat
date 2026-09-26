//! Re-link fork_off children after a host restart.
//!
//! A detached fork child's outcome is delivered by a process-local custodian.
//! If the host restarts while a child runs, that custodian is gone while the
//! child (and its durable [`meerkat_mob::ForkJobRecord`]) survives. This pass
//! runs once after restore and, for every seated child with a fork job:
//!
//! - job already over (its completion was admitted to the forker before the
//!   restart): left alone, whatever its limit, since the child may be doing
//!   later work that is not this job's;
//! - already finished (its reply to the job is in its durable transcript):
//!   delivers that result, however late the restart landed, and leaves the
//!   child seated; the opt-in `max_run` limit only bounds a run still going;
//! - still running: waits for the run to end, racing the opt-in `max_run`
//!   limit measured from the original start, then delivers the outcome; when
//!   the limit wins, the run is cancelled and `max_run_elapsed` delivered,
//!   and the child (with its descendants) is retired only once that outcome
//!   is settled, so a delivery that must wait keeps the job on record;
//! - idle with its own reply after the fork prefix: delivers that result;
//! - idle without one (the turn did not survive the restart): delivers a
//!   `restart_interrupted` outcome and leaves the child seated for its forker.
//!
//! A status read that does not observe the child (the mob has one status
//! observation lane, so another reader can hold it) says nothing about the
//! child's state: the pass reads again, and never takes it for an idle child.
//! A read whose run state is unknown (the member was busy and the status
//! projection's bounded runtime read did not answer) is settled by reading
//! the child's runtime state directly.
//!
//! The owner is resolved from the job's owner session before anything can
//! retire the child, never from the child's roster entry: a member of the
//! child's mob or of another managed mob is revived through its mob, and a
//! plain session through the host's owner hook.
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

use meerkat_mob::{AgentIdentity, ForkJobRecord, MemberRunState, MobError, MobHandle, MobId};

use crate::MobMcpState;
use crate::agent_tools::{ForkOffCompletion, ForkOffCompletionStatus, TOOL_FORK_OFF};
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
    /// The forker cannot be revived to receive the outcome yet, for a
    /// reason that clears on its own. The automatic pass delivers again once
    /// it has.
    AwaitingOwner(OwnerRevivalDeferral),
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
    /// Managed mobs, besides the child's own, whose members may own a job:
    /// such an owner is revived through its mob.
    pub owner_mobs: Vec<MobHandle>,
}

impl RelinkDelivery {
    fn for_state(state: &MobMcpState, owner_mobs: Vec<MobHandle>) -> Self {
        Self {
            runtime: state.runtime_adapter_for_relink(),
            owner_host: state.detached_owner_host(),
            owner_mobs,
        }
    }

    /// The delivery of `state`, with every mob it manages now as a
    /// possible owner mob.
    pub(crate) async fn from_state(state: &MobMcpState) -> Self {
        Self::for_state(state, state.managed_mob_handles().await)
    }
}

const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// Pause before reading a child's status again after a read that did not
/// observe it (another reader held the mob's status lane).
const UNOBSERVED_RETRY_INTERVAL: Duration = Duration::from_millis(100);

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
    let delivery = RelinkDelivery::for_state(
        state,
        handles.iter().map(|(_, handle)| handle.clone()).collect(),
    );
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
                .any(|report| matches!(report.action, ForkRelinkAction::AwaitingOwner(_)));
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
/// forker could not be revived yet ([`ForkRelinkAction::AwaitingOwner`]),
/// each time that clears: the mob starts running, or the operation in
/// progress on the forker has had time to finish. Returns the final report
/// of every child in `reports`. Waiting ends when the mob completes, is
/// destroyed or its actor is gone (its members are gone with it), or after
/// [`MAX_OWNER_REVIVAL_WAITS`] waits. Delivery is idempotent per job, so
/// nothing is recorded twice.
pub(crate) async fn redeliver_when_owners_revivable(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    delivery: RelinkDelivery,
    mob_id: &MobId,
    handle: &MobHandle,
    restored_before_ms: u64,
    mut reports: Vec<ForkRelinkReport>,
) -> Vec<ForkRelinkReport> {
    for attempt in 0..MAX_OWNER_REVIVAL_WAITS {
        let awaiting: std::collections::BTreeMap<String, OwnerRevivalDeferral> = reports
            .iter()
            .filter_map(|report| match &report.action {
                ForkRelinkAction::AwaitingOwner(reason) => {
                    Some((report.job_id.clone(), reason.clone()))
                }
                _ => None,
            })
            .collect();
        // An operation in progress clears without a run transition, so it
        // paces the wait when any child waits on one.
        let Some(reason) = awaiting
            .values()
            .find(|reason| {
                matches!(
                    reason,
                    OwnerRevivalDeferral::LifecycleOperationPending { .. }
                )
            })
            .or_else(|| awaiting.values().next())
        else {
            break;
        };
        if !reason.cleared(handle, attempt).await {
            break;
        }
        let retried = relink_mob_fork_children_where(
            Arc::clone(&service),
            delivery.clone(),
            mob_id,
            handle,
            restored_before_ms,
            |job| awaiting.contains_key(&job.job_id),
        )
        .await;
        let delivered = retried
            .iter()
            .filter(|report| report.action == ForkRelinkAction::Delivered)
            .count();
        if delivered > 0 {
            tracing::info!(
                mob_id = %mob_id,
                children = delivered,
                "fork_off re-link delivered outcomes once their forker could be revived"
            );
        }
        reports.retain(|report| !awaiting.contains_key(&report.job_id));
        reports.extend(retried);
    }
    reports
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
        |_| true,
    )
    .await
}

/// [`relink_mob_fork_children`] for the children whose job `select` picks.
async fn relink_mob_fork_children_where(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    delivery: RelinkDelivery,
    mob_id: &MobId,
    handle: &MobHandle,
    restored_before_ms: u64,
    select: impl Fn(&ForkJobRecord) -> bool,
) -> Vec<ForkRelinkReport> {
    // One roster read lists the children and, before anything can retire a
    // child, resolves each job's owner among the mob's members.
    let roster = handle.roster().await;
    let mut children = Vec::new();
    for entry in roster.list() {
        let Some(job) = entry.fork_job.clone() else {
            continue;
        };
        if job.started_at_ms >= restored_before_ms || !select(&job) {
            continue;
        }
        let owner = match roster.find_by_bridge_session_id(&job.owner_session_id) {
            Some(owner) => JobOwner::Member(handle.clone(), owner.agent_identity.clone()),
            None => JobOwner::among_other_mobs(handle, &delivery, &job.owner_session_id).await,
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
            let action =
                relink_owned_child(service, &delivery, &mob_id, &handle, &child, &job, owner).await;
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
    /// A member of a managed mob: revived through its mob.
    Member(MobHandle, AgentIdentity),
    /// A plain session: revived through the host's owner hook, if any.
    Session,
}

impl JobOwner {
    /// The owner of a job bound to `owner_session_id`: a member of the
    /// child's mob (`handle`) or of another managed mob, else a plain
    /// session.
    async fn resolve(
        handle: &MobHandle,
        delivery: &RelinkDelivery,
        owner_session_id: &meerkat_core::SessionId,
    ) -> Self {
        match handle
            .roster()
            .await
            .find_by_bridge_session_id(owner_session_id)
        {
            Some(owner) => Self::Member(handle.clone(), owner.agent_identity.clone()),
            None => Self::among_other_mobs(handle, delivery, owner_session_id).await,
        }
    }

    async fn among_other_mobs(
        handle: &MobHandle,
        delivery: &RelinkDelivery,
        owner_session_id: &meerkat_core::SessionId,
    ) -> Self {
        for other in &delivery.owner_mobs {
            if other.mob_id() == handle.mob_id() {
                continue;
            }
            if let Some(owner) = other
                .roster()
                .await
                .find_by_bridge_session_id(owner_session_id)
            {
                return Self::Member(other.clone(), owner.agent_identity.clone());
            }
        }
        Self::Session
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
    let owner = JobOwner::resolve(handle, delivery, &job.owner_session_id).await;
    relink_owned_child(service, delivery, mob_id, handle, child, job, owner).await
}

async fn relink_owned_child(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    delivery: &RelinkDelivery,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
    owner: JobOwner,
) -> ForkRelinkAction {
    let runtime = delivery.runtime.as_deref();
    // A job whose completion the forker's runtime already admitted is over.
    // The child stays seated for further work that is no longer this job's,
    // so neither the job's limit nor another delivery applies to it.
    if let Some(runtime) = runtime
        && crate::detached_delivery::detached_completion_admitted(
            runtime,
            &job.owner_session_id,
            TOOL_FORK_OFF,
            &job.job_id,
        )
        .await
    {
        return ForkRelinkAction::AlreadyDelivered;
    }
    if let Some(completion) = durable_reply(&service, mob_id, handle, child, job).await {
        return deliver(delivery, &owner, job, completion).await;
    }
    let deadline_ms = job
        .max_run_ms
        .map(|limit| job.started_at_ms.saturating_add(limit));
    loop {
        let observe = observe_child(runtime, handle, child);
        let observed = match deadline_ms {
            None => Some(observe.await),
            Some(deadline) => {
                let remaining = Duration::from_millis(deadline.saturating_sub(now_ms()));
                tokio::select! {
                    observed = observe => Some(observed),
                    () = tokio::time::sleep(remaining) => None,
                }
            }
        };
        let Some(observed) = observed else {
            // The child may have finished while the limit ran down.
            if let Some(completion) = durable_reply(&service, mob_id, handle, child, job).await {
                return deliver(delivery, &owner, job, completion).await;
            }
            return limit_elapsed(delivery, &owner, mob_id, handle, child, job).await;
        };
        match observed {
            ChildObservation::Running => tokio::time::sleep(WATCH_INTERVAL).await,
            ChildObservation::Settled => {
                let completion = settled_outcome(&service, mob_id, handle, child, job).await;
                return deliver(delivery, &owner, job, completion).await;
            }
            ChildObservation::Unobserved(detail) => {
                // The read says nothing about the child's state. A child
                // that finished meanwhile shows in its durable transcript;
                // otherwise read again.
                if let Some(completion) = durable_reply(&service, mob_id, handle, child, job).await
                {
                    return deliver(delivery, &owner, job, completion).await;
                }
                tracing::debug!(
                    mob_id = %mob_id,
                    child = %child,
                    detail = %detail,
                    "fork_off re-link could not observe the child; reading again"
                );
                tokio::time::sleep(UNOBSERVED_RETRY_INTERVAL).await;
            }
        }
    }
}

/// What reading a fork child's status says about it.
#[derive(Debug, PartialEq, Eq)]
enum ChildObservation {
    /// The child has a run open or work in flight.
    Running,
    /// The child is not running: idle, no longer seated, its runtime no
    /// longer holds it, or its mob's actor is gone (nothing runs it any
    /// more).
    Settled,
    /// The read did not observe the child, so it says nothing about the
    /// child's state: another reader held the mob's one status observation
    /// lane, the actor did not answer in time, or a read failed.
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

/// Read a fork child's status: its member status, and when that leaves the
/// run state unknown, its runtime state.
async fn observe_child(
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
/// it, as a `completed` outcome. The child stays seated.
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

/// The opt-in limit won: cancel the child's run and deliver
/// `max_run_elapsed`, then retire the child with its descendants once that
/// outcome is settled. A delivery that must wait (or failed) keeps the child,
/// cancelled, and its job record, so the next pass delivers it.
async fn limit_elapsed(
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
    let action = deliver(delivery, owner, job, completion).await;
    if matches!(
        action,
        ForkRelinkAction::Delivered
            | ForkRelinkAction::AlreadyDelivered
            | ForkRelinkAction::OwnerGone
    ) && let Err(error) = handle.retire_with_descendants(child.clone()).await
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

/// The outcome of an idle child: its own reply after the fork prefix, or
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

async fn deliver(
    delivery: &RelinkDelivery,
    owner: &JobOwner,
    job: &ForkJobRecord,
    completion: ForkOffCompletion,
) -> ForkRelinkAction {
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
    let delivered = match owner {
        JobOwner::Member(owner_mob, owner_identity) => {
            crate::detached_delivery::deliver_detached_completion_to_member(
                runtime,
                owner_mob,
                owner_identity,
                &job.owner_session_id,
                TOOL_FORK_OFF,
                &job.job_id,
                status,
                value,
            )
            .await
        }
        JobOwner::Session => {
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
            reason,
            ..
        }) => ForkRelinkAction::AwaitingOwner(reason),
        Err(error) => ForkRelinkAction::Failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{ChildObservation, ProgressVerdict, from_runtime};
    use meerkat_mob::MemberRunState;
    use meerkat_runtime::{RuntimeDriverError, RuntimeState};

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
}
