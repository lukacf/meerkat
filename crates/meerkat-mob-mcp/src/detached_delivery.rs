//! Delivery of a detached job's outcome (fork_off, council) to its owner.
//!
//! The outcome is recorded once as a durable `BackgroundJob` system notice
//! (`SystemNoticeBlock::BackgroundJob { persisted: true, .. }`) in the owner's
//! transcript. It is submitted as a runtime prompt input whose only content is
//! that notice, as a typed append with no user text:
//!
//! - an idle owner gets a real pending boundary and runs one turn that sees
//!   the outcome;
//! - a running owner gets exactly one follow-up turn after its current turn
//!   ends, and that turn sees the outcome;
//! - the input's idempotency key names the job (`{tool}:{job_id}`), so the
//!   record is admitted and written exactly once however often delivery runs.
//!
//! A system notice (not a role=System message) is what every provider accepts
//! mid-conversation.

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_core::event::BackgroundJobTerminalStatus;
use meerkat_core::types::SystemNoticeMessage;

#[cfg(target_arch = "wasm32")]
use crate::tokio;

/// Outcome of one delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DetachedCompletionDelivered {
    /// The record was admitted now.
    Delivered,
    /// This job's record was already admitted earlier.
    AlreadyDelivered,
}

/// Why a delivery could not be admitted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DetachedCompletionError {
    #[error("could not encode the {tool} outcome: {detail}")]
    Encode { tool: &'static str, detail: String },
    #[error("the owner session refused the {tool} completion: {detail}")]
    Rejected { tool: &'static str, detail: String },
    #[error("the {tool} completion could not reach its owner session: {detail}")]
    Runtime { tool: &'static str, detail: String },
    /// The owner no longer exists: its member was retired, or its session
    /// was archived or deleted. The completion can never be delivered, so a
    /// caller may stop retrying it.
    #[error("the owner of the {tool} completion is gone: {detail}")]
    OwnerGone { tool: &'static str, detail: String },
    /// The owner is a member of mob `mob_id` and cannot be revived to
    /// receive the completion yet, for a reason that clears on its own.
    /// Delivery succeeds once it has.
    #[error("the owner of the {tool} completion, in mob {mob_id}, cannot be revived yet: {reason}")]
    OwnerRevivalDeferred {
        tool: &'static str,
        mob_id: meerkat_mob::MobId,
        reason: OwnerRevivalDeferral,
    },
}

/// Why a mob member cannot be revived to receive a detached completion yet.
/// Each reason clears on its own.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum OwnerRevivalDeferral {
    /// Its mob is not running (`phase`): revival is admitted only while the
    /// mob runs.
    #[error("its mob is {phase}, not running")]
    MobNotRunning { phase: meerkat_mob::MobState },
    /// A lifecycle operation on the member is still in progress, such as
    /// the mob's resume reviving it.
    #[error("a lifecycle operation on it is still in progress: {intent}")]
    LifecycleOperationPending { intent: String },
}

/// Longest pause before another delivery to an owner whose lifecycle
/// operation was still in progress.
const PENDING_OPERATION_MAX_PAUSE: std::time::Duration = std::time::Duration::from_secs(5);

impl OwnerRevivalDeferral {
    /// Wait until an owner deferred for this reason may be revivable in
    /// `handle`'s mob. `false` when it never will be: the mob completed, was
    /// destroyed or its actor is gone. `attempt` counts the earlier waits and
    /// paces the wait for an operation in progress, which has no completion
    /// signal of its own.
    pub(crate) async fn cleared(&self, handle: &meerkat_mob::MobHandle, attempt: u32) -> bool {
        if let Self::LifecycleOperationPending { .. } = self {
            let pause = std::time::Duration::from_millis(100)
                .saturating_mul(2_u32.saturating_pow(attempt.min(16)))
                .min(PENDING_OPERATION_MAX_PAUSE);
            tokio::time::sleep(pause).await;
        }
        mob_runs(handle).await
    }
}

/// Wait until `handle`'s mob is running. `false` when it will not run again:
/// it completed or was destroyed, or its actor is gone.
pub(crate) async fn mob_runs(handle: &meerkat_mob::MobHandle) -> bool {
    use meerkat_mob::MobState;
    // Subscribe before reading, so a transition after the read wakes us.
    let mut changes = handle.machine_state_changes();
    loop {
        match handle.status_observation_snapshot() {
            MobState::Running => return true,
            MobState::Completed | MobState::Destroyed => return false,
            MobState::Creating | MobState::Stopped => {}
        }
        if changes.changed().await.is_err() {
            return false;
        }
    }
}

/// The idempotency key of job `job_id`'s completion input: one per job.
pub(crate) fn detached_completion_key(tool: &str, job_id: &str) -> String {
    format!("{tool}:{job_id}")
}

/// Whether job `job_id`'s completion was already admitted to its owner,
/// read from the runtime's durable input index (never live state). A job
/// with an admitted completion is over. `false` when there is no durable
/// evidence, including on a store-less runtime.
pub(crate) async fn detached_completion_admitted(
    runtime: &meerkat_runtime::MeerkatMachine,
    owner_session_id: &SessionId,
    tool: &str,
    job_id: &str,
) -> bool {
    use meerkat_runtime::SessionServiceRuntimeExt as _;
    matches!(
        runtime
            .durable_input_state_by_idempotency_key(
                owner_session_id,
                &detached_completion_key(tool, job_id),
            )
            .await,
        Ok(Some(_))
    )
}

/// The durable completion record for one job.
pub fn detached_completion_notice(
    tool: &'static str,
    job_id: &str,
    status: BackgroundJobTerminalStatus,
    outcome: &serde_json::Value,
) -> Result<SystemNoticeMessage, DetachedCompletionError> {
    let detail =
        serde_json::to_string(outcome).map_err(|error| DetachedCompletionError::Encode {
            tool,
            detail: error.to_string(),
        })?;
    Ok(SystemNoticeMessage::persisted_background_job(
        tool, job_id, status, detail,
    ))
}

/// Deliver one detached job's outcome to its owner session, exactly once.
pub async fn deliver_detached_completion(
    runtime: &meerkat_runtime::MeerkatMachine,
    owner_session_id: &SessionId,
    tool: &'static str,
    job_id: &str,
    status: BackgroundJobTerminalStatus,
    outcome: serde_json::Value,
) -> Result<DetachedCompletionDelivered, DetachedCompletionError> {
    let notice = detached_completion_notice(tool, job_id, status, &outcome)?;
    let input =
        meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::detached_job_completed(
            detached_completion_key(tool, job_id),
            notice,
        ));
    match runtime
        .accept_input_with_completion(owner_session_id, input)
        .await
    {
        Ok((meerkat_runtime::AcceptOutcome::Accepted { .. }, _)) => {
            Ok(DetachedCompletionDelivered::Delivered)
        }
        Ok((meerkat_runtime::AcceptOutcome::Deduplicated { .. }, _)) => {
            Ok(DetachedCompletionDelivered::AlreadyDelivered)
        }
        Ok((outcome, _)) => Err(DetachedCompletionError::Rejected {
            tool,
            detail: format!("{outcome:?}"),
        }),
        Err(error) => Err(DetachedCompletionError::Runtime {
            tool,
            detail: error.to_string(),
        }),
    }
}

/// [`deliver_detached_completion`] for an owner that is a mob member.
///
/// When the runtime no longer has the owner's session live (its idle
/// executor was retired, or the host restarted), the owner is revived
/// through the mob and delivery is retried once. The job's idempotency key
/// still guarantees a single record.
#[allow(clippy::too_many_arguments)]
pub async fn deliver_detached_completion_to_member(
    runtime: &meerkat_runtime::MeerkatMachine,
    owner: &meerkat_mob::MobHandle,
    owner_identity: &meerkat_mob::AgentIdentity,
    owner_session_id: &SessionId,
    tool: &'static str,
    job_id: &str,
    status: BackgroundJobTerminalStatus,
    outcome: serde_json::Value,
) -> Result<DetachedCompletionDelivered, DetachedCompletionError> {
    match deliver_detached_completion(
        runtime,
        owner_session_id,
        tool,
        job_id,
        status,
        outcome.clone(),
    )
    .await
    {
        Err(DetachedCompletionError::Runtime { .. }) => {
            owner
                .ensure_member_live(owner_identity)
                .await
                .map_err(|error| match error {
                    meerkat_mob::MobError::MemberNotFound(_) => {
                        DetachedCompletionError::OwnerGone {
                            tool,
                            detail: format!(
                                "the owner member {owner_identity} is no longer seated"
                            ),
                        }
                    }
                    // Revival is admitted only while the mob runs. A mob that
                    // has ended for good takes its members with it; any other
                    // phase is a mob that is not running yet or again.
                    meerkat_mob::MobError::InvalidTransition {
                        from:
                            phase
                            @ (meerkat_mob::MobState::Completed | meerkat_mob::MobState::Destroyed),
                        to: meerkat_mob::MobState::Running,
                    } => DetachedCompletionError::OwnerGone {
                        tool,
                        detail: format!(
                            "the owner member {owner_identity} is in mob {}, which is {phase}",
                            owner.mob_id()
                        ),
                    },
                    meerkat_mob::MobError::InvalidTransition {
                        from: phase,
                        to: meerkat_mob::MobState::Running,
                    } => DetachedCompletionError::OwnerRevivalDeferred {
                        tool,
                        mob_id: owner.mob_id().clone(),
                        reason: OwnerRevivalDeferral::MobNotRunning { phase },
                    },
                    meerkat_mob::MobError::LifecycleOperationPending { intent } => {
                        DetachedCompletionError::OwnerRevivalDeferred {
                            tool,
                            mob_id: owner.mob_id().clone(),
                            reason: OwnerRevivalDeferral::LifecycleOperationPending { intent },
                        }
                    }
                    error => DetachedCompletionError::Runtime {
                        tool,
                        detail: format!("reviving the owner failed: {error}"),
                    },
                })?;
            deliver_detached_completion(runtime, owner_session_id, tool, job_id, status, outcome)
                .await
        }
        other => other,
    }
}

/// Why a host cannot use detached delivery for a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DetachedDeliveryUnavailable {
    /// The host declared it cannot deliver later (one-shot surfaces).
    HostDeclaredUnavailable,
    /// The host claims delivery but has no runtime to admit the completion.
    NoRuntimeAdapter,
}

pub(crate) type DetachedDeliveryRoute =
    Result<Arc<meerkat_runtime::MeerkatMachine>, DetachedDeliveryUnavailable>;
