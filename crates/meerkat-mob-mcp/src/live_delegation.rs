//! Shared provider-neutral coordinator for experimental GPT Live execution.
//!
//! Client-context delegation and Responses function bridging retain distinct
//! provider contracts, but both execute tool-bearing work on ordinary Mob
//! members. A Responses call waits for exact final-user transcript authority,
//! then persists a real durable fork of the channel-bound member and runs one
//! bounded child turn. The live endpoint itself never owns callback or effect
//! execution. ClientContext callers may explicitly borrow the existing member
//! instead; operation-scoped custody never confers member retirement authority.

use std::sync::Arc;

use meerkat::experimental_gpt_live::{
    ExperimentalGptLiveBridgeError, ExperimentalGptLiveControlObservation,
    ExperimentalGptLiveControlPlane, ExperimentalGptLiveNarrationDispatch,
    ExperimentalGptLiveResultDeliveryDispatch, ExperimentalLiveLifecycleObservationError,
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
    AgentIdentity, BoundedResultSpec, DelegationCancellationHandle, DelegationExecutionError,
    DelegationExecutionHandle, DelegationExecutionRequest, DelegationExecutionService,
    DelegationMemberOptions, DelegationTerminalizedExecution, DelegationTurnTerminal,
    DurableBoundedMemberState, DurableBoundedWorkState, LiveBridgeExecutionSnapshot,
    LiveBridgeOperationTerminal, MobDeliveryIdentity, MobHandle, WorkOrigin, WorkSpec,
    render_bounded_delegation_task,
};
use meerkat_runtime::live_execution::{
    LiveBridgeExecutionTerminalReceipt, LiveBridgeOperationAdmission,
    LiveBridgeRecoveredSubmissionReceipt, LiveBridgeRecoveredTerminalReceipt,
    LiveBridgeSubmissionAttemptAuthority, LiveBridgeSubmissionAuthority,
    LiveBridgeSubmissionReceipt, LiveDelegationCancellationDirective,
    LiveDelegationCancellationOutcome, LiveDelegationExecutionAdmission,
    LiveDelegationNarrationKind, LiveDelegationRecoverySnapshot,
    LiveDelegationResultDeliveryAuthority, LiveDelegationResultDeliveryObservation,
    LiveDelegationResultDeliveryResolution, LiveDelegationResultReleaseAuthority,
    LiveDelegationRuntimeBinding, LiveDelegationWorkerOwnership, LiveDelegationWorkerTerminalKind,
    LiveHandoffReconciliationReceipt,
};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Provider window behind one client delegation: the executor request and
/// what the assistant said natively in the same window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveDelegationExecutorInput {
    /// Every user transcript delta since the previous
    /// `session.delegation.created` on the channel (or since open).
    pub(crate) request_transcript: String,
    /// Assistant transcript received in that window; empty when none.
    pub(crate) assistant_context: String,
}

/// Heading of the labelled context section in the executor task text.
pub(crate) const LIVE_DELEGATION_ASSISTANT_CONTEXT_HEADING: &str = "Assistant already generated on the call meanwhile (context only, not part of the request; \
     the user may not have heard all of it; do not repeat it, and treat anything it already \
     answered as answered):";

/// The one seam that turns a delegation's provider window into the worker's
/// task text. The request is the user transcript of the whole window; the
/// assistant's native output in that window is appended as a separately
/// labelled section so the worker can see what was already answered, and is
/// never merged into the request itself. The label says "generated", not
/// "said": after a barge-in the transcript can describe audio the user never
/// heard.
pub(crate) fn delegation_request_text(input: &LiveDelegationExecutorInput) -> String {
    let request = input.request_transcript.trim();
    let context = input.assistant_context.trim();
    if context.is_empty() {
        return request.to_string();
    }
    format!("{request}\n\n{LIVE_DELEGATION_ASSISTANT_CONTEXT_HEADING}\n{context}")
}
mod schedule;

use schedule::{
    LIVE_DELEGATION_CHANNEL_WORKER_CAP, VoiceWorkGraph, VoiceWorkItem, WorkItemDisposition,
    fork_work_instructions, narration_text, narration_title, post_close_merge_text,
    task_after_failed_blockers, task_with_waited_results,
};

const LIVE_DELEGATION_RESULT_BYTES: usize = 16 * 1024;
/// Bounded attempts for post-close reconciliation steps that have no live
/// channel left to fence them; giving up is logged, never silent.
const POST_CLOSE_RECONCILE_ATTEMPTS: usize = 24;
/// Delay before a channel whose source member was mid-turn is offered the
/// queued delegation again.
const SOURCE_BUSY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(3);
/// Delay before a delegation whose WorkGraph item refused the scheduler's
/// claim (a sibling fork linked it between read and claim, or the store was
/// briefly unavailable) is offered again.
const WORKGRAPH_START_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(250);
/// Bounded WorkGraph start attempts before an item that keeps refusing the
/// claim is retired and the failure is spoken.
const WORKGRAPH_START_ATTEMPTS: u32 = 8;

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
const CLIENT_CONTEXT_RESTART_OBSERVE_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(50);
const CLIENT_CONTEXT_RESTART_PASS_BOUND: std::time::Duration =
    std::time::Duration::from_millis(250);
const CLIENT_CONTEXT_RESTART_RETRY_DELAY: std::time::Duration =
    std::time::Duration::from_millis(25);
const CLIENT_CONTEXT_RESTART_RETRY_MAX_DELAY: std::time::Duration =
    std::time::Duration::from_secs(1);

type ActiveChannelKey = (SessionId, meerkat_core::LiveChannelId);

fn live_worker_failure_terminal(
    failure: &meerkat_mob::BoundedTurnFailure,
) -> LiveDelegationWorkerTerminalKind {
    match failure {
        meerkat_mob::BoundedTurnFailure::Cancelled { .. } => {
            LiveDelegationWorkerTerminalKind::Cancelled
        }
        _ => LiveDelegationWorkerTerminalKind::Failed,
    }
}

/// Why a live delegation's executor did not start, as the voice channel
/// needs to know it.
///
/// `SourceBusy` is the one start failure a caller can act on truthfully (the
/// backing member was still mid-turn after the bounded wait; nothing was
/// forked); everything else is a failure. Narration keys off this type, not
/// off the rendered string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveDelegationStartFailure {
    /// The source member stayed mid-turn past the fork bound.
    SourceBusy {
        source_identity: AgentIdentity,
        waited_ms: u64,
    },
    /// Any other executor start failure, rendered.
    Failed(String),
}

impl LiveDelegationStartFailure {
    /// Stable kind label for logs and metrics.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SourceBusy { .. } => "source_busy",
            Self::Failed(_) => "failed",
        }
    }
}

impl From<&DelegationExecutionError> for LiveDelegationStartFailure {
    fn from(error: &DelegationExecutionError) -> Self {
        match error {
            DelegationExecutionError::SourceBusy {
                source_identity,
                waited_ms,
            } => Self::SourceBusy {
                source_identity: source_identity.clone(),
                waited_ms: *waited_ms,
            },
            other => Self::Failed(other.to_string()),
        }
    }
}

impl std::fmt::Display for LiveDelegationStartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceBusy {
                source_identity,
                waited_ms,
            } => write!(
                f,
                "source member {source_identity} was still mid-turn after {waited_ms} ms; the request was not started"
            ),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

/// Feature-owned execution placement for confirmed ClientContext delegations.
/// This never changes the chosen member's text model or build configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LiveDelegationExecutionPolicy {
    #[default]
    DurableFork,
    ExistingMember,
}

impl LiveDelegationExecutionPolicy {
    fn worker_identity(self, source: &AgentIdentity, operation: &OperationId) -> AgentIdentity {
        match self {
            Self::DurableFork => AgentIdentity::from(format!("live-delegation-{operation}")),
            Self::ExistingMember => source.clone(),
        }
    }

    fn worker_ownership(self) -> meerkat_runtime::live_execution::LiveDelegationWorkerOwnership {
        match self {
            Self::DurableFork => {
                meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::OwnedMember
            }
            Self::ExistingMember => {
                meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::ExistingMember
            }
        }
    }
}

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
    #[allow(
        dead_code,
        reason = "cancellation custody stays bound to the running worker"
    )]
    cancellation: DelegationCancellationHandle,
    #[allow(dead_code, reason = "the task owns its terminal realization")]
    task: JoinHandle<()>,
}

/// One admitted delegation between provider join and a worker slot: queued
/// (`Created`), or blocked behind WorkGraph dependencies awaiting requeue.
#[derive(Clone)]
struct PendingDelegation {
    provider_binding: ProviderWebrtcBinding,
    control: Arc<dyn ExperimentalGptLiveControlPlane>,
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    provisional: ProvisionalLiveHandoff,
    runtime_binding: LiveDelegationRuntimeBinding,
    mob_handle: MobHandle,
    source_identity: AgentIdentity,
    delegation: LiveSidebandDelegationRef,
    /// The provider user turn that carried the request; its adapter key
    /// identifies the final transcript item at commit time.
    turn: LiveSidebandTurnRef,
    /// Provider-final transcript text, committed canonically at dispatch.
    /// This is the canonical user row and the provisional handoff; it never
    /// carries the composed executor text.
    final_transcript: String,
    /// The provider window behind the delegation: `request_transcript` (the
    /// user-only window text) names the WorkGraph item and every narration;
    /// the composed `delegation_request_text` output is the fork's task.
    executor_input: LiveDelegationExecutorInput,
    /// Canonical commit evidence and its reconciliation once committed; a
    /// deferred item keeps them so a retry never commits twice.
    transcript: Option<ConfirmedTranscript>,
    workgraph: Option<VoiceWorkGraph>,
    work: Option<VoiceWorkItem>,
    title: String,
    /// The machine holds the item as Blocked; dispatch requeues it first.
    blocked: bool,
    /// Returned to the queue after a busy source or a WorkGraph refusal;
    /// skipped by the pump until the retry clears the flag.
    deferred: bool,
    /// The Queued narration has been released for this item.
    queued_narrated: bool,
    /// Dispatch attempts that ended in a deferral.
    start_attempts: u32,
    waited_on: Vec<meerkat::WorkItemId>,
    /// Blockers that ended without completing, found by the pump; dispatch
    /// replaces the item and prefaces the task with them.
    failed_blockers: Vec<(meerkat::WorkItemId, String)>,
}

/// Canonical transcript commit evidence and its `Confirmed` reconciliation.
#[derive(Clone)]
struct ConfirmedTranscript {
    final_evidence: FinalLiveUserTranscriptCommitEvidence,
    reconciliation: LiveHandoffReconciliationReceipt,
}

/// Why a scheduled delegation did not start on this pass.
#[derive(Debug)]
enum ScheduledStartFailure {
    /// The source member stayed mid-turn past the bound, at the canonical
    /// transcript commit or at the fork. Nothing physical exists; the item
    /// is deferred and retried.
    SourceBusy {
        source_identity: AgentIdentity,
        waited_ms: u64,
    },
    /// The WorkGraph refused the scheduler's claim or replacement (a revision
    /// conflict with a sibling fork's link, or a transient store error). The
    /// item is deferred and retried a bounded number of times.
    WorkGraph(String),
    /// Any other start failure: the request is retired and spoken as failed.
    Failed(String),
}

impl From<LiveDelegationStartFailure> for ScheduledStartFailure {
    fn from(failure: LiveDelegationStartFailure) -> Self {
        match failure {
            LiveDelegationStartFailure::SourceBusy {
                source_identity,
                waited_ms,
            } => Self::SourceBusy {
                source_identity,
                waited_ms,
            },
            LiveDelegationStartFailure::Failed(message) => Self::Failed(message),
        }
    }
}

/// Per-channel schedule. Arrival order is kept; readiness comes from the
/// WorkGraph; the running set is bounded by the generated worker cap.
struct ChannelSchedule {
    queue: std::collections::VecDeque<OperationId>,
    running: std::collections::BTreeSet<OperationId>,
    pending: std::collections::HashMap<OperationId, PendingDelegation>,
    /// Serializes delegation-lane provider appends (narration and results):
    /// the provider accepts one delegation append in flight per session.
    append_lane: Arc<Mutex<()>>,
    /// Results of completed items on this channel, by item, so a worker
    /// restarted after waiting on them receives them verbatim.
    completed_results: std::collections::HashMap<meerkat::WorkItemId, (String, String)>,
}

impl ChannelSchedule {
    fn new() -> Self {
        Self {
            queue: std::collections::VecDeque::new(),
            running: std::collections::BTreeSet::new(),
            pending: std::collections::HashMap::new(),
            append_lane: Arc::new(Mutex::new(())),
            completed_results: std::collections::HashMap::new(),
        }
    }
}

/// Exact identity and transport needed to narrate one delegation's state.
struct NarrationSubject {
    runtime_binding: LiveDelegationRuntimeBinding,
    operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
    control: Arc<dyn ExperimentalGptLiveControlPlane>,
    delegation: LiveSidebandDelegationRef,
    title: String,
    lane: Arc<Mutex<()>>,
}

impl NarrationSubject {
    fn from_pending(pending: &PendingDelegation, lane: Arc<Mutex<()>>) -> Self {
        Self {
            runtime_binding: pending.runtime_binding.clone(),
            operation: pending.operation.clone(),
            control: Arc::clone(&pending.control),
            delegation: pending.delegation.clone(),
            title: pending.title.clone(),
            lane,
        }
    }

