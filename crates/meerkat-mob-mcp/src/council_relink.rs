//! Re-link detached councils after a host restart, and the lease-aware
//! post-restore council sweep that drives it.
//!
//! A convener that runs `council` detached gets a job id back, and the live
//! process records the council's outcome in the convener's transcript from a
//! process-local task. If the host restarts before that record is written,
//! the task is gone, but the council's durable custody record still carries
//! the convener's job
//! ([`meerkat_mob::temporary_council::TemporaryCouncilJobBinding`]).
//!
//! [`restore_sweep`] runs once per state after restore, on every host whose
//! council store is durable. Each pass:
//!
//! 1. recovers unfinished councils ([`TemporaryCouncilCoordinator::sweep_unfinished`]):
//!    councils are never re-executed, so a council the dead coordinator never
//!    sealed is sealed as a typed `coordinator_interrupted` outcome;
//! 2. re-links every sealed council from an earlier process whose job is
//!    still owed an outcome, delivering that sealed outcome: the council's
//!    real result when it finished before the restart;
//! 3. if a record was skipped because the previous process's claim lease had
//!    not been observed expired (a restart inside the lease is the common
//!    case), waits until that lease expires and runs another pass.
//!
//! The waiting is bounded: a record whose lease keeps being renewed belongs
//! to a live coordinator and stops being waited for, and the sweep stops
//! after a fixed number of passes. It holds the state weakly, so a dropped
//! state ends it.
//!
//! Delivery is the same durable completion record the live custodian admits
//! ([`crate::detached_delivery`]), under the same per-job idempotency key, so
//! the convener sees each job's outcome exactly once. The binding is then
//! marked settled, so later restarts skip it.
//!
//! [`TemporaryCouncilCoordinator::sweep_unfinished`]: crate::TemporaryCouncilCoordinator::sweep_unfinished

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use meerkat_mob::store::TemporaryCouncilRecord;
use meerkat_mob::temporary_council::TemporaryCouncilId;

use crate::MobMcpState;
use crate::agent_tools::{TOOL_COUNCIL, council_outcome_json};
use crate::detached_delivery::{
    DetachedCompletionDelivered, DetachedCompletionError, deliver_detached_completion,
    deliver_detached_completion_to_member,
};
use crate::temporary_council::replay_outcome;
#[cfg(target_arch = "wasm32")]
use crate::tokio;

/// Passes the post-restore sweep makes at most.
const MAX_SWEEP_PASSES: usize = 16;

/// Lease renewals observed on one held record before the sweep treats it as
/// owned by a live coordinator and stops waiting for it.
const MAX_OBSERVED_RENEWALS: u32 = 2;

/// Slack after an observed lease expiry before the next pass, so the pass
/// observes the lease as expired.
const LEASE_EXPIRY_MARGIN: Duration = Duration::from_millis(250);

/// What the re-link did for one detached council.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CouncilRelinkAction {
    /// The outcome was delivered to the convener now.
    Delivered,
    /// The outcome had already been delivered.
    AlreadyDelivered,
    /// The council is not sealed yet: the previous process's claim holds it
    /// until `claim_lease_expires_at`, after which recovery seals it and a
    /// later pass delivers it.
    AwaitingSeal {
        claim_lease_expires_at: DateTime<Utc>,
    },
    /// Delivery failed; a later re-link retries it.
    Failed(String),
}

/// One detached council's re-link result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CouncilRelinkReport {
    pub council_id: TemporaryCouncilId,
    pub job_id: String,
    pub action: CouncilRelinkAction,
}

