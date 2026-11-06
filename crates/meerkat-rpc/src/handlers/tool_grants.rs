//! `tool_grants/*` JSON-RPC handlers: the scoped tool grants of a session or
//! of a mob member identity.
//!
//! Every method reads and writes grant rows through the runtime's grant
//! service only: it never loads, registers or wakes the subject's session or
//! member. A session grant reads the gate the session records (its staged
//! build before or during its first turn, its persisted metadata after). An
//! identity grant reads the gate the member's next build resolves, its own
//! gate joined with its role's current gate, and needs the identity in the
//! mob's roster. List and audit resolve the mob only, so a retired
//! identity's history stays readable. Every refusal of these methods is the
//! typed `tool_grant_request_refused` refusal.

use std::sync::Arc;
use std::time::Duration;

use meerkat_contracts::{
    GrantToolParams, GrantToolResult, ListToolGrantsParams, ListToolGrantsResult,
    RevokeToolGrantParams, RevokeToolGrantResult, ToolGrantAuditParams, ToolGrantAuditResult,
    ToolGrantSubjectRequest, ToolGrantTargetRequest,
};
use meerkat_core::SessionId;
use meerkat_core::approval::decision::DecisionReference;
use meerkat_core::tool_grant::{
    GrantGatedTools, ToolGrantId, ToolGrantIssuer, ToolGrantRequestRefusal, ToolGrantSubject,
};
use meerkat_runtime::RuntimeStoreError;
use meerkat_runtime::tool_grant::wire::{
    wire_tool_grant, wire_tool_grant_audit_row, wire_unreadable_tool_grant,
    wire_unreadable_tool_grant_audit_row,
};
use meerkat_runtime::tool_grant::{
    ToolGrantRequest, ToolGrantRevocation, ToolGrantServiceError, ToolGrantStatus, ToolGrantWindow,
};

use crate::error;
use crate::handlers::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

/// The audit page size when a request names none.
const DEFAULT_AUDIT_PAGE: usize = 100;

/// The largest audit page: a request asking for more reads this many.
const MAX_AUDIT_PAGE: usize = 1_000;

/// The typed refusal of a grant request: invalid params carrying
/// `{"kind": "tool_grant_request_refused", "tool_grant_request_refusal": ...}`.
fn refused(
    id: Option<RpcId>,
    refusal: &ToolGrantRequestRefusal,
    message: impl Into<String>,
) -> RpcResponse {
    RpcResponse::error_with_data(id, error::INVALID_PARAMS, message, refusal.data())
}

fn grant_error_to_response(id: Option<RpcId>, error_value: ToolGrantServiceError) -> RpcResponse {
    let refusal = match &error_value {
        ToolGrantServiceError::InvalidWindow { .. } => Some(ToolGrantRequestRefusal::InvalidWindow),
        ToolGrantServiceError::EmptyTarget => Some(ToolGrantRequestRefusal::EmptyTarget),
        ToolGrantServiceError::NotFound { grant_id } => Some(ToolGrantRequestRefusal::NotFound {
            grant_id: grant_id.clone(),
        }),
        _ => None,
    };
    if let Some(refusal) = refusal {
        return refused(id, &refusal, error_value.to_string());
    }
    let code = match &error_value {
        // Concurrent writers changed the row under this one twice: the
        // busy class, which a retry resolves by reading the row afresh.
        ToolGrantServiceError::Conflict { .. } => error::SESSION_BUSY,
        ToolGrantServiceError::Store(RuntimeStoreError::Unsupported(_)) => {
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code()
        }
        // The clock, a row this binary cannot read, the machine or the
        // store: the server's fault.
        _ => error::INTERNAL_ERROR,
    };
    RpcResponse::error(id, code, error_value.to_string())
}

/// The session a session subject names. An id that names no session is the
/// typed `subject_unknown`.
#[allow(clippy::result_large_err)]
fn session_of(
    id: Option<RpcId>,
    raw: &str,
    runtime: &SessionRuntime,
) -> Result<SessionId, RpcResponse> {
    parse_session_id_for_runtime(id.clone(), raw, runtime).map_err(|response| {
        let message = response
            .error
            .map(|error| error.message)
            .unwrap_or_else(|| format!("no session '{raw}'"));
        refused(id, &ToolGrantRequestRefusal::SubjectUnknown, message)
    })
}

