//! v9 runtime RPC handlers — runtime/session_status, runtime/session_submit, and
//! session/realtime_attachment_status.

use serde_json::value::RawValue;

use meerkat_contracts::{
    InputListParams, InputListResult, InputStateParams, RuntimeAcceptOutcomeType,
    RuntimeAcceptParams, RuntimeAcceptResult, RuntimeRealtimeAttachmentStatusParams,
    RuntimeRealtimeAttachmentStatusResult, RuntimeResetParams, RuntimeResetResult,
    RuntimeRetireParams, RuntimeRetireResult, RuntimeStateParams, RuntimeStateResult,
    WireInputLifecycleState, WireInputState, WireInputStateHistoryEntry,
    WireRealtimeAttachmentStatus, wire::runtime::WireInputPolicy,
};
use meerkat_core::{InputId, SessionId};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use uuid::Uuid;

use super::{RpcResponseExt, parse_params};
use crate::protocol::{RpcId, RpcResponse};

fn to_wire_realtime_attachment_status(
    status: meerkat_runtime::RealtimeAttachmentStatus,
) -> WireRealtimeAttachmentStatus {
    match status {
        meerkat_runtime::RealtimeAttachmentStatus::Unattached => {
            WireRealtimeAttachmentStatus::Unattached
        }
        meerkat_runtime::RealtimeAttachmentStatus::IntentPresentUnbound => {
            WireRealtimeAttachmentStatus::IntentPresentUnbound
        }
        meerkat_runtime::RealtimeAttachmentStatus::BindingNotReady => {
            WireRealtimeAttachmentStatus::BindingNotReady
        }
        meerkat_runtime::RealtimeAttachmentStatus::BindingReady => {
            WireRealtimeAttachmentStatus::BindingReady
        }
        meerkat_runtime::RealtimeAttachmentStatus::ReplacementPending => {
            WireRealtimeAttachmentStatus::ReplacementPending
        }
        meerkat_runtime::RealtimeAttachmentStatus::ReattachRequired => {
            WireRealtimeAttachmentStatus::ReattachRequired
        }
    }
}

fn to_wire_input_lifecycle_state(
    state: meerkat_runtime::InputLifecycleState,
) -> Result<WireInputLifecycleState, String> {
    Ok(match state {
        meerkat_runtime::InputLifecycleState::Accepted => WireInputLifecycleState::Accepted,
        meerkat_runtime::InputLifecycleState::Queued => WireInputLifecycleState::Queued,
        meerkat_runtime::InputLifecycleState::Staged => WireInputLifecycleState::Staged,
        meerkat_runtime::InputLifecycleState::Applied => WireInputLifecycleState::Applied,
        meerkat_runtime::InputLifecycleState::AppliedPendingConsumption => {
            WireInputLifecycleState::AppliedPendingConsumption
        }
        meerkat_runtime::InputLifecycleState::Consumed => WireInputLifecycleState::Consumed,
        meerkat_runtime::InputLifecycleState::Superseded => WireInputLifecycleState::Superseded,
        meerkat_runtime::InputLifecycleState::Coalesced => WireInputLifecycleState::Coalesced,
        meerkat_runtime::InputLifecycleState::Abandoned => WireInputLifecycleState::Abandoned,
        _ => return Err("unsupported input lifecycle state variant".to_string()),
    })
}

pub(crate) fn to_wire_runtime_state(
    state: meerkat_runtime::RuntimeState,
) -> meerkat_contracts::WireRuntimeState {
    match state {
        meerkat_runtime::RuntimeState::Initializing => {
            meerkat_contracts::WireRuntimeState::Initializing
        }
        meerkat_runtime::RuntimeState::Idle => meerkat_contracts::WireRuntimeState::Idle,
        meerkat_runtime::RuntimeState::Attached => meerkat_contracts::WireRuntimeState::Attached,
        meerkat_runtime::RuntimeState::Running => meerkat_contracts::WireRuntimeState::Running,
        meerkat_runtime::RuntimeState::Retired => meerkat_contracts::WireRuntimeState::Retired,
        meerkat_runtime::RuntimeState::Stopped => meerkat_contracts::WireRuntimeState::Stopped,
        meerkat_runtime::RuntimeState::Destroyed => meerkat_contracts::WireRuntimeState::Destroyed,
        _ => meerkat_contracts::WireRuntimeState::Destroyed,
    }
}