/// The post-restore council sweep: recovery, re-link, and a retry at the
/// lease expiry of every record a live-looking claim still held. See the
/// module docs.
pub(crate) async fn restore_sweep(state: Weak<MobMcpState>) {
    // Latest observed lease expiry and renewal count per held record.
    let mut observed: BTreeMap<TemporaryCouncilId, (DateTime<Utc>, u32)> = BTreeMap::new();
    for _ in 0..MAX_SWEEP_PASSES {
        let Some(strong) = state.upgrade() else {
            return;
        };
        let mut clock = strong.temporary_council_clock_changes();
        let held = match strong.temporary_council().sweep_unfinished().await {
            Ok(sweep) => {
                if !sweep.recovered.is_empty() {
                    tracing::info!(
                        recovered = sweep.recovered.len(),
                        "temporary council recovery sweep converged unfinished records"
                    );
                }
                for held in &sweep.held {
                    tracing::info!(
                        council_id = %held.council_id,
                        claim_lease_expires_at = %held.claim_lease_expires_at,
                        "temporary council held by another coordinator's claim; retrying after its lease"
                    );
                }
                sweep.held
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "temporary council recovery sweep failed; records remain unfinished"
                );
                Vec::new()
            }
        };
        // After recovery, so councils it sealed deliver at once.
        let reports = relink_detached_councils(&strong, strong.created_at_ms).await;
        let delivered = reports
            .iter()
            .filter(|report| report.action == CouncilRelinkAction::Delivered)
            .count();
        if delivered > 0 {
            tracing::info!(
                councils = delivered,
                "council re-link delivered detached outcomes from a previous process"
            );
        }

        let mut next_expiry: Option<DateTime<Utc>> = None;
        for record in held {
            let entry = observed
                .entry(record.council_id.clone())
                .or_insert((record.claim_lease_expires_at, 0));
            if record.claim_lease_expires_at > entry.0 {
                entry.0 = record.claim_lease_expires_at;
                entry.1 = entry.1.saturating_add(1);
            }
            if entry.1 > MAX_OBSERVED_RENEWALS {
                // A coordinator keeps renewing its claim: it is alive and
                // owns the record.
                continue;
            }
            next_expiry = Some(next_expiry.map_or(record.claim_lease_expires_at, |next| {
                next.min(record.claim_lease_expires_at)
            }));
        }
        let Some(next_expiry) = next_expiry else {
            return;
        };
        let wait = (next_expiry - strong.temporary_council_now())
            .to_std()
            .unwrap_or_default()
            .saturating_add(LEASE_EXPIRY_MARGIN);
        // Wait without keeping the state alive.
        drop(strong);
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            _ = clock.changed() => {}
        }
    }
    tracing::warn!(
        passes = MAX_SWEEP_PASSES,
        "temporary council recovery sweep stopped waiting for held records"
    );
}

/// Deliver the sealed outcome of every detached council created before
/// `restored_before_ms` (councils run by this process have a live delivery
/// task) whose job is not yet settled. A council that is not sealed yet is
/// reported as [`CouncilRelinkAction::AwaitingSeal`].
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
            // Each council delivers on its own task: one convener's busy
            // session must not hold up the others.
            tokio::spawn(async move { relink_council(&state, record).await })
        });
    futures::future::join_all(tasks)
        .await
        .into_iter()
        .filter_map(Result::ok)
        .collect()
}

/// Deliver one detached council's sealed outcome to its convener.
pub async fn relink_council(
    state: &Arc<MobMcpState>,
    record: TemporaryCouncilRecord,
) -> CouncilRelinkReport {
    let council_id = record.council_id.clone();
    let Some(job) = record.detached_job.clone() else {
        return CouncilRelinkReport {
            council_id,
            job_id: String::new(),
            action: CouncilRelinkAction::Failed("the council has no detached job".to_string()),
        };
    };
    let report = |action| CouncilRelinkReport {
        council_id: council_id.clone(),
        job_id: job.job_id.clone(),
        action,
    };
    if record.result.is_none() {
        return report(CouncilRelinkAction::AwaitingSeal {
            claim_lease_expires_at: record.claim_lease_expires_at,
        });
    }
    let outcome = match replay_outcome(record) {
        Ok(outcome) => outcome,
        Err(error) => return report(CouncilRelinkAction::Failed(error.to_string())),
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
        CouncilRelinkAction::Delivered | CouncilRelinkAction::AlreadyDelivered
    ) {
        mark_settled(state, &council_id).await;
    }
    report(action)
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
) -> CouncilRelinkAction {
    let Some(runtime) = state.runtime_adapter_for_relink() else {
        return CouncilRelinkAction::Failed(
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
        Ok(DetachedCompletionDelivered::Delivered) => CouncilRelinkAction::Delivered,
        Ok(_) => CouncilRelinkAction::AlreadyDelivered,
        Err(error) => CouncilRelinkAction::Failed(error.to_string()),
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
