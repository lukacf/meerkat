//! Runtime-owned sealing of generated GPT Live delegation authority.
//!
//! Provider-neutral vocabulary lives in `meerkat-core`. The generated
//! `MeerkatMachine` effects live here, so this crate is the lowest layer that
//! can turn those effects into unforgeable reconciliation and consequential
//! dispatch witnesses without exposing a public minting constructor.

use meerkat_core::exact_operation::ExactOperationIdentity;
use meerkat_core::{
    FinalLiveUserTranscriptCommitEvidence, FinalLiveUserTranscriptDisposition,
    LiveAppendDeliveryOutcome, LiveChannelId, LiveHandoffReconciliation, LiveResultDisposition,
    LiveUserTurnCorrelation, NormalizedLiveUserInputDigest, ProvisionalLiveHandoff, SessionId,
    ToolDispatchAdmission, ToolDispatchContext, ToolUnavailableReason,
};
#[cfg(feature = "live")]
use meerkat_live::{
    LiveSidebandAppendAuthority, LiveSidebandDelegationRef, LiveSidebandReleaseAuthority,
    ProviderWebrtcBinding,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
#[cfg(feature = "live")]
use std::sync::atomic::Ordering;

use crate::meerkat_machine::dsl::{
    LiveContextAppendObservation, LiveContextRowDisposition,
    LiveDelegationCancellationOutcome as DslLiveDelegationCancellationOutcome,
    LiveDelegationCancellationReason as DslLiveDelegationCancellationReason,
    LiveDelegationReconciliation,
    LiveDelegationResultDeliveryObservation as DslLiveDelegationResultDeliveryObservation,
    LiveDelegationResultDisposition,
    LiveDelegationWorkerTerminalKind as DslLiveDelegationWorkerTerminalKind, MeerkatMachineEffect,
    OperationId as DslOperationId,
};

use crate::live_context_mirror::CommittedLiveContextRow;

/// Failure while sealing a generated live-delegation effect.
///
/// Values are deliberately absent from every variant so provider correlation
/// and operation identifiers cannot leak through diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveExecutionAuthorityError {
    #[error("generated live delegation effect does not match the exact operation correlation")]
    CorrelationMismatch,
    #[error("generated live delegation reconciliation remained provisional")]
    ProvisionalReconciliation,
    #[error("consequential authority requires a confirmed final-user-input receipt")]
    FinalUserInputNotConfirmed,
    #[error("canonical final-user-input evidence does not match the exact live interaction")]
    TranscriptEvidenceMismatch,
    #[error("canonical final-user-input evidence has an invalid terminal shape")]
    InvalidTranscriptEvidence,
    #[error("generated live delegation effect disagrees with canonical transcript evidence")]
    ReconciliationMismatch,
    #[error("generated live append effect does not match the exact pre-send authority")]
    AppendAuthorityMismatch,
    #[error("generated live result delivery effect does not match the exact release authority")]
    ResultDeliveryAuthorityMismatch,
    #[error("live result text does not match the generated delivery digest")]
    ResultDeliveryDigestMismatch,
    #[error("generated live delegation admission does not match the exact operation")]
    DelegationAdmissionMismatch,
    #[error("generated live delegation worker authority does not match the exact operation")]
    DelegationWorkerAuthorityMismatch,
    #[error("generated live context authority does not match the active provider binding")]
    ProviderBindingMismatch,
    #[error("generated live release authority does not match the opaque provider delegation")]
    ProviderDelegationMismatch,
    #[error("generated live context authority was already converted for provider dispatch")]
    ProviderDispatchAlreadyConverted,
    #[error("generated assistant output handle does not match the exact playback target")]
    AssistantOutputMismatch,
    #[error("generated assistant output handle was already consumed")]
    AssistantOutputAlreadyConsumed,
    #[error("generated assistant output handle already has a terminal dispatch in flight")]
    AssistantOutputAlreadyReserved,
    #[error("live delegation tool execution admission is already terminal")]
    ToolExecutionAdmissionTerminal,
}

/// Read-only projection of the exact generated live runtime binding.
///
/// This carries no mutation authority. Every lifecycle input repeats these
/// atoms and is rejected if the generated binding has changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDelegationRuntimeBinding {
    session_id: SessionId,
    channel_id: LiveChannelId,
    runtime_id: crate::identifiers::LogicalRuntimeId,
    fence_token: u64,
    generation: u64,
}

impl LiveDelegationRuntimeBinding {
    pub(crate) fn new(
        session_id: SessionId,
        channel_id: LiveChannelId,
        runtime_id: crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
    ) -> Self {
        Self {
            session_id,
            channel_id,
            runtime_id,
            fence_token,
            generation,
        }
    }

    /// Construct an exact binding for downstream lifecycle custody tests.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[must_use]
    pub fn __test_new(
        session_id: SessionId,
        channel_id: LiveChannelId,
        runtime_id: crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
    ) -> Self {
        Self::new(session_id, channel_id, runtime_id, fence_token, generation)
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub fn runtime_id(&self) -> &crate::identifiers::LogicalRuntimeId {
        &self.runtime_id
    }

    #[must_use]
    pub const fn fence_token(&self) -> u64 {
        self.fence_token
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Clone, PartialEq, Eq)]
enum LiveToolExecutionAdmissionState {
    AwaitingFinalInput,
    Released(FinalUserInputOperationWitness),
    Closed,
}

struct LiveToolExecutionAdmissionGate {
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    state_tx: crate::tokio::sync::watch::Sender<LiveToolExecutionAdmissionState>,
}

impl LiveToolExecutionAdmissionGate {
    fn new(operation: ExactOperationIdentity<LiveUserTurnCorrelation>) -> Self {
        let (state_tx, _) =
            crate::tokio::sync::watch::channel(LiveToolExecutionAdmissionState::AwaitingFinalInput);
        Self {
            operation,
            state_tx,
        }
    }

    fn release(
        &self,
        witness: &FinalUserInputOperationWitness,
    ) -> Result<(), LiveExecutionAuthorityError> {
        if !witness.authorizes(witness.session_id(), &self.operation) {
            return Err(LiveExecutionAuthorityError::CorrelationMismatch);
        }
        match self.state_tx.borrow().clone() {
            LiveToolExecutionAdmissionState::AwaitingFinalInput => {
                self.state_tx
                    .send_replace(LiveToolExecutionAdmissionState::Released(witness.clone()));
                Ok(())
            }
            LiveToolExecutionAdmissionState::Released(existing) if existing == *witness => Ok(()),
            LiveToolExecutionAdmissionState::Released(_)
            | LiveToolExecutionAdmissionState::Closed => {
                Err(LiveExecutionAuthorityError::ToolExecutionAdmissionTerminal)
            }
        }
    }

