//! Hook contracts and engine interfaces.

use crate::error::AgentError;
use crate::event::{AgentErrorClass, AgentErrorReport, ToolCallArguments};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::types::{
    CommsNoticeKind, ContentBlock, HandlingMode, RunInput, ServerToolKind, SessionId, StopReason,
    SystemNoticePeer, ToolProvenance, ToolResult, Usage,
};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Stable identifier for a configured hook.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct HookId(pub String);

impl HookId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for HookId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for HookId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for HookId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Hook points available in V1.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookPoint {
    RunStarted,
    RunCompleted,
    RunFailed,
    PreLlmRequest,
    PostLlmResponse,
    PreToolExecution,
    PostToolExecution,
    TurnBoundary,
    RuntimeInputAccepted,
    RuntimeInputRejected,
    RuntimeInputDeduplicated,
    PeerIngressCommitted,
    PeerEgressCommitted,
    InteractionCompleted,
}

impl HookPoint {
    pub fn is_pre(self) -> bool {
        matches!(
            self,
            Self::RunStarted | Self::PreLlmRequest | Self::PreToolExecution | Self::TurnBoundary
        )
    }

    pub fn is_post(self) -> bool {
        matches!(
            self,
            Self::PostLlmResponse
                | Self::PostToolExecution
                | Self::RunCompleted
                | Self::RunFailed
                | Self::RuntimeInputAccepted
                | Self::RuntimeInputRejected
                | Self::RuntimeInputDeduplicated
                | Self::PeerIngressCommitted
                | Self::PeerEgressCommitted
                | Self::InteractionCompleted
        )
    }

    /// Whether this point observes an already-committed or terminally-resolved
    /// fact and therefore cannot carry policy authority.
    pub fn is_observe_only(self) -> bool {
        matches!(
            self,
            Self::RuntimeInputAccepted
                | Self::RuntimeInputRejected
                | Self::RuntimeInputDeduplicated
                | Self::PeerIngressCommitted
                | Self::PeerEgressCommitted
                | Self::InteractionCompleted
        )
    }
}

/// Foreground hooks block loop progression; background hooks run asynchronously.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookExecutionMode {
    Foreground,
    Background,
}

/// Declared capability determines default failure behavior and constraints.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookCapability {
    Observe,
    Guardrail,
}

/// Runtime input kind projected onto the public hook surface.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookRuntimeInputKind {
    Prompt,
    PeerMessage,
    PeerRequest,
    PeerResponseProgress,
    PeerResponseTerminal,
    FlowStep,
    ExternalEvent,
    Continuation,
    Operation,
}

/// Runtime lifecycle state carried by a typed input rejection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookRuntimeState {
    Initializing,
    Idle,
    Attached,
    Running,
    Retired,
    Stopped,
    Destroyed,
}

/// Typed terminal reason for a runtime input rejection observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "reason_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookRuntimeInputRejection {
    NotReady { state: HookRuntimeState },
    ValidationFailed { detail: String },
    DurabilityViolation { detail: String },
    PeerHandlingModeInvalid { detail: String },
    PeerResponseTerminalInvalid { detail: String },
}

/// An input was durably admitted by runtime authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookRuntimeInputAccepted {
    pub input_id: crate::lifecycle::InputId,
    pub input_kind: HookRuntimeInputKind,
    pub handling_mode: HandlingMode,
}

/// An input was terminally rejected without an admission commit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookRuntimeInputRejected {
    pub input_id: crate::lifecycle::InputId,
    pub input_kind: HookRuntimeInputKind,
    pub reason: HookRuntimeInputRejection,
}

/// An idempotent input submission resolved to an existing admitted input.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookRuntimeInputDeduplicated {
    pub input_id: crate::lifecycle::InputId,
    pub input_kind: HookRuntimeInputKind,
    pub existing_input_id: crate::lifecycle::InputId,
}

/// A typed peer input was committed by runtime admission.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookPeerIngressCommitted {
    pub kind: CommsNoticeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<SystemNoticePeer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_taint: Option<crate::comms::SenderContentTaint>,
}

/// Typed class of a committed peer egress operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookPeerEgressKind {
    Message,
    IncarnationFencedMessage,
    Lifecycle,
    Request,
    Response,
}

/// A peer send reached the strongest successful outcome proved by its
/// transport and local lifecycle authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookPeerEgressCommitted {
    pub kind: HookPeerEgressKind,
    pub peer_id: crate::comms::PeerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<crate::comms::PeerName>,
    pub envelope_id: uuid::Uuid,
    pub delivery: crate::comms::PeerDeliveryOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<crate::interaction::InteractionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<crate::interaction::InteractionId>,
}

/// A correlated interaction completion was durably published.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookInteractionCompleted {
    pub interaction_id: crate::interaction::InteractionId,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
}

/// Typed committed fact carried by an observe-only hook invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "observation_type",
    content = "observation",
    rename_all = "snake_case"
)]
#[non_exhaustive]
pub enum HookObservation {
    RuntimeInputAccepted(HookRuntimeInputAccepted),
    RuntimeInputRejected(HookRuntimeInputRejected),
    RuntimeInputDeduplicated(HookRuntimeInputDeduplicated),
    PeerIngressCommitted(HookPeerIngressCommitted),
    PeerEgressCommitted(HookPeerEgressCommitted),
    InteractionCompleted(HookInteractionCompleted),
}

impl HookObservation {
    #[must_use]
    pub fn point(&self) -> HookPoint {
        match self {
            Self::RuntimeInputAccepted(_) => HookPoint::RuntimeInputAccepted,
            Self::RuntimeInputRejected(_) => HookPoint::RuntimeInputRejected,
            Self::RuntimeInputDeduplicated(_) => HookPoint::RuntimeInputDeduplicated,
            Self::PeerIngressCommitted(_) => HookPoint::PeerIngressCommitted,
            Self::PeerEgressCommitted(_) => HookPoint::PeerEgressCommitted,
            Self::InteractionCompleted(_) => HookPoint::InteractionCompleted,
        }
    }

    /// Project a canonical committed agent event onto the hook surface.
    #[must_use]
    pub fn from_committed_agent_event(event: &crate::event::AgentEvent) -> Option<Self> {
        match event {
            crate::event::AgentEvent::PeerContentIngested {
                kind,
                peer,
                request_id,
                sender_taint,
            } => Some(Self::PeerIngressCommitted(HookPeerIngressCommitted {
                kind: kind.clone(),
                peer: peer.clone(),
                request_id: request_id.clone(),
                sender_taint: *sender_taint,
            })),
            crate::event::AgentEvent::InteractionComplete {
                interaction_id,
                result,
                structured_output,
            } => Some(Self::InteractionCompleted(HookInteractionCompleted {
                interaction_id: *interaction_id,
                result: result.clone(),
                structured_output: structured_output.clone(),
            })),
            _ => None,
        }
    }

    /// Project a successful peer command receipt onto the hook surface.
    #[must_use]
    pub fn from_committed_peer_send(
        command: &crate::comms::CommsCommand,
        receipt: &crate::comms::SendReceipt,
    ) -> Option<Self> {
        use crate::comms::{CommsCommand, SendReceipt};

        let (kind, route, envelope_id, delivery, interaction_id, in_reply_to) =
            match (command, receipt) {
                (
                    CommsCommand::PeerMessage { to, .. },
                    SendReceipt::PeerMessageSent {
                        envelope_id,
                        delivery,
                    },
                ) => (
                    HookPeerEgressKind::Message,
                    to,
                    *envelope_id,
                    *delivery,
                    None,
                    None,
                ),
                (
                    CommsCommand::IncarnationFencedPeerMessage { to, .. },
                    SendReceipt::PeerMessageSent {
                        envelope_id,
                        delivery,
                    },
                ) => (
                    HookPeerEgressKind::IncarnationFencedMessage,
                    to,
                    *envelope_id,
                    *delivery,
                    None,
                    None,
                ),
                (
                    CommsCommand::PeerLifecycle { to, .. },
                    SendReceipt::PeerLifecycleSent {
                        envelope_id,
                        delivery,
                    },
                ) => (
                    HookPeerEgressKind::Lifecycle,
                    to,
                    *envelope_id,
                    *delivery,
                    None,
                    None,
                ),
                (
                    CommsCommand::PeerRequest { to, .. },
                    SendReceipt::PeerRequestSent {
                        envelope_id,
                        interaction_id,
                        delivery,
                        ..
                    },
                ) => (
                    HookPeerEgressKind::Request,
                    to,
                    *envelope_id,
                    *delivery,
                    Some(*interaction_id),
                    None,
                ),
                (
                    CommsCommand::PeerResponse { to, .. },
                    SendReceipt::PeerResponseSent {
                        envelope_id,
                        in_reply_to,
                        delivery,
                    },
                ) => (
                    HookPeerEgressKind::Response,
                    to,
                    *envelope_id,
                    *delivery,
                    None,
                    Some(*in_reply_to),
                ),
                _ => return None,
            };

        Some(Self::PeerEgressCommitted(HookPeerEgressCommitted {
            kind,
            peer_id: route.peer_id,
            display_name: route.display_name.clone(),
            envelope_id,
            delivery,
            interaction_id,
            in_reply_to,
        }))
    }
}

#[derive(Clone)]
struct PostCommitHookRegistration {
    namespace: uuid::Uuid,
    source_id: Option<HookBackgroundSourceId>,
    tasks: Arc<std::sync::atomic::AtomicUsize>,
    engine: Arc<dyn HookEngine>,
    overrides: crate::config::HookRunOverrides,
}