/// A write's subject, resolved: whose grants, the gate a target must be in,
/// and the session serving the subject here when there is one.
struct GrantSubject {
    subject: ToolGrantSubject,
    gate: GrantGatedTools,
    serving: Option<SessionId>,
}

async fn subject_for_grant(
    id: Option<RpcId>,
    request: &ToolGrantSubjectRequest,
    runtime: &SessionRuntime,
) -> Result<GrantSubject, RpcResponse> {
    match request {
        ToolGrantSubjectRequest::Session { session_id } => {
            let session_id = session_of(id.clone(), session_id, runtime)?;
            let gate = match runtime.session_grant_gate(&session_id).await {
                Ok(Some(gate)) => gate,
                Ok(None) => {
                    return Err(refused(
                        id,
                        &ToolGrantRequestRefusal::SubjectUnknown,
                        format!("Session not found: {session_id}"),
                    ));
                }
                Err(error_value) => return Err(RpcResponse::from_error(id, error_value)),
            };
            Ok(GrantSubject {
                subject: ToolGrantSubject::session(session_id.clone()),
                gate,
                serving: Some(session_id),
            })
        }
        ToolGrantSubjectRequest::Identity { mob_id, identity } => {
            members::identity_for_grant(id, mob_id, identity, runtime).await
        }
    }
}

/// A read's subject, resolved: whose grants, and the session serving the
/// subject here when there is one. A session subject is not looked up, and
/// an identity subject resolves its mob only.
async fn subject_for_read(
    id: Option<RpcId>,
    request: &ToolGrantSubjectRequest,
    runtime: &SessionRuntime,
) -> Result<(ToolGrantSubject, Option<SessionId>), RpcResponse> {
    match request {
        ToolGrantSubjectRequest::Session { session_id } => {
            let session_id = session_of(id, session_id, runtime)?;
            Ok((
                ToolGrantSubject::session(session_id.clone()),
                Some(session_id),
            ))
        }
        ToolGrantSubjectRequest::Identity { mob_id, identity } => {
            members::identity_for_read(id, mob_id, identity, runtime).await
        }
    }
}

/// Identity subjects: the members of the mobs this server runs.
#[cfg(feature = "mob")]
mod members {
    use super::*;

    /// The handle of the mob `mob_id`; a mob this server does not run is
    /// the typed `subject_unknown`.
    async fn mob_handle(
        id: Option<RpcId>,
        mob_id: &str,
        runtime: &SessionRuntime,
    ) -> Result<meerkat_mob::MobHandle, RpcResponse> {
        let unknown = || {
            refused(
                id.clone(),
                &ToolGrantRequestRefusal::SubjectUnknown,
                format!("no mob '{mob_id}'"),
            )
        };
        let Some(state) = runtime.mob_state() else {
            return Err(unknown());
        };
        match state.handle_for(&meerkat_mob::MobId::from(mob_id)).await {
            Ok(handle) => Ok(handle),
            Err(meerkat_mob::MobError::MobNotFound(_)) => Err(unknown()),
            Err(error_value) => Err(crate::handlers::mob::mob_call_error(id, &error_value)),
        }
    }

    pub(super) async fn identity_for_grant(
        id: Option<RpcId>,
        mob_id: &str,
        identity: &str,
        runtime: &SessionRuntime,
    ) -> Result<GrantSubject, RpcResponse> {
        let handle = mob_handle(id.clone(), mob_id, runtime).await?;
        let member = meerkat_mob::AgentIdentity::from(identity);
        let gate = match handle.member_grant_gate(&member).await {
            Ok(gate) => gate,
            Err(meerkat_mob::MobError::MemberNotFound(_)) => {
                return Err(refused(
                    id,
                    &ToolGrantRequestRefusal::SubjectUnknown,
                    format!("no member '{identity}' in mob '{mob_id}'"),
                ));
            }
            Err(error_value) => return Err(crate::handlers::mob::mob_call_error(id, &error_value)),
        };
        Ok(GrantSubject {
            subject: ToolGrantSubject::member(mob_id, &handle.live_incarnation(), identity),
            gate: gate.named(),
            serving: handle.resolve_bridge_session_id(&member).await,
        })
    }