pub(crate) fn to_wire_input_state(
    bundle: meerkat_runtime::input_state::StoredInputState,
) -> Result<WireInputState, String> {
    // Wave B: fields that were raw `serde_json::Value` pre-B-9 are now
    // typed enums on the wire. The runtime-side `StoredInputState` still
    // carries the richer internal shapes, so projection happens here —
    // wave-c will plumb the typed projections through `meerkat-runtime`
    // so this fn reduces to field-wise clones. For wave-b the policy /
    // terminal_outcome / durability / reconstruction_source /
    // persisted_input fields are left as `None` until wave-c lands the
    // typed conversions on the runtime side. The structural shape is
    // already typed on the wire — callers consuming this projection see
    // typed enums or nothing.
    let meerkat_runtime::input_state::StoredInputState { state, seed } = bundle;
    let history = state
        .history()
        .iter()
        .map(|entry| {
            Ok(WireInputStateHistoryEntry {
                timestamp: entry.timestamp.to_rfc3339(),
                from: to_wire_input_lifecycle_state(entry.from)?,
                to: to_wire_input_lifecycle_state(entry.to)?,
                reason: entry.reason.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(WireInputState {
        input_id: state.input_id.to_string(),
        current_state: to_wire_input_lifecycle_state(seed.phase)?,
        policy: None,
        terminal_outcome: None,
        durability: None,
        idempotency_key: None,
        attempt_count: state.attempt_count(),
        recovery_count: state.recovery_count,
        history,
        reconstruction_source: None,
        persisted_input: None,
        last_run_id: seed.last_run_id.map(|id| id.to_string()),
        last_boundary_sequence: seed.last_boundary_sequence,
        created_at: state.created_at.to_rfc3339(),
        updated_at: state.updated_at().to_rfc3339(),
    })
}

pub(crate) fn to_wire_accept_result(
    outcome: meerkat_runtime::AcceptOutcome,
) -> Result<RuntimeAcceptResult, String> {
    Ok(match outcome {
        meerkat_runtime::AcceptOutcome::Accepted {
            input_id,
            policy,
            state,
        } => {
            // Re-bundle the returned shell with a queue-seeded seed for the
            // wire payload. accept_input transitions the DSL to Queued for
            // durable/queued inputs; callers that need exact phase/run_id
            // should use input_state lookups post-accept.
            let bundle = meerkat_runtime::input_state::StoredInputState {
                state,
                seed: meerkat_runtime::input_state::InputStateSeed {
                    phase: meerkat_runtime::InputLifecycleState::Queued,
                    last_run_id: None,
                    last_boundary_sequence: None,
                    terminal_outcome: None,
                    attempt_count: 0,
                },
            };
            // Wave B: policy is a typed `WireInputPolicy`. The runtime
            // `policy` value is not yet richly projectable without wave-c
            // plumbing, so callers get `None` until the typed projection
            // lands; the structural shape is typed regardless.
            let _ = policy;
            RuntimeAcceptResult {
                outcome_type: RuntimeAcceptOutcomeType::Accepted,
                input_id: Some(input_id.to_string()),
                existing_id: None,
                reason: None,
                policy: Option::<WireInputPolicy>::None,
                state: Some(to_wire_input_state(bundle)?),
            }
        }
        meerkat_runtime::AcceptOutcome::Deduplicated {
            input_id,
            existing_id,
        } => RuntimeAcceptResult {
            outcome_type: RuntimeAcceptOutcomeType::Deduplicated,
            input_id: Some(input_id.to_string()),
            existing_id: Some(existing_id.to_string()),
            reason: None,
            policy: None,
            state: None,
        },
        meerkat_runtime::AcceptOutcome::Rejected { reason } => RuntimeAcceptResult {
            outcome_type: RuntimeAcceptOutcomeType::Rejected,
            input_id: None,
            existing_id: None,
            reason: Some(reason.to_string()),
            policy: None,
            state: None,
        },
        _ => return Err("unsupported accept outcome variant".to_string()),
    })
}

// ---- Handlers ----

pub async fn handle_runtime_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: RuntimeStateParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };

    match adapter.runtime_state(&session_id).await {
        Ok(state) => RpcResponse::success(
            id,
            RuntimeStateResult {
                state: to_wire_runtime_state(state),
            },
        ),
        Err(err) => RpcResponse::error(id, crate::error::INVALID_PARAMS, err.to_string()),
    }
}

pub async fn handle_runtime_submit(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: RuntimeAcceptParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let input = match serde_json::from_value::<meerkat_runtime::Input>(params.input) {
        Ok(input) => input,
        Err(err) => {
            return RpcResponse::error(
                id,
                crate::error::INVALID_PARAMS,
                format!("invalid runtime input: {err}"),
            );
        }
    };
    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };

    match adapter.accept_input(&session_id, input).await {
        Ok(outcome) => match to_wire_accept_result(outcome) {
            Ok(result) => RpcResponse::success(id, result),
            Err(err) => RpcResponse::error(id, crate::error::INTERNAL_ERROR, err),
        },
        Err(err) => RpcResponse::error(id, crate::error::INVALID_PARAMS, err.to_string()),
    }
}

pub async fn handle_runtime_submission(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: InputStateParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };
    let input_id = match Uuid::parse_str(&params.input_id) {
        Ok(id) => InputId::from_uuid(id),
        Err(err) => {
            return RpcResponse::error(
                id,
                crate::error::INVALID_PARAMS,
                format!("invalid input_id: {err}"),
            );
        }
    };

    match adapter.input_state(&session_id, &input_id).await {
        Ok(Some(state)) => match to_wire_input_state(state) {
            Ok(result) => RpcResponse::success(id, result),
            Err(err) => RpcResponse::error(id, crate::error::INTERNAL_ERROR, err),
        },
        Ok(None) => RpcResponse::error(id, crate::error::INVALID_PARAMS, "input not found"),
        Err(err) => RpcResponse::error(id, crate::error::INVALID_PARAMS, err.to_string()),
    }
}

