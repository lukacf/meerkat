//! Shared provider-neutral coordinator for experimental GPT Live execution.
//!
//! Client-context delegation and Responses function bridging retain distinct
//! provider contracts, but both execute tool-bearing work on ordinary Mob
//! members. A Responses call waits for exact final-user transcript authority,
//! then persists a real durable fork of the channel-bound member and runs one
//! bounded child turn. The live endpoint itself never owns callback or effect
//! execution.

use std::sync::Arc;

use meerkat::experimental_gpt_live::{
    ExperimentalGptLiveBridgeError, ExperimentalGptLiveControlObservation,
    ExperimentalGptLiveControlPlane, ExperimentalGptLiveResultDeliveryDispatch,
};
use meerkat_core::exact_operation::ExactOperationIdentity;
use meerkat_core::ops::OperationId;
use meerkat_core::{
    FinalLiveUserTranscriptCommitEvidence, LiveHandoffInputProvenance, LiveHandoffReconciliation,
    LiveUserTurnCorrelation, OpaqueProviderCorrelation, ProvisionalLiveHandoff, SessionId,
};
use meerkat_live::{
    LiveSidebandDelegationRef, LiveSidebandObservation, LiveSidebandObservationKind,
    LiveSidebandTurnRef, ProviderWebrtcBinding,
};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, DelegationCancellationHandle, DelegationExecutionHandle,
    DelegationExecutionRequest, DelegationExecutionService, DelegationTerminalizedExecution,
    DelegationTurnTerminal, DurableBoundedMemberState, DurableBoundedWorkState,
    LiveBridgeExecutionSnapshot, LiveBridgeOperationTerminal, MobDeliveryIdentity,
    render_bounded_delegation_task,
};
use meerkat_runtime::live_execution::{
    LiveBridgeExecutionTerminalReceipt, LiveBridgeOperationAdmission,
    LiveBridgeRecoveredSubmissionReceipt, LiveBridgeRecoveredTerminalReceipt,
    LiveBridgeSubmissionAttemptAuthority, LiveBridgeSubmissionAuthority,
    LiveBridgeSubmissionReceipt, LiveDelegationCancellationDirective,
    LiveDelegationCancellationOutcome, LiveDelegationExecutionAdmission,
    LiveDelegationResultDeliveryAuthority, LiveDelegationResultDeliveryObservation,
    LiveDelegationResultDeliveryResolution, LiveDelegationResultReleaseAuthority,
    LiveDelegationWorkerTerminalKind, LiveHandoffReconciliationReceipt,
};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const LIVE_DELEGATION_RESULT_BYTES: usize = 16 * 1024;
const LIVE_DELEGATION_CLEANUP_RETRY_DELAY: std::time::Duration =
    std::time::Duration::from_millis(25);
const LIVE_DELEGATION_CLEANUP_RETRY_MAX_DELAY: std::time::Duration =
    std::time::Duration::from_secs(1);
const LIVE_BRIDGE_EXECUTION_RESULT_DIGEST_DOMAIN: &[u8] =
    b"meerkat.live-bridge-execution-result.v1\0";
const LIVE_BRIDGE_SUBMISSION_OUTPUT_DIGEST_DOMAIN: &[u8] =
    b"meerkat.live-bridge-submission-output.v1\0";
const RESPONSES_RESTART_OBSERVE_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(50);

type ActiveChannelKey = (SessionId, meerkat_core::LiveChannelId);

fn live_bridge_execution_result_digest(terminal: &LiveBridgeOperationTerminal) -> Option<String> {
    terminal.output().map(|output| {
        let mut hasher = Sha256::new();
        hasher.update(LIVE_BRIDGE_EXECUTION_RESULT_DIGEST_DOMAIN);
        hasher.update((output.len() as u64).to_be_bytes());
        hasher.update(output.as_bytes());
        format!("sha256:{:x}", hasher.finalize())
    })
}

