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

/// Typed wire view of the `session/create` `initial_turn` field. Mirrors the
/// surface `InitialTurn` enum so lifecycle classification deserializes a closed
/// type instead of string-matching a raw JSON `Value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireInitialTurn {
    /// Run the first turn immediately as part of session creation (default).
    RunImmediately,
    /// Register the session and defer the first turn.
    Deferred,
}

#[derive(serde::Deserialize)]
struct SessionCreateInitialTurnView {
    #[serde(default)]
    initial_turn: Option<WireInitialTurn>,
}

fn session_create_runs_immediately(params_json: Option<&str>) -> bool {
    let Some(params_json) = params_json else {
        return true;
    };
    let Ok(view) = serde_json::from_str::<SessionCreateInitialTurnView>(params_json) else {
        return true;
    };
    !matches!(view.initial_turn, Some(WireInitialTurn::Deferred))
}
