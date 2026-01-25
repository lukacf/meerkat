//! Agent - the core agent orchestrator
//!
//! The Agent struct ties together all components and runs the agent loop.

use crate::budget::{Budget, BudgetLimits};
use crate::comms_config::CoreCommsConfig;
use crate::comms_runtime::CommsRuntime;
use crate::config::AgentConfig;
use crate::error::AgentError;
use crate::event::{AgentEvent, BudgetType};
use crate::ops::{
    ConcurrencyLimits, ForkBranch, ForkBudgetPolicy, OperationId, OperationResult, SpawnSpec,
    SteeringHandle, ToolAccessPolicy,
};
use crate::retry::RetryPolicy;
use crate::session::Session;
use crate::state::LoopState;
use crate::sub_agent::{SubAgentManager, inject_steering_messages};
use crate::types::{
    AssistantMessage, Message, RunResult, StopReason, ToolCall, ToolDef, ToolResult, Usage,
    UserMessage,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Trait for LLM clients that can be used with the agent
#[async_trait]
pub trait AgentLlmClient: Send + Sync {
    /// Stream a response from the LLM
    ///
    /// # Arguments
    /// * `messages` - Conversation history
    /// * `tools` - Available tool definitions
    /// * `max_tokens` - Maximum tokens to generate
    /// * `temperature` - Sampling temperature
    /// * `provider_params` - Provider-specific parameters (e.g., thinking config)
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
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
    ///
    /// Returns the tool result as a JSON Value on success, or a ToolError on failure.
    /// The Value will be stringified when creating ToolResult.content for the LLM.
    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, crate::error::ToolError>;
}

/// A tool dispatcher that filters tools based on a policy
pub struct FilteredToolDispatcher<T: AgentToolDispatcher + ?Sized> {
    inner: Arc<T>,
    /// HashSet for O(1) lookup instead of Vec O(n)
    allowed_tools: HashSet<String>,
}

impl<T: AgentToolDispatcher + ?Sized> FilteredToolDispatcher<T> {
    pub fn new(inner: Arc<T>, allowed_tools: Vec<String>) -> Self {
        Self {
            inner,
            allowed_tools: allowed_tools.into_iter().collect(),
        }
    }
}

#[async_trait]
impl<T: AgentToolDispatcher + ?Sized + 'static> AgentToolDispatcher for FilteredToolDispatcher<T> {
    fn tools(&self) -> Vec<ToolDef> {
        self.inner
            .tools()
            .into_iter()
            .filter(|t| self.allowed_tools.contains(&t.name))
            .collect()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, crate::error::ToolError> {
        if !self.allowed_tools.contains(name) {
            return Err(crate::error::ToolError::access_denied(name));
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
#[derive(Default)]
pub struct AgentBuilder {
    config: AgentConfig,
    system_prompt: Option<String>,
    budget_limits: Option<BudgetLimits>,
    retry_policy: RetryPolicy,
    session: Option<Session>,
    concurrency_limits: ConcurrencyLimits,
    depth: u32,
    /// Comms configuration (optional, enables inter-agent communication)
    comms_config: Option<CoreCommsConfig>,
    /// Base directory for resolving comms paths (defaults to current dir)
    comms_base_dir: Option<std::path::PathBuf>,
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
            comms_config: None,
            comms_base_dir: None,
        }
    }

    /// Set concurrency limits for sub-agents
    pub fn concurrency_limits(mut self, limits: ConcurrencyLimits) -> Self {
        self.concurrency_limits = limits;
        self
    }

    /// Set the nesting depth for sub-agents
    ///
    /// This controls how deeply nested this agent is in the sub-agent hierarchy.
    /// Depth 0 is the top-level agent. Sub-agents spawned from it have depth 1, etc.
    ///
    /// The depth affects:
    /// - Whether comms listeners are enabled (only depth 0 can have listeners)
    /// - Max depth limit checking for further sub-agent spawning
    pub fn depth(mut self, depth: u32) -> Self {
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

    /// Set provider-specific parameters (e.g., thinking config, reasoning effort)
    ///
    /// These parameters are passed through to the LLM client unchanged.
    /// Each provider implementation is responsible for reading and applying
    /// relevant parameters.
    ///
    /// # Example
    /// ```ignore
    /// let agent = AgentBuilder::new()
    ///     .model("claude-sonnet-4")
    ///     .provider_params(serde_json::json!({
    ///         "thinking": {
    ///             "type": "enabled",
    ///             "budget_tokens": 10000
    ///         }
    ///     }))
    ///     .build(client, tools, store);
    /// ```
    pub fn provider_params(mut self, params: Value) -> Self {
        self.config.provider_params = Some(params);
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

    /// Enable inter-agent communication with the given config.
    ///
    /// When comms is enabled, the agent will:
    /// - Start listeners for incoming messages (UDS and/or TCP)
    /// - Provide comms tools (send_message, send_request, send_response, list_peers)
    /// - Drain inbox at turn boundaries and inject messages into the session
    ///
    /// Note: Subagents cannot have comms enabled (security restriction).
    pub fn comms(mut self, config: CoreCommsConfig) -> Self {
        self.comms_config = Some(config);
        self
    }

    /// Set the base directory for resolving comms paths.
    ///
    /// Relative paths in comms config will be resolved relative to this directory.
    /// Defaults to current working directory if not set.
    pub fn comms_base_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.comms_base_dir = Some(dir);
        self
    }

    /// Build the agent
    ///
    /// Supports both concrete types and trait objects (`dyn Trait`).
    pub fn build<C, T, S>(self, client: Arc<C>, tools: Arc<T>, store: Arc<S>) -> Agent<C, T, S>
    where
        C: AgentLlmClient + ?Sized,
        T: AgentToolDispatcher + ?Sized,
        S: AgentSessionStore + ?Sized,
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

        // Create comms runtime if enabled AND this is not a subagent.
        // Subagents cannot have comms for security reasons (no network exposure).
        let comms_runtime = if self.depth == 0 {
            self.comms_config.filter(|c| c.enabled).and_then(|config| {
                let base_dir = self.comms_base_dir.unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                });
                let resolved = config.resolve_paths(&base_dir);
                match CommsRuntime::new(resolved) {
                    Ok(mut runtime) => {
                        tracing::info!(
                            "Comms enabled for agent '{}' (peer ID: {})",
                            config.name,
                            runtime.public_key().to_peer_id()
                        );
                        // Start listeners automatically when comms is enabled
                        // This is done in a blocking context since build() is sync
                        // Listeners run in background tasks and don't block
                        if let Err(e) = futures::executor::block_on(runtime.start_listeners()) {
                            tracing::warn!("Failed to start comms listeners: {}", e);
                        }
                        Some(runtime)
                    }
                    Err(e) => {
                        tracing::warn!("Failed to create comms runtime: {}", e);
                        None
                    }
                }
            })
        } else {
            // Subagents cannot have comms - this is a security restriction to prevent
            // subagents from having network exposure.
            None
        };

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
            comms_runtime,
        }
    }
}

