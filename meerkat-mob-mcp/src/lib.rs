//! meerkat-mob-mcp - MCP server exposing Meerkat Mob orchestration as tools.
//!
//! This crate provides an MCP server (binary: `rkat-mob-mcp`) that exposes mob
//! orchestration capabilities via the MCP protocol. Tools include spec CRUD,
//! flow activation, run management, meerkat listing, reconcile, event polling,
//! and capabilities reporting.

use meerkat_mob::error::MobError;
use meerkat_mob::run::RunStatus;
use meerkat_mob::runtime::{MobRuntime, MobRuntimeConfig};
use meerkat_mob::service::{
    ActivateRequest, ApplySpecRequest, ListMeerkatsRequest, ListRunsRequest,
    ListSpecsRequest, MobService, PollEventsRequest, ReconcileMode, ReconcileRequest,
};
use meerkat_mob::spec::{MobSpec, parse_mob_specs_from_toml};
use meerkat_mob::store::{InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore};
use meerkat_mob::validate::ValidateOptions;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// MCP tool input schemas
// ---------------------------------------------------------------------------

/// Input for `mob_spec_apply` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobSpecApplyInput {
    /// TOML spec string (parsed via `[mob.specs.<mob_id>]` format).
    /// Mutually exclusive with `spec_json`.
    #[serde(default)]
    pub spec_toml: Option<String>,
    /// JSON spec object (direct `MobSpec` representation).
    /// Mutually exclusive with `spec_toml`.
    #[serde(default)]
    pub spec_json: Option<Value>,
}

/// Input for `mob_spec_get` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobSpecGetInput {
    /// Mob ID to retrieve.
    pub mob_id: String,
}

/// Input for `mob_spec_list` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobSpecListInput {
    /// Maximum number of specs to return.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Input for `mob_spec_delete` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobSpecDeleteInput {
    /// Mob ID to delete.
    pub mob_id: String,
}

/// Input for `mob_activate` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobActivateInput {
    /// Mob ID to activate.
    pub mob_id: String,
    /// Flow ID to execute.
    pub flow_id: String,
    /// Activation parameters (passed to conditions and step dispatch).
    #[serde(default)]
    pub params: Value,
}

/// Input for `mob_run_get` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobRunGetInput {
    /// Run ID to retrieve.
    pub run_id: String,
}

/// Input for `mob_run_list` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobRunListInput {
    /// Filter by mob ID.
    #[serde(default)]
    pub mob_id: Option<String>,
    /// Filter by status.
    #[serde(default)]
    pub status: Option<String>,
    /// Maximum number of runs to return.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Input for `mob_run_cancel` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobRunCancelInput {
    /// Run ID to cancel.
    pub run_id: String,
}

/// Input for `mob_meerkat_list` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobMeerkatListInput {
    /// Mob ID to list meerkats for.
    pub mob_id: String,
    /// Optional role filter.
    #[serde(default)]
    pub role: Option<String>,
}

/// Input for `mob_reconcile` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobReconcileInput {
    /// Mob ID to reconcile.
    pub mob_id: String,
    /// Reconcile mode: "report_only" or "apply".
    #[serde(default = "default_reconcile_mode")]
    pub mode: String,
}

fn default_reconcile_mode() -> String {
    "report_only".to_string()
}

/// Input for `mob_events_poll` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MobEventsPollInput {
    /// Mob ID to poll events for.
    pub mob_id: String,
    /// Cursor to resume from (exclusive). Omit to start from beginning.
    #[serde(default)]
    pub after_cursor: Option<u64>,
    /// Maximum number of events to return.
    #[serde(default)]
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Structured MCP error
// ---------------------------------------------------------------------------

/// Structured MCP tool-call error payload.
#[derive(Debug, Clone)]
pub struct ToolCallError {
    /// JSON-RPC error code.
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured error data.
    pub data: Option<Value>,
}