    fn close(&self) {
        self.state_tx
            .send_replace(LiveToolExecutionAdmissionState::Closed);
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl ToolDispatchAdmission for LiveToolExecutionAdmissionGate {
    async fn await_dispatch_admission(
        &self,
        call: meerkat_core::ToolCallView<'_>,
        _context: Option<&ToolDispatchContext>,
    ) -> Result<(), meerkat_core::ToolError> {
        let mut state_rx = self.state_tx.subscribe();
        loop {
            match state_rx.borrow().clone() {
                LiveToolExecutionAdmissionState::Released(witness)
                    if witness.operation() == &self.operation =>
                {
                    return Ok(());
                }
                LiveToolExecutionAdmissionState::Released(_)
                | LiveToolExecutionAdmissionState::Closed => {
                    return Err(meerkat_core::ToolError::unavailable(
                        call.name,
                        ToolUnavailableReason::RuntimeCommandAuthorityUnavailable,
                    ));
                }
                LiveToolExecutionAdmissionState::AwaitingFinalInput => {}
            }
            if state_rx.changed().await.is_err() {
                return Err(meerkat_core::ToolError::unavailable(
                    call.name,
                    ToolUnavailableReason::RuntimeCommandAuthorityUnavailable,
                ));
            }
        }
    }
}

/// Machine-admitted GPT Live delegation execution.
///
/// The value carries exact live operation identity and a process-local tool
/// admission gate. It is sealed from `LiveDelegationAdmitted`; provider and
/// surface code cannot construct one from copied identifiers.
#[derive(Clone)]
pub struct LiveDelegationExecutionAdmission {
    session_id: SessionId,
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    worker_identity: String,
    tool_gate: Arc<LiveToolExecutionAdmissionGate>,
}

impl std::fmt::Debug for LiveDelegationExecutionAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveDelegationExecutionAdmission")
            .field("session_id", &self.session_id)
            .field("operation_id", self.operation.operation_id())
            .field("worker_identity", &self.worker_identity)
            .field(
                "channel_id",
                self.operation.domain_correlation().channel_id(),
            )
            .field(
                "interaction_id",
                &self.operation.domain_correlation().interaction_id(),
            )
            .field("provider_correlation", &"[REDACTED]")
            .finish()
    }
}

impl LiveDelegationExecutionAdmission {
    pub(crate) fn from_generated_effect(
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        provisional: &ProvisionalLiveHandoff,
        worker_identity: &str,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveDelegationWorkerStartAuthorized {
            channel_id,
            interaction_id,
            operation_id,
            worker_identity: authorized_worker_identity,
        } = effect
        else {
            return Ok(None);
        };
        let correlation = operation.domain_correlation();
        if provisional.correlation() != correlation
            || channel_id != correlation.channel_id().as_str()
            || interaction_id != &correlation.interaction_id().to_string()
            || operation_id != &DslOperationId::from_domain(operation.operation_id())
            || authorized_worker_identity != worker_identity
        {
            return Err(LiveExecutionAuthorityError::DelegationAdmissionMismatch);
        }
        Ok(Some(Self {
            session_id: session_id.clone(),
            operation: operation.clone(),
            worker_identity: worker_identity.to_string(),
            tool_gate: Arc::new(LiveToolExecutionAdmissionGate::new(operation.clone())),
        }))
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn operation(&self) -> &ExactOperationIdentity<LiveUserTurnCorrelation> {
        &self.operation
    }

    #[must_use]
    pub fn interaction_id(&self) -> meerkat_core::InteractionId {
        self.operation.domain_correlation().interaction_id()
    }

    #[must_use]
    pub fn worker_identity(&self) -> &str {
        &self.worker_identity
    }

    #[must_use]
    pub fn tool_dispatch_admission(&self) -> Arc<dyn ToolDispatchAdmission> {
        Arc::clone(&self.tool_gate) as Arc<dyn ToolDispatchAdmission>
    }

    /// Release the exact worker's tool gate with machine-minted final-input
    /// authority. This method performs identity matching only; it never
    /// classifies transcript confirmation itself.
    pub fn release_tool_execution(
        &self,
        witness: &FinalUserInputOperationWitness,
    ) -> Result<(), LiveExecutionAuthorityError> {
        if witness.session_id() != &self.session_id || witness.operation() != &self.operation {
            return Err(LiveExecutionAuthorityError::CorrelationMismatch);
        }
        self.tool_gate.release(witness)
    }

    /// Close a provisional gate after its generated operation terminalizes.
    ///
    /// Crate-visible by design: only the runtime method that consumes the
    /// generated abandon/supersede/cancel effect may close it.
    pub(crate) fn close_tool_execution_after_generated_terminal(&self) {
        self.tool_gate.close();
    }
}

/// Machine-derived reason authorizing cancellation of one exact live worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDelegationCancellationReason {
    Abandoned,
    Superseded,
    TranscriptConflict,
    TranscriptMissing,
}

impl From<DslLiveDelegationCancellationReason> for LiveDelegationCancellationReason {
    fn from(reason: DslLiveDelegationCancellationReason) -> Self {
        match reason {
            DslLiveDelegationCancellationReason::Abandoned => Self::Abandoned,
            DslLiveDelegationCancellationReason::Superseded => Self::Superseded,
            DslLiveDelegationCancellationReason::TranscriptConflict => Self::TranscriptConflict,
            DslLiveDelegationCancellationReason::TranscriptMissing => Self::TranscriptMissing,
        }
    }
}

/// Runtime-sealed authority to cancel one exact generated worker binding.
#[derive(Clone)]
pub struct LiveDelegationCancellationAuthority {
    session_id: SessionId,
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    worker_identity: String,
    reason: LiveDelegationCancellationReason,
}

impl std::fmt::Debug for LiveDelegationCancellationAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveDelegationCancellationAuthority")
            .field("operation_id", self.operation.operation_id())
            .field("worker_identity", &"[REDACTED]")
            .field("reason", &self.reason)
            .finish()
    }
}

impl LiveDelegationCancellationAuthority {
    pub(crate) fn from_generated_effect(
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        worker_identity: &str,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveDelegationCancellationAuthorized {
            channel_id,
            interaction_id,
            operation_id,
            worker_identity: authorized_worker_identity,
            reason,
            ..
        } = effect
        else {
            return Ok(None);
        };
        let correlation = operation.domain_correlation();
        if channel_id != correlation.channel_id().as_str()
            || interaction_id != &correlation.interaction_id().to_string()
            || operation_id != &DslOperationId::from_domain(operation.operation_id())
            || authorized_worker_identity != worker_identity
        {
            return Err(LiveExecutionAuthorityError::DelegationWorkerAuthorityMismatch);
        }
        Ok(Some(Self {
            session_id: session_id.clone(),
            operation: operation.clone(),
            worker_identity: worker_identity.to_string(),
            reason: (*reason).into(),
        }))
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn operation(&self) -> &ExactOperationIdentity<LiveUserTurnCorrelation> {
        &self.operation
    }

    #[must_use]
    pub fn worker_identity(&self) -> &str {
        &self.worker_identity
    }

    #[must_use]
    pub const fn reason(&self) -> LiveDelegationCancellationReason {
        self.reason
    }

    pub(crate) fn from_generated_supersession_effect(
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        worker_identity: &str,
        superseding_interaction_id: meerkat_core::InteractionId,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveDelegationCancellationAuthorized {
            reason,
            superseding_interaction_id: Some(recorded_superseding_interaction_id),
            ..
        } = effect
        else {
            return Ok(None);
        };
        if *reason != DslLiveDelegationCancellationReason::Superseded
            || recorded_superseding_interaction_id != &superseding_interaction_id.to_string()
        {
            return Err(LiveExecutionAuthorityError::DelegationWorkerAuthorityMismatch);
        }
        Self::from_generated_effect(session_id, operation, worker_identity, effect)
    }
}

/// Machine-sealed proof that supersession or abandonment does not require a
/// worker cancellation attempt for the exact operation.
#[derive(Clone)]
pub struct LiveDelegationNoCancellationReceipt {
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
}

impl std::fmt::Debug for LiveDelegationNoCancellationReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveDelegationNoCancellationReceipt")
            .field("operation_id", self.operation.operation_id())
            .finish()
    }
}

impl LiveDelegationNoCancellationReceipt {
    pub(crate) fn from_generated_supersession_effect(
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        superseding_interaction_id: meerkat_core::InteractionId,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveInteractionSupersededWithoutCancellation {
            channel_id,
            interaction_id,
            operation_id,
            superseding_interaction_id: recorded_superseding_interaction_id,
        } = effect
        else {
            return Ok(None);
        };
        let correlation = operation.domain_correlation();
        if channel_id != correlation.channel_id().as_str()
            || interaction_id != &correlation.interaction_id().to_string()
            || operation_id != &DslOperationId::from_domain(operation.operation_id())
            || recorded_superseding_interaction_id != &superseding_interaction_id.to_string()
        {
            return Err(LiveExecutionAuthorityError::DelegationWorkerAuthorityMismatch);
        }
        Ok(Some(Self {
            operation: operation.clone(),
        }))
    }

    pub(crate) fn from_generated_abandonment_effect(
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveInteractionAbandoned {
            channel_id,
            interaction_id,
        } = effect
        else {
            return Ok(None);
        };
        let correlation = operation.domain_correlation();
        if channel_id != correlation.channel_id().as_str()
            || interaction_id != &correlation.interaction_id().to_string()
        {
            return Err(LiveExecutionAuthorityError::DelegationWorkerAuthorityMismatch);
        }
        Ok(Some(Self {
            operation: operation.clone(),
        }))
    }

    #[must_use]
    pub fn operation(&self) -> &ExactOperationIdentity<LiveUserTurnCorrelation> {
        &self.operation
    }
}

/// Total machine classification of whether one lifecycle edge requires a
/// mechanical worker cancellation attempt.
#[derive(Clone, Debug)]
pub enum LiveDelegationCancellationDirective {
    CancellationAuthorized(LiveDelegationCancellationAuthority),
    NoCancellationRequired(LiveDelegationNoCancellationReceipt),
}

/// Mechanical cancellation observation reported back to the generated machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDelegationCancellationOutcome {
    Cancelled,
    AlreadyTerminal,
    Failed,
}

impl From<LiveDelegationCancellationOutcome> for DslLiveDelegationCancellationOutcome {
    fn from(outcome: LiveDelegationCancellationOutcome) -> Self {
        match outcome {
            LiveDelegationCancellationOutcome::Cancelled => Self::Cancelled,
            LiveDelegationCancellationOutcome::AlreadyTerminal => Self::AlreadyTerminal,
            LiveDelegationCancellationOutcome::Failed => Self::Failed,
        }
    }
}

/// Mechanical terminal observation for one exact delegated worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDelegationWorkerTerminalKind {
    Completed,
    Cancelled,
    Failed,
}

impl From<LiveDelegationWorkerTerminalKind> for DslLiveDelegationWorkerTerminalKind {
    fn from(terminal: LiveDelegationWorkerTerminalKind) -> Self {
        match terminal {
            LiveDelegationWorkerTerminalKind::Completed => Self::Completed,
            LiveDelegationWorkerTerminalKind::Cancelled => Self::Cancelled,
            LiveDelegationWorkerTerminalKind::Failed => Self::Failed,
        }
    }
}