fn live_bridge_submission_output_digest(output: &str) -> Result<String, String> {
    if output.is_empty() {
        return Err("live bridge submission output must not be empty".to_string());
    }
    let mut hasher = Sha256::new();
    hasher.update(LIVE_BRIDGE_SUBMISSION_OUTPUT_DIGEST_DOMAIN);
    hasher.update((output.len() as u64).to_be_bytes());
    hasher.update(output.as_bytes());
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn live_bridge_admission_matches_current_owner(
    admission: &LiveBridgeOperationAdmission,
    current_binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    durable_identity: &AgentIdentity,
) -> bool {
    admission.binding() == current_binding
        && durable_identity.as_str() == admission.agent_identity()
        && admission.session_id() == current_binding.session_id()
}

struct ActiveDelegation {
    retained: Arc<RetainedDelegation>,
    cancellation: DelegationCancellationHandle,
    task: JoinHandle<()>,
}

struct OwnedDelegationCleanup {
    binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    task: JoinHandle<()>,
}

struct OwnedResultRecovery {
    session_id: SessionId,
    channel_id: meerkat_core::LiveChannelId,
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

struct ActiveResponsesExecution {
    admission: Arc<LiveBridgeOperationAdmission>,
    delivery_fenced: Arc<std::sync::atomic::AtomicBool>,
    terminal_custody: Arc<Mutex<Option<PendingResponsesTerminalCustody>>>,
    _task: JoinHandle<()>,
}

struct PendingResponsesTerminalCustody {
    executor_terminal: DurableExecutorTerminalKind,
    bridge_terminal: LiveBridgeOperationTerminal,
    result_digest: Option<String>,
    retirement_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DurableExecutorTerminalKind {
    Completed,
    Failed,
}

fn accepted_terminal_blocks_operation_cancellation(
    custody: Option<&PendingResponsesTerminalCustody>,
) -> bool {
    custody.is_some()
}

fn fence_provider_delivery_for_accepted_terminal(
    custody: Option<&PendingResponsesTerminalCustody>,
    delivery_fenced: &std::sync::atomic::AtomicBool,
) -> bool {
    if !accepted_terminal_blocks_operation_cancellation(custody) {
        return false;
    }
    delivery_fenced.store(true, std::sync::atomic::Ordering::Release);
    true
}

fn provider_output_after_delivery_fence(
    terminal: LiveBridgeOperationTerminal,
    delivery_fenced: bool,
) -> Option<String> {
    if delivery_fenced {
        None
    } else {
        terminal.into_output()
    }
}

fn live_bridge_terminal_recording_retryable(error: &meerkat_runtime::RuntimeDriverError) -> bool {
    matches!(
        error,
        meerkat_runtime::RuntimeDriverError::NotReady { state }
            if *state != meerkat_runtime::RuntimeState::Destroyed
    ) || matches!(
        error,
        meerkat_runtime::RuntimeDriverError::RecoveryBackoff { .. }
    )
}

async fn record_live_bridge_terminal_with_typed_recovery<F, Fut>(
    mut record: F,
) -> Result<LiveBridgeExecutionTerminalReceipt, meerkat_runtime::RuntimeDriverError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<
            Output = Result<
                LiveBridgeExecutionTerminalReceipt,
                meerkat_runtime::RuntimeDriverError,
            >,
        >,
{
    let mut backoff = std::time::Duration::from_millis(5);
    loop {
        match record().await {
            Ok(receipt) => return Ok(receipt),
            Err(error) if live_bridge_terminal_recording_retryable(&error) => {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_millis(250));
            }
            Err(error) => return Err(error),
        }
    }
}

async fn retry_responses_outcome_custody_step<T, E, F, Fut>(
    operation_id: &OperationId,
    shutdown: &CancellationToken,
    retry_step: &'static str,
    mut append: F,
) -> Option<T>
where
    E: std::fmt::Display,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut retry_delay = LIVE_DELEGATION_CLEANUP_RETRY_DELAY;
    loop {
        let attempt = tokio::select! {
            () = shutdown.cancelled() => return None,
            result = append() => result,
        };
        match attempt {
            Ok(result) => return Some(result),
            Err(error) => {
                tracing::warn!(
                    %error,
                    %operation_id,
                    retry_step,
                    "durable executor terminal is preserved while append-only source-context projection remains pending"
                );
                tokio::select! {
                    () = shutdown.cancelled() => return None,
                    () = tokio::time::sleep(retry_delay) => {}
                }
                retry_delay = retry_delay
                    .saturating_mul(2)
                    .min(LIVE_DELEGATION_CLEANUP_RETRY_MAX_DELAY);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveBridgeRetirementDisposition {
    Retired,
    AlreadyAbsent,
    Unsettled,
    Shutdown,
}

async fn retire_live_bridge_operation_after_persisted_fact(
    runtime: &meerkat_runtime::MeerkatMachine,
    session_id: &SessionId,
    operation: &ExactOperationIdentity<meerkat_core::LiveBridgeOperationCorrelation>,
    shutdown: &CancellationToken,
) -> LiveBridgeRetirementDisposition {
    let mut retry_delay = LIVE_DELEGATION_CLEANUP_RETRY_DELAY;
    loop {
        let retirement = tokio::select! {
            () = shutdown.cancelled() => return LiveBridgeRetirementDisposition::Shutdown,
            result = runtime.retire_settled_live_bridge_operation(session_id, operation) => result,
        };
        match retirement {
            Ok(true) => return LiveBridgeRetirementDisposition::Retired,
            Ok(false) => return LiveBridgeRetirementDisposition::AlreadyAbsent,
            Err(meerkat_runtime::RuntimeDriverError::ValidationFailed { reason }) => {
                tracing::debug!(
                    %reason,
                    operation_id = %operation.operation_id(),
                    "live bridge operation retirement remains machine-ineligible"
                );
                return LiveBridgeRetirementDisposition::Unsettled;
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    operation_id = %operation.operation_id(),
                    "live bridge operation retirement reconciliation remains pending"
                );
                tokio::select! {
                    () = shutdown.cancelled() => return LiveBridgeRetirementDisposition::Shutdown,
                    () = tokio::time::sleep(retry_delay) => {}
                }
                retry_delay = retry_delay
                    .saturating_mul(2)
                    .min(LIVE_DELEGATION_CLEANUP_RETRY_MAX_DELAY);
            }
        }
    }
}

async fn reconcile_responses_retirement_custody(
    pending: &Mutex<PendingResponsesRetirementMap>,
    session_id: &SessionId,
    operation: &ExactOperationIdentity<meerkat_core::LiveBridgeOperationCorrelation>,
    disposition: LiveBridgeRetirementDisposition,
) {
    let operation_id = operation.operation_id().clone();
    let mut pending = pending.lock().await;
    match disposition {
        LiveBridgeRetirementDisposition::Unsettled => {
            pending.insert(operation_id, (session_id.clone(), operation.clone()));
        }
        LiveBridgeRetirementDisposition::Retired
        | LiveBridgeRetirementDisposition::AlreadyAbsent => {
            pending.remove(&operation_id);
        }
        LiveBridgeRetirementDisposition::Shutdown => {}
    }
}

async fn drive_responses_retirement_after_persisted_fact(
    runtime: &meerkat_runtime::MeerkatMachine,
    pending: &Mutex<PendingResponsesRetirementMap>,
    session_id: &SessionId,
    operation: &ExactOperationIdentity<meerkat_core::LiveBridgeOperationCorrelation>,
    shutdown: &CancellationToken,
) -> LiveBridgeRetirementDisposition {
    // Reserve retirement-only custody before asking generated authority. This
    // closes the race where channel-close settlement could happen between an
    // ineligible observation and insertion of the process-local retry anchor.
    reconcile_responses_retirement_custody(
        pending,
        session_id,
        operation,
        LiveBridgeRetirementDisposition::Unsettled,
    )
    .await;
    let disposition =
        retire_live_bridge_operation_after_persisted_fact(runtime, session_id, operation, shutdown)
            .await;
    reconcile_responses_retirement_custody(pending, session_id, operation, disposition).await;
    disposition
}

enum LiveBridgeTerminalCommit {
    Active(LiveBridgeExecutionTerminalReceipt),
    Revoked(LiveBridgeRecoveredTerminalReceipt),
}

async fn record_live_bridge_terminal_across_revocation(
    runtime: &meerkat_runtime::MeerkatMachine,
    admission: &LiveBridgeOperationAdmission,
    terminal: meerkat_core::MeerkatExecutionTerminal,
    result_digest: Option<&str>,
) -> Result<LiveBridgeTerminalCommit, meerkat_runtime::RuntimeDriverError> {
    let channel_id = admission.operation().domain_correlation().channel_id();
    if runtime
        .live_channel_is_active_for_session(admission.session_id(), channel_id)
        .await
    {
        match record_live_bridge_terminal_with_typed_recovery(|| {
            runtime.record_live_bridge_execution_terminal(admission, terminal, result_digest)
        })
        .await
        {
            Ok(receipt) => return Ok(LiveBridgeTerminalCommit::Active(receipt)),
            Err(error) => {
                if runtime
                    .live_channel_is_active_for_session(admission.session_id(), channel_id)
                    .await
                {
                    return Err(error);
                }
            }
        }
    }

    let snapshots = runtime
        .live_bridge_recovery_snapshots(admission.session_id())
        .await?;
    let snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.operation() == admission.operation())
        .ok_or_else(|| meerkat_runtime::RuntimeDriverError::ValidationFailed {
            reason: "revoked live bridge lost its durable operation snapshot".to_string(),
        })?;
    runtime
        .reconcile_revoked_live_bridge_execution_terminal(snapshot, terminal, result_digest)
        .await
        .map(LiveBridgeTerminalCommit::Revoked)
}

struct PreparedResponsesExecution {
    admission: Arc<LiveBridgeOperationAdmission>,
    mob_handle: meerkat_mob::MobHandle,
    source_identity: AgentIdentity,
    semantic_request: String,
    completion: oneshot::Sender<Result<ExperimentalLiveBridgeExecutionCompletion, String>>,
}

type PreparedResponsesMap = std::collections::HashMap<OperationId, PreparedResponsesExecution>;
type ActiveResponsesMap = std::collections::HashMap<OperationId, ActiveResponsesExecution>;
type PendingResponsesRetirementMap = std::collections::HashMap<
    OperationId,
    (
        SessionId,
        ExactOperationIdentity<meerkat_core::LiveBridgeOperationCorrelation>,
    ),
>;

struct ResponsesProjectionShutdown {
    cancellation: CancellationToken,
}

impl Drop for ResponsesProjectionShutdown {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

fn responses_executor_task(semantic_request: &str) -> String {
    render_bounded_delegation_task(semantic_request)
}

fn responses_executor_outcome_receipt(
    operation_id: &OperationId,
    terminal: DurableExecutorTerminalKind,
    output: Option<&str>,
) -> String {
    match (terminal, output) {
        (DurableExecutorTerminalKind::Completed, Some(output)) => format!(
            "MEERKAT_LIVE_EXECUTOR_OUTCOME_V1\nThe delegated executor completed for operation {operation_id}. Its loose best-effort completion report follows:\n\n{output}"
        ),
        (DurableExecutorTerminalKind::Completed, None) => format!(
            "MEERKAT_LIVE_EXECUTOR_OUTCOME_V1\nThe delegated executor completed for operation {operation_id} without a text report."
        ),
        (DurableExecutorTerminalKind::Failed, _) => format!(
            "MEERKAT_LIVE_EXECUTOR_OUTCOME_V1\nThe delegated executor failed for operation {operation_id}."
        ),
    }
}

/// Independent Meerkat execution completion. This carries no provider send
/// authority. The coordinator separately attempts an idempotent append-only
/// outcome receipt on the canonical source session.
pub struct ExperimentalLiveBridgeExecutionCompletion {
    terminal: LiveBridgeExecutionTerminalReceipt,
    output: Option<String>,
}

/// Read-only restart reconciliation outcome for one durable Responses bridge
/// operation. This carries no provider send or work admission authority.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExperimentalResponsesRestartDisposition {
    NoExecutorBeforeFinalInput,
    InFlight,
    OutcomeProjected { completed: bool },
    Broken { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentalResponsesRestartReport {
    operation_id: OperationId,
    disposition: ExperimentalResponsesRestartDisposition,
}

impl ExperimentalResponsesRestartReport {
    #[must_use]
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    #[must_use]
    pub fn disposition(&self) -> &ExperimentalResponsesRestartDisposition {
        &self.disposition
    }
}

impl ExperimentalLiveBridgeExecutionCompletion {
    #[must_use]
    pub fn terminal(&self) -> &LiveBridgeExecutionTerminalReceipt {
        &self.terminal
    }

    #[must_use]
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    #[must_use]
    pub fn into_output(self) -> Option<String> {
        self.output
    }
}

/// One coordinator-owned accepted Responses execution. Dropping this waiter
/// does not drop operation custody; channel cancellation still reaches the
/// exact background task.
pub struct ExperimentalLiveBridgeExecutionWaiter {
    operation_id: OperationId,
    completion: oneshot::Receiver<Result<ExperimentalLiveBridgeExecutionCompletion, String>>,
}

impl ExperimentalLiveBridgeExecutionWaiter {
    #[must_use]
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    pub async fn await_completion(
        self,
    ) -> Result<ExperimentalLiveBridgeExecutionCompletion, String> {
        self.completion
            .await
            .map_err(|_| "live bridge execution completion owner stopped".to_string())?
    }
}

async fn cancel_and_settle_result_recovery(recovery: OwnedResultRecovery) {
    recovery.cancellation.cancel();
    let _ = recovery.task.await;
}

async fn await_result_recovery_attempt_or_shutdown<T>(
    cancellation: CancellationToken,
    attempt: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        () = cancellation.cancelled() => None,
        result = attempt => Some(result),
    }
}

struct RetainedDelegation {
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    provisional: ProvisionalLiveHandoff,
    runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    admission: LiveDelegationExecutionAdmission,
    delegation: LiveSidebandDelegationRef,
    control: Arc<dyn ExperimentalGptLiveControlPlane>,
    result: Mutex<RetainedDelegationResult>,
}

/// One ownership-preserving handoff of an exact bounded executor result to
/// the exact provider delegation that requested it. The carrier deliberately
/// performs no normalization or interpretation: machine authorization binds
/// the result before this mechanical dispatch seam.
struct ExactDelegationResultProjection<Authority> {
    authority: Authority,
    delegation: LiveSidebandDelegationRef,
    result_text: String,
}

impl<Authority> ExactDelegationResultProjection<Authority> {
    fn new(
        authority: Authority,
        delegation: LiveSidebandDelegationRef,
        result_text: String,
    ) -> Self {
        Self {
            authority,
            delegation,
            result_text,
        }
    }

    async fn dispatch<Output, Release, ReleaseFuture>(self, release: Release) -> Output
    where
        Release: FnOnce(Authority, LiveSidebandDelegationRef, String) -> ReleaseFuture,
        ReleaseFuture: std::future::Future<Output = Output>,
    {
        release(self.authority, self.delegation, self.result_text).await
    }
}

#[derive(Default)]
struct RetainedDelegationResult {
    reconciliation: Option<LiveHandoffReconciliationReceipt>,
    result_text: Option<String>,
    release_authority: Option<LiveDelegationResultReleaseAuthority>,
    delivery_authority: Option<LiveDelegationResultDeliveryAuthority>,
    terminal_ineligible: bool,
    delivery_reservation: Option<ResultDeliveryReservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResultDeliveryReservation(uuid::Uuid);

impl RetainedDelegationResult {
    fn reserve_delivery(&mut self) -> Option<ResultDeliveryReservation> {
        if self.terminal_ineligible || self.delivery_reservation.is_some() {
            return None;
        }
        let reservation = ResultDeliveryReservation(uuid::Uuid::new_v4());
        self.delivery_reservation = Some(reservation);
        Some(reservation)
    }

    fn release_delivery(&mut self, reservation: ResultDeliveryReservation) {
        if self.delivery_reservation == Some(reservation) {
            self.delivery_reservation = None;
        }
    }
}

enum StartedDelegationTaskCommand {
    Run,
    CleanupAfterStartPublicationFailure,
}

async fn await_started_delegation_task_command(
    receiver: oneshot::Receiver<StartedDelegationTaskCommand>,
) -> StartedDelegationTaskCommand {
    receiver
        .await
        .unwrap_or(StartedDelegationTaskCommand::CleanupAfterStartPublicationFailure)
}

struct ActiveProviderTurn {
    authority: meerkat_runtime::meerkat_machine::LiveProviderTurnStartedAuthority,
}

type CompletedDelegationTurnKey = (SessionId, meerkat_core::LiveChannelId, String);

#[derive(Clone)]
struct CompletedDelegationTurn {
    authority: meerkat_runtime::meerkat_machine::LiveProviderTurnFinishedAuthority,
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    provisional: ProvisionalLiveHandoff,
    runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    delegation_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundChannelPhase {
    Prepared,
    Running,
    StopRequested,
    Stopped,
}

struct BoundChannelCustody {
    binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    cancellation: CancellationToken,
    completion: Arc<tokio::sync::Notify>,
    phase: BoundChannelPhase,
}

type BoundChannelMap = Arc<Mutex<std::collections::HashMap<ActiveChannelKey, BoundChannelCustody>>>;

fn provider_binding_from_runtime(
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
) -> ProviderWebrtcBinding {
    ProviderWebrtcBinding::new(
        binding.channel_id().clone(),
        binding.session_id().clone(),
        meerkat_live::LiveRuntimeBindingGeneration::new(binding.generation()),
        meerkat_live::LiveRuntimeBindingFence::new(binding.fence_token()),
    )
}

async fn reserve_bound_channel(
    bound_channels: &BoundChannelMap,
    binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
) -> Result<(), String> {
    let key = (binding.session_id().clone(), binding.channel_id().clone());
    let mut channels = bound_channels.lock().await;
    if let Some(existing) = channels.get(&key) {
        if existing.binding != binding {
            return Err(
                "experimental live channel retains a different runtime incarnation".to_string(),
            );
        }
        if existing.phase != BoundChannelPhase::Stopped {
            return Err("experimental live control binding is already prepared".to_string());
        }
    }
    channels.insert(
        key,
        BoundChannelCustody {
            binding,
            cancellation: CancellationToken::new(),
            completion: Arc::new(tokio::sync::Notify::new()),
            phase: BoundChannelPhase::Prepared,
        },
    );
    Ok(())
}

async fn begin_bound_channel_run(
    bound_channels: &BoundChannelMap,
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
) -> Option<CancellationToken> {
    let key = (binding.session_id().clone(), binding.channel_id().clone());
    let mut channels = bound_channels.lock().await;
    let custody = channels.get_mut(&key)?;
    if custody.binding != *binding || custody.phase != BoundChannelPhase::Prepared {
        return None;
    }
    custody.phase = BoundChannelPhase::Running;
    Some(custody.cancellation.clone())
}

async fn finish_bound_channel_run(
    bound_channels: &BoundChannelMap,
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
) {
    let key = (binding.session_id().clone(), binding.channel_id().clone());
    let completion = {
        let mut channels = bound_channels.lock().await;
        let Some(custody) = channels.get_mut(&key) else {
            return;
        };
        if custody.binding != *binding {
            return;
        }
        custody.phase = BoundChannelPhase::Stopped;
        Arc::clone(&custody.completion)
    };
    completion.notify_waiters();
}

async fn release_bound_channel(
    bound_channels: &BoundChannelMap,
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
) -> Result<(), String> {
    let key = (binding.session_id().clone(), binding.channel_id().clone());
    let (cancellation, completion) = {
        let mut channels = bound_channels.lock().await;
        let Some(custody) = channels.get_mut(&key) else {
            return Ok(());
        };
        if custody.binding != *binding {
            return Err(
                "experimental live deactivation does not match the bound runtime incarnation"
                    .to_string(),
            );
        }
        match custody.phase {
            BoundChannelPhase::Prepared => {
                channels.remove(&key);
                return Ok(());
            }
            BoundChannelPhase::Running => custody.phase = BoundChannelPhase::StopRequested,
            BoundChannelPhase::StopRequested => {}
            BoundChannelPhase::Stopped => {
                channels.remove(&key);
                return Ok(());
            }
        }
        (
            custody.cancellation.clone(),
            Arc::clone(&custody.completion),
        )
    };
    cancellation.cancel();
    loop {
        let completed = completion.notified();
        tokio::pin!(completed);
        completed.as_mut().enable();
        let stopped = bound_channels.lock().await.get(&key).is_none_or(|custody| {
            custody.binding == *binding && custody.phase == BoundChannelPhase::Stopped
        });
        if stopped {
            break;
        }
        completed.await;
    }
    let mut channels = bound_channels.lock().await;
    if channels.get(&key).is_some_and(|custody| {
        custody.binding == *binding && custody.phase == BoundChannelPhase::Stopped
    }) {
        channels.remove(&key);
    }
    Ok(())
}

/// One coordinator per RPC host. Channel actors are fenced by the exact
/// provider binding and retain at most one generated delegation per channel.
#[derive(Clone)]
pub struct ExperimentalLiveDelegationCoordinator {
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    mobs: Arc<crate::MobMcpState>,
    responses_projection_shutdown: Arc<ResponsesProjectionShutdown>,
    responses_prepared: Arc<Mutex<PreparedResponsesMap>>,
    responses_active: Arc<Mutex<ActiveResponsesMap>>,
    responses_pending_retirements: Arc<Mutex<PendingResponsesRetirementMap>>,
    active: Arc<Mutex<std::collections::HashMap<ActiveChannelKey, ActiveDelegation>>>,
    retained: Arc<Mutex<std::collections::HashMap<OperationId, Arc<RetainedDelegation>>>>,
    failed_start_cleanups:
        Arc<Mutex<std::collections::HashMap<OperationId, OwnedDelegationCleanup>>>,
    result_delivery_tasks: Arc<Mutex<std::collections::HashMap<OperationId, JoinHandle<()>>>>,
    pending_result_recoveries: Arc<
        Mutex<
            std::collections::HashMap<
                OperationId,
                meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
            >,
        >,
    >,
    result_recovery_tasks: Arc<Mutex<std::collections::HashMap<OperationId, OwnedResultRecovery>>>,
    active_turns: Arc<Mutex<std::collections::HashMap<ActiveChannelKey, ActiveProviderTurn>>>,
    completed_delegation_turns:
        Arc<Mutex<std::collections::HashMap<CompletedDelegationTurnKey, CompletedDelegationTurn>>>,
    bound_channels: BoundChannelMap,
}

/// Compose the one shared live-delegation lifecycle owner used by RPC and
/// MobKit hosts. The returned typed handle is also the provider binder's
/// erased `ExperimentalLiveBoundChannelActivator`.
#[must_use]
pub fn compose_experimental_live_delegation_coordinator(
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    mobs: Arc<crate::MobMcpState>,
) -> Arc<ExperimentalLiveDelegationCoordinator> {
    Arc::new(ExperimentalLiveDelegationCoordinator::new(runtime, mobs))
}

impl ExperimentalLiveDelegationCoordinator {
    fn classify_absent_responses_executor(
        member: DurableBoundedMemberState,
        phase: meerkat_core::LiveBridgeOperationPhase,
    ) -> ExperimentalResponsesRestartDisposition {
        match member {
            DurableBoundedMemberState::Absent
                if phase == meerkat_core::LiveBridgeOperationPhase::PreFinalInference =>
            {
                ExperimentalResponsesRestartDisposition::NoExecutorBeforeFinalInput
            }
            DurableBoundedMemberState::Absent => ExperimentalResponsesRestartDisposition::Broken {
                reason: "final-input bridge operation has no durable executor child".to_string(),
            },
            _ => ExperimentalResponsesRestartDisposition::Broken {
                reason: "durable executor child exists without stable work admission".to_string(),
            },
        }
    }

    pub fn new(
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
        mobs: Arc<crate::MobMcpState>,
    ) -> Self {
        Self {
            runtime,
            mobs,
            responses_projection_shutdown: Arc::new(ResponsesProjectionShutdown {
                cancellation: CancellationToken::new(),
            }),
            responses_prepared: Arc::new(Mutex::new(std::collections::HashMap::new())),
            responses_active: Arc::new(Mutex::new(std::collections::HashMap::new())),
            responses_pending_retirements: Arc::new(Mutex::new(std::collections::HashMap::new())),
            active: Arc::new(Mutex::new(std::collections::HashMap::new())),
            retained: Arc::new(Mutex::new(std::collections::HashMap::new())),
            failed_start_cleanups: Arc::new(Mutex::new(std::collections::HashMap::new())),
            result_delivery_tasks: Arc::new(Mutex::new(std::collections::HashMap::new())),
            pending_result_recoveries: Arc::new(Mutex::new(std::collections::HashMap::new())),
            result_recovery_tasks: Arc::new(Mutex::new(std::collections::HashMap::new())),
            active_turns: Arc::new(Mutex::new(std::collections::HashMap::new())),
            completed_delegation_turns: Arc::new(Mutex::new(std::collections::HashMap::new())),
            bound_channels: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Whether Meerkat has supplied the durable-fork execution half of
    /// Responses bridging. Provider ingress and settlement Gate 0 remain
    /// independent.
    #[must_use]
    pub const fn responses_executor_available(&self) -> bool {
        true
    }

    async fn reconcile_one_responses_snapshot(
        &self,
        mob_handle: &meerkat_mob::MobHandle,
        snapshot: &meerkat_runtime::live_execution::LiveBridgeRecoverySnapshot,
        observe_until: tokio::time::Instant,
    ) -> ExperimentalResponsesRestartDisposition {
        let operation_id = snapshot.operation().operation_id();
        let child_identity = AgentIdentity::from(format!("live-executor:{operation_id}"));
        let interaction_id = snapshot.operation().domain_correlation().interaction_id();
        let delivery_identity =
            match MobDeliveryIdentity::new(operation_id.to_string(), interaction_id.to_string()) {
                Ok(identity) => identity,
                Err(error) => {
                    return ExperimentalResponsesRestartDisposition::Broken {
                        reason: format!("recovered durable delivery identity is invalid: {error}"),
                    };
                }
            };
        let result_spec =
            match BoundedResultSpec::new("gpt_live_responses", LIVE_DELEGATION_RESULT_BYTES) {
                Ok(spec) => spec,
                Err(error) => {
                    return ExperimentalResponsesRestartDisposition::Broken {
                        reason: format!(
                            "recovered bounded result specification is invalid: {error}"
                        ),
                    };
                }
            };

        loop {
            let recovery = match mob_handle
                .recover_bounded_work_for_identity_with_delivery_identity(
                    &child_identity,
                    &delivery_identity,
                    &result_spec,
                )
                .await
            {
                Ok(recovery) => recovery,
                Err(error) => {
                    return ExperimentalResponsesRestartDisposition::Broken {
                        reason: format!("durable executor recovery observation failed: {error}"),
                    };
                }
            };
            let (member, work) = recovery.into_parts();
            match work {
                DurableBoundedWorkState::Absent => {
                    return Self::classify_absent_responses_executor(member, snapshot.phase());
                }
                DurableBoundedWorkState::Broken { reason, .. } => {
                    return ExperimentalResponsesRestartDisposition::Broken { reason };
                }
                DurableBoundedWorkState::InFlight { .. } => {
                    if tokio::time::Instant::now() >= observe_until {
                        return ExperimentalResponsesRestartDisposition::InFlight;
                    }
                    tokio::time::sleep(RESPONSES_RESTART_OBSERVE_INTERVAL).await;
                }
                DurableBoundedWorkState::Terminal { result, .. } => {
                    let (executor_terminal, executor_output, bridge_terminal) = match result {
                        Ok(turn) => {
                            let output = turn.result().text().to_string();
                            match LiveBridgeOperationTerminal::completed(
                                &output,
                                LIVE_DELEGATION_RESULT_BYTES,
                            ) {
                                Ok(terminal) => (
                                    DurableExecutorTerminalKind::Completed,
                                    Some(output),
                                    terminal,
                                ),
                                Err(error) => {
                                    tracing::warn!(
                                        %error,
                                        %operation_id,
                                        "recovered completed executor has no bridge-eligible output"
                                    );
                                    (
                                        DurableExecutorTerminalKind::Completed,
                                        None,
                                        LiveBridgeOperationTerminal::failed(),
                                    )
                                }
                            }
                        }
                        Err(_error) => (
                            DurableExecutorTerminalKind::Failed,
                            None,
                            LiveBridgeOperationTerminal::failed(),
                        ),
                    };
                    let recovered_digest = live_bridge_execution_result_digest(&bridge_terminal);
                    if let Some(committed_terminal) = snapshot.terminal()
                        && committed_terminal != bridge_terminal.terminal()
                    {
                        return ExperimentalResponsesRestartDisposition::Broken {
                            reason: "recovered executor terminal conflicts with committed bridge terminal"
                                .to_string(),
                        };
                    }
                    if let Some(committed_digest) = snapshot.result_digest()
                        && recovered_digest.as_deref() != Some(committed_digest)
                    {
                        return ExperimentalResponsesRestartDisposition::Broken {
                            reason:
                                "recovered executor output conflicts with committed bridge digest"
                                    .to_string(),
                        };
                    }
                    if snapshot.terminal().is_none()
                        && let Err(error) = self
                            .runtime
                            .reconcile_revoked_live_bridge_execution_terminal(
                                snapshot,
                                bridge_terminal.terminal(),
                                recovered_digest.as_deref(),
                            )
                            .await
                    {
                        return ExperimentalResponsesRestartDisposition::Broken {
                            reason: format!(
                                "recovered executor terminal remains uncommitted: {error}"
                            ),
                        };
                    }
                    let receipt_text = responses_executor_outcome_receipt(
                        operation_id,
                        executor_terminal,
                        executor_output.as_deref(),
                    );
                    let projection = meerkat_core::service::AppendSystemContextRequest {
                        content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                            receipt_text,
                        ),
                        source: Some(format!("gpt-live-responses:{operation_id}")),
                        idempotency_key: Some(format!("gpt-live-responses-outcome:{operation_id}")),
                    };
                    if let Err(error) = self
                        .mobs
                        .session_service()
                        .append_system_context(snapshot.session_id(), projection)
                        .await
                    {
                        return ExperimentalResponsesRestartDisposition::Broken {
                            reason: format!(
                                "durable executor outcome projection remains pending: {error}"
                            ),
                        };
                    }
                    if let Err(error) = self
                        .runtime
                        .record_live_bridge_outcome_receipt(
                            snapshot.session_id(),
                            snapshot.operation(),
                        )
                        .await
                    {
                        return ExperimentalResponsesRestartDisposition::Broken {
                            reason: format!(
                                "durable executor outcome was projected but its machine receipt remains pending: {error}"
                            ),
                        };
                    }
                    if matches!(
                        member,
                        DurableBoundedMemberState::Active { .. }
                            | DurableBoundedMemberState::Retiring { .. }
                    ) && let Err(error) = mob_handle.retire(child_identity.clone()).await
                    {
                        return ExperimentalResponsesRestartDisposition::Broken {
                            reason: format!(
                                "durable executor outcome projected with retirement debt: {error}"
                            ),
                        };
                    }
                    if let Err(error) = self
                        .runtime
                        .retire_settled_live_bridge_operation(
                            snapshot.session_id(),
                            snapshot.operation(),
                        )
                        .await
                    {
                        return ExperimentalResponsesRestartDisposition::Broken {
                            reason: format!(
                                "durable executor outcome projected but bridge retirement remains pending: {error}"
                            ),
                        };
                    }
                    return ExperimentalResponsesRestartDisposition::OutcomeProjected {
                        completed: executor_terminal == DurableExecutorTerminalKind::Completed,
                    };
                }
                _ => {
                    return ExperimentalResponsesRestartDisposition::Broken {
                        reason: "durable executor recovery returned an unsupported work state"
                            .to_string(),
                    };
                }
            }
        }
    }

    /// Reconcile Responses executor work whose process-local waiter was lost.
    ///
    /// Stale provider channels are abandoned before child observation, which
    /// fences every output send and classifies an escaped write as ambiguous.
    /// Work is never submitted, spawned, or retried here. An in-flight input
    /// is observed only until `observation_bound` expires.
    pub async fn reconcile_responses_after_restart(
        &self,
        observation_bound: std::time::Duration,
    ) -> Result<Vec<ExperimentalResponsesRestartReport>, String> {
        let handles = self
            .mobs
            .mob_handles_snapshot()
            .await
            .map_err(|error| error.to_string())?;
        let mut recovered = Vec::new();
        let mut seen_sessions = std::collections::HashSet::new();
        let observe_until = tokio::time::Instant::now() + observation_bound;
        for (_, mob_handle) in handles {
            for member in mob_handle.list_members_including_retiring().await {
                let source_identity = member.agent_identity;
                let Some(session_id) = mob_handle.resolve_bridge_session_id(&source_identity).await
                else {
                    continue;
                };
                if !seen_sessions.insert(session_id.clone()) {
                    continue;
                }
                let snapshots = self
                    .runtime
                    .live_bridge_recovery_snapshots(&session_id)
                    .await
                    .map_err(|error| error.to_string())?;
                for mut snapshot in snapshots {
                    if snapshot.source_agent_identity() != source_identity.as_str() {
                        continue;
                    }
                    let channel_id = snapshot
                        .operation()
                        .domain_correlation()
                        .channel_id()
                        .clone();
                    if self
                        .runtime
                        .live_channel_is_active_for_session(&session_id, &channel_id)
                        .await
                    {
                        self.runtime
                            .abandon_live_open_admission(&session_id, &channel_id)
                            .await
                            .map_err(|error| {
                                format!(
                                    "failed to fence stale live channel '{channel_id}' before recovery: {error}"
                                )
                            })?;
                    } else if snapshot.terminal().is_none()
                        && snapshot.cancellation_reason().is_none()
                    {
                        snapshot = self
                            .runtime
                            .fence_restored_live_bridge_operation_for_restart(&snapshot)
                            .await
                            .map_err(|error| {
                                format!(
                                    "failed to fence restored live bridge operation '{}' before executor observation: {error}",
                                    snapshot.operation().operation_id()
                                )
                            })?;
                    }
                    let disposition = self
                        .reconcile_one_responses_snapshot(&mob_handle, &snapshot, observe_until)
                        .await;
                    recovered.push(ExperimentalResponsesRestartReport {
                        operation_id: snapshot.operation().operation_id().clone(),
                        disposition,
                    });
                }
            }
        }
        Ok(recovered)
    }

    /// Capture the exact durable-member Session clone before machine
    /// admission. The caller derives `canonical_context_revision` from this
    /// opaque value, admits the operation, then supplies the same value to
    /// [`Self::start_admitted_responses_execution`].
    #[doc(hidden)]
    pub async fn prepare_responses_execution_snapshot(
        &self,
        binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> Result<LiveBridgeExecutionSnapshot, String> {
        let current_binding = self
            .runtime
            .live_delegation_runtime_binding(binding.session_id(), binding.channel_id())
            .await
            .map_err(|error| error.to_string())?;
        if &current_binding != binding {
            return Err("live bridge snapshot binding is not current".to_string());
        }
        let (_, _, durable_identity) = self
            .mobs
            .live_member_owner(binding.session_id())
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "live bridge durable Mob member owner is unavailable".to_string())?;
        self.mobs
            .session_service()
            .capture_live_bridge_execution_snapshot(binding.session_id(), durable_identity.as_str())
            .await
            .map_err(|error| error.to_string())
    }

    /// Retain one already-admitted Responses bridge operation until exact
    /// final-user transcript authority permits durable-fork execution.
    ///
    /// Raw provider ingress is intentionally outside this method. Gate 0 must
    /// first construct the exact correlation and obtain the sealed machine
    /// admission. This method rechecks the current runtime incarnation and
    /// authoritative durable Mob member, but intentionally starts no executor
    /// work. [`Self::confirm_responses_final_input`] owns the exact-boundary
    /// durable fork.
    #[doc(hidden)]
    pub async fn start_admitted_responses_execution(
        &self,
        admission: LiveBridgeOperationAdmission,
        snapshot: LiveBridgeExecutionSnapshot,
        semantic_request: String,
    ) -> Result<ExperimentalLiveBridgeExecutionWaiter, String> {
        let derived_digest = meerkat_core::LiveBridgeRequestDigest::derive(&semantic_request)
            .map_err(|error| error.to_string())?;
        if admission.request_digest() != &derived_digest {
            return Err("live bridge request does not match the sealed admission".to_string());
        }
        let current_binding = self
            .runtime
            .live_delegation_runtime_binding(
                admission.session_id(),
                admission.binding().channel_id(),
            )
            .await
            .map_err(|error| error.to_string())?;
        let (_, mob_handle, durable_identity) = self
            .mobs
            .live_member_owner(admission.session_id())
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "live bridge durable Mob member owner is unavailable".to_string())?;
        if !live_bridge_admission_matches_current_owner(
            &admission,
            &current_binding,
            &durable_identity,
        ) {
            return Err(
                "live bridge admission does not match the current durable member owner".to_string(),
            );
        }

        let admission = Arc::new(admission);
        let operation_id = admission.operation().operation_id().clone();
        if snapshot.session().id() != admission.session_id()
            || snapshot.agent_identity() != admission.agent_identity()
            || snapshot.canonical_context_revision() != admission.canonical_context_revision()
        {
            return Err(
                "live bridge admission does not match its retained execution snapshot".to_string(),
            );
        }
        let (completion_tx, completion_rx) = oneshot::channel();
        let mut prepared = self.responses_prepared.lock().await;
        if prepared.contains_key(&operation_id)
            || self
                .responses_active
                .lock()
                .await
                .contains_key(&operation_id)
        {
            return Err("live bridge operation already has local execution custody".to_string());
        }
        prepared.insert(
            operation_id.clone(),
            PreparedResponsesExecution {
                admission,
                mob_handle,
                source_identity: durable_identity,
                semantic_request,
                completion: completion_tx,
            },
        );
        Ok(ExperimentalLiveBridgeExecutionWaiter {
            operation_id,
            completion: completion_rx,
        })
    }

    /// Start a real durable executor fork only after exact canonical transcript
    /// evidence for the active operation. Provider prose, request digests, and
    /// inferred turn boundaries cannot cross this seam.
    pub async fn confirm_responses_final_input(
        &self,
        evidence: &FinalLiveUserTranscriptCommitEvidence,
    ) -> Result<(), String> {
        let mut prepared = self.responses_prepared.lock().await;
        let matching_ids = prepared
            .iter()
            .filter_map(|(operation_id, execution)| {
                let correlation = execution.admission.operation().domain_correlation();
                (execution.admission.session_id() == evidence.session_id()
                    && correlation.channel_id() == evidence.channel_id()
                    && correlation.interaction_id() == evidence.interaction_id())
                .then_some(operation_id.clone())
            })
            .collect::<Vec<_>>();
        let operation_id = matching_ids
            .first()
            .cloned()
            .ok_or_else(|| "final input has no active live bridge operation".to_string())?;
        if matching_ids.len() != 1 {
            return Err("final input matches multiple active live bridge operations".to_string());
        }
        let admission = Arc::clone(
            &prepared
                .get(&operation_id)
                .ok_or_else(|| "prepared live bridge custody disappeared".to_string())?
                .admission,
        );
        self.runtime
            .confirm_live_bridge_final_input(admission.as_ref(), evidence)
            .await
            .map_err(|error| error.to_string())?;
        let committed_message_count = evidence.committed_message_count().ok_or_else(|| {
            "committed final input is missing its exact transcript boundary".to_string()
        })?;
        let _start_authority = self
            .runtime
            .authorize_live_bridge_execution_start(admission.as_ref())
            .await
            .map_err(|error| error.to_string())?;

        let execution = prepared
            .remove(&operation_id)
            .ok_or_else(|| "prepared live bridge custody disappeared".to_string())?;
        let child_identity = AgentIdentity::from(format!("live-executor:{operation_id}"));
        let interaction_id = admission.operation().domain_correlation().interaction_id();
        let delivery_identity =
            MobDeliveryIdentity::new(operation_id.to_string(), interaction_id.to_string())
                .map_err(|error| format!("live executor delivery identity rejected: {error}"))?;
        let result_spec =
            BoundedResultSpec::new("gpt_live_responses", LIVE_DELEGATION_RESULT_BYTES)
                .map_err(|error| error.to_string())?;
        let service = DelegationExecutionService::new(execution.mob_handle);
        let request = DelegationExecutionRequest::new(
            child_identity,
            responses_executor_task(&execution.semantic_request),
            result_spec,
        )
        .with_durable_fork(execution.source_identity, Some(committed_message_count))
        .with_delivery_identity(delivery_identity, interaction_id);
        let delegated = match service.start(request).await {
            Ok(delegated) => delegated,
            Err(error) => {
                drop(prepared);
                let terminal = LiveBridgeOperationTerminal::failed();
                let committed = record_live_bridge_terminal_across_revocation(
                    self.runtime.as_ref(),
                    admission.as_ref(),
                    terminal.terminal(),
                    None,
                )
                .await
                .map_err(|record_error| record_error.to_string())?;
                let waiter_outcome = match committed {
                    LiveBridgeTerminalCommit::Active(receipt) => {
                        Ok(ExperimentalLiveBridgeExecutionCompletion {
                            terminal: receipt,
                            output: None,
                        })
                    }
                    LiveBridgeTerminalCommit::Revoked(receipt) => Err(format!(
                        "provider channel was revoked; executor start failure was durably reconciled for operation {} without submission authority",
                        receipt.operation().operation_id()
                    )),
                };
                let _ = execution.completion.send(waiter_outcome);
                return Err(format!("durable live executor fork failed: {error}"));
            }
        };

        let terminal_custody = Arc::new(Mutex::new(None));
        let task_terminal_custody = Arc::clone(&terminal_custody);
        let delivery_fenced = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_delivery_fenced = Arc::clone(&delivery_fenced);
        let runtime = Arc::clone(&self.runtime);
        let session_service = self.mobs.session_service();
        let source_session_id = admission.session_id().clone();
        let active = Arc::clone(&self.responses_active);
        let pending_retirements = Arc::clone(&self.responses_pending_retirements);
        let task_admission = Arc::clone(&admission);
        let task_operation_id = operation_id.clone();
        let projection_shutdown = self.responses_projection_shutdown.cancellation.clone();
        let (start_tx, start_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = start_rx.await;
            let terminalized = delegated.await_terminal().await;
            let (executor_terminal, executor_output, ordinary_terminal) =
                match terminalized.terminal() {
                    DelegationTurnTerminal::Completed(turn) => {
                        let output = turn.result().result().text().to_string();
                        match LiveBridgeOperationTerminal::completed(
                            &output,
                            LIVE_DELEGATION_RESULT_BYTES,
                        ) {
                            Ok(terminal) => (
                                DurableExecutorTerminalKind::Completed,
                                Some(output),
                                terminal,
                            ),
                            Err(error) => {
                                tracing::warn!(
                                    %error,
                                    %task_operation_id,
                                    "completed durable executor returned no bridge-eligible output"
                                );
                                (
                                    DurableExecutorTerminalKind::Completed,
                                    None,
                                    LiveBridgeOperationTerminal::failed(),
                                )
                            }
                        }
                    }
                    DelegationTurnTerminal::Failed(_) => (
                        DurableExecutorTerminalKind::Failed,
                        None,
                        LiveBridgeOperationTerminal::failed(),
                    ),
                    _ => (
                        DurableExecutorTerminalKind::Failed,
                        None,
                        LiveBridgeOperationTerminal::failed(),
                    ),
                };
            let retirement_error = service
                .retire_terminalized(&terminalized)
                .await
                .err()
                .map(|error| error.to_string());
            // Retain the ordinary executor's actual physical outcome before
            // attempting the idempotent source projection. A transient or
            // ambiguous append cannot reopen cancellation or let local
            // operation custody disappear.
            let bridge_terminal = ordinary_terminal;
            let result_digest = live_bridge_execution_result_digest(&bridge_terminal);
            *task_terminal_custody.lock().await = Some(PendingResponsesTerminalCustody {
                executor_terminal,
                bridge_terminal: bridge_terminal.clone(),
                result_digest: result_digest.clone(),
                retirement_error: retirement_error.clone(),
            });
            // The durable LiveBridge execution terminal records the ordinary
            // executor's actual physical outcome. Cancellation/supersession
            // fences provider delivery but never rewrites or delays this fact.
            let delivery_fenced = task_delivery_fenced.load(std::sync::atomic::Ordering::Acquire);
            let committed = record_live_bridge_terminal_across_revocation(
                runtime.as_ref(),
                task_admission.as_ref(),
                bridge_terminal.terminal(),
                result_digest.as_deref(),
            )
            .await;
            let terminal_committed = committed.is_ok();
            let outcome = match committed {
                Ok(LiveBridgeTerminalCommit::Active(receipt)) => {
                    Ok(ExperimentalLiveBridgeExecutionCompletion {
                        terminal: receipt,
                        output: provider_output_after_delivery_fence(
                            bridge_terminal,
                            delivery_fenced,
                        ),
                    })
                }
                Ok(LiveBridgeTerminalCommit::Revoked(receipt)) => Err(format!(
                    "provider channel was revoked; executor terminal was durably reconciled for operation {} without submission authority",
                    receipt.operation().operation_id()
                )),
                Err(error) => Err(error.to_string()),
            };
            let _ = execution.completion.send(outcome);
            if !terminal_committed {
                return;
            }

            let receipt_text = responses_executor_outcome_receipt(
                &task_operation_id,
                executor_terminal,
                executor_output.as_deref(),
            );
            let projection = meerkat_core::service::AppendSystemContextRequest {
                content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(receipt_text),
                source: Some(format!("gpt-live-responses:{task_operation_id}")),
                idempotency_key: Some(format!("gpt-live-responses-outcome:{task_operation_id}")),
            };
            let Some(_append_status) = retry_responses_outcome_custody_step(
                &task_operation_id,
                &projection_shutdown,
                "source-context-append",
                || session_service.append_system_context(&source_session_id, projection.clone()),
            )
            .await
            else {
                return;
            };
            let Some(()) = retry_responses_outcome_custody_step(
                &task_operation_id,
                &projection_shutdown,
                "machine-outcome-receipt",
                || {
                    runtime.record_live_bridge_outcome_receipt(
                        task_admission.session_id(),
                        task_admission.operation(),
                    )
                },
            )
            .await
            else {
                return;
            };
            let retirement_disposition = drive_responses_retirement_after_persisted_fact(
                runtime.as_ref(),
                pending_retirements.as_ref(),
                task_admission.session_id(),
                task_admission.operation(),
                &projection_shutdown,
            )
            .await;
            if retirement_disposition == LiveBridgeRetirementDisposition::Shutdown {
                return;
            }

            if retirement_error.is_none() {
                task_terminal_custody.lock().await.take();
                active.lock().await.remove(&task_operation_id);
            } else if let Some(error) = retirement_error.as_deref() {
                tracing::warn!(
                    %error,
                    operation_id = %task_operation_id,
                    "durable executor terminal was preserved with unresolved ordinary retirement debt"
                );
            }
        });
        self.responses_active.lock().await.insert(
            operation_id,
            ActiveResponsesExecution {
                admission,
                delivery_fenced,
                terminal_custody,
                _task: task,
            },
        );
        drop(prepared);
        let _ = start_tx.send(());
        Ok(())
    }

    async fn cancel_responses_executions_for_binding(&self, binding: &ProviderWebrtcBinding) {
        let prepared_ids = {
            let prepared = self.responses_prepared.lock().await;
            prepared
                .iter()
                .filter_map(|(operation_id, execution)| {
                    let admission = execution.admission.as_ref();
                    (admission.session_id() == binding.session_id()
                        && admission.binding().channel_id() == binding.channel_id()
                        && admission.binding().generation() == binding.runtime_generation().get()
                        && admission.binding().fence_token() == binding.runtime_fence().get())
                    .then_some(operation_id.clone())
                })
                .collect::<Vec<_>>()
        };
        for operation_id in prepared_ids {
            let mut prepared = self.responses_prepared.lock().await;
            let Some(execution) = prepared.get(&operation_id) else {
                continue;
            };
            let admission = Arc::clone(&execution.admission);
            if let Err(error) = self
                .runtime
                .cancel_live_bridge_operation(
                    admission.as_ref(),
                    meerkat_core::LiveBridgeCancellationReason::ChannelClose,
                )
                .await
            {
                tracing::warn!(%error, %operation_id, "live bridge cancellation failed closed before executor fork");
                continue;
            }
            let Some(execution) = prepared.remove(&operation_id) else {
                tracing::warn!(
                    %operation_id,
                    "prepared live bridge custody disappeared during cancellation"
                );
                continue;
            };
            drop(prepared);
            let terminal = LiveBridgeOperationTerminal::cancelled();
            let outcome = record_live_bridge_terminal_with_typed_recovery(|| {
                self.runtime.record_live_bridge_execution_terminal(
                    admission.as_ref(),
                    terminal.terminal(),
                    None,
                )
            })
            .await
            .map(|receipt| ExperimentalLiveBridgeExecutionCompletion {
                terminal: receipt,
                output: None,
            })
            .map_err(|error| error.to_string());
            let _ = execution.completion.send(outcome);
        }
        let operation_ids = {
            let active = self.responses_active.lock().await;
            active
                .iter()
                .filter_map(|(operation_id, execution)| {
                    (execution.admission.session_id() == binding.session_id()
                        && execution.admission.binding().channel_id() == binding.channel_id()
                        && execution.admission.binding().generation()
                            == binding.runtime_generation().get()
                        && execution.admission.binding().fence_token()
                            == binding.runtime_fence().get())
                    .then_some(operation_id.clone())
                })
                .collect::<Vec<_>>()
        };
        for operation_id in operation_ids {
            let Some((admission, delivery_fenced, terminal_custody)) = self
                .responses_active
                .lock()
                .await
                .get(&operation_id)
                .map(|execution| {
                    (
                        Arc::clone(&execution.admission),
                        Arc::clone(&execution.delivery_fenced),
                        Arc::clone(&execution.terminal_custody),
                    )
                })
            else {
                continue;
            };
            let terminal_custody = terminal_custody.lock().await;
            if fence_provider_delivery_for_accepted_terminal(
                terminal_custody.as_ref(),
                delivery_fenced.as_ref(),
            ) {
                let terminal = terminal_custody
                    .as_ref()
                    .map(|pending| pending.bridge_terminal.terminal());
                let executor_terminal = terminal_custody
                    .as_ref()
                    .map(|pending| pending.executor_terminal);
                let has_result_digest = terminal_custody
                    .as_ref()
                    .is_some_and(|pending| pending.result_digest.is_some());
                let retirement_pending = terminal_custody
                    .as_ref()
                    .is_some_and(|pending| pending.retirement_error.is_some());
                tracing::debug!(
                    %operation_id,
                    ?terminal,
                    ?executor_terminal,
                    has_result_digest,
                    retirement_pending,
                    "accepted live bridge terminal retains recovery custody across channel shutdown"
                );
                continue;
            }
            delivery_fenced.store(true, std::sync::atomic::Ordering::Release);
            let cancellation = self
                .runtime
                .cancel_live_bridge_operation(
                    admission.as_ref(),
                    meerkat_core::LiveBridgeCancellationReason::ChannelClose,
                )
                .await;
            if let Err(error) = cancellation {
                tracing::warn!(
                    %error,
                    %operation_id,
                    "live bridge cancellation authority failed closed; provider delivery remains fenced while the exact executor drains"
                );
            } else {
                tracing::debug!(%operation_id, "live bridge result delivery fenced; ordinary executor drains to terminal");
            }
            drop(terminal_custody);
        }
    }

    /// Authorize exact function output only after independent Meerkat
    /// execution terminality has been recorded. This does not claim or send.
    pub async fn authorize_responses_submission(
        &self,
        completion: &ExperimentalLiveBridgeExecutionCompletion,
        output_kind: meerkat_core::LiveBridgeOutputKind,
        exact_output: &str,
    ) -> Result<LiveBridgeSubmissionAuthority, String> {
        let digest = live_bridge_submission_output_digest(exact_output)?;
        self.runtime
            .authorize_live_bridge_submission(completion.terminal(), output_kind, &digest)
            .await
            .map_err(|error| error.to_string())
    }

    /// Consume the one durable pre-IO claim. A failure before this method
    /// returns is pre-accept. After it returns, callers must not retry by
    /// claiming another attempt.
    pub async fn claim_responses_submission_attempt(
        &self,
        submission: &LiveBridgeSubmissionAuthority,
    ) -> Result<LiveBridgeSubmissionAttemptAuthority, String> {
        self.runtime
            .claim_live_bridge_submission_attempt(submission)
            .await
            .map_err(|error| error.to_string())
    }

    /// Record only that the exact output reached the local transport write
    /// boundary. Provider processing remains unresolved.
    pub async fn record_responses_submission_local_write(
        &self,
        attempt: LiveBridgeSubmissionAttemptAuthority,
    ) -> Result<LiveBridgeSubmissionReceipt, String> {
        self.runtime
            .record_live_bridge_submission_local_write(attempt)
            .await
            .map_err(|error| error.to_string())
    }

    /// Settle the server-owned Responses call from an exact provider
    /// observation. Local write alone must never call this as processed.
    pub async fn resolve_responses_submission(
        &self,
        submission: &LiveBridgeSubmissionAuthority,
        observation: meerkat_core::LiveBridgeSubmissionObservation,
    ) -> Result<LiveBridgeSubmissionReceipt, String> {
        let receipt = self
            .runtime
            .resolve_live_bridge_submission(submission, observation)
            .await
            .map_err(|error| error.to_string())?;
        let admission = submission.terminal().admission();
        drive_responses_retirement_after_persisted_fact(
            self.runtime.as_ref(),
            self.responses_pending_retirements.as_ref(),
            admission.session_id(),
            admission.operation(),
            &self.responses_projection_shutdown.cancellation,
        )
        .await;
        Ok(receipt)
    }

    /// Reconcile a claimed submission whose process lost exact settlement.
    /// The receipt has no transport authority and cannot resend.
    pub async fn recover_responses_submission(
        &self,
        completion: &ExperimentalLiveBridgeExecutionCompletion,
    ) -> Result<LiveBridgeRecoveredSubmissionReceipt, String> {
        let admission = completion.terminal().admission();
        let receipt = self
            .runtime
            .recover_live_bridge_submission(admission.session_id(), admission.operation())
            .await
            .map_err(|error| error.to_string())?;
        drive_responses_retirement_after_persisted_fact(
            self.runtime.as_ref(),
            self.responses_pending_retirements.as_ref(),
            admission.session_id(),
            admission.operation(),
            &self.responses_projection_shutdown.cancellation,
        )
        .await;
        Ok(receipt)
    }

    async fn settle_responses_retirement_debt_for_binding(&self, binding: &ProviderWebrtcBinding) {
        let pending = self
            .responses_pending_retirements
            .lock()
            .await
            .values()
            .filter(|(session_id, operation)| {
                session_id == binding.session_id()
                    && operation.domain_correlation().channel_id() == binding.channel_id()
            })
            .cloned()
            .collect::<Vec<_>>();
        for (session_id, operation) in pending {
            drive_responses_retirement_after_persisted_fact(
                self.runtime.as_ref(),
                self.responses_pending_retirements.as_ref(),
                &session_id,
                &operation,
                &self.responses_projection_shutdown.cancellation,
            )
            .await;
        }
    }

    async fn prepare_bound_channel(
        &self,
        runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
    ) -> Result<(), String> {
        control
            .active_binding(runtime_binding.session_id())
            .await
            .filter(|binding| {
                binding.channel_id() == runtime_binding.channel_id()
                    && binding.runtime_generation().get() == runtime_binding.generation()
                    && binding.runtime_fence().get() == runtime_binding.fence_token()
            })
            .ok_or_else(|| "experimental live control binding is unavailable".to_string())?;
        reserve_bound_channel(&self.bound_channels, runtime_binding).await
    }

    async fn run_bound_channel(
        &self,
        runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
    ) {
        let Some(cancellation) =
            begin_bound_channel_run(&self.bound_channels, &runtime_binding).await
        else {
            return;
        };
        let binding = control
            .active_binding(runtime_binding.session_id())
            .await
            .filter(|binding| {
                binding.channel_id() == runtime_binding.channel_id()
                    && binding.runtime_generation().get() == runtime_binding.generation()
                    && binding.runtime_fence().get() == runtime_binding.fence_token()
            });
        if let Some(binding) = binding {
            self.run_channel(binding, control, cancellation).await;
        } else {
            self.cancel_channel_binding(&provider_binding_from_runtime(&runtime_binding))
                .await;
        }
        finish_bound_channel_run(&self.bound_channels, &runtime_binding).await;
    }

    async fn deactivate_bound_channel(
        &self,
        runtime_binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> Result<(), String> {
        release_bound_channel(&self.bound_channels, runtime_binding).await?;
        self.cancel_channel_binding(&provider_binding_from_runtime(runtime_binding))
            .await;
        Ok(())
    }

    /// Candidate-only projection used by the direct Gate0 harness to ask the
    /// SessionDocument owner to seal the provider-final transcript for the
    /// exact provisional handoff already admitted by this coordinator.
    ///
    /// This returns no worker, tool, release, or provider authority. The
    /// caller must still obtain sealed final evidence from the session owner
    /// and return it through `reconcile_exact_final`.
    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[doc(hidden)]
    pub async fn __gate0_candidate_provisional(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_core::LiveChannelId,
        turn_adapter_key: &str,
    ) -> Result<ProvisionalLiveHandoff, String> {
        if turn_adapter_key.trim().is_empty() {
            return Err("Gate0 final turn key is empty".to_string());
        }
        self.retained
            .lock()
            .await
            .values()
            .find(|retained| {
                retained.runtime_binding.session_id() == session_id
                    && retained.runtime_binding.channel_id() == channel_id
                    && retained.provisional.correlation().provider().user_turn_id()
                        == turn_adapter_key
            })
            .map(|retained| retained.provisional.clone())
            .ok_or_else(|| "Gate0 final turn has no exact admitted provisional handoff".to_string())
    }

    #[allow(
        clippy::while_let_loop,
        reason = "the select loop has explicit cancellation, stream-end, binding-mismatch, and error exits"
    )]
    async fn run_channel(
        &self,
        binding: ProviderWebrtcBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
        cancellation: CancellationToken,
    ) {
        loop {
            let observation = match tokio::select! {
                biased;
                () = cancellation.cancelled() => break,
                observation = control.next_observation(&binding) => observation,
            } {
                Ok(Some(observation)) => observation,
                Ok(None) | Err(_) => break,
            };
            match observation {
                ExperimentalGptLiveControlObservation::Provider(observation) => {
                    if observation.binding() != &binding {
                        break;
                    }
                    if let LiveSidebandObservationKind::DelegationRequested {
                        turn,
                        delegation,
                        final_transcript,
                    } = observation.kind()
                        && let Err(error) = self
                            .start_client_context_delegation(
                                &binding,
                                Arc::clone(&control),
                                turn.clone(),
                                delegation.clone(),
                                final_transcript.clone(),
                            )
                            .await
                    {
                        tracing::warn!(error, "experimental live delegation start failed closed");
                    }
                }
                ExperimentalGptLiveControlObservation::AppendResolved(resolution) => {
                    let (authority, outcome) = resolution.into_parts();
                    let runtime_binding = match self
                        .runtime
                        .live_delegation_runtime_binding(
                            authority.session_id(),
                            authority.channel_id(),
                        )
                        .await
                    {
                        Ok(binding) => binding,
                        Err(error) => {
                            tracing::warn!(%error, "live append resolution lost runtime binding");
                            continue;
                        }
                    };
                    if let Err(error) = self
                        .runtime
                        .resolve_live_context_append(
                            runtime_binding.runtime_id(),
                            runtime_binding.fence_token(),
                            runtime_binding.generation(),
                            &authority,
                            outcome,
                        )
                        .await
                    {
                        tracing::warn!(%error, "generated live append resolution failed");
                    }
                }
                ExperimentalGptLiveControlObservation::ResultDeliveryResolved(resolution) => {
                    let (authority, observation) = resolution.into_parts();
                    let operation_id = authority.operation().operation_id().clone();
                    let resolution = retry_reconciled_cleanup_step(
                        "fallback-result-delivery-resolution",
                        || {
                            self.runtime
                                .resolve_live_delegation_result_delivery(&authority, observation)
                        },
                    )
                    .await;
                    match resolution {
                        LiveDelegationResultDeliveryResolution::Resolved(receipt)
                            if !receipt.retry_allowed() =>
                        {
                            let retained = self.retained.lock().await.get(&operation_id).cloned();
                            if let Some(retained) = retained {
                                self.remove_retained_delegation(&retained).await;
                            }
                        }
                        LiveDelegationResultDeliveryResolution::AmbiguityRecovery(recovery) => {
                            self.retain_and_realize_result_recovery(recovery).await;
                            let retained = self.retained.lock().await.get(&operation_id).cloned();
                            if let Some(retained) = retained {
                                self.remove_retained_delegation(&retained).await;
                            }
                        }
                        LiveDelegationResultDeliveryResolution::Resolved(_) => {
                            tracing::warn!(
                                %operation_id,
                                "generated fallback result delivery unexpectedly allowed retry"
                            );
                        }
                    }
                }
            }
        }
        self.cancel_channel_binding(&binding).await;
    }

    async fn observe_provider_lifecycle(
        &self,
        observation: &LiveSidebandObservation,
    ) -> Result<(), String> {
        let key = (
            observation.binding().session_id().clone(),
            observation.binding().channel_id().clone(),
        );
        let bound = self
            .bound_channels
            .lock()
            .await
            .get(&key)
            .is_some_and(|custody| {
                custody.binding.session_id() == observation.binding().session_id()
                    && custody.binding.channel_id() == observation.binding().channel_id()
                    && custody.binding.generation()
                        == observation.binding().runtime_generation().get()
                    && custody.binding.fence_token() == observation.binding().runtime_fence().get()
                    && matches!(
                        custody.phase,
                        BoundChannelPhase::Prepared | BoundChannelPhase::Running
                    )
            });
        if !bound {
            return Err(
                "provider lifecycle observation has no exact running channel custody".to_string(),
            );
        }
        match observation.kind() {
            LiveSidebandObservationKind::TurnStarted {
                role: meerkat_live::LiveSidebandTurnRole::User,
                ..
            } => self.observe_turn_started(observation).await,
            LiveSidebandObservationKind::TurnFinished {
                role: meerkat_live::LiveSidebandTurnRole::User,
                ..
            } => self.observe_turn_finished(observation).await,
            LiveSidebandObservationKind::DelegationRequested {
                delegation,
                turn,
                final_transcript,
                ..
            } => {
                // In client mode the joined delegation is the provider's sole
                // terminal observation for this user turn. Project that exact
                // terminal fact into conversational lifecycle authority before
                // the provider can begin its assistant acknowledgement. The
                // ordinary transcript adapter must not see a duplicate user
                // final - canonical delegation commitment remains below.
                let terminal = LiveSidebandObservation::new(
                    observation.binding().clone(),
                    LiveSidebandObservationKind::TurnFinished {
                        turn: turn.clone(),
                        role: meerkat_live::LiveSidebandTurnRole::User,
                        transcript: final_transcript.clone(),
                    },
                );
                self.observe_delegation_turn_finished(&terminal, delegation, final_transcript)
                    .await
            }
            LiveSidebandObservationKind::TurnStarted { .. }
            | LiveSidebandObservationKind::TurnFinished { .. }
            | LiveSidebandObservationKind::TurnSnapshotDelta { .. } => Ok(()),
            _ => Err("provider lifecycle seam received a non-lifecycle observation".to_string()),
        }
    }

    async fn observe_turn_started(
        &self,
        observation: &LiveSidebandObservation,
    ) -> Result<(), String> {
        let authority = self
            .runtime
            .observe_live_provider_turn_started(observation)
            .await
            .map_err(|error| error.to_string())?;
        let key = (
            authority.binding().session_id().clone(),
            authority.binding().channel_id().clone(),
        );
        let mut turns = self.active_turns.lock().await;
        if turns.contains_key(&key) {
            return Err(
                "provider turn start duplicated an active local turn projection".to_string(),
            );
        }
        turns.insert(key, ActiveProviderTurn { authority });
        Ok(())
    }

    async fn observe_turn_finished(
        &self,
        observation: &LiveSidebandObservation,
    ) -> Result<(), String> {
        let finished = self
            .runtime
            .observe_live_provider_turn_finished(observation)
            .await
            .map_err(|error| error.to_string())?;
        let key = (
            finished.binding().session_id().clone(),
            finished.binding().channel_id().clone(),
        );
        let started = self.active_turns.lock().await.remove(&key).ok_or_else(|| {
            "provider turn finish has no local started-turn projection".to_string()
        })?;
        if started.authority.binding() != finished.binding()
            || started.authority.interaction_id() != finished.interaction_id()
            || started.authority.provider_turn_ref() != finished.provider_turn_ref()
        {
            return Err("provider turn finish does not match the exact started turn".to_string());
        }
        self.runtime
            .drain_live_context_outbox(finished.binding().session_id())
            .await
            .map_err(|error| error.to_string())
    }

    async fn observe_delegation_turn_finished(
        &self,
        observation: &LiveSidebandObservation,
        delegation: &LiveSidebandDelegationRef,
        final_transcript: &str,
    ) -> Result<(), String> {
        let provider_binding = observation.binding();
        let channel_key = (
            provider_binding.session_id().clone(),
            provider_binding.channel_id().clone(),
        );
        let started = self
            .active_turns
            .lock()
            .await
            .get(&channel_key)
            .map(|active| active.authority.clone())
            .ok_or_else(|| {
                "client delegation final has no local started-turn projection".to_string()
            })?;
        let LiveSidebandObservationKind::TurnFinished { turn, .. } = observation.kind() else {
            return Err("client delegation final requires a typed terminal user turn".to_string());
        };
        if started.binding().session_id() != provider_binding.session_id()
            || started.binding().channel_id() != provider_binding.channel_id()
            || started.binding().fence_token() != provider_binding.runtime_fence().get()
            || started.binding().generation() != provider_binding.runtime_generation().get()
            || started.provider_turn_ref() != turn.adapter_key()
        {
            return Err(
                "client delegation final does not match the exact started turn".to_string(),
            );
        }
        let provider_correlation =
            OpaqueProviderCorrelation::new(delegation.adapter_key(), turn.adapter_key())
                .map_err(|error| error.to_string())?;
        let correlation = LiveUserTurnCorrelation::new(
            provider_binding.channel_id().clone(),
            started.interaction_id(),
            provider_correlation,
        )
        .map_err(|error| error.to_string())?;
        let runtime_binding = self
            .runtime
            .live_delegation_runtime_binding(
                provider_binding.session_id(),
                correlation.channel_id(),
            )
            .await
            .map_err(|error| error.to_string())?;
        if runtime_binding.fence_token() != provider_binding.runtime_fence().get()
            || runtime_binding.generation() != provider_binding.runtime_generation().get()
        {
            return Err("delegation observation has a stale runtime binding".to_string());
        }
        let operation = ExactOperationIdentity::for_domain(OperationId::new(), correlation);
        let provisional = ProvisionalLiveHandoff::new(
            operation.domain_correlation().clone(),
            final_transcript,
            LiveHandoffInputProvenance::ProvisionalTranscriptSnapshot,
        )
        .map_err(|error| error.to_string())?;
        // The provider can start its assistant acknowledgement immediately
        // after the joined delegation turn. Admit the exact provisional join
        // while the generated interaction is still active, then close the
        // conversational turn. Canonical transcript reconciliation and all
        // executor authority remain control-owned below.
        self.runtime
            .admit_live_delegation(&runtime_binding, &operation, &provisional)
            .await
            .map_err(|error| error.to_string())?;
        let finished = self
            .runtime
            .observe_live_provider_turn_finished(observation)
            .await
            .map_err(|error| error.to_string())?;
        self.active_turns.lock().await.remove(&channel_key);
        if started.binding() != finished.binding()
            || started.interaction_id() != finished.interaction_id()
            || started.provider_turn_ref() != finished.provider_turn_ref()
        {
            return Err(
                "client delegation final does not match the exact finished turn".to_string(),
            );
        }
        let completed_key = (
            finished.binding().session_id().clone(),
            finished.binding().channel_id().clone(),
            finished.provider_turn_ref().to_string(),
        );
        if self
            .completed_delegation_turns
            .lock()
            .await
            .insert(
                completed_key,
                CompletedDelegationTurn {
                    authority: finished.clone(),
                    operation,
                    provisional,
                    runtime_binding,
                    delegation_ref: delegation.adapter_key().to_string(),
                },
            )
            .is_some()
        {
            return Err("client delegation final duplicated completed-turn custody".to_string());
        }
        self.runtime
            .drain_live_context_outbox(finished.binding().session_id())
            .await
            .map_err(|error| error.to_string())
    }

    /// Client-context capability only. The provider-final transcript remains
    /// provisional until the canonical session owner commits it and runtime
    /// reconciliation confirms the exact digest. No executor model or tool
    /// work starts before that boundary.
    async fn start_client_context_delegation(
        &self,
        provider_binding: &ProviderWebrtcBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
        turn: LiveSidebandTurnRef,
        delegation: LiveSidebandDelegationRef,
        final_transcript: String,
    ) -> Result<(), String> {
        tracing::debug!("client-context control received an exact delegation join");
        let session_id = provider_binding.session_id();
        let channel_key = (session_id.clone(), provider_binding.channel_id().clone());
        let completed_key = (
            session_id.clone(),
            provider_binding.channel_id().clone(),
            turn.adapter_key().to_string(),
        );
        let completed_turn = self
            .completed_delegation_turns
            .lock()
            .await
            .get(&completed_key)
            .cloned()
            .ok_or_else(|| "delegation has no exact completed provider turn".to_string())?;
        if completed_turn.authority.binding().channel_id() != provider_binding.channel_id()
            || completed_turn.authority.binding().session_id() != session_id
            || completed_turn.authority.provider_turn_ref() != turn.adapter_key()
            || completed_turn.delegation_ref != delegation.adapter_key()
            || completed_turn.provisional.executor_input() != final_transcript
        {
            return Err(
                "delegation turn ref does not match the completed generated turn".to_string(),
            );
        }
        let operation = completed_turn.operation;
        let provisional = completed_turn.provisional;
        let runtime_binding = completed_turn.runtime_binding;
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "client-context control claimed pre-admitted delegation custody"
        );
        if runtime_binding.fence_token() != provider_binding.runtime_fence().get()
            || runtime_binding.generation() != provider_binding.runtime_generation().get()
        {
            return Err("delegation observation has a stale runtime binding".to_string());
        }
        let (_, mob_handle, source_identity) = self
            .mobs
            .live_member_owner(session_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                "live delegation requires a durable Meerkat-Mob member owner".to_string()
            })?;
        let final_event = meerkat_core::RealtimeTranscriptEvent::UserTranscriptFinal {
            item_id: turn.adapter_key().to_string(),
            previous_item_id: None,
            content_index: 0,
            text: final_transcript.clone(),
        };
        let final_evidence = self
            .mobs
            .session_service()
            .commit_live_delegation_final_transcript(session_id, provisional.clone(), final_event)
            .await
            .map_err(|error| error.to_string())?;
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "client-context control committed canonical final transcript"
        );

        if let Some(previous) = self.active.lock().await.remove(&channel_key) {
            if previous.task.is_finished() {
                previous
                    .task
                    .await
                    .map_err(|error| format!("live delegation terminal task failed: {error}"))?;
            } else {
                let directive = match self
                    .runtime
                    .supersede_live_delegation(
                        runtime_binding.runtime_id(),
                        runtime_binding.fence_token(),
                        runtime_binding.generation(),
                        &previous.retained.admission,
                        operation.domain_correlation().interaction_id(),
                    )
                    .await
                {
                    Ok(directive) => Some(directive),
                    Err(_error) if previous.task.is_finished() => None,
                    Err(error) => {
                        self.active.lock().await.insert(channel_key, previous);
                        return Err(error.to_string());
                    }
                };
                if let Some(LiveDelegationCancellationDirective::CancellationAuthorized(
                    cancellation,
                )) = directive
                {
                    let outcome = previous
                        .cancellation
                        .cancel(&cancellation)
                        .await
                        .unwrap_or(LiveDelegationCancellationOutcome::Failed);
                    if let Err(error) = self
                        .runtime
                        .resolve_live_delegation_cancellation(
                            runtime_binding.runtime_id(),
                            runtime_binding.fence_token(),
                            runtime_binding.generation(),
                            &cancellation,
                            outcome,
                        )
                        .await
                    {
                        self.active.lock().await.insert(channel_key, previous);
                        return Err(error.to_string());
                    }
                }
                previous
                    .task
                    .await
                    .map_err(|error| format!("live delegation terminal task failed: {error}"))?;
            }
        }
        let reconciliation = self
            .runtime
            .reconcile_live_delegation_transcript(
                session_id,
                runtime_binding.runtime_id(),
                runtime_binding.fence_token(),
                runtime_binding.generation(),
                &operation,
                &provisional,
                &final_evidence,
            )
            .await
            .map_err(|error| error.to_string())?;
        tracing::debug!(
            operation_id = %operation.operation_id(),
            disposition = ?reconciliation.disposition(),
            "client-context control reconciled canonical transcript"
        );
        if reconciliation.disposition() != LiveHandoffReconciliation::Confirmed {
            return Err(
                "canonical final transcript did not confirm the client delegation".to_string(),
            );
        }
        let started = self
            .start_admitted_delegation(
                provider_binding,
                control,
                channel_key,
                operation,
                provisional,
                runtime_binding,
                mob_handle,
                source_identity,
                delegation,
                final_evidence,
                reconciliation,
            )
            .await;
        if started.is_ok() {
            self.completed_delegation_turns
                .lock()
                .await
                .remove(&completed_key);
        }
        started
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "this exact delegation boundary carries independent provider, runtime, fork, and operation authorities"
    )]
    async fn start_admitted_delegation(
        &self,
        provider_binding: &ProviderWebrtcBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
        channel_key: ActiveChannelKey,
        operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
        provisional: ProvisionalLiveHandoff,
        runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        mob_handle: meerkat_mob::MobHandle,
        source_identity: AgentIdentity,
        delegation: LiveSidebandDelegationRef,
        final_evidence: FinalLiveUserTranscriptCommitEvidence,
        reconciliation: LiveHandoffReconciliationReceipt,
    ) -> Result<(), String> {
        let session_id = provider_binding.session_id();

        if reconciliation.disposition() != LiveHandoffReconciliation::Confirmed {
            return Err(
                "live delegation worker start requires Confirmed reconciliation".to_string(),
            );
        }
        let committed_message_count =
            final_evidence.committed_message_count().ok_or_else(|| {
                "confirmed live delegation is missing its exact transcript boundary".to_string()
            })?;

        let worker_identity =
            AgentIdentity::from(format!("live-delegation-{}", operation.operation_id()));
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "client-context control requesting generated worker-start authority"
        );
        let admission = self
            .runtime
            .authorize_live_delegation_worker_start(
                session_id,
                runtime_binding.runtime_id(),
                runtime_binding.fence_token(),
                runtime_binding.generation(),
                &operation,
                &provisional,
                worker_identity.as_str(),
            )
            .await
            .map_err(|error| error.to_string())?;
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "client-context control received generated worker-start authority"
        );
        let consequential = self
            .runtime
            .authorize_live_consequential_effect(
                session_id,
                runtime_binding.runtime_id(),
                runtime_binding.fence_token(),
                runtime_binding.generation(),
                &operation,
                &reconciliation,
            )
            .await
            .map_err(|error| error.to_string())?;
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "client-context control received consequential-effect authority"
        );
        admission
            .release_tool_execution(&consequential)
            .map_err(|error| error.to_string())?;
        let result_spec =
            BoundedResultSpec::new("gpt_live_delegation", LIVE_DELEGATION_RESULT_BYTES)
                .map_err(|error| error.to_string())?;
        let service = DelegationExecutionService::new(mob_handle);
        let request = DelegationExecutionRequest::new_live(
            worker_identity.clone(),
            provisional.executor_input(),
            result_spec,
            admission.clone(),
        )
        .with_durable_fork(source_identity, Some(committed_message_count));
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "client-context control entering durable delegation service"
        );
        let execution = match service.start(request).await {
            Ok(execution) => execution,
            Err(error) => {
                let start_error = error.to_string();
                let operation_id = operation.operation_id().clone();
                let runtime = Arc::clone(&self.runtime);
                let cleanup_binding = runtime_binding.clone();
                let cleanup_admission = admission.clone();
                let cleanup_tasks = Arc::clone(&self.failed_start_cleanups);
                let cleanup_operation_id = operation_id.clone();
                let (first_report_tx, first_report_rx) = oneshot::channel();
                let (cleanup_start_tx, cleanup_start_rx) = oneshot::channel();
                let cleanup = tokio::spawn(async move {
                    let _ = cleanup_start_rx.await;
                    retire_failed_start_with_retry(
                        runtime,
                        cleanup_binding,
                        cleanup_admission,
                        service,
                        first_report_tx,
                    )
                    .await;
                    cleanup_tasks.lock().await.remove(&cleanup_operation_id);
                });
                self.failed_start_cleanups.lock().await.insert(
                    operation_id,
                    OwnedDelegationCleanup {
                        binding: runtime_binding.clone(),
                        task: cleanup,
                    },
                );
                let _ = cleanup_start_tx.send(());
                let report_error = first_report_rx
                    .await
                    .ok()
                    .flatten()
                    .map(|error| format!("; generated failed-start report pending retry: {error}"))
                    .unwrap_or_default();
                return Err(format!("{start_error}{report_error}"));
            }
        };
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "durable live delegation worker accepted its bounded turn"
        );
        let Some(cancellation) = execution.cancellation_handle() else {
            let operation_id = operation.operation_id().clone();
            let cleanup_operation_id = operation_id.clone();
            let cleanup_tasks = Arc::clone(&self.failed_start_cleanups);
            let cleanup_runtime = Arc::clone(&self.runtime);
            let cleanup_binding = runtime_binding.clone();
            let cleanup_admission = admission.clone();
            let (cleanup_start_tx, cleanup_start_rx) = oneshot::channel();
            let cleanup = tokio::spawn(async move {
                let _ = cleanup_start_rx.await;
                retry_reconciled_cleanup_step("missing-cancellation-start-report", || {
                    cleanup_runtime.resolve_live_delegation_worker_start(
                        cleanup_binding.runtime_id(),
                        cleanup_binding.fence_token(),
                        cleanup_binding.generation(),
                        &cleanup_admission,
                        true,
                    )
                })
                .await;
                let _ = realize_terminal(
                    cleanup_runtime,
                    &cleanup_binding,
                    &cleanup_admission,
                    &service,
                    execution.await_terminal().await,
                )
                .await;
                cleanup_tasks.lock().await.remove(&cleanup_operation_id);
            });
            self.failed_start_cleanups.lock().await.insert(
                operation_id,
                OwnedDelegationCleanup {
                    binding: runtime_binding.clone(),
                    task: cleanup,
                },
            );
            let _ = cleanup_start_tx.send(());
            return Err(
                "live execution lost cancellation binding; terminal cleanup retained".to_string(),
            );
        };
        let retained = Arc::new(RetainedDelegation {
            operation,
            provisional,
            runtime_binding,
            admission,
            delegation,
            control,
            result: Mutex::new(RetainedDelegationResult {
                reconciliation: Some(reconciliation),
                ..RetainedDelegationResult::default()
            }),
        });
        self.retained.lock().await.insert(
            retained.operation.operation_id().clone(),
            Arc::clone(&retained),
        );
        let task_coordinator = Arc::new(self.clone());
        let task_runtime = Arc::clone(&self.runtime);
        let task_retained = Arc::clone(&retained);
        let task_cancellation = cancellation.clone();
        let (task_start_tx, task_start_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let command = await_started_delegation_task_command(task_start_rx).await;
            if matches!(
                command,
                StartedDelegationTaskCommand::CleanupAfterStartPublicationFailure
            ) {
                cleanup_started_execution_after_publication_failure(
                    Arc::clone(&task_runtime),
                    &task_retained.runtime_binding,
                    &task_retained.admission,
                    &service,
                    &task_cancellation,
                    execution,
                )
                .await;
                task_coordinator
                    .remove_retained_delegation(&task_retained)
                    .await;
                return;
            }
            let terminal = realize_terminal(
                task_runtime,
                &task_retained.runtime_binding,
                &task_retained.admission,
                &service,
                execution.await_terminal().await,
            )
            .await;
            tracing::debug!(
                operation_id = %task_retained.operation.operation_id(),
                result_present = terminal.result_text.is_some(),
                terminal_ineligible = terminal.terminal_ineligible,
                "durable live delegation worker reached realized terminality"
            );
            task_coordinator
                .record_terminal_realization(&task_retained, terminal)
                .await;
        });
        self.active.lock().await.insert(
            channel_key,
            ActiveDelegation {
                retained: Arc::clone(&retained),
                cancellation,
                task,
            },
        );
        if let Err(error) = self
            .runtime
            .resolve_live_delegation_worker_start(
                retained.runtime_binding.runtime_id(),
                retained.runtime_binding.fence_token(),
                retained.runtime_binding.generation(),
                &retained.admission,
                true,
            )
            .await
        {
            let _ = task_start_tx
                .send(StartedDelegationTaskCommand::CleanupAfterStartPublicationFailure);
            return Err(format!(
                "generated successful worker-start publication failed; cleanup retained: {error}"
            ));
        }
        let _ = task_start_tx.send(StartedDelegationTaskCommand::Run);
        Ok(())
    }

    async fn record_terminal_realization(
        &self,
        retained: &Arc<RetainedDelegation>,
        terminal: RealizedDelegationTerminal,
    ) {
        {
            let mut result = retained.result.lock().await;
            result.result_text = terminal.result_text;
            result.terminal_ineligible |= terminal.terminal_ineligible;
        }
        if retained.result.lock().await.terminal_ineligible {
            self.remove_retained_delegation(retained).await;
            return;
        }
        self.schedule_result_delivery(Arc::clone(retained)).await;
    }

    async fn schedule_result_delivery(&self, retained: Arc<RetainedDelegation>) {
        let ready = {
            let result = retained.result.lock().await;
            !result.terminal_ineligible
                && result.reconciliation.is_some()
                && result.result_text.is_some()
        };
        if !ready {
            return;
        }
        let operation_id = retained.operation.operation_id().clone();
        let mut tasks = self.result_delivery_tasks.lock().await;
        if tasks.contains_key(&operation_id) {
            return;
        }
        let coordinator = Arc::new(self.clone());
        let task_operation_id = operation_id.clone();
        let task_retained = Arc::clone(&retained);
        let tasks_owner = Arc::clone(&self.result_delivery_tasks);
        let (start_tx, start_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = start_rx.await;
            let mut retry_delay = LIVE_DELEGATION_CLEANUP_RETRY_DELAY;
            loop {
                if task_retained.result.lock().await.terminal_ineligible {
                    break;
                }
                match coordinator
                    .try_release_retained_result(&task_retained)
                    .await
                {
                    Ok(()) => break,
                    Err(error) => {
                        tracing::warn!(%error, %task_operation_id, "owned live result delivery retry remains pending");
                        tokio::time::sleep(retry_delay).await;
                        retry_delay = retry_delay
                            .saturating_mul(2)
                            .min(LIVE_DELEGATION_CLEANUP_RETRY_MAX_DELAY);
                    }
                }
            }
            tasks_owner.lock().await.remove(&task_operation_id);
        });
        tasks.insert(operation_id, task);
        drop(tasks);
        let _ = start_tx.send(());
    }

    async fn retain_and_realize_result_recovery(
        &self,
        recovery: meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
    ) {
        let operation_id = recovery.delivery().operation().operation_id().clone();
        {
            let mut pending = self.pending_result_recoveries.lock().await;
            if pending.contains_key(&operation_id) {
                tracing::warn!(%operation_id, "duplicate ambiguity recovery observation retained without replay");
                return;
            }
            pending.insert(operation_id.clone(), recovery.clone());
        }
        let mut tasks = self.result_recovery_tasks.lock().await;
        if tasks.contains_key(&operation_id) {
            return;
        }
        let runtime = Arc::clone(&self.runtime);
        let pending = Arc::clone(&self.pending_result_recoveries);
        let tasks_owner = Arc::clone(&self.result_recovery_tasks);
        let task_operation_id = operation_id.clone();
        let session_id = recovery.session_id().clone();
        let channel_id = recovery.closing_channel_id().clone();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let (start_tx, start_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = start_rx.await;
            let realization = await_result_recovery_attempt_or_shutdown(
                task_cancellation,
                runtime.realize_live_delegation_result_ambiguity_recovery(recovery),
            )
            .await;
            match realization {
                Some(Ok(())) => {
                    pending.lock().await.remove(&task_operation_id);
                }
                Some(Err(error)) => {
                    tracing::warn!(%error, %task_operation_id, "generated live result ambiguity recovery remains retained without replay");
                }
                None => {}
            }
            tasks_owner.lock().await.remove(&task_operation_id);
        });
        tasks.insert(
            operation_id,
            OwnedResultRecovery {
                session_id,
                channel_id,
                cancellation,
                task,
            },
        );
        drop(tasks);
        let _ = start_tx.send(());
    }

    async fn settle_result_recovery_tasks(&self, binding: &ProviderWebrtcBinding) {
        let pending_operation_ids = self
            .pending_result_recoveries
            .lock()
            .await
            .iter()
            .filter_map(|(operation_id, recovery)| {
                (recovery.session_id() == binding.session_id()
                    && recovery.closing_channel_id() == binding.channel_id())
                .then_some(operation_id.clone())
            })
            .collect::<Vec<_>>();
        let owned = {
            let mut tasks = self.result_recovery_tasks.lock().await;
            let operation_ids = tasks
                .iter()
                .filter_map(|(operation_id, recovery)| {
                    (recovery.session_id == *binding.session_id()
                        && recovery.channel_id == *binding.channel_id())
                    .then_some(operation_id.clone())
                })
                .collect::<Vec<_>>();
            operation_ids
                .into_iter()
                .filter_map(|operation_id| {
                    tasks
                        .remove(&operation_id)
                        .map(|recovery| (operation_id, recovery))
                })
                .collect::<Vec<_>>()
        };
        for (operation_id, recovery) in owned {
            cancel_and_settle_result_recovery(recovery).await;
            self.pending_result_recoveries
                .lock()
                .await
                .remove(&operation_id);
        }
        let mut pending = self.pending_result_recoveries.lock().await;
        for operation_id in pending_operation_ids {
            pending.remove(&operation_id);
        }
    }

    async fn remove_retained_delegation(&self, retained: &Arc<RetainedDelegation>) {
        let operation_id = retained.operation.operation_id();
        let mut retained_by_operation = self.retained.lock().await;
        if retained_by_operation
            .get(operation_id)
            .is_some_and(|current| Arc::ptr_eq(current, retained))
        {
            retained_by_operation.remove(operation_id);
        }
        drop(retained_by_operation);

        let channel_key = (
            retained.runtime_binding.session_id().clone(),
            retained.runtime_binding.channel_id().clone(),
        );
        let mut active = self.active.lock().await;
        if active
            .get(&channel_key)
            .is_some_and(|current| Arc::ptr_eq(&current.retained, retained))
        {
            active.remove(&channel_key);
        }
    }

    async fn settle_result_delivery_task(&self, operation_id: &OperationId) {
        let task = self.result_delivery_tasks.lock().await.remove(operation_id);
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    async fn settle_failed_start_cleanups(&self, binding: &ProviderWebrtcBinding) {
        let tasks = {
            let mut cleanups = self.failed_start_cleanups.lock().await;
            let operation_ids = cleanups
                .iter()
                .filter_map(|(operation_id, cleanup)| {
                    (cleanup.binding.session_id() == binding.session_id()
                        && cleanup.binding.channel_id() == binding.channel_id()
                        && cleanup.binding.generation() == binding.runtime_generation().get()
                        && cleanup.binding.fence_token() == binding.runtime_fence().get())
                    .then_some(operation_id.clone())
                })
                .collect::<Vec<_>>();
            operation_ids
                .into_iter()
                .filter_map(|operation_id| {
                    cleanups.remove(&operation_id).map(|cleanup| cleanup.task)
                })
                .collect::<Vec<_>>()
        };
        for task in tasks {
            let _ = task.await;
        }
    }

    async fn release_exact_delegation_result_projection(
        control: &dyn ExperimentalGptLiveControlPlane,
        projection: ExactDelegationResultProjection<LiveDelegationResultDeliveryAuthority>,
    ) -> Result<ExperimentalGptLiveResultDeliveryDispatch, ExperimentalGptLiveBridgeError> {
        projection
            .dispatch(|authority, delegation, result_text| {
                control.release_delegation_context(authority, delegation, result_text)
            })
            .await
    }

    async fn try_release_retained_result(
        &self,
        retained: &Arc<RetainedDelegation>,
    ) -> Result<(), String> {
        let (reservation, reconciliation, result_text, existing_release, existing_delivery) = {
            let mut result = retained.result.lock().await;
            let (Some(reconciliation), Some(result_text)) =
                (result.reconciliation.clone(), result.result_text.clone())
            else {
                return Ok(());
            };
            let Some(reservation) = result.reserve_delivery() else {
                return Ok(());
            };
            (
                reservation,
                reconciliation,
                result_text,
                result.release_authority.clone(),
                result.delivery_authority.clone(),
            )
        };

        let release = match existing_release {
            Some(release) => release,
            None => {
                let release = match self
                    .runtime
                    .authorize_live_delegation_result_release(
                        retained.runtime_binding.session_id(),
                        retained.runtime_binding.runtime_id(),
                        retained.runtime_binding.fence_token(),
                        retained.runtime_binding.generation(),
                        &retained.operation,
                        &reconciliation,
                    )
                    .await
                {
                    Ok(release) => release,
                    Err(error) => {
                        retained.result.lock().await.release_delivery(reservation);
                        return Err(error.to_string());
                    }
                };
                retained.result.lock().await.release_authority = Some(release.clone());
                release
            }
        };
        let delivery = match existing_delivery {
            Some(delivery) => delivery,
            None => {
                let delivery = self
                    .runtime
                    .authorize_live_delegation_result_delivery(&release, &result_text)
                    .await;
                let delivery = match delivery {
                    Ok(delivery) => delivery,
                    Err(error) => {
                        retained.result.lock().await.release_delivery(reservation);
                        return Err(error.to_string());
                    }
                };
                retained.result.lock().await.delivery_authority = Some(delivery.clone());
                delivery
            }
        };
        let ambiguity_authority = delivery.clone();
        let dispatch = Self::release_exact_delegation_result_projection(
            retained.control.as_ref(),
            ExactDelegationResultProjection::new(
                delivery,
                retained.delegation.clone(),
                result_text,
            ),
        )
        .await;
        let resolution = match dispatch {
            Err(ExperimentalGptLiveBridgeError::ActiveBindingUnavailable) => {
                retained.result.lock().await.release_delivery(reservation);
                return Err("exact provider binding is temporarily unavailable".to_string());
            }
            Err(error) => {
                tracing::warn!(%error, "result delivery authority may have crossed the provider boundary; resolving ambiguous");
                self.resolve_ambiguous_result_delivery(&ambiguity_authority)
                    .await;
                self.remove_retained_delegation(retained).await;
                return Ok(());
            }
            Ok(ExperimentalGptLiveResultDeliveryDispatch::AwaitingAcknowledgement(waiter)) => {
                match waiter.resolve().await {
                    Ok(resolution) => resolution,
                    Err(error) => {
                        tracing::warn!(%error, "accepted live result delivery lost acknowledgement; forcing ambiguity recovery");
                        self.resolve_ambiguous_result_delivery(&ambiguity_authority)
                            .await;
                        self.remove_retained_delegation(retained).await;
                        return Ok(());
                    }
                }
            }
            Ok(ExperimentalGptLiveResultDeliveryDispatch::Resolved(resolution)) => resolution,
        };
        let (authority, observation) = resolution.into_parts();
        let resolution = retry_reconciled_cleanup_step("result-delivery-resolution", || {
            self.runtime
                .resolve_live_delegation_result_delivery(&authority, observation)
        })
        .await;
        match resolution {
            LiveDelegationResultDeliveryResolution::Resolved(receipt) => {
                if receipt.retry_allowed() || receipt.recovery_required() {
                    self.remove_retained_delegation(retained).await;
                    return Err(
                        "generated terminal result delivery returned invalid retry or recovery facts"
                            .to_string(),
                    );
                }
            }
            LiveDelegationResultDeliveryResolution::AmbiguityRecovery(recovery) => {
                self.retain_and_realize_result_recovery(recovery).await;
            }
        }
        self.remove_retained_delegation(retained).await;
        Ok(())
    }

    async fn resolve_ambiguous_result_delivery(
        &self,
        authority: &LiveDelegationResultDeliveryAuthority,
    ) {
        let resolution =
            retry_reconciled_cleanup_step("ambiguous-result-delivery-resolution", || {
                self.runtime.resolve_live_delegation_result_delivery(
                    authority,
                    LiveDelegationResultDeliveryObservation::Ambiguous,
                )
            })
            .await;
        match resolution {
            LiveDelegationResultDeliveryResolution::AmbiguityRecovery(recovery) => {
                self.retain_and_realize_result_recovery(recovery).await;
            }
            LiveDelegationResultDeliveryResolution::Resolved(_) => {
                tracing::error!(
                    "generated ambiguous result delivery resolved without mandatory recovery"
                );
            }
        }
    }

    /// Supply only SessionDocument-sealed exact final-user evidence. Until the
    /// provider can prove its item/turn join, production never calls this and
    /// the worker's tool gate/result release remain closed.
    pub async fn reconcile_exact_final(
        &self,
        evidence: FinalLiveUserTranscriptCommitEvidence,
    ) -> Result<(), String> {
        let channel_key = (evidence.session_id().clone(), evidence.channel_id().clone());
        let retained = self
            .retained
            .lock()
            .await
            .values()
            .find(|retained| {
                retained.runtime_binding.session_id() == evidence.session_id()
                    && retained.runtime_binding.channel_id() == evidence.channel_id()
                    && retained.operation.domain_correlation().interaction_id()
                        == evidence.interaction_id()
            })
            .cloned()
            .ok_or_else(|| "final transcript has no exact active delegation".to_string())?;
        let binding = retained.runtime_binding.clone();
        let receipt = self
            .runtime
            .reconcile_live_delegation_transcript(
                evidence.session_id(),
                binding.runtime_id(),
                binding.fence_token(),
                binding.generation(),
                &retained.operation,
                &retained.provisional,
                &evidence,
            )
            .await
            .map_err(|error| error.to_string())?;
        if receipt.disposition() == LiveHandoffReconciliation::Confirmed {
            let witness = self
                .runtime
                .authorize_live_consequential_effect(
                    evidence.session_id(),
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    &retained.operation,
                    &receipt,
                )
                .await
                .map_err(|error| error.to_string())?;
            if let Err(error) = retained.admission.release_tool_execution(&witness)
                && error
                    != meerkat_runtime::live_execution::LiveExecutionAuthorityError::ToolExecutionAdmissionTerminal
            {
                return Err(error.to_string());
            }
            retained.result.lock().await.reconciliation = Some(receipt);
            self.schedule_result_delivery(Arc::clone(&retained)).await;
        } else if receipt.cancellation_required() {
            retained.result.lock().await.terminal_ineligible = true;
            let cancellation = self
                .runtime
                .authorize_live_delegation_transcript_cancellation(
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    &retained.admission,
                )
                .await
                .map_err(|error| error.to_string())?;
            let cancellation_handle = self
                .active
                .lock()
                .await
                .get(&channel_key)
                .filter(|active| Arc::ptr_eq(&active.retained, &retained))
                .map(|active| active.cancellation.clone())
                .ok_or_else(|| {
                    "negative transcript has no exact active worker cancellation handle".to_string()
                })?;
            let outcome = cancellation_handle
                .cancel(&cancellation)
                .await
                .unwrap_or(LiveDelegationCancellationOutcome::Failed);
            self.runtime
                .resolve_live_delegation_cancellation(
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    &cancellation,
                    outcome,
                )
                .await
                .map_err(|error| error.to_string())?;
        } else {
            retained.result.lock().await.terminal_ineligible = true;
            self.remove_retained_delegation(&retained).await;
        }
        Ok(())
    }

    async fn cancel_channel_binding(&self, binding: &ProviderWebrtcBinding) {
        let key = (binding.session_id().clone(), binding.channel_id().clone());
        self.active_turns.lock().await.remove(&key);
        self.completed_delegation_turns
            .lock()
            .await
            .retain(|(session_id, channel_id, _), _| {
                session_id != binding.session_id() || channel_id != binding.channel_id()
            });
        self.cancel_responses_executions_for_binding(binding).await;
        self.settle_result_recovery_tasks(binding).await;
        self.settle_failed_start_cleanups(binding).await;
        if let Some(active) = self.active.lock().await.remove(&key) {
            if active.retained.runtime_binding.session_id() != binding.session_id()
                || active.retained.runtime_binding.channel_id() != binding.channel_id()
                || active.retained.runtime_binding.generation()
                    != binding.runtime_generation().get()
                || active.retained.runtime_binding.fence_token() != binding.runtime_fence().get()
            {
                self.active.lock().await.insert(key.clone(), active);
            } else {
                active.retained.result.lock().await.terminal_ineligible = true;
                let runtime_binding = &active.retained.runtime_binding;
                if let Ok(directive) = self
                    .runtime
                    .abandon_live_delegation(
                        runtime_binding.runtime_id(),
                        runtime_binding.fence_token(),
                        runtime_binding.generation(),
                        &active.retained.admission,
                    )
                    .await
                    && let LiveDelegationCancellationDirective::CancellationAuthorized(authority) =
                        directive
                {
                    let outcome = active
                        .cancellation
                        .cancel(&authority)
                        .await
                        .unwrap_or(LiveDelegationCancellationOutcome::Failed);
                    let _ = self
                        .runtime
                        .resolve_live_delegation_cancellation(
                            runtime_binding.runtime_id(),
                            runtime_binding.fence_token(),
                            runtime_binding.generation(),
                            &authority,
                            outcome,
                        )
                        .await;
                }
                let _ = active.task.await;
                self.settle_result_delivery_task(active.retained.operation.operation_id())
                    .await;
                self.remove_retained_delegation(&active.retained).await;
            }
        }
        let retained = self
            .retained
            .lock()
            .await
            .values()
            .filter(|retained| {
                retained.runtime_binding.session_id() == binding.session_id()
                    && retained.runtime_binding.channel_id() == binding.channel_id()
                    && retained.runtime_binding.generation() == binding.runtime_generation().get()
                    && retained.runtime_binding.fence_token() == binding.runtime_fence().get()
            })
            .cloned()
            .collect::<Vec<_>>();
        for retained in retained {
            retained.result.lock().await.terminal_ineligible = true;
            self.settle_result_delivery_task(retained.operation.operation_id())
                .await;
            self.remove_retained_delegation(&retained).await;
        }
        self.settle_responses_retirement_debt_for_binding(binding)
            .await;
    }
}