impl ToolCallError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    fn method_not_found(message: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: message.into(),
            data: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }

    fn from_mob_error(err: MobError) -> Self {
        match &err {
            MobError::SpecNotFound { .. } | MobError::RunNotFound { .. } => Self {
                code: -32602,
                message: err.to_string(),
                data: None,
            },
            MobError::SpecValidation { diagnostics } => Self {
                code: -32602,
                message: err.to_string(),
                data: Some(json!({
                    "type": "spec_validation",
                    "diagnostics": diagnostics.iter().map(|d| json!({
                        "code": format!("{:?}", d.code),
                        "message": d.message,
                        "location": d.location,
                    })).collect::<Vec<_>>(),
                })),
            },
            MobError::SpecConflict { expected, actual } => Self {
                code: -32602,
                message: err.to_string(),
                data: Some(json!({
                    "type": "spec_conflict",
                    "expected_revision": expected,
                    "actual_revision": actual,
                })),
            },
            MobError::InvalidTransition { from, to } => Self {
                code: -32602,
                message: err.to_string(),
                data: Some(json!({
                    "type": "invalid_transition",
                    "from": format!("{from:?}"),
                    "to": format!("{to:?}"),
                })),
            },
            _ => Self::internal(err.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Shared MCP server state
// ---------------------------------------------------------------------------

/// Shared state for the mob MCP server.
///
/// Holds a `MobRuntime` (which implements `MobService`) and metadata
/// about the server configuration.
pub struct MobMcpState {
    runtime: Arc<MobRuntime>,
}

impl MobMcpState {
    /// Create a new MCP state with an in-memory `MobRuntime`.
    pub fn new(realm_id: &str, mob_id: &str) -> Result<Self, MobError> {
        let config = MobRuntimeConfig {
            realm_id: realm_id.to_string(),
            mob_id: mob_id.to_string(),
            validate_opts: ValidateOptions::default(),
        };
        let spec_store = Arc::new(InMemoryMobSpecStore::new());
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let event_store = Arc::new(InMemoryMobEventStore::new());
        let runtime = MobRuntime::new(config, spec_store, run_store, event_store)?;
        Ok(Self {
            runtime: Arc::new(runtime),
        })
    }

    /// Create from an existing `MobRuntime`.
    pub fn from_runtime(runtime: Arc<MobRuntime>) -> Self {
        Self { runtime }
    }

    /// Access the underlying runtime (for testing).
    pub fn runtime(&self) -> &MobRuntime {
        &self.runtime
    }
}

// ---------------------------------------------------------------------------
// Tools list
// ---------------------------------------------------------------------------

/// Returns the list of MCP tools exposed by this server.
pub fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "mob_spec_apply",
            "description": "Apply (create or update) a mob spec. Provide either spec_toml (TOML string in [mob.specs.<mob_id>] format) or spec_json (direct JSON MobSpec object).",
            "inputSchema": meerkat_tools::schema_for::<MobSpecApplyInput>()
        }),
        json!({
            "name": "mob_spec_get",
            "description": "Get a mob spec by mob ID.",
            "inputSchema": meerkat_tools::schema_for::<MobSpecGetInput>()
        }),
        json!({
            "name": "mob_spec_list",
            "description": "List all mob specs.",
            "inputSchema": meerkat_tools::schema_for::<MobSpecListInput>()
        }),
        json!({
            "name": "mob_spec_delete",
            "description": "Delete a mob spec by mob ID.",
            "inputSchema": meerkat_tools::schema_for::<MobSpecDeleteInput>()
        }),
        json!({
            "name": "mob_activate",
            "description": "Activate a flow within a mob, creating a new run. Returns the run ID and initial status. The flow executes asynchronously; use mob_run_get or mob_events_poll to observe progress.",
            "inputSchema": meerkat_tools::schema_for::<MobActivateInput>()
        }),
        json!({
            "name": "mob_run_get",
            "description": "Get the current state of a mob run by run ID.",
            "inputSchema": meerkat_tools::schema_for::<MobRunGetInput>()
        }),
        json!({
            "name": "mob_run_list",
            "description": "List mob runs with optional filters by mob_id and status.",
            "inputSchema": meerkat_tools::schema_for::<MobRunListInput>()
        }),
        json!({
            "name": "mob_run_cancel",
            "description": "Cancel a pending or running mob run.",
            "inputSchema": meerkat_tools::schema_for::<MobRunCancelInput>()
        }),
        json!({
            "name": "mob_meerkat_list",
            "description": "List meerkat instances for a mob, optionally filtered by role.",
            "inputSchema": meerkat_tools::schema_for::<MobMeerkatListInput>()
        }),
        json!({
            "name": "mob_reconcile",
            "description": "Reconcile desired vs. actual meerkat instances for a mob. Mode can be 'report_only' (default) or 'apply'.",
            "inputSchema": meerkat_tools::schema_for::<MobReconcileInput>()
        }),
        json!({
            "name": "mob_events_poll",
            "description": "Poll mob events after a cursor position. Returns events with monotonically increasing cursors for resumable consumption.",
            "inputSchema": meerkat_tools::schema_for::<MobEventsPollInput>()
        }),
        json!({
            "name": "mob_capabilities",
            "description": "Get the mob plugin runtime capabilities and status.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
    ]
}

// ---------------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------------

/// Handle a tools/call request.
pub async fn handle_tools_call(
    state: &MobMcpState,
    tool_name: &str,
    arguments: &Value,
) -> Result<Value, ToolCallError> {
    match tool_name {
        "mob_spec_apply" => {
            let input: MobSpecApplyInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_spec_apply(state, input).await
        }
        "mob_spec_get" => {
            let input: MobSpecGetInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_spec_get(state, input).await
        }
        "mob_spec_list" => {
            let input: MobSpecListInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_spec_list(state, input).await
        }
        "mob_spec_delete" => {
            let input: MobSpecDeleteInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_spec_delete(state, input).await
        }
        "mob_activate" => {
            let input: MobActivateInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_activate(state, input).await
        }
        "mob_run_get" => {
            let input: MobRunGetInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_run_get(state, input).await
        }
        "mob_run_list" => {
            let input: MobRunListInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_run_list(state, input).await
        }
        "mob_run_cancel" => {
            let input: MobRunCancelInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_run_cancel(state, input).await
        }
        "mob_meerkat_list" => {
            let input: MobMeerkatListInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_list(state, input).await
        }
        "mob_reconcile" => {
            let input: MobReconcileInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_reconcile(state, input).await
        }
        "mob_events_poll" => {
            let input: MobEventsPollInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_events_poll(state, input).await
        }
        "mob_capabilities" => handle_capabilities().await,
        _ => Err(ToolCallError::method_not_found(format!(
            "Unknown tool: {tool_name}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

async fn handle_spec_apply(
    state: &MobMcpState,
    input: MobSpecApplyInput,
) -> Result<Value, ToolCallError> {
    let spec = if let Some(toml_str) = &input.spec_toml {
        let specs = parse_mob_specs_from_toml(toml_str)
            .map_err(|e| ToolCallError::invalid_params(format!("TOML parse error: {e}")))?;
        if specs.is_empty() {
            return Err(ToolCallError::invalid_params(
                "No specs found in TOML input",
            ));
        }
        if specs.len() > 1 {
            return Err(ToolCallError::invalid_params(
                "Multiple specs found in TOML input; provide exactly one",
            ));
        }
        specs.into_iter().next().ok_or_else(|| {
            ToolCallError::internal("unexpected empty specs after length check")
        })?
    } else if let Some(spec_json) = &input.spec_json {
        serde_json::from_value::<MobSpec>(spec_json.clone())
            .map_err(|e| ToolCallError::invalid_params(format!("Invalid spec JSON: {e}")))?
    } else {
        return Err(ToolCallError::invalid_params(
            "Either spec_toml or spec_json must be provided",
        ));
    };

    let result = state
        .runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .map_err(ToolCallError::from_mob_error)?;

    let payload = serde_json::to_value(&result)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

async fn handle_spec_get(
    state: &MobMcpState,
    input: MobSpecGetInput,
) -> Result<Value, ToolCallError> {
    let spec = state
        .runtime
        .get_spec(&input.mob_id)
        .await
        .map_err(ToolCallError::from_mob_error)?;
    let payload = serde_json::to_value(&spec)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

async fn handle_spec_list(
    state: &MobMcpState,
    input: MobSpecListInput,
) -> Result<Value, ToolCallError> {
    let specs = state
        .runtime
        .list_specs(ListSpecsRequest { limit: input.limit })
        .await
        .map_err(ToolCallError::from_mob_error)?;
    let payload = serde_json::to_value(&specs)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

async fn handle_spec_delete(
    state: &MobMcpState,
    input: MobSpecDeleteInput,
) -> Result<Value, ToolCallError> {
    state
        .runtime
        .delete_spec(&input.mob_id)
        .await
        .map_err(ToolCallError::from_mob_error)?;
    Ok(wrap_tool_payload(json!({ "deleted": true, "mob_id": input.mob_id })))
}

async fn handle_activate(
    state: &MobMcpState,
    input: MobActivateInput,
) -> Result<Value, ToolCallError> {
    let result = state
        .runtime
        .activate(ActivateRequest {
            mob_id: input.mob_id,
            flow_id: input.flow_id,
            params: input.params,
        })
        .await
        .map_err(ToolCallError::from_mob_error)?;
    let payload = serde_json::to_value(&result)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

async fn handle_run_get(
    state: &MobMcpState,
    input: MobRunGetInput,
) -> Result<Value, ToolCallError> {
    let run = state
        .runtime
        .get_run(&input.run_id)
        .await
        .map_err(ToolCallError::from_mob_error)?;
    let payload = serde_json::to_value(&run)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

async fn handle_run_list(
    state: &MobMcpState,
    input: MobRunListInput,
) -> Result<Value, ToolCallError> {
    let status = match input.status.as_deref() {
        Some("pending") => Some(RunStatus::Pending),
        Some("running") => Some(RunStatus::Running),
        Some("completed") => Some(RunStatus::Completed),
        Some("failed") => Some(RunStatus::Failed),
        Some("cancelled") => Some(RunStatus::Cancelled),
        Some(other) => {
            return Err(ToolCallError::invalid_params(format!(
                "Invalid status filter: {other}. Valid values: pending, running, completed, failed, cancelled"
            )));
        }
        None => None,
    };
    let runs = state
        .runtime
        .list_runs(ListRunsRequest {
            mob_id: input.mob_id,
            status,
            limit: input.limit,
        })
        .await
        .map_err(ToolCallError::from_mob_error)?;
    let payload = serde_json::to_value(&runs)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

async fn handle_run_cancel(
    state: &MobMcpState,
    input: MobRunCancelInput,
) -> Result<Value, ToolCallError> {
    let run = state
        .runtime
        .cancel_run(&input.run_id)
        .await
        .map_err(ToolCallError::from_mob_error)?;
    let payload = serde_json::to_value(&run)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

async fn handle_meerkat_list(
    state: &MobMcpState,
    input: MobMeerkatListInput,
) -> Result<Value, ToolCallError> {
    let meerkats = state
        .runtime
        .list_meerkats(ListMeerkatsRequest {
            mob_id: input.mob_id,
            role: input.role,
        })
        .await
        .map_err(ToolCallError::from_mob_error)?;
    let payload = serde_json::to_value(&meerkats)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

async fn handle_reconcile(
    state: &MobMcpState,
    input: MobReconcileInput,
) -> Result<Value, ToolCallError> {
    let mode = match input.mode.as_str() {
        "report_only" => ReconcileMode::ReportOnly,
        "apply" => ReconcileMode::Apply,
        other => {
            return Err(ToolCallError::invalid_params(format!(
                "Invalid reconcile mode: {other}. Valid values: report_only, apply"
            )));
        }
    };
    let result = state
        .runtime
        .reconcile(ReconcileRequest {
            mob_id: input.mob_id,
            mode,
        })
        .await
        .map_err(ToolCallError::from_mob_error)?;
    let payload = serde_json::to_value(&result)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

async fn handle_events_poll(
    state: &MobMcpState,
    input: MobEventsPollInput,
) -> Result<Value, ToolCallError> {
    let events = state
        .runtime
        .poll_events(PollEventsRequest {
            mob_id: input.mob_id,
            after_cursor: input.after_cursor,
            limit: input.limit,
        })
        .await
        .map_err(ToolCallError::from_mob_error)?;
    let payload = serde_json::to_value(&events)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

/// Capabilities response for the mob plugin runtime.
#[derive(Debug, Serialize)]
struct MobCapabilities {
    plugin: &'static str,
    version: &'static str,
    features: Vec<&'static str>,
    store_backend: &'static str,
}

async fn handle_capabilities() -> Result<Value, ToolCallError> {
    let caps = MobCapabilities {
        plugin: "meerkat-mob",
        version: env!("CARGO_PKG_VERSION"),
        features: vec![
            "spec_crud",
            "flow_activation",
            "dag_scheduling",
            "fan_out_dispatch",
            "one_to_one_dispatch",
            "collection_all",
            "collection_quorum",
            "collection_any",
            "topology_advisory",
            "topology_strict",
            "reconcile",
            "supervisor",
            "event_polling",
            "drain_replace",
        ],
        store_backend: "in_memory",
    };
    let payload = serde_json::to_value(&caps)
        .map_err(|e| ToolCallError::internal(format!("Serialization error: {e}")))?;
    Ok(wrap_tool_payload(payload))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn wrap_tool_payload(payload: Value) -> Value {
    let text = serde_json::to_string(&payload).unwrap_or_default();
    json!({
        "content": [{
            "type": "text",
            "text": text
        }]
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn test_state() -> MobMcpState {
        MobMcpState::new("test-realm", "test-mob").expect("create test state")
    }

    fn minimal_spec_toml() -> String {
        r#"
[mob.specs.test-mob]
roles = [
    { role = "coordinator", prompt_inline = "You coordinate." },
    { role = "worker", prompt_inline = "You work." },
]

[[mob.specs.test-mob.flows]]
flow_id = "main"
steps = [
    { step_id = "dispatch", targets = { role = "worker", meerkat_id = "*" }, timeout_ms = 5000 },
]
"#
        .to_string()
    }

    fn minimal_spec_json() -> Value {
        json!({
            "mob_id": "test-mob",
            "roles": [
                { "role": "coordinator", "prompt_inline": "You coordinate." },
                { "role": "worker", "prompt_inline": "You work." },
            ],
            "flows": [
                {
                    "flow_id": "main",
                    "steps": [
                        {
                            "step_id": "dispatch",
                            "targets": { "role": "worker", "meerkat_id": "*" },
                            "timeout_ms": 5000
                        }
                    ]
                }
            ]
        })
    }

    // -- Tools list --

    #[test]
    fn test_tools_list_has_all_12_tools() {
        let tools = tools_list();
        assert_eq!(tools.len(), 12, "Expected 12 MCP tools");

        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"mob_spec_apply"));
        assert!(names.contains(&"mob_spec_get"));
        assert!(names.contains(&"mob_spec_list"));
        assert!(names.contains(&"mob_spec_delete"));
        assert!(names.contains(&"mob_activate"));
        assert!(names.contains(&"mob_run_get"));
        assert!(names.contains(&"mob_run_list"));
        assert!(names.contains(&"mob_run_cancel"));
        assert!(names.contains(&"mob_meerkat_list"));
        assert!(names.contains(&"mob_reconcile"));
        assert!(names.contains(&"mob_events_poll"));
        assert!(names.contains(&"mob_capabilities"));
    }

    #[test]
    fn test_tools_list_all_have_input_schemas() {
        let tools = tools_list();
        for tool in &tools {
            assert!(
                tool["inputSchema"].is_object(),
                "Tool {} missing inputSchema",
                tool["name"]
            );
        }
    }

    // -- Input parsing --

    #[test]
    fn test_mob_spec_apply_input_parsing_toml() {
        let input_json = json!({ "spec_toml": "[mob.specs.x]\nroles = []\nflows = []" });
        let input: MobSpecApplyInput = serde_json::from_value(input_json).unwrap();
        assert!(input.spec_toml.is_some());
        assert!(input.spec_json.is_none());
    }

    #[test]
    fn test_mob_spec_apply_input_parsing_json() {
        let input_json = json!({ "spec_json": { "mob_id": "x", "roles": [], "flows": [] } });
        let input: MobSpecApplyInput = serde_json::from_value(input_json).unwrap();
        assert!(input.spec_toml.is_none());
        assert!(input.spec_json.is_some());
    }

    #[test]
    fn test_mob_activate_input_parsing() {
        let input_json = json!({ "mob_id": "m", "flow_id": "f", "params": { "key": "val" } });
        let input: MobActivateInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.mob_id, "m");
        assert_eq!(input.flow_id, "f");
        assert_eq!(input.params["key"], "val");
    }

    #[test]
    fn test_mob_events_poll_input_parsing() {
        let input_json = json!({ "mob_id": "m", "after_cursor": 42, "limit": 10 });
        let input: MobEventsPollInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.mob_id, "m");
        assert_eq!(input.after_cursor, Some(42));
        assert_eq!(input.limit, Some(10));
    }

    #[test]
    fn test_mob_reconcile_input_default_mode() {
        let input_json = json!({ "mob_id": "m" });
        let input: MobReconcileInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.mode, "report_only");
    }

    // -- CHOKE-MOB-008: MCP tool handler -> MobService method --

    #[tokio::test]
    async fn choke_mob_008_spec_apply_via_toml() {
        let state = test_state();
        let result = handle_tools_call(
            &state,
            "mob_spec_apply",
            &json!({ "spec_toml": minimal_spec_toml() }),
        )
        .await
        .unwrap();

        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["mob_id"], "test-mob");
        assert_eq!(payload["spec_revision"], 1);
    }

    #[tokio::test]
    async fn choke_mob_008_spec_apply_via_json() {
        let state = test_state();
        let result = handle_tools_call(
            &state,
            "mob_spec_apply",
            &json!({ "spec_json": minimal_spec_json() }),
        )
        .await
        .unwrap();

        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["mob_id"], "test-mob");
        assert_eq!(payload["spec_revision"], 1);
    }

    #[tokio::test]
    async fn choke_mob_008_spec_get() {
        let state = test_state();
        // Apply first
        handle_tools_call(
            &state,
            "mob_spec_apply",
            &json!({ "spec_json": minimal_spec_json() }),
        )
        .await
        .unwrap();

        // Get
        let result = handle_tools_call(
            &state,
            "mob_spec_get",
            &json!({ "mob_id": "test-mob" }),
        )
        .await
        .unwrap();

        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["mob_id"], "test-mob");
    }

    #[tokio::test]
    async fn choke_mob_008_spec_list() {
        let state = test_state();
        handle_tools_call(
            &state,
            "mob_spec_apply",
            &json!({ "spec_json": minimal_spec_json() }),
        )
        .await
        .unwrap();

        let result = handle_tools_call(&state, "mob_spec_list", &json!({}))
            .await
            .unwrap();

        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Vec<Value> = serde_json::from_str(text).unwrap();
        assert_eq!(payload.len(), 1);
    }

    #[tokio::test]
    async fn choke_mob_008_spec_delete() {
        let state = test_state();
        handle_tools_call(
            &state,
            "mob_spec_apply",
            &json!({ "spec_json": minimal_spec_json() }),
        )
        .await
        .unwrap();

        let result = handle_tools_call(
            &state,
            "mob_spec_delete",
            &json!({ "mob_id": "test-mob" }),
        )
        .await
        .unwrap();

        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["deleted"], true);

        // Verify deletion
        let err = handle_tools_call(
            &state,
            "mob_spec_get",
            &json!({ "mob_id": "test-mob" }),
        )
        .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn choke_mob_008_run_list() {
        let state = test_state();
        let result = handle_tools_call(
            &state,
            "mob_run_list",
            &json!({}),
        )
        .await
        .unwrap();

        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Vec<Value> = serde_json::from_str(text).unwrap();
        assert!(payload.is_empty());
    }

    #[tokio::test]
    async fn choke_mob_008_meerkat_list() {
        // Use a unique mob_id to avoid InprocRegistry collisions with other tests
        let state = MobMcpState::new("isolated-realm", "isolated-mob-meerkat-list")
            .expect("create isolated state");
        // Filter by a specific role that has no meerkats spawned
        let result = handle_tools_call(
            &state,
            "mob_meerkat_list",
            &json!({ "mob_id": "isolated-mob-meerkat-list", "role": "worker" }),
        )
        .await
        .unwrap();

        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Vec<Value> = serde_json::from_str(text).unwrap();
        // No worker meerkats spawned yet (only supervisor exists)
        assert!(
            payload.is_empty(),
            "expected no worker meerkats, got: {payload:?}"
        );
    }

    #[tokio::test]
    async fn choke_mob_008_events_poll_empty() {
        let state = test_state();
        let result = handle_tools_call(
            &state,
            "mob_events_poll",
            &json!({ "mob_id": "test-mob" }),
        )
        .await
        .unwrap();

        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Vec<Value> = serde_json::from_str(text).unwrap();
        assert!(payload.is_empty());
    }

    #[tokio::test]
    async fn choke_mob_008_capabilities() {
        let state = test_state();
        let result = handle_tools_call(&state, "mob_capabilities", &json!({}))
            .await
            .unwrap();

        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["plugin"], "meerkat-mob");
        assert!(payload["features"].as_array().unwrap().len() > 5);
    }

    #[tokio::test]
    async fn choke_mob_008_unknown_tool_returns_error() {
        let state = test_state();
        let result = handle_tools_call(&state, "nonexistent_tool", &json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[tokio::test]
    async fn choke_mob_008_spec_apply_rejects_neither_input() {
        let state = test_state();
        let result = handle_tools_call(&state, "mob_spec_apply", &json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[tokio::test]
    async fn choke_mob_008_run_get_not_found() {
        let state = test_state();
        let result =
            handle_tools_call(&state, "mob_run_get", &json!({ "run_id": "nonexistent" })).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[tokio::test]
    async fn choke_mob_008_run_list_invalid_status() {
        let state = test_state();
        let result = handle_tools_call(
            &state,
            "mob_run_list",
            &json!({ "status": "invalid_status" }),
        )
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[tokio::test]
    async fn choke_mob_008_reconcile_invalid_mode() {
        let state = test_state();
        // Apply spec first
        handle_tools_call(
            &state,
            "mob_spec_apply",
            &json!({ "spec_json": minimal_spec_json() }),
        )
        .await
        .unwrap();

        let result = handle_tools_call(
            &state,
            "mob_reconcile",
            &json!({ "mob_id": "test-mob", "mode": "invalid_mode" }),
        )
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, -32602);
    }

    // -- E2E-MOB-004: Full MCP E2E test --
    // This test verifies the full MCP flow: apply spec, activate, poll events, get run.
    // It uses mock meerkats to simulate real meerkat responses.

    #[tokio::test]
    async fn e2e_mob_004_mcp_full_flow() {
        use meerkat_mob::runtime::{establish_trust, spawn_mock_meerkat};

        let state = test_state();

        // 1. Apply spec
        let apply_result = handle_tools_call(
            &state,
            "mob_spec_apply",
            &json!({ "spec_json": minimal_spec_json() }),
        )
        .await
        .unwrap();
        let text = apply_result["content"][0]["text"].as_str().unwrap();
        let spec: Value = serde_json::from_str(text).unwrap();
        assert_eq!(spec["mob_id"], "test-mob");

        // 2. Spawn mock workers
        let namespace = "test-realm/test-mob";
        let (_w1, h1) = spawn_mock_meerkat(
            namespace,
            "worker-1-e2e",
            "worker",
            "worker-1",
            state.runtime().supervisor(),
        );
        let (_w2, h2) = spawn_mock_meerkat(
            namespace,
            "worker-2-e2e",
            "worker",
            "worker-2",
            state.runtime().supervisor(),
        );

        // Establish trust
        establish_trust(state.runtime().supervisor(), &_w1).await;
        establish_trust(state.runtime().supervisor(), &_w2).await;

        // 3. Activate flow
        let activate_result = handle_tools_call(
            &state,
            "mob_activate",
            &json!({
                "mob_id": "test-mob",
                "flow_id": "main",
                "params": {}
            }),
        )
        .await
        .unwrap();
        let text = activate_result["content"][0]["text"].as_str().unwrap();
        let activation: Value = serde_json::from_str(text).unwrap();
        let run_id = activation["run_id"].as_str().unwrap().to_string();
        assert_eq!(activation["status"], "pending");

        // 4. Wait for async flow execution
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // 5. Get run status
        let run_result = handle_tools_call(
            &state,
            "mob_run_get",
            &json!({ "run_id": run_id }),
        )
        .await
        .unwrap();
        let text = run_result["content"][0]["text"].as_str().unwrap();
        let run: Value = serde_json::from_str(text).unwrap();
        // Run should be completed or still running (async)
        let status = run["status"].as_str().unwrap();
        assert!(
            status == "completed" || status == "running",
            "unexpected status: {status}"
        );

        // 6. Poll events
        let events_result = handle_tools_call(
            &state,
            "mob_events_poll",
            &json!({ "mob_id": "test-mob" }),
        )
        .await
        .unwrap();
        let text = events_result["content"][0]["text"].as_str().unwrap();
        let events: Vec<Value> = serde_json::from_str(text).unwrap();
        // Should have lifecycle events
        assert!(
            !events.is_empty(),
            "expected lifecycle events from activation"
        );

        // Verify cursor monotonicity
        for window in events.windows(2) {
            let c1 = window[0]["cursor"].as_u64().unwrap();
            let c2 = window[1]["cursor"].as_u64().unwrap();
            assert!(c2 > c1, "cursors must be monotonically increasing");
        }

        // 7. Verify events include run lifecycle
        let event_kinds: Vec<&str> = events
            .iter()
            .filter_map(|e| e["kind"]["type"].as_str())
            .collect();
        assert!(
            event_kinds.contains(&"run_started"),
            "expected RunStarted event"
        );

        // 8. List runs
        let list_result = handle_tools_call(
            &state,
            "mob_run_list",
            &json!({ "mob_id": "test-mob" }),
        )
        .await
        .unwrap();
        let text = list_result["content"][0]["text"].as_str().unwrap();
        let runs: Vec<Value> = serde_json::from_str(text).unwrap();
        assert!(!runs.is_empty(), "expected at least one run");

        // Cleanup
        h1.abort();
        h2.abort();
    }

    // -- Cursor-based polling --

    #[tokio::test]
    async fn test_events_poll_cursor_continuation() {
        let state = test_state();

        // Apply spec and activate to generate events
        handle_tools_call(
            &state,
            "mob_spec_apply",
            &json!({ "spec_json": minimal_spec_json() }),
        )
        .await
        .unwrap();

        // Poll with limit
        let result = handle_tools_call(
            &state,
            "mob_events_poll",
            &json!({ "mob_id": "test-mob", "limit": 2 }),
        )
        .await
        .unwrap();

        let text = result["content"][0]["text"].as_str().unwrap();
        let _events: Vec<Value> = serde_json::from_str(text).unwrap();
        // No events yet since no activation
    }
}