/// Generated terminal classification. Late terminals are evidence only and
/// can never regain result eligibility.
#[derive(Debug, Clone)]
pub struct LiveDelegationWorkerTerminalReceipt {
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    worker_identity: String,
    terminal: LiveDelegationWorkerTerminalKind,
    late: bool,
    result_eligible: bool,
}

impl LiveDelegationWorkerTerminalReceipt {
    pub(crate) fn from_generated_effect(
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        worker_identity: &str,
        terminal: LiveDelegationWorkerTerminalKind,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveDelegationWorkerTerminalRecorded {
            channel_id,
            interaction_id,
            operation_id,
            worker_identity: recorded_worker_identity,
            terminal: recorded_terminal,
            late,
            result_eligible,
        } = effect
        else {
            return Ok(None);
        };
        let correlation = operation.domain_correlation();
        if channel_id != correlation.channel_id().as_str()
            || interaction_id != &correlation.interaction_id().to_string()
            || operation_id != &DslOperationId::from_domain(operation.operation_id())
            || recorded_worker_identity != worker_identity
            || *recorded_terminal != DslLiveDelegationWorkerTerminalKind::from(terminal)
        {
            return Err(LiveExecutionAuthorityError::DelegationWorkerAuthorityMismatch);
        }
        Ok(Some(Self {
            operation: operation.clone(),
            worker_identity: worker_identity.to_string(),
            terminal,
            late: *late,
            result_eligible: *result_eligible,
        }))
    }

    #[must_use]
    pub fn operation(&self) -> &ExactOperationIdentity<LiveUserTurnCorrelation> {
        &self.operation
    }

    #[must_use]
    pub fn worker_identity(&self) -> &str {
        &self.worker_identity
    }

    #[must_use]
    pub const fn terminal(&self) -> LiveDelegationWorkerTerminalKind {
        self.terminal
    }

    #[must_use]
    pub const fn late(&self) -> bool {
        self.late
    }

    #[must_use]
    pub const fn result_eligible(&self) -> bool {
        self.result_eligible
    }
}

/// Runtime-sealed authority to retire one exact terminal worker binding.
#[derive(Clone)]
pub struct LiveDelegationWorkerRetirementAuthority {
    session_id: SessionId,
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    worker_identity: String,
}

impl LiveDelegationWorkerRetirementAuthority {
    pub(crate) fn from_generated_effect(
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        worker_identity: &str,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveDelegationWorkerRetirementAuthorized {
            channel_id,
            interaction_id,
            operation_id,
            worker_identity: authorized_worker_identity,
        } = effect
        else {
            return Ok(None);
        };
        let correlation = operation.domain_correlation();
        if channel_id != correlation.channel_id().as_str()
            || interaction_id != &correlation.interaction_id().to_string()
            || operation_id != &DslOperationId::from_domain(operation.operation_id())
            || authorized_worker_identity != worker_identity
        {
            return Err(LiveExecutionAuthorityError::DelegationWorkerAuthorityMismatch);
        }
        Ok(Some(Self {
            session_id: session_id.clone(),
            operation: operation.clone(),
            worker_identity: worker_identity.to_string(),
        }))
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn operation(&self) -> &ExactOperationIdentity<LiveUserTurnCorrelation> {
        &self.operation
    }

    #[must_use]
    pub fn worker_identity(&self) -> &str {
        &self.worker_identity
    }
}

/// Derive the runtime classification from sealed canonical transcript evidence.
///
/// Callers cannot select `Confirmed`: exact normalized digest equality is the
/// only path to it. A committed mismatch is a material conflict, while a
/// machine-terminal missing observation remains a distinct transition.
pub(crate) fn reconciliation_from_final_transcript(
    operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
    provisional: &ProvisionalLiveHandoff,
    evidence: &FinalLiveUserTranscriptCommitEvidence,
) -> Result<LiveHandoffReconciliation, LiveExecutionAuthorityError> {
    let correlation = operation.domain_correlation();
    if provisional.correlation() != correlation
        || evidence.channel_id() != correlation.channel_id()
        || evidence.interaction_id() != correlation.interaction_id()
    {
        return Err(LiveExecutionAuthorityError::TranscriptEvidenceMismatch);
    }

    reconciliation_from_terminal_digest(
        &provisional.normalized_input_digest(),
        evidence.disposition(),
        evidence.normalized_final_input_digest(),
    )
}

fn reconciliation_from_terminal_digest(
    provisional_digest: &NormalizedLiveUserInputDigest,
    disposition: FinalLiveUserTranscriptDisposition,
    final_digest: Option<&NormalizedLiveUserInputDigest>,
) -> Result<LiveHandoffReconciliation, LiveExecutionAuthorityError> {
    match (disposition, final_digest) {
        (FinalLiveUserTranscriptDisposition::Committed, Some(final_digest)) => {
            Ok(if provisional_digest == final_digest {
                LiveHandoffReconciliation::Confirmed
            } else {
                LiveHandoffReconciliation::MaterialConflict
            })
        }
        (FinalLiveUserTranscriptDisposition::Missing, None) => {
            Ok(LiveHandoffReconciliation::Missing)
        }
        _ => Err(LiveExecutionAuthorityError::InvalidTranscriptEvidence),
    }
}

/// Machine-admitted final-user-input reconciliation for one exact operation.
///
/// The type is public so the outer dispatcher can receive it, but only this
/// crate can construct it from a generated `MeerkatMachine` effect.
#[derive(Clone, PartialEq, Eq)]
pub struct FinalLiveUserInputAdmission {
    session_id: SessionId,
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    disposition: LiveHandoffReconciliation,
    cancellation_required: bool,
}

impl std::fmt::Debug for FinalLiveUserInputAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinalLiveUserInputAdmission")
            .field("session_id", &self.session_id)
            .field("operation_id", self.operation.operation_id())
            .field(
                "channel_id",
                self.operation.domain_correlation().channel_id(),
            )
            .field(
                "interaction_id",
                &self.operation.domain_correlation().interaction_id(),
            )
            .field("provider_correlation", &"[REDACTED]")
            .field("disposition", &self.disposition)
            .field("cancellation_required", &self.cancellation_required)
            .finish()
    }
}

impl FinalLiveUserInputAdmission {
    fn from_generated_reconciliation(
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveDelegationTranscriptReconciled {
            channel_id,
            interaction_id,
            operation_id,
            reconciliation,
            cancellation_required,
        } = effect
        else {
            return Ok(None);
        };

        let correlation = operation.domain_correlation();
        if channel_id != correlation.channel_id().as_str()
            || interaction_id != &correlation.interaction_id().to_string()
            || operation_id != &DslOperationId::from_domain(operation.operation_id())
        {
            return Err(LiveExecutionAuthorityError::CorrelationMismatch);
        }

        let disposition = match reconciliation {
            LiveDelegationReconciliation::Confirmed => LiveHandoffReconciliation::Confirmed,
            LiveDelegationReconciliation::MaterialConflict => {
                LiveHandoffReconciliation::MaterialConflict
            }
            LiveDelegationReconciliation::Missing => LiveHandoffReconciliation::Missing,
            LiveDelegationReconciliation::Provisional => {
                return Err(LiveExecutionAuthorityError::ProvisionalReconciliation);
            }
        };
        if disposition == LiveHandoffReconciliation::Confirmed && *cancellation_required {
            return Err(LiveExecutionAuthorityError::ReconciliationMismatch);
        }

        Ok(Some(Self {
            session_id: session_id.clone(),
            operation: operation.clone(),
            disposition,
            cancellation_required: *cancellation_required,
        }))
    }

    #[must_use]
    pub fn operation(&self) -> &ExactOperationIdentity<LiveUserTurnCorrelation> {
        &self.operation
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn disposition(&self) -> LiveHandoffReconciliation {
        self.disposition
    }

    #[must_use]
    pub const fn cancellation_required(&self) -> bool {
        self.cancellation_required
    }
}

/// Typed reconciliation receipt sealed from generated authority.
#[derive(Clone, PartialEq, Eq)]
pub struct LiveHandoffReconciliationReceipt {
    admission: FinalLiveUserInputAdmission,
}

impl std::fmt::Debug for LiveHandoffReconciliationReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveHandoffReconciliationReceipt")
            .field("operation_id", self.admission.operation.operation_id())
            .field(
                "channel_id",
                self.admission.operation.domain_correlation().channel_id(),
            )
            .field(
                "interaction_id",
                &self
                    .admission
                    .operation
                    .domain_correlation()
                    .interaction_id(),
            )
            .field("provider_correlation", &"[REDACTED]")
            .field("disposition", &self.admission.disposition)
            .field(
                "cancellation_required",
                &self.admission.cancellation_required,
            )
            .finish()
    }
}

