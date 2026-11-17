//! JSON-RPC adapters for the optional topology control plane.

use serde_json::{Value, json};

use crate::access::{
    ACTION_TOPOLOGY_AUDIT, ACTION_TOPOLOGY_BULK, ACTION_TOPOLOGY_CROSS_AUTHORITY,
    ACTION_TOPOLOGY_VIEW, AccessView,
};
use crate::topology_control::{
    TopologyApplyRequest, TopologyApplyScope, TopologyAuditPage, TopologyControlError,
    TopologyControlMode, TopologyMutation, TopologyNodeAffordances, TopologyOperationRecordStatus,
    TopologyPlanRequest, TopologyRuntimeHandle,
};

use super::{JSONRPC_VERSION, JsonRpcError, JsonRpcResponse};

pub(crate) const TOPOLOGY_QUERY_METHOD: &str = "meta/topology/query";
pub(crate) const TOPOLOGY_PLAN_METHOD: &str = "meta/topology/plan";
pub(crate) const TOPOLOGY_APPLY_METHOD: &str = "meta/topology/apply";
pub(crate) const TOPOLOGY_OPERATION_METHOD: &str = "meta/topology/operation/get";
pub(crate) const TOPOLOGY_AUDIT_METHOD: &str = "meta/topology/audit/query";

pub(crate) fn is_topology_mutating_method(method: &str) -> bool {
    method == TOPOLOGY_APPLY_METHOD
}

pub(crate) fn capability_projection(
    topology: &TopologyRuntimeHandle,
    view: Option<&AccessView>,
    read_only: bool,
) -> (Vec<&'static str>, Value) {
    let policy = topology.policy();
    let can_query = view
        .map(|view| view.may_perform_anywhere(ACTION_TOPOLOGY_VIEW))
        .unwrap_or(true);
    let can_change_any = view
        .map(|view| {
            [
                crate::access::ACTION_TOPOLOGY_CONNECT,
                crate::access::ACTION_TOPOLOGY_DISCONNECT,
                crate::access::ACTION_TOPOLOGY_RECONNECT,
            ]
            .into_iter()
            .any(|action| view.may_perform_anywhere(action))
        })
        .unwrap_or(true);
    // Planning reveals whether a requested mutation would be accepted and is
    // therefore gated by the same per-action grants as applying it. Planning
    // remains side-effect free; this only prevents it from becoming an ABAC
    // oracle for actions the caller is not allowed to perform.
    let can_plan = can_query && policy.mode != TopologyControlMode::Disabled && can_change_any;
    let can_apply =
        can_query && !read_only && policy.mode == TopologyControlMode::Editable && can_change_any;
    let can_bulk = can_apply
        && policy.allow_bulk
        && view
            .map(|view| view.may_perform_anywhere(ACTION_TOPOLOGY_BULK))
            .unwrap_or(true);
    // The JSON-RPC product surface is intentionally authority-local. A
    // same-process bilateral host API may opt into cross-authority changes,
    // but this endpoint must not advertise an operation it always rejects.
    let can_cross_authority = false;

    let mut methods = Vec::new();
    if can_query {
        methods.extend([TOPOLOGY_QUERY_METHOD, TOPOLOGY_OPERATION_METHOD]);
    }
    if can_query && audit_method_allowed(view) {
        methods.push(TOPOLOGY_AUDIT_METHOD);
    }
    if can_plan {
        methods.push(TOPOLOGY_PLAN_METHOD);
    }
    if can_apply {
        methods.push(TOPOLOGY_APPLY_METHOD);
    }
    (
        methods,
        json!({
            "mode": policy.mode,
            "can_query": can_query,
            "can_plan": can_plan,
            "can_apply": can_apply,
            "can_bulk": can_bulk,
            "max_batch_size": policy.max_batch_size,
            "can_cross_authority": can_cross_authority,
        }),
    )
}

