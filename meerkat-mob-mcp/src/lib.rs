use meerkat::{AgentFactory, Config, FactoryAgentBuilder, SessionService};
use meerkat_core::{ConfigStore, FileConfigStore, RealmSelection, RuntimeBootstrap};
use meerkat_mob::{
    ApplyContext, ApplySpecRequest, FlowId, MobActivationRequest, MobId, MobReconcileRequest,
    MobRunFilter, MobRuntimeBuilder, MobRuntimeService, MobService, RedbMobEventStore,
    RedbMobRunStore, RedbMobSpecStore,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ToolCallError {
    pub code: i32,
    pub message: String,
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
}

pub struct MobMcpState {
    mob: Arc<dyn MobService>,
    realm_id: String,
    backend: String,
}

impl MobMcpState {
    pub async fn new_with_bootstrap(
        bootstrap: RuntimeBootstrap,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let locator = bootstrap.realm.resolve_locator()?;
        let realm_id = locator.realm_id;
        let realms_root = locator.state_root;

        let backend_hint = bootstrap
            .realm
            .backend_hint
            .as_deref()
            .and_then(parse_backend_hint)
            .or(Some(meerkat_store::RealmBackend::Redb));

        let origin_hint = Some(realm_origin_from_selection(&bootstrap.realm.selection));

        let (manifest, session_store) = meerkat_store::open_realm_session_store_in(
            &realms_root,
            &realm_id,
            backend_hint,
            origin_hint,
        )
        .await?;

        let realm_paths = meerkat_store::realm_paths_in(&realms_root, &realm_id);
        let config_store: Arc<dyn ConfigStore> = Arc::new(FileConfigStore::new(
            realm_paths.config_path.clone(),
        ));
        let mut config = config_store.get().await.unwrap_or_else(|_| Config::default());
        let _ = config.apply_env_overrides();
        let default_model = config.agent.model.clone();

        let store_path = match manifest.backend {
            meerkat_store::RealmBackend::Redb => realm_paths.sessions_redb_path.clone(),
            #[allow(unreachable_patterns)]
            _ => realm_paths.sessions_redb_path.clone(),
        };

        let project_root = bootstrap
            .context
            .context_root
            .clone()
            .unwrap_or_else(|| realm_paths.root.clone());

        let mut factory = AgentFactory::new(store_path)
            .session_store(session_store.clone())
            .runtime_root(realm_paths.root.clone())
            .project_root(project_root.clone())
            .builtins(true)
            .shell(true)
            .comms(true);

        if let Some(context_root) = bootstrap.context.context_root.clone() {
            factory = factory.context_root(context_root);
        }

        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let builder = FactoryAgentBuilder::new_with_config_store(factory.clone(), config, config_store);
        let session_service = meerkat::PersistentSessionService::new(builder, 100, session_store);
        let session_service: Arc<dyn SessionService> = Arc::new(session_service);

        let mob_root = realm_paths.root.join("mob");
        tokio::fs::create_dir_all(&mob_root).await?;
        let spec_store = Arc::new(RedbMobSpecStore::open(mob_root.join("specs.redb"))?);
        let run_store = Arc::new(RedbMobRunStore::open(mob_root.join("runs.redb"))?);
        let event_store = Arc::new(RedbMobEventStore::open(mob_root.join("events.redb"))?);

        let mut mob_builder = MobRuntimeBuilder::new(
            realm_id.clone(),
            session_service,
            spec_store,
            run_store,
            event_store,
        )
        .default_model(default_model)
        .runtime_root(realm_paths.root)
        .context_root(project_root);
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            mob_builder = mob_builder.user_config_root(user_root);
        }
        let mob_runtime: MobRuntimeService = mob_builder.build_service()?;

        Ok(Self {
            mob: Arc::new(mob_runtime),
            realm_id,
            backend: manifest.backend.as_str().to_string(),
        })
    }

    pub async fn new_default() -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap(RuntimeBootstrap::default()).await
    }

    pub fn realm_id(&self) -> &str {
        &self.realm_id
    }

    pub fn backend(&self) -> &str {
        &self.backend
    }
}

