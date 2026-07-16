//! Member-side operator upcall lane — the forwarding tool dispatcher mounted
//! on a HOST-MATERIALIZED member when `spec.profile.tools.mob == true`
//! (design-upcalls §2; mounted by `host_materialize` — D-X2).
//!
//! [`MemberUpcallToolDispatcher`] exposes the same twelve operator tools as
//! the controlling host's in-process `MobOperatorToolDispatcher` — a
//! NAME+SEMANTICS contract, not a byte-schema contract (DEC-U1): the two
//! spawn schemas drop the args the wire vocabulary structurally excludes
//! (`backend`, the deprecated `resume_session_id` alias) and add optional
//! `placement`. Each call maps to exactly one [`MemberOperatorOp`] variant,
//! rides the member's own comms runtime to the controlling supervisor bridge
//! (`member_operator_forwarder`), and reconstructs the exact agent-visible
//! `ToolResult`/`ToolError` from the typed [`UpcallToolOutcome`] envelope so
//! behavior is call-for-call identical to a local dispatch.
//!
//! No authority claim rides the wire: admission is MobMachine-owned
//! (`ResolveMemberOperatorAdmission`) and the operator authority is the
//! controlling host's recorded facts, re-minted controlling-side (ADJ-15).
//!
//! Phase-5 lane separation (§6): `PrincipalId` / `ControlScope` are not named
//! anywhere in this module — operator grants neither gate member upcalls nor
//! are satisfied by them.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use meerkat_contracts::wire::WireResolvedToolAccessPolicy;
use meerkat_contracts::wire::{WireForkContext, WireMemberLaunchMode, WireMobRuntimeMode};
use meerkat_core::ToolUnavailableReason;
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::comms::PeerRoute;
use meerkat_core::error::ToolError;
use meerkat_core::ops::{ToolAccessPolicy, ToolDispatchOutcome};
use meerkat_core::types::{
    ContentInput, ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::bridge_protocol::{
    MemberOperatorOp, MemberOperatorOutcome, MemberOperatorSpawnSpec, WireOpaqueJson,
};
use super::member_operator_forwarder::MemberOperatorForwarder;
use crate::ids::AgentIdentity;

// ---------------------------------------------------------------------------
// Upcall budget (ADJ-3)
// ---------------------------------------------------------------------------

/// Per-attempt reply timeout. Sized for the nested-materialize worst case:
/// an upcall `spawn_member` placed on the requester's host holds the
/// controlling-side execution for a 60s-class `MaterializeMember` round trip
/// under the single-flight bridge request lock.
pub const UPCALL_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(45);
/// Exactly this many attempts per logical call, re-sending the SAME
/// `request_id` (controlling-side dedup replays the recorded reply).
pub const UPCALL_ATTEMPTS: u32 = 2;
/// Total budget across attempts; exhaustion is the typed `upcall_timeout`
/// tool error (`ToolError::Timeout`).
pub const UPCALL_TOTAL_BUDGET: Duration = Duration::from_secs(90);

/// Retry/timeout budget for one logical upcall (module constants above are
/// the ADJ-3 defaults; single-flight caveat recorded, revisit at phase 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpcallBudget {
    pub attempt: Duration,
    pub attempts: u32,
    pub total: Duration,
}

impl Default for UpcallBudget {
    fn default() -> Self {
        Self {
            attempt: UPCALL_ATTEMPT_TIMEOUT,
            attempts: UPCALL_ATTEMPTS,
            total: UPCALL_TOTAL_BUDGET,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool names — the twelve-op mirror (§15.10 closed-surface pin, T-1)
// ---------------------------------------------------------------------------

pub(crate) const TOOL_SPAWN_MEMBER: &str = "spawn_member";
pub(crate) const TOOL_SPAWN_MANY_MEMBERS: &str = "spawn_many_members";
pub(crate) const TOOL_RETIRE_MEMBER: &str = "retire_member";
pub(crate) const TOOL_FORCE_CANCEL_MEMBER: &str = "force_cancel_member";
pub(crate) const TOOL_MEMBER_STATUS: &str = "member_status";
pub(crate) const TOOL_WIRE_MEMBERS: &str = "wire_members";
pub(crate) const TOOL_UNWIRE_MEMBERS: &str = "unwire_members";
pub(crate) const TOOL_LIST_MEMBERS: &str = "list_members";
pub(crate) const TOOL_MOB_LIST_FLOWS: &str = "mob_list_flows";
pub(crate) const TOOL_MOB_RUN_FLOW: &str = "mob_run_flow";
pub(crate) const TOOL_MOB_FLOW_STATUS: &str = "mob_flow_status";
pub(crate) const TOOL_MOB_CANCEL_FLOW: &str = "mob_cancel_flow";

/// Total both-direction mapping: every [`MemberOperatorOp`] variant names
/// exactly one tool. The match is exhaustive over the CLOSED wire enum, so a
/// contracts-side variant addition fails compilation here (the closed-enum
/// pin at the tool seam).
pub(crate) fn op_tool_name(op: &MemberOperatorOp) -> &'static str {
    match op {
        MemberOperatorOp::SpawnMember(_) => TOOL_SPAWN_MEMBER,
        MemberOperatorOp::SpawnManyMembers { .. } => TOOL_SPAWN_MANY_MEMBERS,
        MemberOperatorOp::RetireMember { .. } => TOOL_RETIRE_MEMBER,
        MemberOperatorOp::ForceCancelMember { .. } => TOOL_FORCE_CANCEL_MEMBER,
        MemberOperatorOp::MemberStatus { .. } => TOOL_MEMBER_STATUS,
        MemberOperatorOp::WireMembers { .. } => TOOL_WIRE_MEMBERS,
        MemberOperatorOp::UnwireMembers { .. } => TOOL_UNWIRE_MEMBERS,
        MemberOperatorOp::ListMembers => TOOL_LIST_MEMBERS,
        MemberOperatorOp::MobListFlows => TOOL_MOB_LIST_FLOWS,
        MemberOperatorOp::MobRunFlow { .. } => TOOL_MOB_RUN_FLOW,
        MemberOperatorOp::MobFlowStatus { .. } => TOOL_MOB_FLOW_STATUS,
        MemberOperatorOp::MobCancelFlow { .. } => TOOL_MOB_CANCEL_FLOW,
    }
}

// ---------------------------------------------------------------------------
// Result envelope — mob-owned, rides `WireOpaqueJson` (§2.5)
// ---------------------------------------------------------------------------

/// Typed tool-outcome envelope carried inside
/// `MemberOperatorOutcome::Completed { result }`. Both producer (the
/// controlling-side upcall responder) and consumer (this dispatcher) are
/// meerkat-mob code; tool-level failures are a SUCCESSFUL upcall whose tool
/// outcome is an error — conflating them with bridge rejections would
/// launder dispatcher policy denials into transport faults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum UpcallToolOutcome {
    /// `ToolResult` passthrough (text projection).
    Ok { content: String, is_error: bool },
    /// Closed mirror of the `ToolError` classes; reconstructs the exact
    /// variant member-side (T-4: `access_denied` stays `AccessDenied`).
    ToolError(UpcallToolError),
    /// Durable idempotency terminal. This is a successful bridge round trip
    /// whose operator effect cannot be reported as success: either a
    /// recovered Pending row makes the original result unknowable, or the
    /// same request key was reused for a different operation.
    DurabilityFailure(UpcallDurabilityFailure),
}

/// Closed durable-request terminal vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpcallDurabilityFailureKind {
    Indeterminate,
    RequestConflict,
}