#[async_trait::async_trait]
impl meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator
    for ExperimentalLiveDelegationCoordinator
{
    async fn prepare_bound_channel(
        &self,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
    ) -> Result<(), String> {
        ExperimentalLiveDelegationCoordinator::prepare_bound_channel(self, binding, control).await
    }

    async fn run_bound_channel(
        &self,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
    ) {
        ExperimentalLiveDelegationCoordinator::run_bound_channel(self, binding, control).await;
    }

    async fn observe_provider_lifecycle(
        &self,
        observation: &LiveSidebandObservation,
    ) -> Result<(), String> {
        ExperimentalLiveDelegationCoordinator::observe_provider_lifecycle(self, observation).await
    }

    async fn deactivate_bound_channel(
        &self,
        binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> Result<(), String> {
        ExperimentalLiveDelegationCoordinator::deactivate_bound_channel(self, binding).await
    }
}

/// Retry an operation whose runtime entrypoint first reconciles exact generated
/// state. Callers must not use this for raw generated transitions.
async fn retry_reconciled_cleanup_step<T, E, F, Fut>(label: &'static str, mut step: F) -> T
where
    E: std::fmt::Display,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut retry_delay = LIVE_DELEGATION_CLEANUP_RETRY_DELAY;
    loop {
        match step().await {
            Ok(value) => return value,
            Err(error) => {
                tracing::warn!(%error, cleanup_step = label, "live delegation cleanup remains pending");
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay
                    .saturating_mul(2)
                    .min(LIVE_DELEGATION_CLEANUP_RETRY_MAX_DELAY);
            }
        }
    }
}

async fn retire_failed_start_with_retry(
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    admission: LiveDelegationExecutionAdmission,
    service: DelegationExecutionService,
    first_report_tx: oneshot::Sender<Option<String>>,
) {
    let first_report = runtime
        .resolve_live_delegation_worker_start(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &admission,
            false,
        )
        .await;
    let first_report_error = first_report.as_ref().err().map(ToString::to_string);
    let _ = first_report_tx.send(first_report_error);
    if first_report.is_err() {
        retry_reconciled_cleanup_step("failed-start-report", || {
            runtime.resolve_live_delegation_worker_start(
                binding.runtime_id(),
                binding.fence_token(),
                binding.generation(),
                &admission,
                false,
            )
        })
        .await;
    }
    let retirement = retry_reconciled_cleanup_step("failed-start-retirement-authority", || {
        runtime.authorize_live_delegation_worker_retirement(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &admission,
        )
    })
    .await;
    retry_reconciled_cleanup_step("failed-start-physical-retirement", || {
        service.retire_live_failed_start(&admission, &retirement)
    })
    .await;
    retry_reconciled_cleanup_step("failed-start-retirement-resolution", || {
        runtime.resolve_live_delegation_worker_retirement(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &retirement,
            true,
        )
    })
    .await;
}

async fn cleanup_started_execution_after_publication_failure(
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    admission: &LiveDelegationExecutionAdmission,
    service: &DelegationExecutionService,
    cancellation: &DelegationCancellationHandle,
    execution: DelegationExecutionHandle,
) {
    retry_reconciled_cleanup_step("successful-start-report", || {
        runtime.resolve_live_delegation_worker_start(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            admission,
            true,
        )
    })
    .await;
    let directive = retry_reconciled_cleanup_step("unpublished-start-abandonment", || {
        runtime.abandon_live_delegation(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            admission,
        )
    })
    .await;
    if let LiveDelegationCancellationDirective::CancellationAuthorized(authority) = directive {
        let outcome =
            retry_reconciled_cleanup_step("unpublished-start-physical-cancellation", || async {
                match cancellation.cancel(&authority).await {
                    Ok(LiveDelegationCancellationOutcome::Failed) => {
                        Err("exact worker cancellation failed mechanically".to_string())
                    }
                    Err(error) => Err(error.to_string()),
                    Ok(outcome) => Ok(outcome),
                }
            })
            .await;
        if let Err(error) = runtime
            .resolve_live_delegation_cancellation(
                binding.runtime_id(),
                binding.fence_token(),
                binding.generation(),
                &authority,
                outcome,
            )
            .await
        {
            tracing::warn!(%error, "cancellation observation publication was not replayed; terminal observation remains authoritative");
        }
    }
    let _ = realize_terminal(
        runtime,
        binding,
        admission,
        service,
        execution.await_terminal().await,
    )
    .await;
}

struct RealizedDelegationTerminal {
    result_text: Option<String>,
    terminal_ineligible: bool,
}

async fn realize_terminal(
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    admission: &LiveDelegationExecutionAdmission,
    service: &DelegationExecutionService,
    terminalized: DelegationTerminalizedExecution,
) -> RealizedDelegationTerminal {
    let terminal_kind = match terminalized.terminal() {
        DelegationTurnTerminal::Completed(_) => LiveDelegationWorkerTerminalKind::Completed,
        DelegationTurnTerminal::Failed(_) => LiveDelegationWorkerTerminalKind::Failed,
        _ => LiveDelegationWorkerTerminalKind::Failed,
    };
    let result_text = match terminalized.terminal() {
        DelegationTurnTerminal::Completed(turn) => Some(turn.result().result().text().to_string()),
        DelegationTurnTerminal::Failed(_) => None,
        _ => None,
    };
    let terminal_receipt = retry_reconciled_cleanup_step("worker-terminal-record", || {
        runtime.record_live_delegation_worker_terminal(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            admission,
            terminal_kind,
        )
    })
    .await;
    let retirement = retry_reconciled_cleanup_step("worker-retirement-authority", || {
        runtime.authorize_live_delegation_worker_retirement(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            admission,
        )
    })
    .await;
    retry_reconciled_cleanup_step("worker-physical-retirement", || {
        service.retire_live_terminalized(&terminalized, &retirement)
    })
    .await;
    retry_reconciled_cleanup_step("worker-retirement-resolution", || {
        runtime.resolve_live_delegation_worker_retirement(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &retirement,
            true,
        )
    })
    .await;
    let retired = true;
    let result_text =
        retain_terminal_result(retired, terminal_kind, terminal_receipt.late(), result_text);
    let terminal_ineligible = result_text.is_none();
    RealizedDelegationTerminal {
        result_text,
        terminal_ineligible,
    }
}

fn retain_terminal_result(
    retired: bool,
    terminal: LiveDelegationWorkerTerminalKind,
    late: bool,
    result_text: Option<String>,
) -> Option<String> {
    (retired && terminal == LiveDelegationWorkerTerminalKind::Completed && !late)
        .then_some(result_text)
        .flatten()
        .filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "focused invariant tests use explicit assertion messages for impossible setup and timeout failures"
)]
mod tests {
    use super::*;
    use meerkat_core::exact_operation::ExactOperationIdentity;
    use meerkat_core::interaction::InteractionId;