    fn from_retained(retained: &RetainedDelegation) -> Self {
        Self {
            runtime_binding: retained.runtime_binding.clone(),
            operation: retained.operation.clone(),
            control: Arc::clone(&retained.control),
            delegation: retained.delegation.clone(),
            title: retained.title.clone(),
            lane: Arc::clone(&retained.append_lane),
        }
    }
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

/// Restart reconciliation outcome for one durable ClientContext executor.
///
/// This report carries no provider output or work-admission authority.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExperimentalClientContextRestartDisposition {
    InFlight,
    Reconciled { completed: bool },
    AlreadyReconciled,
    Broken { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentalClientContextRestartReport {
    operation_id: OperationId,
    disposition: ExperimentalClientContextRestartDisposition,
}

impl ExperimentalClientContextRestartReport {
    #[must_use]
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    #[must_use]
    pub fn disposition(&self) -> &ExperimentalClientContextRestartDisposition {
        &self.disposition
    }
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
    /// WorkGraph item this worker holds, absent in degraded serial mode.
    work: Option<VoiceWorkItem>,
    workgraph: Option<VoiceWorkGraph>,
    title: String,
    append_lane: Arc<Mutex<()>>,
    /// Source mob handle for post-close merging and owned-child retirement.
    mob_handle: Option<MobHandle>,
    source_identity: AgentIdentity,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExactDelegationResultProjectionEvidence {
    delegation_ref_digest: String,
    result_digest: String,
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

    async fn dispatch<Output, Release, ReleaseFuture>(
        self,
        release: Release,
    ) -> (Output, ExactDelegationResultProjectionEvidence)
    where
        Release: FnOnce(Authority, LiveSidebandDelegationRef, String) -> ReleaseFuture,
        ReleaseFuture: std::future::Future<Output = Output>,
    {
        let evidence = ExactDelegationResultProjectionEvidence {
            delegation_ref_digest: format!(
                "sha256:{:x}",
                Sha256::digest(self.delegation.adapter_key().as_bytes())
            ),
            result_digest: format!("sha256:{:x}", Sha256::digest(self.result_text.as_bytes())),
        };
        let output = release(self.authority, self.delegation, self.result_text).await;
        (output, evidence)
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
    /// A result dispatch reached the provider boundary (delivered or
    /// ambiguous). A closed channel merges only results that never crossed.
    dispatch_crossed: bool,
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
/// provider binding. Every client delegation becomes an item in the mob's
/// shared WorkGraph; up to the generated per-channel worker cap run at once,
/// the rest wait in arrival order until the WorkGraph reports them ready.
#[derive(Clone)]
pub struct ExperimentalLiveDelegationCoordinator {
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    mobs: Arc<crate::MobMcpState>,
    execution_policy: LiveDelegationExecutionPolicy,
    responses_projection_shutdown: Arc<ResponsesProjectionShutdown>,
    responses_prepared: Arc<Mutex<PreparedResponsesMap>>,
    responses_active: Arc<Mutex<ActiveResponsesMap>>,
    responses_pending_retirements: Arc<Mutex<PendingResponsesRetirementMap>>,
    active: Arc<Mutex<std::collections::HashMap<OperationId, ActiveDelegation>>>,
    schedules: Arc<Mutex<std::collections::HashMap<ActiveChannelKey, ChannelSchedule>>>,
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
    // User-input custody is channel-serial. Assistant output custody is
    // independently frozen by provider turn ref in `MeerkatMachine`, so a
    // later user turn may barge in without replacing the interrupted
    // assistant turn's interaction.
    active_user_turns: Arc<Mutex<std::collections::HashMap<ActiveChannelKey, ActiveProviderTurn>>>,
    completed_delegation_turns:
        Arc<Mutex<std::collections::HashMap<CompletedDelegationTurnKey, CompletedDelegationTurn>>>,
    bound_channels: BoundChannelMap,
    client_context_restart_reconciler_armed: Arc<std::sync::atomic::AtomicBool>,
    client_context_restart_inventory_ready: Arc<ClientContextRestartInventoryReady>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClientContextRestartInventoryEntry {
    session_id: SessionId,
    operation_id: OperationId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ClientContextRestartInventory {
    entries: Vec<ClientContextRestartInventoryEntry>,
}

impl ClientContextRestartInventory {
    #[cfg(test)]
    fn contains(&self, session_id: &SessionId, operation_id: &OperationId) -> bool {
        self.entries
            .iter()
            .any(|entry| &entry.session_id == session_id && &entry.operation_id == operation_id)
    }
}

#[derive(Default)]
struct ClientContextRestartInventoryReady {
    ready: std::sync::atomic::AtomicBool,
    changed: tokio::sync::Notify,
}

impl ClientContextRestartInventoryReady {
    fn mark_ready(&self) {
        self.ready.store(true, std::sync::atomic::Ordering::Release);
        self.changed.notify_waiters();
    }

    fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Acquire)
    }

    async fn wait(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_ready() {
                return;
            }
            changed.await;
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ClientContextRestartReconcileTiming {
    observation_bound: std::time::Duration,
    in_flight_delay: std::time::Duration,
    retry_delay: std::time::Duration,
    retry_max_delay: std::time::Duration,
}

impl ClientContextRestartReconcileTiming {
    const SHIPPING: Self = Self {
        observation_bound: CLIENT_CONTEXT_RESTART_PASS_BOUND,
        in_flight_delay: CLIENT_CONTEXT_RESTART_OBSERVE_INTERVAL,
        retry_delay: CLIENT_CONTEXT_RESTART_RETRY_DELAY,
        retry_max_delay: CLIENT_CONTEXT_RESTART_RETRY_MAX_DELAY,
    };
}

#[async_trait::async_trait]
trait ClientContextRestartRecoveryOwner: Send + Sync {
    async fn force_mob_restore(&self) -> Result<(), String>;

    async fn capture_client_context_restart_inventory(
        &self,
    ) -> Result<ClientContextRestartInventory, String>;

    async fn observe_client_context_restart_pass(
        &self,
        inventory: &ClientContextRestartInventory,
        observation_bound: std::time::Duration,
    ) -> Result<Vec<ExperimentalClientContextRestartReport>, String>;
}

async fn run_client_context_restart_reconciler<R>(
    owner: std::sync::Weak<R>,
    inventory_ready: Arc<ClientContextRestartInventoryReady>,
    timing: ClientContextRestartReconcileTiming,
) where
    R: ClientContextRestartRecoveryOwner + ?Sized + 'static,
{
    let mut retry_delay = timing.retry_delay;
    loop {
        let Some(current_owner) = owner.upgrade() else {
            return;
        };
        let restore = current_owner.force_mob_restore().await;
        drop(current_owner);
        match restore {
            Ok(()) => break,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "ClientContext restart reconciliation could not restore Mob state"
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay.saturating_mul(2).min(timing.retry_max_delay);
            }
        }
    }

    retry_delay = timing.retry_delay;
    let mut inventory = loop {
        let Some(current_owner) = owner.upgrade() else {
            return;
        };
        let capture = current_owner
            .capture_client_context_restart_inventory()
            .await;
        drop(current_owner);
        match capture {
            Ok(inventory) => break inventory,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "ClientContext restart inventory capture remains pending"
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay.saturating_mul(2).min(timing.retry_max_delay);
            }
        }
    };
    inventory_ready.mark_ready();

    retry_delay = timing.retry_delay;
    loop {
        let Some(current_owner) = owner.upgrade() else {
            return;
        };
        let pass = current_owner
            .observe_client_context_restart_pass(&inventory, timing.observation_bound)
            .await;
        drop(current_owner);
        match pass {
            Ok(reports) => {
                retry_delay = timing.retry_delay;
                for report in &reports {
                    if matches!(
                        report.disposition(),
                        ExperimentalClientContextRestartDisposition::Broken { .. }
                    ) {
                        tracing::warn!(
                            operation_id = %report.operation_id(),
                            disposition_reason = "durable_client_context_recovery_broken",
                            "ClientContext restart reconciliation stopped with durable recovery debt"
                        );
                    }
                }
                let in_flight_operations = reports
                    .iter()
                    .filter(|report| {
                        matches!(
                            report.disposition(),
                            ExperimentalClientContextRestartDisposition::InFlight
                        )
                    })
                    .map(|report| report.operation_id().clone())
                    .collect::<Vec<_>>();
                if in_flight_operations.is_empty() {
                    return;
                }
                inventory
                    .entries
                    .retain(|entry| in_flight_operations.contains(&entry.operation_id));
                tokio::time::sleep(timing.in_flight_delay).await;
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "ClientContext restart reconciliation scan remains pending"
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay.saturating_mul(2).min(timing.retry_max_delay);
            }
        }
    }
}

fn try_arm_client_context_restart_reconciler<R>(
    armed: &std::sync::atomic::AtomicBool,
    owner: Arc<R>,
    inventory_ready: Arc<ClientContextRestartInventoryReady>,
    timing: ClientContextRestartReconcileTiming,
) -> Option<JoinHandle<()>>
where
    R: ClientContextRestartRecoveryOwner + ?Sized + 'static,
{
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    if armed
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_err()
    {
        return None;
    }
    Some(runtime.spawn(run_client_context_restart_reconciler(
        Arc::downgrade(&owner),
        inventory_ready,
        timing,
    )))
}

/// Compose the one shared live-delegation lifecycle owner used by RPC and
/// MobKit hosts. The returned typed handle is also the provider binder's
/// erased `ExperimentalLiveBoundChannelActivator`.
#[must_use]
pub fn compose_experimental_live_delegation_coordinator(
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    mobs: Arc<crate::MobMcpState>,
) -> Arc<ExperimentalLiveDelegationCoordinator> {
    compose_experimental_live_delegation_coordinator_with_policy(
        runtime,
        mobs,
        LiveDelegationExecutionPolicy::default(),
    )
}

/// Compose an opt-in execution strategy at the feature owner, not in a host
/// callback. Recovery uses each operation's persisted custody, not this policy.
#[must_use]
pub fn compose_experimental_live_delegation_coordinator_with_policy(
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    mobs: Arc<crate::MobMcpState>,
    policy: LiveDelegationExecutionPolicy,
) -> Arc<ExperimentalLiveDelegationCoordinator> {
    let coordinator = Arc::new(
        ExperimentalLiveDelegationCoordinator::new(runtime, mobs).with_execution_policy(policy),
    );
    coordinator.arm_client_context_restart_reconciler();
    coordinator
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
            execution_policy: LiveDelegationExecutionPolicy::default(),
            responses_projection_shutdown: Arc::new(ResponsesProjectionShutdown {
                cancellation: CancellationToken::new(),
            }),
            responses_prepared: Arc::new(Mutex::new(std::collections::HashMap::new())),
            responses_active: Arc::new(Mutex::new(std::collections::HashMap::new())),
            responses_pending_retirements: Arc::new(Mutex::new(std::collections::HashMap::new())),
            active: Arc::new(Mutex::new(std::collections::HashMap::new())),
            schedules: Arc::new(Mutex::new(std::collections::HashMap::new())),
            retained: Arc::new(Mutex::new(std::collections::HashMap::new())),
            failed_start_cleanups: Arc::new(Mutex::new(std::collections::HashMap::new())),
            result_delivery_tasks: Arc::new(Mutex::new(std::collections::HashMap::new())),
            pending_result_recoveries: Arc::new(Mutex::new(std::collections::HashMap::new())),
            result_recovery_tasks: Arc::new(Mutex::new(std::collections::HashMap::new())),
            active_user_turns: Arc::new(Mutex::new(std::collections::HashMap::new())),
            completed_delegation_turns: Arc::new(Mutex::new(std::collections::HashMap::new())),
            bound_channels: Arc::new(Mutex::new(std::collections::HashMap::new())),
            client_context_restart_reconciler_armed: Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            )),
            client_context_restart_inventory_ready: Arc::new(
                ClientContextRestartInventoryReady::default(),
            ),
        }
    }

    #[must_use]
    pub fn with_execution_policy(mut self, policy: LiveDelegationExecutionPolicy) -> Self {
        self.execution_policy = policy;
        self
    }

    fn arm_client_context_restart_reconciler(self: &Arc<Self>) -> bool {
        try_arm_client_context_restart_reconciler(
            &self.client_context_restart_reconciler_armed,
            Arc::clone(self),
            Arc::clone(&self.client_context_restart_inventory_ready),
            ClientContextRestartReconcileTiming::SHIPPING,
        )
        .is_some()
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

    async fn reconcile_one_client_context_snapshot(
        &self,
        mob_handle: &meerkat_mob::MobHandle,
        snapshot: &LiveDelegationRecoverySnapshot,
        observe_until: tokio::time::Instant,
    ) -> ExperimentalClientContextRestartDisposition {
        if snapshot.phase() == meerkat_runtime::live_execution::LiveDelegationRecoveryPhase::Retired
            && !snapshot.result_eligible()
            && (snapshot.late() || snapshot.terminal().is_none())
        {
            // A retired worker without a turn terminal is generated
            // failed-start cleanup, not a missing durable work obligation.
            return ExperimentalClientContextRestartDisposition::AlreadyReconciled;
        }
        if snapshot.phase() == meerkat_runtime::live_execution::LiveDelegationRecoveryPhase::Failed
        {
            return ExperimentalClientContextRestartDisposition::Broken {
                reason: "generated ClientContext worker start failed before restart recovery"
                    .to_string(),
            };
        }

        let child_identity = AgentIdentity::from(snapshot.worker_identity());
        let delivery_identity = match MobDeliveryIdentity::new(
            snapshot.operation_id().to_string(),
            snapshot.interaction_id().to_string(),
        ) {
            Ok(identity) => identity,
            Err(error) => {
                return ExperimentalClientContextRestartDisposition::Broken {
                    reason: format!(
                        "recovered ClientContext delivery identity is invalid: {error}"
                    ),
                };
            }
        };
        let result_spec =
            match BoundedResultSpec::new("gpt_live_delegation", LIVE_DELEGATION_RESULT_BYTES) {
                Ok(spec) => spec,
                Err(error) => {
                    return ExperimentalClientContextRestartDisposition::Broken {
                        reason: format!(
                            "recovered ClientContext result specification is invalid: {error}"
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
                    return ExperimentalClientContextRestartDisposition::Broken {
                        reason: format!(
                            "durable ClientContext executor observation failed: {error}"
                        ),
                    };
                }
            };
            let (member, work) = recovery.into_parts();
            let terminal = match work {
                DurableBoundedWorkState::Absent => {
                    return ExperimentalClientContextRestartDisposition::Broken {
                        reason: "generated ClientContext worker has no durable work admission; restart recovery never resubmits"
                            .to_string(),
                    };
                }
                DurableBoundedWorkState::Broken { reason, .. } => {
                    return ExperimentalClientContextRestartDisposition::Broken { reason };
                }
                DurableBoundedWorkState::InFlight { .. } => {
                    if tokio::time::Instant::now() >= observe_until {
                        return ExperimentalClientContextRestartDisposition::InFlight;
                    }
                    tokio::time::sleep(CLIENT_CONTEXT_RESTART_OBSERVE_INTERVAL).await;
                    continue;
                }
                DurableBoundedWorkState::Terminal { result, .. } => match result {
                    Ok(_) => LiveDelegationWorkerTerminalKind::Completed,
                    Err(failure) => live_worker_failure_terminal(&failure),
                },
                _ => {
                    return ExperimentalClientContextRestartDisposition::Broken {
                        reason: "durable ClientContext recovery returned an unsupported work state"
                            .to_string(),
                    };
                }
            };

            match member {
                DurableBoundedMemberState::Active { .. }
                | DurableBoundedMemberState::Retiring { .. } => {
                    if snapshot.worker_ownership()
                        == meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::OwnedMember
                        && let Err(error) = mob_handle.retire(child_identity.clone()).await {
                        return ExperimentalClientContextRestartDisposition::Broken {
                            reason: format!(
                                "durable ClientContext executor retirement remains pending: {error}"
                            ),
                        };
                    }
                }
                DurableBoundedMemberState::Retired { .. } => {}
                DurableBoundedMemberState::Absent => {
                    return ExperimentalClientContextRestartDisposition::Broken {
                        reason: "durable ClientContext executor terminal lost member custody"
                            .to_string(),
                    };
                }
                DurableBoundedMemberState::Broken { reason, .. } => {
                    return ExperimentalClientContextRestartDisposition::Broken { reason };
                }
                _ => {
                    return ExperimentalClientContextRestartDisposition::Broken {
                        reason:
                            "durable ClientContext recovery returned an unsupported member state"
                                .to_string(),
                    };
                }
            }

            if let Err(error) = self
                .runtime
                .reconcile_revoked_live_delegation_worker_after_restart(snapshot, terminal)
                .await
            {
                return ExperimentalClientContextRestartDisposition::Broken {
                    reason: format!("durable ClientContext terminal remains unreconciled: {error}"),
                };
            }
            return ExperimentalClientContextRestartDisposition::Reconciled {
                completed: terminal == LiveDelegationWorkerTerminalKind::Completed,
            };
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

    async fn collect_client_context_restart_inventory(
        &self,
    ) -> Result<ClientContextRestartInventory, String> {
        let handles = self
            .mobs
            .mob_handles_snapshot()
            .await
            .map_err(|error| error.to_string())?;
        let mut inventory = ClientContextRestartInventory::default();
        let mut seen_sessions = std::collections::HashSet::new();
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
                    .live_delegation_recovery_snapshots(&session_id)
                    .await
                    .map_err(|error| error.to_string())?;
                for snapshot in snapshots {
                    inventory.entries.push(ClientContextRestartInventoryEntry {
                        session_id: session_id.clone(),
                        operation_id: snapshot.operation_id().clone(),
                    });
                }
            }
        }
        inventory.entries.sort_by(|left, right| {
            (left.session_id.to_string(), left.operation_id.to_string())
                .cmp(&(right.session_id.to_string(), right.operation_id.to_string()))
        });
        inventory.entries.dedup();
        Ok(inventory)
    }

    /// Reconcile only the ClientContext executor operations captured before
    /// live channel preparation was released for this process epoch.
    ///
    /// A channel is revoked only after its exact session/operation identity
    /// matches the fixed startup inventory. Operations admitted after the
    /// readiness boundary are invisible to every repeated recovery pass.
    async fn reconcile_client_context_inventory_after_restart(
        &self,
        inventory: &ClientContextRestartInventory,
        observation_bound: std::time::Duration,
    ) -> Result<Vec<ExperimentalClientContextRestartReport>, String> {
        if inventory.entries.is_empty() {
            return Ok(Vec::new());
        }
        let handles = self
            .mobs
            .mob_handles_snapshot()
            .await
            .map_err(|error| error.to_string())?;
        let mut recovered = Vec::new();
        let mut pending = inventory.entries.clone();
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
                    .live_delegation_recovery_snapshots(&session_id)
                    .await
                    .map_err(|error| error.to_string())?;
                for snapshot in snapshots {
                    let Some(pending_index) = pending.iter().position(|entry| {
                        entry.session_id == session_id
                            && &entry.operation_id == snapshot.operation_id()
                    }) else {
                        continue;
                    };
                    pending.swap_remove(pending_index);
                    if self
                        .runtime
                        .live_channel_is_active_for_session(&session_id, snapshot.channel_id())
                        .await
                    {
                        self.runtime
                            .abandon_live_open_admission(&session_id, snapshot.channel_id())
                            .await
                            .map_err(|error| {
                                format!(
                                    "failed to fence stale ClientContext channel '{}' before recovery: {error}",
                                    snapshot.channel_id()
                                )
                            })?;
                    }
                    let disposition = self
                        .reconcile_one_client_context_snapshot(
                            &mob_handle,
                            &snapshot,
                            observe_until,
                        )
                        .await;
                    recovered.push(ExperimentalClientContextRestartReport {
                        operation_id: snapshot.operation_id().clone(),
                        disposition,
                    });
                }
            }
        }
        if !pending.is_empty() {
            return Err(format!(
                "{} startup ClientContext operation(s) remain unavailable for exact recovery observation",
                pending.len()
            ));
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
                let failure = LiveDelegationStartFailure::from(&error);
                tracing::warn!(
                    kind = failure.kind(),
                    %failure,
                    "durable live executor fork did not start"
                );
                return Err(format!("durable live executor fork failed: {failure}"));
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
        if !self
            .client_context_restart_reconciler_armed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(
                "experimental live startup recovery was not armed on a Tokio runtime".to_string(),
            );
        }
        self.client_context_restart_inventory_ready.wait().await;
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
                        request_transcript,
                        assistant_context,
                    } = observation.kind()
                        && let Err(error) = self
                            .start_client_context_delegation(
                                &binding,
                                Arc::clone(&control),
                                turn.clone(),
                                delegation.clone(),
                                final_transcript.clone(),
                                LiveDelegationExecutorInput {
                                    request_transcript: request_transcript.clone(),
                                    assistant_context: assistant_context.clone(),
                                },
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
    ) -> Result<(), ExperimentalLiveLifecycleObservationError> {
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
            return Err(ExperimentalLiveLifecycleObservationError::CustodyLost(
                "provider lifecycle observation has no exact running channel custody".to_string(),
            ));
        }
        let applied = self.apply_provider_lifecycle(observation).await;
        match applied {
            Ok(()) => Ok(()),
            Err(reason) => Err(self.classify_lifecycle_failure(observation, reason).await),
        }
    }

