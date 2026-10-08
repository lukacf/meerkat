//! Mechanical projection from the job-owned outbox into the runtime-owned
//! durable delivery inbox.

use std::sync::Arc;

use meerkat_jobs::{
    DetachedJobError, DetachedJobService, DetachedJobStore, InteractionLineageId, JobDeliveryKind,
    JobId, JobNotification, JobOutboxEntry, JobOutboxPayload, JobSubscription, JobTerminalResult,
};
use meerkat_runtime::{
    LogicalRuntimeId, RuntimeDeliveryError, RuntimeDeliveryId, RuntimeDeliveryInbox,
    RuntimeDeliveryKind, RuntimeDeliveryRecipient, RuntimeDeliveryRecipientGroupOutcome,
    RuntimeDeliveryRecipientOutcome, RuntimeDeliveryRecipientState, RuntimeDeliveryRecord,
    RuntimeDeliverySubmission,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobTerminalDeliveryPayload {
    pub job_id: JobId,
    pub delivery_sequence: u64,
    pub origin_session_id: meerkat_core::SessionId,
    pub interaction_lineage_id: InteractionLineageId,
    pub targets: Vec<JobSubscription>,
    pub terminal_result: JobTerminalResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobNotificationDeliveryPayload {
    pub job_id: JobId,
    pub delivery_sequence: u64,
    pub origin_session_id: meerkat_core::SessionId,
    pub interaction_lineage_id: InteractionLineageId,
    pub targets: Vec<JobSubscription>,
    pub notification: JobNotification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobDeliveryContent {
    Notification(JobNotification),
    Terminal(JobTerminalResult),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobDeliveryApplication {
    Record {
        job_id: JobId,
        delivery_sequence: u64,
        subscription: JobSubscription,
        content: JobDeliveryContent,
    },
    Notification {
        job_id: JobId,
        delivery_sequence: u64,
        subscription: JobSubscription,
        content: JobDeliveryContent,
    },
    Event {
        job_id: JobId,
        delivery_sequence: u64,
        subscription: JobSubscription,
        interaction_lineage_id: InteractionLineageId,
        handling_mode: meerkat_core::HandlingMode,
        content: JobDeliveryContent,
    },
}

/// Typed disposition of one recipient application. Refusal is distinct from
/// unavailable authority and failed observation; a caller must not recover
/// that distinction by parsing a diagnostic string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobDeliveryApplyError {
    #[error(transparent)]
    Authorization(#[from] meerkat_core::OperationAuthorizationError),
    /// Required review settled locally before entry. Keep its exact reason
    /// here; the runtime recipient outcome is the coarser `Refused`.
    #[error(transparent)]
    Review(#[from] meerkat_core::approval::review::OperationReviewRefusal),
    /// Another runtime owner hosts the recipient's session (#1813): the
    /// recipient is skipped, not settled, and stays pending for that owner.
    #[error("session {session_id} is served by another runtime owner on this realm")]
    ServedElsewhere { session_id: meerkat_core::SessionId },
    /// The recipient's session cannot take its cross-process hosting claim
    /// (#1813): nothing was applied without it. The recipient is skipped,
    /// not settled, and stays pending until a claim can be taken.
    #[error("hosting claim for session {session_id} is unavailable")]
    HostingUnavailable { session_id: meerkat_core::SessionId },
    #[error("{0}")]
    Infrastructure(String),
}

#[async_trait::async_trait]
pub trait JobDeliverySink: Send + Sync {
    /// Apply one stable subscription delivery idempotently.
    ///
    /// `Notification` is a turn-free user-visible append. Only `Event` may
    /// request ordinary runtime work/provider inference.
    /// An authorization refusal means the recipient effect did not enter.
    /// It does not undo an already completed producer job or its wait binding.
    /// Infrastructure and observation failures never imply permission denial
    /// or prove that an effect did not happen.
    async fn apply(&self, application: JobDeliveryApplication)
    -> Result<(), JobDeliveryApplyError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedRuntimeJobDelivery {
    pub delivery_id: RuntimeDeliveryId,
    pub runtime_sequence: u64,
    pub applications: usize,
}

/// A completed recipient group that was not wholly applied. These are exact
/// runtime-owned dispositions, not a claim that the producer job failed or
/// that a refused recipient's effect ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocallySettledRuntimeJobDelivery {
    pub runtime_id: LogicalRuntimeId,
    pub delivery_id: RuntimeDeliveryId,
    pub runtime_sequence: u64,
    pub outcome: RuntimeDeliveryRecipientGroupOutcome,
    pub recipients: Vec<RuntimeDeliveryRecipientState>,
}

/// A pending delivery whose application failed during a drain pass.
///
/// The row is never discarded: it stays pending in the runtime inbox and is
/// retried on the next pass. Because the inbox is an ordered cursor machine,
/// later rows for the same runtime stay queued behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedRuntimeJobDelivery {
    pub delivery_id: RuntimeDeliveryId,
    pub runtime_sequence: u64,
    pub reason: BlockedDeliveryReason,
    pub error: String,
}

/// Why a pending delivery is blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlockedDeliveryReason {
    /// Its application failed; it is retried on the next wake for its
    /// runtime.
    ApplyFailed,
    /// No handler for its delivery kind is armed on this owner. Visible, and
    /// never treated as corruption.
    UnsupportedKind,
}

/// A continuation delivery settled as refused this pass: a terminal policy
/// outcome, never applied. The cursor passed it, so the rows after it
/// proceeded. Job deliveries settle per recipient instead
/// ([`LocallySettledRuntimeJobDelivery`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedRuntimeJobDelivery {
    pub delivery_id: RuntimeDeliveryId,
    pub runtime_sequence: u64,
    pub reason: meerkat_runtime::RuntimeDeliveryRefusalReason,
}

/// Outcome of one ordered drain pass over a runtime's pending deliveries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeJobDeliveryDrain {
    /// Deliveries applied and acknowledged this pass, in inbox order.
    pub applied: Vec<AppliedRuntimeJobDelivery>,
    /// Groups fully settled with at least one local refusal or unavailable
    /// authorization. Their cursor progress is distinct from applied effects.
    pub locally_settled: Vec<LocallySettledRuntimeJobDelivery>,
    /// The first delivery whose application failed, if any. Progress made
    /// before it is retained in `applied`, `locally_settled` and `refused`.
    pub blocked: Option<BlockedRuntimeJobDelivery>,
    /// Continuation deliveries settled as refused this pass, in inbox order.
    /// A refusal is progress: the drain continues past it.
    pub refused: Vec<RefusedRuntimeJobDelivery>,
    /// The first delivery with recipients another process must apply (or the
    /// cold-delivery owner, for recipients no process hosts), if any. Not a
    /// failure: this process applied and settled what it serves, and the row
    /// stays pending, in order, for the others.
    pub awaiting_other_hosts: Option<AwaitingRuntimeJobDelivery>,
}

/// The blocked report of `record`, whose application failed with `error`.
fn blocked_delivery(
    record: &RuntimeDeliveryRecord,
    error: &JobOutboxProjectionError,
) -> BlockedRuntimeJobDelivery {
    let reason = match error {
        JobOutboxProjectionError::UnsupportedKind(_) => BlockedDeliveryReason::UnsupportedKind,
        _ => BlockedDeliveryReason::ApplyFailed,
    };
    BlockedRuntimeJobDelivery {
        delivery_id: record.submission.delivery_id().clone(),
        runtime_sequence: record.sequence,
        reason,
        error: error.to_string(),
    }
}

/// A pending delivery waiting on recipients this process does not serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwaitingRuntimeJobDelivery {
    pub delivery_id: RuntimeDeliveryId,
    pub runtime_sequence: u64,
    /// The sessions of the recipients this pass skipped: the delivery owner
    /// re-routes them on its store-watch ticks.
    pub skipped_recipients: Vec<meerkat_core::SessionId>,
    /// Whether this pass issued any delivery-authority input for the row. A
    /// pass whose recipients were all served elsewhere issues none.
    pub touched_authority: bool,
}

