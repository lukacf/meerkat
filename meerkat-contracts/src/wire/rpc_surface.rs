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

/// Parameters for `schedule/call`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScheduleToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}
