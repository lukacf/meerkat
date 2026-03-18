//! §17 DefaultPolicyTable — resolves Input × runtime_idle to PolicyDecision.
//!
//! All input kinds × 2 idle states, exact per spec §17.

use crate::identifiers::{KindId, PolicyVersion};
use crate::input::Input;
use crate::policy::{
    ApplyMode, ConsumePoint, DrainPolicy, InterruptPolicy, PolicyDecision, QueueMode,
    RoutingDisposition, WakeMode,
};

/// The default policy version for the built-in table.
pub const DEFAULT_POLICY_VERSION: PolicyVersion = PolicyVersion(1);

/// Helper to construct a PolicyDecision with transcript defaults.
fn pd(
    apply_mode: ApplyMode,
    wake_mode: WakeMode,
    queue_mode: QueueMode,
    consume_point: ConsumePoint,
    interrupt_policy: InterruptPolicy,
    drain_policy: DrainPolicy,
    routing_disposition: RoutingDisposition,
    record_transcript: bool,
) -> PolicyDecision {
    PolicyDecision {
        apply_mode,
        wake_mode,
        queue_mode,
        consume_point,
        interrupt_policy,
        drain_policy,
        routing_disposition,
        record_transcript,
        emit_operator_content: record_transcript,
        policy_version: DEFAULT_POLICY_VERSION,
    }
}

/// Default policy table implementing §17.
pub struct DefaultPolicyTable;

impl DefaultPolicyTable {
    /// Resolve a policy decision for the given input and runtime state.
    pub fn resolve(input: &Input, runtime_idle: bool) -> PolicyDecision {
        if let Some(mode) = input.handling_mode() {
            return match mode {
                meerkat_core::types::HandlingMode::Queue => pd(
                    ApplyMode::StageRunStart,
                    if runtime_idle {
                        WakeMode::WakeIfIdle
                    } else {
                        WakeMode::None
                    },
                    QueueMode::Fifo,
                    ConsumePoint::OnRunComplete,
                    InterruptPolicy::None,
                    DrainPolicy::QueueNextTurn,
                    RoutingDisposition::Queue,
                    !matches!(input, Input::Continuation(_)),
                ),
                meerkat_core::types::HandlingMode::Steer => pd(
                    ApplyMode::StageRunBoundary,
                    if runtime_idle {
                        WakeMode::WakeIfIdle
                    } else {
                        WakeMode::InterruptYielding
                    },
                    QueueMode::Fifo,
                    ConsumePoint::OnRunComplete,
                    InterruptPolicy::InterruptYielding,
                    DrainPolicy::SteerBatch,
                    RoutingDisposition::Steer,
                    !matches!(input, Input::Continuation(_)),
                ),
            };
        }

        let kind = input.kind_id();
        Self::resolve_by_kind(&kind, runtime_idle)
    }

