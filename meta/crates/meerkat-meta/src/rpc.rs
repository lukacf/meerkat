//! JSON-RPC request handling for both module-only and unified runtime modes.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::console_aggregator::is_implicit_delegate_member;
use crate::mob_handle_runtime::{topology_restore_failed_peer_ids, topology_restore_warning_json};
use crate::runtime::{
    BigQuerySessionStoreAdapter, BigQuerySessionStoreError, ConsoleRestJsonRequest,
    ConsoleRestJsonResponse, DeliveryHistoryRequest, DeliverySendError, DeliverySendRequest,
    GatingDecideError, GatingDecideRequest, GatingDecision, GatingEvaluateRequest, GatingRiskTier,
    LocalJsonMemoryStoreError, MemoryIndexError, MemoryIndexRequest, MemoryQueryRequest,
    MetaRuntimeHandle, ModuleRouteError, ModuleRouteRequest, ROUTING_RETRY_MAX_CAP,
    RoutingResolveError, RoutingResolveRequest, RuntimeDecisionState, RuntimeRoute,
    RuntimeRouteMutationError, SessionPersistenceRow, SubscribeError, SubscribeRequest,
    SubscribeScope, handle_console_rest_json_route, route_module_call,
};
use crate::unified_runtime::{EventQuery, UnifiedRuntime};

mod console_ingress;
mod gating_methods;
pub(crate) mod memory_methods;
pub(crate) mod mob_methods;
pub(crate) mod operator_methods;
pub(crate) mod params;
mod routing_delivery_methods;
pub(crate) mod schedule_methods;
mod session_store_methods;
pub(crate) mod storage_methods;
mod subscribe_methods;
pub(crate) mod topology_methods;
pub(crate) mod workgraph_methods;

pub use console_ingress::handle_console_ingress_json;

use gating_methods::{
    GatingParamsError, gating_audit_result, gating_decide_rpc_error, parse_gating_audit_params,
    parse_gating_decide_params, parse_gating_evaluate_params, parse_gating_pending_params,
};
pub(crate) use gating_methods::{gating_ids_unavailable_rpc_error, parse_gating_audit_cursor};
use memory_methods::{
    MemoryParamsError, parse_agent_memory_forget_params, parse_agent_memory_manifest_params,
    parse_agent_memory_recall_params, parse_agent_memory_remember_params,
    parse_agent_memory_update_params, parse_memory_index_params, parse_memory_query_params,
    parse_memory_stores_params,
};
use routing_delivery_methods::{
    RoutingDeliveryParamsError, parse_delivery_history_params, parse_delivery_send_params,
    parse_routing_resolve_params, parse_routing_route_add_params,
    parse_routing_route_delete_params, parse_routing_routes_list_params,
};
use session_store_methods::{
    BigQuerySessionStoreOperation, BigQuerySessionStoreRpcError, format_bigquery_store_error,
    parse_bigquery_session_store_operation, parse_bigquery_session_store_params,
    run_bigquery_session_store_request,
};
use subscribe_methods::{SubscribeParamsError, parse_subscribe_request};

pub const JSONRPC_VERSION: &str = "2.0";
pub const META_CONTRACT_VERSION: &str = "0.5.0";
/// JSON-RPC code for a known feature that is unavailable on this host.
pub const CAPABILITY_UNAVAILABLE_CODE: i64 = -32004;
/// Revoke every Console session of one principal as of now: the Console's
/// verifier refuses each token of the principal issued before then, on
/// every route, and ends the streams and voice sessions such tokens opened.
pub const CONSOLE_REVOKE_SESSIONS_METHOD: &str = "meta/console/revoke_sessions";
/// `meta/capabilities.feature_capabilities` name of a keyed `meta/send` that
/// takes `handling_mode`; a gateway without it ignores the mode and queues.
pub(crate) const KEYED_SEND_HANDLING_MODE_FEATURE: &str = "keyed_send_handling_mode";
pub(crate) const MOBPACK_AUTHORING_METHODS: &[&str] = &[
    "meta/mobpacks/schema",
    "meta/mobpacks/catalogs",
    "meta/tools/catalog",
    "meta/skills/catalog",
    "meta/agent_definitions/list",
    "meta/mobpacks/templates",
    "meta/mobpacks/validate",
    "meta/mobpacks/source",
    "meta/mobpacks/export",
    "meta/mobpacks/import",
    "meta/mobpacks/list",
    "meta/mobpacks/get",
    "meta/mobpacks/create",
    "meta/mobpacks/save",
    "meta/mobpacks/delete",
    "meta/mobpacks/undo",
    "meta/mobpacks/redo",
    "meta/mobpacks/apply_operation",
    "meta/mobpacks/graph_projection",
    "meta/mobpacks/graph_to_flow",
    "meta/mobpacks/deploy_command",
    "meta/mobpacks/deploy",
];

pub(crate) fn mobpack_authoring_capabilities() -> Value {
    serde_json::json!({
        "domain": "mobpack_authoring",
        "runtime_mutation": false,
        "host_mutation_methods": {
            "meta/mobpacks/deploy": "when execute=true, writes a mobpack archive and runs rkat mob run on the host",
            "meta/mobpacks/validate": "when rkat_validate=true, writes a mobpack archive and runs rkat mob validate on the host"
        },
        "deploy_command": "rkat mob run",
        "methods": MOBPACK_AUTHORING_METHODS,
        "operations": crate::mobpack::mobpack_authoring_operations(),
    })
}

async fn mobpack_runtime_catalog_state(
    runtime: &UnifiedRuntime,
) -> crate::mobpack::MobpackRuntimeCatalogState {
    let loaded_modules = runtime.loaded_modules().await;
    let mob_handle = runtime.mob_handle();
    let definition = mob_handle.definition();
    // The runtime projects its own definition, as the host.
    let (runtime_flow_rows, runtime_agent_definition_sources, runtime_skill_realms) =
        crate::mobpack::with_host_path_skill_reads(|| {
            (
                crate::mobpack::runtime_flow_registry_rows_from_definition(definition),
                crate::mobpack::runtime_agent_definition_sources_from_definition(definition),
                crate::mobpack::runtime_skill_realms_from_definition(definition),
            )
        });
    let mut runtime_methods = vec![
        "meta/capabilities".to_string(),
        "meta/models/catalog".to_string(),
        "meta/spawn_member".to_string(),
        "meta/list_members".to_string(),
        "meta/get_member".to_string(),
        "meta/run_flow".to_string(),
        "meta/list_flows".to_string(),
        "meta/list_runs".to_string(),
    ];
    runtime_methods.extend(
        MOBPACK_AUTHORING_METHODS
            .iter()
            .map(std::string::ToString::to_string),
    );
    if runtime.has_contact_directory() {
        runtime_methods.push("meta/cross_mob/directory".to_string());
    }
    if (runtime.has_peer_mob_handles().await && runtime.has_inproc_contacts())
        || runtime.has_remote_contacts()
    {
        runtime_methods.extend([
            "meta/cross_mob/wire".to_string(),
            "meta/cross_mob/unwire".to_string(),
            "meta/cross_mob/send".to_string(),
        ]);
    }
    crate::mobpack::MobpackRuntimeCatalogState {
        loaded_modules,
        runtime_methods,
        has_contact_directory: runtime.has_contact_directory(),
        has_peer_mob_handles: runtime.has_peer_mob_handles().await,
        has_inproc_contacts: runtime.has_inproc_contacts(),
        runtime_flow_rows,
        runtime_agent_definition_sources,
        runtime_skill_realms,
    }
}

async fn handle_unified_mobpack_authoring_rpc(
    runtime: &UnifiedRuntime,
    method: &str,
    params: &Value,
    response_id: Value,
) -> JsonRpcResponse {
    if let Some(refusal) = explicit_provider_credentials_refusal(
        runtime.has_explicit_provider_credentials(),
        method,
        params,
        response_id.clone(),
    ) {
        return refusal;
    }
    let runtime_catalog_state = match method {
        "meta/mobpacks/schema"
        | "meta/mobpacks/catalogs"
        | "meta/tools/catalog"
        | "meta/skills/catalog"
        | "meta/agent_definitions/list"
        | "meta/mobpacks/templates"
        | "meta/mobpacks/list"
        | "meta/mobpacks/get"
        | "meta/mobpacks/apply_operation" => Some(mobpack_runtime_catalog_state(runtime).await),
        _ => None,
    };
    // The host's own JSON-RPC channel (stdio, in-process SDK hosts): path
    // skills read as the host.
    crate::mobpack::with_host_path_skill_reads(|| {
        unified_mobpack_authoring_response(
            method,
            params,
            response_id,
            runtime_catalog_state.as_ref(),
        )
    })
}

fn unified_mobpack_authoring_response(
    method: &str,
    params: &Value,
    response_id: Value,
    runtime_catalog_state: Option<&crate::mobpack::MobpackRuntimeCatalogState>,
) -> JsonRpcResponse {
    let result = match method {
        "meta/mobpacks/catalogs" => Ok(crate::mobpack::mobpack_catalogs_response_with_runtime(
            runtime_catalog_state,
        )),
        "meta/tools/catalog" => Ok(crate::mobpack::mobpack_tools_catalog_response_with_runtime(
            runtime_catalog_state,
        )),
        "meta/skills/catalog" => {
            Ok(crate::mobpack::mobpack_skills_catalog_response_with_runtime(runtime_catalog_state))
        }
        "meta/agent_definitions/list" => Ok(
            crate::mobpack::mobpack_agent_definitions_response_with_runtime(runtime_catalog_state),
        ),
        "meta/mobpacks/templates" => Ok(crate::mobpack::mobpack_templates_response_with_runtime(
            runtime_catalog_state,
        )),
        _ => {
            return handle_mobpack_authoring_rpc_with_runtime(
                method,
                params,
                response_id.clone(),
                runtime_catalog_state,
            )
            .unwrap_or_else(|| JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: "Method not found".to_string(),
                    data: None,
                }),
            });
        }
    };
    match result {
        Ok(result) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(result),
            error: None,
        },
        Err(message) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message,
                data: None,
            }),
        },
    }
}

pub(crate) fn handle_mobpack_authoring_rpc(
    method: &str,
    params: &Value,
    response_id: Value,
) -> Option<JsonRpcResponse> {
    handle_mobpack_authoring_rpc_with_runtime(method, params, response_id, None)
}

pub(crate) fn handle_mobpack_authoring_rpc_with_runtime(
    method: &str,
    params: &Value,
    response_id: Value,
    runtime: Option<&crate::mobpack::MobpackRuntimeCatalogState>,
) -> Option<JsonRpcResponse> {
    // A caller that set no path skill read policy reads only the files the
    // skills catalog lists: host reads are an explicit choice of the entry
    // point (see `crate::mobpack::with_host_path_skill_reads`).
    crate::mobpack::with_path_skill_reads_or_untrusted(runtime, || {
        mobpack_authoring_response(method, params, response_id, runtime)
    })
}

fn mobpack_authoring_response(
    method: &str,
    params: &Value,
    response_id: Value,
    runtime: Option<&crate::mobpack::MobpackRuntimeCatalogState>,
) -> Option<JsonRpcResponse> {
    if let Some(result) = crate::mobpack::mobpack_draft_store_method(method, params, runtime) {
        let (result, error) = match result {
            Ok(result) => (Some(result), None),
            Err(failure) => (
                None,
                Some(JsonRpcError {
                    code: -32602,
                    message: failure.message,
                    data: failure.data,
                }),
            ),
        };
        return Some(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result,
            error,
        });
    }
    let result = match method {
        "meta/mobpacks/schema" => Ok(crate::mobpack::mobpack_schema_response_with_runtime(
            runtime,
        )),
        "meta/mobpacks/catalogs" => Ok(crate::mobpack::mobpack_catalogs_response_with_runtime(
            runtime,
        )),
        "meta/tools/catalog" => Ok(crate::mobpack::mobpack_tools_catalog_response_with_runtime(
            runtime,
        )),
        "meta/skills/catalog" => {
            Ok(crate::mobpack::mobpack_skills_catalog_response_with_runtime(runtime))
        }
        "meta/agent_definitions/list" => {
            Ok(crate::mobpack::mobpack_agent_definitions_response_with_runtime(runtime))
        }
        "meta/mobpacks/templates" => Ok(crate::mobpack::mobpack_templates_response_with_runtime(
            runtime,
        )),
        "meta/mobpacks/validate" => crate::mobpack::validate_mobpack(params)
            .and_then(|result| serde_json::to_value(result).map_err(|err| err.to_string())),
        "meta/mobpacks/source" => crate::mobpack::source_mobpack(params)
            .and_then(|result| serde_json::to_value(result).map_err(|err| err.to_string())),
        "meta/mobpacks/export" => crate::mobpack::export_mobpack(params)
            .and_then(|result| serde_json::to_value(result).map_err(|err| err.to_string())),
        "meta/mobpacks/import" => crate::mobpack::import_mobpack(params),
        "meta/mobpacks/apply_operation" => {
            crate::mobpack::apply_mobpack_authoring_operation_with_runtime(params, runtime)
        }
        "meta/mobpacks/graph_projection" => crate::mobpack::graph_projection_mobpack(params),
        "meta/mobpacks/graph_to_flow" => crate::mobpack::graph_to_flow_mobpack(params),
        "meta/mobpacks/deploy_command" => crate::mobpack::deploy_command_preview(params)
            .and_then(|result| serde_json::to_value(result).map_err(|err| err.to_string())),
        "meta/mobpacks/deploy" => crate::mobpack::deploy_mobpack(params)
            .and_then(|result| serde_json::to_value(result).map_err(|err| err.to_string())),
        _ => return None,
    };
    Some(match result {
        Ok(result) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(result),
            error: None,
        },
        Err(message) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message,
                data: None,
            }),
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcCapabilitiesError {
    InvalidJson,
    InvalidSchema,
    MissingContractVersion,
    InvalidContractVersion,
}

impl std::fmt::Display for RpcCapabilitiesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson => write!(f, "invalid JSON"),
            Self::InvalidSchema => write!(f, "invalid schema"),
            Self::MissingContractVersion => write!(f, "missing contract version"),
            Self::InvalidContractVersion => write!(f, "invalid contract version"),
        }
    }
}

impl std::error::Error for RpcCapabilitiesError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcCapabilities {
    pub contract_version: String,
    #[serde(default)]
    pub feature_capabilities: Vec<crate::live_contracts::FeatureCapability>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl RpcCapabilities {
    /// Whether this exact gateway admitted the strict v1 live identity
    /// envelope. Clients must check this before sending `execution_identity`.
    #[must_use]
    pub fn supports_live_execution_identity_v1(&self) -> bool {
        self.feature_capabilities.iter().any(|capability| {
            capability.as_str() == crate::live_contracts::LIVE_EXECUTION_IDENTITY_V1
        })
    }

    #[must_use]
    pub fn supports_live_execution_mode(
        &self,
        mode: crate::live_contracts::LiveExecutionMode,
    ) -> bool {
        self.feature_capabilities
            .iter()
            .any(|capability| capability.as_str() == mode.capability())
    }
}

pub fn parse_rpc_capabilities(line: &str) -> Result<RpcCapabilities, RpcCapabilitiesError> {
    let raw: Value = serde_json::from_str(line).map_err(|_| RpcCapabilitiesError::InvalidJson)?;
    let object = raw.as_object().ok_or(RpcCapabilitiesError::InvalidSchema)?;
    let contract = object
        .get("contract_version")
        .ok_or(RpcCapabilitiesError::MissingContractVersion)?;
    let contract_str = contract
        .as_str()
        .ok_or(RpcCapabilitiesError::InvalidContractVersion)?;
    if contract_str.trim().is_empty() {
        return Err(RpcCapabilitiesError::InvalidContractVersion);
    }
    serde_json::from_value(raw).map_err(|_| RpcCapabilitiesError::InvalidSchema)
}

/// JSON-RPC error code returned by `meta/mob_events/{query,subscribe}`
/// when the caller's `after_seq` is past the current ledger frontier.
/// The error `data` field carries `{ after_cursor, latest_cursor }` so
/// SDKs can surface a typed exception. Single source of truth — keep
/// this in sync with `MobEventsStaleError` in the Python and TypeScript
/// SDKs.
pub const MOB_EVENTS_STALE_CURSOR_CODE: i64 = -32010;

/// JSON-RPC error code returned by `meta/memory/index` and
/// `meta/memory/query` when the configured memory backend cannot
/// persist or retrieve the row. Distinct from
/// [`MOB_EVENTS_STALE_CURSOR_CODE`] so SDKs can branch on `-32010`
/// without misclassifying a memory backend failure as a stale-cursor
/// event.
pub const MEMORY_BACKEND_UNAVAILABLE_CODE: i64 = -32012;

/// JSON-RPC error code returned by `meta/console/query_timeline` when
/// the requested console cursor cannot be replayed from the durable console
/// timeline. Distinct from [`MOB_EVENTS_STALE_CURSOR_CODE`] because SDKs
/// reify `-32010` specifically as a mob-events ledger error.
pub const CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE: i64 = -32013;

/// JSON-RPC error code for the fail-closed storage refusals `rpc_gateway`
/// emits at `meta/init` (M5): file-name twins the layout refuses to pick
/// between, a store that failed to open where the silent fallback used to
/// be (session/runtime/blob/metadata/console/continuity), and state-root
/// creation failures. The message carries the remediation (the storage
/// doctor, or the explicit ephemeral declaration). Distinct from `-32603`
/// so SDKs can reify a deliberate durability refusal instead of reporting
/// a generic internal error. Single source of truth — keep in sync with
/// `StorageResolutionError` in the Python and TypeScript SDKs.
pub const STORAGE_RESOLUTION_CODE: i64 = -32014;

/// JSON-RPC error code the stdio gateway returns, at once, for an ordinary
/// request that arrives before `meta/init` has settled. Ordinary dispatch
/// starts only after init settles, so the request is refused rather than
/// queued behind startup: a provider callback that awaited it during init
/// would otherwise wait on itself. `meta/shutdown` and callback responses
/// are not refused. Keep in sync with `InitInProgressError` in the Python and
/// TypeScript SDKs.
pub const INIT_IN_PROGRESS_CODE: i64 = -32018;

/// JSON-RPC error code for the mob composition provenance refusals
/// `rpc_gateway` emits at `meta/init`: a persistent mob storage whose
/// recorded composition the launch cannot be proven to match (a diverged
/// definition, a rehearsal-created store, a candidate whose definition
/// differs from the stored one, an unreadable, malformed or unsupported
/// manifest, or undeclared non-empty storage; an absent manifest is adopted
/// by an authoritative launch rather than refused). The error `data` is a
/// [`crate::mob_composition_manifest::CompositionProvenanceRefusal`]: its
/// `kind` and, where the variant has them, the diverged `fields`. Distinct
/// from `-32603` so SDKs can reify a deliberate composition refusal instead of
/// a generic internal error. Keep in sync with `CompositionProvenanceError` in
/// the Python and TypeScript SDKs.
pub const COMPOSITION_PROVENANCE_CODE: i64 = -32019;

/// JSON-RPC error code returned by every `meta/workgraph/*` method when
/// the runtime has no WorkGraph service configured
/// (`data.kind = "workgraph_unavailable"`). Single source of truth — keep
/// in sync with the Python and TypeScript SDKs.
pub const WORKGRAPH_UNAVAILABLE_CODE: i64 = -32041;

/// JSON-RPC error code for WorkGraph CAS/revision conflicts (upstream
/// `StaleRevision`/`Conflict`), `data.kind = "workgraph_conflict"` with the
/// upstream message in `data.detail`. SDKs and the console retry by
/// refetching the current revision.
pub const WORKGRAPH_CONFLICT_CODE: i64 = -32042;

/// JSON-RPC error code for all other WorkGraph domain failures
/// (`data.kind = "workgraph_error"`, full detail — K2 disclosure posture).
pub const WORKGRAPH_ERROR_CODE: i64 = -32000;

/// JSON-RPC error code for a quarantine review the memory store refused
/// (`meta/memory/quarantine/decide`). Nothing was written.
/// `data.kind = "memory_quarantine_review_refused"`; `data.reason` is the
/// typed refusal (`not_found`, `content_mismatch`, `not_quarantined`,
/// `gate_pending`, `successor_conflict`, `secret_detected`, `stale_update`)
/// with its fields.
pub const MEMORY_QUARANTINE_REVIEW_REFUSED_CODE: i64 = -32043;

/// The answer to [`CONSOLE_REVOKE_SESSIONS_METHOD`]: the principal, the fence
/// (seconds since the epoch) and the earliest `iat` a token of the principal
/// is accepted with from now on. That is the fence plus the verifier's clock
/// leeway, so a sign-in within that window after the revocation is refused
/// too.
pub(crate) fn console_session_fence_json(
    fence: &crate::auth::session_fence::ConsoleSessionFence,
) -> Value {
    serde_json::json!({
        "principal": fence.principal,
        "not_before": fence.not_before_epoch_seconds,
        "accepts_tokens_issued_from": crate::runtime::session_fence_first_accepted_iat(
            fence.not_before_epoch_seconds,
        ),
    })
}

/// The `non_expiring_console_principals` field of `meta/status`: each
/// principal the Console accepted a token without `exp` for, as the
/// principal and its flag, or `None` (the field is omitted) when there is
/// none.
pub(crate) fn non_expiring_console_principals_status(
    fences: &crate::auth::session_fence::ConsoleSessionFences,
) -> Option<Value> {
    let principals = fences.non_expiring_principals();
    (!principals.is_empty()).then(|| {
        Value::Array(
            principals
                .into_iter()
                .map(
                    |principal| serde_json::json!({ "principal": principal, "non_expiring": true }),
                )
                .collect(),
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    /// Optional structured payload as defined by JSON-RPC 2.0. Used by
    /// typed errors (e.g. `event_query_stale` with `after_cursor` /
    /// `latest_cursor`) so SDKs can surface a typed exception. Existing
    /// construction sites can omit it via `..Default::default()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

pub fn handle_meta_rpc_json(
    runtime: &mut MetaRuntimeHandle,
    request_json: &str,
    timeout: Duration,
) -> String {
    let raw_request: Value = match serde_json::from_str(request_json) {
        Ok(raw_request) => raw_request,
        Err(_) => {
            return serialize_response(&JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Value::Null,
                result: None,
                error: Some(JsonRpcError {
                    code: -32700,
                    message: "Parse error".to_string(),
                    data: None,
                }),
            });
        }
    };
    let response_id = raw_request
        .as_object()
        .and_then(|object| object.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    let request: JsonRpcRequest = match serde_json::from_value(raw_request) {
        Ok(request) => request,
        Err(_) => {
            return serialize_response(&JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32600,
                    message: "Invalid Request".to_string(),
                    data: None,
                }),
            });
        }
    };
    let is_notification = request.id.is_none();
    let response_id = request.id.clone().unwrap_or(Value::Null);

    if request.jsonrpc != "2.0" {
        let response = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32600,
                message: "Invalid Request".to_string(),
                data: None,
            }),
        };
        return if is_notification {
            String::new()
        } else {
            serialize_response(&response)
        };
    }

    if let Err(message) =
        crate::member_comms_id::validate_public_rpc_member_aliases(&request.params)
    {
        let response = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: format!("Invalid params: {message}"),
                data: None,
            }),
        };
        return if is_notification {
            String::new()
        } else {
            serialize_response(&response)
        };
    }

    if let Some(response) = live_open_execution_identity_preflight(
        request.method.as_str(),
        &request.params,
        response_id.clone(),
        false,
    ) {
        return if is_notification {
            String::new()
        } else {
            serialize_response(&response)
        };
    }
    if let Some(response) = experimental_live_target_preflight(
        request.method.as_str(),
        &request.params,
        response_id.clone(),
    ) {
        return if is_notification {
            String::new()
        } else {
            serialize_response(&response)
        };
    }

    let response = match request.method.as_str() {
        // No `storage` object here: the module-only server holds a bare
        // `MetaRuntimeHandle` with no mob/unified runtime, so the
        // composition-time `ResolvedStorageSummary` (H1/H2) is not reachable
        // on this surface.
        "meta/status" => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(serde_json::json!({
                "contract_version": META_CONTRACT_VERSION,
                "running": runtime.is_running(),
                "loaded_modules": runtime.loaded_modules(),
            })),
            error: None,
        },
        "meta/capabilities" => {
            let mut methods = vec![
                "meta/status",
                "meta/capabilities",
                "meta/reconcile",
                "meta/spawn_member",
                "meta/routing/resolve",
                "meta/routing/routes/list",
                "meta/routing/routes/add",
                "meta/routing/routes/delete",
                "meta/delivery/send",
                "meta/delivery/history",
                "meta/events/subscribe",
                "meta/memory/stores",
                "meta/memory/index",
                "meta/memory/query",
                "meta/session_store/bigquery",
                "meta/gating/evaluate",
                "meta/gating/pending",
                "meta/gating/decide",
                "meta/gating/audit",
                "meta/call_tool",
                "meta/models/catalog",
                storage_methods::STORAGE_DOCTOR_METHOD,
            ];
            methods.extend_from_slice(MOBPACK_AUTHORING_METHODS);
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({
                    "contract_version": META_CONTRACT_VERSION,
                    "feature_capabilities": serde_json::json!([]),
                    "methods": methods,
                    "loaded_modules": runtime.loaded_modules(),
                    "runtime_capabilities": {
                        "can_spawn_members": false,
                        "can_send_messages": false,
                        "can_wire_members": false,
                        "can_retire_members": false,
                        "available_spawn_modes": ["module"],
                    },
                    "authoring_capabilities": mobpack_authoring_capabilities(),
                })),
                error: None,
            }
        }
        "meta/models/catalog" => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(build_models_catalog_result()),
            error: None,
        },
        // Read-only state-directory diagnosis. The module-only server holds
        // no runtime state directory, so `state_dir` is required here and
        // the durability census always reports unavailable.
        storage_methods::STORAGE_DOCTOR_METHOD => {
            match storage_methods::parse_storage_doctor_params(&request.params) {
                Ok(Some(params)) => {
                    let diagnosis = crate::storage_doctor::diagnose_state_dir_blocking_with_options(
                        &params.scope(),
                        None,
                        params.doctor_options(),
                    );
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(storage_methods::storage_doctor_result_json(
                            &params, &diagnosis, None,
                        )),
                        error: None,
                    }
                }
                Ok(None) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: "Invalid params: state_dir required (the module-only runtime \
                                  has no state directory)"
                            .to_string(),
                        data: None,
                    }),
                },
                Err(reason) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {reason}"),
                        data: None,
                    }),
                },
            }
        }
        method if MOBPACK_AUTHORING_METHODS.contains(&method) => {
            // The host's own JSON-RPC channel: path skills read as the host.
            crate::mobpack::with_host_path_skill_reads(|| {
                handle_mobpack_authoring_rpc(method, &request.params, response_id.clone())
            })
            .unwrap_or_else(|| JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: "Method not found".to_string(),
                    data: None,
                }),
            })
        }
        "meta/reconcile" => {
            let modules = match params::required_string_array(&request.params, "modules") {
                Ok(m) => m,
                Err(reason) => {
                    return serialize_response(&JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {reason}"),
                            data: None,
                        }),
                    });
                }
            };

            match runtime.reconcile_modules(modules.clone(), timeout) {
                Ok(added) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "accepted": true,
                        "reconciled_modules": modules,
                        "added": added
                    })),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {err:?}"),
                        data: None,
                    }),
                },
            }
        }
        "meta/spawn_member" => {
            let module_id = request
                .params
                .get("module_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if module_id.is_empty() {
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: "Invalid params: module_id required".to_string(),
                        data: None,
                    }),
                }
            } else {
                match runtime.spawn_member(&module_id, timeout) {
                    Ok(()) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "accepted": true,
                            "module_id": module_id
                        })),
                        error: None,
                    },
                    Err(err) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {err:?}"),
                            data: None,
                        }),
                    },
                }
            }
        }
        "meta/routing/resolve" => {
            match parse_routing_resolve_params(&request.params).and_then(|resolve_request| {
                runtime
                    .resolve_routing(resolve_request)
                    .map_err(RoutingDeliveryParamsError::Routing)
            }) {
                Ok(resolution) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(resolution).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/routing/routes/list" => match parse_routing_routes_list_params(&request.params) {
            Ok(()) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({
                    "routes": runtime.list_runtime_routes()
                })),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/routing/routes/add" => {
            match parse_routing_route_add_params(&request.params).and_then(|route| {
                runtime
                    .add_runtime_route(route)
                    .map_err(RoutingDeliveryParamsError::RouteMutation)
            }) {
                Ok(route) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({ "route": route })),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/routing/routes/delete" => match parse_routing_route_delete_params(&request.params)
            .and_then(|route_key| {
                runtime
                    .delete_runtime_route(&route_key)
                    .map_err(RoutingDeliveryParamsError::RouteMutation)
            }) {
            Ok(route) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({ "deleted": route })),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/delivery/send" => {
            match parse_delivery_send_params(&request.params).and_then(|send_request| {
                runtime
                    .send_delivery(send_request)
                    .map_err(RoutingDeliveryParamsError::Delivery)
            }) {
                Ok(record) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(record).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/delivery/history" => match parse_delivery_history_params(&request.params) {
            Ok(history_request) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(
                    serde_json::to_value(runtime.delivery_history(history_request))
                        .unwrap_or(Value::Null),
                ),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/events/subscribe" => {
            match parse_subscribe_request(&request.params).and_then(|subscribe_request| {
                runtime
                    .subscribe_events(subscribe_request)
                    .map_err(SubscribeParamsError::Runtime)
            }) {
                Ok(subscribe_result) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(subscribe_result).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/memory/stores" => match parse_memory_stores_params(&request.params) {
            Ok(()) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({
                    "stores": runtime.memory_stores(),
                })),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/memory/index" => match parse_memory_index_params(&request.params) {
            Ok(index_request) => match runtime.memory_index(index_request) {
                Ok(indexed) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(indexed).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(MemoryIndexError::BackendPersistFailed(error)) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: MEMORY_BACKEND_UNAVAILABLE_CODE,
                        message: format!(
                            "Memory backend unavailable: {}",
                            MemoryParamsError::backend_message(&error)
                        ),
                        data: None,
                    }),
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!(
                            "Invalid params: {}",
                            MemoryParamsError::Index(err).message()
                        ),
                        data: None,
                    }),
                },
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/memory/query" => match parse_memory_query_params(&request.params) {
            Ok(query_request) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(
                    serde_json::to_value(runtime.memory_query(query_request))
                        .unwrap_or(Value::Null),
                ),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/session_store/bigquery" => {
            match parse_bigquery_session_store_params(&request.params)
                .and_then(run_bigquery_session_store_request)
            {
                Ok(result) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(result),
                    error: None,
                },
                Err(BigQuerySessionStoreRpcError::Params(message)) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {message}"),
                        data: None,
                    }),
                },
                Err(BigQuerySessionStoreRpcError::Store(error)) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32011,
                        message: format!(
                            "BigQuery session store request failed: {}",
                            format_bigquery_store_error(&error)
                        ),
                        data: None,
                    }),
                },
            }
        }
        "meta/gating/evaluate" => match parse_gating_evaluate_params(&request.params) {
            Ok(gating_request) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(
                    serde_json::to_value(runtime.evaluate_gating_action(gating_request))
                        .unwrap_or(Value::Null),
                ),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/gating/pending" => match parse_gating_pending_params(&request.params) {
            Ok(()) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({
                    "pending": runtime.list_gating_pending(),
                })),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/gating/decide" => {
            match parse_gating_decide_params(&request.params).and_then(|decide_request| {
                runtime
                    .decide_gating_action(decide_request)
                    .map_err(GatingParamsError::Decision)
            }) {
                Ok(result) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(gating_decide_rpc_error(err)),
                },
            }
        }
        "meta/gating/audit" => match parse_gating_audit_params(&request.params) {
            Ok(params) => {
                let (entries, gap) = match params.after_audit_id.as_deref() {
                    Some(cursor) => {
                        let page = runtime.gating_audit_after(Some(cursor));
                        (
                            page.entries.into_iter().take(params.limit).collect(),
                            page.gap,
                        )
                    }
                    None => (runtime.gating_audit_entries(params.limit), false),
                };
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(gating_audit_result(
                        entries,
                        gap,
                        params.after_audit_id.as_deref(),
                    )),
                    error: None,
                }
            }
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/call_tool" => {
            let module_id = request.params.get("module_id").and_then(Value::as_str);
            let tool = request.params.get("tool").and_then(Value::as_str);
            let arguments = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            match (module_id, tool) {
                (Some(module_id), Some(tool)) if !module_id.is_empty() && !tool.is_empty() => {
                    let route = route_module_call(
                        runtime,
                        &ModuleRouteRequest {
                            module_id: module_id.to_string(),
                            method: tool.to_string(),
                            params: arguments,
                        },
                        timeout,
                    );
                    match route {
                        Ok(response) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: Some(serde_json::json!({
                                "module_id": response.module_id,
                                "tool": response.method,
                                "result": response.payload
                            })),
                            error: None,
                        },
                        Err(ModuleRouteError::UnloadedModule(mid)) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32601,
                                message: format!("Module '{mid}' not loaded"),
                                data: None,
                            }),
                        },
                        Err(err) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32000,
                                message: format!("Tool call failed: {err:?}"),
                                data: None,
                            }),
                        },
                    }
                }
                _ => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: "Invalid params: module_id and tool required".to_string(),
                        data: None,
                    }),
                },
            }
        }
        method if method.contains('/') && !method.starts_with("meta/") => {
            let module_id = method
                .split('/')
                .next()
                .map(ToString::to_string)
                .unwrap_or_default();
            let route = route_module_call(
                runtime,
                &ModuleRouteRequest {
                    module_id,
                    method: method.to_string(),
                    params: request.params,
                },
                timeout,
            );
            match route {
                Ok(response) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "module_id": response.module_id,
                        "method": response.method,
                        "payload": response.payload
                    })),
                    error: None,
                },
                Err(ModuleRouteError::UnloadedModule(module_id)) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!("Module '{module_id}' not loaded"),
                        data: None,
                    }),
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("Module route failed: {err:?}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        },
    };
    if is_notification {
        String::new()
    } else {
        serialize_response(&response)
    }
}

/// Identity-first runtime context passed to the RPC handler.
#[derive(Clone)]
pub struct IdentityFirstContext {
    /// The owning public view: the context is handed to the RPC handler.
    pub runtime: crate::identity_first::PublicIdentityRuntime,
    pub roster_provider: std::sync::Arc<dyn crate::identity_first::contracts::RosterProvider>,
    pub topology_provider:
        Option<std::sync::Arc<dyn crate::identity_first::contracts::TopologyProvider>>,
    pub customizer: Option<std::sync::Arc<dyn crate::identity_first::contracts::AgentCustomizer>>,
    pub agent_memory_provider:
        Option<std::sync::Arc<dyn crate::identity_first::AgentMemoryProvider>>,
    pub mob_definition: Option<meerkat_mob::MobDefinition>,
    /// CONCRETE persistent session service reached through the transcript
    /// extension traits, for the `bound_member_transcript` operator verb (and
    /// `compact_member`'s before/after evidence). The erased
    /// `Arc<dyn MobSessionService>` cannot reach
    /// `SessionServiceTranscriptEditExt` (no trait upcasting), so composition
    /// must thread the typed handle here. `None` disables the verbs with a
    /// typed refusal.
    pub transcript_edit_service:
        Option<std::sync::Arc<dyn crate::memory::hygienist::TranscriptEditSessionService>>,
    /// The identity bridge's compaction-floor registry
    /// ([`crate::identity_first::MobSessionBridge::compaction_floors`]),
    /// shared here so the `compact_member` operator verb arms/disarms floors
    /// on the same registry the materialization path reads. `None` disables
    /// the verb with a typed refusal.
    pub compaction_floors: Option<std::sync::Arc<crate::identity_first::CompactionFloorRegistry>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IdentityMemberReadiness {
    Ready,
    TimedOut,
    Failed(String),
}

/// The owner readiness wait also collects fresh member snapshots. Retry
/// only refused snapshot observations, within the caller's remaining budget.
async fn wait_identity_members_ready_with_observation_retry<T, Wait, WaitFuture>(
    remaining: Duration,
    mut wait: Wait,
) -> IdentityMemberReadiness
where
    Wait: FnMut(Duration) -> WaitFuture,
    WaitFuture: Future<Output = Result<T, meerkat_mob::MobError>>,
{
    let deadline = tokio::time::Instant::now() + remaining;
    let result = if remaining.is_zero() {
        // A zero-timeout query has always admitted an immediately successful
        // owner read (including an empty roster). It cannot wait or retry.
        match tokio::time::timeout_at(deadline, wait(Duration::ZERO)).await {
            Ok(Err(meerkat_mob::MobError::LifecycleOperationAdmissionPending {
                intent,
                stage,
            })) if intent == "member_status_observation"
                && stage == "observation_lane_saturated" =>
            {
                return IdentityMemberReadiness::TimedOut;
            }
            Ok(result) => result,
            Err(_) => return IdentityMemberReadiness::TimedOut,
        }
    } else {
        crate::member_status_observation::retry_member_status_observation(deadline, || {
            wait(deadline.saturating_duration_since(tokio::time::Instant::now()))
        })
        .await
    };
    match result {
        Ok(_) => IdentityMemberReadiness::Ready,
        Err(meerkat_mob::MobError::ActorCommandTimedOut {
            command_kind: "MemberStatus",
            stage: "member_status_observation",
        }) => IdentityMemberReadiness::TimedOut,
        Err(error) if crate::unified_runtime::mob_ops::is_ready_wait_timeout(&error) => {
            IdentityMemberReadiness::TimedOut
        }
        Err(error) => IdentityMemberReadiness::Failed(error.to_string()),
    }
}

struct IdentityStartupReadyWait {
    status: crate::identity_first::IdentityBootstrapStatus,
    timed_out: bool,
    startup_ready: bool,
}

/// Join identity bootstrap terminality and mob-member readiness at one
/// bootstrap generation. The readiness callback takes owned ids so production
/// can move them into the mob future while deterministic tests inject a gated
/// completion/error without mocking the full mob actor.
async fn wait_identity_startup_ready<F, Fut>(
    identity_rt: &crate::identity_first::IdentityRuntime,
    wait_timeout: Duration,
    mut wait_members: F,
) -> Result<IdentityStartupReadyWait, String>
where
    F: FnMut(Vec<meerkat_mob::ids::AgentIdentity>, Duration) -> Fut + Send,
    Fut: Future<Output = IdentityMemberReadiness> + Send,
{
    let started = std::time::Instant::now();
    let (mut status, mut timed_out, mut generation) = identity_rt
        .wait_identity_bootstrap_terminal_with_generation(wait_timeout)
        .await;
    loop {
        if timed_out || !status.ready {
            return Ok(IdentityStartupReadyWait {
                status,
                timed_out,
                startup_ready: false,
            });
        }

        // Subscribe before validating the terminal snapshot so a reconcile
        // that starts during member readiness wakes this loop instead of
        // combining observations from two different passes.
        let mut bootstrap_changes = identity_rt.subscribe_identity_bootstrap_status();
        let (current_generation, current_status) =
            identity_rt.identity_bootstrap_status_with_generation();
        if current_generation != generation || current_status != status {
            let remaining = wait_timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Ok(IdentityStartupReadyWait {
                    status: current_status,
                    timed_out: true,
                    startup_ready: false,
                });
            }
            (status, timed_out, generation) = identity_rt
                .wait_identity_bootstrap_terminal_with_generation(remaining)
                .await;
            continue;
        }

        let member_ids = identity_rt
            .identity_bootstrap_member_ids_for_status(&status)
            .await;
        let (mapped_generation, mapped_status) =
            identity_rt.identity_bootstrap_status_with_generation();
        if mapped_generation != generation || mapped_status != status {
            let remaining = wait_timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Ok(IdentityStartupReadyWait {
                    status: mapped_status,
                    timed_out: true,
                    startup_ready: false,
                });
            }
            (status, timed_out, generation) = identity_rt
                .wait_identity_bootstrap_terminal_with_generation(remaining)
                .await;
            continue;
        }
        if member_ids.len() != status.identities.len() {
            return Err("identity bootstrap readiness mapping is incomplete".to_string());
        }

        let remaining = wait_timeout.saturating_sub(started.elapsed());
        let readiness = wait_members(member_ids, remaining);
        tokio::pin!(readiness);
        let readiness_result = tokio::select! {
            result = &mut readiness => Some(result),
            changed = bootstrap_changes.changed() => {
                if changed.is_err() {
                    return Err(
                        "identity bootstrap readiness status channel closed".to_string(),
                    );
                }
                None
            }
        };
        let Some(readiness_result) = readiness_result else {
            let remaining = wait_timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                let (_, latest) = identity_rt.identity_bootstrap_status_with_generation();
                return Ok(IdentityStartupReadyWait {
                    status: latest,
                    timed_out: true,
                    startup_ready: false,
                });
            }
            (status, timed_out, generation) = identity_rt
                .wait_identity_bootstrap_terminal_with_generation(remaining)
                .await;
            continue;
        };

        // `select!` may choose a simultaneously-ready stale member result
        // even though the bootstrap-change branch is ready too. Validate the
        // generation before handling success, timeout, or error alike.
        let (result_generation, result_status) =
            identity_rt.identity_bootstrap_status_with_generation();
        if result_generation != generation || result_status != status {
            let remaining = wait_timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Ok(IdentityStartupReadyWait {
                    status: result_status,
                    timed_out: true,
                    startup_ready: false,
                });
            }
            (status, timed_out, generation) = identity_rt
                .wait_identity_bootstrap_terminal_with_generation(remaining)
                .await;
            continue;
        }
        match readiness_result {
            IdentityMemberReadiness::Ready => {
                let (ready_generation, ready_status) =
                    identity_rt.identity_bootstrap_status_with_generation();
                if ready_generation == generation && ready_status == status {
                    return Ok(IdentityStartupReadyWait {
                        status,
                        timed_out: false,
                        startup_ready: true,
                    });
                }
                let remaining = wait_timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Ok(IdentityStartupReadyWait {
                        status: ready_status,
                        timed_out: true,
                        startup_ready: false,
                    });
                }
                (status, timed_out, generation) = identity_rt
                    .wait_identity_bootstrap_terminal_with_generation(remaining)
                    .await;
            }
            IdentityMemberReadiness::TimedOut => {
                return Ok(IdentityStartupReadyWait {
                    status,
                    timed_out: true,
                    startup_ready: false,
                });
            }
            IdentityMemberReadiness::Failed(error) => {
                return Err(format!("identity bootstrap readiness failed: {error}"));
            }
        }
    }
}

pub fn handle_unified_rpc_json<'a>(
    runtime: &'a UnifiedRuntime,
    request_json: &'a str,
    timeout: Duration,
    http_base_url: Option<&'a str>,
    identity_ctx: Option<&'a IdentityFirstContext>,
) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
    handle_unified_rpc_json_with_live(
        runtime,
        request_json,
        timeout,
        http_base_url,
        identity_ctx,
        None,
    )
}

/// Runtime-owned JSON-RPC dispatch. Hosts that keep the unified runtime in an
/// [`Arc`] should use this entrypoint so cancellation-safe member mutations
/// can move a runtime owner into the identity foreground supervisor.
pub fn handle_unified_rpc_json_arc<'a>(
    runtime: &'a Arc<UnifiedRuntime>,
    request_json: &'a str,
    timeout: Duration,
    http_base_url: Option<&'a str>,
    identity_ctx: Option<&'a IdentityFirstContext>,
) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
    handle_unified_rpc_json_with_live_arc(
        runtime,
        request_json,
        timeout,
        http_base_url,
        identity_ctx,
        None,
    )
}

/// [`handle_unified_rpc_json`] plus the gateway's type-erased live handler
/// (`meta/live/*`). `None` keeps every live method answering the typed
/// `live_unavailable` error — the posture of an ephemeral gateway or a
/// deployment that did not opt into `runtime_options.live`.
pub fn handle_unified_rpc_json_with_live<'a>(
    runtime: &'a UnifiedRuntime,
    request_json: &'a str,
    timeout: Duration,
    http_base_url: Option<&'a str>,
    identity_ctx: Option<&'a IdentityFirstContext>,
    live: Option<&'a crate::live_wiring::LiveRpcHandler>,
) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
    Box::pin(handle_unified_rpc_json_inner(
        runtime,
        None,
        request_json,
        timeout,
        http_base_url,
        identity_ctx,
        live,
    ))
}

/// [`handle_unified_rpc_json_with_live`] with an owned runtime available to
/// cancellation-safe foreground operations.
pub fn handle_unified_rpc_json_with_live_arc<'a>(
    runtime: &'a Arc<UnifiedRuntime>,
    request_json: &'a str,
    timeout: Duration,
    http_base_url: Option<&'a str>,
    identity_ctx: Option<&'a IdentityFirstContext>,
    live: Option<&'a crate::live_wiring::LiveRpcHandler>,
) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
    Box::pin(handle_unified_rpc_json_inner(
        runtime.as_ref(),
        Some(runtime),
        request_json,
        timeout,
        http_base_url,
        identity_ctx,
        live,
    ))
}

/// Serialized RPC response plus any experimental live publication custody.
/// A transport owner settles this only after its actual write and flush
/// result is known.
pub struct SerializedRpcResponseDelivery {
    pub response: String,
    #[cfg(feature = "openai-live")]
    delivery: Option<crate::live_wiring::LiveRpcResponseDeliveryCustody>,
}

impl SerializedRpcResponseDelivery {
    #[must_use]
    pub fn plain(response: String) -> Self {
        Self {
            response,
            #[cfg(feature = "openai-live")]
            delivery: None,
        }
    }

    #[cfg(all(test, feature = "openai-live-test"))]
    pub(crate) fn with_delivery_for_test(
        response: String,
        delivery: meerkat::surface::LiveWebrtcAnswerDeliveryCustody,
    ) -> Self {
        Self {
            response,
            delivery: Some(
                crate::live_wiring::LiveRpcResponseDeliveryCustody::WebrtcAnswer(delivery),
            ),
        }
    }

    #[cfg(all(test, feature = "openai-live"))]
    pub(crate) fn with_open_delivery_for_test(
        response: String,
        delivery: crate::live_wiring::LiveOpenResponseDeliveryCustody,
    ) -> Self {
        Self {
            response,
            delivery: Some(crate::live_wiring::LiveRpcResponseDeliveryCustody::Open(
                delivery,
            )),
        }
    }

    /// Commit or reject live publication after the outer response write.
    pub async fn settle_delivery(&mut self, delivered: bool) -> Result<(), String> {
        #[cfg(feature = "openai-live")]
        if let Some(custody) = self.delivery.take() {
            return if delivered {
                custody.delivered().await
            } else {
                custody.rejected().await
            };
        }
        // Keep the public async contract identical when live custody is not
        // compiled in. The experimental branch above performs the real await.
        #[cfg(not(feature = "openai-live"))]
        std::future::ready(()).await;
        let _ = delivered;
        Ok(())
    }
}

impl Drop for SerializedRpcResponseDelivery {
    fn drop(&mut self) {
        #[cfg(feature = "openai-live")]
        drop(self.delivery.take());
    }
}

/// Delivery-aware variant used by an outer writer that can acknowledge the
/// actual response write instead of treating JSON encoding as publication.
pub fn handle_unified_rpc_json_with_live_arc_delivery<'a>(
    runtime: &'a Arc<UnifiedRuntime>,
    request_json: &'a str,
    timeout: Duration,
    http_base_url: Option<&'a str>,
    identity_ctx: Option<&'a IdentityFirstContext>,
    live: Option<&'a crate::live_wiring::LiveRpcHandler>,
) -> Pin<Box<dyn Future<Output = SerializedRpcResponseDelivery> + Send + 'a>> {
    Box::pin(async move {
        #[cfg(feature = "openai-live")]
        {
            let (response, mut delivery) = crate::live_wiring::capture_live_rpc_response_delivery(
                handle_unified_rpc_json_inner(
                    runtime.as_ref(),
                    Some(runtime),
                    request_json,
                    timeout,
                    http_base_url,
                    identity_ctx,
                    live,
                ),
            )
            .await;
            if response.is_empty()
                && let Some(custody) = delivery.take()
            {
                let _ = custody.rejected().await;
            }
            SerializedRpcResponseDelivery { response, delivery }
        }
        #[cfg(not(feature = "openai-live"))]
        SerializedRpcResponseDelivery {
            response: handle_unified_rpc_json_inner(
                runtime.as_ref(),
                Some(runtime),
                request_json,
                timeout,
                http_base_url,
                identity_ctx,
                live,
            )
            .await,
        }
    })
}

/// Typed error code for a read-only console arm that exhausted its deadline.
///
/// Continues the operator-verb block (`OPERATOR_SESSION_BUSY_CODE` -32015,
/// `OPERATOR_VERB_UNAVAILABLE_CODE` -32016). The `-32001..-32005` range is the
/// identity plane and `-32004` is the SDKs' reserved capability code, so the
/// read-timeout signal takes the next free code above the operator block.
pub const CONSOLE_READ_TIMEOUT_CODE: i64 = -32017;

/// Read budget for console/identity arms that cross the member's session task
/// or the mob actor loop.
///
/// Both seams are strict sequential command loops, so a member that is busy -
/// a long tool chain, a post-cycle compaction - makes every console read on
/// that member queue behind the turn with no signal at all. That is the production incident
/// shape (2026-08-16): `meta/identity/resolved_tools` hung past 60 seconds
/// and never completed, because a read has no way to say "the loop is busy".
///
/// Thirty seconds is deliberately generous. These arms take snapshot reads,
/// not turns, so a healthy one answers in milliseconds; a bound this loose
/// fires only when the loop is genuinely blocked rather than merely slow.
const CONSOLE_READ_BUDGET: Duration = Duration::from_secs(30);

/// Effective read budget: `META_CONSOLE_READ_TIMEOUT_SECS` overrides the
/// default, clamped to [1, 3600] seconds. The floor keeps `0` from failing
/// every read; the ceiling keeps a mistyped value from silently restoring the
/// unbounded wait this replaces (the `META_BRIDGE_ACTOR_ADMISSION_SECS`
/// idiom).
///
/// The dispatch `timeout` argument is deliberately NOT the source here: it is
/// the gateway's runtime-event drain budget reused as a dispatch parameter,
/// and callers pass values as low as one second. Deriving the read budget
/// from it would convert slow-but-working reads into typed failures.
fn console_read_budget() -> Duration {
    parse_console_read_budget(
        std::env::var("META_CONSOLE_READ_TIMEOUT_SECS")
            .ok()
            .as_deref(),
    )
}

fn parse_console_read_budget(raw: Option<&str>) -> Duration {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .map(|secs| Duration::from_secs(secs.clamp(1, 3600)))
        .unwrap_or(CONSOLE_READ_BUDGET)
}

/// Bound a READ-ONLY console arm, converting a silent queue-behind-the-turn
/// hang into a typed error naming the arm and the seam it was awaiting.
///
/// Only pure reads may route through here. Abandoning the future drops the
/// reply oneshot of an in-flight `send_actor_command` or session-service read.
/// Meerkat skips queued member-list and member-status observations whose reply
/// receiver has closed; an observation already executing may finish, and other
/// pure reads may still execute before their reply send fails. Mutation and
/// lifecycle arms - and reads that run inside a member authority transaction,
/// such as `cross_mob/peer_info` - keep their own semantics and MUST NOT be
/// wrapped.
async fn with_read_deadline<F>(
    arm: &'static str,
    awaiting: &'static str,
    response_id: Value,
    fut: F,
) -> JsonRpcResponse
where
    F: Future<Output = JsonRpcResponse>,
{
    let budget = console_read_budget();
    match tokio::time::timeout(budget, fut).await {
        Ok(response) => response,
        Err(_) => console_read_timeout_response(arm, awaiting, budget, response_id),
    }
}

fn console_read_timeout_response(
    arm: &'static str,
    awaiting: &'static str,
    budget: Duration,
    response_id: Value,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: CONSOLE_READ_TIMEOUT_CODE,
            message: format!(
                "{arm} timed out after {}s awaiting {awaiting}; the member's session task or \
                 the mob actor loop may be mid-turn, and both are strict sequential command \
                 loops - reads queue behind a running turn rather than degrading",
                budget.as_secs()
            ),
            data: Some(serde_json::json!({
                "kind": "console_read_timeout",
                "arm": arm,
                "awaiting": awaiting,
                "timeout_secs": budget.as_secs(),
            })),
        }),
    }
}

/// The methods a candidate launch
/// (`runtime_options.mob_composition.authority = "candidate"`) refuses with
/// `-32047` `candidate_launch`, on every RPC surface that serves them. Each
/// retires a stored member, respawns or resets it, recomputes the roster or
/// rewires members, which is the authoritative launch's to change; decides a
/// pending gate, whose decision resumes the authoritative launch's work;
/// creates a session (a spawn, an ensure, a helper, a fork or an attach),
/// which nothing holds and whose turns could reach a held member through
/// peer comms; or admits input to a member or acts on its runs and session. A member input
/// admitted on a candidate would be served by a registration the candidate's
/// shutdown must then stop, ending the stored work with it, so a candidate
/// admits none. The operator mutations follow: each writes state the
/// authoritative launch reads from the shared state directory (routes,
/// memory, labels, the Console's access policy and session fences, blobs,
/// flow runs, jobs), invokes a tool or starts a monitor, or drives a live
/// channel, which a candidate opens none of. (`meta/reconcile_edges` is not
/// refused: a candidate answers it with a dry run. WorkGraph writes and the
/// host-executing arms of other methods are refused by
/// [`candidate_launch_refuses`].)
pub(crate) const CANDIDATE_LAUNCH_REFUSED_METHODS: &[&str] = &[
    "meta/retire",
    "meta/retire_member",
    "meta/respawn",
    "meta/respawn_member",
    "meta/reset",
    "meta/reset_all",
    "meta/delete_identity",
    "meta/reconcile_identity",
    "meta/rediscover",
    "meta/cross_mob/wire",
    "meta/cross_mob/unwire",
    "meta/cross_mob/wire_local",
    "meta/cross_mob/unwire_local",
    "meta/topology/apply",
    "meta/gating/decide",
    "meta/spawn_member",
    "meta/ensure_member",
    "meta/spawn_helper",
    "meta/fork_helper",
    "meta/attach_existing_session",
    "meta/send",
    "meta/send_message",
    "meta/interact",
    "meta/dispatch",
    "meta/delivery/send",
    "meta/cross_mob/send",
    "meta/run_flow",
    "meta/reload_member",
    "meta/stop_member_run",
    "meta/force_cancel_member",
    "meta/compact_member",
    "meta/member_activate_instruction",
    "meta/live/open",
    "meta/console/send",
    crate::console_voice::VOICE_OPEN_METHOD,
    "meta/console/recover_send",
    // Operator mutations.
    "meta/call_tool",
    "meta/routing/routes/add",
    "meta/routing/routes/delete",
    "meta/memory/index",
    "meta/memory/quarantine/decide",
    "meta/agent_memory/remember",
    "meta/agent_memory/update",
    "meta/agent_memory/forget",
    "meta/mob_labels/set",
    "meta/mob_labels/delete",
    "meta/run_labels/set",
    "meta/run_labels/delete",
    "meta/access/set",
    "meta/access/enable",
    "meta/access/rules/upsert",
    "meta/access/rules/delete",
    "meta/access/groups/set",
    "meta/access/groups/delete",
    CONSOLE_REVOKE_SESSIONS_METHOD,
    "meta/blob/upload",
    "meta/cancel_flow",
    "jobs/cancel",
    "jobs/retry",
    "monitors/start",
    "meta/live/close",
    "meta/live/refresh",
    "meta/live/send_input",
    "meta/live/commit_input",
    "meta/live/interrupt",
    "meta/live/truncate",
    "meta/live/playback_complete",
    "meta/live/replacement_required",
    "meta/live/playback_owner/register",
    "meta/live/playback_owner/revoke",
    "live/webrtc/answer",
    // Starts the configured modules not loaded yet: external processes whose
    // start can act.
    "meta/reconcile",
];

/// Whether a candidate launch whose state directory is `state_dir` refuses
/// `method` with `params`: one of [`CANDIDATE_LAUNCH_REFUSED_METHODS`]; a
/// member declaration mutation (an identity adoption or a tool declaration
/// apply, [`mob_methods::is_member_declaration_mutating_method`]), which
/// writes durable member intent and schedules the identity convergence that
/// can replace a stored member; a WorkGraph write
/// ([`workgraph_methods::is_workgraph_mutating_method`]); the
/// host-executing or externally writing arm of a method a candidate
/// otherwise serves ([`candidate_launch_refuses_host_execution`]); or a
/// Flow Editor draft store write the authoritative launch reads
/// ([`candidate_launch_refuses_draft_store_write`]). Reads still answer:
/// the declaration read, the WorkGraph reads, the draft reads, a deploy
/// plan, a structural validation and a BigQuery read among them.
pub(crate) fn candidate_launch_refuses(
    method: &str,
    params: &Value,
    state_dir: Option<&std::path::Path>,
) -> bool {
    CANDIDATE_LAUNCH_REFUSED_METHODS.contains(&method)
        || mob_methods::is_member_declaration_mutating_method(method)
        || workgraph_methods::is_workgraph_mutating_method(method)
        || candidate_launch_refuses_host_execution(method, params)
        || candidate_launch_refuses_draft_store_write(method, params, state_dir)
}

/// The Flow Editor draft store methods that write the store.
const MOBPACK_DRAFT_STORE_WRITE_METHODS: &[&str] = &[
    "meta/mobpacks/create",
    "meta/mobpacks/save",
    "meta/mobpacks/delete",
    "meta/mobpacks/undo",
    "meta/mobpacks/redo",
];

/// Whether a candidate launch refuses a Flow Editor draft store write: every
/// one but a write to a store file the caller names (`store_path`) outside
/// the launch's `state_dir` and the gateway home that is not the store
/// `META_FLOW_EDITOR_DRAFT_STORE` names
/// ([`draft_store_write_reaches_a_read_store`]).
fn candidate_launch_refuses_draft_store_write(
    method: &str,
    params: &Value,
    state_dir: Option<&std::path::Path>,
) -> bool {
    MOBPACK_DRAFT_STORE_WRITE_METHODS.contains(&method)
        && draft_store_write_reaches_a_read_store(
            params,
            state_dir,
            crate::mobpack::env_draft_store_path().as_deref(),
        )
}

/// Whether a draft write with `params` reaches a store the authoritative
/// launch reads: the store a write names no file for (`env_store`, the one
/// `META_FLOW_EDITOR_DRAFT_STORE` names, or else the gateway home's), the
/// `env_store` file itself however a `store_path` spells it, and anything
/// under the launch's `state_dir` or the gateway home. Paths are compared as
/// the file system resolves them. A named path that climbs (`..`) reaches
/// one as well, since where it lands is not read here.
fn draft_store_write_reaches_a_read_store(
    params: &Value,
    state_dir: Option<&std::path::Path>,
    env_store: Option<&std::path::Path>,
) -> bool {
    let Some(named) = params
        .get("store_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(std::path::Path::new)
    else {
        return true;
    };
    if named
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return true;
    }
    let named = resolved_host_path(named);
    env_store.is_some_and(|store| named == resolved_host_path(store))
        || state_dir
            .map(std::path::Path::to_path_buf)
            .into_iter()
            .chain(crate::storage_layout::derived_gateway_home().ok())
            .any(|root| named.starts_with(resolved_host_path(&root)))
}

/// `path` as the file system resolves it, as far as it exists: its longest
/// existing ancestor canonicalized, with the rest appended, and a relative
/// path taken from the working directory.
fn resolved_host_path(path: &std::path::Path) -> std::path::PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    let mut existing = absolute.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(existing) {
            return rest
                .iter()
                .rev()
                .fold(canonical, |resolved, part| resolved.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => return absolute,
        }
    }
}

/// The arms of methods a candidate otherwise serves that run something on
/// the host or write outside it: `meta/mobpacks/deploy` with `execute`
/// (writes a mobpack archive and runs `rkat mob run`),
/// `meta/mobpacks/validate` with `rkat_validate` (writes an archive and runs
/// `rkat mob validate`), `meta/session_store/bigquery`'s row insert, and a
/// `meta/gating/evaluate` of an R3 action that names an approval route
/// (`approval_recipient` and `approval_channel`), which sends the approval
/// request through the delivery module. Each is decided on the request as
/// its handler parses it.
fn candidate_launch_refuses_host_execution(method: &str, params: &Value) -> bool {
    let flag = |name: &str| params.get(name).and_then(Value::as_bool).unwrap_or(false);
    match method {
        "meta/mobpacks/deploy" => flag("execute"),
        "meta/mobpacks/validate" => flag("rkat_validate") || flag("rkatValidate"),
        "meta/gating/evaluate" => parse_gating_evaluate_params(params).is_ok_and(|request| {
            let named = |field: Option<&str>| field.is_some_and(|value| !value.trim().is_empty());
            request.risk_tier == GatingRiskTier::R3
                && named(request.approval_recipient.as_deref())
                && named(request.approval_channel.as_deref())
        }),
        "meta/session_store/bigquery" => matches!(
            parse_bigquery_session_store_operation(params),
            Ok(BigQuerySessionStoreOperation::StreamInsert)
        ),
        _ => false,
    }
}

/// The typed refusal of `meta/mobpacks/deploy` with `execute: true` on a
/// runtime whose agent factories hold a host-chosen provider credential
/// store, before anything is written or run: the deploy writes a mobpack
/// archive and runs `rkat mob run`, a process of its own that would resolve
/// provider credentials in the process default store instead. `-32004` with
/// `data.kind = "provider_credentials_not_carried"`. `None` without such a
/// store, and for every other request (deploy plans included).
pub(crate) fn explicit_provider_credentials_refusal(
    explicit_provider_credentials: bool,
    method: &str,
    params: &Value,
    response_id: Value,
) -> Option<JsonRpcResponse> {
    let executes = method == "meta/mobpacks/deploy"
        && params
            .get("execute")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if !(explicit_provider_credentials && executes) {
        return None;
    }
    Some(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: CAPABILITY_UNAVAILABLE_CODE,
            message: format!(
                "{method} with execute is refused: this runtime keeps provider credentials in a \
                 store its host chose, and the rkat process a host deploy runs would resolve \
                 them in the process default store instead; the deploy plan (execute: false) \
                 still answers"
            ),
            data: Some(serde_json::json!({ "kind": "provider_credentials_not_carried" })),
        }),
    })
}

/// A candidate launch's typed refusal of `method` with `params`, before
/// anything is read or written: `-32047` with `data.kind =
/// "candidate_launch"`, the refusal a candidate answers for a mob
/// announcement change. `None` for a launch that speaks for its
/// composition, and for a method a candidate serves
/// ([`candidate_launch_refuses`], over the launch's `state_dir`).
pub(crate) fn candidate_launch_refusal(
    speaks_for_composition: bool,
    state_dir: Option<&std::path::Path>,
    method: &str,
    params: &Value,
    response_id: Value,
) -> Option<JsonRpcResponse> {
    if speaks_for_composition || !candidate_launch_refuses(method, params, state_dir) {
        return None;
    }
    Some(candidate_launch_refused(method, response_id))
}

/// A candidate launch's typed refusal of `method`: `-32047` with
/// `data.kind = "candidate_launch"`. The refusal of a session revocation
/// names where to revoke instead.
fn candidate_launch_refused(method: &str, response_id: Value) -> JsonRpcResponse {
    let message = if method == CONSOLE_REVOKE_SESSIONS_METHOD {
        format!(
            "{method} is refused: this gateway is a candidate launch, which writes no session \
             fence; revoke the sessions on the authoritative launch, whose fence record this \
             launch's Console reads"
        )
    } else {
        format!(
            "{method} is refused: this gateway is a candidate launch, and its stored members' \
             roster and work, and the state it shares with the authoritative launch, belong \
             to that launch; reads still answer"
        )
    };
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: crate::mob_announcements::rpc::ANNOUNCEMENTS_REFUSED_CODE,
            message,
            data: Some(serde_json::json!({ "kind": "candidate_launch" })),
        }),
    }
}

async fn handle_unified_rpc_json_inner(
    runtime: &UnifiedRuntime,
    runtime_owner: Option<&Arc<UnifiedRuntime>>,
    request_json: &str,
    timeout: Duration,
    http_base_url: Option<&str>,
    identity_ctx: Option<&IdentityFirstContext>,
    live: Option<&crate::live_wiring::LiveRpcHandler>,
) -> String {
    let raw_request: Value = match serde_json::from_str(request_json) {
        Ok(raw_request) => raw_request,
        Err(_) => {
            return serialize_response(&JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Value::Null,
                result: None,
                error: Some(JsonRpcError {
                    code: -32700,
                    message: "Parse error".to_string(),
                    data: None,
                }),
            });
        }
    };
    let response_id = raw_request
        .as_object()
        .and_then(|object| object.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    let request: JsonRpcRequest = match serde_json::from_value(raw_request) {
        Ok(request) => request,
        Err(_) => {
            return serialize_response(&JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32600,
                    message: "Invalid Request".to_string(),
                    data: None,
                }),
            });
        }
    };
    let is_notification = request.id.is_none();
    let response_id = request.id.clone().unwrap_or(Value::Null);

    if request.jsonrpc != "2.0" {
        let response = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32600,
                message: "Invalid Request".to_string(),
                data: None,
            }),
        };
        return if is_notification {
            String::new()
        } else {
            serialize_response(&response)
        };
    }

    if let Err(message) =
        crate::member_comms_id::validate_public_rpc_member_aliases(&request.params)
    {
        let response = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: format!("Invalid params: {message}"),
                data: None,
            }),
        };
        return if is_notification {
            String::new()
        } else {
            serialize_response(&response)
        };
    }

    let speaks_for_composition = runtime.speaks_for_composition();
    let state_dir = if speaks_for_composition {
        None
    } else {
        runtime
            .resolved_storage()
            .and_then(|storage| storage.state_dir)
    };
    if let Some(response) = candidate_launch_refusal(
        speaks_for_composition,
        state_dir.as_deref(),
        request.method.as_str(),
        &request.params,
        response_id.clone(),
    ) {
        return if is_notification {
            String::new()
        } else {
            serialize_response(&response)
        };
    }

    if let Some(response) = live_open_execution_identity_preflight(
        request.method.as_str(),
        &request.params,
        response_id.clone(),
        live.is_some_and(crate::live_wiring::LiveRpcHandler::supports_live_execution_identity_v1),
    ) {
        return if is_notification {
            String::new()
        } else {
            serialize_response(&response)
        };
    }
    if let Some(response) = experimental_live_target_preflight(
        request.method.as_str(),
        &request.params,
        response_id.clone(),
    ) {
        return if is_notification {
            String::new()
        } else {
            serialize_response(&response)
        };
    }

    let response = match request.method.as_str() {
        "meta/status" => {
            // The last phase the mob's actor published, and, once the actor
            // has stopped, why: a fail-stopped actor keeps its last live
            // phase, so the phase alone reads a stopped mob as running.
            let observation = runtime.mob_handle().lifecycle_observation();
            let mob_state = Some(observation.phase);
            let is_running = runtime.module_is_running().await;
            let loaded = runtime.loaded_modules().await;
            let mut result = serde_json::json!({
                "contract_version": META_CONTRACT_VERSION,
                "running": is_running,
                "loaded_modules": loaded,
                "mob_state": format!("{mob_state:?}"),
            });
            if let Some(failure) = &observation.failure {
                result["mob_failure"] = Value::String(failure.to_string());
            }
            if let Some(url) = http_base_url {
                result["http_base_url"] = Value::String(url.to_string());
            }
            if let Some(ctx) = identity_ctx {
                result["identity_bootstrap"] =
                    serde_json::to_value(ctx.runtime.identity_bootstrap_status())
                        .unwrap_or(Value::Null);
            }
            if let Some(storage) = runtime.resolved_storage() {
                result["storage"] = storage.status_json();
            }
            if let Some(job_health) = runtime.job_health_projection() {
                result["detached_jobs"] = job_health
                    .get("detached_jobs")
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            if let Some(announcements) = runtime.mob_announcements() {
                result["mob_announcements"] =
                    crate::mob_announcements::rpc::status_projection(&announcements).await;
            }
            // Members created under one of the console's runtime-plane
            // identities before every creation path refused them.
            if let Some(reserved) =
                crate::member_comms_id::reserved_identity_members_status(&runtime.mob_handle())
                    .await
            {
                result["reserved_identity_members"] = reserved;
            }
            // Members Core holds broken (it could not restore them): a
            // readiness probe's cue, as `/healthz` reports them.
            let broken_members = runtime.broken_members().await;
            if !broken_members.is_empty() {
                result["broken_members"] =
                    crate::unified_runtime::http::broken_members_json(&broken_members);
            }
            // Principals the Console accepted a token without `exp` for:
            // the principal and the flag, never a token.
            if let Some(non_expiring) =
                non_expiring_console_principals_status(runtime.console_session_fences())
            {
                result["non_expiring_console_principals"] = non_expiring;
            }
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(result),
                error: None,
            }
        }
        CONSOLE_REVOKE_SESSIONS_METHOD => {
            use crate::auth::session_fence::ConsoleSessionFencesError;
            let refusal = |code: i64, message: String| JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code,
                    message,
                    data: None,
                }),
            };
            match request.params.get("principal").and_then(Value::as_str) {
                None => refusal(
                    -32602,
                    "Invalid params: principal must be a string".to_string(),
                ),
                Some(principal) => match runtime
                    .console_session_fences()
                    .revoke_sessions(principal, std::time::SystemTime::now())
                {
                    Ok(fence) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id.clone(),
                        result: Some(console_session_fence_json(&fence)),
                        error: None,
                    },
                    Err(ConsoleSessionFencesError::InvalidPrincipal) => refusal(
                        -32602,
                        "Invalid params: principal must not be empty".to_string(),
                    ),
                    Err(error @ ConsoleSessionFencesError::NotDurable) => {
                        refusal(CAPABILITY_UNAVAILABLE_CODE, error.to_string())
                    }
                    // The revocation took effect; only its durability is not
                    // confirmed, which the message says.
                    Err(error @ ConsoleSessionFencesError::DurabilityUnconfirmed { .. }) => {
                        refusal(-32603, error.to_string())
                    }
                    Err(error) => refusal(
                        -32603,
                        format!("console session revocation failed: {error}"),
                    ),
                },
            }
        }
        "meta/capabilities" => {
            let loaded = runtime.loaded_modules().await;
            let mut methods = vec![
                "meta/init",
                "meta/status",
                "meta/capabilities",
                "meta/reconcile",
                "meta/spawn_member",
                "meta/routing/resolve",
                "meta/routing/routes/list",
                "meta/routing/routes/add",
                "meta/routing/routes/delete",
                "meta/delivery/send",
                "meta/delivery/history",
                "meta/events/subscribe",
                "meta/query_events",
                "meta/memory/stores",
                "meta/memory/index",
                "meta/memory/query",
                "meta/session_store/bigquery",
                "meta/gating/evaluate",
                "meta/gating/pending",
                "meta/gating/decide",
                "meta/gating/audit",
                "meta/call_tool",
                "meta/models/catalog",
                schedule_methods::SCHEDULE_TOOLS_METHOD,
                "meta/blob/get",
                "meta/send_message",
                "meta/find_members",
                "meta/ensure_member",
                "meta/list_members",
                "meta/get_member",
                "meta/member_health",
                "meta/retire_member",
                "meta/respawn_member",
                "meta/reload_member",
                "meta/reconcile_edges",
                "meta/rediscover",
                "meta/mob_events/query",
                "meta/mob_events/subscribe",
                // Always available: local-only member introspection
                "meta/cross_mob/peer_info",
                "meta/cross_mob/wire_local",
                "meta/cross_mob/unwire_local",
                "meta/peer_pubkey",
                "meta/member_status",
                "meta/identity/resolved_tools",
                "meta/identity/routing_status",
                "meta/force_cancel_member",
                "meta/stop_member_run",
                "meta/spawn_helper",
                "meta/fork_helper",
                "meta/attach_existing_session",
                "meta/cancel_flow",
                "meta/flow_status",
                "meta/list_flows",
                "meta/list_runs",
                "meta/run_flow",
                "meta/collect_completed",
                "meta/wait_ready",
                "meta/mob_labels/set",
                "meta/mob_labels/get",
                "meta/mob_labels/delete",
                "meta/run_labels/set",
                "meta/run_labels/get",
                "meta/run_labels/delete",
                storage_methods::STORAGE_DOCTOR_METHOD,
                CONSOLE_REVOKE_SESSIONS_METHOD,
            ];
            methods.extend_from_slice(MOBPACK_AUTHORING_METHODS);
            // DERIVED from the canonical registry, never listed. #343 listed these
            // literally here and in the dispatch; 0.8.21 then shipped them dead on
            // the console plane. A derived list cannot disagree with the dispatcher
            // about membership.
            //
            // NOT added to `handle_meta_rpc_json`'s list: that surface takes a
            // `MetaRuntimeHandle` (the module runtime) and has no `MobHandle`, so
            // it cannot SERVE these. Advertising what a surface cannot serve is the
            // same defect as serving what it does not advertise.
            methods.extend_from_slice(mob_methods::MEMBER_DECLARATION_METHODS);
            // Workgraph methods are advertised only when the service is
            // configured (`runtime_options.workgraph = false` or a failed
            // store open leave them off).
            let workgraph_configured = runtime.workgraph_service().is_some();
            if workgraph_configured {
                methods.extend_from_slice(workgraph_methods::WORKGRAPH_READ_METHODS);
                methods.extend_from_slice(workgraph_methods::WORKGRAPH_MUTATE_METHODS);
            }
            // Mob announcement methods are advertised only when the host
            // enabled them.
            if runtime.mob_announcements().is_some() {
                methods.extend_from_slice(crate::mob_announcements::rpc::ANNOUNCEMENT_METHODS);
            }
            // Live methods are advertised only when the gateway attached a
            // live transport (`runtime_options.live`, persistent mode).
            if live.is_some() {
                methods.extend_from_slice(&[
                    "meta/live/open",
                    "meta/live/status",
                    "meta/live/close",
                    "meta/live/refresh",
                    "meta/live/send_input",
                    "meta/live/commit_input",
                    "meta/live/interrupt",
                ]);
                if live.is_some_and(
                    crate::live_wiring::LiveRpcHandler::supports_live_execution_identity_v1,
                ) {
                    #[cfg(feature = "openai-live")]
                    methods.extend_from_slice(&[
                        "meta/live/replacement_required",
                        "meta/live/playback_owner/register",
                        "meta/live/playback_owner/revoke",
                        "meta/live/truncate",
                        "meta/live/playback_complete",
                        meerkat_live::LIVE_WEBRTC_ANSWER_METHOD,
                    ]);
                }
            }
            if identity_ctx.is_some() {
                methods.extend_from_slice(&[
                    "meta/send",
                    "meta/send_status",
                    "meta/registered_spec",
                    "meta/interact",
                    "meta/dispatch",
                    "meta/recover_delivery",
                    "meta/subscribe",
                    "meta/status_identity",
                    "meta/respawn",
                    "meta/retire",
                    "meta/reset",
                    "meta/delete_identity",
                    "meta/inspect_identity",
                    "meta/completion_cursor",
                    "meta/wait_for_completion",
                    "meta/turn_result",
                    "meta/wait_for_turn",
                    "meta/compact_member",
                    "meta/bound_member_transcript",
                    "meta/member_activate_instruction",
                    "meta/member_instruction_activations",
                    "meta/reconcile_identity",
                    "meta/status_identity_bootstrap",
                    "meta/wait_identity_bootstrap",
                ]);
            }
            if identity_ctx
                .and_then(|ctx| ctx.agent_memory_provider.as_ref())
                .is_some()
            {
                methods.push("meta/agent_memory/recall");
                if identity_ctx
                    .and_then(|ctx| ctx.agent_memory_provider.as_ref())
                    .is_some_and(|provider| provider.supports_remember())
                {
                    methods.push("meta/agent_memory/remember");
                }
                if identity_ctx
                    .and_then(|ctx| ctx.agent_memory_provider.as_ref())
                    .is_some_and(|provider| provider.supports_forget())
                {
                    methods.push("meta/agent_memory/forget");
                }
                if identity_ctx
                    .and_then(|ctx| ctx.agent_memory_provider.as_ref())
                    .is_some_and(|provider| provider.supports_supersede())
                {
                    methods.push("meta/agent_memory/update");
                }
                if identity_ctx
                    .and_then(|ctx| ctx.agent_memory_provider.as_ref())
                    .is_some_and(|provider| provider.supports_manifest())
                {
                    methods.push("meta/agent_memory/manifest");
                }
            }
            // Cross-mob directory always advertised when configured
            if runtime.has_contact_directory() {
                methods.push("meta/cross_mob/directory");
            }
            // High-level wire/unwire/send are reachable through two shapes:
            // same-process peers (registered handles + inproc contacts) and
            // cross-process peers (TCP/UDS contact entries served by the
            // remote gateway's control listener).
            if (runtime.has_peer_mob_handles().await && runtime.has_inproc_contacts())
                || runtime.has_remote_contacts()
            {
                methods.extend_from_slice(&[
                    "meta/cross_mob/wire",
                    "meta/cross_mob/unwire",
                    "meta/cross_mob/send",
                ]);
            }
            let job_health = runtime.job_health_projection();
            if job_health.is_some() {
                methods.extend_from_slice(&[
                    "jobs/get",
                    "jobs/list",
                    "jobs/cancel",
                    "jobs/progress",
                    "jobs/result",
                    "jobs/artifacts",
                    "jobs/retry",
                    "jobs/health",
                    "jobs/subscribe",
                    "jobs/unsubscribe",
                ]);
                if job_health
                    .as_ref()
                    .and_then(|projection| projection.get("monitors_available"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    methods.push("monitors/start");
                }
            }
            let topology = runtime.topology_runtime_handle();
            let (topology_methods, topology_capabilities) =
                topology_methods::capability_projection(&topology, None, false);
            methods.extend(topology_methods);
            // H1/H2 storage durability resolution — same object as
            // `meta/status`; `null` when the spec was composed externally
            // without a declaration.
            let storage = runtime
                .resolved_storage()
                .map(|summary| summary.status_json())
                .unwrap_or(Value::Null);
            let mut feature_capabilities = live
                .map(crate::live_wiring::LiveRpcHandler::feature_capabilities)
                .unwrap_or_default();
            if runtime.mob_announcements().is_some() {
                for name in crate::mob_announcements::rpc::announcement_feature_capabilities() {
                    match crate::live_contracts::FeatureCapability::new(name) {
                        Ok(capability) => feature_capabilities.push(capability),
                        // The names are non-empty constants: a refusal is a
                        // defect to see, not a capability to drop unseen.
                        Err(error) => tracing::error!(
                            error,
                            name,
                            "mob announcement feature capability refused"
                        ),
                    }
                }
            }
            // `meta/run_flow` accepts `role_targets`. A client checks for this
            // before sending them: a server without it ignores the binding
            // and runs the flow role-wide.
            // It also accepts `idempotency_key`; a server without this
            // capability ignores the key, so a retry admits a second run.
            for name in [
                crate::flow_role_targets::FLOW_ROLE_TARGETS_FEATURE,
                crate::flow_role_targets::FLOW_RUN_IDEMPOTENCY_KEY_FEATURE,
            ] {
                match crate::live_contracts::FeatureCapability::new(name) {
                    Ok(capability) => feature_capabilities.push(capability),
                    Err(error) => {
                        tracing::error!(error, name, "flow run feature capability refused");
                    }
                }
            }
            // A keyed `meta/send` accepts `handling_mode`. A client checks
            // for this before sending a keyed steer: a server without it
            // ignores the mode and commits the content as a queued send.
            if identity_ctx.is_some() {
                match crate::live_contracts::FeatureCapability::new(
                    KEYED_SEND_HANDLING_MODE_FEATURE,
                ) {
                    Ok(capability) => feature_capabilities.push(capability),
                    Err(error) => tracing::error!(
                        error,
                        name = KEYED_SEND_HANDLING_MODE_FEATURE,
                        "keyed send feature capability refused"
                    ),
                }
            }
            let announcement_capabilities = crate::mob_announcements::rpc::capabilities_projection(
                runtime.mob_announcements().as_ref(),
            )
            .await;
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({
                    "contract_version": META_CONTRACT_VERSION,
                    // The live atoms in this projection revalidate upstream
                    // Gate0/operator/realm/factory qualification: it carries
                    // no live atom unless the same live registration carries
                    // open authority and its mechanically derived bound-ready
                    // binder beside the sealed WebRTC answer transport. It
                    // also names the non-live features this gateway serves
                    // (mob announcement sources, `flow_role_targets`).
                    "feature_capabilities": feature_capabilities,
                    // Mob announcement sources, as this gateway runs them;
                    // null when announcements are disabled.
                    "mob_announcements": announcement_capabilities,
                    "runtime_type": "unified",
                    "methods": methods,
                    "storage": storage,
                    // Doctrine flag: when true the identity RPC set is live
                    // and member RPCs route durable targets through the
                    // identity authority.
                    "identity_first": identity_ctx.is_some(),
                    // True when a WorkGraph service is configured and the
                    // meta/workgraph/* group is live.
                    "workgraph": workgraph_configured,
                    // Bootstrap migration of member-bound attention bindings
                    // into their mob realm (null without a WorkGraph service).
                    "workgraph_realm_migration": runtime
                        .workgraph_realm_migration()
                        .map(|report| serde_json::to_value(report.as_ref()).unwrap_or(Value::Null))
                        .unwrap_or(Value::Null),
                    "detached_jobs": job_health
                        .as_ref()
                        .and_then(|projection| projection.get("detached_jobs"))
                        .cloned()
                        .unwrap_or(Value::Null),
                    "loaded_modules": loaded,
                    "runtime_capabilities": {
                        "can_spawn_members": true,
                        "can_send_messages": true,
                        "can_wire_members": true,
                        "can_retire_members": true,
                        "available_spawn_modes": ["module", "profile"],
                    },
                    "authoring_capabilities": mobpack_authoring_capabilities(),
                    "topology_control": topology_capabilities,
                })),
                error: None,
            }
        }
        topology_methods::TOPOLOGY_QUERY_METHOD => {
            let topology = runtime.topology_runtime_handle();
            topology_methods::handle_query(&topology, response_id, None, false).await
        }
        topology_methods::TOPOLOGY_PLAN_METHOD => {
            let topology = runtime.topology_runtime_handle();
            topology_methods::handle_plan(&topology, response_id, &request.params, None).await
        }
        topology_methods::TOPOLOGY_APPLY_METHOD => {
            let topology = runtime.topology_runtime_handle();
            topology_methods::handle_apply(
                &topology,
                response_id,
                &request.params,
                None,
                Some("local-host"),
                &crate::topology_control::TopologyApplyScope::LocalHost,
            )
            .await
        }
        topology_methods::TOPOLOGY_OPERATION_METHOD => {
            let topology = runtime.topology_runtime_handle();
            topology_methods::handle_operation(&topology, response_id, &request.params, None).await
        }
        topology_methods::TOPOLOGY_AUDIT_METHOD => {
            let topology = runtime.topology_runtime_handle();
            topology_methods::handle_audit(&topology, response_id, &request.params, None).await
        }
        "meta/reconcile" => {
            let modules = match params::required_string_array(&request.params, "modules") {
                Ok(m) => m,
                Err(reason) => {
                    return serialize_response(&JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {reason}"),
                            data: None,
                        }),
                    });
                }
            };

            match runtime.reconcile_modules(modules.clone(), timeout).await {
                Ok(added) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "accepted": true,
                        "reconciled_modules": modules,
                        "added": added
                    })),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {err:?}"),
                        data: None,
                    }),
                },
            }
        }
        "meta/spawn_member" => {
            // Support both legacy module_id pattern and mob profile+meerkat_id pattern
            let module_id = request.params.get("module_id").and_then(Value::as_str);
            let profile = request.params.get("profile").and_then(Value::as_str);
            let meerkat_id = request.params.get("meerkat_id").and_then(Value::as_str);

            if let Some(module_id) = module_id {
                // Legacy module spawn: {"module_id": "routing"}
                if module_id.is_empty() {
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: "Invalid params: module_id required".to_string(),
                            data: None,
                        }),
                    }
                } else {
                    match runtime.spawn_member(module_id, timeout).await {
                        Ok(()) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: Some(serde_json::json!({
                                "accepted": true,
                                "module_id": module_id
                            })),
                            error: None,
                        },
                        Err(err) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32602,
                                message: format!("Invalid params: {err:?}"),
                                data: None,
                            }),
                        },
                    }
                }
            } else if let (Some(profile), Some(meerkat_id)) = (profile, meerkat_id) {
                // The raw-target check refuses the reserved namespaces, the
                // console's runtime-plane identities among them.
                let target_identity_runtime = identity_ctx
                    .map(|ctx| ctx.runtime.runtime())
                    .or_else(|| runtime.identity_runtime_ref());
                let raw_target_validation = crate::member_comms_id::validate_raw_member_target(
                    target_identity_runtime,
                    meerkat_id,
                )
                .await;
                if let Err(message) = raw_target_validation.as_ref() {
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {message}"),
                            data: None,
                        }),
                    }
                } else {
                    // `runtime.spawn` takes the attached identity runtime's
                    // alias lock. Compatibility callers may instead supply an
                    // explicit identity context to a runtime without one; keep
                    // that authority's reservation through the lower-plane
                    // spawn without double-locking attached runtimes.
                    let compatibility_reservation = if runtime.identity_runtime_ref().is_none() {
                        crate::member_comms_id::reserve_raw_member_target(
                            target_identity_runtime,
                            meerkat_id,
                        )
                        .await
                        .map(Some)
                    } else {
                        Ok(None)
                    };
                    match compatibility_reservation {
                        Err(message) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32602,
                                message: format!("Invalid params: {message}"),
                                data: None,
                            }),
                        },
                        Ok(compatibility_reservation) => {
                            let member_id = compatibility_reservation
                                .as_ref()
                                .map(|reservation| reservation.alias().to_string())
                                .or_else(|| raw_target_validation.as_ref().ok().cloned())
                                .unwrap_or_else(|| meerkat_id.trim().to_string());
                            // Mob agent spawn: {"profile": "default", "meerkat_id": "agent-1"}
                            let spec = meerkat_mob::SpawnMemberSpec::from_wire(
                                profile.to_string(),
                                member_id.clone(),
                                request
                                    .params
                                    .get("initial_message")
                                    .and_then(Value::as_str)
                                    .map(|s| meerkat_core::ContentInput::from(s.to_string())),
                                None,
                                None,
                            );
                            let spawn_result = Box::pin(runtime.spawn(spec)).await;
                            drop(compatibility_reservation);
                            match spawn_result {
                                Ok(_member_ref) => JsonRpcResponse {
                                    jsonrpc: JSONRPC_VERSION.to_string(),
                                    id: response_id,
                                    result: Some(serde_json::json!({
                                        "accepted": true,
                                        "meerkat_id": member_id
                                    })),
                                    error: None,
                                },
                                Err(err) => JsonRpcResponse {
                                    jsonrpc: JSONRPC_VERSION.to_string(),
                                    id: response_id,
                                    result: None,
                                    error: Some(JsonRpcError {
                                        code: -32602,
                                        message: format!("Invalid params: {err}"),
                                        data: None,
                                    }),
                                },
                            }
                        }
                    }
                }
            } else {
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: "Invalid params: module_id or (profile + meerkat_id) required"
                            .to_string(),
                        data: None,
                    }),
                }
            }
        }
        "meta/routing/resolve" => {
            let resolve_result = match parse_routing_resolve_params(&request.params) {
                Ok(resolve_request) => runtime
                    .resolve_routing(resolve_request)
                    .await
                    .map_err(RoutingDeliveryParamsError::Routing),
                Err(e) => Err(e),
            };
            match resolve_result {
                Ok(resolution) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(resolution).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/routing/routes/list" => match parse_routing_routes_list_params(&request.params) {
            Ok(()) => {
                let routes = runtime.list_runtime_routes().await;
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "routes": routes
                    })),
                    error: None,
                }
            }
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/routing/routes/add" => {
            let add_result = match parse_routing_route_add_params(&request.params) {
                Ok(route) => runtime
                    .add_runtime_route(route)
                    .await
                    .map_err(RoutingDeliveryParamsError::RouteMutation),
                Err(e) => Err(e),
            };
            match add_result {
                Ok(route) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({ "route": route })),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/routing/routes/delete" => {
            let delete_result = match parse_routing_route_delete_params(&request.params) {
                Ok(route_key) => runtime
                    .delete_runtime_route(&route_key)
                    .await
                    .map_err(RoutingDeliveryParamsError::RouteMutation),
                Err(e) => Err(e),
            };
            match delete_result {
                Ok(route) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({ "deleted": route })),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/delivery/send" => {
            let send_result = match parse_delivery_send_params(&request.params) {
                Ok(send_request) => runtime
                    .send_delivery(send_request)
                    .await
                    .map_err(RoutingDeliveryParamsError::Delivery),
                Err(e) => Err(e),
            };
            match send_result {
                Ok(record) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(record).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/delivery/history" => match parse_delivery_history_params(&request.params) {
            Ok(history_request) => {
                let history = runtime.delivery_history(history_request).await;
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(history).unwrap_or(Value::Null)),
                    error: None,
                }
            }
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/events/subscribe" => match parse_subscribe_request(&request.params) {
            Ok(subscribe_request) => match runtime.subscribe_events(subscribe_request).await {
                Ok(subscribe_result) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(subscribe_result).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {err}"),
                        data: None,
                    }),
                },
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/query_events" => {
            let query: EventQuery = if request.params.is_null() {
                EventQuery::default()
            } else {
                match serde_json::from_value(request.params.clone()) {
                    Ok(query) => query,
                    Err(err) => {
                        return serde_json::to_string(&JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32602,
                                message: format!("Invalid params: invalid query params: {err}"),
                                data: None,
                            }),
                        })
                        .unwrap_or_else(|_| r#"{"error":"serialization failed"}"#.to_string());
                    }
                }
            };
            match runtime.event_log_store() {
                Some(store) => match store.query(query).await {
                    Ok(events) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::to_value(events).unwrap_or(Value::Null)),
                        error: None,
                    },
                    Err(err) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32603,
                            message: format!("query_events failed: {err}"),
                            data: None,
                        }),
                    },
                },
                None => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "status": "no_event_log_configured",
                        "events": [],
                    })),
                    error: None,
                },
            }
        }
        "meta/memory/stores" => match parse_memory_stores_params(&request.params) {
            Ok(()) => {
                let stores = runtime.memory_stores().await;
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "stores": stores,
                    })),
                    error: None,
                }
            }
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/memory/index" => match parse_memory_index_params(&request.params) {
            Ok(index_request) => match runtime.memory_index(index_request).await {
                Ok(indexed) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(indexed).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(MemoryIndexError::BackendPersistFailed(error)) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: MEMORY_BACKEND_UNAVAILABLE_CODE,
                        message: format!(
                            "Memory backend unavailable: {}",
                            MemoryParamsError::backend_message(&error)
                        ),
                        data: None,
                    }),
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!(
                            "Invalid params: {}",
                            MemoryParamsError::Index(err).message()
                        ),
                        data: None,
                    }),
                },
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/memory/query" => match parse_memory_query_params(&request.params) {
            Ok(query_request) => {
                let query_result = runtime.memory_query(query_request).await;
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(query_result).unwrap_or(Value::Null)),
                    error: None,
                }
            }
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/agent_memory/remember" => {
            let runtime = match identity_ctx.map(|ctx| &*ctx.runtime) {
                Some(runtime) => runtime,
                None => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32601,
                        "agent memory is not configured".to_string(),
                    );
                }
            };
            match parse_agent_memory_remember_params(&request.params) {
                Ok(remember_request) => match runtime
                    .remember_agent_memory(
                        &remember_request.realm,
                        &remember_request.identity,
                        remember_request.memory,
                    )
                    .await
                {
                    Ok(record) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::to_value(record).unwrap_or(Value::Null)),
                        error: None,
                    },
                    Err(err) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(agent_memory_rpc_error("write", err)),
                    },
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/agent_memory/forget" => {
            let runtime = match identity_ctx.map(|ctx| &*ctx.runtime) {
                Some(runtime) => runtime,
                None => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32601,
                        "agent memory is not configured".to_string(),
                    );
                }
            };
            match parse_agent_memory_forget_params(&request.params) {
                Ok(forget_request) => match runtime
                    .forget_agent_memory(
                        &forget_request.realm,
                        &forget_request.identity,
                        &forget_request.memory_id,
                    )
                    .await
                {
                    Ok(result) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                        error: None,
                    },
                    Err(err) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(agent_memory_rpc_error("forget", err)),
                    },
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/agent_memory/recall" => {
            let runtime = match identity_ctx.map(|ctx| &*ctx.runtime) {
                Some(runtime) => runtime,
                None => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32601,
                        "agent memory is not configured".to_string(),
                    );
                }
            };
            match parse_agent_memory_recall_params(&request.params) {
                Ok(recall_request) => {
                    match runtime.recall_agent_memory(recall_request.request).await {
                        Ok(records) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: Some(serde_json::json!({ "records": records })),
                            error: None,
                        },
                        Err(err) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(agent_memory_rpc_error("recall", err)),
                        },
                    }
                }
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/agent_memory/update" => {
            let runtime = match identity_ctx.map(|ctx| &*ctx.runtime) {
                Some(runtime) => runtime,
                None => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32601,
                        "agent memory is not configured".to_string(),
                    );
                }
            };
            match parse_agent_memory_update_params(&request.params) {
                Ok(update_request) => match runtime
                    .update_agent_memory(
                        &update_request.realm,
                        &update_request.identity,
                        &update_request.memory_id,
                        update_request.memory,
                    )
                    .await
                {
                    Ok(new_id) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "memory_id": new_id,
                            "supersedes": update_request.memory_id,
                        })),
                        error: None,
                    },
                    Err(err) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(agent_memory_rpc_error("update", err)),
                    },
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/agent_memory/manifest" => {
            let runtime = match identity_ctx.map(|ctx| &*ctx.runtime) {
                Some(runtime) => runtime,
                None => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32601,
                        "agent memory is not configured".to_string(),
                    );
                }
            };
            match parse_agent_memory_manifest_params(&request.params) {
                Ok(manifest_request) => match runtime
                    .manifest_agent_memory(
                        &manifest_request.realm,
                        &manifest_request.identity,
                        manifest_request.tier,
                    )
                    .await
                {
                    Ok(records) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({ "records": records })),
                        error: None,
                    },
                    Err(err) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(agent_memory_rpc_error("manifest", err)),
                    },
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {}", err.message()),
                        data: None,
                    }),
                },
            }
        }
        "meta/session_store/bigquery" => {
            match parse_bigquery_session_store_params(&request.params)
                .and_then(run_bigquery_session_store_request)
            {
                Ok(result) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(result),
                    error: None,
                },
                Err(BigQuerySessionStoreRpcError::Params(message)) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {message}"),
                        data: None,
                    }),
                },
                Err(BigQuerySessionStoreRpcError::Store(error)) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32011,
                        message: format!(
                            "BigQuery session store request failed: {}",
                            format_bigquery_store_error(&error)
                        ),
                        data: None,
                    }),
                },
            }
        }
        "meta/gating/evaluate" => match parse_gating_evaluate_params(&request.params) {
            Ok(gating_request) => {
                let gating_result = runtime.evaluate_gating_action(gating_request).await;
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(gating_result).unwrap_or(Value::Null)),
                    error: None,
                }
            }
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/gating/pending" => match parse_gating_pending_params(&request.params) {
            Ok(()) => {
                let pending = runtime.list_gating_pending().await;
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "pending": pending,
                    })),
                    error: None,
                }
            }
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/gating/decide" => {
            let decide_result = match parse_gating_decide_params(&request.params) {
                Ok(decide_request) => runtime
                    .decide_gating_action(decide_request)
                    .await
                    .map_err(GatingParamsError::Decision),
                Err(e) => Err(e),
            };
            match decide_result {
                Ok(result) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(gating_decide_rpc_error(err)),
                },
            }
        }
        "meta/gating/audit" => match parse_gating_audit_params(&request.params) {
            Ok(params) => {
                let (entries, gap) = match params.after_audit_id.as_deref() {
                    Some(cursor) => {
                        let page = runtime.gating_audit_after(Some(cursor)).await;
                        (
                            page.entries.into_iter().take(params.limit).collect(),
                            page.gap,
                        )
                    }
                    None => (runtime.gating_audit_entries(params.limit).await, false),
                };
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(gating_audit_result(
                        entries,
                        gap,
                        params.after_audit_id.as_deref(),
                    )),
                    error: None,
                }
            }
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {}", err.message()),
                    data: None,
                }),
            },
        },
        "meta/call_tool" => {
            let module_id = request.params.get("module_id").and_then(Value::as_str);
            let tool = request.params.get("tool").and_then(Value::as_str);
            let arguments = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            match (module_id, tool) {
                (Some(module_id), Some(tool)) if !module_id.is_empty() && !tool.is_empty() => {
                    let route = runtime
                        .route_module_call(
                            &ModuleRouteRequest {
                                module_id: module_id.to_string(),
                                method: tool.to_string(),
                                params: arguments,
                            },
                            timeout,
                        )
                        .await;
                    match route {
                        Ok(response) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: Some(serde_json::json!({
                                "module_id": response.module_id,
                                "tool": response.method,
                                "result": response.payload
                            })),
                            error: None,
                        },
                        Err(ModuleRouteError::UnloadedModule(mid)) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32601,
                                message: format!("Module '{mid}' not loaded"),
                                data: None,
                            }),
                        },
                        Err(err) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32000,
                                message: format!("Tool call failed: {err:?}"),
                                data: None,
                            }),
                        },
                    }
                }
                _ => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: "Invalid params: module_id and tool required".to_string(),
                        data: None,
                    }),
                },
            }
        }
        "meta/models/catalog" => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(build_models_catalog_result()),
            error: None,
        },
        // Static Meerkat schedule tool definitions, served without a schedule
        // store or host (see `schedule_methods`).
        schedule_methods::SCHEDULE_TOOLS_METHOD => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(schedule_methods::schedule_tools_result()),
            error: None,
        },
        // Read-only state-directory diagnosis with the live H1/H2 durability
        // census attached. `state_dir` is explicit until the M2 layout
        // authority gives the runtime a reportable state directory.
        storage_methods::STORAGE_DOCTOR_METHOD => {
            match storage_methods::parse_storage_doctor_params(&request.params) {
                Ok(Some(params)) => {
                    let result =
                        storage_methods::run_storage_doctor(&params, runtime.resolved_storage())
                            .await;
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(result),
                        error: None,
                    }
                }
                Ok(None) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(storage_methods::storage_doctor_state_dir_unavailable_error()),
                },
                Err(reason) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {reason}"),
                        data: None,
                    }),
                },
            }
        }
        method if MOBPACK_AUTHORING_METHODS.contains(&method) => {
            handle_unified_mobpack_authoring_rpc(runtime, method, &request.params, response_id)
                .await
        }
        "meta/blob/get" => {
            mob_methods::handle_blob_get(runtime, response_id, &request.params).await
        }
        // ONE arm for the whole member-declaration family, dispatched from the
        // canonical registry. Three separate arms is what let the console plane
        // drift out of sync and ship dead methods in 0.8.21.
        method if mob_methods::is_member_declaration_method(method) => {
            let handle = runtime.mob_handle();
            match mob_methods::handle_member_declaration_rpc(
                &handle,
                identity_ctx
                    .map(|ctx| ctx.runtime.runtime())
                    .or_else(|| runtime.identity_runtime_ref()),
                runtime.speaks_for_composition(),
                method,
                response_id.clone(),
                &request.params,
            )
            .await
            {
                Some(response) => response,
                // Unreachable while the guard and the dispatcher read the SAME
                // registry. Kept as a loud fallthrough rather than unreachable!()
                // so a future divergence between them surfaces as a method error
                // instead of a panic.
                None => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!(
                            "{method} is registered in the member-declaration family but has no dispatcher arm"
                        ),
                        data: None,
                    }),
                },
            }
        }
        "meta/send_message" => {
            // Pass the identity runtime so bare durable identities resolve
            // through the identity bridge. An active identity-owned roster row
            // takes that same path so Meta delivery preparation is not
            // bypassed; raw roster rows preserve mob-plane semantics.
            Box::pin(mob_methods::handle_send_message(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            ))
            .await
        }
        "meta/find_members" => {
            with_read_deadline(
                "meta/find_members",
                "the mob actor roster search and the identity staleness reads",
                response_id.clone(),
                mob_methods::handle_find_members(
                    runtime,
                    identity_ctx.map(|ctx| ctx.runtime.runtime()),
                    response_id,
                    &request.params,
                ),
            )
            .await
        }
        "meta/ensure_member" => {
            Box::pin(mob_methods::handle_ensure_member(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            ))
            .await
        }
        "meta/list_members" => {
            with_read_deadline(
                "meta/list_members",
                "the mob actor roster snapshot and the identity staleness reads",
                response_id.clone(),
                mob_methods::handle_list_members(
                    runtime,
                    identity_ctx.map(|ctx| ctx.runtime.runtime()),
                    response_id,
                ),
            )
            .await
        }
        "meta/get_member" => {
            with_read_deadline(
                "meta/get_member",
                "the mob actor roster read and the identity staleness read",
                response_id.clone(),
                mob_methods::handle_get_member(
                    runtime,
                    identity_ctx.map(|ctx| ctx.runtime.runtime()),
                    response_id,
                    &request.params,
                ),
            )
            .await
        }
        "meta/retire_member" => {
            mob_methods::handle_retire_member(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            )
            .await
        }
        "meta/respawn_member" => {
            Box::pin(mob_methods::handle_respawn_member(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            ))
            .await
        }
        "meta/reload_member" => {
            Box::pin(mob_methods::handle_reload_member(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            ))
            .await
        }
        "meta/member_health" => {
            mob_methods::handle_member_health(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            )
            .await
        }
        "meta/reconcile_edges" => mob_methods::handle_reconcile_edges(runtime, response_id).await,
        "meta/rediscover" => mob_methods::handle_rediscover(runtime, response_id).await,
        "meta/mob_events/query" => {
            mob_methods::handle_mob_events_query(runtime, response_id, request.params).await
        }
        "meta/mob_events/subscribe" => {
            mob_methods::handle_mob_events_subscribe(runtime, response_id, request.params).await
        }
        "meta/cross_mob/wire" => {
            Box::pin(mob_methods::handle_cross_mob_wire(
                runtime,
                runtime_owner.cloned(),
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            ))
            .await
        }
        "meta/cross_mob/unwire" => {
            Box::pin(mob_methods::handle_cross_mob_unwire(
                runtime,
                runtime_owner.cloned(),
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            ))
            .await
        }
        "meta/cross_mob/send" => {
            mob_methods::handle_cross_mob_send(
                runtime,
                runtime_owner.cloned(),
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            )
            .await
        }
        "meta/cross_mob/directory" => {
            mob_methods::handle_cross_mob_directory(runtime, response_id).await
        }
        "meta/cross_mob/peer_info" => {
            mob_methods::handle_cross_mob_peer_info(runtime, response_id, &request.params).await
        }
        "meta/cross_mob/wire_local" => {
            mob_methods::handle_cross_mob_wire_local(
                runtime,
                runtime_owner.cloned(),
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            )
            .await
        }
        "meta/cross_mob/unwire_local" => {
            mob_methods::handle_cross_mob_unwire_local(
                runtime,
                runtime_owner.cloned(),
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            )
            .await
        }
        "meta/peer_pubkey" => mob_methods::handle_peer_pubkey(runtime, response_id).await,
        "meta/member_status" => {
            with_read_deadline(
                "meta/member_status",
                "the mob actor member-status round trip",
                response_id.clone(),
                mob_methods::handle_member_status(
                    runtime,
                    identity_ctx.map(|ctx| ctx.runtime.runtime()),
                    response_id,
                    &request.params,
                ),
            )
            .await
        }
        "meta/identity/resolved_tools" => {
            with_read_deadline(
                "meta/identity/resolved_tools",
                "the identity status read and the session tool-scope snapshot",
                response_id.clone(),
                mob_methods::handle_identity_resolved_tools(
                    runtime,
                    identity_ctx.map(|ctx| ctx.runtime.runtime()),
                    response_id,
                    &request.params,
                ),
            )
            .await
        }
        // Same deadline posture as `resolved_tools`: this crosses the
        // identity-runtime status read and then a runtime-machine command,
        // so it can queue behind a member's in-flight turn exactly the way
        // the operator deployment observed for `resolved_tools`.
        "meta/identity/routing_status" => {
            with_read_deadline(
                "meta/identity/routing_status",
                "the identity status read and the session model-routing status",
                response_id.clone(),
                mob_methods::handle_identity_routing_status(
                    runtime,
                    identity_ctx.map(|ctx| ctx.runtime.runtime()),
                    response_id,
                    &request.params,
                ),
            )
            .await
        }
        "meta/force_cancel_member" => {
            mob_methods::handle_force_cancel_member(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            )
            .await
        }
        "meta/stop_member_run" => {
            mob_methods::handle_stop_member_run(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            )
            .await
        }
        "meta/spawn_helper" => {
            Box::pin(mob_methods::handle_spawn_helper(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            ))
            .await
        }
        "meta/fork_helper" => {
            Box::pin(mob_methods::handle_fork_helper(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            ))
            .await
        }
        "meta/attach_existing_session" => {
            Box::pin(mob_methods::handle_attach_existing_session(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            ))
            .await
        }
        "meta/cancel_flow" => {
            mob_methods::handle_cancel_flow(runtime, response_id, &request.params).await
        }
        "meta/flow_status" => {
            with_read_deadline(
                "meta/flow_status",
                "the mob actor flow-status read",
                response_id.clone(),
                mob_methods::handle_flow_status(runtime, response_id, &request.params),
            )
            .await
        }
        "meta/list_flows" => mob_methods::handle_list_flows(runtime, response_id).await,
        "meta/list_runs" => {
            with_read_deadline(
                "meta/list_runs",
                "the mob actor run-ledger read",
                response_id.clone(),
                mob_methods::handle_list_runs(runtime, response_id, &request.params),
            )
            .await
        }
        "meta/run_flow" => {
            Box::pin(mob_methods::handle_run_flow(
                runtime,
                identity_ctx.map(|ctx| ctx.runtime.runtime()),
                response_id,
                &request.params,
            ))
            .await
        }
        "meta/collect_completed" => {
            mob_methods::handle_collect_completed(runtime, response_id).await
        }
        "meta/wait_ready" => {
            mob_methods::handle_wait_ready(runtime, response_id, &request.params).await
        }
        method
            if matches!(
                method,
                "meta/mob_labels/set"
                    | "meta/mob_labels/get"
                    | "meta/mob_labels/delete"
                    | "meta/run_labels/set"
                    | "meta/run_labels/get"
                    | "meta/run_labels/delete",
            ) =>
        {
            mob_methods::handle_label_method(runtime, method, response_id, &request.params).await
        }
        // ----- identity-first methods -----
        "meta/send" => {
            let Some(ctx) = identity_ctx else {
                return maybe_identity_not_configured(is_notification, response_id);
            };
            let identity_rt = ctx.runtime.runtime();
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            let content_val = request
                .params
                .get("content")
                .cloned()
                .unwrap_or(Value::Null);
            let content = match serde_json::from_value::<meerkat_core::ContentInput>(content_val) {
                Ok(content) => content,
                Err(err) => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        format!("invalid content: {err}"),
                    );
                }
            };
            let expected_alias = crate::member_comms_id::is_reserved_generated_alias(identity_str)
                .then_some(identity_str);
            let expected_spec_digest = match request.params.get("expected_spec_digest") {
                None | Some(Value::Null) => None,
                Some(Value::String(digest)) => Some(digest.as_str()),
                Some(_) => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        "invalid expected_spec_digest: expected a string".to_string(),
                    );
                }
            };
            // `handling_mode` ("queue" or "steer", default "queue") is a keyed
            // send's: it is committed with the content under the key.
            let handling_mode = match mob_methods::parse_handling_mode(&request.params) {
                Ok(handling_mode) => handling_mode,
                Err(message) => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        message.to_string(),
                    );
                }
            };
            match request.params.get("idempotency_key") {
                None | Some(Value::Null) => {}
                Some(Value::String(idempotency_key)) => {
                    // A keyed send is committed as a member continuation: it
                    // answers the continuation's receipt and delivery status,
                    // never a turn, and is checked against no authored row.
                    let combined = if rpc_track_turn_requested(&request.params) {
                        Some("track_turn")
                    } else if expected_spec_digest.is_some() {
                        Some("expected_spec_digest")
                    } else {
                        None
                    };
                    if let Some(field) = combined {
                        return maybe_error_response(
                            is_notification,
                            response_id,
                            -32602,
                            format!("idempotency_key does not combine with {field}"),
                        );
                    }
                    let response = match ctx
                        .runtime
                        .send_keyed_with_handling_mode(
                            &identity,
                            expected_alias,
                            &content,
                            handling_mode,
                            idempotency_key,
                        )
                        .await
                    {
                        Ok(receipt) => match serde_json::to_value(&receipt) {
                            Ok(continuation) => JsonRpcResponse {
                                jsonrpc: JSONRPC_VERSION.to_string(),
                                id: response_id,
                                result: Some(serde_json::json!({ "continuation": continuation })),
                                error: None,
                            },
                            Err(error) => {
                                return maybe_error_response(
                                    is_notification,
                                    response_id,
                                    -32603,
                                    format!("continuation receipt serialization failed: {error}"),
                                );
                            }
                        },
                        Err(error) => identity_error_response(response_id, &error),
                    };
                    return if is_notification {
                        String::new()
                    } else {
                        serialize_response(&response)
                    };
                }
                Some(_) => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        "invalid idempotency_key: expected a string".to_string(),
                    );
                }
            }
            // An unkeyed send queues: a steer without a key is refused, never
            // queued unseen.
            if matches!(handling_mode, meerkat_core::types::HandlingMode::Steer) {
                return maybe_error_response(
                    is_notification,
                    response_id,
                    -32602,
                    "handling_mode \"steer\" needs idempotency_key: an unkeyed meta/send queues"
                        .to_string(),
                );
            }
            let send_result = if let Some(expected_spec_digest) = expected_spec_digest {
                // The input was authored from the registered row this digest
                // names (`meta/registered_spec`); the send re-checks it under
                // the identity's lifecycle lock before delivery.
                identity_rt
                    .send_expecting_registered_spec(
                        &identity,
                        expected_alias,
                        &content,
                        meerkat_core::types::HandlingMode::Queue,
                        rpc_track_turn_requested(&request.params),
                        expected_spec_digest,
                    )
                    .await
            } else if rpc_track_turn_requested(&request.params) {
                identity_rt
                    .send_with_turn_ticket(
                        &identity,
                        expected_alias,
                        &content,
                        meerkat_core::types::HandlingMode::Queue,
                        None,
                    )
                    .await
                    .map(|ticketed| (ticketed.admission, Some(ticketed.turn)))
            } else {
                identity_rt
                    .send_admission_tracked(
                        &identity,
                        expected_alias,
                        &content,
                        meerkat_core::types::HandlingMode::Queue,
                        None,
                    )
                    .await
                    .map(|admission| (admission, None))
            };
            match send_result {
                Ok((admission, turn)) => {
                    let mut result = serde_json::json!({
                        "fencing_token": admission.fencing_token.get(),
                        // Cursor read before delivery: an identity-wide
                        // barrier (another delivery's completion also passes
                        // it). Wait on `turn.ticket` with meta/turn_result
                        // for THIS turn; never compare output text.
                        "completion_baseline": completion_cursor_json(admission.completion_baseline),
                    });
                    insert_turn_tracking(&mut result, turn.as_ref());
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(result),
                        error: None,
                    }
                }
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/send_status" => {
            let Some(ctx) = identity_ctx else {
                return maybe_identity_not_configured(is_notification, response_id);
            };
            let identity_rt = ctx.runtime.runtime();
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // meta/send's own target checks, in its order; the identity's
            // addressability and its hold on the session are the runtime's.
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            let Some(session_id) = request
                .params
                .get("session_id")
                .and_then(Value::as_str)
                .and_then(|raw| meerkat_core::types::SessionId::parse(raw).ok())
            else {
                return maybe_error_response(
                    is_notification,
                    response_id,
                    -32602,
                    "invalid session_id: expected a session id".to_string(),
                );
            };
            let Some(input_id) = request
                .params
                .get("input_id")
                .and_then(Value::as_str)
                .and_then(|raw| uuid::Uuid::parse_str(raw).ok())
                .map(meerkat_core::lifecycle::InputId::from_uuid)
            else {
                return maybe_error_response(
                    is_notification,
                    response_id,
                    -32602,
                    "invalid input_id: expected an input id".to_string(),
                );
            };
            match ctx
                .runtime
                .send_status(&identity, &session_id, &input_id)
                .await
            {
                Ok(read) => match serde_json::to_value(&read) {
                    Ok(result) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(result),
                        error: None,
                    },
                    Err(error) => {
                        return maybe_error_response(
                            is_notification,
                            response_id,
                            -32603,
                            format!("input state serialization failed: {error}"),
                        );
                    }
                },
                Err(error) => identity_error_response(response_id, &error),
            }
        }
        "meta/registered_spec" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let identity = match crate::identity_first::AgentIdentity::parse(identity_str) {
                Ok(identity) => identity,
                Err(e) => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        format!("invalid identity: {e}"),
                    );
                }
            };
            match identity_rt.registered_spec(&identity).await {
                Ok(registered) => match serde_json::to_value(&registered) {
                    Ok(mut result) => {
                        result["identity"] = Value::String(identity.to_string());
                        JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: Some(result),
                            error: None,
                        }
                    }
                    Err(e) => {
                        return maybe_error_response(
                            is_notification,
                            response_id,
                            -32603,
                            format!("registered row serialization failed: {e}"),
                        );
                    }
                },
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/interact" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            let content_val = request
                .params
                .get("content")
                .cloned()
                .unwrap_or(Value::Null);
            let content =
                match serde_json::from_value::<meerkat_core::ContentInput>(content_val.clone()) {
                    Ok(content) => content,
                    Err(err) => {
                        return maybe_error_response(
                            is_notification,
                            response_id,
                            -32602,
                            format!("invalid content: {err}"),
                        );
                    }
                };
            let origin = request
                .params
                .get("origin")
                .and_then(|v| v.as_str())
                .unwrap_or("console");
            let caller_interaction_id = request
                .params
                .get("interaction_id")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let runtime_member_id = identity_rt
                .status(&identity)
                .await
                .ok()
                .and_then(|status| status.agent_runtime_id.map(|id| id.as_str().to_string()));

            let interaction_id = match caller_interaction_id {
                // A caller-supplied id rides into the runtime as the run's
                // transcript interaction id. Two in-flight sends under one id
                // would make both runs' frames and terminals ambiguous, so a
                // reuse while the first is still in flight is refused, typed,
                // before anything is delivered.
                Some(interaction_id) => {
                    if runtime
                        .reserve_caller_identity_interaction(
                            identity.as_str(),
                            runtime_member_id.as_deref(),
                            &interaction_id,
                            origin,
                            content_val,
                        )
                        .await
                        .is_err()
                    {
                        if is_notification {
                            return String::new();
                        }
                        return serialize_response(&JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32602,
                                message: format!(
                                    "interaction_id {interaction_id} is already in flight for {}",
                                    identity.as_str()
                                ),
                                data: Some(serde_json::json!({
                                    "code": "interaction_id_in_flight",
                                    "identity": identity.as_str(),
                                    "interaction_id": interaction_id,
                                })),
                            }),
                        });
                    }
                    interaction_id
                }
                None => {
                    let interaction_id = meerkat_core::types::SessionId::new().to_string();
                    if let Err(err) = runtime
                        .reserve_identity_interaction(
                            identity.as_str(),
                            runtime_member_id.as_deref(),
                            &interaction_id,
                            origin,
                            content_val,
                        )
                        .await
                    {
                        return maybe_error_response(
                            is_notification,
                            response_id,
                            -32003,
                            format!("failed to reserve interaction: {err}"),
                        );
                    }
                    interaction_id
                }
            };

            let expected_alias = crate::member_comms_id::is_reserved_generated_alias(identity_str)
                .then_some(identity_str);
            let send_result = if rpc_track_turn_requested(&request.params) {
                // The reserved interaction id rides the admission unchanged;
                // the ticket is minted per admission, so a caller reusing a
                // settled interaction id still gets a ticket of its own.
                identity_rt
                    .send_with_turn_ticket(
                        &identity,
                        expected_alias,
                        &content,
                        meerkat_core::types::HandlingMode::Queue,
                        Some(&interaction_id),
                    )
                    .await
                    .map(|ticketed| (ticketed.admission, Some(ticketed.turn)))
            } else {
                // The untracked lane threads the same reserved id, so the
                // run's typed lineage settles this exact reservation too.
                identity_rt
                    .send_admission_tracked(
                        &identity,
                        expected_alias,
                        &content,
                        meerkat_core::types::HandlingMode::Queue,
                        Some(&interaction_id),
                    )
                    .await
                    .map(|admission| (admission, None))
            };
            match send_result {
                Ok((admission, turn)) => {
                    let mut result = serde_json::json!({
                        "interaction_id": interaction_id,
                        "fencing_token": admission.fencing_token.get(),
                        "completion_baseline": completion_cursor_json(admission.completion_baseline),
                        "stream": {
                            "route": format!("/console/identity/{}/stream", identity.as_str()),
                            "identity": identity.as_str(),
                        }
                    });
                    insert_turn_tracking(&mut result, turn.as_ref());
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(result),
                        error: None,
                    }
                }
                Err(e) => {
                    runtime
                        .record_console_lifecycle(
                            identity.as_str(),
                            "interaction_failed",
                            serde_json::json!({
                                "interaction_id": interaction_id,
                                "origin": origin,
                                "error": e.to_string(),
                            }),
                        )
                        .await;
                    identity_error_response(response_id, &e)
                }
            }
        }
        "meta/dispatch" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            let di_val = request
                .params
                .get("dispatch_input")
                .cloned()
                .unwrap_or(Value::Null);
            let content_val = di_val
                .get("content")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new()));
            let content = match serde_json::from_value::<meerkat_core::ContentInput>(content_val) {
                Ok(content) => content,
                Err(err) => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        format!("invalid dispatch_input.content: {err}"),
                    );
                }
            };
            let origin_str = di_val
                .get("origin")
                .and_then(|v| v.as_str())
                .unwrap_or("system");
            let origin = match origin_str {
                "connector" => crate::identity_first::DispatchOrigin::Connector,
                "scheduler" => crate::identity_first::DispatchOrigin::Scheduler,
                "policy" => crate::identity_first::DispatchOrigin::Policy,
                "flow" => crate::identity_first::DispatchOrigin::Flow,
                _ => crate::identity_first::DispatchOrigin::System,
            };
            let correlation_id = di_val
                .get("correlation_id")
                .and_then(|v| v.as_str())
                .map(crate::identity_first::CorrelationId::new);
            let idempotency_key = di_val
                .get("idempotency_key")
                .and_then(|v| v.as_str())
                .map(crate::identity_first::DispatchIdempotencyKey::new);
            let dispatch_input = crate::identity_first::DispatchInput {
                content,
                origin,
                correlation_id,
                idempotency_key,
            };
            // Scope-bound dispatch: exact content to the session the caller
            // captured from `meta/status_identity` and persisted. Never
            // materializes, repairs, retargets or prepares the delivery.
            if let Some(scope_value) = request
                .params
                .get("expected_scope")
                .filter(|value| !value.is_null())
            {
                if rpc_track_turn_requested(&request.params) {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        "expected_scope does not combine with track_turn".to_string(),
                    );
                }
                let scope = match crate::identity_first::DeliveryScope::from_json_value(
                    scope_value.clone(),
                ) {
                    Ok(scope) => scope,
                    Err(error) => {
                        return if is_notification {
                            String::new()
                        } else {
                            serialize_response(&invalid_delivery_scope_response(
                                response_id,
                                "expected_scope",
                                &error,
                            ))
                        };
                    }
                };
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&match identity_rt
                        .dispatch_at_scope(&identity, &scope, &dispatch_input)
                        .await
                    {
                        Ok(receipt) => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: Some(scoped_dispatch_receipt_json(&receipt)),
                            error: None,
                        },
                        Err(error) => scoped_delivery_error_response(response_id, &error),
                    })
                };
            }
            let expected_alias = crate::member_comms_id::is_reserved_generated_alias(identity_str)
                .then_some(identity_str);
            let dispatch_result = if rpc_track_turn_requested(&request.params) {
                identity_rt
                    .dispatch_with_turn_ticket(&identity, expected_alias, &dispatch_input)
                    .await
                    .map(|ticketed| (ticketed.admission, Some(ticketed.turn)))
            } else {
                identity_rt
                    .dispatch_admission_tracked(&identity, expected_alias, &dispatch_input)
                    .await
                    .map(|admission| (admission, None))
            };
            match dispatch_result {
                Ok((admission, turn)) => {
                    let mut result = serde_json::json!({
                        "fencing_token": admission.fencing_token.get(),
                        "durable": admission.durable,
                        // See meta/send: an identity-wide barrier; wait on
                        // `turn.ticket` for the turn this dispatch started.
                        "completion_baseline": completion_cursor_json(admission.completion_baseline),
                    });
                    insert_turn_tracking(&mut result, turn.as_ref());
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(result),
                        error: None,
                    }
                }
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/recover_delivery" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => &ctx.runtime,
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            let scope = match request
                .params
                .get("scope")
                .filter(|value| !value.is_null())
                .cloned()
            {
                Some(value) => match crate::identity_first::DeliveryScope::from_json_value(value) {
                    Ok(scope) => scope,
                    Err(error) => {
                        return if is_notification {
                            String::new()
                        } else {
                            serialize_response(&invalid_delivery_scope_response(
                                response_id,
                                "scope",
                                &error,
                            ))
                        };
                    }
                },
                None => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        "missing scope".to_string(),
                    );
                }
            };
            let timeout = match rpc_wait_timeout(&request.params) {
                Ok(timeout) => timeout,
                Err(message) => {
                    return maybe_error_response(is_notification, response_id, -32602, message);
                }
            };
            let mut lookup = crate::identity_first::DispatchInput::system("");
            lookup.idempotency_key = request
                .params
                .get("idempotency_key")
                .and_then(|v| v.as_str())
                .map(crate::identity_first::DispatchIdempotencyKey::new);
            lookup.correlation_id = request
                .params
                .get("correlation_id")
                .and_then(|v| v.as_str())
                .map(crate::identity_first::CorrelationId::new);
            match identity_rt
                .recover_at_scope(&identity, &scope, &lookup, timeout)
                .await
            {
                Ok(recovery) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "identity": identity.as_str(),
                        "delivery_scope": scope.to_json_value(),
                        "recovery": scoped_recovery_json(&recovery),
                    })),
                    error: None,
                },
                Err(error) => scoped_delivery_error_response(response_id, &error),
            }
        }
        "meta/turn_result" | "meta/wait_for_turn" => {
            // `meta/wait_for_turn` is the same read, taken once the ticket
            // settles: the server waits on the ticket registry's change signal
            // (#468), so a caller never polls turn_result. `timeout_ms` is the
            // caller's deadline; reaching it returns the ticket still pending.
            let wait_timeout = if request.method == "meta/wait_for_turn" {
                match rpc_wait_timeout(&request.params) {
                    Ok(wait_timeout) => Some(wait_timeout),
                    Err(message) => {
                        return maybe_error_response(is_notification, response_id, -32602, message);
                    }
                }
            } else {
                None
            };
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            let ticket = match request
                .params
                .get("ticket")
                .and_then(|v| v.as_str())
                .map(crate::identity_first::TurnTicket::parse)
            {
                Some(Ok(ticket)) => ticket,
                Some(Err(err)) => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        format!("invalid ticket: {err}"),
                    );
                }
                None => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        "missing ticket".to_string(),
                    );
                }
            };
            let (outcome, wait) = match wait_timeout {
                Some(wait_timeout) => match identity_rt
                    .wait_for_turn_outcome(&identity, ticket, wait_timeout)
                    .await
                {
                    crate::identity_first::TurnWait::Settled(outcome) => (outcome, Some("settled")),
                    crate::identity_first::TurnWait::TimedOut(outcome) => {
                        (outcome, Some("timed_out"))
                    }
                    crate::identity_first::TurnWait::ShuttingDown(outcome) => {
                        (outcome, Some("shutting_down"))
                    }
                },
                None => (identity_rt.turn_outcome(&identity, ticket), None),
            };
            let completion_cursor = identity_rt.completion_cursor(&identity).await;
            let mut result = turn_outcome_json(&outcome);
            if let (Value::Object(fields), Some(wait)) = (&mut result, wait) {
                // How the server-side wait ended: `settled`, `timed_out` (the
                // turn is still pending) or `shutting_down`.
                fields.insert("wait".to_string(), Value::from(wait));
            }
            if let Value::Object(fields) = &mut result {
                fields.insert("identity".to_string(), Value::from(identity.as_str()));
                fields.insert("ticket".to_string(), Value::from(ticket.to_string()));
                fields.insert(
                    "completion_cursor".to_string(),
                    completion_cursor_json(completion_cursor),
                );
            }
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(result),
                error: None,
            }
        }
        "meta/subscribe" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            match identity_rt.subscribe(&identity).await {
                Ok(_receiver) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "identity": identity.as_str(),
                        "stream_id": identity.as_str(),
                        "subscribed": true,
                    })),
                    error: None,
                },
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/status_identity_bootstrap" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(
                    serde_json::to_value(identity_rt.identity_bootstrap_status())
                        .unwrap_or(Value::Null),
                ),
                error: None,
            }
        }
        "meta/wait_identity_bootstrap" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let params = match request.params.as_object() {
                Some(params) => params,
                None => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        "params must be an object".to_string(),
                    );
                }
            };
            if let Some(field) = params
                .keys()
                .find(|field| !matches!(field.as_str(), "target" | "timeout_ms"))
            {
                return maybe_error_response(
                    is_notification,
                    response_id,
                    -32602,
                    format!("unsupported parameter: {field}"),
                );
            }
            let target = match params.get("target") {
                None => "materialized".to_string(),
                Some(Value::String(target)) => target.clone(),
                Some(_) => {
                    return maybe_error_response(
                        is_notification,
                        response_id,
                        -32602,
                        "target must be a string".to_string(),
                    );
                }
            };
            if !matches!(target.as_str(), "materialized" | "startup_ready") {
                return maybe_error_response(
                    is_notification,
                    response_id,
                    -32602,
                    "target must be 'materialized' or 'startup_ready'".to_string(),
                );
            }
            let wait_timeout = match params.get("timeout_ms") {
                None => timeout,
                Some(value) => match value.as_u64() {
                    Some(value) => Duration::from_millis(value),
                    None => {
                        return maybe_error_response(
                            is_notification,
                            response_id,
                            -32602,
                            "timeout_ms must be a non-negative integer".to_string(),
                        );
                    }
                },
            };
            let (status, timed_out, startup_ready) = if target == "startup_ready" {
                let mob_handle = runtime.mob_handle();
                let wait = wait_identity_startup_ready(
                    identity_rt,
                    wait_timeout,
                    move |member_ids, remaining| {
                        let mob_handle = mob_handle.clone();
                        async move {
                            wait_identity_members_ready_with_observation_retry(
                                remaining,
                                |remaining| {
                                    mob_handle.wait_for_members_ready(&member_ids, Some(remaining))
                                },
                            )
                            .await
                        }
                    },
                )
                .await;
                match wait {
                    Ok(wait) => (wait.status, wait.timed_out, Some(wait.startup_ready)),
                    Err(error) => {
                        return maybe_error_response(is_notification, response_id, -32000, error);
                    }
                }
            } else {
                let (status, timed_out) = identity_rt
                    .wait_identity_bootstrap_terminal(wait_timeout)
                    .await;
                (status, timed_out, None)
            };
            let mut result = serde_json::to_value(status).unwrap_or_else(|_| serde_json::json!({}));
            if let Some(object) = result.as_object_mut() {
                object.insert("timed_out".to_string(), Value::Bool(timed_out));
                object.insert("target".to_string(), Value::String(target));
                if let Some(startup_ready) = startup_ready {
                    object.insert("startup_ready".to_string(), Value::Bool(startup_ready));
                }
            }
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(result),
                error: None,
            }
        }
        "meta/status_identity" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            match identity_rt.status(&identity).await {
                Ok(status) => {
                    let mut result = identity_status_wire(&status);
                    insert_delivery_scope(
                        &mut result,
                        identity_rt.capture_delivery_scope(&identity).await,
                    );
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(result),
                        error: None,
                    }
                }
                Err(e @ crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                    if rpc_live_only_fallback_allowed(&target, identity_str)
                        && let Some(live) = target.live.as_ref()
                    {
                        JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: Some(rpc_live_identity_status_json(live)),
                            error: None,
                        }
                    } else {
                        identity_error_response(response_id, &e)
                    }
                }
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/respawn" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            let expected_alias = crate::member_comms_id::is_reserved_generated_alias(identity_str)
                .then_some(identity_str);
            let respawn_result = identity_rt
                .respawn_identity_in_place_tracked(&identity, expected_alias)
                .await;
            match respawn_result {
                Ok(scope) => {
                    // A durable identity is retired and woken onto its same
                    // session at a new Core generation. Raw-member
                    // respawn remains available only to unregistered/classic
                    // members because it rotates the session. Keep the legacy
                    // warning fields for wire compatibility. A peer-only
                    // member has no Core scope: its atoms are null.
                    let live_respawn_warning: Option<Value> = None;
                    let cleanup_warning: Option<Value> = None;
                    let generation = scope.as_ref().map(|scope| scope.generation().get());
                    runtime
                        .record_console_lifecycle(
                            identity.as_str(),
                            "identity_respawned",
                            serde_json::json!({
                                "generation": generation,
                                "live_respawn_warning": live_respawn_warning.clone(),
                                "cleanup_warning": cleanup_warning.clone(),
                            }),
                        )
                        .await;
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "identity": identity.as_str(),
                            "agent_runtime_id": scope.as_ref().map(|scope| {
                                crate::member_comms_id::public_runtime_alias(scope.runtime_id())
                            }),
                            "session_id": scope.as_ref().map(|scope| scope.session_id().to_string()),
                            "generation": generation,
                            "live_respawn_warning": live_respawn_warning,
                            "cleanup_warning": cleanup_warning,
                        })),
                        error: None,
                    }
                }
                Err(e @ crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                    if rpc_live_only_fallback_allowed(&target, identity_str)
                        && let Some(live) = target.live.as_ref()
                    {
                        match Box::pin(respawn_rpc_live_identity(runtime, live)).await {
                            Ok(result) => {
                                runtime
                                    .record_console_lifecycle(
                                        live.identity.as_str(),
                                        "identity_respawned",
                                        serde_json::json!({}),
                                    )
                                    .await;
                                JsonRpcResponse {
                                    jsonrpc: JSONRPC_VERSION.to_string(),
                                    id: response_id,
                                    result: Some(result),
                                    error: None,
                                }
                            }
                            Err(err) => JsonRpcResponse {
                                jsonrpc: JSONRPC_VERSION.to_string(),
                                id: response_id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32000,
                                    message: format!("respawn failed: {err}"),
                                    data: None,
                                }),
                            },
                        }
                    } else {
                        identity_error_response(response_id, &e)
                    }
                }
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/retire" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            let cleanup_handle = runtime.mob_handle();
            let cleanup_identity = identity.clone();
            let include_current = !identity_rt.has_session_bridge();
            let expected_alias = crate::member_comms_id::is_reserved_generated_alias(identity_str)
                .then_some(identity_str);
            let retire_result = identity_rt
                .retire_and_cleanup_live_members_tracked(
                    &identity,
                    expected_alias,
                    move |retired_alias| async move {
                        let stale_member_ids = stale_rpc_member_ids_for_identity_with_handle(
                            &cleanup_handle,
                            cleanup_identity.as_str(),
                            retired_alias
                                .as_ref()
                                .map(crate::identity_first::AgentRuntimeId::as_str),
                            include_current,
                        )
                        .await;
                        retire_rpc_member_ids_with_handle(&cleanup_handle, stale_member_ids)
                            .await
                            .err()
                            .map(|error| {
                                serde_json::json!({
                                    "kind": "stale_member_cleanup_failed_after_identity_retire",
                                    "message": error,
                                    "identity": cleanup_identity.as_str(),
                                })
                            })
                    },
                )
                .await;
            match retire_result {
                Ok(cleanup_warning) => {
                    // Core retired the member; Meta holds no fence of
                    // its own to report.
                    runtime
                        .record_console_lifecycle(
                            identity.as_str(),
                            "identity_retired",
                            serde_json::json!({
                                "cleanup_warning": cleanup_warning.clone(),
                            }),
                        )
                        .await;
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "identity": identity.as_str(),
                            "cleanup_warning": cleanup_warning,
                        })),
                        error: None,
                    }
                }
                Err(e @ crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                    if rpc_live_only_fallback_allowed(&target, identity_str)
                        && let Some(live) = target.live.as_ref()
                    {
                        match retire_rpc_live_identity(runtime, live).await {
                            Ok(()) => {
                                runtime
                                    .record_console_lifecycle(
                                        live.identity.as_str(),
                                        "identity_retired",
                                        serde_json::json!({}),
                                    )
                                    .await;
                                JsonRpcResponse {
                                    jsonrpc: JSONRPC_VERSION.to_string(),
                                    id: response_id,
                                    result: Some(
                                        serde_json::json!({ "identity": live.identity.as_str() }),
                                    ),
                                    error: None,
                                }
                            }
                            Err(err) => JsonRpcResponse {
                                jsonrpc: JSONRPC_VERSION.to_string(),
                                id: response_id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32000,
                                    message: format!("retire failed: {err}"),
                                    data: None,
                                }),
                            },
                        }
                    } else {
                        identity_error_response(response_id, &e)
                    }
                }
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/reset" => {
            let identity_reset_ctx = match identity_ctx {
                Some(ctx) => ctx,
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_rt = identity_reset_ctx.runtime.runtime();
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            let _registered_status = match identity_rt.status(&identity).await {
                Ok(status) => {
                    if !identity_rt.has_session_bridge() {
                        let response = rpc_reset_requires_session_bridge_response(response_id);
                        return if is_notification {
                            String::new()
                        } else {
                            serialize_response(&response)
                        };
                    }
                    status
                }
                Err(e @ crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                    if rpc_live_only_fallback_allowed(&target, identity_str)
                        && let Some(live) = target.live.as_ref()
                    {
                        let response =
                            match Box::pin(respawn_rpc_live_identity(runtime, live)).await {
                                Ok(result) => {
                                    runtime
                                        .record_console_lifecycle(
                                            live.identity.as_str(),
                                            "identity_reset",
                                            serde_json::json!({}),
                                        )
                                        .await;
                                    JsonRpcResponse {
                                        jsonrpc: JSONRPC_VERSION.to_string(),
                                        id: response_id,
                                        result: Some(result),
                                        error: None,
                                    }
                                }
                                Err(err) => JsonRpcResponse {
                                    jsonrpc: JSONRPC_VERSION.to_string(),
                                    id: response_id,
                                    result: None,
                                    error: Some(JsonRpcError {
                                        code: -32000,
                                        message: format!("reset failed: {err}"),
                                        data: None,
                                    }),
                                },
                            };
                        return if is_notification {
                            String::new()
                        } else {
                            serialize_response(&response)
                        };
                    }
                    let response = identity_error_response(response_id, &e);
                    return if is_notification {
                        String::new()
                    } else {
                        serialize_response(&response)
                    };
                }
                Err(e) => {
                    let response = identity_error_response(response_id, &e);
                    return if is_notification {
                        String::new()
                    } else {
                        serialize_response(&response)
                    };
                }
            };
            identity_rt.set_reset_roster_provider_context(
                Some(identity_reset_ctx.roster_provider.clone()),
                identity_reset_ctx.mob_definition.clone(),
            );
            let reset_result = if crate::member_comms_id::is_reserved_generated_alias(identity_str)
            {
                identity_rt
                    .reset_member_alias_tracked(&identity, identity_str)
                    .await
            } else {
                identity_rt.reset_tracked(&identity).await
            };
            match reset_result {
                Ok(binding) => {
                    // Core's binding of the fresh life: a live identity
                    // runs it now; a dormant one wakes onto it later.
                    let agent_runtime_id = crate::identity_first::live_binding_runtime_id(&binding)
                        .map(|runtime_id| runtime_id.as_str().to_string());
                    // The reset's successor transition retired the previous
                    // live member, so there is no cleanup to report (as for
                    // reset_all). Keep the legacy warning field, always null,
                    // for wire compatibility, as respawn does.
                    let cleanup_warning: Option<Value> = None;
                    runtime
                        .record_console_lifecycle(
                            identity.as_str(),
                            "identity_reset",
                            serde_json::json!({
                                "generation": binding.generation().get(),
                                "cleanup_warning": cleanup_warning.clone(),
                            }),
                        )
                        .await;
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "identity": identity.as_str(),
                            "agent_runtime_id": agent_runtime_id,
                            "session_id": binding.session_id().map(ToString::to_string),
                            "generation": binding.generation().get(),
                            "cleanup_warning": cleanup_warning,
                        })),
                        error: None,
                    }
                }
                Err(e @ crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                    if rpc_live_only_fallback_allowed(&target, identity_str)
                        && let Some(live) = target.live.as_ref()
                    {
                        match Box::pin(respawn_rpc_live_identity(runtime, live)).await {
                            Ok(result) => {
                                runtime
                                    .record_console_lifecycle(
                                        live.identity.as_str(),
                                        "identity_reset",
                                        serde_json::json!({}),
                                    )
                                    .await;
                                JsonRpcResponse {
                                    jsonrpc: JSONRPC_VERSION.to_string(),
                                    id: response_id,
                                    result: Some(result),
                                    error: None,
                                }
                            }
                            Err(err) => JsonRpcResponse {
                                jsonrpc: JSONRPC_VERSION.to_string(),
                                id: response_id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32000,
                                    message: format!("reset failed: {err}"),
                                    data: None,
                                }),
                            },
                        }
                    } else {
                        identity_error_response(response_id, &e)
                    }
                }
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/delete_identity" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            let _registered_status = match identity_rt.status(&identity).await {
                Ok(status) => status,
                Err(e @ crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                    if target.live.is_some() {
                        let response = JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32602,
                                message: format!(
                                    "delete_identity requires durable identity: {} is live-only",
                                    identity.as_str()
                                ),
                                data: Some(serde_json::json!({
                                    "kind": "live_only_identity_delete_unsupported",
                                    "identity": identity.as_str(),
                                })),
                            }),
                        };
                        return if is_notification {
                            String::new()
                        } else {
                            serialize_response(&response)
                        };
                    }
                    let response = identity_error_response(response_id, &e);
                    return if is_notification {
                        String::new()
                    } else {
                        serialize_response(&response)
                    };
                }
                Err(e) => {
                    let response = identity_error_response(response_id, &e);
                    return if is_notification {
                        String::new()
                    } else {
                        serialize_response(&response)
                    };
                }
            };
            let cleanup_handle = runtime.mob_handle();
            let cleanup_identity = identity.clone();
            let include_current = !identity_rt.has_session_bridge();
            let expected_alias = crate::member_comms_id::is_reserved_generated_alias(identity_str)
                .then_some(identity_str);
            let delete_result = identity_rt
                .delete_identity_and_cleanup_live_members_tracked(
                    &identity,
                    expected_alias,
                    move |deleted_alias| async move {
                        let stale_member_ids = stale_rpc_member_ids_for_identity_with_handle(
                            &cleanup_handle,
                            cleanup_identity.as_str(),
                            deleted_alias
                                .as_ref()
                                .map(crate::identity_first::AgentRuntimeId::as_str),
                            include_current,
                        )
                        .await;
                        retire_rpc_member_ids_with_handle(&cleanup_handle, stale_member_ids)
                            .await
                            .err()
                            .map(|error| {
                                serde_json::json!({
                                    "kind": "stale_member_cleanup_failed_after_identity_delete",
                                    "identity": cleanup_identity.as_str(),
                                    "message": error,
                                })
                            })
                    },
                )
                .await;
            match delete_result {
                Ok(cleanup_warning) => {
                    runtime
                        .record_console_lifecycle(
                            identity.as_str(),
                            "identity_deleted",
                            serde_json::json!({
                                "cleanup_warning": cleanup_warning,
                            }),
                        )
                        .await;
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "identity": identity.as_str(),
                            "cleanup_warning": cleanup_warning,
                        })),
                        error: None,
                    }
                }
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/wait_for_completion" => {
            // Server-side completion wait (#468): resolves when the identity's
            // completion cursor moves past `after` (or, with no `after`,
            // counts one completed turn), woken by the runtime's typed
            // completion and identity-table change signal. Nothing polls, and
            // the member session is never touched. `timeout_ms` is the
            // caller's deadline; reaching it is the typed `timed_out` outcome.
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let wait_timeout = match rpc_wait_timeout(&request.params) {
                Ok(wait_timeout) => wait_timeout,
                Err(message) => {
                    return maybe_error_response(is_notification, response_id, -32602, message);
                }
            };
            let after = match request.params.get("after") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    match serde_json::from_value::<crate::identity_first::CompletionCursor>(
                        value.clone(),
                    ) {
                        Ok(cursor) => Some(cursor),
                        Err(err) => {
                            return maybe_error_response(
                                is_notification,
                                response_id,
                                -32602,
                                format!("invalid after cursor: {err}"),
                            );
                        }
                    }
                }
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            let registered = match identity_rt.status(&identity).await {
                Ok(_) => true,
                Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => false,
                Err(e) => {
                    return if is_notification {
                        String::new()
                    } else {
                        serialize_response(&identity_error_response(response_id, &e))
                    };
                }
            };
            if !registered && target.live.is_some() {
                // A raw live alias: no identity authority tracks its turns,
                // so there is nothing to wait on. Null, never a fabricated
                // zero cursor.
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "identity": identity.as_str(),
                        "outcome": "untracked",
                        "completion_cursor": Value::Null,
                    })),
                    error: None,
                }
            } else if !registered && after.is_some() {
                // A baseline names an incarnation of a registered identity.
                // (A readiness wait, with no baseline, may start before its
                // identity materializes.)
                identity_error_response(
                    response_id,
                    &crate::identity_first::IdentityRuntimeError::UnknownIdentity(identity),
                )
            } else {
                let wait = identity_rt
                    .await_completion(&identity, after, wait_timeout)
                    .await;
                let (outcome, cursor) = (wait.wire_str(), wait.cursor());
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "identity": identity.as_str(),
                        "outcome": outcome,
                        "completion_cursor": completion_cursor_json(cursor),
                    })),
                    error: None,
                }
            }
        }
        "meta/completion_cursor" => {
            // Cursor-only completion read for pollers (#468). It reads the
            // identity's in-memory completion cursor and lifecycle entry and
            // never touches the member session: unlike inspect_identity, it
            // sends no execution snapshot to the member's session task, so a
            // tight completion poll cannot hold a staged run.
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            match identity_rt.status(&identity).await {
                Ok(status) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "identity": identity.as_str(),
                        "state": identity_lifecycle_state_json(status.state),
                        "completion_cursor": completion_cursor_json(
                            identity_rt.completion_cursor(&identity).await,
                        ),
                    })),
                    error: None,
                },
                // Raw live aliases are not identity-first owned, so no
                // identity authority tracks their completions: null, as on
                // inspect_identity, never a fabricated zero.
                Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_))
                    if target.live.is_some() =>
                {
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "identity": identity.as_str(),
                            "state": Value::Null,
                            "completion_cursor": Value::Null,
                        })),
                        error: None,
                    }
                }
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/inspect_identity" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => ctx.runtime.runtime(),
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            let identity_str = request
                .params
                .get("identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target =
                match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
                {
                    Ok(target) => target,
                    Err(error) => {
                        return identity_target_error_response(is_notification, response_id, error);
                    }
                };
            let identity = target.identity.clone();
            let status = identity_rt.status(&identity).await;
            let completion_cursor = identity_rt.completion_cursor(&identity).await;
            if let Some(response) =
                rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
            {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&response)
                };
            }
            match identity_rt.inspect(&identity).await {
                Ok(inspection) => {
                    let status = status.ok();
                    let mut result = serde_json::json!({
                            "identity": identity.as_str(),
                            "state": status.as_ref().map(|status| identity_lifecycle_state_json(status.state)),
                            "profile": status.as_ref().and_then(|status| status.profile.as_ref().map(meerkat_mob::ProfileName::as_str)),
                            "addressability": status.as_ref().map(|status| addressability_json(status.addressability)),
                            "display_name": status.as_ref().and_then(|status| status.display_name.as_ref().map(super::identity_first::DisplayName::as_str)),
                            "labels": status.as_ref().map(|status| status.labels.clone()).unwrap_or_default(),
                            "agent_runtime_id": status.as_ref().and_then(|status| status.agent_runtime_id.as_ref().map(super::identity_first::AgentRuntimeId::as_str)),
                            "session_id": status.as_ref().and_then(|status| status.session_id.as_ref().map(ToString::to_string)),
                            "generation": status.as_ref().and_then(|status| status.generation.map(meerkat_mob::Generation::get)),
                            "intent_managed": status.as_ref().is_some_and(|status| status.intent_managed),
                            "output_preview": inspection.output_preview,
                            // Typed (meerkat 0.8.45): why `output_preview`
                            // is not an observation. Null when it is one.
                            "preview_unavailable": inspection.preview_unavailable,
                            "is_final": inspection.is_final,
                            "peer_reachable_count": inspection.peer_reachable_count,
                            // Completion identity. `output_preview` cannot
                            // answer "did a new turn finish?" — two turns may
                            // emit identical text — so pollers compare this
                            // against the baseline their send/dispatch
                            // returned.
                            "completion_cursor": completion_cursor_json(completion_cursor),
                    });
                    if let Some(status) = status.as_ref() {
                        insert_identity_status_extras(&mut result, status);
                    }
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(result),
                        error: None,
                    }
                }
                Err(e @ crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                    if let Some(live) = target.live.as_ref() {
                        JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: Some(rpc_live_identity_inspect_json(runtime, live).await),
                            error: None,
                        }
                    } else {
                        identity_error_response(response_id, &e)
                    }
                }
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        "meta/member_activate_instruction" => {
            let ctx = match identity_ctx {
                Some(ctx) => ctx,
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            Box::pin(operator_methods::handle_member_activate_instruction(
                runtime,
                ctx,
                &request.params,
                response_id,
            ))
            .await
        }
        "meta/member_instruction_activations" => {
            let ctx = match identity_ctx {
                Some(ctx) => ctx,
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            Box::pin(operator_methods::handle_member_instruction_activations(
                runtime,
                ctx,
                &request.params,
                response_id,
            ))
            .await
        }
        "meta/compact_member" => {
            let ctx = match identity_ctx {
                Some(ctx) => ctx,
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            Box::pin(operator_methods::handle_compact_member(
                runtime,
                ctx,
                &request.params,
                response_id,
            ))
            .await
        }
        "meta/bound_member_transcript" => {
            let ctx = match identity_ctx {
                Some(ctx) => ctx,
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            Box::pin(operator_methods::handle_bound_member_transcript(
                runtime,
                ctx,
                &request.params,
                response_id,
            ))
            .await
        }
        "meta/reconcile_identity" => {
            let ctx = match identity_ctx {
                Some(ctx) => ctx,
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
            // The attached runtime context owns the bootstrap policy. Reuse it
            // so lazy deployments never turn an ordinary reconcile into an
            // eager fleet hydration. The fallback preserves tests/embedders
            // that construct the public RPC context without attaching it.
            let reconciled = match runtime.refresh_desired_topology().await {
                Ok(Some(result)) => Ok(result),
                Ok(None) => {
                    let roster_specs = match ctx
                        .roster_provider
                        .roster(&crate::identity_first::RosterContext {
                            mob_definition: ctx.mob_definition.clone(),
                            previous_identities: Vec::new(),
                        })
                        .await
                    {
                        Ok(specs) => specs,
                        Err(e) => {
                            return maybe_error_response(
                                is_notification,
                                response_id,
                                -32603,
                                format!("roster provider failed: {e}"),
                            );
                        }
                    };
                    ctx.runtime
                        .runtime()
                        .restore_flow_tracked(
                            roster_specs,
                            ctx.topology_provider.clone(),
                            ctx.customizer.clone(),
                        )
                        .await
                }
                Err(error) => Err(error),
            };
            match reconciled {
                Ok(result) => {
                    let outcomes: serde_json::Map<String, Value> = result
                        .outcomes
                        .iter()
                        .map(|(id, outcome)| {
                            let val = match outcome {
                                crate::identity_first::RestoreOutcome::Created {
                                    binding,
                                    replaced_session,
                                    ..
                                } => {
                                    serde_json::json!({
                                        "outcome": "created",
                                        "identity": id.as_str(),
                                        "agent_runtime_id": binding.agent_runtime_id.as_str(),
                                        "session_id": binding.session_id.as_ref().map(ToString::to_string),
                                        "replaced_session_id": replaced_session.as_ref().map(ToString::to_string),
                                    })
                                }
                                crate::identity_first::RestoreOutcome::Dormant { .. } => {
                                    serde_json::json!({
                                        "outcome": "dormant",
                                        "identity": id.as_str(),
                                    })
                                }
                                crate::identity_first::RestoreOutcome::Resumed {
                                    binding, ..
                                } => {
                                    serde_json::json!({
                                        "outcome": "resumed",
                                        "identity": id.as_str(),
                                        "agent_runtime_id": binding.agent_runtime_id.as_str(),
                                        "session_id": binding.session_id.as_ref().map(ToString::to_string),
                                    })
                                }
                                crate::identity_first::RestoreOutcome::Refused(error) => {
                                    serde_json::json!({
                                        "outcome": "refused",
                                        "identity": id.as_str(),
                                        "error": {
                                            "code": identity_error_code(error),
                                            "message": error.to_string(),
                                            "data": error.structured_data(),
                                        },
                                    })
                                }
                            };
                            (id.to_string(), val)
                        })
                        .collect();
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "outcomes": outcomes,
                            "managed_edges": result.managed_edges.len(),
                        })),
                        error: None,
                    }
                }
                Err(e) => identity_error_response(response_id, &e),
            }
        }
        // A candidate launch calls no loaded module: a module call is opaque
        // and can act, as a tool call can.
        method if method.contains('/') && !method.starts_with("meta/") => {
            if !runtime.speaks_for_composition() {
                return if is_notification {
                    String::new()
                } else {
                    serialize_response(&candidate_launch_refused(method, response_id))
                };
            }
            let module_id = method
                .split('/')
                .next()
                .map(ToString::to_string)
                .unwrap_or_default();
            let route = runtime
                .route_module_call(
                    &ModuleRouteRequest {
                        module_id: module_id.clone(),
                        method: method.to_string(),
                        params: request.params,
                    },
                    timeout,
                )
                .await;
            match route {
                Ok(response) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "module_id": response.module_id,
                        "method": response.method,
                        "payload": response.payload
                    })),
                    error: None,
                },
                Err(ModuleRouteError::UnloadedModule(module_id)) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!("Module '{module_id}' not loaded"),
                        data: None,
                    }),
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("Module route failed: {err:?}"),
                        data: None,
                    }),
                },
            }
        }
        method
            if method.starts_with("meta/live/")
                || cfg!(feature = "openai-live") && method == "live/webrtc/answer" =>
        {
            match live {
                None => crate::live_wiring::live_unavailable_response(response_id),
                Some(live) => {
                    let params = request.params.clone();
                    let member_alias = live_member_alias(&params);
                    let identity_runtime = identity_ctx
                        .map(|context| context.runtime.runtime())
                        .or_else(|| runtime.identity_runtime_ref());
                    let authority_target = if let Some(alias) = member_alias.as_deref()
                        && let Some(identity_runtime) = identity_runtime
                    {
                        identity_runtime.member_alias_lifecycle_target(alias).await
                    } else {
                        Ok(None)
                    };
                    match authority_target {
                        Err(error) => identity_error_response(response_id, &error),
                        Ok(None)
                            if member_alias.as_deref().is_some_and(
                                crate::member_comms_id::is_reserved_generated_alias,
                            ) =>
                        {
                            JsonRpcResponse {
                                jsonrpc: JSONRPC_VERSION.to_string(),
                                id: response_id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32000,
                                    message: format!(
                                        "generated live target requires current identity authority: {}",
                                        member_alias.as_deref().unwrap_or_default()
                                    ),
                                    data: None,
                                }),
                            }
                        }
                        Ok(Some(target)) => {
                            let canonical_target_identity = target.durable_identity().to_string();
                            let handle = runtime.mob_handle();
                            let identity_runtime = identity_runtime.cloned();
                            let live = live.clone();
                            let method = method.to_string();
                            let operation_response_id = response_id.clone();
                            match crate::identity_first::IdentityRuntime::run_member_alias_targets_operation_tracked(
                            vec![target],
                            move || async move {
                                let session = resolve_live_target(
                                    &handle,
                                    identity_runtime.as_ref(),
                                    true,
                                    &params,
                                )
                                .await?;
                                Ok(live.dispatch(
                                    crate::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
                                    session,
                                    Some(canonical_target_identity),
                                    method,
                                    params,
                                    operation_response_id,
                                )
                                .await)
                            },
                        )
                        .await
                        {
                            Ok(response) => response,
                            Err(error) => identity_error_response(response_id, &error),
                        }
                        }
                        Ok(None) => {
                            match resolve_live_target(&runtime.mob_handle(), None, false, &params).await
                        {
                            Ok(session) => {
                                live.dispatch(
                                    crate::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
                                    session,
                                    None,
                                    method.to_string(),
                                    params,
                                    response_id,
                                )
                                .await
                            }
                            Err(error) => JsonRpcResponse {
                                jsonrpc: JSONRPC_VERSION.to_string(),
                                id: response_id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32000,
                                    message: format!("live target resolution failed: {error}"),
                                    data: None,
                                }),
                            },
                        }
                        }
                    }
                }
            }
        }
        method if crate::mob_announcements::rpc::is_announcement_method(method) => {
            let service = runtime.mob_announcements();
            let outcome = match crate::mob_announcements::MobId::new(
                runtime.mob_handle().mob_id().to_string(),
            ) {
                Ok(own_mob) => {
                    crate::mob_announcements::rpc::handle_announcement_method(
                        service.as_ref(),
                        &own_mob,
                        method,
                        &request.params,
                    )
                    .await
                }
                Err(error) => Err(JsonRpcError::new(-32000, error.to_string())),
            };
            match outcome {
                Ok(result) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(result),
                    error: None,
                },
                Err(error) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(error),
                },
            }
        }
        method if workgraph_methods::is_workgraph_method(method) => {
            let service = runtime.workgraph_service();
            let admission = runtime.workgraph_admission();
            // The stdin surface is host-trusted; no wire principal exists to
            // promote into goal/confirm.
            match workgraph_methods::handle_workgraph_method(
                service.as_ref(),
                &admission,
                workgraph_methods::WorkgraphSurface::HostStdin,
                method,
                &request.params,
            )
            .await
            {
                Ok(result) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(result),
                    error: None,
                },
                Err(error) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(error),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        },
    };
    if is_notification {
        String::new()
    } else {
        serialize_response(&response)
    }
}

fn build_models_catalog_result() -> Value {
    let entries: Vec<Value> = meerkat_models::catalog()
        .iter()
        .filter_map(|e| {
            let mut val = serde_json::to_value(e).ok()?;
            if let Some(provider) = meerkat_core::Provider::parse_strict(e.provider)
                && let Some(profile) = meerkat_models::profile_for(provider, e.id)
                && let Ok(p) = serde_json::to_value(&profile)
            {
                val["profile"] = p;
            }
            Some(val)
        })
        .collect();
    let defaults: Vec<Value> = meerkat_models::provider_defaults()
        .iter()
        .filter_map(|d| serde_json::to_value(d).ok())
        .collect();
    serde_json::json!({
        "models": entries,
        "provider_defaults": defaults,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct RpcLiveIdentityAlias {
    pub(crate) identity: crate::identity_first::AgentIdentity,
    pub(crate) runtime_member_id: String,
    pub(crate) member: meerkat_mob::runtime::MobMemberListEntry,
    pub(crate) session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RpcIdentityControlTarget {
    pub(crate) identity: crate::identity_first::AgentIdentity,
    pub(crate) live: Option<RpcLiveIdentityAlias>,
    /// True when resolution observed this identity in the durable runtime.
    /// Live-only fallback must never be used after a later `UnknownIdentity`
    /// in this case: that transition means a concurrent delete won the race.
    pub(crate) was_registered: bool,
}

/// Whether an `UnknownIdentity` observed after target resolution may use the
/// legacy live-member compatibility path. Once durable ownership was seen,
/// a later miss means a concurrent delete won and must remain authoritative.
/// Generated `rt:*` names are never raw/live-only identities.
fn rpc_live_only_fallback_allowed(
    target: &RpcIdentityControlTarget,
    requested_identity: &str,
) -> bool {
    !target.was_registered
        && !crate::member_comms_id::is_reserved_generated_alias(requested_identity)
        && target.live.as_ref().is_some_and(|live| {
            // Resolution can race an identity delete after the durable entry
            // is removed but before its generated member is retired. Never
            // reinterpret such an identity-owned generation as a legacy raw
            // member, even when the request used the durable identity name.
            !crate::member_comms_id::is_reserved_generated_alias(&live.runtime_member_id)
        })
}

fn rpc_live_identity_alias_member_visible(
    member: &meerkat_mob::runtime::MobMemberListEntry,
) -> bool {
    rpc_live_identity_alias_visible(member.role.as_str(), &member.labels)
}

fn rpc_live_identity_alias_visible(
    member_role: &str,
    labels: &std::collections::BTreeMap<String, String>,
) -> bool {
    let projected_role = labels
        .get("role")
        .map(String::as_str)
        .unwrap_or(member_role);
    !is_implicit_delegate_member(member_role, labels)
        && !is_implicit_delegate_member(projected_role, labels)
}

impl TryFrom<crate::identity_control_target::LiveIdentityMember> for RpcLiveIdentityAlias {
    type Error = String;

    fn try_from(
        live: crate::identity_control_target::LiveIdentityMember,
    ) -> Result<Self, Self::Error> {
        let identity = crate::identity_first::AgentIdentity::parse(&live.identity)
            .map_err(|error| format!("invalid projected identity {}: {error}", live.identity))?;
        Ok(Self {
            identity,
            runtime_member_id: live.runtime_member_id,
            member: live.member,
            session_id: live.session_id,
        })
    }
}

/// Why a stdio identity-control target did not resolve.
#[derive(Debug)]
pub(crate) enum RpcIdentityTargetError {
    /// The request names no valid target. Answered `-32602 invalid
    /// identity: <detail>`, with the typed data `error_response` derives
    /// from the detail.
    Invalid(String),
    /// A generated runtime id of an identity the identity runtime still
    /// knows, but not its binding: answered with the typed stale-binding
    /// refusal (see [`rpc_stale_generated_alias_error`]).
    StaleRuntimeBinding(Box<JsonRpcError>),
}

impl std::fmt::Display for RpcIdentityTargetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(detail) => formatter.write_str(detail),
            Self::StaleRuntimeBinding(error) => formatter.write_str(&error.message),
        }
    }
}

impl std::error::Error for RpcIdentityTargetError {}

impl RpcIdentityTargetError {
    fn into_response(self, response_id: Value) -> JsonRpcResponse {
        match self {
            Self::Invalid(detail) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("invalid identity: {detail}"),
                    data: None,
                }),
            },
            Self::StaleRuntimeBinding(error) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(*error),
            },
        }
    }
}

/// The response to a stdio identity-control target that did not resolve.
fn identity_target_error_response(
    is_notification: bool,
    response_id: Value,
    error: RpcIdentityTargetError,
) -> String {
    match error {
        RpcIdentityTargetError::Invalid(detail) => maybe_error_response(
            is_notification,
            response_id,
            -32602,
            format!("invalid identity: {detail}"),
        ),
        stale @ RpcIdentityTargetError::StaleRuntimeBinding(_) => {
            if is_notification {
                String::new()
            } else {
                serialize_response(&stale.into_response(response_id))
            }
        }
    }
}

/// The refusal of a generated runtime id the shared resolver left
/// unresolved: no identity is bound to it and no live member carries it.
/// When it names an identity the identity runtime still knows, that
/// identity's binding moved on (a reset published a new generation and
/// rebound its one roster row), so it answers the typed stale-binding
/// refusal the member methods give for the same id, naming the registered
/// binding. An identity the identity runtime does not know keeps "runtime
/// identity not found" (`None`).
async fn rpc_stale_generated_alias_error(
    identity_rt: &crate::identity_first::IdentityRuntime,
    alias: &str,
) -> Option<RpcIdentityTargetError> {
    let identity = mob_methods::identity_from_runtime_alias(alias)?;
    let status = identity_rt.status(&identity).await.ok()?;
    // A stale id names the registered binding only of an identity the
    // stdio surface shows, by the rule its live aliases follow.
    let profile = status
        .profile
        .as_ref()
        .map(meerkat_mob::ProfileName::as_str)
        .unwrap_or_default();
    if !rpc_live_identity_alias_visible(profile, &status.labels) {
        return Some(RpcIdentityTargetError::Invalid(format!(
            "identity hidden by policy: {alias}"
        )));
    }
    let registered = status
        .agent_runtime_id
        .as_ref()
        .map(crate::identity_first::AgentRuntimeId::as_str);
    if registered == Some(alias) {
        return None;
    }
    Some(RpcIdentityTargetError::StaleRuntimeBinding(Box::new(
        crate::identity_control_target::stale_identity_runtime_binding_error(
            "",
            &identity,
            Some(&status),
            alias,
            None,
        ),
    )))
}

async fn resolve_rpc_identity_control_target(
    runtime: &UnifiedRuntime,
    identity_rt: &crate::identity_first::IdentityRuntime,
    requested_identity: &str,
) -> Result<RpcIdentityControlTarget, RpcIdentityTargetError> {
    resolve_rpc_identity_control_target_with_handle(
        &runtime.mob_handle(),
        identity_rt,
        requested_identity,
    )
    .await
}

pub(crate) async fn resolve_rpc_identity_control_target_with_handle(
    handle: &meerkat_mob::MobHandle,
    identity_rt: &crate::identity_first::IdentityRuntime,
    requested_identity: &str,
) -> Result<RpcIdentityControlTarget, RpcIdentityTargetError> {
    use crate::identity_control_target::IdentityControlResolution;

    let resolution = crate::identity_control_target::resolve_identity_control_target(
        handle,
        Some(identity_rt),
        requested_identity,
        |live| rpc_live_identity_alias_member_visible(&live.member),
    )
    .await
    .map_err(|error| RpcIdentityTargetError::Invalid(error.to_string()))?;

    match resolution {
        IdentityControlResolution::Resolved(target) => {
            let target = *target;
            Ok(RpcIdentityControlTarget {
                identity: target.identity,
                live: target
                    .live
                    .map(RpcLiveIdentityAlias::try_from)
                    .transpose()
                    .map_err(RpcIdentityTargetError::Invalid)?,
                was_registered: target.was_registered,
            })
        }
        IdentityControlResolution::Unresolved {
            requested_identity,
            parsed_identity,
            generated_runtime_alias,
        } => {
            if generated_runtime_alias {
                if let Some(error) =
                    rpc_stale_generated_alias_error(identity_rt, &requested_identity).await
                {
                    return Err(error);
                }
                return Err(RpcIdentityTargetError::Invalid(format!(
                    "runtime identity not found: {requested_identity}"
                )));
            }
            Ok(RpcIdentityControlTarget {
                identity: parsed_identity.map_err(RpcIdentityTargetError::Invalid)?,
                live: None,
                was_registered: false,
            })
        }
    }
}

fn rpc_reset_requires_session_bridge_response(response_id: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message: "reset requires an identity runtime with a session bridge".to_string(),
            data: Some(serde_json::json!({
                "kind": "identity_reset_requires_session_bridge",
            })),
        }),
    }
}

fn rpc_live_alias_matches_status_runtime(
    alias: Option<&RpcLiveIdentityAlias>,
    status: &crate::identity_first::IdentityStatus,
) -> bool {
    let Some(alias) = alias else {
        return true;
    };
    // A registered binding must exist. A control call naming a live member for
    // an identity that has no runtime binding at all is not a match.
    if status.agent_runtime_id.is_none() {
        return false;
    }
    let registered_session = status.session_id.as_ref().map(ToString::to_string);
    // One centralized rule: live roster id decoded exactly to the durable
    // identity, plus EXACT session equality (a one-sided missing session fails
    // closed). `agent_runtime_id` stays binding bookkeeping and is not the
    // roster spelling.
    crate::member_comms_id::live_binding_matches_identity(
        &alias.runtime_member_id,
        alias.session_id.as_deref(),
        status.identity.as_str(),
        registered_session.as_deref(),
        status
            .agent_runtime_id
            .as_ref()
            .map(crate::identity_first::AgentRuntimeId::as_str),
    ) && alias.identity == status.identity
}

async fn rpc_stale_live_alias_error_response(
    identity_rt: &crate::identity_first::IdentityRuntime,
    target: &RpcIdentityControlTarget,
    response_id: Value,
) -> Option<JsonRpcResponse> {
    let live = target.live.as_ref()?;
    let Ok(status) = identity_rt.status(&target.identity).await else {
        return None;
    };
    if rpc_live_alias_matches_status_runtime(Some(live), &status) {
        return None;
    }
    Some(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(
            crate::identity_control_target::stale_identity_runtime_binding_error(
                "",
                &target.identity,
                Some(&status),
                &live.runtime_member_id,
                live.session_id.as_deref(),
            ),
        ),
    })
}

fn rpc_member_is_addressable(member: &meerkat_mob::runtime::MobMemberListEntry) -> bool {
    member
        .labels
        .get("addressable")
        .map(|value| !value.eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

fn rpc_live_identity_status_json(alias: &RpcLiveIdentityAlias) -> Value {
    serde_json::json!({
        "state": crate::mob_handle_runtime::member_status_state_string(alias.member.status),
        "identity": alias.identity.as_str(),
        "agent_runtime_id": alias.runtime_member_id,
        "session_id": alias.session_id,
        "profile": alias.member.role.to_string(),
        "addressability": if rpc_member_is_addressable(&alias.member) { "addressable" } else { "internal_only" },
        "display_name": alias.member.labels.get("display_name"),
        "labels": alias.member.labels,
        "generation": Value::Null,
        "intent_managed": false,
    })
}

async fn rpc_live_identity_inspect_json(
    runtime: &UnifiedRuntime,
    alias: &RpcLiveIdentityAlias,
) -> Value {
    let snapshot = runtime
        .mob_handle()
        .member_status(&crate::member_comms_id::mob_member_id(
            alias.runtime_member_id.as_str(),
        ))
        .await
        .ok();
    serde_json::json!({
        "identity": alias.identity.as_str(),
        "state": crate::mob_handle_runtime::member_status_state_string(alias.member.status),
        "profile": alias.member.role.to_string(),
        "addressability": if rpc_member_is_addressable(&alias.member) { "addressable" } else { "internal_only" },
        "display_name": alias.member.labels.get("display_name"),
        "labels": alias.member.labels,
        "agent_runtime_id": alias.runtime_member_id,
        "session_id": alias.session_id,
        "generation": Value::Null,
        "intent_managed": false,
        "output_preview": snapshot.as_ref().and_then(|snapshot| snapshot.output_preview.clone()),
        "preview_unavailable": snapshot.as_ref().and_then(|snapshot| snapshot.preview_unavailable),
        "is_final": snapshot.as_ref().map(|snapshot| snapshot.is_final).unwrap_or(false),
        "peer_reachable_count": alias.member.wired_to.len(),
        // Raw live aliases are not identity-first owned, so no identity
        // authority tracks their completions. Null rather than a fabricated
        // zero: a client must not mistake "not tracked" for "no turns yet".
        "completion_cursor": Value::Null,
        // Machine-owned liveness projection (meerkat 0.7.29, ask 14):
        // run_state / in_flight_work / health for operator triage.
        "progress": snapshot.as_ref().and_then(|snapshot| snapshot.progress.clone()),
    })
}

async fn retire_rpc_live_identity(
    runtime: &UnifiedRuntime,
    alias: &RpcLiveIdentityAlias,
) -> Result<(), String> {
    retire_rpc_runtime_member_id(runtime, alias.runtime_member_id.as_str()).await
}

async fn retire_rpc_runtime_member_id(
    runtime: &UnifiedRuntime,
    runtime_member_id: &str,
) -> Result<(), String> {
    retire_rpc_runtime_member_id_with_handle(&runtime.mob_handle(), runtime_member_id).await
}

async fn retire_rpc_runtime_member_id_with_handle(
    handle: &meerkat_mob::MobHandle,
    runtime_member_id: &str,
) -> Result<(), String> {
    match handle
        .retire(crate::member_comms_id::mob_member_id(runtime_member_id))
        .await
    {
        Ok(()) => Ok(()),
        Err(err) if mob_methods::lifecycle_archive_cleanup_completed(&err.to_string()) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn rpc_member_id_matches_durable_identity(member_id: &str, durable_identity: &str) -> bool {
    // Roster ids are comms-safe encodings of public aliases (meerkat 0.7
    // MemberCommsName); compare in the public alias space.
    crate::member_comms_id::runtime_alias_str(member_id) == durable_identity
}

fn rpc_runtime_alias_generation(alias: &str, durable_identity: &str) -> Option<u64> {
    let alias = crate::member_comms_id::runtime_alias_str(alias);
    let rest = alias.strip_prefix("rt:")?;
    let (identity, generation) = rest.rsplit_once(':')?;
    if identity != durable_identity {
        return None;
    }
    generation.parse().ok()
}

async fn stale_rpc_member_ids_for_identity_with_handle(
    handle: &meerkat_mob::MobHandle,
    durable_identity: &str,
    current_runtime_member_id: Option<&str>,
    include_current: bool,
) -> Vec<String> {
    let Some(current_generation) = current_runtime_member_id
        .and_then(|alias| rpc_runtime_alias_generation(alias, durable_identity))
    else {
        return Vec::new();
    };
    handle
        .list_members_including_retiring()
        .await
        .into_iter()
        .filter(|member| {
            if !rpc_live_identity_alias_member_visible(member) {
                return false;
            }
            let matches_identity =
                rpc_member_id_matches_durable_identity(
                    member.agent_identity.as_str(),
                    durable_identity,
                ) || crate::member_comms_id::durable_identity_label(&member.labels)
                    .is_some_and(|identity| identity == durable_identity);
            let public_alias =
                crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str());
            matches_identity
                && rpc_runtime_alias_generation(public_alias.as_ref(), durable_identity)
                    .is_some_and(|generation| {
                        generation < current_generation
                            || (include_current && generation == current_generation)
                    })
        })
        // `retire_rpc_runtime_member_id` re-encodes; hand it the alias.
        .map(|member| {
            crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str()).into_owned()
        })
        .collect()
}

async fn retire_rpc_member_ids_with_handle(
    handle: &meerkat_mob::MobHandle,
    member_ids: Vec<String>,
) -> Result<(), String> {
    for member_id in member_ids {
        retire_rpc_runtime_member_id_with_handle(handle, &member_id).await?;
    }
    Ok(())
}

async fn respawn_rpc_live_identity(
    runtime: &UnifiedRuntime,
    alias: &RpcLiveIdentityAlias,
) -> Result<Value, String> {
    let mut result = Box::pin(respawn_rpc_runtime_member_id(
        runtime,
        alias.runtime_member_id.as_str(),
    ))
    .await?;
    result["identity"] = serde_json::json!(alias.identity.as_str());
    Ok(result)
}

async fn respawn_rpc_runtime_member_id(
    runtime: &UnifiedRuntime,
    runtime_member_id: &str,
) -> Result<Value, String> {
    respawn_rpc_runtime_member_id_with_handle(
        &runtime.mob_handle(),
        runtime
            .mob_runtime()
            .implicit_delegate_retirement_overrides()
            .as_ref(),
        runtime_member_id,
    )
    .await
}

async fn respawn_rpc_runtime_member_id_with_handle(
    handle: &meerkat_mob::MobHandle,
    idle_retire_overrides: Option<&crate::mob_handle_runtime::ImplicitDelegateRetirementOverrides>,
    runtime_member_id: &str,
) -> Result<Value, String> {
    let member_id = crate::member_comms_id::mob_member_id(runtime_member_id);
    // Best-effort repair material: a faulted lookup degrades to None (the
    // respawn itself surfaces real faults).
    let entry_before_respawn = handle.get_member(&member_id).await.ok().flatten();
    let mut topology_restore_warning = None;
    match crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
        idle_retire_overrides,
        handle,
        &member_id,
        handle.respawn(member_id.clone(), None),
        Result::is_ok,
    )
    .await
    {
        Ok(_receipt) => {}
        Err(err) => {
            if let Some(failed_peer_ids) = topology_restore_failed_peer_ids(&err) {
                tracing::warn!(
                    member_id = %member_id,
                    failed_peer_count = failed_peer_ids.len(),
                    failed_peer_ids = ?failed_peer_ids,
                    "rpc member respawn restored member with isolated peer edges; continuing degraded respawn"
                );
                topology_restore_warning = Some(topology_restore_warning_json(&failed_peer_ids));
            } else if mob_methods::lifecycle_archive_cleanup_completed(&err.to_string()) {
                // A faulted lookup must not read as "absent" (that would mint
                // a spurious replacement member); surface it instead.
                if handle
                    .get_member(&member_id)
                    .await
                    .map_err(|lookup_err| lookup_err.to_string())?
                    .is_none()
                    && let Some(entry) = entry_before_respawn
                {
                    let mut spec =
                        meerkat_mob::SpawnMemberSpec::new(entry.role.clone(), member_id.clone());
                    if !entry.labels.is_empty() {
                        spec = spec.with_labels(entry.labels.clone());
                    }
                    handle
                        .ensure_member(spec)
                        .await
                        .map_err(|ensure_err| ensure_err.to_string())?;
                }
            } else {
                return Err(err.to_string());
            }
        }
    }
    let session_id = handle
        .resolve_bridge_session_id_observation(&member_id)
        .await
        .map(|session_id| session_id.to_string());
    Ok(serde_json::json!({
        "agent_runtime_id": runtime_member_id,
        "session_id": session_id,
        "generation": Value::Null,
        "topology_restore_warning": topology_restore_warning,
    }))
}

fn identity_not_configured(response_id: Value) -> String {
    error_response(response_id, -32601, "identity-first runtime not configured")
}

fn maybe_identity_not_configured(is_notification: bool, response_id: Value) -> String {
    if is_notification {
        String::new()
    } else {
        identity_not_configured(response_id)
    }
}

/// The caller's deadline for a server-side wait: `timeout_ms` when given,
/// else the dispatcher's RPC timeout.
/// Default deadline of a server-side wait (`meta/wait_for_completion`,
/// `meta/wait_for_turn`) when the caller gives no `timeout_ms`.
pub const RPC_WAIT_DEFAULT_TIMEOUT: Duration = Duration::from_mins(1);
/// Longest deadline one server-side wait accepts; a longer `timeout_ms` is
/// clamped to it (the SDKs re-issue the wait until their own deadline).
pub const RPC_WAIT_MAX_TIMEOUT: Duration = Duration::from_mins(10);

/// The caller's deadline for a server-side wait: `timeout_ms` when given
/// (clamped to [`RPC_WAIT_MAX_TIMEOUT`]), else [`RPC_WAIT_DEFAULT_TIMEOUT`].
/// Deliberately independent of the dispatcher's generic RPC timeout.
fn rpc_wait_timeout(params: &Value) -> Result<Duration, String> {
    match params.get("timeout_ms") {
        None | Some(Value::Null) => Ok(RPC_WAIT_DEFAULT_TIMEOUT),
        Some(value) => value
            .as_u64()
            .map(|ms| Duration::from_millis(ms).min(RPC_WAIT_MAX_TIMEOUT))
            .ok_or_else(|| "timeout_ms must be a non-negative integer".to_string()),
    }
}

/// Wire form of a [`CompletionCursor`]: the comparable atom a client polls to
/// learn that a NEW turn finished, without ever comparing output text.
///
/// [`CompletionCursor`]: crate::identity_first::CompletionCursor
fn completion_cursor_json(cursor: crate::identity_first::CompletionCursor) -> Value {
    serde_json::json!({
        "epoch": cursor.epoch.get(),
        "turns": cursor.turns,
    })
}

fn addressability_json(addressability: crate::identity_first::AgentAddressability) -> &'static str {
    match addressability {
        crate::identity_first::AgentAddressability::Addressable => "addressable",
        crate::identity_first::AgentAddressability::InternalOnly => "internal_only",
    }
}

/// Wire vocabulary for identity-first lifecycle states — see
/// [`crate::identity_first::IdentityLifecycleState::wire_str`].
fn identity_lifecycle_state_json(
    state: crate::identity_first::IdentityLifecycleState,
) -> &'static str {
    state.wire_str()
}

/// The `meta/status_identity` answer of a registered identity: its
/// roster policy and Core's facts, read per call. `agent_runtime_id`,
/// `session_id` and `generation` are always present (null when Core holds
/// no binding); `convergence`, `wake_refusal` and `replaced_session` only
/// when set.
fn identity_status_wire(status: &crate::identity_first::IdentityStatus) -> Value {
    let mut result = serde_json::json!({
        "state": identity_lifecycle_state_json(status.state),
        "identity": status.identity.as_str(),
        "agent_runtime_id": status.agent_runtime_id.as_ref().map(super::identity_first::AgentRuntimeId::as_str),
        "session_id": status.session_id.as_ref().map(ToString::to_string),
        "profile": status.profile.as_ref().map(meerkat_mob::ProfileName::as_str),
        "addressability": addressability_json(status.addressability),
        "display_name": status.display_name.as_ref().map(super::identity_first::DisplayName::as_str),
        "labels": status.labels,
        "generation": status.generation.map(meerkat_mob::Generation::get),
        "intent_managed": status.intent_managed,
    });
    insert_identity_status_extras(&mut result, status);
    result
}

/// The optional status fields: Core's mob member id of the identity's
/// binding, Core's convergence of a host-adopted identity, the last refused
/// wake, and the session the last wake replaced.
fn insert_identity_status_extras(
    result: &mut Value,
    status: &crate::identity_first::IdentityStatus,
) {
    let Value::Object(fields) = result else {
        return;
    };
    if let Some(member_id) = status.member_id.as_ref() {
        fields.insert("member_id".to_string(), Value::String(member_id.clone()));
    }
    if let Some(convergence) = status.convergence.as_ref()
        && let Ok(value) = serde_json::to_value(convergence)
    {
        fields.insert("convergence".to_string(), value);
    }
    if let Some(wake_refusal) = status.wake_refusal.as_ref()
        && let Ok(value) = serde_json::to_value(wake_refusal)
    {
        fields.insert("wake_refusal".to_string(), value);
    }
    if let Some(replaced_session) = status.replaced_session.as_ref()
        && let Ok(value) = serde_json::to_value(replaced_session)
    {
        fields.insert("replaced_session".to_string(), value);
    }
}

/// `track_turn: true` on `meta/send`, `meta/interact` or
/// `meta/dispatch` asks for the admitted turn to be tracked by ticket.
fn rpc_track_turn_requested(params: &Value) -> bool {
    params
        .get("track_turn")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// A tracked delivery's `turn`: `{"ticket": ...}` when the admitted turn is
/// tracked; `turn: null` plus `turn_unavailable: {"code", "reason",
/// "delivered"}` when it cannot be tracked (`code` is the stable
/// `TurnUntrackable` code, for example `autonomous_host`; `delivered` is
/// false only for `not_delivered`). Nothing when tracking was not requested.
fn insert_turn_tracking(result: &mut Value, turn: Option<&crate::identity_first::TurnTracking>) {
    let (Some(turn), Value::Object(fields)) = (turn, result) else {
        return;
    };
    match turn {
        crate::identity_first::TurnTracking::Tracked(ticket) => {
            fields.insert(
                "turn".to_string(),
                serde_json::json!({ "ticket": ticket.to_string() }),
            );
        }
        crate::identity_first::TurnTracking::Unavailable(reason) => {
            fields.insert("turn".to_string(), Value::Null);
            fields.insert(
                "turn_unavailable".to_string(),
                serde_json::json!({
                    "code": reason.code(),
                    "reason": reason.to_string(),
                    "delivered": reason.delivered(),
                }),
            );
        }
    }
}

/// `meta/turn_result`'s typed outcome: `state` is `pending`, `completed`,
/// `failed` or `unknown`. A completed turn carries `output_status` (`text`,
/// `empty`, `no_own_result` or `unavailable`), its own `output` text (only for
/// `text`) and `output_truncated`.
fn turn_outcome_json(outcome: &crate::identity_first::TurnOutcome) -> Value {
    use crate::identity_first::{TurnOutcome, TurnOutput};
    match outcome {
        TurnOutcome::Pending => serde_json::json!({ "state": "pending" }),
        TurnOutcome::Completed { output } => serde_json::json!({
            "state": "completed",
            "output_status": output.code(),
            "output": output.text(),
            "output_truncated": matches!(output, TurnOutput::Text { truncated: true, .. }),
        }),
        TurnOutcome::Failed { reason } => serde_json::json!({
            "state": "failed",
            "error": reason,
        }),
        TurnOutcome::Unknown => serde_json::json!({ "state": "unknown" }),
    }
}

/// JSON-RPC code for a delivery scope that no longer matches the identity's
/// binding (`StaleScopeError` in the SDKs). Nothing was submitted.
pub const STALE_DELIVERY_SCOPE_CODE: i64 = -32007;

/// JSON-RPC code of a wake Core refused for good: Core cannot
/// resume the identity's last session (archived beyond revival, or its
/// durable authority contradicts its body). Nothing was spawned;
/// `delete_identity` starts a fresh life.
pub const IDENTITY_UNRESUMABLE_CODE: i64 = -32051;

/// JSON-RPC code of a retryable wake: Core's member history could
/// not be read, or the wake failed transiently. Nothing was spawned; retry.
pub const IDENTITY_HISTORY_UNAVAILABLE_CODE: i64 = -32052;

/// JSON-RPC code of a Meta lifecycle verb on a host-adopted identity:
/// Core holds its identity intent row and drives it.
pub const IDENTITY_HOST_ADOPTED_CODE: i64 = -32053;

/// A scoped dispatch or recovery error: a stale scope has its own code, an
/// unsupported bridge is a capability gap, and every class carries `data.kind`
/// plus `data.admission_possible` (true only for an uncertain admission).
fn scoped_delivery_error_response(
    response_id: Value,
    err: &crate::identity_first::ScopedDeliveryError,
) -> JsonRpcResponse {
    use crate::identity_first::ScopedDeliveryError;
    let code = match err {
        ScopedDeliveryError::StaleScope { .. } => STALE_DELIVERY_SCOPE_CODE,
        ScopedDeliveryError::Unsupported { .. } => -32004,
        _ => -32603,
    };
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: err.to_string(),
            data: Some(err.structured_data()),
        }),
    }
}

fn invalid_delivery_scope_response(
    response_id: Value,
    param: &str,
    err: &crate::identity_first::DeliveryScopeError,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message: format!("invalid {param}: {err}"),
            data: Some(err.structured_data()),
        }),
    }
}

/// `meta/status_identity`'s `delivery_scope`: the versioned scope a host
/// persists before a scoped dispatch, or `null` plus
/// `delivery_scope_unavailable: {"kind", "reason"}` when none can be captured.
fn insert_delivery_scope(
    result: &mut Value,
    scope: Result<crate::identity_first::DeliveryScope, crate::identity_first::ScopedDeliveryError>,
) {
    let Value::Object(fields) = result else {
        return;
    };
    match scope {
        Ok(scope) => {
            fields.insert("delivery_scope".to_string(), scope.to_json_value());
        }
        Err(error) => {
            fields.insert("delivery_scope".to_string(), Value::Null);
            fields.insert(
                "delivery_scope_unavailable".to_string(),
                serde_json::json!({ "kind": error.kind(), "reason": error.to_string() }),
            );
        }
    }
}

/// A scoped dispatch's result: meerkat's receipt (`work_ref`, the `stage` it
/// proves, the admitted `session_id`) and the scope it was admitted under.
fn scoped_dispatch_receipt_json(receipt: &crate::identity_first::ScopedDispatchReceipt) -> Value {
    serde_json::json!({
        "receipt": {
            "work_ref": receipt.work_ref,
            "stage": receipt.stage,
            "session_id": receipt.session_id.to_string(),
        },
        "delivery_scope": receipt.scope.to_json_value(),
        "fencing_token": receipt.scope.lease_fencing_token().get(),
    })
}

/// `meta/recover_delivery`'s typed state: `absent`, `in_flight`,
/// `completed`, `failed`, `terminal_without_run`, `broken` or `unresolved`.
/// `absent` and `unresolved` are never permission to retry.
fn scoped_recovery_json(recovery: &crate::identity_first::ScopedRecovery) -> Value {
    use crate::identity_first::{ScopedRecovery, ScopedRecoveryUnresolved, TurnOutput};
    match recovery {
        ScopedRecovery::Absent => serde_json::json!({ "state": "absent" }),
        ScopedRecovery::InFlight {
            input_id,
            phase,
            durable_witness,
        } => serde_json::json!({
            "state": "in_flight",
            "input_id": input_id,
            "phase": phase,
            "durable_witness": durable_witness,
        }),
        ScopedRecovery::Completed { input_id, output } => serde_json::json!({
            "state": "completed",
            "input_id": input_id,
            "output_status": output.code(),
            "output": output.text(),
            "output_truncated": matches!(output, TurnOutput::Text { truncated: true, .. }),
        }),
        ScopedRecovery::Failed { input_id, error } => serde_json::json!({
            "state": "failed",
            "input_id": input_id,
            "error": error,
        }),
        ScopedRecovery::TerminalWithoutRun {
            input_id,
            terminal,
            last_run_id,
        } => serde_json::json!({
            "state": "terminal_without_run",
            "input_id": input_id,
            "terminal": terminal,
            "last_run_id": last_run_id,
        }),
        ScopedRecovery::Broken { input_id, reason } => serde_json::json!({
            "state": "broken",
            "input_id": input_id,
            "reason": reason,
        }),
        ScopedRecovery::Unresolved { cause } => serde_json::json!({
            "state": "unresolved",
            "cause": cause.code(),
            "detail": match cause {
                ScopedRecoveryUnresolved::OriginalOwnerUnavailable { detail } => Some(detail),
                _ => None,
            },
        }),
    }
}

#[cfg(test)]
mod scoped_delivery_json_tests {
    use super::scoped_recovery_json;
    use crate::identity_first::{ScopedRecovery, ScopedRecoveryUnresolved, TurnOutput};

    /// G9 at the gateway: every recovery class keeps its own wire state and
    /// fields; none collapses into another (an unresolved read is never
    /// absence, a run-less terminal is never completion).
    #[test]
    fn every_recovery_class_has_its_own_wire_state() {
        let cases = [
            (ScopedRecovery::Absent, "absent"),
            (
                ScopedRecovery::InFlight {
                    input_id: "in-1".to_string(),
                    phase: serde_json::json!("queued"),
                    durable_witness: true,
                },
                "in_flight",
            ),
            (
                ScopedRecovery::Completed {
                    input_id: "in-1".to_string(),
                    output: TurnOutput::Text {
                        text: "answer".to_string(),
                        truncated: true,
                    },
                },
                "completed",
            ),
            (
                ScopedRecovery::Failed {
                    input_id: "in-1".to_string(),
                    error: "boom".to_string(),
                },
                "failed",
            ),
            (
                ScopedRecovery::TerminalWithoutRun {
                    input_id: "in-1".to_string(),
                    terminal: serde_json::json!({"outcome_type": "abandoned"}),
                    last_run_id: None,
                },
                "terminal_without_run",
            ),
            (
                ScopedRecovery::Broken {
                    input_id: None,
                    reason: "inconsistent".to_string(),
                },
                "broken",
            ),
            (
                ScopedRecovery::Unresolved {
                    cause: ScopedRecoveryUnresolved::OriginalOwnerUnavailable {
                        detail: "store down".to_string(),
                    },
                },
                "unresolved",
            ),
        ];
        for (recovery, state) in &cases {
            assert_eq!(
                scoped_recovery_json(recovery)["state"],
                *state,
                "{recovery:?}"
            );
        }
        let in_flight = scoped_recovery_json(&cases[1].0);
        assert_eq!(in_flight["durable_witness"], true);
        assert_eq!(in_flight["phase"], "queued");
        let completed = scoped_recovery_json(&cases[2].0);
        assert_eq!(completed["output_status"], "text");
        assert_eq!(completed["output"], "answer");
        assert_eq!(completed["output_truncated"], true);
        let unresolved = scoped_recovery_json(&cases[6].0);
        assert_eq!(unresolved["cause"], "original_owner_unavailable");
        assert_eq!(unresolved["detail"], "store down");
        let no_own_result = scoped_recovery_json(&ScopedRecovery::Completed {
            input_id: "in-2".to_string(),
            output: TurnOutput::NoOwnResult,
        });
        assert_eq!(no_own_result["output_status"], "no_own_result");
        assert!(no_own_result["output"].is_null());
    }
}

/// The JSON-RPC code of an identity runtime error. -32003 and -32005 (the
/// retired lease codes) are never answered again.
pub(crate) fn identity_error_code(err: &crate::identity_first::IdentityRuntimeError) -> i64 {
    use crate::identity_first::IdentityRuntimeError;
    match err {
        IdentityRuntimeError::UnknownIdentity(_) => -32001,
        IdentityRuntimeError::NotAddressable(_) => -32002,
        // The send's expected registered row is no longer the identity's
        // registered row; nothing was submitted.
        IdentityRuntimeError::RegisteredSpecChanged { .. } => -32006,
        IdentityRuntimeError::IdentityUnresumable { .. } => IDENTITY_UNRESUMABLE_CODE,
        IdentityRuntimeError::IdentityHistoryUnavailable { .. }
        | IdentityRuntimeError::MemberHistoryUnavailable { .. }
        | IdentityRuntimeError::IdentityWakeRetryable { .. }
        | IdentityRuntimeError::IdentitySessionAbsent { .. } => IDENTITY_HISTORY_UNAVAILABLE_CODE,
        IdentityRuntimeError::IdentityHostAdopted { .. } => IDENTITY_HOST_ADOPTED_CODE,
        // The first-boot manifest: the migrated state directory and
        // Core's stores disagree, the storage-resolution class.
        IdentityRuntimeError::FirstBootManifestUnreadable { .. }
        | IdentityRuntimeError::FirstBootRowRefused { .. }
        | IdentityRuntimeError::FirstBootRowPending { .. } => STORAGE_RESOLUTION_CODE,
        // Core's own class for its refusal: invalid params for an invalid key
        // or a conflict, a capability gap, an unreadable status.
        IdentityRuntimeError::KeyedSendRefused { error, .. } => {
            crate::identity_first::runtime::mob_error_wire(error).map_or(-32603, |(code, _)| code)
        }
        IdentityRuntimeError::SessionNotHeldByIdentity { .. } => -32602,
        _ => -32603,
    }
}

fn identity_error_response(
    response_id: Value,
    err: &crate::identity_first::IdentityRuntimeError,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(identity_json_rpc_error(err)),
    }
}

/// The JSON-RPC error this gateway answers an identity runtime refusal with.
pub(crate) fn identity_json_rpc_error(
    err: &crate::identity_first::IdentityRuntimeError,
) -> JsonRpcError {
    use crate::identity_first::IdentityRuntimeError;
    let message = match err {
        IdentityRuntimeError::UnknownIdentity(id) => format!("unknown identity: {id}"),
        IdentityRuntimeError::NotAddressable(na) => format!("not addressable: {}", na.identity),
        _ => format!("{err}"),
    };
    JsonRpcError {
        code: identity_error_code(err),
        message,
        data: err.structured_data(),
    }
}

fn error_response(response_id: Value, code: i64, message: impl Into<String>) -> String {
    let message = message.into();
    let ambiguous_alias_rest = message
        .strip_prefix("ambiguous live identity alias ")
        .or_else(|| message.strip_prefix("invalid identity: ambiguous live identity alias "));
    let stale_live_alias_rest = message
        .strip_prefix("stale live identity alias: live console alias ")
        .or_else(|| {
            message.strip_prefix("invalid identity: stale live identity alias: live console alias ")
        });
    let hidden_policy_identity = message
        .strip_prefix("identity hidden by policy: ")
        .or_else(|| message.strip_prefix("invalid identity: identity hidden by policy: "));
    let data = if let Some(rest) = ambiguous_alias_rest {
        let (identity, candidates) = rest
            .split_once(": candidates [")
            .map(|(identity, candidates)| {
                (
                    identity.to_string(),
                    candidates
                        .trim_end_matches(']')
                        .split(',')
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_else(|| (rest.to_string(), Vec::new()));
        Some(serde_json::json!({
            "kind": "ambiguous_live_identity_alias",
            "identity": identity,
            "candidates": candidates,
        }))
    } else if let Some(rest) = stale_live_alias_rest {
        let (identity, rest) = rest.split_once(" resolves to ").unwrap_or((rest, ""));
        let (runtime_member_id, bound_identity) = rest
            .split_once(", but identity runtime binding belongs to ")
            .unwrap_or((rest, ""));
        Some(serde_json::json!({
            "kind": "stale_live_identity_alias",
            "identity": identity,
            "live_runtime_member_id": runtime_member_id,
            "bound_identity": bound_identity,
        }))
    } else {
        hidden_policy_identity.map(|identity| {
            serde_json::json!({
                "kind": "identity_hidden_by_policy",
                "identity": identity,
            })
        })
    };
    serialize_response(&JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message,
            data,
        }),
    })
}

/// Resolve a `meta/live/*` member target to the member's bridge session.
/// Accepts `{session_id}` verbatim, `{identity}` / `{member_id}` as a plain
/// member name, runtime alias, or durable identity (roster `agent_identity`
/// label fallback — the same resolution as `/agents/{id}/events` and
/// `cross_mob/peer_info`).
fn live_member_alias(params: &Value) -> Option<String> {
    if params.get("session_id").and_then(Value::as_str).is_some() {
        return None;
    }
    params
        .get("identity")
        .and_then(Value::as_str)
        .or_else(|| params.get("member_id").and_then(Value::as_str))
        .map(|raw| crate::member_comms_id::runtime_alias_str(raw).into_owned())
}

/// Validate and negotiate the execution-identity envelope before target
/// resolution or any live-channel side effect.
fn live_open_execution_identity_preflight(
    method: &str,
    params: &Value,
    response_id: Value,
    capability_available: bool,
) -> Option<JsonRpcResponse> {
    if method != "meta/live/open" {
        return None;
    }
    match crate::live_contracts::parse_live_open_execution_identity(params) {
        Ok(None) => None,
        Ok(Some(_))
            if let Err(error) =
                crate::live_contracts::validate_experimental_live_open_surface(params) =>
        {
            Some(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {error}"),
                    data: None,
                }),
            })
        }
        Ok(Some(_)) if capability_available => None,
        Ok(Some(_)) => Some(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: CAPABILITY_UNAVAILABLE_CODE,
                message: format!(
                    "capability {} is not available",
                    crate::live_contracts::LIVE_EXECUTION_IDENTITY_V1
                ),
                data: Some(serde_json::json!({
                    "capability": crate::live_contracts::LIVE_EXECUTION_IDENTITY_V1,
                })),
            }),
        }),
        Err(error) => Some(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: format!("Invalid params: {error}"),
                data: None,
            }),
        }),
    }
}

fn experimental_live_target_preflight(
    method: &str,
    params: &Value,
    response_id: Value,
) -> Option<JsonRpcResponse> {
    let strict = method == "meta/live/playback_owner/register"
        || method == "meta/live/playback_owner/revoke"
        || params.get("pending_receipt").is_some()
        || params.get("activation_receipt").is_some()
        || params.get("readiness_receipt").is_some();
    if !strict {
        return None;
    }
    crate::live_contracts::validate_experimental_live_target_surface(params)
        .err()
        .map(|error| JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: format!("Invalid params: {error}"),
                data: None,
            }),
        })
}

pub(crate) async fn resolve_live_target(
    handle: &meerkat_mob::MobHandle,
    identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    identity_authoritative: bool,
    params: &Value,
) -> Result<Option<meerkat_core::types::SessionId>, String> {
    if let Some(raw) = params.get("session_id").and_then(Value::as_str) {
        return Ok(meerkat_core::types::SessionId::parse(raw).ok());
    }
    let Some(raw) = live_member_alias(params) else {
        return Ok(None);
    };
    Ok(
        resolve_live_member_alias(handle, identity_runtime, identity_authoritative, raw)
            .await?
            .session,
    )
}

/// The roster member a live identity resolves to, and the bridge session
/// its machine binding names (`None` for an unmaterialized bridge).
#[cfg_attr(not(feature = "openai-live"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveMemberTarget {
    pub member: meerkat_mob::AgentIdentity,
    pub session: Option<meerkat_core::types::SessionId>,
}

/// [`resolve_live_target`] for a member identity, keeping the member it
/// resolved. The session is read from that member's own machine binding,
/// so the member is the session's owner without a roster scan.
#[cfg(feature = "openai-live")]
pub(crate) async fn resolve_live_member_target(
    handle: &meerkat_mob::MobHandle,
    identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    identity_authoritative: bool,
    identity: &str,
) -> Result<LiveMemberTarget, String> {
    let raw = crate::member_comms_id::runtime_alias_str(identity).into_owned();
    resolve_live_member_alias(handle, identity_runtime, identity_authoritative, raw).await
}

async fn resolve_live_member_alias(
    handle: &meerkat_mob::MobHandle,
    identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    identity_authoritative: bool,
    raw: String,
) -> Result<LiveMemberTarget, String> {
    let mut registered_session: Option<meerkat_core::types::SessionId> = None;
    let current_alias = if identity_authoritative {
        let identity_runtime = identity_runtime
            .ok_or_else(|| "durable live target lost its IdentityRuntime authority".to_string())?;
        let identity =
            crate::identity_first::IdentityRuntime::identity_for_generated_member_alias(&raw)
                .or_else(|| crate::identity_first::AgentIdentity::parse(&raw).ok())
                .ok_or_else(|| format!("invalid durable live target {raw:?}"))?;
        let status = identity_runtime
            .status(&identity)
            .await
            .map_err(|error| error.to_string())?;
        // A runtime binding must EXIST for this to be a live target, but it is
        // not the roster spelling. Since the stable-identity lowering the
        // roster is keyed by the encoded durable identity, so returning the
        // AgentRuntimeId here resolved a member id no roster row answers to.
        if status.agent_runtime_id.is_none() {
            return Err(format!("identity {identity} has no current runtime member"));
        }
        registered_session = status.session_id;
        identity.as_str().to_string()
    } else {
        let direct = crate::member_comms_id::mob_member_id(&raw);
        if handle
            .get_member(&direct)
            .await
            .map_err(|error| error.to_string())?
            .is_some()
        {
            raw.clone()
        } else {
            let candidates = handle
                .list_members_including_retiring()
                .await
                .into_iter()
                .filter(|entry| {
                    crate::member_comms_id::durable_identity_label(&entry.labels)
                        .is_some_and(|identity| identity == raw)
                })
                .map(|entry| {
                    crate::member_comms_id::runtime_alias_str(entry.agent_identity.as_str())
                        .into_owned()
                })
                .collect::<BTreeSet<_>>();
            match candidates.len() {
                0 => raw.clone(),
                1 => candidates
                    .into_iter()
                    .next()
                    .ok_or_else(|| "live member alias candidate disappeared".to_string())?,
                _ => {
                    return Err(format!(
                        "ambiguous live member alias {raw}: candidates [{}]",
                        candidates.into_iter().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
        }
    };
    let member_id = crate::member_comms_id::mob_member_id(&current_alias);
    let resolved = handle.resolve_bridge_session_id(&member_id).await;
    // When the identity runtime is authoritative, the session the machine
    // resolves for that roster member must be the session the binding is
    // registered for. Disagreement means we resolved a live target that
    // belongs to a different session, which is the shape that let a control
    // call act on the wrong member.
    //
    // A registered session with nothing resolved yet is NOT a disagreement:
    // that is an unmaterialized bridge, and the caller already handles `None`.
    if let (Some(registered), Some(resolved_id)) = (registered_session.as_ref(), resolved.as_ref())
        && registered != resolved_id
    {
        return Err(format!(
            "identity live target resolved session {resolved_id} but its runtime binding is \
             registered for {registered}"
        ));
    }
    Ok(LiveMemberTarget {
        member: member_id,
        session: resolved,
    })
}

fn maybe_error_response(
    is_notification: bool,
    response_id: Value,
    code: i64,
    message: impl Into<String>,
) -> String {
    if is_notification {
        String::new()
    } else {
        error_response(response_id, code, message)
    }
}

pub(crate) fn agent_memory_rpc_error(
    operation: &str,
    err: crate::identity_first::AgentMemoryError,
) -> JsonRpcError {
    let code = match &err {
        crate::identity_first::AgentMemoryError::InvalidConfig(_)
        | crate::identity_first::AgentMemoryError::InvalidRecord(_) => -32602,
        crate::identity_first::AgentMemoryError::Unsupported(_) => -32601,
        crate::identity_first::AgentMemoryError::Io(_)
        | crate::identity_first::AgentMemoryError::Parse(_)
        | crate::identity_first::AgentMemoryError::Timeout(_) => -32603,
    };
    JsonRpcError {
        code,
        message: format!("agent memory {operation} failed: {err}"),
        data: None,
    }
}

fn serialize_response(response: &JsonRpcResponse) -> String {
    serde_json::to_string(response).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Internal error"}}"#
            .to_string()
    })
}

/// Gateway RPC waits over a scripted ticket bridge; in-crate because they
/// use pub(crate) view constructors (D67).
#[cfg(test)]
#[allow(
    dead_code,
    reason = "shared fixtures moved with the turn-ticket RPC tests; not every test uses every helper"
)]
mod turn_ticket_rpc_tests;

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{
        CONSOLE_READ_BUDGET, CONSOLE_READ_TIMEOUT_CODE, IdentityMemberReadiness, error_response,
        experimental_live_target_preflight, handle_unified_rpc_json, identity_error_response,
        live_open_execution_identity_preflight, parse_console_read_budget, resolve_live_target,
        resolve_rpc_identity_control_target, rpc_live_identity_alias_visible,
        rpc_member_id_matches_durable_identity, rpc_runtime_alias_generation,
        wait_identity_startup_ready, with_read_deadline,
    };
    use crate::identity_first::contracts::RosterProvider;
    use crate::identity_first::core_test_bridge::{CoreReadsBridge, bind_live, register_live};
    use crate::identity_first::core_test_members::CoreTestMembers;
    use crate::identity_first::{
        AgentAddressability, AgentBuildDraft, AgentIdentity, AgentMemoryProvider,
        AgentMemoryRecallRequest, AgentMemorySelection, AgentRuntimeId, BridgeError,
        DurabilityPolicy, DurableAgentSpec, IdentityLifecycleState, IdentityRuntime,
        IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, ResumeSessionOutcome,
        RosterContext, RosterError, SessionBridge, SessionSnapshot,
    };
    use crate::memory::SqliteAgentMemoryStore;
    use crate::{
        DiscoverySpec, IdentityFirstContext, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
        UnifiedRuntime,
    };
    use async_trait::async_trait;
    use meerkat::{AgentFactory, Config, build_ephemeral_service};
    use meerkat_client::TestClient;
    use meerkat_mob::{MobDefinition, MobStorage, SpawnMemberSpec};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    /// A candidate launch refuses each roster change, each member input and
    /// each operator mutation typed, with the refusal a candidate answers
    /// for an announcement change; a launch that speaks for its composition,
    /// and every read, pass to the dispatch.
    #[test]
    fn a_candidate_launch_refuses_roster_changes_and_member_input_typed() {
        let no_params = json!({});
        for method in super::CANDIDATE_LAUNCH_REFUSED_METHODS {
            let refusal =
                super::candidate_launch_refusal(false, None, method, &no_params, json!(7));
            assert!(refusal.is_some(), "a candidate refuses {method}");
            let refusal = refusal.expect("a candidate refuses every listed method");
            assert_eq!(refusal.id, json!(7));
            assert!(refusal.result.is_none());
            let error = refusal.error.expect("a typed refusal");
            assert_eq!(
                error.code,
                crate::mob_announcements::rpc::ANNOUNCEMENTS_REFUSED_CODE
            );
            assert_eq!(error.data, Some(json!({ "kind": "candidate_launch" })));
            assert!(error.message.starts_with(method), "{}", error.message);
            assert!(
                super::candidate_launch_refusal(true, None, method, &no_params, json!(7)).is_none(),
                "an authoritative launch serves {method}"
            );
        }
        let refused_kind = |method: &str, params: &Value| {
            super::candidate_launch_refusal(false, None, method, params, json!(7))
                .and_then(|refusal| refusal.error)
                .and_then(|error| error.data)
        };
        let candidate_launch = Some(json!({ "kind": "candidate_launch" }));
        // The member declaration family is derived from its registry, never
        // listed here: its two mutations are refused, its read answers.
        let mut declaration_mutations = 0;
        for &method in super::mob_methods::MEMBER_DECLARATION_METHODS {
            let refused = refused_kind(method, &no_params);
            if super::mob_methods::is_member_declaration_mutating_method(method) {
                declaration_mutations += 1;
                assert_eq!(refused, candidate_launch, "a candidate refuses {method}");
            } else {
                assert_eq!(refused, None, "a candidate still answers {method}");
            }
            assert!(
                super::candidate_launch_refusal(true, None, method, &no_params, json!(7)).is_none(),
                "an authoritative launch serves {method}"
            );
        }
        assert_eq!(
            declaration_mutations, 2,
            "adoption and the tool declaration apply"
        );
        // Every WorkGraph write is refused, from the WorkGraph's own
        // registry; its reads answer.
        for &method in super::workgraph_methods::WORKGRAPH_MUTATE_METHODS
            .iter()
            .chain(super::workgraph_methods::WORKGRAPH_CONSOLE_MUTATE_METHODS)
        {
            assert_eq!(
                refused_kind(method, &no_params),
                candidate_launch,
                "a candidate refuses {method}"
            );
        }
        for &method in super::workgraph_methods::WORKGRAPH_READ_METHODS {
            assert_eq!(
                refused_kind(method, &no_params),
                None,
                "a candidate still answers {method}"
            );
        }
        // A method that runs something on the host, or writes outside it,
        // is refused in exactly that arm.
        for (method, refused, served) in [
            (
                "meta/mobpacks/deploy",
                json!({ "execute": true }),
                json!({ "execute": false }),
            ),
            (
                "meta/mobpacks/validate",
                json!({ "rkat_validate": true }),
                json!({}),
            ),
            (
                "meta/mobpacks/validate",
                json!({ "rkatValidate": true }),
                json!({ "rkat_validate": false }),
            ),
            (
                "meta/session_store/bigquery",
                json!({ "operation": "stream_insert_rows" }),
                json!({ "operation": "read_rows" }),
            ),
            (
                "meta/session_store/bigquery",
                json!({ "operation": " stream_insert_rows" }),
                json!({ "operation": " read_latest " }),
            ),
            (
                "meta/session_store/bigquery",
                json!({ "operation": "stream_insert\t" }),
                json!({ "operation": "stream_insert_rows_later" }),
            ),
            (
                "meta/gating/evaluate",
                json!({
                    "action": "deploy",
                    "actor_id": "ops",
                    "risk_tier": "r3",
                    "approval_recipient": " lead@example.test ",
                    "approval_channel": "email",
                }),
                json!({
                    "action": "deploy",
                    "actor_id": "ops",
                    "risk_tier": "R3",
                    "approval_recipient": "lead@example.test",
                    "approval_channel": "  ",
                }),
            ),
            (
                "meta/gating/evaluate",
                json!({
                    "action": "deploy",
                    "actor_id": "ops",
                    "risk_tier": "R3",
                    "approval_recipient": "lead@example.test",
                    "approval_channel": "email",
                }),
                json!({
                    "action": "deploy",
                    "actor_id": "ops",
                    "risk_tier": "R2",
                    "approval_recipient": "lead@example.test",
                    "approval_channel": "email",
                }),
            ),
        ] {
            assert_eq!(
                (
                    refused_kind(method, &refused),
                    refused_kind(method, &served)
                ),
                (candidate_launch.clone(), None),
                "{method}: refused with {refused}, served with {served}"
            );
        }
        for method in [
            "meta/status",
            "meta/status_identity",
            "meta/list_members",
            "meta/collect_completed",
            "meta/memory/query",
            "meta/agent_memory/recall",
            "meta/routing/routes/list",
            "meta/mob_labels/get",
            "meta/mobpacks/list",
            "meta/mobpacks/get",
            "jobs/list",
        ] {
            assert_eq!(
                refused_kind(method, &no_params),
                None,
                "a candidate still answers {method}"
            );
        }
    }

    /// A candidate's refusal of a session revocation sends the operator to
    /// the authoritative launch.
    #[test]
    fn a_candidate_launch_refusal_of_a_revocation_names_the_authoritative_launch() {
        let refusal = super::candidate_launch_refusal(
            false,
            None,
            super::CONSOLE_REVOKE_SESSIONS_METHOD,
            &json!({ "principal": "alice@example.test" }),
            json!(7),
        )
        .and_then(|refusal| refusal.error)
        .expect("a candidate refuses a revocation");
        assert_eq!(
            (
                refusal.code,
                refusal.data,
                refusal
                    .message
                    .contains("revoke the sessions on the authoritative launch"),
            ),
            (
                crate::mob_announcements::rpc::ANNOUNCEMENTS_REFUSED_CODE,
                Some(json!({ "kind": "candidate_launch" })),
                true,
            ),
            "{}",
            refusal.message
        );
    }

    /// A candidate launch writes no Flow Editor draft store the
    /// authoritative launch reads: a draft write to the default store, to a
    /// file in its state directory or to a path that climbs out of one is
    /// refused, and one to a file the caller names outside it answers, as
    /// every draft read does.
    #[test]
    fn a_candidate_launch_writes_only_a_caller_named_draft_store() {
        let state_dir = tempfile::tempdir().expect("state dir");
        let outside = tempfile::tempdir().expect("outside dir");
        let refuses = |method: &str, params: &Value| {
            super::candidate_launch_refusal(false, Some(state_dir.path()), method, params, json!(7))
                .is_some()
        };
        let in_state = json!({ "store_path": state_dir.path().join("drafts.json") });
        let climbing = json!({
            "store_path": outside.path().join("..").join("drafts.json")
        });
        let outside_store = json!({ "store_path": outside.path().join("drafts.json") });
        for method in [
            "meta/mobpacks/create",
            "meta/mobpacks/save",
            "meta/mobpacks/delete",
            "meta/mobpacks/undo",
            "meta/mobpacks/redo",
        ] {
            assert_eq!(
                (
                    refuses(method, &json!({})),
                    refuses(method, &in_state),
                    refuses(method, &climbing),
                    refuses(method, &outside_store),
                ),
                (true, true, true, false),
                "{method}"
            );
            assert!(
                super::candidate_launch_refusal(
                    true,
                    Some(state_dir.path()),
                    method,
                    &json!({}),
                    json!(7)
                )
                .is_none(),
                "an authoritative launch serves {method}"
            );
        }
        for method in ["meta/mobpacks/list", "meta/mobpacks/get"] {
            assert!(
                !refuses(method, &json!({})),
                "a candidate still answers {method}"
            );
        }
    }

    /// The store `META_FLOW_EDITOR_DRAFT_STORE` names is the authoritative
    /// launch's wherever it lies: a draft write whose `store_path` names it,
    /// however the path is spelled, reaches it and is refused, and so does a
    /// write that names no store; a write to another file beside it does
    /// not.
    #[test]
    fn a_candidate_launch_writes_no_draft_to_the_store_its_environment_names() {
        let state_dir = tempfile::tempdir().expect("state dir");
        let outside = tempfile::tempdir().expect("outside dir");
        let env_store = outside.path().join("drafts.json");
        let reaches = |params: &Value| {
            super::draft_store_write_reaches_a_read_store(
                params,
                Some(state_dir.path()),
                Some(env_store.as_path()),
            )
        };
        let named = |path: std::path::PathBuf| json!({ "store_path": path });
        assert_eq!(
            (
                reaches(&named(env_store.clone())),
                reaches(&named(outside.path().join(".").join("drafts.json"))),
                reaches(&json!({})),
                reaches(&named(outside.path().join("other-drafts.json"))),
            ),
            (true, true, true, false)
        );
        #[cfg(unix)]
        {
            let links = tempfile::tempdir().expect("link dir");
            let link = links.path().join("drafts-dir");
            std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");
            assert!(
                reaches(&named(link.join("drafts.json"))),
                "a spelling through a symlinked directory reaches the store"
            );
        }
    }

    #[test]
    fn execution_identity_preflight_is_fail_closed_before_live_dispatch() {
        let unavailable = live_open_execution_identity_preflight(
            "meta/live/open",
            &json!({
                "identity": "identity:luka",
                "execution_identity": {"version": "v1", "profile_id": "example.device.open-room.v1"}
            }),
            json!(1),
            false,
        )
        .expect("experimental envelope must stop before live dispatch");
        let error = unavailable.error.expect("preflight must return an error");
        assert_eq!(error.code, -32004);
        assert_eq!(
            error.data,
            Some(json!({"capability": "live.execution_identity.v1"}))
        );

        let conflict = live_open_execution_identity_preflight(
            "meta/live/open",
            &json!({
                "model": "legacy",
                "execution_identity": {"version": "v1", "profile_id": "example.device.open-room.v1"}
            }),
            json!(2),
            true,
        )
        .expect("conflict must stop before live dispatch");
        assert_eq!(conflict.error.expect("conflict error").code, -32602);

        assert!(
            live_open_execution_identity_preflight(
                "meta/live/open",
                &json!({"model": "legacy"}),
                json!(3),
                false,
            )
            .is_none(),
            "legacy live/open remains on the existing path"
        );

        assert!(
            live_open_execution_identity_preflight(
                "meta/live/open",
                &json!({
                    "identity": "identity:luka",
                    "execution_identity": {"version": "v1", "profile_id": "example.device.open-room.v1"}
                }),
                json!(4),
                true,
            )
            .is_none(),
            "an admitted envelope continues to live target resolution"
        );

        for params in [
            json!({
                "session_id": "session:raw",
                "execution_identity": {"version": "v1"}
            }),
            json!({
                "identity": "identity:luka",
                "member_id": "rt:luka:0",
                "execution_identity": {"version": "v1"}
            }),
            json!({
                "identity": "identity:luka",
                "execution_mode": "responses",
                "execution_identity": {"version": "v1"}
            }),
            json!({
                "identity": "identity:luka",
                "responses_model": "gpt-5.5",
                "execution_identity": {"version": "v1"}
            }),
        ] {
            let response =
                live_open_execution_identity_preflight("meta/live/open", &params, json!(5), true)
                    .expect("ineligible target or provider-native field must fail preflight");
            assert_eq!(response.error.expect("preflight error").code, -32602);
        }

        let unavailable_raw_target = live_open_execution_identity_preflight(
            "meta/live/open",
            &json!({
                "session_id": "session:raw",
                "execution_identity": {"version": "v1"}
            }),
            json!(6),
            false,
        )
        .expect("strict targeting must fail before capability negotiation");
        assert_eq!(
            unavailable_raw_target
                .error
                .expect("strict target error")
                .code,
            -32602
        );
    }

    #[test]
    fn strict_live_receipts_reject_raw_mixed_and_runtime_alias_targets() {
        for params in [
            json!({"session_id": "session:raw", "pending_receipt": "pending"}),
            json!({
                "identity": "identity:luka",
                "member_id": "member:raw",
                "activation_receipt": "active"
            }),
            json!({
                "identity": "rt:identity:luka:0",
                "pending_receipt": "pending"
            }),
        ] {
            let response =
                experimental_live_target_preflight("meta/live/status", &params, json!(7))
                    .expect("ineligible strict target must fail before resolution");
            assert_eq!(response.error.expect("target error").code, -32602);
        }

        assert!(
            experimental_live_target_preflight(
                "meta/live/status",
                &json!({
                    "identity": "identity:luka",
                    "pending_receipt": "pending"
                }),
                json!(8),
            )
            .is_none(),
            "a durable identity claim continues to authoritative lifecycle resolution"
        );
    }

    /// a 2026-08-16 production incident: `meta/identity/resolved_tools` hung past 60 seconds
    /// with no completion because the member's session task was mid-turn and
    /// the read had no deadline - the session task is a strict sequential
    /// command loop, so the read queued instead of degrading. A read that
    /// never answers must now become a TYPED timeout naming the arm and the
    /// seam it was waiting on.
    #[tokio::test(start_paused = true)]
    async fn a_blocked_read_arm_returns_the_typed_timeout_instead_of_hanging() {
        let response = with_read_deadline(
            "meta/identity/resolved_tools",
            "the identity status read and the session tool-scope snapshot",
            json!(7),
            std::future::pending::<super::JsonRpcResponse>(),
        )
        .await;

        assert_eq!(
            response.id,
            json!(7),
            "the timeout answers the same request"
        );
        assert!(
            response.result.is_none(),
            "a timed-out read must not fabricate a result"
        );
        let error = response
            .error
            .expect("a blocked read must surface an error");
        assert_eq!(error.code, CONSOLE_READ_TIMEOUT_CODE);
        assert!(
            error.message.contains("meta/identity/resolved_tools")
                && error.message.contains("session tool-scope snapshot"),
            "the message must name the arm AND what it was awaiting: {}",
            error.message
        );
        let data = error
            .data
            .expect("the typed timeout carries structured data");
        assert_eq!(data["kind"], json!("console_read_timeout"));
        assert_eq!(data["arm"], json!("meta/identity/resolved_tools"));
        assert_eq!(data["timeout_secs"], json!(CONSOLE_READ_BUDGET.as_secs()));
    }

    /// A read that answers within its budget is returned untouched - the
    /// deadline must not change any healthy read's response.
    #[tokio::test(start_paused = true)]
    async fn a_read_arm_that_answers_within_budget_is_passed_through() {
        let response = with_read_deadline(
            "meta/member_status",
            "the mob actor member-status round trip",
            json!(1),
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                super::JsonRpcResponse {
                    jsonrpc: super::JSONRPC_VERSION.to_string(),
                    id: json!(1),
                    result: Some(json!({"state": "idle"})),
                    error: None,
                }
            },
        )
        .await;

        assert!(response.error.is_none(), "a healthy read must not time out");
        assert_eq!(response.result, Some(json!({"state": "idle"})));
    }

    /// `META_CONSOLE_READ_TIMEOUT_SECS` is clamped like the bridge admission
    /// knob: `0` must not turn every console read into an instant failure, and
    /// an over-large value must not restore the unbounded wait.
    #[test]
    fn the_console_read_budget_knob_is_clamped_at_both_ends() {
        assert_eq!(parse_console_read_budget(None), CONSOLE_READ_BUDGET);
        assert_eq!(
            parse_console_read_budget(Some("not-a-number")),
            CONSOLE_READ_BUDGET,
            "an unparseable value falls back to the default"
        );
        assert_eq!(
            parse_console_read_budget(Some(" 45 ")),
            Duration::from_secs(45),
            "a plain value is honored (whitespace trimmed)"
        );
        assert_eq!(
            parse_console_read_budget(Some("0")),
            Duration::from_secs(1),
            "zero must not fail every read"
        );
        assert_eq!(
            parse_console_read_budget(Some("100000")),
            Duration::from_hours(1),
            "an over-large value must not restore the unbounded wait"
        );
    }

    #[derive(Debug, Default)]
    struct EmptyRosterProvider;

    #[async_trait]
    impl RosterProvider for EmptyRosterProvider {
        async fn roster(
            &self,
            _context: &RosterContext,
        ) -> Result<Vec<DurableAgentSpec>, RosterError> {
            Ok(Vec::new())
        }
    }

    #[derive(Debug, Default)]
    struct ContextRequiredEmptyRosterProvider {
        missing_definition_calls: std::sync::atomic::AtomicUsize,
    }

    impl ContextRequiredEmptyRosterProvider {
        fn missing_definition_calls(&self) -> usize {
            self.missing_definition_calls
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RosterProvider for ContextRequiredEmptyRosterProvider {
        async fn roster(
            &self,
            context: &RosterContext,
        ) -> Result<Vec<DurableAgentSpec>, RosterError> {
            if context.mob_definition.is_none() {
                self.missing_definition_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return Err(RosterError::ProviderUnavailable(
                    "mob definition required".to_string(),
                ));
            }
            Ok(Vec::new())
        }
    }

    #[derive(Debug)]
    struct ContextRequiredStaticRosterProvider {
        specs: Vec<DurableAgentSpec>,
        missing_definition_calls: std::sync::atomic::AtomicUsize,
    }

    impl ContextRequiredStaticRosterProvider {
        fn new(specs: Vec<DurableAgentSpec>) -> Self {
            Self {
                specs,
                missing_definition_calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn missing_definition_calls(&self) -> usize {
            self.missing_definition_calls
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RosterProvider for ContextRequiredStaticRosterProvider {
        async fn roster(
            &self,
            context: &RosterContext,
        ) -> Result<Vec<DurableAgentSpec>, RosterError> {
            if context.mob_definition.is_none() {
                self.missing_definition_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return Err(RosterError::ProviderUnavailable(
                    "mob definition required".to_string(),
                ));
            }
            Ok(self.specs.clone())
        }
    }

    #[derive(Default)]
    struct RpcResetTestBridge {
        create_calls: std::sync::atomic::AtomicUsize,
        last_create_spec: tokio::sync::Mutex<Option<DurableAgentSpec>>,
        /// The members Core holds: a create or reset commits its binding here.
        core: CoreTestMembers,
    }

    impl RpcResetTestBridge {
        async fn last_create_spec(&self) -> Option<DurableAgentSpec> {
            self.last_create_spec.lock().await.clone()
        }
    }

    #[async_trait]
    impl SessionBridge for RpcResetTestBridge {
        /// The single authoritative successor transition.
        ///
        /// Unlocked only after the production path was proven: destructive reset
        /// lowers to one respawn carrying the successor spec, and
        /// `respawn_with_successor_spec` applies that spec atomically - verified
        /// end to end on a real mob handle, with no double in the path, by the
        /// identity_first_builder reprofile pair. Before that landed, this double
        /// had no implementation at all and the reset refused, which is what kept
        /// this test honestly red rather than green against nothing.
        async fn reset_member_to_successor(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
            self.create_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.last_create_spec.lock().await = Some(spec.clone());
            let generation = self.create_calls.load(std::sync::atomic::Ordering::SeqCst) as u64;
            let alias = format!("rt:{}:{generation}", identity.as_str());
            let agent_runtime_id =
                crate::identity_first::AgentRuntimeId::parse(&alias).map_err(|error| {
                    BridgeError::Mob(format!("test double minted an unusable successor: {error}"))
                })?;
            let binding = crate::identity_first::MemberRuntimeBinding {
                agent_runtime_id,
                session_id: Some(meerkat_core::types::SessionId::new()),
            };
            self.core.bind(identity, &binding, &spec.profile);
            Ok(binding)
        }

        async fn create_session(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            let session_id = &meerkat_core::types::SessionId::new();
            self.create_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.last_create_spec.lock().await = Some(spec.clone());
            let binding = crate::identity_first::MemberRuntimeBinding {
                agent_runtime_id,
                session_id: Some(session_id.clone()),
            };
            self.core.bind(identity, &binding, &spec.profile);
            Ok(binding)
        }

        async fn resume_session(
            &self,
            identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            session_id: &meerkat_core::types::SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            Ok(ResumeSessionOutcome::Resumed {
                session_id: session_id.clone(),
                agent_runtime_id,
            })
        }

        async fn live_member(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn live_member_scope(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            self.core.live_member_scope(identity)
        }

        async fn ensure_member_live(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            self.core.history()
        }

        async fn last_member_binding(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            self.core.last_member_binding(identity)
        }

        async fn identity_management(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
        }

        async fn deliver_admitted(
            &self,
            _runtime_id: &AgentRuntimeId,
            _delivery: crate::identity_first::BridgeDelivery,
        ) -> Result<meerkat_core::types::SessionId, BridgeError> {
            Ok(meerkat_core::types::SessionId::new())
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<SessionSnapshot, BridgeError> {
            Ok(SessionSnapshot { data: Vec::new() })
        }

        async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            self.core.retire(runtime_id);
            Ok(())
        }
    }

    struct ReadOnlyAgentMemoryProvider;

    #[async_trait]
    impl crate::identity_first::AgentMemoryProvider for ReadOnlyAgentMemoryProvider {
        async fn recall(
            &self,
            _request: crate::identity_first::AgentMemoryRecallRequest,
        ) -> Result<
            Vec<crate::identity_first::AgentMemoryRecord>,
            crate::identity_first::AgentMemoryError,
        > {
            Ok(Vec::new())
        }
    }

    /// Per-test mob id: supervisor routes live in the process-global in-proc
    /// registry under `{mob_id}/__mob_supervisor__`, and meerkat 0.8.23
    /// refuses displacement, so concurrently running tests must not share an
    /// id.
    fn unique_rpc_test_mob_id() -> String {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!(
            "rpc-identity-alias-test-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }

    fn rpc_test_mob_spec(
        temp_dir: &tempfile::TempDir,
    ) -> Result<MobBootstrapSpec, Box<dyn std::error::Error + Send + Sync>> {
        rpc_test_mob_spec_for(temp_dir, &unique_rpc_test_mob_id())
    }

    /// [`rpc_test_mob_spec`] of the mob `mob_id`.
    fn rpc_test_mob_spec_for(
        temp_dir: &tempfile::TempDir,
        mob_id: &str,
    ) -> Result<MobBootstrapSpec, Box<dyn std::error::Error + Send + Sync>> {
        let session_path = temp_dir.path().join("sessions");
        std::fs::create_dir_all(&session_path)?;
        let factory = AgentFactory::new(&session_path).comms(true);
        let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
        ))?;
        Ok(
            MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
                .with_options(MobBootstrapOptions {
                    allow_ephemeral_sessions: true,
                    notify_orchestrator_on_resume: true,
                    default_llm_client: Some(Arc::new(TestClient::default())),
                }),
        )
    }

    /// [`rpc_test_mob_spec`] over a runtime with a store: the persistent
    /// session service and a persistent runtime authority over SQLite stores
    /// in `temp_dir`. Identity adoption needs one: Core refuses it on a
    /// runtime without a store
    /// (`mob_identity_adoption_requires_persistent_runtime`).
    fn rpc_test_store_backed_mob_spec(
        temp_dir: &tempfile::TempDir,
    ) -> Result<MobBootstrapSpec, Box<dyn std::error::Error + Send + Sync>> {
        let mob_id = unique_rpc_test_mob_id();
        let state = temp_dir.path().join("state");
        std::fs::create_dir_all(&state)?;
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
            meerkat_store::SqliteSessionStore::open(state.join("sessions.db"))?,
        );
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))?,
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let factory = AgentFactory::new(&state).comms(true);
        let mut builder = meerkat::FactoryAgentBuilder::new(factory, Config::default());
        builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        builder.default_blob_store = Some(blob_store.clone());
        let machine = Arc::new(meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )?);
        let session_service = Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            16,
            session_store,
            runtime_store,
            blob_store,
        ));
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
        ))?;
        Ok(
            MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
                .with_session_runtime_adapter(machine)?
                .with_options(MobBootstrapOptions {
                    allow_ephemeral_sessions: true,
                    notify_orchestrator_on_resume: true,
                    default_llm_client: Some(Arc::new(TestClient::default())),
                }),
        )
    }

    /// Test-only identity bridge seam. Public raw creation deliberately
    /// rejects generated aliases and runtime-owned `agent_identity` labels;
    /// fixtures exercising already-projected identity members must model the
    /// trusted lower plane that the identity runtime itself uses.
    async fn spawn_identity_projection_fixture(
        runtime: &UnifiedRuntime,
        mut spec: SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        let runtime_alias = spec.identity.to_string();
        spec.identity = crate::member_comms_id::mob_member_id(&runtime_alias);
        Box::pin(runtime.mob_handle().spawn_spec(spec)).await?;
        Ok(())
    }

    fn rpc_reprofile_mob_spec(
        temp_dir: &tempfile::TempDir,
    ) -> Result<MobBootstrapSpec, Box<dyn std::error::Error + Send + Sync>> {
        let session_path = temp_dir.path().join("sessions");
        std::fs::create_dir_all(&session_path)?;
        let factory = AgentFactory::new(&session_path).comms(true);
        let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "rpc-reset-reprofile-test-{}"

[profiles.domain]
model = "gpt-5.5"
external_addressable = true

[profiles.domain.tools]
comms = true

[profiles.security]
model = "gpt-5.5"
external_addressable = true

[profiles.security.tools]
comms = true
shell = true
"#,
            unique_rpc_test_mob_id()
        ))?;
        Ok(
            MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
                .with_options(MobBootstrapOptions {
                    allow_ephemeral_sessions: true,
                    notify_orchestrator_on_resume: true,
                    default_llm_client: Some(Arc::new(TestClient::default())),
                }),
        )
    }

    fn rpc_durable_spec(identity: &str, profile: &str) -> DurableAgentSpec {
        DurableAgentSpec {
            identity: AgentIdentity::parse(identity).expect("valid identity"),
            profile: meerkat_mob::ProfileName::from(profile),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }
    }

    /// A control handler composed without session-service access resolves
    /// no dialable address for a member whose runtime is local: it reports
    /// none rather than inventing one, while the member's peer id and
    /// transport key are still published.
    #[tokio::test]
    async fn lookup_member_without_session_service_reports_no_address_for_a_local_member()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-lookup-no-session-service".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        Box::pin(runtime.spawn(meerkat_mob::SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "lookup-local".to_string(),
            None,
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            None,
        )))
        .await
        .map_err(|error| error.to_string())?;
        let response = crate::runtime::cross_mob_control::handle_lookup_member_raw(
            &runtime.mob_handle(),
            None,
            "lookup-local",
        )
        .await
        .map_err(|(code, message)| format!("{code}: {message}"))?;
        match response {
            crate::runtime::cross_mob_control::ControlResponse::Member {
                advertised_address,
                pubkey_b64,
                ..
            } => {
                assert_eq!(advertised_address, None);
                assert!(pubkey_b64.is_some(), "the member's key is still published");
            }
            other => {
                return Err(format!("expected a Member response, got {other:?}").into());
            }
        }
        runtime.shutdown().await;
        Ok(())
    }

    /// `register_peer_runtime` carries the peer mob's session service, so a
    /// same-process cross-mob wire resolves the peer member's dialable
    /// address from that mob (a legacy `register_peer_mob` handle carries
    /// none).
    #[tokio::test]
    async fn registered_peer_runtime_carries_its_session_service()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-peer-runtime-session-service".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let mob_id = runtime.mob_id();
        runtime
            .register_peer_mob(&mob_id, runtime.mob_handle())
            .await;
        assert!(!runtime.peer_mob_carries_session_service(&mob_id).await);
        runtime.register_peer_runtime(&runtime).await;
        assert!(runtime.peer_mob_carries_session_service(&mob_id).await);
        runtime.shutdown().await;
        Ok(())
    }

    /// On a runtime whose agents keep their provider credentials in a store
    /// the host named, a deploy that runs `rkat` is refused typed, and
    /// nothing else is: a deploy plan, a validation and other methods pass,
    /// as does every request on a runtime without such a store.
    #[test]
    fn a_named_credential_store_refuses_only_an_executing_deploy() {
        let refusal = super::explicit_provider_credentials_refusal(
            true,
            "meta/mobpacks/deploy",
            &json!({ "execute": true }),
            json!(7),
        )
        .expect("an executing deploy is refused");
        assert_eq!(refusal.id, json!(7));
        let error = refusal.error.expect("a typed error");
        assert_eq!(
            (error.code, error.data),
            (
                super::CAPABILITY_UNAVAILABLE_CODE,
                Some(json!({ "kind": "provider_credentials_not_carried" }))
            )
        );
        for (method, params) in [
            ("meta/mobpacks/deploy", json!({ "execute": false })),
            ("meta/mobpacks/deploy", json!({})),
            ("meta/mobpacks/validate", json!({ "rkat_validate": true })),
            ("meta/mobpacks/deploy_command", json!({ "execute": true })),
        ] {
            assert!(
                super::explicit_provider_credentials_refusal(true, method, &params, json!(7))
                    .is_none(),
                "{method} {params}"
            );
        }
        assert!(
            super::explicit_provider_credentials_refusal(
                false,
                "meta/mobpacks/deploy",
                &json!({ "execute": true }),
                json!(7),
            )
            .is_none()
        );
    }

    /// A runtime recorded as holding a host-chosen provider credential store
    /// refuses an executing deploy on its own RPC before anything runs; it
    /// still answers the deploy plan.
    #[tokio::test]
    async fn a_runtime_with_a_named_credential_store_refuses_an_executing_deploy()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let mut runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-named-credential-store-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        runtime.set_explicit_provider_credentials(true);
        let deploy = |execute: bool| {
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "meta/mobpacks/deploy",
                "params": { "execute": execute },
            })
            .to_string()
        };

        let refused: Value = serde_json::from_str(
            &handle_unified_rpc_json(&runtime, &deploy(true), Duration::from_secs(1), None, None)
                .await,
        )?;
        assert_eq!(
            refused["error"]["code"],
            json!(super::CAPABILITY_UNAVAILABLE_CODE),
            "{refused:#?}"
        );
        assert_eq!(
            refused["error"]["data"]["kind"],
            json!("provider_credentials_not_carried"),
            "{refused:#?}"
        );
        let planned: Value = serde_json::from_str(
            &handle_unified_rpc_json(&runtime, &deploy(false), Duration::from_secs(1), None, None)
                .await,
        )?;
        assert_ne!(
            planned["error"]["data"]["kind"],
            json!("provider_credentials_not_carried"),
            "{planned:#?}"
        );
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// `_system` is the reserved runtime-plane console identity: the
    /// aggregator exempts it from the roster-visibility gate and identity
    /// namespacing (memory.* sink attribution), so a member spawned under
    /// that name would emit frames indistinguishable from runtime events
    /// and bypass the per-member hidden gate. The spawn surface must
    /// reject the name outright.
    #[tokio::test]
    async fn unified_rpc_spawn_rejects_reserved_system_identity()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-reserved-identity-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;

        let response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/spawn_member",
                    "params": {
                        "profile": "worker",
                        "meerkat_id": crate::console_contracts::SYSTEM_EVENT_IDENTITY,
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?;

        assert_eq!(response["error"]["code"], json!(-32602), "{response:#?}");
        assert!(
            response["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("reserved")),
            "rejection must name the reservation: {response:#?}"
        );
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    async fn reservation_test_runtime(
        temp_dir: &tempfile::TempDir,
        namespace: &str,
    ) -> Result<UnifiedRuntime, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: namespace.to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?)
    }

    /// The console treats frames under `_system` and `__console__` as the
    /// runtime's own events, so no member may take either. The RPC spawn
    /// refuses both, including spellings that become one once trimmed.
    #[tokio::test]
    async fn unified_rpc_spawn_refuses_every_runtime_plane_member_id()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = reservation_test_runtime(&temp_dir, "rpc-runtime-plane-spawn").await?;
        for (request_id, meerkat_id) in
            [(1, "__console__"), (2, " _system"), (3, "_system\u{2003}")]
        {
            let response: Value = serde_json::from_str(
                &handle_unified_rpc_json(
                    &runtime,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "method": "meta/spawn_member",
                        "params": { "profile": "worker", "meerkat_id": meerkat_id },
                    })
                    .to_string(),
                    Duration::from_secs(1),
                    None,
                    None,
                )
                .await,
            )?;
            assert_eq!(
                response["error"]["code"],
                json!(-32602),
                "{meerkat_id:?}: {response:#?}"
            );
            assert!(
                response["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("reserved")),
                "{meerkat_id:?}: the refusal names the reservation: {response:#?}"
            );
        }
        assert!(
            runtime.mob_handle().list_members().await.is_empty(),
            "no member took a runtime-plane identity"
        );
        let status: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({"jsonrpc": "2.0", "id": 9, "method": "meta/status"}).to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?;
        assert!(status["result"].is_object(), "{status:#?}");
        assert!(
            status["result"].get("reserved_identity_members").is_none(),
            "meta/status omits the field when no member holds a reserved identity: {status:#?}"
        );
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// The stdio `meta/status` of `runtime`.
    async fn unified_rpc_status(
        runtime: &UnifiedRuntime,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Ok(serde_json::from_str(
            &handle_unified_rpc_json(
                runtime,
                &json!({"jsonrpc": "2.0", "id": 1, "method": "meta/status"}).to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?)
    }

    /// The stdio `meta/status` names why a mob's actor stopped, beside
    /// `mob_state`, which stays the last phase the actor published: a
    /// fail-stopped actor keeps its last live phase, so the phase alone reads
    /// a stopped mob as running. A running mob's status names no failure.
    #[tokio::test(flavor = "multi_thread")]
    async fn unified_rpc_status_names_a_stopped_mob_actors_failure()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let running_dir = tempfile::tempdir()?;
        let running = reservation_test_runtime(&running_dir, "rpc-status-running").await?;
        let status = unified_rpc_status(&running).await?;
        assert!(
            status["result"].get("mob_failure").is_none(),
            "a running mob's status names no failure: {status:#?}"
        );
        let _ = running.mob_handle().stop().await;

        let stopped_dir = tempfile::tempdir()?;
        let mob_id = unique_rpc_test_mob_id();
        meerkat_mob::fail_stop_next_actor_start_for_test(&meerkat_mob::MobId::from(
            mob_id.as_str(),
        ));
        let stopped = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec_for(&stopped_dir, &mob_id)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-status-fail-stopped".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        tokio::time::timeout(
            Duration::from_secs(30),
            stopped.mob_handle().actor_task_watch().wait(),
        )
        .await?;
        let status = unified_rpc_status(&stopped).await?;
        assert!(
            status["result"]["mob_failure"]
                .as_str()
                .is_some_and(|failure| !failure.is_empty()),
            "the stopped actor's failure is named: {status:#?}"
        );
        assert!(
            matches!(
                status["result"]["mob_state"].as_str(),
                Some("Some(Creating)" | "Some(Running)")
            ),
            "mob_state stays the last phase: {status:#?}"
        );
        Ok(())
    }

    /// `meta/ensure_member` with a `resume_session_id` reaches Core as a
    /// resume, which the spawn customizer guard admits, so the method's own
    /// check is what refuses a runtime-plane identity there.
    #[tokio::test]
    async fn unified_rpc_ensure_with_a_resume_session_refuses_runtime_plane_identities()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = reservation_test_runtime(&temp_dir, "rpc-runtime-plane-resume").await?;
        let ensure = |request_id: u64, params: Value| {
            let request = json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "method": "meta/ensure_member",
                "params": params,
            })
            .to_string();
            let runtime = &runtime;
            async move {
                handle_unified_rpc_json(runtime, &request, Duration::from_secs(5), None, None).await
            }
        };
        let seated: Value = serde_json::from_str(
            &ensure(1, json!({ "role": "worker", "agent_identity": "alice" })).await,
        )?;
        assert!(seated["error"].is_null(), "alice is seated: {seated:#?}");
        let session = runtime
            .mob_handle()
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("alice"))
            .await
            .ok_or("alice has a session")?;
        for (request_id, agent_identity) in [(2, "__console__"), (3, "_system")] {
            let ensured: Value = serde_json::from_str(
                &ensure(
                    request_id,
                    json!({
                        "role": "worker",
                        "agent_identity": agent_identity,
                        "resume_session_id": session.to_string(),
                    }),
                )
                .await,
            )?;
            assert_eq!(
                ensured["error"]["code"],
                json!(-32602),
                "{agent_identity}: {ensured:#?}"
            );
            assert!(
                ensured["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("reserved")),
                "{agent_identity}: the refusal names the reservation: {ensured:#?}"
            );
        }
        let members = runtime.mob_handle().list_members().await;
        assert!(
            members
                .iter()
                .all(|member| member.agent_identity.as_str() == "alice"),
            "no member took a runtime-plane identity: {members:?}"
        );
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// The Rust spawn API, its batch form and the declarative roster
    /// reconcile refuse the runtime-plane identities before any member exists.
    #[tokio::test]
    async fn unified_runtime_spawn_and_reconcile_refuse_runtime_plane_member_ids()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = reservation_test_runtime(&temp_dir, "rust-runtime-plane-spawn").await?;
        let spawned = Box::pin(runtime.spawn(SpawnMemberSpec::new("worker", "__console__"))).await;
        assert!(spawned.is_err(), "spawn refuses `__console__`");
        let batch = Box::pin(runtime.spawn_many(vec![
            SpawnMemberSpec::new("worker", "batch-ok"),
            SpawnMemberSpec::new("worker", "_system"),
        ]))
        .await;
        assert!(
            batch.is_err(),
            "spawn_many refuses a batch naming `_system`"
        );
        let reconciled =
            Box::pin(runtime.reconcile(vec![SpawnMemberSpec::new("worker", "__console__")])).await;
        assert!(
            reconciled.is_err(),
            "reconcile refuses a desired `__console__` member"
        );
        assert!(
            runtime.mob_handle().list_members().await.is_empty(),
            "no member took a runtime-plane identity"
        );
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test]
    async fn unified_rpc_spawn_member_rejects_generated_runtime_alias_namespace()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-generated-alias-reservation-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;

        let response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/spawn_member",
                    "params": {
                        "profile": "worker",
                        "meerkat_id": "rt:user:forged:0",
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?;

        assert_eq!(response["error"]["code"], json!(-32602), "{response:#?}");
        assert!(
            response["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("reserved")),
            "generated aliases must be rejected at admission: {response:#?}"
        );
        assert!(
            runtime.mob_handle().list_members().await.is_empty(),
            "reserved namespace rejection must happen before raw member spawn"
        );
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test]
    async fn unified_rpc_spawn_holds_explicit_identity_context_alias_reservation()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-explicit-identity-context-reservation-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        assert!(runtime.identity_runtime().is_none());

        let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-explicit-identity-context-reservation-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        }));
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                identity_runtime.clone(),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: Some(runtime.mob_handle().definition().clone()),
            transcript_edit_service: None,
            compaction_floors: None,
        };
        let held_lock = identity_runtime
            .raw_member_alias_lock("compat-worker")
            .await
            .lock_owned()
            .await;
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/spawn_member",
            "params": {
                "profile": "worker",
                "meerkat_id": " compat-worker ",
            },
        })
        .to_string();

        assert!(
            tokio::time::timeout(
                Duration::from_millis(25),
                handle_unified_rpc_json(
                    &runtime,
                    &request,
                    Duration::from_secs(1),
                    None,
                    Some(&identity_ctx),
                ),
            )
            .await
            .is_err(),
            "explicit identity authority must block the raw spawn on its canonical alias lock"
        );
        drop(held_lock);

        let response: Value = serde_json::from_str(
            &tokio::time::timeout(
                Duration::from_secs(2),
                handle_unified_rpc_json(
                    &runtime,
                    &request,
                    Duration::from_secs(1),
                    None,
                    Some(&identity_ctx),
                ),
            )
            .await?,
        )?;
        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(response["result"]["meerkat_id"], json!("compat-worker"));

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test]
    async fn unified_rpc_rejects_encoded_roster_ingress_and_authoritative_identity_labels()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-public-alias-ingress-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;

        let encoded = crate::member_comms_id::mob_member_id_str("rt:secret:0");
        let encoded_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/get_member",
                    "params": { "member_id": encoded.as_ref() },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?;
        assert_eq!(
            encoded_response["error"]["code"],
            json!(-32602),
            "encoded roster spelling must be rejected before resolution: {encoded_response:#?}"
        );

        let forged_label_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "meta/ensure_member",
                    "params": {
                        "role": "worker",
                        "agent_identity": "raw-worker",
                        "labels": { "agent_identity": "secret" },
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?;
        assert_eq!(
            forged_label_response["error"]["code"],
            json!(-32602),
            "raw creation must not mint runtime-authoritative identity labels: {forged_label_response:#?}"
        );
        assert!(runtime.mob_handle().list_members().await.is_empty());

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test]
    async fn unified_capabilities_separate_mobpack_authoring_from_runtime_controls()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-authoring-capabilities-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;

        let response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/capabilities",
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?;

        assert!(response["error"].is_null(), "{response:#?}");
        let methods = response["result"]["methods"]
            .as_array()
            .expect("methods array")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        for method in super::MOBPACK_AUTHORING_METHODS {
            assert!(
                methods.contains(method),
                "missing authoring method {method}"
            );
        }
        assert_eq!(
            response["result"]["authoring_capabilities"]["domain"],
            json!("mobpack_authoring")
        );
        assert_eq!(
            response["result"]["authoring_capabilities"]["runtime_mutation"],
            json!(false)
        );
        assert_eq!(
            response["result"]["authoring_capabilities"]["host_mutation_methods"]["meta/mobpacks/deploy"],
            json!("when execute=true, writes a mobpack archive and runs rkat mob run on the host")
        );
        assert_eq!(
            response["result"]["authoring_capabilities"]["methods"]
                .as_array()
                .expect("authoring methods")
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
            super::MOBPACK_AUTHORING_METHODS
        );
        assert_eq!(
            response["result"]["authoring_capabilities"]["deploy_command"],
            json!("rkat mob run")
        );

        Ok(())
    }

    /// A library-mode runtime built without `schedule_store(...)`: no schedule
    /// store and no schedule host exist behind it.
    async fn schedule_tools_test_runtime(
        temp_dir: &tempfile::TempDir,
        namespace: &str,
    ) -> Result<UnifiedRuntime, Box<dyn std::error::Error + Send + Sync>> {
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: namespace.to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        Ok(runtime)
    }

    #[tokio::test]
    async fn unified_rpc_serves_the_schedule_tool_definitions_without_a_schedule_store()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime =
            schedule_tools_test_runtime(&temp_dir, "rpc-schedule-tools-serve-test").await?;

        let response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/schedule/tools",
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?;
        let _ = runtime.mob_handle().stop().await;

        assert!(response["error"].is_null(), "{response:#?}");
        // Meerkat's own `schedule/tools` result, unchanged.
        assert_eq!(
            response["result"],
            json!({ "tools": meerkat::schedule_tools_list() })
        );
        let tools = response["result"]["tools"].as_array().expect("tools array");
        let names = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(names, meerkat_schedule::schedule_tool_names());
        assert!(
            tools.iter().all(|tool| tool["inputSchema"].is_object()),
            "{response:#?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn unified_capabilities_advertise_schedule_tools()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime =
            schedule_tools_test_runtime(&temp_dir, "rpc-schedule-tools-capabilities-test").await?;

        let capabilities: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/capabilities",
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?;
        let _ = runtime.mob_handle().stop().await;

        let advertised = capabilities["result"]["methods"]
            .as_array()
            .expect("methods array")
            .iter()
            .filter(|method| method.as_str() == Some("meta/schedule/tools"))
            .count();
        assert_eq!(advertised, 1, "{capabilities:#?}");
        Ok(())
    }

    /// The module-only surface has no mob and no members to give schedule
    /// tools to, so it neither serves nor advertises their definitions.
    #[test]
    fn module_only_rpc_neither_serves_nor_advertises_schedule_tools()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let config = MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: "module-rpc-schedule-tools-test".to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        };
        let mut runtime = crate::start_meta_runtime(config, Vec::new(), Duration::from_secs(1))?;
        let served: Value = serde_json::from_str(&super::handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "meta/schedule/tools",
            })
            .to_string(),
            Duration::from_secs(1),
        ))?;
        let capabilities: Value = serde_json::from_str(&super::handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "meta/capabilities",
            })
            .to_string(),
            Duration::from_secs(1),
        ))?;
        let _ = runtime.shutdown();

        assert_eq!(served["error"]["code"], json!(-32601), "{served:#?}");
        assert!(
            !capabilities["result"]["methods"]
                .as_array()
                .expect("methods array")
                .iter()
                .any(|method| method.as_str() == Some("meta/schedule/tools")),
            "{capabilities:#?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn unified_rpc_dispatches_mobpack_authoring_methods()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-authoring-dispatch-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;

        let response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/mobpacks/schema",
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?;

        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(
            response["result"]["media_type"],
            json!("application/vnd.meerkat.mobpack")
        );
        assert_eq!(
            response["result"]["commands"]["deploy_rpc"],
            json!("meta/mobpacks/deploy")
        );
        assert_eq!(
            response["result"]["deploy_settings"]["runtime_backed"],
            json!(true)
        );
        assert_eq!(
            response["result"]["deploy_settings"]["authoring_provider"]["runtime_binding"],
            json!("bound")
        );
        assert_eq!(
            response["result"]["deploy_settings"]["provenance"]["source"],
            json!("UnifiedRuntime.authoring_provider.deploy_target")
        );
        assert!(response["result"]["sample_mobpacks"].is_null());
        assert!(response["result"]["agent_definitions"].is_null());

        let catalogs: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "meta/mobpacks/catalogs",
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                None,
            )
            .await,
        )?;
        assert!(catalogs["error"].is_null(), "{catalogs:#?}");
        assert_eq!(catalogs["result"]["runtime_backed"], json!(true));
        assert_eq!(
            catalogs["result"]["authoring_provider"]["id"],
            json!("unified_runtime")
        );
        assert_eq!(
            catalogs["result"]["authoring_provider"]["runtime_binding"],
            json!("bound")
        );
        assert_eq!(
            catalogs["result"]["sources"]["runtime"],
            json!("unified_runtime")
        );
        assert_eq!(
            catalogs["result"]["sources"]["runtime_binding"],
            json!("bound")
        );
        assert!(
            catalogs["result"]["runtime_unavailable_reason"].is_null(),
            "{catalogs:#?}"
        );
        assert_eq!(
            catalogs["result"]["catalog_snapshot"]["runtime_backed"],
            json!(true)
        );
        assert_eq!(
            catalogs["result"]["authoring_provider"]["deploy_target"]["command"],
            json!("rkat mob run")
        );
        assert!(
            catalogs["result"]["authoring_provider"]["runtime_methods"]
                .as_array()
                .is_some_and(|methods| methods.contains(&json!("meta/mobpacks/deploy"))),
            "{catalogs:#?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn reconcile_identity_passes_mob_definition_to_roster_provider()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-reconcile-roster-context-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let identity_rt = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-reconcile-roster-context-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        }));
        let roster_provider = Arc::new(ContextRequiredEmptyRosterProvider::default());
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                identity_rt,
                runtime.mob_runtime().clone(),
            ),
            roster_provider: roster_provider.clone(),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: Some(runtime.mob_handle().definition().clone()),
            transcript_edit_service: None,
            compaction_floors: None,
        };

        let response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/reconcile_identity",
                    "params": {},
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;

        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(
            roster_provider.missing_definition_calls(),
            0,
            "reconcile_identity must preserve mob_definition in roster context"
        );
        Ok(())
    }

    /// An identity whose held member a held pass did not publish owes its
    /// host a held completion. `meta/reconcile_identity` reports it refused
    /// with typed data on the internal-error code, so a host reads the
    /// pending admission without parsing the message.
    #[tokio::test]
    async fn reconcile_identity_reports_an_owed_held_completion_with_typed_data()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-reconcile-owed-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let identity_rt = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-reconcile-owed-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(CoreReadsBridge::default())),
            default_timeout: None,
        }));
        let owed = AgentIdentity::parse("owed-member")?;
        // Its held pass ended without publishing it: Core never spawned it,
        // and the identity owes its host a fresh held completion.
        identity_rt.record_held_completion(&owed, false);
        let roster_provider = Arc::new(ContextRequiredStaticRosterProvider::new(vec![
            DurableAgentSpec {
                identity: owed.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
        ]));
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                identity_rt,
                runtime.mob_runtime().clone(),
            ),
            roster_provider: roster_provider.clone(),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: Some(runtime.mob_handle().definition().clone()),
            transcript_edit_service: None,
            compaction_floors: None,
        };

        let response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/reconcile_identity",
                    "params": {},
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;

        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(roster_provider.missing_definition_calls(), 0);
        let outcome = &response["result"]["outcomes"]["owed-member"];
        assert_eq!(outcome["outcome"], json!("refused"), "{response:#?}");
        assert_eq!(outcome["error"]["code"], json!(-32603), "{response:#?}");
        assert_eq!(
            outcome["error"]["data"],
            json!({"kind": "held_completion_owed", "identity": "owed-member"}),
            "{response:#?}"
        );
        Ok(())
    }

    fn member_observation_contention() -> meerkat_mob::MobError {
        meerkat_mob::MobError::LifecycleOperationAdmissionPending {
            intent: "member_status_observation".to_string(),
            stage: "observation_lane_saturated",
        }
    }

    #[tokio::test(start_paused = true)]
    async fn startup_ready_observation_retries_only_until_actual_success_with_remaining_budget() {
        let mut budgets = Vec::new();
        let result = super::wait_identity_members_ready_with_observation_retry(
            Duration::from_millis(200),
            |remaining| {
                budgets.push(remaining);
                std::future::ready(if budgets.len() == 1 {
                    Err(member_observation_contention())
                } else {
                    Ok(())
                })
            },
        )
        .await;
        assert_eq!(result, IdentityMemberReadiness::Ready);
        assert_eq!(budgets.len(), 2);
        assert_eq!(budgets[0], Duration::from_millis(200));
        assert!(budgets[1] < budgets[0]);
        assert!(!budgets[1].is_zero());
    }

    #[tokio::test(start_paused = true)]
    async fn startup_ready_observation_preserves_other_errors_and_owner_timeout() {
        let errors = [
            meerkat_mob::MobError::LifecycleOperationAdmissionPending {
                intent: "member_resume".to_string(),
                stage: "observation_lane_saturated",
            },
            meerkat_mob::MobError::Internal(
                "observation_lane_saturated: member_status_observation".to_string(),
            ),
            meerkat_mob::MobError::ActorCommandTimedOut {
                command_kind: "DifferentOwnerRead",
                stage: "original_stage",
            },
            meerkat_mob::MobError::ReadyWaitTimedOut {
                pending_member_ids: vec![meerkat_mob::AgentIdentity::from("still-starting")],
            },
        ];
        for error in errors {
            let expected = if crate::unified_runtime::mob_ops::is_ready_wait_timeout(&error) {
                IdentityMemberReadiness::TimedOut
            } else {
                IdentityMemberReadiness::Failed(error.to_string())
            };
            let mut reply = Some(error);
            let result = super::wait_identity_members_ready_with_observation_retry(
                Duration::from_secs(1),
                |_| std::future::ready(Err::<(), _>(reply.take().expect("no extra read"))),
            )
            .await;
            assert_eq!(result, expected);
            assert!(reply.is_none());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn startup_ready_observation_deadline_never_becomes_ready() {
        let start = tokio::time::Instant::now();
        let mut budgets = Vec::new();
        let result = super::wait_identity_members_ready_with_observation_retry(
            Duration::from_millis(51),
            |remaining| {
                budgets.push(remaining);
                std::future::ready(Err::<(), _>(member_observation_contention()))
            },
        )
        .await;
        assert_eq!(result, IdentityMemberReadiness::TimedOut);
        assert_eq!(start.elapsed(), Duration::from_millis(51));
        assert!(budgets.len() > 1);
        assert!(budgets.windows(2).all(|pair| pair[1] < pair[0]));

        let start = tokio::time::Instant::now();
        let hanging = super::wait_identity_members_ready_with_observation_retry(
            Duration::from_millis(30),
            |_| std::future::pending::<Result<(), meerkat_mob::MobError>>(),
        )
        .await;
        assert_eq!(hanging, IdentityMemberReadiness::TimedOut);
        assert_eq!(start.elapsed(), Duration::from_millis(30));
    }

    #[tokio::test(start_paused = true)]
    async fn startup_ready_observation_zero_budget_keeps_only_immediate_owner_success() {
        let ready = super::wait_identity_members_ready_with_observation_retry(
            Duration::ZERO,
            |remaining| {
                assert!(remaining.is_zero());
                std::future::ready(Ok::<_, meerkat_mob::MobError>(()))
            },
        )
        .await;
        assert_eq!(ready, IdentityMemberReadiness::Ready);
        let mut calls = 0;
        let contended =
            super::wait_identity_members_ready_with_observation_retry(Duration::ZERO, |_| {
                calls += 1;
                std::future::ready(Err::<(), _>(member_observation_contention()))
            })
            .await;
        assert_eq!(contended, IdentityMemberReadiness::TimedOut);
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn startup_ready_observation_retry_is_cancelled_by_bootstrap_generation_change()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-startup-ready-observation-generation".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        }));
        let generations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stale_reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let wait = wait_identity_startup_ready(&runtime, Duration::from_secs(1), {
            let runtime = runtime.clone();
            let generations = generations.clone();
            let stale_reads = stale_reads.clone();
            move |_ids, remaining| {
                let generation = generations.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let runtime = runtime.clone();
                let stale_reads = stale_reads.clone();
                async move {
                    super::wait_identity_members_ready_with_observation_retry(remaining, |_| {
                        if generation == 0 {
                            let attempt =
                                stale_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            assert_eq!(attempt, 0, "cancelled generation must not retry");
                            runtime.test_supersede_identity_bootstrap_ready();
                            std::future::ready(Err(member_observation_contention()))
                        } else {
                            std::future::ready(Ok(()))
                        }
                    })
                    .await
                }
            }
        })
        .await?;
        assert!(wait.startup_ready);
        assert!(!wait.timed_out);
        assert_eq!(generations.load(std::sync::atomic::Ordering::SeqCst), 2);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(stale_reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn startup_ready_wait_retries_stale_member_success_and_error_after_generation_change()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for stale_result in [
            IdentityMemberReadiness::Ready,
            IdentityMemberReadiness::Failed("stale member failure".to_string()),
        ] {
            let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: "rpc-startup-ready-generation-test".to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            }));
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let callback_runtime = identity_runtime.clone();
            let callback_calls = calls.clone();
            let wait = wait_identity_startup_ready(
                &identity_runtime,
                Duration::from_secs(1),
                move |_member_ids, _remaining| {
                    let call = callback_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let callback_runtime = callback_runtime.clone();
                    let stale_result = stale_result.clone();
                    async move {
                        if call == 0 {
                            // Supersede the pass in the same poll that returns
                            // the old member result. Whichever `select!` branch
                            // wins, the generation check must discard it.
                            callback_runtime.test_supersede_identity_bootstrap_ready();
                            stale_result
                        } else {
                            IdentityMemberReadiness::Ready
                        }
                    }
                },
            )
            .await?;

            assert!(wait.startup_ready);
            assert!(!wait.timed_out);
            assert!(wait.status.ready);
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        }
        Ok(())
    }

    #[tokio::test]
    async fn startup_ready_wait_reports_timeout_and_terminal_broken_status()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let timeout_runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-startup-ready-timeout-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        let timed_out = wait_identity_startup_ready(
            &timeout_runtime,
            Duration::from_secs(1),
            |_member_ids, _remaining| async { IdentityMemberReadiness::TimedOut },
        )
        .await?;
        assert!(timed_out.timed_out);
        assert!(!timed_out.startup_ready);

        let broken_runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-startup-ready-broken-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        broken_runtime.test_fail_identity_bootstrap("injected terminal failure");
        let readiness_calls = std::sync::atomic::AtomicUsize::new(0);
        let broken = wait_identity_startup_ready(
            &broken_runtime,
            Duration::from_secs(1),
            |_member_ids, _remaining| {
                readiness_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { IdentityMemberReadiness::Ready }
            },
        )
        .await?;
        assert!(!broken.timed_out);
        assert!(!broken.startup_ready);
        assert!(!broken.status.ready);
        assert_eq!(broken.status.counts.broken, 1);
        assert!(
            broken
                .status
                .error
                .as_deref()
                .is_some_and(|error| error.contains("injected terminal failure"))
        );
        assert_eq!(
            readiness_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "terminal bootstrap failure must not enter member readiness"
        );
        Ok(())
    }

    #[tokio::test]
    async fn identity_bootstrap_status_and_wait_rpc_are_typed_and_advertised()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-identity-bootstrap-status-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
                    continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
                    lease_provider: Arc::new(LocalLeaseProvider::new()),
                    runtime_instance_id: "rpc-identity-bootstrap-status-test".to_string(),
                    has_runtime_store: true,
                    durability_policy: DurabilityPolicy::SyncWriteThrough,
                    bridge: None,
                    default_timeout: None,
                })),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(ContextRequiredEmptyRosterProvider::default()),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: Some(runtime.mob_handle().definition().clone()),
            transcript_edit_service: None,
            compaction_floors: None,
        };

        let runtime_ref = &runtime;
        let identity_ctx_ref = &identity_ctx;
        let call = move |method: &'static str, params: Value| {
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": method,
                "method": method,
                "params": params,
            })
            .to_string();
            async move {
                handle_unified_rpc_json(
                    runtime_ref,
                    &request,
                    Duration::from_secs(1),
                    None,
                    Some(identity_ctx_ref),
                )
                .await
            }
        };

        let status: Value = serde_json::from_str(
            &call("meta/status_identity_bootstrap", serde_json::json!({})).await,
        )?;
        assert_eq!(status["result"]["mode"]["mode"], json!("eager_materialize"));
        assert_eq!(status["result"]["complete"], json!(true));
        assert_eq!(status["result"]["ready"], json!(true));

        let waited: Value = serde_json::from_str(
            &call(
                "meta/wait_identity_bootstrap",
                serde_json::json!({"target": "materialized", "timeout_ms": 0}),
            )
            .await,
        )?;
        assert_eq!(waited["result"]["timed_out"], json!(false));
        assert_eq!(waited["result"]["target"], json!("materialized"));

        let default_waited: Value = serde_json::from_str(
            &call("meta/wait_identity_bootstrap", serde_json::json!({})).await,
        )?;
        assert_eq!(default_waited["result"]["target"], json!("materialized"));

        let startup_ready: Value = serde_json::from_str(
            &call(
                "meta/wait_identity_bootstrap",
                serde_json::json!({"target": "startup_ready", "timeout_ms": 0}),
            )
            .await,
        )?;
        assert_eq!(startup_ready["result"]["timed_out"], json!(false));
        assert_eq!(startup_ready["result"]["startup_ready"], json!(true));

        for (params, expected_message) in [
            (serde_json::json!(null), "params must be an object"),
            (serde_json::json!([]), "params must be an object"),
            (
                serde_json::json!({"target": null}),
                "target must be a string",
            ),
            (serde_json::json!({"target": 42}), "target must be a string"),
            (
                serde_json::json!({"target": "not_ready"}),
                "target must be 'materialized' or 'startup_ready'",
            ),
            (
                serde_json::json!({"timeout_ms": null}),
                "timeout_ms must be a non-negative integer",
            ),
            (
                serde_json::json!({"timeout_ms": -1}),
                "timeout_ms must be a non-negative integer",
            ),
            (
                serde_json::json!({"timeout_ms": 1.5}),
                "timeout_ms must be a non-negative integer",
            ),
            (
                serde_json::json!({"timeout_ms": true}),
                "timeout_ms must be a non-negative integer",
            ),
            (
                serde_json::json!({"unexpected": true}),
                "unsupported parameter: unexpected",
            ),
        ] {
            let response: Value =
                serde_json::from_str(&call("meta/wait_identity_bootstrap", params).await)?;
            assert_eq!(response["error"]["code"], json!(-32602));
            assert_eq!(response["error"]["message"], json!(expected_message));
        }

        let capabilities: Value =
            serde_json::from_str(&call("meta/capabilities", serde_json::json!({})).await)?;
        let methods = capabilities["result"]["methods"]
            .as_array()
            .expect("methods array");
        assert!(methods.contains(&json!("meta/status_identity_bootstrap")));
        assert!(methods.contains(&json!("meta/wait_identity_bootstrap")));
        Ok(())
    }

    #[tokio::test]
    async fn reset_identity_preserves_mob_definition_for_roster_reprofile()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_reprofile_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-reset-roster-context-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;

        let bridge = Arc::new(RpcResetTestBridge::default());
        let identity_rt = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-reset-roster-context-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        }));

        let identity = AgentIdentity::parse("domain:security")?;
        // Core holds the identity's member at generation 0 under its old role.
        register_live(
            &identity_rt,
            &bridge.core,
            rpc_durable_spec(identity.as_str(), "domain"),
            0,
            meerkat_core::types::SessionId::new(),
        )
        .await?;

        let roster_provider = Arc::new(ContextRequiredStaticRosterProvider::new(vec![
            rpc_durable_spec(identity.as_str(), "security"),
        ]));
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                identity_rt.clone(),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: roster_provider.clone(),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: Some(runtime.mob_handle().definition().clone()),
            transcript_edit_service: None,
            compaction_floors: None,
        };

        let response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/reset",
                    "params": { "identity": identity.as_str() },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;

        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(
            roster_provider.missing_definition_calls(),
            0,
            "meta/reset must preserve mob_definition when installing the reset roster provider"
        );
        // The reset's successor transition retired the previous live member:
        // the reset reports no cleanup warning. The field stays, always null.
        // The result carries no `checkpoint_version`: Meta keeps no
        // continuity record, and Core's binding has none.
        let mut fields = response["result"]
            .as_object()
            .expect("reset result")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "agent_runtime_id",
                "cleanup_warning",
                "generation",
                "identity",
                "session_id"
            ],
            "{response:#?}"
        );
        assert_eq!(
            response["result"]["cleanup_warning"],
            Value::Null,
            "{response:#?}"
        );
        // The identity_reset lifecycle frame carries the same null field.
        let lifecycle = runtime
            .console_events()
            .replay_all(None)
            .await
            .map_err(|err| format!("replay_all: {}", err.error))?;
        let frame = lifecycle
            .iter()
            .find(|event| event.event_type == "identity_reset")
            .ok_or("the reset records an identity_reset frame")?;
        assert_eq!(
            frame.data,
            json!({
                "generation": response["result"]["generation"],
                "cleanup_warning": null,
            }),
            "{lifecycle:#?}"
        );
        assert_eq!(
            bridge
                .create_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "reset should rebuild through the bridge"
        );
        let created = bridge
            .last_create_spec()
            .await
            .expect("reset should record created spec");
        assert_eq!(created.profile.as_str(), "security");
        assert_eq!(
            identity_rt
                .status(&identity)
                .await?
                .profile
                .expect("identity should keep profile")
                .as_str(),
            "security"
        );
        Ok(())
    }

    /// A provider that honestly supports only the required v1 read surface:
    /// every `supports_*` flag keeps its trait default of `false`. Rebuilt
    /// negative arm for the capability gates after the positive flip above -
    /// the gate intent (unadvertised ops stay off the wire) must be proven by
    /// a fixture that truly lacks support, not one that hides it.
    struct V1RecallOnlyMemoryProvider;

    #[async_trait]
    impl crate::identity_first::AgentMemoryProvider for V1RecallOnlyMemoryProvider {
        async fn recall(
            &self,
            _request: crate::identity_first::AgentMemoryRecallRequest,
        ) -> Result<
            Vec<crate::identity_first::AgentMemoryRecord>,
            crate::identity_first::AgentMemoryError,
        > {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn agent_memory_capability_gates_keep_unsupported_v2_ops_off_the_wire()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-agent-memory-gates-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-agent-memory-gates-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        let provider: Arc<dyn crate::identity_first::AgentMemoryProvider> =
            Arc::new(V1RecallOnlyMemoryProvider);
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::new(identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: Some(provider),
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };
        let capabilities: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/capabilities",
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        let advertised = capabilities["result"]["methods"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        // Positive control: the provider is visible at all.
        assert!(
            advertised.contains(&json!("meta/agent_memory/recall")),
            "{capabilities:#?}"
        );
        // The gate arm proper: nothing unsupported reaches the wire.
        for gated_method in [
            "meta/agent_memory/remember",
            "meta/agent_memory/forget",
            "meta/agent_memory/update",
            "meta/agent_memory/manifest",
        ] {
            assert!(
                !advertised.contains(&json!(gated_method)),
                "{gated_method} must stay off the wire for a provider without support: {capabilities:#?}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn unified_rpc_agent_memory_remember_writes_identity_scoped_record()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-agent-memory-remember-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-agent-memory-remember-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        let store = Arc::new(SqliteAgentMemoryStore::open(
            temp_dir.path().join("agent-memory"),
        )?);
        let provider: Arc<dyn crate::identity_first::AgentMemoryProvider> = store.clone();
        identity_rt
            .set_agent_memory(Some(
                crate::identity_first::AgentMemoryRuntimeInjector::new(
                    provider.clone(),
                    crate::identity_first::AgentMemoryConfig::default(),
                ),
            ))
            .await;
        let memory_identity = AgentIdentity::parse("identity:luka")?;
        identity_rt
            .register(DurableAgentSpec {
                identity: memory_identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: Default::default(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::new(identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: Some(provider),
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };

        let capabilities: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/capabilities",
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert!(
            capabilities["result"]["methods"]
                .as_array()
                .is_some_and(|methods| methods.contains(&json!("meta/agent_memory/remember"))),
            "{capabilities:#?}"
        );
        assert!(
            capabilities["result"]["methods"]
                .as_array()
                .is_some_and(|methods| methods.contains(&json!("meta/agent_memory/recall"))),
            "{capabilities:#?}"
        );
        assert!(
            capabilities["result"]["methods"]
                .as_array()
                .is_some_and(|methods| methods.contains(&json!("meta/agent_memory/forget"))),
            "{capabilities:#?}"
        );
        // Flipped at the meerkat 0.8.22 / mobkit 0.8.16 pair (release-lead
        // ruling 2026-08-13): SqliteAgentMemoryStore genuinely implements
        // supersede and manifest, so advertising them is truth surfacing.
        // The old binding hid that support behind a non-forwarding wrapper;
        // this assertion pinned the hidden state, not an intended contract.
        // The gates-keep-unsupported-ops-off-the-wire arm lives on in
        // `agent_memory_capability_gates_keep_unsupported_v2_ops_off_the_wire`
        // with a provider that honestly lacks the v2 surface.
        assert!(
            capabilities["result"]["methods"]
                .as_array()
                .is_some_and(|methods| methods.contains(&json!("meta/agent_memory/update"))),
            "{capabilities:#?}"
        );
        assert!(
            capabilities["result"]["methods"]
                .as_array()
                .is_some_and(|methods| methods.contains(&json!("meta/agent_memory/manifest"))),
            "{capabilities:#?}"
        );

        let response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "meta/agent_memory/remember",
                    "params": {
                        "identity": "identity:luka",
                        "realm": "team",
                        "title": "Weekly standup",
                        "body": "Standup is before calendar planning.",
                        "tags": ["team", "calendar"]
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;

        assert!(response["error"].is_null(), "{response:#?}");
        let memory_id = response["result"]["memory_id"]
            .as_str()
            .ok_or("memory_id should be present")?
            .to_string();
        assert_eq!(response["result"]["title"], json!("Weekly standup"));
        assert_eq!(
            response["result"]["body"],
            json!("Standup is before calendar planning.")
        );
        assert_eq!(response["result"]["tags"], json!(["calendar", "team"]));

        let records = store
            .recall(AgentMemoryRecallRequest {
                identity: memory_identity.clone(),
                realm: "team".to_string(),
                query_text: None,
                query_terms: Vec::new(),
                selection: AgentMemorySelection::Always,
                max_entries: 64,
            })
            .await?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].title, "Weekly standup");
        assert_eq!(records[0].tags, vec!["calendar", "team"]);

        let recall_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "meta/agent_memory/recall",
                    "params": {
                        "identity": "identity:luka",
                        "realm": "team",
                        "selection": "contextual",
                        "query_terms": ["standup"],
                        "max_entries": 4
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;

        assert!(recall_response["error"].is_null(), "{recall_response:#?}");
        assert_eq!(
            recall_response["result"]["records"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            recall_response["result"]["records"][0]["body"],
            json!("Standup is before calendar planning.")
        );

        let forget_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "meta/agent_memory/forget",
                    "params": {
                        "identity": "identity:luka",
                        "realm": "team",
                        "memory_id": memory_id.clone()
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert!(forget_response["error"].is_null(), "{forget_response:#?}");
        assert_eq!(forget_response["result"]["memory_id"], json!(memory_id));
        assert_eq!(forget_response["result"]["deleted"], json!(true));
        assert!(
            store
                .recall(AgentMemoryRecallRequest {
                    identity: memory_identity.clone(),
                    realm: "team".to_string(),
                    query_text: None,
                    query_terms: Vec::new(),
                    selection: AgentMemorySelection::Always,
                    max_entries: 64,
                })
                .await?
                .is_empty()
        );

        let recall_after_forget_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 5,
                    "method": "meta/agent_memory/recall",
                    "params": {
                        "identity": "identity:luka",
                        "realm": "team",
                        "selection": "always"
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert!(
            recall_after_forget_response["error"].is_null(),
            "{recall_after_forget_response:#?}"
        );
        assert_eq!(
            recall_after_forget_response["result"]["records"]
                .as_array()
                .map(Vec::len),
            Some(0)
        );

        let unknown_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 6,
                    "method": "meta/agent_memory/remember",
                    "params": {
                        "identity": "identity:unknown",
                        "title": "Orphan",
                        "body": "This should not be written."
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert_eq!(unknown_response["error"]["code"], json!(-32602));
        assert!(
            unknown_response["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("unknown identity")),
            "{unknown_response:#?}"
        );

        let unknown_forget_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 7,
                    "method": "meta/agent_memory/forget",
                    "params": {
                        "identity": "identity:unknown",
                        "memory_id": "mem-missing"
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert_eq!(unknown_forget_response["error"]["code"], json!(-32602));
        assert!(
            unknown_forget_response["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("unknown identity")),
            "{unknown_forget_response:#?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn unified_rpc_agent_memory_update_and_manifest_over_sqlite_store()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-agent-memory-sqlite-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-agent-memory-sqlite-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        let store = Arc::new(crate::memory::SqliteAgentMemoryStore::open(
            temp_dir.path().join("agent-memory"),
        )?);
        let provider: Arc<dyn crate::identity_first::AgentMemoryProvider> = store.clone();
        identity_rt
            .set_agent_memory(Some(
                crate::identity_first::AgentMemoryRuntimeInjector::new(
                    provider.clone(),
                    crate::identity_first::AgentMemoryConfig::default(),
                ),
            ))
            .await;
        let memory_identity = AgentIdentity::parse("identity:luka")?;
        identity_rt
            .register(DurableAgentSpec {
                identity: memory_identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: Default::default(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::new(identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: Some(provider),
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };

        // The sqlite store supports the v2 surface, so update/manifest are
        // advertised alongside the v1 methods.
        let capabilities: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/capabilities",
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        for method in [
            "meta/agent_memory/recall",
            "meta/agent_memory/remember",
            "meta/agent_memory/forget",
            "meta/agent_memory/update",
            "meta/agent_memory/manifest",
        ] {
            assert!(
                capabilities["result"]["methods"]
                    .as_array()
                    .is_some_and(|methods| methods.contains(&json!(method))),
                "missing {method}: {capabilities:#?}"
            );
        }

        let remember_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "meta/agent_memory/remember",
                    "params": {
                        "identity": "identity:luka",
                        "realm": "team",
                        "title": "Weekly standup",
                        "body": "Standup is before calendar planning.",
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert!(
            remember_response["error"].is_null(),
            "{remember_response:#?}"
        );
        let memory_id = remember_response["result"]["memory_id"]
            .as_str()
            .ok_or("memory_id should be present")?
            .to_string();

        let update_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "meta/agent_memory/update",
                    "params": {
                        "identity": "identity:luka",
                        "realm": "team",
                        "memory_id": memory_id.clone(),
                        "title": "Weekly standup",
                        "body": "Standup moved to after calendar planning.",
                        "tags": ["team"]
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert!(update_response["error"].is_null(), "{update_response:#?}");
        let new_id = update_response["result"]["memory_id"]
            .as_str()
            .ok_or("updated memory_id should be present")?
            .to_string();
        assert_ne!(new_id, memory_id);
        assert_eq!(update_response["result"]["supersedes"], json!(memory_id));

        // Only the successor is recallable after the supersede.
        let recall_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "meta/agent_memory/recall",
                    "params": {
                        "identity": "identity:luka",
                        "realm": "team",
                        "selection": "always"
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert!(recall_response["error"].is_null(), "{recall_response:#?}");
        assert_eq!(
            recall_response["result"]["records"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            recall_response["result"]["records"][0]["memory_id"],
            json!(new_id)
        );

        let manifest_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 5,
                    "method": "meta/agent_memory/manifest",
                    "params": {
                        "identity": "identity:luka",
                        "realm": "team",
                        "tier": "working_set",
                        "k": 4
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert!(
            manifest_response["error"].is_null(),
            "{manifest_response:#?}"
        );
        let records = manifest_response["result"]["records"]
            .as_array()
            .ok_or("manifest records array")?;
        assert_eq!(records.len(), 1, "{manifest_response:#?}");
        assert_eq!(records[0]["id"], json!(new_id));
        assert_eq!(records[0]["kind"], json!("fact"));
        assert_eq!(records[0]["age_days"], json!(0));
        assert!(
            records[0].get("body").is_none(),
            "manifest is an index, never a dump: {manifest_response:#?}"
        );

        let bad_tier_response: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 6,
                    "method": "meta/agent_memory/manifest",
                    "params": {
                        "identity": "identity:luka",
                        "tier": "everything"
                    },
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert_eq!(bad_tier_response["error"]["code"], json!(-32602));

        Ok(())
    }

    #[tokio::test]
    async fn unified_rpc_agent_memory_capabilities_do_not_advertise_read_only_writes()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-agent-memory-read-only-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-agent-memory-read-only-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        let provider: Arc<dyn crate::identity_first::AgentMemoryProvider> =
            Arc::new(ReadOnlyAgentMemoryProvider);
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::new(identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: Some(provider),
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };

        let capabilities: Value = serde_json::from_str(
            &handle_unified_rpc_json(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/capabilities",
                })
                .to_string(),
                Duration::from_secs(1),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        let methods = capabilities["result"]["methods"]
            .as_array()
            .ok_or("methods should be an array")?;

        assert!(methods.contains(&json!("meta/agent_memory/recall")));
        assert!(!methods.contains(&json!("meta/agent_memory/remember")));
        assert!(!methods.contains(&json!("meta/agent_memory/forget")));
        Ok(())
    }

    #[test]
    fn identity_actor_timeout_rpc_preserves_owner_observations() {
        for stage in ["actor_command_admission", "actor_command_reply"] {
            let err = crate::identity_first::IdentityRuntimeError::ActorAdmissionTimeout {
                identity: crate::identity_first::AgentIdentity::parse("review:singleton")
                    .expect("valid identity"),
                operation: "deliver.submit_work",
                waited: std::time::Duration::from_millis(5),
                command: Some(crate::identity_first::ActorCommandTimeout {
                    command_kind: "SubmitWork",
                    stage,
                }),
            };
            let error = identity_error_response(json!("req-1"), &err)
                .error
                .expect("RPC error");
            assert_eq!(error.data, err.structured_data());
            let data = error.data.expect("timeout data");
            assert_eq!(data["stage"], stage);
            assert!(data.get("executed").is_none());
            assert!(data.get("retryable").is_none());
            assert!(error.message.contains("execution fate"));
            assert!(!error.message.contains("never started"));
        }
    }

    #[test]
    fn mobpack_authoring_rpc_helper_preserves_runtime_catalog_binding()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = crate::mobpack::MobpackRuntimeCatalogState {
            loaded_modules: vec!["editor-host".to_string()],
            runtime_methods: vec![
                "meta/mobpacks/catalogs".to_string(),
                "meta/mobpacks/apply_operation".to_string(),
                "meta/mobpacks/deploy".to_string(),
            ],
            has_contact_directory: true,
            has_peer_mob_handles: false,
            has_inproc_contacts: false,
            runtime_flow_rows: vec![json!({
                "id": "runtime_rpc_main",
                "source": "meta/runtime/flow_projection",
                "document": {
                    "mob_id": "runtime_rpc",
                    "flow": { "name": "main", "steps": [] },
                    "members": []
                },
                "validation": { "ok": true }
            })],
            runtime_agent_definition_sources: vec![json!({
                "id": "runtime_profiles_rpc",
                "name": "Runtime RPC profiles",
                "source": "meta/runtime/agent-definitions",
                "document": {
                    "mob_id": "runtime_rpc",
                    "members": [{
                        "id": "m_runtime_reviewer",
                        "name": "Runtime reviewer",
                        "role": "runtime_reviewer",
                        "profileBinding": "inline",
                        "model": "gpt-5.5",
                        "runtimeMode": "turn_driven",
                        "tools": ["builtins"],
                        "skills": ["mob.runtime.review"],
                        "schema": ""
                    }],
                    "schemas": []
                }
            })],
            runtime_skill_realms: vec![json!({
                "id": "runtime_rpc",
                "label": "Runtime RPC",
                "source": "meta/runtime/skills",
                "skills": [{
                    "id": "mob.runtime.review",
                    "label": "Runtime review",
                    "source": "inline",
                    "content": "Review runtime work."
                }]
            })],
        };

        let catalogs = super::handle_mobpack_authoring_rpc_with_runtime(
            "meta/mobpacks/catalogs",
            &json!({}),
            json!(1),
            Some(&runtime),
        )
        .expect("catalogs method");
        let catalogs: Value = serde_json::to_value(catalogs)?;
        assert_eq!(catalogs["result"]["runtime_backed"], json!(true));
        assert_eq!(
            catalogs["result"]["authoring_provider"]["runtime_binding"],
            json!("bound")
        );
        assert_eq!(
            catalogs["result"]["runtime_flows"][0]["id"],
            json!("runtime_rpc_main")
        );
        // The list also reads the draft store. An explicit store path keeps it
        // off the host's gateway home, which another Meta install on the same
        // machine may own (its drafts would add rows, and an unadopted
        // predecessor home refuses the call).
        let drafts = tempfile::tempdir()?;
        let listed = super::handle_mobpack_authoring_rpc_with_runtime(
            "meta/mobpacks/list",
            &json!({ "store_path": drafts.path().join("drafts.json") }),
            json!(2),
            Some(&runtime),
        )
        .expect("list method");
        let listed: Value = serde_json::to_value(listed)?;
        assert_eq!(listed["result"]["runtime_backed"], json!(true));
        assert_eq!(listed["result"]["rows"][0]["id"], json!("runtime_rpc_main"));
        assert_eq!(
            listed["result"]["rows"].as_array().map(Vec::len),
            Some(1),
            "an empty draft store adds no rows"
        );

        let fetched = super::handle_mobpack_authoring_rpc_with_runtime(
            "meta/mobpacks/get",
            &json!({ "id": "runtime_rpc_main" }),
            json!(3),
            Some(&runtime),
        )
        .expect("get method");
        let fetched: Value = serde_json::to_value(fetched)?;
        assert_eq!(fetched["result"]["runtime_backed"], json!(true));
        assert_eq!(fetched["result"]["row"]["id"], json!("runtime_rpc_main"));

        let definitions = super::handle_mobpack_authoring_rpc_with_runtime(
            "meta/agent_definitions/list",
            &json!({}),
            json!(4),
            Some(&runtime),
        )
        .expect("agent definitions method");
        let definitions: Value = serde_json::to_value(definitions)?;
        let runtime_definition = definitions["result"]["agent_definitions"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row["sourceOrigin"] == "meta/runtime/agent-definitions")
            })
            .expect("runtime profile definition");
        assert_eq!(runtime_definition["role"], json!("runtime_reviewer"));
        assert_eq!(
            runtime_definition["toolDefinitions"][0]["id"],
            json!("builtins")
        );
        assert_eq!(
            runtime_definition["skillDefinitions"][0]["id"],
            json!("mob.runtime.review")
        );
        assert_eq!(
            definitions["result"]["catalog_snapshot"]["runtime_backed"],
            json!(true)
        );

        let tools = super::handle_mobpack_authoring_rpc_with_runtime(
            "meta/tools/catalog",
            &json!({}),
            json!(5),
            Some(&runtime),
        )
        .expect("tools catalog method");
        let tools: Value = serde_json::to_value(tools)?;
        let mob_tool = tools["result"]["tool_catalog"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|tool| tool["id"] == "mob")
            .expect("mob tool");
        assert_eq!(mob_tool["runtime_availability"]["available"], json!(false));

        let agents = super::handle_mobpack_authoring_rpc_with_runtime(
            "meta/agent_definitions/list",
            &json!({}),
            json!(3),
            Some(&runtime),
        )
        .expect("agent definitions method");
        let agents: Value = serde_json::to_value(agents)?;
        assert_eq!(agents["result"]["runtime_backed"], json!(true));
        let planner = agents["result"]["agent_definitions"]
            .as_array()
            .expect("agent definitions")
            .iter()
            .find(|definition| definition["role"] == "planner")
            .expect("planner definition");
        let planner_mob_tool = planner["toolDefinitions"]
            .as_array()
            .expect("planner tools")
            .iter()
            .find(|tool| tool["id"] == "mob")
            .expect("planner mob tool");
        assert_eq!(
            planner_mob_tool["runtimeAvailability"]["state"],
            json!("unavailable")
        );

        Ok(())
    }

    #[test]
    fn module_rpc_dispatches_mobpack_authoring_methods()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let config = MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: "module-rpc-authoring-dispatch-test".to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        };
        let mut runtime = crate::start_meta_runtime(config, Vec::new(), Duration::from_secs(1))?;

        let capabilities: Value = serde_json::from_str(&super::handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "meta/capabilities",
            })
            .to_string(),
            Duration::from_secs(1),
        ))?;
        let methods = capabilities["result"]["methods"]
            .as_array()
            .expect("methods array")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        for method in super::MOBPACK_AUTHORING_METHODS {
            assert!(
                methods.contains(method),
                "missing authoring method {method}"
            );
        }
        assert_eq!(
            capabilities["result"]["authoring_capabilities"]["runtime_mutation"],
            json!(false)
        );
        assert_eq!(
            capabilities["result"]["authoring_capabilities"]["host_mutation_methods"]["meta/mobpacks/deploy"],
            json!("when execute=true, writes a mobpack archive and runs rkat mob run on the host")
        );

        let schema: Value = serde_json::from_str(&super::handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "meta/mobpacks/schema",
            })
            .to_string(),
            Duration::from_secs(1),
        ))?;
        assert!(schema["error"].is_null(), "{schema:#?}");
        assert_eq!(
            schema["result"]["commands"]["deploy_rpc"],
            json!("meta/mobpacks/deploy")
        );
        assert!(schema["result"]["agent_definitions"].is_null());

        let _ = runtime.shutdown();
        Ok(())
    }

    #[test]
    fn generated_runtime_ids_match_their_durable_identity_prefix() {
        assert!(!rpc_member_id_matches_durable_identity(
            "rt:review:singleton:0",
            "review:singleton",
        ));
        assert!(!rpc_member_id_matches_durable_identity(
            "review:singleton:gen1",
            "review:singleton",
        ));
        assert!(!rpc_member_id_matches_durable_identity(
            "review:singleton:1",
            "review:singleton",
        ));
        assert!(!rpc_member_id_matches_durable_identity(
            "rt:reviewer:singleton:0",
            "review:singleton",
        ));
        assert!(!rpc_member_id_matches_durable_identity(
            "rt:review:singleton:qa:0",
            "review:singleton",
        ));
        assert!(!rpc_member_id_matches_durable_identity(
            "review:singleton:qa",
            "review:singleton",
        ));
        assert_eq!(
            rpc_runtime_alias_generation("rt:review:singleton:7", "review:singleton"),
            Some(7)
        );
        assert_eq!(
            rpc_runtime_alias_generation("rt:review:singleton:8", "review:other"),
            None
        );
    }

    #[test]
    fn rpc_live_identity_visibility_matches_delegate_projection_labels() {
        assert!(rpc_live_identity_alias_visible("worker", &BTreeMap::new()));

        let mut labels = BTreeMap::new();
        labels.insert("role".to_string(), "delegate".to_string());
        labels.insert("source_mob_id".to_string(), "mob-a".to_string());
        labels.insert("agent_identity".to_string(), "review:singleton".to_string());
        assert!(!rpc_live_identity_alias_visible("worker", &labels));
        assert!(!rpc_live_identity_alias_visible("delegate", &labels));
    }

    #[test]
    fn ambiguous_live_alias_errors_include_structured_data() -> Result<(), serde_json::Error> {
        let response: Value = serde_json::from_str(&error_response(
            json!(1),
            -32602,
            "ambiguous live identity alias review:singleton: candidates [rt:review:singleton:0, rt:review:singleton:1]",
        ))?;

        assert_eq!(
            response["error"]["data"]["kind"],
            json!("ambiguous_live_identity_alias")
        );
        assert_eq!(
            response["error"]["data"]["identity"],
            json!("review:singleton")
        );
        assert_eq!(
            response["error"]["data"]["candidates"],
            json!(["rt:review:singleton:0", "rt:review:singleton:1"])
        );
        Ok(())
    }

    #[test]
    fn wrapped_ambiguous_live_alias_errors_include_structured_data() -> Result<(), serde_json::Error>
    {
        let response: Value = serde_json::from_str(&error_response(
            json!(1),
            -32602,
            "invalid identity: ambiguous live identity alias review:singleton: candidates [rt:review:singleton:0, rt:review:singleton:1]",
        ))?;

        assert_eq!(
            response["error"]["data"]["kind"],
            json!("ambiguous_live_identity_alias")
        );
        assert_eq!(
            response["error"]["data"]["identity"],
            json!("review:singleton")
        );
        assert_eq!(
            response["error"]["data"]["candidates"],
            json!(["rt:review:singleton:0", "rt:review:singleton:1"])
        );
        Ok(())
    }

    #[tokio::test]
    async fn runtime_id_live_only_resolution_rejects_duplicate_projected_identity()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-identity-alias-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        for runtime_id in ["rt:review:singleton:0", "rt:review:singleton:1"] {
            let mut labels = BTreeMap::new();
            labels.insert("agent_identity".to_string(), "review:singleton".to_string());
            spawn_identity_projection_fixture(
                &runtime,
                SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    runtime_id.to_string(),
                    Some("You are a duplicate Review Agent.".into()),
                    None,
                    None,
                )
                .with_labels(labels),
            )
            .await?;
        }
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-identity-alias-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });

        let err =
            resolve_rpc_identity_control_target(&runtime, &identity_rt, "rt:review:singleton:0")
                .await
                .expect_err("runtime-id live-only fallback should reject duplicate durable alias");
        assert!(
            err.to_string()
                .contains("ambiguous live identity alias review:singleton"),
            "unexpected error: {err}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn stdio_identity_methods_refuse_a_stale_runtime_id_with_the_typed_binding_refusal()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // After a reset the predecessor's generated id is no identity's binding
        // and no live member carries it: the reset rebinds the identity's one
        // roster row. Here no member is live at all.
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-stale-runtime-id-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-stale-runtime-id-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        }));
        // An implicit delegate member the stdio surface hides, bound at
        // generation 1 like the singleton.
        let hidden_identity = AgentIdentity::parse("review:delegate")?;
        register_live(
            &identity_runtime,
            &core_bridge.core,
            DurableAgentSpec {
                identity: hidden_identity.clone(),
                profile: meerkat_mob::ProfileName::from("delegate"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::from([("source_mob_id".to_string(), "child-mob".to_string())]),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            1,
            meerkat_core::types::SessionId::new(),
        )
        .await?;
        let identity = AgentIdentity::parse("review:singleton")?;
        let session_id = meerkat_core::types::SessionId::new();
        register_live(
            &identity_runtime,
            &core_bridge.core,
            DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            1,
            session_id.clone(),
        )
        .await?;
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                identity_runtime.clone(),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: Some(runtime.mob_handle().definition().clone()),
            transcript_edit_service: None,
            // Wired, so meta/compact_member reaches its identity target.
            compaction_floors: Some(Arc::new(
                crate::identity_first::CompactionFloorRegistry::default(),
            )),
        };
        let call = |method: &'static str, requested: &'static str| {
            let runtime = &runtime;
            let identity_ctx = &identity_ctx;
            async move {
                let params = if method == "meta/get_member" {
                    json!({ "member_id": requested })
                } else {
                    json!({ "identity": requested })
                };
                let request = json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": method,
                    "params": params,
                })
                .to_string();
                let response = handle_unified_rpc_json(
                    runtime,
                    &request,
                    Duration::from_secs(1),
                    None,
                    Some(identity_ctx),
                )
                .await;
                serde_json::from_str::<Value>(&response)
            }
        };

        // Every identity method the stale-binding refusal covers: the identity
        // status, inspect and lifecycle methods, the delivery methods, and two
        // operator member verbs (the operator path answers through its own
        // response conversion). meta/member_activate_instruction and
        // meta/bound_member_transcript need an activation and a wired
        // transcript service before they resolve; the operator verb harness
        // covers them (operator_verbs_with_preconditions_refuse_a_stale_runtime_id).
        let operator_methods = ["meta/member_instruction_activations", "meta/compact_member"];
        for method in [
            "meta/status_identity",
            "meta/inspect_identity",
            "meta/retire",
            "meta/respawn",
            "meta/reset",
            "meta/delete_identity",
            "meta/send",
            "meta/send_status",
            "meta/interact",
            "meta/dispatch",
            "meta/recover_delivery",
            "meta/turn_result",
            "meta/wait_for_turn",
            "meta/subscribe",
            "meta/wait_for_completion",
            "meta/completion_cursor",
            "meta/member_instruction_activations",
            "meta/compact_member",
        ] {
            let response = Box::pin(call(method, "rt:review:singleton:0")).await?;
            assert_eq!(
                response["error"],
                json!({
                    "code": -32000,
                    "message": "identity runtime binding for review:singleton points at \
                                rt:review:singleton:1, but requested live member is \
                                rt:review:singleton:0",
                    "data": {
                        "kind": "stale_identity_runtime_binding",
                        "identity": "review:singleton",
                        "registered_runtime_member_id": "rt:review:singleton:1",
                        "live_runtime_member_id": "rt:review:singleton:0",
                        "registered_session_id": session_id.to_string(),
                        "live_session_id": null,
                    },
                }),
                "{method} answers a stale runtime id with the typed refusal: {response:#?}"
            );

            // A runtime id of an identity the identity runtime does not know
            // stays not found.
            let unknown = Box::pin(call(method, "rt:review:nobody:0")).await?;
            assert_eq!(
                (&unknown["error"]["code"], &unknown["error"]["message"]),
                (
                    &json!(-32602),
                    &json!("invalid identity: runtime identity not found: rt:review:nobody:0")
                ),
                "{method}: {unknown:#?}"
            );

            // A stale runtime id of an identity the stdio surface hides
            // answers hidden by policy and names no binding.
            let hidden = Box::pin(call(method, "rt:review:delegate:0")).await?;
            assert_eq!(
                (&hidden["error"]["code"], &hidden["error"]["message"]),
                (
                    &json!(-32602),
                    &json!("invalid identity: identity hidden by policy: rt:review:delegate:0")
                ),
                "{method}: {hidden:#?}"
            );
            if !operator_methods.contains(&method) {
                assert_eq!(
                    hidden["error"]["data"],
                    json!({"kind": "identity_hidden_by_policy", "identity": "rt:review:delegate:0"}),
                    "{method}: {hidden:#?}"
                );
            }
            assert!(
                !hidden.to_string().contains("rt:review:delegate:1"),
                "{method} names no binding of a hidden identity: {hidden}"
            );
        }

        // The stdio member methods send the same six-field data object.
        let member = Box::pin(call("meta/get_member", "rt:review:singleton:0")).await?;
        assert_eq!(
            member["error"],
            json!({
                "code": -32000,
                "message": "identity runtime binding for review:singleton points at \
                            rt:review:singleton:1, but requested live member is \
                            rt:review:singleton:0",
                "data": {
                    "kind": "stale_identity_runtime_binding",
                    "identity": "review:singleton",
                    "registered_runtime_member_id": "rt:review:singleton:1",
                    "live_runtime_member_id": "rt:review:singleton:0",
                    "registered_session_id": session_id.to_string(),
                    "live_session_id": null,
                },
            }),
            "{member:#?}"
        );
        let status = identity_runtime.status(&identity).await?;
        assert_eq!(
            (
                status.state,
                status
                    .agent_runtime_id
                    .as_ref()
                    .map(|runtime_id| runtime_id.as_str().to_string()),
                status.session_id,
            ),
            (
                IdentityLifecycleState::Active,
                Some("rt:review:singleton:1".to_string()),
                Some(session_id),
            ),
            "refusing a stale runtime id changes nothing"
        );
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test]
    async fn durable_resolution_prefers_registered_live_binding_over_stale_duplicates()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-identity-alias-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        for runtime_id in ["rt:review:singleton:0", "rt:review:singleton:1"] {
            let mut labels = BTreeMap::new();
            labels.insert("agent_identity".to_string(), "review:singleton".to_string());
            spawn_identity_projection_fixture(
                &runtime,
                SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    runtime_id.to_string(),
                    Some("You are a duplicate Review Agent.".into()),
                    None,
                    None,
                )
                .with_labels(labels),
            )
            .await?;
        }
        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-identity-alias-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        });
        let identity = AgentIdentity::parse("review:singleton")?;
        register_live(
            &identity_rt,
            &core_bridge.core,
            DurableAgentSpec {
                identity,
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            1,
            meerkat_core::types::SessionId::new(),
        )
        .await?;

        let target =
            resolve_rpc_identity_control_target(&runtime, &identity_rt, "review:singleton").await?;
        assert_eq!(target.identity.as_str(), "review:singleton");
        assert_eq!(
            target
                .live
                .as_ref()
                .map(|alias| alias.runtime_member_id.as_str()),
            Some("rt:review:singleton:1")
        );

        let target =
            resolve_rpc_identity_control_target(&runtime, &identity_rt, "rt:review:singleton:1")
                .await?;
        assert_eq!(
            target
                .live
                .as_ref()
                .map(|alias| alias.runtime_member_id.as_str()),
            Some("rt:review:singleton:1")
        );

        let stale_target =
            resolve_rpc_identity_control_target(&runtime, &identity_rt, "rt:review:singleton:0")
                .await?;
        assert_eq!(
            stale_target
                .live
                .as_ref()
                .map(|alias| alias.runtime_member_id.as_str()),
            Some("rt:review:singleton:0")
        );
        let stale_response =
            super::rpc_stale_live_alias_error_response(&identity_rt, &stale_target, json!(99))
                .await
                .expect("old reset generation should be rejected as stale");
        assert_eq!(
            stale_response
                .error
                .as_ref()
                .and_then(|error| error.data.as_ref())
                .and_then(|data| data.get("kind")),
            Some(&json!("stale_identity_runtime_binding"))
        );
        assert_eq!(
            stale_response
                .error
                .as_ref()
                .and_then(|error| error.data.as_ref())
                .and_then(|data| data.get("registered_runtime_member_id")),
            Some(&json!("rt:review:singleton:1"))
        );
        assert_eq!(
            stale_response
                .error
                .as_ref()
                .and_then(|error| error.data.as_ref())
                .and_then(|data| data.get("live_runtime_member_id")),
            Some(&json!("rt:review:singleton:0"))
        );

        Ok(())
    }

    #[tokio::test]
    async fn current_generation_resolvers_ignore_stale_reset_member_rows()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let mut runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-current-generation-target-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let durable_identity = "review:current-generation";
        // ONE stable roster row. This used to seat two, rt:...:0 and rt:...:1,
        // because the generation was part of the roster id and a reset left the
        // predecessor behind for resolvers to skip. Under the durable-roster
        // contract there is exactly one row per identity and a reset REPLACES
        // its binding, so two live generations cannot coexist as rows. The
        // property still worth guarding is that the resolver follows the
        // replacement and refuses a stale binding projection, which is what this
        // test now asserts.
        let successor_alias = "rt:review:current-generation:1";
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                durable_identity.to_string(),
                None,
                None,
                None,
            )
            .with_labels(BTreeMap::from([(
                "agent_identity".to_string(),
                durable_identity.to_string(),
            )])),
        )
        .await?;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-current-generation-target-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        }));
        let identity = AgentIdentity::parse(durable_identity)?;
        // The row's ACTUAL bridge session. The old fixture registered a fresh
        // random SessionId here, which only went unnoticed because nothing
        // compared it; a real registered binding names the session its member is
        // actually bound to.
        let expected_session = runtime
            .mob_handle()
            .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id(durable_identity))
            .await
            .ok_or("the stable roster row must have a bridge session")?;
        // Core holds the identity's successor at generation 1, bound to the
        // row's session.
        let core_runtime_id = register_live(
            &identity_runtime,
            &core_bridge.core,
            rpc_durable_spec(durable_identity, "worker"),
            1,
            expected_session.clone(),
        )
        .await?;
        assert_eq!(core_runtime_id.as_str(), successor_alias);
        let params = json!({"identity": durable_identity});
        let target = identity_runtime
            .member_alias_lifecycle_target(durable_identity)
            .await?
            .ok_or("durable identity must resolve to lifecycle authority")?;
        let operation_runtime = Arc::clone(&identity_runtime);
        let handle = runtime.mob_handle();
        let resolved_session = IdentityRuntime::run_member_alias_targets_operation_tracked(
            vec![target],
            move || async move {
                resolve_live_target(&handle, Some(&operation_runtime), true, &params).await
            },
        )
        .await?;
        assert_eq!(resolved_session, Some(expected_session.clone()));

        // A STALE BINDING PROJECTION must not be adopted. Core's history now
        // binds the identity to a session the row is not bound to -
        // the shape a resolver would see if it read a projection left behind by a
        // superseded incarnation - and require the resolver to refuse rather than
        // hand back a session that does not belong to the live member.
        bind_live(
            &core_bridge.core,
            &identity,
            &meerkat_mob::ProfileName::from("worker"),
            1,
            Some(meerkat_core::types::SessionId::new()),
        )?;
        let stale_target = identity_runtime
            .member_alias_lifecycle_target(durable_identity)
            .await?
            .ok_or("durable identity must resolve to lifecycle authority")?;
        let stale_runtime = Arc::clone(&identity_runtime);
        let stale_handle = runtime.mob_handle();
        let stale_params = json!({"identity": durable_identity});
        let stale_result = IdentityRuntime::run_member_alias_targets_operation_tracked(
            vec![stale_target],
            move || async move {
                resolve_live_target(&stale_handle, Some(&stale_runtime), true, &stale_params).await
            },
        )
        .await;
        let stale_error = stale_result
            .expect_err("a binding naming a session the live row is not bound to must be refused");
        assert!(
            stale_error.to_string().contains("registered for"),
            "the refusal must name the disagreeing registered session: {stale_error}"
        );

        // Restore the truthful binding for the remainder of the test.
        bind_live(
            &core_bridge.core,
            &identity,
            &meerkat_mob::ProfileName::from("worker"),
            1,
            Some(expected_session),
        )?;

        runtime.attach_identity_first_context(Arc::new(
            crate::identity_first::IdentityFirstRuntimeContext::new(
                Arc::clone(&identity_runtime),
                Arc::new(EmptyRosterProvider),
                None,
                None,
                None,
            ),
        ));
        let (_, comms_name, _) = runtime.local_member_peer_info(durable_identity).await?;
        assert!(
            comms_name
                .ends_with(crate::member_comms_id::mob_member_id_str(durable_identity).as_ref()),
            "peer info must name the stable roster row: {comms_name}"
        );

        let remote_comms_name = "remote-mob/worker/peer";
        let remote_pubkey = [42_u8; 32];
        let remote_peer_id =
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&remote_pubkey).to_string();
        runtime
            .wire_local(
                durable_identity,
                remote_comms_name,
                &remote_peer_id,
                &format!("inproc://{remote_comms_name}"),
                Some(remote_pubkey),
            )
            .await?;
        // The wire lands on the stable row. The companion half of this - that a
        // STALE generation's row does not receive it - is unrepresentable now
        // that there is one row per identity; the wire cannot be delivered to a
        // predecessor that no longer exists as a row.
        let current = runtime
            .mob_handle()
            .get_member(&crate::member_comms_id::mob_member_id(durable_identity))
            .await?
            .ok_or("the stable roster row must exist")?;
        assert!(
            current
                .wired_to
                .iter()
                .any(|peer| peer.as_str() == remote_comms_name),
            "the stable roster row must receive the wire"
        );
        let rows = runtime.mob_handle().list_members_including_retiring().await;
        let matching = rows
            .iter()
            .filter(|row| {
                crate::member_comms_id::runtime_alias_str(row.agent_identity.as_str())
                    == durable_identity
            })
            .count();
        assert_eq!(
            matching, 1,
            "exactly one roster row may exist for a durable identity: {rows:#?}"
        );

        runtime.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn durable_resolution_rejects_hidden_registered_live_binding()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-hidden-bound-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are a hidden Review Agent.".into()),
                None,
                None,
            )
            .with_labels(BTreeMap::from([
                ("agent_identity".to_string(), "review:singleton".to_string()),
                ("role".to_string(), "delegate".to_string()),
                ("source_mob_id".to_string(), "upstream".to_string()),
            ])),
        )
        .await?;
        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-hidden-bound-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        });
        let identity = AgentIdentity::parse("review:singleton")?;
        register_live(
            &identity_rt,
            &core_bridge.core,
            DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            meerkat_core::types::SessionId::new(),
        )
        .await?;

        for requested_identity in ["review:singleton", "rt:review:singleton:0"] {
            let err =
                resolve_rpc_identity_control_target(&runtime, &identity_rt, requested_identity)
                    .await
                    .expect_err("hidden registered live binding must not resolve");
            assert!(
                err.to_string().contains("identity hidden by policy"),
                "unexpected error for {requested_identity}: {err}"
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn live_only_hidden_alias_reports_policy_error()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-hidden-live-only-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are a hidden Review Agent.".into()),
                None,
                None,
            )
            .with_labels(BTreeMap::from([
                ("agent_identity".to_string(), "review:singleton".to_string()),
                ("role".to_string(), "delegate".to_string()),
                ("source_mob_id".to_string(), "upstream".to_string()),
            ])),
        )
        .await?;
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-hidden-live-only-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });

        for requested_identity in ["review:singleton", "rt:review:singleton:0"] {
            let err =
                resolve_rpc_identity_control_target(&runtime, &identity_rt, requested_identity)
                    .await
                    .expect_err("hidden live-only alias must not collapse into unknown identity");
            assert!(
                err.to_string().contains("identity hidden by policy"),
                "unexpected error for {requested_identity}: {err}"
            );
        }

        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::new(identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };
        for requested_identity in ["review:singleton", "rt:review:singleton:0"] {
            let response: Value = serde_json::from_str(
                &handle_unified_rpc_json(
                    &runtime,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "meta/status_identity",
                        "params": { "identity": requested_identity },
                    })
                    .to_string(),
                    Duration::from_secs(1),
                    None,
                    Some(&identity_ctx),
                )
                .await,
            )?;
            assert_eq!(
                response["error"]["data"]["kind"],
                json!("identity_hidden_by_policy"),
                "unexpected hidden response for {requested_identity}: {response:#?}"
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn live_only_resolution_rejects_runtime_member_bound_to_other_durable_identity()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-identity-alias-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let mut labels = BTreeMap::new();
        labels.insert("agent_identity".to_string(), "other:singleton".to_string());
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are a wrong-projected Review Agent.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await?;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-identity-alias-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        });
        let identity = AgentIdentity::parse("review:singleton")?;
        register_live(
            &identity_rt,
            &core_bridge.core,
            DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            meerkat_core::types::SessionId::new(),
        )
        .await?;

        let err = resolve_rpc_identity_control_target(&runtime, &identity_rt, "other:singleton")
            .await
            .expect_err("wrong-projected live alias must not resolve as live-only");
        assert!(
            err.to_string()
                .contains("identity runtime binding belongs to review:singleton"),
            "unexpected error: {err}"
        );

        Ok(())
    }

    /// Regression: a gateway-plane `meta/send_message` addressed to the bare
    /// durable identity (the only id the SDK hands out pre-burst) used to fail
    /// with `mob member not found`, because identity-first roster members were
    /// keyed `rt:{identity}:{generation}`. Roster members are now keyed by the
    /// durable identity itself, so the bare id resolves directly. Bridge
    /// resolution stays in place for callers that hand over a generated alias,
    /// and an exact roster member id match still keeps raw member-id semantics.
    #[tokio::test]
    async fn send_message_resolves_bare_durable_identity_through_identity_bridge()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-send-message-identity-bridge-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(5))
                .build(),
        )
        .await?;

        // Identity-first roster shape: each durable identity IS the roster
        // identity, seated under its comms-safe encoding. This changed
        // deliberately - it used to seat `rt:{identity}:0` and this comment used
        // to say the bare identity was not a roster id. Meerkat's adoption takes
        // ONE identity for both the roster lookup and the durable intent key, so
        // a per-incarnation roster id could not carry intent across a respawn.
        // `AgentRuntimeId` is now incarnation detail only.
        for durable in ["atlas-base-001", "draco-base-001"] {
            spawn_identity_projection_fixture(
                &runtime,
                SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    durable.to_string(),
                    Some("You are a swarm base agent.".into()),
                    None,
                    None,
                )
                .with_labels(BTreeMap::from([(
                    "agent_identity".to_string(),
                    durable.to_string(),
                )])),
            )
            .await?;
        }
        // Raw roster member, on an id that is NOT a durable identity.
        //
        // This used to be a PRECEDENCE probe: a raw member seated under the same
        // name as a durable identity, asserting an exact roster-id match wins
        // over identity resolution. That collision is now unrepresentable - the
        // durable identity IS the roster id, so the two would be one row and the
        // second spawn would collide. The precedence rule still exists in the
        // resolver and still applies wherever a raw id and an identity differ;
        // what is gone is the case where they are the same string.
        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "raw-worker-001".to_string(),
                Some("You are the raw roster member.".into()),
                None,
                None,
            ))
            .await?;

        let session_service = runtime
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("test mob spec has a session service");
        let bridge: Arc<dyn crate::identity_first::SessionBridge> = Arc::new(
            crate::identity_first::MobSessionBridge::with_session_service(
                runtime.mob_handle(),
                session_service,
            ),
        );
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-send-message-identity-bridge-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        })
        .with_runtime_services(crate::identity_first::AgentRuntimeServices::new(
            runtime.mob_handle(),
        ));
        // Core holds both identities' members (the rows above), so each
        // registered identity is Active at Core's generation 0.
        for durable in ["atlas-base-001", "draco-base-001"] {
            let identity = AgentIdentity::parse(durable)?;
            identity_rt
                .register(DurableAgentSpec {
                    identity: identity.clone(),
                    profile: meerkat_mob::ProfileName::from("worker"),
                    addressability: AgentAddressability::Addressable,
                    display_name: None,
                    labels: BTreeMap::new(),
                    context: None,
                    additional_instructions: Vec::new(),
                    initial_message: None,
                    runtime_mode_override: None,
                    backend: None,
                    binding: None,
                    placement: None,
                })
                .await;
        }
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::new(identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };

        let send = |id: u64, params: Value| {
            let runtime = &runtime;
            let identity_ctx = &identity_ctx;
            async move {
                let raw = handle_unified_rpc_json(
                    runtime,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "meta/send_message",
                        "params": params,
                    })
                    .to_string(),
                    Duration::from_secs(10),
                    None,
                    Some(identity_ctx),
                )
                .await;
                serde_json::from_str::<Value>(&raw)
            }
        };

        // 1. A bare durable identity reaches its roster member and reports the
        //    bridge session that took the delivery.
        let response = send(
            1,
            json!({ "member_id": "atlas-base-001", "message": "status check" }),
        )
        .await?;
        assert!(
            response["error"].is_null(),
            "bare identity send must bridge-resolve: {response:#?}"
        );
        assert_eq!(response["result"]["accepted"], json!(true));
        assert_eq!(response["result"]["member_id"], json!("atlas-base-001"));
        let atlas_session = runtime
            .mob_handle()
            .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id("atlas-base-001"))
            .await
            .expect("atlas member has a bridge session after send")
            .to_string();
        assert_eq!(response["result"]["session_id"], json!(atlas_session));

        // 2. Steer rides the same bridge resolution.
        let response = send(
            2,
            json!({
                "member_id": "atlas-base-001",
                "message": "steer: stand down",
                "handling_mode": "steer",
            }),
        )
        .await?;
        assert!(
            response["error"].is_null(),
            "bare identity steer must bridge-resolve: {response:#?}"
        );
        assert_eq!(response["result"]["accepted"], json!(true));

        // 3. A raw roster member id keeps raw semantics: it takes the delivery
        //    directly, with no identity resolution involved.
        let response = send(
            3,
            json!({ "member_id": "raw-worker-001", "message": "raw roster delivery" }),
        )
        .await?;
        assert!(
            response["error"].is_null(),
            "exact roster member send must keep raw semantics: {response:#?}"
        );
        assert_eq!(response["result"]["accepted"], json!(true));
        let raw_session = runtime
            .mob_handle()
            .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id("raw-worker-001"))
            .await
            .expect("raw roster member has a bridge session after send")
            .to_string();
        assert_eq!(response["result"]["session_id"], json!(raw_session));
        // And it is not the durable identity's session: a raw member id must not
        // be resolved through the identity plane.
        let draco_session = runtime
            .mob_handle()
            .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id("draco-base-001"))
            .await
            .map(|session| session.to_string());
        assert_ne!(
            Some(raw_session),
            draco_session,
            "a raw roster member must not share a durable identity's bridge session"
        );

        // 4. Unknown ids keep raw member-not-found semantics.
        let response = send(
            4,
            json!({ "member_id": "phantom-base-999", "message": "nobody home" }),
        )
        .await?;
        assert_eq!(response["error"]["code"], json!(-32000), "{response:#?}");
        let message = response["error"]["message"]
            .as_str()
            .expect("error message");
        assert!(
            message.starts_with("send_message failed:"),
            "unexpected error message: {message}"
        );

        Ok(())
    }

    /// #469: `meta/send_message` used to reserve "meta-send-..." ids that
    /// no delivery path could thread, so every reservation stayed pending
    /// until the 256 cap evicted settled sends with a false
    /// `interaction_failed { reason: "queue_overflow" }`. The id is now a UUID
    /// carried as typed lineage on both delivery targets (identity bridge and
    /// raw roster member), and each run's terminal closes its reservation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_message_reservations_close_on_their_run_past_the_pending_cap()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let definition = MobDefinition::from_toml(
            r#"
[mob]
id = "send-message-reservation-settlement-test"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true

[profiles.autonomous]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.autonomous.tools]
comms = true
"#,
        )?;
        let identity = AgentIdentity::parse("identity:settler")?;
        let roster = Arc::new(crate::identity_first::MutableRosterProvider::new(vec![
            DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                backend: None,
                binding: None,
                placement: None,
            },
        ]));
        let scratch = tempfile::tempdir()?;
        let runtime = UnifiedRuntime::builder()
            .definition(definition)
            .continuity_store(Arc::new(LocalContinuityStore::in_memory()?))
            .lease_provider(Arc::new(LocalLeaseProvider::new()))
            .roster_provider(roster)
            .scratch_dir(scratch.path())
            .ephemeral_runtime_store(true)
            .identity_runtime_instance_id("send-message-reservation-settlement-test")
            .default_llm_client(Arc::new(TestClient::for_provider(
                meerkat_core::Provider::OpenAI,
            )))
            .build()
            .await?;
        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "raw-settler".to_string(),
                Some("You are a raw roster worker.".into()),
                None,
                None,
            ))
            .await?;
        // #479: an autonomous host must carry the send_message interaction id
        // on the run lineage it starts, the same as a turn-driven member.
        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "autonomous".to_string(),
                "raw-autonomous-settler".to_string(),
                Some("You are an autonomous raw roster worker.".into()),
                None,
                None,
            ))
            .await?;

        let sends = crate::unified_runtime::console_events::PENDING_INTERACTION_CAP + 4;
        for member_id in [identity.as_str(), "raw-settler", "raw-autonomous-settler"] {
            for index in 0..sends {
                let raw = handle_unified_rpc_json(
                    &runtime,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": index,
                        "method": "meta/send_message",
                        "params": {
                            "member_id": member_id,
                            "message": format!("probe {index}"),
                        },
                    })
                    .to_string(),
                    Duration::from_secs(10),
                    None,
                    None,
                )
                .await;
                let response: Value = serde_json::from_str(&raw)?;
                assert!(
                    response["error"].is_null(),
                    "send {index} to {member_id} must be accepted: {response:#?}"
                );

                let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
                loop {
                    runtime.drain_mob_agent_events().await?;
                    if runtime
                        .console_events()
                        .pending_interaction_ids(member_id)
                        .await
                        .is_empty()
                    {
                        break;
                    }
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "reservation for send {index} to {member_id} never closed: {:?}",
                        runtime
                            .console_events()
                            .pending_interaction_ids(member_id)
                            .await
                    );
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }

            let replay = runtime
                .console_events()
                .replay_all(None)
                .await
                .map_err(|err| format!("console replay unavailable: {err:?}"))?;
            let member_events = replay
                .iter()
                .filter(|event| event.identity == member_id)
                .collect::<Vec<_>>();
            let reserved = member_events
                .iter()
                .filter(|event| {
                    event.event_type == "user_input"
                        && event.data["origin"] == json!("meta/send_message")
                })
                .filter_map(|event| event.interaction_id.clone())
                .collect::<Vec<_>>();
            assert!(!reserved.is_empty(), "{member_id} sends must be recorded");
            for interaction_id in &reserved {
                assert!(
                    uuid::Uuid::parse_str(interaction_id).is_ok(),
                    "send_message reservation ids are UUIDs: {interaction_id}"
                );
                assert!(
                    member_events.iter().any(|event| {
                        event.event_type == "interaction_complete"
                            && event.interaction_id.as_deref() == Some(interaction_id)
                    }),
                    "{member_id} reservation {interaction_id} must settle from its own run"
                );
            }
            assert!(
                !member_events
                    .iter()
                    .any(|event| event.data.get("reason") == Some(&json!("queue_overflow"))),
                "{member_id}: no settled send may be evicted as queue overflow"
            );
        }

        runtime.shutdown().await;
        Ok(())
    }

    /// Regression for the SDK `handle.send()` door (`meta/send_message`):
    /// identity-first members use a stable roster id, so the old roster-first
    /// resolver took the raw MobMember arm and skipped `prepare_member_delivery`.
    /// A turn-surface ledger row is written synchronously by Meta preparation
    /// and means the memory was OFFERED; typed transcript materialization is the
    /// later DELIVERED fact owned by Meerkat activation.
    ///
    /// This 0.8.32 companion test is intentionally turn-driven. The paired
    /// Meerkat 0.8.33 regression covers the same typed carrier at an
    /// AutonomousHost wait-and-hold activation boundary.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_message_identity_roster_row_prepares_memory_exactly_once()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Default)]
        struct CapturingMemoryProvider {
            recalls: std::sync::Mutex<Vec<AgentMemoryRecallRequest>>,
            injections: std::sync::Mutex<Vec<crate::memory::InjectionLogEntry>>,
        }

        #[async_trait]
        impl AgentMemoryProvider for CapturingMemoryProvider {
            async fn recall(
                &self,
                request: AgentMemoryRecallRequest,
            ) -> Result<
                Vec<crate::identity_first::AgentMemoryRecord>,
                crate::identity_first::AgentMemoryError,
            > {
                self.recalls
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(request);
                Ok(vec![crate::identity_first::AgentMemoryRecord {
                    memory_id: "mem-sdk-routing".to_string(),
                    title: "Archive code".to_string(),
                    body: "The archive code is ORCHID-7734.".to_string(),
                    tags: vec!["archive".to_string()],
                    created_at_ms: 1,
                    updated_at_ms: 1,
                }])
            }

            async fn mark_usage(
                &self,
                _ids: &[String],
                _event: crate::memory::UsageEvent,
            ) -> Result<(), crate::identity_first::AgentMemoryError> {
                Ok(())
            }

            async fn log_injections(
                &self,
                _realm: &str,
                entries: &[crate::memory::InjectionLogEntry],
            ) -> Result<(), crate::identity_first::AgentMemoryError> {
                self.injections
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend_from_slice(entries);
                Ok(())
            }
        }

        let definition = MobDefinition::from_toml(
            r#"
[mob]
id = "send-message-memory-preparation-test"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        )?;
        let identity = AgentIdentity::parse("identity:archivist")?;
        let roster = Arc::new(crate::identity_first::MutableRosterProvider::new(vec![
            DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                backend: None,
                binding: None,
                placement: None,
            },
        ]));
        let memory = Arc::new(CapturingMemoryProvider::default());
        let scratch = tempfile::tempdir()?;
        let runtime = UnifiedRuntime::builder()
            .definition(definition)
            .continuity_store(Arc::new(LocalContinuityStore::in_memory()?))
            .lease_provider(Arc::new(LocalLeaseProvider::new()))
            .roster_provider(roster)
            .scratch_dir(scratch.path())
            .ephemeral_runtime_store(true)
            .identity_runtime_instance_id("send-message-memory-preparation-test")
            .agent_memory(
                memory.clone(),
                crate::identity_first::AgentMemoryConfig {
                    selection: AgentMemorySelection::Always,
                    max_entries: 1,
                    ..crate::identity_first::AgentMemoryConfig::default()
                },
            )
            .default_llm_client(Arc::new(TestClient::for_provider(
                meerkat_core::Provider::OpenAI,
            )))
            .build()
            .await?;

        let stable_roster_id =
            crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        assert!(
            runtime
                .mob_handle()
                .get_member(&stable_roster_id)
                .await?
                .is_some(),
            "fixture must exercise the exact roster-row branch"
        );

        let raw = handle_unified_rpc_json(
            &runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "meta/send_message",
                "params": {
                    "member_id": identity.as_str(),
                    "message": "What is the archive code?"
                },
            })
            .to_string(),
            Duration::from_secs(10),
            None,
            None,
        )
        .await;
        let response: Value = serde_json::from_str(&raw)?;
        assert!(
            response["error"].is_null(),
            "SDK-style identity send must be accepted: {response:#?}"
        );
        assert_eq!(response["result"]["accepted"], json!(true));
        let response_session = response["result"]["session_id"]
            .as_str()
            .expect("identity delivery reports its bridge session");

        {
            let recalls = memory
                .recalls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let turn_recalls = recalls
                .iter()
                .filter(|request| {
                    request.query_text.as_deref() == Some("What is the archive code?")
                })
                .count();
            assert_eq!(
                turn_recalls, 1,
                "the SDK door must invoke per-turn preparation exactly once"
            );
        }

        {
            let injections = memory
                .injections
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let turn_injections = injections
                .iter()
                .filter(|entry| entry.surface == crate::memory::InjectionSurface::Turn)
                .collect::<Vec<_>>();
            assert_eq!(
                turn_injections.len(),
                1,
                "one prepared record must produce one turn-surface offer"
            );
            assert_eq!(turn_injections[0].record_id, "mem-sdk-routing");
            assert_eq!(turn_injections[0].identity, identity.as_str());
            assert_eq!(
                turn_injections[0].session_key.as_deref(),
                Some(response_session)
            );
            assert!(turn_injections[0].at_ms > 0);
        }

        runtime.shutdown().await;
        Ok(())
    }

    /// Regression: the member-state wire vocabulary is lowercase
    /// (`"active"`/`"retiring"`, matching the published SDK constants) on
    /// BOTH member-state surfaces — the roster member rows and the
    /// identity-first status RPC. The identity surface used to Debug-format
    /// the lifecycle state (`"Active"`), so consumers comparing across the
    /// two surfaces broke on casing.
    #[tokio::test]
    async fn member_state_wire_vocabulary_is_lowercase_on_both_surfaces()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-state-vocabulary-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(5))
                .build(),
        )
        .await?;
        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "worker-one".to_string(),
                None,
                None,
                None,
            ))
            .await?;

        // Surface 1: roster member rows.
        let raw = handle_unified_rpc_json(
            &runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "meta/get_member",
                "params": { "member_id": "worker-one" },
            })
            .to_string(),
            Duration::from_secs(5),
            None,
            None,
        )
        .await;
        let response: Value = serde_json::from_str(&raw)?;
        assert_eq!(
            response["result"]["state"],
            json!("active"),
            "member rows must speak the lowercase SDK vocabulary: {response:#?}"
        );

        // Surface 2: identity-first status RPC.
        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-state-vocabulary-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        });
        let identity = AgentIdentity::parse("review:singleton")?;
        register_live(
            &identity_rt,
            &core_bridge.core,
            DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            meerkat_core::types::SessionId::new(),
        )
        .await?;
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::new(identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };
        let raw = handle_unified_rpc_json(
            &runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "meta/status_identity",
                "params": { "identity": "review:singleton" },
            })
            .to_string(),
            Duration::from_secs(5),
            None,
            Some(&identity_ctx),
        )
        .await;
        let response: Value = serde_json::from_str(&raw)?;
        assert_eq!(
            response["result"]["state"],
            json!("active"),
            "identity status must speak the same lowercase vocabulary: {response:#?}"
        );

        Ok(())
    }

    /// Regression: `meta/send_message` precedence is resolved from a
    /// point-in-time roster probe. When a roster member whose id collides
    /// with a registered durable identity is transiently absent mid-
    /// reconcile (retire completes before the replacement spawn lands), the
    /// send must NOT silently fall through to the identity bridge and land
    /// in a different agent's conversation — membership declared in the
    /// reconcile baseline pins raw member-id semantics, surfacing the mob's
    /// own member-not-found error instead.
    #[tokio::test]
    async fn send_message_pins_baseline_member_over_identity_fallback()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-send-message-baseline-pin-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(5))
                .build(),
        )
        .await?;

        // Identity-first member backing the durable identity, plus a raw
        // roster member with the colliding bare id.
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:draco-base-001:0".to_string(),
                None,
                None,
                None,
            ),
        )
        .await?;
        runtime
            .spawn(
                SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    "draco-base-001".to_string(),
                    None,
                    None,
                    None,
                )
                // This regression needs roster membership, not an autonomous
                // kickoff. Keep the fixture quiescent so its deliberate retire
                // is the sole teardown owner and the test exercises baseline
                // dispatch precedence rather than kickoff/retire interleaving.
                .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven),
            )
            .await?;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-send-message-baseline-pin-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        });
        let identity = AgentIdentity::parse("draco-base-001")?;
        register_live(
            &identity_rt,
            &core_bridge.core,
            DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            meerkat_core::types::SessionId::new(),
        )
        .await?;
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::new(identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };

        // The raw roster member is part of the declared baseline ...
        runtime
            .mob_runtime()
            .set_baseline_member_specs(vec![SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                meerkat_mob::AgentIdentity::from("draco-base-001"),
            )])
            .await;
        // ... and is transiently absent (reconcile retired it; the
        // replacement spawn has not landed yet).
        runtime
            .mob_handle()
            .retire(meerkat_mob::AgentIdentity::from("draco-base-001"))
            .await?;

        let raw = handle_unified_rpc_json(
            &runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "meta/send_message",
                "params": { "member_id": "draco-base-001", "message": "mid-reconcile send" },
            })
            .to_string(),
            Duration::from_secs(10),
            None,
            Some(&identity_ctx),
        )
        .await;
        let response: Value = serde_json::from_str(&raw)?;
        assert_eq!(
            response["error"]["code"],
            json!(-32000),
            "transiently-absent baseline member must keep raw member-id semantics \
             instead of silently delivering through the identity bridge: {response:#?}"
        );
        let message = response["error"]["message"]
            .as_str()
            .expect("error message");
        assert!(
            message.starts_with("send_message failed:"),
            "unexpected error message: {message}"
        );

        Ok(())
    }

    /// An explicitly supplied identity context is authoritative even when it
    /// is not attached to the unified runtime. Generated aliases must still
    /// be generation-pinned before cross-mob or helper-fork lower-plane work.
    #[tokio::test]
    async fn separate_identity_context_pins_generated_alias_for_cross_mob_and_fork()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Arc::new(
            Box::pin(
                UnifiedRuntime::builder()
                    .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                    .module_config(MetaConfig {
                        modules: Vec::new(),
                        discovery: DiscoverySpec {
                            namespace: "rpc-separate-identity-context-test".to_string(),
                            modules: Vec::new(),
                        },
                        pre_spawn: Vec::new(),
                    })
                    .timeout(Duration::from_secs(5))
                    .build(),
            )
            .await?,
        );
        let stale_alias = "rt:review:separate:0";
        let current_alias = "rt:review:separate:1";
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                stale_alias.to_string(),
                None,
                None,
                None,
            )
            .with_labels(BTreeMap::from([(
                "agent_identity".to_string(),
                "review:separate".to_string(),
            )])),
        )
        .await?;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_rt = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-separate-identity-context-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        }));
        let identity = AgentIdentity::parse("review:separate")?;
        let core_runtime_id = register_live(
            &identity_rt,
            &core_bridge.core,
            rpc_durable_spec(identity.as_str(), "worker"),
            1,
            meerkat_core::types::SessionId::new(),
        )
        .await?;
        assert_eq!(core_runtime_id.as_str(), current_alias);
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                identity_rt,
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };
        assert!(
            runtime.identity_runtime().is_none(),
            "the regression requires a context supplied only at dispatch"
        );

        for (method, params) in [
            (
                "meta/cross_mob/wire",
                json!({
                    "local_member_id": stale_alias,
                    "remote_member_id": "remote-worker",
                    "remote_mob_id": "remote-mob",
                }),
            ),
            (
                "meta/fork_helper",
                json!({
                    "source_member_id": stale_alias,
                    "agent_identity": "fork-probe",
                    "task": "prove stale source rejection",
                    "result_label": "stale-source-probe",
                    "max_text_bytes": 4096,
                }),
            ),
        ] {
            let response: Value = serde_json::from_str(
                &super::handle_unified_rpc_json_arc(
                    &runtime,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": method,
                        "method": method,
                        "params": params,
                    })
                    .to_string(),
                    Duration::from_secs(5),
                    None,
                    Some(&identity_ctx),
                )
                .await,
            )?;
            assert_eq!(response["error"]["code"], json!(-32000), "{response:#?}");
            assert!(
                response["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("stale runtime alias")),
                "{method} must reject through identity authority before raw fallthrough: {response:#?}"
            );
        }

        assert!(
            runtime
                .mob_handle()
                .list_members_including_retiring()
                .await
                .iter()
                .any(|member| {
                    crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str())
                        == stale_alias
                }),
            "the raw stale member remains present, proving rejection was not member-not-found"
        );
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test]
    async fn embedded_rpc_uses_runtime_owned_identity_context_when_argument_is_absent()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let mut runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-owned-identity-context-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(5))
                .build(),
        )
        .await?;
        let stale_alias = "rt:review:owned-context:0";
        let current_alias = "rt:review:owned-context:1";
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                stale_alias.to_string(),
                None,
                None,
                None,
            )
            .with_labels(BTreeMap::from([(
                "agent_identity".to_string(),
                "review:owned-context".to_string(),
            )])),
        )
        .await?;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-owned-identity-context-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        }));
        let identity = AgentIdentity::parse("review:owned-context")?;
        let core_runtime_id = register_live(
            &identity_runtime,
            &core_bridge.core,
            rpc_durable_spec(identity.as_str(), "worker"),
            1,
            meerkat_core::types::SessionId::new(),
        )
        .await?;
        assert_eq!(core_runtime_id.as_str(), current_alias);
        runtime.attach_identity_first_context(Arc::new(
            crate::identity_first::IdentityFirstRuntimeContext::new(
                identity_runtime,
                Arc::new(EmptyRosterProvider),
                None,
                None,
                None,
            ),
        ));
        let runtime = Arc::new(runtime);

        let list: Value = serde_json::from_str(
            &super::handle_unified_rpc_json_arc(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/list_members",
                    "params": {},
                })
                .to_string(),
                Duration::from_secs(5),
                None,
                None,
            )
            .await,
        )?;
        assert_eq!(list["result"], json!([]), "{list:#?}");

        let get: Value = serde_json::from_str(
            &super::handle_unified_rpc_json_arc(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "meta/get_member",
                    "params": {"member_id": stale_alias},
                })
                .to_string(),
                Duration::from_secs(5),
                None,
                None,
            )
            .await,
        )?;
        assert_eq!(
            get["error"]["data"]["kind"],
            json!("stale_identity_runtime_binding"),
            "{get:#?}"
        );

        // #366: resolved tools decode a supplied runtime alias to its durable
        // member, so the stale-alias refusal must still come first here too.
        let resolved_tools: Value = serde_json::from_str(
            &super::handle_unified_rpc_json_arc(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "meta/identity/resolved_tools",
                    "params": {"identity": stale_alias},
                })
                .to_string(),
                Duration::from_secs(5),
                None,
                None,
            )
            .await,
        )?;
        assert_eq!(
            resolved_tools["error"]["data"]["kind"],
            json!("stale_identity_runtime_binding"),
            "{resolved_tools:#?}"
        );

        // The member-declaration read maps a current runtime alias to the
        // durable row, so it refuses a stale one first, as `meta/get_member`
        // does.
        let declaration: Value = serde_json::from_str(
            &super::handle_unified_rpc_json_arc(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": crate::rpc::mob_methods::MEMBER_TOOL_DECLARATION,
                    "params": {
                        "mob_id": runtime.mob_handle().mob_id().to_string(),
                        "agent_identity": stale_alias,
                    },
                })
                .to_string(),
                Duration::from_secs(5),
                None,
                None,
            )
            .await,
        )?;
        assert_eq!(
            declaration["error"]["data"]["kind"],
            json!("stale_identity_runtime_binding"),
            "{declaration:#?}"
        );

        runtime.shutdown().await;
        Ok(())
    }

    /// The stdio `meta/member_status` decodes a supplied id the way
    /// `meta/get_member` and the console's `meta/member_status` do: the
    /// current runtime alias reads the member its durable identity names.
    /// Before, it read a member that does not exist and answered a snapshot
    /// with no session.
    #[tokio::test]
    async fn stdio_member_status_reads_every_spelling_of_one_member()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const DURABLE: &str = "review:status-alias";
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-member-status-alias-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(5))
                .build(),
        )
        .await?;
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire("worker".to_string(), DURABLE.to_string(), None, None, None),
        )
        .await?;
        let runtime = Arc::new(runtime);
        let session_of =
            async |member_id: &str| -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
                let response: Value = serde_json::from_str(
                    &super::handle_unified_rpc_json_arc(
                        &runtime,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "meta/member_status",
                            "params": {"member_id": member_id},
                        })
                        .to_string(),
                        Duration::from_secs(5),
                        None,
                        None,
                    )
                    .await,
                )?;
                Ok(response["result"]["current_session_id"].clone())
            };

        let durable = session_of(DURABLE).await?;
        assert!(
            durable.is_string(),
            "the durable identity reads its member: {durable}"
        );
        let alias = format!("rt:{DURABLE}:0");
        assert_eq!(
            session_of(&alias).await?,
            durable,
            "{alias} must read the member {DURABLE} names"
        );

        runtime.shutdown().await;
        Ok(())
    }

    /// `meta/fork_helper` with the CURRENT runtime alias as its source forks
    /// the member the identity authority owns. The lifecycle target accepts
    /// the alias; the fork then has to name the durable roster row, not the
    /// alias's own encoding, which no row has ("mob member not found").
    #[tokio::test]
    async fn fork_helper_forks_the_source_its_current_runtime_alias_names()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const DURABLE: &str = "review:fork-current";
        let current_alias = "rt:review:fork-current:1";
        let temp_dir = tempfile::tempdir()?;
        let runtime = Arc::new(
            Box::pin(
                UnifiedRuntime::builder()
                    .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                    .module_config(MetaConfig {
                        modules: Vec::new(),
                        discovery: DiscoverySpec {
                            namespace: "rpc-fork-current-alias-test".to_string(),
                            modules: Vec::new(),
                        },
                        pre_spawn: Vec::new(),
                    })
                    .timeout(Duration::from_secs(5))
                    .build(),
            )
            .await?,
        );
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire("worker".to_string(), DURABLE.to_string(), None, None, None),
        )
        .await?;
        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_rt = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-fork-current-alias-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        }));
        let identity = AgentIdentity::parse(DURABLE)?;
        register_live(
            &identity_rt,
            &core_bridge.core,
            rpc_durable_spec(identity.as_str(), "worker"),
            1,
            meerkat_core::types::SessionId::new(),
        )
        .await?;
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                identity_rt,
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };

        let response: Value = serde_json::from_str(
            &super::handle_unified_rpc_json_arc(
                &runtime,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/fork_helper",
                    "params": {
                        "source_member_id": current_alias,
                        "agent_identity": "fork-current-probe",
                        "task": "answer briefly",
                        "result_label": "fork-current-probe",
                        "max_text_bytes": 4096,
                    },
                })
                .to_string(),
                Duration::from_secs(5),
                None,
                Some(&identity_ctx),
            )
            .await,
        )?;
        assert!(
            !response.to_string().contains("mob member not found"),
            "a current runtime alias must fork its durable source member: {response:#?}"
        );

        runtime.shutdown().await;
        Ok(())
    }

    /// The member-declaration family follows the alias rule. The read takes a
    /// runtime alias to the durable row, like `meta/member_status`. The two
    /// writes have no identity authority to pin a generation, so they refuse
    /// a generated alias instead of collapsing it onto the durable row (or
    /// missing, as they did: the read answered "no durable identity intent"
    /// and apply reported a commit for an absent member).
    #[tokio::test]
    async fn member_declarations_read_a_runtime_alias_and_refuse_it_for_writes()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const DURABLE: &str = "review:declarations";
        let alias = "rt:review:declarations:0";
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_store_backed_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-declaration-alias-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(5))
                .build(),
        )
        .await?;
        let mut spec = SpawnMemberSpec::from_wire(
            "worker".to_string(),
            DURABLE.to_string(),
            Some("Declaration alias fixture.".into()),
            None,
            None,
        );
        spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
        spawn_identity_projection_fixture(&runtime, spec).await?;
        let runtime = Arc::new(runtime);
        let handle = runtime.mob_handle();
        let mob_id = handle.mob_id().to_string();
        let session_id = handle
            .resolve_bridge_session_id(&crate::member_comms_id::mob_member_id(DURABLE))
            .await
            .ok_or("the spawned member must have a bridge session")?;
        let call = async |method: &str,
                          params: Value|
               -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
            Ok(serde_json::from_str(
                &super::handle_unified_rpc_json_arc(
                    &runtime,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": method,
                        "method": method,
                        "params": params,
                    })
                    .to_string(),
                    Duration::from_secs(5),
                    None,
                    None,
                )
                .await,
            )?)
        };
        let adoption = |agent_identity: &str, request_id: &str| {
            json!({
                "mob_id": mob_id,
                "agent_identity": agent_identity,
                "request_id": request_id,
                "precondition": "expected_absent",
                "declaration_scope": "rpc-declaration-alias-test",
                "declaration_revision": 1,
                "session": {
                    "session_id": session_id.to_string(),
                    "lineage_id": format!("session:{session_id}"),
                    "lineage_generation": 0,
                    "authority_policy": "require_existing"
                },
                "member": {
                    "profile_name": "worker",
                    "runtime_mode": "turn_driven",
                    "execution": { "execution": "controlling_session" }
                },
                "owned_wiring": [],
                "convergence": { "kind": "drain", "max_wait_ms": 5000 }
            })
        };
        let refused = |response: &Value| {
            response["error"]["code"] == json!(-32602)
                && response["error"]["message"]
                    .as_str()
                    .is_some_and(|message| {
                        message.contains("generated member alias requires identity authority")
                    })
        };

        // Each of the three outcomes is recorded first and asserted together,
        // so one run shows all three: the adoption and the apply through the
        // alias, and the read through the alias against the durable read.
        let refused_adoption = call(
            crate::rpc::mob_methods::ADOPT_MEMBER_IDENTITY_DECLARATION,
            adoption(alias, "adopt-alias-1"),
        )
        .await?;
        let adopted = call(
            crate::rpc::mob_methods::ADOPT_MEMBER_IDENTITY_DECLARATION,
            adoption(DURABLE, "adopt-durable-1"),
        )
        .await?;
        assert_eq!(adopted["error"], Value::Null, "{adopted:#?}");

        let direct = call(
            crate::rpc::mob_methods::MEMBER_TOOL_DECLARATION,
            json!({ "mob_id": mob_id, "agent_identity": DURABLE }),
        )
        .await?;
        assert_eq!(
            direct["result"]["desired_intent_revision"],
            json!(1),
            "{direct:#?}"
        );
        let aliased = call(
            crate::rpc::mob_methods::MEMBER_TOOL_DECLARATION,
            json!({ "mob_id": mob_id, "agent_identity": alias }),
        )
        .await?;
        let refused_apply = call(
            crate::rpc::mob_methods::APPLY_MEMBER_TOOL_DECLARATION,
            json!({
                "mob_id": mob_id,
                "agent_identity": alias,
                "request_id": "apply-alias-1",
                "expected_intent_revision": 1,
                "declaration": direct["result"]["declaration"].clone(),
                "convergence": { "kind": "drain", "max_wait_ms": 5000 }
            }),
        )
        .await?;
        let read_matches = (
            &aliased["result"]["desired_intent_revision"],
            &aliased["result"]["declaration"],
        ) == (
            &direct["result"]["desired_intent_revision"],
            &direct["result"]["declaration"],
        );
        assert_eq!(
            (
                refused(&refused_adoption),
                read_matches,
                refused(&refused_apply)
            ),
            (true, true, true),
            "(adoption refused, alias read matches, apply refused) through {alias}:\n  \
             adoption: {refused_adoption:#?}\n  read: {aliased:#?}\n  apply: {refused_apply:#?}"
        );

        runtime.shutdown().await;
        Ok(())
    }

    async fn alias_rule_test_runtime(
        temp_dir: &tempfile::TempDir,
        namespace: &str,
    ) -> Result<UnifiedRuntime, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: namespace.to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(5))
                .build(),
        )
        .await?)
    }

    /// `UnifiedRuntime::start_member_turn` follows the alias rule for a direct
    /// action without an identity gate: an encoded roster id is decoded, not
    /// encoded twice, and a generated runtime alias is refused instead of
    /// being collapsed onto the durable row.
    #[tokio::test]
    async fn start_member_turn_decodes_roster_ids_and_refuses_generated_aliases()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const DURABLE: &str = "review:turn-alias";
        let temp_dir = tempfile::tempdir()?;
        let runtime = alias_rule_test_runtime(&temp_dir, "rpc-start-turn-alias-test").await?;
        // Turn-driven: an autonomous host member takes no completion-bearing
        // turn whatever its id is spelled.
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                DURABLE.to_string(),
                None,
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
            ),
        )
        .await?;
        let start = async |member: &str| {
            runtime
                .start_member_turn(
                    member,
                    meerkat_core::types::ContentInput::from("hello"),
                    meerkat_core::types::HandlingMode::Queue,
                    meerkat_mob::MemberTurnOptions::default(),
                    None,
                )
                .await
                .map(|admission| admission.session_id)
                .map_err(|error| error.to_string())
        };

        let durable = start(DURABLE).await;
        assert!(
            durable.is_ok(),
            "the durable identity starts a turn: {durable:?}"
        );
        let encoded = crate::member_comms_id::mob_member_id_str(DURABLE).into_owned();
        let encoded_turn = start(&encoded).await;
        let alias_turn = start(&format!("rt:{DURABLE}:0")).await;
        assert_eq!(
            (encoded_turn, alias_turn),
            (
                durable,
                Err(crate::mob_handle_runtime::MobRuntimeError::InvalidInput(
                    "generated member alias requires identity authority"
                )
                .to_string())
            ),
            "(encoded roster id, generated alias)"
        );

        runtime.shutdown().await;
        Ok(())
    }

    /// `UnifiedRuntime::reconcile` decodes an encoded roster id instead of
    /// seating a twice-encoded phantom member, and refuses a generated runtime
    /// alias, which only the identity authority can own, as spawn does.
    #[tokio::test]
    async fn reconcile_decodes_roster_ids_and_refuses_generated_aliases()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const DURABLE: &str = "review:reconcile-alias";
        let temp_dir = tempfile::tempdir()?;
        let runtime = alias_rule_test_runtime(&temp_dir, "rpc-reconcile-alias-test").await?;
        let worker = |identity: &str| {
            SpawnMemberSpec::from_wire("worker".to_string(), identity.to_string(), None, None, None)
        };

        let refused = runtime
            .reconcile(vec![worker(&format!("rt:{DURABLE}:0"))])
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
        let encoded = crate::member_comms_id::mob_member_id_str(DURABLE).into_owned();
        let seated = runtime
            .reconcile(vec![worker(&encoded)])
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
        let rows = runtime
            .mob_handle()
            .list_members_including_retiring()
            .await
            .into_iter()
            .map(|row| row.agent_identity.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            (
                refused
                    .as_ref()
                    .is_err_and(|error| error
                        .contains("generated member alias requires identity authority")),
                seated,
                rows
            ),
            (true, Ok(()), vec![encoded.clone()]),
            "(generated alias refused, encoded id reconciled, roster rows); refusal: {refused:?}"
        );

        runtime.shutdown().await;
        Ok(())
    }

    /// `UnifiedRuntime::fork_member_with_identity_runtime` follows the alias
    /// rule for its source: a CURRENT runtime alias the identity authority owns
    /// forks the durable roster row it names, and without an identity runtime
    /// a generated source alias is refused instead of being encoded as a row
    /// nobody has.
    #[tokio::test]
    async fn fork_member_resolves_a_current_source_alias_and_refuses_an_unowned_one()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const DURABLE: &str = "review:fork-api";
        let current_alias = "rt:review:fork-api:1";
        let temp_dir = tempfile::tempdir()?;
        let runtime = alias_rule_test_runtime(&temp_dir, "rpc-fork-api-alias-test").await?;
        // Turn-driven, so the source session is idle: a fork refuses a source
        // whose session is running.
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                DURABLE.to_string(),
                None,
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
            ),
        )
        .await?;
        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_rt = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-fork-api-alias-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        }));
        let identity = AgentIdentity::parse(DURABLE)?;
        register_live(
            &identity_rt,
            &core_bridge.core,
            rpc_durable_spec(identity.as_str(), "worker"),
            1,
            meerkat_core::types::SessionId::new(),
        )
        .await?;
        let child = |identity: &str| {
            SpawnMemberSpec::from_wire("worker".to_string(), identity.to_string(), None, None, None)
        };

        let owned = runtime
            .fork_member_with_identity_runtime(
                Some(&identity_rt),
                current_alias,
                child("fork-api-child"),
                None,
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
        let unowned = runtime
            .fork_member_with_identity_runtime(
                None,
                "rt:review:fork-api-unowned:0",
                child("fork-api-orphan"),
                None,
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
        // The test session service is ephemeral, so a fork that reached its
        // source member stops at the session service's durable transcript
        // fork authority. That refusal is the evidence the source resolved:
        // without the alias rule the fork stopped earlier, at "source member
        // has no session" for the row keyed by the alias's own encoding.
        assert_eq!(
            (
                owned
                    .as_ref()
                    .is_err_and(|error| error.contains("durable transcript fork authority")),
                unowned
                    .as_ref()
                    .is_err_and(|error| error.contains("requires identity authority")),
            ),
            (true, true),
            "(fork through the current alias reached its source, unowned alias refused): \
             owned {owned:?}, unowned {unowned:?}"
        );

        runtime.shutdown().await;
        Ok(())
    }

    /// The same-process topology primitives follow the member-id alias rule.
    /// Identity-first topology names each end by its durable identity; the
    /// primitives pin the identity's current runtime alias under its
    /// lifecycle lock and act on the durable roster row that alias names.
    /// Before, they handed the runtime alias itself to the mob, which no row
    /// has, so wiring an identity-first pair failed with MemberNotFound.
    #[tokio::test]
    async fn same_process_topology_reaches_identity_first_members_by_their_durable_rows()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        async fn identity_first_runtime(
            temp_dir: &tempfile::TempDir,
            namespace: &str,
            durable: &str,
        ) -> Result<UnifiedRuntime, Box<dyn std::error::Error + Send + Sync>> {
            let mut runtime = alias_rule_test_runtime(temp_dir, namespace).await?;
            spawn_identity_projection_fixture(
                &runtime,
                SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    durable.to_string(),
                    None,
                    Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                    None,
                )
                .with_labels(BTreeMap::from([(
                    "agent_identity".to_string(),
                    durable.to_string(),
                )])),
            )
            .await?;
            let identity_rt = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: namespace.to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(crate::identity_first::MobSessionBridge::new(
                    runtime.mob_handle(),
                ))),
                default_timeout: None,
            }));
            let identity = AgentIdentity::parse(durable)?;
            identity_rt
                .register(rpc_durable_spec(identity.as_str(), "worker"))
                .await;
            runtime.attach_identity_first_context(Arc::new(
                crate::identity_first::IdentityFirstRuntimeContext::new(
                    identity_rt,
                    Arc::new(EmptyRosterProvider),
                    None,
                    None,
                    None,
                ),
            ));
            Ok(runtime)
        }

        let left_dir = tempfile::tempdir()?;
        let right_dir = tempfile::tempdir()?;
        let left = identity_first_runtime(&left_dir, "rpc-topology-left", "review:left").await?;
        let right =
            identity_first_runtime(&right_dir, "rpc-topology-right", "review:right").await?;

        let wired = left
            .wire_bilateral_same_process(&right, "review:left", "review:right")
            .await
            .map_err(|error| error.to_string());
        let after_wire = left
            .bilateral_same_process_state(&right, "review:left", "review:right")
            .await
            .map_err(|error| error.to_string());
        let unwired = left
            .unwire_bilateral_same_process(&right, "review:left", "review:right")
            .await
            .map_err(|error| error.to_string());
        let after_unwire = left
            .bilateral_same_process_state(&right, "review:left", "review:right")
            .await
            .map_err(|error| error.to_string());
        assert_eq!(
            (wired, after_wire, unwired, after_unwire),
            (Ok(()), Ok((true, true)), Ok(()), Ok((false, false))),
            "(wire, state after wire, unwire, state after unwire)"
        );

        left.shutdown().await;
        right.shutdown().await;
        Ok(())
    }

    /// Capturing durable ownership is a one-way authority decision for a
    /// request. If the identity disappears before the later status probe,
    /// the live projection must not resurrect it through compatibility mode.
    #[tokio::test]
    async fn durable_ownership_observed_before_unknown_identity_never_live_fallbacks()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-durable-delete-race-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(1))
                .build(),
        )
        .await?;
        let durable_identity = "review:delete-race";
        let runtime_alias = "rt:review:delete-race:0";
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                runtime_alias.to_string(),
                None,
                None,
                None,
            )
            .with_labels(BTreeMap::from([(
                "agent_identity".to_string(),
                durable_identity.to_string(),
            )])),
        )
        .await?;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-durable-delete-race-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        });
        let identity = AgentIdentity::parse(durable_identity)?;
        register_live(
            &identity_rt,
            &core_bridge.core,
            rpc_durable_spec(durable_identity, "worker"),
            0,
            meerkat_core::types::SessionId::new(),
        )
        .await?;

        let target =
            resolve_rpc_identity_control_target(&runtime, &identity_rt, durable_identity).await?;
        assert!(
            target.was_registered,
            "resolution must capture durable ownership"
        );
        assert!(target.live.is_some(), "the race requires a live projection");

        identity_rt
            .remove_unrecorded_for_test(&identity)
            .await
            .expect("simulate a concurrent durable delete after resolution");
        assert!(matches!(
            identity_rt.status(&identity).await,
            Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_))
        ));
        assert!(
            !super::rpc_live_only_fallback_allowed(&target, durable_identity),
            "a later UnknownIdentity must preserve the observed durable delete"
        );

        let mut live_only = target.clone();
        live_only.was_registered = false;
        assert!(
            !super::rpc_live_only_fallback_allowed(&live_only, durable_identity),
            "a resolved generated alias must remain identity-owned even when registration vanished"
        );
        assert!(
            !super::rpc_live_only_fallback_allowed(&live_only, runtime_alias),
            "generated aliases never enter raw/live-only fallback"
        );
        live_only
            .live
            .as_mut()
            .expect("live projection")
            .runtime_member_id = "legacy-review-member".to_string();
        assert!(
            super::rpc_live_only_fallback_allowed(&live_only, durable_identity),
            "genuine bare live-only identities retain compatibility fallback"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// `meta/member_health` answers from identity-plane reads and
    /// `meta/reload_member` reloads NON-destructively: same session id, same
    /// generation, never the destructive reset `respawn_member` performs.
    #[tokio::test]
    async fn reload_member_and_member_health_rpcs_use_identity_authority()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-reload-member-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(5))
                .build(),
        )
        .await?;
        let runtime_alias = "rt:review:reload:3";
        let continuity_store = Arc::new(LocalContinuityStore::in_memory()?);
        // Core reloads the member's registration in place (`discarded`): the
        // member keeps its session and its generation.
        let core_bridge = Arc::new(CoreReadsBridge::with_registration_reload(
            crate::identity_first::MemberReloadDisposition::Discarded,
        ));
        let identity_rt = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store,
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-reload-member-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        }));
        let identity = AgentIdentity::parse("review:reload")?;
        let session_id = meerkat_core::types::SessionId::new();
        register_live(
            &identity_rt,
            &core_bridge.core,
            DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            3,
            session_id.clone(),
        )
        .await?;
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::clone(&identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };
        let call = |id: u64, method: &'static str, params: Value| {
            let runtime = &runtime;
            let identity_ctx = &identity_ctx;
            async move {
                let raw = handle_unified_rpc_json(
                    runtime,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": method,
                        "params": params,
                    })
                    .to_string(),
                    Duration::from_secs(5),
                    None,
                    Some(identity_ctx),
                )
                .await;
                serde_json::from_str::<Value>(&raw)
            }
        };

        let health = call(
            1,
            "meta/member_health",
            json!({ "member_id": runtime_alias }),
        )
        .await?;
        assert!(
            health["error"].is_null(),
            "member_health failed: {health:#?}"
        );
        assert_eq!(health["result"]["identity"], json!("review:reload"));
        assert_eq!(health["result"]["state"], json!("active"));
        assert_eq!(health["result"]["generation"], json!(3));
        assert_eq!(health["result"]["materialization_in_flight"], json!(false));
        assert!(
            health["result"]["actor_loop"]["state"].is_string(),
            "the probe verdict must be present: {health:#?}"
        );
        assert!(
            health["result"].get("durability").is_none(),
            "durability is absent until meerkat exposes it: {health:#?}"
        );

        let reload = call(
            2,
            "meta/reload_member",
            json!({ "member_id": runtime_alias }),
        )
        .await?;
        assert!(
            reload["error"].is_null(),
            "reload_member failed: {reload:#?}"
        );
        assert_eq!(reload["result"]["identity_first"], json!(true));
        assert_eq!(reload["result"]["reloaded"], json!(true));
        assert_eq!(reload["result"]["disposition"], json!("discarded"));
        assert_eq!(
            reload["result"]["session_id"],
            json!(session_id.to_string()),
            "a reload binds the SAME durable session"
        );
        assert_eq!(
            reload["result"]["generation"],
            json!(3),
            "a reload never advances the continuity generation (respawn_member does)"
        );
        let status = identity_rt.status(&identity).await?;
        assert_eq!(status.state, IdentityLifecycleState::Active);
        assert_eq!(status.session_id, Some(session_id));
        assert_eq!(status.generation, Some(meerkat_mob::Generation::new(3)));

        // A stale generated alias is rejected, exactly like the other member
        // verbs.
        let stale = call(
            3,
            "meta/reload_member",
            json!({ "member_id": "rt:review:reload:9" }),
        )
        .await?;
        assert_eq!(
            stale["error"]["data"]["kind"],
            json!("stale_identity_runtime_binding"),
            "{stale:#?}"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// A current identity-owned runtime alias must resolve through the durable
    /// identity authority. Raw mob retirement would leave continuity, lease,
    /// and lifecycle state stale even though the projected member disappeared.
    #[tokio::test]
    async fn retire_member_current_runtime_alias_uses_identity_authority()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-owned-retire-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(5))
                .build(),
        )
        .await?;
        let runtime_alias = "rt:review:owned:0";
        spawn_identity_projection_fixture(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                runtime_alias.to_string(),
                None,
                None,
                None,
            ),
        )
        .await?;

        let continuity_store = Arc::new(LocalContinuityStore::in_memory()?);
        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_rt = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store,
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "rpc-owned-retire-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge.clone()),
            default_timeout: None,
        }));
        let identity = AgentIdentity::parse("review:owned")?;
        register_live(
            &identity_rt,
            &core_bridge.core,
            DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            meerkat_core::types::SessionId::new(),
        )
        .await?;
        let identity_ctx = IdentityFirstContext {
            runtime: crate::identity_first::PublicIdentityRuntime::new(
                Arc::clone(&identity_rt),
                runtime.mob_runtime().clone(),
            ),
            roster_provider: Arc::new(EmptyRosterProvider),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };

        let raw = handle_unified_rpc_json(
            &runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "meta/retire_member",
                "params": { "member_id": runtime_alias },
            })
            .to_string(),
            Duration::from_secs(5),
            None,
            Some(&identity_ctx),
        )
        .await;
        let response: Value = serde_json::from_str(&raw)?;
        assert!(
            response["error"].is_null(),
            "identity-owned retire_member failed: {response:#?}"
        );
        assert_eq!(response["result"]["identity_first"], json!(true));
        assert_eq!(
            identity_rt.status(&identity).await?.state,
            IdentityLifecycleState::Dormant,
            "the durable authority, not only the Mob projection, must observe retirement"
        );
        assert!(
            runtime
                .mob_handle()
                .list_members_including_retiring()
                .await
                .iter()
                .any(|entry| {
                    crate::member_comms_id::runtime_alias_str(entry.agent_identity.as_str())
                        == runtime_alias
                }),
            "the test bridge retires only in Core's history, so the row proves the raw Mob \
             fallback was not invoked"
        );

        Ok(())
    }

    /// Regression (meerkat 0.7.1 migration): an idle member's session
    /// machine sits in `Stopped`, where the archive step's final `Retire`
    /// transition is guard-rejected ("disposal completed but ArchiveSession
    /// failed: … guard rejected transition from Stopped for input::Retire").
    /// `meta/retire_member` and `meta/respawn_member` must treat that
    /// bookkeeping failure as completed cleanup instead of surfacing -32000,
    /// and a recovered respawn must leave an active replacement — never a
    /// member wedged in `retiring` with its session disposed.
    #[tokio::test]
    async fn retire_and_respawn_rpcs_succeed_for_idle_member()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .mob_spec(rpc_test_mob_spec(&temp_dir)?)
                .module_config(MetaConfig {
                    modules: Vec::new(),
                    discovery: DiscoverySpec {
                        namespace: "rpc-idle-lifecycle-test".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                })
                .timeout(Duration::from_secs(5))
                .build(),
        )
        .await?;

        for member in ["worker-one", "worker-two"] {
            runtime
                .spawn(SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    member.to_string(),
                    None,
                    None,
                    None,
                ))
                .await?;
        }

        let send = |id: u64, method: &'static str, params: Value| {
            let runtime = &runtime;
            async move {
                let raw = handle_unified_rpc_json(
                    runtime,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": method,
                        "params": params,
                    })
                    .to_string(),
                    Duration::from_secs(10),
                    None,
                    None,
                )
                .await;
                serde_json::from_str::<Value>(&raw)
            }
        };

        // Retire an idle member: must report accepted and remove the member.
        let response = send(1, "meta/retire_member", json!({"member_id": "worker-one"})).await?;
        assert!(
            response["error"].is_null(),
            "retire_member must succeed for an idle member: {response:#?}"
        );
        assert_eq!(response["result"]["accepted"], json!(true));
        assert!(
            !runtime
                .mob_handle()
                .list_members_including_retiring()
                .await
                .iter()
                .any(|entry| entry.agent_identity.as_str() == "worker-one"),
            "retired member must leave the roster"
        );

        // Respawn an idle member: must report accepted and leave an active
        // (not retiring) replacement in the roster.
        let response = send(2, "meta/respawn_member", json!({"member_id": "worker-two"})).await?;
        assert!(
            response["error"].is_null(),
            "respawn_member must succeed for an idle member: {response:#?}"
        );
        assert_eq!(response["result"]["accepted"], json!(true));
        let members = runtime.mob_handle().list_members_including_retiring().await;
        let worker_two = members
            .iter()
            .find(|entry| entry.agent_identity.as_str() == "worker-two")
            .expect("respawned member must remain in the roster");
        assert_eq!(
            worker_two.status,
            meerkat_mob::MobMemberStatus::Active,
            "respawned member must be active, not wedged in retiring"
        );

        Ok(())
    }
}
