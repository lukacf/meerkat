//! `tool_grants/*` JSON-RPC handlers: the scoped tool grants of a session.
//!
//! Every method reads and writes grant rows through the runtime's grant
//! service only: it never loads, registers or wakes the subject's session.
//! A grant reads the gate the session records (its staged build before or
//! during its first turn, its persisted metadata after) and refuses a
//! target the session does not gate. A `current_run` grant takes the run the
//! runtime records for the session now; a session that runs none (idle, or
//! not served here) refuses it as `run_not_active`.
//!
//! `tool_grants/request` checks its subject, target and scope the same way
//! and holds a durable decision of kind `tool_grant.issue` (this host is its
//! requester) instead of issuing anything: the runtime's executor of the kind
//! issues the grant once a person approved it. `tool_grants/list` lists the
//! subject's open and approved requests beside its grants, as `requested`,
//! and with `include_ended` its declined ones too.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use meerkat_contracts::{
    GrantToolParams, GrantToolResult, ListToolGrantsParams, ListToolGrantsResult,
    RequestToolGrantParams, RequestToolGrantResult, RevokeToolGrantParams, RevokeToolGrantResult,
    ToolGrantAuditParams, ToolGrantAuditResult, ToolGrantScopeRequest, ToolGrantSubjectRequest,
    ToolGrantTargetRequest, WireToolGrantErrorData,
};
use meerkat_core::tool_grant::{
    GrantGatedTools, ToolGrantId, ToolGrantIssuer, ToolGrantSubject, ToolGrantTarget,
};
use meerkat_core::types::ToolName;
use meerkat_core::{RunId, SessionId};
use meerkat_runtime::RuntimeStoreError;
use meerkat_runtime::decision::wire::wire_decision;
use meerkat_runtime::tool_grant::wire::{
    wire_tool_grant, wire_tool_grant_audit_row, wire_tool_grant_request,
    wire_unreadable_tool_grant, wire_unreadable_tool_grant_audit_row,
};
use meerkat_runtime::tool_grant::{
    ToolGrantIssueRequest, ToolGrantRequest, ToolGrantRequestError, ToolGrantRequestOrigin,
    ToolGrantRequestPolicy, ToolGrantRevocation, ToolGrantServiceError, ToolGrantStatus,
    ToolGrantWindow, tool_grant_requests_of,
};

use crate::error;
use crate::handlers::decision::{decision_error_to_response, decision_owner};
use crate::handlers::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

/// The audit page size when a request names none.
const DEFAULT_AUDIT_PAGE: usize = 100;

/// The largest audit page: a request asking for more reads this many.
const MAX_AUDIT_PAGE: usize = 1_000;