    fn test_bridge_admission(
        session_id: SessionId,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        identity: &str,
    ) -> LiveBridgeOperationAdmission {
        let canonical_context_revision = meerkat_core::Session::with_id(session_id.clone())
            .canonical_context_revision()
            .expect("test Session mints canonical context revision");
        test_bridge_admission_with_revision(
            session_id,
            binding,
            identity,
            canonical_context_revision,
        )
    }

    fn test_bridge_admission_with_revision(
        session_id: SessionId,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        identity: &str,
        canonical_context_revision: meerkat_core::CanonicalContextRevision,
    ) -> LiveBridgeOperationAdmission {
        let provider = meerkat_core::LiveBridgeProviderCorrelation::new(
            "turn:opaque",
            "delegation:opaque",
            "call:opaque",
        )
        .expect("provider correlation");
        let correlation = meerkat_core::LiveBridgeOperationCorrelation::new(
            binding.channel_id().clone(),
            InteractionId::new(),
            provider,
        )
        .expect("bridge correlation");
        LiveBridgeOperationAdmission::__test_new(
            session_id,
            binding,
            ExactOperationIdentity::for_domain(OperationId::new(), correlation),
            identity,
            canonical_context_revision,
            meerkat_core::LiveBridgeRequestDigest::derive("request").expect("request digest"),
        )
    }