pub(crate) async fn handle_query(
    topology: &TopologyRuntimeHandle,
    response_id: Value,
    view: Option<&AccessView>,
    read_only: bool,
) -> JsonRpcResponse {
    match topology.query().await {
        Ok(mut snapshot) => {
            // Every view decides: one that enforces nothing still hides the
            // agents private to another person.
            if let Some(view) = view {
                snapshot.retain_visible_to(view);
            }
            project_node_affordances(&mut snapshot.nodes, topology, view, read_only);
            success(response_id, snapshot)
        }
        Err(error) => failure(response_id, error),
    }
}

fn project_node_affordances(
    nodes: &mut [crate::topology_control::TopologyNodeSnapshot],
    topology: &TopologyRuntimeHandle,
    view: Option<&AccessView>,
    read_only: bool,
) {
    let policy = topology.policy();
    let editable = !read_only && policy.mode == TopologyControlMode::Editable;
    for node in nodes {
        let identity = node.endpoint.identity.as_str();
        let allowed = |action: &str| {
            editable
                && view
                    .filter(|view| view.enforced())
                    .is_none_or(|view| view.allows_name(action, identity))
        };
        node.affordances = Some(TopologyNodeAffordances {
            can_connect: allowed(crate::access::ACTION_TOPOLOGY_CONNECT),
            can_disconnect: allowed(crate::access::ACTION_TOPOLOGY_DISCONNECT),
            can_reconnect: allowed(crate::access::ACTION_TOPOLOGY_RECONNECT),
            can_bulk: policy.allow_bulk && allowed(ACTION_TOPOLOGY_BULK),
            // Cross-authority mutation is available only through the
            // explicitly bilateral same-process host helper, never this RPC.
            can_cross_authority: false,
        });
    }
}

pub(crate) async fn handle_plan(
    topology: &TopologyRuntimeHandle,
    response_id: Value,
    params: &Value,
    view: Option<&AccessView>,
) -> JsonRpcResponse {
    let mut request: TopologyPlanRequest = match serde_json::from_value(params.clone()) {
        Ok(request) => request,
        Err(error) => {
            return invalid_params(response_id, error.to_string());
        }
    };
    request.operations = match topology.normalize_for_authorization(request.operations) {
        Ok(operations) => operations,
        Err(error) => return failure(response_id, error),
    };
    if let Some(error) = authorize_operations(topology, view, &request.operations, true) {
        return access_denied(response_id, error);
    }
    match topology.plan(request).await {
        Ok(plan) => success(response_id, plan),
        Err(error) => failure(response_id, error),
    }
}

/// The request the denied operation record of an apply the access rules
/// refuse is written of: the one place that decides what such a record
/// stores. A request is recorded as it is unless one of its ends is a name
/// the attribute records of the caller's audience (the Console's) know no
/// agent under (no member holds it, or the caller's surface stood it in
/// for a member hidden from the caller) or an agent hidden from the
/// caller; an agent the rules only refuse the caller a view of keeps its
/// name in the record. With such an end the request is recorded with its
/// operations left out: the record then holds no name and nothing derived
/// from one (its operation id covers the request), it is written and
/// counted like any other, and [`handle_audit`] shows it to access
/// administrators only.
fn denied_record_request(
    view: &AccessView,
    request: &TopologyApplyRequest,
) -> TopologyApplyRequest {
    let mut recorded = request.clone();
    if request.operations.iter().any(|operation| {
        [&operation.edge.a, &operation.edge.b]
            .into_iter()
            .any(|end| {
                !view.knows_agent(end.identity.as_str()) || view.hides_agent(end.identity.as_str())
            })
    }) {
        recorded.operations.clear();
    }
    recorded
}