pub fn tools_list() -> Vec<Value> {
    vec![
        tool("mob_spec_apply", "Apply and validate a mob spec", json!({
            "type": "object",
            "properties": {
                "spec_toml": {"type": "string"},
                "requested_mob_id": {"type": "string"},
                "expected_revision": {"type": "integer"},
                "base_dir": {"type": "string"},
                "update_mode": {"type": "string", "enum": ["drain_replace", "force_reset", "hot_reload"]}
            },
            "required": ["spec_toml"]
        })),
        tool("mob_spec_get", "Get a mob spec by ID", json!({
            "type": "object",
            "properties": { "mob_id": {"type": "string"} },
            "required": ["mob_id"]
        })),
        tool("mob_spec_list", "List mob specs", json!({"type": "object", "properties": {}})),
        tool("mob_spec_delete", "Delete a mob spec", json!({
            "type": "object",
            "properties": { "mob_id": {"type": "string"} },
            "required": ["mob_id"]
        })),
        tool("mob_activate", "Activate a mob flow", json!({
            "type": "object",
            "properties": {
                "mob_id": {"type": "string"},
                "flow_id": {"type": "string"},
                "payload": {},
                "dry_run": {"type": "boolean"}
            },
            "required": ["mob_id", "flow_id"]
        })),
        tool("mob_run_get", "Get a run", json!({
            "type": "object",
            "properties": { "run_id": {"type": "string"} },
            "required": ["run_id"]
        })),
        tool("mob_run_list", "List runs", json!({
            "type": "object",
            "properties": {
                "status": {"type": "string"},
                "mob_id": {"type": "string"},
                "flow_id": {"type": "string"},
                "limit": {"type": "integer"},
                "offset": {"type": "integer"}
            }
        })),
        tool("mob_run_cancel", "Cancel a run", json!({
            "type": "object",
            "properties": { "run_id": {"type": "string"} },
            "required": ["run_id"]
        })),
        tool("mob_meerkat_list", "List active meerkats for a mob", json!({
            "type": "object",
            "properties": { "mob_id": {"type": "string"} },
            "required": ["mob_id"]
        })),
        tool("mob_reconcile", "Reconcile dynamic meerkat population", json!({
            "type": "object",
            "properties": {
                "mob_id": {"type": "string"},
                "mode": {"type": "string", "enum": ["report", "apply"]}
            },
            "required": ["mob_id"]
        })),
        tool("mob_events_poll", "Poll mob events by cursor", json!({
            "type": "object",
            "properties": {
                "cursor": {"type": "integer"},
                "limit": {"type": "integer"}
            }
        })),
        tool("mob_capabilities", "Get mob plugin capabilities", json!({"type": "object", "properties": {}})),
    ]
}