impl PostCommitHookRegistration {
    fn matches_engine(
        &self,
        engine: &Arc<dyn HookEngine>,
        source_id: Option<HookBackgroundSourceId>,
    ) -> bool {
        Arc::ptr_eq(&self.engine, engine) || self.source_id.is_some_and(|id| Some(id) == source_id)
    }
}

struct TrackedPostCommitHookTask {
    abort: tokio::task::AbortHandle,
    finished: Arc<std::sync::atomic::AtomicBool>,
}

struct PostCommitHookTaskCompletion {
    finished: Arc<std::sync::atomic::AtomicBool>,
    registration_tasks: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for PostCommitHookTaskCompletion {
    fn drop(&mut self) {
        self.finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.registration_tasks
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Session-scoped mechanical dispatcher for observe-only committed facts.
///
/// Producers call [`Self::dispatch`] only after their owning commit or terminal
/// resolution. Dispatch never awaits hook execution and cannot change the fact
/// being observed.
pub struct PostCommitHookDispatcher {
    session_id: SessionId,
    state: std::sync::Mutex<PostCommitHookDispatcherState>,
    changed: tokio::sync::Notify,
    flush: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
struct PendingBackgroundNotice {
    fact: HookRegisteredBackgroundCompletion,
    record: crate::types::SystemNoticeRecord,
}

struct PostCommitHookDispatcherState {
    registration: Option<PostCommitHookRegistration>,
    retained: Vec<PostCommitHookRegistration>,
    pending: Vec<PendingBackgroundNotice>,
    inflight: Vec<TrackedPostCommitHookTask>,
    shutdown: bool,
}

impl PostCommitHookDispatcher {
    #[must_use]
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            state: std::sync::Mutex::new(PostCommitHookDispatcherState {
                registration: None,
                retained: Vec::new(),
                pending: Vec::new(),
                inflight: Vec::new(),
                shutdown: false,
            }),
            changed: tokio::sync::Notify::new(),
            flush: tokio::sync::Mutex::new(()),
        }
    }

    /// Install a host configuration without dropping outstanding observations.
    /// A full retained-source set rejects this proposed activation atomically.
    /// The host must stop invoking a replaced engine for this session; already
    /// entered engine calls and their background work remain retained.
    /// Native ledger clones retain one passive source namespace. Custom engines
    /// without that optional identity use same-Arc matching. To replace such an
    /// engine, it must report its actual lifecycle; a custom engine with no
    /// background work may report supported empty status. Unknown status refuses
    /// the proposed configuration and leaves the installed engine unchanged.
    pub fn configure(
        &self,
        engine: Option<Arc<dyn HookEngine>>,
        overrides: crate::config::HookRunOverrides,
    ) -> Result<(), HookEngineError> {
        const MAX_RETAINED_ENGINES: usize = 64;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.shutdown {
            return Err(HookEngineError::InvalidConfiguration(
                "hook dispatcher is shut down".to_owned(),
            ));
        }
        let source_id = engine
            .as_ref()
            .and_then(|engine| engine.background_completion_source_id());
        if let Some(current) = &state.registration {
            let same_source = engine
                .as_ref()
                .is_some_and(|engine| current.matches_engine(engine, source_id));
            if !same_source
                && current
                    .engine
                    .background_session_status(&self.session_id)
                    .is_none()
            {
                return Err(HookEngineError::InvalidConfiguration(
                    "installed hook engine cannot currently provide the lifecycle status required for replacement".to_owned(),
                ));
            }
        }
        self.prune_registrations(&mut state);
        let registration = if let Some(engine) = engine {
            if let Some(existing) = state
                .retained
                .iter()
                .find(|entry| entry.matches_engine(&engine, source_id))
            {
                Some(PostCommitHookRegistration {
                    engine,
                    overrides,
                    ..existing.clone()
                })
            } else {
                if state.retained.len() >= MAX_RETAINED_ENGINES {
                    return Err(HookEngineError::InvalidConfiguration(
                        "hook configuration cannot replace retained background sources until they settle".to_owned(),
                    ));
                }
                let registration = PostCommitHookRegistration {
                    namespace: source_id.map_or_else(uuid::Uuid::new_v4, |id| id.0),
                    source_id,
                    tasks: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                    engine,
                    overrides,
                };
                state.retained.push(registration.clone());
                Some(registration)
            }
        } else {
            None
        };
        state.registration = registration;
        drop(state);
        self.changed.notify_waiters();
        Ok(())
    }

    fn prune_registrations(&self, state: &mut PostCommitHookDispatcherState) {
        let current = state.registration.as_ref().map(|entry| entry.namespace);
        let pending = &state.pending;
        state.retained.retain(|entry| {
            Some(entry.namespace) == current
                || entry.tasks.load(std::sync::atomic::Ordering::Acquire) != 0
                || pending
                    .iter()
                    .any(|item| item.fact.registration_id == entry.namespace)
                || entry.engine.background_session_status(&self.session_id)
                    != Some(HookBackgroundSessionStatus::default())
        });
    }

    /// The active consumer uses the same passive namespace as late persistence.
    /// No engine or run is substituted when the requested source is absent.
    pub fn take_active_background_completions(
        &self,
        engine: &Arc<dyn HookEngine>,
        run_id: &crate::RunId,
        limit: usize,
    ) -> Vec<HookRegisteredBackgroundCompletion> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.shutdown {
            return Vec::new();
        }
        let source_id = engine.background_completion_source_id();
        let Some(registration) = state
            .retained
            .iter()
            .find(|entry| entry.matches_engine(engine, source_id))
        else {
            return Vec::new();
        };
        registration
            .engine
            .take_background_completions(&self.session_id, Some(run_id), limit.min(32))
            .into_iter()
            .map(|completion| HookRegisteredBackgroundCompletion {
                registration_id: registration.namespace,
                completion,
            })
            .collect()
    }

    /// Cheap readiness inspection only. The real writer still runs under the
    /// current attachment and idle boundary, never under this passive fact.
    pub fn has_ready_background_completions(&self) -> bool {
        let Ok(state) = self.state.try_lock() else {
            return false;
        };
        !state.shutdown
            && (!state.pending.is_empty()
                || state.retained.iter().any(|entry| {
                    entry
                        .engine
                        .background_session_status(&self.session_id)
                        .is_some_and(|status| status.ready != 0)
                }))
    }

    /// Move a bounded ready batch into this session owner even when its writer
    /// is temporarily unavailable. This does not claim persistence or receipt.
    pub fn retain_ready_background_completions(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.shutdown {
            return;
        }
        let mut remaining = 32usize.saturating_sub(state.pending.len());
        for registration in state.retained.clone() {
            if remaining == 0 {
                break;
            }
            let ready = registration
                .engine
                .take_session_background_completions(&self.session_id, remaining);
            remaining = remaining.saturating_sub(ready.len());
            state.pending.extend(ready.into_iter().map(|completion| {
                let fact = HookRegisteredBackgroundCompletion {
                    registration_id: registration.namespace,
                    completion,
                };
                let record = fact.system_notice_record();
                PendingBackgroundNotice { fact, record }
            }));
        }
    }

    /// Persist fixed facts while the caller owns the current attachment's idle
    /// turn-finalization boundary. No model call, input admission or work
    /// authorization is performed. Pending records survive cancellation and
    /// writer failure; a successful duplicate is the existing writer's no-op.
    pub async fn flush_background_completions_under_turn_finalization_boundary(
        &self,
        writer: &dyn crate::lifecycle::CoreExecutorTranscriptNoticeHandle,
    ) -> Result<usize, crate::lifecycle::CoreExecutorError> {
        let _flush = self.flush.lock().await;
        self.retain_ready_background_completions();
        let mut persisted = 0;
        loop {
            let pending = {
                let state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.shutdown {
                    return Ok(persisted);
                }
                state.pending.first().cloned()
            };
            let Some(pending) = pending else {
                break;
            };
            writer
                .append_system_notice_under_turn_finalization_boundary(pending.record.clone())
                .await?;
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.pending.first().is_some_and(|item| {
                item.fact.registration_id == pending.fact.registration_id
                    && item.fact.completion.ordinal == pending.fact.completion.ordinal
            }) {
                state.pending.remove(0);
                persisted += 1;
            }
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.prune_registrations(&mut state);
        Ok(persisted)
    }

    /// Non-consuming readiness for the existing idle loop. Failed writes stay
    /// pending and wait for its bounded retry delay, avoiding a ready-spin.
    /// Registration changes rebuild the waits; no polling worker is spawned.
    pub async fn wait_for_background_completion(&self, retry_delay: std::time::Duration) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let (registrations, pending, shutdown) = {
                let state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (
                    state.retained.clone(),
                    !state.pending.is_empty(),
                    state.shutdown,
                )
            };
            if shutdown {
                std::future::pending::<()>().await;
            }
            if pending {
                tokio::time::sleep(retry_delay.max(std::time::Duration::from_millis(25))).await;
                return;
            }
            let mut waits = futures::stream::FuturesUnordered::new();
            for registration in &registrations {
                waits.push(
                    registration
                        .engine
                        .wait_for_session_background_completion(&self.session_id),
                );
            }
            if waits.is_empty() {
                changed.await;
            } else {
                use futures::StreamExt;
                tokio::select! {
                    () = &mut changed => {},
                    _ = waits.next() => return,
                }
            }
        }
    }