/// What applying one continuation row achieved (#1813).
#[cfg(not(target_arch = "wasm32"))]
enum ContinuationProgress {
    Applied,
    /// Left untouched for the host of this session.
    AwaitingOtherHost(meerkat_core::SessionId),
}

/// What applying one record achieved.
enum RecordProgress {
    Completed(
        RuntimeDeliveryRecipientGroupOutcome,
        Vec<RuntimeDeliveryRecipientState>,
    ),
    AwaitingOtherHosts {
        skipped_recipients: Vec<meerkat_core::SessionId>,
        touched_authority: bool,
    },
}

impl RuntimeJobDeliveryDrain {
    pub fn is_fully_drained(&self) -> bool {
        self.blocked.is_none()
    }
}

/// Where one recipient's application goes, as decided by the host (#1813).
///
/// The host classifies from its runtime owner's claim registry only; it never
/// takes a session's lock to probe it.
#[derive(Clone)]
pub enum DeliveryRoute {
    /// The host's runtime owner hosts the recipient's session (its lineage
    /// holds the session's hosting claim): apply through the sink, then
    /// settle.
    ServedHere(Arc<dyn JobDeliverySink>),
    /// Another runtime owner of this process hosts the recipient's session:
    /// the recipient is skipped, neither applied nor settled, with no
    /// delivery-authority input, and stays pending for that owner.
    ServedElsewhere,
    /// No runtime owner of this process holds the recipient's claim (another
    /// process may). A cold delivery: only the store's cold-delivery owner
    /// applies it, after taking the session's claim through
    /// [`JobDeliveryRouter::claim_cold`] before any delivery-authority input;
    /// any other process skips it.
    Unserved(Arc<dyn JobDeliverySink>),
}

/// Routes each recipient's application by the recipient's own session.
#[async_trait::async_trait]
pub trait JobDeliveryRouter: Send + Sync {
    /// The route for `recipient`, or `None` once the host is gone.
    async fn route(&self, recipient: &meerkat_core::SessionId) -> Option<DeliveryRoute>;

    /// Take the hosting claim of an [`DeliveryRoute::Unserved`] recipient's
    /// session for a cold delivery by this process: `Ok(claim)` to apply
    /// under it, or [`meerkat_runtime::HostingRefused`] when another runtime
    /// owner holds it or the claim is unavailable. `None` once the host is
    /// gone.
    async fn claim_cold(
        &self,
        recipient: &meerkat_core::SessionId,
    ) -> Option<Result<meerkat_runtime::HostingClaim, meerkat_runtime::HostingRefused>>;
}

/// Every recipient is served here, through one sink: a single-process
/// applier.
struct SoleSinkRouter {
    sink: Arc<dyn JobDeliverySink>,
    owner: meerkat_runtime::HostingOwner,
}

#[async_trait::async_trait]
impl JobDeliveryRouter for SoleSinkRouter {
    async fn route(&self, _recipient: &meerkat_core::SessionId) -> Option<DeliveryRoute> {
        Some(DeliveryRoute::ServedHere(Arc::clone(&self.sink)))
    }

    async fn claim_cold(
        &self,
        recipient: &meerkat_core::SessionId,
    ) -> Option<Result<meerkat_runtime::HostingClaim, meerkat_runtime::HostingRefused>> {
        // A single-process applier has no other host: the claim is
        // process-local.
        Some(meerkat_runtime::grant_session_hosting(
            &meerkat_runtime::HostingCapability::ProcessLocal,
            &self.owner,
            recipient,
        ))
    }
}

/// A recipient this pass applies: its sink, and the cold delivery's hosting
/// claim, held from before the recipients are bound until the row's
/// application ends.
struct ApplicableRecipient {
    sink: Arc<dyn JobDeliverySink>,
    _cold_claim: Option<meerkat_runtime::HostingClaim>,
}

#[derive(Clone)]
pub struct JobRuntimeDeliveryApplier {
    runtime_inbox: RuntimeDeliveryInbox,
    router: Arc<dyn JobDeliveryRouter>,
    /// Whether this process applies cold deliveries (recipients no process
    /// hosts): it is the store's cold-delivery owner, or the only process.
    applies_cold_deliveries: bool,
    #[cfg(not(target_arch = "wasm32"))]
    continuations: Option<(
        Arc<dyn crate::ContinuationDeliverySink>,
        meerkat_core::SessionId,
        Option<Arc<dyn crate::RetainedJobSource>>,
    )>,
}

impl std::fmt::Debug for JobRuntimeDeliveryApplier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRuntimeDeliveryApplier")
            .finish_non_exhaustive()
    }
}

