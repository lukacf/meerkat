//! Agent builder.

use crate::budget::{Budget, BudgetLimits};
use crate::comms_config::CoreCommsConfig;
use crate::comms_runtime::CommsRuntime;
use crate::config::AgentConfig;
use crate::ops::ConcurrencyLimits;
use crate::prompt::SystemPromptConfig;
use crate::retry::RetryPolicy;
use crate::session::Session;
use crate::state::LoopState;
use crate::sub_agent::SubAgentManager;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};

/// Builder for creating an Agent
#[derive(Default)]
pub struct AgentBuilder {
    pub(super) config: AgentConfig,
    pub(super) system_prompt: Option<String>,
    pub(super) budget_limits: Option<BudgetLimits>,
    pub(super) retry_policy: RetryPolicy,
    pub(super) session: Option<Session>,
    pub(super) concurrency_limits: ConcurrencyLimits,
    pub(super) depth: u32,
    /// Comms configuration (optional, enables inter-agent communication)
    pub(super) comms_config: Option<CoreCommsConfig>,
    /// Base directory for resolving comms paths (defaults to current dir)
    pub(super) comms_base_dir: Option<std::path::PathBuf>,
    /// Pre-created comms runtime (alternative to comms_config)
    pub(super) comms_runtime: Option<CommsRuntime>,
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
            comms_runtime: None,
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
    /// ```text
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
    /// Note: For top-level agents (depth == 0), this config is used to create comms.
    /// For sub-agents, use `with_comms_runtime()` to pass a pre-configured runtime.
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

    /// Set a pre-created comms runtime.
    ///
    /// Use this when you need to share the router/trusted_peers with other components
    /// (e.g., CommsToolDispatcher) before building the agent.
    ///
    /// This takes precedence over `comms()` configuration.
    pub fn with_comms_runtime(mut self, runtime: CommsRuntime) -> Self {
        self.comms_runtime = Some(runtime);
        self
    }

    /// Build the agent
    ///
    /// Supports both concrete types and trait objects (`dyn Trait`).
    pub async fn build<C, T, S>(
        self,
        client: Arc<C>,
        tools: Arc<T>,
        store: Arc<S>,
    ) -> Agent<C, T, S>
    where
        C: AgentLlmClient + ?Sized,
        T: AgentToolDispatcher + ?Sized,
        S: AgentSessionStore + ?Sized,
    {
        let session = match self.session {
            Some(s) => s,
            None => {
                let mut s = Session::new();
                let prompt = SystemPromptConfig::new().compose().await;
                s.set_system_prompt(prompt);
                s
            }
        };

        let budget = Budget::new(self.budget_limits.unwrap_or_default());
        let sub_agent_manager = Arc::new(SubAgentManager::new(self.concurrency_limits, self.depth));

        // Create steering channel for receiving steering messages from parent
        let (steering_tx, steering_rx) = mpsc::channel(16);

        // Create comms runtime.
        // Priority: 1) Pre-created runtime (always used, regardless of depth)
        //           2) Create from config (only for top-level agents, depth == 0)
        //
        // Sub-agents can have comms when explicitly provided via with_comms_runtime().
        // This allows parent-child communication over UDS sockets.
        let comms_runtime = if let Some(runtime) = self.comms_runtime {
            // Always use pre-created runtime regardless of depth
            // This enables sub-agent comms when the parent sets it up
            Some(runtime)
        } else if self.depth == 0 {
            // Only auto-create from config for top-level agents
            if let Some(config) = self.comms_config.filter(|c| c.enabled) {
                let base_dir = self.comms_base_dir.unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                });
                let resolved = config.resolve_paths(&base_dir);
                match CommsRuntime::new(resolved).await {
                    Ok(mut runtime) => {
                        tracing::info!(
                            "Comms enabled for agent '{}' (peer ID: {})",
                            config.name,
                            runtime.public_key().to_peer_id()
                        );
                        // Start listeners automatically when comms is enabled
                        if let Err(e) = runtime.start_listeners().await {
                            tracing::warn!("Failed to start comms listeners: {}", e);
                        }
                        Some(runtime)
                    }
                    Err(e) => {
                        tracing::warn!("Failed to create comms runtime: {}", e);
                        None
                    }
                }
            } else {
                None
            }
        } else {
            // Sub-agents without explicit comms runtime get None
            // (They can still communicate via parent-provided runtime)
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
