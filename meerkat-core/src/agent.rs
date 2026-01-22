//! Agent - the core agent orchestrator
//!
//! The Agent struct ties together all components and runs the agent loop.

use crate::budget::{Budget, BudgetLimits};
use crate::config::AgentConfig;
use crate::error::AgentError;
use crate::event::{AgentEvent, BudgetType};
use crate::ops::{
    ConcurrencyLimits, ContextStrategy, ForkBranch, ForkBudgetPolicy, OperationId,
    OperationResult, SpawnSpec, SteeringHandle, ToolAccessPolicy,
};
use crate::retry::RetryPolicy;
use crate::session::Session;
use crate::state::LoopState;
use crate::sub_agent::{inject_steering_messages, SubAgentManager};
use crate::types::{AssistantMessage, Message, RunResult, StopReason, ToolCall, ToolDef, ToolResult, Usage};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Trait for LLM clients that can be used with the agent
#[async_trait]
pub trait AgentLlmClient: Send + Sync {
    /// Stream a response from the LLM
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
    ) -> Result<LlmStreamResult, AgentError>;

    /// Get the provider name
    fn provider(&self) -> &'static str;
}

/// Result of streaming from the LLM
pub struct LlmStreamResult {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

/// Trait for tool dispatchers
#[async_trait]
pub trait AgentToolDispatcher: Send + Sync {
    /// Get available tool definitions
    fn tools(&self) -> Vec<ToolDef>;

    /// Execute a tool call
    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String>;
}

/// A tool dispatcher that filters tools based on a policy
pub struct FilteredToolDispatcher<T: AgentToolDispatcher> {
    inner: Arc<T>,
    allowed_tools: Vec<String>,
}

impl<T: AgentToolDispatcher> FilteredToolDispatcher<T> {
    pub fn new(inner: Arc<T>, allowed_tools: Vec<String>) -> Self {
        Self { inner, allowed_tools }
    }
}

#[async_trait]
impl<T: AgentToolDispatcher + 'static> AgentToolDispatcher for FilteredToolDispatcher<T> {
    fn tools(&self) -> Vec<ToolDef> {
        self.inner
            .tools()
            .into_iter()
            .filter(|t| self.allowed_tools.contains(&t.name))
            .collect()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        if !self.allowed_tools.contains(&name.to_string()) {
            return Err(format!("Tool '{}' is not allowed by policy", name));
        }
        self.inner.dispatch(name, args).await
    }
}

/// Trait for session stores
#[async_trait]
pub trait AgentSessionStore: Send + Sync {
    /// Save a session
    async fn save(&self, session: &Session) -> Result<(), AgentError>;

    /// Load a session by ID
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError>;
}

/// Builder for creating an Agent
pub struct AgentBuilder {
    config: AgentConfig,
    system_prompt: Option<String>,
    budget_limits: Option<BudgetLimits>,
    retry_policy: RetryPolicy,
    session: Option<Session>,
    concurrency_limits: ConcurrencyLimits,
    depth: u32,
}

impl AgentBuilder {
    /// Create a new agent builder with default config
    pub fn new() -> Self {
        Self {
            config: AgentConfig::default(),
            system_prompt: None,
            budget_limits: None,
            retry_policy: RetryPolicy::default(),
            session: None,
            concurrency_limits: ConcurrencyLimits::default(),
            depth: 0,
        }
    }

    /// Set concurrency limits for sub-agents
    pub fn concurrency_limits(mut self, limits: ConcurrencyLimits) -> Self {
        self.concurrency_limits = limits;
        self
    }

    /// Set the nesting depth (internal, used for sub-agents)
    pub(crate) fn depth(mut self, depth: u32) -> Self {
        self.depth = depth;
        self
    }

    /// Set the model to use
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    /// Set the system prompt
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set max tokens per turn
    pub fn max_tokens_per_turn(mut self, tokens: u32) -> Self {
        self.config.max_tokens_per_turn = tokens;
        self
    }

    /// Set temperature
    pub fn temperature(mut self, temp: f32) -> Self {
        self.config.temperature = Some(temp);
        self
    }

    /// Set budget limits
    pub fn budget(mut self, limits: BudgetLimits) -> Self {
        self.budget_limits = Some(limits);
        self
    }

    /// Set retry policy for LLM calls
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    /// Resume from an existing session
    pub fn resume_session(mut self, session: Session) -> Self {
        self.session = Some(session);
        self
    }

