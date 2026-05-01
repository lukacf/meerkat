use meerkat_core::handles::SurfaceRequestKind;

/// How an RPC catalog entry derives lifecycle semantics from request params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcRequestLifecycleRule {
    Static(SurfaceRequestKind),
    SessionCreateInitialTurn,
}

impl RpcRequestLifecycleRule {
    pub const INLINE_OBSERVATION: Self = Self::Static(SurfaceRequestKind::InlineObservation);
    pub const SESSION_TURN: Self = Self::Static(SurfaceRequestKind::SessionTurn);

    pub fn resolve(self, params_json: Option<&str>) -> SurfaceRequestKind {
        match self {
            Self::Static(kind) => kind,
            Self::SessionCreateInitialTurn if session_create_runs_immediately(params_json) => {
                SurfaceRequestKind::SessionCreateWithTurn
            }
            Self::SessionCreateInitialTurn => SurfaceRequestKind::InlineObservation,
        }
    }
}

pub fn rpc_surface_request_kind(method: &str, params_json: Option<&str>) -> SurfaceRequestKind {
    rpc_surface_request_kind_for_options(
        crate::RpcMethodCatalogOptions::documented_surface(),
        method,
        params_json,
    )
}

pub fn rpc_surface_request_kind_for_options(
    options: crate::RpcMethodCatalogOptions,
    method: &str,
    params_json: Option<&str>,
) -> SurfaceRequestKind {
    crate::rpc_method_catalog(options)
        .into_iter()
        .find(|descriptor| descriptor.name == method)
        .map(|descriptor| descriptor.request_lifecycle.resolve(params_json))
        .unwrap_or(SurfaceRequestKind::InlineObservation)
}

pub fn rpc_tracked_surface_request_kind(
    method: &str,
    params_json: Option<&str>,
) -> Option<SurfaceRequestKind> {
    match rpc_surface_request_kind(method, params_json) {
        SurfaceRequestKind::InlineObservation => None,
        kind => Some(kind),
    }
}

pub fn rpc_tracked_surface_request_kind_for_options(
    options: crate::RpcMethodCatalogOptions,
    method: &str,
    params_json: Option<&str>,
) -> Option<SurfaceRequestKind> {
    match rpc_surface_request_kind_for_options(options, method, params_json) {
        SurfaceRequestKind::InlineObservation => None,
        kind => Some(kind),
    }
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
    pub request_kind: SurfaceRequestKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpToolRequestLifecycleCatalog {
    pub default_kind: SurfaceRequestKind,
    pub tools: &'static [McpToolRequestLifecycleDescriptor],
}

pub const MCP_TOOL_REQUEST_LIFECYCLE_CATALOG: McpToolRequestLifecycleCatalog =
    McpToolRequestLifecycleCatalog {
        default_kind: SurfaceRequestKind::CancellableObservation,
        tools: &[
            McpToolRequestLifecycleDescriptor {
                name: "meerkat_run",
                request_kind: SurfaceRequestKind::SessionCreateWithTurn,
            },
            McpToolRequestLifecycleDescriptor {
                name: "meerkat_resume",
                request_kind: SurfaceRequestKind::SessionTurn,
            },
            McpToolRequestLifecycleDescriptor {
                name: "meerkat_sessions",
                request_kind: SurfaceRequestKind::InlineObservation,
            },
        ],
    };

pub fn mcp_tool_surface_request_kind(tool_name: &str) -> SurfaceRequestKind {
    MCP_TOOL_REQUEST_LIFECYCLE_CATALOG
        .tools
        .iter()
        .find(|descriptor| descriptor.name == tool_name)
        .map(|descriptor| descriptor.request_kind)
        .unwrap_or(MCP_TOOL_REQUEST_LIFECYCLE_CATALOG.default_kind)
}

pub fn mcp_tracked_surface_request_kind(tool_name: &str) -> Option<SurfaceRequestKind> {
    match mcp_tool_surface_request_kind(tool_name) {
        SurfaceRequestKind::InlineObservation => None,
        kind => Some(kind),
    }
}
