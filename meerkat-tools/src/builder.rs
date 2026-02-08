//! Shared tool dispatcher builders.

use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "comms")]
use meerkat_comms::agent::DynCommsToolDispatcher;
#[cfg(feature = "comms")]
use meerkat_comms::{Router, TrustedPeers};
use meerkat_core::AgentToolDispatcher;
#[cfg(feature = "mcp")]
use meerkat_mcp::McpRouter;
use tokio::sync::RwLock;

use crate::builtin::shell::ShellConfig;
use crate::builtin::{BuiltinToolConfig, CompositeDispatcher, CompositeDispatcherError, TaskStore};
use crate::dispatcher::{EmptyToolDispatcher, ToolDispatcher};

/// Configuration for a specific tool dispatcher source
pub enum ToolDispatcherSource {
    /// Empty dispatcher (default)
    Empty,
    /// MCP-based tools
    #[cfg(feature = "mcp")]
    Mcp(McpDispatcherConfig),
    /// Built-in and sub-agent tools (boxed to reduce enum size)
    Composite(Box<BuiltinDispatcherConfig>),
}

/// Configuration for an MCP-based dispatcher
#[cfg(feature = "mcp")]
pub struct McpDispatcherConfig {
    pub router: Arc<McpRouter>,
}

/// Configuration for a built-in and sub-agent dispatcher
pub struct BuiltinDispatcherConfig {
    pub store: Arc<dyn TaskStore>,
    pub config: BuiltinToolConfig,
    pub shell_config: Option<ShellConfig>,
    pub external: Option<Arc<dyn AgentToolDispatcher>>,
    pub session_id: Option<String>,
}

/// Configuration for a comms-enabled dispatcher
#[cfg(feature = "comms")]
pub struct CommsDispatcherConfig {
    pub router: Arc<Router>,
    pub trusted_peers: Arc<RwLock<TrustedPeers>>,
}

/// Top-level configuration for building a ToolDispatcher
pub struct ToolDispatcherConfig {
    pub source: ToolDispatcherSource,
    #[cfg(feature = "comms")]
    pub comms: Option<CommsDispatcherConfig>,
    pub default_timeout: Duration,
}

impl Default for ToolDispatcherConfig {
    fn default() -> Self {
        Self {
            source: ToolDispatcherSource::Empty,
            #[cfg(feature = "comms")]
            comms: None,
            default_timeout: Duration::from_secs(30),
        }
    }
}

/// Builder for creating a ToolDispatcher
pub struct ToolDispatcherBuilder {
    config: ToolDispatcherConfig,
}

impl ToolDispatcherBuilder {
    /// Create a new builder with the given config
    pub fn new(config: ToolDispatcherConfig) -> Self {
        Self { config }
    }

    /// Set the dispatcher source
    pub fn source(mut self, source: ToolDispatcherSource) -> Self {
        self.config.source = source;
        self
    }

    /// Add comms tools to the dispatcher
    #[cfg(feature = "comms")]
    pub fn comms(mut self, comms: CommsDispatcherConfig) -> Self {
        self.config.comms = Some(comms);
        self
    }

    /// Set the default timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.default_timeout = timeout;
        self
    }

    /// Build the ToolDispatcher
    pub async fn build(self) -> Result<ToolDispatcher, CompositeDispatcherError> {
        let router: Arc<dyn AgentToolDispatcher> = match self.config.source {
            ToolDispatcherSource::Empty => Arc::new(EmptyToolDispatcher),
            #[cfg(feature = "mcp")]
            ToolDispatcherSource::Mcp(mcp) => mcp.router,
            ToolDispatcherSource::Composite(comp) => Arc::new(CompositeDispatcher::new(
                comp.store,
                &comp.config,
                comp.shell_config,
                comp.external,
                comp.session_id,
            )?),
        };

        // Wrap with comms if enabled
        #[cfg(feature = "comms")]
        let router = if let Some(comms) = self.config.comms {
            Arc::new(DynCommsToolDispatcher::new(
                comms.router,
                comms.trusted_peers,
                router,
            ))
        } else {
            router
        };

        // Populate registry from router's tools
        let mut registry = crate::registry::ToolRegistry::new();
        for tool_def in router.tools().iter() {
            registry.register((**tool_def).clone());
        }

        Ok(ToolDispatcher::new(registry, router).with_timeout(self.config.default_timeout))
    }
}

/// Create a builtin tool dispatcher with the given config.
pub fn build_builtin_dispatcher(
    config: BuiltinDispatcherConfig,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    Ok(Arc::new(CompositeDispatcher::new(
        config.store,
        &config.config,
        config.shell_config,
        config.external,
        config.session_id,
    )?))
}