impl LiveHandoffReconciliationReceipt {
    /// Seal the machine's reconciliation effect against the exact staged input.
    ///
    /// This is crate-visible rather than public: provider adapters and hosts
    /// cannot construct an authority receipt from copied public identifiers.
    pub(crate) fn from_generated_effect(
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        provisional: &ProvisionalLiveHandoff,
        expected_reconciliation: LiveHandoffReconciliation,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        if provisional.correlation() != operation.domain_correlation() {
            return Err(LiveExecutionAuthorityError::CorrelationMismatch);
        }
        let admission = FinalLiveUserInputAdmission::from_generated_reconciliation(
            session_id, operation, effect,
        )?;
        if let Some(admission) = admission {
            if admission.disposition != expected_reconciliation {
                return Err(LiveExecutionAuthorityError::ReconciliationMismatch);
            }
            Ok(Some(Self { admission }))
        } else {
            Ok(None)
        }
    }

    #[must_use]
    pub fn admission(&self) -> &FinalLiveUserInputAdmission {
        &self.admission
    }

    #[must_use]
    pub const fn disposition(&self) -> LiveHandoffReconciliation {
        self.admission.disposition
    }

    /// Whether the generated reconciliation edge requires a distinct
    /// cancellation authorization for the exact still-running worker.
    #[must_use]
    pub const fn cancellation_required(&self) -> bool {
        self.admission.cancellation_required
    }
}

/// Sealed witness authorizing consequential dispatch for one exact operation.
///
/// Minting requires both a confirmed final-user-input receipt and the distinct
/// generated `LiveConsequentialEffectAuthorized` effect. The witness is not
/// serializable and exposes no public constructor.
#[derive(Clone, PartialEq, Eq)]
pub struct FinalUserInputOperationWitness {
    session_id: SessionId,
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
}

impl std::fmt::Debug for FinalUserInputOperationWitness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinalUserInputOperationWitness")
            .field("session_id", &self.session_id)
            .field("operation_id", self.operation.operation_id())
            .field(
                "channel_id",
                self.operation.domain_correlation().channel_id(),
            )
            .field(
                "interaction_id",
                &self.operation.domain_correlation().interaction_id(),
            )
            .field("provider_correlation", &"[REDACTED]")
            .finish()
    }
}

impl FinalUserInputOperationWitness {
    /// Consume the distinct generated consequential-effect authority.
    pub(crate) fn from_generated_effect(
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        reconciliation: &LiveHandoffReconciliationReceipt,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveConsequentialEffectAuthorized {
            channel_id,
            interaction_id,
            operation_id,
            ..
        } = effect
        else {
            return Ok(None);
        };

        if reconciliation.disposition() != LiveHandoffReconciliation::Confirmed {
            return Err(LiveExecutionAuthorityError::FinalUserInputNotConfirmed);
        }
        let correlation = operation.domain_correlation();
        if reconciliation.admission.session_id != *session_id
            || reconciliation.admission.operation != *operation
            || channel_id != correlation.channel_id().as_str()
            || interaction_id != &correlation.interaction_id().to_string()
            || operation_id != &DslOperationId::from_domain(operation.operation_id())
        {
            return Err(LiveExecutionAuthorityError::CorrelationMismatch);
        }

        Ok(Some(Self {
            session_id: session_id.clone(),
            operation: operation.clone(),
        }))
    }

    #[must_use]
    pub fn operation(&self) -> &ExactOperationIdentity<LiveUserTurnCorrelation> {
        &self.operation
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// True only for the exact operation admitted by both generated effects.
    #[must_use]
    pub fn authorizes(
        &self,
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
    ) -> bool {
        self.session_id == *session_id && self.operation == *operation
    }
}

/// Pre-send authority for one exact context append edge.
///
/// Only a generated `LiveContextAppendAuthorized` effect can construct this
/// carrier. Provider code receives it before sending and must return it with
/// the delivery observation, so post-send resolution cannot be forged from
/// copied channel and cursor values.
#[derive(Clone)]
pub struct LiveContextAppendAuthority {
    session_id: SessionId,
    channel_id: LiveChannelId,
    append_id: String,
    previous_cursor: u64,
    next_cursor: u64,
    provider_dispatch_consumed: Arc<AtomicBool>,
}

/// Sealed custody for one SessionDocument-classified row admitted into the
/// generated per-session live-context outbox.
#[derive(Debug, Clone)]
pub struct LiveContextQueuedRow {
    binding: LiveDelegationRuntimeBinding,
    append_id: String,
    row: CommittedLiveContextRow,
}

impl LiveContextQueuedRow {
    pub(crate) fn from_generated_effect(
        binding: &LiveDelegationRuntimeBinding,
        append_id: &str,
        row: CommittedLiveContextRow,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveContextRowQueued {
            session_id,
            channel_id,
            append_id: effect_append_id,
            canonical_cursor,
            disposition,
        } = effect
        else {
            return Ok(None);
        };
        let expected_disposition = match row.disposition() {
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::MirrorParentText => LiveContextRowDisposition::MirrorParentText,
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel => LiveContextRowDisposition::AlreadyPresentInLiveChannel,
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::ExcludedFromLiveContext => LiveContextRowDisposition::ExcludedFromLiveContext,
        };
        if session_id != &binding.session_id().to_string()
            || channel_id != binding.channel_id().as_str()
            || effect_append_id != append_id
            || *canonical_cursor != row.canonical_row_sequence()
            || disposition != &expected_disposition
            || row.session_id() != binding.session_id()
        {
            return Err(LiveExecutionAuthorityError::AppendAuthorityMismatch);
        }
        Ok(Some(Self {
            binding: binding.clone(),
            append_id: append_id.to_string(),
            row,
        }))
    }

    #[must_use]
    pub fn binding(&self) -> &LiveDelegationRuntimeBinding {
        &self.binding
    }

    #[must_use]
    pub fn append_id(&self) -> &str {
        &self.append_id
    }

    #[must_use]
    pub fn row(&self) -> &CommittedLiveContextRow {
        &self.row
    }
}

/// Generated acknowledgement that a no-send row advanced canonical coverage.
#[derive(Debug, Clone)]
pub struct LiveContextCanonicalCoverageReceipt {
    queued: LiveContextQueuedRow,
}

impl LiveContextCanonicalCoverageReceipt {
    pub(crate) fn from_generated_effect(
        queued: &LiveContextQueuedRow,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveContextCanonicalCoverageAdvanced {
            channel_id,
            append_id,
            previous_cursor,
            next_cursor,
            disposition,
        } = effect
        else {
            return Ok(None);
        };
        let expected_disposition = match queued.row.disposition() {
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel => LiveContextRowDisposition::AlreadyPresentInLiveChannel,
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::ExcludedFromLiveContext => LiveContextRowDisposition::ExcludedFromLiveContext,
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::MirrorParentText => return Err(LiveExecutionAuthorityError::AppendAuthorityMismatch),
        };
        if channel_id != queued.binding.channel_id().as_str()
            || append_id != &queued.append_id
            || *next_cursor != queued.row.canonical_row_sequence()
            || previous_cursor.checked_add(1) != Some(*next_cursor)
            || disposition != &expected_disposition
        {
            return Err(LiveExecutionAuthorityError::AppendAuthorityMismatch);
        }
        Ok(Some(Self {
            queued: queued.clone(),
        }))
    }

