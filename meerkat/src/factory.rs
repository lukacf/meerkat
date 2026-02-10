//! AgentFactory - shared wiring for Meerkat interfaces.

#[cfg(not(feature = "memory-store"))]
use async_trait::async_trait;
#[cfg(not(feature = "memory-store"))]
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use meerkat_client::{
    DefaultClientFactory, DefaultFactoryConfig, FactoryError, LlmClient, LlmClientAdapter,
    LlmClientFactory, LlmProvider, ProviderResolver,
};
use meerkat_core::{
    Agent, AgentBuilder, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    BudgetLimits, Config, HookRunOverrides, OutputSchema, Provider, Session, SessionMetadata,
    SessionTooling,
};
#[cfg(feature = "sub-agents")]
use meerkat_core::{ConcurrencyLimits, SubAgentManager};
#[cfg(not(feature = "memory-store"))]
use meerkat_core::{SessionId, SessionMeta};
#[cfg(feature = "jsonl-store")]
use meerkat_store::JsonlStore;
#[cfg(feature = "memory-store")]
use meerkat_store::MemoryStore;
#[cfg(not(feature = "memory-store"))]
use meerkat_store::SessionFilter;
use meerkat_store::{SessionStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;
use meerkat_tools::builtin::shell::ShellConfig;
#[cfg(feature = "sub-agents")]
use meerkat_tools::builtin::sub_agent::{SubAgentConfig, SubAgentToolSet, SubAgentToolState};
use meerkat_tools::builtin::{
    BuiltinToolConfig, CompositeDispatcher, FileTaskStore, MemoryTaskStore, TaskStore,
    ToolPolicyLayer,
};
use meerkat_tools::{BuiltinDispatcherConfig, CompositeDispatcherError, build_builtin_dispatcher};
use tokio::sync::{RwLock, mpsc};

#[cfg(feature = "comms")]
use crate::{build_comms_runtime_from_config, compose_tools_with_comms};
use crate::{create_default_hook_engine, resolve_layered_hooks_config};

/// Ephemeral in-process store used when no storage backend feature is enabled.
#[cfg(not(feature = "memory-store"))]
#[derive(Default)]
struct EphemeralSessionStore {
    sessions: RwLock<HashMap<SessionId, Session>>,
}

#[cfg(not(feature = "memory-store"))]
impl EphemeralSessionStore {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }
}