    pub fn dispatch(&self, observation: HookObservation) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.shutdown {
            return;
        }
        let registration = state.registration.clone();
        let Some(registration) = registration else {
            return;
        };
        let invocation = HookInvocation::committed(self.session_id.clone(), observation);
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_finished = Arc::clone(&finished);
        registration
            .tasks
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        // Own the counter before spawn so dropping a never-polled future also
        // retires this exact dispatch task.
        let completion = PostCommitHookTaskCompletion {
            finished: task_finished,
            registration_tasks: Arc::clone(&registration.tasks),
        };
        let task = async move {
            let _completion = completion;
            match registration
                .engine
                .execute_post_commit(invocation.clone(), Some(&registration.overrides))
                .await
            {
                Ok(report) => {
                    for refusal in &report.launch_refusals {
                        tracing::warn!(
                            session_id = %invocation.session_id,
                            hook_id = %refusal.hook_id,
                            point = ?refusal.point,
                            refusal = %refusal.refusal,
                            "post-commit observer launch refused; the committed fact is unchanged"
                        );
                    }
                    if matches!(report.decision, Some(HookDecision::Deny { .. })) {
                        tracing::warn!(
                            point = ?invocation.point,
                            "observe-only post-commit hook returned a denial; the committed fact is unchanged"
                        );
                    }
                }
                Err(error) => {
                    let mut retained_error = &error;
                    while let HookEngineError::WithReport {
                        report,
                        error: inner,
                    } = retained_error
                    {
                        for refusal in &report.launch_refusals {
                            tracing::warn!(
                                session_id = %invocation.session_id,
                                hook_id = %refusal.hook_id,
                                point = ?refusal.point,
                                refusal = %refusal.refusal,
                                "post-commit observer launch refused; the committed fact is unchanged"
                            );
                        }
                        retained_error = inner.as_ref();
                    }
                    tracing::warn!(
                        point = ?invocation.point,
                        error = %error,
                        "observe-only post-commit hook execution failed; the committed fact is unchanged"
                    );
                }
            }
        };
        let handle = tokio::spawn(task);
        state
            .inflight
            .retain(|task| !task.finished.load(std::sync::atomic::Ordering::Acquire));
        state.inflight.push(TrackedPostCommitHookTask {
            abort: handle.abort_handle(),
            finished,
        });
    }

    /// Stop accepting configured work and abort every owned observation task.
    ///
    /// Runtime session teardown calls this explicitly; `Drop` is the fallback.
    pub fn shutdown(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.shutdown {
            return;
        }
        state.shutdown = true;
        state.registration = None;
        // Explicit process-local disposal. Aborting a future does not prove
        // target non-entry or child exit; existing process custody owns that.
        let disposed = state.pending.len();
        let retained_sources = state.retained.len();
        state.pending.clear();
        state.retained.clear();
        if disposed != 0 || retained_sources != 0 {
            tracing::warn!(session_id = %self.session_id, disposed, retained_sources,
                "session shutdown disposed background feedback custody; task cancellation is not child-exit evidence");
        }
        self.changed.notify_waiters();
        for task in state.inflight.drain(..) {
            task.abort.abort();
        }
    }

    #[cfg(test)]
    fn inflight_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .inflight
            .len()
    }
}

impl std::fmt::Debug for PostCommitHookDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostCommitHookDispatcher")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl Drop for PostCommitHookDispatcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Typed reason codes for guardrail denials.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookReasonCode {
    PolicyViolation,
    SafetyViolation,
    SchemaViolation,
    Timeout,
    RuntimeError,
}

/// Typed reason a hook execution failed (engine-level fault, not a guardrail
/// denial).
///
/// Mirrors the [`HookReasonCode`] precedent: the variant is the typed owner of
/// the failure cause; the human-readable string is a [`Display`] derivation,
/// never a separately-stored field.
///
/// [`Display`]: std::fmt::Display
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason_code", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookFailureReason {
    /// The hook runtime did not complete within its configured timeout.
    Timeout { timeout_ms: u64 },
    /// The invocation failed. Entry disposition is retained by its engine error.
    ExecutionFailed {
        /// Display projection of the underlying execution error.
        message: String,
    },
    /// The hook configuration was rejected.
    ConfigInvalid {
        /// Display projection of the configuration error.
        message: String,
    },
    /// A background hook attempted a non-observe action, which is not
    /// permitted for observe-only background hooks at any hook point.
    ObserveOnlyViolation,
    /// Mechanical requirements prevented the hook from entering.
    ConfinementRefused {
        refusal: crate::confinement::ConfinementRefusal,
    },
}

impl std::fmt::Display for HookFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfinementRefused { refusal } => write!(f, "{refusal}"),
            Self::Timeout { timeout_ms } => write!(f, "hook timed out after {timeout_ms}ms"),
            Self::ExecutionFailed { message } => write!(f, "{message}"),
            Self::ConfigInvalid { message } => write!(f, "{message}"),
            Self::ObserveOnlyViolation => {
                write!(f, "background hooks are observe-only")
            }
        }
    }
}

impl HookFailureReason {
    /// Typed execution failure carrying the display message.
    pub fn execution_failed(message: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: message.into(),
        }
    }

    /// Project a typed [`HookEngineError`] into its failure reason.
    #[must_use]
    pub fn from_engine_error(error: &HookEngineError) -> Self {
        match error {
            HookEngineError::WithReport { error, .. } => Self::from_engine_error(error),
            HookEngineError::LaunchRefused { reason, .. } => reason.clone(),
            HookEngineError::InvalidConfiguration(reason) => Self::ConfigInvalid {
                message: reason.clone(),
            },
            HookEngineError::ExecutionFailed { reason, .. } => Self::ExecutionFailed {
                message: reason.clone(),
            },
            HookEngineError::Timeout { timeout_ms, .. } => Self::Timeout {
                timeout_ms: *timeout_ms,
            },
        }
    }
}

/// Final decision produced by merged hook outcomes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum HookDecision {
    Allow,
    Deny {
        hook_id: HookId,
        reason_code: HookReasonCode,
        message: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_hook_payload",
            skip_serializing_if = "Option::is_none"
        )]
        payload: Option<Value>,
    },
}

impl HookDecision {
    pub fn deny(
        hook_id: HookId,
        reason_code: HookReasonCode,
        message: impl Into<String>,
        payload: Option<Value>,
    ) -> Self {
        Self::Deny {
            hook_id,
            reason_code,
            message: message.into(),
            payload,
        }
    }
}

/// Exact facts of an authoritative hook decision that refuses an operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, thiserror::Error)]
#[serde(deny_unknown_fields)]
#[error("Hook '{hook_id}' denied at {point:?}: {reason_code:?} - {message}")]
pub struct HookDenial {
    pub hook_id: HookId,
    pub point: HookPoint,
    pub reason_code: HookReasonCode,
    pub message: String,
    // A present JSON null remains distinct from an absent payload on transport.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hook_payload",
        skip_serializing_if = "Option::is_none"
    )]
    pub payload: Option<Value>,
}

pub(crate) fn deserialize_present_hook_payload<'de, D>(
    deserializer: D,
) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

/// LLM request view exposed to hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookLlmRequest {
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Typed effective provider parameter overrides for this LLM call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<crate::lifecycle::run_primitive::ProviderParamsOverride>,
    pub message_count: usize,
}

/// LLM response view exposed to hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookLlmResponse {
    pub assistant_text: String,
    #[serde(default)]
    pub tool_call_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Typed kinds of provider-executed server-tool evidence present in this
    /// response (`AssistantBlock::ServerToolContent` blocks), in block order.
    /// Projection of the typed block owner: a foreground `PostLlmResponse`
    /// hook classifies provider-native content (e.g. web search) synchronously
    /// from this field instead of racing the lossy observe stream.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub server_tool_content: Vec<ServerToolKind>,
}

/// Tool call view exposed to hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookToolCall {
    pub tool_use_id: String,
    pub name: String,
    pub args: ToolCallArguments,
    /// Typed provenance of the dispatched tool definition, when the active
    /// tool catalog carries one. Projection of the `ToolDef.provenance` owner
    /// (never re-derived from the tool name string): dispatch-time policy
    /// hooks steer on this typed field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
}

/// Tool result view exposed to hooks.
///
/// `content_blocks` is the canonical typed tool-result content that post-tool
/// hooks deny/terminalize on. The text projection is presentation-only and is
/// derived from the blocks via [`HookToolResult::text_projection`]; it is never
/// a separately-stored field that policy code can read.
///
/// The wire envelope additionally serializes a `content` string for external
/// (command/HTTP) hook consumers that read the text result. That field is a
/// pure serialize-only derivation of `content_blocks` (the text projection); it
/// is never a stored field, is never deserialized back as authority, and policy
/// code must steer on `content_blocks`. Restoring it (remediation row #331)
/// keeps external hook consumers — which previously read `content` — working
/// without re-introducing a lossy mutable mirror.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookToolResult {
    pub tool_use_id: String,
    pub name: String,
    /// Canonical typed tool-result content exposed to hooks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_blocks: Vec<ContentBlock>,
    pub is_error: bool,
    /// Typed provenance of the dispatched tool definition, when the active
    /// tool catalog carries one. Projection of the `ToolDef.provenance` owner
    /// (never re-derived from the tool name string).
    #[serde(default)]
    pub provenance: Option<ToolProvenance>,
}

impl Serialize for HookToolResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        // `content` is a serialize-only text projection derived from the
        // canonical `content_blocks` for external hook consumers (row #331).
        // The field count is `tool_use_id`, `name`, `content`, `is_error`,
        // plus `content_blocks` and `provenance` when present.
        let mut len = 4;
        if !self.content_blocks.is_empty() {
            len += 1;
        }
        if self.provenance.is_some() {
            len += 1;
        }
        let mut state = serializer.serialize_struct("HookToolResult", len)?;
        state.serialize_field("tool_use_id", &self.tool_use_id)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("content", &self.text_projection())?;
        if !self.content_blocks.is_empty() {
            state.serialize_field("content_blocks", &self.content_blocks)?;
        }
        state.serialize_field("is_error", &self.is_error)?;
        if let Some(provenance) = &self.provenance {
            state.serialize_field("provenance", provenance)?;
        }
        state.end()
    }
}