    #[must_use]
    pub fn queued(&self) -> &LiveContextQueuedRow {
        &self.queued
    }
}

/// Generated no-retry recovery authority for one ambiguous provider append.
#[derive(Debug, Clone)]
pub struct LiveContextAmbiguityRecoveryAuthority {
    session_id: SessionId,
    closing_channel_id: LiveChannelId,
    replacement_channel_id: LiveChannelId,
    append_id: String,
    canonical_seed_cursor: u64,
    llm_identity: meerkat_core::SessionLlmIdentity,
    runtime_id: crate::identifiers::LogicalRuntimeId,
    fence_token: u64,
    generation: u64,
}

impl LiveContextAmbiguityRecoveryAuthority {
    pub(crate) fn from_generated_effect(
        append: &LiveContextAppendAuthority,
        replacement_channel_id: &LiveChannelId,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveContextAmbiguityRecoveryAuthorized {
            session_id,
            closing_channel_id,
            replacement_channel_id: effect_replacement,
            append_id,
            canonical_seed_cursor,
            llm_identity,
            runtime_id,
            fence_token,
            generation,
        } = effect
        else {
            return Ok(None);
        };
        if session_id != &append.session_id.to_string()
            || closing_channel_id != append.channel_id.as_str()
            || effect_replacement != replacement_channel_id.as_str()
            || append_id != &append.append_id
            || *canonical_seed_cursor != append.next_cursor
        {
            return Err(LiveExecutionAuthorityError::AppendAuthorityMismatch);
        }
        let llm_identity = llm_identity
            .clone()
            .try_into()
            .map_err(|_| LiveExecutionAuthorityError::AppendAuthorityMismatch)?;
        Ok(Some(Self {
            session_id: append.session_id.clone(),
            closing_channel_id: append.channel_id.clone(),
            replacement_channel_id: replacement_channel_id.clone(),
            append_id: append.append_id.clone(),
            canonical_seed_cursor: *canonical_seed_cursor,
            llm_identity,
            runtime_id: crate::identifiers::LogicalRuntimeId::new(runtime_id.0.clone()),
            fence_token: fence_token.0,
            generation: generation.0,
        }))
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    #[must_use]
    pub fn closing_channel_id(&self) -> &LiveChannelId {
        &self.closing_channel_id
    }
    #[must_use]
    pub fn replacement_channel_id(&self) -> &LiveChannelId {
        &self.replacement_channel_id
    }
    #[must_use]
    pub fn append_id(&self) -> &str {
        &self.append_id
    }
    #[must_use]
    pub const fn canonical_seed_cursor(&self) -> u64 {
        self.canonical_seed_cursor
    }
    #[must_use]
    pub fn llm_identity(&self) -> &meerkat_core::SessionLlmIdentity {
        &self.llm_identity
    }
    #[must_use]
    pub fn runtime_id(&self) -> &crate::identifiers::LogicalRuntimeId {
        &self.runtime_id
    }
    #[must_use]
    pub const fn fence_token(&self) -> u64 {
        self.fence_token
    }
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
}

/// Exact generated result of one provider delivery observation.
#[derive(Debug, Clone)]
pub enum LiveContextAppendResolution {
    Resolved(LiveContextAppendResolutionReceipt),
    AmbiguityRecovery(LiveContextAmbiguityRecoveryAuthority),
}

impl PartialEq for LiveContextAppendAuthority {
    fn eq(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.channel_id == other.channel_id
            && self.append_id == other.append_id
            && self.previous_cursor == other.previous_cursor
            && self.next_cursor == other.next_cursor
            && Arc::ptr_eq(
                &self.provider_dispatch_consumed,
                &other.provider_dispatch_consumed,
            )
    }
}

impl Eq for LiveContextAppendAuthority {}

impl std::fmt::Debug for LiveContextAppendAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveContextAppendAuthority")
            .field("session_id", &"[REDACTED]")
            .field("channel_id", &"[REDACTED]")
            .field("append_id", &"[REDACTED]")
            .field("previous_cursor", &"[REDACTED]")
            .field("next_cursor", &"[REDACTED]")
            .finish()
    }
}

impl LiveContextAppendAuthority {
    pub(crate) fn from_generated_effect(
        session_id: &SessionId,
        channel_id: &LiveChannelId,
        append_id: &str,
        previous_cursor: u64,
        next_cursor: u64,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveContextAppendAuthorized {
            channel_id: effect_channel_id,
            append_id: effect_append_id,
            previous_cursor: effect_previous_cursor,
            next_cursor: effect_next_cursor,
        } = effect
        else {
            return Ok(None);
        };
        if effect_channel_id != channel_id.as_str()
            || effect_append_id != append_id
            || *effect_previous_cursor != previous_cursor
            || *effect_next_cursor != next_cursor
        {
            return Err(LiveExecutionAuthorityError::AppendAuthorityMismatch);
        }
        Ok(Some(Self {
            session_id: session_id.clone(),
            channel_id: channel_id.clone(),
            append_id: append_id.to_string(),
            previous_cursor,
            next_cursor,
            provider_dispatch_consumed: Arc::new(AtomicBool::new(false)),
        }))
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub fn append_id(&self) -> &str {
        &self.append_id
    }

    #[must_use]
    pub const fn previous_cursor(&self) -> u64 {
        self.previous_cursor
    }

    #[must_use]
    pub const fn next_cursor(&self) -> u64 {
        self.next_cursor
    }

    /// Convert one generated append edge into the provider-neutral send
    /// carrier while retaining this same authority for typed post-send
    /// resolution. Clones share the one-use conversion fence.
    #[cfg(feature = "live")]
    pub fn into_sideband_append_authority(
        self,
        binding: ProviderWebrtcBinding,
    ) -> Result<(Self, LiveSidebandAppendAuthority), LiveExecutionAuthorityError> {
        if self.session_id != *binding.session_id() || self.channel_id != *binding.channel_id() {
            return Err(LiveExecutionAuthorityError::ProviderBindingMismatch);
        }
        self.provider_dispatch_consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| LiveExecutionAuthorityError::ProviderDispatchAlreadyConverted)?;
        let sideband = LiveSidebandAppendAuthority::__from_generated_authority(
            binding,
            self.append_id.clone(),
            self.next_cursor,
        )
        .ok_or(LiveExecutionAuthorityError::AppendAuthorityMismatch)?;
        Ok((self, sideband))
    }
}

/// Machine-sealed resolution of one pre-authorized context append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveContextAppendResolutionReceipt {
    authority: LiveContextAppendAuthority,
    outcome: LiveAppendDeliveryOutcome,
    cursor: u64,
    retry_allowed: bool,
}

impl LiveContextAppendResolutionReceipt {
    pub(crate) fn from_generated_effect(
        authority: &LiveContextAppendAuthority,
        expected_outcome: LiveAppendDeliveryOutcome,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveContextAppendResolved {
            channel_id,
            append_id,
            cursor,
            observation,
            retry_allowed,
        } = effect
        else {
            return Ok(None);
        };
        let effect_outcome = match observation {
            LiveContextAppendObservation::Delivered => LiveAppendDeliveryOutcome::Acknowledged,
            LiveContextAppendObservation::Rejected => LiveAppendDeliveryOutcome::Rejected,
            LiveContextAppendObservation::Ambiguous => LiveAppendDeliveryOutcome::Ambiguous,
        };
        let expected_cursor = if matches!(expected_outcome, LiveAppendDeliveryOutcome::Acknowledged)
        {
            authority.next_cursor
        } else {
            authority.previous_cursor
        };
        let expected_retry = matches!(expected_outcome, LiveAppendDeliveryOutcome::Rejected);
        if channel_id != authority.channel_id.as_str()
            || append_id != &authority.append_id
            || *cursor != expected_cursor
            || effect_outcome != expected_outcome
            || *retry_allowed != expected_retry
        {
            return Err(LiveExecutionAuthorityError::AppendAuthorityMismatch);
        }
        Ok(Some(Self {
            authority: authority.clone(),
            outcome: effect_outcome,
            cursor: *cursor,
            retry_allowed: *retry_allowed,
        }))
    }

    #[must_use]
    pub fn authority(&self) -> &LiveContextAppendAuthority {
        &self.authority
    }

    #[must_use]
    pub const fn outcome(&self) -> LiveAppendDeliveryOutcome {
        self.outcome
    }

    #[must_use]
    pub const fn cursor(&self) -> u64 {
        self.cursor
    }

    #[must_use]
    pub const fn retry_allowed(&self) -> bool {
        self.retry_allowed
    }
}

/// Pre-send authority to place one confirmed executor result in the live
/// conversation. This is distinct from consequential tool-effect authority.
#[derive(Clone)]
pub struct LiveDelegationResultReleaseAuthority {
    session_id: SessionId,
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    disposition: LiveResultDisposition,
}

impl PartialEq for LiveDelegationResultReleaseAuthority {
    fn eq(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.operation == other.operation
            && self.disposition == other.disposition
    }
}

impl Eq for LiveDelegationResultReleaseAuthority {}

impl std::fmt::Debug for LiveDelegationResultReleaseAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveDelegationResultReleaseAuthority")
            .field("session_id", &self.session_id)
            .field("operation_id", self.operation.operation_id())
            .field(
                "channel_id",
                self.operation.domain_correlation().channel_id(),
            )
            .field(
                "interaction_id",
                &self.operation.domain_correlation().interaction_id(),
            )
            .field("provider_correlation", &"[REDACTED]")
            .field("disposition", &self.disposition)
            .finish()
    }
}

impl LiveDelegationResultReleaseAuthority {
    pub(crate) fn from_generated_effect(
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        reconciliation: &LiveHandoffReconciliationReceipt,
        expected_disposition: LiveResultDisposition,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveDelegationResultReleaseAuthorized {
            channel_id,
            interaction_id,
            operation_id,
            provider_turn_correlation,
            disposition,
        } = effect
        else {
            return Ok(None);
        };
        if reconciliation.disposition() != LiveHandoffReconciliation::Confirmed {
            return Err(LiveExecutionAuthorityError::FinalUserInputNotConfirmed);
        }
        let correlation = operation.domain_correlation();
        let effect_disposition = match disposition {
            LiveDelegationResultDisposition::OpenTurn => LiveResultDisposition::OpenTurn,
            LiveDelegationResultDisposition::DeferredContext => {
                LiveResultDisposition::DeferredContext
            }
        };
        if reconciliation.admission.session_id != *session_id
            || reconciliation.admission.operation != *operation
            || channel_id != correlation.channel_id().as_str()
            || interaction_id != &correlation.interaction_id().to_string()
            || operation_id != &DslOperationId::from_domain(operation.operation_id())
            || provider_turn_correlation != correlation.provider().user_turn_id()
            || effect_disposition != expected_disposition
        {
            return Err(LiveExecutionAuthorityError::CorrelationMismatch);
        }
        Ok(Some(Self {
            session_id: session_id.clone(),
            operation: operation.clone(),
            disposition: effect_disposition,
        }))
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn operation(&self) -> &ExactOperationIdentity<LiveUserTurnCorrelation> {
        &self.operation
    }

    #[must_use]
    pub const fn disposition(&self) -> LiveResultDisposition {
        self.disposition
    }

    #[must_use]
    pub fn authorizes(
        &self,
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
    ) -> bool {
        self.session_id == *session_id && self.operation == *operation
    }
}

pub(crate) fn live_delegation_result_digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Machine-sealed authority for one provider-context delivery of an exact
/// released delegation result. It carries no canonical session cursor and is
/// not interchangeable with ordinary context append authority.
#[derive(Clone)]
pub struct LiveDelegationResultDeliveryAuthority {
    release: LiveDelegationResultReleaseAuthority,
    result_digest: String,
    provider_dispatch_consumed: Arc<AtomicBool>,
}

impl std::fmt::Debug for LiveDelegationResultDeliveryAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveDelegationResultDeliveryAuthority")
            .field("release", &self.release)
            .field("result_digest", &"[REDACTED]")
            .finish()
    }
}