pub async fn handle_runtime_submissions(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: InputListParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };

    let input_ids = match adapter.list_active_inputs(&session_id).await {
        Ok(ids) => ids,
        Err(err) => return RpcResponse::error(id, crate::error::INVALID_PARAMS, err.to_string()),
    };
    let mut inputs = Vec::with_capacity(input_ids.len());
    for input_id in input_ids {
        match adapter.input_state(&session_id, &input_id).await {
            Ok(Some(state)) => match to_wire_input_state(state) {
                Ok(wire) => inputs.push(wire),
                Err(err) => return RpcResponse::error(id, crate::error::INTERNAL_ERROR, err),
            },
            Ok(None) => {}
            Err(err) => {
                return RpcResponse::error(id, crate::error::INVALID_PARAMS, err.to_string());
            }
        }
    }
    RpcResponse::success(id, InputListResult { inputs })
}

pub async fn handle_runtime_retire(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: RuntimeRetireParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };
    match adapter.retire_runtime(&session_id).await {
        Ok(report) => RpcResponse::success(
            id,
            RuntimeRetireResult {
                inputs_abandoned: report.inputs_abandoned,
                inputs_pending_drain: report.inputs_pending_drain,
            },
        ),
        Err(err) => RpcResponse::error(id, crate::error::INVALID_PARAMS, err.to_string()),
    }
}

pub async fn handle_runtime_reset(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: RuntimeResetParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };
    match adapter.reset_runtime(&session_id).await {
        Ok(report) => RpcResponse::success(
            id,
            RuntimeResetResult {
                inputs_abandoned: report.inputs_abandoned,
            },
        ),
        Err(err) => RpcResponse::error(id, crate::error::INVALID_PARAMS, err.to_string()),
    }
}