fn grant_error_to_response(id: Option<RpcId>, error_value: ToolGrantServiceError) -> RpcResponse {
    let code = match &error_value {
        ToolGrantServiceError::InvalidWindow { .. }
        | ToolGrantServiceError::EmptyTarget
        | ToolGrantServiceError::NotFound { .. } => error::INVALID_PARAMS,
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

/// A typed `tool_grants/*` refusal: `code`, `message` and the typed `data`.
fn refusal_with_data(
    id: Option<RpcId>,
    code: i32,
    message: impl Into<String>,
    data: WireToolGrantErrorData,
) -> RpcResponse {
    match serde_json::to_value(data) {
        Ok(data) => RpcResponse::error_with_data(id, code, message, data),
        Err(serialize_error) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("failed to serialize tool grant error data: {serialize_error}"),
        ),
    }
}

/// The session a subject names. This server grants to sessions only.
#[allow(clippy::result_large_err)]
fn subject_session(
    id: Option<RpcId>,
    subject: &ToolGrantSubjectRequest,
    runtime: &SessionRuntime,
) -> Result<SessionId, RpcResponse> {
    match subject {
        ToolGrantSubjectRequest::Session { session_id } => {
            parse_session_id_for_runtime(id, session_id, runtime)
        }
        ToolGrantSubjectRequest::Identity { .. } => Err(RpcResponse::error(
            id,
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            "this server grants tools to sessions only: identity subjects are unsupported",
        )),
    }
}

/// The target a grant request names, checked against the gate its session
/// records: a tool name the session gates (by name, or through a gated
/// family), or a family the session gates, with every name the family can
/// provide (the gate admits only the names the session's build gated).
fn resolve_target(
    target: ToolGrantTargetRequest,
    gate: &GrantGatedTools,
) -> Result<ToolGrantTarget, String> {
    match target {
        ToolGrantTargetRequest::Tool { name } => {
            let gated = gate.tools.contains(&name)
                || gate.families.iter().any(|family| {
                    meerkat::tool_family_names(family)
                        .is_some_and(|names| names.contains(&name.as_str()))
                });
            if !gated {
                return Err(format!(
                    "the session does not grant-gate the tool '{name}': a grant opens only \
                     names its build gated"
                ));
            }
            Ok(ToolGrantTarget::Tool {
                name: ToolName::from(name),
            })
        }
        ToolGrantTargetRequest::Family { family } => {
            if !gate.families.contains(&family) {
                return Err(format!(
                    "the session does not grant-gate the tool family '{family}'"
                ));
            }
            let Some(names) = meerkat::tool_family_names(&family) else {
                return Err(format!("'{family}' is not a built-in tool family"));
            };
            Ok(ToolGrantTarget::Family {
                family,
                names: names.into_iter().map(ToolName::from).collect(),
            })
        }
    }
}

/// The gate `session_id` records, or the session's absence.
#[allow(clippy::result_large_err)]
async fn session_gate(
    id: Option<RpcId>,
    session_id: &SessionId,
    runtime: &SessionRuntime,
) -> Result<GrantGatedTools, RpcResponse> {
    match runtime.session_grant_gate(session_id).await {
        Ok(Some(gate)) => Ok(gate),
        Ok(None) => Err(RpcResponse::error(
            id,
            error::SESSION_NOT_FOUND,
            format!("Session not found: {session_id}"),
        )),
        Err(error_value) => Err(RpcResponse::from_error(id, error_value)),
    }
}

/// The run a requested scope binds: none for a window, and for
/// `current_run` the run the runtime records for `session_id` now, which a
/// session that runs none refuses as `run_not_active`.
#[allow(clippy::result_large_err)]
async fn run_scope_of(
    id: Option<RpcId>,
    scope: Option<ToolGrantScopeRequest>,
    session_id: &SessionId,
    runtime: &SessionRuntime,
) -> Result<Option<RunId>, RpcResponse> {
    match scope {
        None | Some(ToolGrantScopeRequest::Window {}) => Ok(None),
        Some(ToolGrantScopeRequest::CurrentRun {}) => {
            match runtime.runtime_adapter().current_run(session_id).await {
                Some(run_id) => Ok(Some(run_id)),
                None => Err(refusal_with_data(
                    id,
                    error::INVALID_PARAMS,
                    format!(
                        "the session {session_id} runs no turn now: a current_run grant needs \
                         a run"
                    ),
                    WireToolGrantErrorData::RunNotActive,
                )),
            }
        }
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
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "a tool grant is bounded by exactly one of expires_at_ms and duration_ms",
            );
        }
    };
    let session_id = match subject_session(id.clone(), &params.subject, &runtime) {
        Ok(session_id) => session_id,
        Err(response) => return response,
    };
    let gate = match session_gate(id.clone(), &session_id, &runtime).await {
        Ok(gate) => gate,
        Err(response) => return response,
    };
    let target = match resolve_target(params.target, &gate) {
        Ok(target) => target,
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
    };
    let run_scope = match run_scope_of(id.clone(), params.scope, &session_id, &runtime).await {
        Ok(run_scope) => run_scope,
        Err(response) => return response,
    };
    let request = ToolGrantRequest {
        subject: ToolGrantSubject::session(session_id),
        target,
        issuer: ToolGrantIssuer::Operator { caller: None },
        reason: params.reason,
        window,
        run_scope,
    };
    match runtime.runtime_adapter().grant_tool(request).await {
        Ok(grant) => RpcResponse::success(
            id,
            GrantToolResult {
                grant: wire_tool_grant(&grant),
            },
        ),
        Err(error_value) => grant_error_to_response(id, error_value),
    }
}