impl LiveDelegationResultDeliveryAuthority {
    pub(crate) fn from_generated_effect(
        release: &LiveDelegationResultReleaseAuthority,
        expected_result_digest: &str,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveDelegationResultDeliveryAuthorized {
            channel_id,
            interaction_id,
            operation_id,
            provider_turn_correlation,
            result_digest,
            disposition,
        } = effect
        else {
            return Ok(None);
        };
        let correlation = release.operation.domain_correlation();
        let effect_disposition = match disposition {
            LiveDelegationResultDisposition::OpenTurn => LiveResultDisposition::OpenTurn,
            LiveDelegationResultDisposition::DeferredContext => {
                LiveResultDisposition::DeferredContext
            }
        };
        if channel_id != correlation.channel_id().as_str()
            || interaction_id != &correlation.interaction_id().to_string()
            || operation_id != &DslOperationId::from_domain(release.operation.operation_id())
            || provider_turn_correlation != correlation.provider().user_turn_id()
            || result_digest != expected_result_digest
            || effect_disposition != release.disposition
        {
            return Err(LiveExecutionAuthorityError::ResultDeliveryAuthorityMismatch);
        }
        Ok(Some(Self {
            release: release.clone(),
            result_digest: result_digest.clone(),
            provider_dispatch_consumed: Arc::new(AtomicBool::new(false)),
        }))
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        self.release.session_id()
    }

    #[must_use]
    pub fn operation(&self) -> &ExactOperationIdentity<LiveUserTurnCorrelation> {
        self.release.operation()
    }

    #[must_use]
    pub const fn disposition(&self) -> LiveResultDisposition {
        self.release.disposition()
    }

    #[must_use]
    pub fn authorizes_text(&self, text: &str) -> bool {
        self.result_digest == live_delegation_result_digest(text)
    }

    pub(crate) fn result_digest(&self) -> &str {
        &self.result_digest
    }

    #[cfg(feature = "live")]
    pub fn into_sideband_release_authority(
        self,
        binding: ProviderWebrtcBinding,
        delegation: &LiveSidebandDelegationRef,
        text: &str,
    ) -> Result<(Self, LiveSidebandReleaseAuthority), LiveExecutionAuthorityError> {
        let correlation = self.operation().domain_correlation();
        if self.session_id() != binding.session_id()
            || correlation.channel_id() != binding.channel_id()
        {
            return Err(LiveExecutionAuthorityError::ProviderBindingMismatch);
        }
        if !delegation.__matches_provider_delegation_id(correlation.provider().delegation_item_id())
        {
            return Err(LiveExecutionAuthorityError::ProviderDelegationMismatch);
        }
        if !self.authorizes_text(text) {
            return Err(LiveExecutionAuthorityError::ResultDeliveryDigestMismatch);
        }
        self.provider_dispatch_consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| LiveExecutionAuthorityError::ProviderDispatchAlreadyConverted)?;
        let sideband = LiveSidebandReleaseAuthority::__from_generated_result_authority(
            binding,
            self.operation().operation_id().to_string(),
            self.disposition(),
            self.result_digest.clone(),
        )
        .ok_or(LiveExecutionAuthorityError::ResultDeliveryAuthorityMismatch)?;
        Ok((self, sideband))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDelegationResultDeliveryObservation {
    Delivered,
    Rejected,
    Ambiguous,
}

#[derive(Debug, Clone)]
pub struct LiveDelegationResultDeliveryReceipt {
    authority: LiveDelegationResultDeliveryAuthority,
    observation: LiveDelegationResultDeliveryObservation,
    retry_allowed: bool,
    recovery_required: bool,
}

/// Generated non-replay recovery authority for one ambiguous delegation
/// result delivery. It is keyed by exact operation and result digest, and is
/// deliberately distinct from canonical context append recovery.
#[derive(Debug, Clone)]
pub struct LiveDelegationResultAmbiguityRecoveryAuthority {
    delivery: LiveDelegationResultDeliveryAuthority,
    closing_channel_id: LiveChannelId,
    replacement_channel_id: LiveChannelId,
    canonical_seed_cursor: u64,
    llm_identity: meerkat_core::SessionLlmIdentity,
    runtime_id: crate::identifiers::LogicalRuntimeId,
    fence_token: u64,
    generation: u64,
}

impl LiveDelegationResultAmbiguityRecoveryAuthority {
    pub(crate) fn from_generated_effect(
        delivery: &LiveDelegationResultDeliveryAuthority,
        replacement_channel_id: &LiveChannelId,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveDelegationResultAmbiguityRecoveryAuthorized {
            session_id,
            closing_channel_id,
            replacement_channel_id: effect_replacement,
            interaction_id,
            operation_id,
            provider_turn_correlation,
            result_digest,
            disposition,
            canonical_seed_cursor,
            llm_identity,
            runtime_id,
            fence_token,
            generation,
        } = effect
        else {
            return Ok(None);
        };
        let correlation = delivery.operation().domain_correlation();
        let effect_disposition = match disposition {
            LiveDelegationResultDisposition::OpenTurn => LiveResultDisposition::OpenTurn,
            LiveDelegationResultDisposition::DeferredContext => {
                LiveResultDisposition::DeferredContext
            }
        };
        if session_id != &delivery.session_id().to_string()
            || closing_channel_id != correlation.channel_id().as_str()
            || effect_replacement != replacement_channel_id.as_str()
            || interaction_id != &correlation.interaction_id().to_string()
            || operation_id != &DslOperationId::from_domain(delivery.operation().operation_id())
            || provider_turn_correlation != correlation.provider().user_turn_id()
            || result_digest != delivery.result_digest()
            || effect_disposition != delivery.disposition()
        {
            return Err(LiveExecutionAuthorityError::ResultDeliveryAuthorityMismatch);
        }
        let llm_identity = llm_identity
            .clone()
            .try_into()
            .map_err(|_| LiveExecutionAuthorityError::ResultDeliveryAuthorityMismatch)?;
        Ok(Some(Self {
            delivery: delivery.clone(),
            closing_channel_id: correlation.channel_id().clone(),
            replacement_channel_id: replacement_channel_id.clone(),
            canonical_seed_cursor: *canonical_seed_cursor,
            llm_identity,
            runtime_id: crate::identifiers::LogicalRuntimeId::new(runtime_id.0.clone()),
            fence_token: fence_token.0,
            generation: generation.0,
        }))
    }

    #[must_use]
    pub fn delivery(&self) -> &LiveDelegationResultDeliveryAuthority {
        &self.delivery
    }
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        self.delivery.session_id()
    }
    #[must_use]
    pub fn closing_channel_id(&self) -> &LiveChannelId {
        &self.closing_channel_id
    }
    #[must_use]
    pub fn replacement_channel_id(&self) -> &LiveChannelId {
        &self.replacement_channel_id
    }
    #[must_use]
    pub const fn canonical_seed_cursor(&self) -> u64 {
        self.canonical_seed_cursor
    }
    #[must_use]
    pub fn llm_identity(&self) -> &meerkat_core::SessionLlmIdentity {
        &self.llm_identity
    }
    #[must_use]
    pub fn runtime_id(&self) -> &crate::identifiers::LogicalRuntimeId {
        &self.runtime_id
    }
    #[must_use]
    pub const fn fence_token(&self) -> u64 {
        self.fence_token
    }
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Debug, Clone)]
pub enum LiveDelegationResultDeliveryResolution {
    Resolved(LiveDelegationResultDeliveryReceipt),
    AmbiguityRecovery(LiveDelegationResultAmbiguityRecoveryAuthority),
}