#[cfg(not(feature = "memory-store"))]
#[async_trait]
impl SessionStore for EphemeralSessionStore {
    async fn save(&self, session: &Session) -> Result<(), meerkat_store::StoreError> {
        self.sessions
            .write()
            .await
            .insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, meerkat_store::StoreError> {
        Ok(self.sessions.read().await.get(id).cloned())
    }

    async fn list(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, meerkat_store::StoreError> {
        let mut metas: Vec<SessionMeta> = self
            .sessions
            .read()
            .await
            .values()
            .map(SessionMeta::from)
            .collect();

        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        if let Some(created_after) = filter.created_after {
            metas.retain(|m| m.created_at >= created_after);
        }
        if let Some(updated_after) = filter.updated_after {
            metas.retain(|m| m.updated_at >= updated_after);
        }
        if let Some(offset) = filter.offset {
            metas = metas.into_iter().skip(offset).collect();
        }
        if let Some(limit) = filter.limit {
            metas.truncate(limit);
        }

        Ok(metas)
    }

    async fn delete(&self, id: &SessionId) -> Result<(), meerkat_store::StoreError> {
        self.sessions.write().await.remove(id);
        Ok(())
    }
}

/// Type-erased agent using trait objects.
pub type DynAgent = Agent<dyn AgentLlmClient, dyn AgentToolDispatcher, dyn AgentSessionStore>;

/// Full configuration for building an agent via [`AgentFactory::build_agent()`].
pub struct AgentBuildConfig {
    /// Model name (e.g. "claude-sonnet-4-5").
    pub model: String,
    /// Explicit provider. If `None`, inferred from the model name.
    pub provider: Option<Provider>,
    /// Max tokens per turn. If `None`, uses `Config::max_tokens`.
    pub max_tokens: Option<u32>,
    /// Override the system prompt. If `None`, uses the default composed prompt.
    pub system_prompt: Option<String>,
    /// Optional output schema for structured extraction.
    pub output_schema: Option<OutputSchema>,
    /// How many retries for structured output validation.
    pub structured_output_retries: u32,
    /// Run-scoped hook overrides.
    pub hooks_override: HookRunOverrides,
    /// Whether to enable comms host mode.
    pub host_mode: bool,
    /// Name for the comms participant (required when `host_mode` is `true`).
    pub comms_name: Option<String>,
    /// Resume from an existing session instead of starting fresh.
    pub resume_session: Option<Session>,
    /// Budget limits. If `None`, uses `Config::budget_limits()`.
    pub budget_limits: Option<BudgetLimits>,
    /// Optional event channel for streaming agent events.
    pub event_tx: Option<mpsc::Sender<AgentEvent>>,
    /// Override LLM client (for testing or embedding).
    pub llm_client_override: Option<Arc<dyn LlmClient>>,
    /// Provider-specific parameters (e.g., thinking config, reasoning effort).
    pub provider_params: Option<serde_json::Value>,
    /// External tool dispatcher to compose with builtins (e.g., MCP callback tools).
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Per-build override for factory-level `enable_builtins`.
    /// When `Some`, takes precedence over `AgentFactory::enable_builtins`.
    pub override_builtins: Option<bool>,
    /// Per-build override for factory-level `enable_shell`.
    /// When `Some`, takes precedence over `AgentFactory::enable_shell`.
    pub override_shell: Option<bool>,
}

impl std::fmt::Debug for AgentBuildConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBuildConfig")
            .field("model", &self.model)
            .field("provider", &self.provider)
            .field("max_tokens", &self.max_tokens)
            .field(
                "system_prompt",
                &self
                    .system_prompt
                    .as_deref()
                    .map(|s| if s.len() > 64 { &s[..64] } else { s }),
            )
            .field("output_schema", &self.output_schema.is_some())
            .field("structured_output_retries", &self.structured_output_retries)
            .field("host_mode", &self.host_mode)
            .field("comms_name", &self.comms_name)
            .field("resume_session", &self.resume_session.is_some())
            .field("budget_limits", &self.budget_limits)
            .field("event_tx", &self.event_tx.is_some())
            .field("llm_client_override", &self.llm_client_override.is_some())
            .field("provider_params", &self.provider_params.is_some())
            .field("external_tools", &self.external_tools.is_some())
            .field("override_builtins", &self.override_builtins)
            .field("override_shell", &self.override_shell)
            .finish()
    }
}

impl AgentBuildConfig {
    /// Create a new build config with sensible defaults for the given model.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            provider: None,
            max_tokens: None,
            system_prompt: None,
            output_schema: None,
            structured_output_retries: 2,
            hooks_override: HookRunOverrides::default(),
            host_mode: false,
            comms_name: None,
            resume_session: None,
            budget_limits: None,
            event_tx: None,
            llm_client_override: None,
            provider_params: None,
            external_tools: None,
            override_builtins: None,
            override_shell: None,
        }
    }
}

/// Errors that can occur when building an agent via [`AgentFactory::build_agent()`].
#[derive(Debug, thiserror::Error)]
pub enum BuildAgentError {
    /// Cannot infer provider from the given model name.
    #[error("Cannot infer provider from model '{model}'")]
    UnknownProvider { model: String },

    /// API key is not set for the resolved provider.
    #[error("API key not set for provider '{provider}'")]
    MissingApiKey { provider: String },

