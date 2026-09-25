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