    /// A lifecycle fact the runtime did not apply is a refusal while the
    /// observation's channel is still bound in the machine with the same
    /// fence and generation; once that binding is gone (the channel closed or
    /// was rebound under the observation), nothing further can be applied and
    /// the failure is lost custody. Both are typed state reads, never a
    /// reading of the error text.
    async fn classify_lifecycle_failure(
        &self,
        observation: &LiveSidebandObservation,
        reason: String,
    ) -> ExperimentalLiveLifecycleObservationError {
        let binding = observation.binding();
        let bound = self
            .runtime
            .live_delegation_runtime_binding(binding.session_id(), binding.channel_id())
            .await
            .is_ok_and(|runtime_binding| {
                runtime_binding.generation() == binding.runtime_generation().get()
                    && runtime_binding.fence_token() == binding.runtime_fence().get()
            });
        if bound {
            ExperimentalLiveLifecycleObservationError::Refused(reason)
        } else {
            ExperimentalLiveLifecycleObservationError::CustodyLost(reason)
        }
    }

    async fn apply_provider_lifecycle(
        &self,
        observation: &LiveSidebandObservation,
    ) -> Result<(), String> {
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
            // The outer context-mirror host owns the one-shot assistant-turn
            // correlation used by canonical playback projection. Repeating
            // that transition here would consume the same authority twice.
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
        let mut turns = self.active_user_turns.lock().await;
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
        let started = self
            .active_user_turns
            .lock()
            .await
            .remove(&key)
            .ok_or_else(|| {
                "provider turn finish has no local started-turn projection".to_string()
            })?;
        if started.authority.binding() != finished.binding()
            || started.authority.interaction_id() != finished.interaction_id()
            || started.authority.provider_turn_ref() != finished.provider_turn_ref()
        {
            return Err("provider turn finish does not match the exact started turn".to_string());
        }
        // Wake the owned delivery task even if the final transcript is empty
        // or already committed; ingress must not await its own provider ACK.
        self.runtime.wake_live_context_outbox(finished.binding());
        Ok(())
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
            .active_user_turns
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
        // conversational turn. Earlier delegations on this channel keep
        // running: arrival never cancels or supersedes them. Canonical
        // transcript reconciliation and all executor authority remain
        // control-owned below.
        self.runtime
            .admit_live_delegation(&runtime_binding, &operation, &provisional)
            .await
            .map_err(|error| error.to_string())?;
        let finished = self
            .runtime
            .observe_live_provider_turn_finished(observation)
            .await
            .map_err(|error| error.to_string())?;
        self.active_user_turns.lock().await.remove(&channel_key);
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
        self.runtime.wake_live_context_outbox(finished.binding());
        Ok(())
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
        executor_input: LiveDelegationExecutorInput,
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
        self.admit_scheduled_delegation(
            provider_binding,
            control,
            channel_key,
            operation,
            provisional,
            runtime_binding,
            mob_handle,
            source_identity,
            delegation,
            turn,
            final_transcript,
            executor_input,
        )
        .await;
        self.completed_delegation_turns
            .lock()
            .await
            .remove(&completed_key);
        Ok(())
    }