    fn test_runtime_binding(
        session_id: SessionId,
        channel: &str,
        generation: u64,
    ) -> meerkat_runtime::live_execution::LiveDelegationRuntimeBinding {
        meerkat_runtime::live_execution::LiveDelegationRuntimeBinding::__test_new(
            session_id,
            meerkat_core::LiveChannelId::new(channel),
            meerkat_runtime::identifiers::LogicalRuntimeId::new("live:test-runtime"),
            generation + 100,
            generation,
        )
    }

    #[test]
    fn responses_executor_task_preserves_request_and_appends_visible_report_instruction() {
        let task = responses_executor_task("check the garden irrigation");

        assert!(task.starts_with("check the garden irrigation\n\n"));
        assert!(task.contains("MEERKAT_BOUNDED_DELEGATION_REPORT_V1"));
        assert!(task.contains("loose best-effort completion report"));
        assert!(task.contains("not a structured schema or success guarantee"));
    }

    #[test]
    fn responses_executor_outcome_receipt_preserves_terminal_and_best_effort_text() {
        let operation_id = OperationId::new();
        let completed = responses_executor_outcome_receipt(
            &operation_id,
            DurableExecutorTerminalKind::Completed,
            Some("watered the north beds"),
        );
        assert!(completed.contains("MEERKAT_LIVE_EXECUTOR_OUTCOME_V1"));
        assert!(completed.contains(&operation_id.to_string()));
        assert!(completed.contains("watered the north beds"));

        let failed = responses_executor_outcome_receipt(
            &operation_id,
            DurableExecutorTerminalKind::Failed,
            Some("must not be projected as a successful result"),
        );
        assert!(failed.contains("failed"));
        assert!(!failed.contains("must not be projected"));
    }