pub async fn handle_tools_call(
    state: &MobMcpState,
    tool_name: &str,
    arguments: &Value,
) -> Result<Value, ToolCallError> {
    match tool_name {
        "mob_spec_apply" => {
            let input: MobSpecApplyInput = serde_json::from_value(arguments.clone())
                .map_err(|err| ToolCallError::invalid_params(format!("Invalid arguments: {err}")))?;
            let update_mode = match input.update_mode.as_deref() {
                Some(raw) => parse_update_mode(raw)
                    .ok_or_else(|| ToolCallError::invalid_params("invalid update_mode"))?,
                None => meerkat_mob::SpecUpdateMode::DrainReplace,
            };
            let spec = state
                .mob
                .apply_spec(ApplySpecRequest {
                    spec_toml: input.spec_toml,
                    requested_mob_id: input.requested_mob_id,
                    expected_revision: input.expected_revision,
                    update_mode,
                    context: ApplyContext {
                        context_root: None,
                        base_dir: input.base_dir.map(PathBuf::from),
                        now: chrono::Utc::now(),
                    },
                })
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"spec": spec})))
        }
        "mob_spec_get" => {
            let input: MobSpecGetInput = serde_json::from_value(arguments.clone())
                .map_err(|err| ToolCallError::invalid_params(format!("Invalid arguments: {err}")))?;
            let spec = state
                .mob
                .get_spec(&input.mob_id)
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"spec": spec})))
        }
        "mob_spec_list" => {
            let specs = state
                .mob
                .list_specs()
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"specs": specs})))
        }
        "mob_spec_delete" => {
            let input: MobSpecDeleteInput = serde_json::from_value(arguments.clone())
                .map_err(|err| ToolCallError::invalid_params(format!("Invalid arguments: {err}")))?;
            state
                .mob
                .delete_spec(&input.mob_id)
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"deleted": true})))
        }
        "mob_activate" => {
            let input: MobActivateInput = serde_json::from_value(arguments.clone())
                .map_err(|err| ToolCallError::invalid_params(format!("Invalid arguments: {err}")))?;
            let response = state
                .mob
                .activate(MobActivationRequest {
                    mob_id: input.mob_id.into(),
                    flow_id: input.flow_id.into(),
                    payload: input.payload.unwrap_or(Value::Null),
                    dry_run: input.dry_run.unwrap_or(false),
                })
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"run": response})))
        }
        "mob_run_get" => {
            let input: MobRunGetInput = serde_json::from_value(arguments.clone())
                .map_err(|err| ToolCallError::invalid_params(format!("Invalid arguments: {err}")))?;
            let run = state
                .mob
                .get_run(&input.run_id)
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"run": run})))
        }
        "mob_run_list" => {
            let input: MobRunListInput = serde_json::from_value(arguments.clone())
                .map_err(|err| ToolCallError::invalid_params(format!("Invalid arguments: {err}")))?;
            let status = match input.status.as_deref() {
                Some(raw) => parse_run_status(raw)
                    .map(Some)
                    .ok_or_else(|| ToolCallError::invalid_params("invalid status"))?,
                None => None,
            };

            let runs = state
                .mob
                .list_runs(MobRunFilter {
                    status,
                    mob_id: input.mob_id.map(MobId::from),
                    flow_id: input.flow_id.map(FlowId::from),
                    limit: input.limit,
                    offset: input.offset,
                })
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"runs": runs})))
        }
        "mob_run_cancel" => {
            let input: MobRunCancelInput = serde_json::from_value(arguments.clone())
                .map_err(|err| ToolCallError::invalid_params(format!("Invalid arguments: {err}")))?;
            state
                .mob
                .cancel_run(&input.run_id)
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"canceled": true})))
        }
        "mob_meerkat_list" => {
            let input: MobMeerkatListInput = serde_json::from_value(arguments.clone())
                .map_err(|err| ToolCallError::invalid_params(format!("Invalid arguments: {err}")))?;
            let meerkats = state
                .mob
                .list_meerkats(&input.mob_id)
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"meerkats": meerkats})))
        }
        "mob_reconcile" => {
            let input: MobReconcileInput = serde_json::from_value(arguments.clone())
                .map_err(|err| ToolCallError::invalid_params(format!("Invalid arguments: {err}")))?;
            let mode = match input.mode.as_deref() {
                Some("report") => crate_mode_report(),
                Some("apply") | None => crate_mode_apply(),
                Some(_) => {
                    return Err(ToolCallError::invalid_params(
                        "mode must be 'report' or 'apply'",
                    ));
                }
            };

            let result = state
                .mob
                .reconcile(MobReconcileRequest {
                    mob_id: input.mob_id.into(),
                    mode,
                })
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"reconcile": result})))
        }
        "mob_events_poll" => {
            let input: MobEventsPollInput = serde_json::from_value(arguments.clone())
                .map_err(|err| ToolCallError::invalid_params(format!("Invalid arguments: {err}")))?;
            let events = state
                .mob
                .poll_events(input.cursor, input.limit)
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(json!({"events": events.events, "next_cursor": events.next_cursor})))
        }
        "mob_capabilities" => {
            let capabilities = state
                .mob
                .capabilities()
                .await
                .map_err(|err| ToolCallError::internal(err.to_string()))?;
            Ok(wrap_tool_payload(capabilities))
        }
        _ => Err(ToolCallError::method_not_found(format!(
            "Unknown tool: {tool_name}"
        ))),
    }
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