impl LiveDelegationResultDeliveryReceipt {
    pub(crate) fn from_generated_effect(
        authority: &LiveDelegationResultDeliveryAuthority,
        expected_observation: LiveDelegationResultDeliveryObservation,
        effect: &MeerkatMachineEffect,
    ) -> Result<Option<Self>, LiveExecutionAuthorityError> {
        let MeerkatMachineEffect::LiveDelegationResultDeliveryResolved {
            channel_id,
            operation_id,
            result_digest,
            disposition,
            observation,
            retry_allowed,
            recovery_required,
        } = effect
        else {
            return Ok(None);
        };
        let effect_disposition = match disposition {
            LiveDelegationResultDisposition::OpenTurn => LiveResultDisposition::OpenTurn,
            LiveDelegationResultDisposition::DeferredContext => {
                LiveResultDisposition::DeferredContext
            }
        };
        let effect_observation = match observation {
            DslLiveDelegationResultDeliveryObservation::Delivered => {
                LiveDelegationResultDeliveryObservation::Delivered
            }
            DslLiveDelegationResultDeliveryObservation::Rejected => {
                LiveDelegationResultDeliveryObservation::Rejected
            }
            DslLiveDelegationResultDeliveryObservation::Ambiguous => {
                LiveDelegationResultDeliveryObservation::Ambiguous
            }
        };
        let correlation = authority.operation().domain_correlation();
        if channel_id != correlation.channel_id().as_str()
            || operation_id != &DslOperationId::from_domain(authority.operation().operation_id())
            || result_digest != &authority.result_digest
            || effect_disposition != authority.disposition()
            || effect_observation != expected_observation
            || *retry_allowed
            || *recovery_required
                != matches!(
                    effect_observation,
                    LiveDelegationResultDeliveryObservation::Ambiguous
                )
        {
            return Err(LiveExecutionAuthorityError::ResultDeliveryAuthorityMismatch);
        }
        Ok(Some(Self {
            authority: authority.clone(),
            observation: effect_observation,
            retry_allowed: *retry_allowed,
            recovery_required: *recovery_required,
        }))
    }

    #[must_use]
    pub fn authority(&self) -> &LiveDelegationResultDeliveryAuthority {
        &self.authority
    }

    #[must_use]
    pub const fn observation(&self) -> LiveDelegationResultDeliveryObservation {
        self.observation
    }

    #[must_use]
    pub const fn retry_allowed(&self) -> bool {
        self.retry_allowed
    }