/// The main Agent struct
///
/// Supports both concrete types and trait objects (`dyn Trait`).
/// When using trait objects, pass `Arc<dyn AgentLlmClient>` etc.
pub struct Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized,
    T: AgentToolDispatcher + ?Sized,
    S: AgentSessionStore + ?Sized,
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
    /// Optional comms runtime for inter-agent communication.
    /// None if comms is disabled or if this is a subagent (subagents cannot have comms).
    comms_runtime: Option<CommsRuntime>,
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

    /// Get the comms runtime, if enabled.
    ///
    /// Returns `None` if comms is disabled or if this is a subagent.
    pub fn comms(&self) -> Option<&CommsRuntime> {
        self.comms_runtime.as_ref()
    }

    /// Get mutable access to the comms runtime, if enabled.
    ///
    /// Returns `None` if comms is disabled or if this is a subagent.
    pub fn comms_mut(&mut self) -> Option<&mut CommsRuntime> {
        self.comms_runtime.as_mut()
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
                .build(client, filtered_tools, store);

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

            // Create steering channel
            let (steering_tx, _steering_rx) = mpsc::channel(16);

            // Register the branch as a sub-agent
            self.sub_agent_manager
                .register(op_id.clone(), branch.name.clone(), steering_tx)
                .await?;

            // Apply tool access policy for this branch
            let all_tools = self.tools.tools();
            let allowed_tools = match &branch.tool_access {
                Some(policy) => self
                    .sub_agent_manager
                    .apply_tool_access_policy(&all_tools, policy),
                None => all_tools, // Inherit all tools
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
                    .build(client, filtered_tools, store);

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

    /// Send a steering message to a running sub-agent
    pub async fn steer(
        &self,
        op_id: &OperationId,
        message: String,
    ) -> Result<SteeringHandle, AgentError> {
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

    /// Drain comms inbox and inject messages into session.
    ///
    /// This is called at turn boundaries to process incoming inter-agent messages.
    /// It is non-blocking - if the inbox is empty, it returns immediately.
    fn drain_comms_inbox(&mut self) {
        if let Some(ref mut comms) = self.comms_runtime {
            let messages = comms.drain_messages();
            if !messages.is_empty() {
                tracing::debug!("Injecting {} comms messages into session", messages.len());

                // Format all messages into a single user message for the LLM
                // Pre-calculate capacity to avoid reallocations
                let mut combined = String::new();
                let mut first = true;
                for msg in &messages {
                    if !first {
                        combined.push_str("\n\n");
                    }
                    combined.push_str(&msg.to_user_message_text());
                    first = false;
                }

                self.session
                    .push(Message::User(UserMessage { content: combined }));
            }
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

            match self
                .client
                .stream_response(
                    messages,
                    tools,
                    max_tokens,
                    self.config.temperature,
                    self.config.provider_params.as_ref(),
                )
                .await
            {
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

        // Add user message
        self.session.push(Message::User(crate::types::UserMessage {
            content: user_input,
        }));

        self.run_loop(Some(event_tx)).await
    }

    /// The main agent loop
    async fn run_loop(
        &mut self,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<RunResult, AgentError> {
        let mut turn_count = 0u32;
        let max_turns = self.config.max_turns.unwrap_or(100);
        let mut tool_call_count = 0u32;

        // Helper to conditionally emit events (only when listener exists)
        macro_rules! emit_event {
            ($event:expr) => {
                if let Some(ref tx) = event_tx {
                    let _ = tx.send($event).await;
                }
            };
        }

        loop {
            // Check turn limit
            if turn_count >= max_turns {
                self.state = LoopState::Completed;
                return Ok(self.build_result(turn_count, tool_call_count));
            }

            // Check budget
            if self.budget.is_exhausted() {
                emit_event!(AgentEvent::BudgetWarning {
                    budget_type: BudgetType::Tokens,
                    used: self.session.total_tokens(),
                    limit: self.budget.remaining(),
                    percent: 1.0,
                });
                self.state = LoopState::Completed;
                return Ok(self.build_result(turn_count, tool_call_count));
            }

            match self.state {
                LoopState::CallingLlm => {
                    // Emit turn start
                    emit_event!(AgentEvent::TurnStarted {
                        turn_number: turn_count,
                    });

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
                            emit_event!(AgentEvent::ToolCallRequested {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                args: tc.args.clone(),
                            });
                        }

                        // Transition to waiting for ops
                        self.state.transition(LoopState::WaitingForOps)?;

                        // Execute tool calls in parallel
                        let num_tool_calls = result.tool_calls.len();
                        let tools_ref = &self.tools;

                        // Emit all execution start events
                        for tc in &result.tool_calls {
                            emit_event!(AgentEvent::ToolExecutionStarted {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                            });
                        }

                        // Execute all tool calls in parallel using join_all
                        let dispatch_futures: Vec<_> = result
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                let id = tc.id.clone();
                                let name = tc.name.clone();
                                let args = tc.args.clone();
                                let thought_signature = tc.thought_signature.clone();
                                async move {
                                    let start = std::time::Instant::now();
                                    let dispatch_result = tools_ref.dispatch(&name, &args).await;
                                    let duration_ms = start.elapsed().as_millis() as u64;
                                    (id, name, dispatch_result, duration_ms, thought_signature)
                                }
                            })
                            .collect();

                        let dispatch_results = futures::future::join_all(dispatch_futures).await;

                        // Process results and emit events
                        let mut tool_results = Vec::with_capacity(num_tool_calls);
                        for (id, name, dispatch_result, duration_ms, thought_signature) in
                            dispatch_results
                        {
                            let (content, is_error) = match dispatch_result {
                                Ok(v) => {
                                    // Stringify the Value for the LLM
                                    let s = match &v {
                                        Value::String(s) => s.clone(),
                                        _ => serde_json::to_string(&v).unwrap_or_default(),
                                    };
                                    (s, false)
                                }
                                Err(e) => (e.to_string(), true),
                            };

                            // Emit execution complete
                            emit_event!(AgentEvent::ToolExecutionCompleted {
                                id: id.clone(),
                                name: name.clone(),
                                result: content.clone(),
                                is_error,
                                duration_ms,
                            });

                            // Emit result received
                            emit_event!(AgentEvent::ToolResultReceived {
                                id: id.clone(),
                                name: name.clone(),
                                is_error,
                            });

                            tool_results.push(ToolResult {
                                tool_use_id: id,
                                content,
                                is_error,
                                thought_signature,
                            });

                            // Track tool call in budget
                            self.budget.record_tool_call();
                            tool_call_count += 1;
                        }

                        // Add tool results to session
                        self.session.push(Message::ToolResults {
                            results: tool_results,
                        });

                        // Go through DrainingEvents to CallingLlm (state machine requires this)
                        self.state.transition(LoopState::DrainingEvents)?;

                        // === TURN BOUNDARY: Apply steering, drain comms, collect sub-agent results ===

                        // Apply any pending steering messages from parent
                        self.apply_pending_steering().await;

                        // Drain comms inbox and inject messages into session (non-blocking)
                        self.drain_comms_inbox();

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
                                    thought_signature: None, // Sub-agents don't use thought signatures
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
                        emit_event!(AgentEvent::TurnCompleted {
                            stop_reason: result.stop_reason,
                            usage: result.usage,
                        });

                        // Transition to completed
                        self.state.transition(LoopState::DrainingEvents)?;
                        self.state.transition(LoopState::Completed)?;

                        // Save session
                        if let Err(e) = self.store.save(&self.session).await {
                            tracing::warn!("Failed to save session: {}", e);
                        }

                        // Emit run completed
                        emit_event!(AgentEvent::RunCompleted {
                            session_id: self.session.id().clone(),
                            result: final_text.clone(),
                            usage: self.session.total_usage(),
                        });

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
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
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

        async fn dispatch(
            &self,
            name: &str,
            args: &Value,
        ) -> Result<Value, crate::error::ToolError> {
            Ok(Value::String(format!(
                "Result from {} with args: {}",
                name, args
            )))
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
                tool_calls: vec![ToolCall::new(
                    "tc_1".to_string(),
                    "get_weather".to_string(),
                    serde_json::json!({"city": "Tokyo"}),
                )],
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

        let result = agent
            .run("What's the weather in Tokyo?".to_string())
            .await
            .unwrap();

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

    // =========================================================================
    // Phase 10: Agent Integration Tests (comms wiring)
    // =========================================================================

    #[test]
    fn test_agent_builder_has_comms_config() {
        // Verify AgentBuilder has comms_config field
        let builder = AgentBuilder::new();
        // The field exists (compiles) and is None by default
        // We test this by using the comms() method
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = builder.build(client, tools, store);
        // Without comms config, comms runtime should be None
        assert!(agent.comms().is_none());
    }

    #[test]
    fn test_agent_builder_comms_method() {
        use crate::comms_config::CoreCommsConfig;

        let config = CoreCommsConfig::with_name("test-agent");
        let builder = AgentBuilder::new().comms(config.clone());

        // Verify builder accepted the config (implicitly - build will use it)
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Note: This test doesn't verify comms_runtime creation because that
        // requires filesystem access. See test_agent_builder_creates_comms_runtime.
        let _agent = builder.build(client, tools, store);
    }

    #[test]
    fn test_agent_has_comms_runtime() {
        // Verify Agent has comms_runtime field (accessible via comms())
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new().build(client, tools, store);

        // comms() and comms_mut() methods should exist and return None when disabled
        assert!(agent.comms().is_none());
    }

    #[test]
    fn test_agent_builder_creates_comms_runtime() {
        use crate::comms_config::CoreCommsConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        // No listeners to avoid port binding issues in tests
        config.listen_uds = None;
        config.listen_tcp = None;

        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .build(client, tools, store);

        // Comms runtime should be created
        assert!(agent.comms().is_some());

        // Public key should be valid
        let pubkey = agent.comms().unwrap().public_key();
        assert_eq!(pubkey.as_bytes().len(), 32);
    }

    #[test]
    fn test_agent_comms_accessor() {
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new().build(client, tools, store);

        // comms() should return Option<&CommsRuntime>
        let comms_ref: Option<&crate::comms_runtime::CommsRuntime> = agent.comms();
        assert!(comms_ref.is_none());
    }

    #[test]
    fn test_agent_comms_mut_accessor() {
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new().build(client, tools, store);

        // comms_mut() should return Option<&mut CommsRuntime>
        let comms_mut: Option<&mut crate::comms_runtime::CommsRuntime> = agent.comms_mut();
        assert!(comms_mut.is_none());
    }

    #[test]
    fn test_subagent_has_no_comms() {
        use crate::comms_config::CoreCommsConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");

        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Build agent with depth > 0 (simulating subagent)
        let agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .depth(1) // Subagent depth
            .build(client, tools, store);

        // Subagents cannot have comms - security restriction
        assert!(agent.comms().is_none());
        assert_eq!(agent.depth(), 1);
    }

    #[test]
    fn test_agent_no_comms_tools_when_disabled() {
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![ToolDef {
            name: "my_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new().build(client, tools, store);

        // Without comms, agent should have no comms runtime
        assert!(agent.comms().is_none());
    }

    #[tokio::test]
    async fn test_agent_empty_inbox_nonblocking() {
        use crate::comms_config::CoreCommsConfig;
        use std::time::{Duration, Instant};
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        config.listen_uds = None;
        config.listen_tcp = None;

        let client = Arc::new(MockLlmClient::new(vec![LlmStreamResult {
            content: "Done".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .build(client, tools, store);

        // Verify comms is enabled
        assert!(agent.comms().is_some());

        // Run agent - should complete quickly even with empty inbox
        let start = Instant::now();
        let result = agent.run("Test".to_string()).await.unwrap();
        let elapsed = start.elapsed();

        // Should complete in reasonable time (not blocked on inbox)
        assert!(
            elapsed < Duration::from_secs(5),
            "Agent run took too long: {:?}",
            elapsed
        );
        assert_eq!(result.text, "Done");
    }

    #[tokio::test]
    async fn test_agent_starts_comms_listeners() {
        // Test that listeners are started automatically when comms is enabled.
        // We verify this by checking that the runtime was created (listeners start
        // during CommsRuntime::start_listeners() which is called in build()).
        use crate::comms_config::CoreCommsConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        // Enable UDS listener
        config.listen_uds = Some(tmp.path().join("test.sock"));
        config.listen_tcp = None;

        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new()
            .comms(config.clone())
            .comms_base_dir(tmp.path().to_path_buf())
            .build(client, tools, store);

        // Comms runtime should exist
        assert!(agent.comms().is_some());

        // Give the listener a moment to create the socket
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Socket file should have been created by the listener
        // (This verifies start_listeners was called)
        assert!(
            config.listen_uds.as_ref().unwrap().exists(),
            "UDS socket file should exist after listener starts"
        );
    }

    #[tokio::test]
    async fn test_agent_drains_inbox_at_turn_boundary() {
        // This test verifies that drain_comms_inbox is called at turn boundaries.
        // Since we can't easily inject messages into the inbox, we verify:
        // 1. The method exists and is called (via code inspection)
        // 2. Empty inbox doesn't affect agent behavior
        // Full inbox draining is tested in e2e tests with real connections.
        use crate::comms_config::CoreCommsConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        config.listen_uds = None;
        config.listen_tcp = None;

        let client = Arc::new(MockLlmClient::new(vec![
            // First: tool call response
            LlmStreamResult {
                content: "Calling tool".to_string(),
                tool_calls: vec![ToolCall::new(
                    "tc_1".to_string(),
                    "test_tool".to_string(),
                    serde_json::json!({}),
                )],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            // Second: final response (after turn boundary where inbox would be drained)
            LlmStreamResult {
                content: "Done after turn boundary".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));
        let tools = Arc::new(MockToolDispatcher::new(vec![ToolDef {
            name: "test_tool".to_string(),
            description: "Test".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .build(client, tools, store);

        assert!(agent.comms().is_some());

        // Run agent - it should complete even with comms enabled and empty inbox
        let result = agent.run("Test".to_string()).await.unwrap();

        // Should reach final response (turn boundary was crossed successfully)
        assert_eq!(result.text, "Done after turn boundary");
        assert_eq!(result.turns, 2);
    }

    #[test]
    fn test_agent_formats_inbox_for_llm() {
        // Test that CommsMessage::to_user_message_text produces proper format.
        // This is already tested in comms_runtime::tests::test_comms_message_formatting
        // but we add a quick sanity check here for the integration context.
        use crate::comms_runtime::{CommsContent, CommsMessage};
        use meerkat_comms::PubKey;

        let msg = CommsMessage {
            id: uuid::Uuid::new_v4(),
            from_peer: "alice".to_string(),
            from_pubkey: PubKey::new([1u8; 32]),
            content: CommsContent::Message {
                body: "Hello from Alice".to_string(),
            },
        };

        let text = msg.to_user_message_text();

        // Should be formatted for LLM injection
        assert!(text.contains("[Comms]"), "Should have [Comms] prefix");
        assert!(text.contains("alice"), "Should include peer name");
        assert!(
            text.contains("Hello from Alice"),
            "Should include message body"
        );
    }

    // =========================================================================
    // Provider params tests (TDD)
    // =========================================================================

    #[test]
    fn test_agent_config_accepts_provider_params() {
        use crate::config::AgentConfig;

        // Test that AgentConfig has provider_params field
        let mut config = AgentConfig::default();
        assert!(config.provider_params.is_none());

        // Set provider params
        config.provider_params = Some(serde_json::json!({
            "thinking": {
                "type": "enabled",
                "budget_tokens": 10000
            }
        }));

        assert!(config.provider_params.is_some());
        let params = config.provider_params.unwrap();
        assert_eq!(params["thinking"]["budget_tokens"], 10000);
    }

    #[test]
    fn test_agent_builder_provider_params() {
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Test builder has provider_params method
        let _agent = AgentBuilder::new()
            .model("test-model")
            .provider_params(serde_json::json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 5000
                }
            }))
            .build(client, tools, store);

        // Agent builds successfully with provider_params
    }

    /// Mock client that captures provider_params for verification
    struct ProviderParamsCapturingClient {
        captured_params: Mutex<Option<Value>>,
        responses: Mutex<Vec<LlmStreamResult>>,
    }

    impl ProviderParamsCapturingClient {
        fn new(responses: Vec<LlmStreamResult>) -> Self {
            Self {
                captured_params: Mutex::new(None),
                responses: Mutex::new(responses),
            }
        }

        fn captured_params(&self) -> Option<Value> {
            self.captured_params.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentLlmClient for ProviderParamsCapturingClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[ToolDef],
            _max_tokens: u32,
            _temperature: Option<f32>,
            provider_params: Option<&Value>,
        ) -> Result<LlmStreamResult, AgentError> {
            // Capture the provider_params
            *self.captured_params.lock().unwrap() = provider_params.cloned();

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
            "mock-capturing"
        }
    }

    #[tokio::test]
    async fn test_provider_params_flows_to_stream_response() {
        let client = Arc::new(ProviderParamsCapturingClient::new(vec![LlmStreamResult {
            content: "Response with thinking".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let thinking_config = serde_json::json!({
            "thinking": {
                "type": "enabled",
                "budget_tokens": 10000
            }
        });

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .provider_params(thinking_config.clone())
            .build(client.clone(), tools, store);

        let _result = agent.run("Test".to_string()).await.unwrap();

        // Verify provider_params was passed to stream_response
        let captured = client.captured_params();
        assert!(captured.is_some(), "provider_params should be captured");
        assert_eq!(captured.unwrap(), thinking_config);
    }

    #[tokio::test]
    async fn test_provider_params_none_when_not_set() {
        let client = Arc::new(ProviderParamsCapturingClient::new(vec![LlmStreamResult {
            content: "Response".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Build without provider_params
        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client.clone(), tools, store);

        let _result = agent.run("Test".to_string()).await.unwrap();

        // Verify provider_params was None
        let captured = client.captured_params();
        assert!(
            captured.is_none(),
            "provider_params should be None when not set"
        );
    }

    // =========================================================================
    // Performance fix tests (TDD)
    // =========================================================================

    /// Test that FilteredToolDispatcher uses HashSet for O(1) lookups
    #[test]
    fn test_filtered_tool_dispatcher_uses_hashset() {
        // FilteredToolDispatcher now uses HashSet internally for O(1) lookups.
        // This test verifies the behavioral correctness of the filtering.

        // Create a dispatcher with many tools
        let mut tools = Vec::new();
        for i in 0..100 {
            tools.push(ToolDef {
                name: format!("tool_{}", i),
                description: format!("Tool {}", i),
                input_schema: serde_json::json!({"type": "object"}),
            });
        }
        let inner = Arc::new(MockToolDispatcher::new(tools));

        // Allow only 50 tools
        let allowed: Vec<String> = (0..50).map(|i| format!("tool_{}", i)).collect();
        let filtered = FilteredToolDispatcher::new(inner, allowed);

        // Verify it filters correctly
        let filtered_tools = filtered.tools();
        assert_eq!(filtered_tools.len(), 50);

        // Check first and last allowed tool are present
        assert!(filtered_tools.iter().any(|t| t.name == "tool_0"));
        assert!(filtered_tools.iter().any(|t| t.name == "tool_49"));

        // Check disallowed tool is not present
        assert!(!filtered_tools.iter().any(|t| t.name == "tool_50"));
    }

    #[tokio::test]
    async fn test_filtered_tool_dispatcher_blocks_disallowed() {
        let tools = vec![
            ToolDef {
                name: "allowed_tool".to_string(),
                description: "Allowed".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            ToolDef {
                name: "blocked_tool".to_string(),
                description: "Blocked".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        ];
        let inner = Arc::new(MockToolDispatcher::new(tools));
        let allowed = vec!["allowed_tool".to_string()];
        let filtered = FilteredToolDispatcher::new(inner, allowed);

        // Allowed tool should work
        let result = filtered
            .dispatch("allowed_tool", &serde_json::json!({}))
            .await;
        assert!(result.is_ok());

        // Blocked tool should fail
        let result = filtered
            .dispatch("blocked_tool", &serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not allowed"));
    }

    /// Test that tool_results Vec is pre-allocated
    #[tokio::test]
    async fn test_tool_results_vec_preallocated() {
        // This is a behavioral test - we verify multiple tool calls work correctly.
        // The pre-allocation is an implementation detail verified via code review.
        let client = Arc::new(MockLlmClient::new(vec![
            LlmStreamResult {
                content: "Calling tools".to_string(),
                tool_calls: vec![
                    ToolCall::new(
                        "tc_1".to_string(),
                        "tool_a".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_2".to_string(),
                        "tool_b".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_3".to_string(),
                        "tool_c".to_string(),
                        serde_json::json!({}),
                    ),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            LlmStreamResult {
                content: "Done".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));

        let tools = Arc::new(MockToolDispatcher::new(vec![
            ToolDef {
                name: "tool_a".to_string(),
                description: "A".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            ToolDef {
                name: "tool_b".to_string(),
                description: "B".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            ToolDef {
                name: "tool_c".to_string(),
                description: "C".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        ]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools, store);

        let result = agent.run("Test".to_string()).await.unwrap();

        assert_eq!(result.tool_calls, 3);
        assert_eq!(result.text, "Done");
    }

    /// Mock tool dispatcher that tracks execution order and timing for parallel execution tests
    struct TimingToolDispatcher {
        tools: Vec<ToolDef>,
        /// Records (tool_name, start_time, end_time) for each dispatch
        timings: std::sync::Arc<Mutex<Vec<(String, std::time::Instant, std::time::Instant)>>>,
        /// How long each tool should "execute" (simulated delay)
        delay_ms: u64,
    }

    impl TimingToolDispatcher {
        fn new(tools: Vec<ToolDef>, delay_ms: u64) -> Self {
            Self {
                tools,
                timings: std::sync::Arc::new(Mutex::new(Vec::new())),
                delay_ms,
            }
        }

        fn timings(&self) -> Vec<(String, std::time::Instant, std::time::Instant)> {
            self.timings.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for TimingToolDispatcher {
        fn tools(&self) -> Vec<ToolDef> {
            self.tools.clone()
        }

        async fn dispatch(
            &self,
            name: &str,
            _args: &Value,
        ) -> Result<Value, crate::error::ToolError> {
            let start = std::time::Instant::now();
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            let end = std::time::Instant::now();

            self.timings
                .lock()
                .unwrap()
                .push((name.to_string(), start, end));

            Ok(Value::String(format!("Result from {}", name)))
        }
    }

    /// Test that tool calls execute in parallel, not serially
    #[tokio::test]
    async fn test_tool_calls_execute_in_parallel() {
        let tool_delay_ms = 50;
        let num_tools = 3;

        let client = Arc::new(MockLlmClient::new(vec![
            LlmStreamResult {
                content: "Calling tools".to_string(),
                tool_calls: vec![
                    ToolCall::new(
                        "tc_1".to_string(),
                        "slow_tool_1".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_2".to_string(),
                        "slow_tool_2".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_3".to_string(),
                        "slow_tool_3".to_string(),
                        serde_json::json!({}),
                    ),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            LlmStreamResult {
                content: "Done".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));

        let tools = Arc::new(TimingToolDispatcher::new(
            vec![
                ToolDef {
                    name: "slow_tool_1".to_string(),
                    description: "Slow 1".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                ToolDef {
                    name: "slow_tool_2".to_string(),
                    description: "Slow 2".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                ToolDef {
                    name: "slow_tool_3".to_string(),
                    description: "Slow 3".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ],
            tool_delay_ms,
        ));
        let store = Arc::new(MockSessionStore);

        let overall_start = std::time::Instant::now();
        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools.clone(), store);

        let result = agent.run("Test".to_string()).await.unwrap();
        let overall_duration = overall_start.elapsed();

        assert_eq!(result.tool_calls, 3);

        // If tools ran in parallel, total time should be ~tool_delay_ms (plus overhead)
        // If they ran serially, total time would be ~num_tools * tool_delay_ms
        let serial_time_ms = (num_tools * tool_delay_ms) as u128;

        assert!(
            overall_duration.as_millis() < serial_time_ms,
            "Tool execution took {}ms, which suggests serial execution (expected < {}ms for parallel)",
            overall_duration.as_millis(),
            serial_time_ms
        );

        // Verify execution times overlap (parallel execution)
        let timings = tools.timings();
        assert_eq!(timings.len(), 3, "Should have 3 timing records");

        // Check that tools started within a small window of each other (parallel start)
        let starts: Vec<_> = timings.iter().map(|(_, s, _)| *s).collect();
        let first_start = *starts.iter().min().unwrap();
        let last_start = *starts.iter().max().unwrap();
        let start_spread = last_start.duration_since(first_start);

        // If parallel, all tools should start within ~10ms of each other
        assert!(
            start_spread.as_millis() < 20,
            "Tools started {}ms apart, suggesting serial execution",
            start_spread.as_millis()
        );
    }

    /// Tool dispatcher that can simulate failures for specific tools
    struct FailingToolDispatcher {
        tools: Vec<ToolDef>,
        /// Tools that should fail (by name)
        failing_tools: std::collections::HashSet<String>,
        /// Delay in ms for each tool
        delay_ms: u64,
        /// Records dispatch order
        dispatch_order: std::sync::Arc<Mutex<Vec<String>>>,
    }

    impl FailingToolDispatcher {
        fn new(tools: Vec<ToolDef>, failing_tools: Vec<&str>, delay_ms: u64) -> Self {
            Self {
                tools,
                failing_tools: failing_tools.into_iter().map(|s| s.to_string()).collect(),
                delay_ms,
                dispatch_order: std::sync::Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn dispatch_order(&self) -> Vec<String> {
            self.dispatch_order.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for FailingToolDispatcher {
        fn tools(&self) -> Vec<ToolDef> {
            self.tools.clone()
        }

        async fn dispatch(
            &self,
            name: &str,
            _args: &Value,
        ) -> Result<Value, crate::error::ToolError> {
            // Record that dispatch started
            self.dispatch_order.lock().unwrap().push(name.to_string());

            // Simulate work
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;

            if self.failing_tools.contains(name) {
                Err(crate::error::ToolError::execution_failed(format!(
                    "Tool {} failed intentionally",
                    name
                )))
            } else {
                Ok(Value::String(format!("Success from {}", name)))
            }
        }
    }

    /// Test that parallel tool results preserve order (results match call order)
    #[tokio::test]
    async fn test_parallel_tool_results_preserve_order() {
        let client = Arc::new(MockLlmClient::new(vec![
            LlmStreamResult {
                content: "Calling tools".to_string(),
                tool_calls: vec![
                    ToolCall::new(
                        "tc_1".to_string(),
                        "tool_a".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_2".to_string(),
                        "tool_b".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_3".to_string(),
                        "tool_c".to_string(),
                        serde_json::json!({}),
                    ),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            LlmStreamResult {
                content: "Done".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));

        let tools = Arc::new(FailingToolDispatcher::new(
            vec![
                ToolDef {
                    name: "tool_a".to_string(),
                    description: "Tool A".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                ToolDef {
                    name: "tool_b".to_string(),
                    description: "Tool B".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                ToolDef {
                    name: "tool_c".to_string(),
                    description: "Tool C".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ],
            vec![], // No failures
            10,     // 10ms delay
        ));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools.clone(), store);

        let _result = agent.run("Test".to_string()).await.unwrap();

        // Check session has tool results in correct order
        let messages = agent.session().messages();
        let tool_results_msg = messages
            .iter()
            .find(|m| matches!(m, Message::ToolResults { .. }));
        assert!(
            tool_results_msg.is_some(),
            "Should have tool results message"
        );

        if let Message::ToolResults { results } = tool_results_msg.unwrap() {
            assert_eq!(results.len(), 3);
            assert_eq!(
                results[0].tool_use_id, "tc_1",
                "First result should be tc_1"
            );
            assert_eq!(
                results[1].tool_use_id, "tc_2",
                "Second result should be tc_2"
            );
            assert_eq!(
                results[2].tool_use_id, "tc_3",
                "Third result should be tc_3"
            );
        }
    }

    /// Test that partial failures don't block other tools
    #[tokio::test]
    async fn test_parallel_tool_partial_failure() {
        let client = Arc::new(MockLlmClient::new(vec![
            LlmStreamResult {
                content: "Calling tools".to_string(),
                tool_calls: vec![
                    ToolCall::new(
                        "tc_1".to_string(),
                        "good_tool".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_2".to_string(),
                        "bad_tool".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_3".to_string(),
                        "another_good_tool".to_string(),
                        serde_json::json!({}),
                    ),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            LlmStreamResult {
                content: "Done".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));

        let tools = Arc::new(FailingToolDispatcher::new(
            vec![
                ToolDef {
                    name: "good_tool".to_string(),
                    description: "Good Tool".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                ToolDef {
                    name: "bad_tool".to_string(),
                    description: "Bad Tool".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                ToolDef {
                    name: "another_good_tool".to_string(),
                    description: "Another Good Tool".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ],
            vec!["bad_tool"], // Only bad_tool fails
            10,
        ));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools.clone(), store);

        // Should complete successfully even with partial failure
        let result = agent.run("Test".to_string()).await;
        assert!(
            result.is_ok(),
            "Agent should complete even with partial tool failure"
        );

        // Check that all tools were called
        let dispatch_order = tools.dispatch_order();
        assert_eq!(
            dispatch_order.len(),
            3,
            "All 3 tools should have been dispatched"
        );

        // Check session has correct results with error flags
        let messages = agent.session().messages();
        let tool_results_msg = messages
            .iter()
            .find(|m| matches!(m, Message::ToolResults { .. }));

        if let Some(Message::ToolResults { results }) = tool_results_msg {
            assert_eq!(results.len(), 3);

            // First tool succeeded
            assert!(!results[0].is_error, "good_tool should succeed");
            assert!(results[0].content.contains("Success"));

            // Second tool failed
            assert!(results[1].is_error, "bad_tool should fail");
            assert!(results[1].content.contains("failed"));

            // Third tool succeeded
            assert!(!results[2].is_error, "another_good_tool should succeed");
            assert!(results[2].content.contains("Success"));
        }
    }

    /// Test that all tools failing still completes the turn
    #[tokio::test]
    async fn test_parallel_tool_all_failures() {
        let client = Arc::new(MockLlmClient::new(vec![
            LlmStreamResult {
                content: "Calling tools".to_string(),
                tool_calls: vec![
                    ToolCall::new(
                        "tc_1".to_string(),
                        "fail1".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_2".to_string(),
                        "fail2".to_string(),
                        serde_json::json!({}),
                    ),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            LlmStreamResult {
                content: "I see both tools failed".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));

        let tools = Arc::new(FailingToolDispatcher::new(
            vec![
                ToolDef {
                    name: "fail1".to_string(),
                    description: "Fail 1".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                ToolDef {
                    name: "fail2".to_string(),
                    description: "Fail 2".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ],
            vec!["fail1", "fail2"], // Both fail
            10,
        ));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools.clone(), store);

        // Should complete - failures go back to LLM as error results
        let result = agent.run("Test".to_string()).await;
        assert!(
            result.is_ok(),
            "Agent should complete even when all tools fail"
        );

        // Check both errors were captured
        let messages = agent.session().messages();
        let tool_results_msg = messages
            .iter()
            .find(|m| matches!(m, Message::ToolResults { .. }));

        if let Some(Message::ToolResults { results }) = tool_results_msg {
            assert_eq!(results.len(), 2);
            assert!(results[0].is_error);
            assert!(results[1].is_error);
        }
    }
}
