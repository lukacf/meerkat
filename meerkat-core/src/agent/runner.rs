//! Agent runner interface.

use crate::budget::Budget;
use crate::error::{AgentError, ToolError};
use crate::event::AgentEvent;
use crate::hooks::{HookInvocation, HookPatch, HookPoint};
use crate::ops::{ToolDispatchOutcome, ToolDispatchTimeoutPolicy};
use crate::retry::RetryPolicy;
use crate::service::TurnToolOverlay;
use crate::session::{PendingSystemContextAppend, Session};
use crate::state::LoopState;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_scope::{
    EXTERNAL_TOOL_FILTER_METADATA_KEY, ExternalToolSurfaceBaseState,
    ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceEntrySnapshot, ExternalToolSurfaceSnapshot, ToolFilter, ToolScopeRevision,
    ToolScopeStageError,
};
use crate::turn_execution_authority::{
    TurnPrimitiveKind, TurnTerminalCauseKind, TurnTerminalOutcome,
};
use crate::types::{ContentInput, Message, RunResult, ToolCallView, ToolNameSet};
use async_trait::async_trait;
use serde_json::value::to_raw_value;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{Agent, AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};

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

fn runtime_execution_snapshot(
    handle: &dyn crate::TurnStateHandle,
    applied_cursor: crate::completion_feed::CompletionSeq,
) -> Option<crate::AgentExecutionSnapshot> {
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

    Some(crate::AgentExecutionSnapshot {
        loop_state: snapshot.loop_state,
        turn_phase,
        active_run_id: snapshot.active_run_id,
        primitive_kind,
        admitted_content_shape: snapshot.admitted_content_shape,
        vision_enabled: snapshot.vision_enabled,
        image_tool_results_enabled: snapshot.image_tool_results_enabled,
        tool_calls_pending: u32::try_from(snapshot.tool_calls_pending).ok()?,
        pending_operation_ids,
        barrier_operation_ids,
        has_barrier_ops: snapshot.has_barrier_ops,
        barrier_satisfied: snapshot.barrier_satisfied,
        boundary_count: u32::try_from(snapshot.boundary_count).ok()?,
        cancel_after_boundary: snapshot.cancel_after_boundary,
        terminal_outcome,
        terminal_cause_kind: snapshot.terminal_cause_kind,
        extraction_attempts: u32::try_from(snapshot.extraction_attempts).ok()?,
        max_extraction_retries: u32::try_from(snapshot.max_extraction_retries).ok()?,
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
        if let Ok(visibility_state) = self.tool_scope.visibility_state() {
            if let Err(err) = self.session.set_tool_visibility_state(visibility_state) {
                tracing::warn!(
                    error = %err,
                    "failed to persist staged canonical tool visibility state"
                );
            }
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
            let allow = overlay
                .allowed_tools
                .map(|tools| tools.into_iter().collect::<HashSet<_>>());
            let deny = overlay
                .blocked_tools
                .unwrap_or_default()
                .into_iter()
                .collect::<HashSet<_>>();
            handle.set_turn_overlay(allow, deny)?;
        } else {
            handle.clear_turn_overlay();
        }
        Ok(())
    }

    pub fn set_runtime_execution_kind(
        &mut self,
        execution_kind: Option<crate::lifecycle::RuntimeExecutionKind>,
    ) {
        self.runtime_execution_kind = execution_kind;
    }

    fn clear_runtime_execution_kind(&mut self) {
        self.runtime_execution_kind = None;
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
    /// The session's `build_state` is the source of truth. The shared
    /// `mob_authority_handle` (if present) is updated as a derived projection
    /// after the canonical write succeeds.
    pub(crate) fn apply_session_effects(
        &mut self,
        effects: &[crate::ops::SessionEffect],
    ) -> Result<(), crate::error::AgentError> {
        use crate::error::AgentError;

        let mut build_state = self.session.build_state().unwrap_or_default();
        let mut build_state_changed = false;
        let mut visibility_changed = false;

        for effect in effects {
            match effect {
                crate::ops::SessionEffect::GrantManageMob { mob_id } => {
                    let authority =
                        build_state
                            .mob_tool_authority_context
                            .as_mut()
                            .ok_or_else(|| {
                                AgentError::InternalError(
                                "mob authority effect applied without canonical authority context"
                                    .into(),
                            )
                            })?;
                    authority.grant_manage_mob_in_place(mob_id.clone());
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
                    self.session.push(crate::types::Message::BlockAssistant(
                        crate::types::BlockAssistantMessage::new(
                            blocks.clone(),
                            crate::types::StopReason::EndTurn,
                        ),
                    ));
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
        // subsequent batches see the widened scope. The handle is a derived
        // projection of the canonical session build_state — it is never
        // treated as an independent truth source.
        if build_state_changed && let Some(ref handle) = self.mob_authority_handle {
            let updated = self
                .session
                .build_state()
                .and_then(|bs| bs.mob_tool_authority_context);
            if let Some(authority) = updated {
                *handle
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = authority;
            }
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
        self.config.provider_params = policy.provider_params;
        self.config.provider_tool_defaults = policy.provider_tool_defaults;
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
            let _ = handle.release_lease(&previous_key);
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

    /// Shared live control flag for boundary-only cancellation requests.
    pub fn cancel_after_boundary_handle(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.cancel_after_boundary_requested)
    }

    /// Persist the currently committed visible tool set into canonical session metadata.
    pub(crate) fn publish_committed_visible_set(&mut self) -> Result<(), AgentError> {
        // Session metadata is a durable projection/export of the canonical
        // visibility owner state so checkpoint/recovery stays aligned.
        let visibility_state = self.tool_scope.visibility_state().map_err(|err| {
            AgentError::InternalError(format!(
                "failed to snapshot canonical tool visibility state: {err}"
            ))
        })?;
        self.session
            .set_tool_visibility_state(visibility_state)
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
        let dispatch_result = match timeout_policy.timeout() {
            Some(timeout) => match tokio::time::timeout(timeout, self.tools.dispatch(view)).await {
                Ok(result) => result,
                Err(_) => Err(crate::error::ToolError::timeout(
                    call.name.clone(),
                    timeout_policy.timeout_ms().unwrap_or(u64::MAX),
                )),
            },
            None => self.tools.dispatch(view).await,
        };

        match dispatch_result {
            Ok(mut outcome) => {
                outcome.clear_terminal_cause();
                if outcome.result.tool_use_id.is_empty() {
                    outcome.result.tool_use_id = call.id;
                }
                if !outcome.session_effects.is_empty() {
                    self.apply_session_effects(&outcome.session_effects)?;
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

    /// Get the current state
    pub fn state(&self) -> LoopState {
        self.execution_snapshot()
            .map(|snapshot| snapshot.loop_state)
            .unwrap_or(LoopState::CallingLlm)
    }

    /// Snapshot the agent's live execution state for diagnostics and mapping.
    ///
    /// Returns `None` when the agent has no runtime-backed turn-state
    /// handle attached (standalone/ephemeral execution paths).
    pub fn execution_snapshot(&self) -> Option<crate::AgentExecutionSnapshot> {
        let handle = self.turn_state_handle.as_deref()?;
        runtime_execution_snapshot(handle, self.applied_cursor)
    }

    /// Snapshot the agent's live tool-scope state for diagnostics and mapping.
    pub fn tool_scope_snapshot(&self) -> Option<crate::ToolScopeSnapshot> {
        self.tool_scope.snapshot()
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
    pub fn system_context_state(
        &self,
    ) -> Arc<std::sync::Mutex<crate::session::SessionSystemContextState>> {
        Arc::clone(&self.system_context_state)
    }

    /// Clone the current session with the latest shared system-context state merged into metadata.
    pub fn session_with_system_context_state(&self) -> Session {
        let mut session = self.session.clone();
        let state = match self.system_context_state.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                tracing::warn!("system-context state lock poisoned while cloning session");
                poisoned.into_inner().clone()
            }
        };
        if let Err(err) = session.set_system_context_state(state) {
            tracing::warn!(error = %err, "failed to serialize system-context state into session");
        }
        if let Ok(visibility_state) = self.tool_scope.visibility_state()
            && let Err(err) = session.set_tool_visibility_state(visibility_state)
        {
            tracing::warn!(error = %err, "failed to serialize tool visibility state into session");
        }
        session
    }

    /// Synchronize the shared system-context state into the in-memory session metadata.
    #[doc(hidden)]
    pub fn sync_system_context_state_to_session(&mut self) {
        let state = match self.system_context_state.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                tracing::warn!("system-context state lock poisoned while syncing session");
                poisoned.into_inner().clone()
            }
        };
        if let Err(err) = self.session.set_system_context_state(state) {
            tracing::warn!(error = %err, "failed to serialize system-context state into session");
        }
    }

    /// Consume all pending system-context appends for the next LLM boundary.
    ///
    /// The returned appends are intended for transient request composition only;
    /// they must not be written back into the canonical session prompt.
    pub(crate) fn take_pending_system_context_boundary(
        &mut self,
    ) -> Vec<PendingSystemContextAppend> {
        let pending = {
            let mut state = match self.system_context_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("system-context state lock poisoned while applying boundary");
                    poisoned.into_inner()
                }
            };
            if state.pending.is_empty() {
                return Vec::new();
            }
            let pending = state.pending.clone();
            state.mark_pending_applied();
            pending
        };

        self.sync_system_context_state_to_session();
        pending
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
    #[doc(hidden)]
    pub async fn checkpoint_current_session(&mut self) {
        self.sync_system_context_state_to_session();
        if let Some(ref cp) = self.checkpointer {
            cp.checkpoint(&self.session).await;
        }
    }

    async fn run_started_hooks(
        &self,
        prompt: &ContentInput,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<(), AgentError> {
        let report = self
            .execute_hooks(
                HookInvocation::run_started(self.session.id().clone(), prompt.clone()),
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

        for outcome in &report.outcomes {
            for patch in &outcome.patches {
                if let HookPatch::RunResult { text } = patch {
                    crate::event_tap::tap_emit(
                        &self.event_tap,
                        event_tx,
                        AgentEvent::HookRewriteApplied {
                            hook_id: outcome.hook_id.clone(),
                            point: HookPoint::RunCompleted,
                            patch: HookPatch::RunResult { text: text.clone() },
                        },
                    )
                    .await;
                    result.text.clone_from(text);
                    if result.structured_output.is_some() {
                        tracing::info!(
                            hook_id = %outcome.hook_id,
                            "clearing structured_output after hook text rewrite"
                        );
                        result.structured_output = None;
                    }
                    self.apply_run_result_text_patch(text);
                }
            }
        }
        if let Err(err) = self.store.save(&self.session).await {
            tracing::warn!("Failed to save session after run_completed hooks: {}", err);
        }
        self.run_completed_hooks_applied = true;
        Ok(())
    }

    async fn emit_run_completed_event(
        &self,
        result: &RunResult,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            event_tx,
            AgentEvent::RunCompleted {
                session_id: self.session.id().clone(),
                result: result.text.clone(),
                usage: result.usage.clone(),
                terminal_cause_kind: result.terminal_cause_kind,
            },
        )
        .await;
    }

    async fn emit_run_started_event(
        &self,
        prompt: ContentInput,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            event_tx,
            AgentEvent::RunStarted {
                session_id: self.session.id().clone(),
                prompt,
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
            _ => self
                .execution_snapshot()
                .and_then(|snapshot| snapshot.terminal_cause_kind)
                .filter(|cause_kind| *cause_kind != TurnTerminalCauseKind::Unknown),
        };
        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            event_tx,
            AgentEvent::RunFailed {
                session_id: self.session.id().clone(),
                error_class: error_report.class,
                error: error_report.message.clone(),
                terminal_cause_kind,
                error_report: Some(error_report),
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

    fn apply_run_result_text_patch(&mut self, text: &str) {
        use super::state::rewrite_assistant_text;
        let messages = self.session.messages_mut_internal();
        if let Some(last_assistant) = messages
            .iter_mut()
            .rev()
            .find(|message| matches!(message, Message::BlockAssistant(_) | Message::Assistant(_)))
        {
            match last_assistant {
                Message::BlockAssistant(block_assistant) => {
                    rewrite_assistant_text(&mut block_assistant.blocks, text.to_string());
                }
                Message::Assistant(assistant) => {
                    assistant.content = text.to_string();
                }
                _ => {}
            }
            self.session.touch();
        }
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
        self.run_inner(user_input, None).await
    }

    /// Run the agent with events streamed to the provided channel.
    pub async fn run_with_events(
        &mut self,
        user_input: ContentInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        self.run_inner(user_input, Some(event_tx)).await
    }

    /// Run the agent using the pending continuation boundary already in the session.
    ///
    /// This is useful when the session has been pre-populated with a continuation
    /// boundary (for example a deferred first-turn user message or staged callback
    /// tool results). Unlike `run()`, this method does NOT add a new user message;
    /// it runs directly from the session's current state.
    ///
    /// Returns an error if the session doesn't end at a resumable continuation
    /// boundary (`User` or `ToolResults`).
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

    /// Core run implementation shared by `run()` and `run_with_events()`.
    ///
    /// Adds user_input as a user message, emits lifecycle events when `event_tx`
    /// is provided, and delegates to `run_loop`.
    async fn run_inner(
        &mut self,
        user_input: ContentInput,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<RunResult, AgentError> {
        let event_tx = event_tx.or_else(|| self.default_event_tx.clone());

        self.require_runtime_execution_kind()?;

        // Reset state for new run (allows multi-turn on same agent).
        self.extraction_state.reset();
        self.run_completed_hooks_applied = false;

        // Apply canonical per-turn skill references staged by the surface.
        // Skill refs are text-only so they operate on the text projection.
        let user_input = if user_input.has_non_text_content() {
            // For multimodal input, prepend skill text to the text blocks only.
            let skill_text = self.apply_skill_ref(String::new()).await;
            if skill_text.is_empty() {
                user_input
            } else {
                // Prepend skill text as a leading text block.
                let mut blocks = vec![crate::types::ContentBlock::Text { text: skill_text }];
                blocks.extend(user_input.into_blocks());
                ContentInput::Blocks(blocks)
            }
        } else {
            let text = self.apply_skill_ref(user_input.text_content()).await;
            ContentInput::Text(text)
        };

        // Hooks/events receive the typed content input; legacy hook fields
        // still include the text projection for compatibility.
        let run_prompt_input = user_input.clone();

        // Add user message — preserve image blocks when present.
        let user_message = if user_input.has_non_text_content() {
            crate::types::UserMessage::with_blocks(user_input.into_blocks())
        } else {
            crate::types::UserMessage::text(user_input.text_content())
        };
        self.session.push(Message::User(user_message));

        self.emit_run_started_event(run_prompt_input.clone(), event_tx.as_ref())
            .await;

        if let Err(err) = self
            .run_started_hooks(&run_prompt_input, event_tx.as_ref())
            .await
        {
            self.handle_run_failure(&err, event_tx.as_ref()).await;
            self.clear_runtime_execution_kind();
            return Err(err);
        }

        match self.run_loop(event_tx.clone()).await {
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
                self.emit_run_completed_event(&result, event_tx.as_ref())
                    .await;
                self.checkpoint_current_session().await;
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

        let pending_prompt = self.session.messages().last().and_then(|m| match m {
            Message::User(u) => Some(u.text_content()),
            Message::ToolResults { .. } => Some(String::new()),
            _ => None,
        });

        let Some(prompt) = pending_prompt else {
            self.clear_runtime_execution_kind();
            return Err(AgentError::ConfigError(
                "run_pending requires a pending user or tool-results continuation boundary in the session".to_string(),
            ));
        };

        self.require_runtime_execution_kind()?;

        // Reset state for new run (allows multi-turn on same agent).
        self.extraction_state.reset();
        self.run_completed_hooks_applied = false;

        self.emit_run_started_event(ContentInput::Text(prompt.clone()), event_tx.as_ref())
            .await;

        if let Err(err) = self
            .run_started_hooks(&ContentInput::Text(prompt.clone()), event_tx.as_ref())
            .await
        {
            self.handle_run_failure(&err, event_tx.as_ref()).await;
            self.clear_runtime_execution_kind();
            return Err(err);
        }

        match self.run_loop(event_tx.clone()).await {
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
                self.emit_run_completed_event(&result, event_tx.as_ref())
                    .await;
                self.checkpoint_current_session().await;
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
    /// prepend resolved skill bodies to the next user input.
    ///
    /// Compatibility slash refs are handled at transport/resolver boundaries;
    /// core runtime no longer parses slash refs directly.
    async fn apply_skill_ref(&mut self, user_input: String) -> String {
        let engine = match &self.skill_engine {
            Some(e) => e.clone(),
            None => return user_input,
        };

        let mut prefix_parts: Vec<String> = Vec::new();

        // 1. Consume pending_skill_references (from wire format / API)
        if let Some(refs) = self.pending_skill_references.take()
            && !refs.is_empty()
        {
            let canonical_keys: Vec<crate::skills::SkillKey> = refs.into_iter().collect();
            match engine.resolve_and_render(&canonical_keys).await {
                Ok(resolved) => {
                    for skill in &resolved {
                        tracing::info!(
                            skill_key = %skill.key,
                            "Per-turn skill activation via skill_references"
                        );
                        prefix_parts.push(skill.rendered_body.clone());
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to resolve source-pinned skill_references"
                    );
                }
            }
        }

        if prefix_parts.is_empty() {
            return user_input;
        }

        if user_input.is_empty() {
            prefix_parts.join("\n\n")
        } else {
            format!("{}\n\n{user_input}", prefix_parts.join("\n\n"))
        }
    }
}