fn wrap_tool_payload(payload: Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": payload.to_string(),
        }],
        "payload": payload,
    })
}

fn parse_backend_hint(value: &str) -> Option<meerkat_store::RealmBackend> {
    match value {
        "redb" => Some(meerkat_store::RealmBackend::Redb),
        _ => None,
    }
}

fn realm_origin_from_selection(selection: &RealmSelection) -> meerkat_store::RealmOrigin {
    match selection {
        RealmSelection::Explicit { .. } => meerkat_store::RealmOrigin::Explicit,
        RealmSelection::WorkspaceDerived { .. } => meerkat_store::RealmOrigin::Workspace,
        RealmSelection::Isolated => meerkat_store::RealmOrigin::Generated,
    }
}

fn parse_run_status(value: &str) -> Option<meerkat_mob::MobRunStatus> {
    match value {
        "pending" => Some(meerkat_mob::MobRunStatus::Pending),
        "running" => Some(meerkat_mob::MobRunStatus::Running),
        "completed" => Some(meerkat_mob::MobRunStatus::Completed),
        "failed" => Some(meerkat_mob::MobRunStatus::Failed),
        "canceled" => Some(meerkat_mob::MobRunStatus::Canceled),
        _ => None,
    }
}

fn parse_update_mode(value: &str) -> Option<meerkat_mob::SpecUpdateMode> {
    match value {
        "drain_replace" => Some(meerkat_mob::SpecUpdateMode::DrainReplace),
        "force_reset" => Some(meerkat_mob::SpecUpdateMode::ForceReset),
        "hot_reload" => Some(meerkat_mob::SpecUpdateMode::HotReload),
        _ => None,
    }
}

fn crate_mode_report() -> meerkat_mob::ReconcileMode {
    meerkat_mob::ReconcileMode::Report
}

fn crate_mode_apply() -> meerkat_mob::ReconcileMode {
    meerkat_mob::ReconcileMode::Apply
}

#[derive(Debug, Deserialize)]
struct MobSpecApplyInput {
    spec_toml: String,
    #[serde(default)]
    requested_mob_id: Option<String>,
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(default)]
    base_dir: Option<String>,
    #[serde(default)]
    update_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MobSpecGetInput {
    mob_id: String,
}

#[derive(Debug, Deserialize)]
struct MobSpecDeleteInput {
    mob_id: String,
}

