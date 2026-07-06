//! RPC-surface request/response wire contracts.
//!
//! These are the canonical, schema-emitted shapes for JSON-RPC methods whose
//! request/response types were previously declared handler-local in
//! `meerkat-rpc` and therefore never landed in `params.json` / `wire-types.json`
//! (Generated-artifact-theater: the catalog named types with no emitted schema).
//! The handlers now deserialize/serialize these contracts types directly, so
//! the `rpc_catalog` type names resolve and SDK codegen can generate them.
//!
//! Rich session-construction params (`session/create`, `turn/start`) remain
//! handler-local because they embed deep `meerkat-core` domain graphs
//! (`ContentInput`, `ToolDef`, `PeerMeta`, `BudgetLimits`, hook/skill overlays)
//! that do not yet derive `JsonSchema`; they are tracked on the explicit
//! allowlist in `emitted_rpc_catalog_type_names_resolve_to_schema_artifacts`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wire::WireSessionSummary;

/// Parameters for `session/list` (all optional).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListSessionsParams {
    /// Filter sessions by label key-value pairs (AND match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    /// Maximum number of sessions to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Number of sessions to skip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

/// Result for `session/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListSessionsResult {
    pub sessions: Vec<WireSessionSummary>,
}

/// Parameters for `session/read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReadSessionParams {
    pub session_id: String,
}

/// Parameters for `session/history`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReadSessionHistoryParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Parameters for `session/archive`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ArchiveSessionParams {
    pub session_id: String,
}

/// Parameters for `session/inject_context`.
///
/// The injected body is the typed [`CoreRenderable`] owner rather than a bare
/// `text` string: surfaces parse their inbound payload into the renderable at
/// the ingress boundary and the handler threads it straight through to
/// `AppendSystemContextRequest.content`. A plain-text client payload still
/// deserializes via `CoreRenderable`'s tagged `text` variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InjectSystemContextParams {
    pub session_id: String,
    pub content: meerkat_core::lifecycle::run_primitive::CoreRenderable,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Typed selector for `session/input_status`: exactly one lookup key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "by", content = "value")]
pub enum SessionInputStateSelector {
    /// Look up by the canonical runtime input id.
    InputId(String),
    /// Look up by the caller-supplied idempotency key (durable
    /// reconciliation: "did the interaction I submitted under this key reach
    /// a terminal state, and which?").
    IdempotencyKey(String),
}

/// Parameters for `session/input_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionInputStateParams {
    pub session_id: String,
    /// Exactly-one lookup key, carried as a tagged object (NOT flattened:
    /// the SDK codegen path drops flattened tagged enums from generated
    /// param types, which would leave raw SDK callers unable to express the
    /// required selector).
    pub selector: SessionInputStateSelector,
}

/// Result for `session/input_status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionInputStateResult {
    /// `None` when no input matches the selector (unknown id or key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<super::runtime::WireInputState>,
}

/// Result for `session/inject_context`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InjectSystemContextResult {
    pub status: meerkat_core::AppendSystemContextStatus,
}

/// Result for deferred `session/create` (no turn executed).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeferredCreateResult {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
}

/// Parameters for `turn/interrupt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InterruptParams {
    pub session_id: String,
}

/// Parameters for `blob/get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BlobGetParams {
    pub blob_id: String,
}

/// Result for `schedule/tools`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScheduleToolsResult {
    pub tools: Vec<Value>,
}

/// Public callback tool definition accepted by `tools/register`.
///
/// The server stamps callback provenance itself; clients provide only the
/// model-facing definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CallbackToolDefinition {
    pub name: meerkat_core::ToolName,
    pub description: String,
    #[cfg_attr(feature = "schema", schemars(with = "Value"))]
    pub input_schema: Value,
}

impl From<CallbackToolDefinition> for meerkat_core::ToolDef {
    fn from(value: CallbackToolDefinition) -> Self {
        Self {
            name: value.name,
            description: value.description,
            input_schema: value.input_schema,
            provenance: None,
        }
    }
}

/// Parameters for `tools/register`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolsRegisterParams {
    pub tools: Vec<CallbackToolDefinition>,
}

/// Result for `tools/register`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolsRegisterResult {
    pub registered: usize,
}

/// Parameters for `schedule/call`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScheduleToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// Basic server identity advertised by the RPC `initialize` handshake.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// Capabilities returned by the RPC server during `initialize`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServerCapabilities {
    pub server_info: ServerInfo,
    pub contract_version: String,
    pub methods: Vec<String>,
}
