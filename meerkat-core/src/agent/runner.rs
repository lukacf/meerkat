//! Agent runner interface.

use crate::budget::Budget;
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::hooks::{HookDecision, HookInvocation, HookPatch, HookPoint};
use crate::ops::{
    ForkBranch, ForkBudgetPolicy, OperationId, OperationResult, SpawnSpec, ToolAccessPolicy,
};
use crate::retry::RetryPolicy;
use crate::session::Session;
use crate::state::LoopState;
use crate::types::{Message, RunResult};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{
    Agent, AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    FilteredToolDispatcher,
};

/// Minimal runner interface for an Agent.
#[async_trait]
pub trait AgentRunner: Send {
    async fn run(&mut self, prompt: String) -> Result<RunResult, AgentError>;

    async fn run_with_events(
        &mut self,
        prompt: String,
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError>;
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

    /// Get the event tap for interaction-scoped streaming.
    pub fn event_tap(&self) -> &crate::event_tap::EventTap {
        &self.event_tap
    }

    /// Spawn a new sub-agent with minimal context
    ///
    /// The sub-agent runs independently with its own budget and tool access.
    /// Results are collected at turn boundaries.
    pub async fn spawn(&self, spec: SpawnSpec) -> Result<OperationId, AgentError> {
        // Check depth limit
        if self.depth + 1 > self.sub_agent_manager.limits.max_depth {
            return Err(AgentError::DepthLimitExceeded {
                depth: self.depth + 1,
                max: self.sub_agent_manager.limits.max_depth,
            });
        }

        // Check if we can spawn more sub-agents
        if !self.sub_agent_manager.can_spawn().await {
            return Err(AgentError::SubAgentLimitExceeded {
                limit: self.sub_agent_manager.limits.max_concurrent_agents,
            });
        }

        // Validate tool access policy
        let all_tools = self.tools.tools();
        let allowed_tools = self
            .sub_agent_manager
            .apply_tool_access_policy(&all_tools, &spec.tool_access);

        if let ToolAccessPolicy::AllowList(ref names) = spec.tool_access {
            for name in names {
                if !all_tools.iter().any(|t| &t.name == name) {
                    return Err(AgentError::InvalidToolAccess { tool: name.clone() });
                }
            }
        }

        // Apply context strategy to get messages for sub-agent
        let messages = self
            .sub_agent_manager
            .apply_context_strategy(&self.session, &spec.context);

        // Create sub-agent session with context
        let mut sub_session = Session::new();
        for msg in messages {
            sub_session.push(msg);
        }
        if let Some(sys_prompt) = &spec.system_prompt {
            sub_session.set_system_prompt(sys_prompt.clone());
        }

        // Generate operation ID
        let op_id = OperationId::new();

        // Register the sub-agent
        self.sub_agent_manager
            .register(op_id.clone(), "spawn".to_string())
            .await?;

        // Clone components for the spawned task
        let client = self.client.clone();
        let store = self.store.clone();
        let prompt = spec.prompt.clone();
        let budget = spec.budget.clone();
        let sub_agent_manager = self.sub_agent_manager.clone();
        let op_id_clone = op_id.clone();
        let depth = self.depth + 1;
        let model = self.config.model.clone();
        let max_tokens = self.config.max_tokens_per_turn;

        // Create filtered tools based on policy
        let allowed_tool_names: Vec<String> =
            allowed_tools.iter().map(|t| t.name.clone()).collect();
        let filtered_tools = Arc::new(FilteredToolDispatcher::new(
            self.tools.clone(),
            allowed_tool_names,
        ));

        // Spawn the sub-agent in a background task
        tokio::spawn(async move {
            let start = std::time::Instant::now();

            // Build sub-agent with filtered tools
            let mut sub_agent = AgentBuilder::new()
                .model(&model)
                .max_tokens_per_turn(max_tokens)
                .budget(budget)
                .resume_session(sub_session)
                .build(client, filtered_tools, store)
                .await;

            // Run the sub-agent
            let result = sub_agent.run(prompt).await;

            // Report completion
            match result {
                Ok(run_result) => {
                    sub_agent_manager
                        .complete(
                            &op_id_clone,
                            OperationResult {
                                id: op_id_clone.clone(),
                                content: run_result.text,
                                is_error: false,
                                duration_ms: start.elapsed().as_millis() as u64,
                                tokens_used: run_result.usage.total_tokens(),
                            },
                        )
                        .await;
                }
                Err(e) => {
                    sub_agent_manager.fail(&op_id_clone, e.to_string()).await;
                }
            }
        });

        tracing::info!(
            "Spawned sub-agent {} at depth {} with {} tools",
            op_id,
            depth,
            allowed_tools.len()
        );

        Ok(op_id)
    }

    /// Fork the current conversation into parallel branches
    ///
    /// Each branch gets a copy of the full conversation history and runs independently.
    pub async fn fork(
        &self,
        branches: Vec<ForkBranch>,
        budget_policy: ForkBudgetPolicy,
    ) -> Result<Vec<OperationId>, AgentError> {
        // Check depth limit
        if self.depth + 1 > self.sub_agent_manager.limits.max_depth {
            return Err(AgentError::DepthLimitExceeded {
                depth: self.depth + 1,
                max: self.sub_agent_manager.limits.max_depth,
            });
        }

        // Check if we can spawn enough sub-agents
        let running = self.sub_agent_manager.running_ids().await.len();
        if running + branches.len() > self.sub_agent_manager.limits.max_concurrent_agents {
            return Err(AgentError::SubAgentLimitExceeded {
                limit: self.sub_agent_manager.limits.max_concurrent_agents,
            });
        }

        // Allocate budget for each branch
        let remaining_tokens = self.budget.remaining();
        let budgets = self.sub_agent_manager.allocate_fork_budget(
            remaining_tokens,
            branches.len(),
            &budget_policy,
        );

        let mut op_ids = Vec::with_capacity(branches.len());

        for (i, branch) in branches.into_iter().enumerate() {
            let op_id = OperationId::new();

            // Validate tool access if specified
            if let Some(ToolAccessPolicy::AllowList(names)) = &branch.tool_access {
                let all_tools = self.tools.tools();
                for name in names {
                    if !all_tools.iter().any(|t| &t.name == name) {
                        return Err(AgentError::InvalidToolAccess { tool: name.clone() });
                    }
                }
            }

            // Register the branch as a sub-agent
            self.sub_agent_manager
                .register(op_id.clone(), branch.name.clone())
                .await?;

            // Apply tool access policy for this branch
            let all_tools = self.tools.tools();
            let allowed_tools = match &branch.tool_access {
                Some(policy) => self
                    .sub_agent_manager
                    .apply_tool_access_policy(&all_tools, policy),
                None => all_tools.to_vec(), // Inherit all tools
            };
            let allowed_tool_names: Vec<String> =
                allowed_tools.iter().map(|t| t.name.clone()).collect();
            let filtered_tools = Arc::new(FilteredToolDispatcher::new(
                self.tools.clone(),
                allowed_tool_names,
            ));

            // Clone components for the spawned task
            let client = self.client.clone();
            let store = self.store.clone();
            let prompt = branch.prompt.clone();
            let budget = budgets[i].clone();
            let sub_agent_manager = self.sub_agent_manager.clone();
            let op_id_clone = op_id.clone();
            let model = self.config.model.clone();
            let max_tokens = self.config.max_tokens_per_turn;
            let branch_name = branch.name.clone();

            // Create session with full history (fork uses FullHistory context)
            let mut fork_session = Session::new();
            for msg in self.session.messages() {
                fork_session.push(msg.clone());
            }

            // Spawn the branch in a background task
            tokio::spawn(async move {
                let start = std::time::Instant::now();

                // Build sub-agent for this branch with filtered tools
                let mut sub_agent = AgentBuilder::new()
                    .model(&model)
                    .max_tokens_per_turn(max_tokens)
                    .budget(budget)
                    .resume_session(fork_session)
                    .build(client, filtered_tools, store)
                    .await;

                // Run the sub-agent with the branch prompt
                let result = sub_agent.run(prompt).await;

                // Report completion
                match result {
                    Ok(run_result) => {
                        sub_agent_manager
                            .complete(
                                &op_id_clone,
                                OperationResult {
                                    id: op_id_clone.clone(),
                                    content: format!("[{}] {}", branch_name, run_result.text),
                                    is_error: false,
                                    duration_ms: start.elapsed().as_millis() as u64,
                                    tokens_used: run_result.usage.total_tokens(),
                                },
                            )
                            .await;
                    }
                    Err(e) => {
                        sub_agent_manager.fail(&op_id_clone, e.to_string()).await;
                    }
                }
            });

            tracing::info!(
                "Forked branch '{}' as {} at depth {}",
                branch.name,
                op_id,
                self.depth + 1
            );

            op_ids.push(op_id);
        }

        Ok(op_ids)
    }

    /// Cancel a running sub-agent
    pub async fn cancel_sub_agent(&self, op_id: &OperationId) {
        self.sub_agent_manager.cancel(op_id).await;
    }

    /// Collect completed sub-agent results (called at turn boundaries)
    pub async fn collect_sub_agent_results(&self) -> Vec<OperationResult> {
        self.sub_agent_manager.collect_completed().await
    }

    /// Check if there are running sub-agents
    pub async fn has_running_sub_agents(&self) -> bool {
        self.sub_agent_manager.has_running().await
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
                    result.text = text.clone();
                    self.apply_run_result_text_patch(text);
                }
            }
        }
        if let Err(err) = self.store.save(&self.session).await {
            tracing::warn!("Failed to save session after run_completed hooks: {}", err);
        }
        Ok(())
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

    /// Run the agent with a user message
    pub async fn run(&mut self, user_input: String) -> Result<RunResult, AgentError> {
        // Reset state for new run (allows multi-turn on same agent)
        self.state = LoopState::CallingLlm;

        // Detect /skill-ref at start of message for per-turn activation
        let user_input = self.apply_skill_ref(user_input).await;

        // Add user message
        self.session.push(Message::User(crate::types::UserMessage {
            content: user_input.clone(),
        }));

        self.run_started_hooks(&user_input, None).await?;

        match self.run_loop(None).await {
            Ok(mut result) => {
                self.run_completed_hooks(&mut result, None).await?;
                Ok(result)
            }
            Err(err) => {
                if let Err(hook_err) = self.run_failed_hooks(&err, None).await {
                    tracing::warn!(?hook_err, "run_failed hook execution failed");
                }
                Err(err)
            }
        }
    }

    /// Run the agent with events streamed to the provided channel
    pub async fn run_with_events(
        &mut self,
        user_input: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        // Reset state for new run (allows multi-turn on same agent)
        self.state = LoopState::CallingLlm;

        // Detect /skill-ref at start of message for per-turn activation
        let user_input = self.apply_skill_ref(user_input).await;

        let session_id = self.session.id().clone();
        let run_prompt = user_input.clone();

        // Add user message
        self.session.push(Message::User(crate::types::UserMessage {
            content: user_input,
        }));

        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            Some(&event_tx),
            AgentEvent::RunStarted {
                session_id,
                prompt: run_prompt.clone(),
            },
        )
        .await;

        self.run_started_hooks(&run_prompt, Some(&event_tx)).await?;

        match self.run_loop(Some(event_tx.clone())).await {
            Ok(mut result) => {
                self.run_completed_hooks(&mut result, Some(&event_tx))
                    .await?;
                Ok(result)
            }
            Err(err) => {
                if let Err(hook_err) = self.run_failed_hooks(&err, Some(&event_tx)).await {
                    tracing::warn!(?hook_err, "run_failed hook execution failed");
                }
                let _ = crate::event_tap::tap_emit(
                    &self.event_tap,
                    Some(&event_tx),
                    AgentEvent::RunFailed {
                        session_id: self.session.id().clone(),
                        error: err.to_string(),
                    },
                )
                .await;
                Err(err)
            }
        }
    }

    /// Run the agent using the pending user message already in the session.
    ///
    /// This is useful when the session has been pre-populated with a user message
    /// (e.g., via `create_spawn_session` or `create_fork_session`). Unlike `run()`,
    /// this method does NOT add a new user message - it runs directly from the
    /// session's current state.
    ///
    /// Returns an error if the session doesn't have a pending user message.
    pub async fn run_pending(&mut self) -> Result<RunResult, AgentError> {
        let has_pending_user_message = self
            .session
            .messages()
            .last()
            .is_some_and(|m| matches!(m, Message::User(_)));

        if !has_pending_user_message {
            return Err(AgentError::ConfigError(
                "run_pending requires a pending user message in the session".to_string(),
            ));
        }

        // Reset state for new run (allows multi-turn on same agent)
        self.state = LoopState::CallingLlm;

        let pending_prompt = self
            .session
            .messages()
            .last()
            .and_then(|msg| match msg {
                Message::User(user) => Some(user.content.clone()),
                _ => None,
            })
            .unwrap_or_default();

        self.run_started_hooks(&pending_prompt, None).await?;

        // Run the loop without adding a message
        match self.run_loop(None).await {
            Ok(mut result) => {
                self.run_completed_hooks(&mut result, None).await?;
                Ok(result)
            }
            Err(err) => {
                if let Err(hook_err) = self.run_failed_hooks(&err, None).await {
                    tracing::warn!(?hook_err, "run_failed hook execution failed");
                }
                Err(err)
            }
        }
    }

    /// Run the agent using the pending user message, with event streaming.
    ///
    /// Like `run_pending()`, but emits events to the provided channel.
    pub async fn run_pending_with_events(
        &mut self,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        let pending_prompt = self.session.messages().last().and_then(|m| match m {
            Message::User(u) => Some(u.content.clone()),
            _ => None,
        });

        let Some(prompt) = pending_prompt else {
            return Err(AgentError::ConfigError(
                "run_pending_with_events requires a pending user message in the session"
                    .to_string(),
            ));
        };

        // Reset state for new run (allows multi-turn on same agent)
        self.state = LoopState::CallingLlm;

        let session_id = self.session.id().clone();

        let _ = crate::event_tap::tap_emit(
            &self.event_tap,
            Some(&event_tx),
            AgentEvent::RunStarted {
                session_id,
                prompt: prompt.clone(),
            },
        )
        .await;

        self.run_started_hooks(&prompt, Some(&event_tx)).await?;

        match self.run_loop(Some(event_tx.clone())).await {
            Ok(mut result) => {
                self.run_completed_hooks(&mut result, Some(&event_tx))
                    .await?;
                Ok(result)
            }
            Err(err) => {
                if let Err(hook_err) = self.run_failed_hooks(&err, Some(&event_tx)).await {
                    tracing::warn!(?hook_err, "run_failed hook execution failed");
                }
                let _ = crate::event_tap::tap_emit(
                    &self.event_tap,
                    Some(&event_tx),
                    AgentEvent::RunFailed {
                        session_id: self.session.id().clone(),
                        error: err.to_string(),
                    },
                )
                .await;
                Err(err)
            }
        }
    }

    /// Cancel the current run
    pub fn cancel(&mut self) {
        if !self.state.is_terminal() {
            let _ = self.state.transition(LoopState::Cancelling);
        }
    }

    /// Detect and resolve `/skill-ref` patterns in user input.
    ///
    /// If the message starts with `/skill-id`, resolves the skill body via the
    /// skill engine and prepends it to the user message. Returns the original
    /// message unchanged if no skill ref is found or no engine is configured.
    /// Detect `/skill-ref` in user input AND consume any pending
    /// `skill_references` staged by the surface. Returns the user input
    /// with resolved skill bodies prepended.
    async fn apply_skill_ref(&mut self, user_input: String) -> String {
        let engine = match &self.skill_engine {
            Some(e) => e.clone(),
            None => return user_input,
        };

        let mut prefix_parts: Vec<String> = Vec::new();

        // 1. Consume pending_skill_references (from wire format / API)
        if let Some(refs) = self.pending_skill_references.take() {
            if !refs.is_empty() {
                match engine.resolve_and_render(&refs).await {
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
                        tracing::warn!(error = %e, "Failed to resolve skill_references");
                    }
                }
            }
        }

        // 2. Detect /skill-ref at start of message
        let (remaining_input, detected_ref) =
            if let Some((skill_id, remaining)) = super::skills::detect_skill_ref(&user_input) {
                let skill_id_owned = crate::skills::SkillId(skill_id.to_string());
                match engine.resolve_and_render(&[skill_id_owned]).await {
                    Ok(resolved) if !resolved.is_empty() => {
                        tracing::info!(skill_id, "Per-turn skill activation via /skill-ref");
                        prefix_parts.push(resolved[0].rendered_body.clone());
                        (remaining.to_string(), true)
                    }
                    Ok(_) => {
                        tracing::warn!(skill_id, "Skill ref detected but resolved to empty");
                        (user_input.clone(), false)
                    }
                    Err(e) => {
                        tracing::warn!(skill_id, error = %e, "Failed to resolve skill ref");
                        (user_input.clone(), false)
                    }
                }
            } else {
                (user_input.clone(), false)
            };

        if prefix_parts.is_empty() {
            return user_input;
        }

        let text = if detected_ref {
            remaining_input
        } else {
            user_input
        };
        if text.is_empty() {
            prefix_parts.join("\n\n")
        } else {
            format!("{}\n\n{text}", prefix_parts.join("\n\n"))
        }
    }
}