    #[must_use]
    pub const fn recovery_required(&self) -> bool {
        self.recovery_required
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::ops::OperationId;
    use meerkat_core::{
        InteractionId, LiveChannelId, LiveHandoffInputProvenance, OpaqueProviderCorrelation,
    };

    fn session(seed: u128) -> SessionId {
        SessionId::from_uuid(uuid::Uuid::from_u128(seed))
    }

    fn exact_operation(
        channel: &str,
        provider_turn: &str,
        operation_seed: u128,
    ) -> ExactOperationIdentity<LiveUserTurnCorrelation> {
        let correlation = LiveUserTurnCorrelation::new(
            LiveChannelId::new(channel),
            InteractionId(uuid::Uuid::from_u128(7)),
            OpaqueProviderCorrelation::new("delegation-secret", provider_turn)
                .expect("provider correlation"),
        )
        .expect("turn correlation");
        ExactOperationIdentity::for_domain(
            OperationId(uuid::Uuid::from_u128(operation_seed)),
            correlation,
        )
    }

    fn reconciliation_effect(
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        reconciliation: LiveDelegationReconciliation,
    ) -> MeerkatMachineEffect {
        MeerkatMachineEffect::LiveDelegationTranscriptReconciled {
            channel_id: operation.domain_correlation().channel_id().to_string(),
            interaction_id: operation.domain_correlation().interaction_id().to_string(),
            operation_id: DslOperationId::from_domain(operation.operation_id()),
            reconciliation,
            cancellation_required: reconciliation != LiveDelegationReconciliation::Confirmed,
        }
    }

    fn consequential_effect(
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
    ) -> MeerkatMachineEffect {
        MeerkatMachineEffect::LiveConsequentialEffectAuthorized {
            channel_id: operation.domain_correlation().channel_id().to_string(),
            interaction_id: operation.domain_correlation().interaction_id().to_string(),
            operation_id: DslOperationId::from_domain(operation.operation_id()),
            authority_id: "machine-authority-1".to_string(),
        }
    }

    fn context_authorized_effect(
        channel_id: &str,
        append_id: &str,
        previous_cursor: u64,
        next_cursor: u64,
    ) -> MeerkatMachineEffect {
        MeerkatMachineEffect::LiveContextAppendAuthorized {
            channel_id: channel_id.to_string(),
            append_id: append_id.to_string(),
            previous_cursor,
            next_cursor,
        }
    }

    fn context_resolved_effect(
        channel_id: &str,
        append_id: &str,
        cursor: u64,
        observation: LiveContextAppendObservation,
        retry_allowed: bool,
    ) -> MeerkatMachineEffect {
        MeerkatMachineEffect::LiveContextAppendResolved {
            channel_id: channel_id.to_string(),
            append_id: append_id.to_string(),
            cursor,
            observation,
            retry_allowed,
        }
    }

    fn result_release_effect(
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        disposition: LiveDelegationResultDisposition,
    ) -> MeerkatMachineEffect {
        let correlation = operation.domain_correlation();
        MeerkatMachineEffect::LiveDelegationResultReleaseAuthorized {
            channel_id: correlation.channel_id().to_string(),
            interaction_id: correlation.interaction_id().to_string(),
            operation_id: DslOperationId::from_domain(operation.operation_id()),
            provider_turn_correlation: correlation.provider().user_turn_id().to_string(),
            disposition,
        }
    }

    fn result_delivery_authorized_effect(
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
        result_digest: &str,
        disposition: LiveDelegationResultDisposition,
    ) -> MeerkatMachineEffect {
        let correlation = operation.domain_correlation();
        MeerkatMachineEffect::LiveDelegationResultDeliveryAuthorized {
            channel_id: correlation.channel_id().to_string(),
            interaction_id: correlation.interaction_id().to_string(),
            operation_id: DslOperationId::from_domain(operation.operation_id()),
            provider_turn_correlation: correlation.provider().user_turn_id().to_string(),
            result_digest: result_digest.to_string(),
            disposition,
        }
    }

    fn provisional(
        operation: &ExactOperationIdentity<LiveUserTurnCorrelation>,
    ) -> ProvisionalLiveHandoff {
        ProvisionalLiveHandoff::new(
            operation.domain_correlation().clone(),
            "executor-input-secret",
            LiveHandoffInputProvenance::NormalizedHandoff,
        )
        .expect("provisional")
    }

    #[test]
    fn generated_reconciliation_and_consequential_effect_mint_exact_witness() {
        let session_id = session(1);
        let operation = exact_operation("channel-a", "provider-turn-secret", 11);
        let receipt = LiveHandoffReconciliationReceipt::from_generated_effect(
            &session_id,
            &operation,
            &provisional(&operation),
            LiveHandoffReconciliation::Confirmed,
            &reconciliation_effect(&operation, LiveDelegationReconciliation::Confirmed),
        )
        .expect("reconciliation")
        .expect("generated effect");
        let witness = FinalUserInputOperationWitness::from_generated_effect(
            &session_id,
            &operation,
            &receipt,
            &consequential_effect(&operation),
        )
        .expect("consequential authority")
        .expect("generated effect");

        assert!(witness.authorizes(&session_id, &operation));
        assert!(!witness.authorizes(
            &session_id,
            &exact_operation("channel-a", "provider-turn-secret", 12,)
        ));
        assert!(!witness.authorizes(&session(2), &operation));
    }

    #[test]
    fn context_append_authority_is_pre_send_exact_and_resolution_is_sealed() {
        let session_id = session(1);
        let channel_id = LiveChannelId::new("channel-a");
        let authority = LiveContextAppendAuthority::from_generated_effect(
            &session_id,
            &channel_id,
            "append-1",
            3,
            4,
            &context_authorized_effect("channel-a", "append-1", 3, 4),
        )
        .expect("authority effect is valid")
        .expect("matching authority effect");
        assert_eq!(authority.previous_cursor(), 3);
        assert_eq!(authority.next_cursor(), 4);

        let receipt = LiveContextAppendResolutionReceipt::from_generated_effect(
            &authority,
            LiveAppendDeliveryOutcome::Ambiguous,
            &context_resolved_effect(
                "channel-a",
                "append-1",
                3,
                LiveContextAppendObservation::Ambiguous,
                false,
            ),
        )
        .expect("resolution effect is valid")
        .expect("matching resolution effect");
        assert_eq!(receipt.outcome(), LiveAppendDeliveryOutcome::Ambiguous);
        assert_eq!(receipt.cursor(), 3);
        assert!(!receipt.retry_allowed());

        assert_eq!(
            LiveContextAppendAuthority::from_generated_effect(
                &session_id,
                &channel_id,
                "append-1",
                3,
                4,
                &context_authorized_effect("channel-a", "other-append", 3, 4),
            ),
            Err(LiveExecutionAuthorityError::AppendAuthorityMismatch)
        );
    }

    #[cfg(feature = "live")]
    #[test]
    fn generated_append_conversion_is_exact_and_one_use_across_clones() {
        let session_id = session(1);
        let channel_id = LiveChannelId::new("channel-a");
        let authority = LiveContextAppendAuthority::from_generated_effect(
            &session_id,
            &channel_id,
            "append-1",
            3,
            4,
            &context_authorized_effect("channel-a", "append-1", 3, 4),
        )
        .expect("authority effect")
        .expect("matching authority effect");
        let duplicate = authority.clone();
        let binding = ProviderWebrtcBinding::new(
            channel_id,
            session_id,
            meerkat_live::LiveRuntimeBindingGeneration::new(7),
            meerkat_live::LiveRuntimeBindingFence::new(9),
        );

        let (authority, _sideband) = authority
            .into_sideband_append_authority(binding.clone())
            .expect("first conversion");
        assert_eq!(authority.next_cursor(), 4);
        assert!(matches!(
            duplicate.into_sideband_append_authority(binding),
            Err(LiveExecutionAuthorityError::ProviderDispatchAlreadyConverted)
        ));
    }

    #[cfg(feature = "live")]
    #[test]
    fn result_delivery_conversion_is_digest_bound_and_has_no_canonical_cursor() {
        let session_id = session(1);
        let operation = exact_operation("channel-a", "provider-turn-secret", 11);
        let reconciliation = LiveHandoffReconciliationReceipt::from_generated_effect(
            &session_id,
            &operation,
            &provisional(&operation),
            LiveHandoffReconciliation::Confirmed,
            &reconciliation_effect(&operation, LiveDelegationReconciliation::Confirmed),
        )
        .expect("reconciliation")
        .expect("effect");
        let release = LiveDelegationResultReleaseAuthority::from_generated_effect(
            &session_id,
            &operation,
            &reconciliation,
            LiveResultDisposition::DeferredContext,
            &result_release_effect(&operation, LiveDelegationResultDisposition::DeferredContext),
        )
        .expect("release")
        .expect("effect");
        let result_text = "bounded worker result";
        let result_digest = live_delegation_result_digest(result_text);
        let delivery = LiveDelegationResultDeliveryAuthority::from_generated_effect(
            &release,
            &result_digest,
            &result_delivery_authorized_effect(
                &operation,
                &result_digest,
                LiveDelegationResultDisposition::DeferredContext,
            ),
        )
        .expect("delivery")
        .expect("effect");
        let duplicate = delivery.clone();
        let binding = ProviderWebrtcBinding::new(
            operation.domain_correlation().channel_id().clone(),
            session_id,
            meerkat_live::LiveRuntimeBindingGeneration::new(7),
            meerkat_live::LiveRuntimeBindingFence::new(9),
        );
        let delegation = LiveSidebandDelegationRef::__from_provider_observation(
            "adapter-key".to_string(),
            "delegation-secret".to_string(),
        )
        .expect("delegation");

        let (_, _sideband) = delivery
            .into_sideband_release_authority(binding.clone(), &delegation, result_text)
            .expect("joined release");
        assert!(matches!(
            duplicate.into_sideband_release_authority(binding, &delegation, result_text),
            Err(LiveExecutionAuthorityError::ProviderDispatchAlreadyConverted)
        ));
    }

    #[test]
    fn result_release_authority_requires_exact_confirmed_operation_and_disposition() {
        let session_id = session(1);
        let operation = exact_operation("channel-a", "provider-turn-secret", 11);
        let receipt = LiveHandoffReconciliationReceipt::from_generated_effect(
            &session_id,
            &operation,
            &provisional(&operation),
            LiveHandoffReconciliation::Confirmed,
            &reconciliation_effect(&operation, LiveDelegationReconciliation::Confirmed),
        )
        .expect("reconciliation")
        .expect("generated effect");
        let authority = LiveDelegationResultReleaseAuthority::from_generated_effect(
            &session_id,
            &operation,
            &receipt,
            LiveResultDisposition::DeferredContext,
            &result_release_effect(&operation, LiveDelegationResultDisposition::DeferredContext),
        )
        .expect("release effect is valid")
        .expect("matching release effect");
        assert!(authority.authorizes(&session_id, &operation));
        assert_eq!(
            authority.disposition(),
            LiveResultDisposition::DeferredContext
        );

        assert_eq!(
            LiveDelegationResultReleaseAuthority::from_generated_effect(
                &session_id,
                &operation,
                &receipt,
                LiveResultDisposition::OpenTurn,
                &result_release_effect(
                    &operation,
                    LiveDelegationResultDisposition::DeferredContext,
                ),
            ),
            Err(LiveExecutionAuthorityError::CorrelationMismatch)
        );
    }

    #[test]
    fn nonconfirmed_reconciliation_cannot_mint_consequential_witness() {
        let session_id = session(1);
        let operation = exact_operation("channel-a", "provider-turn-secret", 11);
        for reconciliation in [
            LiveDelegationReconciliation::MaterialConflict,
            LiveDelegationReconciliation::Missing,
        ] {
            let receipt = LiveHandoffReconciliationReceipt::from_generated_effect(
                &session_id,
                &operation,
                &provisional(&operation),
                match reconciliation {
                    LiveDelegationReconciliation::MaterialConflict => {
                        LiveHandoffReconciliation::MaterialConflict
                    }
                    LiveDelegationReconciliation::Missing => LiveHandoffReconciliation::Missing,
                    _ => unreachable!("test only supplies terminal non-confirmed states"),
                },
                &reconciliation_effect(&operation, reconciliation),
            )
            .expect("reconciliation")
            .expect("generated effect");
            assert_eq!(
                FinalUserInputOperationWitness::from_generated_effect(
                    &session_id,
                    &operation,
                    &receipt,
                    &consequential_effect(&operation),
                ),
                Err(LiveExecutionAuthorityError::FinalUserInputNotConfirmed)
            );
        }
    }

    #[test]
    fn stale_channel_generated_effect_is_rejected_before_witness_mint() {
        let session_id = session(1);
        let operation = exact_operation("channel-a", "provider-turn-secret", 11);
        let stale_operation = exact_operation("channel-b", "provider-turn-secret", 11);
        assert_eq!(
            LiveHandoffReconciliationReceipt::from_generated_effect(
                &session_id,
                &operation,
                &provisional(&operation),
                LiveHandoffReconciliation::Confirmed,
                &reconciliation_effect(&stale_operation, LiveDelegationReconciliation::Confirmed,),
            ),
            Err(LiveExecutionAuthorityError::CorrelationMismatch)
        );
    }

    #[test]
    fn generated_reconciliation_cannot_override_derived_classification() {
        let session_id = session(1);
        let operation = exact_operation("channel-a", "provider-turn-secret", 11);
        assert_eq!(
            LiveHandoffReconciliationReceipt::from_generated_effect(
                &session_id,
                &operation,
                &provisional(&operation),
                LiveHandoffReconciliation::MaterialConflict,
                &reconciliation_effect(&operation, LiveDelegationReconciliation::Confirmed),
            ),
            Err(LiveExecutionAuthorityError::ReconciliationMismatch)
        );
    }

    #[test]
    fn exact_final_digest_is_the_only_confirmed_classification() {
        let provisional =
            NormalizedLiveUserInputDigest::derive("same normalized bytes").expect("digest");
        let same =
            NormalizedLiveUserInputDigest::derive("same normalized bytes").expect("same digest");
        let different =
            NormalizedLiveUserInputDigest::derive("different normalized bytes").expect("digest");

        assert_eq!(
            reconciliation_from_terminal_digest(
                &provisional,
                FinalLiveUserTranscriptDisposition::Committed,
                Some(&same),
            ),
            Ok(LiveHandoffReconciliation::Confirmed)
        );
        assert_eq!(
            reconciliation_from_terminal_digest(
                &provisional,
                FinalLiveUserTranscriptDisposition::Committed,
                Some(&different),
            ),
            Ok(LiveHandoffReconciliation::MaterialConflict)
        );
        assert_eq!(
            reconciliation_from_terminal_digest(
                &provisional,
                FinalLiveUserTranscriptDisposition::Missing,
                None,
            ),
            Ok(LiveHandoffReconciliation::Missing)
        );
        assert_eq!(
            reconciliation_from_terminal_digest(
                &provisional,
                FinalLiveUserTranscriptDisposition::Committed,
                None,
            ),
            Err(LiveExecutionAuthorityError::InvalidTranscriptEvidence)
        );
    }

    #[test]
    fn debug_output_redacts_provider_ids_and_executor_input() {
        let session_id = session(1);
        let operation = exact_operation("channel-a", "provider-turn-secret", 11);
        let provisional = provisional(&operation);
        let receipt = LiveHandoffReconciliationReceipt::from_generated_effect(
            &session_id,
            &operation,
            &provisional,
            LiveHandoffReconciliation::Confirmed,
            &reconciliation_effect(&operation, LiveDelegationReconciliation::Confirmed),
        )
        .expect("reconciliation")
        .expect("generated effect");
        let rendered = format!("{provisional:?} {receipt:?}");

        assert!(!rendered.contains("provider-turn-secret"));
        assert!(!rendered.contains("delegation-secret"));
        assert!(!rendered.contains("executor-input-secret"));
    }
}
