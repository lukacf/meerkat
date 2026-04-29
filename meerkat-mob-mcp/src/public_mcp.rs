#![allow(unused_imports)]

use crate::{McpToolError, MobMcpState, decode_public_mob_definition};
use meerkat_contracts::{
    MobCreateParams, MobMemberSendParams, MobPeerTarget, MobUnwireParams, MobWireParams,
    RealtimeCapabilities, RealtimeCapabilitiesParams, RealtimeCapabilitiesResult,
    RealtimeChannelTarget, RealtimeOpenRequest, RealtimeStatusParams, RealtimeStatusResult,
    WireContentInput, WireMobBackendKind, WireMobRuntimeMode, WireRuntimeBinding,
};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(not(target_arch = "wasm32"))]
use tokio::net::TcpStream;

fn spawn_result_payload(mob_id: &meerkat_mob::MobId, result: &meerkat_mob::SpawnResult) -> Value {
    json!({
        "agent_identity": result.agent_identity,
        "member_ref": meerkat_contracts::WireMemberRef::encode(
            mob_id.as_str(),
            result.agent_identity.as_ref(),
        ),
    })
}

fn respawn_receipt_payload(
    mob_id: &meerkat_mob::MobId,
    receipt: &meerkat_mob::MemberRespawnReceipt,
) -> Value {
    json!({
        "agent_identity": receipt.identity,
        "member_ref": meerkat_contracts::WireMemberRef::encode(
            mob_id.as_str(),
            receipt.identity.as_ref(),
        ),
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobIdInput {
    mob_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobLifecycleInput {
    mob_id: String,
    action: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobSpawnInput {
    mob_id: String,
    profile: String,
    agent_identity: String,
    #[serde(default)]
    initial_message: Option<WireContentInput>,
    #[serde(default)]
    runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default)]
    backend: Option<WireMobBackendKind>,
    #[serde(default)]
    binding: Option<WireRuntimeBinding>,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    context: Option<Value>,
    #[serde(default)]
    additional_instructions: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobSpawnManyInput {
    mob_id: String,
    specs: Vec<MeerkatMobSpawnInputSpec>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobSpawnInputSpec {
    profile: String,
    agent_identity: String,
    #[serde(default)]
    initial_message: Option<WireContentInput>,
    #[serde(default)]
    runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default)]
    backend: Option<WireMobBackendKind>,
    #[serde(default)]
    binding: Option<WireRuntimeBinding>,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    context: Option<Value>,
    #[serde(default)]
    additional_instructions: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobMemberInput {
    mob_id: String,
    agent_identity: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobRespawnInput {
    mob_id: String,
    agent_identity: String,
    #[serde(default)]
    initial_message: Option<WireContentInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobAppendSystemContextInput {
    mob_id: String,
    agent_identity: String,
    text: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobEventsInput {
    mob_id: String,
    #[serde(default)]
    after_cursor: u64,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobFlowRunInput {
    mob_id: String,
    flow_id: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobRunIdInput {
    mob_id: String,
    run_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobWaitKickoffInput {
    mob_id: String,
    #[serde(default)]
    member_ids: Option<Vec<String>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobWaitReadyInput {
    mob_id: String,
    #[serde(default)]
    member_ids: Option<Vec<String>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeerkatMobProfileCreateInput {
    name: String,
    profile: meerkat_mob::Profile,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeerkatMobProfileNameInput {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeerkatMobProfileUpdateInput {
    name: String,
    profile: meerkat_mob::Profile,
    expected_revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeerkatMobProfileDeleteInput {
    name: String,
    expected_revision: u64,
}

const fn default_limit() -> usize {
    100
}

/// W3-H: resolve a `RealtimeChannelTarget` to the concrete bridge session id
/// the RPC query should operate on. `SessionTarget` returns its session id
/// directly; `MobMember` looks up the current binding from the MobMachine's
/// canonical binding map via the mob handle — the single-source-of-truth
/// read path (dogma #1).
async fn resolve_target_session_id(
    state: &Arc<MobMcpState>,
    target: &RealtimeChannelTarget,
) -> Result<meerkat_core::types::SessionId, McpToolError> {
    match target {
        RealtimeChannelTarget::SessionTarget { session_id } => {
            meerkat_core::types::SessionId::parse(session_id)
                .map_err(|err| McpToolError::invalid_params(err.to_string()))
        }
        RealtimeChannelTarget::MobMember {
            mob_id,
            agent_identity,
        } => {
            let dsl_mob_id = meerkat_mob::ids::MobId::from(mob_id.as_str());
            let mob_handle = state
                .handle_for(&dsl_mob_id)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            let identity = meerkat_mob::ids::AgentIdentity::from(agent_identity.as_str());
            mob_handle
                .current_realtime_binding(identity)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?
                .ok_or_else(|| {
                    McpToolError::invalid_params(format!(
                        "mob {mob_id:?} has no realtime binding for identity {agent_identity:?}"
                    ))
                })
        }
    }
}

async fn realtime_status_payload(
    state: &Arc<MobMcpState>,
    params: RealtimeStatusParams,
) -> Result<RealtimeStatusResult, McpToolError> {
    let session_id = resolve_target_session_id(state, &params.target).await?;
    let status = state
        .realtime_session_realtime_channel_status(&session_id)
        .await
        .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
    Ok(RealtimeStatusResult { status })
}

pub fn public_tool_names() -> &'static [&'static str] {
    &[
        "meerkat_realtime_status",
        "meerkat_mob_create",
        "meerkat_mob_list",
        "meerkat_mob_status",
        "meerkat_mob_lifecycle",
        "meerkat_mob_spawn",
        "meerkat_mob_spawn_many",
        "meerkat_mob_retire",
        "meerkat_mob_respawn",
        "meerkat_mob_wire",
        "meerkat_mob_unwire",
        "meerkat_mob_member_send",
        "meerkat_mob_append_system_context",
        "meerkat_mob_events",
        "meerkat_mob_flows",
        "meerkat_mob_flow_run",
        "meerkat_mob_flow_status",
        "meerkat_mob_flow_cancel",
        "meerkat_mob_force_cancel",
        "meerkat_mob_wait_kickoff",
        "meerkat_mob_profile_create",
        "meerkat_mob_profile_get",
        "meerkat_mob_profile_list",
        "meerkat_mob_profile_update",
        "meerkat_mob_profile_delete",
    ]
}

pub fn public_tools_list() -> Vec<Value> {
    vec![
        tool(
            "meerkat_realtime_status",
            "Get product-layer realtime channel status for a target.",
            schema_for!(RealtimeStatusParams),
        ),
        tool(
            "meerkat_mob_create",
            "Create a mob from a typed public definition.",
            schema_for!(MobCreateParams),
        ),
        tool_json(
            "meerkat_mob_list",
            "List active mobs.",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ),
        tool(
            "meerkat_mob_status",
            "Get lifecycle status for one mob.",
            schema_for!(MeerkatMobIdInput),
        ),
        tool(
            "meerkat_mob_lifecycle",
            "Apply a lifecycle action to a mob.",
            schema_for!(MeerkatMobLifecycleInput),
        ),
        tool(
            "meerkat_mob_spawn",
            "Spawn one member into a mob.",
            schema_for!(MeerkatMobSpawnInput),
        ),
        tool(
            "meerkat_mob_spawn_many",
            "Spawn multiple members into a mob.",
            schema_for!(MeerkatMobSpawnManyInput),
        ),
        tool(
            "meerkat_mob_retire",
            "Retire a mob member.",
            schema_for!(MeerkatMobMemberInput),
        ),
        tool(
            "meerkat_mob_respawn",
            "Respawn a mob member with topology restore.",
            schema_for!(MeerkatMobRespawnInput),
        ),
        tool(
            "meerkat_mob_wire",
            "Wire a mob member to a local or external peer.",
            schema_for!(MobWireParams),
        ),
        tool(
            "meerkat_mob_unwire",
            "Remove a mob wiring relationship.",
            schema_for!(MobUnwireParams),
        ),
        tool(
            "meerkat_mob_member_send",
            "Deliver host-owned work to a specific mob member.",
            schema_for!(MobMemberSendParams),
        ),
        tool(
            "meerkat_mob_append_system_context",
            "Stage system context for a specific mob member session.",
            schema_for!(MeerkatMobAppendSystemContextInput),
        ),
        tool(
            "meerkat_mob_events",
            "Read mob event history by cursor.",
            schema_for!(MeerkatMobEventsInput),
        ),
        tool(
            "meerkat_mob_flows",
            "List flows defined for a mob.",
            schema_for!(MeerkatMobIdInput),
        ),
        tool(
            "meerkat_mob_flow_run",
            "Start a mob flow run.",
            schema_for!(MeerkatMobFlowRunInput),
        ),
        tool(
            "meerkat_mob_flow_status",
            "Read status for a mob flow run.",
            schema_for!(MeerkatMobRunIdInput),
        ),
        tool(
            "meerkat_mob_flow_cancel",
            "Cancel a mob flow run.",
            schema_for!(MeerkatMobRunIdInput),
        ),
        tool(
            "meerkat_mob_force_cancel",
            "Force-cancel a mob member.",
            schema_for!(MeerkatMobMemberInput),
        ),
        tool(
            "meerkat_mob_wait_kickoff",
            "Wait for autonomous kickoff turns to complete.",
            schema_for!(MeerkatMobWaitKickoffInput),
        ),
        tool(
            "meerkat_mob_wait_ready",
            "Wait for mob members to become startup-ready for orchestration.",
            schema_for!(MeerkatMobWaitReadyInput),
        ),
        tool_json(
            "meerkat_mob_profile_create",
            "Create a new realm profile for spawning mob members.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Unique profile name"},
                    "profile": {"type": "object", "description": "Profile definition (model, skills, tools, etc.)"}
                },
                "required": ["name", "profile"]
            }),
        ),
        tool_json(
            "meerkat_mob_profile_get",
            "Get a realm profile by name.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Profile name to retrieve"}
                },
                "required": ["name"]
            }),
        ),
        tool_json(
            "meerkat_mob_profile_list",
            "List all realm profiles.",
            json!({ "type": "object", "properties": {}, "required": [] }),
        ),
        tool_json(
            "meerkat_mob_profile_update",
            "Update a realm profile with CAS revision check.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Profile name to update"},
                    "profile": {"type": "object", "description": "Updated profile definition"},
                    "expected_revision": {"type": "integer", "description": "Expected current revision for CAS"}
                },
                "required": ["name", "profile", "expected_revision"]
            }),
        ),
        tool_json(
            "meerkat_mob_profile_delete",
            "Delete a realm profile.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Profile name to delete"},
                    "expected_revision": {"type": "integer", "description": "Expected current revision for CAS"}
                },
                "required": ["name", "expected_revision"]
            }),
        ),
    ]
}

