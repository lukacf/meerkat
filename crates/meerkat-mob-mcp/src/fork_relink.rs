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
//!   limit measured from the original start, then delivers the outcome;
//! - idle with its own reply after the fork prefix: delivers that result;
//! - idle without one (the turn did not survive the restart): delivers a
//!   `restart_interrupted` outcome and leaves the child seated for its forker.
//!
//! A status read that does not observe the child (the mob has one status
//! observation lane, so another reader can hold it) says nothing about the
//! child's state: the pass reads again, and never takes it for an idle child.
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

use meerkat_mob::{
    AgentIdentity, ForkJobRecord, MemberRunState, MobError, MobHandle, MobId, MobMemberSnapshot,
};

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
    for (mob_id, handle) in handles {
        let claimed = state.claim_fork_relink(&mob_id);
        if claimed || !respect_claims {
            let mob_reports = relink_mob_fork_children(
                state.session_service(),
                state.runtime_adapter_for_relink(),
                state.detached_owner_host(),
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
                let runtime = state.runtime_adapter_for_relink();
                let owner_host = state.detached_owner_host();
                let pending = mob_reports.clone();
                let (mob_id, handle) = (mob_id.clone(), handle.clone());
                tokio::spawn(async move {
                    redeliver_when_owners_revivable(
                        service,
                        runtime,
                        owner_host,
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
    runtime: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    owner_host: Option<Arc<dyn DetachedOwnerHost>>,
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
            runtime.clone(),
            owner_host.clone(),
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
/// `owner_host` makes an owner that is a plain session, not a mob member,
/// live for its delivery (see [`DetachedOwnerHost`]).
pub async fn relink_mob_fork_children(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    runtime: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    owner_host: Option<Arc<dyn DetachedOwnerHost>>,
    mob_id: &MobId,
    handle: &MobHandle,
    restored_before_ms: u64,
) -> Vec<ForkRelinkReport> {
    relink_mob_fork_children_where(
        service,
        runtime,
        owner_host,
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
    runtime: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    owner_host: Option<Arc<dyn DetachedOwnerHost>>,
    mob_id: &MobId,
    handle: &MobHandle,
    restored_before_ms: u64,
    select: impl Fn(&ForkJobRecord) -> bool,
) -> Vec<ForkRelinkReport> {
    let children: Vec<(AgentIdentity, ForkJobRecord)> = handle
        .roster()
        .await
        .list()
        .filter_map(|entry| {
            entry
                .fork_job
                .clone()
                .map(|job| (entry.agent_identity.clone(), job))
        })
        .filter(|(_, job)| job.started_at_ms < restored_before_ms && select(job))
        .collect();
    // Each child settles on its own task: observing a member waits for its
    // turn boundary, so one busy child must not hold up the others.
    let tasks = children.into_iter().map(|(child, job)| {
        let service = Arc::clone(&service);
        let runtime = runtime.clone();
        let owner_host = owner_host.clone();
        let mob_id = mob_id.clone();
        let handle = handle.clone();
        tokio::spawn(async move {
            let action =
                relink_child(service, runtime, owner_host, &mob_id, &handle, &child, &job).await;
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

/// Settle one fork child and deliver its outcome to the forker.
///
/// A child whose reply to the job is already durable delivers that reply
/// first, before any limit is evaluated: the restart may land long after the
/// child finished within its limit, and its real outcome must not be replaced
/// by `max_run_elapsed`. Otherwise observing the child waits for its current
/// turn boundary. With an opt-in `max_run`, that wait is raced against the
/// limit measured from the job's original start; the limit winning (with
/// still no durable reply) cancels and retires the child (and its
/// descendants) and delivers `max_run_elapsed`.
///
/// An owner that is not the child's forker in the mob (a plain session the
/// job was bound to) is made live through `owner_host` when the runtime does
/// not have it live; without a host the runtime's refusal is reported.
pub async fn relink_child(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    runtime: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    owner_host: Option<Arc<dyn DetachedOwnerHost>>,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
) -> ForkRelinkAction {
    // A job whose completion the forker's runtime already admitted is over.
    // The child stays seated for further work that is no longer this job's,
    // so neither the job's limit nor another delivery applies to it.
    if let Some(runtime) = runtime.as_deref()
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
        return deliver(
            runtime.as_deref(),
            owner_host.as_deref(),
            handle,
            child,
            job,
            completion,
        )
        .await;
    }
    let deadline_ms = job
        .max_run_ms
        .map(|limit| job.started_at_ms.saturating_add(limit));
    loop {
        let observed = match deadline_ms {
            None => Some(handle.member_status(child).await),
            Some(deadline) => {
                let remaining = Duration::from_millis(deadline.saturating_sub(now_ms()));
                tokio::select! {
                    observed = handle.member_status(child) => Some(observed),
                    () = tokio::time::sleep(remaining) => None,
                }
            }
        };
        let Some(observed) = observed else {
            // The child may have finished while the limit ran down.
            let completion = match durable_reply(&service, mob_id, handle, child, job).await {
                Some(completion) => completion,
                None => autokill(mob_id, handle, child, job).await,
            };
            return deliver(
                runtime.as_deref(),
                owner_host.as_deref(),
                handle,
                child,
                job,
                completion,
            )
            .await;
        };
        match ChildObservation::of(observed) {
            ChildObservation::Running => tokio::time::sleep(WATCH_INTERVAL).await,
            ChildObservation::Settled => {
                let completion = settled_outcome(&service, mob_id, handle, child, job).await;
                return deliver(
                    runtime.as_deref(),
                    owner_host.as_deref(),
                    handle,
                    child,
                    job,
                    completion,
                )
                .await;
            }
            ChildObservation::Unobserved(error) => {
                // The read says nothing about the child's state. A child
                // that finished meanwhile shows in its durable transcript;
                // otherwise read again.
                if let Some(completion) = durable_reply(&service, mob_id, handle, child, job).await
                {
                    return deliver(
                        runtime.as_deref(),
                        owner_host.as_deref(),
                        handle,
                        child,
                        job,
                        completion,
                    )
                    .await;
                }
                tracing::debug!(
                    mob_id = %mob_id,
                    child = %child,
                    error = %error,
                    "fork_off re-link could not observe the child; reading again"
                );
                tokio::time::sleep(UNOBSERVED_RETRY_INTERVAL).await;
            }
        }
    }
}

/// What one status read says about a fork child.
enum ChildObservation {
    /// The child has a run open or work in flight.
    Running,
    /// The child is not running: idle, no longer seated, or its mob's actor
    /// is gone (nothing runs it any more).
    Settled,
    /// The read did not observe the child, so it says nothing about the
    /// child's state: another reader held the mob's one status observation
    /// lane, the actor did not answer in time, or the read failed.
    Unobserved(MobError),
}

impl ChildObservation {
    fn of(read: Result<MobMemberSnapshot, MobError>) -> Self {
        match read {
            Ok(snapshot) => {
                let running = snapshot.progress.as_ref().is_some_and(|progress| {
                    progress.run_state == MemberRunState::RunOpen || progress.in_flight_work > 0
                });
                if running {
                    Self::Running
                } else {
                    Self::Settled
                }
            }
            Err(MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed) => {
                Self::Settled
            }
            Err(error) => Self::Unobserved(error),
        }
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

async fn autokill(
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
) -> ForkOffCompletion {
    let _ = handle.force_cancel_member(child.clone()).await;
    let retirement_error = handle
        .retire_with_descendants(child.clone())
        .await
        .err()
        .map(|error| error.to_string());
    let mut completion = ForkOffCompletion::empty(
        child.to_string(),
        member_ref(mob_id, child),
        ForkOffCompletionStatus::MaxRunElapsed,
    );
    completion.max_run_secs = job.max_run_ms.map(|limit| limit / 1000);
    completion.retirement_error = retirement_error;
    completion
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
    runtime: Option<&meerkat_runtime::MeerkatMachine>,
    owner_host: Option<&dyn DetachedOwnerHost>,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
    completion: ForkOffCompletion,
) -> ForkRelinkAction {
    let Some(runtime) = runtime else {
        return ForkRelinkAction::Failed(
            "no runtime to admit the completion on this host".to_string(),
        );
    };
    let status = match completion.status {
        ForkOffCompletionStatus::Completed => {
            meerkat_core::event::BackgroundJobTerminalStatus::Completed
        }
        ForkOffCompletionStatus::MaxRunElapsed => {
            meerkat_core::event::BackgroundJobTerminalStatus::Terminated
        }
        _ => meerkat_core::event::BackgroundJobTerminalStatus::Failed,
    };
    let value = match serde_json::to_value(&completion) {
        Ok(value) => value,
        Err(error) => return ForkRelinkAction::Failed(error.to_string()),
    };
    // The forker is the child's spawner; reviving it through the mob covers
    // an owner the restarted runtime does not have live.
    let owner = handle
        .roster()
        .await
        .get_by_identity(child)
        .and_then(|entry| entry.spawned_by.clone());
    let delivered = match owner {
        Some(owner) => {
            crate::detached_delivery::deliver_detached_completion_to_member(
                runtime,
                handle,
                &owner,
                &job.owner_session_id,
                TOOL_FORK_OFF,
                &job.job_id,
                status,
                value,
            )
            .await
        }
        // An owner that is not the child's forker in the mob is a plain
        // session; the host makes it live when the runtime does not have it.
        None => {
            crate::detached_delivery::deliver_detached_completion_to_session(
                runtime,
                owner_host,
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
