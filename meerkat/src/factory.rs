//! AgentFactory - shared wiring for Meerkat interfaces.

use std::path::PathBuf;
use std::sync::Arc;

use meerkat_client::{
    DefaultClientFactory, DefaultFactoryConfig, FactoryError, LlmClient, LlmClientAdapter,
    LlmClientFactory, LlmProvider,
};
use meerkat_core::{
    AgentEvent, AgentToolDispatcher, ConcurrencyLimits, Provider, Session, SubAgentManager,
};
use meerkat_store::MemoryStore;
use meerkat_store::{SessionStore, StoreAdapter};
use meerkat_tools::builtin::shell::ShellConfig;
use meerkat_tools::builtin::sub_agent::{SubAgentConfig, SubAgentToolSet, SubAgentToolState};
use meerkat_tools::builtin::{BuiltinToolConfig, CompositeDispatcher, MemoryTaskStore, TaskStore};
use meerkat_tools::{BuiltinDispatcherConfig, CompositeDispatcherError, build_builtin_dispatcher};
use tokio::sync::{RwLock, mpsc};

/// Factory for creating agents with standard configuration.
#[derive(Debug, Clone)]
pub struct AgentFactory {
    pub store_path: PathBuf,
    pub project_root: Option<PathBuf>,
    pub enable_builtins: bool,
    pub enable_shell: bool,
    pub enable_subagents: bool,
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
        if !self.enable_subagents {
            return build_builtin_dispatcher(builder);
        }

        let BuiltinDispatcherConfig {
            store,
            config,
            shell_config,
            external,
            session_id,
        } = builder;

        let shell_config_for_subagents = shell_config.clone();
        let mut composite = self
            .build_composite_dispatcher(store, &config, shell_config, external.clone(), session_id)
            .await?;

        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits, 0));
        let client_factory: Arc<dyn LlmClientFactory> = Arc::new(DefaultClientFactory::new());

        let sub_agent_task_store = MemoryTaskStore::new();
        let sub_agent_factory = self.clone().subagents(false).comms(false);
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

        let sub_agent_store: Arc<dyn meerkat_core::AgentSessionStore> = Arc::new(
            sub_agent_factory
                .build_store_adapter(Arc::new(MemoryStore::new()))
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