    /// LLM client creation failed.
    #[error("LLM client creation failed: {0}")]
    LlmClient(#[from] FactoryError),

    /// Tool dispatcher creation failed.
    #[error("Tool dispatcher creation failed: {0}")]
    ToolDispatcher(#[from] CompositeDispatcherError),

    /// Comms runtime failed to initialize.
    #[error("Comms runtime failed: {0}")]
    #[cfg(feature = "comms")]
    Comms(String),

    /// Configuration error.
    #[error("Config error: {0}")]
    Config(String),

    /// `host_mode` was set but `comms_name` is missing.
    #[error("host_mode requires comms_name to be set")]
    #[cfg(feature = "comms")]
    HostModeRequiresCommsName,
}

/// Return the canonical string key for a provider.
pub fn provider_key(provider: Provider) -> &'static str {
    provider.as_str()
}

/// Factory for creating agents with standard configuration.
#[derive(Debug, Clone)]
pub struct AgentFactory {
    pub store_path: PathBuf,
    pub project_root: Option<PathBuf>,
    pub enable_builtins: bool,
    pub enable_shell: bool,
    pub enable_subagents: bool,
    #[cfg(feature = "comms")]
    pub enable_comms: bool,
}

impl AgentFactory {
    /// Create a new factory with the required session store path.
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        Self {
            store_path: store_path.into(),
            project_root: None,
            enable_builtins: false,
            enable_shell: false,
            enable_subagents: false,
            #[cfg(feature = "comms")]
            enable_comms: false,
        }
    }