impl HookToolResult {
    pub fn from_tool_result(name: impl Into<String>, result: &ToolResult) -> Self {
        Self::from_tool_result_with_id(result.tool_use_id.clone(), name, result)
    }

    pub fn from_tool_result_with_id(
        tool_use_id: impl Into<String>,
        name: impl Into<String>,
        result: &ToolResult,
    ) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            name: name.into(),
            content_blocks: result.content.clone(),
            is_error: result.is_error,
            provenance: None,
        }
    }

    /// Attach the typed provenance of the dispatched tool definition
    /// (chainable builder, mirroring [`crate::types::ToolDef::with_provenance`]).
    #[must_use]
    pub fn with_provenance(mut self, provenance: Option<ToolProvenance>) -> Self {
        self.provenance = provenance;
        self
    }

    /// Presentation-only text projection derived from the canonical typed
    /// `content_blocks`.
    ///
    /// This is for rendering/diagnostics only and MUST NOT feed hook policy
    /// (allow/deny/terminalize) decisions — those steer on `content_blocks`.
    #[must_use]
    pub fn text_projection(&self) -> String {
        crate::types::text_content(&self.content_blocks)
    }
}

/// Full invocation payload passed into the hook engine.
///
/// `prompt_input` and `error_report` are the typed owners of the prompt and
/// failure facts. The wire envelope additionally serializes `prompt` and
/// `error` strings for external (command/HTTP) hook consumers; those fields
/// are pure serialize-only derivations of the typed owners (precedent:
/// [`HookToolResult`]'s `content`, row #331). They are never stored fields,
/// never deserialize back as authority, and in-process policy code must steer
/// on the typed owners.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookInvocation {
    pub point: HookPoint,
    pub session_id: SessionId,
    /// The run the invocation belongs to, when it happens inside one. The
    /// agent fills it in from its bound run; a host that keeps durable
    /// process custody records it with the hook's process, so a hook
    /// interrupted by an abrupt host stop settles its run instead of letting
    /// the run replay. Post-commit observations carry none: their run has
    /// already committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<crate::RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_input: Option<RunInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_report: Option<AgentErrorReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<AgentErrorClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_request: Option<HookLlmRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_response: Option<HookLlmResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<HookToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<HookToolResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<HookObservation>,
}

impl Serialize for HookInvocation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        // `prompt` and `error` are serialize-only text projections derived
        // from the typed owners at serialization time for external hook
        // consumers; they are never stored and never deserialized back.
        let prompt = self.prompt_input.as_ref().and_then(RunInput::prompt_text);
        let error = self
            .error_report
            .as_ref()
            .map(|report| report.message.clone());
        let len = 2
            + usize::from(self.run_id.is_some())
            + usize::from(self.turn_number.is_some())
            + usize::from(self.prompt_input.is_some())
            + usize::from(prompt.is_some())
            + usize::from(self.error_report.is_some())
            + usize::from(self.error_class.is_some())
            + usize::from(error.is_some())
            + usize::from(self.llm_request.is_some())
            + usize::from(self.llm_response.is_some())
            + usize::from(self.tool_call.is_some())
            + usize::from(self.tool_result.is_some())
            + usize::from(self.observation.is_some());
        let mut state = serializer.serialize_struct("HookInvocation", len)?;
        state.serialize_field("point", &self.point)?;
        state.serialize_field("session_id", &self.session_id)?;
        if let Some(run_id) = &self.run_id {
            state.serialize_field("run_id", run_id)?;
        }
        if let Some(turn_number) = &self.turn_number {
            state.serialize_field("turn_number", turn_number)?;
        }
        if let Some(prompt_input) = &self.prompt_input {
            state.serialize_field("prompt_input", prompt_input)?;
        }
        if let Some(prompt) = &prompt {
            state.serialize_field("prompt", prompt)?;
        }
        if let Some(error_report) = &self.error_report {
            state.serialize_field("error_report", error_report)?;
        }
        if let Some(error_class) = &self.error_class {
            state.serialize_field("error_class", error_class)?;
        }
        if let Some(error) = &error {
            state.serialize_field("error", error)?;
        }
        if let Some(llm_request) = &self.llm_request {
            state.serialize_field("llm_request", llm_request)?;
        }
        if let Some(llm_response) = &self.llm_response {
            state.serialize_field("llm_response", llm_response)?;
        }
        if let Some(tool_call) = &self.tool_call {
            state.serialize_field("tool_call", tool_call)?;
        }
        if let Some(tool_result) = &self.tool_result {
            state.serialize_field("tool_result", tool_result)?;
        }
        if let Some(observation) = &self.observation {
            state.serialize_field("observation", observation)?;
        }
        state.end()
    }
}

impl HookInvocation {
    pub fn new(point: HookPoint, session_id: SessionId) -> Self {
        Self {
            point,
            session_id,
            run_id: None,
            turn_number: None,
            prompt_input: None,
            error_report: None,
            error_class: None,
            llm_request: None,
            llm_response: None,
            tool_call: None,
            tool_result: None,
            observation: None,
        }
    }

    pub fn committed(session_id: SessionId, observation: HookObservation) -> Self {
        let mut invocation = Self::new(observation.point(), session_id);
        invocation.observation = Some(observation);
        invocation
    }

    pub fn run_started(session_id: SessionId, prompt_input: RunInput) -> Self {
        Self {
            prompt_input: Some(prompt_input),
            ..Self::new(HookPoint::RunStarted, session_id)
        }
    }

    pub fn run_completed(session_id: SessionId, turn_number: u32) -> Self {
        Self {
            turn_number: Some(turn_number),
            ..Self::new(HookPoint::RunCompleted, session_id)
        }
    }

    pub fn run_failed(session_id: SessionId, error: &AgentError) -> Self {
        let error_report = AgentErrorReport::from_agent_error(error);
        Self {
            error_class: Some(error_report.class),
            error_report: Some(error_report),
            ..Self::new(HookPoint::RunFailed, session_id)
        }
    }
}

/// Outcome emitted by one executed hook entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookOutcome {
    pub hook_id: HookId,
    pub point: HookPoint,
    pub priority: i32,
    pub registration_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
    /// Typed failure cause for this hook outcome, when the hook did not succeed.
    /// The string form is derived via [`HookOutcome::failure_message`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<HookFailureReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl HookOutcome {
    /// Display projection of the typed [`HookOutcome::failure_reason`], when
    /// present. This is presentation-only — policy code must branch on the
    /// typed `failure_reason` variant.
    #[must_use]
    pub fn failure_message(&self) -> Option<String> {
        self.failure_reason.as_ref().map(ToString::to_string)
    }
}

/// A confinement refusal before a foreground or post-commit Observe hook entered.
///
/// This is a no-entry fact, not an executed outcome or a hook decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct HookLaunchRefusal {
    pub hook_id: HookId,
    pub point: HookPoint,
    pub refusal: crate::confinement::ConfinementRefusal,
}

/// Passive identifiers of an original committed observation, without its content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookBackgroundObservation {
    Input {
        input_id: crate::lifecycle::InputId,
        existing_input_id: Option<crate::lifecycle::InputId>,
    },
    PeerIngress {
        peer_id: Option<crate::comms::PeerId>,
        request_id: Option<String>,
    },
    PeerEgress {
        envelope_id: uuid::Uuid,
    },
    Interaction {
        interaction_id: crate::interaction::InteractionId,
    },
}

impl HookBackgroundObservation {
    pub(crate) fn from_observation(observation: &HookObservation) -> Self {
        match observation {
            HookObservation::RuntimeInputAccepted(value) => HookBackgroundObservation::Input {
                input_id: value.input_id.clone(),
                existing_input_id: None,
            },
            HookObservation::RuntimeInputRejected(value) => HookBackgroundObservation::Input {
                input_id: value.input_id.clone(),
                existing_input_id: None,
            },
            HookObservation::RuntimeInputDeduplicated(value) => HookBackgroundObservation::Input {
                input_id: value.input_id.clone(),
                existing_input_id: Some(value.existing_input_id.clone()),
            },
            HookObservation::PeerIngressCommitted(value) => {
                HookBackgroundObservation::PeerIngress {
                    peer_id: value.peer.as_ref().map(|peer| peer.id),
                    request_id: value.request_id.clone(),
                }
            }
            HookObservation::PeerEgressCommitted(value) => HookBackgroundObservation::PeerEgress {
                envelope_id: value.envelope_id,
            },
            HookObservation::InteractionCompleted(value) => {
                HookBackgroundObservation::Interaction {
                    interaction_id: value.interaction_id,
                }
            }
        }
    }
}

/// Original ownership coordinates, never a recovered producer capability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookBackgroundAttribution {
    pub session_id: SessionId,
    pub run_id: Option<crate::RunId>,
    pub turn_number: Option<u32>,
    pub hook_id: HookId,
    pub point: HookPoint,
    pub tool_use_id: Option<String>,
    pub observation: Option<HookBackgroundObservation>,
}

impl HookBackgroundAttribution {
    pub fn from_invocation(hook_id: HookId, invocation: &HookInvocation) -> Self {
        let observation = invocation
            .observation
            .as_ref()
            .map(HookBackgroundObservation::from_observation);
        Self {
            session_id: invocation.session_id.clone(),
            run_id: invocation.run_id.clone(),
            turn_number: invocation.turn_number,
            hook_id,
            point: invocation.point,
            tool_use_id: invocation
                .tool_call
                .as_ref()
                .map(|call| call.tool_use_id.clone())
                .or_else(|| {
                    invocation
                        .tool_result
                        .as_ref()
                        .map(|result| result.tool_use_id.clone())
                }),
            observation,
        }
    }