/// Handle `session/realtime_attachment_status` — get live attachment status for a session.
pub async fn handle_runtime_realtime_attachment_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: RuntimeRealtimeAttachmentStatusParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };

    match adapter.realtime_attachment_status(&session_id).await {
        Ok(status) => RpcResponse::success(
            id,
            RuntimeRealtimeAttachmentStatusResult {
                status: to_wire_realtime_attachment_status(status),
            },
        ),
        Err(e) => RpcResponse::error(id, crate::error::INVALID_PARAMS, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::InputId;
    use meerkat_runtime::input_state::StoredInputState;
    use meerkat_runtime::{
        AcceptOutcome, Input, ResetReport, RetireReport, RuntimeDriverError, RuntimeMode,
        RuntimeState,
    };
    use serde_json::{json, value::to_raw_value};

    struct RetiredRejectingAdapter;

    #[async_trait]
    impl SessionServiceRuntimeExt for RetiredRejectingAdapter {
        fn runtime_mode(&self) -> RuntimeMode {
            RuntimeMode::V9Compliant
        }

        async fn accept_input(
            &self,
            _session_id: &SessionId,
            _input: Input,
        ) -> Result<AcceptOutcome, RuntimeDriverError> {
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Retired,
            })
        }

        async fn accept_input_with_completion(
            &self,
            _session_id: &SessionId,
            _input: Input,
        ) -> Result<(AcceptOutcome, Option<meerkat_runtime::CompletionHandle>), RuntimeDriverError>
        {
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Retired,
            })
        }

        async fn runtime_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<RuntimeState, RuntimeDriverError> {
            Ok(RuntimeState::Retired)
        }

        async fn realtime_attachment_status(
            &self,
            _session_id: &SessionId,
        ) -> Result<meerkat_runtime::RealtimeAttachmentStatus, RuntimeDriverError> {
            Ok(meerkat_runtime::RealtimeAttachmentStatus::Unattached)
        }

        async fn realtime_channel_status(
            &self,
            _session_id: &SessionId,
        ) -> Result<meerkat_contracts::RealtimeChannelStatus, RuntimeDriverError> {
            Ok(meerkat_runtime::RealtimeAttachmentStatus::Unattached.into())
        }

        async fn retire_runtime(
            &self,
            _session_id: &SessionId,
        ) -> Result<RetireReport, RuntimeDriverError> {
            Ok(RetireReport {
                inputs_abandoned: 0,
                inputs_pending_drain: 0,
            })
        }

        async fn reset_runtime(
            &self,
            _session_id: &SessionId,
        ) -> Result<ResetReport, RuntimeDriverError> {
            Ok(ResetReport {
                inputs_abandoned: 0,
            })
        }

        async fn input_state(
            &self,
            _session_id: &SessionId,
            _input_id: &InputId,
        ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
            Ok(None)
        }

        async fn list_active_inputs(
            &self,
            _session_id: &SessionId,
        ) -> Result<Vec<InputId>, RuntimeDriverError> {
            Ok(Vec::new())
        }

        async fn reconfigure_session_llm_identity(
            &self,
            _session_id: &SessionId,
            _request: meerkat_runtime::SessionLlmReconfigureRequest,
        ) -> Result<meerkat_runtime::SessionLlmReconfigureReport, RuntimeDriverError> {
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Retired,
            })
        }
    }

    #[tokio::test]
    async fn realtime_attachment_status_returns_runtime_owned_status() {
        struct ReattachRequiredAdapter;

        #[async_trait::async_trait]
        impl SessionServiceRuntimeExt for ReattachRequiredAdapter {
            fn runtime_mode(&self) -> RuntimeMode {
                RuntimeMode::V9Compliant
            }

            async fn accept_input(
                &self,
                _session_id: &SessionId,
                _input: Input,
            ) -> Result<AcceptOutcome, RuntimeDriverError> {
                Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Retired,
                })
            }

            async fn accept_input_with_completion(
                &self,
                _session_id: &SessionId,
                _input: Input,
            ) -> Result<
                (AcceptOutcome, Option<meerkat_runtime::CompletionHandle>),
                RuntimeDriverError,
            > {
                Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Retired,
                })
            }

            async fn runtime_state(
                &self,
                _session_id: &SessionId,
            ) -> Result<RuntimeState, RuntimeDriverError> {
                Ok(RuntimeState::Retired)
            }

            async fn realtime_attachment_status(
                &self,
                _session_id: &SessionId,
            ) -> Result<meerkat_runtime::RealtimeAttachmentStatus, RuntimeDriverError> {
                Ok(meerkat_runtime::RealtimeAttachmentStatus::ReattachRequired)
            }

            async fn realtime_channel_status(
                &self,
                _session_id: &SessionId,
            ) -> Result<meerkat_contracts::RealtimeChannelStatus, RuntimeDriverError> {
                Ok(meerkat_runtime::RealtimeAttachmentStatus::ReattachRequired.into())
            }

            async fn retire_runtime(
                &self,
                _session_id: &SessionId,
            ) -> Result<RetireReport, RuntimeDriverError> {
                Ok(RetireReport {
                    inputs_abandoned: 0,
                    inputs_pending_drain: 0,
                })
            }

            async fn reset_runtime(
                &self,
                _session_id: &SessionId,
            ) -> Result<ResetReport, RuntimeDriverError> {
                Ok(ResetReport {
                    inputs_abandoned: 0,
                })
            }

            async fn input_state(
                &self,
                _session_id: &SessionId,
                _input_id: &InputId,
            ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
                Ok(None)
            }

            async fn list_active_inputs(
                &self,
                _session_id: &SessionId,
            ) -> Result<Vec<InputId>, RuntimeDriverError> {
                Ok(Vec::new())
            }

            async fn reconfigure_session_llm_identity(
                &self,
                _session_id: &SessionId,
                _request: meerkat_runtime::SessionLlmReconfigureRequest,
            ) -> Result<meerkat_runtime::SessionLlmReconfigureReport, RuntimeDriverError>
            {
                Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Retired,
                })
            }
        }

        let params_result = to_raw_value(&json!({
            "session_id": uuid::Uuid::new_v4().to_string(),
        }));
        assert!(params_result.is_ok(), "serialize params should succeed");
        let Some(params) = params_result.ok() else {
            return;
        };

        let response = handle_runtime_realtime_attachment_status(
            Some(RpcId::Num(7)),
            Some(params.as_ref()),
            &ReattachRequiredAdapter,
        )
        .await;
        assert!(response.error.is_none(), "runtime status should succeed");
        assert!(
            response.result.is_some(),
            "runtime status result should be present"
        );
        let Some(result) = response.result else {
            return;
        };
        let value_result: Result<serde_json::Value, _> = serde_json::from_str(result.get());
        assert!(value_result.is_ok(), "json result parse should succeed");
        let Some(value) = value_result.ok() else {
            return;
        };
        assert_eq!(value["status"], "reattach_required");
    }
}
