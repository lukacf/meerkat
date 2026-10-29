//! `tool_grants/*` JSON-RPC handlers: the scoped tool grants of a session.
//!
//! Every method reads and writes grant rows through the runtime's grant
//! service only: it never loads, registers or wakes the subject's session.
//! A grant reads the gate the session records (its staged build before or
//! during its first turn, its persisted metadata after) and refuses a
//! target the session does not gate.

use std::sync::Arc;
use std::time::Duration;

use meerkat_contracts::{
    GrantToolParams, GrantToolResult, ListToolGrantsParams, ListToolGrantsResult,
    RevokeToolGrantParams, RevokeToolGrantResult, ToolGrantAuditParams, ToolGrantAuditResult,
    WireToolGrantSubject, WireToolGrantTarget,
};
use meerkat_core::SessionId;
use meerkat_core::tool_grant::{
    GrantGatedTools, ToolGrantId, ToolGrantIssuer, ToolGrantSubject, ToolGrantTarget,
};
use meerkat_core::types::ToolName;
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

fn grant_error_to_response(id: Option<RpcId>, error_value: ToolGrantServiceError) -> RpcResponse {
    let code = match &error_value {
        ToolGrantServiceError::InvalidWindow { .. }
        | ToolGrantServiceError::EmptyTarget
        | ToolGrantServiceError::NotFound { .. } => error::INVALID_PARAMS,
        ToolGrantServiceError::Store(RuntimeStoreError::Unsupported(_)) => {
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code()
        }
        // A concurrent writer twice, the clock, a row this binary cannot
        // read, the machine or the store: the server's fault.
        _ => error::INTERNAL_ERROR,
    };
    RpcResponse::error(id, code, error_value.to_string())
}

/// The session a subject names. This server grants to sessions only.
#[allow(clippy::result_large_err)]
fn subject_session(
    id: Option<RpcId>,
    subject: &WireToolGrantSubject,
    runtime: &SessionRuntime,
) -> Result<SessionId, RpcResponse> {
    match subject {
        WireToolGrantSubject::Session { session_id } => {
            parse_session_id_for_runtime(id, session_id, runtime)
        }
        WireToolGrantSubject::Identity { .. } => Err(RpcResponse::error(
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
    target: WireToolGrantTarget,
    gate: &GrantGatedTools,
) -> Result<ToolGrantTarget, String> {
    match target {
        WireToolGrantTarget::Tool { name } => {
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
        WireToolGrantTarget::Family { family, names } => {
            if !names.is_empty() {
                return Err(
                    "a family target's names are filled in by the server: omit them".to_string(),
                );
            }
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
    let gate = match runtime.session_grant_gate(&session_id).await {
        Ok(Some(gate)) => gate,
        Ok(None) => {
            return RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            );
        }
        Err(error_value) => return RpcResponse::from_error(id, error_value),
    };
    let target = match resolve_target(params.target, &gate) {
        Ok(target) => target,
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
    };
    let request = ToolGrantRequest {
        subject: ToolGrantSubject::session(session_id),
        target,
        issuer: ToolGrantIssuer::Operator { caller: None },
        reason: params.reason,
        window,
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
    let session_id = match subject_session(id.clone(), &params.subject, &runtime) {
        Ok(session_id) => session_id,
        Err(response) => return response,
    };
    match runtime
        .runtime_adapter()
        .list_tool_grants(&ToolGrantSubject::session(session_id))
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
                WireToolGrantTarget::Tool {
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
                WireToolGrantTarget::Tool {
                    name: "shell".to_string()
                },
                &gate
            )
            .is_ok()
        );
        let Ok(ToolGrantTarget::Family { family, names }) = resolve_target(
            WireToolGrantTarget::Family {
                family: "shell".to_string(),
                names: Vec::new(),
            },
            &gate,
        ) else {
            panic!("a gated family resolves");
        };
        assert_eq!(family, "shell");
        assert!(names.contains("shell"), "{names:?}");

        for refused in [
            WireToolGrantTarget::Tool {
                name: "web_search".to_string(),
            },
            WireToolGrantTarget::Family {
                family: "tasks".to_string(),
                names: Vec::new(),
            },
            WireToolGrantTarget::Family {
                family: "shell".to_string(),
                names: vec!["shell".to_string()],
            },
        ] {
            assert!(
                resolve_target(refused.clone(), &gate).is_err(),
                "{refused:?} must be refused"
            );
        }
    }
}
