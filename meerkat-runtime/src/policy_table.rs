//! Generated admission-policy projection facade.
//!
//! Policy/default facts are resolved by `MeerkatMachine::ResolveAdmissionPlan`.
//! This module remains as a compatibility facade for tests and older callers
//! that ask for a `PolicyDecision` directly.

use std::collections::BTreeSet;

use crate::identifiers::{InputKind, KindId, LogicalRuntimeId, PolicyVersion};
use crate::ingress_types::RuntimeInputSemantics;
use crate::input::Input;
use crate::meerkat_machine::dsl as mm_dsl;
use crate::policy::PolicyDecision;
use crate::runtime_state::RuntimeState;
use meerkat_core::lifecycle::{RunApplyBoundary, RunId, RuntimeExecutionKind};
use meerkat_core::types::SessionId;

pub struct DefaultPolicyTable;

impl DefaultPolicyTable {
    #[allow(clippy::expect_used)]
    pub fn resolve(input: &Input, runtime_idle: bool) -> PolicyDecision {
        Self::try_resolve(input, runtime_idle)
            .expect("generated admission authority must resolve compatibility policy projection")
    }

    pub fn try_resolve(input: &Input, runtime_idle: bool) -> Result<PolicyDecision, String> {
        generated_admission_projection(
            input.id().to_string(),
            input.kind(),
            requested_lane_for_admission(input),
            runtime_idle,
        )
        .map(|projection| projection.policy)
    }

    #[allow(clippy::expect_used)]
    pub fn resolve_by_kind(kind: KindId, runtime_idle: bool) -> PolicyDecision {
        Self::try_resolve_by_kind(kind, runtime_idle)
            .expect("generated admission authority must resolve compatibility policy projection")
    }

    pub fn try_resolve_by_kind(kind: KindId, runtime_idle: bool) -> Result<PolicyDecision, String> {
        generated_admission_projection_for_kind(kind, runtime_idle)
            .map(|projection| projection.policy)
    }
}

pub fn generated_default_policy_version() -> PolicyVersion {
    DefaultPolicyTable::resolve_by_kind(KindId::new(InputKind::Prompt), true).policy_version
}

pub(crate) struct GeneratedAdmissionProjection {
    pub policy: PolicyDecision,
    pub runtime_semantics: RuntimeInputSemantics,
}

pub(crate) fn generated_admission_projection_for_input(
    input: &Input,
    runtime_idle: bool,
) -> Result<GeneratedAdmissionProjection, String> {
    let mut projection = generated_admission_projection(
        input.id().to_string(),
        input.kind(),
        requested_lane_for_admission(input),
        runtime_idle,
    )?;
    if is_workgraph_attention_queue_continuation(input) {
        projection.runtime_semantics = RuntimeInputSemantics {
            boundary: RunApplyBoundary::RunStart,
            execution_kind: RuntimeExecutionKind::ContentTurn,
            peer_response_terminal_apply_intent: None,
        };
    }
    Ok(projection)
}

fn requested_lane_for_admission(input: &Input) -> Option<mm_dsl::InputLane> {
    if is_workgraph_attention_queue_continuation(input) {
        return Some(mm_dsl::InputLane::Queue);
    }
    input.handling_mode().map(mm_dsl::InputLane::from)
}

fn is_workgraph_attention_queue_continuation(input: &Input) -> bool {
    let Input::Continuation(continuation) = input else {
        return false;
    };
    continuation.reason != "workgraph_attention"
        && continuation
            .flow_tool_overlay
            .as_ref()
            .is_some_and(|overlay| {
                overlay
                    .dispatch_context
                    .contains_key("workgraph.attention_projection")
            })
}

pub(crate) fn generated_admission_projection_for_kind(
    kind: KindId,
    runtime_idle: bool,
) -> Result<GeneratedAdmissionProjection, String> {
    generated_admission_projection(
        format!("policy-projection:{:?}", kind.kind()),
        kind.kind(),
        None,
        runtime_idle,
    )
}

