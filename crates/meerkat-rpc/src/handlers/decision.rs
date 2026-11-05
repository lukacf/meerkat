//! `decision/*` JSON-RPC handlers: the durable decisions of the runtime's
//! persistence.
//!
//! Every method reads or writes one decision row, or reads one index page,
//! through the persistence's decision owner. A runtime whose persistence
//! keeps no durable decisions (an ephemeral one) answers capability
//! unavailable. `decision/decide` has no route in plain dispatch: a host
//! adapter decides through its authenticated event
//! ([`crate::router::MethodRouter::dispatch_with_decision_event`]).

use std::num::NonZeroU32;
use std::sync::Arc;

use meerkat_contracts::{
    DecisionDecideParams, DecisionGetParams, DecisionListParams, DecisionPresentParams,
    DecisionRenderingParams, DecisionRenderingResult, DecisionResult, DecisionWithdrawParams,
    ErrorCode, WireDecisionErrorData,
};
use meerkat_core::approval::decision::{
    ApproverId, AuthenticatedDecisionEvent, DECISION_PAGE_MAX, DecisionContractError, DecisionId,
    DecisionKind, DecisionListFilter, DecisionPrincipalKey, canonical_parts,
};
use meerkat_runtime::decision::wire::{
    decision_phase_from_wire, presentation_from_wire, wire_canonical_part, wire_decision,
    wire_decision_listing, wire_decision_refusal,
};
use meerkat_runtime::decision::{DecisionError, DecisionWithdrawer, DurableDecisionService};

use crate::error;
use crate::handlers::{RpcResponseExt, parse_params};
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

/// The page size when a listing names none.
const DEFAULT_LIST_PAGE: usize = 100;

fn error_with_data(
    id: Option<RpcId>,
    code: i32,
    message: impl Into<String>,
    data: WireDecisionErrorData,
) -> RpcResponse {
    match serde_json::to_value(data) {
        Ok(data) => RpcResponse::error_with_data(id, code, message, data),
        Err(serialize_error) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("failed to serialize decision error data: {serialize_error}"),
        ),
    }
}

/// The response to a failed decision call. A refusal and a missing
/// decision are typed (`data.kind`); store faults keep their commitment
/// truth apart, as for approvals: nothing committed, another owner won, or
/// the write may have committed.
pub(crate) fn decision_error_to_response(
    id: Option<RpcId>,
    error_value: DecisionError,
) -> RpcResponse {
    let message = error_value.to_string();
    match error_value {
        DecisionError::NotFound(_) => error_with_data(
            id,
            error::INVALID_PARAMS,
            message,
            WireDecisionErrorData::DecisionNotFound,
        ),
        DecisionError::Refused { refusal, .. } => error_with_data(
            id,
            error::INVALID_PARAMS,
            message,
            WireDecisionErrorData::DecisionRefused {
                decision_refusal: wire_decision_refusal(refusal),
            },
        ),
        DecisionError::Unreadable { .. } => error_with_data(
            id,
            error::INTERNAL_ERROR,
            message,
            WireDecisionErrorData::DecisionUnreadable,
        ),
        DecisionError::CandidateLaunch => error_with_data(
            id,
            ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            message,
            WireDecisionErrorData::CandidateLaunch,
        ),
        DecisionError::Unavailable(_) | DecisionError::Unsettled(_) => RpcResponse::error(
            id,
            ErrorCode::ApprovalStoreUnavailable.jsonrpc_code(),
            message,
        ),
        DecisionError::Conflict(_) => {
            RpcResponse::error(id, ErrorCode::ApprovalConflict.jsonrpc_code(), message)
        }
        DecisionError::CommitUnknown(_) => {
            RpcResponse::error(id, ErrorCode::ApprovalCommitUnknown.jsonrpc_code(), message)
        }
        DecisionError::Invalid(_) | DecisionError::NotAReceipt => {
            RpcResponse::error(id, error::INVALID_PARAMS, message)
        }
        // The clock, the machine or an executor claim: the server's fault.
        _ => RpcResponse::error(id, error::INTERNAL_ERROR, message),
    }
}

