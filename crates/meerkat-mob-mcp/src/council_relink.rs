//! Re-link detached councils after a host restart.
//!
//! A convener that runs `council` detached gets a job id back, and the live
//! process records the council's outcome in the convener's transcript from a
//! process-local task. If the host restarts before that record is written,
//! the task is gone, but the council's durable custody record still carries
//! the convener's job
//! ([`meerkat_mob::temporary_council::TemporaryCouncilJobBinding`]). This
//! pass runs after restore and, for every council from an earlier process
//! whose job is still owed an outcome:
//!
//! - sealed (the council finished, or recovery already sealed it): delivers
//!   that sealed outcome, which is the council's real result when it
//!   finished before the restart;
//! - not sealed: councils are never re-executed, so once the dead
//!   coordinator's claim lease is observed expired the ordinary recovery
//!   seals it as a typed `coordinator_interrupted` outcome, and that is
//!   delivered.
//!
//! Delivery is the same durable completion record the live custodian admits
//! ([`crate::detached_delivery`]), under the same per-job idempotency key, so
//! the convener sees each job's outcome exactly once. The binding is then
//! marked settled, so later restarts skip it.

use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::store::TemporaryCouncilRecord;
use meerkat_mob::temporary_council::TemporaryCouncilId;

use crate::MobMcpState;
use crate::agent_tools::{TOOL_COUNCIL, council_outcome_json};
use crate::detached_delivery::{
    DetachedCompletionDelivered, DetachedCompletionError, deliver_detached_completion,
    deliver_detached_completion_to_member,
};
use crate::fork_relink::ForkRelinkAction;
use crate::temporary_council::{TemporaryCouncilError, replay_outcome};
#[cfg(target_arch = "wasm32")]
use crate::tokio;

/// Longest wait between looks at a council still held by a dead coordinator's
/// claim lease.
const LEASE_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Wait between looks at a council that another live owner will seal.
const OWNED_ELSEWHERE_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// One detached council's re-link result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CouncilRelinkReport {
    pub council_id: TemporaryCouncilId,
    pub job_id: String,
    pub action: ForkRelinkAction,
}

/// Re-link every detached council created before `restored_before_ms`
/// (councils run by this process have a live delivery task) whose job is not
/// yet settled.
pub async fn relink_detached_councils(
    state: &Arc<MobMcpState>,
    restored_before_ms: u64,
) -> Vec<CouncilRelinkReport> {
    let Ok(records) = state.temporary_council_store().list_all().await else {
        return Vec::new();
    };
    let restored_before_ms = i64::try_from(restored_before_ms).unwrap_or(i64::MAX);
    let tasks = records
        .into_iter()
        .filter(|record| {
            record
                .detached_job
                .as_ref()
                .is_some_and(|job| job.settled_at.is_none())
                && record.created_at.timestamp_millis() < restored_before_ms
        })
        .map(|record| {
            let state = Arc::clone(state);
            // Each council settles on its own task: one still held by a
            // lease must not hold up the others.
            tokio::spawn(async move { relink_council(&state, record).await })
        });
    futures::future::join_all(tasks)
        .await
        .into_iter()
        .filter_map(Result::ok)
        .collect()
}