impl JobRuntimeDeliveryApplier {
    /// A single-process applier: every recipient is applied through `sink`.
    pub fn new(runtime_inbox: RuntimeDeliveryInbox, sink: Arc<dyn JobDeliverySink>) -> Self {
        Self {
            runtime_inbox,
            router: Arc::new(SoleSinkRouter {
                sink,
                owner: meerkat_runtime::HostingOwner::mint(),
            }),
            applies_cold_deliveries: true,
            #[cfg(not(target_arch = "wasm32"))]
            continuations: None,
        }
    }

    /// A multi-process applier (#1813): each recipient is routed by the
    /// hosting of its own session. Recipients another runtime owner serves
    /// are skipped without any delivery-authority input; cold recipients are
    /// applied only when `applies_cold_deliveries`, under the session's
    /// claim taken first.
    pub fn routed(
        runtime_inbox: RuntimeDeliveryInbox,
        router: Arc<dyn JobDeliveryRouter>,
        applies_cold_deliveries: bool,
    ) -> Self {
        Self {
            runtime_inbox,
            router,
            applies_cold_deliveries,
            #[cfg(not(target_arch = "wasm32"))]
            continuations: None,
        }
    }

    /// Also admit continuation rows, into `session` through `sink`.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn with_continuations(
        mut self,
        sink: Arc<dyn crate::ContinuationDeliverySink>,
        session: meerkat_core::SessionId,
        job_source: Option<Arc<dyn crate::RetainedJobSource>>,
    ) -> Self {
        self.continuations = Some((sink, session, job_source));
        self
    }

    /// Apply pending deliveries in inbox order, failing SAFE per runtime.
    ///
    /// `Err` means the inbox itself could not be read. Corrupt payloads,
    /// unsupported kinds, infrastructure or observation failures, and failed
    /// persistence are reported as `blocked` in an `Ok` drain. Typed local
    /// authorization refusal or unavailability settles only that recipient;
    /// successful siblings and later rows can continue.
    ///
    /// Within one runtime the drain stops at an unresolved recipient. Every
    /// completed recipient is durably settled before the next sink call, and
    /// retry skips its immutable outcome. Sink effect and settlement are not
    /// atomic: an unknown effect still requires sink idempotency or external
    /// reconciliation before retry. The generated owner derives the completed
    /// group and advances the cursor only after all recipients have settled.
    ///
    /// A continuation row ([`Self::with_continuations`]) has one owner and
    /// settles whole: applied, or settled as refused for a terminal policy
    /// reason (`refused`), which the cursor passes.
    pub async fn apply_pending(
        &self,
        runtime_id: &LogicalRuntimeId,
        limit: usize,
    ) -> Result<RuntimeJobDeliveryDrain, JobOutboxProjectionError> {
        let pending = self.runtime_inbox.list_pending(runtime_id, limit).await?;
        // Rows whose effect already reached the runtime out of band (the
        // shell's completion projection). Their sink must not run again; the
        // inbox consumes them when the cursor reaches them, which happens
        // when the preceding row finishes.
        let acknowledged = self
            .runtime_inbox
            .acknowledged_pending_sequences(runtime_id)
            .await?;
        let mut drain = RuntimeJobDeliveryDrain {
            applied: Vec::with_capacity(pending.len()),
            locally_settled: Vec::new(),
            blocked: None,
            refused: Vec::new(),
            awaiting_other_hosts: None,
        };
        for record in pending {
            if acknowledged.contains(&record.sequence) {
                drain.applied.push(AppliedRuntimeJobDelivery {
                    delivery_id: record.submission.delivery_id().clone(),
                    runtime_sequence: record.sequence,
                    applications: 0,
                });
                continue;
            }
            // A continuation row has one owner and settles whole: applied,
            // or refused for a terminal policy reason.
            #[cfg(not(target_arch = "wasm32"))]
            if record.submission.kind() == RuntimeDeliveryKind::Continuation {
                match self.apply_continuation(runtime_id, &record).await {
                    Ok(ContinuationProgress::Applied) => {
                        drain.applied.push(AppliedRuntimeJobDelivery {
                            delivery_id: record.submission.delivery_id().clone(),
                            runtime_sequence: record.sequence,
                            applications: 1,
                        });
                    }
                    // #1813: its session is another host's (or cold while
                    // this process may not apply it): in order, untouched.
                    Ok(ContinuationProgress::AwaitingOtherHost(session)) => {
                        drain.awaiting_other_hosts = Some(AwaitingRuntimeJobDelivery {
                            delivery_id: record.submission.delivery_id().clone(),
                            runtime_sequence: record.sequence,
                            skipped_recipients: vec![session],
                            touched_authority: false,
                        });
                        break;
                    }
                    Err(JobOutboxProjectionError::DeliveryHostGone) => {
                        return Err(JobOutboxProjectionError::DeliveryHostGone);
                    }
                    Err(JobOutboxProjectionError::Refused { reason, .. }) => {
                        // A terminal policy outcome: settle it at the cursor
                        // and continue, so it never holds the rows behind it.
                        let settled = self
                            .runtime_inbox
                            .mark_refused(
                                runtime_id,
                                record.submission.delivery_id(),
                                record.sequence,
                                reason,
                            )
                            .await;
                        if let Err(error) = settled {
                            drain.blocked = Some(BlockedRuntimeJobDelivery {
                                delivery_id: record.submission.delivery_id().clone(),
                                runtime_sequence: record.sequence,
                                reason: BlockedDeliveryReason::ApplyFailed,
                                error: format!("refused delivery could not be settled: {error}"),
                            });
                            break;
                        }
                        drain.refused.push(RefusedRuntimeJobDelivery {
                            delivery_id: record.submission.delivery_id().clone(),
                            runtime_sequence: record.sequence,
                            reason,
                        });
                    }
                    Err(error) => {
                        drain.blocked = Some(blocked_delivery(&record, &error));
                        break;
                    }
                }
                continue;
            }
            match self.apply_record(runtime_id, &record).await {
                Ok(RecordProgress::AwaitingOtherHosts {
                    skipped_recipients,
                    touched_authority,
                }) => {
                    // In order: no later row of this runtime may complete
                    // before this one.
                    drain.awaiting_other_hosts = Some(AwaitingRuntimeJobDelivery {
                        delivery_id: record.submission.delivery_id().clone(),
                        runtime_sequence: record.sequence,
                        skipped_recipients,
                        touched_authority,
                    });
                    break;
                }
                Ok(RecordProgress::Completed(
                    RuntimeDeliveryRecipientGroupOutcome::AllApplied,
                    recipients,
                )) => {
                    drain.applied.push(AppliedRuntimeJobDelivery {
                        delivery_id: record.submission.delivery_id().clone(),
                        runtime_sequence: record.sequence,
                        applications: recipients.len(),
                    });
                }
                Ok(RecordProgress::Completed(outcome, recipients)) => {
                    drain
                        .locally_settled
                        .push(LocallySettledRuntimeJobDelivery {
                            runtime_id: runtime_id.clone(),
                            delivery_id: record.submission.delivery_id().clone(),
                            runtime_sequence: record.sequence,
                            outcome,
                            recipients,
                        });
                }
                Err(JobOutboxProjectionError::DeliveryHostGone) => {
                    return Err(JobOutboxProjectionError::DeliveryHostGone);
                }
                Err(error) => {
                    drain.blocked = Some(blocked_delivery(&record, &error));
                    break;
                }
            }
        }
        Ok(drain)
    }

    /// Admit one continuation row into the session serving its owner and
    /// acknowledge it. A terminal policy refusal comes back as
    /// [`JobOutboxProjectionError::Refused`] for the drain to settle.
    ///
    /// The row's one recipient is that session, routed by its hosting like a
    /// job recipient (#1813) BEFORE any reservation or admission: served by
    /// another runtime owner, or cold while this process is not the
    /// cold-delivery owner (or its claim is refused), the row is left
    /// untouched for its host. A cold admission holds the session's claim,
    /// taken first, until it returns.
    #[cfg(not(target_arch = "wasm32"))]
    async fn apply_continuation(
        &self,
        runtime_id: &LogicalRuntimeId,
        record: &RuntimeDeliveryRecord,
    ) -> Result<ContinuationProgress, JobOutboxProjectionError> {
        let Some((sink, session, job_source)) = self.continuations.as_ref() else {
            return Err(JobOutboxProjectionError::UnsupportedKind(
                record.submission.delivery_id().to_string(),
            ));
        };
        let _cold_claim = match self
            .router
            .route(session)
            .await
            .ok_or(JobOutboxProjectionError::DeliveryHostGone)?
        {
            DeliveryRoute::ServedHere(_) => None,
            DeliveryRoute::Unserved(_) if self.applies_cold_deliveries => {
                match self
                    .router
                    .claim_cold(session)
                    .await
                    .ok_or(JobOutboxProjectionError::DeliveryHostGone)?
                {
                    Ok(claim) => Some(claim),
                    Err(_) => return Ok(ContinuationProgress::AwaitingOtherHost(session.clone())),
                }
            }
            DeliveryRoute::Unserved(_) | DeliveryRoute::ServedElsewhere => {
                return Ok(ContinuationProgress::AwaitingOtherHost(session.clone()));
            }
        };
        crate::continuation::admit_continuation_row(
            &self.runtime_inbox,
            runtime_id,
            record,
            &**sink,
            session,
            job_source.as_deref(),
        )
        .await?;
        self.runtime_inbox
            .mark_applied(runtime_id, record.submission.delivery_id(), record.sequence)
            .await?;
        Ok(ContinuationProgress::Applied)
    }

    async fn apply_record(
        &self,
        runtime_id: &LogicalRuntimeId,
        record: &RuntimeDeliveryRecord,
    ) -> Result<RecordProgress, JobOutboxProjectionError> {
        let applications = match record.submission.kind() {
            RuntimeDeliveryKind::JobNotification => {
                let payload: JobNotificationDeliveryPayload =
                    serde_json::from_slice(record.submission.payload()).map_err(|error| {
                        JobOutboxProjectionError::Corrupt(format!(
                            "notification delivery {} payload is invalid: {error}",
                            record.submission.delivery_id()
                        ))
                    })?;
                subscription_applications(
                    payload.job_id,
                    payload.delivery_sequence,
                    payload.origin_session_id,
                    payload.interaction_lineage_id,
                    payload.targets,
                    JobDeliveryContent::Notification(payload.notification),
                )?
            }
            RuntimeDeliveryKind::JobTerminal => {
                let payload: JobTerminalDeliveryPayload =
                    serde_json::from_slice(record.submission.payload()).map_err(|error| {
                        JobOutboxProjectionError::Corrupt(format!(
                            "terminal delivery {} payload is invalid: {error}",
                            record.submission.delivery_id()
                        ))
                    })?;
                subscription_applications(
                    payload.job_id,
                    payload.delivery_sequence,
                    payload.origin_session_id,
                    payload.interaction_lineage_id,
                    payload.targets,
                    JobDeliveryContent::Terminal(payload.terminal_result),
                )?
            }
            _ => {
                return Err(JobOutboxProjectionError::UnsupportedKind(
                    record.submission.delivery_id().to_string(),
                ));
            }
        };
        // Bind the complete immutable manifest before any sink effect. Exact
        // subscription bytes include the target, delivery kind and mode; an
        // ID by itself cannot silently bind a different subscription on retry.
        let recipients = applications
            .iter()
            .map(|application| {
                let subscription = match application {
                    JobDeliveryApplication::Record { subscription, .. }
                    | JobDeliveryApplication::Notification { subscription, .. }
                    | JobDeliveryApplication::Event { subscription, .. } => subscription,
                };
                Ok(RuntimeDeliveryRecipient::new(
                    subscription.subscription_id().as_str(),
                    serde_json::to_string(subscription)
                        .map_err(|error| JobOutboxProjectionError::Encode(error.to_string()))?,
                )?)
            })
            .collect::<Result<Vec<_>, JobOutboxProjectionError>>()?;
        // #1813: route every recipient by ITS session's hosting, and take
        // the claim of every cold recipient this process applies, before any
        // delivery-authority input. A row with nothing this process may apply
        // (every recipient served by another owner, or cold while another
        // process is the cold-delivery owner or holds the session) is skipped
        // without binding or settling.
        let mut routes = Vec::with_capacity(applications.len());
        for application in &applications {
            let recipient = application_recipient(application);
            let route = self
                .router
                .route(recipient)
                .await
                .ok_or(JobOutboxProjectionError::DeliveryHostGone)?;
            let applicable = match route {
                DeliveryRoute::ServedHere(sink) => Some(ApplicableRecipient {
                    sink,
                    _cold_claim: None,
                }),
                DeliveryRoute::Unserved(sink) if self.applies_cold_deliveries => {
                    match self
                        .router
                        .claim_cold(recipient)
                        .await
                        .ok_or(JobOutboxProjectionError::DeliveryHostGone)?
                    {
                        Ok(claim) => Some(ApplicableRecipient {
                            sink,
                            _cold_claim: Some(claim),
                        }),
                        // Another process hosts it (the Unserved-to-hosted
                        // race), or its claim is unavailable: settled at the
                        // claim with zero input, never applied unclaimed.
                        Err(
                            meerkat_runtime::HostingRefused::ServedElsewhere(_)
                            | meerkat_runtime::HostingRefused::Unavailable(_),
                        ) => None,
                    }
                }
                DeliveryRoute::Unserved(_) | DeliveryRoute::ServedElsewhere => None,
            };
            routes.push(applicable);
        }
        if routes.iter().all(Option::is_none) {
            return Ok(RecordProgress::AwaitingOtherHosts {
                skipped_recipients: applications
                    .iter()
                    .map(|application| application_recipient(application).clone())
                    .collect(),
                touched_authority: false,
            });
        }
        let mut states = self
            .runtime_inbox
            .bind_recipients(runtime_id, record, &recipients)
            .await?;
        let mut skipped_recipients = Vec::new();
        for ((application, state), applicable) in
            applications.into_iter().zip(&mut states).zip(&routes)
        {
            if state.outcome.is_some() {
                continue;
            }
            let Some(applicable) = applicable else {
                skipped_recipients.push(application_recipient(&application).clone());
                continue;
            };
            let outcome = match applicable.sink.apply(application).await {
                Ok(()) => RuntimeDeliveryRecipientOutcome::Applied,
                Err(JobDeliveryApplyError::Authorization(
                    meerkat_core::OperationAuthorizationError::Refused(_),
                )) => RuntimeDeliveryRecipientOutcome::Refused,
                Err(JobDeliveryApplyError::Review(_)) => RuntimeDeliveryRecipientOutcome::Refused,
                Err(JobDeliveryApplyError::Authorization(
                    meerkat_core::OperationAuthorizationError::Unavailable,
                )) => RuntimeDeliveryRecipientOutcome::OperationAuthorizationUnavailable,
                // Another runtime owner took the recipient's session after
                // routing, or its claim is unavailable. Skip it: not settled,
                // not blocked.
                Err(
                    JobDeliveryApplyError::ServedElsewhere { session_id }
                    | JobDeliveryApplyError::HostingUnavailable { session_id },
                ) => {
                    skipped_recipients.push(session_id);
                    continue;
                }
                Err(error) => return Err(JobOutboxProjectionError::Apply(error)),
            };
            *state = self
                .runtime_inbox
                .settle_recipient(runtime_id, record, &state.recipient, outcome)
                .await?;
        }
        if !skipped_recipients.is_empty() {
            return Ok(RecordProgress::AwaitingOtherHosts {
                skipped_recipients,
                touched_authority: true,
            });
        }
        let outcome = self
            .runtime_inbox
            .finish_recipients(runtime_id, record)
            .await?;
        Ok(RecordProgress::Completed(outcome, states))
    }
}