fn generated_admission_projection(
    input_id: String,
    input_kind: InputKind,
    requested_lane: Option<mm_dsl::InputLane>,
    runtime_idle: bool,
) -> Result<GeneratedAdmissionProjection, String> {
    let mut authority = projection_authority(runtime_idle)?;
    let transition = mm_dsl::MeerkatMachineMutator::apply(
        &mut authority,
        mm_dsl::MeerkatMachineInput::ResolveAdmissionPlan {
            input_id: input_id.clone(),
            input_kind: mm_dsl::AdmissionInputKind::from(input_kind),
            requested_lane,
            silent_intent_match: false,
            existing_superseded_input_id: None,
            runtime_running: !runtime_idle,
            active_turn_boundary_available: false,
            without_wake: false,
        },
    )
    .map_err(|err| {
        format!("generated admission authority rejected policy projection for '{input_id}': {err}")
    })?;

    transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            mm_dsl::MeerkatMachineEffect::AdmissionResolved {
                input_id: effect_input_id,
                policy_version,
                policy_apply_mode,
                policy_wake_mode,
                policy_queue_mode,
                policy_consume_point,
                policy_drain_policy,
                policy_routing_disposition,
                runtime_boundary,
                runtime_execution_kind,
                runtime_peer_response_terminal_apply_intent,
                record_transcript,
                ..
            } if effect_input_id == input_id => Some(GeneratedAdmissionProjection {
                policy: PolicyDecision {
                    apply_mode: policy_apply_mode.into(),
                    wake_mode: policy_wake_mode.into(),
                    queue_mode: policy_queue_mode.into(),
                    consume_point: policy_consume_point.into(),
                    drain_policy: policy_drain_policy.into(),
                    routing_disposition: policy_routing_disposition.into(),
                    record_transcript,
                    emit_operator_content: record_transcript,
                    policy_version: PolicyVersion(policy_version),
                },
                runtime_semantics: RuntimeInputSemantics {
                    boundary: runtime_boundary.into(),
                    execution_kind: runtime_execution_kind.into(),
                    peer_response_terminal_apply_intent:
                        runtime_peer_response_terminal_apply_intent.map(Into::into),
                },
            }),
            _ => None,
        })
        .ok_or_else(|| {
            format!("generated admission authority emitted no policy projection for '{input_id}'")
        })
}

fn projection_authority(runtime_idle: bool) -> Result<mm_dsl::MeerkatMachineAuthority, String> {
    let session_id = SessionId::new();
    let runtime_id = LogicalRuntimeId::new("policy-projection");
    let run_id = (!runtime_idle).then(RunId::new);
    let pre_run_phase = (!runtime_idle).then_some(RuntimeState::Attached);
    crate::meerkat_machine::dsl_authority::recover_authority_from_runtime_observation(
        &session_id,
        if runtime_idle {
            RuntimeState::Idle
        } else {
            RuntimeState::Running
        },
        Some(&runtime_id),
        run_id.as_ref(),
        pre_run_phase,
        BTreeSet::new(),
        None,
        None,
        None,
    )
    .map_err(|err| format!("generated admission authority recovery failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{ApplyMode, WakeMode};

    #[test]
    fn compatibility_projection_uses_generated_admission_authority() {
        let idle = DefaultPolicyTable::resolve_by_kind(KindId::new(InputKind::PeerMessage), true);
        let running =
            DefaultPolicyTable::resolve_by_kind(KindId::new(InputKind::PeerMessage), false);

        assert_eq!(idle.apply_mode, ApplyMode::StageRunStart);
        assert_eq!(idle.wake_mode, WakeMode::WakeIfIdle);
        assert_eq!(running.apply_mode, ApplyMode::StageRunStart);
        assert_eq!(running.wake_mode, WakeMode::InterruptYielding);
        assert_eq!(generated_default_policy_version(), idle.policy_version);
    }
}
