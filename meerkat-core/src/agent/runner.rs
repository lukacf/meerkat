//! Agent runner interface.

use crate::budget::Budget;
use crate::error::{AgentError, ToolError};
use crate::event::AgentEvent;
use crate::hooks::{HookInvocation, HookPoint};
use crate::lifecycle::run_primitive::{ConversationAppend, ConversationAppendRole, CoreRenderable};
use crate::ops::{ToolDispatchOutcome, ToolDispatchTimeoutPolicy};
use crate::pending_continuation::{observe_session_tail, resolve_pending_continuation};
use crate::retry::RetryPolicy;
use crate::service::TurnToolOverlay;
use crate::session::{PendingSystemContextAppend, Session};
use crate::session_document::{
    ObservedSessionTailKind, PendingContinuationDisposition, PendingContinuationPublicTerminal,
};
use crate::state::LoopState;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_scope::{
    EXTERNAL_TOOL_FILTER_METADATA_KEY, ExternalToolSurfaceBaseState,
    ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceEntrySnapshot, ExternalToolSurfaceSnapshot, ToolFilter, ToolScopeApplyError,
    ToolScopeRevision, ToolScopeStageError,
};
use crate::turn_execution_authority::{
    TurnPrimitiveKind, TurnTerminalCauseKind, TurnTerminalOutcome,
};
use crate::types::{
    BlockAssistantMessage, ContentInput, Message, RunInput, RunResult, ToolCallView, ToolNameSet,
    TranscriptMessageIdentity, UserMessage,
};
use async_trait::async_trait;
use serde_json::value::to_raw_value;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{Agent, AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};

fn user_message_from_operator_renderable(
    content: CoreRenderable,
) -> Result<crate::types::UserMessage, AgentError> {
    match content {
        CoreRenderable::Text { text } => Ok(crate::types::UserMessage::text(text)),
        CoreRenderable::Blocks { blocks } => Ok(crate::types::UserMessage::with_blocks(blocks)),
        CoreRenderable::SystemNotice { .. }
        | CoreRenderable::Json { .. }
        | CoreRenderable::Reference { .. } => Err(AgentError::ConfigError(
            "role=user transcript append only accepts operator text or content blocks".to_string(),
        )),
    }
}

fn injected_context_message_from_operator_renderable(
    content: CoreRenderable,
) -> Result<crate::types::UserMessage, AgentError> {
    match content {
        CoreRenderable::Text { text } => Ok(crate::types::UserMessage::injected_context(text)),
        CoreRenderable::Blocks { blocks } => Ok(
            crate::types::UserMessage::injected_context_with_blocks(blocks),
        ),
        CoreRenderable::SystemNotice { .. }
        | CoreRenderable::Json { .. }
        | CoreRenderable::Reference { .. } => Err(AgentError::ConfigError(
            "role=injected_context transcript append only accepts operator text or content blocks"
                .to_string(),
        )),
    }
}

fn run_input_from_admitted_pending_tail(
    messages: &[Message],
    admitted_tail: ObservedSessionTailKind,
) -> Result<RunInput, AgentError> {
    match (admitted_tail, messages.last()) {
        (ObservedSessionTailKind::User, Some(Message::User(user)))
            if user.has_non_text_content() =>
        {
            Ok(RunInput::Content {
                content: ContentInput::Blocks(user.content.clone()),
            })
        }
        (ObservedSessionTailKind::User, Some(Message::User(user))) => Ok(RunInput::Content {
            content: ContentInput::Text(user.text_content()),
        }),
        (ObservedSessionTailKind::ToolResults, Some(Message::ToolResults { .. })) => {
            // The pending tail is staged tool results: that fact travels as
            // its own typed variant — never as a fabricated empty prompt.
            Ok(RunInput::PendingToolResults)
        }
        _ => Err(AgentError::InternalError(format!(
            "generated pending-continuation authority admitted tail {admitted_tail:?}, but transcript tail no longer matches"
        ))),
    }
}

fn dispatcher_knows_tool<T>(dispatcher: &T, name: &str) -> bool
where
    T: AgentToolDispatcher + ?Sized,
{
    if dispatcher.tool_catalog_capabilities().exact_catalog {
        dispatcher
            .tool_catalog()
            .iter()
            .any(|entry| entry.tool.name == name)
    } else {
        dispatcher.tools().iter().any(|tool| tool.name == name)
    }
}

fn precheck_visible_tool_call<T>(
    dispatcher: &T,
    visible_names: &ToolNameSet,
    name: &str,
) -> Result<(), ToolError>
where
    T: AgentToolDispatcher + ?Sized,
{
    if visible_names.contains(name) {
        return Ok(());
    }
    if dispatcher_knows_tool(dispatcher, name) {
        return Err(ToolError::access_denied(name));
    }
    Err(ToolError::not_found(name))
}

/// Typed failure projecting a runtime turn-state handle into an
/// [`crate::AgentExecutionSnapshot`].
///
/// The handle stores wide (`u64`) counters; the snapshot exposes them as
/// `u32`. A counter that does not fit is a genuine projection fault, never a
/// reason to fabricate a missing snapshot (`None`) or a default running state.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum SnapshotProjectionError {
    /// A wide turn-state counter overflowed the snapshot's `u32` field.
    #[error("turn-state counter `{field}` ({value}) does not fit the snapshot u32 projection")]
    CounterOverflow {
        /// Name of the snapshot field whose source counter overflowed.
        field: &'static str,
        /// The source counter value that failed to project.
        value: u64,
    },
}

fn project_counter(field: &'static str, value: u64) -> Result<u32, SnapshotProjectionError> {
    u32::try_from(value).map_err(|_| SnapshotProjectionError::CounterOverflow { field, value })
}