pub fn wrap_public_tool_payload(payload: Value) -> Value {
    let text = serde_json::to_string(&payload).unwrap_or_default();
    json!({
        "content": [{
            "type": "text",
            "text": text
        }]
    })
}

pub async fn handle_public_tools_call(
    state: &Arc<MobMcpState>,
    name: &str,
    arguments: &Value,
) -> Result<Value, McpToolError> {
    match name {
        "meerkat_realtime_status" => {
            let input: RealtimeStatusParams = parse_args(arguments)?;
            Ok(json!(realtime_status_payload(state, input).await?))
        }
        "meerkat_mob_create" => {
            let input: MobCreateParams = parse_args(arguments)?;
            let definition = decode_public_mob_definition(input.definition).map_err(|error| {
                McpToolError::invalid_params(format!("invalid mob definition: {error}"))
            })?;
            let mob_id = state
                .mob_create_definition(definition)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "mob_id": mob_id }))
        }
        "meerkat_mob_list" => {
            let mobs = state
                .mob_list()
                .await
                .into_iter()
                .map(|(mob_id, status)| json!({"mob_id": mob_id, "status": status.to_string()}))
                .collect::<Vec<_>>();
            Ok(json!({ "mobs": mobs }))
        }
        "meerkat_mob_status" => {
            let input: MeerkatMobIdInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let status = state
                .mob_status(&mob_id)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "mob_id": mob_id, "status": status.to_string() }))
        }
        "meerkat_mob_lifecycle" => {
            let input: MeerkatMobLifecycleInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            // `destroy` returns a structured MobDestroyReport; the public
            // MCP surface projects it into the response body. Other actions
            // stay `()` on success.
            let destroy_report = match input.action.as_str() {
                "stop" => {
                    state
                        .mob_stop(&mob_id)
                        .await
                        .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
                    None
                }
                "resume" => {
                    state
                        .mob_resume(&mob_id)
                        .await
                        .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
                    None
                }
                "complete" => {
                    state
                        .mob_complete(&mob_id)
                        .await
                        .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
                    None
                }
                "reset" => {
                    state
                        .mob_reset(&mob_id)
                        .await
                        .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
                    None
                }
                "destroy" => {
                    let report = state
                        .mob_destroy(&mob_id)
                        .await
                        .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
                    Some(report)
                }
                other => {
                    return Err(McpToolError::invalid_params(format!(
                        "unknown lifecycle action: {other}"
                    )));
                }
            };
            let mut body = json!({ "mob_id": mob_id, "action": input.action, "ok": true });
            if let Some(report) = destroy_report
                && let Some(obj) = body.as_object_mut()
            {
                let report_value = serde_json::to_value(&report).map_err(|err| {
                    McpToolError::internal(format!("destroy report serialize: {err}"))
                })?;
                obj.insert("destroy_report".to_string(), report_value);
            }
            Ok(body)
        }
        "meerkat_mob_spawn" => {
            let input: MeerkatMobSpawnInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let spec = build_spawn_spec(
                input.profile,
                input.agent_identity.clone(),
                input.initial_message,
                input.runtime_mode,
                input.backend,
                input.binding,
                input.labels,
                input.context,
                input.additional_instructions,
            )?;
            let spawn_result = state
                .mob_spawn_spec(&mob_id, spec)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            let mut payload = spawn_result_payload(&mob_id, &spawn_result);
            payload["mob_id"] = json!(mob_id);
            Ok(payload)
        }
        "meerkat_mob_spawn_many" => {
            let input: MeerkatMobSpawnManyInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let specs = input
                .specs
                .into_iter()
                .map(|spec| {
                    build_spawn_spec(
                        spec.profile,
                        spec.agent_identity,
                        spec.initial_message,
                        spec.runtime_mode,
                        spec.backend,
                        spec.binding,
                        spec.labels,
                        spec.context,
                        spec.additional_instructions,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let results = state
                .mob_spawn_many(&mob_id, specs)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({
                "results": results.into_iter().map(|result: Result<meerkat_mob::SpawnResult, meerkat_mob::MobError>| match result {
                    Ok(spawn_result) => {
                        let mut payload = spawn_result_payload(&mob_id, &spawn_result);
                        payload["ok"] = json!(true);
                        payload
                    }
                    Err(error) => json!({
                        "ok": false,
                        "error": error.to_string(),
                    }),
                }).collect::<Vec<_>>()
            }))
        }
        "meerkat_mob_retire" => {
            let input: MeerkatMobMemberInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            state
                .mob_retire(
                    &mob_id,
                    meerkat_mob::AgentIdentity::from(input.agent_identity.as_str()),
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "retired": true }))
        }
        "meerkat_mob_respawn" => {
            let input: MeerkatMobRespawnInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let initial_message = input
                .initial_message
                .map(content_input_from_wire)
                .transpose()?;
            match state
                .mob_respawn(
                    &mob_id,
                    meerkat_mob::AgentIdentity::from(input.agent_identity.as_str()),
                    initial_message,
                )
                .await
            {
                Ok(receipt) => Ok(json!({
                    "status": "completed",
                    "receipt": respawn_receipt_payload(&mob_id, &receipt),
                })),
                Err(meerkat_mob::MobRespawnError::TopologyRestoreFailed {
                    receipt,
                    failed_peer_ids,
                }) => Ok(json!({
                    "status": "topology_restore_failed",
                    "receipt": respawn_receipt_payload(&mob_id, &receipt),
                    "failed_peer_ids": failed_peer_ids
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect::<Vec<_>>(),
                })),
                Err(err) => Err(McpToolError::invalid_params(err.to_string())),
            }
        }
        "meerkat_mob_wire" => {
            let input: MobWireParams = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let target = peer_target_from_wire(input.peer)?;
            state
                .mob_wire(
                    &mob_id,
                    meerkat_mob::AgentIdentity::from(input.member.as_str()),
                    target,
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "wired": true }))
        }
        "meerkat_mob_unwire" => {
            let input: MobUnwireParams = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let target = peer_target_from_wire(input.peer)?;
            state
                .mob_unwire(
                    &mob_id,
                    meerkat_mob::AgentIdentity::from(input.member.as_str()),
                    target,
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "unwired": true }))
        }
        "meerkat_mob_member_send" => {
            let input: MobMemberSendParams = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let content = content_input_from_wire(input.content)?;
            let receipt = state
                .mob_member_send(
                    &mob_id,
                    meerkat_mob::AgentIdentity::from(input.agent_identity.as_str()),
                    content,
                    input.handling_mode.into(),
                    input.render_metadata.map(Into::into),
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({
                "mob_id": mob_id,
                "agent_identity": receipt.identity,
                "member_ref": meerkat_contracts::WireMemberRef::encode(
                    mob_id.as_str(),
                    receipt.identity.as_ref(),
                ),
                "handling_mode": input.handling_mode,
            }))
        }
        "meerkat_mob_append_system_context" => {
            let input: MeerkatMobAppendSystemContextInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let agent_identity = meerkat_mob::AgentIdentity::from(input.agent_identity.as_str());
            let (_bridge_session_id, result) = state
                .mob_append_system_context(
                    &mob_id,
                    &agent_identity,
                    meerkat_core::service::AppendSystemContextRequest {
                        text: input.text,
                        source: input.source,
                        idempotency_key: input.idempotency_key,
                    },
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({
                "mob_id": mob_id,
                "agent_identity": agent_identity,
                "status": result.status,
            }))
        }
        "meerkat_mob_events" => {
            let input: MeerkatMobEventsInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let events = state
                .mob_events(&mob_id, input.after_cursor, input.limit)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "events": events }))
        }
        "meerkat_mob_flows" => {
            let input: MeerkatMobIdInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let flows = state
                .mob_list_flows(&mob_id)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "mob_id": mob_id, "flows": flows }))
        }
        "meerkat_mob_flow_run" => {
            let input: MeerkatMobFlowRunInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let run_id = state
                .mob_run_flow(
                    &mob_id,
                    meerkat_mob::FlowId::from(input.flow_id.as_str()),
                    input.params,
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "run_id": run_id }))
        }
        "meerkat_mob_flow_status" => {
            let input: MeerkatMobRunIdInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let run_id = parse_run_id(&input.run_id)?;
            let run = state
                .mob_flow_status(&mob_id, run_id)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "run": run }))
        }
        "meerkat_mob_flow_cancel" => {
            let input: MeerkatMobRunIdInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let run_id = parse_run_id(&input.run_id)?;
            state
                .mob_cancel_flow(&mob_id, run_id)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "canceled": true }))
        }
        "meerkat_mob_force_cancel" => {
            let input: MeerkatMobMemberInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            state
                .mob_force_cancel(
                    &mob_id,
                    meerkat_mob::AgentIdentity::from(input.agent_identity.as_str()),
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "cancelled": true }))
        }
        "meerkat_mob_wait_kickoff" => {
            let input: MeerkatMobWaitKickoffInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let member_ids = input.member_ids.map(|ids| {
                ids.into_iter()
                    .map(|member_id| meerkat_mob::AgentIdentity::from(member_id.as_str()))
                    .collect::<Vec<_>>()
            });
            let members = state
                .mob_wait_kickoff(&mob_id, member_ids, input.timeout_ms)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "members": members }))
        }
        "meerkat_mob_wait_ready" => {
            let input: MeerkatMobWaitReadyInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let member_ids = input.member_ids.map(|ids| {
                ids.into_iter()
                    .map(|member_id| meerkat_mob::AgentIdentity::from(member_id.as_str()))
                    .collect::<Vec<_>>()
            });
            let members = state
                .mob_wait_ready(&mob_id, member_ids, input.timeout_ms)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "members": members }))
        }
        "meerkat_mob_profile_create" => {
            let input: MeerkatMobProfileCreateInput = parse_args(arguments)?;
            let stored = state
                .realm_profile_create(&input.name, &input.profile)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!(stored))
        }
        "meerkat_mob_profile_get" => {
            let input: MeerkatMobProfileNameInput = parse_args(arguments)?;
            match state.realm_profile_get(&input.name).await {
                Ok(Some(stored)) => Ok(json!(stored)),
                Ok(None) => Ok(json!({"not_found": true, "name": input.name})),
                Err(err) => Err(McpToolError::invalid_params(err.to_string())),
            }
        }
        "meerkat_mob_profile_list" => {
            let profiles = state
                .realm_profile_list()
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({"profiles": profiles}))
        }
        "meerkat_mob_profile_update" => {
            let input: MeerkatMobProfileUpdateInput = parse_args(arguments)?;
            let stored = state
                .realm_profile_update(&input.name, &input.profile, input.expected_revision)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!(stored))
        }
        "meerkat_mob_profile_delete" => {
            let input: MeerkatMobProfileDeleteInput = parse_args(arguments)?;
            let deleted = state
                .realm_profile_delete(&input.name, input.expected_revision)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({"name": deleted.name, "deleted_revision": deleted.revision}))
        }
        _ => Err(McpToolError::method_not_found(format!(
            "Method not found: {name}"
        ))),
    }
}

