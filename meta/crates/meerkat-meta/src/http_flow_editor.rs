use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, Uri, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::Value;
use tower::limit::GlobalConcurrencyLimitLayer;

use crate::access::{ACTION_MOBPACK_AUTHOR, ACTION_MOBPACK_DEPLOY, AccessController, AccessView};
use crate::http_console::ACCESS_DENIED_RPC_CODE;
use crate::http_sse::sse_access_context;
use crate::mobpack::MobpackRuntimeCatalogState;
use crate::rpc::{JSONRPC_VERSION, JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use crate::runtime::RuntimeDecisionState;

const FLOW_EDITOR_FRONTEND_INDEX_HTML: &str = include_str!("../flow-editor-dist/index.html");
const FLOW_EDITOR_FRONTEND_VENDOR_JS: &str = include_str!("../flow-editor-dist/react-globals.js");
const FLOW_EDITOR_FRONTEND_APP_JS: &str = include_str!("../flow-editor-dist/flow-editor.js");
const FLOW_EDITOR_FRONTEND_APP_CSS: &str = include_str!("../flow-editor-dist/flow-editor.css");

/// Standalone HTTP surface for the Meta Flow Editor.
///
/// This route plane deliberately serves only the editor shell and the
/// mobpack-authoring JSON-RPC methods. Console runtime methods remain on the
/// console RPC plane.
pub fn flow_editor_router() -> Router {
    standalone_favicon_router::<()>()
        .merge(flow_editor_frontend_router::<()>())
        .merge(flow_editor_rpc_router::<()>())
        .layer(standalone_concurrency_limit())
}

pub fn flow_editor_router_with_host_deploy() -> Router {
    standalone_favicon_router::<()>()
        .merge(flow_editor_frontend_router::<()>())
        .merge(flow_editor_rpc_router_allowing_host_deploy::<()>())
        .layer(standalone_concurrency_limit())
}

/// The most requests the standalone Flow Editor server handles at once; the
/// rest wait for a slot. Any local process can call it, and every request it
/// handles runs on the blocking pool, which it could otherwise fill.
const STANDALONE_FLOW_EDITOR_MAX_CONCURRENT_REQUESTS: usize = 16;

/// One limit for every request to a standalone router: one semaphore,
/// however often the layer is applied. axum applies a router's layer to
/// each route, and to a handler's route again for each connection it
/// serves, so a per-service limit (`ConcurrencyLimitLayer`) would bound one
/// route on one connection.
fn standalone_concurrency_limit() -> GlobalConcurrencyLimitLayer {
    GlobalConcurrencyLimitLayer::new(STANDALONE_FLOW_EDITOR_MAX_CONCURRENT_REQUESTS)
}

/// Quiet-favicon route for the standalone Flow Editor server only. It must
/// not be part of `flow_editor_frontend_router`: hosts that merge the Flow
/// Editor with the console (for example `build_reference_app_router`) already
/// register `/favicon.ico`, and axum panics on duplicate routes.
fn standalone_favicon_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
}

/// The Flow Editor behind the console's HTTP authentication, with no access
/// controller. Nobody holds a `mobpack.deploy` grant here, so it never runs
/// `rkat` on the host: deploys stay plans and validation stays structural.
///
/// Applies no session fence; use the routers a [`crate::UnifiedRuntime`] builds,
/// or the `_under_session_fences` variant
/// ([`crate::runtime::validate_console_token_under_session_fences`]).
pub fn protected_flow_editor_router(decisions: RuntimeDecisionState) -> Router {
    flow_editor_frontend_router::<()>().merge(flow_editor_rpc_router_with_decisions(decisions))
}

/// The Flow Editor a runtime mounts on its reference app router.
/// `host_deploy_without_access_control` is the host's opt-in to host deploys
/// for callers that no enforced access controller governs. A runtime that
/// does not speak for its composition (a candidate launch, `candidate`
/// carrying its state directory) refuses typed every host deploy, whatever
/// grant or opt-in the caller has, and every draft store write the
/// authoritative launch reads. A runtime whose agent factories hold a
/// host-chosen provider credential store (`explicit_provider_credentials`)
/// refuses typed every deploy with `execute`, whose `rkat` process could not
/// carry that store.
pub(crate) fn protected_flow_editor_router_with_runtime_catalog(
    decisions: RuntimeDecisionState,
    runtime_catalog: MobpackRuntimeCatalogState,
    access: Option<AccessController>,
    host_deploy_without_access_control: bool,
    session_fences: crate::auth::session_fence::ConsoleSessionFences,
    candidate: Option<FlowEditorCandidate>,
    explicit_provider_credentials: bool,
) -> Router {
    flow_editor_frontend_router::<()>().merge(flow_editor_rpc_router_with_runtime_catalog(
        decisions,
        Some(runtime_catalog),
        access,
        host_deploy_without_access_control,
        session_fences,
        candidate,
        explicit_provider_credentials,
    ))
}

/// The candidate launch a Flow Editor serves: its state directory, which
/// its refusal of a draft store write reads.
#[derive(Clone, Debug, Default)]
pub(crate) struct FlowEditorCandidate {
    pub(crate) state_dir: Option<std::path::PathBuf>,
}

pub fn flow_editor_frontend_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/flow-editor", get(flow_editor_frontend_index_handler))
        .route("/flow-editor/", get(flow_editor_frontend_index_handler))
        .route(
            "/flow-editor/assets/react-globals.js",
            get(flow_editor_frontend_vendor_js_handler),
        )
        .route(
            "/flow-editor/assets/flow-editor.js",
            get(flow_editor_frontend_app_js_handler),
        )
        .route(
            "/flow-editor/assets/flow-editor.css",
            get(flow_editor_frontend_app_css_handler),
        )
}

pub fn flow_editor_rpc_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().route("/flow-editor/rpc", post(flow_editor_rpc_handler))
}

pub fn flow_editor_rpc_router_allowing_host_deploy<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().route(
        "/flow-editor/rpc",
        post(flow_editor_rpc_handler_allowing_host_deploy),
    )
}

/// The RPC plane of [`protected_flow_editor_router`]: no access controller,
/// so no host deploys.
///
/// Applies no session fence; use the routers a [`crate::UnifiedRuntime`] builds,
/// or the `_under_session_fences` variant
/// ([`crate::runtime::validate_console_token_under_session_fences`]).
pub fn flow_editor_rpc_router_with_decisions(decisions: RuntimeDecisionState) -> Router {
    flow_editor_rpc_router_with_runtime_catalog(
        decisions,
        None,
        None,
        false,
        crate::auth::session_fence::ConsoleSessionFences::detached(),
        None,
        false,
    )
}

fn flow_editor_rpc_router_with_runtime_catalog(
    decisions: RuntimeDecisionState,
    runtime_catalog: Option<MobpackRuntimeCatalogState>,
    access: Option<AccessController>,
    host_deploy_without_access_control: bool,
    session_fences: crate::auth::session_fence::ConsoleSessionFences,
    candidate: Option<FlowEditorCandidate>,
    explicit_provider_credentials: bool,
) -> Router {
    Router::new()
        .route("/flow-editor/rpc", post(protected_flow_editor_rpc_handler))
        .with_state(FlowEditorRpcState {
            decisions,
            session_fences,
            runtime_catalog,
            access,
            host_deploy_without_access_control,
            candidate,
            explicit_provider_credentials,
        })
}

pub async fn flow_editor_frontend_index_handler() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            // Same policy as the console document: only its own origin
            // (for example a console operator panel) may frame it.
            (header::CONTENT_SECURITY_POLICY, "frame-ancestors 'self'"),
        ],
        FLOW_EDITOR_FRONTEND_INDEX_HTML,
    )
}

pub async fn flow_editor_frontend_vendor_js_handler() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        FLOW_EDITOR_FRONTEND_VENDOR_JS,
    )
}

pub async fn flow_editor_frontend_app_js_handler() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        FLOW_EDITOR_FRONTEND_APP_JS,
    )
}

pub async fn flow_editor_frontend_app_css_handler() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        FLOW_EDITOR_FRONTEND_APP_CSS,
    )
}

pub async fn flow_editor_rpc_handler(Json(request): Json<Value>) -> impl IntoResponse {
    flow_editor_rpc_handler_with_policy(request, false).await
}

pub async fn flow_editor_rpc_handler_allowing_host_deploy(
    Json(request): Json<Value>,
) -> impl IntoResponse {
    flow_editor_rpc_handler_with_policy(request, true).await
}

/// The store path of each request the standalone Flow Editor RPC handler has
/// started, for the test of how many it handles at once: the test counts
/// only the requests that name its own store, whatever else runs in the
/// process.
#[cfg(test)]
static STANDALONE_RPC_REQUESTS_STARTED: std::sync::Mutex<Vec<String>> =
    std::sync::Mutex::new(Vec::new());

async fn flow_editor_rpc_handler_with_policy(
    request: Value,
    allow_host_deploy: bool,
) -> impl IntoResponse {
    #[cfg(test)]
    if let Some(store_path) = request
        .get("params")
        .and_then(|params| params.get("store_path"))
        .and_then(Value::as_str)
    {
        STANDALONE_RPC_REQUESTS_STARTED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(store_path.to_string());
    }
    let parsed_request = match serde_json::from_value::<JsonRpcRequest>(request) {
        Ok(req) => req,
        Err(_) => {
            return (
                StatusCode::OK,
                Json::<Value>(serde_json::json!({
                    "jsonrpc": JSONRPC_VERSION,
                    "id": Value::Null,
                    "error": { "code": -32600, "message": "Invalid Request" }
                })),
            );
        }
    };
    let response_id = parsed_request.id.clone().unwrap_or(Value::Null);
    let response = dispatch_flow_editor_rpc_blocking(
        parsed_request,
        FlowEditorAuthReport {
            authenticated: false,
            mode: if allow_host_deploy {
                "standalone_host_deploy"
            } else {
                "none"
            },
            reason: if allow_host_deploy {
                "standalone Flow Editor authoring server with explicit host deploy opt-in"
            } else {
                "standalone Flow Editor authoring server"
            },
            host_access: allow_host_deploy,
            host_mutation_allowed: allow_host_deploy,
            deploy_execute_allowed: allow_host_deploy,
        },
        None,
        response_id,
    )
    .await;
    (StatusCode::OK, Json::<Value>(response))
}

