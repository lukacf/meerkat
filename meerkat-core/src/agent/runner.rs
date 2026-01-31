//! Agent runner interface.

use crate::budget::Budget;
use crate::error::AgentError;
use crate::event::AgentEvent;
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

    /// Run the agent with a user message
    pub async fn run(&mut self, user_input: String) -> Result<RunResult, AgentError> {
        // Reset state for new run (allows multi-turn on same agent)
        self.state = LoopState::CallingLlm;

        // Add user message
        self.session.push(Message::User(crate::types::UserMessage {
            content: user_input,
        }));

        // Run the loop without event emission (no listener)
        self.run_loop(None).await
    }

    /// Run the agent with events streamed to the provided channel
    pub async fn run_with_events(
        &mut self,
        user_input: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        // Reset state for new run (allows multi-turn on same agent)
        self.state = LoopState::CallingLlm;

        let session_id = self.session.id().clone();
        let run_prompt = user_input.clone();

        // Add user message
        self.session.push(Message::User(crate::types::UserMessage {
            content: user_input,
        }));

        let _ = event_tx
            .send(AgentEvent::RunStarted {
                session_id,
                prompt: run_prompt,
            })
            .await;

        match self.run_loop(Some(event_tx.clone())).await {
            Ok(result) => Ok(result),
            Err(err) => {
                let _ = event_tx
                    .send(AgentEvent::RunFailed {
                        session_id: self.session.id().clone(),
                        error: err.to_string(),
                    })
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
}