    pub(super) async fn identity_for_read(
        id: Option<RpcId>,
        mob_id: &str,
        identity: &str,
        runtime: &SessionRuntime,
    ) -> Result<(ToolGrantSubject, Option<SessionId>), RpcResponse> {
        let handle = mob_handle(id, mob_id, runtime).await?;
        let serving = handle
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(identity))
            .await;
        Ok((
            ToolGrantSubject::member(mob_id, &handle.live_incarnation(), identity),
            serving,
        ))
    }
}

/// Identity subjects: a server without mobs has none.
#[cfg(not(feature = "mob"))]
mod members {
    use super::*;

    fn unsupported(id: Option<RpcId>) -> RpcResponse {
        RpcResponse::error(
            id,
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            "this server runs no mobs: identity subjects are unsupported",
        )
    }

    pub(super) async fn identity_for_grant(
        id: Option<RpcId>,
        _mob_id: &str,
        _identity: &str,
        _runtime: &SessionRuntime,
    ) -> Result<GrantSubject, RpcResponse> {
        Err(unsupported(id))
    }

    pub(super) async fn identity_for_read(
        id: Option<RpcId>,
        _mob_id: &str,
        _identity: &str,
        _runtime: &SessionRuntime,
    ) -> Result<(ToolGrantSubject, Option<SessionId>), RpcResponse> {
        Err(unsupported(id))
    }
}

/// Handle `tool_grants/grant`.
pub async fn handle_grant(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: GrantToolParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let window = match (params.expires_at_ms, params.duration_ms) {
        (Some(expires_at_ms), None) => ToolGrantWindow::Until { expires_at_ms },
        (None, Some(duration_ms)) => ToolGrantWindow::For(Duration::from_millis(duration_ms)),
        _ => {
            return refused(
                id,
                &ToolGrantRequestRefusal::InvalidWindow,
                "a tool grant is bounded by exactly one of expires_at_ms and duration_ms",
            );
        }
    };
    let decision_ref = match params.decision_ref.map(DecisionReference::new).transpose() {
        Ok(decision_ref) => decision_ref,
        Err(error_value) => {
            return refused(
                id,
                &ToolGrantRequestRefusal::InvalidDecisionRef,
                error_value.to_string(),
            );
        }
    };
    let subject = match subject_for_grant(id.clone(), &params.subject, &runtime).await {
        Ok(subject) => subject,
        Err(response) => return response,
    };
    let named = match &params.target {
        ToolGrantTargetRequest::Tool { name } => format!("the tool '{name}'"),
        ToolGrantTargetRequest::Family { family } => format!("the tool family '{family}'"),
    };
    let target = match meerkat::resolve_grant_target(params.target, &subject.gate) {
        Ok(target) => target,
        Err(refusal) => {
            let message = match refusal {
                ToolGrantRequestRefusal::TargetNotGated => format!(
                    "the subject's grant gate does not name {named}: a grant opens only names \
                     the subject's build gates"
                ),
                _ => refusal.to_string(),
            };
            return refused(id, &refusal, message);
        }
    };
    let request = ToolGrantRequest {
        subject: subject.subject,
        target,
        issuer: ToolGrantIssuer::Operator {
            caller: None,
            decision_ref,
        },
        reason: params.reason,
        window,
    };
    match runtime
        .runtime_adapter()
        .grant_tool(request, subject.serving.as_ref())
        .await
    {
        Ok(grant) => RpcResponse::success(
            id,
            GrantToolResult {
                grant: wire_tool_grant(&grant),
            },
        ),
        Err(error_value) => grant_error_to_response(id, error_value),
    }
}

