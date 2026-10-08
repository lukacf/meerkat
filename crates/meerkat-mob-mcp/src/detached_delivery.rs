//! Delivery of a detached job's outcome (fork_off, council) to its owner.
//!
//! The outcome is recorded once as a durable `BackgroundJob` system notice
//! (`SystemNoticeBlock::BackgroundJob { persisted: true, .. }`) in the owner's
//! transcript. It is submitted as a continuation
//! ([`crate::detached_completion_sink`]) whose applied input is a runtime
//! prompt input with that notice as its only content, a typed append with no
//! user text:
//!
//! - an idle owner gets a real pending boundary and runs one turn that sees
//!   the outcome;
//! - a running owner takes it as a durable steer: it joins the running turn
//!   and the turn's next model call sees the outcome; only if the turn ends
//!   before another model call does the owner get one follow-up turn;
//! - the input's idempotency key names the job (`{tool}:{job_id}`), so the
//!   record is admitted and written exactly once however often delivery runs.
//!
//! A system notice (not a role=System message) is what every provider accepts
//! mid-conversation.

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

/// Job `job_id`'s completion input, when it was already admitted to its
/// owner, read from the runtime's durable input index (never live state). A
/// job with an admitted completion is over. `None` when there is no durable
/// evidence, including on a store-less runtime.
///
/// While the input is pending, its retained payload
/// (`state.persisted_input`) carries the admitted record; once the owner's
/// commit retires that payload, the record is in the owner's transcript.
pub(crate) async fn admitted_detached_completion(
    runtime: &meerkat_runtime::MeerkatMachine,
    owner_session_id: &SessionId,
    tool: &str,
    job_id: &str,
) -> Option<meerkat_runtime::input_state::StoredInputState> {
    use meerkat_runtime::SessionServiceRuntimeExt as _;
    runtime
        .durable_input_state_by_idempotency_key(
            owner_session_id,
            &detached_completion_key(tool, job_id),
        )
        .await
        .ok()
        .flatten()
}

/// The completion record an admitted completion input carries in its
/// retained payload: the `BackgroundJob` notice blocks of its typed append.
/// Empty once the payload is retired.
pub(crate) fn admitted_completion_notice_blocks(
    admitted: &meerkat_runtime::input_state::StoredInputState,
) -> impl Iterator<Item = &meerkat_core::types::SystemNoticeBlock> {
    let appends = match admitted.state.persisted_input.as_ref() {
        Some(meerkat_runtime::Input::Prompt(prompt)) => prompt.typed_turn_appends.as_slice(),
        _ => &[],
    };
    appends.iter().flat_map(|append| match &append.content {
        meerkat_core::lifecycle::run_primitive::CoreRenderable::SystemNotice { blocks, .. } => {
            blocks.as_slice()
        }
        _ => &[],
    })
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

/// Why a host could not make a non-member owner session live.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DetachedOwnerError {
    /// The owner session no longer exists (archived or deleted): a
    /// completion for it can never be delivered.
    #[error("the owner session is gone: {detail}")]
    OwnerGone { detail: String },
    /// The host could not make the session live now.
    #[error("the owner session could not be made live: {detail}")]
    Failed { detail: String },
}

/// Host hook that makes a detached job's owner live in the runtime when the
/// owner is a plain session, not a mob member (a top-level RPC, REST or CLI
/// session that called `council`).
///
/// A mob member owner is revived through its mob. A plain session is
/// materialized by the host that serves it: the runtime retires an idle
/// session's executor routinely, and only the host knows how to attach its
/// executor again. Hosts implement this with the same path their own next
/// turn takes, so a revived owner is indistinguishable from one the host
/// resumed itself. The host's delivery owner calls it when it serves a
/// continuation for the session. Without a hook, a completion for a retired
/// plain session waits until the session is attached again.
#[async_trait::async_trait]
pub trait DetachedOwnerHost: Send + Sync {
    /// Make `session_id` live in the runtime: attach its executor, and
    /// materialize the session from its durable record if it has no live
    /// actor. A session that is already live is left as it is.
    async fn ensure_owner_live(&self, session_id: &SessionId) -> Result<(), DetachedOwnerError>;
}

/// Who receives a detached job's completion.
#[derive(Clone)]
pub(crate) enum DetachedCompletionOwner {
    /// A mob member, addressed by its incarnation and revived through its
    /// mob.
    Member(meerkat_mob::MobHandle, meerkat_mob::AgentIdentity),
    /// A plain session, revived through the host's owner hook.
    Session,
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
    /// The caller's session could not be made live to receive the result
    /// later: it is not a member of a mob this host manages (whose mob would
    /// revive it) and the host installed no [`DetachedOwnerHost`].
    NoOwnerRevivalHost,
    /// The host never bound its runtime delivery inbox, so an outcome cannot
    /// be submitted durably for later delivery.
    NoContinuationOwner,
}

pub(crate) type DetachedDeliveryRoute =
    Result<crate::detached_completion_sink::DetachedCompletionSink, DetachedDeliveryUnavailable>;
