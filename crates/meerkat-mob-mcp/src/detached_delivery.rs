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

/// How many times a delivery waits for an owner whose revival is deferred
/// before it gives up (the restart re-link then recovers the completion).
pub(crate) const MAX_LIVE_OWNER_REVIVAL_WAITS: u32 = 16;

/// [`deliver_detached_completion_to_member`], waiting out a deferred owner
/// revival: while the owner's mob is not running, or a lifecycle operation
/// on the owner is in progress, wait until that clears and deliver again.
/// Waiting ends when the mob completes or is destroyed (the error is then
/// returned) or after a bounded number of waits. The job's idempotency key
/// keeps the record single however often delivery runs.
#[allow(clippy::too_many_arguments)]
pub async fn deliver_detached_completion_to_member_when_revivable(
    runtime: &meerkat_runtime::MeerkatMachine,
    owner: &meerkat_mob::MobHandle,
    owner_identity: &meerkat_mob::AgentIdentity,
    owner_session_id: &SessionId,
    tool: &'static str,
    job_id: &str,
    status: BackgroundJobTerminalStatus,
    outcome: serde_json::Value,
) -> Result<DetachedCompletionDelivered, DetachedCompletionError> {
    let mut attempt = 0_u32;
    loop {
        let result = deliver_detached_completion_to_member(
            runtime,
            owner,
            owner_identity,
            owner_session_id,
            tool,
            job_id,
            status,
            outcome.clone(),
        )
        .await;
        let Err(DetachedCompletionError::OwnerRevivalDeferred { reason, .. }) = &result else {
            return result;
        };
        if attempt >= MAX_LIVE_OWNER_REVIVAL_WAITS || !reason.cleared(owner, attempt).await {
            return result;
        }
        attempt = attempt.saturating_add(1);
    }
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
/// resumed itself. Without a hook, a completion for a retired plain session
/// is not admitted live (it arrives through the restart re-link).
#[async_trait::async_trait]
pub trait DetachedOwnerHost: Send + Sync {
    /// Make `session_id` live in the runtime: attach its executor, and
    /// materialize the session from its durable record if it has no live
    /// actor. A session that is already live is left as it is.
    async fn ensure_owner_live(&self, session_id: &SessionId) -> Result<(), DetachedOwnerError>;
}

/// Deliver to an owner that is a plain session (not a mob member). When the
/// runtime refuses because the session is not live, `host` makes it live and
/// the delivery is retried once, as [`deliver_detached_completion_to_member`]
/// does for a member. Without a host the runtime's refusal is returned.
#[allow(clippy::too_many_arguments)]
pub async fn deliver_detached_completion_to_session(
    runtime: &meerkat_runtime::MeerkatMachine,
    host: Option<&dyn DetachedOwnerHost>,
    owner_session_id: &SessionId,
    tool: &'static str,
    job_id: &str,
    status: BackgroundJobTerminalStatus,
    outcome: serde_json::Value,
) -> Result<DetachedCompletionDelivered, DetachedCompletionError> {
    let first = deliver_detached_completion(
        runtime,
        owner_session_id,
        tool,
        job_id,
        status,
        outcome.clone(),
    )
    .await;
    match (first, host) {
        (Err(DetachedCompletionError::Runtime { .. }), Some(host)) => {
            host.ensure_owner_live(owner_session_id)
                .await
                .map_err(|error| match error {
                    DetachedOwnerError::OwnerGone { detail } => {
                        DetachedCompletionError::OwnerGone { tool, detail }
                    }
                    DetachedOwnerError::Failed { detail } => DetachedCompletionError::Runtime {
                        tool,
                        detail: format!("reviving the owner session failed: {detail}"),
                    },
                })?;
            deliver_detached_completion(runtime, owner_session_id, tool, job_id, status, outcome)
                .await
        }
        (other, _) => other,
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A host that answers every revival with a fixed verdict and counts
    /// the calls.
    struct FixedOwnerHost {
        verdict: Result<(), DetachedOwnerError>,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl DetachedOwnerHost for FixedOwnerHost {
        async fn ensure_owner_live(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), DetachedOwnerError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.verdict.clone()
        }
    }

    async fn deliver_to_unknown_session(
        host: Option<&dyn DetachedOwnerHost>,
    ) -> Result<DetachedCompletionDelivered, DetachedCompletionError> {
        // The runtime has never had this session: exactly what it answers
        // for a plain session whose idle executor it retired.
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        deliver_detached_completion_to_session(
            &runtime,
            host,
            &SessionId::new(),
            "council",
            "job-1",
            BackgroundJobTerminalStatus::Completed,
            serde_json::json!({"summary": "done"}),
        )
        .await
    }

    /// Without a host, a plain-session owner the runtime does not have live
    /// keeps today's behaviour: the runtime's refusal, for the caller to log
    /// and the restart re-link to recover.
    #[tokio::test]
    async fn without_an_owner_host_a_session_the_runtime_lacks_is_refused() {
        assert!(matches!(
            deliver_to_unknown_session(None).await,
            Err(DetachedCompletionError::Runtime { .. })
        ));
    }

    /// With a host, the runtime's refusal asks the host to revive the owner:
    /// a session the host reports gone settles as the typed OwnerGone, and a
    /// revival the host could not do stays a retryable runtime error.
    #[tokio::test]
    async fn an_owner_host_is_asked_once_and_its_verdict_is_typed() {
        let gone = FixedOwnerHost {
            verdict: Err(DetachedOwnerError::OwnerGone {
                detail: "archived".to_string(),
            }),
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        assert!(matches!(
            deliver_to_unknown_session(Some(&gone)).await,
            Err(DetachedCompletionError::OwnerGone {
                tool: "council",
                ..
            })
        ));
        assert_eq!(gone.calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let failed = FixedOwnerHost {
            verdict: Err(DetachedOwnerError::Failed {
                detail: "materialization failed".to_string(),
            }),
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        assert!(matches!(
            deliver_to_unknown_session(Some(&failed)).await,
            Err(DetachedCompletionError::Runtime { detail, .. })
                if detail.contains("reviving the owner session failed")
        ));
        assert_eq!(failed.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
