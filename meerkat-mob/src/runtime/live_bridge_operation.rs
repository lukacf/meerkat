//! Provider-neutral execution seam for a live bridge operation.
//!
//! The generated machine admits the operation and seals its durable member,
//! canonical session, exact context revision, and operation phase before this
//! service is called. The service starts actor-owned, noncommitting execution
//! on that already-materialized member. It never creates, retires, or cancels
//! a Mob member and it has no parent-message publication surface.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::Session;
use meerkat_core::ops::OperationId;
use meerkat_runtime::live_execution::{LiveBridgeOperationAdmission, MeerkatExecutionTerminal};

/// Default maximum UTF-8 payload returned by one bridge execution.
pub const DEFAULT_LIVE_BRIDGE_OUTPUT_BYTES: usize = 16 * 1024;

/// Exact generation-bound Session clone retained before machine admission.
///
/// Only the Mob session-service owner can mint this value. The coordinator
/// derives machine admission from its revision and later hands this identical
/// clone to the member's serialized session actor. No post-admission session re-read is
/// permitted.
#[derive(Clone)]
pub struct LiveBridgeExecutionSnapshot {
    session: Arc<Session>,
    agent_identity: Arc<str>,
    canonical_context_revision: meerkat_core::CanonicalContextRevision,
}

