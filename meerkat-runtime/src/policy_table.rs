//! §17 DefaultPolicyTable — resolves Input × runtime_idle to PolicyDecision.
//!
//! All input kinds × 2 idle states, exact per spec §17.

use crate::identifiers::{KindId, PolicyVersion};
use crate::input::Input;
use crate::policy::{ApplyMode, ConsumePoint, PolicyDecision, QueueMode, WakeMode};

/// The default policy version for the built-in table.
pub const DEFAULT_POLICY_VERSION: PolicyVersion = PolicyVersion(1);

/// Helper to construct a PolicyDecision with transcript defaults.
fn pd(
    apply_mode: ApplyMode,
    wake_mode: WakeMode,
    queue_mode: QueueMode,
    consume_point: ConsumePoint,
    record_transcript: bool,
) -> PolicyDecision {
    PolicyDecision {
        apply_mode,
        wake_mode,
        queue_mode,
        consume_point,
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
                true,
            ),
            ("prompt", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),

            // PeerInput(Message) — StageRunStart, WakeIfIdle/None
            ("peer_message", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),
            ("peer_message", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),

            // PeerInput(Request) — same as Message
            ("peer_request", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),
            ("peer_request", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),

            // PeerInput(ResponseProgress) — StageRunBoundary, None, Coalesce
            ("peer_response_progress", true) => pd(
                ApplyMode::StageRunBoundary,
                WakeMode::None,
                QueueMode::Coalesce,
                ConsumePoint::OnRunComplete,
                true,
            ),
            ("peer_response_progress", false) => pd(
                ApplyMode::StageRunBoundary,
                WakeMode::None,
                QueueMode::Coalesce,
                ConsumePoint::OnRunComplete,
                true,
            ),

            // PeerInput(ResponseTerminal) — StageRunStart, WakeIfIdle/None
            ("peer_response_terminal", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),
            ("peer_response_terminal", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),

            // FlowStepInput — StageRunStart, WakeIfIdle/None
            ("flow_step", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),
            ("flow_step", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),

            // ExternalEventInput — StageRunStart, WakeIfIdle/None
            ("external_event", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),
            ("external_event", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                true,
            ),

            // SystemGenerated — InjectNow, no wake, OnAccept
            ("system_generated", true | false) => pd(
                ApplyMode::InjectNow,
                WakeMode::None,
                QueueMode::None,
                ConsumePoint::OnAccept,
                true,
            ),

            // Projected — Ignore, None, None(queue), OnAccept, transcript=false
            ("projected", true) => pd(
                ApplyMode::Ignore,
                WakeMode::None,
                QueueMode::None,
                ConsumePoint::OnAccept,
                false,
            ),
            ("projected", false) => pd(
                ApplyMode::Ignore,
                WakeMode::None,
                QueueMode::Coalesce,
                ConsumePoint::OnAccept,
                false,
            ),

            // Unknown kind — default conservative: StageRunStart, no wake
            (_, _) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
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
            WakeMode::None,
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
            WakeMode::None,
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
    fn system_generated_idle() {
        assert_cell(
            "system_generated",
            true,
            ApplyMode::InjectNow,
            WakeMode::None,
            QueueMode::None,
            ConsumePoint::OnAccept,
            true,
        );
    }
    #[test]
    fn system_generated_running() {
        assert_cell(
            "system_generated",
            false,
            ApplyMode::InjectNow,
            WakeMode::None,
            QueueMode::None,
            ConsumePoint::OnAccept,
            true,
        );
    }
    #[test]
    fn projected_idle() {
        assert_cell(
            "projected",
            true,
            ApplyMode::Ignore,
            WakeMode::None,
            QueueMode::None,
            ConsumePoint::OnAccept,
            false,
        );
    }
    #[test]
    fn projected_running() {
        assert_cell(
            "projected",
            false,
            ApplyMode::Ignore,
            WakeMode::None,
            QueueMode::Coalesce,
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
            turn_metadata: None,
        });
        let decision = DefaultPolicyTable::resolve(&input, true);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunStart);
        assert_eq!(decision.wake_mode, WakeMode::WakeIfIdle);
    }
}