    /// Resolve by kind ID (for testing and extensibility).
    pub fn resolve_by_kind(kind: &KindId, runtime_idle: bool) -> PolicyDecision {
        match (kind.0.as_str(), runtime_idle) {
            // PromptInput — StageRunStart, WakeIfIdle (idle) / None (running)
            ("prompt", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("prompt", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // PeerInput(Message) — StageRunStart, WakeIfIdle (idle) /
            // InterruptYielding (running) so cooperative yielding points
            // (e.g., `wait` tool) are interrupted when a peer message arrives.
            ("peer_message", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("peer_message", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::InterruptYielding,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::InterruptYielding,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // PeerInput(Request) — same as Message
            ("peer_request", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("peer_request", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::InterruptYielding,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::InterruptYielding,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // PeerInput(ResponseProgress) — StageRunBoundary, None, Coalesce
            ("peer_response_progress", true) => pd(
                ApplyMode::StageRunBoundary,
                WakeMode::None,
                QueueMode::Coalesce,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::SteerBatch,
                RoutingDisposition::Steer,
                true,
            ),
            ("peer_response_progress", false) => pd(
                ApplyMode::StageRunBoundary,
                WakeMode::None,
                QueueMode::Coalesce,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::SteerBatch,
                RoutingDisposition::Steer,
                true,
            ),

            // PeerInput(ResponseTerminal) — StageRunStart, WakeIfIdle/None
            ("peer_response_terminal", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("peer_response_terminal", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // FlowStepInput — StageRunStart, WakeIfIdle/None
            ("flow_step", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("flow_step", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // ExternalEventInput — StageRunStart, WakeIfIdle/None
            ("external_event", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("external_event", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // Continuation work remains explicit ordinary runtime work.
            ("continuation", true) => pd(
                ApplyMode::StageRunBoundary,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::InterruptYielding,
                DrainPolicy::SteerBatch,
                RoutingDisposition::Steer,
                false,
            ),
            ("continuation", false) => pd(
                ApplyMode::StageRunBoundary,
                WakeMode::InterruptYielding,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::InterruptYielding,
                DrainPolicy::SteerBatch,
                RoutingDisposition::Steer,
                false,
            ),

            // Typed operation/lifecycle inputs are admitted explicitly but do
            // not inject ordinary transcript-visible work in this phase.
            ("operation", true | false) => pd(
                ApplyMode::Ignore,
                WakeMode::None,
                QueueMode::Priority,
                ConsumePoint::OnAccept,
                InterruptPolicy::None,
                DrainPolicy::Ignore,
                RoutingDisposition::Drop,
                false,
            ),

            // Unknown kind — default conservative: StageRunStart, no wake
            (_, _) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                InterruptPolicy::None,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn assert_cell(
        kind: &str,
        idle: bool,
        expected_apply: ApplyMode,
        expected_wake: WakeMode,
        expected_queue: QueueMode,
        expected_consume: ConsumePoint,
        expected_transcript: bool,
    ) {
        let decision = DefaultPolicyTable::resolve_by_kind(&KindId::new(kind), idle);
        assert_eq!(
            decision.apply_mode, expected_apply,
            "kind={kind}, idle={idle}: apply_mode"
        );
        assert_eq!(
            decision.wake_mode, expected_wake,
            "kind={kind}, idle={idle}: wake_mode"
        );
        assert_eq!(
            decision.queue_mode, expected_queue,
            "kind={kind}, idle={idle}: queue_mode"
        );
        assert_eq!(
            decision.consume_point, expected_consume,
            "kind={kind}, idle={idle}: consume_point"
        );
        assert_eq!(
            decision.record_transcript, expected_transcript,
            "kind={kind}, idle={idle}: record_transcript"
        );
    }

    #[test]
    fn prompt_idle() {
        assert_cell(
            "prompt",
            true,
            ApplyMode::StageRunStart,
            WakeMode::WakeIfIdle,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn prompt_running() {
        assert_cell(
            "prompt",
            false,
            ApplyMode::StageRunStart,
            WakeMode::None,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn peer_message_idle() {
        assert_cell(
            "peer_message",
            true,
            ApplyMode::StageRunStart,
            WakeMode::WakeIfIdle,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn peer_message_running() {
        assert_cell(
            "peer_message",
            false,
            ApplyMode::StageRunStart,
            WakeMode::InterruptYielding,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn peer_request_idle() {
        assert_cell(
            "peer_request",
            true,
            ApplyMode::StageRunStart,
            WakeMode::WakeIfIdle,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn peer_request_running() {
        assert_cell(
            "peer_request",
            false,
            ApplyMode::StageRunStart,
            WakeMode::InterruptYielding,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn peer_response_progress_idle() {
        assert_cell(
            "peer_response_progress",
            true,
            ApplyMode::StageRunBoundary,
            WakeMode::None,
            QueueMode::Coalesce,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn peer_response_progress_running() {
        assert_cell(
            "peer_response_progress",
            false,
            ApplyMode::StageRunBoundary,
            WakeMode::None,
            QueueMode::Coalesce,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn peer_response_terminal_idle() {
        assert_cell(
            "peer_response_terminal",
            true,
            ApplyMode::StageRunStart,
            WakeMode::WakeIfIdle,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn peer_response_terminal_running() {
        assert_cell(
            "peer_response_terminal",
            false,
            ApplyMode::StageRunStart,
            WakeMode::None,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn flow_step_idle() {
        assert_cell(
            "flow_step",
            true,
            ApplyMode::StageRunStart,
            WakeMode::WakeIfIdle,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn flow_step_running() {
        assert_cell(
            "flow_step",
            false,
            ApplyMode::StageRunStart,
            WakeMode::None,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn external_event_idle() {
        assert_cell(
            "external_event",
            true,
            ApplyMode::StageRunStart,
            WakeMode::WakeIfIdle,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn external_event_running() {
        assert_cell(
            "external_event",
            false,
            ApplyMode::StageRunStart,
            WakeMode::None,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn continuation_idle() {
        assert_cell(
            "continuation",
            true,
            ApplyMode::StageRunBoundary,
            WakeMode::WakeIfIdle,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            false,
        );
    }
    #[test]
    fn continuation_running() {
        assert_cell(
            "continuation",
            false,
            ApplyMode::StageRunBoundary,
            WakeMode::InterruptYielding,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            false,
        );
    }
    #[test]
    fn operation_idle() {
        assert_cell(
            "operation",
            true,
            ApplyMode::Ignore,
            WakeMode::None,
            QueueMode::Priority,
            ConsumePoint::OnAccept,
            false,
        );
    }
    #[test]
    fn operation_running() {
        assert_cell(
            "operation",
            false,
            ApplyMode::Ignore,
            WakeMode::None,
            QueueMode::Priority,
            ConsumePoint::OnAccept,
            false,
        );
    }

    #[test]
    fn resolve_via_input_object() {
        use crate::input::*;
        use chrono::Utc;
        use meerkat_core::lifecycle::InputId;

        let header = InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        };
        let input = Input::Prompt(PromptInput {
            header,
            text: "hello".into(),
            blocks: None,
            turn_metadata: None,
        });
        let decision = DefaultPolicyTable::resolve(&input, true);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunStart);
        assert_eq!(decision.wake_mode, WakeMode::WakeIfIdle);
    }

    #[test]
    fn explicit_steer_metadata_maps_to_checkpoint_policy() {
        use crate::input::*;
        use chrono::Utc;
        use meerkat_core::lifecycle::InputId;
        use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;

        let input = Input::Prompt(PromptInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: "hello".into(),
            blocks: None,
            turn_metadata: Some(RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            }),
        });
        let decision = DefaultPolicyTable::resolve(&input, true);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunBoundary);
        assert_eq!(decision.drain_policy, DrainPolicy::SteerBatch);
        assert_eq!(decision.routing_disposition, RoutingDisposition::Steer);
    }

    #[test]
    fn peer_message_running_interrupts_yielding() {
        // Peer messages arriving while running should use InterruptYielding
        // so cooperative yielding points (e.g., wait tool) are interrupted.
        let decision = DefaultPolicyTable::resolve_by_kind(&KindId::new("peer_message"), false);
        assert_eq!(
            decision.wake_mode,
            WakeMode::InterruptYielding,
            "peer_message while running must use InterruptYielding"
        );
        // Must not set wake (only interrupt yielding points)
        assert_ne!(
            decision.wake_mode,
            WakeMode::WakeIfIdle,
            "peer_message while running must not use WakeIfIdle"
        );
    }

    #[test]
    fn peer_request_running_interrupts_yielding() {
        // Peer requests arriving while running should use InterruptYielding.
        let decision = DefaultPolicyTable::resolve_by_kind(&KindId::new("peer_request"), false);
        assert_eq!(
            decision.wake_mode,
            WakeMode::InterruptYielding,
            "peer_request while running must use InterruptYielding"
        );
    }

    #[test]
    fn peer_message_idle_still_wakes() {
        // Peer messages while idle should still wake normally.
        let decision = DefaultPolicyTable::resolve_by_kind(&KindId::new("peer_message"), true);
        assert_eq!(
            decision.wake_mode,
            WakeMode::WakeIfIdle,
            "peer_message while idle must use WakeIfIdle"
        );
    }

    #[test]
    fn peer_request_idle_still_wakes() {
        // Peer requests while idle should still wake normally.
        let decision = DefaultPolicyTable::resolve_by_kind(&KindId::new("peer_request"), true);
        assert_eq!(
            decision.wake_mode,
            WakeMode::WakeIfIdle,
            "peer_request while idle must use WakeIfIdle"
        );
    }
}