/// `meta/topology/apply` for a caller of `scope`: its idempotency keys are
/// its own, so the same key from another caller names another operation.
///
/// `owed` is a refusal the caller's surface owes the request whatever the
/// topology would answer: the console's, for a request in which it stood a
/// name no member holds in for an end
/// ([`crate::http_console::ConsoleAbsentNames::recorded_refusal`]). It is
/// the answer once the request parsed and the access rules allowed every
/// operation, so the rules' refusal comes first, as it does for any other
/// name, and such a request is never applied. A refusal of it by the rules
/// writes its denied operation record like that of any request with an end
/// that is a name the caller's audience knows no agent under, or an agent
/// hidden from the caller ([`denied_record_request`]).
pub(crate) async fn handle_apply(
    topology: &TopologyRuntimeHandle,
    response_id: Value,
    params: &Value,
    view: Option<&AccessView>,
    actor: Option<&str>,
    scope: &TopologyApplyScope,
    owed: Option<JsonRpcError>,
) -> JsonRpcResponse {
    let mut request: TopologyApplyRequest = match serde_json::from_value(params.clone()) {
        Ok(request) => request,
        Err(error) => {
            return invalid_params(response_id, error.to_string());
        }
    };
    request.operations = match topology.normalize_for_authorization(request.operations) {
        Ok(operations) => operations,
        Err(error) => return failure(response_id, error),
    };
    if let Some(violation) = authorize_operations(topology, view, &request.operations, true) {
        let denied = TopologyControlError::AccessDenied {
            authority: topology.authority(),
            action: violation.action.to_string(),
            identity: violation
                .resource
                .clone()
                .unwrap_or_else(|| "<redacted>".to_string()),
        };
        let recorded = match view {
            Some(view) => denied_record_request(view, &request),
            None => request,
        };
        if let Err(audit_error) = topology
            .record_denied_apply(
                &recorded,
                scope,
                actor,
                actor.unwrap_or("local-host"),
                &denied,
            )
            .await
        {
            return failure(response_id, audit_error);
        }
        return access_denied(response_id, violation);
    }
    if let Some(refusal) = owed {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(refusal),
        };
    }
    let expose_attribution = attribution_allowed_for_operations(view, &request.operations);
    match topology
        .apply_in_scope(request, scope, actor, actor.unwrap_or("local-host"))
        .await
    {
        Ok(mut receipt) => {
            if !expose_attribution {
                receipt.actor.clear();
            }
            success(response_id, receipt)
        }
        Err(error) => {
            let mut response = failure(response_id, error);
            if !expose_attribution {
                redact_error_receipt_actor(&mut response);
            }
            response
        }
    }
}

pub(crate) async fn handle_operation(
    topology: &TopologyRuntimeHandle,
    response_id: Value,
    params: &Value,
    view: Option<&AccessView>,
) -> JsonRpcResponse {
    let Some(operation_id) = params
        .get("operation_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return invalid_params(response_id, "operation_id required");
    };
    match topology.controller().operation(operation_id).await {
        Ok(mut receipt) => {
            if let Some(view) = view {
                let visible = receipt.results.iter().all(|result| {
                    [&result.edge.a, &result.edge.b]
                        .into_iter()
                        .all(|endpoint| {
                            view.can_view_name(endpoint.identity.as_str())
                                && view
                                    .allows_name(ACTION_TOPOLOGY_VIEW, endpoint.identity.as_str())
                        })
                });
                if !visible {
                    return access_denied(
                        response_id,
                        AccessViolation {
                            action: ACTION_TOPOLOGY_VIEW,
                            resource: None,
                        },
                    );
                }
            }
            if !attribution_allowed_for_results(view, &receipt.results) {
                receipt.actor.clear();
            }
            success(response_id, receipt)
        }
        Err(error) => failure(response_id, error),
    }
}