/// The session one application is delivered to.
fn application_recipient(application: &JobDeliveryApplication) -> &meerkat_core::SessionId {
    match application {
        JobDeliveryApplication::Record { subscription, .. }
        | JobDeliveryApplication::Notification { subscription, .. }
        | JobDeliveryApplication::Event { subscription, .. } => subscription.session_id(),
    }
}

/// Idempotency key of one subscription's application of one delivery.
fn job_delivery_idempotency_key(
    job_id: &JobId,
    delivery_sequence: u64,
    subscription: &JobSubscription,
) -> String {
    format!(
        "job:{job_id}:{delivery_sequence}:{}",
        subscription.subscription_id()
    )
}

/// The ordered System message a `Notification` subscription delivery appends:
/// turn-free, and idempotent per (job, delivery sequence, subscription).
pub fn job_delivery_notification_request(
    job_id: &JobId,
    delivery_sequence: u64,
    subscription: &JobSubscription,
    content: &JobDeliveryContent,
) -> meerkat_core::service::AppendSystemContextRequest {
    let text = match content {
        JobDeliveryContent::Notification(notification) => format!(
            "Detached job {job_id}: {}\n\n{}",
            notification.title(),
            notification.body()
        ),
        JobDeliveryContent::Terminal(result) => {
            format!("Detached job {job_id} reached terminal state: {result:?}")
        }
    };
    let mut request = meerkat_core::service::AppendSystemContextRequest::from_text(text);
    request.source = Some(format!("detached_job:{job_id}"));
    request.idempotency_key = Some(job_delivery_idempotency_key(
        job_id,
        delivery_sequence,
        subscription,
    ));
    request
}