/// Handle `tool_grants/request`: hold a durable decision of kind
/// `tool_grant.issue` for a grant a person approves. The subject, target and
/// scope are checked as `tool_grants/grant` checks them, and the duration
/// against the host's maximum lifetime. This host is the requester, and the
/// decision is held for the approver id `operator` until the request's
/// deadline. Nothing is issued here: the runtime's executor of the kind
/// issues the grant, under the id the answer names, once a person approved
/// it.
pub async fn handle_request(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: RequestToolGrantParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let decisions = match decision_owner(id.clone(), &runtime) {
        Ok(decisions) => decisions,
        Err(response) => return response,
    };
    let session_id = match subject_session(id.clone(), &params.subject, &runtime) {
        Ok(session_id) => session_id,
        Err(response) => return response,
    };
    let gate = match session_gate(id.clone(), &session_id, &runtime).await {
        Ok(gate) => gate,
        Err(response) => return response,
    };
    let target = match resolve_target(params.target, &gate) {
        Ok(target) => target,
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
    };
    let run_scope = match run_scope_of(id.clone(), params.scope, &session_id, &runtime).await {
        Ok(run_scope) => run_scope,
        Err(response) => return response,
    };
    let requester = match runtime.tool_grant_host_requester() {
        Ok(requester) => requester,
        Err(message) => return RpcResponse::error(id, error::INTERNAL_ERROR, message),
    };
    let request = ToolGrantIssueRequest {
        subject: ToolGrantSubject::session(session_id),
        target,
        duration: Duration::from_millis(params.duration_ms),
        run_scope,
        reason: params.reason,
        origin: ToolGrantRequestOrigin::Host {
            request_id: uuid::Uuid::new_v4().to_string(),
        },
        requester,
    };
    match runtime.runtime_adapter().request_tool_grant(
        &decisions,
        &ToolGrantRequestPolicy::default(),
        request,
    ) {
        Ok((decision, body)) => RpcResponse::success(
            id,
            RequestToolGrantResult {
                grant: wire_tool_grant_request(&decision, &body),
                decision: wire_decision(&decision, false),
            },
        ),
        Err(ToolGrantRequestError::Decision(error_value)) => {
            decision_error_to_response(id, error_value)
        }
        // The clock, or a held body this binary cannot read back: the
        // server's fault.
        Err(
            error_value @ (ToolGrantRequestError::Clock(_)
            | ToolGrantRequestError::UnreadableRequest(_)),
        ) => RpcResponse::error(id, error::INTERNAL_ERROR, error_value.to_string()),
        Err(error_value) => RpcResponse::error(id, error::INVALID_PARAMS, error_value.to_string()),
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
            return RpcResponse::error(id, error::INVALID_PARAMS, error_value.to_string());
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
/// ended grants (expired, revoked, or ended with their run) are listed only
/// when the request asks for them. On a runtime that keeps durable
/// decisions the subject's grant requests follow its grants: the open and
/// approved ones as `requested` and, when ended ones are asked for, the
/// rejected, expired, withdrawn and refused ones as `declined`. A request
/// whose grant was issued is listed as that grant only.
pub async fn handle_list(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ListToolGrantsParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let session_id = match subject_session(id.clone(), &params.subject, &runtime) {
        Ok(session_id) => session_id,
        Err(response) => return response,
    };
    let subject = ToolGrantSubject::session(session_id);
    let listing = match runtime.runtime_adapter().list_tool_grants(&subject).await {
        Ok(listing) => listing,
        Err(error_value) => return grant_error_to_response(id, error_value),
    };
    let mut grants: Vec<_> = listing
        .grants
        .iter()
        .filter(|grant| params.include_ended || matches!(grant.status, ToolGrantStatus::Active))
        .map(wire_tool_grant)
        .collect();
    let decisions = runtime.decision_service();
    if decisions.is_available() {
        let requests = match tool_grant_requests_of(&decisions, &subject, params.include_ended) {
            Ok(requests) => requests,
            Err(error_value) => return decision_error_to_response(id, error_value),
        };
        let issued: HashSet<&ToolGrantId> = listing
            .grants
            .iter()
            .map(|grant| &grant.record.grant_id)
            .collect();
        grants.extend(
            requests
                .iter()
                .filter(|(decision, _)| {
                    !issued.contains(&ToolGrantId::approved(&decision.decision_id))
                })
                .map(|(decision, body)| wire_tool_grant_request(decision, body)),
        );
    }
    RpcResponse::success(
        id,
        ListToolGrantsResult {
            grants,
            unreadable: listing
                .unreadable
                .iter()
                .map(wire_unreadable_tool_grant)
                .collect(),
        },
    )
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
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "an audit page limit must be at least 1",
            );
        }
        Some(limit) => usize::try_from(limit)
            .unwrap_or(MAX_AUDIT_PAGE)
            .min(MAX_AUDIT_PAGE),
    };
    let session_id = match subject_session(id.clone(), &params.subject, &runtime) {
        Ok(session_id) => session_id,
        Err(response) => return response,
    };
    match runtime
        .runtime_adapter()
        .read_tool_grant_audit(
            &ToolGrantSubject::session(session_id),
            params.after_sequence,
            limit,
        )
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
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_target_must_be_gated_by_the_session() {
        let gate = GrantGatedTools::default()
            .with_family("shell")
            .with_tool("deploy");

        assert_eq!(
            resolve_target(
                ToolGrantTargetRequest::Tool {
                    name: "deploy".to_string()
                },
                &gate
            ),
            Ok(ToolGrantTarget::Tool {
                name: ToolName::from("deploy")
            })
        );
        // A name a gated family provides is gated through the family.
        assert!(
            resolve_target(
                ToolGrantTargetRequest::Tool {
                    name: "shell".to_string()
                },
                &gate
            )
            .is_ok()
        );
        let Ok(ToolGrantTarget::Family { family, names }) = resolve_target(
            ToolGrantTargetRequest::Family {
                family: "shell".to_string(),
            },
            &gate,
        ) else {
            panic!("a gated family resolves");
        };
        assert_eq!(family, "shell");
        assert!(names.contains("shell"), "{names:?}");

        for refused in [
            ToolGrantTargetRequest::Tool {
                name: "web_search".to_string(),
            },
            ToolGrantTargetRequest::Family {
                family: "tasks".to_string(),
            },
        ] {
            assert!(
                resolve_target(refused.clone(), &gate).is_err(),
                "{refused:?} must be refused"
            );
        }
    }

    /// A grant row that concurrent writers kept changing answers the busy
    /// class, which a retry resolves; the other service faults stay
    /// internal.
    #[test]
    fn a_contended_grant_write_answers_busy() {
        let code = |error_value| {
            grant_error_to_response(None, error_value)
                .error
                .map(|error| error.code)
        };
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
    }
}