#[derive(Debug, Deserialize)]
struct MobActivateInput {
    mob_id: String,
    flow_id: String,
    #[serde(default)]
    payload: Option<Value>,
    #[serde(default)]
    dry_run: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct MobRunGetInput {
    run_id: String,
}

#[derive(Debug, Deserialize)]
struct MobRunCancelInput {
    run_id: String,
}

#[derive(Debug, Deserialize)]
struct MobRunListInput {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    mob_id: Option<String>,
    #[serde(default)]
    flow_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct MobMeerkatListInput {
    mob_id: String,
}

#[derive(Debug, Deserialize)]
struct MobReconcileInput {
    mob_id: String,
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MobEventsPollInput {
    #[serde(default)]
    cursor: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use meerkat_mob::{
        MeerkatInstance, MobEvent, MobReconcileResult, MobRun, MobRunStatus, MobSpec,
        NewMobEvent, PollEventsResponse, ReconcileMode,
    };
    use std::collections::BTreeMap;

    struct FakeMob;

    #[async_trait]
    impl MobService for FakeMob {
        async fn apply_spec(&self, request: ApplySpecRequest) -> meerkat_mob::MobResult<MobSpec> {
            Ok(sample_spec(request.requested_mob_id.as_deref().unwrap_or("invoice")))
        }

        async fn get_spec(&self, mob_id: &str) -> meerkat_mob::MobResult<Option<MobSpec>> {
            Ok(Some(sample_spec(mob_id)))
        }

        async fn list_specs(&self) -> meerkat_mob::MobResult<Vec<MobSpec>> {
            Ok(vec![sample_spec("invoice")])
        }

        async fn delete_spec(&self, _mob_id: &str) -> meerkat_mob::MobResult<()> {
            Ok(())
        }

        async fn activate(
            &self,
            request: MobActivationRequest,
        ) -> meerkat_mob::MobResult<meerkat_mob::MobActivationResponse> {
            Ok(meerkat_mob::MobActivationResponse {
                run_id: format!("run-{}", request.flow_id).into(),
                status: MobRunStatus::Pending,
                spec_revision: 1,
            })
        }

        async fn get_run(&self, run_id: &str) -> meerkat_mob::MobResult<Option<MobRun>> {
            Ok(Some(MobRun::new(
                run_id.into(),
                "invoice".into(),
                "triage".into(),
                1,
                Value::Null,
            )))
        }

        async fn list_runs(
            &self,
            _filter: MobRunFilter,
        ) -> meerkat_mob::MobResult<Vec<MobRun>> {
            Ok(vec![MobRun::new(
                "run-1".into(),
                "invoice".into(),
                "triage".into(),
                1,
                Value::Null,
            )])
        }

        async fn cancel_run(&self, _run_id: &str) -> meerkat_mob::MobResult<()> {
            Ok(())
        }

        async fn list_meerkats(
            &self,
            _mob_id: &str,
        ) -> meerkat_mob::MobResult<Vec<MeerkatInstance>> {
            Ok(Vec::new())
        }

        async fn reconcile(
            &self,
            _request: MobReconcileRequest,
        ) -> meerkat_mob::MobResult<MobReconcileResult> {
            Ok(MobReconcileResult {
                spawned: vec!["reviewer/k1".to_string()],
                retired: Vec::new(),
                unchanged: Vec::new(),
            })
        }

        async fn poll_events(
            &self,
            _cursor: Option<u64>,
            _limit: Option<usize>,
        ) -> meerkat_mob::MobResult<PollEventsResponse> {
            Ok(PollEventsResponse {
                next_cursor: 2,
                events: vec![MobEvent {
                    cursor: 1,
                    timestamp: Utc::now(),
                    category: meerkat_mob::MobEventCategory::Flow,
                    mob_id: "invoice".into(),
                    run_id: Some("run-1".into()),
                    flow_id: Some("triage".into()),
                    step_id: None,
                    meerkat_id: None,
                    kind: meerkat_mob::MobEventKind::RunActivated,
                    payload: Value::Null,
                }],
            })
        }

        async fn capabilities(&self) -> meerkat_mob::MobResult<Value> {
            Ok(json!({"flow_model": "parallel_dag"}))
        }

        async fn emit_event(&self, _event: NewMobEvent) -> meerkat_mob::MobResult<()> {
            Ok(())
        }

        async fn shutdown(&self) -> meerkat_mob::MobResult<()> {
            Ok(())
        }
    }

    fn sample_spec(mob_id: &str) -> MobSpec {
        MobSpec {
            mob_id: mob_id.into(),
            revision: 1,
            roles: BTreeMap::new(),
            topology: meerkat_mob::TopologySpec::default(),
            flows: BTreeMap::new(),
            prompts: BTreeMap::new(),
            schemas: BTreeMap::new(),
            tool_bundles: BTreeMap::new(),
            resolvers: BTreeMap::new(),
            supervisor: meerkat_mob::SupervisorSpec::default(),
            limits: meerkat_mob::LimitsSpec::default(),
            retention: meerkat_mob::RetentionSpec::default(),
            applied_at: Utc::now(),
        }
    }

    fn state() -> MobMcpState {
        MobMcpState {
            mob: Arc::new(FakeMob),
            realm_id: "realm-test".to_string(),
            backend: "redb".to_string(),
        }
    }

    #[tokio::test]
    async fn tools_list_contains_required_surface() {
        let names: Vec<String> = tools_list()
            .into_iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(ToOwned::to_owned))
            .collect();

        for required in [
            "mob_spec_apply",
            "mob_spec_get",
            "mob_spec_list",
            "mob_spec_delete",
            "mob_activate",
            "mob_run_get",
            "mob_run_list",
            "mob_run_cancel",
            "mob_meerkat_list",
            "mob_reconcile",
            "mob_events_poll",
            "mob_capabilities",
        ] {
            assert!(names.contains(&required.to_string()));
        }
    }