/// Run the synchronous flow-editor RPC dispatcher on the blocking pool.
///
/// `meta/mobpacks/deploy` with `execute: true` (and `validate` via
/// `rkat mob validate`) blocks on a child process for up to the deploy
/// execution timeout; parking that wait on a tokio worker thread would
/// starve the async runtime, so every HTTP entry point routes through
/// `spawn_blocking` here.
async fn dispatch_flow_editor_rpc_blocking(
    request: JsonRpcRequest,
    auth: FlowEditorAuthReport,
    runtime_catalog: Option<MobpackRuntimeCatalogState>,
    response_id: Value,
) -> Value {
    match tokio::task::spawn_blocking(move || {
        handle_flow_editor_rpc_with_auth(request, auth, runtime_catalog.as_ref())
    })
    .await
    {
        Ok(response) => response,
        Err(err) => serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": response_id,
            "error": {
                "code": -32603,
                "message": format!("flow editor rpc task failed: {err}"),
            }
        }),
    }
}

#[derive(Clone)]
struct FlowEditorRpcState {
    decisions: RuntimeDecisionState,
    /// The Console's session fence record the route's token checks read.
    session_fences: crate::auth::session_fence::ConsoleSessionFences,
    runtime_catalog: Option<MobpackRuntimeCatalogState>,
    access: Option<AccessController>,
    /// Whether host deploys are allowed for callers that no enforced access
    /// controller governs. Off unless the host opted in: without a
    /// controller nobody holds a `mobpack.deploy` grant, and the route may
    /// be anonymous (an open console on loopback).
    host_deploy_without_access_control: bool,
    /// `Some` on a candidate launch, which runs no host deploy and writes no
    /// draft store the authoritative launch reads.
    candidate: Option<FlowEditorCandidate>,
    /// Whether the runtime's agent factories hold a host-chosen provider
    /// credential store, which a deploy's `rkat` process could not carry.
    explicit_provider_credentials: bool,
}

/// The `auth.mode` the runtime-mounted Flow Editor reports.
const REFERENCE_APP_AUTH_MODE: &str = "reference_app";

#[derive(Clone, Copy)]
struct FlowEditorAuthReport {
    authenticated: bool,
    mode: &'static str,
    reason: &'static str,
    /// The caller's host access, decided per call: whether it may run `rkat`
    /// on this host (deploy `execute`, `rkat_validate`), choose the draft
    /// store file, and pack path skills the skills catalog does not list.
    /// Under an enforced access controller it is the caller's own
    /// `mobpack.deploy` decision, deny rules included.
    host_access: bool,
    /// What `meta/capabilities` advertises as `host_mutation_allowed` and
    /// `deploy_execute_allowed`. Under an enforced access controller this is
    /// the coarse "allowed somewhere" signal, which can say yes where
    /// `host_access` says no. It grants nothing.
    host_mutation_allowed: bool,
    deploy_execute_allowed: bool,
}

async fn protected_flow_editor_rpc_handler(
    State(state): State<FlowEditorRpcState>,
    headers: HeaderMap,
    uri: Uri,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    let parsed_request = match serde_json::from_value::<JsonRpcRequest>(request) {
        Ok(req) => req,
        Err(_) => {
            return (
                StatusCode::OK,
                Json::<Value>(serde_json::json!({
                    "jsonrpc": JSONRPC_VERSION,
                    "id": Value::Null,
                    "error": { "code": -32600, "message": "Invalid Request" }
                })),
            );
        }
    };
    let access_view = match sse_access_context(
        Some(&state.decisions),
        &state.session_fences,
        state.access.as_ref(),
        &headers,
        &uri,
    ) {
        Ok(caller) => caller.access_view,
        Err(()) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json::<Value>(serde_json::json!({
                    "jsonrpc": JSONRPC_VERSION,
                    "id": parsed_request.id.unwrap_or(Value::Null),
                    "error": {
                        "code": -32600,
                        "message": "unauthorized: flow editor rpc requires a valid auth token",
                    }
                })),
            );
        }
    };
    if let Some(error) = flow_editor_rpc_access_violation(access_view.as_ref(), &parsed_request) {
        return (
            StatusCode::OK,
            Json::<Value>(serde_json::json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": parsed_request.id.unwrap_or(Value::Null),
                "error": error,
            })),
        );
    }
    // A candidate launch runs nothing on the host and writes no draft store
    // the authoritative launch reads: a deploy with `execute`, a validation
    // through `rkat` and such a draft write are refused as its RPC surface
    // refuses them. Authoring reads and deploy plans still answer.
    if let Some(refusal) = crate::rpc::candidate_launch_refusal(
        state.candidate.is_none(),
        state
            .candidate
            .as_ref()
            .and_then(|candidate| candidate.state_dir.as_deref()),
        parsed_request.method.as_str(),
        &parsed_request.params,
        parsed_request.id.clone().unwrap_or(Value::Null),
    ) {
        return (
            StatusCode::OK,
            Json::<Value>(serde_json::to_value(refusal).unwrap_or(Value::Null)),
        );
    }
    // A deploy that runs `rkat` could not carry the runtime's host-chosen
    // provider credential store: refused typed, whatever the caller's grant.
    if let Some(refusal) = crate::rpc::explicit_provider_credentials_refusal(
        state.explicit_provider_credentials,
        parsed_request.method.as_str(),
        &parsed_request.params,
        parsed_request.id.clone().unwrap_or(Value::Null),
    ) {
        return (
            StatusCode::OK,
            Json::<Value>(serde_json::to_value(refusal).unwrap_or(Value::Null)),
        );
    }
    // Host access (running `rkat` on this host, choosing the draft store
    // file, packing path skills the skills catalog does not list) needs a
    // `mobpack.deploy` grant. An enforced access controller decides it per
    // call, deny rules included; the check above already refused a host
    // deploy it does not grant. The capability advertisement uses the
    // coarser "allowed somewhere" signal, so the editor hides deploy
    // affordances the caller can never use; it grants nothing. Without a
    // controller nobody holds the grant, so only the host's explicit opt-in
    // gives host access. A deploy with `execute` that this runtime refuses
    // whatever the caller holds (a candidate launch, a host-chosen provider
    // credential store) is not advertised either.
    let (host_access, advertised_host_deploy) =
        match access_view.as_ref().filter(|view| view.enforced()) {
            Some(view) => (
                view.allows(ACTION_MOBPACK_DEPLOY),
                view.may_perform_anywhere(ACTION_MOBPACK_DEPLOY),
            ),
            None => (
                state.host_deploy_without_access_control,
                state.host_deploy_without_access_control,
            ),
        };
    let response_id = parsed_request.id.clone().unwrap_or(Value::Null);
    let response = dispatch_flow_editor_rpc_blocking(
        parsed_request,
        FlowEditorAuthReport {
            authenticated: true,
            mode: REFERENCE_APP_AUTH_MODE,
            reason: "reference app Flow Editor authoring server",
            host_access,
            host_mutation_allowed: advertised_host_deploy,
            deploy_execute_allowed: advertised_host_deploy
                && !state.explicit_provider_credentials
                && state.candidate.is_none(),
        },
        state.runtime_catalog.clone(),
        response_id,
    )
    .await;
    (StatusCode::OK, Json::<Value>(response))
}

/// Map a flow-editor RPC method to the ABAC action it requires. Mirrors the
/// console's `console_rpc_access_requirement`: `meta/capabilities` stays
/// open (it only describes the method surface), every mobpack-authoring
/// method requires `mobpack.author`, and a deploy with `execute: true`
/// requires `mobpack.deploy` instead.
fn flow_editor_rpc_access_requirement(method: &str, params: &Value) -> Option<&'static str> {
    if method == "meta/capabilities" {
        return None;
    }
    if !crate::rpc::MOBPACK_AUTHORING_METHODS.contains(&method) {
        // Unknown methods fall through to the dispatcher's method-not-found.
        return None;
    }
    if method == "meta/mobpacks/deploy"
        && params
            .get("execute")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Some(ACTION_MOBPACK_DEPLOY);
    }
    // `validate` with `rkat_validate: true` writes a rendered mobpack archive
    // to a caller-controlled path and spawns `rkat mob validate` — a
    // host-mutating side effect, not a pure authoring read. Gate it behind
    // the same deploy grant as host-executing deploys.
    if method == "meta/mobpacks/validate" && validate_rkat_execution_requested(params) {
        return Some(ACTION_MOBPACK_DEPLOY);
    }
    Some(ACTION_MOBPACK_AUTHOR)
}

