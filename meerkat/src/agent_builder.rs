//! Public facade AgentBuilder.
//!
//! The facade builder routes through [`AgentFactory::build_agent`] so public
//! construction observes the same provider, session metadata, runtime-handle,
//! hook, and tool/store override semantics as the factory path. The lower-level
//! core builder remains available as [`crate::CoreAgentBuilder`] for tests and
//! embeddings that intentionally own all agent loop primitives themselves.

use std::{future::Future, pin::Pin, sync::Arc};

use meerkat_client::LlmClient;
use meerkat_core::{
    AgentLlmClient, AgentSessionStore, AgentToolDispatcher, BudgetLimits, Config, HookEngine,
    HookRunOverrides, OutputSchema, Provider, RetryPolicy, RuntimeBuildMode, Session,
};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::mpsc;

use crate::{AgentBuildConfig, AgentFactory, BuildAgentError, DynAgent};

#[cfg(not(target_arch = "wasm32"))]
pub type AgentBuilderBuildFuture =
    Pin<Box<dyn Future<Output = Result<DynAgent, BuildAgentError>> + Send>>;
#[cfg(target_arch = "wasm32")]
pub type AgentBuilderBuildFuture = Pin<Box<dyn Future<Output = Result<DynAgent, BuildAgentError>>>>;

/// Public builder for creating agents through the canonical facade pipeline.
pub struct AgentBuilder {
    factory: AgentFactory,
    config: Config,
    build_config: AgentBuildConfig,
    model_explicit: bool,
    unsupported_core_injections: Vec<&'static str>,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBuilder {
    /// Create a builder with default config and the default user state root.
    pub fn new() -> Self {
        let config = Config::default();
        let model = config.agent.model.clone();
        let factory = AgentFactory::new(meerkat_core::default_state_root().join("sessions"));
        Self {
            factory,
            config,
            build_config: AgentBuildConfig::new(model),
            model_explicit: false,
            unsupported_core_injections: Vec::new(),
        }
    }

    /// Use a specific factory for store roots, provider registry state, and
    /// surface resource defaults.
    pub fn with_factory(mut self, factory: AgentFactory) -> Self {
        self.factory = factory;
        self
    }

    /// Use a specific config snapshot for the factory build.
    pub fn with_config(mut self, config: Config) -> Self {
        if !self.model_explicit {
            self.build_config.model = config.agent.model.clone();
        }
        self.config = config;
        self
    }