    /// Set the project root used for tool persistence.
    pub fn project_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.project_root = Some(path.into());
        self
    }

    /// Enable or disable builtin tools.
    pub fn builtins(mut self, enabled: bool) -> Self {
        self.enable_builtins = enabled;
        self
    }

    /// Enable or disable shell tools.
    pub fn shell(mut self, enabled: bool) -> Self {
        self.enable_shell = enabled;
        self
    }

    /// Enable or disable sub-agent tools.
    pub fn subagents(mut self, enabled: bool) -> Self {
        self.enable_subagents = enabled;
        self
    }

    /// Enable or disable comms tools.
    #[cfg(feature = "comms")]
    pub fn comms(mut self, enabled: bool) -> Self {
        self.enable_comms = enabled;
        self
    }

    /// Build an LLM adapter for the provided client/model.
    pub async fn build_llm_adapter(
        &self,
        client: Arc<dyn LlmClient>,
        model: impl Into<String>,
    ) -> LlmClientAdapter {
        LlmClientAdapter::new(client, model.into())
    }

    /// Build an LLM adapter, optionally wiring an event channel for streaming.
    pub async fn build_llm_adapter_with_events(
        &self,
        client: Arc<dyn LlmClient>,
        model: impl Into<String>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> LlmClientAdapter {
        match event_tx {
            Some(tx) => LlmClientAdapter::with_event_channel(client, model.into(), tx),
            None => LlmClientAdapter::new(client, model.into()),
        }
    }

    /// Build an LLM client for a provider with optional base URL override.
    pub async fn build_llm_client(
        &self,
        provider: Provider,
        api_key: Option<String>,
        base_url: Option<String>,
    ) -> Result<Arc<dyn LlmClient>, FactoryError> {
        let mapped = match provider {
            Provider::Anthropic => LlmProvider::Anthropic,
            Provider::OpenAI => LlmProvider::OpenAi,
            Provider::Gemini => LlmProvider::Gemini,
            Provider::Other => return Err(FactoryError::UnsupportedProvider("other".to_string())),
        };

        let mut config = DefaultFactoryConfig::default();
        if let Some(url) = base_url {
            match mapped {
                LlmProvider::Anthropic => config = config.with_anthropic_base_url(url),
                LlmProvider::OpenAi => config = config.with_openai_base_url(url),
                LlmProvider::Gemini => config = config.with_gemini_base_url(url),
            }
        }

        let factory = DefaultClientFactory::with_config(config);
        factory.create_client(mapped, api_key)
    }

    /// Wrap a session store in the shared adapter.
    pub async fn build_store_adapter<S: SessionStore + 'static>(
        &self,
        store: Arc<S>,
    ) -> StoreAdapter<S> {
        StoreAdapter::new(store)
    }

    /// Build a composite dispatcher so callers can register sub-agent tools.
    pub async fn build_composite_dispatcher(
        &self,
        store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
    ) -> Result<CompositeDispatcher, CompositeDispatcherError> {
        CompositeDispatcher::new(store, config, shell_config, external, session_id)
    }

    /// Build a shared builtin dispatcher using the provided config.
    pub async fn build_builtin_dispatcher(
        &self,
        store: Arc<dyn TaskStore>,
        config: BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
    ) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
        let builder = BuiltinDispatcherConfig {
            store,
            config,
            shell_config,
            external,
            session_id,
        };
        #[cfg(not(feature = "sub-agents"))]
        {
            return build_builtin_dispatcher(builder);
        }

        #[cfg(feature = "sub-agents")]
        if !self.enable_subagents {
            return build_builtin_dispatcher(builder);
        }

        #[cfg(feature = "sub-agents")]
        {
            let BuiltinDispatcherConfig {
                store,
                config,
                shell_config,
                external,
                session_id,
            } = builder;

            let shell_config_for_subagents = shell_config.clone();
            let mut composite = self
                .build_composite_dispatcher(
                    store,
                    &config,
                    shell_config,
                    external.clone(),
                    session_id,
                )
                .await?;

            let limits = ConcurrencyLimits::default();
            let manager = Arc::new(SubAgentManager::new(limits, 0));
            let client_factory: Arc<dyn LlmClientFactory> = Arc::new(DefaultClientFactory::new());

            let sub_agent_task_store = MemoryTaskStore::new();
            let sub_agent_factory = {
                let factory = self.clone().subagents(false);
                #[cfg(feature = "comms")]
                let factory = factory.comms(false);
                factory
            };
            let sub_agent_dispatcher = sub_agent_factory
                .build_composite_dispatcher(
                    Arc::new(sub_agent_task_store),
                    &config,
                    shell_config_for_subagents,
                    external,
                    None,
                )
                .await?;
            let sub_agent_tools: Arc<dyn AgentToolDispatcher> = Arc::new(sub_agent_dispatcher);

            #[cfg(feature = "memory-store")]
            let sub_agent_store: Arc<dyn meerkat_core::AgentSessionStore> = Arc::new(
                sub_agent_factory
                    .build_store_adapter(Arc::new(MemoryStore::new()))
                    .await,
            );
            #[cfg(not(feature = "memory-store"))]
            let sub_agent_store: Arc<dyn meerkat_core::AgentSessionStore> = Arc::new(
                sub_agent_factory
                    .build_store_adapter(Arc::new(EphemeralSessionStore::new()))
                    .await,
            );

            let parent_session = Arc::new(RwLock::new(Session::new()));
            let sub_agent_config = SubAgentConfig::default();
            let state = Arc::new(SubAgentToolState::new(
                manager,
                client_factory,
                sub_agent_tools,
                sub_agent_store,
                parent_session,
                sub_agent_config,
                0,
            ));

            let tool_set = SubAgentToolSet::new(state);
            composite.register_sub_agent_tools(tool_set, &config)?;

            Ok(Arc::new(composite))
        }
    }

    /// Build a fully-configured, type-erased agent ready to run.
    ///
    /// This method consolidates the agent construction pipeline that was previously
    /// repeated across all surfaces (CLI, REST, MCP server):
    ///   load config, resolve provider/model, check API key, create LLM client +
    ///   adapter, build tool dispatcher, create comms runtime, compose tools with
    ///   comms, resolve hooks, build system prompt, wire AgentBuilder, and set
    ///   SessionMetadata.
    pub async fn build_agent(
        &self,
        mut build_config: AgentBuildConfig,
        config: &Config,
    ) -> Result<DynAgent, BuildAgentError> {
        // 1. Validate host_mode
        #[cfg(feature = "comms")]
        if build_config.host_mode && build_config.comms_name.is_none() {
            return Err(BuildAgentError::HostModeRequiresCommsName);
        }

        // 2. Resolve provider
        let provider = match build_config.provider {
            Some(p) => p,
            None => {
                let inferred = ProviderResolver::infer_from_model(&build_config.model);
                if inferred == Provider::Other {
                    return Err(BuildAgentError::UnknownProvider {
                        model: build_config.model.clone(),
                    });
                }
                inferred
            }
        };

        // 3. Create LLM client
        let llm_client: Arc<dyn LlmClient> = match build_config.llm_client_override {
            Some(client) => client,
            None => {
                if ProviderResolver::api_key_for(provider).is_none() {
                    return Err(BuildAgentError::MissingApiKey {
                        provider: provider_key(provider).to_string(),
                    });
                }
                let base_url = config
                    .providers
                    .base_urls
                    .as_ref()
                    .and_then(|map| map.get(provider_key(provider)).cloned());
                ProviderResolver::client_for(provider, base_url)
            }
        };

        // 4. Create LLM adapter (with optional provider_params and event channel)
        let model = build_config.model.clone();
        let mut llm_adapter_inner = match build_config.event_tx.clone() {
            Some(tx) => LlmClientAdapter::with_event_channel(llm_client, model.clone(), tx),
            None => LlmClientAdapter::new(llm_client, model.clone()),
        };
        if let Some(params) = build_config.provider_params.clone() {
            llm_adapter_inner = llm_adapter_inner.with_provider_params(Some(params));
        }
        let llm_adapter: Arc<dyn AgentLlmClient> = Arc::new(llm_adapter_inner);

        // 5. Resolve max_tokens
        let max_tokens = build_config.max_tokens.unwrap_or(config.max_tokens);

        // 6. Build tool dispatcher (with optional external tools and per-build overrides)
        //    Extract system_prompt before build_config is partially moved.
        let per_request_prompt = build_config.system_prompt.take();
        let effective_builtins = build_config
            .override_builtins
            .unwrap_or(self.enable_builtins);
        let effective_shell = build_config.override_shell.unwrap_or(self.enable_shell);
        let (mut tools, mut tool_usage_instructions) = self
            .build_tool_dispatcher_for_agent_with_overrides(
                config,
                build_config.external_tools,
                effective_builtins,
                effective_shell,
            )?;

        // 7. Create session store adapter
        #[cfg(feature = "jsonl-store")]
        let store_adapter: Arc<dyn AgentSessionStore> = {
            let store = JsonlStore::new(self.store_path.clone());
            store
                .init()
                .await
                .map_err(|e| BuildAgentError::Config(format!("Store init failed: {e}")))?;
            Arc::new(StoreAdapter::new(Arc::new(store)))
        };
        #[cfg(all(not(feature = "jsonl-store"), feature = "memory-store"))]
        let store_adapter: Arc<dyn AgentSessionStore> =
            Arc::new(self.build_store_adapter(Arc::new(MemoryStore::new())).await);
        #[cfg(all(not(feature = "jsonl-store"), not(feature = "memory-store")))]
        let store_adapter: Arc<dyn AgentSessionStore> = Arc::new(
            self.build_store_adapter(Arc::new(EphemeralSessionStore::new()))
                .await,
        );

        // 8. Create comms runtime
        #[cfg(feature = "comms")]
        let comms_runtime = if build_config.host_mode {
            let comms_name = build_config
                .comms_name
                .as_ref()
                .ok_or(BuildAgentError::HostModeRequiresCommsName)?;
            let base_dir = self
                .project_root
                .clone()
                .unwrap_or_else(|| self.store_path.clone());
            let runtime = build_comms_runtime_from_config(config, base_dir, comms_name)
                .await
                .map_err(BuildAgentError::Comms)?;
            Some(runtime)
        } else {
            None
        };

        // 9. Compose tools with comms
        #[cfg(feature = "comms")]
        if let Some(ref runtime) = comms_runtime {
            let composed = compose_tools_with_comms(tools, tool_usage_instructions, runtime)
                .map_err(|e| {
                    BuildAgentError::Config(format!("Failed to compose comms tools: {e}"))
                })?;
            tools = composed.0;
            tool_usage_instructions = composed.1;
        }

        // 10. Resolve hooks
        let hooks_root = self
            .project_root
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("."));
        let layered_hooks = resolve_layered_hooks_config(hooks_root, config).await;
        let hook_engine = create_default_hook_engine(layered_hooks);

        // 11. Build skill engine and inventory section (if skills feature enabled)
        #[cfg(feature = "skills")]
        let skill_inventory_section = {
            use meerkat_skills::{DefaultSkillEngine, EmbeddedSkillSource, FilesystemSkillSource, CompositeSkillSource};
            use meerkat_core::skills::{SkillEngine, SkillScope, SkillSource};

            let mut sources: Vec<Box<dyn meerkat_core::skills::SkillSource>> = Vec::new();

            // Project-level skills (highest precedence)
            let project_root = self.project_root.as_deref().unwrap_or_else(|| std::path::Path::new("."));
            sources.push(Box::new(FilesystemSkillSource::new(
                project_root.join(".rkat/skills"),
                SkillScope::Project,
            )));

            // User-level skills
            if let Some(home) = std::env::var_os("HOME") {
                sources.push(Box::new(FilesystemSkillSource::new(
                    std::path::PathBuf::from(home).join(".rkat/skills"),
                    SkillScope::User,
                )));
            }

            // Embedded skills (lowest precedence)
            sources.push(Box::new(EmbeddedSkillSource::new()));

            let composite = CompositeSkillSource::new(sources);

            // Collect active skill IDs from all sources (project, user, embedded)
            let skill_ids: Vec<meerkat_core::skills::SkillId> = match composite.list().await {
                Ok(descriptors) => descriptors.into_iter().map(|d| d.id).collect(),
                Err(_) => Vec::new(),
            };

            // Collect available capability strings for filtering
            let available_caps: Vec<String> = meerkat_contracts::build_capabilities()
                .into_iter()
                .map(|c| c.id.to_string())
                .collect();

            let engine = DefaultSkillEngine::new(Box::new(composite), available_caps);

            // Generate inventory section for system prompt
            let section = match engine.inventory_section().await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to generate skill inventory section: {e}");
                    String::new()
                }
            };

            (section, Some(skill_ids))
        };
        #[cfg(not(feature = "skills"))]
        let skill_inventory_section: (String, Option<Vec<meerkat_core::skills::SkillId>>) = (String::new(), None);
        let (inventory_section, active_skill_ids) = skill_inventory_section;

        // 12. Build system prompt (single canonical path)
        let extra_sections: Vec<&str> = if inventory_section.is_empty() {
            vec![]
        } else {
            vec![inventory_section.as_str()]
        };
        let system_prompt = crate::assemble_system_prompt(
            config,
            per_request_prompt.as_deref(),
            &extra_sections,
            &tool_usage_instructions,
        )
        .await;

        // 12. Build AgentBuilder
        let budget_limits = build_config
            .budget_limits
            .unwrap_or_else(|| config.budget_limits());

        let mut builder = AgentBuilder::new()
            .model(model.clone())
            .max_tokens_per_turn(max_tokens)
            .budget(budget_limits)
            .system_prompt(system_prompt)
            .structured_output_retries(build_config.structured_output_retries)
            .with_hook_run_overrides(build_config.hooks_override);

        if let Some(schema) = build_config.output_schema {
            builder = builder.output_schema(schema);
        }
        if let Some(session) = build_config.resume_session {
            builder = builder.resume_session(session);
        }
        #[cfg(feature = "comms")]
        if let Some(runtime) = comms_runtime {
            builder =
                builder.with_comms_runtime(
                    Arc::new(runtime) as Arc<dyn meerkat_core::agent::CommsRuntime>
                );
        }
        if let Some(engine) = hook_engine {
            builder = builder.with_hook_engine(engine);
        }

        // 12b. Wire memory store (when memory-store-session is enabled)
        #[cfg(feature = "memory-store-session")]
        {
            let memory_dir = self.store_path.join("memory");
            match meerkat_memory::HnswMemoryStore::open(&memory_dir) {
                Ok(store) => {
                    builder = builder.memory_store(
                        Arc::new(store) as Arc<dyn meerkat_core::memory::MemoryStore>
                    );
                }
                Err(e) => {
                    tracing::warn!("Failed to open HnswMemoryStore at {}: {e}", memory_dir.display());
                }
            }
        }

        // 12c. Wire compactor (when session-compaction is enabled)
        #[cfg(feature = "session-compaction")]
        {
            use meerkat_core::CompactionConfig;
            let compactor = Arc::new(meerkat_session::DefaultCompactor::new(
                CompactionConfig::default(),
            ));
            builder = builder.compactor(compactor);
        }

        // 13. Build agent
        let mut agent = builder.build(llm_adapter, tools, store_adapter).await;

        // 14. Set SessionMetadata
        let metadata = SessionMetadata {
            model,
            max_tokens,
            provider,
            tooling: SessionTooling {
                builtins: effective_builtins,
                shell: effective_shell,
                comms: build_config.host_mode,
                subagents: self.enable_subagents,
                active_skills: active_skill_ids,
            },
            host_mode: build_config.host_mode,
            comms_name: build_config.comms_name,
        };
        if let Err(err) = agent.session_mut().set_session_metadata(metadata) {
            tracing::warn!("Failed to store session metadata: {}", err);
        }

        Ok(agent)
    }

    /// Build the tool dispatcher and usage instructions.
    ///
    /// `effective_builtins` and `effective_shell` override the factory-level
    /// `enable_builtins` / `enable_shell` flags for this specific build.
    fn build_tool_dispatcher_for_agent_with_overrides(
        &self,
        _config: &Config,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        effective_builtins: bool,
        effective_shell: bool,
    ) -> Result<(Arc<dyn AgentToolDispatcher>, String), BuildAgentError> {
        if !effective_builtins {
            // No builtins — return the external tools if provided, otherwise empty.
            return match external {
                Some(ext) => Ok((ext, String::new())),
                None => Ok((Arc::new(EmptyToolDispatcher), String::new())),
            };
        }

        // Create a task store - use in-memory for simplicity; callers that need
        // file-backed persistence should use the lower-level APIs.
        let task_store: Arc<dyn TaskStore> = match self.project_root.as_ref() {
            Some(root) => Arc::new(FileTaskStore::in_project(root)),
            None => Arc::new(MemoryTaskStore::new()),
        };

        // Create shell config if shell is enabled
        let shell_config = if effective_shell {
            let project_root = self
                .project_root
                .clone()
                .unwrap_or_else(|| self.store_path.clone());
            Some(ShellConfig::with_project_root(project_root))
        } else {
            None
        };

        // Create builtin tool config - enable shell tools in policy if shell is enabled
        let builtin_config = if effective_shell {
            BuiltinToolConfig {
                policy: ToolPolicyLayer::new()
                    .enable_tool("shell")
                    .enable_tool("shell_job_status")
                    .enable_tool("shell_jobs")
                    .enable_tool("shell_job_cancel"),
                ..Default::default()
            }
        } else {
            BuiltinToolConfig::default()
        };

        // Create composite dispatcher (with optional external tools)
        let dispatcher =
            CompositeDispatcher::new(task_store, &builtin_config, shell_config, external, None)?;
        let usage_instructions = dispatcher.usage_instructions();

        Ok((Arc::new(dispatcher), usage_instructions))
    }
}