    /// The mob's shared WorkGraph as this mob's members see it. Members build
    /// in `mob.<id>`, and the host state rescopes its service to that realm;
    /// a host without a WorkGraph store leaves the channel on a strict serial
    /// queue instead of starting forks that can never build.
    fn voice_workgraph(&self, mob_handle: &MobHandle) -> Option<VoiceWorkGraph> {
        match self
            .mobs
            .workgraph_service_for_mob(&mob_handle.definition().id)
        {
            Ok(Some(service)) => Some(VoiceWorkGraph::new(service)),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(
                    %error,
                    mob_id = %mob_handle.definition().id,
                    "mob WorkGraph service unavailable; voice delegation runs serially"
                );
                None
            }
        }
    }

    /// Concurrent worker bound for one channel. The generated machine caps
    /// forks; an existing member executes one admitted turn at a time, so
    /// that policy is strictly serial regardless of the machine cap.
    fn channel_worker_cap(&self) -> usize {
        match self.execution_policy {
            LiveDelegationExecutionPolicy::DurableFork => LIVE_DELEGATION_CHANNEL_WORKER_CAP,
            LiveDelegationExecutionPolicy::ExistingMember => 1,
        }
    }

    /// Place an admitted delegation into its channel schedule and wake the
    /// schedule. The canonical transcript commit and reconciliation happen
    /// when the item is dispatched (see [`Self::start_scheduled_delegation`]):
    /// they wait, bounded, for the source member's turn boundary, and that
    /// wait must never hold up the channel's observation loop.
    #[allow(
        clippy::too_many_arguments,
        reason = "this exact delegation boundary carries independent provider, runtime, mob, and operation authorities"
    )]
    async fn admit_scheduled_delegation(
        &self,
        provider_binding: &ProviderWebrtcBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
        channel_key: ActiveChannelKey,
        operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
        provisional: ProvisionalLiveHandoff,
        runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        mob_handle: MobHandle,
        source_identity: AgentIdentity,
        delegation: LiveSidebandDelegationRef,
        turn: LiveSidebandTurnRef,
        final_transcript: String,
        executor_input: LiveDelegationExecutorInput,
    ) {
        let title = narration_title(&executor_input.request_transcript);
        let workgraph = self.voice_workgraph(&mob_handle);
        let work = match workgraph.as_ref() {
            Some(workgraph) => match workgraph
                .create_item(
                    provider_binding.channel_id(),
                    provider_binding.session_id(),
                    delegation.adapter_key(),
                    &executor_input.request_transcript,
                )
                .await
            {
                Ok(work) => Some(work),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        operation_id = %operation.operation_id(),
                        "voice delegation continues without a WorkGraph item (serial fallback)"
                    );
                    None
                }
            },
            None => None,
        };
        let operation_id = operation.operation_id().clone();
        let pending = PendingDelegation {
            provider_binding: provider_binding.clone(),
            control,
            operation,
            provisional,
            runtime_binding,
            mob_handle,
            source_identity,
            delegation,
            turn,
            final_transcript,
            executor_input,
            transcript: None,
            workgraph,
            work,
            title,
            blocked: false,
            deferred: false,
            queued_narrated: false,
            start_attempts: 0,
            waited_on: Vec::new(),
            failed_blockers: Vec::new(),
        };
        {
            let mut schedules = self.schedules.lock().await;
            let schedule = schedules
                .entry(channel_key.clone())
                .or_insert_with(ChannelSchedule::new);
            schedule.pending.insert(operation_id.clone(), pending);
            schedule.queue.push_back(operation_id);
        }
        self.spawn_pump(channel_key);
    }

    /// Wake one channel's schedule off the caller's task. Starting a worker
    /// may wait for the source member's turn boundary (bounded), so the
    /// observation loop that admits delegations never runs the pump inline.
    fn spawn_pump(&self, channel_key: ActiveChannelKey) {
        let coordinator = self.clone();
        tokio::spawn(async move {
            coordinator.pump_channel_schedule(&channel_key).await;
        });
    }

    /// Start queued delegations while a worker slot is free. An item with a
    /// WorkGraph binding starts when the WorkGraph reports it ready, or when
    /// every item it waited on ended without completing; an item without one
    /// (degraded mode) starts only when nothing else runs. Items deferred
    /// after a busy source or a transient WorkGraph refusal are skipped until
    /// their retry wakes the schedule again. When nothing can start, the
    /// items still waiting are narrated as queued, once each.
    ///
    /// Boxed because a worker's terminal realization pumps the schedule that
    /// starts the next worker: the future type would otherwise be recursive.
    fn pump_channel_schedule<'a>(
        &'a self,
        channel_key: &'a ActiveChannelKey,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(self.pump_channel_schedule_inner(channel_key))
    }

    async fn pump_channel_schedule_inner(&self, channel_key: &ActiveChannelKey) {
        loop {
            let workgraph = {
                let schedules = self.schedules.lock().await;
                let Some(schedule) = schedules.get(channel_key) else {
                    return;
                };
                schedule.queue.iter().find_map(|operation_id| {
                    schedule
                        .pending
                        .get(operation_id)
                        .and_then(|pending| pending.workgraph.clone())
                })
            };
            let readiness = match workgraph {
                Some(workgraph) => match workgraph.channel_readiness(&channel_key.1).await {
                    Ok(readiness) => Some(readiness),
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            "voice work readiness unavailable; scheduling this pass serially"
                        );
                        None
                    }
                },
                None => None,
            };
            let cap = self.channel_worker_cap();
            let next = {
                let mut schedules = self.schedules.lock().await;
                let Some(schedule) = schedules.get_mut(channel_key) else {
                    return;
                };
                let position = if schedule.running.len() >= cap {
                    None
                } else {
                    schedule.queue.iter().position(|operation_id| {
                        let Some(pending) = schedule.pending.get(operation_id) else {
                            return false;
                        };
                        if pending.deferred {
                            return false;
                        }
                        match (&pending.work, &readiness) {
                            (Some(work), Some(readiness)) => {
                                readiness.ready.contains(&work.id)
                                    || readiness.blockers_failed.contains_key(&work.id)
                            }
                            _ => schedule.running.is_empty(),
                        }
                    })
                };
                match position
                    .and_then(|index| schedule.queue.remove(index))
                    .and_then(|operation_id| {
                        let pending = schedule.pending.get_mut(&operation_id)?;
                        pending.failed_blockers = pending
                            .work
                            .as_ref()
                            .zip(readiness.as_ref())
                            .and_then(|(work, readiness)| {
                                readiness.blockers_failed.get(&work.id).cloned()
                            })
                            .unwrap_or_default();
                        let pending = pending.clone();
                        let waited = pending
                            .waited_on
                            .iter()
                            .filter_map(|item| schedule.completed_results.get(item).cloned())
                            .collect::<Vec<_>>();
                        schedule.running.insert(operation_id.clone());
                        Some((
                            operation_id,
                            pending,
                            waited,
                            Arc::clone(&schedule.append_lane),
                        ))
                    }) {
                    Some(next) => Some(next),
                    None => {
                        // Nothing can start on this pass: tell the user once
                        // about each item still waiting for a slot or a
                        // dependency. Deferred items carry their own narration.
                        let running_ahead = schedule.running.len();
                        let lane = Arc::clone(&schedule.append_lane);
                        let queued = schedule.queue.iter().cloned().collect::<Vec<_>>();
                        let mut waiting = Vec::new();
                        for operation_id in queued {
                            if let Some(pending) = schedule.pending.get_mut(&operation_id)
                                && !pending.deferred
                                && !pending.queued_narrated
                            {
                                pending.queued_narrated = true;
                                waiting.push(NarrationSubject::from_pending(
                                    pending,
                                    Arc::clone(&lane),
                                ));
                            }
                        }
                        for subject in waiting {
                            self.spawn_narration(
                                subject,
                                LiveDelegationNarrationKind::Queued,
                                running_ahead,
                                Vec::new(),
                                false,
                            );
                        }
                        None
                    }
                }
            };
            let Some((operation_id, pending, waited, lane)) = next else {
                return;
            };
            match self
                .start_scheduled_delegation(channel_key, &pending, waited, Arc::clone(&lane))
                .await
            {
                Ok(()) => {
                    self.spawn_narration(
                        NarrationSubject::from_pending(&pending, lane),
                        LiveDelegationNarrationKind::Claimed,
                        0,
                        Vec::new(),
                        false,
                    );
                }
                Err(ScheduledStartFailure::SourceBusy {
                    source_identity,
                    waited_ms,
                }) => {
                    tracing::info!(
                        operation_id = %operation_id,
                        %source_identity,
                        waited_ms,
                        "source member is mid-turn; voice delegation deferred for a later start"
                    );
                    self.defer_delegation(
                        channel_key,
                        &operation_id,
                        &pending,
                        lane,
                        Some(LiveDelegationNarrationKind::SourceBusy),
                        SOURCE_BUSY_RETRY_DELAY,
                    )
                    .await;
                }
                Err(ScheduledStartFailure::WorkGraph(error)) => {
                    let attempts = pending.start_attempts.saturating_add(1);
                    if attempts <= WORKGRAPH_START_ATTEMPTS {
                        tracing::info!(
                            %error,
                            operation_id = %operation_id,
                            attempts,
                            "voice work item refused the scheduler's claim; retrying shortly"
                        );
                        self.defer_delegation(
                            channel_key,
                            &operation_id,
                            &pending,
                            lane,
                            None,
                            WORKGRAPH_START_RETRY_DELAY,
                        )
                        .await;
                    } else {
                        tracing::warn!(
                            %error,
                            operation_id = %operation_id,
                            attempts,
                            "voice work item kept refusing the scheduler's claim; the request is retired"
                        );
                        self.retire_unstartable_delegation(
                            channel_key,
                            &operation_id,
                            &pending,
                            lane,
                        )
                        .await;
                    }
                }
                Err(ScheduledStartFailure::Failed(error)) => {
                    tracing::warn!(
                        %error,
                        operation_id = %operation_id,
                        "scheduled live delegation failed to start"
                    );
                    self.retire_unstartable_delegation(channel_key, &operation_id, &pending, lane)
                        .await;
                }
            }
        }
    }

    /// Return a delegation that could not start yet to its channel queue,
    /// marked deferred so the current pass moves on to other ready items, and
    /// wake the schedule again after `delay`. A start the machine recorded as
    /// failed (the executor refused with a busy source) is requeued in the
    /// machine first; a deferral before any worker authority was requested
    /// leaves the machine untouched. `narration`, when given, tells the user
    /// why the request has not started.
    async fn defer_delegation(
        &self,
        channel_key: &ActiveChannelKey,
        operation_id: &OperationId,
        pending: &PendingDelegation,
        lane: Arc<Mutex<()>>,
        narration: Option<LiveDelegationNarrationKind>,
        delay: std::time::Duration,
    ) {
        let state = self
            .runtime
            .live_delegation_schedule_state(pending.runtime_binding.session_id(), operation_id)
            .await
            .ok()
            .flatten();
        if state == Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Failed)
            && let Err(error) = self
                .runtime
                .requeue_live_delegation(&pending.runtime_binding, &pending.operation)
                .await
        {
            tracing::warn!(%error, %operation_id, "deferred delegation could not be requeued in the machine");
            self.retire_unstartable_delegation(channel_key, operation_id, pending, lane)
                .await;
            return;
        }
        {
            let mut schedules = self.schedules.lock().await;
            let Some(schedule) = schedules.get_mut(channel_key) else {
                return;
            };
            schedule.running.remove(operation_id);
            if let Some(entry) = schedule.pending.get_mut(operation_id) {
                entry.blocked = false;
                entry.deferred = true;
                entry.start_attempts = entry.start_attempts.saturating_add(1);
                entry.transcript = pending.transcript.clone();
                entry.work = pending.work.clone();
            }
            if !schedule.queue.contains(operation_id) {
                schedule.queue.push_back(operation_id.clone());
            }
        }
        if let Some(kind) = narration {
            self.spawn_narration(
                NarrationSubject::from_pending(pending, lane),
                kind,
                0,
                Vec::new(),
                false,
            );
        }
        let coordinator = self.clone();
        let key = channel_key.clone();
        let operation_id = operation_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Some(schedule) = coordinator.schedules.lock().await.get_mut(&key)
                && let Some(entry) = schedule.pending.get_mut(&operation_id)
            {
                entry.deferred = false;
            }
            coordinator.pump_channel_schedule(&key).await;
        });
    }

    /// Commit the delegation's canonical transcript if that has not happened
    /// yet, bind its WorkGraph item to the worker, and start the worker.
    ///
    /// The transcript commit waits at most the source turn bound for the
    /// member's turn-finalization boundary and reports a busy source instead
    /// of blocking behind a running turn. The committed evidence is kept on
    /// the pending item so a later retry never commits twice.
    async fn start_scheduled_delegation(
        &self,
        channel_key: &ActiveChannelKey,
        pending: &PendingDelegation,
        waited: Vec<(String, String)>,
        append_lane: Arc<Mutex<()>>,
    ) -> Result<(), ScheduledStartFailure> {
        let operation_id = pending.operation.operation_id().clone();
        if pending.blocked {
            self.runtime
                .requeue_live_delegation(&pending.runtime_binding, &pending.operation)
                .await
                .map_err(|error| ScheduledStartFailure::Failed(error.to_string()))?;
            if let Some(schedule) = self.schedules.lock().await.get_mut(channel_key)
                && let Some(entry) = schedule.pending.get_mut(&operation_id)
            {
                entry.blocked = false;
            }
        }
        let transcript = match pending.transcript.clone() {
            Some(transcript) => transcript,
            None => {
                let transcript = self.commit_delegation_transcript(pending).await?;
                if let Some(schedule) = self.schedules.lock().await.get_mut(channel_key)
                    && let Some(entry) = schedule.pending.get_mut(&operation_id)
                {
                    entry.transcript = Some(transcript.clone());
                }
                transcript
            }
        };
        let worker_identity = self
            .execution_policy
            .worker_identity(&pending.source_identity, &operation_id);
        let mut member = DelegationMemberOptions::default();
        let mut work = pending.work.clone();
        if self.execution_policy == LiveDelegationExecutionPolicy::DurableFork
            && let Some(current) = work.as_ref()
        {
            let workgraph = pending.workgraph.as_ref().ok_or_else(|| {
                ScheduledStartFailure::Failed(
                    "voice work item exists without a WorkGraph service".to_string(),
                )
            })?;
            if !pending.failed_blockers.is_empty() {
                let replacement = workgraph
                    .replace_after_failed_blockers(
                        current,
                        &pending.failed_blockers,
                        pending.provider_binding.channel_id(),
                        pending.provider_binding.session_id(),
                        pending.delegation.adapter_key(),
                        &pending.executor_input.request_transcript,
                    )
                    .await
                    .map_err(ScheduledStartFailure::WorkGraph)?;
                if let Some(schedule) = self.schedules.lock().await.get_mut(channel_key)
                    && let Some(entry) = schedule.pending.get_mut(&operation_id)
                {
                    entry.work = Some(replacement.clone());
                }
                work = Some(replacement);
            }
            let item = work.as_ref().ok_or_else(|| {
                ScheduledStartFailure::Failed("voice work item vanished before claim".to_string())
            })?;
            workgraph
                .claim(&item.id, worker_identity.as_str())
                .await
                .map_err(ScheduledStartFailure::WorkGraph)?;
            member.additional_instructions = Some(vec![fork_work_instructions(item)]);
            member.grant_workgraph_tools = true;
        }
        let task = task_after_failed_blockers(
            &task_with_waited_results(&delegation_request_text(&pending.executor_input), &waited),
            &pending.failed_blockers,
        );
        self.start_admitted_delegation(
            &pending.provider_binding,
            Arc::clone(&pending.control),
            channel_key.clone(),
            pending.operation.clone(),
            pending.provisional.clone(),
            pending.runtime_binding.clone(),
            pending.mob_handle.clone(),
            pending.source_identity.clone(),
            pending.delegation.clone(),
            transcript.final_evidence,
            transcript.reconciliation,
            pending.workgraph.clone(),
            work,
            pending.title.clone(),
            task,
            member,
            append_lane,
        )
        .await
        .map_err(ScheduledStartFailure::from)
    }

    /// Commit the provider-final transcript to the canonical session through
    /// the session owner, bounded by the source turn wait, and reconcile it
    /// with the generated machine. Only `Confirmed` reconciliation lets the
    /// delegation proceed to a worker.
    async fn commit_delegation_transcript(
        &self,
        pending: &PendingDelegation,
    ) -> Result<ConfirmedTranscript, ScheduledStartFailure> {
        let session_id = pending.provider_binding.session_id();
        let final_event = meerkat_core::RealtimeTranscriptEvent::UserTranscriptFinal {
            item_id: pending.turn.adapter_key().to_string(),
            previous_item_id: None,
            content_index: 0,
            text: pending.final_transcript.clone(),
        };
        let committed = self
            .mobs
            .session_service()
            .commit_live_delegation_final_transcript_at_turn_boundary(
                &self.runtime,
                session_id,
                pending.provisional.clone(),
                final_event,
                DelegationExecutionService::SOURCE_TURN_BOUNDARY_WAIT,
            )
            .await
            .map_err(|error| ScheduledStartFailure::Failed(error.to_string()))?;
        let final_evidence = match committed {
            meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary::Committed(evidence) => evidence,
            meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary::SourceBusy { waited } => {
                return Err(ScheduledStartFailure::SourceBusy {
                    source_identity: pending.source_identity.clone(),
                    waited_ms: u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
                });
            }
        };
        tracing::debug!(
            operation_id = %pending.operation.operation_id(),
            "client-context control committed canonical final transcript"
        );
        let reconciliation = self
            .runtime
            .reconcile_live_delegation_transcript(
                session_id,
                pending.runtime_binding.runtime_id(),
                pending.runtime_binding.fence_token(),
                pending.runtime_binding.generation(),
                &pending.operation,
                &pending.provisional,
                &final_evidence,
            )
            .await
            .map_err(|error| ScheduledStartFailure::Failed(error.to_string()))?;
        tracing::debug!(
            operation_id = %pending.operation.operation_id(),
            disposition = ?reconciliation.disposition(),
            "client-context control reconciled canonical transcript"
        );
        if reconciliation.disposition() != LiveHandoffReconciliation::Confirmed {
            return Err(ScheduledStartFailure::Failed(
                "canonical final transcript did not confirm the client delegation".to_string(),
            ));
        }
        Ok(ConfirmedTranscript {
            final_evidence,
            reconciliation,
        })
    }

    /// Drop a delegation that cannot start: cancel it in the machine when it
    /// is still queued, fail its WorkGraph item, and tell the user. The
    /// narration is authorized only once the machine holds the item as
    /// Failed or Cancelled, so an unstartable request is never silent.
    async fn retire_unstartable_delegation(
        &self,
        channel_key: &ActiveChannelKey,
        operation_id: &OperationId,
        pending: &PendingDelegation,
        lane: Arc<Mutex<()>>,
    ) {
        if let Some(schedule) = self.schedules.lock().await.get_mut(channel_key) {
            schedule.running.remove(operation_id);
            schedule.pending.remove(operation_id);
        }
        let state = self
            .runtime
            .live_delegation_schedule_state(pending.runtime_binding.session_id(), operation_id)
            .await
            .ok()
            .flatten();
        if matches!(
            state,
            Some(
                meerkat_runtime::live_execution::LiveDelegationScheduleState::Created
                    | meerkat_runtime::live_execution::LiveDelegationScheduleState::Blocked
            )
        ) && let Err(error) = self
            .runtime
            .cancel_queued_live_delegation(&pending.runtime_binding, &pending.operation)
            .await
        {
            tracing::warn!(%error, %operation_id, "unstartable live delegation could not be cancelled");
        }
        if let (Some(workgraph), Some(work)) = (pending.workgraph.as_ref(), pending.work.as_ref())
            && let Err(error) = workgraph
                .close(&work.id, meerkat::WorkStatus::Failed, None)
                .await
        {
            tracing::warn!(%error, %operation_id, "unstartable voice work item could not be closed");
        }
        self.narrate(
            NarrationSubject::from_pending(pending, lane),
            LiveDelegationNarrationKind::Failed,
            0,
            Vec::new(),
            false,
        )
        .await;
    }

    fn spawn_narration(
        &self,
        subject: NarrationSubject,
        kind: LiveDelegationNarrationKind,
        running_ahead: usize,
        blockers: Vec<String>,
        explicit_block: bool,
    ) {
        let coordinator = self.clone();
        tokio::spawn(async move {
            coordinator
                .narrate(subject, kind, running_ahead, blockers, explicit_block)
                .await;
        });
    }

    /// Release one templated narration under generated authority, taking the
    /// channel's delegation append lane for the dispatch.
    async fn narrate(
        &self,
        subject: NarrationSubject,
        kind: LiveDelegationNarrationKind,
        running_ahead: usize,
        blockers: Vec<String>,
        explicit_block: bool,
    ) {
        let lane = Arc::clone(&subject.lane);
        let _lane = lane.lock().await;
        self.narrate_on_held_lane(&subject, kind, running_ahead, blockers, explicit_block)
            .await;
    }

    /// [`Self::narrate`] for a caller that already holds the channel's
    /// delegation append lane. The machine refuses kinds that do not match
    /// the item's schedule state and repeats of the last released kind; a
    /// refusal is not an error here.
    async fn narrate_on_held_lane(
        &self,
        subject: &NarrationSubject,
        kind: LiveDelegationNarrationKind,
        running_ahead: usize,
        blockers: Vec<String>,
        explicit_block: bool,
    ) {
        // Lifecycle facts first: a stopped session or a channel that is no
        // longer this worker's active binding cannot grant narration
        // authority, and asking the machine anyway would only surface a
        // guard rejection for a condition already known here.
        if let Err(skip) = self
            .runtime
            .live_delegation_narration_eligibility(&subject.runtime_binding)
            .await
        {
            tracing::debug!(%skip, ?kind, "live delegation narration skipped");
            return;
        }
        let authority = match self
            .runtime
            .authorize_live_delegation_narration(&subject.runtime_binding, &subject.operation, kind)
            .await
        {
            Ok(authority) => authority,
            Err(error) => {
                tracing::debug!(%error, ?kind, "live delegation narration was not authorized");
                return;
            }
        };
        let text = narration_text(
            kind,
            &subject.title,
            running_ahead,
            &blockers,
            explicit_block,
        );
        match subject
            .control
            .narrate_delegation(authority, subject.delegation.clone(), text)
            .await
        {
            Ok(ExperimentalGptLiveNarrationDispatch::AwaitingAcknowledgement(waiter)) => {
                if let Err(error) = waiter.resolve().await {
                    tracing::debug!(%error, ?kind, "live delegation narration lost its acknowledgement");
                }
            }
            Ok(ExperimentalGptLiveNarrationDispatch::Resolved(_)) => {}
            Err(error) => {
                tracing::debug!(%error, ?kind, "live delegation narration was not delivered");
            }
        }
    }

    /// The channel closed under a retained result. Stop further provider
    /// delivery attempts and, for an owned fork whose result never crossed
    /// the provider boundary, merge it into the source member exactly once.
    /// The result text is taken under the lock, so whichever of the close
    /// sweep and the delivery task reaches this first merges and the other
    /// finds nothing, whatever order the machine close and the transport
    /// retirement arrive in.
    async fn merge_result_after_channel_close(&self, retained: &Arc<RetainedDelegation>) {
        let undelivered = {
            let mut result = retained.result.lock().await;
            result.terminal_ineligible = true;
            if result.dispatch_crossed {
                None
            } else {
                result.result_text.take()
            }
        };
        if retained.admission.worker_ownership() == LiveDelegationWorkerOwnership::OwnedMember
            && let Some(text) = undelivered
        {
            self.merge_result_into_source(retained, &text).await;
        }
        self.remove_retained_delegation(retained).await;
    }

    /// A worker that finished after its voice channel closed still merges:
    /// its result is queued on the source member as ordinary internal work.
    async fn merge_result_into_source(&self, retained: &RetainedDelegation, result_text: &str) {
        let result_spec =
            match BoundedResultSpec::new("gpt_live_delegation_merge", LIVE_DELEGATION_RESULT_BYTES)
            {
                Ok(spec) => spec,
                Err(error) => {
                    tracing::warn!(%error, "post-close voice result merge has no result spec");
                    return;
                }
            };
        let work = WorkSpec::new(
            post_close_merge_text(&retained.title, result_text),
            WorkOrigin::Internal,
        );
        let Some(mob_handle) = retained.mob_handle.as_ref() else {
            tracing::warn!(
                operation_id = %retained.operation.operation_id(),
                "post-close voice delegation result has no source mob handle to merge into"
            );
            return;
        };
        match mob_handle
            .start_work_for_identity_bounded(
                retained.source_identity.clone(),
                work,
                meerkat_core::types::HandlingMode::Queue,
                result_spec,
            )
            .await
        {
            Ok(_) => tracing::info!(
                operation_id = %retained.operation.operation_id(),
                "post-close voice delegation result merged into the source member"
            ),
            Err(error) => tracing::warn!(
                %error,
                operation_id = %retained.operation.operation_id(),
                "post-close voice delegation result could not be merged"
            ),
        }
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
        workgraph: Option<VoiceWorkGraph>,
        work: Option<VoiceWorkItem>,
        title: String,
        task: String,
        member: DelegationMemberOptions,
        append_lane: Arc<Mutex<()>>,
    ) -> Result<(), LiveDelegationStartFailure> {
        self.start_admitted_delegation_inner(
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
            workgraph,
            work,
            title,
            task,
            member,
            append_lane,
        )
        .await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "this exact delegation boundary carries independent provider, runtime, fork, and operation authorities"
    )]
    async fn start_admitted_delegation_inner(
        &self,
        provider_binding: &ProviderWebrtcBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
        channel_key: ActiveChannelKey,
        operation: ExactOperationIdentity<LiveUserTurnCorrelation>,
        provisional: ProvisionalLiveHandoff,
        runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        mob_handle: MobHandle,
        source_identity: AgentIdentity,
        delegation: LiveSidebandDelegationRef,
        final_evidence: FinalLiveUserTranscriptCommitEvidence,
        reconciliation: LiveHandoffReconciliationReceipt,
        workgraph: Option<VoiceWorkGraph>,
        work: Option<VoiceWorkItem>,
        title: String,
        task: String,
        member: DelegationMemberOptions,
        append_lane: Arc<Mutex<()>>,
    ) -> Result<(), LiveDelegationStartFailure> {
        let other = LiveDelegationStartFailure::Failed;
        let session_id = provider_binding.session_id();

        if reconciliation.disposition() != LiveHandoffReconciliation::Confirmed {
            return Err(other(
                "live delegation worker start requires Confirmed reconciliation".to_string(),
            ));
        }
        let committed_message_count =
            final_evidence.committed_message_count().ok_or_else(|| {
                other(
                    "confirmed live delegation is missing its exact transcript boundary"
                        .to_string(),
                )
            })?;

        let worker_identity = self
            .execution_policy
            .worker_identity(&source_identity, operation.operation_id());
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "client-context control requesting generated worker-start authority"
        );
        let admission = self
            .runtime
            .authorize_live_delegation_worker_start_with_ownership(
                session_id,
                runtime_binding.runtime_id(),
                runtime_binding.fence_token(),
                runtime_binding.generation(),
                &operation,
                &provisional,
                worker_identity.as_str(),
                self.execution_policy.worker_ownership(),
            )
            .await
            .map_err(|error| other(error.to_string()))?;
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
            .map_err(|error| other(error.to_string()))?;
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "client-context control received consequential-effect authority"
        );
        admission
            .release_tool_execution(&consequential)
            .map_err(|error| other(error.to_string()))?;
        let result_spec =
            BoundedResultSpec::new("gpt_live_delegation", LIVE_DELEGATION_RESULT_BYTES)
                .map_err(|error| other(error.to_string()))?;
        let service = DelegationExecutionService::new(mob_handle.clone());
        let request = DelegationExecutionRequest::new_live(
            worker_identity.clone(),
            task,
            result_spec,
            admission.clone(),
        );
        let mut request = match self.execution_policy {
            LiveDelegationExecutionPolicy::DurableFork => {
                request.with_durable_fork(source_identity.clone(), Some(committed_message_count))
            }
            LiveDelegationExecutionPolicy::ExistingMember => request.with_existing_member(),
        };
        request.member = member;
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "client-context control entering durable delegation service"
        );
        let execution = match service.start(request).await {
            Ok(execution) => execution,
            Err(error) => {
                let failure = LiveDelegationStartFailure::from(&error);
                if let LiveDelegationStartFailure::SourceBusy { .. } = &failure {
                    // Nothing physical exists: the fork was never cut.
                    // Record the failed start so the machine can requeue the
                    // exact operation; no child retirement is owed.
                    retry_reconciled_cleanup_step("busy-source-start-report", || {
                        self.runtime.resolve_live_delegation_worker_start(
                            runtime_binding.runtime_id(),
                            runtime_binding.fence_token(),
                            runtime_binding.generation(),
                            &admission,
                            false,
                        )
                    })
                    .await;
                    return Err(failure);
                }
                tracing::warn!(
                    operation_id = %operation.operation_id(),
                    kind = failure.kind(),
                    %failure,
                    "client-context delegation executor did not start"
                );
                let start_error = failure.to_string();
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
                return Err(other(format!("{start_error}{report_error}")));
            }
        };
        tracing::debug!(
            operation_id = %operation.operation_id(),
            "durable live delegation worker accepted its bounded turn"
        );
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
            work,
            workgraph,
            title,
            append_lane,
            mob_handle: Some(mob_handle),
            source_identity,
        });
        let Some(cancellation) = execution.cancellation_handle() else {
            let operation_id = retained.operation.operation_id().clone();
            let cleanup_operation_id = operation_id.clone();
            let cleanup_tasks = Arc::clone(&self.failed_start_cleanups);
            let cleanup_runtime = Arc::clone(&self.runtime);
            let cleanup_binding = retained.runtime_binding.clone();
            let cleanup_admission = retained.admission.clone();
            let cleanup_coordinator = self.clone();
            let cleanup_retained = Arc::clone(&retained);
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
                    &cleanup_coordinator,
                    &cleanup_retained,
                    &service,
                    execution.await_terminal().await,
                )
                .await;
                cleanup_tasks.lock().await.remove(&cleanup_operation_id);
            });
            self.failed_start_cleanups.lock().await.insert(
                operation_id,
                OwnedDelegationCleanup {
                    binding: retained.runtime_binding.clone(),
                    task: cleanup,
                },
            );
            let _ = cleanup_start_tx.send(());
            return Err(other(
                "live execution lost cancellation binding; terminal cleanup retained".to_string(),
            ));
        };
        self.retained.lock().await.insert(
            retained.operation.operation_id().clone(),
            Arc::clone(&retained),
        );
        let task_coordinator = Arc::new(self.clone());
        let task_retained = Arc::clone(&retained);
        let task_channel_key = channel_key.clone();
        let task_cancellation = cancellation.clone();
        let (task_start_tx, task_start_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let command = await_started_delegation_task_command(task_start_rx).await;
            if matches!(
                command,
                StartedDelegationTaskCommand::CleanupAfterStartPublicationFailure
            ) {
                cleanup_started_execution_after_publication_failure(
                    task_coordinator.as_ref(),
                    &task_retained,
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
                task_coordinator.as_ref(),
                &task_retained,
                &service,
                execution.await_terminal().await,
            )
            .await;
            tracing::debug!(
                operation_id = %task_retained.operation.operation_id(),
                result_present = terminal.result_text.is_some(),
                terminal_ineligible = terminal.terminal_ineligible,
                terminal = ?terminal.terminal,
                channel_closed = terminal.channel_closed,
                "durable live delegation worker reached realized terminality"
            );
            task_coordinator
                .record_terminal_realization(&task_channel_key, &task_retained, terminal)
                .await;
        });
        self.active.lock().await.insert(
            retained.operation.operation_id().clone(),
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
            return Err(other(format!(
                "generated successful worker-start publication failed; cleanup retained: {error}"
            )));
        }
        let _ = task_start_tx.send(StartedDelegationTaskCommand::Run);
        Ok(())
    }

    async fn record_terminal_realization(
        &self,
        channel_key: &ActiveChannelKey,
        retained: &Arc<RetainedDelegation>,
        terminal: RealizedDelegationTerminal,
    ) {
        let operation_id = retained.operation.operation_id().clone();
        {
            let mut result = retained.result.lock().await;
            result.result_text = terminal.result_text.clone();
            result.terminal_ineligible |= terminal.terminal_ineligible;
        }
        let blocker_titles = terminal
            .blockers
            .iter()
            .map(|(_, title)| title.clone())
            .collect::<Vec<_>>();
        {
            let mut schedules = self.schedules.lock().await;
            if let Some(schedule) = schedules.get_mut(channel_key) {
                schedule.running.remove(&operation_id);
                if terminal.terminal == LiveDelegationWorkerTerminalKind::Blocked {
                    if let Some(pending) = schedule.pending.get_mut(&operation_id) {
                        pending.blocked = true;
                        pending.waited_on =
                            terminal.blockers.iter().map(|(id, _)| id.clone()).collect();
                    }
                    if !schedule.queue.contains(&operation_id) {
                        schedule.queue.push_back(operation_id.clone());
                    }
                } else {
                    schedule.pending.remove(&operation_id);
                    if terminal.terminal == LiveDelegationWorkerTerminalKind::Completed
                        && let (Some(work), Some(text)) =
                            (retained.work.as_ref(), terminal.result_text.as_ref())
                    {
                        schedule
                            .completed_results
                            .insert(work.id.clone(), (retained.title.clone(), text.clone()));
                    }
                }
            }
        }
        match terminal.terminal {
            LiveDelegationWorkerTerminalKind::Blocked => {
                self.narrate(
                    NarrationSubject::from_retained(retained),
                    LiveDelegationNarrationKind::Blocked,
                    0,
                    blocker_titles,
                    terminal.explicit_block,
                )
                .await;
                self.remove_retained_delegation(retained).await;
            }
            LiveDelegationWorkerTerminalKind::Completed if terminal.channel_closed => {
                // An existing member executed the turn in its own canonical
                // session; only an owned fork's result needs merging back.
                if retained.admission.worker_ownership()
                    == LiveDelegationWorkerOwnership::OwnedMember
                    && let Some(text) = terminal.result_text.as_deref()
                {
                    self.merge_result_into_source(retained, text).await;
                }
                self.remove_retained_delegation(retained).await;
            }
            LiveDelegationWorkerTerminalKind::Completed if !terminal.terminal_ineligible => {
                // The Completed sentence and the result are released under one
                // hold of the channel's delegation append lane (see
                // `try_release_retained_result`), so another worker's
                // narration or result cannot land between them.
                self.schedule_result_delivery(Arc::clone(retained)).await;
            }
            LiveDelegationWorkerTerminalKind::Failed if !terminal.channel_closed => {
                self.narrate(
                    NarrationSubject::from_retained(retained),
                    LiveDelegationNarrationKind::Failed,
                    0,
                    Vec::new(),
                    false,
                )
                .await;
                self.remove_retained_delegation(retained).await;
            }
            _ => {
                self.remove_retained_delegation(retained).await;
            }
        }
        self.pump_channel_schedule(channel_key).await;
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
                        // The provider binding may be gone for good: a worker
                        // whose terminal landed after the transport retired
                        // but before the machine recorded the close. Once the
                        // machine no longer holds the channel active, the
                        // result takes the post-close path (merged into the
                        // source member) instead of retrying forever against
                        // a channel that will never come back.
                        // An unreadable machine state is unknown, not
                        // inactive: the attempt is retried, never merged.
                        if coordinator
                            .runtime
                            .live_channel_activity_for_session(
                                task_retained.runtime_binding.session_id(),
                                task_retained.runtime_binding.channel_id(),
                            )
                            .await
                            == Some(false)
                        {
                            coordinator
                                .merge_result_after_channel_close(&task_retained)
                                .await;
                            break;
                        }
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

        let mut active = self.active.lock().await;
        if active
            .get(operation_id)
            .is_some_and(|current| Arc::ptr_eq(&current.retained, retained))
        {
            active.remove(operation_id);
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
    ) -> (
        Result<ExperimentalGptLiveResultDeliveryDispatch, ExperimentalGptLiveBridgeError>,
        ExactDelegationResultProjectionEvidence,
    ) {
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
        // One delegation-lane append in flight per session: narration and
        // results of every worker on this channel share this lane. The
        // Completed sentence and the result it introduces go out under the
        // same hold, so nothing from another worker interleaves.
        let lane_guard = retained.append_lane.lock().await;
        if retained.result.lock().await.terminal_ineligible {
            retained.result.lock().await.release_delivery(reservation);
            return Ok(());
        }
        self.narrate_on_held_lane(
            &NarrationSubject::from_retained(retained),
            LiveDelegationNarrationKind::Completed,
            0,
            Vec::new(),
            false,
        )
        .await;
        retained.result.lock().await.dispatch_crossed = true;
        let (dispatch, projection_evidence) = Self::release_exact_delegation_result_projection(
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
                let mut result = retained.result.lock().await;
                result.dispatch_crossed = false;
                result.release_delivery(reservation);
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
        drop(lane_guard);
        let (authority, observation) = resolution.into_parts();
        if observation == LiveDelegationResultDeliveryObservation::Delivered {
            tracing::info!(
                operation_id = %retained.operation.operation_id(),
                delegation_ref_digest = %projection_evidence.delegation_ref_digest,
                result_digest = %projection_evidence.result_digest,
                "exact bounded live delegation result received provider append acknowledgement"
            );
        }
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
                .get(retained.operation.operation_id())
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
        self.active_user_turns.lock().await.remove(&key);
        self.completed_delegation_turns
            .lock()
            .await
            .retain(|(session_id, channel_id, _), _| {
                session_id != binding.session_id() || channel_id != binding.channel_id()
            });
        self.cancel_responses_executions_for_binding(binding).await;
        self.settle_result_recovery_tasks(binding).await;
        self.settle_failed_start_cleanups(binding).await;
        // Channel close cancels only work that never started: queued items
        // and blocked items awaiting requeue. Running forks keep their
        // machine custody and finish; their results merge into the source
        // member instead of reaching the closed provider channel.
        let (queued, running) = {
            let mut schedules = self.schedules.lock().await;
            match schedules.remove(&key) {
                Some(schedule) => {
                    let queued = schedule
                        .queue
                        .iter()
                        .filter_map(|operation_id| schedule.pending.get(operation_id).cloned())
                        .collect::<Vec<_>>();
                    (queued, schedule.running)
                }
                None => (Vec::new(), std::collections::BTreeSet::new()),
            }
        };
        for pending in queued {
            if pending.runtime_binding.generation() != binding.runtime_generation().get()
                || pending.runtime_binding.fence_token() != binding.runtime_fence().get()
            {
                continue;
            }
            if let Err(error) = self
                .runtime
                .cancel_queued_live_delegation(&pending.runtime_binding, &pending.operation)
                .await
            {
                tracing::warn!(
                    %error,
                    operation_id = %pending.operation.operation_id(),
                    "queued live delegation could not be cancelled at channel close"
                );
            }
            if let (Some(workgraph), Some(work)) =
                (pending.workgraph.as_ref(), pending.work.as_ref())
                && let Err(error) = workgraph
                    .close(&work.id, meerkat::WorkStatus::Cancelled, None)
                    .await
            {
                tracing::warn!(%error, "queued voice work item could not be cancelled");
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
                    && !running.contains(retained.operation.operation_id())
            })
            .cloned()
            .collect::<Vec<_>>();
        for retained in retained {
            // Stop further delivery attempts, let an attempt already in
            // flight settle, then decide under the lock whether the result
            // ever reached the provider. A result that crossed the boundary
            // (delivered or ambiguous) is never also merged into the source.
            retained.result.lock().await.terminal_ineligible = true;
            self.settle_result_delivery_task(retained.operation.operation_id())
                .await;
            self.merge_result_after_channel_close(&retained).await;
        }
        self.settle_responses_retirement_debt_for_binding(binding)
            .await;
    }
}

#[async_trait::async_trait]
impl ClientContextRestartRecoveryOwner for ExperimentalLiveDelegationCoordinator {
    async fn force_mob_restore(&self) -> Result<(), String> {
        self.mobs
            .ensure_restored()
            .await
            .map_err(|error| error.to_string())
    }

    async fn capture_client_context_restart_inventory(
        &self,
    ) -> Result<ClientContextRestartInventory, String> {
        ExperimentalLiveDelegationCoordinator::collect_client_context_restart_inventory(self).await
    }

    async fn observe_client_context_restart_pass(
        &self,
        inventory: &ClientContextRestartInventory,
        observation_bound: std::time::Duration,
    ) -> Result<Vec<ExperimentalClientContextRestartReport>, String> {
        self.reconcile_client_context_inventory_after_restart(inventory, observation_bound)
            .await
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
    ) -> Result<(), ExperimentalLiveLifecycleObservationError> {
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
    coordinator: &ExperimentalLiveDelegationCoordinator,
    retained: &RetainedDelegation,
    service: &DelegationExecutionService,
    cancellation: &DelegationCancellationHandle,
    execution: DelegationExecutionHandle,
) {
    let runtime = coordinator.runtime.as_ref();
    let binding = &retained.runtime_binding;
    let admission = &retained.admission;
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
        coordinator,
        retained,
        service,
        execution.await_terminal().await,
    )
    .await;
}

struct RealizedDelegationTerminal {
    result_text: Option<String>,
    terminal_ineligible: bool,
    terminal: LiveDelegationWorkerTerminalKind,
    blockers: Vec<(meerkat::WorkItemId, String)>,
    explicit_block: bool,
    /// The provider channel unbound before the terminal could be recorded
    /// under its binding; the worker was reconciled as revoked instead.
    channel_closed: bool,
}

/// Retry a binding-fenced step while the worker's channel is still bound.
/// `None` means the channel unbound and the caller must switch to the
/// revoked-worker path instead of retrying forever.
async fn retry_while_channel_active<T, E, F, Fut>(
    runtime: &meerkat_runtime::MeerkatMachine,
    binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    label: &'static str,
    mut step: F,
) -> Option<T>
where
    E: std::fmt::Display,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut retry_delay = LIVE_DELEGATION_CLEANUP_RETRY_DELAY;
    loop {
        match step().await {
            Ok(value) => return Some(value),
            Err(error) => {
                if !runtime
                    .live_channel_is_active_for_session(binding.session_id(), binding.channel_id())
                    .await
                {
                    tracing::info!(
                        cleanup_step = label,
                        "live channel unbound; switching to revoked worker reconciliation"
                    );
                    return None;
                }
                tracing::warn!(%error, cleanup_step = label, "live delegation cleanup remains pending");
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay
                    .saturating_mul(2)
                    .min(LIVE_DELEGATION_CLEANUP_RETRY_MAX_DELAY);
            }
        }
    }
}

