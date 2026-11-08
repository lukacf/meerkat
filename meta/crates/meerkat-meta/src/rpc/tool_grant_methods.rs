//! `meta/tool_grants/*`: the scoped tool grants of the identities this
//! gateway serves.
//!
//! One family, served by the same handlers on the stdio gateway (to its SDK
//! host, the operator) and on the Console RPC (to its signed-in caller,
//! under the access actions). Meta keeps no grant state: the grant rows, the
//! gate a member's build installs and every adjudication are the runtime's
//! (the grant service of the gateway's `MeerkatMachine`), and a member's
//! build opens what its identity's live grants cover.
//!
//! A grant names an identity the gateway's identity runtime serves. One
//! created at runtime (store-owned: its record is its row in the mob's
//! identity store) is not one yet, and is `subject_unknown`. When Core holds
//! no member of the identity, the grant seats it through the declared
//! apply's one-identity door, Dormant, building nothing, so its first build
//! reads the grant. The target must be named by the gate the member's next
//! build resolves ([`meerkat_mob::MobHandle::member_grant_gate`]), and the
//! grant commits through the mob
//! ([`meerkat_mob::MobHandle::grant_member_tool`]), which decides it against
//! its live roster in order with the retirements that end an identity's
//! grants: an identity no longer on the roster then is `subject_unknown`,
//! and nothing is issued. Meta's answers do not report a grant's `applies`
//! or a member's gate yet. List and audit resolve the mob only, so a retired
//! identity's history stays readable. A revoke reads the grant first: only
//! an identity grant of this gateway's mob is revoked here. Every refusal of
//! a request is invalid params carrying the typed
//! `tool_grant_request_refused` refusal.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use meerkat_contracts::{
    GrantToolResult, ListToolGrantsResult, RevokeToolGrantResult, ToolGrantAuditResult,
    ToolGrantTargetRequest, WireToolGrant, WireToolGrantSubject,
};
use meerkat_core::approval::decision::DecisionReference;
use meerkat_core::tool_grant::{
    ToolGrantId, ToolGrantIssuer, ToolGrantRequestRefusal, ToolGrantSubject,
};
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::tool_grant::wire::{
    wire_tool_grant, wire_tool_grant_audit_row, wire_unreadable_tool_grant,
    wire_unreadable_tool_grant_audit_row,
};
use meerkat_runtime::tool_grant::{
    StoredToolGrant, ToolGrantRevocation, ToolGrantServiceError, ToolGrantStatus, ToolGrantWindow,
};
use serde::Deserialize;
use serde_json::Value;

use super::{CAPABILITY_UNAVAILABLE_CODE, JSONRPC_VERSION, JsonRpcError, JsonRpcResponse};
use crate::access::{ACTION_AGENT_TOOL_GRANTS_MANAGE, AccessView};
use crate::identity_first::{AgentIdentity, IdentityRuntime};
use crate::mob_handle_runtime::MobRuntime;

/// Issue one grant to an identity.
pub(crate) const TOOL_GRANTS_GRANT: &str = "meta/tool_grants/grant";
/// Revoke one grant.
pub(crate) const TOOL_GRANTS_REVOKE: &str = "meta/tool_grants/revoke";
/// An identity's grants.
pub(crate) const TOOL_GRANTS_LIST: &str = "meta/tool_grants/list";
/// One page of an identity's grant audit log.
pub(crate) const TOOL_GRANTS_AUDIT: &str = "meta/tool_grants/audit";

/// The family, one registry every surface derives its dispatch arm, its
/// read/mutate split and its advertised methods from.
pub(crate) const TOOL_GRANT_METHODS: &[&str] = &[
    TOOL_GRANTS_GRANT,
    TOOL_GRANTS_REVOKE,
    TOOL_GRANTS_LIST,
    TOOL_GRANTS_AUDIT,
];

/// The audit page size when a request names none.
const DEFAULT_AUDIT_PAGE: usize = 100;

/// The largest audit page: a request asking for more reads this many.
const MAX_AUDIT_PAGE: usize = 1_000;

/// Whether `method` belongs to the family.
#[must_use]
pub(crate) fn is_tool_grant_method(method: &str) -> bool {
    TOOL_GRANT_METHODS.contains(&method)
}