fn tool(name: &str, description: &str, schema: schemars::Schema) -> Value {
    tool_json(
        name,
        description,
        serde_json::to_value(schema).unwrap_or_else(|_| json!({ "type": "object" })),
    )
}

fn tool_json(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn parse_args<T: DeserializeOwned>(arguments: &Value) -> Result<T, McpToolError> {
    serde_json::from_value(arguments.clone())
        .map_err(|error| McpToolError::invalid_params(format!("invalid arguments: {error}")))
}

fn parse_mob_id(raw: &str) -> Result<meerkat_mob::MobId, McpToolError> {
    if raw.trim().is_empty() {
        return Err(McpToolError::invalid_params("mob_id must not be empty"));
    }
    Ok(meerkat_mob::MobId::from(raw))
}

fn parse_run_id(raw: &str) -> Result<meerkat_mob::RunId, McpToolError> {
    raw.parse::<meerkat_mob::RunId>()
        .map_err(|err| McpToolError::invalid_params(format!("invalid run_id: {err}")))
}

fn content_input_from_wire(
    input: WireContentInput,
) -> Result<meerkat_core::types::ContentInput, McpToolError> {
    meerkat_core::types::ContentInput::try_from(input).map_err(McpToolError::invalid_params)
}

fn peer_target_from_wire(peer: MobPeerTarget) -> Result<meerkat_mob::PeerTarget, McpToolError> {
    match peer {
        MobPeerTarget::Local(member_id) => Ok(meerkat_mob::PeerTarget::Local(member_id.into())),
        MobPeerTarget::External(spec) => crate::trusted_peer_descriptor_from_wire_spec(spec)
            .map(meerkat_mob::PeerTarget::External)
            .map_err(McpToolError::invalid_params),
    }
}

fn runtime_mode_from_wire(mode: WireMobRuntimeMode) -> meerkat_mob::MobRuntimeMode {
    match mode {
        WireMobRuntimeMode::AutonomousHost => meerkat_mob::MobRuntimeMode::AutonomousHost,
        WireMobRuntimeMode::TurnDriven => meerkat_mob::MobRuntimeMode::TurnDriven,
    }
}

fn backend_kind_from_wire(kind: WireMobBackendKind) -> meerkat_mob::MobBackendKind {
    match kind {
        WireMobBackendKind::Session => meerkat_mob::MobBackendKind::Session,
        WireMobBackendKind::External => meerkat_mob::MobBackendKind::External,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_spawn_spec(
    profile: String,
    agent_identity: String,
    initial_message: Option<WireContentInput>,
    runtime_mode: Option<WireMobRuntimeMode>,
    backend: Option<WireMobBackendKind>,
    binding: Option<WireRuntimeBinding>,
    labels: Option<BTreeMap<String, String>>,
    context: Option<Value>,
    additional_instructions: Option<Vec<String>>,
) -> Result<meerkat_mob::SpawnMemberSpec, McpToolError> {
    let mut spec = meerkat_mob::SpawnMemberSpec::new(profile.as_str(), agent_identity.as_str());
    spec.initial_message = initial_message.map(content_input_from_wire).transpose()?;
    spec.runtime_mode = runtime_mode.map(runtime_mode_from_wire);
    // Resolve binding: explicit binding takes precedence over legacy backend tag.
    // Conflicting backend + binding is rejected.
    spec.binding = match (binding, backend) {
        (Some(wb), None) => Some(runtime_binding_from_wire(wb)?),
        (Some(wb), Some(bk)) => {
            let resolved = runtime_binding_from_wire(wb)?;
            if resolved.kind() != backend_kind_from_wire(bk) {
                return Err(McpToolError::invalid_params(
                    "conflicting 'backend' and 'binding' fields",
                ));
            }
            Some(resolved)
        }
        (None, Some(bk)) => {
            let kind = backend_kind_from_wire(bk);
            match kind {
                meerkat_mob::MobBackendKind::Session => Some(meerkat_mob::RuntimeBinding::Session),
                meerkat_mob::MobBackendKind::External => {
                    // Bare external without binding — let the actor reject it
                    // with a clear error about requiring RuntimeBinding.
                    spec.backend = Some(kind);
                    None
                }
            }
        }
        (None, None) => None,
    };
    spec.labels = labels;
    spec.context = context;
    spec.additional_instructions = additional_instructions;
    Ok(spec)
}

fn runtime_binding_from_wire(
    wb: WireRuntimeBinding,
) -> Result<meerkat_mob::RuntimeBinding, McpToolError> {
    match wb {
        WireRuntimeBinding::Session => Ok(meerkat_mob::RuntimeBinding::Session),
        WireRuntimeBinding::External {
            address,
            bootstrap_token,
            identity,
        } => {
            let resolved = identity
                .resolve()
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(meerkat_mob::RuntimeBinding::External {
                peer_id: resolved.peer_id.to_string(),
                address,
                bootstrap_token,
                pubkey: Some(resolved.pubkey),
            })
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::MobMcpState;

    const ED25519_PUBLIC_KEY_7: &str = "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    const ED25519_PUBLIC_KEY_ZERO: &str = "ed25519:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    fn canonical_external_peer_target() -> meerkat_contracts::MobPeerTarget {
        serde_json::from_value(serde_json::json!({
            "external": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": ED25519_PUBLIC_KEY_7
                }
            }
        }))
        .expect("canonical external peer target should deserialize")
    }

    fn external_peer_target(public_key: &str) -> meerkat_contracts::MobPeerTarget {
        meerkat_contracts::MobPeerTarget::External(meerkat_contracts::WireTrustedPeerSpec {
            name: "external-worker".to_string(),
            address: "inproc://external-worker".to_string(),
            identity: meerkat_contracts::WireTrustedPeerIdentity::Ed25519PublicKey {
                public_key: public_key.to_string(),
            },
        })
    }

    fn external_runtime_binding(public_key: &str) -> meerkat_contracts::WireRuntimeBinding {
        serde_json::from_value(serde_json::json!({
            "kind": "external",
            "address": "inproc://external-worker",
            "identity": {
                "kind": "ed25519_public_key",
                "public_key": public_key
            }
        }))
        .expect("canonical external runtime binding should deserialize")
    }

    #[test]
    fn public_mcp_wire_accepts_canonical_external_peer_identity() {
        let target = peer_target_from_wire(canonical_external_peer_target())
            .expect("canonical external peer identity should resolve");

        let meerkat_mob::PeerTarget::External(descriptor) = target else {
            panic!("canonical external peer should resolve to an external descriptor");
        };
        let pubkey = [7u8; 32];
        assert_eq!(descriptor.name.as_str(), "external-worker");
        assert_eq!(descriptor.address.to_string(), "inproc://external-worker");
        assert_eq!(descriptor.pubkey, pubkey);
        assert_eq!(
            descriptor.peer_id,
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey)
        );
    }

    #[test]
    fn public_mcp_wire_rejects_external_peer_raw_peer_id_shape() {
        let err = serde_json::from_value::<meerkat_contracts::MobPeerTarget>(serde_json::json!({
            "external": {
                "name": "external-worker",
                "peer_id": meerkat_core::comms::PeerId::from_ed25519_pubkey(&[7u8; 32]).to_string(),
                "address": "inproc://external-worker",
                "pubkey": vec![7u8; 32]
            }
        }))
        .expect_err("raw peer_id/pubkey external peer shape must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("peer_id") || msg.contains("identity"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn public_mcp_wire_rejects_external_peer_missing_pubkey_material() {
        let err = serde_json::from_value::<meerkat_contracts::MobPeerTarget>(serde_json::json!({
            "external": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key"
                }
            }
        }))
        .expect_err("external peer identity must not default missing pubkey material");

        let msg = err.to_string();
        assert!(
            msg.contains("public_key") || msg.contains("identity"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn public_mcp_wire_rejects_external_peer_zero_pubkey() {
        let err = peer_target_from_wire(external_peer_target(ED25519_PUBLIC_KEY_ZERO))
            .expect_err("zero pubkey external peers must be rejected");

        assert!(
            err.message.contains("public_key") || err.message.contains("non-zero"),
            "expected pubkey validation error, got: {}",
            err.message
        );
    }

    #[test]
    fn public_mcp_wire_rejects_invalid_external_peer_identity() {
        let err = peer_target_from_wire(external_peer_target("not-a-typed-public-key"))
            .expect_err("invalid canonical external peer identity must be rejected");

        assert!(
            err.message.contains("public_key") || err.message.contains("ed25519"),
            "expected typed identity validation error, got: {}",
            err.message
        );
    }

    #[test]
    fn public_mcp_spawn_binding_resolves_canonical_external_identity() {
        let binding = runtime_binding_from_wire(external_runtime_binding(ED25519_PUBLIC_KEY_7))
            .expect("canonical runtime binding identity should resolve");

        let meerkat_mob::RuntimeBinding::External {
            peer_id,
            address,
            pubkey,
            ..
        } = binding
        else {
            panic!("expected external runtime binding");
        };
        let expected_pubkey = [7u8; 32];
        assert_eq!(address, "inproc://external-worker");
        assert_eq!(
            peer_id,
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&expected_pubkey).to_string()
        );
        assert_eq!(pubkey, Some(expected_pubkey));
    }

    #[test]
    fn public_mcp_spawn_binding_rejects_raw_peer_id_shape() {
        let err =
            serde_json::from_value::<meerkat_contracts::WireRuntimeBinding>(serde_json::json!({
                "kind": "external",
                "peer_id": meerkat_core::comms::PeerId::from_ed25519_pubkey(&[7u8; 32]).to_string(),
                "address": "inproc://external-worker",
                "pubkey": vec![7u8; 32]
            }))
            .expect_err("raw peer_id/pubkey external runtime binding shape must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("peer_id") || msg.contains("identity"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn public_mcp_spawn_binding_rejects_zero_pubkey() {
        let err = runtime_binding_from_wire(external_runtime_binding(ED25519_PUBLIC_KEY_ZERO))
            .expect_err("zero pubkey external runtime binding must be rejected");

        assert!(
            err.message.contains("public_key") || err.message.contains("non-zero"),
            "expected pubkey validation error, got: {}",
            err.message
        );
    }

    #[allow(dead_code)]
    async fn spawn_realtime_open_info_stub(
        open_info: Value,
    ) -> (
        String,
        tokio::sync::oneshot::Receiver<Value>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind realtime rpc stub");
        let addr = listener
            .local_addr()
            .expect("realtime rpc stub local addr")
            .to_string();
        let (captured_tx, captured_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept realtime rpc stub");
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = tokio::io::BufReader::new(read_half).lines();
            let mut captured_tx = Some(captured_tx);
            while let Some(line) = reader
                .next_line()
                .await
                .expect("read realtime rpc stub line")
            {
                let value: Value =
                    serde_json::from_str(&line).expect("parse realtime rpc stub request");
                let id = value["id"].clone();
                let method = value["method"]
                    .as_str()
                    .expect("realtime rpc stub request method");
                let response = match method {
                    "initialize" => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "server": "realtime-stub",
                        }
                    }),
                    "realtime/open_info" => {
                        if let Some(tx) = captured_tx.take() {
                            let _ = tx.send(value["params"].clone());
                        }
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": open_info,
                        })
                    }
                    other => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": format!("unexpected method: {other}"),
                        }
                    }),
                };
                write_half
                    .write_all(response.to_string().as_bytes())
                    .await
                    .expect("write realtime rpc stub response");
                write_half
                    .write_all(b"\n")
                    .await
                    .expect("terminate realtime rpc stub response");
                write_half
                    .flush()
                    .await
                    .expect("flush realtime rpc stub response");
                if method == "realtime/open_info" {
                    break;
                }
            }
        });
        (addr, captured_rx, task)
    }

    #[allow(dead_code)]
    fn live_test_definition(mob_id: &str) -> meerkat_mob::MobDefinition {
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(meerkat_mob::profile::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::profile::ToolConfig {
                    comms: true,
                    ..meerkat_mob::profile::ToolConfig::default()
                },
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );
        let mut definition = meerkat_mob::MobDefinition::explicit(meerkat_mob::MobId::from(mob_id));
        definition.profiles = profiles;
        definition
    }

    #[tokio::test]
    async fn public_tools_reject_raw_dispatcher_tool_names() {
        let state = MobMcpState::new_in_memory();
        let err = handle_public_tools_call(&state, "mob_create", &json!({}))
            .await
            .expect_err("raw mob_create must stay unavailable on public MCP surface");
        assert_eq!(err.code, -32601);
    }

    #[tokio::test]
    async fn public_tools_create_mob_and_reject_internal_fields() {
        let state = MobMcpState::new_in_memory();

        let created = handle_public_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": "typed-public-mob",
                    "profiles": {
                        "lead": {
                            "model": "gpt-5.4",
                            "tools": {"comms": true}
                        }
                    }
                }
            }),
        )
        .await
        .expect("typed public create");
        assert_eq!(created["mob_id"], "typed-public-mob");

        let err = handle_public_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": "bad-mob",
                    "profiles": {
                        "lead": {
                            "model": "gpt-5.4",
                            "tools": {
                                "comms": true,
                                "rust_bundles": ["internal-only"]
                            }
                        }
                    }
                }
            }),
        )
        .await
        .expect_err("internal profile tool bundles must be rejected");
        assert_eq!(err.code, -32602);

        let names: Vec<_> = public_tools_list()
            .into_iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        assert!(names.contains(&"meerkat_mob_create".to_string()));
        assert!(!names.contains(&"mob_create".to_string()));
    }
}