async fn retry_bounded<T, E, F, Fut>(label: &'static str, attempts: usize, mut step: F) -> Option<T>
where
    E: std::fmt::Display,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut retry_delay = LIVE_DELEGATION_CLEANUP_RETRY_DELAY;
    for attempt in 1..=attempts {
        match step().await {
            Ok(value) => return Some(value),
            Err(error) if attempt == attempts => {
                tracing::error!(%error, cleanup_step = label, attempts, "post-close live delegation step gave up");
            }
            Err(error) => {
                tracing::warn!(%error, cleanup_step = label, attempt, "post-close live delegation step remains pending");
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay
                    .saturating_mul(2)
                    .min(LIVE_DELEGATION_CLEANUP_RETRY_MAX_DELAY);
            }
        }
    }
    None
}

/// Combine the Mob bounded-turn terminal with the exact WorkGraph facts the
/// worker left behind. Only statuses, edges, and readiness are read.
async fn classify_worker_terminal(
    retained: &RetainedDelegation,
    mob_terminal: LiveDelegationWorkerTerminalKind,
    result_text: Option<&str>,
) -> (
    LiveDelegationWorkerTerminalKind,
    Vec<(meerkat::WorkItemId, String)>,
    bool,
) {
    let (Some(workgraph), Some(work)) = (retained.workgraph.as_ref(), retained.work.as_ref())
    else {
        return (mob_terminal, Vec::new(), false);
    };
    let disposition = match workgraph.disposition_after_worker_turn(&work.id).await {
        Ok(disposition) => disposition,
        Err(error) => {
            tracing::warn!(%error, "voice work item state unavailable; using the Mob terminal alone");
            return (mob_terminal, Vec::new(), false);
        }
    };
    let close = |status: meerkat::WorkStatus, summary: Option<&str>| {
        let workgraph = workgraph.clone();
        let item = work.id.clone();
        let summary = summary.map(str::to_string);
        async move {
            if let Err(error) = workgraph.close(&item, status, summary.as_deref()).await {
                tracing::warn!(%error, "voice work item could not be closed after its worker ended");
            }
        }
    };
    match (mob_terminal, disposition) {
        (LiveDelegationWorkerTerminalKind::Completed, WorkItemDisposition::Completed) => (
            LiveDelegationWorkerTerminalKind::Completed,
            Vec::new(),
            false,
        ),
        (
            LiveDelegationWorkerTerminalKind::Completed,
            WorkItemDisposition::InProgress | WorkItemDisposition::ReleasedReady,
        ) => {
            close(meerkat::WorkStatus::Completed, result_text).await;
            (
                LiveDelegationWorkerTerminalKind::Completed,
                Vec::new(),
                false,
            )
        }
        (LiveDelegationWorkerTerminalKind::Completed, WorkItemDisposition::Failed) => {
            (LiveDelegationWorkerTerminalKind::Failed, Vec::new(), false)
        }
        (LiveDelegationWorkerTerminalKind::Completed, WorkItemDisposition::Cancelled) => (
            LiveDelegationWorkerTerminalKind::Cancelled,
            Vec::new(),
            false,
        ),
        (
            LiveDelegationWorkerTerminalKind::Completed,
            WorkItemDisposition::Waiting {
                blockers,
                explicit_block,
            },
        ) => (
            LiveDelegationWorkerTerminalKind::Blocked,
            blockers,
            explicit_block,
        ),
        (terminal, disposition) => {
            if !disposition.is_terminal() {
                let status = if terminal == LiveDelegationWorkerTerminalKind::Cancelled {
                    meerkat::WorkStatus::Cancelled
                } else {
                    meerkat::WorkStatus::Failed
                };
                close(status, None).await;
            }
            (terminal, Vec::new(), false)
        }
    }
}

