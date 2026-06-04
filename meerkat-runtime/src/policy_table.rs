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
use meerkat_core::lifecycle::RunId;
use meerkat_core::types::SessionId;

pub struct DefaultPolicyTable;

impl DefaultPolicyTable {
    #[allow(clippy::expect_used)]
    pub fn resolve(input: &Input, runtime_idle: bool) -> PolicyDecision {
        Self::try_resolve(input, runtime_idle)
            .expect("generated admission authority must resolve compatibility policy projection")
    }

    pub fn try_resolve(input: &Input, runtime_idle: bool) -> Result<PolicyDecision, String> {
        generated_admission_projection_for_input(input, runtime_idle)
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

/// Idle-steer execution-handling-mode normalization (origin/main behavior).
///
/// A peer-initiated Steer/Immediate input that arrives while the runtime is IDLE
/// has no active turn to steer into, so its fresh turn must run on the
/// queue-compatible session-service path. Inputs that carry explicit runtime
/// hints — external events, operator prompts, flow steps — keep their requested
/// handling mode (the Steer there is an authored runtime hint, not an admission
/// lane to normalize). Derived from the machine-supplied input kind, runtime-idle
/// fact, and routing disposition; shared by every admission-projection site so
/// the normalization is single-sourced.
pub(crate) fn idle_steer_execution_handling_mode(
    input_kind: InputKind,
    runtime_idle: bool,
    routing_disposition: crate::policy::RoutingDisposition,
) -> Option<meerkat_core::types::HandlingMode> {
    let peer_initiated = matches!(
        input_kind,
        InputKind::PeerMessage
            | InputKind::PeerRequest
            | InputKind::PeerResponseProgress
            | InputKind::PeerResponseTerminal
    );
    if peer_initiated
        && runtime_idle
        && matches!(
            routing_disposition,
            crate::policy::RoutingDisposition::Steer | crate::policy::RoutingDisposition::Immediate
        )
    {
        Some(meerkat_core::types::HandlingMode::Queue)
    } else {
        None
    }
}

pub(crate) fn generated_admission_projection_for_input(
    input: &Input,
    runtime_idle: bool,
) -> Result<GeneratedAdmissionProjection, String> {
    // The shell mirrors the machine's emitted projection: it threads the typed
    // input facts (kind, requested lane, continuation kind) into
    // `ResolveAdmissionPlan` and returns whatever lane/runtime-semantics the
    // machine emits. No shell-side reclassification or override.
    generated_admission_projection(
        input.id().to_string(),
        input.kind(),
        input.handling_mode().map(mm_dsl::InputLane::from),
        mm_dsl::AdmissionContinuationKind::from(input.continuation_kind()),
        runtime_idle,
    )
}

pub(crate) fn generated_admission_projection_for_kind(
    kind: KindId,
    runtime_idle: bool,
) -> Result<GeneratedAdmissionProjection, String> {
    generated_admission_projection(
        format!("policy-projection:{:?}", kind.kind()),
        kind.kind(),
        None,
        mm_dsl::AdmissionContinuationKind::Ordinary,
        runtime_idle,
    )
}

fn generated_admission_projection(
    input_id: String,
    input_kind: InputKind,
    requested_lane: Option<mm_dsl::InputLane>,
    continuation_kind: mm_dsl::AdmissionContinuationKind,
    runtime_idle: bool,
) -> Result<GeneratedAdmissionProjection, String> {
    let mut authority = projection_authority(runtime_idle)?;
    let transition = mm_dsl::MeerkatMachineMutator::apply(
        &mut authority,
        mm_dsl::MeerkatMachineInput::ResolveAdmissionPlan {
            input_id: input_id.clone(),
            input_kind: mm_dsl::AdmissionInputKind::from(input_kind),
            requested_lane,
            continuation_kind,
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
            } if effect_input_id == input_id => {
                let apply_mode: crate::policy::ApplyMode = policy_apply_mode.into();
                let boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary =
                    runtime_boundary.into();
                let routing_disposition: crate::policy::RoutingDisposition =
                    policy_routing_disposition.into();
                let execution_handling_mode = idle_steer_execution_handling_mode(
                    input_kind,
                    runtime_idle,
                    routing_disposition,
                );
                Some(GeneratedAdmissionProjection {
                    policy: PolicyDecision {
                        apply_mode,
                        wake_mode: policy_wake_mode.into(),
                        queue_mode: policy_queue_mode.into(),
                        consume_point: policy_consume_point.into(),
                        drain_policy: policy_drain_policy.into(),
                        routing_disposition,
                        record_transcript,
                        emit_operator_content: record_transcript,
                        policy_version: PolicyVersion(policy_version),
                    },
                    runtime_semantics: RuntimeInputSemantics {
                        boundary,
                        execution_kind: runtime_execution_kind.into(),
                        execution_handling_mode,
                        peer_response_terminal_apply_intent:
                            runtime_peer_response_terminal_apply_intent.map(Into::into),
                    },
                })
            }
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

    #[test]
    fn idle_steer_handling_mode_only_normalizes_idle_peer_steers() {
        use crate::policy::RoutingDisposition;
        use meerkat_core::types::HandlingMode;

        // A peer-initiated Steer/Immediate while the runtime is IDLE normalizes
        // to the queue-compatible session path (Queue).
        assert_eq!(
            idle_steer_execution_handling_mode(
                InputKind::PeerRequest,
                true,
                RoutingDisposition::Steer
            ),
            Some(HandlingMode::Queue)
        );
        assert_eq!(
            idle_steer_execution_handling_mode(
                InputKind::PeerMessage,
                true,
                RoutingDisposition::Immediate
            ),
            Some(HandlingMode::Queue)
        );

        // CONC-2: the production `runtime_idle == false` (phase == Running)
        // branch must PRESERVE the authored hint (None = no normalization). The
        // post-merge admission reconcile introduced this branch but left it
        // without a direct test; a regression here would silently re-queue
        // peer steers that should interrupt a running turn.
        assert_eq!(
            idle_steer_execution_handling_mode(
                InputKind::PeerRequest,
                false,
                RoutingDisposition::Steer
            ),
            None,
            "a peer Steer while Running must preserve its hint, not queue"
        );

        // Non-peer inputs (prompts, external events, flow steps) keep their
        // authored handling hint even when idle + Steer.
        assert_eq!(
            idle_steer_execution_handling_mode(InputKind::Prompt, true, RoutingDisposition::Steer),
            None,
            "non-peer inputs keep their authored handling hint"
        );

        // A peer input that is neither Steer nor Immediate is untouched.
        assert_eq!(
            idle_steer_execution_handling_mode(
                InputKind::PeerRequest,
                true,
                RoutingDisposition::Drop
            ),
            None
        );
    }
}