pub(crate) async fn handle_audit(
    topology: &TopologyRuntimeHandle,
    response_id: Value,
    params: &Value,
    view: Option<&AccessView>,
) -> JsonRpcResponse {
    if !audit_method_allowed(view) {
        return access_denied(
            response_id,
            AccessViolation {
                action: ACTION_TOPOLOGY_AUDIT,
                resource: None,
            },
        );
    }
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 1024) as usize;
    let after_seq = match params.get("after_seq") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_u64() {
            Some(value) => Some(value),
            None => return invalid_params(response_id, "after_seq must be a non-negative integer"),
        },
    };
    let page = match topology
        .controller()
        .all_operation_records_after(after_seq)
        .await
    {
        Ok(page) => page,
        Err(error) => return failure(response_id, error),
    };
    // Every view decides each record: one that enforces nothing still hides
    // the agents private to another person.
    let mut records = Vec::new();
    let mut next_after_seq = page.next_after_seq;
    let mut consumed_all = true;
    for record in page.records {
        next_after_seq = record.seq;
        let visible = view.is_none_or(|view| {
            // A denied record that holds no operation is of a request with
            // an end that was a name its caller's audience knew no agent
            // under, or an agent hidden from its caller
            // ([`denied_record_request`]): no end decides who reads it;
            // access administrators do.
            if record.status == TopologyOperationRecordStatus::Denied
                && record.operations.is_empty()
            {
                return view.can_administer();
            }
            record.operations.iter().all(|operation| {
                [&operation.edge.a, &operation.edge.b]
                    .into_iter()
                    .all(|endpoint| {
                        view.can_view_name(endpoint.identity.as_str())
                            && view.allows_name(ACTION_TOPOLOGY_VIEW, endpoint.identity.as_str())
                            && view.allows_name(ACTION_TOPOLOGY_AUDIT, endpoint.identity.as_str())
                    })
            })
        });
        if visible {
            records.push(record);
            if records.len() == limit {
                consumed_all = false;
                break;
            }
        }
    }
    success(
        response_id,
        TopologyAuditPage {
            records,
            next_after_seq,
            oldest_available_seq: page.oldest_available_seq,
            latest_seq: page.latest_seq,
            has_more: page.has_more || !consumed_all,
        },
    )
}

fn audit_method_allowed(view: Option<&AccessView>) -> bool {
    view.filter(|view| view.enforced())
        .is_none_or(|view| view.may_perform_anywhere(ACTION_TOPOLOGY_AUDIT))
}

#[derive(Debug)]
pub(crate) struct AccessViolation {
    action: &'static str,
    resource: Option<String>,
}

fn authorize_operations(
    topology: &TopologyRuntimeHandle,
    view: Option<&AccessView>,
    operations: &[TopologyMutation],
    mutation: bool,
) -> Option<AccessViolation> {
    let view = view?;
    // An endpoint the caller may not know of (private to another person) is
    // refused whether or not access is enforced, naming no endpoint.
    if operations.iter().any(|operation| {
        [&operation.edge.a, &operation.edge.b]
            .into_iter()
            .any(|endpoint| view.hides_agent(endpoint.identity.as_str()))
    }) {
        return Some(AccessViolation {
            action: ACTION_TOPOLOGY_VIEW,
            resource: None,
        });
    }
    if !view.enforced() {
        return None;
    }
    let local_authority = topology.authority();
    if operations.len() > 1 {
        for operation in operations {
            for endpoint in [&operation.edge.a, &operation.edge.b] {
                if !view.allows_name(ACTION_TOPOLOGY_BULK, endpoint.identity.as_str()) {
                    return Some(AccessViolation {
                        action: ACTION_TOPOLOGY_BULK,
                        resource: Some(endpoint.identity.clone()),
                    });
                }
            }
        }
    }
    for operation in operations {
        let cross_authority = [
            operation.edge.a.authority.as_deref(),
            operation.edge.b.authority.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|authority| !authority.is_empty() && authority != local_authority);
        for endpoint in [&operation.edge.a, &operation.edge.b] {
            if !view.can_view_name(endpoint.identity.as_str())
                || !view.allows_name(ACTION_TOPOLOGY_VIEW, endpoint.identity.as_str())
            {
                return Some(AccessViolation {
                    action: ACTION_TOPOLOGY_VIEW,
                    resource: Some(endpoint.identity.clone()),
                });
            }
            if mutation
                && !view.allows_name(operation.action.access_action(), endpoint.identity.as_str())
            {
                return Some(AccessViolation {
                    action: operation.action.access_action(),
                    resource: Some(endpoint.identity.clone()),
                });
            }
            if cross_authority
                && !view.allows_name(ACTION_TOPOLOGY_CROSS_AUTHORITY, endpoint.identity.as_str())
            {
                return Some(AccessViolation {
                    action: ACTION_TOPOLOGY_CROSS_AUTHORITY,
                    resource: Some(endpoint.identity.clone()),
                });
            }
        }
    }
    None
}

