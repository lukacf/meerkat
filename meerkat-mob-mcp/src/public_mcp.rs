#![allow(unused_imports)]

use crate::{McpToolError, MobMcpState, decode_public_mob_definition};
use meerkat_contracts::{
    MobCreateParams, MobLifecycleParams, MobLifecycleResult, MobMemberSendParams, WireContentInput,
    WireMemberRef, WireMobBackendKind, WireMobRuntimeMode, WireRuntimeBinding,
    WireTrustedPeerIdentity,
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

fn spawn_result_payload(
    mob_id: &meerkat_mob::MobId,
    result: &meerkat_mob::SpawnResult,
) -> meerkat_contracts::MobSpawnResult {
    let identity = result.agent_identity.to_string();
    meerkat_contracts::MobSpawnResult {
        mob_id: mob_id.to_string(),
        agent_identity: identity.clone(),
        member_ref: meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity),
    }
}

fn respawn_receipt_payload(
    mob_id: &meerkat_mob::MobId,
    receipt: &meerkat_mob::MemberRespawnReceipt,
) -> meerkat_contracts::MobRespawnReceipt {
    let identity = receipt.identity.to_string();
    meerkat_contracts::MobRespawnReceipt {
        identity: identity.clone(),
        member_ref: meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobIdInput {
    mob_id: String,
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
struct MeerkatMobWireInput {
    mob_id: String,
    member: String,
    peer: MeerkatMobWirePeerTarget,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobWireMembersBatchInput {
    mob_id: String,
    edges: Vec<MeerkatMobWireMembersBatchEdgeInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobWireMembersBatchEdgeInput {
    a: String,
    b: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobUnwireInput {
    mob_id: String,
    member: String,
    peer: MeerkatMobUnwirePeerTarget,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MeerkatMobWirePeerTarget {
    Local(String),
    ExternalBinding(MeerkatExternalPeerBindingInput),
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MeerkatMobUnwirePeerTarget {
    Local(String),
    External(MeerkatExternalPeerHandleInput),
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatExternalPeerBindingInput {
    name: String,
    address: String,
    identity: WireTrustedPeerIdentity,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatExternalPeerHandleInput {
    name: String,
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
struct MeerkatMobRunInput {
    mob_id: String,
    #[serde(default)]
    flow_id: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobProfileCreateInput {
    /// Unique profile name.
    name: String,
    /// Profile definition (model, skills, tools, etc.).
    #[schemars(with = "serde_json::Value")]
    profile: meerkat_mob::Profile,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobProfileNameInput {
    /// Profile name to retrieve.
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobProfileUpdateInput {
    /// Profile name to update.
    name: String,
    /// Updated profile definition.
    #[schemars(with = "serde_json::Value")]
    profile: meerkat_mob::Profile,
    /// Expected current revision for CAS.
    expected_revision: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobProfileDeleteInput {
    /// Profile name to delete.
    name: String,
    /// Expected current revision for CAS.
    expected_revision: u64,
}

/// Schema source for tools that accept no parameters.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeerkatMobNoParamsInput {}

const fn default_limit() -> usize {
    100
}

/// Single source of truth for every public MCP tool exposed by this surface.
///
/// Each entry carries the advertised name, its description, and a function that
/// produces the `inputSchema` JSON. Every entry's schema derives from
/// `schema_for!` (via [`typed_schema`]) over a typed input struct; no entry
/// carries a hand-authored `json!` literal (pinned by the
/// `public_tool_schemas_are_typed_not_handwritten` gate). Both surfaces —
/// [`public_tool_names`] (the routing roster gated on by the unified MCP server)
/// and [`public_tools_list`] (the advertised catalog) — are derived from this
/// table so they cannot drift.
/// The dispatch match in [`handle_public_tools_call`] is pinned to this table by
/// the `public_tool_surfaces_have_no_drift` parity test via `DISPATCH_TOOL_NAMES`.
struct PublicTool {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
}

fn typed_schema<T: JsonSchema>() -> Value {
    // K1: ONE infallible schema generator (the core schema module owns the
    // tool-input schema contract) — no fail-open `{"type": "object"}` null
    // schema fallback.
    meerkat_core::schema::tool_input_schema_for::<T>()
}

static PUBLIC_TOOLS: &[PublicTool] = &[
    PublicTool {
        name: "meerkat_mob_create",
        description: "Create a mob from a typed public definition.",
        schema: typed_schema::<MobCreateParams>,
    },
    PublicTool {
        name: "meerkat_mob_list",
        description: "List active mobs.",
        schema: typed_schema::<MeerkatMobNoParamsInput>,
    },
    PublicTool {
        name: "meerkat_mob_status",
        description: "Get lifecycle status for one mob.",
        schema: typed_schema::<MeerkatMobIdInput>,
    },
    PublicTool {
        name: "meerkat_mob_lifecycle",
        description: "Apply a lifecycle action to a mob.",
        schema: typed_schema::<MobLifecycleParams>,
    },
    PublicTool {
        name: "meerkat_mob_spawn",
        description: "Spawn one member into a mob.",
        schema: typed_schema::<MeerkatMobSpawnInput>,
    },
    PublicTool {
        name: "meerkat_mob_spawn_many",
        description: "Spawn multiple members into a mob.",
        schema: typed_schema::<MeerkatMobSpawnManyInput>,
    },
    PublicTool {
        name: "meerkat_mob_retire",
        description: "Retire a mob member.",
        schema: typed_schema::<MeerkatMobMemberInput>,
    },
    PublicTool {
        name: "meerkat_mob_respawn",
        description: "Respawn a mob member with topology restore.",
        schema: typed_schema::<MeerkatMobRespawnInput>,
    },
    PublicTool {
        name: "meerkat_mob_wire",
        description: "Wire a mob member to a local peer or typed external binding.",
        schema: typed_schema::<MeerkatMobWireInput>,
    },
    PublicTool {
        name: "meerkat_mob_wire_members_batch",
        description: "Wire multiple local mob-member pairs in one batch.",
        schema: typed_schema::<MeerkatMobWireMembersBatchInput>,
    },
    PublicTool {
        name: "meerkat_mob_unwire",
        description: "Remove a mob wiring relationship.",
        schema: typed_schema::<MeerkatMobUnwireInput>,
    },
    PublicTool {
        name: "meerkat_mob_member_send",
        description: "Deliver host-owned work to a specific mob member.",
        schema: typed_schema::<MobMemberSendParams>,
    },
    PublicTool {
        name: "meerkat_mob_append_system_context",
        description: "Stage system context for a specific mob member session.",
        schema: typed_schema::<MeerkatMobAppendSystemContextInput>,
    },
    PublicTool {
        name: "meerkat_mob_events",
        description: "Read mob event history by cursor.",
        schema: typed_schema::<MeerkatMobEventsInput>,
    },
    PublicTool {
        name: "meerkat_mob_flows",
        description: "List flows defined for a mob.",
        schema: typed_schema::<MeerkatMobIdInput>,
    },
    PublicTool {
        name: "meerkat_mob_flow_run",
        description: "Start a mob flow run.",
        schema: typed_schema::<MeerkatMobFlowRunInput>,
    },
    PublicTool {
        name: "meerkat_mob_run",
        description: "Invoke a mob as a typed callable run.",
        schema: typed_schema::<MeerkatMobRunInput>,
    },
    PublicTool {
        name: "meerkat_mob_flow_status",
        description: "Read status for a mob flow run.",
        schema: typed_schema::<MeerkatMobRunIdInput>,
    },
    PublicTool {
        name: "meerkat_mob_run_result",
        description: "Read the typed output envelope for a mob run.",
        schema: typed_schema::<MeerkatMobRunIdInput>,
    },
    PublicTool {
        name: "meerkat_mob_flow_cancel",
        description: "Cancel a mob flow run.",
        schema: typed_schema::<MeerkatMobRunIdInput>,
    },
    PublicTool {
        name: "meerkat_mob_force_cancel",
        description: "Force-cancel a mob member.",
        schema: typed_schema::<MeerkatMobMemberInput>,
    },
    PublicTool {
        name: "meerkat_mob_wait_kickoff",
        description: "Wait for autonomous kickoff turns to complete.",
        schema: typed_schema::<MeerkatMobWaitKickoffInput>,
    },
    PublicTool {
        name: "meerkat_mob_wait_ready",
        description: "Wait for mob members to become startup-ready for orchestration.",
        schema: typed_schema::<MeerkatMobWaitReadyInput>,
    },
    PublicTool {
        name: "meerkat_mob_profile_create",
        description: "Create a new realm profile for spawning mob members.",
        schema: typed_schema::<MeerkatMobProfileCreateInput>,
    },
    PublicTool {
        name: "meerkat_mob_profile_get",
        description: "Get a realm profile by name.",
        schema: typed_schema::<MeerkatMobProfileNameInput>,
    },
    PublicTool {
        name: "meerkat_mob_profile_list",
        description: "List all realm profiles.",
        schema: typed_schema::<MeerkatMobNoParamsInput>,
    },
    PublicTool {
        name: "meerkat_mob_profile_update",
        description: "Update a realm profile with CAS revision check.",
        schema: typed_schema::<MeerkatMobProfileUpdateInput>,
    },
    PublicTool {
        name: "meerkat_mob_profile_delete",
        description: "Delete a realm profile.",
        schema: typed_schema::<MeerkatMobProfileDeleteInput>,
    },
];

pub fn public_tool_names() -> Vec<&'static str> {
    PUBLIC_TOOLS.iter().map(|tool| tool.name).collect()
}

pub fn public_tools_list() -> Vec<Value> {
    PUBLIC_TOOLS
        .iter()
        .map(|entry| tool_json(entry.name, entry.description, (entry.schema)()))
        .collect()
}

/// Wrap a structured payload in the MCP text-content envelope.
///
/// Serialization is a TRUE fault: a payload that fails to serialize must surface
/// as an error, never as an empty-text content envelope masquerading as success.
pub fn wrap_public_tool_payload(payload: Value) -> Result<Value, String> {
    let text = serde_json::to_string(&payload)
        .map_err(|err| format!("Failed to serialize tool payload: {err}"))?;
    Ok(json!({
        "content": [{
            "type": "text",
            "text": text
        }]
    }))
}

/// Tool names matched by the dispatch arms in [`handle_public_tools_call`].
///
/// This MUST mirror the match arms below exactly: adding, removing, or renaming
/// an arm requires updating this slice. The `public_tool_surfaces_have_no_drift`
/// parity test pins this slice against the [`PUBLIC_TOOLS`] table and the
/// advertised [`public_tools_list`] catalog, so any drift fails the build.
#[cfg(test)]
const DISPATCH_TOOL_NAMES: &[&str] = &[
    "meerkat_mob_create",
    "meerkat_mob_list",
    "meerkat_mob_status",
    "meerkat_mob_lifecycle",
    "meerkat_mob_spawn",
    "meerkat_mob_spawn_many",
    "meerkat_mob_retire",
    "meerkat_mob_respawn",
    "meerkat_mob_wire",
    "meerkat_mob_wire_members_batch",
    "meerkat_mob_unwire",
    "meerkat_mob_member_send",
    "meerkat_mob_append_system_context",
    "meerkat_mob_events",
    "meerkat_mob_flows",
    "meerkat_mob_flow_run",
    "meerkat_mob_run",
    "meerkat_mob_flow_status",
    "meerkat_mob_run_result",
    "meerkat_mob_flow_cancel",
    "meerkat_mob_force_cancel",
    "meerkat_mob_wait_kickoff",
    "meerkat_mob_wait_ready",
    "meerkat_mob_profile_create",
    "meerkat_mob_profile_get",
    "meerkat_mob_profile_list",
    "meerkat_mob_profile_update",
    "meerkat_mob_profile_delete",
];

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
            // Status is projected through the typed wire contract
            // (`WireMobLifecycleStatus`), never via Display-string folklore.
            let mobs = state
                .mob_list()
                .await
                .map_err(|err| McpToolError::internal(err.to_string()))?
                .into_iter()
                .map(|(mob_id, status)| meerkat_contracts::MobStatusResult {
                    mob_id: mob_id.to_string(),
                    status: crate::wire_mob_lifecycle_status(status),
                })
                .collect::<Vec<_>>();
            Ok(json!(meerkat_contracts::MobListResult { mobs }))
        }
        "meerkat_mob_status" => {
            let input: MeerkatMobIdInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let status = state
                .mob_status(&mob_id)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!(meerkat_contracts::MobStatusResult {
                mob_id: mob_id.to_string(),
                status: crate::wire_mob_lifecycle_status(status),
            }))
        }
        "meerkat_mob_lifecycle" => {
            let input: MobLifecycleParams = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let destroy_report = state
                .mob_lifecycle_action(&mob_id, input.action)
                .await
                .map_err(|err| match err {
                    crate::MobMcpDestroyError::Incomplete { report } => {
                        McpToolError::destroy_incomplete(&report)
                    }
                    crate::MobMcpDestroyError::Mob(err) => {
                        McpToolError::invalid_params(err.to_string())
                    }
                })?
                .map(|report| {
                    serde_json::to_value(&report).map_err(|err| {
                        McpToolError::internal(format!("destroy report serialize: {err}"))
                    })
                })
                .transpose()?;
            Ok(json!(MobLifecycleResult {
                mob_id: mob_id.to_string(),
                action: input.action,
                ok: true,
                destroy_report,
            }))
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
            Ok(json!(spawn_result_payload(&mob_id, &spawn_result)))
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
            Ok(json!(meerkat_contracts::MobSpawnManyResult {
                results: results
                    .into_iter()
                    .map(
                        |result: Result<
                            meerkat_mob::SpawnResult,
                            meerkat_mob::MobSpawnManyFailure,
                        >| match result {
                            Ok(spawn_result) => {
                                let identity = spawn_result.agent_identity.to_string();
                                meerkat_contracts::MobSpawnManyResultEntry::spawned(
                                    identity.clone(),
                                    WireMemberRef::encode(mob_id.as_str(), &identity),
                                )
                            }
                            Err(error) => meerkat_contracts::MobSpawnManyResultEntry::failed(
                                error.cause(),
                                error.to_string(),
                            ),
                        },
                    )
                    .collect(),
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
            Ok(json!(meerkat_contracts::MobRetireResult { retired: true }))
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
                Ok(receipt) => Ok(json!(meerkat_contracts::MobRespawnResult {
                    status: meerkat_contracts::WireMobRespawnOutcome::Completed,
                    receipt: respawn_receipt_payload(&mob_id, &receipt),
                    failed_peer_ids: Vec::new(),
                })),
                Err(meerkat_mob::MobRespawnError::TopologyRestoreFailed {
                    receipt,
                    failed_peer_ids,
                }) => Ok(json!(meerkat_contracts::MobRespawnResult {
                    status: meerkat_contracts::WireMobRespawnOutcome::TopologyRestoreFailed,
                    receipt: respawn_receipt_payload(&mob_id, &receipt),
                    failed_peer_ids: failed_peer_ids
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect::<Vec<_>>(),
                })),
                Err(err) => Err(McpToolError::invalid_params(err.to_string())),
            }
        }
        "meerkat_mob_wire" => {
            let input: MeerkatMobWireInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let target = wire_peer_target_from_wire(input.peer);
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
        "meerkat_mob_wire_members_batch" => {
            let input: MeerkatMobWireMembersBatchInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let edges = input
                .edges
                .into_iter()
                .map(|edge| {
                    (
                        meerkat_mob::AgentIdentity::from(edge.a.as_str()),
                        meerkat_mob::AgentIdentity::from(edge.b.as_str()),
                    )
                })
                .collect::<Vec<_>>();
            let report = state
                .mob_wire_members_batch(&mob_id, edges)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            Ok(json!(report))
        }
        "meerkat_mob_unwire" => {
            let input: MeerkatMobUnwireInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let target = unwire_peer_target_from_wire(input.peer)?;
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
                        content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                            input.text,
                        ),
                        source: input.source,
                        idempotency_key: input.idempotency_key,
                        source_kind: meerkat_core::session::SystemContextSource::Normal,
                        peer_response_terminal: None,
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
        "meerkat_mob_run" => {
            let input: MeerkatMobRunInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let params = bind_prompt_param(input.params, input.prompt)?;
            let flow_id = meerkat_mob::FlowId::from(input.flow_id.as_deref().unwrap_or("main"));
            let run_id = state
                .mob_run_flow(&mob_id, flow_id, params)
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
            let run = meerkat_mob::MobRun::public_flow_status_run_value(run.as_ref())
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            serde_json::to_value(meerkat_contracts::MobFlowStatusResult { run })
                .map_err(|err| McpToolError::invalid_params(err.to_string()))
        }
        "meerkat_mob_run_result" => {
            let input: MeerkatMobRunIdInput = parse_args(arguments)?;
            let mob_id = parse_mob_id(&input.mob_id)?;
            let run_id = parse_run_id(&input.run_id)?;
            let run = state
                .mob_flow_status(&mob_id, run_id)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            let run = meerkat_mob::MobRun::public_run_result_value(run.as_ref())
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            serde_json::to_value(meerkat_contracts::MobRunResult { run })
                .map_err(|err| McpToolError::invalid_params(err.to_string()))
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
            serde_json::to_value(meerkat_mob::stored_realm_profile_to_wire(&stored))
                .map_err(|err| McpToolError::invalid_params(err.to_string()))
        }
        "meerkat_mob_profile_get" => {
            let input: MeerkatMobProfileNameInput = parse_args(arguments)?;
            match state.realm_profile_get(&input.name).await {
                Ok(Some(stored)) => {
                    serde_json::to_value(meerkat_mob::stored_realm_profile_to_wire(&stored))
                        .map_err(|err| McpToolError::invalid_params(err.to_string()))
                }
                Ok(None) => serde_json::to_value(meerkat_contracts::MobProfileLookupResult {
                    not_found: true,
                    name: input.name,
                    profile: None,
                    revision: None,
                    created_at: None,
                    updated_at: None,
                })
                .map_err(|err| McpToolError::invalid_params(err.to_string())),
                Err(err) => Err(McpToolError::invalid_params(err.to_string())),
            }
        }
        "meerkat_mob_profile_list" => {
            let profiles = state
                .realm_profile_list()
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            let profiles = profiles
                .iter()
                .map(meerkat_mob::stored_realm_profile_to_wire)
                .collect();
            serde_json::to_value(meerkat_contracts::MobProfileListResult { profiles })
                .map_err(|err| McpToolError::invalid_params(err.to_string()))
        }
        "meerkat_mob_profile_update" => {
            let input: MeerkatMobProfileUpdateInput = parse_args(arguments)?;
            let stored = state
                .realm_profile_update(&input.name, &input.profile, input.expected_revision)
                .await
                .map_err(|err| McpToolError::invalid_params(err.to_string()))?;
            serde_json::to_value(meerkat_mob::stored_realm_profile_to_wire(&stored))
                .map_err(|err| McpToolError::invalid_params(err.to_string()))
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

fn bind_prompt_param(params: Value, prompt: Option<String>) -> Result<Value, McpToolError> {
    let mut params = match params {
        Value::Null => serde_json::Map::new(),
        Value::Object(map) => map,
        _ => return Err(McpToolError::invalid_params("params must be an object")),
    };
    if let Some(prompt) = prompt
        && !params.contains_key("prompt")
    {
        params.insert("prompt".to_string(), Value::String(prompt));
    }
    Ok(Value::Object(params))
}

fn content_input_from_wire(
    input: WireContentInput,
) -> Result<meerkat_core::types::ContentInput, McpToolError> {
    meerkat_core::types::ContentInput::try_from(input).map_err(McpToolError::invalid_params)
}

fn wire_peer_target_from_wire(peer: MeerkatMobWirePeerTarget) -> meerkat_mob::PeerTarget {
    match peer {
        MeerkatMobWirePeerTarget::Local(member_id) => {
            meerkat_mob::PeerTarget::Local(member_id.into())
        }
        MeerkatMobWirePeerTarget::ExternalBinding(spec) => {
            meerkat_mob::PeerTarget::ExternalBinding(meerkat_mob::ExternalPeerBindingSpec::new(
                spec.name,
                spec.address,
                spec.identity,
            ))
        }
    }
}

fn unwire_peer_target_from_wire(
    peer: MeerkatMobUnwirePeerTarget,
) -> Result<meerkat_mob::PeerTarget, McpToolError> {
    match peer {
        MeerkatMobUnwirePeerTarget::Local(member_id) => {
            Ok(meerkat_mob::PeerTarget::Local(member_id.into()))
        }
        MeerkatMobUnwirePeerTarget::External(spec) => {
            let peer_name = meerkat_core::comms::PeerName::new(spec.name)
                .map_err(McpToolError::invalid_params)?;
            Ok(meerkat_mob::PeerTarget::ExternalName(peer_name))
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
                pubkey: resolved.pubkey,
            })
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::MobMcpState;
    use std::collections::BTreeSet;

    /// All three public-tool surfaces must agree on the exact same name set:
    ///   (a) the [`PUBLIC_TOOLS`] source-of-truth table (via [`public_tool_names`]),
    ///   (b) the advertised catalog names from [`public_tools_list`],
    ///   (c) the dispatch arm names pinned in [`DISPATCH_TOOL_NAMES`].
    /// Adding or removing any roster entry or match arm without updating the
    /// others fails this test. `meerkat_mob_wait_ready` must be in all three.
    #[test]
    fn public_tool_surfaces_have_no_drift() {
        let table_names: BTreeSet<String> = public_tool_names()
            .into_iter()
            .map(str::to_string)
            .collect();

        let advertised_names: BTreeSet<String> = public_tools_list()
            .iter()
            .map(|tool| {
                tool["name"]
                    .as_str()
                    .expect("advertised tool entry must carry a string name")
                    .to_string()
            })
            .collect();

        let dispatch_names: BTreeSet<String> = DISPATCH_TOOL_NAMES
            .iter()
            .map(|name| name.to_string())
            .collect();

        assert_eq!(
            table_names, advertised_names,
            "roster/advertised name drift between PUBLIC_TOOLS and public_tools_list()"
        );
        assert_eq!(
            table_names, dispatch_names,
            "roster/dispatch name drift between PUBLIC_TOOLS and DISPATCH_TOOL_NAMES"
        );

        assert!(
            table_names.contains("meerkat_mob_wait_ready"),
            "meerkat_mob_wait_ready must be routable, advertised, and dispatched"
        );
        assert!(
            table_names.contains("meerkat_mob_run_result"),
            "meerkat_mob_run_result must be routable, advertised, and dispatched"
        );
        assert!(
            table_names.contains("meerkat_mob_run"),
            "meerkat_mob_run must be routable, advertised, and dispatched"
        );
        assert_eq!(
            table_names.len(),
            28,
            "expected exactly 28 public tools across all surfaces"
        );
    }

    /// Gate for remediation rows #32/#157: every public mob MCP tool's
    /// `inputSchema` must be produced by `schema_for!` via [`typed_schema`],
    /// never a hand-authored `json!` literal. `schema_for!` always stamps the
    /// root with a `$schema` meta-schema URL; hand-written `json!({...})` object
    /// literals in this surface never carry one. Asserting `$schema` is present
    /// on every advertised schema therefore fails on the old hand-literal form
    /// and passes once each entry routes through `typed_schema::<T>()`.
    #[test]
    fn public_tool_schemas_are_typed_not_handwritten() {
        for entry in PUBLIC_TOOLS {
            let schema = (entry.schema)();
            assert!(
                schema.get("$schema").and_then(Value::as_str).is_some(),
                "tool {} has a hand-authored json! schema (missing $schema marker); \
                 it must derive from schema_for! via typed_schema::<T>()",
                entry.name
            );
        }
    }

    const ED25519_PUBLIC_KEY_7: &str = "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    const ED25519_PUBLIC_KEY_ZERO: &str = "ed25519:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    fn canonical_external_peer_target() -> MeerkatMobWirePeerTarget {
        serde_json::from_value(serde_json::json!({
            "external_binding": {
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
    fn public_mcp_wire_accepts_typed_external_binding_request() {
        let target = wire_peer_target_from_wire(canonical_external_peer_target());

        let meerkat_mob::PeerTarget::ExternalBinding(binding) = target else {
            panic!("canonical external peer should remain a mob-resolved external binding");
        };
        assert_eq!(binding.name, "external-worker");
        assert_eq!(binding.address, "inproc://external-worker");
    }

    #[test]
    fn public_mcp_wire_rejects_external_peer_raw_peer_id_shape() {
        let err = serde_json::from_value::<MeerkatMobWirePeerTarget>(serde_json::json!({
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
            msg.contains("external") || msg.contains("external_binding"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn public_mcp_wire_rejects_external_peer_missing_pubkey_material() {
        let err = serde_json::from_value::<MeerkatMobWirePeerTarget>(serde_json::json!({
            "external_binding": {
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
    fn public_mcp_unwire_accepts_external_peer_name_handle() {
        let target = unwire_peer_target_from_wire(
            serde_json::from_value::<MeerkatMobUnwirePeerTarget>(serde_json::json!({
                "external": { "name": "external-worker" }
            }))
            .expect("external handle should deserialize"),
        )
        .expect("external handle should convert");

        let meerkat_mob::PeerTarget::ExternalName(peer_name) = target else {
            panic!("unwire external should use the external peer handle");
        };
        assert_eq!(peer_name.as_str(), "external-worker");
    }

    #[test]
    fn public_mcp_wire_schema_uses_external_binding_without_raw_peer_atoms() {
        let tools = public_tools_list();
        let schema = tools
            .iter()
            .find(|tool| tool["name"] == "meerkat_mob_wire")
            .and_then(|tool| tool.get("inputSchema"))
            .expect("wire schema present");
        let schema_text = serde_json::to_string(schema).expect("schema should encode");

        assert!(schema_text.contains("external_binding"));
        assert!(
            !schema_text.contains("\"peer_id\"") && !schema_text.contains("\"pubkey\""),
            "wire schema must not expose raw comms identity atoms: {schema_text}"
        );
    }

    #[test]
    fn public_mcp_wire_members_batch_schema_is_local_member_edges_only() {
        let tools = public_tools_list();
        let schema = tools
            .iter()
            .find(|tool| tool["name"] == "meerkat_mob_wire_members_batch")
            .and_then(|tool| tool.get("inputSchema"))
            .expect("wire_members_batch schema present");
        let schema_text = serde_json::to_string(schema).expect("schema should encode");

        assert!(schema_text.contains("mob_id"));
        assert!(schema_text.contains("edges"));
        assert!(schema_text.contains("\"a\""));
        assert!(schema_text.contains("\"b\""));
        assert!(
            !schema_text.contains("external")
                && !schema_text.contains("\"peer\"")
                && !schema_text.contains("\"peer_id\"")
                && !schema_text.contains("\"pubkey\""),
            "batch schema must stay local-member only: {schema_text}"
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
        assert_eq!(pubkey, expected_pubkey);
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
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::profile::Profile {
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
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
            })),
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

    #[tokio::test]
    async fn public_mcp_lifecycle_rejects_unknown_action_at_contract_boundary() {
        let state = MobMcpState::new_in_memory();
        handle_public_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": "typed-public-lifecycle-rejects-unknown",
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
        .expect("create typed public mob");

        let err = handle_public_tools_call(
            &state,
            "meerkat_mob_lifecycle",
            &json!({
                "mob_id": "typed-public-lifecycle-rejects-unknown",
                "action": "explode"
            }),
        )
        .await
        .expect_err("unknown action must fail at contract parse boundary");

        assert_eq!(err.code, -32602);
        assert!(
            err.message.contains("unknown variant")
                && !err.message.contains("unknown lifecycle action"),
            "unexpected error: {}",
            err.message
        );

        let status = handle_public_tools_call(
            &state,
            "meerkat_mob_status",
            &json!({"mob_id": "typed-public-lifecycle-rejects-unknown"}),
        )
        .await
        .expect("status after rejected lifecycle action");
        assert_eq!(status["status"], "Running");

        handle_public_tools_call(
            &state,
            "meerkat_mob_lifecycle",
            &json!({
                "mob_id": "typed-public-lifecycle-rejects-unknown",
                "action": "destroy"
            }),
        )
        .await
        .expect("cleanup mob");
    }

    #[tokio::test]
    async fn public_mcp_lifecycle_accepts_typed_contract_params() {
        let state = MobMcpState::new_in_memory();
        handle_public_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": "typed-public-lifecycle-complete",
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
        .expect("create typed public mob");

        let params = meerkat_contracts::MobLifecycleParams {
            mob_id: "typed-public-lifecycle-complete".to_string(),
            action: meerkat_contracts::WireMobLifecycleAction::Complete,
        };
        let payload = serde_json::to_value(&params).expect("typed lifecycle params serialize");
        let result = handle_public_tools_call(&state, "meerkat_mob_lifecycle", &payload)
            .await
            .expect("typed lifecycle action dispatches");
        let result: meerkat_contracts::MobLifecycleResult =
            serde_json::from_value(result).expect("typed lifecycle result");

        assert_eq!(result.mob_id, "typed-public-lifecycle-complete");
        assert_eq!(
            result.action,
            meerkat_contracts::WireMobLifecycleAction::Complete
        );
        assert!(result.ok);

        handle_public_tools_call(
            &state,
            "meerkat_mob_lifecycle",
            &json!({
                "mob_id": "typed-public-lifecycle-complete",
                "action": "destroy"
            }),
        )
        .await
        .expect("cleanup mob");
    }

    #[tokio::test]
    async fn public_tools_spawn_many_returns_typed_failure_cause() {
        let state = MobMcpState::new_in_memory();

        let created = handle_public_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": "typed-public-spawn-many",
                    "profiles": {
                        "worker": {
                            "model": "gpt-5.4",
                            "tools": {"comms": true}
                        }
                    }
                }
            }),
        )
        .await
        .expect("typed public create");

        let spawned = handle_public_tools_call(
            &state,
            "meerkat_mob_spawn_many",
            &json!({
                "mob_id": created["mob_id"],
                "specs": [{
                    "profile": "missing",
                    "agent_identity": "w-missing"
                }]
            }),
        )
        .await
        .expect("spawn_many returns per-member failure row");
        let row = &spawned["results"].as_array().expect("results array")[0];
        assert_eq!(row["status"], "failed");
        assert_eq!(row["result"]["cause"], "profile_not_found");
        assert!(
            row["result"]["message"].as_str().is_some_and(|msg| {
                msg.contains("profile not found") && msg.contains("missing")
            })
        );
        assert!(row.get("ok").is_none());
        assert!(row.get("error").is_none());
    }

    #[tokio::test]
    async fn public_mcp_wire_members_batch_returns_batch_report_shape() {
        let state = MobMcpState::new_in_memory();
        handle_public_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": "typed-public-wire-members-batch",
                    "profiles": {
                        "worker": {
                            "model": "gpt-5.4",
                            "tools": {"comms": true}
                        }
                    }
                }
            }),
        )
        .await
        .expect("create typed public mob");

        for agent_identity in ["w-a", "w-b"] {
            handle_public_tools_call(
                &state,
                "meerkat_mob_spawn",
                &json!({
                    "mob_id": "typed-public-wire-members-batch",
                    "profile": "worker",
                    "agent_identity": agent_identity,
                    "runtime_mode": "turn_driven"
                }),
            )
            .await
            .expect("spawn local member");
        }

        let result = handle_public_tools_call(
            &state,
            "meerkat_mob_wire_members_batch",
            &json!({
                "mob_id": "typed-public-wire-members-batch",
                "edges": [{"a": "w-a", "b": "w-b"}]
            }),
        )
        .await
        .expect("batch wires local members");

        assert_eq!(result["requested"], 1);
        assert_eq!(result["wired"], json!([{ "a": "w-a", "b": "w-b" }]));
        assert_eq!(
            result["already_wired"]
                .as_array()
                .expect("already_wired array")
                .len(),
            0
        );

        let second = handle_public_tools_call(
            &state,
            "meerkat_mob_wire_members_batch",
            &json!({
                "mob_id": "typed-public-wire-members-batch",
                "edges": [{"a": "w-b", "b": "w-a"}]
            }),
        )
        .await
        .expect("batch reports already-wired local edge");

        assert_eq!(second["requested"], 1);
        assert_eq!(second["wired"].as_array().expect("wired array").len(), 0);
        assert_eq!(second["already_wired"], json!([{ "a": "w-a", "b": "w-b" }]));
    }
}
