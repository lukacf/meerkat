//! §17 DefaultPolicyTable — resolves Input × runtime_idle to PolicyDecision.
//!
//! All input kinds × 2 idle states, exact per spec §17.

use crate::identifiers::{InputKind, KindId, PolicyVersion};
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
        let kind = input.kind();
        // ResponseProgress must not have its policy overridden by
        // handling_mode. Admission validation rejects this combination,
        // but the policy table also refuses to honor it so the contract
        // holds for any caller of resolve(), not just the driver path.
        let is_response_progress = matches!(kind, InputKind::PeerResponseProgress);
        if matches!(kind, InputKind::PeerResponseTerminal)
            && let Some(mode) = input.handling_mode()
        {
            let (wake_mode, drain_policy, routing_disposition) = match mode {
                meerkat_core::types::HandlingMode::Queue => (
                    WakeMode::WakeIfIdle,
                    DrainPolicy::QueueNextTurn,
                    RoutingDisposition::Queue,
                ),
                meerkat_core::types::HandlingMode::Steer => (
                    if runtime_idle {
                        WakeMode::WakeIfIdle
                    } else {
                        WakeMode::InterruptYielding
                    },
                    DrainPolicy::SteerBatch,
                    RoutingDisposition::Steer,
                ),
            };
            return pd(
                ApplyMode::StageRunStart,
                wake_mode,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                drain_policy,
                routing_disposition,
                true,
            );
        }
        if !is_response_progress && let Some(mode) = input.handling_mode() {
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

        Self::resolve_by_kind(KindId::new(kind), runtime_idle)
    }

    /// Resolve by typed kind (for testing and extensibility).
    pub fn resolve_by_kind(kind: KindId, runtime_idle: bool) -> PolicyDecision {
        match (kind.kind(), runtime_idle) {
            // PromptInput — StageRunStart, WakeIfIdle (idle) / None (running)
            (InputKind::Prompt, true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            (InputKind::Prompt, false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // PeerInput(Message) — StageRunStart, WakeIfIdle.
            //
            // A default peer message is queued work for the next turn. While
            // a turn is running it must wake the runtime after the current
            // run settles, but it must not request a cooperative boundary
            // cancel; explicit `handling_mode=steer` is the typed path for
            // in-turn steering.
            (InputKind::PeerMessage, true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            (InputKind::PeerMessage, false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // PeerInput(Request) — same as Message
            (InputKind::PeerRequest, true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            (InputKind::PeerRequest, false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // PeerInput(ResponseProgress) — StageRunBoundary, None, Coalesce
            (InputKind::PeerResponseProgress, _) => pd(
                ApplyMode::StageRunBoundary,
                WakeMode::None,
                QueueMode::Coalesce,
                ConsumePoint::OnRunComplete,
                DrainPolicy::SteerBatch,
                RoutingDisposition::Steer,
                true,
            ),

            // PeerInput(ResponseTerminal) — StageRunStart, WakeIfIdle, Fifo
            //
            // Terminal peer responses are both authoritative system-context
            // facts for later turns AND turn-kicking events for turn-driven
            // async request/response flows: a peer that issued `send_request`
            // and is waiting for the response would otherwise strand on an
            // idle session after the response lands. Staging as a runnable
            // input with `QueueMode::Fifo` queues a turn-start so the runtime
            // loop's `WakeIfIdle` path has something to dequeue and execute.
            //
            // The payload still flows through the durable typed
            // system-context append path (`input_to_context_append`), so the
            // peer terminal response fact is deduped on
            // `peer_response_terminal:{peer_id}:{request_id}` rather than
            // stacking as ordinary user appends (`input_to_append` returns
            // `None` for this convention).
            //
            // Autonomous-host members are unaffected: their continuous loop
            // dequeues and runs a turn regardless; turn-driven members (the
            // realtime audio case) now react to the response instead of
            // sitting on the appended context forever.
            (InputKind::PeerResponseTerminal, _) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // FlowStepInput — StageRunStart, WakeIfIdle/None
            (InputKind::FlowStep, true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            (InputKind::FlowStep, false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // ExternalEventInput — StageRunStart, WakeIfIdle/None
            (InputKind::ExternalEvent, true) => pd(
                ApplyMode::StageRunStart,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),
            (InputKind::ExternalEvent, false) => pd(
                ApplyMode::StageRunStart,
                WakeMode::None,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::QueueNextTurn,
                RoutingDisposition::Queue,
                true,
            ),

            // Continuation work remains explicit ordinary runtime work.
            (InputKind::Continuation, true) => pd(
                ApplyMode::StageRunBoundary,
                WakeMode::WakeIfIdle,
                QueueMode::Fifo,
                ConsumePoint::OnRunComplete,
                DrainPolicy::SteerBatch,
                RoutingDisposition::Steer,
                false,
            ),
            (InputKind::Continuation, false) => pd(
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
            (InputKind::Operation, _) => pd(
                ApplyMode::Ignore,
                WakeMode::None,
                QueueMode::Priority,
                ConsumePoint::OnAccept,
                DrainPolicy::Ignore,
                RoutingDisposition::Drop,
                false,
            ),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn assert_cell(
        kind: InputKind,
        idle: bool,
        expected_apply: ApplyMode,
        expected_wake: WakeMode,
        expected_queue: QueueMode,
        expected_consume: ConsumePoint,
        expected_transcript: bool,
    ) {
        let decision = DefaultPolicyTable::resolve_by_kind(KindId::new(kind), idle);
        assert_eq!(
            decision.apply_mode, expected_apply,
            "kind={kind:?}, idle={idle}: apply_mode"
        );
        assert_eq!(
            decision.wake_mode, expected_wake,
            "kind={kind:?}, idle={idle}: wake_mode"
        );
        assert_eq!(
            decision.queue_mode, expected_queue,
            "kind={kind:?}, idle={idle}: queue_mode"
        );
        assert_eq!(
            decision.consume_point, expected_consume,
            "kind={kind:?}, idle={idle}: consume_point"
        );
        assert_eq!(
            decision.record_transcript, expected_transcript,
            "kind={kind:?}, idle={idle}: record_transcript"
        );
    }

    #[test]
    fn prompt_idle() {
        assert_cell(
            InputKind::Prompt,
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
            InputKind::Prompt,
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
            InputKind::PeerMessage,
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
            InputKind::PeerMessage,
            false,
            ApplyMode::StageRunStart,
            WakeMode::WakeIfIdle,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn peer_request_idle() {
        assert_cell(
            InputKind::PeerRequest,
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
            InputKind::PeerRequest,
            false,
            ApplyMode::StageRunStart,
            WakeMode::WakeIfIdle,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn peer_response_progress_idle() {
        assert_cell(
            InputKind::PeerResponseProgress,
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
            InputKind::PeerResponseProgress,
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
            InputKind::PeerResponseTerminal,
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
            InputKind::PeerResponseTerminal,
            false,
            ApplyMode::StageRunStart,
            WakeMode::WakeIfIdle,
            QueueMode::Fifo,
            ConsumePoint::OnRunComplete,
            true,
        );
    }
    #[test]
    fn flow_step_idle() {
        assert_cell(
            InputKind::FlowStep,
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
            InputKind::FlowStep,
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
            InputKind::ExternalEvent,
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
            InputKind::ExternalEvent,
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
            InputKind::Continuation,
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
            InputKind::Continuation,
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
            InputKind::Operation,
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
            InputKind::Operation,
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
            typed_turn_appends: Vec::new(),
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
            typed_turn_appends: Vec::new(),
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
    fn peer_message_running_wakes_after_current_turn() {
        let decision =
            DefaultPolicyTable::resolve_by_kind(KindId::new(InputKind::PeerMessage), false);
        assert_eq!(
            decision.wake_mode,
            WakeMode::WakeIfIdle,
            "peer_message while running must wake the runtime after the current turn"
        );
    }

    #[test]
    fn peer_request_running_wakes_after_current_turn() {
        let decision =
            DefaultPolicyTable::resolve_by_kind(KindId::new(InputKind::PeerRequest), false);
        assert_eq!(
            decision.wake_mode,
            WakeMode::WakeIfIdle,
            "peer_request while running must wake the runtime after the current turn"
        );
    }

    #[test]
    fn peer_message_idle_still_wakes() {
        // Peer messages while idle should still wake normally.
        let decision =
            DefaultPolicyTable::resolve_by_kind(KindId::new(InputKind::PeerMessage), true);
        assert_eq!(
            decision.wake_mode,
            WakeMode::WakeIfIdle,
            "peer_message while idle must use WakeIfIdle"
        );
    }

    #[test]
    fn peer_request_idle_still_wakes() {
        // Peer requests while idle should still wake normally.
        let decision =
            DefaultPolicyTable::resolve_by_kind(KindId::new(InputKind::PeerRequest), true);
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
                    display_identity: None,
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

        let running_decision = DefaultPolicyTable::resolve(&input, false);
        assert_eq!(
            running_decision.routing_disposition,
            RoutingDisposition::Queue
        );
        assert_eq!(
            running_decision.wake_mode,
            WakeMode::None,
            "explicit queue means next boundary, not interrupt-yielding, while the target is running"
        );
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
    // P2: Policy table refuses to honor handling_mode for response progress
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
        assert_eq!(
            decision.apply_mode,
            ApplyMode::StageRunStart,
            "terminal peer-response apply intent owns the context+reaction boundary; steer only changes urgency/lane"
        );
        assert_eq!(decision.drain_policy, DrainPolicy::SteerBatch);
        assert_eq!(decision.wake_mode, WakeMode::WakeIfIdle);
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

        let running_decision = DefaultPolicyTable::resolve(&input, false);
        assert_eq!(
            running_decision.routing_disposition,
            RoutingDisposition::Queue
        );
        assert_eq!(running_decision.apply_mode, ApplyMode::StageRunStart);
        assert_eq!(running_decision.wake_mode, WakeMode::WakeIfIdle);
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
        // Kind default for peer_response_terminal idle: queue a turn-start so
        // turn-driven async request/response flows (realtime audio members
        // waiting for `send_response`) react to the response instead of
        // stranding on durable context. The rendered notice still flows
        // through `input_to_context_append` for authoritative system-context
        // dedup on `peer_response_terminal:{peer_id}:{request_id}`.
        assert_eq!(decision.routing_disposition, RoutingDisposition::Queue);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunStart);
        assert_eq!(decision.wake_mode, WakeMode::WakeIfIdle);
    }
}