/// Typed payload for a non-success durable request terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpcallDurabilityFailure {
    pub failure_kind: UpcallDurabilityFailureKind,
    pub message: String,
    pub original_op_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_op_digest: Option<String>,
}

/// Closed mirror of the reconstructable `ToolError` classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpcallToolErrorClass {
    NotFound,
    Unavailable,
    InvalidArguments,
    ExecutionFailed,
    Timeout,
    AccessDenied,
    Other,
}

/// Typed error payload: enough atoms to reconstruct the exact `ToolError`
/// variant on the member side (T-4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpcallToolError {
    pub class: UpcallToolErrorClass,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<ToolUnavailableReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl UpcallToolOutcome {
    /// Envelope a successful dispatch result (controlling side).
    pub(crate) fn from_tool_result(result: &ToolResult) -> Self {
        Self::Ok {
            content: result.text_content(),
            is_error: result.is_error,
        }
    }

    /// Envelope a typed tool error (controlling side).
    pub(crate) fn from_tool_error(error: &ToolError) -> Self {
        let payload = match error {
            ToolError::NotFound { name } => UpcallToolError {
                class: UpcallToolErrorClass::NotFound,
                message: error.to_string(),
                name: Some(name.clone()),
                timeout_ms: None,
                unavailable_reason: None,
                data: None,
            },
            ToolError::Unavailable { name, reason } => UpcallToolError {
                class: UpcallToolErrorClass::Unavailable,
                message: error.to_string(),
                name: Some(name.clone()),
                timeout_ms: None,
                unavailable_reason: Some(*reason),
                data: None,
            },
            ToolError::InvalidArguments { name, reason } => UpcallToolError {
                class: UpcallToolErrorClass::InvalidArguments,
                message: reason.clone(),
                name: Some(name.clone()),
                timeout_ms: None,
                unavailable_reason: None,
                data: None,
            },
            ToolError::ExecutionFailed { message } => UpcallToolError {
                class: UpcallToolErrorClass::ExecutionFailed,
                message: message.clone(),
                name: None,
                timeout_ms: None,
                unavailable_reason: None,
                data: None,
            },
            ToolError::ExecutionFailedWithData { message, data } => UpcallToolError {
                class: UpcallToolErrorClass::ExecutionFailed,
                message: message.clone(),
                name: None,
                timeout_ms: None,
                unavailable_reason: None,
                data: Some(data.clone()),
            },
            ToolError::Timeout { name, timeout_ms } => UpcallToolError {
                class: UpcallToolErrorClass::Timeout,
                message: error.to_string(),
                name: Some(name.clone()),
                timeout_ms: Some(*timeout_ms),
                unavailable_reason: None,
                data: None,
            },
            ToolError::AccessDenied { name } => UpcallToolError {
                class: UpcallToolErrorClass::AccessDenied,
                message: error.to_string(),
                name: Some(name.clone()),
                timeout_ms: None,
                unavailable_reason: None,
                data: None,
            },
            ToolError::Other(message) => UpcallToolError {
                class: UpcallToolErrorClass::Other,
                message: message.clone(),
                name: None,
                timeout_ms: None,
                unavailable_reason: None,
                data: None,
            },
            // CallbackPending is an in-process external-routing signal with
            // no cross-host meaning; the operator tool set never produces it.
            // Deliberately folded to a typed execution failure (stated, not
            // laundered).
            ToolError::CallbackPending { tool_name, .. } => UpcallToolError {
                class: UpcallToolErrorClass::ExecutionFailed,
                message: format!(
                    "tool '{tool_name}' returned callback_pending, which cannot cross the \
                     member upcall bridge"
                ),
                name: Some(tool_name.clone()),
                timeout_ms: None,
                unavailable_reason: None,
                data: None,
            },
        };
        Self::ToolError(payload)
    }

    pub(crate) fn indeterminate(op_digest: impl Into<String>) -> Self {
        let op_digest = op_digest.into();
        Self::DurabilityFailure(UpcallDurabilityFailure {
            failure_kind: UpcallDurabilityFailureKind::Indeterminate,
            message: "member operator effect outcome is indeterminate after recovery; the operation was not re-executed".to_string(),
            original_op_digest: op_digest,
            received_op_digest: None,
        })
    }

    pub(crate) fn request_conflict(
        original_op_digest: impl Into<String>,
        received_op_digest: impl Into<String>,
    ) -> Self {
        Self::DurabilityFailure(UpcallDurabilityFailure {
            failure_kind: UpcallDurabilityFailureKind::RequestConflict,
            message: "member operator request_id was reused for a different operation; no operation was executed".to_string(),
            original_op_digest: original_op_digest.into(),
            received_op_digest: Some(received_op_digest.into()),
        })
    }

    /// Reconstruct the agent-visible dispatch outcome (member side).
    pub(crate) fn into_dispatch_outcome(
        self,
        call_id: &str,
        tool_name: &str,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        match self {
            Self::Ok { content, is_error } => Ok(ToolDispatchOutcome::from(ToolResult::new(
                call_id.to_string(),
                content,
                is_error,
            ))),
            Self::ToolError(payload) => Err(payload.into_tool_error(tool_name)),
            Self::DurabilityFailure(failure) => {
                let code = match failure.failure_kind {
                    UpcallDurabilityFailureKind::Indeterminate => "upcall_indeterminate",
                    UpcallDurabilityFailureKind::RequestConflict => "upcall_request_conflict",
                };
                Err(ToolError::execution_failed_with_data(
                    failure.message,
                    json!({
                        "code": code,
                        "original_op_digest": failure.original_op_digest,
                        "received_op_digest": failure.received_op_digest,
                    }),
                ))
            }
        }
    }
}