/// Handle `tool_grants/revoke`. Idempotent: revoking a grant that already
/// ended answers it as it stands, with `changed` false.
pub async fn handle_revoke(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: RevokeToolGrantParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let grant_id = match ToolGrantId::parse(&params.grant_id) {
        Ok(grant_id) => grant_id,
        Err(error_value) => {
            return refused(
                id,
                &ToolGrantRequestRefusal::NotFound {
                    grant_id: params.grant_id,
                },
                error_value.to_string(),
            );
        }
    };
    let revocation = ToolGrantRevocation {
        actor: None,
        reason: params.reason,
    };
    match runtime
        .runtime_adapter()
        .revoke_tool_grant(&grant_id, revocation)
        .await
    {
        Ok(revoked) => RpcResponse::success(
            id,
            RevokeToolGrantResult {
                grant: wire_tool_grant(&revoked.grant),
                changed: revoked.changed,
            },
        ),
        Err(error_value) => grant_error_to_response(id, error_value),
    }
}

/// Handle `tool_grants/list`. Each due grant is settled as expired first;
/// ended grants are listed only when the request asks for them.
pub async fn handle_list(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ListToolGrantsParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let (subject, serving) = match subject_for_read(id.clone(), &params.subject, &runtime).await {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };
    match runtime
        .runtime_adapter()
        .list_tool_grants(&subject, serving.as_ref())
        .await
    {
        Ok(listing) => RpcResponse::success(
            id,
            ListToolGrantsResult {
                grants: listing
                    .grants
                    .iter()
                    .filter(|grant| {
                        params.include_ended || matches!(grant.status, ToolGrantStatus::Active)
                    })
                    .map(wire_tool_grant)
                    .collect(),
                unreadable: listing
                    .unreadable
                    .iter()
                    .map(wire_unreadable_tool_grant)
                    .collect(),
            },
        ),
        Err(error_value) => grant_error_to_response(id, error_value),
    }
}

/// Handle `tool_grants/audit`: one page of the subject's audit log, in
/// sequence order. A row the server cannot read is reported in the page's
/// `unreadable`, and the rest of the page is served. A full page names its
/// last sequence, readable or not, as the next page's `after_sequence`.
pub async fn handle_audit(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ToolGrantAuditParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let limit = match params.limit {
        None => DEFAULT_AUDIT_PAGE,
        Some(0) => {
            return refused(
                id,
                &ToolGrantRequestRefusal::InvalidPageLimit,
                "an audit page limit must be at least 1",
            );
        }
        Some(limit) => usize::try_from(limit)
            .unwrap_or(MAX_AUDIT_PAGE)
            .min(MAX_AUDIT_PAGE),
    };
    let (subject, _serving) = match subject_for_read(id.clone(), &params.subject, &runtime).await {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };
    match runtime
        .runtime_adapter()
        .read_tool_grant_audit(&subject, params.after_sequence, limit)
        .await
    {
        Ok(page) => {
            let next_sequence = if page.len() == limit {
                page.last_sequence()
            } else {
                None
            };
            RpcResponse::success(
                id,
                ToolGrantAuditResult {
                    rows: page.rows.iter().map(wire_tool_grant_audit_row).collect(),
                    unreadable: page
                        .unreadable
                        .iter()
                        .map(wire_unreadable_tool_grant_audit_row)
                        .collect(),
                    next_sequence,
                },
            )
        }
        Err(error_value) => grant_error_to_response(id, error_value),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// A grant row that concurrent writers kept changing answers the busy
    /// class, which a retry resolves; the other service faults stay
    /// internal; the request's own faults are typed refusals.
    #[test]
    fn grant_service_errors_answer_their_classes() {
        let response = |error_value| grant_error_to_response(None, error_value);
        let code = |error_value| response(error_value).error.map(|error| error.code);
        assert_eq!(
            code(ToolGrantServiceError::Conflict {
                grant_id: "g1".to_string()
            }),
            Some(error::SESSION_BUSY)
        );
        assert_eq!(
            code(ToolGrantServiceError::Clock),
            Some(error::INTERNAL_ERROR)
        );
        let not_found = response(ToolGrantServiceError::NotFound {
            grant_id: "g1".to_string(),
        })
        .error
        .expect("refused");
        assert_eq!(not_found.code, error::INVALID_PARAMS);
        assert_eq!(
            not_found.data,
            Some(serde_json::json!({
                "kind": "tool_grant_request_refused",
                "tool_grant_request_refusal": {"kind": "not_found", "grant_id": "g1"},
            }))
        );
    }
}
