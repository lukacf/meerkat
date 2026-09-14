//! Native committed-store realization of the canonical Live request DSL.

use std::sync::Arc;

use meerkat_core::SessionId;

use crate::live_ledger::transcript::LiveHeadReference;
use crate::live_ledger::write::{LiveLedgerCommitOutcome, PreparedLiveLedgerCommit};
use crate::store::{
    MachineLifecycleObservationVersion, RuntimeStore, RuntimeStoreError, RuntimeStoreWriteFence,
    RuntimeStoreWriteFenceOutcome, execute_runtime_store_write_fence,
};

use super::dsl;

#[path = "admission.rs"]
mod admission;
#[path = "callback_application.rs"]
pub(in crate::live_ledger) mod callback_application;
#[path = "callback_credits.rs"]
pub(in crate::live_ledger) mod callback_credits;
#[path = "cancellation.rs"]
pub(crate) mod cancellation;
#[path = "claim.rs"]
pub(in crate::live_ledger) mod claim;
#[path = "effect_credits.rs"]
pub(in crate::live_ledger) mod effect_credits;
#[path = "recovery.rs"]
pub(crate) mod recovery;
#[path = "request_completion.rs"]
pub(crate) mod request_completion;
#[path = "request_credits.rs"]
pub(in crate::live_ledger) mod request_credits;
#[path = "restore.rs"]
mod restore;
#[path = "settlement.rs"]
pub(crate) mod settlement;
#[path = "source_reservation.rs"]
pub(in crate::live_ledger) mod source_reservation;
#[path = "stage.rs"]
mod stage;
pub(crate) use admission::{
    CommittedLiveAdmission, LiveAdmissionRuntimeBinding, PendingLiveAdmission,
};
pub(crate) use stage::LiveInputStageError;

pub(crate) struct LiveRequestStoreOwner {
    store: Arc<dyn RuntimeStore>,
    session_id: SessionId,
    clock: Arc<dyn Fn() -> Result<u64, RuntimeStoreError> + Send + Sync>,
}

/// A committed generated transition is not itself a physical effect permit.
/// Run admission and effect-specific completion reservations are separate
/// handoffs; callers cannot manufacture this receipt from a snapshot.
pub(crate) struct CommittedLiveRequestTransition {
    pub(in crate::live_ledger) head: LiveHeadReference,
    pub(in crate::live_ledger) transition: dsl::LiveRequestMachineTransition,
}