impl UpcallToolError {
    /// Rebuild the exact `ToolError` variant (T-4).
    fn into_tool_error(self, fallback_name: &str) -> ToolError {
        let name = self.name.unwrap_or_else(|| fallback_name.to_string());
        match self.class {
            UpcallToolErrorClass::NotFound => ToolError::not_found(name),
            UpcallToolErrorClass::Unavailable => ToolError::unavailable(
                name,
                self.unavailable_reason
                    .unwrap_or(ToolUnavailableReason::TemporarilyUnavailable),
            ),
            UpcallToolErrorClass::InvalidArguments => {
                ToolError::invalid_arguments(name, self.message)
            }
            UpcallToolErrorClass::ExecutionFailed => match self.data {
                Some(data) => ToolError::execution_failed_with_data(self.message, data),
                None => ToolError::execution_failed(self.message),
            },
            UpcallToolErrorClass::Timeout => {
                ToolError::timeout(name, self.timeout_ms.unwrap_or_default())
            }
            UpcallToolErrorClass::AccessDenied => ToolError::access_denied(name),
            UpcallToolErrorClass::Other => ToolError::other(self.message),
        }
    }
}

// ---------------------------------------------------------------------------
// Remote-flavor tool defs (§2.2, DEC-U1)
// ---------------------------------------------------------------------------

fn tool_def(name: &str, description: &str, input_schema: serde_json::Value) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.into(),
        description: description.to_string(),
        input_schema,
        provenance: Some(ToolProvenance {
            kind: ToolSourceKind::Mob,
            source_id: "mob".into(),
        }),
    })
}

fn content_input_schema() -> serde_json::Value {
    json!({
        "oneOf": [
            { "type": "string" },
            {
                "type": "array",
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "text" },
                                "text": { "type": "string" }
                            },
                            "required": ["type", "text"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "image" },
                                "media_type": { "type": "string" },
                                "data": { "type": "string" }
                            },
                            "required": ["type", "media_type", "data"]
                        }
                    ]
                }
            }
        ]
    })
}

