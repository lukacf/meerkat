//! `mob/*` method handlers.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::convert::TryFrom;

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;
use meerkat::surface::RequestContext;
use meerkat_contracts::{MobCreateParams, MobCreateResult, WireAgentRuntimeId};
use meerkat_core::service::AppendSystemContextRequest;
use meerkat_core::types::ContentInput;
use meerkat_mob::{
    AgentIdentity, FlowId, MemberRespawnReceipt, MobBackendKind, MobId, MobRespawnError,
    MobRuntimeMode, RunId, SpawnMemberSpec, SpawnResult,
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

fn spawn_result_payload(mob_id: &MobId, result: &SpawnResult) -> serde_json::Value {
    serde_json::json!({
        "mob_id": mob_id,
        "agent_identity": result.agent_identity,
        "agent_runtime_id": result.agent_runtime_id,
        "fence_token": result.fence_token,
    })
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

    let definition = match meerkat_mob_mcp::decode_public_mob_definition(params.definition) {
        Ok(definition) => definition,
        Err(error) => return invalid_params(id, format!("invalid mob definition: {error}")),
    };

    let result = state.mob_create_definition(definition).await;

    match result {
        Ok(mob_id) => RpcResponse::success(
            id,
            MobCreateResult {
                mob_id: mob_id.to_string(),
            },
        ),
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
    pub agent_identity: String,
    #[serde(default)]
    pub initial_message: Option<ContentInput>,
    #[serde(default)]
    pub runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
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
    let mut spec = SpawnMemberSpec::new(params.profile.as_str(), params.agent_identity.as_str());
    spec.initial_message = params.initial_message;
    spec.runtime_mode = params.runtime_mode;
    spec.backend = params.backend;
    spec.context = params.context;
    spec.labels = params.labels;
    spec.additional_instructions = params.additional_instructions;
    match state.mob_spawn_spec(&mob_id, spec).await {
        Ok(spawn_result) => RpcResponse::success(id, spawn_result_payload(&mob_id, &spawn_result)),
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
    pub agent_identity: String,
    #[serde(default)]
    pub initial_message: Option<ContentInput>,
    #[serde(default)]
    pub runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
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
    agent_identity: Option<meerkat_mob::AgentIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_runtime_id: Option<meerkat_mob::AgentRuntimeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fence_token: Option<meerkat_mob::FenceToken>,
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
        let mut spec = SpawnMemberSpec::new(s.profile.as_str(), s.agent_identity.as_str());
        spec.initial_message = s.initial_message.clone();
        spec.runtime_mode = s.runtime_mode;
        spec.backend = s.backend;
        spec.context = s.context.clone();
        spec.labels = s.labels.clone();
        spec.additional_instructions = s.additional_instructions.clone();
        specs.push(spec);
    }

    match state.mob_spawn_many(&mob_id, specs).await {
        Ok(results) => {
            let entries: Vec<SpawnManyResultEntry> = results
                .into_iter()
                .map(|r| match r {
                    Ok(spawn_result) => SpawnManyResultEntry {
                        ok: true,
                        agent_identity: Some(spawn_result.agent_identity),
                        agent_runtime_id: Some(spawn_result.agent_runtime_id),
                        fence_token: Some(spawn_result.fence_token),
                        error: None,
                    },
                    Err(err) => SpawnManyResultEntry {
                        ok: false,
                        agent_identity: None,
                        agent_runtime_id: None,
                        fence_token: None,
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
    pub agent_identity: String,
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
        .mob_retire(&mob_id, AgentIdentity::from(params.agent_identity.as_str()))
        .await
    {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"retired": true})),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobRespawnParams {
    pub mob_id: String,
    pub agent_identity: String,
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
            AgentIdentity::from(params.agent_identity.as_str()),
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
#[serde(deny_unknown_fields)]
pub struct MobWireParams {
    pub mob_id: String,
    pub member: String,
    pub peer: meerkat_mob::PeerTarget,
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
    match state
        .mob_wire(
            &mob_id,
            AgentIdentity::from(params.member.as_str()),
            params.peer,
        )
        .await
    {
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
    match state
        .mob_unwire(
            &mob_id,
            AgentIdentity::from(params.member.as_str()),
            params.peer,
        )
        .await
    {
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
pub struct MobAppendSystemContextParams {
    pub mob_id: String,
    pub agent_identity: String,
    pub text: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

pub type MobMemberSendParams = meerkat_contracts::MobMemberSendParams;
pub type MobMemberSendResult = meerkat_contracts::MobMemberSendResult;

pub async fn handle_member_send(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobMemberSendParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let agent_identity = AgentIdentity::from(params.agent_identity.as_str());
    let content = match ContentInput::try_from(params.content) {
        Ok(content) => content,
        Err(error) => return invalid_params(id, error),
    };
    match state
        .mob_member_send(
            &mob_id,
            agent_identity.clone(),
            content,
            params.handling_mode.into(),
            params.render_metadata.map(Into::into),
        )
        .await
    {
        Ok(receipt) => RpcResponse::success(
            id,
            MobMemberSendResult {
                mob_id: mob_id.to_string(),
                agent_identity: receipt.identity.to_string(),
                agent_runtime_id: WireAgentRuntimeId {
                    identity: receipt.agent_runtime_id.identity.to_string(),
                    generation: receipt.agent_runtime_id.generation.get(),
                },
                fence_token: receipt.fence_token.get(),
                handling_mode: receipt.handling_mode.into(),
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
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
    let agent_identity = AgentIdentity::from(params.agent_identity.as_str());
    match state
        .mob_append_system_context(
            &mob_id,
            &agent_identity,
            AppendSystemContextRequest {
                text: params.text,
                source: params.source,
                idempotency_key: params.idempotency_key,
            },
        )
        .await
    {
        Ok((_bridge_session_id, result)) => RpcResponse::success(
            id,
            serde_json::json!({
                "mob_id": mob_id,
                "agent_identity": agent_identity,
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
    pub agent_identity: Option<String>,
    #[serde(default, alias = "profile_name")]
    pub role_name: Option<String>,
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
    let agent_identity = AgentIdentity::from(
        params
            .agent_identity
            .unwrap_or_else(|| format!("helper-{}", uuid::Uuid::new_v4())),
    );
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(role) = params.role_name {
        options.role_name = Some(meerkat_mob::ProfileName::from(role));
    }
    options.runtime_mode = params.runtime_mode;
    options.backend = params.backend;
    match state
        .mob_spawn_helper(&mob_id, agent_identity, params.prompt, options)
        .await
    {
        Ok(result) => RpcResponse::success(
            id,
            serde_json::json!({
                "output": result.output,
                "tokens_used": result.tokens_used,
                "agent_identity": result.agent_identity,
                "agent_runtime_id": result.agent_runtime_id,
                "fence_token": result.fence_token,
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
    pub agent_identity: Option<String>,
    #[serde(default, alias = "profile_name")]
    pub role_name: Option<String>,
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
    let source_identity = AgentIdentity::from(params.source_member_id.as_str());
    let agent_identity = AgentIdentity::from(
        params
            .agent_identity
            .unwrap_or_else(|| format!("fork-{}", uuid::Uuid::new_v4())),
    );
    let fork_context = params
        .fork_context
        .unwrap_or(meerkat_mob::ForkContext::FullHistory);
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(role) = params.role_name {
        options.role_name = Some(meerkat_mob::ProfileName::from(role));
    }
    options.runtime_mode = params.runtime_mode;
    options.backend = params.backend;
    match state
        .mob_fork_helper(
            &mob_id,
            &source_identity,
            agent_identity,
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
                "agent_identity": result.agent_identity,
                "agent_runtime_id": result.agent_runtime_id,
                "fence_token": result.fence_token,
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
        .mob_force_cancel(&mob_id, AgentIdentity::from(params.agent_identity.as_str()))
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
        .mob_member_status(
            &mob_id,
            &AgentIdentity::from(params.agent_identity.as_str()),
        )
        .await
    {
        Ok(snapshot) => RpcResponse::success(id, serde_json::json!(snapshot)),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/wait_kickoff
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MobWaitKickoffParams {
    pub mob_id: String,
    #[serde(default)]
    pub member_ids: Option<Vec<String>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

pub async fn handle_wait_kickoff(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobWaitKickoffParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };

    let member_ids = params.member_ids.map(|ids| {
        ids.into_iter()
            .map(|member_id| AgentIdentity::from(member_id.as_str()))
            .collect::<Vec<_>>()
    });

    match state
        .mob_wait_kickoff(&mob_id, member_ids, params.timeout_ms)
        .await
    {
        Ok(members) => RpcResponse::success(id, serde_json::json!({ "members": members })),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/profile/* — realm profile CRUD
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ProfileCreateParams {
    name: String,
    profile: meerkat_mob::Profile,
}

#[derive(Debug, Deserialize)]
struct ProfileNameParams {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ProfileUpdateParams {
    name: String,
    profile: meerkat_mob::Profile,
    expected_revision: u64,
}

#[derive(Debug, Deserialize)]
struct ProfileDeleteParams {
    name: String,
    expected_revision: u64,
}

pub async fn handle_profile_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: ProfileCreateParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match state
        .realm_profile_create(&params.name, &params.profile)
        .await
    {
        Ok(stored) => RpcResponse::success(id, serde_json::json!(stored)),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_profile_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: ProfileNameParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match state.realm_profile_get(&params.name).await {
        Ok(Some(stored)) => RpcResponse::success(id, serde_json::json!(stored)),
        Ok(None) => RpcResponse::success(
            id,
            serde_json::json!({"not_found": true, "name": params.name}),
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_profile_list(id: Option<RpcId>, state: &Arc<MobMcpState>) -> RpcResponse {
    match state.realm_profile_list().await {
        Ok(profiles) => RpcResponse::success(id, serde_json::json!({"profiles": profiles})),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_profile_update(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: ProfileUpdateParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match state
        .realm_profile_update(&params.name, &params.profile, params.expected_revision)
        .await
    {
        Ok(stored) => RpcResponse::success(id, serde_json::json!(stored)),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_profile_delete(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: ProfileDeleteParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match state
        .realm_profile_delete(&params.name, params.expected_revision)
        .await
    {
        Ok(deleted) => RpcResponse::success(
            id,
            serde_json::json!({"name": deleted.name, "deleted_revision": deleted.revision}),
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/turn_start — identity-native turn routing
// ---------------------------------------------------------------------------

/// Parameters for `mob/turn_start`.
///
/// Resolves `agent_identity` to the backing bridge session and delegates to
/// the standard `turn/start` handler. This is the identity-first replacement
/// for extracting `session_id` from spawn responses.
#[derive(Debug, Deserialize)]
pub struct MobTurnStartParams {
    pub mob_id: String,
    pub agent_identity: String,
    pub prompt: serde_json::Value,
    /// All optional turn/start overrides are forwarded transparently.
    #[serde(flatten)]
    pub turn_overrides: serde_json::Map<String, serde_json::Value>,
}

/// Handle `mob/turn_start` — resolve identity to session and delegate to turn/start.
pub async fn handle_mob_turn_start(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
    runtime: Arc<SessionRuntime>,
    notification_sink: &crate::router::NotificationSink,
    runtime_adapter: &meerkat_runtime::MeerkatMachine,
    request_context: Option<RequestContext>,
) -> RpcResponse {
    let mob_params: MobTurnStartParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &mob_params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let identity = AgentIdentity::from(mob_params.agent_identity.as_str());

    // Resolve identity → bridge session ID.
    let handle = match state.handle_for(&mob_id).await {
        Ok(h) => h,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let session_id = match handle.resolve_bridge_session_id(&identity).await {
        Some(sid) => sid,
        None => {
            return invalid_params(
                id,
                format!("member '{identity}' has no active bridge session in mob '{mob_id}'"),
            );
        }
    };

    // Build a turn/start-compatible JSON blob with the resolved session_id.
    let mut turn_params = mob_params.turn_overrides;
    turn_params.insert(
        "session_id".to_string(),
        serde_json::Value::String(session_id.to_string()),
    );
    turn_params.insert("prompt".to_string(), mob_params.prompt);
    let raw_json = serde_json::to_string(&turn_params).unwrap_or_default();
    let raw_value = RawValue::from_string(raw_json).ok();

    super::turn::handle_start(
        id,
        raw_value.as_deref(),
        runtime,
        notification_sink,
        runtime_adapter,
        request_context,
    )
    .await
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_mob::{AgentIdentity, AgentRuntimeId, FenceToken};

    #[test]
    fn respawn_result_preserves_receipt_on_topology_restore_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let receipt = MemberRespawnReceipt::new(
            AgentIdentity::from("worker"),
            AgentRuntimeId::initial(AgentIdentity::from("worker")),
            FenceToken::new(3),
            FenceToken::new(4),
        );
        let response = respawn_result_response(
            Some(RpcId::Num(42)),
            Err(MobRespawnError::TopologyRestoreFailed {
                receipt: receipt.clone(),
                failed_peer_ids: vec![AgentIdentity::from("peer-a"), AgentIdentity::from("peer-b")],
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
        assert_eq!(value["receipt"]["identity"], receipt.identity.to_string());
        assert_eq!(
            value["failed_peer_ids"],
            serde_json::json!(["peer-a", "peer-b"])
        );
        Ok(())
    }

    #[test]
    fn mob_wire_params_accept_canonical_member_and_peer_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::json!({
            "mob_id": "mob-1",
            "member": "worker-a",
            "peer": { "local": "worker-b" }
        });
        let params: MobWireParams = serde_json::from_value(value)?;
        assert_eq!(
            AgentIdentity::from(params.member.as_str()),
            AgentIdentity::from("worker-a")
        );
        assert_eq!(
            params.peer,
            meerkat_mob::PeerTarget::Local(AgentIdentity::from("worker-b"))
        );
        Ok(())
    }

    #[test]
    fn mob_create_params_reject_reserved_owner_session_id() {
        let value = serde_json::json!({
            "definition": {
                "id": "mob-1",
                "owner_session_id": "session-123",
                "profiles": {
                    "worker": { "model": "claude-sonnet-4-6" }
                }
            }
        });
        let err = serde_json::from_value::<MobCreateParams>(value)
            .expect_err("reserved owner_session_id must be rejected");
        assert!(err.to_string().contains("unknown field `owner_session_id`"));
    }

    #[test]
    fn mob_create_params_reject_reserved_internal_lifecycle_flags() {
        let value = serde_json::json!({
            "definition": {
                "id": "mob-1",
                "is_implicit": true,
                "session_cleanup_policy": "destroy_on_owner_archive",
                "profiles": {
                    "worker": { "model": "claude-sonnet-4-6" }
                }
            }
        });
        let err = serde_json::from_value::<MobCreateParams>(value)
            .expect_err("reserved lifecycle fields must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("unknown field `is_implicit`")
                || message.contains("unknown field `session_cleanup_policy`"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn mob_create_params_reject_internal_profile_tool_bundles() {
        let value = serde_json::json!({
            "definition": {
                "id": "mob-1",
                "profiles": {
                    "worker": {
                        "model": "claude-sonnet-4-6",
                        "tools": {
                            "rust_bundles": ["internal-only"]
                        }
                    }
                }
            }
        });
        let err = serde_json::from_value::<MobCreateParams>(value)
            .expect_err("internal rust bundle fields must be rejected");
        assert!(
            err.to_string().contains("did not match any variant")
                || err.to_string().contains("unknown field `rust_bundles`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mob_wire_params_reject_compatibility_shapes() {
        let value = serde_json::json!({
            "mob_id": "mob-1",
            "local": "worker-a",
            "target": { "local": "worker-b" }
        });
        let err = serde_json::from_value::<MobWireParams>(value)
            .expect_err("compatibility shape must be rejected");
        assert!(err.to_string().contains("unknown field `local`"));
    }

    #[test]
    fn mob_wire_params_reject_legacy_a_b_shape() {
        let value = serde_json::json!({
            "mob_id": "mob-1",
            "a": "worker-a",
            "b": "worker-b"
        });
        let err = serde_json::from_value::<MobWireParams>(value)
            .expect_err("legacy a/b shape must be rejected");
        assert!(err.to_string().contains("unknown field `a`"));
    }

    #[test]
    fn mob_wait_kickoff_params_accept_optional_member_ids_and_timeout()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::json!({
            "mob_id": "mob-1",
            "member_ids": ["a", "b"],
            "timeout_ms": 2500
        });
        let params: MobWaitKickoffParams = serde_json::from_value(value)?;
        assert_eq!(params.mob_id, "mob-1");
        assert_eq!(params.member_ids.unwrap_or_default(), vec!["a", "b"]);
        assert_eq!(params.timeout_ms, Some(2500));
        Ok(())
    }
}