    pub fn matches_scope(&self, session_id: &SessionId, run_id: Option<&crate::RunId>) -> bool {
        &self.session_id == session_id && self.run_id.as_ref() == run_id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", content = "result", rename_all = "snake_case")]
pub enum HookBackgroundResult {
    /// Actual completed observation. It proves neither publication nor delivery.
    Completed(HookOutcome),
    /// Exact no-entry disposition from the launch owner.
    LaunchRefused(HookFailureReason),
    /// Entry/effects cannot be inferred from timeout or other runtime failure.
    Failed(HookFailureReason),
}

/// One ready process-local task result. Transfer is not a durable delivery ACK.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookBackgroundCompletion {
    /// Distinguishes repeated invocations in this engine only, not durable identity.
    pub ordinal: u64,
    pub attribution: HookBackgroundAttribution,
    pub result: HookBackgroundResult,
    /// A bounded retained diagnostic is a prefix, not the complete original text.
    pub diagnostic_truncated: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookBackgroundSkipReason {
    ConcurrencyFull,
    RetentionFull,
    AttributionTooLarge,
    OrdinalExhausted,
}

/// A scheduling refusal, with no implication that any target entered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookBackgroundSkip {
    pub hook_id: HookId,
    pub point: HookPoint,
    pub reason: HookBackgroundSkipReason,
}

/// Aggregate result used by the core loop to apply hook decisions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub struct HookExecutionReport {
    /// Hook ids the engine actually began executing. Background scheduling is
    /// excluded: its eventual completion retains the entry disposition. This is the
    /// authoritative basis for `HookStarted` events: a hook only appears here
    /// once execution began, never merely because it matched the invocation
    /// point (a foreground deny short-circuit and a saturated background queue
    /// both leave later/skipped hooks absent).
    #[serde(default)]
    pub started: Vec<HookId>,
    #[serde(default)]
    pub outcomes: Vec<HookOutcome>,
    /// Observe hooks refused before entry. These ids must not also appear in
    /// `started` or `outcomes` for the same attempt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub launch_refusals: Vec<HookLaunchRefusal>,
    /// Background attempts not scheduled. This remains distinct from entry,
    /// completion, or a Guardrail decision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background_skips: Vec<HookBackgroundSkip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
}

impl HookExecutionReport {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Project an authoritative hook denial into the typed agent error shape.
    ///
    /// Runtime policy owns whether the returned error terminalizes the run.
    /// This projection only preserves the denial facts emitted by the hook
    /// engine without reclassifying them through string matching.
    pub fn denial_error(&self, point: HookPoint) -> Option<AgentError> {
        self.denial(point).map(|denial| AgentError::HookDenied {
            hook_id: denial.hook_id,
            point: denial.point,
            reason_code: denial.reason_code,
            message: denial.message,
            payload: denial.payload,
        })
    }

    /// Retain the engine-owned decision without deciding the run disposition.
    pub fn denial(&self, point: HookPoint) -> Option<HookDenial> {
        match self.decision.as_ref()? {
            HookDecision::Deny {
                hook_id,
                reason_code,
                message,
                payload,
            } => Some(HookDenial {
                hook_id: hook_id.clone(),
                point,
                reason_code: *reason_code,
                message: message.clone(),
                payload: payload.clone(),
            }),
            HookDecision::Allow => None,
        }
    }
}

/// Engine-level failures that prevented hook execution.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HookEngineError {
    /// Preserve only prior facts when a later hook fails after a local Observe
    /// refusal. The failing hook is represented solely by `error`; classification
    /// and run disposition remain those of that original error.
    #[error("{error}")]
    WithReport {
        report: Box<HookExecutionReport>,
        #[source]
        error: Box<HookEngineError>,
    },
    #[error("Hook configuration invalid: {0}")]
    InvalidConfiguration(String),
    #[error("Hook runtime execution failed for '{hook_id}': {reason}")]
    ExecutionFailed { hook_id: HookId, reason: String },
    #[error("Hook '{hook_id}' timed out after {timeout_ms}ms")]
    Timeout { hook_id: HookId, timeout_ms: u64 },
    /// The hook was refused before its target code entered. The reason remains
    /// in its original domain, including ordinary invocation setup failures.
    #[error("Hook launch refused for '{hook_id}': {reason}")]
    LaunchRefused {
        hook_id: HookId,
        reason: HookFailureReason,
    },
}

impl HookEngineError {
    pub fn hook_id(&self) -> Option<&HookId> {
        match self {
            Self::WithReport { error, .. } => error.hook_id(),
            Self::InvalidConfiguration(_) | Self::LaunchRefused { .. } => None,
            Self::ExecutionFailed { hook_id, .. } | Self::Timeout { hook_id, .. } => Some(hook_id),
        }
    }

    pub fn into_agent_error(self) -> AgentError {
        match self {
            Self::WithReport { error, .. } => error.into_agent_error(),
            Self::LaunchRefused { hook_id, reason } => {
                AgentError::HookLaunchRefused { hook_id, reason }
            }
            Self::InvalidConfiguration(reason) => AgentError::HookConfigInvalid { reason },
            Self::Timeout {
                hook_id,
                timeout_ms,
            } => AgentError::HookTimeout {
                hook_id,
                timeout_ms,
            },
            Self::ExecutionFailed { hook_id, reason } => {
                AgentError::HookExecutionFailed { hook_id, reason }
            }
        }
    }
}

/// Passive process-local namespace owned once by a completion ledger. Clones
/// and wrappers of that same ledger must retain this value. Construction grants
/// no permission and supplies no durable delivery or session authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookBackgroundSourceId(uuid::Uuid);

impl HookBackgroundSourceId {
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for HookBackgroundSourceId {
    fn default() -> Self {
        Self::new()
    }
}

/// Counts from the actual engine owner, never inferred from its ready queue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HookBackgroundSessionStatus {
    pub running: usize,
    pub ready: usize,
}

/// Passive process-local pairing. This namespace is minted once for the actual
/// installed engine and cannot grant permission or act as a durable receipt.
#[derive(Debug, Clone)]
pub struct HookRegisteredBackgroundCompletion {
    pub registration_id: uuid::Uuid,
    pub completion: HookBackgroundCompletion,
}

impl HookRegisteredBackgroundCompletion {
    /// The same safe fixed block is used for active projection and writer retry.
    /// It accounts for no new user request and retains the original optional run.
    pub fn system_notice_record(&self) -> crate::types::SystemNoticeRecord {
        let completion = &self.completion;
        let (disposition, body) = match &completion.result {
            HookBackgroundResult::Completed(outcome) if outcome.failure_reason.is_none() => (
                "completed",
                "A background observation hook completed. This is an observation result, not a permission decision.",
            ),
            HookBackgroundResult::LaunchRefused(_) => (
                "launch_refused",
                "A background observation hook could not enter. Continue with permitted work; this notice grants no permission.",
            ),
            _ => (
                "failed",
                "A background observation hook did not complete successfully. Entry or side effects cannot be inferred from this notice.",
            ),
        };
        let mut payload = serde_json::json!({
            "registration_id": self.registration_id,
            "ordinal": completion.ordinal,
            "attribution": completion.attribution,
            "disposition": disposition,
        });
        let reason = match &completion.result {
            HookBackgroundResult::Completed(outcome) => outcome.failure_reason.as_ref(),
            HookBackgroundResult::LaunchRefused(reason) | HookBackgroundResult::Failed(reason) => {
                Some(reason)
            }
        };
        if let Some(reason) = reason {
            payload["reason"] = serde_json::json!(background_feedback_reason(reason));
            if let HookBackgroundResult::LaunchRefused(HookFailureReason::ConfinementRefused {
                refusal,
            }) = &completion.result
            {
                payload["refusal"] = serde_json::json!(refusal);
            }
        }
        crate::types::SystemNoticeMessage::with_block(
            crate::types::SystemNoticeKind::Generic,
            Some(body.to_owned()),
            crate::types::SystemNoticeBlock::RuntimeNotice {
                category: "background_hook_completion".to_owned(),
                detail: Some(payload.to_string()),
                payload: Some(payload),
            },
        )
        .into()
    }
}

pub(crate) fn background_feedback_reason(reason: &HookFailureReason) -> HookFailureReason {
    match reason {
        HookFailureReason::ExecutionFailed { .. } => HookFailureReason::ExecutionFailed {
            message: "background hook execution failed".to_owned(),
        },
        HookFailureReason::ConfigInvalid { .. } => HookFailureReason::ConfigInvalid {
            message: "background hook configuration was rejected".to_owned(),
        },
        other => other.clone(),
    }
}