/// The twelve operator tool defs, remote flavor.
///
/// Byte-identical to the controlling host's local defs EXCEPT the two spawn
/// schemas: `backend` has no wire carrier (binding is machine-owned) and the
/// deprecated `resume_session_id` alias is dropped (single wire launch
/// carrier); optional `placement` (comms `PeerId` string) is added — omitted
/// means the requesting member's host (§7.3).
pub(crate) fn remote_operator_tool_defs() -> Vec<Arc<ToolDef>> {
    let mut defs: Vec<Arc<ToolDef>> = Vec::with_capacity(12);
    defs.push(tool_def(
        TOOL_SPAWN_MEMBER,
        "Spawn a mob member from a profile. Supports fresh, resume, or fork launch modes.",
        json!({
            "type": "object",
            "properties": {
                "profile": {"type": "string"},
                "member_id": {"type": "string"},
                "initial_message": content_input_schema(),
                "resume_bridge_session_id": {"type": "string", "description": "Preferred compatibility field for resume bridge bindings when launch_mode is omitted"},
                "runtime_mode": {"type": "string", "enum": ["autonomous_host", "turn_driven"]},
                "launch_mode": {
                    "type": "object",
                    "description": "Launch mode: fresh (default), resume {session_id}, or fork {source_member_id, fork_context}",
                },
                "tool_access_policy": {
                    "type": "object",
                    "description": "Tool access policy: inherit (default), allow_list, or deny_list"
                },
                "auto_wire_parent": {"type": "boolean", "description": "Auto-wire to spawner after spawn"},
                "placement": {"type": "string", "description": "Bound host peer id to place the member on; omitted = this member's host"}
            },
            "required": ["profile", "member_id"]
        }),
    ));
    defs.push(tool_def(
        TOOL_SPAWN_MANY_MEMBERS,
        "Spawn multiple mob members in one call. Returns per-item results in input order.",
        json!({
            "type": "object",
            "properties": {
                "specs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "profile": {"type": "string"},
                            "member_id": {"type": "string"},
                            "initial_message": content_input_schema(),
                            "resume_bridge_session_id": {"type": "string"},
                            "runtime_mode": {"type": "string", "enum": ["autonomous_host", "turn_driven"]},
                            "placement": {"type": "string", "description": "Bound host peer id to place the member on; omitted = this member's host"}
                        },
                        "required": ["profile", "member_id"]
                    }
                }
            },
            "required": ["specs"]
        }),
    ));
    defs.push(tool_def(
        TOOL_RETIRE_MEMBER,
        "Retire a member and archive its session.",
        json!({
            "type": "object",
            "properties": {"member_id": {"type": "string"}},
            "required": ["member_id"]
        }),
    ));
    defs.push(tool_def(
        TOOL_WIRE_MEMBERS,
        "Wire two mob members with bidirectional trust.",
        json!({
            "type": "object",
            "properties": {
                "member_id": {"type": "string"},
                "peer_member_id": {"type": "string"}
            },
            "required": ["member_id", "peer_member_id"]
        }),
    ));
    defs.push(tool_def(
        TOOL_UNWIRE_MEMBERS,
        "Unwire two mob members and revoke bidirectional trust.",
        json!({
            "type": "object",
            "properties": {
                "member_id": {"type": "string"},
                "peer_member_id": {"type": "string"}
            },
            "required": ["member_id", "peer_member_id"]
        }),
    ));
    defs.push(tool_def(
        TOOL_LIST_MEMBERS,
        "List all active mob members. Response includes identity-native lifecycle and runtime fields.",
        json!({
            "type": "object",
            "properties": {}
        }),
    ));
    defs.push(tool_def(
        TOOL_MOB_LIST_FLOWS,
        "List all configured flow IDs for this mob.",
        json!({
            "type": "object",
            "properties": {}
        }),
    ));
    defs.push(tool_def(
        TOOL_MOB_RUN_FLOW,
        "Run a configured flow by ID with optional activation params. Returns run_id.",
        json!({
            "type": "object",
            "properties": {
                "flow_id": {"type": "string"},
                "params": {"type": "object"}
            },
            "required": ["flow_id"]
        }),
    ));
    defs.push(tool_def(
        TOOL_MOB_FLOW_STATUS,
        "Get persisted status and ledgers for a flow run.",
        json!({
            "type": "object",
            "properties": {
                "run_id": {"type": "string"}
            },
            "required": ["run_id"]
        }),
    ));
    defs.push(tool_def(
        TOOL_MOB_CANCEL_FLOW,
        "Cancel an in-flight flow run by run_id.",
        json!({
            "type": "object",
            "properties": {
                "run_id": {"type": "string"}
            },
            "required": ["run_id"]
        }),
    ));
    defs.push(tool_def(
        TOOL_FORCE_CANCEL_MEMBER,
        "Force-cancel a member's in-flight turn. Does not retire the member.",
        json!({
            "type": "object",
            "properties": {
                "member_id": {"type": "string"}
            },
            "required": ["member_id"]
        }),
    ));
    defs.push(tool_def(
        TOOL_MEMBER_STATUS,
        "Get a member's execution status snapshot including output preview and token usage.",
        json!({
            "type": "object",
            "properties": {
                "member_id": {"type": "string"}
            },
            "required": ["member_id"]
        }),
    ));
    defs
}