async fn realize_terminal(
    coordinator: &ExperimentalLiveDelegationCoordinator,
    retained: &RetainedDelegation,
    service: &DelegationExecutionService,
    terminalized: DelegationTerminalizedExecution,
) -> RealizedDelegationTerminal {
    let runtime = coordinator.runtime.as_ref();
    let binding = &retained.runtime_binding;
    let admission = &retained.admission;
    let mob_terminal = match terminalized.terminal() {
        DelegationTurnTerminal::Completed(_) => LiveDelegationWorkerTerminalKind::Completed,
        DelegationTurnTerminal::Failed(error) => {
            tracing::warn!(
                operation_id = %admission.operation().operation_id(),
                %error,
                "live delegation worker turn failed"
            );
            live_worker_failure_terminal(error.failure())
        }
        _ => LiveDelegationWorkerTerminalKind::Failed,
    };
    let mob_result_text = match terminalized.terminal() {
        DelegationTurnTerminal::Completed(turn) => Some(turn.result().result().text().to_string()),
        DelegationTurnTerminal::Failed(_) => None,
        _ => None,
    };
    let (terminal_kind, blockers, explicit_block) =
        classify_worker_terminal(retained, mob_terminal, mob_result_text.as_deref()).await;
    if runtime
        .live_channel_is_active_for_session(binding.session_id(), binding.channel_id())
        .await
        && let Some(terminal_receipt) =
            retry_while_channel_active(runtime, binding, "worker-terminal-record", || {
                runtime.record_live_delegation_worker_terminal(
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    admission,
                    terminal_kind,
                )
            })
            .await
        && let Some(retirement) =
            retry_while_channel_active(runtime, binding, "worker-retirement-authority", || {
                runtime.authorize_live_delegation_worker_retirement(
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    admission,
                )
            })
            .await
    {
        retry_reconciled_cleanup_step("worker-physical-retirement", || {
            service.retire_live_terminalized(&terminalized, &retirement)
        })
        .await;
        let resolved =
            retry_while_channel_active(runtime, binding, "worker-retirement-resolution", || {
                runtime.resolve_live_delegation_worker_retirement(
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    &retirement,
                    true,
                )
            })
            .await;
        if resolved.is_some() {
            let result_text = retain_terminal_result(
                true,
                terminal_kind,
                terminal_receipt.late(),
                mob_result_text,
            );
            return RealizedDelegationTerminal {
                terminal_ineligible: result_text.is_none(),
                result_text,
                terminal: terminal_kind,
                blockers,
                explicit_block,
                channel_closed: false,
            };
        }
    }
    realize_terminal_after_channel_close(
        coordinator,
        retained,
        terminal_kind,
        blockers,
        explicit_block,
        mob_result_text,
    )
    .await
}

