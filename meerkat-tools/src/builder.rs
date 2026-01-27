//! Shared tool dispatcher builders.

use std::sync::Arc;
use std::time::Duration;

use meerkat_comms::{Router, TrustedPeers};
use meerkat_comms_agent::DynCommsToolDispatcher;
use meerkat_core::AgentToolDispatcher;
use meerkat_mcp_client::McpRouter;
use tokio::sync::RwLock;

use crate::builtin::shell::ShellConfig;
use crate::builtin::{BuiltinToolConfig, CompositeDispatcher, CompositeDispatcherError, TaskStore};
use crate::dispatcher::{
    EmptyToolDispatcher, ToolDispatcher, ToolDispatcherConfig, ToolDispatcherKind,
};

/// Configuration for building a CompositeDispatcher.
#[derive(Clone)]
pub struct BuiltinDispatcherConfig {
    pub store: Arc<dyn TaskStore>,
    pub config: BuiltinToolConfig,
    pub shell_config: Option<ShellConfig>,
    pub external: Option<Arc<dyn AgentToolDispatcher>>,
    pub session_id: Option<String>,
    pub enable_wait_interrupt: bool,
}

/// Configuration for building an MCP-backed dispatcher.
#[derive(Clone)]
pub struct McpDispatcherConfig {
    pub router: Arc<McpRouter>,
    pub default_timeout: Duration,
}

/// Configuration for wrapping a dispatcher with comms tools.
#[derive(Clone)]
pub struct CommsDispatcherConfig {
    pub router: Arc<Router>,
    pub trusted_peers: Arc<RwLock<TrustedPeers>>,
}

/// Errors that can occur while building dispatchers.
#[derive(Debug, thiserror::Error)]
pub enum DispatcherBuildError {
    #[error("missing MCP router config")]
    MissingMcpConfig,

    #[error("missing builtins config")]
    MissingBuiltinsConfig,

    #[error("missing comms config")]
    MissingCommsConfig,

    #[error("composite dispatcher error: {0}")]
    Composite(#[from] CompositeDispatcherError),
}

/// Build a shared builtin dispatcher (CompositeDispatcher) as a trait object.
pub fn build_builtin_dispatcher(
    config: BuiltinDispatcherConfig,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    let dispatcher = if config.enable_wait_interrupt {
        CompositeDispatcher::new_with_interrupt(
            config.store,
            &config.config,
            config.shell_config,
            config.external,
            config.session_id,
            true,
        )?
    } else {
        CompositeDispatcher::new(
            config.store,
            &config.config,
            config.shell_config,
            config.external,
            config.session_id,
        )?
    };

    Ok(Arc::new(dispatcher))
}

/// Builder for shared tool dispatchers that returns trait objects.
#[derive(Default)]
pub struct ToolDispatcherBuilder {
    config: ToolDispatcherConfig,
    mcp: Option<McpDispatcherConfig>,
    builtins: Option<BuiltinDispatcherConfig>,
    comms: Option<CommsDispatcherConfig>,
}

impl ToolDispatcherBuilder {
    /// Create a new builder with the provided dispatcher config.
    pub fn new(config: ToolDispatcherConfig) -> Self {
        Self {
            config,
            mcp: None,
            builtins: None,
            comms: None,
        }
    }

    /// Attach MCP router configuration.
    pub fn with_mcp(mut self, router: Arc<McpRouter>, default_timeout: Duration) -> Self {
        self.mcp = Some(McpDispatcherConfig {
            router,
            default_timeout,
        });
        self
    }

    /// Attach builtin dispatcher configuration.
    pub fn with_builtins(mut self, builtins: BuiltinDispatcherConfig) -> Self {
        self.builtins = Some(builtins);
        self
    }

    /// Attach comms dispatcher configuration.
    pub fn with_comms(
        mut self,
        router: Arc<Router>,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
    ) -> Self {
        self.comms = Some(CommsDispatcherConfig {
            router,
            trusted_peers,
        });
        self
    }

    /// Build the dispatcher as a trait object.
    pub fn build(self) -> Result<Arc<dyn AgentToolDispatcher>, DispatcherBuildError> {
        match self.config.kind {
            ToolDispatcherKind::Empty => Ok(Arc::new(EmptyToolDispatcher)),
            ToolDispatcherKind::Mcp => {
                let mcp = self.mcp.ok_or(DispatcherBuildError::MissingMcpConfig)?;
                let mut dispatcher = ToolDispatcher::new(mcp.router, mcp.default_timeout);
                dispatcher.discover_tools();
                Ok(Arc::new(dispatcher))
            }
            ToolDispatcherKind::Composite => {
                let builtins = self
                    .builtins
                    .ok_or(DispatcherBuildError::MissingBuiltinsConfig)?;
                build_builtin_dispatcher(builtins).map_err(DispatcherBuildError::Composite)
            }
            ToolDispatcherKind::WithComms => {
                let comms = self.comms.ok_or(DispatcherBuildError::MissingCommsConfig)?;
                let inner: Arc<dyn AgentToolDispatcher> = if let Some(builtins) = self.builtins {
                    build_builtin_dispatcher(builtins).map_err(DispatcherBuildError::Composite)?
                } else if let Some(mcp) = self.mcp {
                    let mut dispatcher = ToolDispatcher::new(mcp.router, mcp.default_timeout);
                    dispatcher.discover_tools();
                    Arc::new(dispatcher)
                } else {
                    Arc::new(EmptyToolDispatcher)
                };

                let dispatcher =
                    DynCommsToolDispatcher::new(comms.router, comms.trusted_peers, inner);
                Ok(Arc::new(dispatcher))
            }
        }
    }
}