/// Settle one detached council and deliver its outcome to the convener.
pub async fn relink_council(
    state: &Arc<MobMcpState>,
    record: TemporaryCouncilRecord,
) -> CouncilRelinkReport {
    let council_id = record.council_id.clone();
    let Some(job) = record.detached_job.clone() else {
        return CouncilRelinkReport {
            council_id,
            job_id: String::new(),
            action: ForkRelinkAction::Failed("the council has no detached job".to_string()),
        };
    };
    let report = |action| CouncilRelinkReport {
        council_id: council_id.clone(),
        job_id: job.job_id.clone(),
        action,
    };
    let store = state.temporary_council_store().clone();
    let coordinator = state.temporary_council();
    let mut record = record;
    loop {
        if record.result.is_some() {
            let outcome = match replay_outcome(record) {
                Ok(outcome) => outcome,
                Err(error) => return report(ForkRelinkAction::Failed(error.to_string())),
            };
            let status = if outcome.result.exit_reason.is_failure() {
                meerkat_core::event::BackgroundJobTerminalStatus::Failed
            } else {
                meerkat_core::event::BackgroundJobTerminalStatus::Completed
            };
            let action = deliver(
                state,
                &job.owner_session_id,
                &job.job_id,
                status,
                council_outcome_json(&outcome),
            )
            .await;
            if matches!(
                action,
                ForkRelinkAction::Delivered | ForkRelinkAction::AlreadyDelivered
            ) {
                mark_settled(state, &council_id).await;
            }
            return report(action);
        }
        // Not sealed. The dead coordinator's claim holds the record until its
        // lease is observed expired; only then may recovery take over and
        // seal the typed interrupted outcome.
        match (record.claim_lease_expires_at - state.temporary_council_now()).to_std() {
            Ok(remaining) if !remaining.is_zero() => {
                tokio::time::sleep(remaining.min(LEASE_POLL_INTERVAL)).await;
            }
            _ => match coordinator.recover_reserved(record).await {
                Ok(Some(_)) => {}
                // A live task in this process owns the council, or another
                // coordinator still holds it: it will seal; look again soon.
                Ok(None) | Err(TemporaryCouncilError::HeldByAnotherCoordinator { .. }) => {
                    tokio::time::sleep(OWNED_ELSEWHERE_POLL_INTERVAL).await;
                }
                Err(error) => return report(ForkRelinkAction::Failed(error.to_string())),
            },
        }
        record = match store.load(&council_id).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                return report(ForkRelinkAction::Failed(format!(
                    "council {council_id} disappeared before its outcome was delivered"
                )));
            }
            Err(error) => return report(ForkRelinkAction::Failed(error.to_string())),
        };
    }
}

/// Deliver a council outcome to its convener through the live custodian's
/// delivery. When the runtime no longer has the convener live (a restart)
/// and the convener is a mob member, it is revived through its mob and the
/// delivery retried, as for a fork_off owner.
async fn deliver(
    state: &Arc<MobMcpState>,
    owner_session_id: &meerkat_core::SessionId,
    job_id: &str,
    status: meerkat_core::event::BackgroundJobTerminalStatus,
    outcome: serde_json::Value,
) -> ForkRelinkAction {
    let Some(runtime) = state.runtime_adapter_for_relink() else {
        return ForkRelinkAction::Failed(
            "no runtime to admit the completion on this host".to_string(),
        );
    };
    let delivered = match deliver_detached_completion(
        &runtime,
        owner_session_id,
        TOOL_COUNCIL,
        job_id,
        status,
        outcome.clone(),
    )
    .await
    {
        Err(error @ DetachedCompletionError::Runtime { .. }) => {
            match state.member_for_bridge_session(owner_session_id).await {
                Ok(Some((mob_id, identity))) => match state.handle_for(&mob_id).await {
                    Ok(handle) => {
                        deliver_detached_completion_to_member(
                            &runtime,
                            &handle,
                            &identity,
                            owner_session_id,
                            TOOL_COUNCIL,
                            job_id,
                            status,
                            outcome,
                        )
                        .await
                    }
                    Err(_) => Err(error),
                },
                _ => Err(error),
            }
        }
        other => other,
    };
    match delivered {
        Ok(DetachedCompletionDelivered::Delivered) => ForkRelinkAction::Delivered,
        Ok(_) => ForkRelinkAction::AlreadyDelivered,
        Err(error) => ForkRelinkAction::Failed(error.to_string()),
    }
}

/// Mark the council's job settled so later restarts skip it. Best effort: a
/// lost race leaves it unmarked, and the next re-link finds the job already
/// delivered and marks it then.
async fn mark_settled(state: &MobMcpState, council_id: &TemporaryCouncilId) {
    let store = state.temporary_council_store();
    for _ in 0..3 {
        let Ok(Some(mut record)) = store.load(council_id).await else {
            return;
        };
        let Some(job) = record.detached_job.as_mut() else {
            return;
        };
        if job.settled_at.is_some() {
            return;
        }
        job.settled_at = Some(state.temporary_council_now());
        match store.commit(&record).await {
            Ok(_) => return,
            Err(meerkat_mob::store::MobStoreError::CasConflict(_)) => continue,
            Err(error) => {
                tracing::debug!(
                    council_id = %council_id,
                    error = %error,
                    "could not mark a re-linked council job settled; a later re-link will"
                );
                return;
            }
        }
    }
}