    /// Build the agent
    pub fn build<C, T, S>(
        self,
        client: Arc<C>,
        tools: Arc<T>,
        store: Arc<S>,
    ) -> Agent<C, T, S>
    where
        C: AgentLlmClient,
        T: AgentToolDispatcher,
        S: AgentSessionStore,
    {
        let session = self.session.unwrap_or_else(|| {
            let mut s = Session::new();
            if let Some(prompt) = &self.system_prompt {
                s.set_system_prompt(prompt.clone());
            }
            s
        });

        let budget = Budget::new(self.budget_limits.unwrap_or_default());
        let sub_agent_manager = Arc::new(SubAgentManager::new(self.concurrency_limits, self.depth));

        // Create steering channel for receiving steering messages from parent
        let (steering_tx, steering_rx) = mpsc::channel(16);

        Agent {
            config: self.config,
            client,
            tools,
            store,
            session,
            budget,
            retry_policy: self.retry_policy,
            state: LoopState::CallingLlm,
            sub_agent_manager,
            depth: self.depth,
            steering_rx,
            steering_tx,
        }
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self {
            config: AgentConfig::default(),
            system_prompt: None,
            budget_limits: None,
            retry_policy: RetryPolicy::default(),
            session: None,
            concurrency_limits: ConcurrencyLimits::default(),
            depth: 0,
        }
    }
}