/// The runtime input an `Event` subscription delivery admits: a durable
/// external event under the subscription's handling mode, keyed per (job,
/// delivery sequence, subscription) so a replayed delivery is deduplicated by
/// the runtime, and correlated with the job's interaction lineage.
pub fn job_delivery_event_input(
    job_id: &JobId,
    delivery_sequence: u64,
    subscription: &JobSubscription,
    interaction_lineage_id: &InteractionLineageId,
    handling_mode: meerkat_core::HandlingMode,
    content: &JobDeliveryContent,
) -> meerkat_runtime::Input {
    let (event_type, content_value) = match content {
        JobDeliveryContent::Notification(notification) => (
            "job.notification",
            serde_json::json!({
                "kind": "notification",
                "notification": notification,
            }),
        ),
        JobDeliveryContent::Terminal(result) => (
            "job.terminal",
            serde_json::json!({
                "kind": "terminal",
                "result": result,
            }),
        ),
    };
    let payload = serde_json::json!({
        "job_id": job_id.to_string(),
        "delivery_sequence": delivery_sequence,
        "content": content_value,
    });
    meerkat_runtime::Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
        objective_id: None,
        header: meerkat_runtime::InputHeader {
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: meerkat_runtime::InputOrigin::External {
                source_name: event_type.to_string(),
            },
            durability: meerkat_runtime::InputDurability::Durable,
            visibility: meerkat_runtime::InputVisibility::default(),
            idempotency_key: Some(meerkat_runtime::IdempotencyKey::new(
                job_delivery_idempotency_key(job_id, delivery_sequence, subscription),
            )),
            supersession_key: None,
            correlation_id: uuid::Uuid::parse_str(interaction_lineage_id.as_str())
                .ok()
                .map(meerkat_runtime::CorrelationId::from_uuid),
            authority_association: None,
            ingress_context: None,
            retained_resume: None,
        },
        event_type: event_type.to_string(),
        payload,
        blocks: None,
        handling_mode,
        render_metadata: None,
    })
}

