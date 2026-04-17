//! §17 DefaultPolicyTable — resolves Input × runtime_idle to PolicyDecision.
//!
//! All input kinds × 2 idle states, exact per spec §17.

use crate::identifiers::{KindId, PolicyVersion};
use crate::input::Input;
use crate::policy::{
    ApplyMode, ConsumePoint, DrainPolicy, PolicyDecision, QueueMode, RoutingDisposition, WakeMode,
};

/// The default policy version for the built-in table.
pub const DEFAULT_POLICY_VERSION: PolicyVersion = PolicyVersion(1);

/// Helper to construct a PolicyDecision with transcript defaults.
#[allow(clippy::too_many_arguments)]
fn pd(
    apply_mode: ApplyMode,
    wake_mode: WakeMode,
    queue_mode: QueueMode,
    consume_point: ConsumePoint,
    drain_policy: DrainPolicy,
    routing_disposition: RoutingDisposition,
    record_transcript: bool,
) -> PolicyDecision {
    PolicyDecision {
        apply_mode,
        wake_mode,
        queue_mode,
        consume_point,
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
    ///
    /// If the input carries an explicit `handling_mode`, the override is
    /// honored for actionable input kinds only. Response progress
    /// (`peer_response_progress`) always falls through to kind-based
    /// defaults — the policy table does not apply handling_mode overrides
    /// for progress updates. Response terminal inputs honor handling_mode
    /// normally.
    pub fn resolve(input: &Input, runtime_idle: bool) -> PolicyDecision {
        let kind = input.kind_id();
        // ResponseProgress must not have its policy overridden by
        // handling_mode. Admission validation rejects this combination,
        // but the policy table also refuses to honor it so the contract
        // holds for any caller of resolve(), not just the driver path.
        let is_response_convention = matches!(kind.0.as_str(), "peer_response_progress");
        if !is_response_convention && let Some(mode) = input.handling_mode() {
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
                    DrainPolicy::SteerBatch,
                    RoutingDisposition::Steer,
                    !matches!(input, Input::Continuation(_)),
                ),
            };
        }

        // kind already computed above for response-convention check.
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
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("prompt", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // PeerInput(Message) — StageRunStart, WakeIfIdle (idle) /
            // InterruptYielding (running)
            ("peer_message", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("peer_message", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::InterruptYielding,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
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
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("peer_request", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::InterruptYielding,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
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
                DrainPolicy::SteerBatch,
                RoutingDisposition::Steer,
                true,
            ),
            ("peer_response_progress", false) => pd(
                ApplyMode::StageRunBoundary,
                WakeMode::None,
                QueueMode::Coalesce,
                ConsumePoint::OnRunComplete,
                DrainPolicy::SteerBatch,
                RoutingDisposition::Steer,
                true,
            ),

            // PeerInput(ResponseTerminal) — InjectNow, WakeIfIdle
            //
            // Even while a run is currently active, terminal peer responses
            // must request a loop wake so they cannot strand in the queue after
            // the active turn finishes. The runtime now projects these inputs
            // onto the durable system-context append path instead of staging
            // them as ordinary user appends, because terminal peer responses
            // are authoritative semantic facts for later turns rather than
            // transient next-turn prompts.
            ("peer_response_terminal", true) => pd(
                ApplyMode::InjectNow,
                WakeMode::WakeIfIdle,
                QueueMode::None,
                ConsumePoint::OnApply,
                DrainPolicy::Immediate,
                RoutingDisposition::Immediate,
                true,
            ),
            ("peer_response_terminal", false) => pd(
                ApplyMode::InjectNow,
                WakeMode::WakeIfIdle,
                QueueMode::None,
                ConsumePoint::OnApply,
                DrainPolicy::Immediate,
                RoutingDisposition::Immediate,
                true,
            ),

            // FlowStepInput — StageRunStart, WakeIfIdle/None
            ("flow_step", true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("flow_step", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
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
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            ("external_event", false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
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
                DrainPolicy::SteerBatch,
                RoutingDisposition::Steer,
                false,
            ),
            ("continuation", false) => pd(
                ApplyMode::StageRunBoundary,
                WakeMode::InterruptYielding,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
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
            ApplyMode::InjectNow,
            WakeMode::WakeIfIdle,
            QueueMode::None,
            ConsumePoint::OnApply,
            true,
        );
    }
    #[test]
    fn peer_response_terminal_running() {
        assert_cell(
            "peer_response_terminal",
            false,
            ApplyMode::InjectNow,
            WakeMode::WakeIfIdle,
            QueueMode::None,
            ConsumePoint::OnApply,
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
    fn peer_message_running_stays_queued_without_wake() {
        let decision = DefaultPolicyTable::resolve_by_kind(&KindId::new("peer_message"), false);
        assert_eq!(
            decision.wake_mode,
            WakeMode::InterruptYielding,
            "peer_message while running must interrupt cooperative yielding"
        );
    }

    #[test]
    fn peer_request_running_interrupts_yielding() {
        let decision = DefaultPolicyTable::resolve_by_kind(&KindId::new("peer_request"), false);
        assert_eq!(
            decision.wake_mode,
            WakeMode::InterruptYielding,
            "peer_request while running must interrupt cooperative yielding"
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

    // -----------------------------------------------------------------------
    // Peer handling_mode override tests
    // -----------------------------------------------------------------------

    use crate::input::{
        InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
    };
    use chrono::Utc;
    use meerkat_core::lifecycle::InputId;
    use meerkat_core::types::HandlingMode;

    fn make_peer_input(
        convention: Option<PeerConvention>,
        handling_mode: Option<HandlingMode>,
    ) -> Input {
        Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention,
            body: "test".into(),
            payload: None,
            blocks: None,
            handling_mode,
        })
    }

    #[test]
    fn peer_message_with_explicit_queue_resolves_queue_semantics() {
        let input = make_peer_input(Some(PeerConvention::Message), Some(HandlingMode::Queue));
        let decision = DefaultPolicyTable::resolve(&input, true);
        assert_eq!(decision.routing_disposition, RoutingDisposition::Queue);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunStart);
    }

    #[test]
    fn peer_message_with_explicit_steer_resolves_steer_semantics() {
        let input = make_peer_input(Some(PeerConvention::Message), Some(HandlingMode::Steer));
        let decision = DefaultPolicyTable::resolve(&input, true);
        assert_eq!(decision.routing_disposition, RoutingDisposition::Steer);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunBoundary);
    }

    #[test]
    fn peer_request_with_explicit_steer_resolves_steer_semantics() {
        let input = make_peer_input(
            Some(PeerConvention::Request {
                request_id: "r".into(),
                intent: "i".into(),
            }),
            Some(HandlingMode::Steer),
        );
        let decision = DefaultPolicyTable::resolve(&input, false);
        assert_eq!(decision.routing_disposition, RoutingDisposition::Steer);
    }

    #[test]
    fn peer_no_convention_with_explicit_steer_resolves_steer_semantics() {
        let input = make_peer_input(None, Some(HandlingMode::Steer));
        let decision = DefaultPolicyTable::resolve(&input, true);
        assert_eq!(decision.routing_disposition, RoutingDisposition::Steer);
    }

    #[test]
    fn peer_message_without_override_preserves_kind_default() {
        let input = make_peer_input(Some(PeerConvention::Message), None);
        let decision = DefaultPolicyTable::resolve(&input, true);
        // Kind-based default for peer_message idle is Queue
        assert_eq!(decision.routing_disposition, RoutingDisposition::Queue);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunStart);
        assert_eq!(decision.wake_mode, WakeMode::WakeIfIdle);
    }

    // -----------------------------------------------------------------------
    // P2: Policy table refuses to honor handling_mode for response conventions
    // -----------------------------------------------------------------------

    #[test]
    fn response_progress_with_handling_mode_falls_through_to_kind_default() {
        // Even if a ResponseProgress somehow carries handling_mode=Steer,
        // the policy table must ignore it and use kind-based defaults.
        let input = make_peer_input(
            Some(PeerConvention::ResponseProgress {
                request_id: "r".into(),
                phase: crate::input::ResponseProgressPhase::InProgress,
            }),
            Some(HandlingMode::Steer),
        );
        let decision = DefaultPolicyTable::resolve(&input, true);
        // Kind default for peer_response_progress: Coalesce, StageRunBoundary, Steer
        // — but via kind-based resolution, NOT via the handling_mode override path.
        assert_eq!(decision.queue_mode, QueueMode::Coalesce);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunBoundary);
        assert_eq!(decision.wake_mode, WakeMode::None);
    }

    #[test]
    fn response_terminal_with_steer_gets_steer_semantics() {
        let input = make_peer_input(
            Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            Some(HandlingMode::Steer),
        );
        let decision = DefaultPolicyTable::resolve(&input, true);
        assert_eq!(decision.routing_disposition, RoutingDisposition::Steer);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunBoundary);
        assert!(decision.record_transcript);
    }

    #[test]
    fn response_terminal_with_queue_handling_mode_gets_queue_semantics() {
        let input = make_peer_input(
            Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            Some(HandlingMode::Queue),
        );
        let decision = DefaultPolicyTable::resolve(&input, true);
        assert_eq!(decision.routing_disposition, RoutingDisposition::Queue);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunStart);
        assert_eq!(decision.wake_mode, WakeMode::WakeIfIdle);
    }

    #[test]
    fn response_terminal_without_handling_mode_keeps_kind_default() {
        let input = make_peer_input(
            Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            None,
        );
        let decision = DefaultPolicyTable::resolve(&input, true);
        // Kind default for peer_response_terminal idle: Immediate durable
        // context projection with WakeIfIdle so later turns can observe the
        // authoritative result without helper-local shadow state.
        assert_eq!(decision.routing_disposition, RoutingDisposition::Immediate);
        assert_eq!(decision.apply_mode, ApplyMode::InjectNow);
        assert_eq!(decision.wake_mode, WakeMode::WakeIfIdle);
    }
}