fn attribution_allowed_for_operations(
    view: Option<&AccessView>,
    operations: &[TopologyMutation],
) -> bool {
    view.is_none_or(|view| {
        operations.iter().all(|operation| {
            [&operation.edge.a, &operation.edge.b]
                .into_iter()
                .all(|endpoint| {
                    view.can_view_name(endpoint.identity.as_str())
                        && view.allows_name(ACTION_TOPOLOGY_AUDIT, endpoint.identity.as_str())
                })
        })
    })
}

fn attribution_allowed_for_results(
    view: Option<&AccessView>,
    results: &[crate::topology_control::TopologyEdgeResult],
) -> bool {
    view.is_none_or(|view| {
        results.iter().all(|result| {
            [&result.edge.a, &result.edge.b]
                .into_iter()
                .all(|endpoint| {
                    view.can_view_name(endpoint.identity.as_str())
                        && view.allows_name(ACTION_TOPOLOGY_AUDIT, endpoint.identity.as_str())
                })
        })
    })
}

fn redact_error_receipt_actor(response: &mut JsonRpcResponse) {
    if let Some(receipt) = response
        .error
        .as_mut()
        .and_then(|error| error.data.as_mut())
        .and_then(|data| data.get_mut("receipt"))
        .and_then(Value::as_object_mut)
    {
        receipt.remove("actor");
    }
}

fn success<T: serde::Serialize>(response_id: Value, result: T) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: Some(serde_json::to_value(result).unwrap_or(Value::Null)),
        error: None,
    }
}

fn invalid_params(response_id: Value, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message: format!("Invalid params: {}", message.into()),
            data: Some(json!({ "kind": "invalid_request" })),
        }),
    }
}

fn access_denied(response_id: Value, violation: AccessViolation) -> JsonRpcResponse {
    let resource = if violation.action == ACTION_TOPOLOGY_VIEW {
        None
    } else {
        violation.resource
    };
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: crate::http_console::ACCESS_DENIED_RPC_CODE,
            message: format!("access denied: {}", violation.action),
            data: Some(json!({
                "kind": "access_denied",
                "action": violation.action,
                "resource": resource,
            })),
        }),
    }
}

fn failure(response_id: Value, error: TopologyControlError) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(failure_error(error)),
    }
}

/// The refusal of an edge's end no member holds, as the topology answers
/// it. The console answers an end that names a member hidden from its
/// caller with it.
pub(crate) fn member_not_found_rpc_error(identity: &str) -> JsonRpcError {
    failure_error(TopologyControlError::MemberNotFound(identity.to_string()))
}