fn subscription_applications(
    job_id: JobId,
    delivery_sequence: u64,
    origin_session_id: meerkat_core::SessionId,
    interaction_lineage_id: InteractionLineageId,
    mut targets: Vec<JobSubscription>,
    content: JobDeliveryContent,
) -> Result<Vec<JobDeliveryApplication>, JobOutboxProjectionError> {
    if targets.is_empty() {
        targets.push(JobSubscription::new(
            meerkat_jobs::JobSubscriptionId::new("origin")
                .map_err(JobOutboxProjectionError::Job)?,
            origin_session_id,
            JobDeliveryKind::Notification,
        ));
    }
    let mut applications = Vec::with_capacity(targets.len());
    for subscription in targets {
        let delivery = subscription.delivery().clone();
        let application = match delivery {
            JobDeliveryKind::Record => JobDeliveryApplication::Record {
                job_id: job_id.clone(),
                delivery_sequence,
                subscription,
                content: content.clone(),
            },
            JobDeliveryKind::Notification => JobDeliveryApplication::Notification {
                job_id: job_id.clone(),
                delivery_sequence,
                subscription,
                content: content.clone(),
            },
            JobDeliveryKind::Event { handling_mode } => JobDeliveryApplication::Event {
                job_id: job_id.clone(),
                delivery_sequence,
                subscription,
                interaction_lineage_id: interaction_lineage_id.clone(),
                handling_mode,
                content: content.clone(),
            },
        };
        applications.push(application);
    }
    Ok(applications)
}

