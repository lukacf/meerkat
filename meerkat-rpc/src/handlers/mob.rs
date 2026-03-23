//! `mob/*` method handlers.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::value::RawValue;

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;
use meerkat_core::service::AppendSystemContextRequest;
use meerkat_core::types::{ContentInput, HandlingMode, RenderMetadata, SessionId};
use meerkat_mob::{
    FlowId, MeerkatId, MemberRespawnReceipt, MobBackendKind, MobDefinition, MobId, MobRespawnError,
    MobRuntimeMode, Prefab, RunId, SpawnMemberSpec,
};
use meerkat_mob_mcp::MobMcpState;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

fn invalid_params(id: Option<RpcId>, message: impl Into<String>) -> RpcResponse {
    RpcResponse::error(id, error::INVALID_PARAMS, message.into())
}

#[allow(clippy::result_large_err)]
fn parse_mob_id(id: Option<RpcId>, raw: &str) -> Result<MobId, RpcResponse> {
    if raw.trim().is_empty() {
        return Err(invalid_params(id, "mob_id must not be empty"));
    }
    Ok(MobId::from(raw))
}

#[derive(Debug, Serialize)]
struct MobPrefabEntry {
    key: String,
    toml_template: String,
}

#[derive(Debug, Serialize)]
struct MobPrefabsResult {
    prefabs: Vec<MobPrefabEntry>,
}

/// Handle `mob/prefabs` — list built-in mob prefab templates.
pub async fn handle_prefabs(id: Option<RpcId>) -> RpcResponse {
    let prefabs = Prefab::all()
        .into_iter()
        .map(|prefab| MobPrefabEntry {
            key: prefab.key().to_string(),
            toml_template: prefab.toml_template().to_string(),
        })
        .collect();
    RpcResponse::success(id, MobPrefabsResult { prefabs })
}

#[derive(Debug, Serialize)]
struct MobToolsResult {
    tools: Vec<serde_json::Value>,
}