impl LiveRequestStoreOwner {
    #[cfg(feature = "live")]
    pub(crate) async fn fence_for_archive(
        &self,
        registration: Option<&crate::RuntimeSessionRegistrationWitness>,
    ) -> Result<Option<LiveHeadReference>, LiveRequestAuthorityError> {
        if registration.is_some_and(|witness| witness.session_id() != &self.session_id) {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        let ops = self
            .store
            .live_ledger_ops()
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        let mut attempts_remaining = 8;
        loop {
            let before = ops.load_live_head(&self.session_id).await?;
            if !ops.ledger_write_profile().supports_lifecycle_fence() {
                return Err(LiveRequestAuthorityError::Unsupported);
            }
            if let Some(before) = &before {
                before.validate_payload()?;
            }
            let runtime_id = crate::LogicalRuntimeId::for_session(&self.session_id);
            let lifecycle = self.store.observe_machine_lifecycle(&runtime_id).await?;
            if !matches!(
                lifecycle,
                crate::store::MachineLifecycleObservation::Missing
                    | crate::store::MachineLifecycleObservation::Decoded { .. }
            ) {
                return Err(RuntimeStoreError::ReadFailed(
                    "Live archive cannot interpret the retained runtime lifecycle".into(),
                )
                .into());
            }
            let actor = if before.is_none()
                && registration.is_none()
                && matches!(
                    lifecycle,
                    crate::store::MachineLifecycleObservation::Missing
                ) {
                self.store
                    .load_session_boundary_authority(&runtime_id)
                    .await?
            } else {
                None
            };
            if before.is_none()
                && registration.is_none()
                && actor.is_none()
                && matches!(
                    lifecycle,
                    crate::store::MachineLifecycleObservation::Missing
                )
            {
                return Ok(None);
            }
            let initial;
            let payload = match &before {
                Some(head) => &head.payload,
                None => {
                    initial = PreparedLiveLedgerCommit::initial_head(&self.session_id)?;
                    &initial.payload
                }
            };
            let owner = dsl::LiveRequestMachineAuthority::recover_from_state(
                crate::generated::live_request_state::decode(&payload.request_snapshot)?,
            )?;
            let input = dsl::LiveRequestInput::CloseIngress;
            let mut request = owner.prepare_authority();
            dsl::LiveRequestMachineMutator::apply(&mut request, input.clone())?;
            use crate::live_ledger::transcript_authority::dsl as transcript;
            let transcript_owner = transcript::LiveTranscriptMachineAuthority::recover_from_state(
                crate::generated::live_transcript_state::decode(&payload.transcript_snapshot)?,
            )
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            let mut transcript = transcript_owner.prepare_authority();
            transcript::LiveTranscriptMachineMutator::apply(
                &mut transcript,
                transcript::LiveTranscriptInput::CloseCurrentIngress,
            )
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            let head = if let Some(before) = &before
                && request.state() == owner.state()
                && transcript.state() == transcript_owner.state()
            {
                before.reference.clone()
            } else {
                let prepared = PreparedLiveLedgerCommit::from_request_transition(
                    &self.session_id,
                    before.as_ref(),
                    &request,
                )?
                .with_transcript_transition(&transcript)?
                .with_archive_existence_fence(lifecycle.expected_version(), actor);
                let fence = Arc::new(LiveRequestTimeFence {
                    predecessor: owner,
                    input,
                    expected_snapshot: Arc::clone(&prepared.successor().payload.request_snapshot),
                    clock: Arc::clone(&self.clock),
                });
                match ops.commit_live_ledger(prepared, fence).await? {
                    LiveLedgerCommitOutcome::Committed { head }
                    | LiveLedgerCommitOutcome::AlreadyCommitted { head } => head,
                    LiveLedgerCommitOutcome::Conflict { .. } if attempts_remaining > 1 => {
                        attempts_remaining -= 1;
                        tracing::debug!(session_id = %self.session_id, attempts_remaining,
                            "repreparing uncommitted Live archive fence after head advancement");
                        continue;
                    }
                    outcome => {
                        return Err(LiveRequestAuthorityError::NotNewlyCommitted(Box::new(
                            outcome,
                        )));
                    }
                }
            };
            for source in request.state().source_requests.keys() {
                self.cancel_source(meerkat_core::live_execution::request::LiveRequestCancelIntent {
                    source: serde_json::from_str(source)?,
                    reason: meerkat_core::live_execution::request::LiveRequestCancellationReason::SessionArchived,
                }).await?;
            }
            return Ok(Some(head));
        }
    }

    pub(crate) fn new(store: Arc<dyn RuntimeStore>, session_id: SessionId) -> Self {
        Self {
            store,
            session_id,
            clock: Arc::new(|| {
                u64::try_from(chrono::Utc::now().timestamp_millis()).map_err(|_| {
                    RuntimeStoreError::WriteFailed(
                        "Live authority clock is outside the supported domain".into(),
                    )
                })
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_clock(
        mut self,
        clock: Arc<dyn Fn() -> Result<u64, RuntimeStoreError> + Send + Sync>,
    ) -> Self {
        self.clock = clock;
        self
    }

    #[cfg(test)]
    pub(crate) async fn commit(
        &self,
        input: dsl::LiveRequestInput,
        fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<CommittedLiveRequestTransition, LiveRequestAuthorityError> {
        self.commit_inner(input, None, fence).await
    }

    pub(crate) async fn commit_for_runtime(
        &self,
        input: dsl::LiveRequestInput,
        lifecycle: MachineLifecycleObservationVersion,
        fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<CommittedLiveRequestTransition, LiveRequestAuthorityError> {
        self.commit_inner(input, Some(lifecycle), fence).await
    }

    async fn commit_inner(
        &self,
        mut input: dsl::LiveRequestInput,
        lifecycle: Option<MachineLifecycleObservationVersion>,
        fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<CommittedLiveRequestTransition, LiveRequestAuthorityError> {
        let ops = self
            .store
            .live_ledger_ops()
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        if lifecycle.is_some() && !ops.ledger_write_profile().supports_lifecycle_fence() {
            return Err(LiveRequestAuthorityError::Unsupported);
        }
        let before = ops.load_live_head(&self.session_id).await?;
        let owner = match &before {
            Some(head) => {
                if head.reference.session_id != self.session_id {
                    return Err(LiveRequestAuthorityError::SessionMismatch);
                }
                head.validate_payload()?;
                let state =
                    crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
                dsl::LiveRequestMachineAuthority::recover_from_state(state)?
            }
            None => dsl::LiveRequestMachineAuthority::new(),
        };
        refresh_command_time(&mut input, self.clock.as_ref())?;
        let mut candidate = owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, input.clone())?;
        let mut prepared = PreparedLiveLedgerCommit::from_request_transition(
            &self.session_id,
            before.as_ref(),
            &candidate,
        )?;
        if let Some(lifecycle) = lifecycle {
            prepared = prepared.with_lifecycle_fence(lifecycle);
        }
        let fence = Arc::new(CurrentLiveRequestFence {
            time: LiveRequestTimeFence {
                predecessor: owner,
                input,
                expected_snapshot: Arc::clone(&prepared.successor().payload.request_snapshot),
                clock: Arc::clone(&self.clock),
            },
            registration: fence,
        });
        match ops.commit_live_ledger(prepared, fence).await? {
            LiveLedgerCommitOutcome::Committed { head } => {
                Ok(CommittedLiveRequestTransition { head, transition })
            }
            outcome => Err(LiveRequestAuthorityError::NotNewlyCommitted(Box::new(
                outcome,
            ))),
        }
    }
}

struct CurrentLiveRequestFence {
    time: LiveRequestTimeFence,
    registration: Arc<dyn RuntimeStoreWriteFence>,
}

/// The backend holds exact head/input/lifecycle comparisons. This guard only
/// refreshes clock-dependent generated decisions at that publication point.
struct LiveRequestTimeFence {
    predecessor: dsl::LiveRequestMachineAuthority,
    input: dsl::LiveRequestInput,
    expected_snapshot: Arc<Vec<u8>>,
    clock: Arc<dyn Fn() -> Result<u64, RuntimeStoreError> + Send + Sync>,
}

impl RuntimeStoreWriteFence for CurrentLiveRequestFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        execute_runtime_store_write_fence(self.registration.as_ref(), || {
            self.time.publish(operation)
        })
    }
}

impl RuntimeStoreWriteFence for LiveRequestTimeFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        self.publish(operation)?;
        Ok(RuntimeStoreWriteFenceOutcome::Applied)
    }
}

impl LiveRequestTimeFence {
    fn publish(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<(), RuntimeStoreError> {
        self.revalidate()
            .map_err(|error| RuntimeStoreError::LiveRequestPublicationRejected {
                reason: error.to_string(),
            })?;
        operation()
    }

    fn revalidate(&self) -> Result<(), RuntimeStoreError> {
        // The backend has already compared the exact predecessor under its
        // write lock. Refresh only clock facts; the DSL still decides.
        let mut input = self.input.clone();
        refresh_command_time(&mut input, self.clock.as_ref())?;
        let mut candidate = self.predecessor.prepare_authority();
        dsl::LiveRequestMachineMutator::apply(&mut candidate, input).map_err(|error| {
            RuntimeStoreError::WriteFailed(format!(
                "Live request authority changed before publication: {error}"
            ))
        })?;
        let snapshot = crate::generated::live_request_state::encode(candidate.state())
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if snapshot != *self.expected_snapshot {
            return Err(RuntimeStoreError::WriteFailed(
                "Live request authority changed before publication: successor differs".into(),
            ));
        }
        Ok(())
    }
}

fn refresh_command_time(
    input: &mut dsl::LiveRequestInput,
    clock: &(dyn Fn() -> Result<u64, RuntimeStoreError> + Send + Sync),
) -> Result<(), RuntimeStoreError> {
    use dsl::LiveRequestInput as Input;
    match input {
        Input::Activate { now, .. }
        | Input::Reserve { now, .. }
        | Input::Admit { now, .. }
        | Input::Stage { now, .. }
        | Input::AdmitCallbackContinuation { now, .. }
        | Input::StageCallbackContinuation { now, .. }
        | Input::ClaimCallbackApplication { now, .. }
        | Input::RestoreScope { now, .. }
        | Input::ResolveModelAttempt { now, .. }
        | Input::ClaimEffect { now, .. } => *now = clock()?,
        Input::ObserveAdmission { .. }
        | Input::ResolveInputRecovery { .. }
        | Input::ObserveCallbackContinuation { .. }
        | Input::ObserveRequestCompletion { .. }
        | Input::ObserveRunlessCompletion { .. }
        | Input::ObserveUnstagedContinuationCompletion { .. }
        | Input::ObserveEffectSettlement { .. }
        | Input::SettleEffect { .. }
        | Input::Cancel { .. }
        | Input::CancelSource { .. }
        | Input::Revoke { .. }
        | Input::FenceExecutor { .. }
        | Input::CloseIngress
        | Input::Suspend { .. }
        | Input::ObserveCallbackSuspension { .. }
        | Input::ObserveCancelledCallback { .. }
        | Input::Complete { .. }
        | Input::CompleteRunless { .. }
        | Input::CompleteUnstagedContinuation { .. } => {}
    }
    Ok(())
}

impl CommittedLiveRequestTransition {
    pub(crate) fn into_activation_commit(
        self,
        grant_id: &str,
        generation: u64,
        record: &str,
    ) -> Result<LiveHeadReference, LiveRequestAuthorityError> {
        match self.transition.effects() {
            [
                dsl::LiveRequestEffect::ActivationChanged {
                    grant_id: committed_id,
                    generation: committed_generation,
                    record: committed_record,
                },
            ] if committed_id == grant_id
                && *committed_generation == generation
                && committed_record == record =>
            {
                Ok(self.head)
            }
            _ => Err(LiveRequestAuthorityError::ActivationReceiptMismatch),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LiveRequestAuthorityError {
    #[error("the runtime store does not support the independent live ledger")]
    Unsupported,
    #[error("the committed Live request state belongs to another session")]
    SessionMismatch,
    #[error("Live run scope is not current: {0}")]
    ScopeNotCurrent(&'static str),
    #[error("invalid Live effect feedback: {0}")]
    InvalidEffectFeedback(&'static str),
    #[error("invalid Live ordinary completion: {0}")]
    InvalidOrdinaryCompletion(&'static str),
    #[error("invalid Live source cancellation: {0}")]
    InvalidSourceCancellation(&'static str),
    #[error(transparent)]
    RecoveryObservation(#[from] crate::RuntimeDriverError),
    #[error("committed Live activation does not bind the exact grant record")]
    ActivationReceiptMismatch,
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
    #[error("invalid Live request snapshot: {0}")]
    Snapshot(#[from] serde_json::Error),
    #[error(transparent)]
    Transition(#[from] dsl::LiveRequestMachineTransitionError),
    #[error("Live request transition did not acquire a new committed boundary: {0:?}")]
    NotNewlyCommitted(Box<LiveLedgerCommitOutcome>),
}