/// Job id and origin session named by a job-sourced runtime delivery row.
/// `None` for a kind this module does not produce.
fn pending_delivery_provenance(
    record: &RuntimeDeliveryRecord,
) -> Result<Option<(JobId, meerkat_core::SessionId)>, JobOutboxProjectionError> {
    let payload = record.submission.payload();
    match record.submission.kind() {
        RuntimeDeliveryKind::JobTerminal => {
            let decoded: JobTerminalDeliveryPayload = serde_json::from_slice(payload)
                .map_err(|error| JobOutboxProjectionError::Corrupt(error.to_string()))?;
            Ok(Some((decoded.job_id, decoded.origin_session_id)))
        }
        RuntimeDeliveryKind::JobNotification => {
            let decoded: JobNotificationDeliveryPayload = serde_json::from_slice(payload)
                .map_err(|error| JobOutboxProjectionError::Corrupt(error.to_string()))?;
            Ok(Some((decoded.job_id, decoded.origin_session_id)))
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Clone)]
pub struct PreparedJobDelivery {
    pub runtime_id: LogicalRuntimeId,
    pub submission: RuntimeDeliverySubmission,
    /// The row's effect is applied by the job's producer
    /// ([`meerkat_jobs::JobTerminalApplication::Producer`] terminal), so it is
    /// committed already acknowledged.
    pub producer_applied: bool,
}

impl PreparedJobDelivery {
    /// Commit this delivery into `inbox`, acknowledged when its producer
    /// applies it.
    pub async fn submit(
        self,
        inbox: &RuntimeDeliveryInbox,
    ) -> Result<meerkat_runtime::RuntimeDeliveryReceipt, RuntimeDeliveryError> {
        if self.producer_applied {
            inbox
                .submit_acknowledged(&self.runtime_id, self.submission)
                .await
        } else {
            inbox.submit(&self.runtime_id, self.submission).await
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedJobDelivery {
    pub job_id: JobId,
    pub delivery_sequence: u64,
    pub runtime_sequence: u64,
    pub runtime_deduplicated: bool,
}

/// A pending outbox entry whose projection failed during a pass.
///
/// The entry is never acknowledged on failure: it stays pending in the job
/// outbox and is retried on the next pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedJobOutboxEntry {
    pub job_id: JobId,
    pub delivery_sequence: u64,
    pub error: String,
}

/// Outcome of one projection pass over the pending job outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOutboxProjectionPass {
    /// Entries handed to the runtime inbox and acknowledged, in outbox order.
    pub projected: Vec<ProjectedJobDelivery>,
    /// First failing entry per poisoned job. Later pending entries of the
    /// same job are held back for the pass to preserve per-job delivery
    /// order; entries of other jobs keep projecting.
    pub skipped: Vec<SkippedJobOutboxEntry>,
}

impl JobOutboxProjectionPass {
    pub fn is_fully_projected(&self) -> bool {
        self.skipped.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JobOutboxProjectionError {
    #[error(transparent)]
    Job(#[from] DetachedJobError),
    #[error(transparent)]
    Runtime(#[from] RuntimeDeliveryError),
    #[error("job outbox projection is corrupt: {0}")]
    Corrupt(String),
    #[error("failed to encode job delivery: {0}")]
    Encode(String),
    #[error("failed to apply job delivery: {0}")]
    Apply(#[source] JobDeliveryApplyError),
    #[error("no handler for the kind of delivery {0} is armed")]
    UnsupportedKind(String),
    /// A terminal policy outcome for one continuation delivery, as opposed to
    /// a failure: the drain settles it as refused and continues.
    #[error("delivery {delivery_id} is refused: {reason:?}")]
    Refused {
        delivery_id: String,
        reason: meerkat_runtime::RuntimeDeliveryRefusalReason,
    },
    /// The delivery host is gone: the owner stops.
    #[error("the job delivery host is gone")]
    DeliveryHostGone,
}

#[derive(Clone)]
pub struct JobOutboxProjector {
    job_store: Arc<dyn DetachedJobStore>,
    job_service: DetachedJobService,
    runtime_inbox: RuntimeDeliveryInbox,
    realm_id: Option<String>,
}

impl std::fmt::Debug for JobOutboxProjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobOutboxProjector").finish_non_exhaustive()
    }
}

impl JobOutboxProjector {
    pub fn new(job_store: Arc<dyn DetachedJobStore>, runtime_inbox: RuntimeDeliveryInbox) -> Self {
        Self {
            job_service: DetachedJobService::new(job_store.clone()),
            job_store,
            runtime_inbox,
            realm_id: None,
        }
    }

    /// Bind projection authority to one realm.
    ///
    /// Stores are normally realm-scoped already, but this explicit filter
    /// keeps shared/test providers from projecting or acknowledging another
    /// realm's outbox through the current runtime.
    pub fn new_for_realm(
        job_store: Arc<dyn DetachedJobStore>,
        runtime_inbox: RuntimeDeliveryInbox,
        realm_id: impl Into<String>,
    ) -> Self {
        Self::new(job_store, runtime_inbox).bound_to_realm(realm_id)
    }

    /// Rebind projection authority to `realm_id`, replacing any prior binding.
    ///
    /// Used at per-session build time so the projector realm is always the
    /// realm the session actually builds under (mob members build under
    /// `mob.<mob_id>`, not the persistence manifest realm).
    #[must_use]
    pub fn bound_to_realm(mut self, realm_id: impl Into<String>) -> Self {
        self.realm_id = Some(realm_id.into());
        self
    }

    /// Origin sessions holding committed-but-unapplied runtime deliveries for
    /// jobs this projector owns.
    ///
    /// The population comes from the runtime delivery authority (every
    /// runtime with rows past its applied cursor), not from job rows. A
    /// session whose jobs fell outside a bounded job-row read still holds its
    /// pending rows and must still be drained. Ownership is decided by the
    /// provenance of the runtime's first pending row: its job must exist and
    /// belong to this projector's realm. A runtime whose first row is foreign,
    /// of an unknown kind, or names a missing job is left untouched for its
    /// own owner, exactly as before.
    pub async fn sessions_with_pending_deliveries(
        &self,
    ) -> Result<Vec<meerkat_core::SessionId>, JobOutboxProjectionError> {
        let runtimes = self
            .runtime_inbox
            .runtimes_with_pending_deliveries()
            .await?;
        self.sessions_for_runtimes(&runtimes).await
    }

    /// Origin sessions this projector owns among `runtimes`, by the same
    /// provenance rule as [`Self::sessions_with_pending_deliveries`]. A
    /// runtime with no pending row is skipped.
    pub async fn sessions_for_runtimes(
        &self,
        runtimes: &[LogicalRuntimeId],
    ) -> Result<Vec<meerkat_core::SessionId>, JobOutboxProjectionError> {
        let mut sessions = Vec::new();
        for runtime_id in runtimes {
            let Some(first) = self
                .runtime_inbox
                .list_pending(runtime_id, 1)
                .await?
                .into_iter()
                .next()
            else {
                continue;
            };
            let Some((job_id, origin_session_id)) = pending_delivery_provenance(&first)? else {
                continue;
            };
            if &LogicalRuntimeId::for_session(&origin_session_id) != runtime_id {
                continue;
            }
            let Some(job) = self.job_store.get(&job_id).await? else {
                continue;
            };
            if self.owns_job(&job) {
                sessions.push(origin_session_id);
            }
        }
        Ok(sessions)
    }

    fn owns_job(&self, job: &meerkat_jobs::StoredJob) -> bool {
        self.realm_id
            .as_deref()
            .is_none_or(|realm_id| job.spec.realm_id == realm_id)
    }

    pub async fn prepare(
        &self,
        entry: &JobOutboxEntry,
    ) -> Result<PreparedJobDelivery, JobOutboxProjectionError> {
        let job = self.job_store.get(&entry.job_id).await?.ok_or_else(|| {
            JobOutboxProjectionError::Corrupt(format!(
                "outbox entry points to missing job {}",
                entry.job_id
            ))
        })?;
        if !self.owns_job(&job) {
            return Err(JobOutboxProjectionError::Corrupt(format!(
                "job {} belongs to realm {}, outside projector realm {}",
                entry.job_id,
                job.spec.realm_id,
                self.realm_id.as_deref().unwrap_or("<unscoped>")
            )));
        }
        let persisted = job
            .outbox
            .iter()
            .find(|candidate| candidate.delivery_sequence == entry.delivery_sequence)
            .ok_or_else(|| {
                JobOutboxProjectionError::Corrupt(format!(
                    "job {} no longer contains delivery {}",
                    entry.job_id, entry.delivery_sequence
                ))
            })?;
        if persisted != entry {
            return Err(JobOutboxProjectionError::Corrupt(format!(
                "job {} delivery {} disagrees with the pending outbox projection",
                entry.job_id, entry.delivery_sequence
            )));
        }
        if entry.applied {
            return Err(JobOutboxProjectionError::Corrupt(format!(
                "job {} delivery {} is already acknowledged",
                entry.job_id, entry.delivery_sequence
            )));
        }

        let (kind, payload) = match &entry.payload {
            JobOutboxPayload::Terminal(terminal_result) => (
                RuntimeDeliveryKind::JobTerminal,
                serde_json::to_vec(&JobTerminalDeliveryPayload {
                    job_id: entry.job_id.clone(),
                    delivery_sequence: entry.delivery_sequence,
                    origin_session_id: job.spec.origin_session_id.clone(),
                    interaction_lineage_id: job.spec.interaction_lineage_id.clone(),
                    targets: entry.targets.clone(),
                    terminal_result: terminal_result.clone(),
                }),
            ),
            JobOutboxPayload::Notification(notification) => (
                RuntimeDeliveryKind::JobNotification,
                serde_json::to_vec(&JobNotificationDeliveryPayload {
                    job_id: entry.job_id.clone(),
                    delivery_sequence: entry.delivery_sequence,
                    origin_session_id: job.spec.origin_session_id.clone(),
                    interaction_lineage_id: job.spec.interaction_lineage_id.clone(),
                    targets: entry.targets.clone(),
                    notification: notification.clone(),
                }),
            ),
        };
        let payload =
            payload.map_err(|error| JobOutboxProjectionError::Encode(error.to_string()))?;
        let delivery_id = RuntimeDeliveryId::new(entry.runtime_delivery_id())?;
        let submission = RuntimeDeliverySubmission::new(
            delivery_id,
            kind,
            entry.job_id.as_str(),
            entry.delivery_sequence,
            job.spec.interaction_lineage_id.as_str(),
            payload,
        )?;
        Ok(PreparedJobDelivery {
            runtime_id: LogicalRuntimeId::for_session(&job.spec.origin_session_id),
            submission,
            // Interim routing choice until the generated driver declares it
            // (#1762): reads only immutable admission data (the job spec).
            producer_applied: matches!(entry.payload, JobOutboxPayload::Terminal(_))
                && job.spec.terminal_application == meerkat_jobs::JobTerminalApplication::Producer,
        })
    }

    /// Project pending outbox entries, failing SAFE per job.
    ///
    /// `Err` means the pending outbox itself could not be listed. Any
    /// failure on an individual entry is reported in `skipped` instead of
    /// aborting the pass, so one poisoned outbox row cannot head-of-line
    /// block every other job's delivery projection forever. A failed entry
    /// is never acknowledged — it stays pending and retries next pass — and
    /// the rest of that job's entries are held back for the pass so per-job
    /// delivery order is preserved.
    pub async fn project_pending(
        &self,
        limit: usize,
    ) -> Result<JobOutboxProjectionPass, JobOutboxProjectionError> {
        let entries = self.job_store.list_pending_outbox(limit).await?;
        let mut pass = JobOutboxProjectionPass {
            projected: Vec::with_capacity(entries.len()),
            skipped: Vec::new(),
        };
        let mut poisoned_jobs = std::collections::BTreeSet::new();
        for entry in entries {
            if poisoned_jobs.contains(&entry.job_id) {
                continue;
            }
            match self.project_entry(&entry).await {
                Ok(Some(delivery)) => pass.projected.push(delivery),
                Ok(None) => {}
                Err(error) => {
                    pass.skipped.push(SkippedJobOutboxEntry {
                        job_id: entry.job_id.clone(),
                        delivery_sequence: entry.delivery_sequence,
                        error: error.to_string(),
                    });
                    poisoned_jobs.insert(entry.job_id);
                }
            }
        }
        Ok(pass)
    }

    /// Project one pending entry; `Ok(None)` means the entry belongs to a
    /// realm outside this projector's authority.
    async fn project_entry(
        &self,
        entry: &JobOutboxEntry,
    ) -> Result<Option<ProjectedJobDelivery>, JobOutboxProjectionError> {
        if self.realm_id.is_some() {
            let Some(job) = self.job_store.get(&entry.job_id).await? else {
                return Err(JobOutboxProjectionError::Corrupt(format!(
                    "outbox entry points to missing job {}",
                    entry.job_id
                )));
            };
            if !self.owns_job(&job) {
                return Ok(None);
            }
        }
        let prepared = self.prepare(entry).await?;
        let runtime = prepared.submit(&self.runtime_inbox).await?;
        self.job_service
            .mark_delivery_applied(&entry.job_id, entry.delivery_sequence)
            .await?;
        Ok(Some(ProjectedJobDelivery {
            job_id: entry.job_id.clone(),
            delivery_sequence: entry.delivery_sequence,
            runtime_sequence: runtime.sequence,
            runtime_deduplicated: runtime.deduplicated,
        }))
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl meerkat_tools::builtin::shell::ShellJobDeliveryProjector for JobOutboxProjector {
    async fn project_job(&self, job_id: &str) -> Result<(), String> {
        let job_id = JobId::new(job_id).map_err(|error| error.to_string())?;
        loop {
            let Some(job) = self
                .job_store
                .get(&job_id)
                .await
                .map_err(|error| error.to_string())?
            else {
                return Err(format!("cannot project missing job {job_id}"));
            };
            let Some(entry) = job.outbox.iter().find(|entry| !entry.applied).cloned() else {
                return Ok(());
            };
            let prepared = self
                .prepare(&entry)
                .await
                .map_err(|error| error.to_string())?;
            // A producer-applied terminal (the shell's) is committed already
            // acknowledged, whichever of the shell and the delivery owner
            // projects it first. Monitor notifications are applied by the
            // delivery owner.
            prepared
                .submit(&self.runtime_inbox)
                .await
                .map_err(|error| error.to_string())?;
            if let Err(error) = self
                .job_service
                .mark_delivery_applied(&entry.job_id, entry.delivery_sequence)
                .await
            {
                let applied_by_racer = self
                    .job_store
                    .get(&job_id)
                    .await
                    .map_err(|reload_error| reload_error.to_string())?
                    .is_some_and(|current| {
                        current.outbox.iter().any(|candidate| {
                            candidate.delivery_sequence == entry.delivery_sequence
                                && candidate.applied
                        })
                    });
                if !applied_by_racer {
                    return Err(error.to_string());
                }
            }
        }
    }

    async fn acknowledge_applied(&self, job_id: &str) -> Result<(), String> {
        let job_id = JobId::new(job_id).map_err(|error| error.to_string())?;
        let Some(job) = self
            .job_store
            .get(&job_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Err(format!("cannot acknowledge missing job {job_id}"));
        };
        if !self.owns_job(&job) {
            return Err(format!(
                "cannot acknowledge job {job_id} outside projector realm {}",
                self.realm_id.as_deref().unwrap_or("<unscoped>")
            ));
        }
        let runtime_id = LogicalRuntimeId::for_session(&job.spec.origin_session_id);
        let pending = self
            .runtime_inbox
            .list_pending(&runtime_id, usize::MAX)
            .await
            .map_err(|error| error.to_string())?;
        let Some(record) = pending
            .into_iter()
            .find(|record| record.submission.delivery_id().as_str() == job_id.as_str())
        else {
            return Ok(());
        };
        // Acknowledge rather than apply: when an earlier row of this runtime
        // (a monitor notification, or another job's terminal) is still
        // pending, the acknowledgement is recorded and consumed when the
        // cursor reaches it, instead of failing out of order and leaving this
        // row to block every later delivery for the session.
        self.runtime_inbox
            .acknowledge(
                &runtime_id,
                record.submission.delivery_id(),
                record.sequence,
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