fn failure_error(error: TopologyControlError) -> JsonRpcError {
    let code = match error {
        TopologyControlError::InvalidRequest(_)
        | TopologyControlError::InvalidPolicy(_)
        | TopologyControlError::ReconnectTargetMissing(_)
        | TopologyControlError::DisconnectTargetMissing(_)
        | TopologyControlError::ReconnectRequired(_)
        | TopologyControlError::BulkDisabled
        | TopologyControlError::BatchTooLarge { .. }
        | TopologyControlError::CrossAuthorityDisabled
        | TopologyControlError::AuthorityMismatch(_)
        | TopologyControlError::ApprovalUnsupported(_) => -32602,
        TopologyControlError::FeatureDisabled
        | TopologyControlError::ReadOnly
        | TopologyControlError::DurableStateRequired => -32004,
        TopologyControlError::AccessDenied { .. } => -32001,
        TopologyControlError::RevisionConflict { .. }
        | TopologyControlError::IdempotencyConflict(_)
        | TopologyControlError::IdempotencyReceiptExpired { .. }
        | TopologyControlError::IdempotencyHistoryCompacted(_)
        | TopologyControlError::AuditCursorExpired { .. }
        | TopologyControlError::OperationInProgress(_) => -32009,
        TopologyControlError::OperationNotFound(_) | TopologyControlError::MemberNotFound(_) => {
            -32001
        }
        TopologyControlError::CrossAuthorityUnsupported => -32004,
        TopologyControlError::Actuator(_)
        | TopologyControlError::Persistence(_)
        | TopologyControlError::ApplyFailed { .. } => -32000,
    };
    let mut data = json!({ "kind": error.kind() });
    if let TopologyControlError::RevisionConflict { expected, actual } = &error {
        data["expected_revision"] = json!(expected);
        data["actual_revision"] = json!(actual);
    }
    if let TopologyControlError::AuditCursorExpired {
        after_seq,
        oldest_available_seq,
    } = &error
    {
        data["after_seq"] = json!(after_seq);
        data["oldest_available_seq"] = json!(oldest_available_seq);
    }
    if let Some(receipt) = error.receipt() {
        data["receipt"] = serde_json::to_value(receipt).unwrap_or(Value::Null);
    }
    JsonRpcError {
        code,
        message: error.to_string(),
        data: Some(data),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::access::{AccessControlConfig, AccessController, AccessRule};
    use std::collections::BTreeMap;

    fn view(actions: &[&str], agents: &[&str]) -> crate::access::AccessView {
        AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root".to_string()],
            rules: vec![AccessRule {
                id: "allow".to_string(),
                actions: actions.iter().map(|value| (*value).to_string()).collect(),
                agents: agents.iter().map(|value| (*value).to_string()).collect(),
                ..AccessRule::default()
            }],
            ..AccessControlConfig::default()
        })
        .expect("controller")
        .view_for_subject(None)
    }

    #[test]
    fn endpoint_authorization_requires_both_endpoints() {
        let operation = TopologyMutation {
            action: crate::topology_control::TopologyAction::Reconnect,
            edge: crate::topology_control::TopologyEdge::new(
                crate::topology_control::TopologyEndpoint::local("a"),
                crate::topology_control::TopologyEndpoint::local("b"),
            )
            .expect("edge"),
        };
        let allowed = view(
            &["agent.view", "topology.view", "topology.reconnect"],
            &["a"],
        );
        // Use a lightweight policy check directly; no runtime construction is
        // needed to prove the endpoint decision is conjunctive.
        for endpoint in [&operation.edge.a, &operation.edge.b] {
            let expected = endpoint.identity == "a";
            assert_eq!(
                allowed.allows_name(operation.action.access_action(), &endpoint.identity),
                expected
            );
        }
    }

    #[test]
    fn topology_audit_is_a_separate_permission_and_gates_attribution() {
        let operation = TopologyMutation {
            action: crate::topology_control::TopologyAction::Connect,
            edge: crate::topology_control::TopologyEdge::new(
                crate::topology_control::TopologyEndpoint::local("a"),
                crate::topology_control::TopologyEndpoint::local("b"),
            )
            .expect("edge"),
        };
        let view_only = view(&["agent.view", "topology.view"], &["a", "b"]);
        assert!(!audit_method_allowed(Some(&view_only)));
        assert!(!attribution_allowed_for_operations(
            Some(&view_only),
            std::slice::from_ref(&operation)
        ));

        let auditor = view(
            &["agent.view", "topology.view", "topology.audit"],
            &["a", "b"],
        );
        assert!(audit_method_allowed(Some(&auditor)));
        assert!(attribution_allowed_for_operations(
            Some(&auditor),
            std::slice::from_ref(&operation)
        ));

        let mut receipt = crate::topology_control::TopologyOperationReceipt {
            operation_id: "operation".to_string(),
            idempotency_key: "key".to_string(),
            actor: "sensitive@example.test".to_string(),
            status: crate::topology_control::TopologyOperationStatus::Applied,
            base_revision: 0,
            revision: 1,
            created_at: "2026-07-15T00:00:00Z".to_string(),
            reason: None,
            results: vec![crate::topology_control::TopologyEdgeResult {
                action: operation.action,
                edge: operation.edge,
                status: crate::topology_control::TopologyEdgeResultStatus::Applied,
                actual_before: false,
                actual_after: true,
                error: None,
            }],
            authority_revisions: BTreeMap::new(),
        };
        assert!(!attribution_allowed_for_results(
            Some(&view_only),
            &receipt.results
        ));
        receipt.actor.clear();
        let serialized = serde_json::to_value(receipt).expect("serialize redacted receipt");
        assert!(serialized.get("actor").is_none());
        assert!(!serialized.to_string().contains("sensitive@example.test"));
    }
}