/// The provider channel is gone. Release executor custody the way restart
/// reconciliation does: retire the owned child, then record the durable
/// terminal as revoked (late, never provider-eligible). The result itself
/// is returned so the caller can merge it into the source member.
async fn realize_terminal_after_channel_close(
    coordinator: &ExperimentalLiveDelegationCoordinator,
    retained: &RetainedDelegation,
    terminal_kind: LiveDelegationWorkerTerminalKind,
    blockers: Vec<(meerkat::WorkItemId, String)>,
    explicit_block: bool,
    mob_result_text: Option<String>,
) -> RealizedDelegationTerminal {
    let runtime = coordinator.runtime.as_ref();
    let session_id = retained.runtime_binding.session_id();
    let operation_id = retained.operation.operation_id().clone();
    if retained.admission.worker_ownership() == LiveDelegationWorkerOwnership::OwnedMember
        && let Some(mob_handle) = retained.mob_handle.as_ref()
        && let Err(error) = mob_handle
            .retire(AgentIdentity::from(retained.admission.worker_identity()))
            .await
    {
        tracing::debug!(%error, %operation_id, "post-close voice worker retirement reported an error");
    }
    let snapshot = retry_bounded(
        "post-close-worker-snapshot",
        POST_CLOSE_RECONCILE_ATTEMPTS,
        || async {
            runtime
                .live_delegation_recovery_snapshots(session_id)
                .await
                .map_err(|error| error.to_string())
                .and_then(|snapshots| {
                    snapshots
                        .into_iter()
                        .find(|snapshot| snapshot.operation_id() == &operation_id)
                        .ok_or_else(|| "worker operation has no durable snapshot".to_string())
                })
        },
    )
    .await;
    if let Some(snapshot) = snapshot {
        retry_bounded(
            "post-close-revoked-worker-reconciliation",
            POST_CLOSE_RECONCILE_ATTEMPTS,
            || {
                runtime.reconcile_revoked_live_delegation_worker_after_restart(
                    &snapshot,
                    terminal_kind,
                )
            },
        )
        .await;
    }
    let result_text = (terminal_kind == LiveDelegationWorkerTerminalKind::Completed)
        .then_some(mob_result_text)
        .flatten()
        .filter(|text| !text.trim().is_empty());
    RealizedDelegationTerminal {
        result_text,
        terminal_ineligible: true,
        terminal: terminal_kind,
        blockers,
        explicit_block,
        channel_closed: true,
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

    #[cfg(all(
        feature = "experimental-gpt-live-gate0-harness",
        not(target_arch = "wasm32")
    ))]
    mod parallel;

    #[test]
    fn delegation_request_text_keeps_assistant_speech_as_a_labelled_section() {
        // Interjection mid-request (S100): the whole request is the task and
        // the backchannel is context under its own heading, never merged.
        let split = LiveDelegationExecutorInput {
            request_transcript: "please write the standup notes with two headings".into(),
            assistant_context: "mm-hm".into(),
        };
        let text = delegation_request_text(&split);
        let (request, context) = text
            .split_once(&format!(
                "\n\n{LIVE_DELEGATION_ASSISTANT_CONTEXT_HEADING}\n"
            ))
            .expect("labelled context section");
        assert_eq!(request, "please write the standup notes with two headings");
        assert_eq!(context, "mm-hm");
        // Nothing spoken in the window: the task is the request alone.
        let quiet = LiveDelegationExecutorInput {
            request_transcript: " second task ".into(),
            assistant_context: String::new(),
        };
        assert_eq!(delegation_request_text(&quiet), "second task");
        // A native answer between two requests (S103) lands in the context
        // section; the request text carries only user transcript.
        let answered = LiveDelegationExecutorInput {
            request_transcript: "what day is it also add a summary".into(),
            assistant_context: "on it it is Tuesday".into(),
        };
        let text = delegation_request_text(&answered);
        assert!(text.starts_with("what day is it also add a summary\n\n"));
        assert!(text.ends_with("\non it it is Tuesday"));
        assert_eq!(text.matches("Tuesday").count(), 1);
    }

    #[test]
    fn live_and_recovered_cancelled_terminals_keep_the_same_typed_class() {
        assert_eq!(
            live_worker_failure_terminal(&meerkat_mob::BoundedTurnFailure::Cancelled {
                session_id: SessionId::new(),
            }),
            LiveDelegationWorkerTerminalKind::Cancelled,
        );
        assert_eq!(
            live_worker_failure_terminal(
                &meerkat_mob::BoundedTurnFailure::CompletedWithoutResult {
                    session_id: SessionId::new(),
                }
            ),
            LiveDelegationWorkerTerminalKind::Failed,
        );
    }

    #[test]
    fn execution_policy_defaults_to_fork_and_existing_member_is_identity_preserving() {
        let source = AgentIdentity::from("selected-console-agent");
        let operation = OperationId::new();
        assert_eq!(
            LiveDelegationExecutionPolicy::default(),
            LiveDelegationExecutionPolicy::DurableFork
        );
        assert_ne!(
            LiveDelegationExecutionPolicy::default().worker_identity(&source, &operation),
            source
        );
        assert_eq!(
            LiveDelegationExecutionPolicy::ExistingMember.worker_identity(&source, &operation),
            source
        );
        assert_eq!(
            LiveDelegationExecutionPolicy::ExistingMember.worker_ownership(),
            meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::ExistingMember
        );
    }

    #[tokio::test]
    async fn restarted_default_coordinator_retains_borrowed_member_and_never_resubmits() {
        let mut sessions = crate::LocalSessionService::new();
        sessions.runtime_adapter =
            Arc::new(meerkat_runtime::MeerkatMachine::persistent_without_blobs(
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            ));
        let runtime = Arc::clone(&sessions.runtime_adapter);
        let sessions = Arc::new(sessions);
        let mobs = Arc::new(crate::MobMcpState::new(
            sessions.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let mob = meerkat_mob::MobBuilder::new(
            meerkat_mob::MobDefinition::implicit("existing-member-restart", "claude-sonnet-4-5"),
            meerkat_mob::MobStorage::in_memory(),
        )
        .with_session_service(sessions)
        .allow_ephemeral_sessions(true)
        .create()
        .await
        .expect("mob");
        let identity = AgentIdentity::from("existing-console-agent");
        let mut spec = meerkat_mob::SpawnMemberSpec::new("delegate", identity.as_str());
        spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
        mob.spawn_spec(spec).await.expect("existing source");
        let session_id = mob
            .resolve_bridge_session_id(&identity)
            .await
            .expect("source session");
        let admission = runtime
            .__test_admit_confirmed_live_delegation(
                &session_id,
                identity.as_str(),
                meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::ExistingMember,
                "retained voice work",
            )
            .await
            .expect("generated existing admission");
        let execution = DelegationExecutionService::new(mob.clone())
            .start(
                DelegationExecutionRequest::new_live(
                    identity.clone(),
                    "retained voice work",
                    BoundedResultSpec::new("voice", 256).expect("bound"),
                    admission.clone(),
                )
                .with_existing_member(),
            )
            .await
            .expect("existing work");
        let binding = runtime
            .live_delegation_runtime_binding(
                &session_id,
                admission.operation().domain_correlation().channel_id(),
            )
            .await
            .expect("binding");
        runtime
            .resolve_live_delegation_worker_start(
                binding.runtime_id(),
                binding.fence_token(),
                binding.generation(),
                &admission,
                true,
            )
            .await
            .expect("worker running");
        assert!(matches!(
            execution.await_terminal().await.terminal(),
            DelegationTurnTerminal::Completed(_)
        ));
        let snapshot = runtime
            .live_delegation_recovery_snapshots(&session_id)
            .await
            .expect("retained recovery snapshot")
            .remove(0);
        runtime
            .abandon_live_open_admission(&session_id, binding.channel_id())
            .await
            .expect("revoke original provider binding");

        // The replacement host deliberately uses the old/default fork policy.
        // Per-operation durable custody, not host config, controls cleanup.
        let coordinator = ExperimentalLiveDelegationCoordinator::new(Arc::clone(&runtime), mobs);
        let disposition = coordinator
            .reconcile_one_client_context_snapshot(
                &mob,
                &snapshot,
                tokio::time::Instant::now() + std::time::Duration::from_secs(2),
            )
            .await;
        assert!(
            matches!(
                disposition,
                ExperimentalClientContextRestartDisposition::Reconciled { completed: true }
            ),
            "{disposition:?}"
        );
        assert_eq!(
            mob.resolve_bridge_session_id(&identity).await,
            Some(session_id.clone())
        );
        let reconciled = runtime
            .live_delegation_recovery_snapshots(&session_id)
            .await
            .expect("reconciled snapshot")
            .remove(0);
        assert!(reconciled.late());
        assert!(!reconciled.result_eligible());
        assert!(matches!(
            coordinator
                .reconcile_one_client_context_snapshot(
                    &mob,
                    &reconciled,
                    tokio::time::Instant::now(),
                )
                .await,
            ExperimentalClientContextRestartDisposition::AlreadyReconciled
        ));
        mob.start_work_for_identity_bounded(
            identity,
            meerkat_mob::WorkSpec::new(
                "ordinary text after recovery",
                meerkat_mob::WorkOrigin::Internal,
            ),
            meerkat_core::types::HandlingMode::Queue,
            BoundedResultSpec::new("text", 256).expect("bound"),
        )
        .await
        .expect("admit text after recovery")
        .wait_bounded(BoundedResultSpec::new("text", 256).expect("bound"))
        .await
        .expect("text completes on retained source");
        mob.shutdown().await.expect("shutdown");
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RestartRecoveryInvocation {
        ForceRestore,
        CaptureInventory,
        ObservePass,
    }

    #[derive(Default)]
    struct RestartRecoveryProbe {
        invocations: std::sync::Mutex<Vec<RestartRecoveryInvocation>>,
        observed_inventories: std::sync::Mutex<Vec<ClientContextRestartInventory>>,
        capture_entered: tokio::sync::Notify,
        observed: tokio::sync::Notify,
    }

    struct ScriptedRestartRecoveryOwner {
        probe: Arc<RestartRecoveryProbe>,
        restore_results: std::sync::Mutex<std::collections::VecDeque<Result<(), String>>>,
        inventory_results: std::sync::Mutex<
            std::collections::VecDeque<Result<ClientContextRestartInventory, String>>,
        >,
        pass_results: std::sync::Mutex<
            std::collections::VecDeque<Result<Vec<ExperimentalClientContextRestartReport>, String>>,
        >,
        default_in_flight: bool,
        capture_release: Option<Arc<tokio::sync::Notify>>,
    }

    impl ScriptedRestartRecoveryOwner {
        fn new(
            restore_results: Vec<Result<(), String>>,
            inventory_results: Vec<Result<ClientContextRestartInventory, String>>,
            pass_results: Vec<Result<Vec<ExperimentalClientContextRestartReport>, String>>,
            default_in_flight: bool,
        ) -> (Arc<Self>, Arc<RestartRecoveryProbe>) {
            Self::new_with_optional_capture_gate(
                restore_results,
                inventory_results,
                pass_results,
                default_in_flight,
                None,
            )
        }

        fn new_with_optional_capture_gate(
            restore_results: Vec<Result<(), String>>,
            inventory_results: Vec<Result<ClientContextRestartInventory, String>>,
            pass_results: Vec<Result<Vec<ExperimentalClientContextRestartReport>, String>>,
            default_in_flight: bool,
            capture_release: Option<Arc<tokio::sync::Notify>>,
        ) -> (Arc<Self>, Arc<RestartRecoveryProbe>) {
            let probe = Arc::new(RestartRecoveryProbe::default());
            (
                Arc::new(Self {
                    probe: Arc::clone(&probe),
                    restore_results: std::sync::Mutex::new(restore_results.into()),
                    inventory_results: std::sync::Mutex::new(inventory_results.into()),
                    pass_results: std::sync::Mutex::new(pass_results.into()),
                    default_in_flight,
                    capture_release,
                }),
                probe,
            )
        }

        fn report_for(
            operation_id: &OperationId,
            disposition: ExperimentalClientContextRestartDisposition,
        ) -> ExperimentalClientContextRestartReport {
            ExperimentalClientContextRestartReport {
                operation_id: operation_id.clone(),
                disposition,
            }
        }

        fn inventory(
            session_id: &SessionId,
            operation_ids: &[OperationId],
        ) -> ClientContextRestartInventory {
            ClientContextRestartInventory {
                entries: operation_ids
                    .iter()
                    .cloned()
                    .map(|operation_id| ClientContextRestartInventoryEntry {
                        session_id: session_id.clone(),
                        operation_id,
                    })
                    .collect(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ClientContextRestartRecoveryOwner for ScriptedRestartRecoveryOwner {
        async fn force_mob_restore(&self) -> Result<(), String> {
            self.probe
                .invocations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(RestartRecoveryInvocation::ForceRestore);
            self.restore_results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or(Ok(()))
        }

        async fn capture_client_context_restart_inventory(
            &self,
        ) -> Result<ClientContextRestartInventory, String> {
            self.probe
                .invocations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(RestartRecoveryInvocation::CaptureInventory);
            if let Some(release) = &self.capture_release {
                let released = release.notified();
                tokio::pin!(released);
                released.as_mut().enable();
                self.probe.capture_entered.notify_waiters();
                released.await;
            }
            self.inventory_results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or_else(|| Ok(ClientContextRestartInventory::default()))
        }

        async fn observe_client_context_restart_pass(
            &self,
            inventory: &ClientContextRestartInventory,
            _observation_bound: std::time::Duration,
        ) -> Result<Vec<ExperimentalClientContextRestartReport>, String> {
            self.probe
                .invocations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(RestartRecoveryInvocation::ObservePass);
            self.probe
                .observed_inventories
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(inventory.clone());
            self.probe.observed.notify_waiters();
            self.pass_results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or_else(|| {
                    if self.default_in_flight {
                        let operation_id = inventory
                            .entries
                            .first()
                            .map(|entry| entry.operation_id.clone())
                            .unwrap_or_default();
                        Ok(vec![Self::report_for(
                            &operation_id,
                            ExperimentalClientContextRestartDisposition::InFlight,
                        )])
                    } else {
                        Ok(Vec::new())
                    }
                })
        }
    }

    fn immediate_restart_reconcile_timing() -> ClientContextRestartReconcileTiming {
        ClientContextRestartReconcileTiming {
            observation_bound: std::time::Duration::ZERO,
            in_flight_delay: std::time::Duration::ZERO,
            retry_delay: std::time::Duration::ZERO,
            retry_max_delay: std::time::Duration::ZERO,
        }
    }

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
    fn client_context_restart_reconciler_does_not_arm_without_tokio_runtime() {
        std::thread::spawn(|| {
            let armed = std::sync::atomic::AtomicBool::new(false);
            let inventory_ready = Arc::new(ClientContextRestartInventoryReady::default());
            let (owner, probe) = ScriptedRestartRecoveryOwner::new(vec![], vec![], vec![], false);

            assert!(
                try_arm_client_context_restart_reconciler(
                    &armed,
                    owner,
                    Arc::clone(&inventory_ready),
                    immediate_restart_reconcile_timing(),
                )
                .is_none(),
                "sync composition must not panic or claim an arm without a Tokio runtime"
            );
            assert!(!armed.load(std::sync::atomic::Ordering::Acquire));
            assert!(!inventory_ready.is_ready());
            assert!(
                probe
                    .invocations
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_empty()
            );
        })
        .join()
        .expect("no-runtime arm probe thread exits cleanly");
    }

    #[tokio::test]
    async fn client_context_restart_reconciler_single_arm_reobserves_until_terminal() {
        let armed = std::sync::atomic::AtomicBool::new(false);
        let inventory_ready = Arc::new(ClientContextRestartInventoryReady::default());
        let session_id = SessionId::new();
        let old_operation = OperationId::new();
        let broken_operation = OperationId::new();
        let fixed_inventory = ScriptedRestartRecoveryOwner::inventory(
            &session_id,
            &[old_operation.clone(), broken_operation.clone()],
        );
        let in_flight_inventory = ScriptedRestartRecoveryOwner::inventory(
            &session_id,
            std::slice::from_ref(&old_operation),
        );
        let (owner, probe) = ScriptedRestartRecoveryOwner::new(
            vec![Err("restore temporarily unavailable".to_string()), Ok(())],
            vec![Ok(fixed_inventory.clone())],
            vec![
                Err("scan temporarily unavailable".to_string()),
                Ok(vec![
                    ScriptedRestartRecoveryOwner::report_for(
                        &old_operation,
                        ExperimentalClientContextRestartDisposition::InFlight,
                    ),
                    ScriptedRestartRecoveryOwner::report_for(
                        &broken_operation,
                        ExperimentalClientContextRestartDisposition::Broken {
                            reason: "permanent old-operation debt".to_string(),
                        },
                    ),
                ]),
                Ok(vec![ScriptedRestartRecoveryOwner::report_for(
                    &old_operation,
                    ExperimentalClientContextRestartDisposition::Reconciled { completed: true },
                )]),
            ],
            false,
        );
        let first = try_arm_client_context_restart_reconciler(
            &armed,
            Arc::clone(&owner),
            Arc::clone(&inventory_ready),
            immediate_restart_reconcile_timing(),
        )
        .expect("first coordinator composition arms restart recovery");
        assert!(
            try_arm_client_context_restart_reconciler(
                &armed,
                Arc::clone(&owner),
                Arc::clone(&inventory_ready),
                immediate_restart_reconcile_timing(),
            )
            .is_none(),
            "the same coordinator cannot arm a duplicate restart driver"
        );
        first.await.expect("restart reconciliation task completes");
        assert!(
            try_arm_client_context_restart_reconciler(
                &armed,
                owner,
                Arc::clone(&inventory_ready),
                immediate_restart_reconcile_timing(),
            )
            .is_none(),
            "a completed startup reconciler remains one-shot for its coordinator lifetime"
        );
        assert_eq!(
            *probe
                .invocations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![
                RestartRecoveryInvocation::ForceRestore,
                RestartRecoveryInvocation::ForceRestore,
                RestartRecoveryInvocation::CaptureInventory,
                RestartRecoveryInvocation::ObservePass,
                RestartRecoveryInvocation::ObservePass,
                RestartRecoveryInvocation::ObservePass,
            ],
            "the restart driver command alphabet is restore plus read-only observation only; no work-start, resubmit, or provider-release callback exists"
        );
        assert!(inventory_ready.is_ready());
        let new_active_operation = OperationId::new();
        let observed_inventories = probe
            .observed_inventories
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            observed_inventories.as_slice(),
            &[
                fixed_inventory.clone(),
                fixed_inventory.clone(),
                in_flight_inventory,
            ]
        );
        assert!(fixed_inventory.contains(&session_id, &old_operation));
        assert!(
            !fixed_inventory.contains(&session_id, &new_active_operation),
            "an operation admitted after readiness is outside every repeated recovery pass and cannot have its active channel abandoned"
        );
    }

    #[tokio::test]
    async fn client_context_restart_inventory_capture_precedes_live_prepare_readiness() {
        let armed = std::sync::atomic::AtomicBool::new(false);
        let inventory_ready = Arc::new(ClientContextRestartInventoryReady::default());
        let capture_release = Arc::new(tokio::sync::Notify::new());
        let inventory = ScriptedRestartRecoveryOwner::inventory(
            &SessionId::new(),
            std::slice::from_ref(&OperationId::new()),
        );
        let (owner, probe) = ScriptedRestartRecoveryOwner::new_with_optional_capture_gate(
            vec![Ok(())],
            vec![Ok(inventory)],
            vec![Ok(Vec::new())],
            false,
            Some(Arc::clone(&capture_release)),
        );
        let capture_entered = probe.capture_entered.notified();
        tokio::pin!(capture_entered);
        capture_entered.as_mut().enable();
        let task = try_arm_client_context_restart_reconciler(
            &armed,
            Arc::clone(&owner),
            Arc::clone(&inventory_ready),
            immediate_restart_reconcile_timing(),
        )
        .expect("arm inventory-readiness ordering probe");
        capture_entered.await;
        assert!(!inventory_ready.is_ready());

        let prepare_wait = tokio::spawn({
            let inventory_ready = Arc::clone(&inventory_ready);
            async move { inventory_ready.wait().await }
        });
        tokio::task::yield_now().await;
        assert!(
            !prepare_wait.is_finished(),
            "bound-channel preparation remains gated while startup inventory capture is pending"
        );

        capture_release.notify_one();
        prepare_wait
            .await
            .expect("prepare readiness releases after inventory capture");
        task.await
            .expect("inventory-ordering restart reconciler completes");
        assert!(inventory_ready.is_ready());
        assert_eq!(
            *probe
                .invocations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![
                RestartRecoveryInvocation::ForceRestore,
                RestartRecoveryInvocation::CaptureInventory,
                RestartRecoveryInvocation::ObservePass,
            ]
        );
    }

    #[tokio::test]
    async fn client_context_restart_broken_debt_is_terminal_for_startup_driver() {
        let armed = std::sync::atomic::AtomicBool::new(false);
        let inventory_ready = Arc::new(ClientContextRestartInventoryReady::default());
        let operation_id = OperationId::new();
        let inventory = ScriptedRestartRecoveryOwner::inventory(
            &SessionId::new(),
            std::slice::from_ref(&operation_id),
        );
        let (owner, probe) = ScriptedRestartRecoveryOwner::new(
            vec![Ok(())],
            vec![Ok(inventory)],
            vec![Ok(vec![ScriptedRestartRecoveryOwner::report_for(
                &operation_id,
                ExperimentalClientContextRestartDisposition::Broken {
                    reason: "unsanitized mechanical detail stays out of the warning".to_string(),
                },
            )])],
            true,
        );
        let task = try_arm_client_context_restart_reconciler(
            &armed,
            Arc::clone(&owner),
            inventory_ready,
            immediate_restart_reconcile_timing(),
        )
        .expect("arm permanent-debt restart probe");
        task.await
            .expect("permanent broken debt ends the startup driver");
        assert_eq!(
            *probe
                .invocations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![
                RestartRecoveryInvocation::ForceRestore,
                RestartRecoveryInvocation::CaptureInventory,
                RestartRecoveryInvocation::ObservePass,
            ],
            "permanent Broken debt is observable but is not retried as a transient scan failure"
        );
    }

    #[tokio::test]
    async fn client_context_restart_reconciler_drops_with_coordinator_owner() {
        let armed = std::sync::atomic::AtomicBool::new(false);
        let inventory_ready = Arc::new(ClientContextRestartInventoryReady::default());
        let inventory = ScriptedRestartRecoveryOwner::inventory(
            &SessionId::new(),
            std::slice::from_ref(&OperationId::new()),
        );
        let (owner, probe) =
            ScriptedRestartRecoveryOwner::new(vec![Ok(())], vec![Ok(inventory)], vec![], true);
        let observed = probe.observed.notified();
        tokio::pin!(observed);
        observed.as_mut().enable();
        let task = try_arm_client_context_restart_reconciler(
            &armed,
            Arc::clone(&owner),
            inventory_ready,
            ClientContextRestartReconcileTiming {
                observation_bound: std::time::Duration::ZERO,
                in_flight_delay: std::time::Duration::from_millis(1),
                retry_delay: std::time::Duration::ZERO,
                retry_max_delay: std::time::Duration::ZERO,
            },
        )
        .expect("arm drop-sensitive restart reconciliation");
        observed.await;
        drop(owner);
        tokio::time::timeout(std::time::Duration::from_millis(100), task)
            .await
            .expect("weakly-owned restart driver exits after coordinator drop")
            .expect("restart driver exits cleanly");
        assert_eq!(Arc::strong_count(&probe), 1);
    }

    #[test]
    fn barge_in_user_start_preserves_frozen_assistant_interaction() {
        use meerkat_runtime::meerkat_machine::dsl::{
            AgentRuntimeId, FenceToken, Generation, MeerkatMachineAuthority, MeerkatMachineInput,
            MeerkatMachineMutator, MeerkatMachineState,
        };

        let channel_id = "live:barge-in".to_string();
        let runtime_id = AgentRuntimeId("runtime:barge-in".to_string());
        let fence_token = FenceToken(41);
        let generation = Generation(7);
        let first_interaction = InteractionId::new().to_string();
        let second_interaction = InteractionId::new().to_string();
        let first_user_turn = "provider:user:1".to_string();
        let first_assistant_turn = "provider:assistant:1".to_string();
        let second_user_turn = "provider:user:2".to_string();
        let mut state = MeerkatMachineState {
            lifecycle_phase: meerkat_runtime::meerkat_machine::dsl::MeerkatPhase::Idle,
            ..Default::default()
        };
        state
            .live_execution_runtime_id_by_channel
            .insert(channel_id.clone(), runtime_id.clone());
        state
            .live_execution_fence_by_channel
            .insert(channel_id.clone(), fence_token);
        state
            .live_execution_generation_by_channel
            .insert(channel_id.clone(), generation);
        state.live_channel_session_by_channel.insert(
            channel_id.clone(),
            "session:barge-in-authority-fixture".to_string(),
        );
        let mut authority = MeerkatMachineAuthority::recover_from_state(state)
            .expect("recover exact generated barge-in fixture");

        for input in [
            MeerkatMachineInput::ObserveLiveProviderTurnStarted {
                channel_id: channel_id.clone(),
                runtime_id: runtime_id.clone(),
                fence_token,
                generation,
                interaction_id: first_interaction.clone(),
                provider_turn_ref: first_user_turn.clone(),
            },
            MeerkatMachineInput::CompleteLiveInteraction {
                channel_id: channel_id.clone(),
                runtime_id: runtime_id.clone(),
                fence_token,
                generation,
                provider_turn_ref: first_user_turn,
            },
            MeerkatMachineInput::ObserveLiveAssistantTurnStarted {
                channel_id: channel_id.clone(),
                runtime_id: runtime_id.clone(),
                fence_token,
                generation,
                assistant_turn_ref: first_assistant_turn.clone(),
                candidate_interaction_id: "unused-assistant-candidate".to_string(),
            },
            // This is the full-duplex ordering under review: the next user
            // turn starts before assistant turn 1 reaches playback terminal.
            MeerkatMachineInput::ObserveLiveProviderTurnStarted {
                channel_id: channel_id.clone(),
                runtime_id,
                fence_token,
                generation,
                interaction_id: second_interaction.clone(),
                provider_turn_ref: second_user_turn.clone(),
            },
        ] {
            MeerkatMachineMutator::apply(&mut authority, input)
                .expect("generated custody accepts the exact barge-in ordering");
        }

        let state = authority.state();
        assert_eq!(
            state
                .live_assistant_interaction_by_turn
                .get(&first_assistant_turn),
            Some(&first_interaction),
            "barge-in cannot retarget the interrupted assistant output"
        );
        assert_eq!(
            state.live_active_interaction_by_channel.get(&channel_id),
            Some(&second_interaction),
            "the new user turn owns foreground input custody"
        );
        assert_eq!(
            state.live_provider_turn_by_channel.get(&channel_id),
            Some(&second_user_turn),
            "user-input custody advances independently of assistant output"
        );
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
            delegation_ref: String,
            result_digest: String,
        }

        let authority = TestDeliveryAuthority("delivery:exact-bounded-result");
        let retained_delegation = LiveSidebandDelegationRef::__from_provider_observation(
            "adapter:client-context".to_string(),
            "delegation:exact-retained-ref".to_string(),
        )
        .expect("provider delegation fixture");
        let bounded_executor_result = retain_terminal_result(
            true,
            LiveDelegationWorkerTerminalKind::Completed,
            false,
            Some("line one from executor\nline two remains byte-exact  ".to_string()),
        )
        .expect("retired exact bounded completion is projection-eligible");

        let (acknowledgement, projection_evidence) = ExactDelegationResultProjection::new(
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
                    delegation_ref: received_delegation.adapter_key().to_string(),
                    result_digest: format!(
                        "sha256:{:x}",
                        Sha256::digest(received_result.as_bytes())
                    ),
                }
            },
        )
        .await;

        assert_eq!(
            acknowledgement.authority,
            TestDeliveryAuthority("delivery:exact-bounded-result"),
            "the acknowledgement resolves the same delivery authority consumed by the projection"
        );
        assert_eq!(
            acknowledgement.delegation_ref, "adapter:client-context",
            "the acknowledgement is correlated to the exact retained delegation ref"
        );
        assert_eq!(
            acknowledgement.result_digest,
            format!(
                "sha256:{:x}",
                Sha256::digest(b"line one from executor\nline two remains byte-exact  ")
            ),
            "the acknowledged projection carries a safe digest of the exact bounded final text"
        );
        assert_eq!(
            projection_evidence.result_digest, acknowledgement.result_digest,
            "the provider dispatch evidence is derived from the same exact bounded final"
        );
        assert_eq!(
            projection_evidence.delegation_ref_digest,
            format!("sha256:{:x}", Sha256::digest(b"adapter:client-context")),
            "the provider dispatch evidence is derived from the same retained delegation ref"
        );
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    struct ExactProjectionTestAgent {
        session: meerkat_core::Session,
        identity: meerkat_core::SessionLlmIdentity,
        transient: meerkat_core::TransientTurnContextStateHandle,
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[async_trait::async_trait]
    impl meerkat_session::SessionAgent for ExactProjectionTestAgent {
        async fn run_with_events(
            &mut self,
            _prompt: meerkat_core::ContentInput,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>,
        ) -> Result<meerkat_core::RunResult, meerkat_core::AgentError> {
            Ok(meerkat_core::RunResult {
                text: "unused".to_string(),
                session_id: self.session.id().clone(),
                usage: meerkat_core::Usage::default(),
                turns: 0,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_turn_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::AgentError> {
            self.identity = identity;
            Ok(())
        }

        fn cancel(&mut self) {}

        fn session_id(&self) -> SessionId {
            self.session.id().clone()
        }

        fn snapshot(&self) -> meerkat_session::ephemeral::SessionSnapshot {
            meerkat_session::ephemeral::SessionSnapshot {
                created_at: std::time::SystemTime::now(),
                updated_at: std::time::SystemTime::now(),
                message_count: self.session.messages().len(),
                total_tokens: 0,
                usage: meerkat_core::Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::AgentError> {
            Ok(self.session.clone())
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<
            meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot,
            meerkat_core::AgentError,
        > {
            meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot::from_session(
                &self.session,
            )
        }

        fn observed_session_tail(
            &self,
        ) -> meerkat_core::pending_continuation::ObservedSessionTailKind {
            meerkat_core::pending_continuation::observe_session_tail(self.session.messages())
        }

        fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
            Some(self.identity.clone())
        }

        fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
            self.transient.clone()
        }

        fn append_realtime_transcript_event(
            &mut self,
            event: meerkat_core::RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::RealtimeTranscriptApplyOutcome, meerkat_core::AgentError>
        {
            Ok(self.session.append_realtime_transcript_event(event))
        }
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    struct ExactProjectionTestAgentBuilder(SessionId);

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[async_trait::async_trait]
    impl meerkat_session::SessionAgentBuilder for ExactProjectionTestAgentBuilder {
        type Agent = ExactProjectionTestAgent;

        async fn build_agent(
            &self,
            req: &meerkat_core::service::CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>,
        ) -> Result<Self::Agent, meerkat_core::service::SessionError> {
            Ok(ExactProjectionTestAgent {
                session: meerkat_core::Session::with_id(self.0.clone()),
                identity: meerkat_core::SessionLlmIdentity {
                    model: req.model.clone(),
                    provider: meerkat_core::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
                transient: meerkat_core::TransientTurnContextStateHandle::new(),
            })
        }
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    struct ExactProjectionControlCapture {
        authority: LiveDelegationResultDeliveryAuthority,
        delegation_matches_authority: bool,
        delegation_ref_digest: String,
        result_digest: String,
        result_text: String,
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[derive(Default)]
    struct ExactProjectionControl {
        capture: Mutex<Option<ExactProjectionControlCapture>>,
        narrations: Mutex<Vec<(LiveDelegationNarrationKind, String)>>,
        /// Every released result as (delegation adapter key, text), in order.
        releases: Mutex<Vec<(String, String)>>,
        /// Narrations and result releases in the order the provider saw them.
        events: Mutex<Vec<ExactProjectionControlEvent>>,
        /// The provider transport has been retired: every release and
        /// narration reports `ActiveBindingUnavailable`, as the real control
        /// plane does between the physical close and the machine's close.
        binding_unavailable: std::sync::atomic::AtomicBool,
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ExactProjectionControlEvent {
        Narration(LiveDelegationNarrationKind, String),
        Release(String),
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    struct ExactProjectionSideband;

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[async_trait::async_trait]
    impl meerkat_live::ProviderWebrtcSidebandSession for ExactProjectionSideband {
        async fn send_command(
            &self,
            _command: meerkat_live::LiveSidebandCommand,
        ) -> Result<
            meerkat_live::LiveSidebandCommandDelivery,
            meerkat_live::ProviderWebrtcBrokerError,
        > {
            Err(meerkat_live::ProviderWebrtcBrokerError::Unavailable)
        }

        async fn next_observation(
            &self,
        ) -> Result<Option<LiveSidebandObservation>, meerkat_live::ProviderWebrtcBrokerError>
        {
            Ok(None)
        }

        async fn close(&self) -> Result<(), meerkat_live::ProviderWebrtcBrokerError> {
            Ok(())
        }
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    struct ExactProjectionBoundReadyResolver;

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[async_trait::async_trait]
    impl meerkat_live::ProviderWebrtcPendingBoundReadyResolver for ExactProjectionBoundReadyResolver {
        async fn resolve(self: Box<Self>) -> Result<u64, meerkat_live::ProviderWebrtcBrokerError> {
            Ok(0)
        }
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[async_trait::async_trait]
    impl ExperimentalGptLiveControlPlane for ExactProjectionControl {
        async fn active_binding(&self, _session_id: &SessionId) -> Option<ProviderWebrtcBinding> {
            None
        }

        async fn next_observation(
            &self,
            _binding: &ProviderWebrtcBinding,
        ) -> Result<
            Option<ExperimentalGptLiveControlObservation>,
            meerkat_live::ProviderWebrtcBrokerError,
        > {
            Ok(None)
        }

        async fn append_session_context(
            &self,
            _authority: meerkat_runtime::live_execution::LiveContextAppendAuthority,
            _text: String,
        ) -> Result<
            meerkat::experimental_gpt_live::ExperimentalGptLiveAppendDispatch,
            ExperimentalGptLiveBridgeError,
        > {
            Err(ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)
        }

        async fn release_delegation_context(
            &self,
            authority: LiveDelegationResultDeliveryAuthority,
            delegation: LiveSidebandDelegationRef,
            text: String,
        ) -> Result<ExperimentalGptLiveResultDeliveryDispatch, ExperimentalGptLiveBridgeError>
        {
            if self
                .binding_unavailable
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(ExperimentalGptLiveBridgeError::ActiveBindingUnavailable);
            }
            self.releases
                .lock()
                .await
                .push((delegation.adapter_key().to_string(), text.clone()));
            self.events
                .lock()
                .await
                .push(ExactProjectionControlEvent::Release(
                    delegation.adapter_key().to_string(),
                ));
            *self.capture.lock().await = Some(ExactProjectionControlCapture {
                authority: authority.clone(),
                delegation_matches_authority: delegation.adapter_key()
                    == authority
                        .operation()
                        .domain_correlation()
                        .provider()
                        .delegation_item_id(),
                delegation_ref_digest: format!(
                    "sha256:{:x}",
                    Sha256::digest(delegation.adapter_key().as_bytes())
                ),
                result_digest: format!("sha256:{:x}", Sha256::digest(text.as_bytes())),
                result_text: text,
            });
            Ok(ExperimentalGptLiveResultDeliveryDispatch::Resolved(
                meerkat::experimental_gpt_live::ExperimentalGptLiveResultDeliveryResolution::__gate0_harness(
                    authority,
                    LiveDelegationResultDeliveryObservation::Delivered,
                ),
            ))
        }

        async fn narrate_delegation(
            &self,
            authority: meerkat_runtime::live_execution::LiveDelegationNarrationAuthority,
            _delegation: LiveSidebandDelegationRef,
            text: String,
        ) -> Result<ExperimentalGptLiveNarrationDispatch, ExperimentalGptLiveBridgeError> {
            if self
                .binding_unavailable
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(ExperimentalGptLiveBridgeError::ActiveBindingUnavailable);
            }
            self.narrations
                .lock()
                .await
                .push((authority.kind(), text.clone()));
            self.events
                .lock()
                .await
                .push(ExactProjectionControlEvent::Narration(
                    authority.kind(),
                    text,
                ));
            Ok(ExperimentalGptLiveNarrationDispatch::Resolved(
                meerkat_core::LiveAppendDeliveryOutcome::Acknowledged,
            ))
        }
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[tokio::test]
    async fn retained_terminal_result_runs_control_ack_and_runtime_resolution_chain()
    -> Result<(), Box<dyn std::error::Error>> {
        for ownership in [
            meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::OwnedMember,
            meerkat_runtime::live_execution::LiveDelegationWorkerOwnership::ExistingMember,
        ] {
            assert_retained_terminal_result_chain(ownership).await?;
        }
        Ok(())
    }

    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    async fn assert_retained_terminal_result_chain(
        ownership: meerkat_runtime::live_execution::LiveDelegationWorkerOwnership,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use meerkat_core::service::{
            CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionService,
        };

        let session_id = SessionId::new();
        let channel_id = meerkat_core::LiveChannelId::new("live:exact-result-projection");

        let session_service = Arc::new(meerkat_session::EphemeralSessionService::new(
            ExactProjectionTestAgentBuilder(session_id.clone()),
            1,
        ));
        SessionService::create_session(
            session_service.as_ref(),
            CreateSessionRequest {
                injected_context: Vec::new(),
                model: "exact-projection-test".to_string(),
                prompt: "unused".to_string().into(),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            },
        )
        .await
        .expect("materialize canonical transcript owner");

        let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let _bindings = runtime
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare exact runtime binding");
        let identity = meerkat_core::SessionLlmIdentity {
            model: "experimental-live".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        runtime
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("admit exact live channel");
        let execution_profile =
            meerkat_runtime::live_execution::LiveExecutionProfileSelection::__test_new(
                "test-client-context",
                meerkat_core::LiveExecutionMode::ClientContext,
                meerkat_core::LiveExecutionCapabilities {
                    function_bridge: false,
                    client_context: true,
                },
            )
            .expect("construct exact test execution profile");
        runtime
            .resolve_live_execution_profile_admission(&session_id, &channel_id, &execution_profile)
            .await
            .expect("admit exact client-context mode");
        let stage = runtime
            .stage_experimental_live_execution(&session_id, &channel_id, 0)
            .await
            .expect("stage exact experimental live execution");
        runtime
            .register_live_playback_owner(&stage, "test-exact-result-playback-owner")
            .await
            .expect("register exact playback owner");
        runtime
            .record_live_webrtc_token_issued(
                &session_id,
                &channel_id,
                "test-exact-result-token",
                100,
                1_000,
            )
            .await
            .expect("record exact signaling token");
        let mut answer_admission = runtime
            .resolve_live_webrtc_answer_admission(
                &session_id,
                &channel_id,
                "test-exact-result-token",
                101,
            )
            .await
            .expect("resolve exact signaling admission");
        assert!(answer_admission.admitted);
        let admitted_offer = meerkat_live::LiveWebrtcAdmittedOffer::from_machine_admission(
            channel_id.clone(),
            session_id.clone(),
            Some(meerkat_live::LiveWebrtcRuntimeBinding {
                generation: stage.binding().generation(),
                fence: stage.binding().fence_token(),
            }),
            "test-offer-sdp".to_string(),
            answer_admission
                .transport_seal
                .take()
                .expect("admitted answer carries one-use provider seal"),
        );
        let provider_offer = admitted_offer
            .into_provider_offer()
            .expect("consume exact signaling admission");
        let provider_binding = provider_offer.binding().clone();
        let answer = provider_offer.into_pending_bound_ready_answer(
            "test-answer-sdp".to_string(),
            Arc::new(ExactProjectionSideband),
            Box::new(ExactProjectionBoundReadyResolver),
        );
        let (_, _, pending_bound_ready) = answer.into_parts();
        let bound_ready = pending_bound_ready
            .__resolve_after_answer_delivery()
            .await
            .expect("provider acknowledges exact empty canonical seed");
        runtime
            .accept_live_webrtc_answer_and_bind_execution(&provider_binding, &bound_ready, 1)
            .await
            .expect("bind exact live execution after provider readiness");

        let turn = LiveSidebandTurnRef::__from_provider_observation(
            &channel_id,
            "turn:exact-result-projection".to_string(),
            "provider-private-turn-ref".to_string(),
        )
        .expect("provider turn fixture");
        let turn_started = runtime
            .observe_live_provider_turn_started(&LiveSidebandObservation::new(
                provider_binding.clone(),
                LiveSidebandObservationKind::TurnStarted {
                    turn,
                    role: meerkat_live::LiveSidebandTurnRole::User,
                },
            ))
            .await
            .expect("admit exact provider turn");
        let provider_turn_ref = turn_started.provider_turn_ref().to_string();
        let provider = OpaqueProviderCorrelation::new(
            "delegation:exact-result-projection",
            provider_turn_ref.clone(),
        )
        .expect("provider correlation fixture");
        let correlation = LiveUserTurnCorrelation::new(
            channel_id.clone(),
            turn_started.interaction_id(),
            provider,
        )
        .expect("live turn correlation fixture");
        let operation = ExactOperationIdentity::for_domain(OperationId::new(), correlation.clone());
        let provisional = ProvisionalLiveHandoff::new(
            correlation,
            "exact final delegated user input",
            LiveHandoffInputProvenance::NormalizedHandoff,
        )
        .expect("provisional handoff fixture");
        runtime
            .admit_live_delegation(turn_started.binding(), &operation, &provisional)
            .await
            .expect("admit exact live delegation");
        let runtime_id = turn_started.binding().runtime_id().clone();
        let fence_token = turn_started.binding().fence_token();
        let generation = turn_started.binding().generation();

        let final_transcript = session_service
            .commit_live_user_transcript_final(
                &session_id,
                provisional.clone(),
                Some(meerkat_core::RealtimeTranscriptEvent::UserTranscriptFinal {
                    item_id: provider_turn_ref,
                    previous_item_id: None,
                    content_index: 0,
                    text: "exact final delegated user input".to_string(),
                }),
            )
            .await
            .expect("commit exact final transcript evidence");
        let reconciliation = runtime
            .reconcile_live_delegation_transcript(
                &session_id,
                &runtime_id,
                fence_token,
                generation,
                &operation,
                &provisional,
                &final_transcript,
            )
            .await
            .expect("reconcile exact final transcript");
        let admission = runtime
            .authorize_live_delegation_worker_start_with_ownership(
                &session_id,
                &runtime_id,
                fence_token,
                generation,
                &operation,
                &provisional,
                "exact-result-projection-worker",
                ownership,
            )
            .await
            .expect("authorize exact bounded worker");
        runtime
            .resolve_live_delegation_worker_start(
                &runtime_id,
                fence_token,
                generation,
                &admission,
                true,
            )
            .await
            .expect("record exact bounded worker start");
        let terminal_receipt = runtime
            .record_live_delegation_worker_terminal(
                &runtime_id,
                fence_token,
                generation,
                &admission,
                LiveDelegationWorkerTerminalKind::Completed,
            )
            .await
            .expect("record exact completed bounded worker");
        let retirement = runtime
            .authorize_live_delegation_worker_retirement(
                &runtime_id,
                fence_token,
                generation,
                &admission,
            )
            .await
            .expect("authorize exact bounded worker retirement");
        runtime
            .resolve_live_delegation_worker_retirement(
                &runtime_id,
                fence_token,
                generation,
                &retirement,
                true,
            )
            .await
            .expect("record exact bounded worker retirement");
        let exact_result = retain_terminal_result(
            true,
            LiveDelegationWorkerTerminalKind::Completed,
            terminal_receipt.late(),
            Some("line one from executor\nline two remains byte-exact  ".to_string()),
        )
        .expect("retired exact bounded completion is projection-eligible");

        let control = Arc::new(ExactProjectionControl::default());
        let binding = turn_started.binding().clone();
        let retained = Arc::new(RetainedDelegation {
            operation: operation.clone(),
            provisional,
            runtime_binding: binding,
            admission,
            delegation: LiveSidebandDelegationRef::__from_provider_observation(
                "delegation:exact-result-projection".to_string(),
                "provider-private-delegation-ref".to_string(),
            )
            .expect("provider delegation fixture"),
            control: Arc::clone(&control) as Arc<dyn ExperimentalGptLiveControlPlane>,
            result: Mutex::new(RetainedDelegationResult {
                reconciliation: Some(reconciliation),
                result_text: Some(exact_result.clone()),
                ..RetainedDelegationResult::default()
            }),
            work: None,
            workgraph: None,
            title: "exact result projection".to_string(),
            append_lane: Arc::new(Mutex::new(())),
            mob_handle: None,
            source_identity: AgentIdentity::from("exact-result-source"),
        });
        let coordinator = ExperimentalLiveDelegationCoordinator::new(
            Arc::clone(&runtime),
            crate::MobMcpState::new_in_memory_with_archive_delay(0),
        );
        coordinator
            .retained
            .lock()
            .await
            .insert(operation.operation_id().clone(), Arc::clone(&retained));

        coordinator
            .try_release_retained_result(&retained)
            .await
            .expect("production retained-result chain settles provider acknowledgement");

        let capture = control
            .capture
            .lock()
            .await
            .take()
            .expect("control plane received exact production projection");
        assert_eq!(capture.result_text, exact_result);
        assert!(capture.authority.authorizes_text(&capture.result_text));
        assert_eq!(capture.authority.operation(), &operation);
        assert!(capture.delegation_matches_authority);
        assert_eq!(
            capture.result_digest,
            format!("sha256:{:x}", Sha256::digest(exact_result.as_bytes()))
        );
        assert_eq!(
            capture.delegation_ref_digest,
            format!(
                "sha256:{:x}",
                Sha256::digest(b"delegation:exact-result-projection")
            )
        );
        let settled = runtime
            .resolve_live_delegation_result_delivery(
                &capture.authority,
                LiveDelegationResultDeliveryObservation::Delivered,
            )
            .await
            .expect("recover terminal provider-delivery resolution with the same authority");
        let LiveDelegationResultDeliveryResolution::Resolved(settled) = settled else {
            return Err("delivered terminal authority cannot become ambiguity recovery".into());
        };
        assert_eq!(
            settled.observation(),
            LiveDelegationResultDeliveryObservation::Delivered
        );
        assert_eq!(settled.authority().operation(), &operation);
        assert!(
            !coordinator
                .retained
                .lock()
                .await
                .contains_key(operation.operation_id()),
            "terminal provider acknowledgement retires exact retained custody"
        );
        Ok(())
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

#[cfg(test)]
mod start_failure_tests {
    use super::*;

    #[test]
    fn source_busy_start_failure_is_typed_not_stringified() {
        let error = DelegationExecutionError::SourceBusy {
            source_identity: AgentIdentity::from("voice-executor"),
            waited_ms: 20_000,
        };
        let failure = LiveDelegationStartFailure::from(&error);
        assert_eq!(
            failure,
            LiveDelegationStartFailure::SourceBusy {
                source_identity: AgentIdentity::from("voice-executor"),
                waited_ms: 20_000,
            }
        );
        assert_eq!(failure.kind(), "source_busy");
        assert!(
            failure
                .to_string()
                .contains("still mid-turn after 20000 ms")
        );
    }
}