fn invalid(id: Option<RpcId>, error_value: &DecisionContractError) -> RpcResponse {
    RpcResponse::error(
        id,
        error::INVALID_PARAMS,
        format!("invalid decision request: {error_value}"),
    )
}

/// The runtime's decision owner, or the capability-unavailable answer of a
/// runtime that keeps no durable decisions.
#[allow(clippy::result_large_err)]
fn decision_owner(
    id: Option<RpcId>,
    runtime: &SessionRuntime,
) -> Result<DurableDecisionService, RpcResponse> {
    let owner = runtime.decision_service();
    if owner.is_available() {
        Ok(owner)
    } else {
        Err(error_with_data(
            id,
            ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            "this runtime keeps no durable decisions",
            WireDecisionErrorData::NoDecisionStore,
        ))
    }
}

#[allow(clippy::result_large_err)]
fn parse_decision_id(id: Option<RpcId>, decision_id: String) -> Result<DecisionId, RpcResponse> {
    DecisionId::new(decision_id).map_err(|error_value| invalid(id, &error_value))
}

/// The listing filter `params` names: every value is validated, and a phase
/// this version does not know is refused, never read as no filter.
#[allow(clippy::result_large_err)]
fn list_filter(
    id: Option<RpcId>,
    params: &DecisionListParams,
) -> Result<DecisionListFilter, RpcResponse> {
    let mut filter = DecisionListFilter::default();
    if let Some(kind) = &params.kind {
        let kind = DecisionKind::new(kind.clone()).map_err(|e| invalid(id.clone(), &e))?;
        filter = filter.with_kind(kind);
    }
    if let Some(phase) = params.phase {
        let Some(phase) = decision_phase_from_wire(phase) else {
            return Err(RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "unknown decision phase",
            ));
        };
        filter = filter.with_phase(phase);
    }
    if let Some(approver_id) = &params.approver_id {
        let approver_id =
            ApproverId::new(approver_id.clone()).map_err(|e| invalid(id.clone(), &e))?;
        filter = filter.with_approver_id(approver_id);
    }
    if let Some(requester) = &params.requester {
        let requester =
            DecisionPrincipalKey::new(requester.clone()).map_err(|e| invalid(id.clone(), &e))?;
        filter = filter.with_requester(requester);
    }
    Ok(filter)
}

/// Handle `decision/list`.
pub async fn handle_list(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: DecisionListParams = match params {
        Some(_) => match parse_params(params) {
            Ok(params) => params,
            Err(response) => return response.with_id(id),
        },
        None => DecisionListParams::default(),
    };
    let filter = match list_filter(id.clone(), &params) {
        Ok(filter) => filter,
        Err(response) => return response,
    };
    let owner = match decision_owner(id.clone(), &runtime) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let limit = params
        .limit
        .map_or(DEFAULT_LIST_PAGE, |limit| {
            usize::try_from(limit).unwrap_or(DECISION_PAGE_MAX)
        })
        .clamp(1, DECISION_PAGE_MAX);
    match owner.list(&filter, params.after.as_deref(), limit) {
        Ok(listing) => RpcResponse::success(id, wire_decision_listing(&listing)),
        Err(error_value) => decision_error_to_response(id, error_value),
    }
}

/// Handle `decision/get`.
pub async fn handle_get(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: DecisionGetParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let decision_id = match parse_decision_id(id.clone(), params.decision_id) {
        Ok(decision_id) => decision_id,
        Err(response) => return response,
    };
    let owner = match decision_owner(id.clone(), &runtime) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    match owner.get(&decision_id) {
        Ok(row) => RpcResponse::success(
            id,
            DecisionResult {
                decision: wire_decision(&row, params.include_request),
            },
        ),
        Err(error_value) => decision_error_to_response(id, error_value),
    }
}