/// Handle `mob/tools` — list callable mob lifecycle tools.
pub async fn handle_tools(id: Option<RpcId>) -> RpcResponse {
    RpcResponse::success(
        id,
        MobToolsResult {
            tools: meerkat_mob_mcp::tools_list(),
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct MobCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// Handle `mob/call` — call a mob lifecycle tool by name with JSON args.
pub async fn handle_call(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobCallParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match meerkat_mob_mcp::handle_tools_call(state, &params.name, &params.arguments).await {
        Ok(value) => RpcResponse::success(id, value),
        Err(err) => RpcResponse::error(id, error::INVALID_PARAMS, err.message),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobCreateParams {
    #[serde(default)]
    pub prefab: Option<String>,
    #[serde(default)]
    pub definition: Option<MobDefinition>,
}

pub async fn handle_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobCreateParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let result = match (params.prefab.as_deref(), params.definition) {
        (Some(prefab_key), None) => match Prefab::from_key(prefab_key) {
            Some(prefab) => state.mob_create_prefab(prefab).await,
            None => {
                return invalid_params(id, format!("Unknown prefab: {prefab_key}"));
            }
        },
        (None, Some(definition)) => state.mob_create_definition(definition).await,
        (Some(_), Some(_)) => {
            return invalid_params(id, "Provide either prefab or definition, not both");
        }
        (None, None) => {
            return invalid_params(id, "Provide either prefab or definition");
        }
    };

    match result {
        Ok(mob_id) => RpcResponse::success(id, serde_json::json!({"mob_id": mob_id})),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_list(id: Option<RpcId>, state: &Arc<MobMcpState>) -> RpcResponse {
    let mobs = state
        .mob_list()
        .await
        .into_iter()
        .map(|(mob_id, status)| serde_json::json!({"mob_id": mob_id, "status": status.to_string()}))
        .collect::<Vec<_>>();
    RpcResponse::success(id, serde_json::json!({"mobs": mobs}))
}

#[derive(Debug, Deserialize)]
pub struct MobIdParams {
    pub mob_id: String,
}

pub async fn handle_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobIdParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state.mob_status(&mob_id).await {
        Ok(status) => RpcResponse::success(
            id,
            serde_json::json!({"mob_id": mob_id, "status": status.to_string()}),
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobMembersParams {
    pub mob_id: String,
}

pub async fn handle_members(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobMembersParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state.mob_list_members(&mob_id).await {
        Ok(members) => RpcResponse::success(
            id,
            serde_json::json!({"mob_id": mob_id, "members": members}),
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobSpawnParams {
    pub mob_id: String,
    pub profile: String,
    pub meerkat_id: String,
    #[serde(default)]
    pub initial_message: Option<ContentInput>,
    #[serde(default)]
    pub runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
    #[serde(default)]
    pub resume_session_id: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub context: Option<Value>,
    #[serde(default)]
    pub additional_instructions: Option<Vec<String>>,
}

pub async fn handle_spawn(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobSpawnParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let mut spec = SpawnMemberSpec::new(params.profile.as_str(), params.meerkat_id.as_str());
    spec.initial_message = params.initial_message;
    spec.runtime_mode = params.runtime_mode;
    spec.backend = params.backend;
    spec.context = params.context;
    spec.labels = params.labels;
    if let Some(session_id) = params.resume_session_id {
        match SessionId::parse(&session_id) {
            Ok(sid) => {
                spec = spec.with_resume_session_id(sid);
            }
            Err(err) => return invalid_params(id, format!("Invalid resume_session_id: {err}")),
        }
    }
    spec.additional_instructions = params.additional_instructions;
    match state.mob_spawn_spec(&mob_id, spec).await {
        Ok(member_ref) => RpcResponse::success(
            id,
            serde_json::json!({
                "mob_id": mob_id,
                "meerkat_id": params.meerkat_id,
                "member_ref": member_ref,
                "session_id": member_ref.session_id(),
            }),
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/spawn_many
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MobSpawnManyParams {
    pub mob_id: String,
    pub specs: Vec<MobSpawnSpecParams>,
}

/// Per-member spec within a `mob/spawn_many` batch.
#[derive(Debug, Deserialize)]
pub struct MobSpawnSpecParams {
    pub profile: String,
    pub meerkat_id: String,
    #[serde(default)]
    pub initial_message: Option<ContentInput>,
    #[serde(default)]
    pub runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
    #[serde(default)]
    pub resume_session_id: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub context: Option<Value>,
    #[serde(default)]
    pub additional_instructions: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct SpawnManyResultEntry {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    member_ref: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Handle `mob/spawn_many` — batch spawn multiple members.
pub async fn handle_spawn_many(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobSpawnManyParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };

    let mut specs = Vec::with_capacity(params.specs.len());
    for s in &params.specs {
        let mut spec = SpawnMemberSpec::new(s.profile.as_str(), s.meerkat_id.as_str());
        spec.initial_message = s.initial_message.clone();
        spec.runtime_mode = s.runtime_mode;
        spec.backend = s.backend;
        spec.context = s.context.clone();
        spec.labels = s.labels.clone();
        if let Some(raw) = &s.resume_session_id {
            match SessionId::parse(raw) {
                Ok(sid) => {
                    spec = spec.with_resume_session_id(sid);
                }
                Err(err) => {
                    return invalid_params(
                        id,
                        format!("Invalid resume_session_id for {}: {err}", s.meerkat_id),
                    );
                }
            }
        }
        spec.additional_instructions = s.additional_instructions.clone();
        specs.push(spec);
    }

    match state.mob_spawn_many(&mob_id, specs).await {
        Ok(results) => {
            let entries: Vec<SpawnManyResultEntry> = results
                .into_iter()
                .map(|r| match r {
                    Ok(member_ref) => SpawnManyResultEntry {
                        ok: true,
                        session_id: member_ref
                            .session_id()
                            .map(std::string::ToString::to_string),
                        member_ref: serde_json::to_value(&member_ref).ok(),
                        error: None,
                    },
                    Err(err) => SpawnManyResultEntry {
                        ok: false,
                        member_ref: None,
                        session_id: None,
                        error: Some(err.to_string()),
                    },
                })
                .collect();
            RpcResponse::success(id, serde_json::json!({ "results": entries }))
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobMemberParams {
    pub mob_id: String,
    pub meerkat_id: String,
}

pub async fn handle_retire(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobMemberParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_retire(&mob_id, MeerkatId::from(params.meerkat_id.as_str()))
        .await
    {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"retired": true})),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobRespawnParams {
    pub mob_id: String,
    pub meerkat_id: String,
    #[serde(default)]
    pub initial_message: Option<ContentInput>,
}

pub async fn handle_respawn(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobRespawnParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_respawn(
            &mob_id,
            MeerkatId::from(params.meerkat_id.as_str()),
            params.initial_message,
        )
        .await
    {
        Ok(receipt) => respawn_result_response(id, Ok(receipt)),
        Err(err) => respawn_result_response(id, Err(err)),
    }
}

fn respawn_result_response(
    id: Option<RpcId>,
    result: Result<MemberRespawnReceipt, MobRespawnError>,
) -> RpcResponse {
    match result {
        Ok(receipt) => RpcResponse::success(
            id,
            serde_json::json!({
                "status": "completed",
                "receipt": receipt,
            }),
        ),
        Err(MobRespawnError::TopologyRestoreFailed {
            receipt,
            failed_peer_ids,
        }) => RpcResponse::success(
            id,
            serde_json::json!({
                "status": "topology_restore_failed",
                "receipt": receipt,
                "failed_peer_ids": failed_peer_ids
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>(),
            }),
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobWireParams {
    pub mob_id: String,
    #[serde(default)]
    pub member: Option<String>,
    #[serde(default)]
    pub peer: Option<meerkat_mob::PeerTarget>,
    #[serde(default)]
    pub local: Option<String>,
    #[serde(default)]
    pub target: Option<meerkat_mob::PeerTarget>,
    #[serde(default)]
    pub a: Option<String>,
    #[serde(default)]
    pub b: Option<String>,
}

impl MobWireParams {
    fn resolve(self) -> Result<(MeerkatId, meerkat_mob::PeerTarget), String> {
        let has_canonical = self.member.is_some() || self.peer.is_some();
        let has_compat = self.local.is_some() || self.target.is_some();
        let has_legacy = self.a.is_some() || self.b.is_some();

        if has_legacy && (has_canonical || has_compat) {
            return Err(
                "provide either canonical {member, peer}, compatibility {local, target}, or legacy {a, b}, but not both"
                    .to_string(),
            );
        }
        if has_canonical && has_compat {
            return Err(
                "do not mix canonical {member, peer} with compatibility {local, target}"
                    .to_string(),
            );
        }

        match (
            self.member,
            self.peer,
            self.local,
            self.target,
            self.a,
            self.b,
        ) {
            (Some(member), Some(peer), None, None, None, None) => {
                Ok((MeerkatId::from(member), peer))
            }
            (None, None, Some(member), Some(peer), None, None) => {
                Ok((MeerkatId::from(member), peer))
            }
            (None, None, None, None, Some(a), Some(b)) => Ok((
                MeerkatId::from(a),
                meerkat_mob::PeerTarget::Local(MeerkatId::from(b)),
            )),
            _ => Err(
                "mob wire requires canonical {member, peer}, compatibility {local, target}, or legacy {a, b}"
                    .to_string(),
            ),
        }
    }
}

pub async fn handle_wire(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobWireParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let (local, target) = match params.resolve() {
        Ok(resolved) => resolved,
        Err(err) => return invalid_params(id, err),
    };
    match state.mob_wire(&mob_id, local, target).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"wired": true})),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_unwire(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobWireParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let (local, target) = match params.resolve() {
        Ok(resolved) => resolved,
        Err(err) => return invalid_params(id, err),
    };
    match state.mob_unwire(&mob_id, local, target).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"unwired": true})),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobLifecycleParams {
    pub mob_id: String,
    pub action: String,
}

pub async fn handle_lifecycle(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobLifecycleParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let result = match params.action.as_str() {
        "stop" => state.mob_stop(&mob_id).await,
        "resume" => state.mob_resume(&mob_id).await,
        "complete" => state.mob_complete(&mob_id).await,
        "reset" => state.mob_reset(&mob_id).await,
        "destroy" => state.mob_destroy(&mob_id).await,
        other => return invalid_params(id, format!("Unknown mob lifecycle action: {other}")),
    };
    match result {
        Ok(()) => RpcResponse::success(
            id,
            serde_json::json!({"mob_id": mob_id, "action": params.action, "ok": true}),
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobSendParams {
    pub mob_id: String,
    pub meerkat_id: String,
    #[serde(alias = "message")]
    pub content: ContentInput,
    #[serde(default)]
    pub handling_mode: HandlingMode,
    #[serde(default)]
    pub render_metadata: Option<RenderMetadata>,
}

pub async fn handle_send(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobSendParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_member_send(
            &mob_id,
            MeerkatId::from(params.meerkat_id.as_str()),
            params.content,
            params.handling_mode,
            params.render_metadata,
        )
        .await
    {
        Ok(receipt) => RpcResponse::success(id, serde_json::json!(receipt)),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobAppendSystemContextParams {
    pub mob_id: String,
    pub meerkat_id: String,
    pub text: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

pub async fn handle_append_system_context(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
    _runtime: &SessionRuntime,
) -> RpcResponse {
    let params: MobAppendSystemContextParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let meerkat_id = MeerkatId::from(params.meerkat_id.as_str());
    match state
        .mob_append_system_context(
            &mob_id,
            &meerkat_id,
            AppendSystemContextRequest {
                text: params.text,
                source: params.source,
                idempotency_key: params.idempotency_key,
            },
        )
        .await
    {
        Ok((session_id, result)) => RpcResponse::success(
            id,
            serde_json::json!({
                "mob_id": mob_id,
                "meerkat_id": meerkat_id,
                "session_id": session_id,
                "status": result.status,
            }),
        ),
        Err(err) => RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobFlowsParams {
    pub mob_id: String,
}

#[derive(Debug, Deserialize)]
pub struct MobEventsParams {
    pub mob_id: String,
    #[serde(default)]
    pub after_cursor: u64,
    #[serde(default = "default_events_limit")]
    pub limit: usize,
}

const fn default_events_limit() -> usize {
    100
}

pub async fn handle_events(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobEventsParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match state
        .mob_events(
            &meerkat_mob::MobId::from(params.mob_id.as_str()),
            params.after_cursor,
            params.limit,
        )
        .await
    {
        Ok(events) => RpcResponse::success(id, serde_json::json!({ "events": events })),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_flows(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobFlowsParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state.mob_list_flows(&mob_id).await {
        Ok(flows) => {
            RpcResponse::success(id, serde_json::json!({"mob_id": mob_id, "flows": flows}))
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobFlowRunParams {
    pub mob_id: String,
    pub flow_id: String,
    #[serde(default)]
    pub params: Value,
}

pub async fn handle_flow_run(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobFlowRunParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_run_flow(
            &mob_id,
            FlowId::from(params.flow_id.as_str()),
            params.params,
        )
        .await
    {
        Ok(run_id) => RpcResponse::success(id, serde_json::json!({"run_id": run_id})),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobFlowStatusParams {
    pub mob_id: String,
    pub run_id: String,
}

pub async fn handle_flow_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobFlowStatusParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let run_id = match RunId::from_str(&params.run_id) {
        Ok(run_id) => run_id,
        Err(err) => return invalid_params(id, format!("Invalid run_id: {err}")),
    };
    match state.mob_flow_status(&mob_id, run_id).await {
        Ok(run) => RpcResponse::success(id, serde_json::json!({"run": run})),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobFlowCancelParams {
    pub mob_id: String,
    pub run_id: String,
}

pub async fn handle_flow_cancel(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobFlowCancelParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let run_id = match RunId::from_str(&params.run_id) {
        Ok(run_id) => run_id,
        Err(err) => return invalid_params(id, format!("Invalid run_id: {err}")),
    };
    match state.mob_cancel_flow(&mob_id, run_id).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"canceled": true})),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/spawn_helper
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MobSpawnHelperParams {
    pub mob_id: String,
    pub prompt: String,
    #[serde(default)]
    pub meerkat_id: Option<String>,
    #[serde(default)]
    pub profile_name: Option<String>,
    #[serde(default)]
    pub runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
}

pub async fn handle_spawn_helper(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobSpawnHelperParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let meerkat_id = MeerkatId::from(
        params
            .meerkat_id
            .unwrap_or_else(|| format!("helper-{}", uuid::Uuid::new_v4())),
    );
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(profile) = params.profile_name {
        options.profile_name = Some(meerkat_mob::ProfileName::from(profile));
    }
    options.runtime_mode = params.runtime_mode;
    options.backend = params.backend;
    match state
        .mob_spawn_helper(&mob_id, meerkat_id, params.prompt, options)
        .await
    {
        Ok(result) => RpcResponse::success(
            id,
            serde_json::json!({
                "output": result.output,
                "tokens_used": result.tokens_used,
                "session_id": result.session_id,
            }),
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/fork_helper
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MobForkHelperParams {
    pub mob_id: String,
    pub source_member_id: String,
    pub prompt: String,
    #[serde(default)]
    pub meerkat_id: Option<String>,
    #[serde(default)]
    pub profile_name: Option<String>,
    #[serde(default)]
    pub fork_context: Option<meerkat_mob::ForkContext>,
    #[serde(default)]
    pub runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
}

pub async fn handle_fork_helper(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobForkHelperParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let source_member_id = MeerkatId::from(params.source_member_id.as_str());
    let meerkat_id = MeerkatId::from(
        params
            .meerkat_id
            .unwrap_or_else(|| format!("fork-{}", uuid::Uuid::new_v4())),
    );
    let fork_context = params
        .fork_context
        .unwrap_or(meerkat_mob::ForkContext::FullHistory);
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(profile) = params.profile_name {
        options.profile_name = Some(meerkat_mob::ProfileName::from(profile));
    }
    options.runtime_mode = params.runtime_mode;
    options.backend = params.backend;
    match state
        .mob_fork_helper(
            &mob_id,
            &source_member_id,
            meerkat_id,
            params.prompt,
            fork_context,
            options,
        )
        .await
    {
        Ok(result) => RpcResponse::success(
            id,
            serde_json::json!({
                "output": result.output,
                "tokens_used": result.tokens_used,
                "session_id": result.session_id,
            }),
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/force_cancel
// ---------------------------------------------------------------------------

pub async fn handle_force_cancel(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobMemberParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_force_cancel(&mob_id, MeerkatId::from(params.meerkat_id.as_str()))
        .await
    {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"cancelled": true})),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/member_status
// ---------------------------------------------------------------------------

pub async fn handle_member_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobMemberParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_member_status(&mob_id, &MeerkatId::from(params.meerkat_id.as_str()))
        .await
    {
        Ok(snapshot) => RpcResponse::success(id, serde_json::json!(snapshot)),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::types::SessionId;

    #[tokio::test]
    async fn handle_prefabs_returns_expected_shape() -> Result<(), Box<dyn std::error::Error>> {
        let resp = handle_prefabs(Some(RpcId::Num(7))).await;
        assert!(resp.error.is_none());
        assert_eq!(resp.id, Some(RpcId::Num(7)));
        let result = resp.result.as_ref();
        assert!(result.is_some(), "result payload should exist");
        let Some(raw) = result else {
            return Ok(());
        };
        let value: serde_json::Value = serde_json::from_str(raw.get())?;
        let prefabs = value["prefabs"].as_array();
        assert!(prefabs.is_some(), "prefabs should be an array");
        let Some(prefabs) = prefabs else {
            return Ok(());
        };
        assert!(prefabs.iter().all(|entry| entry["key"].is_string()));
        assert!(
            prefabs
                .iter()
                .all(|entry| entry["toml_template"].is_string())
        );
        Ok(())
    }

    #[test]
    fn respawn_result_preserves_receipt_on_topology_restore_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let receipt = MemberRespawnReceipt::new(
            MeerkatId::from("worker"),
            Some(SessionId::new()),
            Some(SessionId::new()),
        );
        let response = respawn_result_response(
            Some(RpcId::Num(42)),
            Err(MobRespawnError::TopologyRestoreFailed {
                receipt: receipt.clone(),
                failed_peer_ids: vec![MeerkatId::from("peer-a"), MeerkatId::from("peer-b")],
            }),
        );

        assert!(
            response.error.is_none(),
            "partial failure should stay in result envelope"
        );
        let Some(raw) = response.result.as_ref() else {
            panic!("result payload should exist");
        };
        let value: serde_json::Value = serde_json::from_str(raw.get())?;
        assert_eq!(value["status"], "topology_restore_failed");
        assert_eq!(value["receipt"]["member_id"], receipt.member_id.to_string());
        assert_eq!(
            value["failed_peer_ids"],
            serde_json::json!(["peer-a", "peer-b"])
        );
        Ok(())
    }

    #[test]
    fn mob_send_params_accept_legacy_message_alias() -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::json!({
            "mob_id": "mob-1",
            "meerkat_id": "worker-1",
            "message": "hello from legacy caller"
        });
        let params: MobSendParams = serde_json::from_value(value)?;
        assert_eq!(params.mob_id, "mob-1");
        assert_eq!(params.meerkat_id, "worker-1");
        assert_eq!(params.content.text_content(), "hello from legacy caller");
        Ok(())
    }

    #[test]
    fn mob_wire_params_reject_mixed_canonical_and_compatibility_fields() {
        let params = MobWireParams {
            mob_id: "mob-1".to_string(),
            member: Some("worker-a".to_string()),
            peer: None,
            local: None,
            target: Some(meerkat_mob::PeerTarget::Local(MeerkatId::from("worker-b"))),
            a: None,
            b: None,
        };
        let err = params
            .resolve()
            .expect_err("mixed canonical/compat fields must fail");
        assert!(err.contains("do not mix canonical"));
    }

    #[test]
    fn mob_wire_params_resolve_compatibility_shape() {
        let params = MobWireParams {
            mob_id: "mob-1".to_string(),
            member: None,
            peer: None,
            local: Some("worker-a".to_string()),
            target: Some(meerkat_mob::PeerTarget::Local(MeerkatId::from("worker-b"))),
            a: None,
            b: None,
        };
        let (member, target) = params
            .resolve()
            .expect("compatibility shape should resolve");
        assert_eq!(member, MeerkatId::from("worker-a"));
        assert_eq!(
            target,
            meerkat_mob::PeerTarget::Local(MeerkatId::from("worker-b"))
        );
    }
}
