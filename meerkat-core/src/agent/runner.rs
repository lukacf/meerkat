//! Agent runner interface.

use crate::budget::Budget;
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::hooks::{HookDecision, HookInvocation, HookPatch, HookPoint};
use crate::retry::RetryPolicy;
use crate::service::TurnToolOverlay;
use crate::session::{PendingSystemContextAppend, Session};
use crate::state::LoopState;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_scope::{
    EXTERNAL_TOOL_FILTER_METADATA_KEY, ToolFilter, ToolScopeRevision, ToolScopeStageError,
};
use crate::types::{ContentInput, Message, RunResult};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

use super::{Agent, AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};

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

/// Outcome of a host-mode poll cycle.
#[derive(Debug)]
pub enum HostModePollOutcome {
    /// No comms work was available.
    Idle,
    /// A pending comms message was drained and processed into a turn.
    Ran(RunResult),
    /// Peer DISMISS was observed; the host-mode drain should stop.
    Dismissed,
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized,
    T: AgentToolDispatcher + ?Sized,
    S: AgentSessionStore + ?Sized,
{
    /// Stage an external tool visibility filter update for subsequent turns.
    pub fn stage_external_tool_filter(
        &mut self,
        filter: ToolFilter,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let handle = self.tool_scope.handle();
        let revision = handle.stage_external_filter(filter.clone())?;
        let _ = handle.staged_revision();
        if let Ok(value) = serde_json::to_value(filter) {
            self.session
                .set_metadata(EXTERNAL_TOOL_FILTER_METADATA_KEY, value);
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

    /// Replace the LLM client for subsequent turns.
    ///
    /// Enables hot-swapping the model/provider on a live session without
    /// rebuilding the agent. The new client takes effect on the next
    /// `run()` / `run_with_events()` call.
    pub fn replace_client(&mut self, client: Arc<C>) {
        self.client = client;
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
    pub fn state(&self) -> &LoopState {
        &self.state
    }

    /// Get the retry policy
    pub fn retry_policy(&self) -> &RetryPolicy {
        &self.retry_policy
    }

    /// Get the current nesting depth
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// Set whether a separate host-mode drain owns comms inbox consumption.
    ///
    /// Only called through the `SessionAgent` trait as the realization endpoint
    /// for `CommsDrainLifecycleEffect::SetTurnBoundaryDrainSuppressed`. Must
    /// not be called directly from shell code — use protocol helpers instead.
    pub fn set_comms_drain_active(&mut self, active: bool) {
        self.comms_drain_active.store(active, Ordering::Release);
    }

    /// Shared comms-drain control flag used by host-mode owners.
    pub fn comms_drain_control(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.comms_drain_active)
    }

    /// Poll a host-mode cycle using the existing comms inbox as the next turn source.
    pub async fn poll_host_mode_with_events(
        &mut self,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<HostModePollOutcome, AgentError> {
        if self
            .comms_arc()
            .is_some_and(|comms| comms.dismiss_received())
        {
            return Ok(HostModePollOutcome::Dismissed);
        }

        let drained = self.drain_comms_inbox().await;
        if !drained {
            return if self
                .comms_arc()
                .is_some_and(|comms| comms.dismiss_received())
            {
                Ok(HostModePollOutcome::Dismissed)
            } else {
                Ok(HostModePollOutcome::Idle)
            };
        }

        self.run_pending_with_events(event_tx)
            .await
            .map(HostModePollOutcome::Ran)
    }

    /// Get the event tap for interaction-scoped streaming.
    pub fn event_tap(&self) -> &crate::event_tap::EventTap {
        &self.event_tap
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
        session
    }

    /// Synchronize the shared system-context state into the in-memory session metadata.
    pub(crate) fn sync_system_context_state_to_session(&mut self) {
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
    #[allow(dead_code)] // Used by persistent session service; call-site wiring pending
    pub(crate) async fn checkpoint_current_session(&mut self) {
        self.sync_system_context_state_to_session();
        if let Some(ref cp) = self.checkpointer {
            cp.checkpoint(&self.session).await;
        }
    }

    async fn run_started_hooks(
        &self,
        prompt: &str,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<(), AgentError> {
        let report = self
            .execute_hooks(
                HookInvocation {
                    point: HookPoint::RunStarted,
                    session_id: self.session.id().clone(),
                    turn_number: None,
                    prompt: Some(prompt.to_string()),
                    error: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                event_tx,
            )
            .await?;

        if let Some(HookDecision::Deny {
            reason_code,
            message,
            payload,
            ..
        }) = report.decision
        {
            return Err(AgentError::HookDenied {
                point: HookPoint::RunStarted,
                reason_code,
                message,
                payload,
            });
        }
        Ok(())
    }

    async fn run_completed_hooks(
        &mut self,
        result: &mut RunResult,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<(), AgentError> {
        let report = self
            .execute_hooks(
                HookInvocation {
                    point: HookPoint::RunCompleted,
                    session_id: self.session.id().clone(),
                    turn_number: Some(result.turns),
                    prompt: None,
                    error: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                event_tx,
            )
            .await?;

        if let Some(HookDecision::Deny {
            reason_code,
            message,
            payload,
            ..
        }) = report.decision
        {
            return Err(AgentError::HookDenied {
                point: HookPoint::RunCompleted,
                reason_code,
                message,
                payload,
            });
        }

        for outcome in &report.outcomes {
            for patch in &outcome.patches {
                if let HookPatch::RunResult { text } = patch {
                    crate::event_tap::tap_emit(
                        &self.event_tap,
                        event_tx,
                        AgentEvent::HookRewriteApplied {
                            hook_id: outcome.hook_id.to_string(),
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
            },
        )
        .await;
    }

    async fn emit_run_started_event(
        &self,
        prompt: &str,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            event_tx,
            AgentEvent::RunStarted {
                session_id: self.session.id().clone(),
                prompt: prompt.to_string(),
            },
        )
        .await;
    }

    async fn emit_run_failed_event(
        &self,
        error: &AgentError,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            event_tx,
            AgentEvent::RunFailed {
                session_id: self.session.id().clone(),
                error: error.to_string(),
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
        let messages = self.session.messages_mut();
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
                HookInvocation {
                    point: HookPoint::RunFailed,
                    session_id: self.session.id().clone(),
                    turn_number: None,
                    prompt: None,
                    error: Some(error.to_string()),
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                event_tx,
            )
            .await?;

        if let Some(HookDecision::Deny {
            reason_code,
            message,
            payload,
            ..
        }) = report.decision
        {
            return Err(AgentError::HookDenied {
                point: HookPoint::RunFailed,
                reason_code,
                message,
                payload,
            });
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

    /// Run the agent using the pending user message already in the session.
    ///
    /// This is useful when the session has been pre-populated with a user message
    /// (e.g., via `create_spawn_session` or `create_fork_session`). Unlike `run()`,
    /// this method does NOT add a new user message — it runs directly from the
    /// session's current state.
    ///
    /// Returns an error if the session doesn't have a pending user message.
    pub async fn run_pending(&mut self) -> Result<RunResult, AgentError> {
        self.run_pending_inner(None).await
    }

    /// Run the agent using the pending user message, with event streaming.
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

        // Reset state for new run (allows multi-turn on same agent)
        self.state = LoopState::CallingLlm;
        self.turn_authority = crate::turn_execution_authority::TurnExecutionAuthority::new();
        self.extraction_mode = false;
        self.extraction_result = None;
        self.extraction_last_error = None;
        self.extraction_schema_warnings = None;

        // Apply canonical per-turn skill references staged by the surface.
        // Skill refs are text-only so they operate on the text projection.
        let user_input = if user_input.has_images() {
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

        // Hooks/events always see the text projection.
        let run_prompt = user_input.text_content();

        // Add user message — preserve image blocks when present.
        let user_message = if user_input.has_images() {
            crate::types::UserMessage::with_blocks(user_input.into_blocks())
        } else {
            crate::types::UserMessage::text(user_input.text_content())
        };
        self.session.push(Message::User(user_message));

        self.emit_run_started_event(&run_prompt, event_tx.as_ref())
            .await;

        if let Err(err) = self.run_started_hooks(&run_prompt, event_tx.as_ref()).await {
            self.handle_run_failure(&err, event_tx.as_ref()).await;
            return Err(err);
        }

        match self.run_loop(event_tx.clone()).await {
            Ok(mut result) => {
                if let Err(err) = self
                    .run_completed_hooks(&mut result, event_tx.as_ref())
                    .await
                {
                    self.handle_run_failure(&err, event_tx.as_ref()).await;
                    return Err(err);
                }
                self.emit_run_completed_event(&result, event_tx.as_ref())
                    .await;
                Ok(result)
            }
            Err(err) => {
                self.handle_run_failure(&err, event_tx.as_ref()).await;
                Err(err)
            }
        }
    }

    /// Core run-pending implementation shared by `run_pending()` and
    /// `run_pending_with_events()`.
    ///
    /// Uses the existing pending user message in the session (does NOT push a new one).
    /// Emits lifecycle events when `event_tx` is provided. Also used by
    /// the host-mode continuation path after response injection.
    pub(super) async fn run_pending_inner(
        &mut self,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<RunResult, AgentError> {
        let event_tx = event_tx.or_else(|| self.default_event_tx.clone());

        let pending_prompt = self.session.messages().last().and_then(|m| match m {
            Message::User(u) => Some(u.text_content()),
            _ => None,
        });

        let Some(prompt) = pending_prompt else {
            return Err(AgentError::ConfigError(
                "run_pending requires a pending user message in the session".to_string(),
            ));
        };

        // Reset state for new run (allows multi-turn on same agent)
        self.state = LoopState::CallingLlm;
        self.turn_authority = crate::turn_execution_authority::TurnExecutionAuthority::new();
        self.extraction_mode = false;
        self.extraction_result = None;
        self.extraction_last_error = None;
        self.extraction_schema_warnings = None;

        self.emit_run_started_event(&prompt, event_tx.as_ref())
            .await;

        if let Err(err) = self.run_started_hooks(&prompt, event_tx.as_ref()).await {
            self.handle_run_failure(&err, event_tx.as_ref()).await;
            return Err(err);
        }

        match self.run_loop(event_tx.clone()).await {
            Ok(mut result) => {
                if let Err(err) = self
                    .run_completed_hooks(&mut result, event_tx.as_ref())
                    .await
                {
                    self.handle_run_failure(&err, event_tx.as_ref()).await;
                    return Err(err);
                }
                self.emit_run_completed_event(&result, event_tx.as_ref())
                    .await;
                Ok(result)
            }
            Err(err) => {
                self.handle_run_failure(&err, event_tx.as_ref()).await;
                Err(err)
            }
        }
    }

    /// Cancel the current run
    pub fn cancel(&mut self) {
        use crate::turn_execution_authority::{TurnExecutionInput, TurnExecutionMutator};

        // Route through the authority whenever cancellation is requested.
        let input = if let Some(run_id) = self.turn_authority.active_run().cloned() {
            TurnExecutionInput::CancelNow { run_id }
        } else {
            TurnExecutionInput::ForceCancelNoRun
        };
        if let Ok(transition) = self.turn_authority.apply(input) {
            self.state = transition.next_phase.to_loop_state();
        }
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
            let canonical_ids: Vec<crate::skills::SkillId> = refs
                .into_iter()
                .map(|key| {
                    crate::skills::SkillId(format!("{}/{}", key.source_uuid, key.skill_name))
                })
                .collect();
            match engine.resolve_and_render(&canonical_ids).await {
                Ok(resolved) => {
                    for skill in &resolved {
                        tracing::info!(
                            skill_id = %skill.id.0,
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