    #[test]
    fn bounded_result_is_retained_only_for_retired_non_late_completion() {
        let completed = || Some("bounded executor result".to_string());
        assert_eq!(
            retain_terminal_result(
                true,
                LiveDelegationWorkerTerminalKind::Completed,
                false,
                completed(),
            )
            .as_deref(),
            Some("bounded executor result")
        );
        assert!(
            retain_terminal_result(
                true,
                LiveDelegationWorkerTerminalKind::Completed,
                true,
                completed(),
            )
            .is_none()
        );
        assert!(
            retain_terminal_result(
                true,
                LiveDelegationWorkerTerminalKind::Failed,
                false,
                completed(),
            )
            .is_none()
        );
        assert!(
            retain_terminal_result(
                false,
                LiveDelegationWorkerTerminalKind::Completed,
                false,
                completed(),
            )
            .is_none()
        );
        assert!(
            retain_terminal_result(
                true,
                LiveDelegationWorkerTerminalKind::Completed,
                false,
                Some("   ".to_string()),
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn exact_bounded_result_and_delegation_ref_share_one_acknowledged_projection() {
        #[derive(Debug, Clone, PartialEq, Eq)]
        struct TestDeliveryAuthority(&'static str);

        #[derive(Debug, PartialEq, Eq)]
        struct TestAcknowledgement {
            authority: TestDeliveryAuthority,
        }

        let authority = TestDeliveryAuthority("delivery:exact-bounded-result");
        let retained_delegation = LiveSidebandDelegationRef::__from_provider_observation(
            "adapter:client-context".to_string(),
            "delegation:exact-retained-ref".to_string(),
        )
        .expect("provider delegation fixture");
        let bounded_executor_result =
            "line one from executor\nline two remains byte-exact  ".to_string();

        let acknowledgement = ExactDelegationResultProjection::new(
            authority.clone(),
            retained_delegation.clone(),
            bounded_executor_result.clone(),
        )
        .dispatch(
            |received_authority, received_delegation, received_result| async move {
                assert_eq!(received_authority, authority);
                assert_eq!(received_delegation, retained_delegation);
                assert_eq!(received_result, bounded_executor_result);
                TestAcknowledgement {
                    authority: received_authority,
                }
            },
        )
        .await;

        assert_eq!(
            acknowledgement.authority,
            TestDeliveryAuthority("delivery:exact-bounded-result"),
            "the acknowledgement resolves the same delivery authority consumed by the projection"
        );
    }

    #[test]
    fn responses_execution_requires_the_current_canonical_source_member() {
        let session_id = SessionId::new();
        let binding = test_runtime_binding(session_id.clone(), "channel:durable-owner", 9);
        let admission = test_bridge_admission(session_id, binding.clone(), "personal-agent");
        assert!(live_bridge_admission_matches_current_owner(
            &admission,
            &binding,
            &AgentIdentity::from("personal-agent")
        ));
        assert!(!live_bridge_admission_matches_current_owner(
            &admission,
            &binding,
            &AgentIdentity::from("helper-agent")
        ));

        let stale = test_runtime_binding(SessionId::new(), "channel:durable-owner", 10);
        assert!(!live_bridge_admission_matches_current_owner(
            &admission,
            &stale,
            &AgentIdentity::from("personal-agent")
        ));
    }

    #[tokio::test]
    async fn terminal_recording_retries_transient_custody_and_stops_on_exact_receipt() {
        let session_id = SessionId::new();
        let binding = test_runtime_binding(session_id.clone(), "terminal-retry", 1);
        let admission = test_bridge_admission(session_id, binding, "personal-agent");
        let receipt = LiveBridgeExecutionTerminalReceipt::__test_new(
            admission,
            meerkat_core::MeerkatExecutionTerminal::Cancelled,
            None,
        );
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let recovered = record_live_bridge_terminal_with_typed_recovery({
            let attempts = Arc::clone(&attempts);
            move || {
                let attempts = Arc::clone(&attempts);
                let receipt = receipt.clone();
                async move {
                    if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        Err(meerkat_runtime::RuntimeDriverError::RecoveryBackoff {
                            reason: "transient test recovery".to_string(),
                        })
                    } else {
                        Ok(receipt)
                    }
                }
            }
        })
        .await
        .expect("exact terminal receipt must settle transient custody");

        assert_eq!(
            recovered.terminal(),
            meerkat_core::MeerkatExecutionTerminal::Cancelled
        );
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn terminal_recording_never_retries_destroyed_session_authority() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let error = record_live_bridge_terminal_with_typed_recovery({
            let attempts = Arc::clone(&attempts);
            move || {
                let attempts = Arc::clone(&attempts);
                async move {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err::<LiveBridgeExecutionTerminalReceipt, _>(
                        meerkat_runtime::RuntimeDriverError::NotReady {
                            state: meerkat_runtime::RuntimeState::Destroyed,
                        },
                    )
                }
            }
        })
        .await
        .expect_err("nonretryable mismatch must retain terminal custody");

        assert!(matches!(
            error,
            meerkat_runtime::RuntimeDriverError::NotReady {
                state: meerkat_runtime::RuntimeState::Destroyed
            }
        ));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn outcome_projection_retries_transient_append_until_exact_success() {
        let operation_id = OperationId::new();
        let shutdown = CancellationToken::new();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let result = retry_responses_outcome_custody_step(
            &operation_id,
            &shutdown,
            "test-transient-append",
            {
                let attempts = Arc::clone(&attempts);
                move || {
                    let attempts = Arc::clone(&attempts);
                    async move {
                        if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                            Err("transient append failure")
                        } else {
                            Ok(meerkat_core::AppendSystemContextStatus::Applied)
                        }
                    }
                }
            },
        )
        .await;

        assert_eq!(
            result,
            Some(meerkat_core::AppendSystemContextStatus::Applied)
        );
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn ambiguous_outcome_projection_replay_converges_as_duplicate_without_second_effect() {
        let operation_id = OperationId::new();
        let shutdown = CancellationToken::new();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let committed_effects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let result = retry_responses_outcome_custody_step(
            &operation_id,
            &shutdown,
            "test-ambiguous-append",
            {
                let attempts = Arc::clone(&attempts);
                let committed_effects = Arc::clone(&committed_effects);
                move || {
                    let attempts = Arc::clone(&attempts);
                    let committed_effects = Arc::clone(&committed_effects);
                    async move {
                        let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if attempt == 0 {
                            committed_effects.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            Err("append committed but acknowledgement was lost")
                        } else {
                            Ok(meerkat_core::AppendSystemContextStatus::Duplicate)
                        }
                    }
                }
            },
        )
        .await;

        assert_eq!(
            result,
            Some(meerkat_core::AppendSystemContextStatus::Duplicate)
        );
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            committed_effects.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the stable append identity admits only one durable source-context effect"
        );
    }

    #[tokio::test]
    async fn projection_retry_shutdown_hands_unsettled_custody_to_restart_reconciliation() {
        let operation_id = OperationId::new();
        let shutdown = CancellationToken::new();
        let attempted = Arc::new(tokio::sync::Notify::new());
        let custody = Arc::new(Mutex::new(Some(PendingResponsesTerminalCustody {
            executor_terminal: DurableExecutorTerminalKind::Completed,
            bridge_terminal: LiveBridgeOperationTerminal::completed(
                "durable executor result",
                LIVE_DELEGATION_RESULT_BYTES,
            )
            .expect("test terminal"),
            result_digest: Some("sha256:test-result".to_string()),
            retirement_error: None,
        })));
        let retry_shutdown = shutdown.clone();
        let retry_attempted = Arc::clone(&attempted);
        let retry_custody = Arc::clone(&custody);
        let retry = tokio::spawn(async move {
            let result = retry_responses_outcome_custody_step(
                &operation_id,
                &retry_shutdown,
                "test-shutdown-handoff",
                move || {
                    retry_attempted.notify_one();
                    async { Err::<(), _>("projection remains unavailable") }
                },
            )
            .await;
            if result.is_some() {
                retry_custody.lock().await.take();
            }
            result
        });

        attempted.notified().await;
        shutdown.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), retry)
            .await
            .expect("shutdown must bound local projection retry")
            .expect("projection retry task joins");
        assert_eq!(result, None, "shutdown never fabricates projection success");
        assert!(
            custody.lock().await.is_some(),
            "unsettled terminal custody remains available for durable restart reconciliation"
        );
    }

    #[test]
    fn projection_retry_shutdown_waits_for_last_coordinator_owner() {
        let shutdown = Arc::new(ResponsesProjectionShutdown {
            cancellation: CancellationToken::new(),
        });
        let observation = shutdown.cancellation.clone();
        let second_owner = Arc::clone(&shutdown);

        drop(shutdown);
        assert!(
            !observation.is_cancelled(),
            "one coordinator clone cannot terminate shared projection custody"
        );
        drop(second_owner);
        assert!(
            observation.is_cancelled(),
            "last coordinator owner hands pending projection to restart recovery"
        );
    }

    #[tokio::test]
    async fn retirement_debt_converges_for_receipt_first_and_provider_first_orderings() {
        for channel in ["receipt-first", "provider-first"] {
            let session_id = SessionId::new();
            let binding = test_runtime_binding(session_id.clone(), channel, 1);
            let admission = test_bridge_admission(session_id.clone(), binding, "personal-agent");
            let pending = Mutex::new(PendingResponsesRetirementMap::new());

            reconcile_responses_retirement_custody(
                &pending,
                &session_id,
                admission.operation(),
                LiveBridgeRetirementDisposition::Unsettled,
            )
            .await;
            assert_eq!(pending.lock().await.len(), 1);

            reconcile_responses_retirement_custody(
                &pending,
                &session_id,
                admission.operation(),
                LiveBridgeRetirementDisposition::Retired,
            )
            .await;
            assert!(
                pending.lock().await.is_empty(),
                "the second persisted fact closes exact retirement debt regardless of ordering"
            );
        }
    }

    #[tokio::test]
    async fn close_after_projection_before_submission_keeps_only_exact_retirement_debt() {
        let session_id = SessionId::new();
        let binding = test_runtime_binding(session_id.clone(), "close-retirement-debt", 1);
        let admission = test_bridge_admission(session_id.clone(), binding, "personal-agent");
        let pending = Mutex::new(PendingResponsesRetirementMap::new());

        reconcile_responses_retirement_custody(
            &pending,
            &session_id,
            admission.operation(),
            LiveBridgeRetirementDisposition::Unsettled,
        )
        .await;
        let retained = pending
            .lock()
            .await
            .get(admission.operation().operation_id())
            .cloned()
            .expect("projection-first operation retains retirement debt");
        assert_eq!(retained.0, session_id);
        assert_eq!(&retained.1, admission.operation());

        reconcile_responses_retirement_custody(
            &pending,
            &session_id,
            admission.operation(),
            LiveBridgeRetirementDisposition::Retired,
        )
        .await;
        assert!(pending.lock().await.is_empty());
    }

    #[test]
    fn channel_shutdown_never_cancels_an_already_accepted_terminal() {
        let pending = PendingResponsesTerminalCustody {
            executor_terminal: DurableExecutorTerminalKind::Completed,
            bridge_terminal: LiveBridgeOperationTerminal::completed(
                "actual executor result",
                LIVE_DELEGATION_RESULT_BYTES,
            )
            .expect("test terminal"),
            result_digest: Some("sha256:test-result".to_string()),
            retirement_error: None,
        };
        let delivery_fenced = std::sync::atomic::AtomicBool::new(false);

        assert!(accepted_terminal_blocks_operation_cancellation(Some(
            &pending
        )));
        assert!(fence_provider_delivery_for_accepted_terminal(
            Some(&pending),
            &delivery_fenced,
        ));
        assert!(delivery_fenced.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            pending.executor_terminal,
            DurableExecutorTerminalKind::Completed,
            "bridge shutdown custody preserves the ordinary executor's actual terminal separately"
        );
        assert_eq!(
            pending.bridge_terminal.terminal(),
            meerkat_core::MeerkatExecutionTerminal::Completed,
            "channel shutdown never rewrites the executor's actual terminal"
        );
        assert_eq!(
            provider_output_after_delivery_fence(pending.bridge_terminal.clone(), true),
            None,
            "delivery fencing suppresses provider output without rewriting the physical terminal"
        );
        assert_eq!(
            pending.bridge_terminal.terminal(),
            meerkat_core::MeerkatExecutionTerminal::Completed,
            "provider suppression leaves the accepted physical terminal intact"
        );
        assert!(!accepted_terminal_blocks_operation_cancellation(None));
        let unaccepted_delivery = std::sync::atomic::AtomicBool::new(false);
        assert!(!fence_provider_delivery_for_accepted_terminal(
            None,
            &unaccepted_delivery,
        ));
        assert!(!unaccepted_delivery.load(std::sync::atomic::Ordering::Acquire));
    }

    #[tokio::test]
    async fn prepared_bound_channel_deactivates_without_starting_a_run() {
        let channels = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let binding = test_runtime_binding(SessionId::new(), "live-prepared", 1);
        reserve_bound_channel(&channels, binding.clone())
            .await
            .expect("reserve prepared channel");

        release_bound_channel(&channels, &binding)
            .await
            .expect("prepared deactivate is idempotent cleanup");

        assert!(channels.lock().await.is_empty());
    }

    #[tokio::test]
    async fn running_bound_channel_cancel_waits_for_run_completion() {
        let channels = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let binding = test_runtime_binding(SessionId::new(), "live-running", 1);
        reserve_bound_channel(&channels, binding.clone())
            .await
            .expect("reserve running channel");
        let cancellation = begin_bound_channel_run(&channels, &binding)
            .await
            .expect("begin exact channel run");

        let release_channels = Arc::clone(&channels);
        let release_binding = binding.clone();
        let release = tokio::spawn(async move {
            release_bound_channel(&release_channels, &release_binding).await
        });
        cancellation.cancelled().await;
        tokio::task::yield_now().await;
        assert!(
            !release.is_finished(),
            "deactivate returned before run exit"
        );

        finish_bound_channel_run(&channels, &binding).await;
        tokio::time::timeout(std::time::Duration::from_secs(1), release)
            .await
            .expect("deactivate cannot miss completion notification")
            .expect("deactivate task joins")
            .expect("deactivate succeeds");
        assert!(channels.lock().await.is_empty());
    }

    #[tokio::test]
    async fn completion_race_cannot_strand_bound_channel_deactivation() {
        for generation in 1..=64 {
            let channels = Arc::new(Mutex::new(std::collections::HashMap::new()));
            let binding =
                test_runtime_binding(SessionId::new(), "live-completion-race", generation);
            reserve_bound_channel(&channels, binding.clone())
                .await
                .expect("reserve raced channel");
            begin_bound_channel_run(&channels, &binding)
                .await
                .expect("begin raced run");

            let release_channels = Arc::clone(&channels);
            let release_binding = binding.clone();
            let release = tokio::spawn(async move {
                release_bound_channel(&release_channels, &release_binding).await
            });
            finish_bound_channel_run(&channels, &binding).await;
            tokio::time::timeout(std::time::Duration::from_secs(1), release)
                .await
                .expect("notification race cannot hang")
                .expect("raced deactivate task joins")
                .expect("raced deactivate succeeds");
            assert!(channels.lock().await.is_empty());
        }
    }

    #[tokio::test]
    async fn stale_bound_channel_deactivation_is_rejected_and_repeat_is_idempotent() {
        let channels = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let session_id = SessionId::new();
        let current = test_runtime_binding(session_id.clone(), "live-stale", 1);
        let stale = test_runtime_binding(session_id, "live-stale", 2);
        reserve_bound_channel(&channels, current.clone())
            .await
            .expect("reserve current channel");

        let error = release_bound_channel(&channels, &stale)
            .await
            .expect_err("stale incarnation cannot deactivate current channel");
        assert!(error.contains("does not match"));
        assert_eq!(channels.lock().await.len(), 1);

        release_bound_channel(&channels, &current)
            .await
            .expect("current prepared channel deactivates");
        release_bound_channel(&channels, &current)
            .await
            .expect("repeated deactivation is idempotent");
        assert!(channels.lock().await.is_empty());
    }

    #[test]
    fn one_result_delivery_reservation_prevents_duplicate_parent_context() {
        let mut result = RetainedDelegationResult::default();
        let first = result.reserve_delivery().expect("first reservation");
        assert!(result.reserve_delivery().is_none());

        result.release_delivery(first);
        assert!(result.reserve_delivery().is_some());

        result.terminal_ineligible = true;
        assert!(result.reserve_delivery().is_none());
    }

    #[test]
    fn responses_submission_digest_is_exact_and_separate_from_execution_terminal() {
        let terminal = LiveBridgeOperationTerminal::completed("exact output", 1024)
            .expect("completed terminal");
        let execution_digest =
            live_bridge_execution_result_digest(&terminal).expect("execution digest");
        let first =
            live_bridge_submission_output_digest("exact output").expect("submission digest");
        let second =
            live_bridge_submission_output_digest("exact output").expect("stable submission digest");
        assert_eq!(first, second);
        assert_ne!(first, execution_digest);
        assert!(live_bridge_submission_output_digest("").is_err());
    }

    #[tokio::test]
    async fn channel_shutdown_cancels_and_joins_owned_recovery() {
        let cancellation = CancellationToken::new();
        let waiter = cancellation.clone();
        let task = tokio::spawn(async move {
            waiter.cancelled().await;
        });
        let owned = OwnedResultRecovery {
            session_id: SessionId::new(),
            channel_id: meerkat_core::LiveChannelId::new("channel:shutdown"),
            cancellation,
            task,
        };
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            cancel_and_settle_result_recovery(owned),
        )
        .await
        .expect("recovery shutdown converges");
    }

    #[tokio::test]
    async fn failed_ambiguity_recovery_is_observed_once_without_blind_replay() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&attempts);
        let outcome =
            await_result_recovery_attempt_or_shutdown(CancellationToken::new(), async move {
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err::<(), _>("partially realized")
            })
            .await;
        assert_eq!(outcome, Some(Err("partially realized")));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn running_bridge_without_durable_child_is_broken_and_never_replayable() {
        let disposition = ExperimentalLiveDelegationCoordinator::classify_absent_responses_executor(
            DurableBoundedMemberState::Absent,
            meerkat_core::LiveBridgeOperationPhase::ExecutionRunning,
        );
        assert!(matches!(
            disposition,
            ExperimentalResponsesRestartDisposition::Broken { .. }
        ));
        assert!(matches!(
            ExperimentalLiveDelegationCoordinator::classify_absent_responses_executor(
                DurableBoundedMemberState::Absent,
                meerkat_core::LiveBridgeOperationPhase::PreFinalInference,
            ),
            ExperimentalResponsesRestartDisposition::NoExecutorBeforeFinalInput
        ));
    }
}
