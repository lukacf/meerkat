//! Re-link fork_off children after a host restart.
//!
//! A detached fork child's outcome is delivered by a process-local custodian.
//! If the host restarts while a child runs, that custodian is gone while the
//! child (and its durable [`meerkat_mob::ForkJobRecord`]) survives. This pass
//! runs once after restore and, for every seated child with a fork job:
//!
//! - still running: waits for the run to end, racing the opt-in `max_run`
//!   limit measured from the original start, then delivers the outcome;
//! - idle with its own reply after the fork prefix: delivers that result;
//! - idle without one (the turn did not survive the restart): delivers a
//!   `restart_interrupted` outcome and leaves the child seated for its forker.
//!
//! Delivery is the same durable completion record the live custodian admits
//! ([`crate::detached_delivery`]), under the same idempotency key, so a job
//! already delivered before the restart is never recorded twice, and an idle
//! owner is woken to see it.

use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::{AgentIdentity, ForkJobRecord, MemberRunState, MobHandle, MobId};

use crate::MobMcpState;
use crate::agent_tools::{ForkOffCompletion, ForkOffCompletionStatus, TOOL_FORK_OFF};

/// What the re-link pass did for one child.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForkRelinkAction {
    /// The outcome was recorded in the forker's transcript now.
    Delivered,
    /// The outcome had already been recorded before the restart.
    AlreadyDelivered,
    /// Delivery failed (for example the forker's session is gone).
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

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
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
            reports.extend(
                relink_mob_fork_children(
                    state.session_service(),
                    state.runtime_adapter_for_relink(),
                    &mob_id,
                    &handle,
                    restored_before_ms,
                )
                .await,
            );
        }
    }
    reports
}

/// Re-link the fork children of one mob (see the module docs).
pub async fn relink_mob_fork_children(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    runtime: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    mob_id: &MobId,
    handle: &MobHandle,
    restored_before_ms: u64,
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
        .filter(|(_, job)| job.started_at_ms < restored_before_ms)
        .collect();
    // Each child settles on its own task: observing a member waits for its
    // turn boundary, so one busy child must not hold up the others.
    let tasks = children.into_iter().map(|(child, job)| {
        let service = Arc::clone(&service);
        let runtime = runtime.clone();
        let mob_id = mob_id.clone();
        let handle = handle.clone();
        tokio::spawn(async move {
            let action = relink_child(service, runtime, &mob_id, &handle, &child, &job).await;
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
/// Observing the child waits for its current turn boundary. With an opt-in
/// `max_run`, that wait is raced against the limit measured from the job's
/// original start; the limit winning cancels and retires the child (and its
/// descendants) and delivers `max_run_elapsed`.
pub async fn relink_child(
    service: Arc<dyn meerkat_mob::MobSessionService>,
    runtime: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    mob_id: &MobId,
    handle: &MobHandle,
    child: &AgentIdentity,
    job: &ForkJobRecord,
) -> ForkRelinkAction {
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
        let running = match &observed {
            None => {
                let completion = autokill(mob_id, handle, child, job).await;
                return deliver(runtime.as_deref(), handle, child, job, completion).await;
            }
            Some(Ok(snapshot)) => snapshot.progress.as_ref().is_some_and(|progress| {
                progress.run_state == MemberRunState::RunOpen || progress.in_flight_work > 0
            }),
            Some(Err(_)) => false,
        };
        if !running {
            let completion = settled_outcome(&service, mob_id, handle, child, job).await;
            return deliver(runtime.as_deref(), handle, child, job, completion).await;
        }
        tokio::time::sleep(WATCH_INTERVAL).await;
    }
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
    let replied = match handle.resolve_bridge_session_id(child).await {
        Some(session_id) => match service.load_persisted_session(&session_id).await {
            Ok(Some(session)) => {
                let messages = session.messages();
                messages.len() > job.prefix_message_count.saturating_add(1)
                    && matches!(
                        messages.last(),
                        Some(meerkat_core::Message::BlockAssistant(_))
                    )
            }
            _ => false,
        },
        None => false,
    };
    if replied
        && let Ok(result) = handle
            .bounded_terminal_member_result(child, job.result_label.clone(), job.max_text_bytes)
            .await
    {
        let mut completion = ForkOffCompletion::empty(
            child.to_string(),
            member_ref(mob_id, child),
            ForkOffCompletionStatus::Completed,
        );
        completion.bounded_result = Some(result.to_wire());
        return completion;
    }
    ForkOffCompletion::empty(
        child.to_string(),
        member_ref(mob_id, child),
        ForkOffCompletionStatus::RestartInterrupted,
    )
}

fn member_ref(mob_id: &MobId, child: &AgentIdentity) -> meerkat_contracts::WireMemberRef {
    meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), child.as_str())
}

async fn deliver(
    runtime: Option<&meerkat_runtime::MeerkatMachine>,
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
        None => {
            crate::detached_delivery::deliver_detached_completion(
                runtime,
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
        Err(error) => ForkRelinkAction::Failed(error.to_string()),
    }
}