impl std::fmt::Debug for LiveBridgeExecutionSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveBridgeExecutionSnapshot")
            .field("session_id", &"[REDACTED]")
            .field("agent_identity", &"[REDACTED]")
            .field("canonical_context_revision", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl LiveBridgeExecutionSnapshot {
    pub(crate) fn from_generation_bound_session(
        session: Session,
        agent_identity: impl Into<String>,
    ) -> Result<Self, LiveBridgeOperationStartError> {
        let agent_identity = agent_identity.into();
        if agent_identity.trim().is_empty() {
            return Err(LiveBridgeOperationStartError::InvalidRequest);
        }
        let member = session
            .session_metadata()
            .and_then(|metadata| metadata.mob_member_binding)
            .map(|binding| binding.member)
            .ok_or(LiveBridgeOperationStartError::Rejected)?;
        if member != agent_identity {
            return Err(LiveBridgeOperationStartError::Rejected);
        }
        let canonical_context_revision = session
            .canonical_context_revision()
            .map_err(|_| LiveBridgeOperationStartError::Failed)?;
        Ok(Self {
            session: Arc::new(session),
            agent_identity: Arc::from(agent_identity),
            canonical_context_revision,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __test_new(
        session: Session,
        agent_identity: impl Into<String>,
    ) -> Result<Self, LiveBridgeOperationStartError> {
        Self::from_generation_bound_session(session, agent_identity)
    }

    #[must_use]
    pub fn session(&self) -> &Session {
        self.session.as_ref()
    }

    #[must_use]
    pub fn session_arc(&self) -> Arc<Session> {
        Arc::clone(&self.session)
    }

    #[must_use]
    pub fn agent_identity(&self) -> &str {
        &self.agent_identity
    }

    #[must_use]
    pub fn canonical_context_revision(&self) -> &meerkat_core::CanonicalContextRevision {
        &self.canonical_context_revision
    }
}

/// One machine-admitted request to the already-bound durable member.
///
/// The provider supplies only `semantic_request`. Identity, session, context
/// revision, phase, and all provider correlation remain sealed in
/// `admission`.
#[derive(Clone)]
pub struct LiveBridgeOperationRequest {
    admission: Arc<LiveBridgeOperationAdmission>,
    snapshot: LiveBridgeExecutionSnapshot,
    semantic_request: Arc<str>,
    max_output_bytes: usize,
    noncommitting_run_permit: Option<meerkat_core::LiveBridgeNoncommittingRunPermit>,
    tool_dispatch_admission: Option<meerkat_core::LiveBridgeToolDispatchAdmission>,
}

impl std::fmt::Debug for LiveBridgeOperationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveBridgeOperationRequest")
            .field("operation_id", &"[REDACTED]")
            .field("session_id", &"[REDACTED]")
            .field("agent_identity", &"[REDACTED]")
            .field("canonical_context_revision", &"[REDACTED]")
            .field("phase", &self.admission.phase())
            .field("snapshot", &self.snapshot)
            .field("semantic_request", &"[REDACTED]")
            .field("max_output_bytes", &self.max_output_bytes)
            .field(
                "execution_authorities",
                &self.noncommitting_run_permit.is_some(),
            )
            .finish()
    }
}

impl LiveBridgeOperationRequest {
    pub fn new(
        admission: Arc<LiveBridgeOperationAdmission>,
        snapshot: LiveBridgeExecutionSnapshot,
        semantic_request: impl Into<String>,
    ) -> Result<Self, LiveBridgeOperationStartError> {
        Self::with_max_output_bytes(
            admission,
            snapshot,
            semantic_request,
            DEFAULT_LIVE_BRIDGE_OUTPUT_BYTES,
        )
    }

    pub fn with_max_output_bytes(
        admission: Arc<LiveBridgeOperationAdmission>,
        snapshot: LiveBridgeExecutionSnapshot,
        semantic_request: impl Into<String>,
        max_output_bytes: usize,
    ) -> Result<Self, LiveBridgeOperationStartError> {
        let semantic_request = semantic_request.into();
        if semantic_request.trim().is_empty()
            || max_output_bytes == 0
            || snapshot.session().id() != admission.session_id()
            || snapshot.agent_identity() != admission.agent_identity()
            || snapshot.canonical_context_revision() != admission.canonical_context_revision()
        {
            return Err(LiveBridgeOperationStartError::InvalidRequest);
        }
        Ok(Self {
            admission,
            snapshot,
            semantic_request: Arc::from(semantic_request),
            max_output_bytes,
            noncommitting_run_permit: None,
            tool_dispatch_admission: None,
        })
    }

    /// Attach the already-consumed model-computation receipt and the
    /// per-dispatch generated tool gate for this exact operation.
    pub fn with_execution_authorities(
        mut self,
        model_computation_authority: Arc<
            meerkat_runtime::live_execution::LiveBridgeEffectDispatchAuthority,
        >,
        tool_dispatch_admission: meerkat_core::LiveBridgeToolDispatchAdmission,
    ) -> Result<Self, LiveBridgeOperationStartError> {
        let effect = model_computation_authority.effect();
        if effect.kind() != meerkat_core::LiveBridgeEffectKind::ModelComputation
            || effect.admission().operation() != self.admission.operation()
            || tool_dispatch_admission.operation_id()
                != self.admission.operation().operation_id().to_string()
        {
            return Err(LiveBridgeOperationStartError::InvalidRequest);
        }
        self.noncommitting_run_permit = Some(
            model_computation_authority
                .sealed_noncommitting_run_permit()
                .map_err(|_| LiveBridgeOperationStartError::InvalidRequest)?,
        );
        self.tool_dispatch_admission = Some(tool_dispatch_admission);
        Ok(self)
    }

    #[must_use]
    pub fn has_execution_authorities(&self) -> bool {
        self.noncommitting_run_permit.is_some() && self.tool_dispatch_admission.is_some()
    }

    pub(crate) fn session_operation_request(
        &self,
    ) -> Result<meerkat_session::LiveBridgeSessionOperationRequest, LiveBridgeOperationStartError>
    {
        let dispatch_admission = self
            .tool_dispatch_admission
            .clone()
            .ok_or(LiveBridgeOperationStartError::Rejected)?;
        let run_permit = self
            .noncommitting_run_permit
            .clone()
            .ok_or(LiveBridgeOperationStartError::Rejected)?;
        Ok(meerkat_session::LiveBridgeSessionOperationRequest {
            operation_id: Arc::from(self.admission.operation().operation_id().to_string()),
            snapshot: self.snapshot.session().clone(),
            semantic_request: self.semantic_request().to_string().into(),
            dispatch_admission,
            run_permit,
        })
    }

    #[must_use]
    pub fn admission(&self) -> &LiveBridgeOperationAdmission {
        self.admission.as_ref()
    }

    #[must_use]
    pub fn semantic_request(&self) -> &str {
        &self.semantic_request
    }

    #[must_use]
    pub fn snapshot(&self) -> &LiveBridgeExecutionSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub const fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }
}

/// Read-only cancellation signal delivered to the exact accepted execution.
#[derive(Clone)]
pub struct LiveBridgeOperationCancellationSignal {
    operation_id: OperationId,
    receiver: tokio::sync::watch::Receiver<bool>,
}

impl LiveBridgeOperationCancellationSignal {
    pub async fn cancelled(&self) {
        let mut receiver = self.receiver.clone();
        loop {
            if *receiver.borrow() {
                return;
            }
            if receiver.changed().await.is_err() {
                // Sender loss is not cancellation authority. An accepted
                // operation continues under actor custody even if its caller
                // drops the observation handle without cancelling it.
                std::future::pending::<()>().await;
            }
        }
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.receiver.borrow()
    }

    #[must_use]
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    pub(crate) fn receiver(&self) -> tokio::sync::watch::Receiver<bool> {
        self.receiver.clone()
    }
}

/// Coordinator-owned cancellation handle. Cancelling this handle cannot
/// retire or otherwise mutate the durable member.
#[derive(Clone)]
pub struct LiveBridgeOperationCancellationHandle {
    operation_id: OperationId,
    sender: tokio::sync::watch::Sender<bool>,
}

impl LiveBridgeOperationCancellationHandle {
    fn pair(operation_id: OperationId) -> (Self, LiveBridgeOperationCancellationSignal) {
        let (sender, receiver) = tokio::sync::watch::channel(false);
        (
            Self {
                operation_id: operation_id.clone(),
                sender,
            },
            LiveBridgeOperationCancellationSignal {
                operation_id,
                receiver,
            },
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[must_use]
    pub fn __test_new(operation_id: OperationId) -> Self {
        Self::pair(operation_id).0
    }

    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    #[must_use]
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }
}

/// Independent Meerkat execution terminal. Provider submission has its own
/// generated lifecycle and cannot rewrite this terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveBridgeOperationTerminal {
    terminal: MeerkatExecutionTerminal,
    output: Option<String>,
}

impl LiveBridgeOperationTerminal {
    pub(crate) const fn failed() -> Self {
        Self {
            terminal: MeerkatExecutionTerminal::Failed,
            output: None,
        }
    }

    pub(crate) const fn cancelled() -> Self {
        Self {
            terminal: MeerkatExecutionTerminal::Cancelled,
            output: None,
        }
    }

    pub fn completed(
        output: impl Into<String>,
        max_output_bytes: usize,
    ) -> Result<Self, LiveBridgeOperationTerminalError> {
        let output = output.into();
        if output.trim().is_empty() {
            return Err(LiveBridgeOperationTerminalError::EmptyCompletedOutput);
        }
        if output.len() > max_output_bytes {
            return Err(LiveBridgeOperationTerminalError::OutputTooLarge {
                actual_bytes: output.len(),
                max_bytes: max_output_bytes,
            });
        }
        Ok(Self {
            terminal: MeerkatExecutionTerminal::Completed,
            output: Some(output),
        })
    }

    pub fn without_output(
        terminal: MeerkatExecutionTerminal,
    ) -> Result<Self, LiveBridgeOperationTerminalError> {
        if terminal == MeerkatExecutionTerminal::Completed {
            return Err(LiveBridgeOperationTerminalError::CompletedRequiresOutput);
        }
        Ok(Self {
            terminal,
            output: None,
        })
    }

    #[must_use]
    pub const fn terminal(&self) -> MeerkatExecutionTerminal {
        self.terminal
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveBridgeOperationTerminalError {
    #[error("a completed live bridge operation requires a non-empty output")]
    EmptyCompletedOutput,
    #[error("a completed live bridge operation requires an output")]
    CompletedRequiresOutput,
    #[error("live bridge output has {actual_bytes} bytes, exceeding the {max_bytes}-byte limit")]
    OutputTooLarge {
        actual_bytes: usize,
        max_bytes: usize,
    },
}

/// Failure before the executor accepts custody. Only the explicitly temporary
/// class is retryable. Once `start` returns an accepted execution, all later
/// outcomes are terminals and must never be replayed as a fresh invocation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveBridgeOperationStartError {
    #[error("live bridge execution host is unavailable")]
    Unavailable,
    #[error("live bridge execution is temporarily unavailable before acceptance")]
    TemporarilyUnavailable,
    #[error("live bridge execution request is invalid")]
    InvalidRequest,
    #[error("live bridge execution was rejected before acceptance")]
    Rejected,
    #[error("live bridge execution start failed before acceptance")]
    Failed,
}

impl LiveBridgeOperationStartError {
    #[must_use]
    pub const fn retry_safe_before_acceptance(&self) -> bool {
        matches!(self, Self::TemporarilyUnavailable)
    }
}

pub type LiveBridgeOperationTerminalFuture =
    Pin<Box<dyn Future<Output = LiveBridgeOperationTerminal> + Send + 'static>>;

/// Host seam for actor-owned, noncommitting execution against an exact
/// authoritative Session snapshot of the already-bound durable member.
///
/// A production host must preserve the durable identity, model, and policy
/// sealed by the admission, use its exact canonical context revision, suppress
/// ambient transcript/checkpointer writes, and preserve the real comms/memory
/// tool surface behind generated per-effect authority. The stock composition
/// injects no executor and therefore does not advertise Responses bridging.
#[async_trait]
pub trait LiveBridgeOperationExecutor: Send + Sync {
    async fn start(
        &self,
        request: LiveBridgeOperationRequest,
        cancellation: LiveBridgeOperationCancellationSignal,
    ) -> Result<LiveBridgeOperationTerminalFuture, LiveBridgeOperationStartError>;
}

/// Production executor adapter for one already-resolved durable member.
/// Construction performs no agent build and owns no lifecycle capability.
#[derive(Clone)]
pub struct DurableMemberLiveBridgeOperationExecutor {
    member: super::MemberHandle,
}

impl DurableMemberLiveBridgeOperationExecutor {
    #[must_use]
    pub fn new(member: super::MemberHandle) -> Self {
        Self { member }
    }
}

#[async_trait]
impl LiveBridgeOperationExecutor for DurableMemberLiveBridgeOperationExecutor {
    async fn start(
        &self,
        request: LiveBridgeOperationRequest,
        cancellation: LiveBridgeOperationCancellationSignal,
    ) -> Result<LiveBridgeOperationTerminalFuture, LiveBridgeOperationStartError> {
        self.member
            .start_live_bridge_operation(request, cancellation)
            .await
    }
}

/// One execution accepted by the host. The coordinator owns this handle until
/// the independent Meerkat terminal has been recorded by generated authority.
pub struct LiveBridgeAcceptedExecution {
    operation_id: OperationId,
    cancellation: LiveBridgeOperationCancellationHandle,
    terminal: LiveBridgeOperationTerminalFuture,
}

impl LiveBridgeAcceptedExecution {
    #[must_use]
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    #[must_use]
    pub fn cancellation_handle(&self) -> LiveBridgeOperationCancellationHandle {
        self.cancellation.clone()
    }

    pub async fn await_terminal(self) -> LiveBridgeOperationTerminal {
        // Cancellation is delivered into the session actor. Its terminal is
        // published only after the noncommitting run has dropped the provider
        // future and restored the canonical member Session/runtime fields.
        // Winning locally would expose Cancelled before that restoration.
        // Retain the sender while awaiting too. Dropping it would close the
        // watch channel and could otherwise be mistaken for cancellation by
        // an actor that is still executing normally.
        let Self {
            cancellation,
            terminal,
            ..
        } = self;
        let result = terminal.await;
        drop(cancellation);
        result
    }
}

/// Mechanical service that makes the pre-accept/accepted boundary explicit.
/// It owns no semantic ledger and cannot address any Mob member lifecycle API.
#[derive(Clone)]
pub struct LiveBridgeOperationService {
    executor: Arc<dyn LiveBridgeOperationExecutor>,
}

impl LiveBridgeOperationService {
    #[must_use]
    pub fn new(executor: Arc<dyn LiveBridgeOperationExecutor>) -> Self {
        Self { executor }
    }

    pub async fn start(
        &self,
        request: LiveBridgeOperationRequest,
    ) -> Result<LiveBridgeAcceptedExecution, LiveBridgeOperationStartError> {
        let operation_id = request.admission().operation().operation_id().clone();
        let (cancellation, signal) =
            LiveBridgeOperationCancellationHandle::pair(operation_id.clone());
        let terminal = self.executor.start(request, signal).await?;
        Ok(LiveBridgeAcceptedExecution {
            operation_id,
            cancellation,
            terminal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::exact_operation::ExactOperationIdentity;
    use meerkat_core::interaction::InteractionId;
    use meerkat_core::{
        LiveBridgeOperationCorrelation, LiveBridgeProviderCorrelation, LiveBridgeRequestDigest,
        LiveChannelId, SessionId,
    };

    fn test_admission(
        request: &str,
    ) -> (
        Arc<LiveBridgeOperationAdmission>,
        LiveBridgeExecutionSnapshot,
    ) {
        let session_id = SessionId::new();
        let mut session = Session::with_id(session_id.clone());
        session
            .set_session_metadata(meerkat_core::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "test-model".to_string(),
                max_tokens: 1024,
                structured_output_retries: 2,
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                auth_binding: None,
                mob_member_binding: Some(meerkat_core::MobMemberBinding {
                    mob_id: "mob".to_string(),
                    role: "personal".to_string(),
                    member: "personal-agent".to_string(),
                }),
            })
            .expect("session metadata");
        let snapshot =
            LiveBridgeExecutionSnapshot::from_generation_bound_session(session, "personal-agent")
                .expect("execution snapshot");
        let channel_id = LiveChannelId::new("channel:responses-service");
        let binding = meerkat_runtime::live_execution::LiveDelegationRuntimeBinding::__test_new(
            session_id.clone(),
            channel_id.clone(),
            meerkat_runtime::identifiers::LogicalRuntimeId::new("runtime:durable-member"),
            41,
            7,
        );
        let provider =
            LiveBridgeProviderCorrelation::new("turn:opaque", "delegation:opaque", "call:opaque")
                .expect("provider correlation");
        let correlation =
            LiveBridgeOperationCorrelation::new(channel_id, InteractionId::new(), provider)
                .expect("bridge operation correlation");
        let operation = ExactOperationIdentity::for_domain(OperationId::new(), correlation);
        let admission = Arc::new(LiveBridgeOperationAdmission::__test_new(
            session_id,
            binding,
            operation,
            "personal-agent",
            snapshot.canonical_context_revision().clone(),
            LiveBridgeRequestDigest::derive(request).expect("request digest"),
        ));
        (admission, snapshot)
    }

    struct RetryOnceExecutor {
        starts: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl LiveBridgeOperationExecutor for RetryOnceExecutor {
        async fn start(
            &self,
            request: LiveBridgeOperationRequest,
            cancellation: LiveBridgeOperationCancellationSignal,
        ) -> Result<LiveBridgeOperationTerminalFuture, LiveBridgeOperationStartError> {
            if self
                .starts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 0
            {
                return Err(LiveBridgeOperationStartError::TemporarilyUnavailable);
            }
            assert_eq!(request.admission().agent_identity(), "personal-agent");
            assert_eq!(
                request.admission().canonical_context_revision(),
                request.snapshot().canonical_context_revision()
            );
            assert_eq!(
                cancellation.operation_id(),
                request.admission().operation().operation_id()
            );
            let max_output_bytes = request.max_output_bytes();
            Ok(Box::pin(async move {
                LiveBridgeOperationTerminal::completed("executor result", max_output_bytes)
                    .expect("bounded terminal")
            }))
        }
    }

    #[test]
    fn only_explicit_pre_accept_unavailability_is_retry_safe() {
        assert!(
            LiveBridgeOperationStartError::TemporarilyUnavailable.retry_safe_before_acceptance()
        );
        for error in [
            LiveBridgeOperationStartError::Unavailable,
            LiveBridgeOperationStartError::InvalidRequest,
            LiveBridgeOperationStartError::Rejected,
            LiveBridgeOperationStartError::Failed,
        ] {
            assert!(!error.retry_safe_before_acceptance());
        }
    }

    #[tokio::test]
    async fn pre_accept_failure_can_retry_same_sealed_durable_operation() {
        let executor = Arc::new(RetryOnceExecutor {
            starts: std::sync::atomic::AtomicUsize::new(0),
        });
        let service = LiveBridgeOperationService::new(
            Arc::clone(&executor) as Arc<dyn LiveBridgeOperationExecutor>
        );
        let (admission, snapshot) = test_admission("handle my live request");

        let first = service
            .start(
                LiveBridgeOperationRequest::new(
                    Arc::clone(&admission),
                    snapshot.clone(),
                    "handle my live request",
                )
                .expect("first request"),
            )
            .await;
        assert!(matches!(
            first,
            Err(LiveBridgeOperationStartError::TemporarilyUnavailable)
        ));

        let accepted = service
            .start(
                LiveBridgeOperationRequest::new(admission, snapshot, "handle my live request")
                    .expect("retry request"),
            )
            .await
            .expect("retry accepted");
        let terminal = accepted.await_terminal().await;
        assert_eq!(terminal.terminal(), MeerkatExecutionTerminal::Completed);
        assert_eq!(terminal.output(), Some("executor result"));
        assert_eq!(executor.starts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn completed_output_is_bounded_and_other_terminals_carry_no_output() {
        let completed =
            LiveBridgeOperationTerminal::completed("result", 6).expect("bounded completed result");
        assert_eq!(completed.output(), Some("result"));
        assert_eq!(completed.terminal(), MeerkatExecutionTerminal::Completed);
        assert!(matches!(
            LiveBridgeOperationTerminal::completed("result", 5),
            Err(LiveBridgeOperationTerminalError::OutputTooLarge { .. })
        ));
        assert!(
            LiveBridgeOperationTerminal::without_output(MeerkatExecutionTerminal::Completed)
                .is_err()
        );
        assert_eq!(
            LiveBridgeOperationTerminal::without_output(MeerkatExecutionTerminal::Cancelled)
                .expect("cancelled terminal")
                .output(),
            None
        );
    }

    #[tokio::test]
    async fn cancellation_signal_converges_without_member_lifecycle_authority() {
        let operation_id = OperationId::new();
        let (handle, signal) = LiveBridgeOperationCancellationHandle::pair(operation_id.clone());
        assert_eq!(handle.operation_id(), &operation_id);
        assert_eq!(signal.operation_id(), &operation_id);
        let waiter = tokio::spawn(async move {
            signal.cancelled().await;
            signal.is_cancelled()
        });
        handle.cancel();
        assert!(waiter.await.expect("cancellation waiter"));
        assert!(handle.is_cancelled());
    }

    #[tokio::test]
    async fn accepted_execution_waits_for_actor_restoration_terminal_after_cancel() {
        let operation_id = OperationId::new();
        let (cancellation, _signal) =
            LiveBridgeOperationCancellationHandle::pair(operation_id.clone());
        let (restored_tx, restored_rx) = tokio::sync::oneshot::channel();
        let execution = LiveBridgeAcceptedExecution {
            operation_id,
            cancellation: cancellation.clone(),
            terminal: Box::pin(async move {
                let _ = restored_rx.await;
                LiveBridgeOperationTerminal::cancelled()
            }),
        };

        cancellation.cancel();
        let mut terminal = Box::pin(execution.await_terminal());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut terminal)
                .await
                .is_err(),
            "cancel must not publish a terminal before actor restoration"
        );
        restored_tx.send(()).expect("publish restored terminal");
        let terminal = terminal.await;
        assert_eq!(terminal.terminal(), MeerkatExecutionTerminal::Cancelled);
    }

    #[tokio::test]
    async fn awaiting_terminal_keeps_cancellation_sender_alive_without_cancelling() {
        let operation_id = OperationId::new();
        let (cancellation, signal) =
            LiveBridgeOperationCancellationHandle::pair(operation_id.clone());
        let (complete_tx, complete_rx) = tokio::sync::oneshot::channel();
        let execution = LiveBridgeAcceptedExecution {
            operation_id,
            cancellation,
            terminal: Box::pin(async move {
                tokio::select! {
                    () = signal.cancelled() => LiveBridgeOperationTerminal::cancelled(),
                    _ = complete_rx => LiveBridgeOperationTerminal::completed("done", 16)
                        .expect("bounded completed terminal"),
                }
            }),
        };

        let mut terminal = Box::pin(execution.await_terminal());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut terminal)
                .await
                .is_err(),
            "awaiting the terminal must not close the cancellation channel"
        );
        complete_tx.send(()).expect("release completed terminal");
        assert_eq!(
            terminal.await.terminal(),
            MeerkatExecutionTerminal::Completed
        );
    }
}