/// Runtime-independent engine interface.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait HookEngine: Send + Sync + 'static {
    /// Optional passive identity of the actual background ledger. The same
    /// ledger must keep one identity across clones and wrapper Arcs; distinct
    /// ledgers must not share it. This is correlation only, never authority.
    /// Without it, dispatcher configuration recognizes only the same Arc.
    fn background_completion_source_id(&self) -> Option<HookBackgroundSourceId> {
        None
    }

    /// Transfer at most `limit` ready items from this exact original scope.
    /// `None` matches only an originally unbound run, never all runs. This must
    /// not wait for running hooks or lock contention. Other scopes remain owned
    /// by the engine for a separately authorized host consumer. Default engines
    /// without background work need no additional state.
    fn take_background_completions(
        &self,
        _session_id: &SessionId,
        _run_id: Option<&crate::RunId>,
        _limit: usize,
    ) -> Vec<HookBackgroundCompletion> {
        Vec::new()
    }

    /// Actual in-flight engine calls plus scheduled tasks and ready facts for
    /// this session. None means unsupported or temporarily unavailable, never
    /// supported quiescence. A source with no background work can return empty
    /// status to permit replacement. The native source may return None while
    /// its nonblocking ledger observation is contended.
    fn background_session_status(
        &self,
        _session_id: &SessionId,
    ) -> Option<HookBackgroundSessionStatus> {
        None
    }

    /// Bounded transfer for the same session across its original run scopes.
    /// This explicit late-owner method never treats an absent run as a wildcard.
    fn take_session_background_completions(
        &self,
        _session_id: &SessionId,
        _limit: usize,
    ) -> Vec<HookBackgroundCompletion> {
        Vec::new()
    }

    /// Non-consuming, cancellation-safe session readiness. Implementations must
    /// register the wake before checking ready state. No background support
    /// means no wake, not an immediately completed empty result.
    async fn wait_for_session_background_completion(&self, _session_id: &SessionId) {
        std::future::pending::<()>().await;
    }

    fn matching_hooks(
        &self,
        _invocation: &HookInvocation,
        _overrides: Option<&crate::config::HookRunOverrides>,
    ) -> Result<Vec<HookId>, HookEngineError> {
        Ok(Vec::new())
    }

    async fn execute(
        &self,
        invocation: HookInvocation,
        overrides: Option<&crate::config::HookRunOverrides>,
    ) -> Result<HookExecutionReport, HookEngineError>;

    /// Execute one post-commit observation inside its dispatcher's owned task.
    ///
    /// Implementations must not detach additional work from this future.
    async fn execute_post_commit(
        &self,
        invocation: HookInvocation,
        overrides: Option<&crate::config::HookRunOverrides>,
    ) -> Result<HookExecutionReport, HookEngineError> {
        self.execute(invocation, overrides).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::comms::{CommsCommand, PeerDeliveryOutcome, PeerId, PeerRoute, SendReceipt};
    use crate::types::{ContentBlock, ToolResult};
    use std::sync::Arc;

    fn text_block(s: &str) -> ContentBlock {
        ContentBlock::Text {
            text: s.to_string(),
        }
    }

    fn image_block(media_type: &str, data: &str) -> ContentBlock {
        ContentBlock::Image {
            media_type: media_type.to_string(),
            data: data.into(),
        }
    }

    struct BlockingObserveEngine {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl HookEngine for BlockingObserveEngine {
        async fn execute(
            &self,
            invocation: HookInvocation,
            _overrides: Option<&crate::config::HookRunOverrides>,
        ) -> Result<HookExecutionReport, HookEngineError> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(HookExecutionReport {
                decision: Some(HookDecision::deny(
                    HookId::new("ignored-denial"),
                    HookReasonCode::PolicyViolation,
                    format!("{:?}", invocation.point),
                    None,
                )),
                ..HookExecutionReport::empty()
            })
        }
    }

    #[tokio::test]
    async fn post_commit_dispatch_does_not_join_or_apply_a_hook_decision() {
        let engine = Arc::new(BlockingObserveEngine {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let dispatcher = PostCommitHookDispatcher::new(SessionId::new());
        dispatcher
            .configure(
                Some(Arc::clone(&engine) as Arc<dyn HookEngine>),
                crate::config::HookRunOverrides::default(),
            )
            .expect("install hook engine");

        dispatcher.dispatch(HookObservation::RuntimeInputAccepted(
            HookRuntimeInputAccepted {
                input_id: crate::lifecycle::InputId::new(),
                input_kind: HookRuntimeInputKind::Prompt,
                handling_mode: HandlingMode::Queue,
            },
        ));

        tokio::time::timeout(std::time::Duration::from_secs(1), engine.entered.notified())
            .await
            .expect("post-commit hook should start asynchronously");
        engine.release.notify_one();
    }

    struct AbortGuard(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for AbortGuard {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    struct AbortObservedEngine {
        entered: tokio::sync::Notify,
        aborted: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl HookEngine for AbortObservedEngine {
        async fn execute(
            &self,
            _invocation: HookInvocation,
            _overrides: Option<&crate::config::HookRunOverrides>,
        ) -> Result<HookExecutionReport, HookEngineError> {
            let sender = self
                .aborted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let _guard = AbortGuard(sender);
            self.entered.notify_one();
            std::future::pending::<()>().await;
            Ok(HookExecutionReport::empty())
        }
    }

    #[tokio::test]
    async fn dispatcher_drop_aborts_a_blocked_observation() {
        let (aborted_tx, aborted_rx) = tokio::sync::oneshot::channel();
        let engine = Arc::new(AbortObservedEngine {
            entered: tokio::sync::Notify::new(),
            aborted: std::sync::Mutex::new(Some(aborted_tx)),
        });
        let dispatcher = PostCommitHookDispatcher::new(SessionId::new());
        dispatcher
            .configure(
                Some(Arc::clone(&engine) as Arc<dyn HookEngine>),
                crate::config::HookRunOverrides::default(),
            )
            .expect("install hook engine");
        dispatcher.dispatch(HookObservation::RuntimeInputAccepted(
            HookRuntimeInputAccepted {
                input_id: crate::lifecycle::InputId::new(),
                input_kind: HookRuntimeInputKind::Prompt,
                handling_mode: HandlingMode::Queue,
            },
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), engine.entered.notified())
            .await
            .expect("blocked observation should start");

        drop(dispatcher);

        tokio::time::timeout(std::time::Duration::from_secs(1), aborted_rx)
            .await
            .expect("dispatcher drop should abort its task")
            .expect("abort guard should report cancellation");
    }

    struct ImmediateObserveEngine {
        completed: std::sync::atomic::AtomicUsize,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl HookEngine for ImmediateObserveEngine {
        async fn execute(
            &self,
            _invocation: HookInvocation,
            _overrides: Option<&crate::config::HookRunOverrides>,
        ) -> Result<HookExecutionReport, HookEngineError> {
            self.completed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(HookExecutionReport::empty())
        }
    }

    #[tokio::test]
    async fn dispatcher_reaps_finished_tasks_before_tracking_new_work() {
        let engine = Arc::new(ImmediateObserveEngine {
            completed: std::sync::atomic::AtomicUsize::new(0),
        });
        let dispatcher = PostCommitHookDispatcher::new(SessionId::new());
        dispatcher
            .configure(
                Some(Arc::clone(&engine) as Arc<dyn HookEngine>),
                crate::config::HookRunOverrides::default(),
            )
            .expect("install hook engine");
        for _ in 0..32 {
            dispatcher.dispatch(HookObservation::RuntimeInputAccepted(
                HookRuntimeInputAccepted {
                    input_id: crate::lifecycle::InputId::new(),
                    input_kind: HookRuntimeInputKind::Prompt,
                    handling_mode: HandlingMode::Queue,
                },
            ));
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while engine.completed.load(std::sync::atomic::Ordering::SeqCst) != 32 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all immediate observations should finish");

        dispatcher.dispatch(HookObservation::RuntimeInputAccepted(
            HookRuntimeInputAccepted {
                input_id: crate::lifecycle::InputId::new(),
                input_kind: HookRuntimeInputKind::Prompt,
                handling_mode: HandlingMode::Queue,
            },
        ));
        assert_eq!(
            dispatcher.inflight_count(),
            1,
            "finished task handles must be reaped before tracking new work"
        );
    }

    struct SharedSourceObserveEngine {
        source_id: HookBackgroundSourceId,
        completed: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl HookEngine for SharedSourceObserveEngine {
        fn background_completion_source_id(&self) -> Option<HookBackgroundSourceId> {
            Some(self.source_id)
        }
        fn background_session_status(
            &self,
            _session_id: &SessionId,
        ) -> Option<HookBackgroundSessionStatus> {
            Some(HookBackgroundSessionStatus::default())
        }
        async fn execute(
            &self,
            _invocation: HookInvocation,
            _overrides: Option<&crate::config::HookRunOverrides>,
        ) -> Result<HookExecutionReport, HookEngineError> {
            self.completed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(HookExecutionReport::empty())
        }
    }

    #[tokio::test]
    async fn same_background_source_installs_the_new_execution_wrapper() {
        let source_id = HookBackgroundSourceId::new();
        let old_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let new_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let dispatcher = PostCommitHookDispatcher::new(SessionId::new());
        dispatcher
            .configure(
                Some(Arc::new(SharedSourceObserveEngine {
                    source_id,
                    completed: old_calls.clone(),
                })),
                Default::default(),
            )
            .expect("old wrapper");
        dispatcher
            .configure(
                Some(Arc::new(SharedSourceObserveEngine {
                    source_id,
                    completed: new_calls.clone(),
                })),
                Default::default(),
            )
            .expect("new wrapper, same passive source");
        dispatcher.dispatch(HookObservation::RuntimeInputAccepted(
            HookRuntimeInputAccepted {
                input_id: crate::lifecycle::InputId::new(),
                input_kind: HookRuntimeInputKind::Prompt,
                handling_mode: HandlingMode::Queue,
            },
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while new_calls.load(std::sync::atomic::Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("new execution wrapper used");
        assert_eq!(old_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(dispatcher.state.lock().unwrap().retained.len(), 1);
    }

    struct KnownBackgroundLifecycleEngine(HookBackgroundSessionStatus);

    #[async_trait::async_trait]
    impl HookEngine for KnownBackgroundLifecycleEngine {
        fn background_session_status(
            &self,
            _session_id: &SessionId,
        ) -> Option<HookBackgroundSessionStatus> {
            Some(self.0)
        }

        async fn execute(
            &self,
            _invocation: HookInvocation,
            _overrides: Option<&crate::config::HookRunOverrides>,
        ) -> Result<HookExecutionReport, HookEngineError> {
            Ok(HookExecutionReport::empty())
        }
    }

    #[test]
    fn full_retained_background_set_refuses_only_the_proposed_configuration() {
        let dispatcher = PostCommitHookDispatcher::new(SessionId::new());
        let mut last: Option<Arc<dyn HookEngine>> = None;
        for _ in 0..64 {
            let engine: Arc<dyn HookEngine> = Arc::new(KnownBackgroundLifecycleEngine(
                HookBackgroundSessionStatus {
                    running: 1,
                    ready: 0,
                },
            ));
            dispatcher
                .configure(Some(engine.clone()), Default::default())
                .expect("bounded slot");
            last = Some(engine);
        }
        let proposed: Arc<dyn HookEngine> = Arc::new(KnownBackgroundLifecycleEngine(
            HookBackgroundSessionStatus::default(),
        ));
        assert!(matches!(
            dispatcher.configure(Some(proposed), Default::default()),
            Err(HookEngineError::InvalidConfiguration(_))
        ));
        let current = last.expect("at least one installed engine");
        {
            let state = dispatcher.state.lock().unwrap();
            assert_eq!(state.retained.len(), 64);
            assert!(Arc::ptr_eq(
                &state.registration.as_ref().unwrap().engine,
                &current
            ));
        }
        dispatcher
            .configure(Some(current), Default::default())
            .expect("reuse needs no new slot");
    }

    #[test]
    fn custom_supported_empty_lifecycle_can_be_replaced_without_retained_growth() {
        let dispatcher = PostCommitHookDispatcher::new(SessionId::new());
        for _ in 0..128 {
            dispatcher
                .configure(
                    Some(Arc::new(KnownBackgroundLifecycleEngine(
                        HookBackgroundSessionStatus::default(),
                    ))),
                    Default::default(),
                )
                .expect("known-empty source can be replaced");
            assert!(dispatcher.state.lock().unwrap().retained.len() <= 2);
        }
        dispatcher
            .configure(None, Default::default())
            .expect("known-empty source can be disabled");
    }

    #[tokio::test]
    async fn unsupported_background_lifecycle_replacement_preserves_current_registration() {
        let current = Arc::new(ImmediateObserveEngine {
            completed: std::sync::atomic::AtomicUsize::new(0),
        });
        let proposed = Arc::new(ImmediateObserveEngine {
            completed: std::sync::atomic::AtomicUsize::new(0),
        });
        let dispatcher = PostCommitHookDispatcher::new(SessionId::new());
        dispatcher
            .configure(Some(current.clone()), Default::default())
            .expect("initial configuration");
        dispatcher
            .configure(Some(current.clone()), Default::default())
            .expect("same Arc remains installed");
        assert!(matches!(
            dispatcher.configure(Some(proposed.clone()), Default::default()),
            Err(HookEngineError::InvalidConfiguration(_))
        ));
        assert!(matches!(
            dispatcher.configure(None, Default::default()),
            Err(HookEngineError::InvalidConfiguration(_))
        ));
        dispatcher.dispatch(HookObservation::RuntimeInputAccepted(
            HookRuntimeInputAccepted {
                input_id: crate::lifecycle::InputId::new(),
                input_kind: HookRuntimeInputKind::Prompt,
                handling_mode: HandlingMode::Queue,
            },
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while current.completed.load(std::sync::atomic::Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the original registration remains usable");
        assert_eq!(
            proposed.completed.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_dispatch_and_shutdown_leave_no_owned_task() {
        let engine = Arc::new(AbortObservedEngine {
            entered: tokio::sync::Notify::new(),
            aborted: std::sync::Mutex::new(None),
        });
        let dispatcher = Arc::new(PostCommitHookDispatcher::new(SessionId::new()));
        dispatcher
            .configure(
                Some(engine as Arc<dyn HookEngine>),
                crate::config::HookRunOverrides::default(),
            )
            .expect("install hook engine");

        let mut dispatches = Vec::new();
        for _ in 0..32 {
            let dispatcher = Arc::clone(&dispatcher);
            dispatches.push(tokio::spawn(async move {
                dispatcher.dispatch(HookObservation::RuntimeInputAccepted(
                    HookRuntimeInputAccepted {
                        input_id: crate::lifecycle::InputId::new(),
                        input_kind: HookRuntimeInputKind::Prompt,
                        handling_mode: HandlingMode::Queue,
                    },
                ));
            }));
        }
        let shutdown = {
            let dispatcher = Arc::clone(&dispatcher);
            tokio::spawn(async move {
                dispatcher.shutdown();
            })
        };
        for dispatch in dispatches {
            dispatch.await.expect("dispatch task joins");
        }
        shutdown.await.expect("shutdown task joins");
        tokio::task::yield_now().await;

        assert_eq!(dispatcher.inflight_count(), 0);
        dispatcher.dispatch(HookObservation::RuntimeInputAccepted(
            HookRuntimeInputAccepted {
                input_id: crate::lifecycle::InputId::new(),
                input_kind: HookRuntimeInputKind::Prompt,
                handling_mode: HandlingMode::Queue,
            },
        ));
        assert_eq!(
            dispatcher.inflight_count(),
            0,
            "shutdown must reject every later dispatch"
        );
    }

    #[test]
    fn committed_agent_event_projects_typed_hook_observations() {
        let peer_event = crate::event::AgentEvent::PeerContentIngested {
            kind: CommsNoticeKind::Request,
            peer: Some(SystemNoticePeer {
                id: PeerId::new(),
                display_name: Some("reviewer".to_string()),
            }),
            request_id: Some("request-1".to_string()),
            sender_taint: Some(crate::comms::SenderContentTaint::Tainted),
        };
        assert!(matches!(
            HookObservation::from_committed_agent_event(&peer_event),
            Some(HookObservation::PeerIngressCommitted(
                HookPeerIngressCommitted {
                    kind: CommsNoticeKind::Request,
                    request_id: Some(request_id),
                    sender_taint: Some(crate::comms::SenderContentTaint::Tainted),
                    ..
                }
            )) if request_id == "request-1"
        ));

        let interaction_id = crate::interaction::InteractionId(uuid::Uuid::new_v4());
        let completion = crate::event::AgentEvent::InteractionComplete {
            interaction_id,
            result: "done".to_string(),
            structured_output: Some(serde_json::json!({"ok": true})),
        };
        assert!(matches!(
            HookObservation::from_committed_agent_event(&completion),
            Some(HookObservation::InteractionCompleted(HookInteractionCompleted {
                interaction_id: observed,
                result,
                ..
            })) if observed == interaction_id && result == "done"
        ));
    }

    #[test]
    fn peer_egress_projection_requires_matching_command_and_receipt() {
        let peer_id = PeerId::new();
        let command = CommsCommand::PeerMessage {
            to: PeerRoute::new(peer_id),
            body: "hello".to_string(),
            blocks: None,
            content_taint: None,
            handling_mode: HandlingMode::Queue,
            objective_id: None,
        };
        let envelope_id = uuid::Uuid::new_v4();
        let receipt = SendReceipt::PeerMessageSent {
            envelope_id,
            delivery: PeerDeliveryOutcome::Acked,
        };
        assert!(matches!(
            HookObservation::from_committed_peer_send(&command, &receipt),
            Some(HookObservation::PeerEgressCommitted(HookPeerEgressCommitted {
                kind: HookPeerEgressKind::Message,
                peer_id: observed_peer,
                envelope_id: observed_envelope,
                delivery: PeerDeliveryOutcome::Acked,
                ..
            })) if observed_peer == peer_id && observed_envelope == envelope_id
        ));

        let mismatched = SendReceipt::PeerLifecycleSent {
            envelope_id,
            delivery: PeerDeliveryOutcome::Acked,
        };
        assert!(
            HookObservation::from_committed_peer_send(&command, &mismatched).is_none(),
            "a mismatched custom runtime receipt must not mint a false egress fact"
        );
    }

    #[test]
    fn hook_tool_call_rejects_string_args_on_deserialize() {
        let value = serde_json::json!({
            "tool_use_id": "tc_1",
            "name": "search",
            "args": "{\"query\":"
        });

        let err = serde_json::from_value::<HookToolCall>(value)
            .expect_err("hook surface must reject string-success tool args");
        assert!(
            err.to_string().contains("JSON object, got string"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn hook_result_from_multimodal_uses_text_projection() {
        let tr = ToolResult::with_blocks(
            "tc_1".into(),
            vec![text_block("hello"), image_block("image/png", "AAAA")],
            false,
        );
        let hook_result = HookToolResult {
            tool_use_id: tr.tool_use_id.clone(),
            name: "test_tool".into(),
            content_blocks: tr.content.clone(),
            is_error: tr.is_error,
            provenance: None,
        };
        // text projection concatenates text from blocks; image blocks produce "[image: image/png]"
        assert_eq!(hook_result.text_projection(), "hello\n[image: image/png]");
    }

    #[test]
    fn hook_result_text_only_uses_text_projection() {
        let tr = ToolResult::new("tc_1".into(), "just text".into(), false);
        let hook_result = HookToolResult {
            tool_use_id: tr.tool_use_id.clone(),
            name: "test_tool".into(),
            content_blocks: tr.content.clone(),
            is_error: tr.is_error,
            provenance: None,
        };
        assert_eq!(hook_result.text_projection(), "just text");
        assert_eq!(hook_result.content_blocks, vec![text_block("just text")]);
    }

    #[test]
    fn hook_result_text_only_serializes_typed_content_blocks() {
        let tr = ToolResult::new("tc_1".into(), "just text".into(), false);
        let hook_result = HookToolResult::from_tool_result("test_tool", &tr);

        assert_eq!(hook_result.text_projection(), "just text");
        assert_eq!(hook_result.content_blocks, vec![text_block("just text")]);

        let json = serde_json::to_value(&hook_result).expect("serialize hook tool result");
        assert_eq!(
            json["content_blocks"],
            serde_json::json!([{"type": "text", "text": "just text"}])
        );
        // The wire envelope serializes a derived `content` text projection for
        // external hook consumers (row #331), derived purely from the blocks.
        assert_eq!(
            json["content"],
            serde_json::json!("just text"),
            "wire envelope must carry the derived `content` text projection"
        );
    }

    #[test]
    fn hook_result_image_only_serializes_typed_content_blocks() {
        let tr =
            ToolResult::with_blocks("tc_1".into(), vec![image_block("image/png", "AAAA")], false);
        let hook_result = HookToolResult::from_tool_result("view_image", &tr);

        assert_eq!(hook_result.text_projection(), "[image: image/png]");
        assert_eq!(
            hook_result.content_blocks,
            vec![image_block("image/png", "AAAA")]
        );

        let json = serde_json::to_value(&hook_result).expect("serialize hook tool result");
        assert_eq!(
            json["content_blocks"],
            serde_json::json!([{
                "type": "image",
                "media_type": "image/png",
                "source": "inline",
                "data": "AAAA"
            }])
        );
    }

    #[test]
    fn hook_result_mixed_content_preserves_block_order() {
        let tr = ToolResult::with_blocks(
            "tc_1".into(),
            vec![
                text_block("before"),
                image_block("image/png", "AAAA"),
                text_block("after"),
            ],
            false,
        );
        let hook_result = HookToolResult::from_tool_result("mixed_tool", &tr);

        assert_eq!(
            hook_result.text_projection(),
            "before\n[image: image/png]\nafter"
        );
        assert_eq!(hook_result.content_blocks, tr.content);
    }

    #[test]
    fn hook_result_can_use_authoritative_tool_call_id() {
        let tr = ToolResult::new("stale_tool_id".into(), "ok".into(), false);
        let hook_result =
            HookToolResult::from_tool_result_with_id("active_tool_id", "test_tool", &tr);

        assert_eq!(hook_result.tool_use_id, "active_tool_id");
        assert_eq!(hook_result.content_blocks, vec![text_block("ok")]);
    }

    #[test]
    fn hook_tool_result_text_projection_is_derived_only_from_typed_blocks() {
        // The text projection must be derived purely from the canonical typed
        // content_blocks — there is no separately-stored `content` mirror that
        // could diverge from the blocks or feed policy.
        let result = HookToolResult {
            tool_use_id: "tc_1".into(),
            name: "tool".into(),
            content_blocks: vec![text_block("alpha"), image_block("image/png", "AAAA")],
            is_error: false,
            provenance: None,
        };
        assert_eq!(
            result.text_projection(),
            crate::types::text_content(&result.content_blocks),
            "text projection must equal the rendering of the typed blocks"
        );

        // Mutating the typed blocks immediately changes the projection (no stale
        // stored mirror).
        let mut mutated = result;
        mutated.content_blocks = vec![text_block("beta")];
        assert_eq!(mutated.text_projection(), "beta");
    }

    #[test]
    fn hook_tool_result_ignores_incoming_content_string_as_authority() {
        // The incoming `content` string is not ingested as authority; the
        // canonical content comes from `content_blocks`.
        let decoded: HookToolResult = serde_json::from_value(serde_json::json!({
            "tool_use_id": "tc_1",
            "name": "tool",
            "content": "ignored-incoming-string",
            "content_blocks": [{"type": "text", "text": "text"}],
            "is_error": false
        }))
        .expect("should deserialize");
        assert_eq!(decoded.content_blocks, vec![text_block("text")]);
        assert_eq!(decoded.text_projection(), "text");
    }

    #[test]
    fn hook_tool_result_wire_content_round_trips_from_blocks() {
        // The wire envelope serializes `content` (derived) + `content_blocks`
        // (canonical); deserializing the envelope reconstructs the canonical
        // blocks and the derived `content` matches the text projection.
        let original = HookToolResult {
            tool_use_id: "tc_1".into(),
            name: "tool".into(),
            content_blocks: vec![text_block("alpha"), image_block("image/png", "AAAA")],
            is_error: false,
            provenance: None,
        };
        let json = serde_json::to_value(&original).expect("serialize");
        assert_eq!(
            json["content"],
            serde_json::json!(original.text_projection())
        );
        let decoded: HookToolResult = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded.content_blocks, original.content_blocks);
        assert_eq!(decoded.text_projection(), original.text_projection());
    }

    #[test]
    fn hook_invocation_prompt_and_error_are_serialize_only_projections() {
        let mut invocation = HookInvocation::run_started(
            SessionId::new(),
            RunInput::Content {
                content: crate::types::ContentInput::Text("typed prompt".to_string()),
            },
        );
        invocation.error_report = Some(AgentErrorReport {
            class: AgentErrorClass::Llm,
            reason: None,
            message: "typed failure".to_string(),
        });

        let json = serde_json::to_value(&invocation).expect("serialize");
        // The wire envelope derives `prompt`/`error` from the typed owners at
        // serialization time for external hook consumers.
        assert_eq!(json["prompt"], serde_json::json!("typed prompt"));
        assert_eq!(json["error"], serde_json::json!("typed failure"));

        // Deserializing a payload with divergent string mirrors must NOT
        // resurrect them as authority — only the typed owners survive.
        let mut forged = json;
        forged["prompt"] = serde_json::json!("forged prompt");
        forged["error"] = serde_json::json!("forged failure");
        let decoded: HookInvocation = serde_json::from_value(forged).expect("deserialize");
        assert_eq!(decoded, invocation);
        assert_eq!(
            serde_json::to_value(&decoded).expect("re-serialize")["prompt"],
            serde_json::json!("typed prompt")
        );
    }

    /// Ask 5 wire discipline: `provenance` is additive on the external hook
    /// envelope — omitted when absent (old payloads stay byte-stable), a
    /// typed projection of `ToolDef.provenance` when present, and round-trips
    /// through the custom `HookToolResult` serializer.
    #[test]
    fn hook_tool_payload_provenance_is_additive_and_round_trips() {
        use crate::types::{ToolProvenance, ToolSourceId, ToolSourceKind};

        let call = HookToolCall {
            tool_use_id: "tc_1".into(),
            name: "lookup".into(),
            args: ToolCallArguments::empty(),
            provenance: None,
        };
        let json = serde_json::to_value(&call).expect("serialize");
        assert!(
            json.get("provenance").is_none(),
            "absent provenance must be omitted from the hook wire envelope"
        );

        let provenance = ToolProvenance {
            kind: ToolSourceKind::Mcp,
            source_id: ToolSourceId::new("test-server"),
        };
        let call_with = HookToolCall {
            provenance: Some(provenance.clone()),
            ..call
        };
        let json = serde_json::to_value(&call_with).expect("serialize");
        assert_eq!(json["provenance"]["kind"], serde_json::json!("mcp"));
        let decoded: HookToolCall = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded.provenance, Some(provenance.clone()));

        let result = HookToolResult {
            tool_use_id: "tc_1".into(),
            name: "lookup".into(),
            content_blocks: vec![text_block("ok")],
            is_error: false,
            provenance: None,
        };
        let json = serde_json::to_value(&result).expect("serialize");
        assert!(
            json.get("provenance").is_none(),
            "absent provenance must be omitted by the custom serializer"
        );
        let result_with = result.with_provenance(Some(provenance.clone()));
        let json = serde_json::to_value(&result_with).expect("serialize");
        assert_eq!(
            json["provenance"]["source_id"],
            serde_json::json!("test-server")
        );
        let decoded: HookToolResult = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded.provenance, Some(provenance));
    }

    /// Ask 5 wire discipline: `server_tool_content` is a projection of the
    /// response's `AssistantBlock::ServerToolContent` kinds — an empty list is
    /// omitted from the envelope (old payloads stay byte-stable) and typed
    /// kinds round-trip.
    #[test]
    fn hook_llm_response_server_tool_content_is_additive_and_typed() {
        use crate::types::ServerToolKind;

        let response = HookLlmResponse {
            assistant_text: "done".into(),
            tool_call_names: Vec::new(),
            stop_reason: Some(StopReason::EndTurn),
            usage: None,
            server_tool_content: Vec::new(),
        };
        let json = serde_json::to_value(&response).expect("serialize");
        assert!(
            json.get("server_tool_content").is_none(),
            "empty server tool content must be omitted from the hook wire envelope"
        );

        let response = HookLlmResponse {
            server_tool_content: vec![
                ServerToolKind::WebSearch,
                ServerToolKind::ProviderNative {
                    name: "code_exec".to_string(),
                },
            ],
            ..response
        };
        let json = serde_json::to_value(&response).expect("serialize");
        assert_eq!(
            json["server_tool_content"][0]["kind"],
            serde_json::json!("web_search")
        );
        let decoded: HookLlmResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded.server_tool_content, response.server_tool_content);
    }

    /// K3 invariant: a pending-tool-results run carries the typed
    /// `RunInput::PendingToolResults` variant — no fabricated empty-string
    /// `prompt` projection appears on the hook wire envelope.
    #[test]
    fn hook_invocation_pending_tail_run_has_typed_variant_and_no_prompt_mirror() {
        let invocation =
            HookInvocation::run_started(SessionId::new(), RunInput::PendingToolResults);
        assert_eq!(
            invocation.prompt_input,
            Some(RunInput::PendingToolResults),
            "pending-tail run must carry the typed variant"
        );

        let json = serde_json::to_value(&invocation).expect("serialize");
        assert_eq!(
            json["prompt_input"],
            serde_json::json!({ "kind": "pending_tool_results" })
        );
        assert!(
            json.get("prompt").is_none(),
            "no empty-string prompt mirror may be fabricated for pending-tail runs: {json}"
        );
    }
}