/// Whether `method` changes grants: a read-only Console and a candidate
/// launch refuse it, and the Console gates it on `agent.tool_grants.manage`.
#[must_use]
pub(crate) fn is_tool_grant_mutating_method(method: &str) -> bool {
    matches!(method, TOOL_GRANTS_GRANT | TOOL_GRANTS_REVOKE)
}

/// What the surface a call arrived on hands the family.
pub(crate) struct ToolGrantSurface<'a> {
    /// The gateway's mob runtime: its mob, and the session service whose
    /// runtime owns the grants.
    pub(crate) mob: &'a MobRuntime,
    /// The gateway's identity runtime, when it serves identities.
    pub(crate) identity_runtime: Option<&'a Arc<IdentityRuntime>>,
    /// The issuer's caller recorded on a grant, and the actor of a revoke:
    /// the Console's signed-in principal. `None` on the stdio gateway, whose
    /// host is the operator.
    pub(crate) caller: Option<&'a str>,
    /// The Console caller's access view. A revoke names no identity, so the
    /// family checks `agent.tool_grants.manage` on the grant's identity once
    /// it has read the grant. `None` on the stdio gateway.
    pub(crate) access: Option<&'a AccessView>,
}

/// Serve `method` of the family: `None` for a method outside it.
pub(crate) async fn handle_tool_grant_rpc(
    surface: &ToolGrantSurface<'_>,
    method: &str,
    response_id: Value,
    params: &Value,
) -> Option<JsonRpcResponse> {
    let answer = match method {
        TOOL_GRANTS_GRANT => grant(surface, params).await,
        TOOL_GRANTS_REVOKE => revoke(surface, params).await,
        TOOL_GRANTS_LIST => list(surface, params).await,
        TOOL_GRANTS_AUDIT => audit(surface, params).await,
        _ => return None,
    };
    let (result, error) = match answer {
        Ok(result) => (Some(result), None),
        Err(error) => (None, Some(error)),
    };
    Some(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result,
        error,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantParams {
    identity: String,
    target: ToolGrantTargetRequest,
    #[serde(default)]
    expires_at_ms: Option<u64>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    decision_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeParams {
    grant_id: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    identity: String,
    #[serde(default)]
    include_ended: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditParams {
    identity: String,
    #[serde(default)]
    after_sequence: Option<u64>,
    #[serde(default)]
    limit: Option<u32>,
}

fn parse<T: serde::de::DeserializeOwned>(params: &Value) -> Result<T, JsonRpcError> {
    serde_json::from_value(params.clone()).map_err(|error| JsonRpcError {
        code: -32602,
        message: format!("Invalid params: {error}"),
        data: None,
    })
}

/// The typed refusal of a grant request: invalid params carrying
/// `{"kind": "tool_grant_request_refused", "tool_grant_request_refusal": ...}`.
fn refused(refusal: &ToolGrantRequestRefusal, message: impl Into<String>) -> JsonRpcError {
    JsonRpcError {
        code: -32602,
        message: message.into(),
        data: Some(refusal.data()),
    }
}

fn subject_unknown(message: impl Into<String>) -> JsonRpcError {
    refused(&ToolGrantRequestRefusal::SubjectUnknown, message)
}

/// The answer of a grant service error, classified as Core's
/// `tool_grants/*` classifies it: the request's own faults are typed
/// refusals, a row two writers kept changing is the busy class a retry
/// resolves, a store without grants is a capability this runtime lacks, and
/// the rest is the server's fault.
fn service_error(error: ToolGrantServiceError) -> JsonRpcError {
    let refusal = match &error {
        ToolGrantServiceError::InvalidWindow { .. } => Some(ToolGrantRequestRefusal::InvalidWindow),
        ToolGrantServiceError::EmptyTarget => Some(ToolGrantRequestRefusal::EmptyTarget),
        ToolGrantServiceError::NotFound { grant_id } => Some(ToolGrantRequestRefusal::NotFound {
            grant_id: grant_id.clone(),
        }),
        _ => None,
    };
    if let Some(refusal) = refusal {
        return refused(&refusal, error.to_string());
    }
    let code = match &error {
        ToolGrantServiceError::Conflict { .. } => {
            i64::from(meerkat_contracts::ErrorCode::SessionBusy.jsonrpc_code())
        }
        ToolGrantServiceError::Store(meerkat_runtime::RuntimeStoreError::Unsupported(_)) => {
            CAPABILITY_UNAVAILABLE_CODE
        }
        _ => -32603,
    };
    JsonRpcError {
        code,
        message: error.to_string(),
        data: None,
    }
}

/// The answer of a gateway whose runtime keeps no scoped tool grants.
fn runtime_keeps_no_grants() -> JsonRpcError {
    JsonRpcError {
        code: CAPABILITY_UNAVAILABLE_CODE,
        message: "this gateway's runtime keeps no scoped tool grants".to_string(),
        data: None,
    }
}

/// The runtime that owns the gateway's grants: the canonical runtime of the
/// mob's session service.
fn grant_owner(surface: &ToolGrantSurface<'_>) -> Result<Arc<MeerkatMachine>, JsonRpcError> {
    let service = surface
        .mob
        .session_service()
        .ok_or_else(runtime_keeps_no_grants)?;
    match service.acquire_runtime_adapter(None) {
        Ok(Some(machine)) => Ok(machine),
        Ok(None) => Err(runtime_keeps_no_grants()),
        Err(error) => Err(JsonRpcError {
            code: -32603,
            message: format!("the runtime that owns the grants is unavailable: {error}"),
            data: None,
        }),
    }
}

/// The Rust error of a mob read, as the gateway answers Meerkat's mob errors.
fn mob_error(error: &meerkat_mob::MobError) -> JsonRpcError {
    match crate::identity_first::runtime::mob_error_wire(error) {
        Ok((code, data)) => JsonRpcError {
            code,
            message: error.to_string(),
            data,
        },
        Err(serialize_error) => JsonRpcError {
            code: -32603,
            message: format!("failed to serialize mob error detail: {serialize_error}"),
            data: None,
        },
    }
}

/// The answer of a member grant the mob did not issue: an identity no
/// longer on the roster is `subject_unknown`, as a gate read answers one
/// outside it; the grant service's refusals answer their classes; a mob
/// without a runtime keeps no grants.
fn member_grant_error(error: meerkat_mob::MemberToolGrantError, identity: &str) -> JsonRpcError {
    match error {
        meerkat_mob::MemberToolGrantError::Mob(meerkat_mob::MobError::MemberNotFound(_)) => {
            subject_unknown(format!(
                "'{identity}' is no longer on the mob's roster (it retired, or a respawn is \
                 replacing its member): no grant was issued"
            ))
        }
        meerkat_mob::MemberToolGrantError::Mob(error) => mob_error(&error),
        meerkat_mob::MemberToolGrantError::Grant(error) => service_error(error),
        meerkat_mob::MemberToolGrantError::NoRuntime => runtime_keeps_no_grants(),
    }
}

/// The public alias of a roster member id, as every Meta answer spells an
/// identity.
fn public_identity(member_id: &str) -> String {
    crate::member_comms_id::runtime_alias_str(member_id).into_owned()
}

/// A grant record with its identity spelled as the caller named it.
fn public_grant(mut grant: WireToolGrant) -> WireToolGrant {
    if let WireToolGrantSubject::Identity { identity, .. } = &mut grant.subject {
        *identity = public_identity(identity);
    }
    grant
}

/// An identity a grant may name: its member in Core's roster, the gate its
/// target must be in with the MCP servers its build configures ungated, and
/// the session serving it here, if any.
struct GrantSubject {
    member: meerkat_mob::ids::AgentIdentity,
    gate: meerkat_core::tool_grant::GrantGatedTools,
    ungated_servers: BTreeSet<String>,
    serving: Option<meerkat_core::types::SessionId>,
}

/// The identity a grant names, resolved to Core's member. It must be one the
/// identity runtime serves and not store-owned; Core's member of it is
/// seated through the declared apply's door when Core holds none.
async fn identity_for_grant(
    surface: &ToolGrantSurface<'_>,
    identity: &str,
) -> Result<GrantSubject, JsonRpcError> {
    let unknown = || subject_unknown(format!("no identity '{identity}' is served here"));
    let identity_runtime = surface.identity_runtime.ok_or_else(unknown)?;
    let parsed = AgentIdentity::parse(identity).map_err(|_| unknown())?;
    identity_runtime
        .registered_spec(&parsed)
        .await
        .map_err(|_| unknown())?;
    let handle = surface.mob.handle();
    let member = crate::member_comms_id::roster_member_id_for_identity(identity);
    if matches!(
        handle.identity_entry(&member),
        Ok(meerkat_mob::IdentityEntryRead::Entry(entry))
            if entry.origin == meerkat_mob::IdentityEntryOrigin::Created
    ) {
        return Err(subject_unknown(format!(
            "'{identity}' is a store-owned identity: Meta's grant methods do not act on \
             store-owned identities yet"
        )));
    }
    let gate = match handle.member_grant_gate(&member).await {
        Ok(gate) => gate,
        Err(meerkat_mob::MobError::MemberNotFound(_)) => {
            identity_runtime
                .seat_for_tool_grant(&parsed)
                .await
                .map_err(|error| crate::rpc::identity_json_rpc_error(&error))?;
            match handle.member_grant_gate(&member).await {
                Ok(gate) => gate,
                Err(meerkat_mob::MobError::MemberNotFound(_)) => {
                    return Err(subject_unknown(format!(
                        "Core holds no member of '{identity}': its host creates it at its \
                         first use"
                    )));
                }
                Err(error) => return Err(mob_error(&error)),
            }
        }
        Err(error) => return Err(mob_error(&error)),
    };
    Ok(GrantSubject {
        gate: gate.named(),
        ungated_servers: gate.ungated_servers,
        serving: handle.resolve_bridge_session_id(&member).await,
        member,
    })
}

/// Issue `grant` to the member `subject` names through the mob, which decides
/// it against its live roster as it commits, in order with the retirements
/// that end an identity's grants: a member no longer on the roster then is
/// refused, and nothing is issued.
async fn commit_member_grant(
    surface: &ToolGrantSurface<'_>,
    identity: &str,
    subject: &GrantSubject,
    grant: meerkat_mob::MemberToolGrant,
) -> Result<StoredToolGrant, JsonRpcError> {
    surface
        .mob
        .handle()
        .grant_member_tool(&subject.member, grant, subject.serving.as_ref())
        .await
        .map_err(|error| member_grant_error(error, identity))
}

/// The identity a read names: the mob only, so a retired identity's history
/// stays readable.
async fn identity_for_read(
    surface: &ToolGrantSurface<'_>,
    identity: &str,
) -> Result<(ToolGrantSubject, Option<meerkat_core::types::SessionId>), JsonRpcError> {
    AgentIdentity::parse(identity)
        .map_err(|_| subject_unknown(format!("'{identity}' is not an identity")))?;
    let handle = surface.mob.handle();
    let member = crate::member_comms_id::roster_member_id_for_identity(identity);
    let serving = handle.resolve_bridge_session_id(&member).await;
    Ok((
        ToolGrantSubject::member(
            handle.mob_id().as_str(),
            &handle.live_incarnation(),
            member.as_str(),
        ),
        serving,
    ))
}

fn result<T: serde::Serialize>(value: &T) -> Result<Value, JsonRpcError> {
    serde_json::to_value(value).map_err(|error| JsonRpcError {
        code: -32603,
        message: format!("failed to serialize the answer: {error}"),
        data: None,
    })
}

async fn grant(surface: &ToolGrantSurface<'_>, params: &Value) -> Result<Value, JsonRpcError> {
    let params: GrantParams = parse(params)?;
    let window = match (params.expires_at_ms, params.duration_ms) {
        (Some(expires_at_ms), None) => ToolGrantWindow::Until { expires_at_ms },
        (None, Some(duration_ms)) => ToolGrantWindow::For(Duration::from_millis(duration_ms)),
        _ => {
            return Err(refused(
                &ToolGrantRequestRefusal::InvalidWindow,
                "a tool grant is bounded by exactly one of expires_at_ms and duration_ms",
            ));
        }
    };
    let decision_ref = params
        .decision_ref
        .map(DecisionReference::new)
        .transpose()
        .map_err(|error| {
            refused(
                &ToolGrantRequestRefusal::InvalidDecisionRef,
                error.to_string(),
            )
        })?;
    // A runtime that keeps no grants answers before any member is seated.
    grant_owner(surface)?;
    let subject = identity_for_grant(surface, &params.identity).await?;
    let named = match &params.target {
        ToolGrantTargetRequest::Tool { name } => format!("the tool '{name}'"),
        ToolGrantTargetRequest::Family { family } => format!("the tool family '{family}'"),
        ToolGrantTargetRequest::McpServer { server, .. } => format!("the MCP server '{server}'"),
    };
    let target =
        meerkat::resolve_grant_target(params.target, &subject.gate, &subject.ungated_servers)
            .map_err(|refusal| {
                let message = match refusal {
                    ToolGrantRequestRefusal::TargetNotGated => format!(
                        "the gate of '{}' does not name {named}: a grant opens only what the \
                         member's build gates",
                        params.identity
                    ),
                    _ => refusal.to_string(),
                };
                refused(&refusal, message)
            })?;
    let grant = meerkat_mob::MemberToolGrant {
        target,
        issuer: ToolGrantIssuer::Operator {
            caller: surface.caller.map(ToString::to_string),
            decision_ref,
        },
        reason: params.reason,
        window,
    };
    let grant = commit_member_grant(surface, &params.identity, &subject, grant).await?;
    result(&GrantToolResult {
        grant: public_grant(wire_tool_grant(&grant)),
        applies: None,
    })
}

async fn revoke(surface: &ToolGrantSurface<'_>, params: &Value) -> Result<Value, JsonRpcError> {
    let params: RevokeParams = parse(params)?;
    let not_found = |message: String| {
        refused(
            &ToolGrantRequestRefusal::NotFound {
                grant_id: params.grant_id.clone(),
            },
            message,
        )
    };
    let grant_id =
        ToolGrantId::parse(&params.grant_id).map_err(|error| not_found(error.to_string()))?;
    // A grant the caller may not know of answers as Core answers an unknown
    // id, so the answer tells nothing of it.
    let unknown = || {
        service_error(ToolGrantServiceError::NotFound {
            grant_id: grant_id.to_string(),
        })
    };
    let owner = grant_owner(surface)?;
    let stored = owner.tool_grant(&grant_id).await.map_err(service_error)?;
    let handle = surface.mob.handle();
    let identity = match &stored.record.subject {
        ToolGrantSubject::Identity {
            mob_id, identity, ..
        } if mob_id == handle.mob_id().as_str() => public_identity(identity),
        _ => return Err(unknown()),
    };
    if let Some(view) = surface.access.filter(|view| view.enforced()) {
        if !view.can_view_agent(&identity) {
            return Err(unknown());
        }
        if !view.allows_agent(ACTION_AGENT_TOOL_GRANTS_MANAGE, &identity) {
            return Err(crate::http_console::member_access_denied(
                ACTION_AGENT_TOOL_GRANTS_MANAGE,
                Some(identity),
            ));
        }
    }
    let revoked = owner
        .revoke_tool_grant(
            &grant_id,
            ToolGrantRevocation {
                actor: surface.caller.map(ToString::to_string),
                reason: params.reason,
            },
        )
        .await
        .map_err(service_error)?;
    result(&RevokeToolGrantResult {
        grant: public_grant(wire_tool_grant(&revoked.grant)),
        changed: revoked.changed,
    })
}

async fn list(surface: &ToolGrantSurface<'_>, params: &Value) -> Result<Value, JsonRpcError> {
    let params: ListParams = parse(params)?;
    let owner = grant_owner(surface)?;
    let (subject, serving) = identity_for_read(surface, &params.identity).await?;
    let listing = owner
        .list_tool_grants(&subject, serving.as_ref())
        .await
        .map_err(service_error)?;
    result(&ListToolGrantsResult {
        grants: listing
            .grants
            .iter()
            .filter(|grant| params.include_ended || matches!(grant.status, ToolGrantStatus::Active))
            .map(|grant| public_grant(wire_tool_grant(grant)))
            .collect(),
        unreadable: listing
            .unreadable
            .iter()
            .map(wire_unreadable_tool_grant)
            .collect(),
        gate: None,
    })
}

/// The rows an audit page reads: the default without a limit, a limit of 0
/// refused, any other limit capped at the largest page.
fn audit_page_limit(limit: Option<u32>) -> Result<usize, JsonRpcError> {
    match limit {
        None => Ok(DEFAULT_AUDIT_PAGE),
        Some(0) => Err(refused(
            &ToolGrantRequestRefusal::InvalidPageLimit,
            "an audit page limit must be at least 1",
        )),
        Some(limit) => Ok(usize::try_from(limit)
            .unwrap_or(MAX_AUDIT_PAGE)
            .min(MAX_AUDIT_PAGE)),
    }
}

async fn audit(surface: &ToolGrantSurface<'_>, params: &Value) -> Result<Value, JsonRpcError> {
    let params: AuditParams = parse(params)?;
    let limit = audit_page_limit(params.limit)?;
    let owner = grant_owner(surface)?;
    let (subject, _serving) = identity_for_read(surface, &params.identity).await?;
    let page = owner
        .read_tool_grant_audit(&subject, params.after_sequence, limit)
        .await
        .map_err(service_error)?;
    let next_sequence = if page.len() == limit {
        page.last_sequence()
    } else {
        None
    };
    result(&ToolGrantAuditResult {
        rows: page.rows.iter().map(wire_tool_grant_audit_row).collect(),
        unreadable: page
            .unreadable
            .iter()
            .map(wire_unreadable_tool_grant_audit_row)
            .collect(),
        next_sequence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The family's registry and its read/mutate split.
    #[test]
    fn grant_and_revoke_mutate_and_list_and_audit_read() {
        for method in TOOL_GRANT_METHODS {
            assert!(is_tool_grant_method(method), "{method}");
        }
        assert!(!is_tool_grant_method("meta/tool_grants/attach"));
        assert!(is_tool_grant_mutating_method(TOOL_GRANTS_GRANT));
        assert!(is_tool_grant_mutating_method(TOOL_GRANTS_REVOKE));
        assert!(!is_tool_grant_mutating_method(TOOL_GRANTS_LIST));
        assert!(!is_tool_grant_mutating_method(TOOL_GRANTS_AUDIT));
    }

    /// A grant record names the identity in its public spelling, not the
    /// roster's comms-safe encoding.
    #[test]
    fn a_grant_names_its_identity_as_the_caller_named_it() {
        let encoded = crate::member_comms_id::roster_member_id_for_identity("team:alice");
        let grant = public_grant(WireToolGrant {
            grant_id: "g1".to_string(),
            subject: WireToolGrantSubject::Identity {
                mob_id: "mob".to_string(),
                identity: encoded.as_str().to_string(),
            },
            target: meerkat_contracts::WireToolGrantTarget::Tool {
                name: "datetime".to_string(),
            },
            issuer: meerkat_contracts::WireToolGrantIssuer::Operator {
                caller: None,
                decision_ref: None,
            },
            reason: None,
            status: meerkat_contracts::WireToolGrantStatus::Active,
            issued_at_ms: 1,
            expires_at_ms: 2,
            ended_at_ms: None,
        });
        assert_eq!(
            grant.subject,
            WireToolGrantSubject::Identity {
                mob_id: "mob".to_string(),
                identity: "team:alice".to_string(),
            }
        );
    }

    /// The service's request faults are typed refusals; a conflict is the
    /// busy class Core answers, and a store without grants a missing
    /// capability.
    #[test]
    fn grant_service_errors_answer_their_classes() {
        let not_found = service_error(ToolGrantServiceError::NotFound {
            grant_id: "g1".to_string(),
        });
        assert_eq!(not_found.code, -32602);
        assert_eq!(
            not_found.data,
            Some(json!({
                "kind": "tool_grant_request_refused",
                "tool_grant_request_refusal": {"kind": "not_found", "grant_id": "g1"},
            }))
        );
        assert_eq!(
            service_error(ToolGrantServiceError::Conflict {
                grant_id: "g1".to_string()
            })
            .code,
            i64::from(meerkat_contracts::ErrorCode::SessionBusy.jsonrpc_code())
        );
        assert_eq!(
            service_error(ToolGrantServiceError::Store(
                meerkat_runtime::RuntimeStoreError::Unsupported("list_tool_grants".into())
            ))
            .code,
            CAPABILITY_UNAVAILABLE_CODE
        );
        assert_eq!(service_error(ToolGrantServiceError::Clock).code, -32603);
    }

    /// An audit page reads the default without a limit, refuses a limit of 0
    /// and caps any other limit at the largest page.
    #[test]
    fn an_audit_page_limit_is_defaulted_refused_at_zero_and_capped() {
        assert_eq!(
            audit_page_limit(None).ok(),
            Some(DEFAULT_AUDIT_PAGE),
            "no limit reads the default page"
        );
        assert_eq!(
            audit_page_limit(Some(0)).err().and_then(|error| error.data),
            Some(json!({
                "kind": "tool_grant_request_refused",
                "tool_grant_request_refusal": {"kind": "invalid_page_limit"},
            }))
        );
        assert_eq!(audit_page_limit(Some(1)).ok(), Some(1));
        assert_eq!(audit_page_limit(Some(1_000)).ok(), Some(1_000));
        for over in [1_001, u32::MAX] {
            assert_eq!(
                audit_page_limit(Some(over)).ok(),
                Some(MAX_AUDIT_PAGE),
                "{over} reads the largest page"
            );
        }
    }

    /// A member grant the mob did not issue answers as a gate read answers
    /// the same facts: an identity no longer on the roster is
    /// `subject_unknown`, a grant service refusal answers its class, and a
    /// mob without a runtime keeps no grants.
    #[test]
    fn a_member_grant_the_mob_refused_answers_its_class() {
        let gone = member_grant_error(
            meerkat_mob::MemberToolGrantError::Mob(meerkat_mob::MobError::MemberNotFound(
                crate::member_comms_id::roster_member_id_for_identity("agent-a"),
            )),
            "agent-a",
        );
        assert_eq!(gone.code, -32602);
        assert_eq!(
            gone.data,
            Some(json!({
                "kind": "tool_grant_request_refused",
                "tool_grant_request_refusal": {"kind": "subject_unknown"},
            }))
        );
        assert_eq!(
            member_grant_error(
                meerkat_mob::MemberToolGrantError::Grant(ToolGrantServiceError::Conflict {
                    grant_id: "g1".to_string(),
                }),
                "agent-a",
            )
            .code,
            i64::from(meerkat_contracts::ErrorCode::SessionBusy.jsonrpc_code())
        );
        assert_eq!(
            member_grant_error(meerkat_mob::MemberToolGrantError::NoRuntime, "agent-a").code,
            CAPABILITY_UNAVAILABLE_CODE
        );
    }

    /// A grant for an identity no longer on the mob's roster answers
    /// `subject_unknown` and issues nothing. The gate read found the member;
    /// a retirement then took it off the roster; the commit, which the mob
    /// decides against its live roster, refuses it. The runtime's own writer
    /// does not read the roster, so a grant written there would outlive the
    /// retirement.
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    async fn a_grant_for_an_identity_no_longer_on_the_roster_issues_nothing() {
        let state = tempfile::tempdir().unwrap();
        let mob_id = format!("meta-grant-roster-{}", uuid::Uuid::new_v4());
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
builtins = true
grant_gated = {{ tools = ["datetime"] }}
"#
        ))
        .unwrap();
        let unified = Box::pin(
            crate::UnifiedRuntimeBuilder::default()
                .definition(definition)
                .persistent_state(state.path())
                .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
                .build(),
        )
        .await
        .unwrap();
        let surface = ToolGrantSurface {
            mob: unified.mob_runtime(),
            identity_runtime: None,
            caller: None,
            access: None,
        };
        let handle = unified.mob_runtime().handle();
        let member = crate::member_comms_id::roster_member_id_for_identity("agent-a");
        handle
            .spawn_spec(meerkat_mob::SpawnMemberSpec::from_wire(
                "worker".to_string(),
                member.as_str().to_string(),
                None,
                None,
                None,
            ))
            .await
            .unwrap();
        let gate = handle.member_grant_gate(&member).await.unwrap();
        let subject = GrantSubject {
            member: member.clone(),
            gate: gate.named(),
            ungated_servers: gate.ungated_servers,
            serving: handle.resolve_bridge_session_id(&member).await,
        };
        handle.retire(member.clone()).await.unwrap();

        let refused = commit_member_grant(
            &surface,
            "agent-a",
            &subject,
            meerkat_mob::MemberToolGrant {
                target: meerkat_core::tool_grant::ToolGrantTarget::Tool {
                    name: meerkat_core::types::ToolName::from("datetime"),
                },
                issuer: ToolGrantIssuer::Operator {
                    caller: None,
                    decision_ref: None,
                },
                reason: None,
                window: ToolGrantWindow::For(Duration::from_mins(10)),
            },
        )
        .await
        .expect_err("a member no longer on the roster is granted nothing");
        assert_eq!(
            refused.data,
            Some(json!({
                "kind": "tool_grant_request_refused",
                "tool_grant_request_refusal": {"kind": "subject_unknown"},
            })),
            "{refused:?}"
        );
        let listed = grant_owner(&surface)
            .unwrap()
            .list_tool_grants(
                &ToolGrantSubject::member(
                    handle.mob_id().as_str(),
                    &handle.live_incarnation(),
                    member.as_str(),
                ),
                None,
            )
            .await
            .unwrap();
        assert!(
            listed.grants.is_empty(),
            "nothing was issued: {:?}",
            listed.grants
        );
        let _ = unified.shutdown().await;
    }
}