    /// Set the model to use.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.build_config.model = model.into();
        self.model_explicit = true;
        self
    }

    /// Set an explicit provider instead of resolving from the model registry.
    pub fn provider(mut self, provider: Provider) -> Self {
        self.build_config.provider = Some(provider);
        self
    }

    /// Set max tokens per turn.
    pub fn max_tokens_per_turn(mut self, tokens: u32) -> Self {
        self.build_config.max_tokens = Some(tokens);
        self
    }

    /// Set the system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.build_config.system_prompt = Some(prompt.into());
        self
    }

    /// Set temperature on the config snapshot used by the factory build.
    pub fn temperature(mut self, temp: f32) -> Self {
        self.config.agent.temperature = Some(temp);
        self
    }

    /// Set budget limits.
    pub fn budget(mut self, limits: BudgetLimits) -> Self {
        self.build_config.budget_limits = Some(limits);
        self
    }

    /// Set provider-specific parameters.
    pub fn provider_params(mut self, params: serde_json::Value) -> Self {
        self.build_config.provider_params = Some(params);
        self
    }

    /// Provider-native tool defaults are owned by the factory pipeline.
    ///
    /// Public facade builds intentionally derive these defaults from
    /// `Config.provider_tools` and the model profile. Use `provider_params` for
    /// explicit per-build provider overrides. Calling this standalone-only
    /// injection method makes `try_build` return a configuration error instead
    /// of silently dropping the supplied defaults.
    pub fn provider_tool_defaults(mut self, _defaults: serde_json::Value) -> Self {
        self.unsupported_core_injections
            .push("provider_tool_defaults");
        self
    }

    /// Set retry policy on the config snapshot used by the factory build.
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.config.retry.max_retries = policy.max_retries;
        self.config.retry.initial_delay = policy.initial_delay;
        self.config.retry.max_delay = policy.max_delay;
        self.config.retry.multiplier = policy.multiplier;
        self
    }

    /// Set output schema for structured output extraction.
    pub fn output_schema(mut self, schema: OutputSchema) -> Self {
        self.build_config.output_schema = Some(schema);
        self
    }

    /// Set maximum retries for structured output validation.
    pub fn structured_output_retries(mut self, retries: u32) -> Self {
        self.build_config.structured_output_retries = retries;
        self
    }

    /// Resume from an existing session.
    pub fn resume_session(mut self, session: Session) -> Self {
        self.build_config.resume_session = Some(session);
        self
    }

    /// Set run-scoped hook overrides.
    pub fn with_hook_run_overrides(mut self, overrides: HookRunOverrides) -> Self {
        self.build_config.hooks_override = overrides;
        self
    }

    /// Set a pre-built hook engine.
    pub fn with_hook_engine(mut self, hook_engine: Arc<dyn HookEngine>) -> Self {
        self.build_config.hook_engine_override = Some(hook_engine);
        self
    }

    /// Set a pre-built skill runtime.
    pub fn with_skill_engine(mut self, engine: Arc<meerkat_core::skills::SkillRuntime>) -> Self {
        self.build_config.skill_engine_override = Some(engine);
        self
    }

    /// Set the runtime build mode used by the factory.
    pub fn runtime_build_mode(mut self, mode: RuntimeBuildMode) -> Self {
        self.build_config.runtime_build_mode = mode;
        self
    }

    /// Set an LLM client override and build through the factory.
    pub fn llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.build_config.llm_client_override = Some(client);
        self
    }

    /// Set a tool dispatcher override and build through the factory.
    pub fn tools(mut self, tools: Arc<dyn AgentToolDispatcher>) -> Self {
        self.build_config.tool_dispatcher_override = Some(tools);
        self
    }

    /// Set a session store override and build through the factory.
    pub fn session_store(mut self, store: Arc<dyn AgentSessionStore>) -> Self {
        self.build_config.session_store_override = Some(store);
        self
    }

    /// Set a default event channel used when run methods are called without
    /// per-call event channels.
    pub fn with_default_event_tx(
        mut self,
        event_tx: mpsc::Sender<meerkat_core::AgentEvent>,
    ) -> Self {
        self.build_config.event_tx = Some(event_tx);
        self
    }

    /// Set a context compactor.
    ///
    /// This is intentionally a builder-only core escape hatch. Facade builds
    /// route compaction through the factory/config pipeline; use
    /// [`crate::CoreAgentBuilder`] when a test or embedding must inject a
    /// bespoke compactor directly into the core agent loop. Calling this on the
    /// facade builder makes `try_build` return a configuration error.
    pub fn compactor(mut self, _compactor: Arc<dyn meerkat_core::compact::Compactor>) -> Self {
        self.unsupported_core_injections.push("compactor");
        self
    }

    /// Set the memory store for indexing compaction discards.
    ///
    /// This direct core-loop injection is intentionally not a facade authority;
    /// the factory owns memory tool/session composition. Use
    /// [`crate::CoreAgentBuilder`] for low-level standalone tests. Calling this
    /// on the facade builder makes `try_build` return a configuration error.
    pub fn memory_store(mut self, _store: Arc<dyn meerkat_core::memory::MemoryStore>) -> Self {
        self.unsupported_core_injections.push("memory_store");
        self
    }

    /// Runtime-backed turn-state handles are owned by the factory runtime mode.
    ///
    /// Use `runtime_build_mode` to supply session-owned runtime bindings.
    /// Calling this standalone-only injection method makes `try_build` return a
    /// configuration error instead of silently dropping the supplied handle.
    pub fn with_turn_state_handle(
        mut self,
        _handle: Arc<dyn meerkat_core::TurnStateHandle>,
    ) -> Self {
        self.unsupported_core_injections
            .push("with_turn_state_handle");
        self
    }

    /// Build an agent from already-created low-level client/tool/store parts.
    ///
    /// The supplied resources are treated as explicit overrides, then the build
    /// still routes through [`AgentFactory::build_agent`] for shared session,
    /// provider, runtime, hook, and metadata semantics.
    pub fn build(
        mut self,
        client: Arc<dyn AgentLlmClient>,
        tools: Arc<dyn AgentToolDispatcher>,
        store: Arc<dyn AgentSessionStore>,
    ) -> AgentBuilderBuildFuture {
        Box::pin(async move {
            if !self.model_explicit {
                self.build_config.model = client.model().to_string();
            }
            self.build_config.agent_llm_client_override = Some(client);
            self.build_config.tool_dispatcher_override = Some(tools);
            self.build_config.session_store_override = Some(store);
            self.try_build().await
        })
    }

    /// Build an agent using the factory resources already set on this builder.
    pub async fn try_build(self) -> Result<DynAgent, BuildAgentError> {
        if !self.unsupported_core_injections.is_empty() {
            return Err(BuildAgentError::Config(format!(
                "public AgentBuilder cannot accept standalone core injection method(s): {}. \
                 Facade builds route through AgentFactory-owned provider/session/runtime \
                 composition; use AgentFactory/AgentBuildConfig for facade-owned settings or \
                 CoreAgentBuilder for explicit standalone construction.",
                self.unsupported_core_injections.join(", ")
            )));
        }

        self.factory
            .build_agent(self.build_config, &self.config)
            .await
    }
}