// ---------------------------------------------------------------------------
// Args — deny_unknown_fields mirrors (T-3: an unexpected field is a typed
// InvalidArguments, never a silent drop)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteSpawnMemberArgs {
    profile: String,
    member_id: String,
    #[serde(default)]
    initial_message: Option<ContentInput>,
    #[serde(default)]
    resume_bridge_session_id: Option<meerkat_core::types::SessionId>,
    #[serde(default)]
    runtime_mode: Option<crate::MobRuntimeMode>,
    #[serde(default)]
    launch_mode: Option<crate::launch::MemberLaunchMode>,
    #[serde(default)]
    tool_access_policy: Option<ToolAccessPolicy>,
    #[serde(default)]
    auto_wire_parent: Option<bool>,
    #[serde(default)]
    placement: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteSpawnManyMembersArgs {
    specs: Vec<RemoteSpawnMemberArgs>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberIdArgs {
    member_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePairArgs {
    member_id: String,
    peer_member_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunFlowArgs {
    flow_id: String,
    #[serde(default)]
    params: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunIdArgs {
    run_id: String,
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Atomically sampled exact residency tuple for member-originated upcalls.
///
/// Same-session Resume can preserve the mounted dispatcher and member key
/// while advancing host or member binding facts, so copying these values into the
/// dispatcher would make every later request stale. The materializer retains
/// and updates this stamp; each logical upcall snapshots the full tuple once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberUpcallBindingTuple {
    pub generation: u64,
    pub fence_token: u64,
    pub host_id: String,
    pub host_binding_generation: u64,
    pub member_session_id: String,
}

#[derive(Debug)]
pub struct MemberUpcallBindingStamp {
    inner: RwLock<MemberUpcallBindingTuple>,
}

impl MemberUpcallBindingStamp {
    pub fn new(
        generation: u64,
        fence_token: u64,
        host_id: impl Into<String>,
        host_binding_generation: u64,
        member_session_id: impl Into<String>,
    ) -> Self {
        Self {
            inner: RwLock::new(MemberUpcallBindingTuple {
                generation,
                fence_token,
                host_id: host_id.into(),
                host_binding_generation,
                member_session_id: member_session_id.into(),
            }),
        }
    }

    pub fn snapshot(&self) -> MemberUpcallBindingTuple {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn update(
        &self,
        generation: u64,
        fence_token: u64,
        host_id: impl Into<String>,
        host_binding_generation: u64,
        member_session_id: impl Into<String>,
    ) {
        *self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = MemberUpcallBindingTuple {
            generation,
            fence_token,
            host_id: host_id.into(),
            host_binding_generation,
            member_session_id: member_session_id.into(),
        };
    }
}

/// Member-side forwarding dispatcher for the twelve operator tools.
///
/// Constructed by the member host's spec decompiler
/// (`host_materialize::mount_member_operator_tools`) when the materialized
/// spec resolves `tools.mob = true`. No `bind_ops_lifecycle` capability:
/// upcall spawns are executed controlling-side synchronously and the reply
/// carries the settled result — detached async receipts have no member-side
/// consumer.
pub struct MemberUpcallToolDispatcher {
    agent_identity: AgentIdentity,
    /// This member's OWN resolved policy (= the spec overlay's sealed
    /// `tool_access_policy`, never `Inherit`) — the O3 Inherit-resolution
    /// source for child spawns: the controlling host resolved the parent
    /// realm; this host never guesses.
    requester_resolved_policy: Option<WireResolvedToolAccessPolicy>,
    tools: Arc<[Arc<ToolDef>]>,
    forwarder: MemberOperatorForwarder,
}

impl MemberUpcallToolDispatcher {
    pub fn new(
        agent_identity: AgentIdentity,
        binding_stamp: Arc<MemberUpcallBindingStamp>,
        supervisor: PeerRoute,
        member_runtime: Arc<meerkat_comms::CommsRuntime>,
        requester_resolved_policy: Option<WireResolvedToolAccessPolicy>,
    ) -> Self {
        Self {
            agent_identity,
            requester_resolved_policy,
            tools: remote_operator_tool_defs().into(),
            forwarder: MemberOperatorForwarder::new(
                supervisor,
                binding_stamp,
                member_runtime,
                UpcallBudget::default(),
            ),
        }
    }

    /// Map (tool name, raw args) to exactly one op (T-2/T-3 rules).
    fn map_call(&self, call: ToolCallView<'_>) -> Result<MemberOperatorOp, ToolError> {
        let invalid =
            |error: serde_json::Error| ToolError::invalid_arguments(call.name, error.to_string());
        match call.name {
            TOOL_SPAWN_MEMBER => {
                let args: RemoteSpawnMemberArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::SpawnMember(Box::new(
                    self.spawn_spec_from_args(args)?,
                )))
            }
            TOOL_SPAWN_MANY_MEMBERS => {
                let args: RemoteSpawnManyMembersArgs = call.parse_args().map_err(invalid)?;
                let specs = args
                    .specs
                    .into_iter()
                    .map(|spec| self.spawn_spec_from_args(spec))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(MemberOperatorOp::SpawnManyMembers { specs })
            }
            TOOL_RETIRE_MEMBER => {
                let args: MemberIdArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::RetireMember {
                    member_id: args.member_id,
                })
            }
            TOOL_FORCE_CANCEL_MEMBER => {
                let args: MemberIdArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::ForceCancelMember {
                    member_id: args.member_id,
                })
            }
            TOOL_MEMBER_STATUS => {
                let args: MemberIdArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::MemberStatus {
                    member_id: args.member_id,
                })
            }
            TOOL_WIRE_MEMBERS => {
                let args: WirePairArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::WireMembers {
                    member_id: args.member_id,
                    peer_member_id: args.peer_member_id,
                })
            }
            TOOL_UNWIRE_MEMBERS => {
                let args: WirePairArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::UnwireMembers {
                    member_id: args.member_id,
                    peer_member_id: args.peer_member_id,
                })
            }
            TOOL_LIST_MEMBERS => {
                let _args: EmptyArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::ListMembers)
            }
            TOOL_MOB_LIST_FLOWS => {
                let _args: EmptyArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::MobListFlows)
            }
            TOOL_MOB_RUN_FLOW => {
                let args: RunFlowArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::MobRunFlow {
                    flow_id: args.flow_id,
                    // Byte-stable opaque carrier (T-2): serialized exactly
                    // once here; the responder parses it back for the local
                    // dispatcher.
                    params: args.params.as_ref().map(WireOpaqueJson::from_value),
                })
            }
            TOOL_MOB_FLOW_STATUS => {
                let args: RunIdArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::MobFlowStatus {
                    run_id: args.run_id,
                })
            }
            TOOL_MOB_CANCEL_FLOW => {
                let args: RunIdArgs = call.parse_args().map_err(invalid)?;
                Ok(MemberOperatorOp::MobCancelFlow {
                    run_id: args.run_id,
                })
            }
            other => Err(ToolError::not_found(other)),
        }
    }

    /// Spawn-arg normalization (T-3) + the O3 two-fact pair (§18 O3, T-U5):
    /// `requested_tool_access_policy_present` comes from the RAW args
    /// (`Some(Inherit)` counts as requested), and the resolved policy is
    /// computed independently — presence is NEVER derived from the resolved
    /// output.
    fn spawn_spec_from_args(
        &self,
        args: RemoteSpawnMemberArgs,
    ) -> Result<MemberOperatorSpawnSpec, ToolError> {
        let requested_tool_access_policy_present = args.tool_access_policy.is_some();
        let resolved_tool_access_policy = match args.tool_access_policy {
            Some(ToolAccessPolicy::AllowList(names)) => Some(
                WireResolvedToolAccessPolicy::AllowList(sorted_tool_names(&names)),
            ),
            Some(ToolAccessPolicy::DenyList(names)) => Some(
                WireResolvedToolAccessPolicy::DenyList(sorted_tool_names(&names)),
            ),
            // Inherit (explicit or absent) resolves against THIS member's
            // own sealed policy — the parent-realm resolution happened on
            // the controlling host at spec-mint (F-C.4 containment: no host
            // pair yields an Inherit-derived unrestricted child).
            Some(ToolAccessPolicy::Inherit) | None => self.requester_resolved_policy.clone(),
        };
        // Launch precedence (tools.rs parity): explicit launch_mode wins,
        // then the resume_bridge_session_id compatibility field.
        let launch_mode = match args.launch_mode {
            Some(mode) => Some(wire_launch_mode(mode)),
            None => args
                .resume_bridge_session_id
                .map(|session_id| WireMemberLaunchMode::Resume {
                    bridge_session_id: session_id.to_string(),
                }),
        };
        Ok(MemberOperatorSpawnSpec {
            profile: args.profile,
            member_id: args.member_id,
            initial_message: args.initial_message,
            runtime_mode: args.runtime_mode.map(wire_runtime_mode),
            launch_mode,
            auto_wire_parent: args.auto_wire_parent,
            placement: args.placement,
            requested_tool_access_policy_present,
            resolved_tool_access_policy,
        })
    }
}

