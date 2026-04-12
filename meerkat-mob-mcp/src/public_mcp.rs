use crate::{McpToolError, MobMcpState, decode_public_mob_definition};
use meerkat_contracts::{
    MobCreateParams, MobMemberSendParams, MobPeerTarget, MobUnwireParams, MobWireParams,
    WireContentInput, WireMobBackendKind, WireMobRuntimeMode, WireRuntimeBinding,
};
use meerkat_core::error::invalid_session_id_message;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::sync::Arc;

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
    meerkat_id: String,
    #[serde(default)]
    initial_message: Option<WireContentInput>,
    #[serde(default)]
    runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default)]
    backend: Option<WireMobBackendKind>,
    #[serde(default)]
    binding: Option<WireRuntimeBinding>,
    #[serde(default)]
    resume_session_id: Option<String>,
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
    meerkat_id: String,
    #[serde(default)]
    initial_message: Option<WireContentInput>,
    #[serde(default)]
    runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default)]
    backend: Option<WireMobBackendKind>,
    #[serde(default)]
    binding: Option<WireRuntimeBinding>,
    #[serde(default)]
    resume_session_id: Option<String>,
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
    meerkat_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobRespawnInput {
    mob_id: String,
    meerkat_id: String,
    #[serde(default)]
    initial_message: Option<WireContentInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobAppendSystemContextInput {
    mob_id: String,
    meerkat_id: String,
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

pub fn public_tool_names() -> &'static [&'static str] {
    &[
        "meerkat_mob_create",
        "meerkat_mob_list",
        "meerkat_mob_status",
        "meerkat_mob_lifecycle",
        "meerkat_mob_members",
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
        "meerkat_mob_member_status",
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
            "meerkat_mob_members",
            "List members in a mob roster.",
            schema_for!(MeerkatMobIdInput),
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
            "meerkat_mob_member_status",
            "Read live status for a mob member.",
            schema_for!(MeerkatMobMemberInput),
        ),
        tool(
            "meerkat_mob_wait_kickoff",
            "Wait for autonomous kickoff turns to complete.",
            schema_for!(MeerkatMobWaitKickoffInput),
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
            match input.action.as_str() {
                "stop" => state.mob_stop(&mob_id).await,
                "resume" => state.mob_resume(&mob_id).await,
                "complete" => state.mob_complete(&mob_id).await,
                "reset" => state.mob_reset(&mob_id).await,
                "destroy" => state.mob_destroy(&mob_id).await,
                other => {
                    return Err(McpToolError::invalid_params(format!(
                        "unknown lifecycle action: {other}"
                    )));
                }
            }
            .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "mob_id": mob_id, "action": input.action, "ok": true }))
        }
        "meerkat_mob_members" => {
            let input: MeerkatMobIdInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let members = state
                .mob_list_members(&mob_id)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "mob_id": mob_id, "members": members }))
        }
        "meerkat_mob_spawn" => {
            let input: MeerkatMobSpawnInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let spec = build_spawn_spec(
                input.profile,
                input.meerkat_id.clone(),
                input.initial_message,
                input.runtime_mode,
                input.backend,
                input.binding,
                input.resume_session_id,
                input.labels,
                input.context,
                input.additional_instructions,
            )?;
            let member_ref = state
                .mob_spawn_spec(&mob_id, spec)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({
                "mob_id": mob_id,
                "meerkat_id": input.meerkat_id,
                "member_ref": member_ref,
                "session_id": member_ref.session_id(),
            }))
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
                        spec.meerkat_id,
                        spec.initial_message,
                        spec.runtime_mode,
                        spec.backend,
                        spec.binding,
                        spec.resume_session_id,
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
                "results": results.into_iter().map(|result| match result {
                    Ok(member_ref) => json!({
                        "ok": true,
                        "member_ref": member_ref,
                        "session_id": member_ref.session_id().map(std::string::ToString::to_string),
                    }),
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
                    meerkat_mob::MeerkatId::from(input.meerkat_id.as_str()),
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
                    meerkat_mob::MeerkatId::from(input.meerkat_id.as_str()),
                    initial_message,
                )
                .await
            {
                Ok(receipt) => Ok(json!({
                    "status": "completed",
                    "receipt": receipt,
                })),
                Err(meerkat_mob::MobRespawnError::TopologyRestoreFailed {
                    receipt,
                    failed_peer_ids,
                }) => Ok(json!({
                    "status": "topology_restore_failed",
                    "receipt": receipt,
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
            state
                .mob_wire(
                    &mob_id,
                    meerkat_mob::MeerkatId::from(input.member.as_str()),
                    peer_target_from_wire(input.peer),
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "wired": true }))
        }
        "meerkat_mob_unwire" => {
            let input: MobUnwireParams = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            state
                .mob_unwire(
                    &mob_id,
                    meerkat_mob::MeerkatId::from(input.member.as_str()),
                    peer_target_from_wire(input.peer),
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
                    meerkat_mob::MeerkatId::from(input.meerkat_id.as_str()),
                    content,
                    input.handling_mode.into(),
                    input.render_metadata.map(Into::into),
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({
                "mob_id": mob_id,
                "member_id": receipt.member_id,
                "session_id": receipt.session_id,
                "handling_mode": input.handling_mode,
            }))
        }
        "meerkat_mob_append_system_context" => {
            let input: MeerkatMobAppendSystemContextInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let meerkat_id = meerkat_mob::MeerkatId::from(input.meerkat_id.as_str());
            let (session_id, result) = state
                .mob_append_system_context(
                    &mob_id,
                    &meerkat_id,
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
                "meerkat_id": meerkat_id,
                "session_id": session_id,
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
                    meerkat_mob::MeerkatId::from(input.meerkat_id.as_str()),
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!({ "cancelled": true }))
        }
        "meerkat_mob_member_status" => {
            let input: MeerkatMobMemberInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let snapshot = state
                .mob_member_status(
                    &mob_id,
                    &meerkat_mob::MeerkatId::from(input.meerkat_id.as_str()),
                )
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!(snapshot))
        }
        "meerkat_mob_wait_kickoff" => {
            let input: MeerkatMobWaitKickoffInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let member_ids = input.member_ids.map(|ids| {
                ids.into_iter()
                    .map(|member_id| meerkat_mob::MeerkatId::from(member_id.as_str()))
                    .collect::<Vec<_>>()
            });
            let members = state
                .mob_wait_kickoff(&mob_id, member_ids, input.timeout_ms)
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

fn peer_target_from_wire(peer: MobPeerTarget) -> meerkat_mob::PeerTarget {
    match peer {
        MobPeerTarget::Local(member_id) => {
            meerkat_mob::PeerTarget::Local(meerkat_mob::MeerkatId::from(member_id.as_str()))
        }
        MobPeerTarget::External(spec) => {
            meerkat_mob::PeerTarget::External(meerkat_core::comms::TrustedPeerSpec {
                name: spec.name,
                peer_id: spec.peer_id,
                address: spec.address,
            })
        }
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
    meerkat_id: String,
    initial_message: Option<WireContentInput>,
    runtime_mode: Option<WireMobRuntimeMode>,
    backend: Option<WireMobBackendKind>,
    binding: Option<WireRuntimeBinding>,
    resume_session_id: Option<String>,
    labels: Option<BTreeMap<String, String>>,
    context: Option<Value>,
    additional_instructions: Option<Vec<String>>,
) -> Result<meerkat_mob::SpawnMemberSpec, McpToolError> {
    let mut spec = meerkat_mob::SpawnMemberSpec::new(profile.as_str(), meerkat_id.as_str());
    spec.initial_message = initial_message.map(content_input_from_wire).transpose()?;
    spec.runtime_mode = runtime_mode.map(runtime_mode_from_wire);
    // Resolve binding: explicit binding takes precedence over legacy backend tag.
    // Conflicting backend + binding is rejected.
    spec.binding = match (binding, backend) {
        (Some(wb), None) => Some(runtime_binding_from_wire(wb)),
        (Some(wb), Some(bk)) => {
            let resolved = runtime_binding_from_wire(wb);
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
    if let Some(session_id) = resume_session_id {
        let parsed = meerkat_core::types::SessionId::parse(&session_id)
            .map_err(invalid_session_id_message)
            .map_err(McpToolError::invalid_params)?;
        spec = spec.with_resume_session_id(parsed);
    }
    Ok(spec)
}

fn runtime_binding_from_wire(wb: WireRuntimeBinding) -> meerkat_mob::RuntimeBinding {
    match wb {
        WireRuntimeBinding::Session => meerkat_mob::RuntimeBinding::Session,
        WireRuntimeBinding::External { peer_id, address } => {
            meerkat_mob::RuntimeBinding::External { peer_id, address }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::MobMcpState;

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