/// True when a `meta/mobpacks/validate` request asks to render the pack and
/// run `rkat mob validate` on the host (mirrors
/// `mobpack::validate_with_rkat_requested`). This is the host-mutating arm:
/// it writes a mobpack archive and spawns a child process.
fn validate_rkat_execution_requested(params: &Value) -> bool {
    params
        .get("rkat_validate")
        .or_else(|| params.get("rkatValidate"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn flow_editor_rpc_access_violation(
    view: Option<&AccessView>,
    request: &JsonRpcRequest,
) -> Option<Value> {
    let view = view.filter(|view| view.enforced())?;
    let action = flow_editor_rpc_access_requirement(request.method.as_str(), &request.params)?;
    if view.allows(action) {
        return None;
    }
    Some(serde_json::json!({
        "code": ACCESS_DENIED_RPC_CODE,
        "message": format!("access denied: {action}"),
        "data": { "kind": "access_denied", "action": action },
    }))
}

pub fn handle_flow_editor_rpc(request: JsonRpcRequest) -> Value {
    handle_flow_editor_rpc_with_auth(
        request,
        FlowEditorAuthReport {
            authenticated: false,
            mode: "none",
            reason: "standalone Flow Editor authoring server",
            host_access: false,
            host_mutation_allowed: false,
            deploy_execute_allowed: false,
        },
        None,
    )
}

/// Every Flow Editor HTTP request runs through here. A caller without host
/// access (no `mobpack.deploy` grant, no host deploy opt-in) names no host
/// path: the draft store is the host's own, and a path skill is read only
/// when the server's skills catalog lists that file. Host execution itself
/// (`execute`, `rkat_validate`) is decided per method below.
fn handle_flow_editor_rpc_with_auth(
    request: JsonRpcRequest,
    auth: FlowEditorAuthReport,
    runtime_catalog: Option<&MobpackRuntimeCatalogState>,
) -> Value {
    if auth.host_access {
        return crate::mobpack::with_host_path_skill_reads(|| {
            dispatch_flow_editor_rpc(request, auth, runtime_catalog)
        });
    }
    if let Some(refusal) = caller_store_path_refusal(&request) {
        return refusal;
    }
    crate::mobpack::with_untrusted_path_skill_reads(runtime_catalog, || {
        dispatch_flow_editor_rpc(request, auth, runtime_catalog)
    })
}

/// `store_path` chooses the draft store file on the host. Without host
/// access a caller cannot: it could create directories and files at any
/// writable path, replace JSON files there, and read any JSON object file
/// back through `meta/mobpacks/list`.
fn caller_store_path_refusal(request: &JsonRpcRequest) -> Option<Value> {
    request
        .params
        .get("store_path")
        .is_some_and(|path| !path.is_null())
        .then(|| {
            response_value(
                request.id.clone().unwrap_or(Value::Null),
                None,
                Some(JsonRpcError {
                    code: -32602,
                    message: "store_path names a host file, which this Flow Editor accepts only \
                              with the mobpack.deploy grant or the host deploy opt-in; omit it to \
                              use the host's draft store"
                        .to_string(),
                    data: Some(serde_json::json!({ "param": "store_path" })),
                }),
            )
        })
}

/// Every method the Flow Editor RPC serves, as `meta/capabilities`
/// advertises them. Dispatch refuses any other method before matching, so no
/// match arm serves a method that the advertisement leaves out.
fn flow_editor_methods() -> Vec<&'static str> {
    let mut methods = vec!["meta/capabilities"];
    methods.extend_from_slice(crate::rpc::MOBPACK_AUTHORING_METHODS);
    methods
}

fn dispatch_flow_editor_rpc(
    request: JsonRpcRequest,
    auth: FlowEditorAuthReport,
    runtime_catalog: Option<&MobpackRuntimeCatalogState>,
) -> Value {
    let response_id = request.id.clone().unwrap_or(Value::Null);
    let methods = flow_editor_methods();
    if !methods.contains(&request.method.as_str()) {
        return response_value(
            response_id,
            None,
            Some(JsonRpcError {
                code: -32601,
                message: format!("method not found on flow editor rpc: {}", request.method),
                data: Some(serde_json::json!({
                    "kind": "method_not_advertised",
                    "method": request.method,
                })),
            }),
        );
    }
    match request.method.as_str() {
        "meta/capabilities" => {
            let mut authoring_capabilities = crate::rpc::mobpack_authoring_capabilities();
            authoring_capabilities["host_mutation_allowed"] =
                serde_json::json!(auth.host_mutation_allowed);
            authoring_capabilities["deploy_execute_allowed"] =
                serde_json::json!(auth.deploy_execute_allowed);
            authoring_capabilities["runtime_backed_catalogs"] =
                serde_json::json!(runtime_catalog.is_some());
            response_value(
                response_id,
                Some(serde_json::json!({
                    "methods": methods,
                    "authenticated": auth.authenticated,
                    "auth": {
                        "mode": auth.mode,
                        "reason": auth.reason
                    },
                    "features": {
                        "flow_editor": true,
                        "mobpack_authoring": true,
                    },
                    "authoring_capabilities": authoring_capabilities,
                })),
                None,
            )
        }
        "meta/mobpacks/catalogs" => response_value(
            response_id,
            Some(crate::mobpack::mobpack_catalogs_response_with_runtime(
                runtime_catalog,
            )),
            None,
        ),
        "meta/tools/catalog" => response_value(
            response_id,
            Some(crate::mobpack::mobpack_tools_catalog_response_with_runtime(
                runtime_catalog,
            )),
            None,
        ),
        "meta/skills/catalog" => response_value(
            response_id,
            Some(crate::mobpack::mobpack_skills_catalog_response_with_runtime(runtime_catalog)),
            None,
        ),
        "meta/agent_definitions/list" => response_value(
            response_id,
            Some(crate::mobpack::mobpack_agent_definitions_response_with_runtime(runtime_catalog)),
            None,
        ),
        "meta/mobpacks/templates" => response_value(
            response_id,
            Some(crate::mobpack::mobpack_templates_response_with_runtime(
                runtime_catalog,
            )),
            None,
        ),
        "meta/mobpacks/deploy"
            if !auth.host_access && deploy_execute_requested(&request.params) =>
        {
            response_value(
                response_id,
                None,
                Some(JsonRpcError {
                    code: -32602,
                    message: host_deploy_refusal(&auth).to_string(),
                    data: Some(serde_json::json!({
                        "method": "meta/mobpacks/deploy",
                        "execute": true,
                        "deploy_command": "rkat mob run"
                    })),
                }),
            )
        }
        // `validate` with `rkat_validate: true` is host-mutating (it writes a
        // rendered mobpack archive to a caller-controlled path and spawns
        // `rkat mob validate`). Without the host grant, strip the flag so the
        // request degrades to pure structural validation instead of touching
        // the host filesystem or spawning a process. Fail-closed: the default
        // standalone surface (no `--allow-host-deploy`) and any caller lacking
        // `mobpack.deploy` can never reach the file-write/exec sink.
        "meta/mobpacks/validate"
            if !auth.host_access && validate_rkat_execution_requested(&request.params) =>
        {
            let mut params = request.params.clone();
            if let Some(object) = params.as_object_mut() {
                object.remove("rkat_validate");
                object.remove("rkatValidate");
            }
            match crate::rpc::handle_mobpack_authoring_rpc_with_runtime(
                "meta/mobpacks/validate",
                &params,
                response_id.clone(),
                runtime_catalog,
            ) {
                Some(response) => serde_json::to_value(response).unwrap_or_else(|_| {
                    serde_json::json!({
                        "jsonrpc": JSONRPC_VERSION,
                        "id": Value::Null,
                        "error": { "code": -32603, "message": "serialization failed" }
                    })
                }),
                None => response_value(
                    response_id,
                    None,
                    Some(JsonRpcError {
                        code: -32601,
                        message: "method not found on flow editor rpc: meta/mobpacks/validate"
                            .to_string(),
                        data: None,
                    }),
                ),
            }
        }
        method if crate::rpc::MOBPACK_AUTHORING_METHODS.contains(&method) => {
            match crate::rpc::handle_mobpack_authoring_rpc_with_runtime(
                method,
                &request.params,
                response_id.clone(),
                runtime_catalog,
            ) {
                Some(response) => serde_json::to_value(response).unwrap_or_else(|_| {
                    serde_json::json!({
                        "jsonrpc": JSONRPC_VERSION,
                        "id": Value::Null,
                        "error": {
                            "code": -32603,
                            "message": "serialization failed",
                        }
                    })
                }),
                None => response_value(
                    response_id,
                    None,
                    Some(JsonRpcError {
                        code: -32601,
                        message: format!("method not found on flow editor rpc: {method}"),
                        data: None,
                    }),
                ),
            }
        }
        other => response_value(
            response_id,
            None,
            Some(JsonRpcError {
                code: -32601,
                message: format!("method not found on flow editor rpc: {other}"),
                data: None,
            }),
        ),
    }
}

/// Why a deploy with `execute: true` was refused, for the surface that
/// refused it.
fn host_deploy_refusal(auth: &FlowEditorAuthReport) -> &'static str {
    if auth.mode == REFERENCE_APP_AUTH_MODE {
        "this Flow Editor cannot execute host deploys without a mobpack.deploy access grant or \
         the host's --allow-flow-editor-host-deploy opt-in; use deploy planning or run rkat mob \
         run manually"
    } else {
        "standalone Flow Editor RPC cannot execute host deploys; use deploy planning or run rkat \
         mob run manually"
    }
}

fn deploy_execute_requested(params: &Value) -> bool {
    params
        .get("execute")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn response_value(id: Value, result: Option<Value>, error: Option<JsonRpcError>) -> Value {
    serde_json::to_value(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result,
        error,
    })
    .unwrap_or_else(|_| {
        serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": Value::Null,
            "error": {
                "code": -32603,
                "message": "serialization failed",
            }
        })
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use axum::{body::Body, http::Request};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::rpc::JsonRpcRequest;

    #[tokio::test]
    async fn flow_editor_document_may_only_be_framed_by_its_own_origin() {
        use tower::ServiceExt;
        for path in ["/flow-editor", "/flow-editor/"] {
            let response = super::flow_editor_frontend_router::<()>()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(path)
                        .body(axum::body::Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            assert_eq!(
                response
                    .headers()
                    .get(axum::http::header::CONTENT_SECURITY_POLICY)
                    .and_then(|value| value.to_str().ok()),
                Some("frame-ancestors 'self'"),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn standalone_flow_editor_serves_empty_favicon_response() {
        let response = super::flow_editor_router()
            .oneshot(
                Request::builder()
                    .uri("/favicon.ico")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("favicon response");

        assert_eq!(response.status(), axum::http::StatusCode::NO_CONTENT);
    }

    fn authoring_access_controller(rules: Vec<crate::access::AccessRule>) -> AccessController {
        AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            groups: std::collections::BTreeMap::new(),
            rules,
            approvers: std::collections::BTreeMap::new(),
        })
        .expect("valid access config")
    }

    fn allow_everyone(id: &str, actions: &[&str]) -> crate::access::AccessRule {
        crate::access::AccessRule {
            id: id.to_string(),
            actions: actions.iter().map(ToString::to_string).collect(),
            ..crate::access::AccessRule::default()
        }
    }

    fn open_decision_state() -> crate::runtime::RuntimeDecisionState {
        crate::runtime::RuntimeDecisionState {
            bigquery: crate::decisions::BigQueryNaming {
                dataset: "flow_editor_dataset".to_string(),
                table: "flow_editor_table".to_string(),
            },
            modules: vec![],
            auth: crate::decisions::AuthPolicy::default(),
            trusted_oidc: crate::runtime::TrustedOidcRuntimeConfig {
                discovery_json: r#"{"issuer":"https://noop.example.com"}"#.to_string(),
                jwks_json: r#"{"keys":[]}"#.to_string(),
                audience: "flow-editor-tests".to_string(),
                require_verified_email: false,
            },
            console: crate::decisions::ConsolePolicy {
                require_app_auth: false,
                ..crate::decisions::ConsolePolicy::default()
            },
            ops: crate::decisions::RuntimeOpsPolicy::default(),
            release_metadata: crate::decisions::ReleaseMetadata {
                targets: vec!["crates.io".to_string()],
                support_matrix: "lts".to_string(),
            },
        }
    }

    async fn protected_rpc_response(access: Option<AccessController>, body: Value) -> Value {
        protected_rpc_response_with_host_deploy(access, false, body).await
    }

    async fn protected_rpc_response_with_host_deploy(
        access: Option<AccessController>,
        host_deploy_without_access_control: bool,
        body: Value,
    ) -> Value {
        protected_rpc_response_of_launch(access, host_deploy_without_access_control, None, body)
            .await
    }

    /// [`protected_rpc_response_with_host_deploy`] on a launch that speaks
    /// for its composition or, with `candidate` set, on a candidate launch.
    async fn protected_rpc_response_of_launch(
        access: Option<AccessController>,
        host_deploy_without_access_control: bool,
        candidate: Option<super::FlowEditorCandidate>,
        body: Value,
    ) -> Value {
        let router = super::flow_editor_rpc_router_with_runtime_catalog(
            open_decision_state(),
            None,
            access,
            host_deploy_without_access_control,
            crate::auth::session_fence::ConsoleSessionFences::detached(),
            candidate,
            false,
        );
        rpc_response_of(router, body).await
    }

    /// [`protected_rpc_response_of_launch`] on a launch that opted into host
    /// deploys and whose agents keep their provider credentials in a store
    /// the host named.
    async fn protected_rpc_response_with_named_credentials(body: Value) -> Value {
        let router = super::flow_editor_rpc_router_with_runtime_catalog(
            open_decision_state(),
            None,
            None,
            true,
            crate::auth::session_fence::ConsoleSessionFences::detached(),
            None,
            true,
        );
        rpc_response_of(router, body).await
    }

    async fn rpc_response_of(router: axum::Router, body: Value) -> Value {
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/flow-editor/rpc")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("rpc response");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    fn rpc_body(method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
    }

    #[tokio::test]
    async fn access_control_denies_authoring_without_a_mobpack_author_grant() {
        let access = authoring_access_controller(Vec::new());
        let response =
            protected_rpc_response(Some(access), rpc_body("meta/mobpacks/schema", json!({}))).await;
        assert_eq!(response["error"]["code"], json!(ACCESS_DENIED_RPC_CODE));
        assert_eq!(response["error"]["data"]["action"], json!("mobpack.author"));
    }

    #[tokio::test]
    async fn access_control_allows_authoring_with_a_mobpack_author_grant() {
        let access =
            authoring_access_controller(vec![allow_everyone("authors", &["mobpack.author"])]);
        let response =
            protected_rpc_response(Some(access), rpc_body("meta/mobpacks/schema", json!({}))).await;
        assert!(response["error"].is_null(), "{response:#?}");
        assert!(response["result"].is_object(), "{response:#?}");
    }

    #[tokio::test]
    async fn access_control_requires_mobpack_deploy_for_deploy_execute() {
        let access =
            authoring_access_controller(vec![allow_everyone("authors", &["mobpack.author"])]);
        let response = protected_rpc_response(
            Some(access),
            rpc_body("meta/mobpacks/deploy", json!({ "execute": true })),
        )
        .await;
        assert_eq!(response["error"]["code"], json!(ACCESS_DENIED_RPC_CODE));
        assert_eq!(response["error"]["data"]["action"], json!("mobpack.deploy"));
    }

    #[tokio::test]
    async fn access_control_capabilities_intersect_deploy_grants() {
        let access =
            authoring_access_controller(vec![allow_everyone("authors", &["mobpack.author"])]);
        let response =
            protected_rpc_response(Some(access), rpc_body("meta/capabilities", Value::Null)).await;
        let capabilities = &response["result"]["authoring_capabilities"];
        assert_eq!(capabilities["deploy_execute_allowed"], json!(false));
        assert_eq!(capabilities["host_mutation_allowed"], json!(false));

        let access = authoring_access_controller(vec![allow_everyone(
            "operators",
            &["mobpack.author", "mobpack.deploy"],
        )]);
        let response =
            protected_rpc_response(Some(access), rpc_body("meta/capabilities", Value::Null)).await;
        let capabilities = &response["result"]["authoring_capabilities"];
        assert_eq!(capabilities["deploy_execute_allowed"], json!(true));
        assert_eq!(capabilities["host_mutation_allowed"], json!(true));
    }

    #[tokio::test]
    async fn access_control_disabled_leaves_authoring_open() {
        let response =
            protected_rpc_response(None, rpc_body("meta/mobpacks/schema", json!({}))).await;
        assert!(response["error"].is_null(), "{response:#?}");
    }

    use super::AccessController;
    use crate::http_console::ACCESS_DENIED_RPC_CODE;

    #[test]
    fn flow_editor_frontend_router_merges_with_console_frontend_router() {
        // `build_reference_app_router` merges both frontends into one app
        // router; duplicate paths (such as /favicon.ico) make axum panic at
        // startup, so the shared frontend router must stay collision-free.
        let _ = crate::http_console::console_frontend_router()
            .merge(super::flow_editor_frontend_router::<()>());
    }

    #[test]
    fn flow_editor_rpc_exposes_only_mobpack_authoring_methods() {
        let response = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "meta/capabilities".to_string(),
            params: Value::Null,
        });
        let methods = response["result"]["methods"]
            .as_array()
            .expect("methods array")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let mut expected_methods = vec!["meta/capabilities"];
        expected_methods.extend_from_slice(crate::rpc::MOBPACK_AUTHORING_METHODS);
        assert_eq!(methods, expected_methods);
        assert!(!methods.contains(&"meta/console/send"));
        assert_eq!(response["result"]["authenticated"], json!(false));
        assert_eq!(response["result"]["auth"]["mode"], json!("none"));
        assert_eq!(
            response["result"]["authoring_capabilities"]["domain"],
            json!("mobpack_authoring")
        );
        assert_eq!(
            response["result"]["authoring_capabilities"]["runtime_mutation"],
            json!(false)
        );
        assert_eq!(
            response["result"]["authoring_capabilities"]["host_mutation_allowed"],
            json!(false)
        );
        assert_eq!(
            response["result"]["authoring_capabilities"]["deploy_execute_allowed"],
            json!(false)
        );
        assert_eq!(
            response["result"]["authoring_capabilities"]["deploy_command"],
            json!("rkat mob run")
        );
        assert_eq!(
            response["result"]["authoring_capabilities"]["methods"]
                .as_array()
                .expect("authoring methods")
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
            crate::rpc::MOBPACK_AUTHORING_METHODS
        );
    }

    #[test]
    fn standalone_flow_editor_rpc_rejects_host_deploy_execution() {
        let catalogs = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "meta/mobpacks/catalogs".to_string(),
            params: Value::Null,
        });
        assert_eq!(catalogs["result"]["runtime_backed"], json!(false));
        assert_eq!(
            catalogs["result"]["authoring_provider"]["runtime_binding"],
            json!("unbound")
        );
        let sample = catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample");

        let response = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "meta/mobpacks/deploy".to_string(),
            params: json!({
                "document": sample["document"].clone(),
                "prompt": "Reply with exactly OK.",
                "execute": true
            }),
        });

        assert_eq!(response["error"]["code"], json!(-32602));
        assert!(
            response["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("cannot execute host deploys")),
            "{response:#?}"
        );
        assert_eq!(
            response["error"]["data"]["deploy_command"],
            json!("rkat mob run")
        );
    }

    #[test]
    fn protected_flow_editor_rpc_returns_runtime_bound_catalogs_when_state_is_available() {
        let response = super::handle_flow_editor_rpc_with_auth(
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(json!(1)),
                method: "meta/mobpacks/catalogs".to_string(),
                params: Value::Null,
            },
            super::FlowEditorAuthReport {
                authenticated: true,
                mode: "reference_app",
                reason: "reference app Flow Editor authoring server",
                host_access: true,
                host_mutation_allowed: true,
                deploy_execute_allowed: true,
            },
            Some(&crate::mobpack::MobpackRuntimeCatalogState {
                loaded_modules: vec!["worker".to_string()],
                runtime_methods: vec!["meta/mobpacks/deploy".to_string()],
                has_contact_directory: true,
                has_peer_mob_handles: false,
                has_inproc_contacts: false,
                runtime_flow_rows: Vec::new(),
                runtime_agent_definition_sources: Vec::new(),
                runtime_skill_realms: Vec::new(),
            }),
        );

        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(response["result"]["runtime_backed"], json!(true));
        assert_eq!(
            response["result"]["authoring_provider"]["id"],
            json!("unified_runtime")
        );
        assert_eq!(
            response["result"]["authoring_provider"]["runtime_binding"],
            json!("bound")
        );
        assert_eq!(
            response["result"]["authoring_provider"]["loaded_modules"],
            json!(["worker"])
        );
        assert_eq!(
            response["result"]["authoring_provider"]["cross_mob"]["contact_directory"],
            json!(true)
        );
        assert_eq!(
            response["result"]["catalog_snapshot"]["runtime_backed"],
            json!(true)
        );
    }

    #[cfg(unix)]
    #[test]
    fn standalone_flow_editor_rpc_executes_host_deploy_when_explicitly_enabled() {
        let catalogs = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "meta/mobpacks/catalogs".to_string(),
            params: Value::Null,
        });
        let sample = catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample");
        let dir = tempfile::tempdir().expect("tempdir");
        let fake_rkat = dir.path().join("rkat");
        let args_file = dir.path().join("rkat.args");
        std::fs::write(
            &fake_rkat,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$@\" > {}\n\
                 echo flow-editor-rkat-ok\n\
                 printf 'run\\tmob=docs\\tflow=main\\trun_id=run-1\\tstatus=completed\\n'\n\
                 printf 'result\\t{{\"reply\":\"OK\"}}\\n'\n",
                args_file.to_string_lossy()
            ),
        )
        .expect("write fake rkat");
        let mut permissions = std::fs::metadata(&fake_rkat)
            .expect("fake rkat metadata")
            .permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_rkat, permissions).expect("chmod fake rkat");

        let capabilities = super::handle_flow_editor_rpc_with_auth(
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(json!(2)),
                method: "meta/capabilities".to_string(),
                params: Value::Null,
            },
            super::FlowEditorAuthReport {
                authenticated: false,
                mode: "standalone_host_deploy",
                reason: "standalone Flow Editor authoring server with explicit host deploy opt-in",
                host_access: true,
                host_mutation_allowed: true,
                deploy_execute_allowed: true,
            },
            None,
        );
        assert_eq!(
            capabilities["result"]["authoring_capabilities"]["host_mutation_allowed"],
            json!(true)
        );
        assert_eq!(
            capabilities["result"]["authoring_capabilities"]["deploy_execute_allowed"],
            json!(true)
        );

        let response = super::handle_flow_editor_rpc_with_auth(
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(json!(3)),
                method: "meta/mobpacks/deploy".to_string(),
                params: json!({
                    "document": sample["document"].clone(),
                    "output_dir": dir.path(),
                    "prompt": "Reply with exactly OK.",
                    "rkat_bin": fake_rkat,
                    "execute": true
                }),
            },
            super::FlowEditorAuthReport {
                authenticated: false,
                mode: "standalone_host_deploy",
                reason: "standalone Flow Editor authoring server with explicit host deploy opt-in",
                host_access: true,
                host_mutation_allowed: true,
                deploy_execute_allowed: true,
            },
            None,
        );

        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(response["result"]["executed"], json!(true));
        assert_eq!(response["result"]["success"], json!(true));
        assert_eq!(response["result"]["status_code"], json!(0));
        assert!(
            response["result"]["stdout"]
                .as_str()
                .is_some_and(|stdout| stdout.contains("flow-editor-rkat-ok")),
            "{response:#?}"
        );
        let argv = std::fs::read_to_string(args_file).expect("recorded fake rkat args");
        assert!(argv.lines().any(|line| line == "mob"));
        assert!(argv.lines().any(|line| line == "run"));
        // Single-token hyphen-safe `--prompt=<text>` form.
        assert!(
            argv.lines()
                .any(|line| line == "--prompt=Reply with exactly OK.")
        );
    }

    /// Security regression: `meta/mobpacks/validate` with
    /// `rkat_validate: true` writes a rendered mobpack archive to a
    /// caller-controlled path and spawns `rkat mob validate`. On the default
    /// standalone surface (no `--allow-host-deploy`, `host_mutation_allowed`
    /// false) an unauthenticated client must NOT reach that file-write/exec
    /// sink: the request degrades to structural-only validation, leaving no
    /// archive on disk and spawning no process.
    #[cfg(unix)]
    #[test]
    fn standalone_flow_editor_validate_cannot_write_files_or_spawn_rkat_without_host_grant() {
        let catalogs = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "meta/mobpacks/catalogs".to_string(),
            params: Value::Null,
        });
        let sample = catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample");

        let dir = tempfile::tempdir().expect("tempdir");
        // A `rkat` that records the fact it ran — it must never be invoked.
        let fake_rkat = dir.path().join("rkat");
        let spawn_marker = dir.path().join("rkat.ran");
        std::fs::write(
            &fake_rkat,
            format!(
                "#!/bin/sh\ntouch {}\necho should-not-run\n",
                spawn_marker.to_string_lossy()
            ),
        )
        .expect("write fake rkat");
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&fake_rkat)
            .expect("fake rkat metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_rkat, permissions).expect("chmod fake rkat");

        // Attacker-controlled absolute pack path: it must never be written.
        let attacker_pack_path = dir.path().join("attacker-controlled.mobpack");

        let response = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "meta/mobpacks/validate".to_string(),
            params: json!({
                "document": sample["document"].clone(),
                "rkat_validate": true,
                "rkat_bin": fake_rkat,
                "validation_pack_path": attacker_pack_path,
            }),
        });

        // Structural validation still runs and succeeds …
        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(response["result"]["ok"], json!(true), "{response:#?}");
        // … but it did NOT escalate to the host-executing rkat path.
        assert_ne!(
            response["result"]["validation_source"],
            json!("rkat mob validate"),
            "rkat validate must not run without the host grant: {response:#?}"
        );
        assert!(
            !attacker_pack_path.exists(),
            "validate must not write a caller-controlled archive without the host grant"
        );
        assert!(
            !spawn_marker.exists(),
            "validate must not spawn rkat without the host grant"
        );
    }

    /// The ABAC access requirement classifies validate-with-rkat as a
    /// host-mutating operation: it requires `mobpack.deploy`, not just
    /// `mobpack.author`. A pure structural validate stays an authoring read.
    #[tokio::test]
    async fn access_control_requires_mobpack_deploy_for_validate_with_rkat() {
        let access =
            authoring_access_controller(vec![allow_everyone("authors", &["mobpack.author"])]);
        let response = protected_rpc_response(
            Some(access),
            rpc_body(
                "meta/mobpacks/validate",
                json!({ "document": json!({}), "rkat_validate": true }),
            ),
        )
        .await;
        assert_eq!(response["error"]["code"], json!(ACCESS_DENIED_RPC_CODE));
        assert_eq!(response["error"]["data"]["action"], json!("mobpack.deploy"));

        // A structural-only validate is authoring, not deploy: the author
        // grant is sufficient (no access-denied).
        let access =
            authoring_access_controller(vec![allow_everyone("authors", &["mobpack.author"])]);
        let response = protected_rpc_response(
            Some(access),
            rpc_body("meta/mobpacks/validate", json!({ "document": json!({}) })),
        )
        .await;
        assert_ne!(
            response["error"]["code"],
            json!(ACCESS_DENIED_RPC_CODE),
            "structural validate must remain an authoring read: {response:#?}"
        );
    }

    #[test]
    fn flow_editor_rpc_plans_real_sample_mobpack_deploy() {
        let catalogs = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "meta/mobpacks/catalogs".to_string(),
            params: Value::Null,
        });
        let sample = catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample");

        let response = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "meta/mobpacks/deploy".to_string(),
            params: json!({
                "document": sample["document"].clone(),
                "prompt": "Reply with exactly OK."
            }),
        });

        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(
            &response["result"]["argv"].as_array().expect("argv")[0..3],
            [json!("rkat"), json!("mob"), json!("run")]
        );
        assert!(
            response["result"]["plan_trace"]
                .as_array()
                .expect("plan trace")
                .iter()
                .any(|row| row["head"]
                    .as_str()
                    .is_some_and(|head| head.starts_with("PROFILE ·"))),
            "{response:#?}"
        );
    }

    #[test]
    fn flow_editor_rpc_previews_document_backed_deploy_command() {
        let catalogs = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "meta/mobpacks/catalogs".to_string(),
            params: Value::Null,
        });
        let sample = catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample");
        let response = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "meta/mobpacks/deploy_command".to_string(),
            params: json!({
                "document": sample["document"].clone(),
                "prompt": "Preview prompt."
            }),
        });

        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(
            &response["result"]["argv"].as_array().expect("argv")[0..3],
            [json!("rkat"), json!("mob"), json!("run")]
        );
        assert_eq!(
            response["result"]["source"],
            json!("meerkat_meta::mobpack::deploy_argv")
        );
        assert_eq!(response["result"]["filename"], json!("docs-only.mobpack"));
        assert_eq!(response["result"]["validation"]["ok"], json!(true));
        assert!(
            response["result"]["command"]
                .as_str()
                .is_some_and(|command| command.contains("docs-only.mobpack")
                    && command.contains("Preview prompt.")),
            "{response:#?}"
        );

        let rejected = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "meta/mobpacks/deploy_command".to_string(),
            params: json!({
                "deploy": { "command": "rkat mob run" },
                "pack_path": "<pack.mobpack>"
            }),
        });
        assert!(
            rejected["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("requires document")),
            "{rejected:#?}"
        );
    }

    #[test]
    fn flow_editor_rpc_previews_source_without_exporting_archive_payload() {
        let catalogs = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "meta/mobpacks/catalogs".to_string(),
            params: Value::Null,
        });
        let sample = catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample");

        let response = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "meta/mobpacks/source".to_string(),
            params: json!({ "document": sample["document"].clone() }),
        });

        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(response["result"]["source"], json!("meta/mobpacks/source"));
        assert!(
            response["result"].get("content_base64").is_none(),
            "{response:#?}"
        );
        let source_files = response["result"]["source_files"]
            .as_array()
            .expect("source files");
        assert!(
            source_files
                .iter()
                .any(|file| file["path"] == "meta/mob.toml"
                    && file["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("[mob]"))),
            "{response:#?}"
        );
    }

    /// A runtime with no access controller, as both bundled gateways build
    /// one by default. Each test names its own mob: lib tests share one
    /// in-process comms registry, which refuses a second live supervisor
    /// route for the same mob.
    async fn runtime_without_access_control(
        dir: &std::path::Path,
        mob_id: &str,
    ) -> crate::UnifiedRuntime {
        let session_path = dir.join("sessions");
        std::fs::create_dir_all(&session_path).expect("session path");
        let session_service: std::sync::Arc<dyn meerkat_mob::MobSessionService> =
            std::sync::Arc::new(meerkat::build_ephemeral_service(
                meerkat::AgentFactory::new(&session_path),
                meerkat::Config::default(),
                16,
            ));
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n"
        ))
        .expect("mob definition");
        let mob_spec = crate::MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            session_service,
        )
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(std::sync::Arc::new(meerkat_client::TestClient::default())),
        });
        let module_config = crate::MetaConfig {
            modules: vec![],
            discovery: crate::types::DiscoverySpec {
                namespace: mob_id.to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        };
        crate::UnifiedRuntime::bootstrap(mob_spec, module_config, std::time::Duration::from_secs(5))
            .await
            .expect("bootstrap runtime")
    }

    /// POST one anonymous JSON-RPC request to `/flow-editor/rpc`, the way the
    /// gateway listener receives it from any local process.
    async fn anonymous_flow_editor_rpc(app: &axum::Router, method: &str, params: Value) -> Value {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/flow-editor/rpc")
                    .header("content-type", "application/json")
                    .body(Body::from(rpc_body(method, params).to_string()))
                    .expect("request"),
            )
            .await
            .expect("flow editor rpc response");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    /// An `rkat` that appends one line to `marker` for every run and answers
    /// like a completed `rkat mob run`.
    #[cfg(unix)]
    fn recording_rkat(dir: &std::path::Path, marker: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let rkat = dir.join("rkat");
        std::fs::write(
            &rkat,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$*\" >> '{}'\n\
                 printf 'run\\tmob=docs\\tflow=main\\trun_id=run-1\\tstatus=completed\\n'\n\
                 printf 'result\\t{{\"reply\":\"OK\"}}\\n'\n",
                marker.to_string_lossy()
            ),
        )
        .expect("write recording rkat");
        let mut permissions = std::fs::metadata(&rkat)
            .expect("recording rkat metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&rkat, permissions).expect("chmod recording rkat");
        rkat
    }

    /// Security regression: the Flow Editor a runtime mounts on its reference
    /// app router is served by both bundled gateways, with an open console on
    /// loopback. Loopback is not same-user isolation, so a caller with no
    /// `mobpack.deploy` grant must not run `rkat` on the host through it. With
    /// no access controller and no host opt-in nobody holds that grant:
    /// anonymous `meta/mobpacks/deploy {execute: true}` and
    /// `meta/mobpacks/validate {rkat_validate: true}` stay plans and
    /// structural checks, write no archive, and the capability advertisement
    /// says so.
    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_flow_editor_deploy_execute_denied_without_host_grant_or_access_controller() {
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime =
            runtime_without_access_control(dir.path(), "flow-editor-deploy-default-deny").await;
        let app = runtime.build_reference_app_router(open_decision_state());
        let marker = dir.path().join("rkat.runs");
        let rkat = recording_rkat(dir.path(), &marker);
        let deploy_dir = dir.path().join("deploy");
        let validation_pack = dir.path().join("validate.mobpack");

        let catalogs = anonymous_flow_editor_rpc(&app, "meta/mobpacks/catalogs", Value::Null).await;
        let document = catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample")["document"]
            .clone();
        let capabilities = anonymous_flow_editor_rpc(&app, "meta/capabilities", Value::Null).await;
        let deploy = anonymous_flow_editor_rpc(
            &app,
            "meta/mobpacks/deploy",
            json!({
                "document": document,
                "prompt": "Reply with exactly OK.",
                "execute": true,
                "output_dir": deploy_dir,
                "rkat_bin": rkat,
            }),
        )
        .await;
        let validate = anonymous_flow_editor_rpc(
            &app,
            "meta/mobpacks/validate",
            json!({
                "document": document,
                "rkat_validate": true,
                "rkat_bin": rkat,
                "validation_pack_path": validation_pack,
            }),
        )
        .await;
        let _shutdown = runtime.shutdown().await;

        assert_eq!(
            std::fs::read_to_string(&marker).ok(),
            None,
            "rkat ran on the host without a deploy grant: deploy {deploy:#?} validate {validate:#?}"
        );
        assert!(
            !deploy_dir.exists(),
            "deploy wrote an archive without a deploy grant: {deploy:#?}"
        );
        assert!(
            !validation_pack.exists(),
            "validate wrote an archive without a deploy grant: {validate:#?}"
        );
        let advertised = &capabilities["result"]["authoring_capabilities"];
        assert_eq!(
            (
                advertised["deploy_execute_allowed"].clone(),
                advertised["host_mutation_allowed"].clone(),
            ),
            (json!(false), json!(false)),
            "{capabilities:#?}"
        );
        assert_eq!(deploy["error"]["code"], json!(-32602), "{deploy:#?}");
        assert_eq!(
            deploy["error"]["data"],
            json!({
                "method": "meta/mobpacks/deploy",
                "execute": true,
                "deploy_command": "rkat mob run"
            }),
            "{deploy:#?}"
        );
        assert_eq!(validate["result"]["ok"], json!(true), "{validate:#?}");
        assert_ne!(
            validate["result"]["validation_source"],
            json!("rkat mob validate"),
            "{validate:#?}"
        );
    }

    /// The host's explicit opt-in (`--allow-flow-editor-host-deploy` on the
    /// gateways) restores host deploys on a runtime without access control.
    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_flow_editor_deploy_execute_runs_with_the_host_deploy_opt_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut runtime =
            runtime_without_access_control(dir.path(), "flow-editor-deploy-host-opt-in").await;
        runtime.allow_flow_editor_host_deploy(true);
        let app = runtime.build_reference_app_router(open_decision_state());
        let marker = dir.path().join("rkat.runs");
        let rkat = recording_rkat(dir.path(), &marker);
        let deploy_dir = dir.path().join("deploy");

        let catalogs = anonymous_flow_editor_rpc(&app, "meta/mobpacks/catalogs", Value::Null).await;
        let document = catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample")["document"]
            .clone();
        let capabilities = anonymous_flow_editor_rpc(&app, "meta/capabilities", Value::Null).await;
        let deploy = anonymous_flow_editor_rpc(
            &app,
            "meta/mobpacks/deploy",
            json!({
                "document": document,
                "prompt": "Reply with exactly OK.",
                "execute": true,
                "output_dir": deploy_dir,
                "rkat_bin": rkat,
            }),
        )
        .await;
        let _shutdown = runtime.shutdown().await;

        let advertised = &capabilities["result"]["authoring_capabilities"];
        assert_eq!(
            (
                advertised["deploy_execute_allowed"].clone(),
                advertised["host_mutation_allowed"].clone(),
            ),
            (json!(true), json!(true)),
            "{capabilities:#?}"
        );
        assert_eq!(
            (
                deploy["result"]["executed"].clone(),
                deploy["result"]["success"].clone()
            ),
            (json!(true), json!(true)),
            "{deploy:#?}"
        );
        let runs = std::fs::read_to_string(&marker).expect("rkat ran");
        assert_eq!(runs.lines().count(), 1, "{runs}");
        assert!(runs.starts_with("mob run "), "{runs}");
    }

    /// A candidate launch's Flow Editor runs nothing on the host, whatever
    /// the host opted into, and writes no draft store the authoritative
    /// launch reads: a deploy with `execute`, a validation through `rkat`, a
    /// draft save to the default store and one to a file in the state
    /// directory are refused with `-32047` `candidate_launch`. Authoring and
    /// a draft save to a file the caller names outside the state directory
    /// still answer.
    #[tokio::test]
    async fn a_candidate_launch_flow_editor_refuses_host_deploys() {
        let state_dir = tempfile::tempdir().expect("state dir");
        let outside = tempfile::tempdir().expect("outside dir");
        let candidate = || {
            Some(super::FlowEditorCandidate {
                state_dir: Some(state_dir.path().to_path_buf()),
            })
        };
        let mut refused = Vec::new();
        for (method, params) in [
            ("meta/mobpacks/deploy", json!({ "execute": true })),
            ("meta/mobpacks/validate", json!({ "rkat_validate": true })),
            ("meta/mobpacks/save", json!({ "id": "draft-1" })),
            (
                "meta/mobpacks/save",
                json!({ "id": "draft-1", "store_path": state_dir.path().join("drafts.json") }),
            ),
        ] {
            let response =
                protected_rpc_response_of_launch(None, true, candidate(), rpc_body(method, params))
                    .await;
            refused.push((
                response["error"]["code"].clone(),
                response["error"]["data"]["kind"].clone(),
            ));
        }
        let mut served = Vec::new();
        for (method, params) in [
            ("meta/mobpacks/schema", json!({})),
            (
                "meta/mobpacks/save",
                json!({ "id": "draft-1", "store_path": outside.path().join("drafts.json") }),
            ),
        ] {
            let response =
                protected_rpc_response_of_launch(None, true, candidate(), rpc_body(method, params))
                    .await;
            served.push(response["error"]["data"]["kind"] != json!("candidate_launch"));
        }
        assert_eq!(
            (refused, served),
            (
                vec![(json!(-32047), json!("candidate_launch")); 4],
                vec![true, true]
            )
        );
        let capabilities = protected_rpc_response_of_launch(
            None,
            true,
            candidate(),
            rpc_body("meta/capabilities", Value::Null),
        )
        .await;
        assert_eq!(
            capabilities["result"]["authoring_capabilities"]["deploy_execute_allowed"],
            json!(false),
            "a candidate launch does not advertise the deploy it refuses: {capabilities:#?}"
        );
    }

    /// A runtime whose agents keep their provider credentials in a store the
    /// host named refuses a deploy with `execute` on its Flow Editor, typed
    /// and whatever the host opted into: the `rkat` process it would run
    /// cannot carry that store. Its capabilities do not advertise that
    /// deploy, while the other host mutations stay advertised, and the
    /// deploy plan still answers.
    #[tokio::test]
    async fn a_named_credential_store_refuses_an_executing_flow_editor_deploy() {
        let capabilities = protected_rpc_response_with_named_credentials(rpc_body(
            "meta/capabilities",
            Value::Null,
        ))
        .await;
        let advertised = &capabilities["result"]["authoring_capabilities"];
        assert_eq!(
            (
                advertised["deploy_execute_allowed"].clone(),
                advertised["host_mutation_allowed"].clone()
            ),
            (json!(false), json!(true)),
            "{capabilities:#?}"
        );
        let refused = protected_rpc_response_with_named_credentials(rpc_body(
            "meta/mobpacks/deploy",
            json!({ "execute": true }),
        ))
        .await;
        assert_eq!(
            (
                refused["error"]["code"].clone(),
                refused["error"]["data"]["kind"].clone()
            ),
            (
                json!(crate::rpc::CAPABILITY_UNAVAILABLE_CODE),
                json!("provider_credentials_not_carried")
            ),
            "{refused:#?}"
        );
        let planned = protected_rpc_response_with_named_credentials(rpc_body(
            "meta/mobpacks/deploy",
            json!({ "execute": false }),
        ))
        .await;
        assert_ne!(
            planned["error"]["data"]["kind"],
            json!("provider_credentials_not_carried"),
            "{planned:#?}"
        );
    }

    /// Without an access controller and without the opt-in, the protected
    /// router refuses execute-deploys and names both ways to allow them.
    #[tokio::test]
    async fn without_access_control_or_opt_in_the_refusal_names_the_grant_and_the_opt_in() {
        let response = protected_rpc_response(
            None,
            rpc_body("meta/mobpacks/deploy", json!({ "execute": true })),
        )
        .await;
        assert_eq!(response["error"]["code"], json!(-32602), "{response:#?}");
        assert_eq!(
            response["error"]["message"],
            json!(
                "this Flow Editor cannot execute host deploys without a mobpack.deploy access \
                 grant or the host's --allow-flow-editor-host-deploy opt-in; use deploy \
                 planning or run rkat mob run manually"
            )
        );
    }

    /// An enforced access controller decides per caller, with or without the
    /// opt-in: a `mobpack.deploy` grant admits host deploys without it, and
    /// the opt-in never admits a caller the controller does not grant.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_enforced_access_controller_decides_host_deploys_with_or_without_the_opt_in() {
        let catalogs = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "meta/mobpacks/catalogs".to_string(),
            params: Value::Null,
        });
        let document = catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample")["document"]
            .clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("rkat.runs");
        let rkat = recording_rkat(dir.path(), &marker);
        let deploy = |output: &str| {
            rpc_body(
                "meta/mobpacks/deploy",
                json!({
                    "document": document,
                    "prompt": "Reply with exactly OK.",
                    "execute": true,
                    "output_dir": dir.path().join(output),
                    "rkat_bin": rkat,
                }),
            )
        };

        let operators = authoring_access_controller(vec![allow_everyone(
            "operators",
            &["mobpack.author", "mobpack.deploy"],
        )]);
        let granted =
            protected_rpc_response_with_host_deploy(Some(operators), false, deploy("granted"))
                .await;
        assert_eq!(granted["result"]["executed"], json!(true), "{granted:#?}");

        let authors =
            authoring_access_controller(vec![allow_everyone("authors", &["mobpack.author"])]);
        let ungranted =
            protected_rpc_response_with_host_deploy(Some(authors), true, deploy("ungranted")).await;
        assert_eq!(
            (
                ungranted["error"]["code"].clone(),
                ungranted["error"]["data"]["action"].clone()
            ),
            (json!(ACCESS_DENIED_RPC_CODE), json!("mobpack.deploy")),
            "{ungranted:#?}"
        );
        let runs = std::fs::read_to_string(&marker).expect("the granted deploy ran rkat");
        assert_eq!(runs.lines().count(), 1, "{runs}");
        assert!(!dir.path().join("ungranted").exists());
    }

    async fn sample_docs_only_document(app: &axum::Router) -> Value {
        let catalogs = anonymous_flow_editor_rpc(app, "meta/mobpacks/catalogs", Value::Null).await;
        catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample")["document"]
            .clone()
    }

    /// Request params that take `method` as far as it goes with `document`,
    /// plus `extra`: the host-side-effect knobs under test.
    fn method_params(method: &str, document: &Value, extra: &Value) -> Value {
        let mut params = json!({ "document": document, "prompt": "Reply with exactly OK." });
        if method == "meta/mobpacks/apply_operation" {
            params["operation"] = json!({
                "type": "update_deploy_settings",
                "field": "prompt",
                "value": "Validate the edited document."
            });
        }
        if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            params.extend(extra.clone());
        }
        params
    }

    /// `document` with a member selecting a path skill that names `path`, in a
    /// realm of its own beside the document's realms.
    fn document_with_path_skill(document: &Value, path: &std::path::Path) -> Value {
        let mut document = document.clone();
        document["mob_toml"] = Value::Null;
        document["members"][0]["skills"] = json!(["probe.skill"]);
        document["skill_realms"]
            .as_array_mut()
            .expect("sample skill realms")
            .push(json!({
                "id": "local/probe",
                "label": "Local probe",
                "source": "filesystem",
                "skills": [{
                    "id": "probe.skill",
                    "label": "Probe",
                    "source": "path",
                    "path": path
                }]
            }));
        document
    }

    /// Allows every mobpack action and denies `mobpack.deploy`: the deny
    /// decides, so a caller under it has no host access.
    fn deploy_denying_access_controller() -> AccessController {
        authoring_access_controller(vec![
            allow_everyone("authors", &["mobpack.*"]),
            crate::access::AccessRule {
                id: "no-host-deploy".to_string(),
                effect: crate::access::AccessEffect::Deny,
                actions: vec!["mobpack.deploy".to_string()],
                ..crate::access::AccessRule::default()
            },
        ])
    }

    /// Security regression: an enforced controller's deny of `mobpack.deploy`
    /// withholds host access although an allow rule also matches the action.
    /// Such a caller chooses no draft store file and reads no file that the
    /// skills catalog does not list.
    #[tokio::test]
    async fn a_mobpack_deploy_deny_withholds_host_paths_although_mobpack_star_is_allowed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let secret_json = dir.path().join("secret.json");
        std::fs::write(
            &secret_json,
            r#"{"credential": {"token": "DEPLOY-DENIED-SECRET-5d1e"}}"#,
        )
        .expect("write secret json");
        let secret_file = dir.path().join("id_secret");
        std::fs::write(&secret_file, "DEPLOY-DENIED-SECRET-5d1e").expect("write secret file");
        let catalogs = super::handle_flow_editor_rpc(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "meta/mobpacks/catalogs".to_string(),
            params: Value::Null,
        });
        let sample = catalogs["result"]["sample_mobpacks"]
            .as_array()
            .expect("sample mobpacks")
            .iter()
            .find(|sample| sample["id"] == "sample_docs_only")
            .expect("docs sample")["document"]
            .clone();
        let leaking = document_with_path_skill(&sample, &secret_file);

        let listed = protected_rpc_response_with_host_deploy(
            Some(deploy_denying_access_controller()),
            false,
            rpc_body("meta/mobpacks/list", json!({ "store_path": secret_json })),
        )
        .await;
        let source = protected_rpc_response_with_host_deploy(
            Some(deploy_denying_access_controller()),
            false,
            rpc_body("meta/mobpacks/source", json!({ "document": leaking })),
        )
        .await;
        let validation = protected_rpc_response_with_host_deploy(
            Some(deploy_denying_access_controller()),
            false,
            rpc_body("meta/mobpacks/validate", json!({ "document": leaking })),
        )
        .await;

        assert_eq!(
            (
                listed["error"]["code"].clone(),
                listed["error"]["data"].clone()
            ),
            (json!(-32602), json!({ "param": "store_path" })),
            "{listed:#?}"
        );
        for response in [&listed, &source, &validation] {
            assert!(
                !response.to_string().contains("DEPLOY-DENIED-SECRET-5d1e"),
                "{response:#?}"
            );
        }
        assert!(
            validation["result"]["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == "selected_skill_path_not_permitted")),
            "{validation:#?}"
        );
    }

    /// Set in the child process of the host path sweep, to its scratch
    /// directory.
    const HOST_PATH_SWEEP_CHILD: &str = "META_TEST_FLOW_EDITOR_HOST_PATH_SWEEP";
    const HOST_PATH_SWEEP_TEST: &str = "http_flow_editor::tests::flow_editor_surfaces_without_host_access_touch_no_caller_named_host_path";

    /// Security regression sweep. On every Flow Editor surface that a caller
    /// without host access reaches (a runtime without an access controller,
    /// a runtime whose controller denies `mobpack.deploy`, and the standalone
    /// router), every advertised method, and the advertised list is exactly
    /// the dispatch surface:
    /// - runs no `rkat` and writes no archive;
    /// - refuses a caller-chosen draft store file;
    /// - returns no bytes, digest or base64 of a file the skills catalog does
    ///   not list, and leaves none in the draft store, while a listed skill
    ///   still packs.
    ///
    /// The sweep runs in a child process whose draft store
    /// (`META_FLOW_EDITOR_DRAFT_STORE`) is a scratch file, so it also covers
    /// the methods that write the store.
    #[cfg(unix)]
    #[test]
    fn flow_editor_surfaces_without_host_access_touch_no_caller_named_host_path() {
        if let Some(scratch) = std::env::var_os(HOST_PATH_SWEEP_CHILD) {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("child runtime")
                .block_on(host_path_sweep(std::path::Path::new(&scratch)));
            return;
        }
        let scratch = tempfile::tempdir().expect("tempdir");
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                HOST_PATH_SWEEP_TEST,
                "--exact",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(HOST_PATH_SWEEP_CHILD, scratch.path())
            .env(
                "META_FLOW_EDITOR_DRAFT_STORE",
                scratch.path().join("drafts.json"),
            )
            .output()
            .expect("run the sweep child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "sweep child failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    async fn host_path_sweep(scratch: &std::path::Path) {
        use base64::Engine as _;
        use sha2::Digest as _;
        let store = std::path::PathBuf::from(
            std::env::var_os("META_FLOW_EDITOR_DRAFT_STORE").expect("child draft store"),
        );
        // A caller without host access is never told where the host keeps
        // the draft store.
        let store_text = store.to_string_lossy().into_owned();
        let secret_text = "SWEEP-HOST-FILE-SECRET-4be7";
        let secret = scratch.join("id_secret");
        std::fs::write(&secret, secret_text).expect("write secret");
        let secret_marks = [
            secret_text.to_string(),
            format!("{:x}", sha2::Sha256::digest(secret_text.as_bytes())),
            base64::engine::general_purpose::STANDARD.encode(secret_text),
        ];
        let secret_json = scratch.join("secret.json");
        std::fs::write(
            &secret_json,
            format!(r#"{{"credential": {{"token": "{secret_text}"}}}}"#),
        )
        .expect("write secret json");
        let listed_skill = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.claude/skills/meerkat-meta/SKILL.md");
        let listed_text = std::fs::read_to_string(&listed_skill).expect("catalog skill file");
        let marker = scratch.join("rkat.runs");
        let rkat = recording_rkat(scratch, &marker);
        let outputs = scratch.join("outputs");
        let mut expected = std::collections::BTreeSet::from(["meta/capabilities".to_string()]);
        expected.extend(
            crate::rpc::MOBPACK_AUTHORING_METHODS
                .iter()
                .map(ToString::to_string),
        );

        let mut runtime =
            runtime_without_access_control(scratch, "flow-editor-host-path-sweep").await;
        let without_controller = runtime.build_reference_app_router(open_decision_state());
        runtime.set_access_controller(deploy_denying_access_controller());
        let denying_deploy = runtime.build_reference_app_router(open_decision_state());
        let surfaces = [
            ("runtime without an access controller", without_controller),
            ("runtime denying mobpack.deploy", denying_deploy),
            ("standalone router", super::flow_editor_router()),
        ];
        let mut findings = Vec::new();
        for (index, (surface, app)) in surfaces.iter().enumerate() {
            let chosen = scratch.join(format!("caller-chosen-{index}"));
            let capabilities =
                anonymous_flow_editor_rpc(app, "meta/capabilities", Value::Null).await;
            let advertised = capabilities["result"]["methods"]
                .as_array()
                .expect("advertised methods")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<std::collections::BTreeSet<_>>();
            if advertised != expected {
                findings.push(format!("{surface}: advertises {advertised:?}"));
            }
            // An unadvertised method is refused by the dispatcher's own guard,
            // not just left out of the advertisement: removing that guard
            // serves it and fails this.
            let unadvertised = anonymous_flow_editor_rpc(app, "meta/status", json!({})).await;
            if unadvertised["error"]["code"] != json!(-32601)
                || unadvertised["error"]["data"]
                    != json!({ "kind": "method_not_advertised", "method": "meta/status" })
            {
                findings.push(format!(
                    "{surface}: served unadvertised meta/status: {unadvertised}"
                ));
            }
            let sample = sample_docs_only_document(app).await;
            let leaking = document_with_path_skill(&sample, &secret);
            let mut swept = std::collections::BTreeSet::new();
            for method in &advertised {
                swept.insert(method.clone());
                let host_knobs = json!({
                    "rkat_validate": true,
                    "rkatValidate": true,
                    "execute": true,
                    "rkat_bin": rkat,
                    "output_dir": outputs.join("deploy"),
                    "validation_output_dir": outputs.join("validate"),
                    "validation_pack_path": outputs.join("validate.mobpack"),
                });
                let response = anonymous_flow_editor_rpc(
                    app,
                    method,
                    method_params(method, &sample, &host_knobs),
                )
                .await;
                if marker.exists() || outputs.exists() {
                    findings.push(format!(
                        "{surface}: {method} ran rkat or wrote an archive: {response}"
                    ));
                    let _ = std::fs::remove_file(&marker);
                    let _ = std::fs::remove_dir_all(&outputs);
                }
                if response.to_string().contains(&store_text) {
                    findings.push(format!(
                        "{surface}: {method} named the draft store: {response}"
                    ));
                }
                let store_path = json!({ "id": "probe", "store_path": chosen.join("drafts.json") });
                let response = anonymous_flow_editor_rpc(
                    app,
                    method,
                    method_params(method, &sample, &store_path),
                )
                .await;
                if response["error"]["data"] != json!({ "param": "store_path" }) || chosen.exists()
                {
                    findings.push(format!(
                        "{surface}: {method} accepted store_path: {response}"
                    ));
                }
                let draft = json!({ "id": "sweep-draft" });
                let response =
                    anonymous_flow_editor_rpc(app, method, method_params(method, &leaking, &draft))
                        .await
                        .to_string();
                if response.contains(&store_text) {
                    findings.push(format!(
                        "{surface}: {method} named the draft store: {response}"
                    ));
                }
                if let Some(mark) = secret_marks
                    .iter()
                    .find(|mark| response.contains(mark.as_str()))
                {
                    findings.push(format!("{surface}: {method} returned the secret ({mark})"));
                }
            }
            if swept != advertised {
                findings.push(format!("{surface}: swept {swept:?}"));
            }
            // The store writers keep exported skill bytes in the draft store,
            // and get/list re-import them: walk one draft through every writer
            // with a document that names the secret, including a redo that
            // rebuilds that document from the draft's history.
            for (method, document) in [
                ("meta/mobpacks/save", Some(&sample)),
                ("meta/mobpacks/save", Some(&leaking)),
                ("meta/mobpacks/undo", None),
                ("meta/mobpacks/redo", None),
                ("meta/mobpacks/get", None),
                ("meta/mobpacks/list", None),
                ("meta/mobpacks/create", None),
                ("meta/mobpacks/delete", None),
            ] {
                let mut params = json!({ "id": "sweep-history" });
                if let Some(document) = document {
                    params["document"] = document.clone();
                }
                let response = anonymous_flow_editor_rpc(app, method, params).await;
                let stepped = matches!(method, "meta/mobpacks/undo" | "meta/mobpacks/redo");
                if response["error"] != Value::Null
                    || (stepped && response["result"]["stepped"] != json!(true))
                {
                    findings.push(format!("{surface}: draft {method} did not run: {response}"));
                }
                let response = response.to_string();
                if response.contains(&store_text) {
                    findings.push(format!("{surface}: draft {method} named the draft store"));
                }
                if let Some(mark) = secret_marks
                    .iter()
                    .find(|mark| response.contains(mark.as_str()))
                {
                    findings.push(format!(
                        "{surface}: draft {method} returned the secret ({mark})"
                    ));
                }
            }
            let leaked_store = anonymous_flow_editor_rpc(
                app,
                "meta/mobpacks/list",
                json!({ "store_path": secret_json }),
            )
            .await
            .to_string();
            if leaked_store.contains(secret_text) {
                findings.push(format!("{surface}: list read a caller-chosen JSON file"));
            }
            let packed = anonymous_flow_editor_rpc(
                app,
                "meta/mobpacks/source",
                json!({ "document": document_with_path_skill(&sample, &listed_skill) }),
            )
            .await;
            if !packed["result"]["source_files"]
                .as_array()
                .is_some_and(|files| {
                    files
                        .iter()
                        .any(|file| file["text"].as_str() == Some(listed_text.as_str()))
                })
            {
                findings.push(format!("{surface}: a listed skill did not pack: {packed}"));
            }
        }
        let stored = std::fs::read_to_string(&store).unwrap_or_default();
        if let Some(mark) = secret_marks
            .iter()
            .find(|mark| stored.contains(mark.as_str()))
        {
            findings.push(format!("the draft store holds the secret ({mark})"));
        }
        let _shutdown = runtime.shutdown().await;
        assert_eq!(findings, Vec::<String>::new());
    }

    /// The standalone Flow Editor server handles at most 16 requests at once;
    /// the rest wait for a slot. Any local process can call it, and every
    /// request it handles waits on the blocking pool, which it could fill.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_standalone_server_handles_at_most_sixteen_requests_at_once() {
        let store = tempfile::tempdir().expect("tempdir");
        let store_path = store.path().join("drafts.json");
        // Every draft method waits for the draft store lock this thread holds.
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _held = crate::mobpack::hold_draft_store_lock_for_test();
            held_tx.send(()).expect("held");
            release_rx.recv().ok();
        });
        held_rx.recv().expect("the draft store lock is held");
        let app = super::flow_editor_router_with_host_deploy();
        let requests = (0..20)
            .map(|index| {
                let body = json!({
                    "jsonrpc": "2.0",
                    "id": index,
                    "method": "meta/mobpacks/list",
                    "params": { "store_path": store_path },
                })
                .to_string();
                let app = app.clone();
                tokio::spawn(async move {
                    app.oneshot(
                        Request::post("/flow-editor/rpc")
                            .header("content-type", "application/json")
                            .body(Body::from(body))
                            .expect("request"),
                    )
                    .await
                })
            })
            .collect::<Vec<_>>();
        let store_name = store_path.to_string_lossy().into_owned();
        let started = || {
            super::STANDALONE_RPC_REQUESTS_STARTED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .filter(|path| **path == store_name)
                .count()
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while started() < 16 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        // Long enough for any request past the limit to start, were it let in.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let at_once = started();
        release_tx.send(()).expect("release the lock");
        holder.join().expect("the lock holder");
        for request in requests {
            let response = request.await.expect("request task").expect("response");
            assert_eq!(response.status(), axum::http::StatusCode::OK);
        }
        assert_eq!(at_once, 16, "requests the server handled at once");
    }
}