/// The main Agent struct
pub struct Agent<C, T, S>
where
    C: AgentLlmClient,
    T: AgentToolDispatcher,
    S: AgentSessionStore,
{
    config: AgentConfig,
    client: Arc<C>,
    tools: Arc<T>,
    store: Arc<S>,
    session: Session,
    budget: Budget,
    retry_policy: RetryPolicy,
    state: LoopState,
    sub_agent_manager: Arc<SubAgentManager>,
    depth: u32,
    steering_rx: mpsc::Receiver<crate::ops::SteeringMessage>,
    steering_tx: mpsc::Sender<crate::ops::SteeringMessage>,
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + 'static,
    T: AgentToolDispatcher + 'static,
    S: AgentSessionStore + 'static,
{
    /// Create a new agent builder
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// Get the current session
    pub fn session(&self) -> &Session {
        &self.session
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

    /// Get the steering sender (for parent to use when spawning this agent)
    pub fn steering_sender(&self) -> mpsc::Sender<crate::ops::SteeringMessage> {
        self.steering_tx.clone()
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
        let allowed_tools = self.sub_agent_manager.apply_tool_access_policy(&all_tools, &spec.tool_access);

        if let ToolAccessPolicy::AllowList(ref names) = spec.tool_access {
            for name in names {
                if !all_tools.iter().any(|t| &t.name == name) {
                    return Err(AgentError::InvalidToolAccess {
                        tool: name.clone(),
                    });
                }
            }
        }

        // Apply context strategy to get messages for sub-agent
        let messages = self.sub_agent_manager.apply_context_strategy(&self.session, &spec.context);

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

        // Create steering channel for this sub-agent
        let (steering_tx, _steering_rx) = mpsc::channel(16);

        // Register the sub-agent
        self.sub_agent_manager
            .register(op_id.clone(), "spawn".to_string(), steering_tx)
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
        let allowed_tool_names: Vec<String> = allowed_tools.iter().map(|t| t.name.clone()).collect();
        let filtered_tools = Arc::new(FilteredToolDispatcher::new(self.tools.clone(), allowed_tool_names));

        // Spawn the sub-agent in a background task
        tokio::spawn(async move {
            let start = std::time::Instant::now();

            // Build sub-agent with filtered tools
            let mut sub_agent = AgentBuilder::new()
                .model(&model)
                .max_tokens_per_turn(max_tokens)
                .budget(budget)
                .resume_session(sub_session)
                .build(client, filtered_tools, store);

            // Run the sub-agent
            let result = sub_agent.run(prompt).await;

            // Report completion
            match result {
                Ok(run_result) => {
                    sub_agent_manager.complete(
                        &op_id_clone,
                        OperationResult {
                            id: op_id_clone.clone(),
                            content: run_result.text,
                            is_error: false,
                            duration_ms: start.elapsed().as_millis() as u64,
                            tokens_used: run_result.usage.total_tokens() as u64,
                        },
                    ).await;
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
            if let Some(policy) = &branch.tool_access {
                if let ToolAccessPolicy::AllowList(names) = policy {
                    let all_tools = self.tools.tools();
                    for name in names {
                        if !all_tools.iter().any(|t| &t.name == name) {
                            return Err(AgentError::InvalidToolAccess {
                                tool: name.clone(),
                            });
                        }
                    }
                }
            }

            // Create steering channel
            let (steering_tx, _steering_rx) = mpsc::channel(16);

            // Register the branch as a sub-agent
            self.sub_agent_manager
                .register(op_id.clone(), branch.name.clone(), steering_tx)
                .await?;

            // Apply tool access policy for this branch
            let all_tools = self.tools.tools();
            let allowed_tools = match &branch.tool_access {
                Some(policy) => self.sub_agent_manager.apply_tool_access_policy(&all_tools, policy),
                None => all_tools, // Inherit all tools
            };
            let allowed_tool_names: Vec<String> = allowed_tools.iter().map(|t| t.name.clone()).collect();
            let filtered_tools = Arc::new(FilteredToolDispatcher::new(self.tools.clone(), allowed_tool_names));

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
                    .build(client, filtered_tools, store);

                // Run the sub-agent with the branch prompt
                let result = sub_agent.run(prompt).await;

                // Report completion
                match result {
                    Ok(run_result) => {
                        sub_agent_manager.complete(
                            &op_id_clone,
                            OperationResult {
                                id: op_id_clone.clone(),
                                content: format!("[{}] {}", branch_name, run_result.text),
                                is_error: false,
                                duration_ms: start.elapsed().as_millis() as u64,
                                tokens_used: run_result.usage.total_tokens() as u64,
                            },
                        ).await;
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

    /// Send a steering message to a running sub-agent
    pub async fn steer(&self, op_id: &OperationId, message: String) -> Result<SteeringHandle, AgentError> {
        self.sub_agent_manager.steer(op_id, message).await
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

    /// Drain pending steering messages and inject them into session
    async fn apply_pending_steering(&mut self) {
        let mut messages = Vec::new();

        // Drain all pending steering messages from the channel
        while let Ok(msg) = self.steering_rx.try_recv() {
            messages.push(msg);
        }

        if !messages.is_empty() {
            inject_steering_messages(&mut self.session, messages);
        }
    }

    /// Call LLM with retry logic
    async fn call_llm_with_retry(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
    ) -> Result<LlmStreamResult, AgentError> {
        let mut attempt = 0u32;

        loop {
            // Wait for retry delay if not first attempt
            if attempt > 0 {
                let delay = self.retry_policy.delay_for_attempt(attempt);
                tokio::time::sleep(delay).await;
            }

            match self.client.stream_response(messages, tools, max_tokens).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    // Check if we should retry
                    if e.is_recoverable() && self.retry_policy.should_retry(attempt) {
                        tracing::warn!(
                            "LLM call failed (attempt {}), retrying: {}",
                            attempt + 1,
                            e
                        );
                        attempt += 1;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Run the agent with a user message
    pub async fn run(&mut self, user_input: String) -> Result<RunResult, AgentError> {
        // Reset state for new run (allows multi-turn on same agent)
        self.state = LoopState::CallingLlm;

        // Add user message
        self.session.push(Message::User(crate::types::UserMessage {
            content: user_input,
        }));

        // Create event channel (for future streaming)
        let (tx, _rx) = mpsc::channel::<AgentEvent>(100);

        // Run the loop
        self.run_loop(tx).await
    }

    /// Run the agent with events streamed to the provided channel
    pub async fn run_with_events(
        &mut self,
        user_input: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        // Reset state for new run (allows multi-turn on same agent)
        self.state = LoopState::CallingLlm;

        // Add user message
        self.session.push(Message::User(crate::types::UserMessage {
            content: user_input,
        }));

        self.run_loop(event_tx).await
    }

    /// The main agent loop
    async fn run_loop(&mut self, event_tx: mpsc::Sender<AgentEvent>) -> Result<RunResult, AgentError> {
        let mut turn_count = 0u32;
        let max_turns = self.config.max_turns.unwrap_or(100);
        let mut tool_call_count = 0u32;

        loop {
            // Check turn limit
            if turn_count >= max_turns {
                self.state = LoopState::Completed;
                return Ok(self.build_result(turn_count, tool_call_count));
            }

            // Check budget
            if self.budget.is_exhausted() {
                let _ = event_tx
                    .send(AgentEvent::BudgetWarning {
                        budget_type: BudgetType::Tokens,
                        used: self.session.total_tokens(),
                        limit: self.budget.remaining(),
                        percent: 1.0,
                    })
                    .await;
                self.state = LoopState::Completed;
                return Ok(self.build_result(turn_count, tool_call_count));
            }

            match self.state {
                LoopState::CallingLlm => {
                    // Emit turn start
                    let _ = event_tx
                        .send(AgentEvent::TurnStarted {
                            turn_number: turn_count,
                        })
                        .await;

                    // Get tool definitions
                    let tool_defs = self.tools.tools();

                    // Call LLM with retry
                    let result = self
                        .call_llm_with_retry(
                            self.session.messages(),
                            &tool_defs,
                            self.config.max_tokens_per_turn,
                        )
                        .await?;

                    // Update budget
                    self.budget.record_usage(&result.usage);

                    // Check if we have tool calls
                    if !result.tool_calls.is_empty() {
                        // Add assistant message with tool calls
                        self.session.push(Message::Assistant(AssistantMessage {
                            content: result.content,
                            tool_calls: result.tool_calls.clone(),
                            stop_reason: result.stop_reason,
                            usage: result.usage,
                        }));

                        // Emit tool call requests
                        for tc in &result.tool_calls {
                            let _ = event_tx
                                .send(AgentEvent::ToolCallRequested {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                    args: tc.args.clone(),
                                })
                                .await;
                        }

                        // Transition to waiting for ops
                        self.state.transition(LoopState::WaitingForOps)?;

                        // Execute tool calls
                        let mut tool_results = Vec::new();
                        for tc in result.tool_calls {
                            // Emit execution start
                            let _ = event_tx
                                .send(AgentEvent::ToolExecutionStarted {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                })
                                .await;

                            let start = std::time::Instant::now();
                            let dispatch_result = self.tools.dispatch(&tc.name, &tc.args).await;
                            let duration_ms = start.elapsed().as_millis() as u64;

                            let (content, is_error) = match dispatch_result {
                                Ok(c) => (c, false),
                                Err(e) => (e, true),
                            };

                            // Emit execution complete
                            let _ = event_tx
                                .send(AgentEvent::ToolExecutionCompleted {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                    result: content.clone(),
                                    is_error,
                                    duration_ms,
                                })
                                .await;

                            // Emit result received
                            let _ = event_tx
                                .send(AgentEvent::ToolResultReceived {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                    is_error,
                                })
                                .await;

                            tool_results.push(ToolResult {
                                tool_use_id: tc.id,
                                content,
                                is_error,
                            });

                            // Track tool call in budget
                            self.budget.record_tool_call();
                            tool_call_count += 1;
                        }

                        // Add tool results to session
                        self.session.push(Message::ToolResults { results: tool_results });

                        // Go through DrainingEvents to CallingLlm (state machine requires this)
                        self.state.transition(LoopState::DrainingEvents)?;

                        // === TURN BOUNDARY: Apply steering and collect sub-agent results ===

                        // Apply any pending steering messages from parent
                        self.apply_pending_steering().await;

                        // Collect completed sub-agent results and inject into session
                        let sub_agent_results = self.collect_sub_agent_results().await;
                        if !sub_agent_results.is_empty() {
                            // Inject sub-agent results as tool results
                            let results: Vec<ToolResult> = sub_agent_results
                                .into_iter()
                                .map(|r| ToolResult {
                                    tool_use_id: r.id.to_string(),
                                    content: r.content,
                                    is_error: r.is_error,
                                })
                                .collect();
                            self.session.push(Message::ToolResults { results });
                        }

                        // === END TURN BOUNDARY ===

                        self.state.transition(LoopState::CallingLlm)?;
                        turn_count += 1;
                    } else {
                        // No tool calls - we're done
                        let final_text = result.content.clone();
                        self.session.push(Message::Assistant(AssistantMessage {
                            content: result.content,
                            tool_calls: vec![],
                            stop_reason: result.stop_reason,
                            usage: result.usage.clone(),
                        }));

                        // Emit turn completed
                        let _ = event_tx
                            .send(AgentEvent::TurnCompleted {
                                stop_reason: result.stop_reason,
                                usage: result.usage,
                            })
                            .await;

                        // Transition to completed
                        self.state.transition(LoopState::DrainingEvents)?;
                        self.state.transition(LoopState::Completed)?;

                        // Save session
                        if let Err(e) = self.store.save(&self.session).await {
                            tracing::warn!("Failed to save session: {}", e);
                        }

                        // Emit run completed
                        let _ = event_tx
                            .send(AgentEvent::RunCompleted {
                                session_id: self.session.id().clone(),
                                result: final_text.clone(),
                                usage: self.session.total_usage(),
                            })
                            .await;

                        return Ok(RunResult {
                            text: final_text,
                            session_id: self.session.id().clone(),
                            usage: self.session.total_usage(),
                            turns: turn_count + 1,
                            tool_calls: tool_call_count,
                        });
                    }
                }
                LoopState::WaitingForOps => {
                    // This state is handled inline above
                    unreachable!("WaitingForOps handled inline");
                }
                LoopState::DrainingEvents => {
                    // Wait for any pending events to be processed
                    self.state.transition(LoopState::Completed)?;
                }
                LoopState::Cancelling => {
                    // Handle cancellation
                    self.state.transition(LoopState::Completed)?;
                    return Ok(self.build_result(turn_count, tool_call_count));
                }
                LoopState::ErrorRecovery => {
                    // Attempt recovery
                    self.state.transition(LoopState::CallingLlm)?;
                }
                LoopState::Completed => {
                    return Ok(self.build_result(turn_count, tool_call_count));
                }
            }
        }
    }

    /// Build a RunResult from current state
    fn build_result(&self, turns: u32, tool_calls: u32) -> RunResult {
        RunResult {
            text: self.session.last_assistant_text().unwrap_or("").to_string(),
            session_id: self.session.id().clone(),
            usage: self.session.total_usage(),
            turns,
            tool_calls,
        }
    }

    /// Cancel the current run
    pub fn cancel(&mut self) {
        if !self.state.is_terminal() {
            let _ = self.state.transition(LoopState::Cancelling);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mock LLM client for testing
    struct MockLlmClient {
        responses: Mutex<Vec<LlmStreamResult>>,
    }

    impl MockLlmClient {
        fn new(responses: Vec<LlmStreamResult>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl AgentLlmClient for MockLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[ToolDef],
            _max_tokens: u32,
        ) -> Result<LlmStreamResult, AgentError> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(LlmStreamResult {
                    content: "Default response".to_string(),
                    tool_calls: vec![],
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                })
            } else {
                Ok(responses.remove(0))
            }
        }

        fn provider(&self) -> &'static str {
            "mock"
        }
    }

    // Mock tool dispatcher
    struct MockToolDispatcher {
        tools: Vec<ToolDef>,
    }

    impl MockToolDispatcher {
        fn new(tools: Vec<ToolDef>) -> Self {
            Self { tools }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MockToolDispatcher {
        fn tools(&self) -> Vec<ToolDef> {
            self.tools.clone()
        }

        async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
            Ok(format!("Result from {} with args: {}", name, args))
        }
    }

    // Mock session store
    struct MockSessionStore;

    #[async_trait]
    impl AgentSessionStore for MockSessionStore {
        async fn save(&self, _session: &Session) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn test_agent_simple_response() {
        let client = Arc::new(MockLlmClient::new(vec![LlmStreamResult {
            content: "Hello! I'm an AI assistant.".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        }]));

        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .system_prompt("You are a helpful assistant.")
            .build(client, tools, store);

        let result = agent.run("Hello".to_string()).await.unwrap();

        assert_eq!(result.text, "Hello! I'm an AI assistant.");
        assert_eq!(result.turns, 1);
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.usage.output_tokens, 20);
    }

    #[tokio::test]
    async fn test_agent_with_tool_call() {
        let client = Arc::new(MockLlmClient::new(vec![
            // First response: tool call
            LlmStreamResult {
                content: "Let me get the weather.".to_string(),
                tool_calls: vec![ToolCall {
                    id: "tc_1".to_string(),
                    name: "get_weather".to_string(),
                    args: serde_json::json!({"city": "Tokyo"}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 15,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            },
            // Second response: final answer
            LlmStreamResult {
                content: "The weather in Tokyo is sunny.".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 25,
                    output_tokens: 20,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            },
        ]));

        let tools = Arc::new(MockToolDispatcher::new(vec![ToolDef {
            name: "get_weather".to_string(),
            description: "Get weather for a city".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "city": {"type": "string"}
                }
            }),
        }]));

        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools, store);

        let result = agent.run("What's the weather in Tokyo?".to_string()).await.unwrap();

        assert_eq!(result.text, "The weather in Tokyo is sunny.");
        assert_eq!(result.turns, 2);
        assert_eq!(result.tool_calls, 1);
        // Total usage should include both turns
        assert_eq!(result.usage.input_tokens, 35);
        assert_eq!(result.usage.output_tokens, 35);
    }

    #[tokio::test]
    async fn test_agent_builder() {
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new()
            .model("claude-3")
            .system_prompt("Test prompt")
            .max_tokens_per_turn(1000)
            .temperature(0.7)
            .budget(BudgetLimits {
                max_tokens: Some(10000),
                max_duration: None,
                max_tool_calls: Some(5),
            })
            .build(client, tools, store);

        assert!(!agent.session().messages().is_empty()); // Should have system prompt
        assert_eq!(agent.state(), &LoopState::CallingLlm);
    }
}