/// Typed failure serializing shared runtime control state into session metadata.
///
/// Projecting the system-context control state or the authorized tool-visibility
/// state into the canonical session metadata map can fail to serialize. That
/// failure must surface as a typed fault, never be laundered into a silent
/// partial snapshot that drops the state while reporting success.
#[derive(Debug, thiserror::Error)]
pub enum SystemContextStateError {
    /// Serializing the system-context control state into session metadata failed.
    #[error("failed to serialize system-context state into session: {0}")]
    SystemContext(#[source] serde_json::Error),
    /// Reading the generated-authority tool visibility projection failed.
    #[error("failed to authorize tool visibility state for session export: {0}")]
    ToolVisibilityAuthorization(#[source] ToolScopeApplyError),
    /// Serializing the authorized tool-visibility state into session metadata failed.
    #[error("failed to serialize tool visibility state into session: {0}")]
    ToolVisibility(#[source] serde_json::Error),
}

fn runtime_execution_snapshot(
    handle: &dyn crate::TurnStateHandle,
    applied_cursor: crate::completion_feed::CompletionSeq,
) -> Result<crate::AgentExecutionSnapshot, SnapshotProjectionError> {
    let snapshot = handle.snapshot();
    let turn_phase = snapshot.turn_phase;
    // Typed handle contract: primitive_kind / terminal_outcome are
    // `Option<TurnPrimitiveKind>` / `Option<TurnTerminalOutcome>`. `None`
    // on the handle means "no primitive / no terminal outcome recorded
    // yet"; collapse to the typed `None` variant for downstream
    // consumers.
    let primitive_kind = snapshot.primitive_kind.unwrap_or(TurnPrimitiveKind::None);
    let terminal_outcome = snapshot
        .terminal_outcome
        .unwrap_or(TurnTerminalOutcome::None);
    let pending_operation_ids = if snapshot.pending_op_refs.is_empty() {
        None
    } else {
        Some(
            snapshot
                .pending_op_refs
                .iter()
                .map(|op_ref| op_ref.operation_id.clone())
                .collect(),
        )
    };
    let barrier_operation_ids = snapshot.barrier_operation_ids.iter().cloned().collect();

    Ok(crate::AgentExecutionSnapshot {
        loop_state: snapshot.loop_state,
        turn_phase,
        turn_terminal: snapshot.turn_terminal,
        active_run_id: snapshot.active_run_id,
        primitive_kind,
        admitted_content_shape: snapshot.admitted_content_shape,
        vision_enabled: snapshot.vision_enabled,
        image_tool_results_enabled: snapshot.image_tool_results_enabled,
        tool_calls_pending: project_counter("tool_calls_pending", snapshot.tool_calls_pending)?,
        pending_operation_ids,
        barrier_operation_ids,
        has_barrier_ops: snapshot.has_barrier_ops,
        barrier_satisfied: snapshot.barrier_satisfied,
        boundary_count: project_counter("boundary_count", snapshot.boundary_count)?,
        cancel_after_boundary: snapshot.cancel_after_boundary,
        terminal_outcome,
        terminal_cause_kind: snapshot.terminal_cause_kind,
        extraction_attempts: project_counter("extraction_attempts", snapshot.extraction_attempts)?,
        max_extraction_retries: project_counter(
            "max_extraction_retries",
            snapshot.max_extraction_retries,
        )?,
        applied_cursor,
    })
}

fn runtime_external_tool_surface_snapshot(
    handle: &dyn crate::ExternalToolSurfaceHandle,
) -> Option<ExternalToolSurfaceSnapshot> {
    let snapshot = handle.diagnostic_snapshot();
    let phase = snapshot.surface_phase;
    let visible_surfaces = snapshot.visible_surfaces;
    let snapshot_epoch = snapshot.snapshot_epoch;
    let snapshot_aligned_epoch = snapshot.snapshot_aligned_epoch;
    let mut entries = Vec::with_capacity(snapshot.entries.len());
    for entry in snapshot.entries {
        entries.push(ExternalToolSurfaceEntrySnapshot {
            visible: visible_surfaces.contains(&entry.surface_id),
            surface_id: entry.surface_id,
            // Typed handle contract: DSL projects a typed enum. `None`
            // means the DSL never recorded a value for this surface, so
            // the projection defaults to `Absent` / `None` per the
            // contract invariants (no state is equivalent to the zero
            // variant).
            base_state: entry
                .base_state
                .unwrap_or(ExternalToolSurfaceBaseState::Absent),
            has_removal_timing: entry.removal_draining_since_ms.is_some()
                || entry.removal_timeout_at_ms.is_some()
                || entry.removal_applied_at_turn.is_some(),
            pending_op: entry.pending_op,
            staged_op: entry.staged_op,
            staged_intent_sequence: entry.staged_intent_sequence.unwrap_or(0),
            pending_task_sequence: entry.pending_task_sequence.unwrap_or(0),
            pending_lineage_sequence: entry.pending_lineage_sequence.unwrap_or(0),
            inflight_call_count: entry.inflight_calls,
            last_delta_operation: entry
                .last_delta_operation
                .unwrap_or(ExternalToolSurfaceDeltaOperation::None),
            last_delta_phase: entry
                .last_delta_phase
                .unwrap_or(ExternalToolSurfaceDeltaPhase::None),
        });
    }
    Some(ExternalToolSurfaceSnapshot {
        phase,
        snapshot_epoch,
        snapshot_aligned_epoch,
        entries,
    })
}

/// Minimal runner interface for an Agent.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AgentRunner: Send {
    async fn run(&mut self, prompt: ContentInput) -> Result<RunResult, AgentError>;

    async fn run_with_events(
        &mut self,
        prompt: ContentInput,
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError>;
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized,
{
    /// Stage an external tool visibility filter update for subsequent turns.
    pub fn stage_external_tool_filter(
        &mut self,
        filter: ToolFilter,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        // Durable visibility intent is machine-owned (or owned by the local
        // fallback owner for standalone builds). ToolScope only routes the
        // staging request and rebuilds the visible-tool projection.
        let handle = self.tool_scope.handle();
        let revision = handle.stage_external_filter(filter)?;
        let _ = handle.staged_revision();
        if self.tool_scope.owns_durable_visibility_projection() {
            let visibility_state =
                self.tool_scope
                    .authorized_visibility_state()
                    .map_err(|err| ToolScopeStageError::Owner {
                        message: err.to_string(),
                    })?;
            self.session
                .set_tool_visibility_state(visibility_state)
                .map_err(|err| ToolScopeStageError::DurableProjectionPersist {
                    message: err.to_string(),
                })?;
            // Only remove the legacy fallback AFTER the canonical write
            // committed — a failed canonical persist must never destroy the
            // legacy recovery source (which would leave both sources gone).
            self.session
                .remove_metadata(EXTERNAL_TOOL_FILTER_METADATA_KEY);
        }
        Ok(revision)
    }

    /// Set or clear a per-turn flow tool overlay.
    pub fn set_flow_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), ToolScopeStageError> {
        let handle = self.tool_scope.handle();
        if let Some(overlay) = overlay {
            let dispatch_context = overlay.dispatch_context;
            let allow = overlay
                .allowed_tools
                .map(|tools| tools.into_iter().collect::<HashSet<_>>());
            let deny = overlay
                .blocked_tools
                .unwrap_or_default()
                .into_iter()
                .collect::<HashSet<_>>();
            handle.set_turn_overlay(allow, deny)?;
            self.turn_tool_dispatch_metadata = dispatch_context;
        } else {
            self.turn_tool_dispatch_metadata.clear();
            handle.clear_turn_overlay()?;
        }
        Ok(())
    }

    pub fn set_runtime_execution_kind(
        &mut self,
        execution_kind: Option<crate::lifecycle::RuntimeExecutionKind>,
    ) {
        self.runtime_execution_kind = execution_kind;
    }

    pub fn set_active_transcript_identity(
        &mut self,
        transcript_identity: Option<TranscriptMessageIdentity>,
    ) {
        self.active_transcript_identity = transcript_identity;
    }

    fn clear_runtime_execution_kind(&mut self) {
        self.runtime_execution_kind = None;
        self.active_transcript_identity = None;
    }

    fn require_runtime_execution_kind(&self) -> Result<(), AgentError> {
        if self.runtime_execution_kind_required && self.runtime_execution_kind.is_none() {
            return Err(AgentError::InternalError(
                "runtime_execution_kind not set: turn-state handle is attached but \
                 the runtime did not stamp RuntimeTurnMetadata.execution_kind"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Apply accumulated session effects from tool dispatch.
    ///
    /// Called by the agent loop after each parallel tool batch completes.
    /// State-mutating effects are applied before `Message::ToolResults`; effects
    /// that append assistant transcript blocks are applied after tool results so
    /// provider tool-call adjacency remains intact.
    ///
    /// Mob authority effects must carry the generated authority seal. The
    /// session `build_state` is a durable projection; the shared
    /// `mob_authority_handle` (if present) is updated from the validated
    /// in-memory effect after the projection write succeeds.
    pub(crate) fn apply_session_effects(
        &mut self,
        effects: &[crate::ops::SessionEffect],
        run_id: Option<&crate::lifecycle::RunId>,
    ) -> Result<(), crate::error::AgentError> {
        use crate::error::AgentError;

        let mut build_state = self.session.build_state().ok_or_else(|| {
            AgentError::InternalError(format!(
                "session {} is missing session build state",
                self.session.id()
            ))
        })?;
        let mut build_state_changed = false;
        let mut visibility_changed = false;
        let mut latest_mob_authority_context = None;

        for effect in effects {
            match effect {
                crate::ops::SessionEffect::ReplaceMobToolAuthorityContext { authority_context } => {
                    if !authority_context.is_generated_authority_context() {
                        return Err(AgentError::InternalError(
                            "refusing to apply mob authority context not minted by generated authority"
                                .to_string(),
                        ));
                    }
                    build_state.mob_tool_authority_context = Some(authority_context.clone());
                    latest_mob_authority_context = Some(authority_context.clone());
                    build_state_changed = true;
                }
                crate::ops::SessionEffect::RequestDeferredTools { authorities } => {
                    self.tool_scope
                        .add_requested_deferred_authorities(authorities)
                        .map_err(|err| {
                            AgentError::InternalError(format!(
                                "failed to record requested deferred tool authorities: {err}"
                            ))
                        })?;
                    visibility_changed = true;
                }
                crate::ops::SessionEffect::AppendAssistantBlocks { blocks } => {
                    let message = crate::types::BlockAssistantMessage::new(
                        blocks.clone(),
                        crate::types::StopReason::EndTurn,
                    );
                    let message = if let Some(run_id) = run_id {
                        let mut message = message;
                        message.identity = self
                            .active_transcript_identity
                            .clone()
                            .unwrap_or_default()
                            .with_run_id(run_id.clone());
                        message
                    } else {
                        message
                    };
                    self.session
                        .push(crate::types::Message::BlockAssistant(message));
                }
            }
        }

        if build_state_changed {
            self.session.set_build_state(build_state).map_err(|e| {
                AgentError::InternalError(format!(
                    "failed to persist session effects into build state: {e}"
                ))
            })?;
        }

        if visibility_changed && let Err(err) = self.publish_committed_visible_set() {
            return Err(AgentError::InternalError(format!(
                "failed to persist session effects into tool visibility state: {err}"
            )));
        }

        // Update the shared effective-authority handle so mob tools in
        // subsequent batches see the widened scope. Serialization drops the
        // generated authority seal, so the handle is updated only from the
        // already-validated in-memory effect.
        if build_state_changed
            && let Some(ref handle) = self.mob_authority_handle
            && let Some(authority) = latest_mob_authority_context
        {
            *handle
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = authority;
        }

        Ok(())
    }

    /// Set the shared mob authority handle for session-effect application.
    ///
    /// The agent updates this handle after merging `SessionEffect`s from tool
    /// dispatch. Mob tools read from it for authorization checks.
    pub fn set_mob_authority_handle(
        &mut self,
        handle: Arc<std::sync::RwLock<crate::service::MobToolAuthorityContext>>,
    ) {
        self.mob_authority_handle = Some(handle);
    }

    /// Replace the LLM client for subsequent turns.
    ///
    /// Enables hot-swapping the model/provider on a live session without
    /// rebuilding the agent. The new client takes effect on the next
    /// `run()` / `run_with_events()` call.
    pub fn replace_client(&mut self, client: Arc<C>) {
        self.client = client;
    }

    /// Apply the live LLM request policy paired with an identity hot-swap.
    pub fn apply_llm_request_policy(&mut self, policy: crate::SessionLlmRequestPolicy) {
        self.config.model = policy.model;
        self.config.provider_params = crate::lifecycle::run_primitive::ProviderParamsCarrier {
            params: policy.provider_params.unwrap_or_default(),
            tool_defaults: policy.provider_tool_defaults,
        };
    }

    /// Replace the LLM client and its next-turn request policy together.
    pub fn replace_client_with_request_policy(
        &mut self,
        client: Arc<C>,
        policy: crate::SessionLlmRequestPolicy,
    ) {
        self.replace_client(client);
        self.apply_llm_request_policy(policy);
    }

    /// Rotate runtime auth-lease tracking alongside a live LLM identity swap.
    pub fn rotate_auth_lease_auth_binding(
        &self,
        previous: Option<&crate::AuthBindingRef>,
        target: Option<&crate::AuthBindingRef>,
    ) -> Result<(), AgentError> {
        let Some(handle) = self.auth_lease_handle.as_deref() else {
            return Ok(());
        };
        if previous == target {
            return Ok(());
        }
        if let Some(previous) = previous {
            let previous_key = crate::handles::LeaseKey::from_auth_binding(previous);
            handle.release_lease(&previous_key).map_err(|err| {
                AgentError::ConfigError(format!(
                    "failed to release previous auth lease {previous_key} during rotation: {err}"
                ))
            })?;
        }
        if let Some(target) = target {
            let target_key = crate::handles::LeaseKey::from_auth_binding(target);
            let target_snapshot = handle.snapshot(&target_key);
            if target_snapshot.credential_present && target_snapshot.phase.is_some() {
                return Ok(());
            }
            handle.acquire_lease(&target_key, u64::MAX).map_err(|err| {
                AgentError::ConfigError(format!(
                    "failed to rotate auth lease to auth_binding {target_key}: {err}"
                ))
            })?;
        }
        Ok(())
    }

    /// Cloneable producer handle for boundary-only cancellation requests.
    ///
    /// Returns a sender on the typed cancel-after-boundary command channel.
    /// The requesting surface sends a [`CancelAfterBoundaryCommand`]; the agent
    /// loop observes it at the next turn boundary.
    pub fn cancel_after_boundary_handle(&self) -> super::CancelAfterBoundarySender {
        self.cancel_after_boundary_tx.clone()
    }

    /// Get the runtime-backed turn-state handle, when this agent was built with one.
    pub fn turn_state_handle(&self) -> Option<Arc<dyn crate::TurnStateHandle>> {
        self.turn_state_handle.clone()
    }

    /// Persist the currently committed visible tool set into canonical session metadata.
    pub(crate) fn publish_committed_visible_set(&mut self) -> Result<(), AgentError> {
        // Session metadata is a durable projection/export of the canonical
        // visibility owner state so checkpoint/recovery stays aligned. The
        // projection only exists when a generated MeerkatMachine authority owns
        // it; standalone builds use a read-only local projection with nothing to
        // persist, so this is a no-op there (not a durable-write fault). When
        // the projection IS owned, a write failure is a genuine fault that must
        // propagate — never be swallowed while a boundary-applied success is
        // reported, which would diverge the recovery source from in-memory
        // authority.
        if !self.tool_scope.owns_durable_visibility_projection() {
            return Ok(());
        }
        let authorized_visibility_state =
            self.tool_scope
                .authorized_visibility_state()
                .map_err(|err| {
                    AgentError::InternalError(format!(
                        "failed to authorize canonical tool visibility state: {err}"
                    ))
                })?;
        self.session
            .set_tool_visibility_state(authorized_visibility_state)
            .map_err(|err| {
                AgentError::InternalError(format!(
                    "failed to persist canonical tool visibility state: {err}"
                ))
            })
    }

    /// Dispatch one external tool call through the canonical tool dispatcher.
    ///
    /// This reuses the same visibility owner and session-effect application path
    /// as ordinary LLM-driven tool batches, but without synthesizing a full turn.
    pub async fn dispatch_external_tool_call(
        &mut self,
        call: crate::types::ToolCall,
    ) -> Result<ToolDispatchOutcome, AgentError> {
        self.dispatch_external_tool_call_with_timeout_policy(
            call,
            ToolDispatchTimeoutPolicy::Disabled,
        )
        .await
    }

    /// Dispatch an external product/runtime tool call with an optional
    /// caller-owned timeout. Timeout terminalization is still canonical:
    /// timeout expiry becomes `ToolError::Timeout`, then flows through
    /// `terminal_tool_outcome_for_error` like normal tool execution failures.
    pub async fn dispatch_external_tool_call_with_timeout_policy(
        &mut self,
        call: crate::types::ToolCall,
        timeout_policy: ToolDispatchTimeoutPolicy,
    ) -> Result<ToolDispatchOutcome, AgentError> {
        let visible_tool_names = self
            .tool_scope
            .visible_tool_names()
            .map_err(|err| AgentError::InternalError(err.to_string()))?
            .into_iter()
            .collect::<ToolNameSet>();
        if let Err(error) =
            precheck_visible_tool_call(self.tools.as_ref(), &visible_tool_names, call.name.as_str())
        {
            return Ok(crate::ops::terminal_tool_outcome_for_error(call.id, error));
        }
        let args = to_raw_value(&call.args).map_err(|err| {
            AgentError::InternalError(format!(
                "failed to serialize external tool-call arguments: {err}"
            ))
        })?;
        let view = ToolCallView {
            id: &call.id,
            name: &call.name,
            args: args.as_ref(),
        };
        let dispatch_context = self.tool_dispatch_context.clone();
        let dispatch_result = match timeout_policy.timeout() {
            Some(timeout) => {
                match tokio::time::timeout(
                    timeout,
                    self.tools.dispatch_with_context(view, &dispatch_context),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(crate::error::ToolError::timeout(
                        call.name.clone(),
                        timeout_policy.timeout_ms().unwrap_or(u64::MAX),
                    )),
                }
            }
            None => {
                self.tools
                    .dispatch_with_context(view, &dispatch_context)
                    .await
            }
        };

        match dispatch_result {
            Ok(mut outcome) => {
                outcome.clear_terminal_cause();
                if outcome.result.tool_use_id.is_empty() {
                    outcome.result.tool_use_id = call.id;
                }
                if !outcome.session_effects.is_empty() {
                    self.apply_session_effects(&outcome.session_effects, None)?;
                }
                Ok(outcome)
            }
            Err(crate::error::ToolError::CallbackPending { tool_name, args }) => {
                Err(AgentError::CallbackPending { tool_name, args })
            }
            Err(error) => Ok(crate::ops::terminal_tool_outcome_for_error(call.id, error)),
        }
    }

    #[cfg(test)]
    pub(crate) fn inject_tool_scope_boundary_failure_once_for_test(&self) {
        self.tool_scope.inject_boundary_failure_once_for_test();
    }
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Create a new agent builder
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// Get the current session
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Get mutable access to the session (for setting metadata)
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Get the current budget
    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    /// Get the current loop state.
    ///
    /// Returns the snapshotted [`LoopState`] when a runtime-backed turn-state
    /// handle is attached, `Ok(LoopState::CallingLlm)` when the agent runs
    /// standalone (no handle, so no machine-owned loop state to project), and a
    /// typed [`SnapshotProjectionError`] when the handle is present but its
    /// counters cannot be projected. A projection fault is never laundered into
    /// a fabricated running state.
    pub fn state(&self) -> Result<LoopState, SnapshotProjectionError> {
        match self.execution_snapshot()? {
            Some(snapshot) => Ok(snapshot.loop_state),
            // No runtime handle attached: standalone/ephemeral execution has no
            // machine-owned loop state, so report the default entry state. This
            // is a genuine "no handle" case, distinct from a projection failure.
            None => Ok(LoopState::CallingLlm),
        }
    }

    /// Snapshot the agent's live execution state for diagnostics and mapping.
    ///
    /// Returns `Ok(None)` when the agent has no runtime-backed turn-state
    /// handle attached (standalone/ephemeral execution paths), and a typed
    /// [`SnapshotProjectionError`] when the handle is present but a turn-state
    /// counter overflows the snapshot projection.
    pub fn execution_snapshot(
        &self,
    ) -> Result<Option<crate::AgentExecutionSnapshot>, SnapshotProjectionError> {
        let Some(handle) = self.turn_state_handle.as_deref() else {
            return Ok(None);
        };
        runtime_execution_snapshot(handle, self.applied_cursor).map(Some)
    }

    /// Snapshot the agent's live tool-scope state for diagnostics and mapping.
    pub fn tool_scope_snapshot(&self) -> Option<crate::ToolScopeSnapshot> {
        let mut snapshot = self.tool_scope.snapshot()?;
        let capability_filter =
            crate::ToolScope::compose(&[self.client.active_capability_base_filter()]);
        snapshot
            .visible_names
            .retain(|name| capability_filter.allows(name.as_str()));
        snapshot.capability_base_filter = self.client.active_capability_base_filter();
        Some(snapshot)
    }

    /// Snapshot the provider-visible tool definitions for the active LLM model.
    pub fn visible_tool_defs(&self) -> Vec<crate::ToolDef> {
        let capability_filter =
            crate::ToolScope::compose(&[self.client.active_capability_base_filter()]);
        self.tool_scope
            .visible_tools()
            .iter()
            .filter(|tool| capability_filter.allows(tool.name.as_str()))
            .map(|tool| tool.as_ref().clone())
            .collect()
    }

    /// Snapshot the live external tool-surface state, if supported by the dispatcher chain.
    pub fn external_tool_surface_snapshot(&self) -> Option<crate::ExternalToolSurfaceSnapshot> {
        if let Some(handle) = self.external_tool_surface_handle.as_deref() {
            if let Some(snapshot) = runtime_external_tool_surface_snapshot(handle) {
                return Some(snapshot);
            }
            tracing::warn!(
                "failed to convert runtime external-tool-surface snapshot; falling back to dispatcher snapshot"
            );
        }
        self.tools.external_tool_surface_snapshot()
    }

    /// Get the retry policy
    pub fn retry_policy(&self) -> &RetryPolicy {
        &self.retry_policy
    }

    /// Get the current nesting depth
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// Get the event tap for interaction-scoped streaming.
    pub fn event_tap(&self) -> &crate::event_tap::EventTap {
        &self.event_tap
    }

    /// Access the live tool-scope projection bridge.
    pub fn tool_scope(&self) -> &crate::ToolScope {
        &self.tool_scope
    }

    /// Get shared runtime system-context control state.
    pub fn system_context_state(&self) -> crate::session::SystemContextStateHandle {
        crate::session::SystemContextStateHandle::from_shared_authority_state(Arc::clone(
            &self.system_context_state,
        ))
    }

    /// Clone the current session with the latest shared system-context state merged into metadata.
    ///
    /// A serialize failure projecting either the system-context control state or
    /// the authorized tool-visibility state into session metadata is a typed
    /// fault, not a silent partial snapshot: it propagates as
    /// [`SystemContextStateError`] rather than being laundered through a
    /// `tracing::warn!` that would hand back a session missing the state.
    pub fn session_with_system_context_state(&self) -> Result<Session, SystemContextStateError> {
        let mut session = self.session.clone();
        let state = self.system_context_state().snapshot();
        session
            .set_system_context_state(state)
            .map_err(SystemContextStateError::SystemContext)?;
        if self.tool_scope.owns_durable_visibility_projection() {
            let visibility_state = self
                .tool_scope
                .authorized_visibility_state()
                .map_err(SystemContextStateError::ToolVisibilityAuthorization)?;
            session
                .set_tool_visibility_state(visibility_state)
                .map_err(SystemContextStateError::ToolVisibility)?;
        }
        Ok(session)
    }

    /// Synchronize the shared system-context state into the in-memory session metadata.
    ///
    /// A serialize failure is a typed fault, not a silent partial sync: it
    /// propagates as [`SystemContextStateError`] rather than being laundered
    /// through a `tracing::warn!` that would leave canonical session metadata
    /// stale while reporting success to the caller.
    #[doc(hidden)]
    pub fn sync_system_context_state_to_session(&mut self) -> Result<(), SystemContextStateError> {
        let state = self.system_context_state().snapshot();
        self.session
            .set_system_context_state(state)
            .map_err(SystemContextStateError::SystemContext)
    }

    /// Consume all pending system-context appends for the next LLM boundary.
    ///
    /// The returned appends are intended for transient request composition only;
    /// they must not be written back into the canonical session prompt.
    pub(crate) fn take_pending_system_context_boundary(
        &mut self,
    ) -> Result<Vec<PendingSystemContextAppend>, SystemContextStateError> {
        let pending = {
            let mut state = match self.system_context_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("system-context state lock poisoned while applying boundary");
                    poisoned.into_inner()
                }
            };
            if state.pending().is_empty() {
                return Ok(Vec::new());
            }
            let pending = state.pending().to_vec();
            state.mark_pending_applied();
            pending
        };

        if !pending.is_empty() {
            tracing::debug!(
                pending_count = pending.len(),
                "applying pending runtime system context at model boundary"
            );
        }
        self.sync_system_context_state_to_session()?;
        Ok(pending)
    }

    pub(crate) fn llm_messages_with_runtime_system_context(
        &self,
        appends: &[PendingSystemContextAppend],
    ) -> Vec<Message> {
        if appends.is_empty() {
            return self.session.messages().to_vec();
        }

        let mut session = self.session.clone();
        session.append_system_context_blocks(appends);
        session.messages().to_vec()
    }

    /// Persist the current session through the configured checkpointer after syncing control state.
    ///
    /// A serialize failure syncing system-context state surfaces as a typed
    /// [`AgentError::InternalError`] rather than being swallowed, so the
    /// checkpoint never persists a session whose control-state projection
    /// silently failed.
    #[doc(hidden)]
    pub async fn checkpoint_current_session(&mut self) -> Result<(), AgentError> {
        self.sync_system_context_state_to_session()
            .map_err(|err| AgentError::InternalError(err.to_string()))?;
        if let Some(ref cp) = self.checkpointer {
            cp.checkpoint(&self.session).await;
        }
        Ok(())
    }

    async fn run_started_hooks(
        &self,
        input: &RunInput,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<(), AgentError> {
        let report = self
            .execute_hooks(
                HookInvocation::run_started(self.session.id().clone(), input.clone()),
                event_tx,
            )
            .await?;

        if let Some(error) = report.denial_error(HookPoint::RunStarted) {
            return Err(error);
        }
        Ok(())
    }

    pub(super) async fn run_completed_hooks(
        &mut self,
        result: &mut RunResult,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<(), AgentError> {
        let report = self
            .execute_hooks(
                HookInvocation::run_completed(self.session.id().clone(), result.turns),
                event_tx,
            )
            .await?;

        if let Some(error) = report.denial_error(HookPoint::RunCompleted) {
            return Err(error);
        }

        self.run_completed_hooks_applied = true;
        Ok(())
    }

    pub(super) async fn emit_run_completed_event(
        &self,
        result: &RunResult,
        extraction_required: bool,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            event_tx,
            AgentEvent::RunCompleted {
                session_id: self.session.id().clone(),
                result: result.text.clone(),
                structured_output: result.structured_output.clone(),
                extraction_required,
                usage: result.usage.clone(),
                terminal_cause_kind: result.terminal_cause_kind,
            },
        )
        .await;
    }

    pub(super) async fn emit_extraction_succeeded_event(
        &self,
        structured_output: serde_json::Value,
        schema_warnings: Option<Vec<crate::schema::SchemaWarning>>,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            event_tx,
            AgentEvent::ExtractionSucceeded {
                session_id: self.session.id().clone(),
                structured_output,
                schema_warnings,
            },
        )
        .await;
    }

    pub(super) async fn emit_extraction_failed_event(
        &self,
        error: &crate::types::ExtractionError,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            event_tx,
            AgentEvent::ExtractionFailed {
                session_id: self.session.id().clone(),
                last_output: error.last_output.clone(),
                attempts: error.attempts,
                reason: error.reason.clone(),
            },
        )
        .await;
    }

    async fn emit_run_started_event(
        &self,
        input: RunInput,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            event_tx,
            AgentEvent::RunStarted {
                session_id: self.session.id().clone(),
                input,
            },
        )
        .await;
    }

    async fn emit_run_failed_event(
        &self,
        error: &AgentError,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        let error_report = crate::event::AgentErrorReport::from_agent_error(error);
        let terminal_cause_kind = match error {
            AgentError::TerminalFailure { cause_kind, .. }
                if cause_kind.is_specific_failure_cause() =>
            {
                Some(*cause_kind)
            }
            _ => match self.execution_snapshot() {
                Ok(snapshot) => snapshot
                    .and_then(|snapshot| snapshot.terminal_cause_kind)
                    .filter(|cause_kind| *cause_kind != TurnTerminalCauseKind::Unknown),
                Err(err) => {
                    // The run is already terminalizing into a failure event; a
                    // snapshot projection fault here only means we cannot
                    // recover a more specific terminal cause kind. Surface the
                    // typed fault rather than laundering it, but still emit the
                    // failure event with no derived cause kind.
                    tracing::warn!(
                        error = %err,
                        "failed to project execution snapshot while emitting run-failed event"
                    );
                    None
                }
            },
        };
        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            event_tx,
            AgentEvent::RunFailed {
                session_id: self.session.id().clone(),
                error_report,
                terminal_cause_kind,
            },
        )
        .await;
    }

    async fn handle_run_failure(
        &self,
        error: &AgentError,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        if let Err(hook_err) = self.run_failed_hooks(error, event_tx).await {
            tracing::warn!(?hook_err, "run_failed hook execution failed");
        }
        self.emit_run_failed_event(error, event_tx).await;
    }

    async fn run_failed_hooks(
        &self,
        error: &AgentError,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<(), AgentError> {
        let report = self
            .execute_hooks(
                HookInvocation::run_failed(self.session.id().clone(), error),
                event_tx,
            )
            .await?;

        if let Some(error) = report.denial_error(HookPoint::RunFailed) {
            return Err(error);
        }
        Ok(())
    }

    /// Run the agent with a user message.
    pub async fn run(&mut self, user_input: ContentInput) -> Result<RunResult, AgentError> {
        self.run_inner(user_input, Vec::new(), Vec::new(), None, None)
            .await
    }

    /// Run the agent with events streamed to the provided channel.
    pub async fn run_with_events(
        &mut self,
        user_input: ContentInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        self.run_inner(user_input, Vec::new(), Vec::new(), None, Some(event_tx))
            .await
    }

    /// Run the agent with provider-facing prompt content derived from typed
    /// runtime appends, while persisting those appends according to their roles.
    ///
    /// `injected_context` carries host-attached ambient context for the direct
    /// (non-runtime) path: each entry materializes as a separate typed
    /// injected-context user-channel message immediately before the turn's
    /// user message. When `typed_turn_appends` is non-empty the runtime
    /// authored every transcript append for this turn (injected context
    /// arrives as `InjectedContext`-role appends), so passing both is a
    /// caller error — exactly one lowering owner per mode.
    pub async fn run_with_events_and_typed_turn_appends(
        &mut self,
        user_input: ContentInput,
        typed_turn_appends: Vec<ConversationAppend>,
        injected_context: Vec<ContentInput>,
        transcript_identity: Option<TranscriptMessageIdentity>,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        self.run_inner(
            user_input,
            typed_turn_appends,
            injected_context,
            transcript_identity,
            Some(event_tx),
        )
        .await
    }

    fn stamp_user_message_identity(&self, mut message: UserMessage) -> UserMessage {
        if let Some(identity) = self.active_transcript_identity.as_ref() {
            message.identity = identity.clone();
        }
        message
    }

    pub(super) fn stamp_assistant_message_identity(
        &self,
        mut message: BlockAssistantMessage,
        run_id: &crate::lifecycle::RunId,
    ) -> BlockAssistantMessage {
        message.identity = self
            .active_transcript_identity
            .clone()
            .unwrap_or_default()
            .with_run_id(run_id.clone());
        message
    }

    /// Run the agent using the pending continuation boundary already in the session.
    ///
    /// This is useful when the session has been pre-populated with a continuation
    /// boundary (for example a deferred first-turn user message or staged callback
    /// tool results). Unlike `run()`, this method does NOT add a new user message;
    /// it runs directly from the session's current state.
    ///
    /// Returns `NoPendingBoundary` when generated pending-continuation
    /// authority does not admit the current transcript tail.
    pub async fn run_pending(&mut self) -> Result<RunResult, AgentError> {
        self.run_pending_inner(None).await
    }

    /// Run the agent using the pending continuation boundary, with event streaming.
    ///
    /// Like `run_pending()`, but emits events to the provided channel.
    pub async fn run_pending_with_events(
        &mut self,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        self.run_pending_inner(Some(event_tx)).await
    }

    fn push_transcript_append(&mut self, append: ConversationAppend) -> Result<(), AgentError> {
        match append.role {
            ConversationAppendRole::User => {
                let message = self.stamp_user_message_identity(
                    user_message_from_operator_renderable(append.content)?,
                );
                self.session.push(Message::User(message));
            }
            ConversationAppendRole::SystemNotice => {
                let notice = match append.content {
                    CoreRenderable::SystemNotice { kind, body, blocks } => {
                        crate::types::SystemNoticeMessage::with_blocks(kind, body, blocks)
                    }
                    CoreRenderable::Text { text } => crate::types::SystemNoticeMessage::with_block(
                        crate::types::SystemNoticeKind::Generic,
                        Some(text.clone()),
                        crate::types::SystemNoticeBlock::RuntimeNotice {
                            category: "runtime_notice".to_string(),
                            detail: Some(text),
                            payload: None,
                        },
                    ),
                    CoreRenderable::Blocks { blocks } => {
                        crate::types::SystemNoticeMessage::with_block(
                            crate::types::SystemNoticeKind::Generic,
                            None,
                            crate::types::SystemNoticeBlock::RuntimeNotice {
                                category: "runtime_notice".to_string(),
                                detail: Some(crate::types::text_content(&blocks)),
                                payload: None,
                            },
                        )
                    }
                    CoreRenderable::Json { value } => {
                        crate::types::SystemNoticeMessage::with_block(
                            crate::types::SystemNoticeKind::Generic,
                            None,
                            crate::types::SystemNoticeBlock::RuntimeNotice {
                                category: "runtime_notice".to_string(),
                                detail: None,
                                payload: Some(value),
                            },
                        )
                    }
                    CoreRenderable::Reference { uri, label } => {
                        crate::types::SystemNoticeMessage::with_block(
                            crate::types::SystemNoticeKind::Generic,
                            label,
                            crate::types::SystemNoticeBlock::RuntimeNotice {
                                category: "runtime_notice".to_string(),
                                detail: Some(uri),
                                payload: None,
                            },
                        )
                    }
                };
                self.session.push(Message::SystemNotice(notice));
            }
            ConversationAppendRole::InjectedContext => {
                let message = self.stamp_user_message_identity(
                    injected_context_message_from_operator_renderable(append.content)?,
                );
                self.session.push(Message::User(message));
            }
            ConversationAppendRole::Assistant | ConversationAppendRole::Tool => {
                return Err(AgentError::ConfigError(
                    "runtime transcript append role is not supported for turn start".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Core run implementation shared by `run()` and `run_with_events()`.
    ///
    /// Adds user_input as a user message, emits lifecycle events when `event_tx`
    /// is provided, and delegates to `run_loop`.
    async fn run_inner(
        &mut self,
        user_input: ContentInput,
        typed_turn_appends: Vec<ConversationAppend>,
        injected_context: Vec<ContentInput>,
        transcript_identity: Option<TranscriptMessageIdentity>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<RunResult, AgentError> {
        let event_tx = event_tx.or_else(|| self.default_event_tx.clone());
        self.set_active_transcript_identity(transcript_identity);

        // Exactly one lowering owner per mode: when the runtime authored the
        // turn's typed appends, injected context must arrive as
        // `InjectedContext`-role appends — a separate direct-path carrier here
        // would double-append or silently drop. Fail closed instead.
        if !typed_turn_appends.is_empty() && !injected_context.is_empty() {
            self.clear_runtime_execution_kind();
            return Err(AgentError::ConfigError(
                "injected context must be lowered into typed turn appends when the runtime \
                 authors the turn transcript"
                    .to_string(),
            ));
        }

        if let Err(err) = self.require_runtime_execution_kind() {
            self.clear_runtime_execution_kind();
            return Err(err);
        }

        // Reset state for new run (allows multi-turn on same agent).
        self.extraction_state.reset();
        self.terminal_error_detail = None;
        self.run_completed_hooks_applied = false;
        self.run_completed_event_emitted = false;

        // Apply canonical per-turn skill references staged by the surface.
        // Resolved activations travel as typed `SkillContext` blocks prepended
        // to the turn input, preserving activation provenance in the durable
        // transcript instead of folding skill bodies into operator text.
        let skill_blocks = match self.resolve_pending_skill_context(event_tx.as_ref()).await {
            Ok(blocks) => blocks,
            Err(err) => {
                self.handle_run_failure(&err, event_tx.as_ref()).await;
                self.clear_runtime_execution_kind();
                return Err(err);
            }
        };
        let user_input = if skill_blocks.is_empty() {
            user_input
        } else {
            let mut blocks = skill_blocks;
            match user_input {
                ContentInput::Text(text) if text.is_empty() => {}
                other => blocks.extend(other.into_blocks()),
            }
            ContentInput::Blocks(blocks)
        };

        // Hooks/events receive the typed run input; the external hook wire
        // envelope derives its text projection at serialization time only.
        let run_prompt_input = RunInput::from(user_input.clone());

        // Run-start hooks own the start veto. They must run — and be able to
        // deny — BEFORE we publish `RunStarted` or commit the user message to
        // the transcript. Otherwise observers see a `RunStarted` for a run that
        // immediately fails at run-start (a false-start window), and the denied
        // run leaves a stray user message in the session. On denial we emit a
        // terminal-failure event with no preceding `RunStarted`.
        if let Err(err) = self
            .run_started_hooks(&run_prompt_input, event_tx.as_ref())
            .await
        {
            self.handle_run_failure(&err, event_tx.as_ref()).await;
            self.clear_runtime_execution_kind();
            return Err(err);
        }

        if typed_turn_appends.is_empty() {
            // Materialize host-attached injected context as separate typed
            // user-channel messages immediately BEFORE the turn's user
            // message, in delivery order. The typed slot the content arrived
            // in mints the transcript role — never free-form role strings.
            for entry in injected_context {
                let injected_message = if entry.has_non_text_content() {
                    crate::types::UserMessage::injected_context_with_blocks(entry.into_blocks())
                } else {
                    crate::types::UserMessage::injected_context(entry.text_content())
                };
                let injected_message = self.stamp_user_message_identity(injected_message);
                self.session.push(Message::User(injected_message));
            }
            // Add user message — preserve image blocks when present.
            let user_message = if user_input.has_non_text_content() {
                crate::types::UserMessage::with_blocks(user_input.into_blocks())
            } else {
                crate::types::UserMessage::text(user_input.text_content())
            };
            let user_message = self.stamp_user_message_identity(user_message);
            self.session.push(Message::User(user_message));
        } else {
            for append in typed_turn_appends {
                // Project the peer-ingestion observe fact BEFORE the append is
                // consumed: one PeerContentIngested event per incoming comms
                // block the transcript commit carries (queued peer deliveries).
                let ingested_events = crate::event::peer_content_ingested_events(&append.content);
                if let Err(err) = self.push_transcript_append(append) {
                    self.clear_runtime_execution_kind();
                    return Err(err);
                }
                for event in ingested_events {
                    let _ =
                        crate::event_tap::tap_emit(&self.event_tap, event_tx.as_ref(), event).await;
                }
            }
        }

        self.emit_run_started_event(run_prompt_input.clone(), event_tx.as_ref())
            .await;

        self.tool_dispatch_context = crate::ToolDispatchContext::from_run_input(&run_prompt_input)
            .with_turn_metadata(self.turn_tool_dispatch_metadata.clone());
        let loop_result = self.run_loop(event_tx.clone()).await;
        self.tool_dispatch_context = crate::ToolDispatchContext::default();

        match loop_result {
            Ok(mut result) => {
                if !self.run_completed_hooks_applied
                    && let Err(err) = self
                        .run_completed_hooks(&mut result, event_tx.as_ref())
                        .await
                {
                    self.handle_run_failure(&err, event_tx.as_ref()).await;
                    self.clear_runtime_execution_kind();
                    return Err(err);
                }
                if !self.run_completed_event_emitted {
                    self.emit_run_completed_event(&result, false, event_tx.as_ref())
                        .await;
                    self.run_completed_event_emitted = true;
                }
                if let Err(err) = self.checkpoint_current_session().await {
                    self.handle_run_failure(&err, event_tx.as_ref()).await;
                    self.clear_runtime_execution_kind();
                    return Err(err);
                }
                self.clear_runtime_execution_kind();
                Ok(result)
            }
            Err(err) => {
                self.handle_run_failure(&err, event_tx.as_ref()).await;
                self.clear_runtime_execution_kind();
                Err(err)
            }
        }
    }

    /// Core run-pending implementation shared by `run_pending()` and
    /// `run_pending_with_events()`.
    ///
    /// Uses the existing pending continuation boundary in the session (does NOT
    /// push a new user message). Emits lifecycle events when `event_tx` is
    /// provided. Also used by continuation paths after response injection or
    /// staged callback tool-result admission.
    pub(super) async fn run_pending_inner(
        &mut self,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<RunResult, AgentError> {
        let event_tx = event_tx.or_else(|| self.default_event_tx.clone());

        let session_tail = observe_session_tail(self.session.messages());
        let pending_resolution = match resolve_pending_continuation(session_tail, 0) {
            Ok(resolution) => resolution,
            Err(error) => {
                self.clear_runtime_execution_kind();
                return Err(AgentError::InternalError(format!(
                    "generated pending-continuation authority rejected run_pending: {error}"
                )));
            }
        };
        let prompt = match pending_resolution.disposition {
            PendingContinuationDisposition::RunPending => {
                if let Some(terminal) = pending_resolution.public_terminal {
                    self.clear_runtime_execution_kind();
                    return Err(AgentError::InternalError(format!(
                        "generated pending-continuation authority emitted terminal {terminal:?} for runnable continuation"
                    )));
                }
                match run_input_from_admitted_pending_tail(self.session.messages(), session_tail) {
                    Ok(prompt) => prompt,
                    Err(error) => {
                        self.clear_runtime_execution_kind();
                        return Err(error);
                    }
                }
            }
            PendingContinuationDisposition::NoPendingBoundary => {
                self.clear_runtime_execution_kind();
                return if pending_resolution.public_terminal
                    == Some(PendingContinuationPublicTerminal::NoPendingBoundary)
                {
                    Err(AgentError::NoPendingBoundary)
                } else {
                    Err(AgentError::InternalError(
                        "generated pending-continuation authority omitted NoPendingBoundary terminal witness".to_string(),
                    ))
                };
            }
        };

        if let Err(err) = self.require_runtime_execution_kind() {
            self.clear_runtime_execution_kind();
            return Err(err);
        }

        // Reset state for new run (allows multi-turn on same agent).
        self.extraction_state.reset();
        self.terminal_error_detail = None;
        self.run_completed_hooks_applied = false;
        self.run_completed_event_emitted = false;

        // Run-start hooks own the start veto on the pending-continuation path
        // too: they must run — and be able to deny — BEFORE we publish
        // `RunStarted`, mirroring `run_inner`. Otherwise observers see a
        // `RunStarted` for a run that immediately fails at run-start (a
        // false-start window).
        if let Err(err) = self.run_started_hooks(&prompt, event_tx.as_ref()).await {
            self.handle_run_failure(&err, event_tx.as_ref()).await;
            self.clear_runtime_execution_kind();
            return Err(err);
        }

        self.emit_run_started_event(prompt.clone(), event_tx.as_ref())
            .await;

        self.tool_dispatch_context = crate::ToolDispatchContext::from_run_input(&prompt)
            .with_turn_metadata(self.turn_tool_dispatch_metadata.clone());
        let loop_result = self.run_loop(event_tx.clone()).await;
        self.tool_dispatch_context = crate::ToolDispatchContext::default();

        match loop_result {
            Ok(mut result) => {
                if !self.run_completed_hooks_applied
                    && let Err(err) = self
                        .run_completed_hooks(&mut result, event_tx.as_ref())
                        .await
                {
                    self.handle_run_failure(&err, event_tx.as_ref()).await;
                    self.clear_runtime_execution_kind();
                    return Err(err);
                }
                if !self.run_completed_event_emitted {
                    self.emit_run_completed_event(&result, false, event_tx.as_ref())
                        .await;
                    self.run_completed_event_emitted = true;
                }
                if let Err(err) = self.checkpoint_current_session().await {
                    self.handle_run_failure(&err, event_tx.as_ref()).await;
                    self.clear_runtime_execution_kind();
                    return Err(err);
                }
                self.clear_runtime_execution_kind();
                Ok(result)
            }
            Err(err) => {
                self.handle_run_failure(&err, event_tx.as_ref()).await;
                self.clear_runtime_execution_kind();
                Err(err)
            }
        }
    }

    /// Cancel the current run
    pub fn cancel(&mut self) {
        use crate::turn_execution_authority::TurnExecutionInput;

        self.clear_runtime_execution_kind();
        let snapshot = self
            .turn_state_handle
            .as_deref()
            .map(crate::handles::TurnStateHandle::snapshot);
        let input = match snapshot.and_then(|s| s.active_run_id) {
            Some(run_id) => TurnExecutionInput::CancelNow { run_id },
            None => TurnExecutionInput::ForceCancelNoRun,
        };
        let _ = self.apply_turn_input(input);
    }

    /// Consume canonical pending `skill_references` staged by the surface and
    /// resolve them into typed [`ContentBlock::SkillContext`] blocks for the
    /// next user input.
    ///
    /// Per-turn skill activation is a typed operational effect, not operator
    /// text: a successful resolution emits the typed
    /// [`AgentEvent::SkillsResolved`] (carrying the canonical [`SkillKey`]s and
    /// injection byte count) AND yields typed skill-context blocks that the
    /// durable transcript preserves with their activation provenance — the
    /// rendered body is never folded into anonymous operator text. A failed
    /// resolution emits the typed [`AgentEvent::SkillResolutionFailed`]
    /// (carrying the typed `SkillKey` and typed
    /// [`SkillResolutionFailureReason`]) rather than being swallowed by a log
    /// line.
    ///
    /// Compatibility slash refs are handled at transport/resolver boundaries;
    /// core runtime no longer parses slash refs directly.
    ///
    /// [`SkillKey`]: crate::skills::SkillKey
    /// [`SkillResolutionFailureReason`]: crate::event::SkillResolutionFailureReason
    async fn resolve_pending_skill_context(
        &mut self,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<Vec<crate::types::ContentBlock>, AgentError> {
        let engine = match &self.skill_engine {
            Some(e) => e.clone(),
            None => return Ok(Vec::new()),
        };

        let mut skill_blocks: Vec<crate::types::ContentBlock> = Vec::new();

        // Consume pending_skill_references (from wire format / API)
        if let Some(refs) = self.pending_skill_references.take()
            && !refs.is_empty()
        {
            let canonical_keys: Vec<crate::skills::SkillKey> = refs.into_iter().collect();
            match engine.resolve_and_render(&canonical_keys).await {
                Ok(resolved) => {
                    // Typed activation effect: resolved skills are observable as
                    // a structured event carrying the canonical keys, distinct
                    // from any operator-typed text. The rendered bodies travel
                    // as typed skill-context blocks, not folded prompt text.
                    let mut injection_bytes = 0usize;
                    let mut activated_keys: Vec<crate::skills::SkillKey> =
                        Vec::with_capacity(resolved.len());
                    for skill in &resolved {
                        tracing::info!(
                            skill_key = %skill.key,
                            "Per-turn skill activation via skill_references"
                        );
                        injection_bytes = injection_bytes.saturating_add(skill.byte_size);
                        activated_keys.push(skill.key.clone());
                        skill_blocks.push(crate::types::ContentBlock::SkillContext {
                            skill_key: skill.key.clone(),
                            text: skill.rendered_body.clone(),
                        });
                    }
                    if !activated_keys.is_empty() {
                        let _ = crate::event_tap::tap_emit(
                            &self.event_tap,
                            event_tx,
                            AgentEvent::SkillsResolved {
                                skills: activated_keys,
                                injection_bytes,
                            },
                        )
                        .await;
                    }
                }
                Err(e) => {
                    // Fail-explicit: emit the typed resolution failure carrying
                    // the typed reason and the canonical key we attempted to
                    // resolve, instead of swallowing the signal in a log line.
                    let reason = crate::event::SkillResolutionFailureReason::from_skill_error(&e);
                    let skill_key = canonical_keys.first().cloned();
                    tracing::warn!(
                        error = %e,
                        "Failed to resolve source-pinned skill_references"
                    );
                    let _ = crate::event_tap::tap_emit(
                        &self.event_tap,
                        event_tx,
                        AgentEvent::SkillResolutionFailed {
                            skill_key: skill_key.clone(),
                            reason: reason.clone(),
                        },
                    )
                    .await;
                    return Err(AgentError::SkillResolutionFailed {
                        skill_key,
                        reason: Box::new(reason),
                    });
                }
            }
        }

        Ok(skill_blocks)
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod typed_transcript_contract_tests {
    use super::*;

    #[test]
    fn user_role_accepts_only_operator_text_or_blocks() {
        assert!(
            user_message_from_operator_renderable(CoreRenderable::Text {
                text: "hello".to_string(),
            })
            .is_ok()
        );
        assert!(
            user_message_from_operator_renderable(CoreRenderable::Blocks {
                blocks: vec![crate::types::ContentBlock::Text {
                    text: "hello".to_string(),
                }],
            })
            .is_ok()
        );
    }

    #[test]
    fn user_role_rejects_runtime_authored_renderables() {
        for content in [
            CoreRenderable::SystemNotice {
                kind: crate::types::SystemNoticeKind::Comms,
                body: Some("runtime".to_string()),
                blocks: Vec::new(),
            },
            CoreRenderable::Json {
                value: serde_json::json!({"runtime": true}),
            },
            CoreRenderable::Reference {
                uri: "artifact://runtime".to_string(),
                label: Some("runtime".to_string()),
            },
        ] {
            let err = match user_message_from_operator_renderable(content) {
                Ok(message) => {
                    panic!("runtime renderable must not become user text: {message:?}")
                }
                Err(err) => err,
            };
            assert!(
                matches!(err, AgentError::ConfigError(ref message) if message.contains("role=user")),
                "unexpected error: {err:?}"
            );
        }
    }
}

/// Gate tests for per-turn skill activation as a typed operational effect.
///
/// Row #65: a failing per-turn `skill_references` resolution must emit the
/// typed [`AgentEvent::SkillResolutionFailed`] carrying the typed `SkillKey`
/// and typed [`crate::event::SkillResolutionFailureReason`], not just a
/// `tracing::warn` log line (the old behavior swallowed the signal after
/// consuming the refs).
///
/// Row #84: a successful per-turn activation must produce a typed activation
/// record — the typed [`AgentEvent::SkillsResolved`] carrying the canonical
/// `SkillKey`s — distinct from any operator-typed text smuggled into the user
/// input.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod skill_activation_effect_tests {
    use super::*;
    use crate::skills::{
        ResolvedSkill, SkillCollection, SkillDescriptor, SkillEngine, SkillError, SkillFilter,
        SkillKey, SkillName, SkillRuntime, SourceUuid,
    };
    use crate::types::{AssistantBlock, StopReason, ToolDef, Usage};
    use std::future::Future;

    fn fixture_skill_key(name: &str) -> SkillKey {
        SkillKey::new(
            SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("valid fixture source uuid"),
            SkillName::parse(name).expect("valid fixture skill name"),
        )
    }

    struct StaticLlmClient;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for StaticLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::super::LlmStreamResult, AgentError> {
            Ok(super::super::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "ok".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage::default(),
            ))
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct NoTools;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for NoTools {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::new([])
        }

        async fn dispatch(
            &self,
            call: crate::types::ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            Err(ToolError::NotFound {
                name: call.name.into(),
            })
        }
    }

    struct NoopStore;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentSessionStore for NoopStore {
        async fn save(&self, _session: &crate::session::Session) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load(&self, _id: &str) -> Result<Option<crate::session::Session>, AgentError> {
            Ok(None)
        }
    }

    struct VisibilityReadFailingOwner;

    impl crate::tool_scope::ToolVisibilityOwner for VisibilityReadFailingOwner {
        fn visibility_state(
            &self,
        ) -> Result<crate::session::SessionToolVisibilityState, ToolScopeApplyError> {
            Err(ToolScopeApplyError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }

        fn replace_visibility_state(
            &self,
            _visibility_state: crate::session::SessionToolVisibilityState,
        ) -> Result<(), ToolScopeApplyError> {
            Ok(())
        }

        fn stage_persistent_filter(
            &self,
            _filter: ToolFilter,
            _witnesses: std::collections::BTreeMap<
                crate::ToolName,
                crate::session::ToolVisibilityWitness,
            >,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            Err(ToolScopeStageError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }

        fn stage_requested_deferred_names(
            &self,
            _names: std::collections::BTreeSet<crate::ToolName>,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            Err(ToolScopeStageError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }

        fn request_deferred_tools(
            &self,
            _authorities: Vec<crate::session::DeferredToolLoadAuthority>,
        ) -> Result<ToolScopeRevision, ToolScopeStageError> {
            Err(ToolScopeStageError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }

        fn boundary_applied(
            &self,
        ) -> Result<crate::session::SessionToolVisibilityState, ToolScopeApplyError> {
            Err(ToolScopeApplyError::Owner {
                message: "visibility read fixture failed".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn generated_visibility_export_read_failure_fails_closed() {
        let owner: Arc<dyn crate::tool_scope::ToolVisibilityOwner> =
            Arc::new(VisibilityReadFailingOwner);
        let agent = AgentBuilder::new()
            .with_tool_visibility_owner(
                crate::tool_scope::generated_test_tool_visibility_owner_from(owner),
            )
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let err = agent
            .session_with_system_context_state()
            .expect_err("generated owner read failures must not export partial visibility state");
        assert!(matches!(
            err,
            SystemContextStateError::ToolVisibilityAuthorization(ToolScopeApplyError::Owner { .. })
        ));
    }

    /// A `SkillEngine` whose `resolve_and_render` always fails with a typed
    /// `NotFound` error carrying the requested key.
    struct FailingSkillEngine;

    // Test mock mirrors the `SkillEngine` trait's `-> impl Future + Send`
    // method shapes verbatim; the manual-future form keeps the mock aligned
    // with the trait signatures it implements.
    #[allow(clippy::manual_async_fn)]
    impl SkillEngine for FailingSkillEngine {
        fn inventory_section(&self) -> impl Future<Output = Result<String, SkillError>> + Send {
            async move { Ok(String::new()) }
        }

        fn resolve_and_render(
            &self,
            keys: &[SkillKey],
        ) -> impl Future<Output = Result<Vec<ResolvedSkill>, SkillError>> + Send {
            let missing = keys
                .first()
                .cloned()
                .unwrap_or_else(|| fixture_skill_key("unknown"));
            async move { Err(SkillError::NotFound { key: missing }) }
        }

        fn collections(
            &self,
        ) -> impl Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send {
            async move { Ok(Vec::new()) }
        }

        fn list_skills(
            &self,
            _filter: &SkillFilter,
        ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send {
            async move { Ok(Vec::new()) }
        }

        fn quarantined_diagnostics(
            &self,
        ) -> impl Future<Output = Result<Vec<crate::skills::SkillQuarantineDiagnostic>, SkillError>> + Send
        {
            async move { Ok(Vec::new()) }
        }

        fn health_snapshot(
            &self,
        ) -> impl Future<Output = Result<crate::skills::SourceHealthSnapshot, SkillError>> + Send
        {
            async move { Ok(crate::skills::SourceHealthSnapshot::default()) }
        }

        fn list_artifacts(
            &self,
            key: &SkillKey,
        ) -> impl Future<Output = Result<Vec<crate::skills::SkillArtifact>, SkillError>> + Send
        {
            let missing = key.clone();
            async move { Err(SkillError::NotFound { key: missing }) }
        }

        fn read_artifact(
            &self,
            key: &SkillKey,
            _artifact_path: &str,
        ) -> impl Future<Output = Result<crate::skills::SkillArtifactContent, SkillError>> + Send
        {
            let missing = key.clone();
            async move { Err(SkillError::NotFound { key: missing }) }
        }

        fn invoke_function(
            &self,
            key: &SkillKey,
            _function_name: &crate::skills::SkillFunctionName,
            _arguments: crate::event::ToolCallArguments,
        ) -> impl Future<Output = Result<crate::skills::SkillFunctionOutput, SkillError>> + Send
        {
            let missing = key.clone();
            async move { Err(SkillError::NotFound { key: missing }) }
        }
    }

    /// A `SkillEngine` whose `resolve_and_render` always succeeds, returning a
    /// single rendered skill keyed by the first requested key.
    struct SucceedingSkillEngine;

    // Test mock mirrors the `SkillEngine` trait's `-> impl Future + Send`
    // method shapes verbatim (see `FailingSkillEngine` above).
    #[allow(clippy::manual_async_fn)]
    impl SkillEngine for SucceedingSkillEngine {
        fn inventory_section(&self) -> impl Future<Output = Result<String, SkillError>> + Send {
            async move { Ok(String::new()) }
        }

        fn resolve_and_render(
            &self,
            keys: &[SkillKey],
        ) -> impl Future<Output = Result<Vec<ResolvedSkill>, SkillError>> + Send {
            let key = keys
                .first()
                .cloned()
                .unwrap_or_else(|| fixture_skill_key("email-extractor"));
            async move {
                Ok(vec![ResolvedSkill {
                    key,
                    name: "email-extractor".into(),
                    rendered_body: "<skill>injected canonical skill</skill>".to_string(),
                    byte_size: 34,
                }])
            }
        }

        fn collections(
            &self,
        ) -> impl Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send {
            async move { Ok(Vec::new()) }
        }

        fn list_skills(
            &self,
            _filter: &SkillFilter,
        ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send {
            async move { Ok(Vec::new()) }
        }

        fn quarantined_diagnostics(
            &self,
        ) -> impl Future<Output = Result<Vec<crate::skills::SkillQuarantineDiagnostic>, SkillError>> + Send
        {
            async move { Ok(Vec::new()) }
        }

        fn health_snapshot(
            &self,
        ) -> impl Future<Output = Result<crate::skills::SourceHealthSnapshot, SkillError>> + Send
        {
            async move { Ok(crate::skills::SourceHealthSnapshot::default()) }
        }

        fn list_artifacts(
            &self,
            key: &SkillKey,
        ) -> impl Future<Output = Result<Vec<crate::skills::SkillArtifact>, SkillError>> + Send
        {
            let missing = key.clone();
            async move { Err(SkillError::NotFound { key: missing }) }
        }

        fn read_artifact(
            &self,
            key: &SkillKey,
            _artifact_path: &str,
        ) -> impl Future<Output = Result<crate::skills::SkillArtifactContent, SkillError>> + Send
        {
            let missing = key.clone();
            async move { Err(SkillError::NotFound { key: missing }) }
        }

        fn invoke_function(
            &self,
            key: &SkillKey,
            _function_name: &crate::skills::SkillFunctionName,
            _arguments: crate::event::ToolCallArguments,
        ) -> impl Future<Output = Result<crate::skills::SkillFunctionOutput, SkillError>> + Send
        {
            let missing = key.clone();
            async move { Err(SkillError::NotFound { key: missing }) }
        }
    }

    async fn build_agent_with_engine<E: SkillEngine + 'static>(
        engine: E,
    ) -> Agent<StaticLlmClient, NoTools, NoopStore> {
        let skill_runtime = Arc::new(SkillRuntime::new(Arc::new(engine)));
        with_test_turn_state_handle(AgentBuilder::new())
            .with_skill_engine(skill_runtime)
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await
    }

    fn with_test_turn_state_handle(builder: AgentBuilder) -> AgentBuilder {
        use crate::agent::test_turn_state_handle::TestTurnStateHandle;

        let mut session = crate::Session::new();
        session
            .set_build_state(crate::SessionBuildState::default())
            .expect("test session build state should serialize");
        builder
            .resume_session(session)
            .with_turn_state_handle(Arc::new(TestTurnStateHandle::new()))
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
    }

    /// Row #65: a failing `skill_references` resolution emits the typed
    /// `SkillResolutionFailed` event (carrying the typed `SkillKey` + typed
    /// reason). The old behavior only logged and returned the input untouched,
    /// so no event reaches the channel — this test fails on that behavior.
    #[tokio::test]
    async fn failed_skill_resolution_emits_typed_failure_event() {
        let mut agent = build_agent_with_engine(FailingSkillEngine).await;
        let key = fixture_skill_key("email-extractor");
        agent.pending_skill_references = Some(vec![key.clone()]);

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(8);
        let err = agent
            .resolve_pending_skill_context(Some(&tx))
            .await
            .expect_err("skill resolution failures must fail closed");
        drop(tx);

        match err {
            AgentError::SkillResolutionFailed {
                skill_key: Some(failed_key),
                reason,
            } => match *reason {
                crate::event::SkillResolutionFailureReason::NotFound { key: missing_key } => {
                    assert_eq!(failed_key, key);
                    assert_eq!(missing_key, key);
                }
                other => panic!("unexpected skill resolution reason: {other:?}"),
            },
            other => panic!("unexpected skill resolution error: {other:?}"),
        }

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let failure = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::SkillResolutionFailed {
                    skill_key, reason, ..
                } => Some((skill_key.clone(), reason.clone())),
                _ => None,
            })
            .expect("failed resolution must emit a typed SkillResolutionFailed event");

        assert_eq!(
            failure.0,
            Some(key.clone()),
            "event must carry the typed SkillKey we attempted to resolve"
        );
        assert_eq!(
            failure.1,
            crate::event::SkillResolutionFailureReason::NotFound { key },
            "event must carry the typed failure reason, not a stringified log"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::SkillsResolved { .. })),
            "a failed resolution must not also report a successful activation"
        );
    }

    #[tokio::test]
    async fn run_fails_closed_when_per_turn_skill_resolution_fails() {
        let mut agent = build_agent_with_engine(FailingSkillEngine).await;
        let key = fixture_skill_key("email-extractor");
        agent.pending_skill_references = Some(vec![key.clone()]);
        let initial_message_count = agent.session().messages().len();

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(8);
        let err = agent
            .run_with_events("hello".to_string().into(), tx)
            .await
            .expect_err("missing per-turn skill refs must fail the run");

        match err {
            AgentError::SkillResolutionFailed {
                skill_key: Some(failed_key),
                reason,
            } => match *reason {
                crate::event::SkillResolutionFailureReason::NotFound { key: missing_key } => {
                    assert_eq!(failed_key, key);
                    assert_eq!(missing_key, key);
                }
                other => panic!("unexpected skill resolution reason: {other:?}"),
            },
            other => panic!("unexpected skill resolution error: {other:?}"),
        }
        assert_eq!(
            agent.session().messages().len(),
            initial_message_count,
            "failed per-turn skill resolution must not commit the user message"
        );

        let mut saw_skill_failure = false;
        let mut saw_run_failed = false;
        let mut saw_run_started = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::SkillResolutionFailed { .. } => saw_skill_failure = true,
                AgentEvent::RunFailed { error_report, .. } => {
                    saw_run_failed = true;
                    assert_eq!(error_report.class, crate::event::AgentErrorClass::Skill);
                }
                AgentEvent::RunStarted { .. } => saw_run_started = true,
                _ => {}
            }
        }

        assert!(
            saw_skill_failure,
            "typed skill failure event should be emitted"
        );
        assert!(saw_run_failed, "run should terminalize with RunFailed");
        assert!(
            !saw_run_started,
            "run must fail before publishing RunStarted"
        );
    }

    #[tokio::test]
    async fn runtime_transcript_identity_is_persisted_on_user_and_assistant_messages() {
        let mut agent = build_agent_with_engine(SucceedingSkillEngine).await;
        let interaction_id = crate::interaction::InteractionId(uuid::Uuid::from_u128(0xfeed_beef));
        let transcript_identity = crate::types::TranscriptMessageIdentity {
            interaction_id: Some(interaction_id),
            run_id: None,
        };

        let (tx, _rx) = mpsc::channel::<AgentEvent>(8);
        agent
            .run_with_events_and_typed_turn_appends(
                "Still here.".to_string().into(),
                Vec::new(),
                Vec::new(),
                Some(transcript_identity),
                tx,
            )
            .await
            .expect("mock turn should complete");

        let user = agent
            .session()
            .messages()
            .iter()
            .find_map(|message| match message {
                Message::User(message) => Some(message),
                _ => None,
            })
            .expect("turn should persist a user transcript message");
        assert_eq!(
            user.identity.interaction_id,
            Some(interaction_id),
            "runtime-stamped interaction id must survive into persisted user history"
        );
        assert_eq!(user.identity.run_id, None);

        let assistant = agent
            .session()
            .messages()
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::BlockAssistant(message) => Some(message),
                _ => None,
            })
            .expect("turn should persist a block assistant transcript message");
        assert_eq!(
            assistant.identity.interaction_id,
            Some(interaction_id),
            "persisted assistant history must carry the same id as live InteractionComplete"
        );
        assert!(
            assistant.identity.run_id.is_some(),
            "assistant transcript identity must include the concrete run id for exact run-scoped joins"
        );
    }

    #[tokio::test]
    async fn append_assistant_blocks_effect_stamps_active_transcript_identity_and_run_id() {
        let mut agent = build_agent_with_engine(SucceedingSkillEngine).await;
        let interaction_id = crate::interaction::InteractionId(uuid::Uuid::from_u128(0xfeed_cafe));
        let run_id = crate::lifecycle::RunId::new();
        agent.set_active_transcript_identity(Some(crate::types::TranscriptMessageIdentity {
            interaction_id: Some(interaction_id),
            run_id: None,
        }));

        agent
            .apply_session_effects(
                &[crate::ops::SessionEffect::AppendAssistantBlocks {
                    blocks: vec![AssistantBlock::Text {
                        text: "effect assistant".to_string(),
                        meta: None,
                    }],
                }],
                Some(&run_id),
            )
            .expect("assistant append effect should apply");

        let assistant = agent
            .session()
            .messages()
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::BlockAssistant(message) => Some(message),
                _ => None,
            })
            .expect("effect should persist a block assistant message");
        assert_eq!(assistant.identity.interaction_id, Some(interaction_id));
        assert_eq!(assistant.identity.run_id, Some(run_id));
    }

    /// Ask 1 direct-path lowering: each injected-context entry materializes as
    /// a SEPARATE typed injected-context user-channel message immediately
    /// BEFORE the turn's user message, in delivery order.
    #[tokio::test]
    async fn direct_path_materializes_injected_context_before_user_message() {
        let mut agent = build_agent_with_engine(SucceedingSkillEngine).await;

        let (tx, _rx) = mpsc::channel::<AgentEvent>(8);
        agent
            .run_with_events_and_typed_turn_appends(
                "the actual prompt".to_string().into(),
                Vec::new(),
                vec![
                    "first ambient entry".to_string().into(),
                    crate::types::ContentInput::Blocks(vec![crate::types::ContentBlock::Text {
                        text: "second ambient entry".to_string(),
                    }]),
                ],
                None,
                tx,
            )
            .await
            .expect("mock turn should complete");

        let user_messages: Vec<&crate::types::UserMessage> = agent
            .session()
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::User(message) => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(
            user_messages.len(),
            3,
            "two injected-context messages plus the turn's user message"
        );
        assert!(
            user_messages[0].transcript_role.is_injected_context(),
            "first injected entry must carry the typed injected-context role"
        );
        assert_eq!(user_messages[0].text_content(), "first ambient entry");
        assert!(
            user_messages[1].transcript_role.is_injected_context(),
            "second injected entry must carry the typed injected-context role"
        );
        assert_eq!(user_messages[1].text_content(), "second ambient entry");
        assert!(
            user_messages[2].transcript_role.is_conversational(),
            "the turn's user message stays conversational"
        );
        assert_eq!(user_messages[2].text_content(), "the actual prompt");
    }

    /// Exactly one lowering owner per mode: when the runtime authored the
    /// turn's typed appends, a direct-path injected-context carrier must fail
    /// closed instead of double-appending or silently dropping.
    #[tokio::test]
    async fn injected_context_with_typed_turn_appends_fails_closed() {
        let mut agent = build_agent_with_engine(SucceedingSkillEngine).await;
        let initial_message_count = agent.session().messages().len();

        let (tx, _rx) = mpsc::channel::<AgentEvent>(8);
        let err = agent
            .run_with_events_and_typed_turn_appends(
                "prompt".to_string().into(),
                vec![crate::lifecycle::run_primitive::ConversationAppend {
                    role: ConversationAppendRole::User,
                    content: CoreRenderable::Text {
                        text: "runtime-authored prompt".to_string(),
                    },
                }],
                vec!["ambient".to_string().into()],
                None,
                tx,
            )
            .await
            .expect_err("both carriers non-empty must fail closed");

        assert!(
            matches!(err, AgentError::ConfigError(_)),
            "expected typed config error, got {err:?}"
        );
        assert_eq!(
            agent.session().messages().len(),
            initial_message_count,
            "fail-closed rejection must not commit any transcript message"
        );
    }

    /// Runtime-mode lowering owner: an `InjectedContext`-role typed append
    /// lowers into a typed injected-context user message, preserving append
    /// order relative to the user append.
    #[tokio::test]
    async fn injected_context_typed_append_lowers_to_typed_user_message() {
        let mut agent = build_agent_with_engine(SucceedingSkillEngine).await;

        let (tx, _rx) = mpsc::channel::<AgentEvent>(8);
        agent
            .run_with_events_and_typed_turn_appends(
                "projected prompt".to_string().into(),
                vec![
                    crate::lifecycle::run_primitive::ConversationAppend {
                        role: ConversationAppendRole::InjectedContext,
                        content: CoreRenderable::Text {
                            text: "runtime-lowered ambient context".to_string(),
                        },
                    },
                    crate::lifecycle::run_primitive::ConversationAppend {
                        role: ConversationAppendRole::User,
                        content: CoreRenderable::Text {
                            text: "the user prompt".to_string(),
                        },
                    },
                ],
                Vec::new(),
                None,
                tx,
            )
            .await
            .expect("mock turn should complete");

        let user_messages: Vec<&crate::types::UserMessage> = agent
            .session()
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::User(message) => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(user_messages.len(), 2);
        assert!(user_messages[0].transcript_role.is_injected_context());
        assert_eq!(
            user_messages[0].text_content(),
            "runtime-lowered ambient context"
        );
        assert!(user_messages[1].transcript_role.is_conversational());
        assert_eq!(user_messages[1].text_content(), "the user prompt");
    }

    /// An `InjectedContext` append only accepts operator text or content
    /// blocks — the same operator-renderable restriction as the user role.
    #[tokio::test]
    async fn injected_context_typed_append_rejects_non_operator_renderable() {
        let mut agent = build_agent_with_engine(SucceedingSkillEngine).await;

        let (tx, _rx) = mpsc::channel::<AgentEvent>(8);
        let err = agent
            .run_with_events_and_typed_turn_appends(
                "prompt".to_string().into(),
                vec![crate::lifecycle::run_primitive::ConversationAppend {
                    role: ConversationAppendRole::InjectedContext,
                    content: CoreRenderable::Json {
                        value: serde_json::json!({"not": "operator content"}),
                    },
                }],
                Vec::new(),
                None,
                tx,
            )
            .await
            .expect_err("json renderable must be rejected for injected-context appends");
        assert!(
            matches!(err, AgentError::ConfigError(_)),
            "expected typed config error, got {err:?}"
        );
    }

    /// Row #84: a successful activation produces a typed activation record (the
    /// `SkillsResolved` event carrying the canonical `SkillKey`s), distinct
    /// from the operator text. The old behavior folded activation purely into
    /// the prompt string and emitted no typed effect — this test fails on that
    /// behavior because no `SkillsResolved` event reaches the channel.
    #[tokio::test]
    async fn successful_skill_activation_emits_typed_activation_record() {
        let mut agent = build_agent_with_engine(SucceedingSkillEngine).await;
        let key = fixture_skill_key("email-extractor");
        agent.pending_skill_references = Some(vec![key.clone()]);

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(8);
        let out = agent
            .resolve_pending_skill_context(Some(&tx))
            .await
            .expect("successful skill resolution should produce skill context");
        drop(tx);

        // The activation yields a typed skill-context block carrying both the
        // canonical key and the rendered body — never an anonymous text fold.
        assert_eq!(out.len(), 1, "one activation yields one typed block");
        let crate::types::ContentBlock::SkillContext { skill_key, text } = &out[0] else {
            panic!("activation must yield a typed SkillContext block, got {out:?}");
        };
        assert_eq!(skill_key, &key, "block must carry the canonical SkillKey");
        assert!(
            text.contains("<skill>injected canonical skill</skill>"),
            "rendered body should reach the typed block, saw: {text}"
        );

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let resolved = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::SkillsResolved {
                    skills,
                    injection_bytes,
                } => Some((skills.clone(), *injection_bytes)),
                _ => None,
            })
            .expect("successful activation must emit a typed SkillsResolved record");

        assert_eq!(
            resolved.0,
            vec![key],
            "typed activation record must carry the canonical SkillKey, distinct from operator text"
        );
        assert_eq!(
            resolved.1, 34,
            "typed activation record must carry the injection byte size"
        );
    }

    /// A `SkillEngine` whose health snapshot fails with a typed error.
    struct HealthFaultSkillEngine;

    // Test mock mirrors the `SkillEngine` trait's `-> impl Future + Send`
    // method shapes verbatim (see `FailingSkillEngine` above).
    #[allow(clippy::manual_async_fn)]
    impl SkillEngine for HealthFaultSkillEngine {
        fn inventory_section(&self) -> impl Future<Output = Result<String, SkillError>> + Send {
            async move { Ok(String::new()) }
        }

        fn resolve_and_render(
            &self,
            _keys: &[SkillKey],
        ) -> impl Future<Output = Result<Vec<ResolvedSkill>, SkillError>> + Send {
            async move { Ok(Vec::new()) }
        }

        fn collections(
            &self,
        ) -> impl Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send {
            async move { Ok(Vec::new()) }
        }

        fn list_skills(
            &self,
            _filter: &SkillFilter,
        ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send {
            async move { Ok(Vec::new()) }
        }

        fn quarantined_diagnostics(
            &self,
        ) -> impl Future<Output = Result<Vec<crate::skills::SkillQuarantineDiagnostic>, SkillError>> + Send
        {
            async move { Ok(Vec::new()) }
        }

        fn health_snapshot(
            &self,
        ) -> impl Future<Output = Result<crate::skills::SourceHealthSnapshot, SkillError>> + Send
        {
            async move {
                Err(SkillError::NotFound {
                    key: fixture_skill_key("health-fault"),
                })
            }
        }

        fn list_artifacts(
            &self,
            key: &SkillKey,
        ) -> impl Future<Output = Result<Vec<crate::skills::SkillArtifact>, SkillError>> + Send
        {
            let missing = key.clone();
            async move { Err(SkillError::NotFound { key: missing }) }
        }

        fn read_artifact(
            &self,
            key: &SkillKey,
            _artifact_path: &str,
        ) -> impl Future<Output = Result<crate::skills::SkillArtifactContent, SkillError>> + Send
        {
            let missing = key.clone();
            async move { Err(SkillError::NotFound { key: missing }) }
        }

        fn invoke_function(
            &self,
            key: &SkillKey,
            _function_name: &crate::skills::SkillFunctionName,
            _arguments: crate::event::ToolCallArguments,
        ) -> impl Future<Output = Result<crate::skills::SkillFunctionOutput, SkillError>> + Send
        {
            let missing = key.clone();
            async move { Err(SkillError::NotFound { key: missing }) }
        }
    }

    /// Dogma row #239: a skill-diagnostics collection fault at run terminality
    /// is recorded as a typed `collection_fault` inside the run result —
    /// surfaces never receive absent or healthy-looking diagnostics in place
    /// of failure truth.
    #[tokio::test]
    async fn skill_diagnostics_collection_fault_is_recorded_in_run_result() {
        let skill_runtime = Arc::new(SkillRuntime::new(Arc::new(HealthFaultSkillEngine)));
        let mut agent = AgentBuilder::new()
            .with_skill_engine(skill_runtime)
            .with_turn_state_handle(Arc::new(
                crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
            ))
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let result = agent
            .run("hello".to_string().into())
            .await
            .expect("run should complete despite diagnostics collection fault");

        let diagnostics = result
            .skill_diagnostics
            .expect("collection fault must yield fault-carrying diagnostics, not None");
        let fault = diagnostics
            .collection_fault
            .expect("diagnostics must carry the typed collection fault");
        assert!(
            matches!(
                fault,
                crate::event::SkillResolutionFailureReason::NotFound { .. }
            ),
            "fault must preserve the typed failure reason, got {fault:?}"
        );
    }
}

/// Gate tests for typed execution-snapshot projection (row #309).
///
/// A wide (`u64`) turn-state counter that overflows the snapshot's `u32`
/// projection must surface a typed [`SnapshotProjectionError`] — never a
/// fabricated missing snapshot (`None`) or a default running `LoopState`.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod snapshot_projection_tests {
    use super::*;
    use crate::handles::{DslTransitionError, TurnStateHandle, TurnStateSnapshot};
    use crate::lifecycle::RunId;
    use crate::ops::{AsyncOpRef, OperationId};
    use crate::retry::LlmRetrySchedule;
    use crate::turn_execution_authority::{
        ContentShape, TurnExecutionEffect, TurnExecutionInput, TurnFailureReason, TurnFailureSource,
    };
    use std::collections::BTreeSet;

    /// Minimal `TurnStateHandle` that returns a caller-supplied snapshot.
    ///
    /// Only `snapshot()` is exercised by the projection path under test; the
    /// remaining transition methods are never called and report a transition
    /// refusal rather than panicking.
    struct StubTurnStateHandle {
        snapshot: TurnStateSnapshot,
    }

    fn refused() -> DslTransitionError {
        DslTransitionError::no_matching(
            "stub-turn-state-handle",
            "transition not exercised in test",
        )
    }

    impl TurnStateHandle for StubTurnStateHandle {
        fn apply_turn_input(
            &self,
            _input: TurnExecutionInput,
        ) -> Result<Vec<TurnExecutionEffect>, DslTransitionError> {
            Err(refused())
        }
        fn start_conversation_run(
            &self,
            _run_id: RunId,
            _primitive_kind: TurnPrimitiveKind,
            _admitted_content_shape: ContentShape,
            _vision_enabled: bool,
            _image_tool_results_enabled: bool,
            _max_extraction_retries: u64,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn start_immediate_append(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn start_immediate_context(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn primitive_applied(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn llm_returned_tool_calls(
            &self,
            _run_id: RunId,
            _tool_count: u64,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn llm_returned_terminal(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn register_pending_ops(
            &self,
            _run_id: RunId,
            _op_refs: BTreeSet<AsyncOpRef>,
            _barrier_operation_ids: BTreeSet<OperationId>,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn tool_calls_resolved(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn ops_barrier_satisfied(
            &self,
            _run_id: RunId,
            _operation_ids: BTreeSet<OperationId>,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn boundary_continue(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn boundary_complete(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn enter_extraction(
            &self,
            _run_id: RunId,
            _max_retries: u32,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn extraction_start(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn extraction_validation_passed(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn extraction_validation_failed(
            &self,
            _run_id: RunId,
            _error: String,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn extraction_failed(
            &self,
            _run_id: RunId,
            _error: String,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn recoverable_failure(
            &self,
            _run_id: RunId,
            _retry: LlmRetrySchedule,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn fatal_failure(
            &self,
            _run_id: RunId,
            _failure: TurnFailureSource,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn retry_requested(
            &self,
            _run_id: RunId,
            _retry_attempt: u32,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn cancel_now(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn request_cancel_after_boundary(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn cancellation_observed(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn acknowledge_terminal(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn turn_limit_reached(
            &self,
            _run_id: RunId,
            _turn_count: u64,
            _max_turns: u64,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn budget_exhausted(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn time_budget_exceeded(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn force_cancel_no_run(&self) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn run_completed(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn run_failed(
            &self,
            _run_id: RunId,
            _reason: TurnFailureReason,
        ) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn run_cancelled(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
            Err(refused())
        }
        fn snapshot(&self) -> TurnStateSnapshot {
            self.snapshot.clone()
        }
    }

    fn base_snapshot() -> TurnStateSnapshot {
        TurnStateSnapshot {
            active_run_id: None,
            loop_state: LoopState::WaitingForOps,
            turn_phase: crate::TurnPhase::WaitingForOps,
            turn_terminal: false,
            primitive_kind: None,
            admitted_content_shape: None,
            vision_enabled: false,
            image_tool_results_enabled: false,
            tool_calls_pending: 0,
            pending_op_refs: BTreeSet::new(),
            barrier_operation_ids: BTreeSet::new(),
            has_barrier_ops: false,
            barrier_satisfied: false,
            boundary_count: 0,
            cancel_after_boundary: false,
            terminal_outcome: None,
            terminal_cause_kind: None,
            extraction_attempts: 0,
            max_extraction_retries: 0,
            extraction_active: false,
            llm_retry_attempt: 0,
            llm_retry_max_retries: 0,
            llm_retry_selected_delay_ms: 0,
        }
    }

    #[test]
    fn well_formed_counters_project_cleanly() {
        let mut snapshot = base_snapshot();
        snapshot.tool_calls_pending = 3;
        snapshot.boundary_count = 7;
        let handle = StubTurnStateHandle { snapshot };

        let projected = runtime_execution_snapshot(&handle, 0)
            .expect("in-range counters must project without error");
        assert_eq!(projected.tool_calls_pending, 3);
        assert_eq!(projected.boundary_count, 7);
        assert_eq!(projected.loop_state, LoopState::WaitingForOps);
    }

    #[test]
    fn overflowing_counter_yields_typed_projection_error() {
        let overflow = u64::from(u32::MAX) + 1;
        let mut snapshot = base_snapshot();
        snapshot.tool_calls_pending = overflow;
        let handle = StubTurnStateHandle { snapshot };

        match runtime_execution_snapshot(&handle, 0) {
            Ok(snapshot) => panic!(
                "overflow must not fabricate a snapshot: {:?}",
                snapshot.loop_state
            ),
            Err(SnapshotProjectionError::CounterOverflow { field, value }) => {
                assert_eq!(field, "tool_calls_pending");
                assert_eq!(value, overflow);
            }
        }
    }

    #[test]
    fn each_wide_counter_field_reports_its_own_overflow() {
        let overflow = u64::from(u32::MAX) + 1;
        let cases: [(&str, fn(&mut TurnStateSnapshot, u64)); 4] = [
            ("tool_calls_pending", |s, v| s.tool_calls_pending = v),
            ("boundary_count", |s, v| s.boundary_count = v),
            ("extraction_attempts", |s, v| s.extraction_attempts = v),
            ("max_extraction_retries", |s, v| {
                s.max_extraction_retries = v;
            }),
        ];
        for (field, set) in cases {
            let mut snapshot = base_snapshot();
            set(&mut snapshot, overflow);
            let handle = StubTurnStateHandle { snapshot };
            match runtime_execution_snapshot(&handle, 0) {
                Err(SnapshotProjectionError::CounterOverflow {
                    field: reported,
                    value,
                }) => {
                    assert_eq!(reported, field);
                    assert_eq!(value, overflow);
                }
                Ok(_) => panic!("{field} overflow must yield a typed projection error"),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod run_input_pending_tail_tests {
    use super::*;
    use crate::types::{ToolResult, UserMessage};

    /// K3 invariant: a pending tool-results tail resolves to the typed
    /// `RunInput::PendingToolResults` variant — never to a fabricated
    /// empty-string prompt.
    #[test]
    fn pending_tool_results_tail_is_typed_variant_not_empty_prompt() {
        let messages = vec![Message::tool_results(vec![ToolResult::new(
            "tc-1".to_string(),
            "ok".to_string(),
            false,
        )])];
        let input =
            run_input_from_admitted_pending_tail(&messages, ObservedSessionTailKind::ToolResults)
                .expect("admitted tool-results tail resolves");
        assert_eq!(input, RunInput::PendingToolResults);
        assert_eq!(
            input.prompt_text(),
            None,
            "pending-tail runs have no prompt; nothing may fabricate an empty string"
        );
    }

    #[test]
    fn user_tail_resolves_to_typed_content() {
        let messages = vec![Message::User(UserMessage::text("hello".to_string()))];
        let input = run_input_from_admitted_pending_tail(&messages, ObservedSessionTailKind::User)
            .expect("admitted user tail resolves");
        assert_eq!(
            input,
            RunInput::Content {
                content: ContentInput::Text("hello".to_string())
            }
        );
    }
}