fn sorted_tool_names(names: &meerkat_core::types::ToolNameSet) -> Vec<String> {
    let mut out: Vec<String> = names.iter().map(|name| name.as_str().to_string()).collect();
    out.sort_unstable();
    out
}

fn wire_runtime_mode(mode: crate::MobRuntimeMode) -> WireMobRuntimeMode {
    match mode {
        crate::MobRuntimeMode::AutonomousHost => WireMobRuntimeMode::AutonomousHost,
        crate::MobRuntimeMode::TurnDriven => WireMobRuntimeMode::TurnDriven,
    }
}

/// Total within-crate mapping (`MemberLaunchMode` is `#[non_exhaustive]`
/// only across crates): a new launch variant fails compilation HERE until a
/// wire carrier exists — never a silent coercion.
fn wire_launch_mode(mode: crate::launch::MemberLaunchMode) -> WireMemberLaunchMode {
    match mode {
        crate::launch::MemberLaunchMode::Fresh => WireMemberLaunchMode::Fresh,
        crate::launch::MemberLaunchMode::Resume { bridge_session_id } => {
            WireMemberLaunchMode::Resume {
                bridge_session_id: bridge_session_id.to_string(),
            }
        }
        crate::launch::MemberLaunchMode::Fork {
            source_member_id,
            fork_context,
        } => WireMemberLaunchMode::Fork {
            source_member_id: source_member_id.as_str().to_string(),
            fork_context: wire_fork_context(fork_context),
        },
    }
}