    #[tokio::test]
    async fn unknown_tool_returns_method_not_found() {
        let err = handle_tools_call(&state(), "unknown_tool", &json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[tokio::test]
    async fn run_list_rejects_invalid_status() {
        let err = handle_tools_call(
            &state(),
            "mob_run_list",
            &json!({"status": "invalid"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[tokio::test]
    async fn capabilities_tool_returns_payload() {
        let value = handle_tools_call(&state(), "mob_capabilities", &json!({}))
            .await
            .unwrap();
        let payload = value.get("payload").cloned().unwrap_or(Value::Null);
        assert_eq!(payload.get("flow_model"), Some(&json!("parallel_dag")));
    }

    #[tokio::test]
    async fn mcp_tool_roundtrip_activate_run_and_events() {
        let st = state();

        let activate = handle_tools_call(
            &st,
            "mob_activate",
            &json!({"mob_id": "invoice", "flow_id": "triage", "payload": {"priority": "high"}}),
        )
        .await
        .unwrap();
        let run_id = activate
            .get("payload")
            .and_then(|p| p.get("run"))
            .and_then(|r| r.get("run_id"))
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        assert_eq!(run_id, "run-triage");

        let run = handle_tools_call(&st, "mob_run_get", &json!({"run_id": run_id}))
            .await
            .unwrap();
        assert!(
            run.get("payload")
                .and_then(|p| p.get("run"))
                .and_then(|r| r.get("flow_id"))
                .is_some()
        );

        let events = handle_tools_call(&st, "mob_events_poll", &json!({"cursor": 0, "limit": 10}))
            .await
            .unwrap();
        let event_len = events
            .get("payload")
            .and_then(|p| p.get("events"))
            .and_then(Value::as_array)
            .map_or(0, std::vec::Vec::len);
        assert!(event_len >= 1);
    }

    #[tokio::test]
    async fn reconcile_mode_defaults_to_apply() {
        let value = handle_tools_call(
            &state(),
            "mob_reconcile",
            &json!({"mob_id": "invoice"}),
        )
        .await
        .unwrap();
        let payload = value.get("payload").cloned().unwrap_or(Value::Null);
        assert!(payload.get("reconcile").is_some());
    }

    #[tokio::test]
    async fn reconcile_mode_rejects_invalid_value() {
        let err = handle_tools_call(
            &state(),
            "mob_reconcile",
            &json!({"mob_id": "invoice", "mode": "invalid"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[tokio::test]
    async fn spec_apply_accepts_force_reset_mode() {
        let value = handle_tools_call(
            &state(),
            "mob_spec_apply",
            &json!({"spec_toml": "[mob.specs.invoice.roles.r]\nprompt_ref='config://prompts/p'\n[mob.specs.invoice.prompts]\np='x'\n[[mob.specs.invoice.flows.f.steps]]\nstep_id='s'\n[mob.specs.invoice.flows.f.steps.targets]\nrole='r'", "update_mode": "force_reset"}),
        )
        .await
        .unwrap();
        let payload = value.get("payload").cloned().unwrap_or(Value::Null);
        assert!(payload.get("spec").is_some());
    }

    #[tokio::test]
    async fn spec_apply_rejects_invalid_update_mode() {
        let err = handle_tools_call(
            &state(),
            "mob_spec_apply",
            &json!({"spec_toml": "x", "update_mode": "invalid"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[test]
    fn reconcile_mode_helpers_map_correctly() {
        assert!(matches!(crate_mode_apply(), ReconcileMode::Apply));
        assert!(matches!(crate_mode_report(), ReconcileMode::Report));
    }
}
