//! CommsToolDispatcher - Implements AgentToolDispatcher for comms tools.

use crate::mcp::tools::{ToolContext, handle_tools_call, tools_list};
use crate::runtime::CommsRuntime;
use crate::{Router, TrustedPeers};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::types::ToolDef;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Tool dispatcher that provides comms tools.
pub struct CommsToolDispatcher<T: AgentToolDispatcher = NoOpDispatcher> {
    tool_context: ToolContext,
    inner: Option<Arc<T>>,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl CommsToolDispatcher<NoOpDispatcher> {
    pub fn new(router: Arc<Router>, trusted_peers: Arc<RwLock<TrustedPeers>>) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner: None,
            tool_defs,
        }
    }
}

impl<T: AgentToolDispatcher> CommsToolDispatcher<T> {
    pub fn with_inner(
        router: Arc<Router>,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        inner: Arc<T>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
        };
        let mut tools = comms_tool_defs();
        tools.extend(inner.tools().iter().map(Arc::clone));
        let tool_defs: Arc<[Arc<ToolDef>]> = tools.into();
        Self {
            tool_context,
            inner: Some(inner),
            tool_defs,
        }
    }
}

pub struct NoOpDispatcher;

#[async_trait]
impl AgentToolDispatcher for NoOpDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }
    async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
        Err(ToolError::NotFound {
            name: name.to_string(),
        })
    }
}

const COMMS_TOOL_NAMES: &[&str] = &[
    "send_message",
    "send_request",
    "send_response",
    "list_peers",
];

fn comms_tool_defs() -> Vec<Arc<ToolDef>> {
    tools_list()
        .into_iter()
        .map(|t| {
            Arc::new(ToolDef {
                name: t["name"].as_str().unwrap_or_default().to_string(),
                description: t["description"].as_str().unwrap_or_default().to_string(),
                input_schema: t["inputSchema"].clone(),
            })
        })
        .collect()
}

#[async_trait]
impl<T: AgentToolDispatcher + 'static> AgentToolDispatcher for CommsToolDispatcher<T> {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        if COMMS_TOOL_NAMES.contains(&name) {
            handle_tools_call(&self.tool_context, name, args)
                .await
                .map_err(|e| ToolError::ExecutionFailed { message: e })
        } else if let Some(inner) = &self.inner {
            inner.dispatch(name, args).await
        } else {
            Err(ToolError::NotFound {
                name: name.to_string(),
            })
        }
    }
}

pub struct DynCommsToolDispatcher {
    tool_context: ToolContext,
    inner: Arc<dyn AgentToolDispatcher>,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl DynCommsToolDispatcher {
    pub fn new(
        router: Arc<Router>,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        inner: Arc<dyn AgentToolDispatcher>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
        };
        let mut tools = comms_tool_defs();
        tools.extend(inner.tools().iter().map(Arc::clone));
        let tool_defs: Arc<[Arc<ToolDef>]> = tools.into();
        Self {
            tool_context,
            inner,
            tool_defs,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for DynCommsToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        if COMMS_TOOL_NAMES.contains(&name) {
            handle_tools_call(&self.tool_context, name, args)
                .await
                .map_err(|e| ToolError::ExecutionFailed { message: e })
        } else {
            self.inner.dispatch(name, args).await
        }
    }
}

pub fn wrap_with_comms(
    tools: Arc<dyn AgentToolDispatcher>,
    runtime: &CommsRuntime,
) -> Arc<dyn AgentToolDispatcher> {
    let router = runtime.router_arc();
    let trusted_peers = runtime.trusted_peers_shared();
    Arc::new(DynCommsToolDispatcher::new(router, trusted_peers, tools))
}