/// Handle `decision/rendering`: the decision's rendering and its canonical
/// parts for the asked part size.
pub async fn handle_rendering(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: DecisionRenderingParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let decision_id = match parse_decision_id(id.clone(), params.decision_id) {
        Ok(decision_id) => decision_id,
        Err(response) => return response,
    };
    let max_part_bytes = match params.max_part_bytes {
        None => None,
        Some(bytes) => match NonZeroU32::new(bytes) {
            Some(bytes) => Some(bytes),
            None => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    "a part size is at least one byte",
                );
            }
        },
    };
    let owner = match decision_owner(id.clone(), &runtime) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    match owner.get(&decision_id) {
        Ok(row) => {
            let rendering = row.descriptor.rendering.as_str();
            RpcResponse::success(
                id,
                DecisionRenderingResult {
                    decision_id: row.decision_id.to_string(),
                    rendering: rendering.to_owned(),
                    renderer: row.descriptor.renderer.to_string(),
                    subject_digest: row.descriptor.subject_digest.to_string(),
                    max_part_bytes: max_part_bytes.map(NonZeroU32::get),
                    parts: canonical_parts(rendering, max_part_bytes)
                        .iter()
                        .map(wire_canonical_part)
                        .collect(),
                },
            )
        }
        Err(error_value) => decision_error_to_response(id, error_value),
    }
}

/// Handle `decision/present`: record what a host presenter showed. The
/// decision counts it only when it shows every canonical part of the held
/// rendering to the party that decides now; it never makes this connection
/// a decider.
pub async fn handle_present(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: DecisionPresentParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let decision_id = match parse_decision_id(id.clone(), params.decision_id) {
        Ok(decision_id) => decision_id,
        Err(response) => return response,
    };
    let presentation = match presentation_from_wire(&params.presentation) {
        Ok(presentation) => presentation,
        Err(error_value) => return invalid(id, &error_value),
    };
    let owner = match decision_owner(id.clone(), &runtime) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    match owner.present(&decision_id, &presentation) {
        Ok(row) => RpcResponse::success(
            id,
            DecisionResult {
                decision: wire_decision(&row, false),
            },
        ),
        Err(error_value) => decision_error_to_response(id, error_value),
    }
}

/// Handle `decision/withdraw`: the connection withdraws as the host.
pub async fn handle_withdraw(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: DecisionWithdrawParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let decision_id = match parse_decision_id(id.clone(), params.decision_id) {
        Ok(decision_id) => decision_id,
        Err(response) => return response,
    };
    let owner = match decision_owner(id.clone(), &runtime) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    match owner.withdraw(&decision_id, &DecisionWithdrawer::Host) {
        Ok(row) => RpcResponse::success(
            id,
            DecisionResult {
                decision: wire_decision(&row, false),
            },
        ),
        Err(error_value) => decision_error_to_response(id, error_value),
    }
}

/// Decide a durable decision with one authenticated decision event,
/// reached only through the host-only dispatch entry. The event, not the
/// request, supplies who decided; a request that names another decision,
/// answer, digest or reason than the event binds is refused before the
/// owner is asked.
pub fn handle_decide_event(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
    event: AuthenticatedDecisionEvent,
) -> RpcResponse {
    let params: DecisionDecideParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let decision = event.decision();
    let matches_event = params.decision_id == event.decision_id().as_str()
        && params.approve == decision.approves()
        && params.presented_digest == decision.presented_digest().as_str()
        && params.reason.as_deref() == decision.reason();
    if !matches_event {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            "the decision does not match its authenticated event",
        );
    }
    let owner = match decision_owner(id.clone(), &runtime) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    match owner.decide(event.decision_id(), decision, event.eligibility()) {
        Ok(row) => RpcResponse::success(
            id,
            DecisionResult {
                decision: wire_decision(&row, false),
            },
        ),
        Err(error_value) => decision_error_to_response(id, error_value),
    }
}
