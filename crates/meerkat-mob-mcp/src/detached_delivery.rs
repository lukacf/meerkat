//! Delivery of a detached job's outcome (fork_off, council) to its owner.
//!
//! The outcome is recorded once as a durable `BackgroundJob` system notice
//! (`SystemNoticeBlock::BackgroundJob { persisted: true, .. }`) in the owner's
//! transcript. It is submitted as a runtime continuation input whose turn
//! append is that notice:
//!
//! - an idle owner gets a real pending boundary and runs one turn that sees
//!   the outcome;
//! - a running owner gets it steered in at its next checkpoint, with no
//!   second turn;
//! - the input's idempotency key names the job (`{tool}:{job_id}`), so the
//!   record is admitted and written exactly once however often delivery runs.
//!
//! A system notice (not a role=System message) is what every provider accepts
//! mid-conversation.

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_core::event::BackgroundJobTerminalStatus;
use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind, SystemNoticeMessage};

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
    Ok(SystemNoticeMessage::with_blocks(
        SystemNoticeKind::BackgroundJob,
        Some(format!(
            "Background {tool} job {job_id} finished ({}):\n{detail}",
            status.as_str()
        )),
        vec![SystemNoticeBlock::BackgroundJob {
            job_id: job_id.to_string(),
            display_name: Some(tool.to_string()),
            status,
            detail: Some(detail),
            persisted: true,
        }],
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
    let input = meerkat_runtime::Input::Continuation(
        meerkat_runtime::ContinuationInput::detached_job_completed(
            format!("{tool}:{job_id}"),
            notice,
        ),
    );
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

/// Why a host cannot use detached delivery for a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DetachedDeliveryUnavailable {
    /// The host declared it cannot deliver later (one-shot surfaces).
    HostDeclaredUnavailable,
    /// The host claims delivery but has no runtime to admit the completion.
    NoRuntimeAdapter,
}

pub(crate) type DetachedDeliveryRoute =
    Result<Arc<meerkat_runtime::MeerkatMachine>, DetachedDeliveryUnavailable>;
