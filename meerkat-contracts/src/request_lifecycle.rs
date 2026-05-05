/// Catalog-owned lifecycle classification for surface requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestLifecycle {
    InlineObservation,
    LongRunningPublishOnSuccess,
    LongRunningObservation,
}

/// How an RPC catalog entry derives lifecycle semantics from request params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcRequestLifecycleRule {
    Static(RequestLifecycle),
    SessionCreateInitialTurn,
}

impl RpcRequestLifecycleRule {
    pub const INLINE_OBSERVATION: Self = Self::Static(RequestLifecycle::InlineObservation);
    pub const LONG_RUNNING_PUBLISH_ON_SUCCESS: Self =
        Self::Static(RequestLifecycle::LongRunningPublishOnSuccess);

    pub fn resolve(self, params_json: Option<&str>) -> RequestLifecycle {
        match self {
            Self::Static(lifecycle) => lifecycle,
            Self::SessionCreateInitialTurn if session_create_runs_immediately(params_json) => {
                RequestLifecycle::LongRunningPublishOnSuccess
            }
            Self::SessionCreateInitialTurn => RequestLifecycle::InlineObservation,
        }
    }
}

pub fn rpc_request_lifecycle(method: &str, params_json: Option<&str>) -> RequestLifecycle {
    crate::rpc_method_catalog(crate::RpcMethodCatalogOptions::documented_surface())
        .into_iter()
        .find(|descriptor| descriptor.name == method)
        .map(|descriptor| descriptor.request_lifecycle.resolve(params_json))
        .unwrap_or(RequestLifecycle::InlineObservation)
}

fn session_create_runs_immediately(params_json: Option<&str>) -> bool {
    let Some(params_json) = params_json else {
        return true;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(params_json) else {
        return true;
    };
    !matches!(
        value
            .get("initial_turn")
            .and_then(serde_json::Value::as_str),
        Some("deferred")
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpToolRequestLifecycleDescriptor {
    pub name: &'static str,
    pub request_lifecycle: RequestLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpToolRequestLifecycleCatalog {
    pub default_lifecycle: RequestLifecycle,
    pub tools: &'static [McpToolRequestLifecycleDescriptor],
}

pub const MCP_TOOL_REQUEST_LIFECYCLE_CATALOG: McpToolRequestLifecycleCatalog =
    McpToolRequestLifecycleCatalog {
        default_lifecycle: RequestLifecycle::LongRunningObservation,
        tools: &[
            McpToolRequestLifecycleDescriptor {
                name: "meerkat_help",
                request_lifecycle: RequestLifecycle::LongRunningPublishOnSuccess,
            },
            McpToolRequestLifecycleDescriptor {
                name: "meerkat_run",
                request_lifecycle: RequestLifecycle::LongRunningPublishOnSuccess,
            },
            McpToolRequestLifecycleDescriptor {
                name: "meerkat_resume",
                request_lifecycle: RequestLifecycle::LongRunningPublishOnSuccess,
            },
        ],
    };

pub fn mcp_tool_request_lifecycle(tool_name: &str) -> RequestLifecycle {
    MCP_TOOL_REQUEST_LIFECYCLE_CATALOG
        .tools
        .iter()
        .find(|descriptor| descriptor.name == tool_name)
        .map(|descriptor| descriptor.request_lifecycle)
        .unwrap_or(MCP_TOOL_REQUEST_LIFECYCLE_CATALOG.default_lifecycle)
}