fn wire_fork_context(context: crate::launch::ForkContext) -> WireForkContext {
    match context {
        crate::launch::ForkContext::FullHistory => WireForkContext::FullHistory,
        crate::launch::ForkContext::LastMessages { count } => {
            WireForkContext::LastMessages { count }
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AgentToolDispatcher for MemberUpcallToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        let op = self.map_call(call)?;
        let outcome = self
            .forwarder
            .forward_operator_request(&self.agent_identity, op)
            .await?;
        match outcome {
            MemberOperatorOutcome::Completed { result } => {
                let value = result.to_value().map_err(|error| {
                    ToolError::execution_failed(format!(
                        "tool '{}' upcall result envelope is not valid JSON: {error}",
                        call.name
                    ))
                })?;
                let tool_outcome: UpcallToolOutcome =
                    serde_json::from_value(value).map_err(|error| {
                        ToolError::execution_failed(format!(
                            "tool '{}' upcall result envelope failed to decode: {error}",
                            call.name
                        ))
                    })?;
                tool_outcome.into_dispatch_outcome(call.id, call.name)
            }
            // Admission / transport / authority-record rejections (§3.4):
            // typed structured failure carrying the wire cause.
            MemberOperatorOutcome::Rejected { cause, reason } => Err(
                ToolError::execution_failed_with_data(reason, json!({ "cause": cause })),
            ),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn dispatcher(policy: Option<WireResolvedToolAccessPolicy>) -> MemberUpcallToolDispatcher {
        MemberUpcallToolDispatcher {
            agent_identity: AgentIdentity::from("b2"),
            requester_resolved_policy: policy,
            tools: remote_operator_tool_defs().into(),
            forwarder: MemberOperatorForwarder::new(
                PeerRoute::new(meerkat_core::comms::PeerId::from_ed25519_pubkey(&[9u8; 32])),
                Arc::new(MemberUpcallBindingStamp::new(
                    1,
                    1,
                    "host-a",
                    1,
                    "member-session-a",
                )),
                test_runtime(),
                UpcallBudget::default(),
            ),
        }
    }

    fn test_runtime() -> Arc<meerkat_comms::CommsRuntime> {
        Arc::new(
            meerkat_comms::CommsRuntime::inproc_only(&format!(
                "member-upcall-test-{}",
                uuid::Uuid::new_v4().simple()
            ))
            .expect("inproc comms runtime"),
        )
    }

    fn call<'a>(name: &'a str, args: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
        ToolCallView {
            id: "call-1",
            name,
            args,
        }
    }

    fn raw(value: serde_json::Value) -> Box<serde_json::value::RawValue> {
        serde_json::value::RawValue::from_string(value.to_string()).expect("raw json")
    }

    /// T-1: closed-surface pin — `remote_operator_tool_defs()` names and
    /// `MemberOperatorOp` variants map 1:1 in both directions.
    #[test]
    fn tool_names_mirror_the_twelve_ops_both_directions() {
        let defs = remote_operator_tool_defs();
        assert_eq!(defs.len(), 12);

        // One representative op per variant; op → name via the exhaustive
        // total match, name → def presence via the defs list.
        let ops: Vec<MemberOperatorOp> = vec![
            MemberOperatorOp::SpawnMember(Box::new(minimal_spawn_spec("w"))),
            MemberOperatorOp::SpawnManyMembers { specs: vec![] },
            MemberOperatorOp::RetireMember {
                member_id: "w".into(),
            },
            MemberOperatorOp::ForceCancelMember {
                member_id: "w".into(),
            },
            MemberOperatorOp::MemberStatus {
                member_id: "w".into(),
            },
            MemberOperatorOp::WireMembers {
                member_id: "a".into(),
                peer_member_id: "b".into(),
            },
            MemberOperatorOp::UnwireMembers {
                member_id: "a".into(),
                peer_member_id: "b".into(),
            },
            MemberOperatorOp::ListMembers,
            MemberOperatorOp::MobListFlows,
            MemberOperatorOp::MobRunFlow {
                flow_id: "f".into(),
                params: None,
            },
            MemberOperatorOp::MobFlowStatus { run_id: "r".into() },
            MemberOperatorOp::MobCancelFlow { run_id: "r".into() },
        ];
        let def_names: std::collections::BTreeSet<&str> =
            defs.iter().map(|def| def.name.as_str()).collect();
        let op_names: std::collections::BTreeSet<&str> = ops.iter().map(op_tool_name).collect();
        assert_eq!(op_names.len(), 12, "twelve distinct tool names");
        assert_eq!(def_names, op_names, "defs ↔ ops name parity");
    }

    fn minimal_spawn_spec(member: &str) -> MemberOperatorSpawnSpec {
        MemberOperatorSpawnSpec {
            profile: "worker".into(),
            member_id: member.into(),
            initial_message: None,
            runtime_mode: None,
            launch_mode: None,
            auto_wire_parent: None,
            placement: None,
            requested_tool_access_policy_present: false,
            resolved_tool_access_policy: None,
        }
    }

    /// T-2: each simple tool call maps to exactly its op; `MobRunFlow.params`
    /// rides `WireOpaqueJson` byte-stable.
    #[tokio::test]
    async fn simple_calls_map_to_their_ops() {
        let d = dispatcher(None);
        let args = raw(json!({"member_id": "w1"}));
        assert_eq!(
            d.map_call(call(TOOL_RETIRE_MEMBER, &args)).unwrap(),
            MemberOperatorOp::RetireMember {
                member_id: "w1".into()
            }
        );
        let args = raw(json!({}));
        assert_eq!(
            d.map_call(call(TOOL_LIST_MEMBERS, &args)).unwrap(),
            MemberOperatorOp::ListMembers
        );
        let params = json!({"alpha": 1, "beta": {"nested": true}});
        let args = raw(json!({"flow_id": "f1", "params": params}));
        let op = d.map_call(call(TOOL_MOB_RUN_FLOW, &args)).unwrap();
        match op {
            MemberOperatorOp::MobRunFlow {
                flow_id,
                params: carried,
            } => {
                assert_eq!(flow_id, "f1");
                let carried = carried.expect("params carried");
                assert_eq!(carried, WireOpaqueJson::from_value(&params), "byte-stable");
            }
            other => panic!("expected MobRunFlow, got {other:?}"),
        }
        let args = raw(json!({"name": "x"}));
        assert!(matches!(
            d.map_call(call("unknown_tool", &args)),
            Err(ToolError::NotFound { .. })
        ));
    }

    /// T-3: spawn-arg normalization — launch precedence, deny_unknown_fields
    /// rejections, Inherit/absent resolution, and the independent presence
    /// fact.
    #[tokio::test]
    async fn spawn_args_normalize_with_the_two_fact_policy_pair() {
        let deny = WireResolvedToolAccessPolicy::DenyList(vec!["shell_execute".into()]);
        let d = dispatcher(Some(deny.clone()));

        // resume_bridge_session_id folds into launch_mode: Resume.
        let sid = meerkat_core::types::SessionId::new();
        let args = raw(json!({
            "profile": "worker", "member_id": "b21",
            "resume_bridge_session_id": sid.to_string(),
        }));
        let op = d.map_call(call(TOOL_SPAWN_MEMBER, &args)).unwrap();
        let MemberOperatorOp::SpawnMember(spec) = op else {
            panic!("expected SpawnMember");
        };
        assert_eq!(
            spec.launch_mode,
            Some(WireMemberLaunchMode::Resume {
                bridge_session_id: sid.to_string()
            })
        );
        // Absent policy: presence=false, resolved = requester's own policy
        // (the F-C.4 containment resolution).
        assert!(!spec.requested_tool_access_policy_present);
        assert_eq!(spec.resolved_tool_access_policy, Some(deny.clone()));

        // Explicit launch_mode takes precedence over the compat field.
        let args = raw(json!({
            "profile": "worker", "member_id": "b21",
            "resume_bridge_session_id": sid.to_string(),
            "launch_mode": {"mode": "fresh"},
        }));
        let MemberOperatorOp::SpawnMember(spec) =
            d.map_call(call(TOOL_SPAWN_MEMBER, &args)).unwrap()
        else {
            panic!("expected SpawnMember");
        };
        assert_eq!(spec.launch_mode, Some(WireMemberLaunchMode::Fresh));

        // backend has no wire carrier: typed InvalidArguments, not a drop.
        let args = raw(json!({
            "profile": "worker", "member_id": "b21", "backend": "session",
        }));
        assert!(matches!(
            d.map_call(call(TOOL_SPAWN_MEMBER, &args)),
            Err(ToolError::InvalidArguments { .. })
        ));
        // Deprecated resume_session_id alias is dropped from this surface.
        let args = raw(json!({
            "profile": "worker", "member_id": "b21",
            "resume_session_id": sid.to_string(),
        }));
        assert!(matches!(
            d.map_call(call(TOOL_SPAWN_MEMBER, &args)),
            Err(ToolError::InvalidArguments { .. })
        ));

        // Some(Inherit): presence=true (a request WAS made) while resolution
        // still lands on the requester's own policy — the two facts are
        // independent (no laundering in either direction).
        let args = raw(json!({
            "profile": "worker", "member_id": "b21",
            "tool_access_policy": {"type": "inherit"},
        }));
        let MemberOperatorOp::SpawnMember(spec) =
            d.map_call(call(TOOL_SPAWN_MEMBER, &args)).unwrap()
        else {
            panic!("expected SpawnMember");
        };
        assert!(spec.requested_tool_access_policy_present);
        assert_eq!(spec.resolved_tool_access_policy, Some(deny.clone()));

        // Explicit AllowList ships verbatim (sorted), presence=true.
        let args = raw(json!({
            "profile": "worker", "member_id": "b21",
            "tool_access_policy": {"type": "allow_list", "value": ["b_tool", "a_tool"]},
        }));
        let MemberOperatorOp::SpawnMember(spec) =
            d.map_call(call(TOOL_SPAWN_MEMBER, &args)).unwrap()
        else {
            panic!("expected SpawnMember");
        };
        assert!(spec.requested_tool_access_policy_present);
        assert_eq!(
            spec.resolved_tool_access_policy,
            Some(WireResolvedToolAccessPolicy::AllowList(vec![
                "a_tool".into(),
                "b_tool".into()
            ]))
        );

        // Unrestricted parent (no requester policy): absent stays None with
        // presence=false.
        let d_open = dispatcher(None);
        let args = raw(json!({"profile": "worker", "member_id": "b21"}));
        let MemberOperatorOp::SpawnMember(spec) =
            d_open.map_call(call(TOOL_SPAWN_MEMBER, &args)).unwrap()
        else {
            panic!("expected SpawnMember");
        };
        assert!(!spec.requested_tool_access_policy_present);
        assert_eq!(spec.resolved_tool_access_policy, None);
    }

    /// T-4: `UpcallToolOutcome` roundtrip — Ok and every ToolError class
    /// reconstruct exactly through the `WireOpaqueJson` envelope.
    #[test]
    fn outcome_envelope_roundtrips_every_class() {
        // Ok passthrough.
        let result = ToolResult::new("id-1".into(), "{\"members\":[]}".into(), false);
        let envelope = UpcallToolOutcome::from_tool_result(&result);
        let wire = WireOpaqueJson::from_value(&serde_json::to_value(&envelope).unwrap());
        let back: UpcallToolOutcome = serde_json::from_value(wire.to_value().unwrap()).unwrap();
        let outcome = back.into_dispatch_outcome("id-1", "list_members").unwrap();
        assert_eq!(outcome.result.text_content(), "{\"members\":[]}");
        assert!(!outcome.result.is_error);

        let errors = vec![
            ToolError::not_found("spawn_member"),
            ToolError::unavailable("wire_members", ToolUnavailableReason::NoPeersConfigured),
            ToolError::invalid_arguments("spawn_member", "bad args"),
            ToolError::execution_failed("boom"),
            ToolError::execution_failed_with_data("boom", json!({"cause": "unavailable"})),
            ToolError::timeout("mob_run_flow", 90_000),
            ToolError::access_denied("retire_member"),
            ToolError::other("misc"),
        ];
        for error in errors {
            let envelope = UpcallToolOutcome::from_tool_error(&error);
            let wire = WireOpaqueJson::from_value(&serde_json::to_value(&envelope).unwrap());
            let back: UpcallToolOutcome = serde_json::from_value(wire.to_value().unwrap()).unwrap();
            let reconstructed = back
                .into_dispatch_outcome("id-1", "fallback_tool")
                .expect_err("error envelope reconstructs an error");
            assert_eq!(
                reconstructed.error_code(),
                error.error_code(),
                "class preserved for {error:?}"
            );
            match (&error, &reconstructed) {
                (
                    ToolError::Timeout { name, timeout_ms },
                    ToolError::Timeout {
                        name: rname,
                        timeout_ms: rms,
                    },
                ) => {
                    assert_eq!(name, rname);
                    assert_eq!(timeout_ms, rms);
                }
                (ToolError::AccessDenied { name }, ToolError::AccessDenied { name: rname }) => {
                    assert_eq!(name, rname);
                }
                (
                    ToolError::Unavailable { name, reason },
                    ToolError::Unavailable {
                        name: rname,
                        reason: rreason,
                    },
                ) => {
                    assert_eq!(name, rname);
                    assert_eq!(reason, rreason);
                }
                _ => {}
            }
        }
    }
}
