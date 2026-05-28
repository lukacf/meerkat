use meerkat_machine_dsl::machine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptEventKind {
    #[default]
    ItemObserved,
    ItemSkipped,
    UserTranscriptFinal,
    AssistantTextDelta,
    AssistantTranscriptDelta,
    AssistantTranscriptTruncated,
    AssistantTranscriptFinalText,
    AssistantTurnCompleted,
    AssistantTurnInterrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptRoleKind {
    #[default]
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptLaneKind {
    #[default]
    Display,
    Spoken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptStopReasonKind {
    Cancelled,
    ToolUse,
    #[default]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptMaterializeDecision {
    #[default]
    Wait,
    MarkSkipped,
    MaterializeUser,
    MaterializeAssistant,
}

machine! {
    machine SessionRealtimeTranscriptAuthorityMachine {
        version: 1,
        rust: "self" / "catalog::dsl::session_realtime_transcript_authority",

        state {
            lifecycle_phase: SessionRealtimeTranscriptAuthorityPhase,
        }

        init(Ready) {}

        terminal []

        phase SessionRealtimeTranscriptAuthorityPhase {
            Ready,
        }

        input SessionRealtimeTranscriptAuthorityInput {
            ResolveItemObserved {
                role: Enum<RealtimeTranscriptRoleKind>,
                response_discarded: bool,
            },
            ResolveItemSkipped,
            ResolveUserTranscriptFinal {
                text_present: bool,
                segment_empty: bool,
                segment_matches: bool,
            },
            ResolveAssistantDelta {
                response_id_valid: bool,
                response_discarded: bool,
                delta_id_present: bool,
                delta_id_seen: bool,
                item_has_text: bool,
                current_lane: Enum<RealtimeTranscriptLaneKind>,
                requested_lane: Enum<RealtimeTranscriptLaneKind>,
                response_completed: bool,
                text_after_write_present: bool,
            },
            ResolveAssistantTextReplacement {
                response_id_valid: bool,
                response_discarded: bool,
                item_materialized: bool,
                item_has_text: bool,
                current_lane: Enum<RealtimeTranscriptLaneKind>,
                requested_lane: Enum<RealtimeTranscriptLaneKind>,
                response_completed: bool,
                text_after_replace_present: bool,
            },
            ResolveAssistantTurnCompleted {
                response_id_valid: bool,
                response_discarded: bool,
                stop_reason: Enum<RealtimeTranscriptStopReasonKind>,
            },
            ResolveAssistantTurnInterrupted {
                response_id_valid: bool,
            },
            ResolveMaterializeCandidate {
                item_materialized: bool,
                predecessor_materialized: bool,
                item_skipped: bool,
                item_ready: bool,
                item_text_present: bool,
                role: Enum<RealtimeTranscriptRoleKind>,
                response_id_present: bool,
                completion_present: bool,
                completion_usage_consumed: bool,
            },
            RestoreRealtimeTranscriptState {
                item_count: u64,
                first_seen_count: u64,
                first_seen_unique_count: u64,
                every_item_has_order_entry: bool,
                every_order_entry_has_item: bool,
                all_identity_fields_valid: bool,
                all_delta_ids_valid: bool,
                all_completion_response_ids_valid: bool,
                all_discarded_response_ids_valid: bool,
                all_materialized_items_were_ready_or_skipped: bool,
                all_assistant_items_have_response_unless_skipped: bool,
                all_ready_assistant_items_have_completion_or_are_skipped: bool,
                all_materialized_assistant_completions_consumed: bool,
                all_completed_assistant_text_items_are_ready_or_materialized_or_skipped: bool,
                all_discarded_assistant_items_are_skipped_or_materialized: bool,
            },
        }

        effect SessionRealtimeTranscriptAuthorityEffect {
            RealtimeTranscriptEventResolved {
                observe_item: bool,
                observe_skipped: bool,
                write_user_segment: bool,
                append_assistant_segment: bool,
                replace_assistant_segment: bool,
                promote_lane: bool,
                mark_item_ready: bool,
                record_delta_id: bool,
                remove_completion: bool,
                record_completion: bool,
                discard_response: bool,
                discard_response_by_lane: bool,
                mark_response_ready: bool,
                materialize_ready_items: bool,
            },
            MaterializeCandidateResolved {
                decision: Enum<RealtimeTranscriptMaterializeDecision>,
                consume_usage: bool,
            },
            SnapshotRestoreAuthorized,
        }

        helper delta_is_duplicate(delta_id_present: bool, delta_id_seen: bool) -> bool {
            delta_id_present && delta_id_seen
        }

        helper lane_accepts(
            item_has_text: bool,
            current_lane: Enum<RealtimeTranscriptLaneKind>,
            requested_lane: Enum<RealtimeTranscriptLaneKind>
        ) -> bool {
            current_lane == requested_lane || item_has_text == false
        }

        helper should_mark_ready_after_write(
            response_completed: bool,
            text_after_write_present: bool
        ) -> bool {
            response_completed && text_after_write_present
        }

        helper stop_reason_discards(stop_reason: Enum<RealtimeTranscriptStopReasonKind>) -> bool {
            stop_reason == RealtimeTranscriptStopReasonKind::Cancelled
        }

        helper stop_reason_removes_completion(stop_reason: Enum<RealtimeTranscriptStopReasonKind>) -> bool {
            stop_reason == RealtimeTranscriptStopReasonKind::ToolUse
        }

        helper stop_reason_records_completion(stop_reason: Enum<RealtimeTranscriptStopReasonKind>) -> bool {
            stop_reason == RealtimeTranscriptStopReasonKind::Other
        }

        disposition RealtimeTranscriptEventResolved => local,
        disposition MaterializeCandidateResolved => local,
        disposition SnapshotRestoreAuthorized => local,

        transition ResolveItemObservedDiscardedAssistant {
            on input ResolveItemObserved { role, response_discarded }
            guard {
                self.lifecycle_phase == Phase::Ready
                && role == RealtimeTranscriptRoleKind::Assistant
                && response_discarded
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: true,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveItemObservedPresent {
            on input ResolveItemObserved { role, response_discarded }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (role != RealtimeTranscriptRoleKind::Assistant || response_discarded == false)
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveItemSkipped {
            on input ResolveItemSkipped
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: true,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveUserTranscriptFinalEmpty {
            on input ResolveUserTranscriptFinal { text_present, segment_empty, segment_matches }
            guard {
                self.lifecycle_phase == Phase::Ready
                && text_present == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: true,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveUserTranscriptFinalStore {
            on input ResolveUserTranscriptFinal { text_present, segment_empty, segment_matches }
            guard {
                self.lifecycle_phase == Phase::Ready
                && text_present
                && segment_empty
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: true,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: true,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveUserTranscriptFinalReplayOrConflict {
            on input ResolveUserTranscriptFinal { text_present, segment_empty, segment_matches }
            guard {
                self.lifecycle_phase == Phase::Ready
                && text_present
                && segment_empty == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: true,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantDeltaInvalidOrDuplicate {
            on input ResolveAssistantDelta {
                response_id_valid,
                response_discarded,
                delta_id_present,
                delta_id_seen,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_write_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (response_id_valid == false || delta_is_duplicate(delta_id_present, delta_id_seen))
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: false
            }
        }

        transition ResolveAssistantDeltaDiscarded {
            on input ResolveAssistantDelta {
                response_id_valid,
                response_discarded,
                delta_id_present,
                delta_id_seen,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_write_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: true,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantDeltaLaneConflict {
            on input ResolveAssistantDelta {
                response_id_valid,
                response_discarded,
                delta_id_present,
                delta_id_seen,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_write_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && delta_is_duplicate(delta_id_present, delta_id_seen) == false
                && lane_accepts(item_has_text, current_lane, requested_lane) == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: delta_id_present,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantDeltaAccepted {
            on input ResolveAssistantDelta {
                response_id_valid,
                response_discarded,
                delta_id_present,
                delta_id_seen,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_write_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && delta_is_duplicate(delta_id_present, delta_id_seen) == false
                && lane_accepts(item_has_text, current_lane, requested_lane)
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: true,
                replace_assistant_segment: false,
                promote_lane: true,
                mark_item_ready: should_mark_ready_after_write(response_completed, text_after_write_present),
                record_delta_id: delta_id_present,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantReplacementInvalid {
            on input ResolveAssistantTextReplacement {
                response_id_valid,
                response_discarded,
                item_materialized,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_replace_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: false
            }
        }

        transition ResolveAssistantReplacementDiscarded {
            on input ResolveAssistantTextReplacement {
                response_id_valid,
                response_discarded,
                item_materialized,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_replace_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: true,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantReplacementLocked {
            on input ResolveAssistantTextReplacement {
                response_id_valid,
                response_discarded,
                item_materialized,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_replace_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && item_materialized
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantReplacementLaneConflict {
            on input ResolveAssistantTextReplacement {
                response_id_valid,
                response_discarded,
                item_materialized,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_replace_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && item_materialized == false
                && lane_accepts(item_has_text, current_lane, requested_lane) == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantReplacementAccepted {
            on input ResolveAssistantTextReplacement {
                response_id_valid,
                response_discarded,
                item_materialized,
                item_has_text,
                current_lane,
                requested_lane,
                response_completed,
                text_after_replace_present
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && item_materialized == false
                && lane_accepts(item_has_text, current_lane, requested_lane)
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: true,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: true,
                promote_lane: true,
                mark_item_ready: should_mark_ready_after_write(response_completed, text_after_replace_present),
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantTurnCompletedInvalid {
            on input ResolveAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: false
            }
        }

        transition ResolveAssistantTurnCompletedDiscard {
            on input ResolveAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && (response_discarded || stop_reason_discards(stop_reason))
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: true,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantTurnCompletedToolUse {
            on input ResolveAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && stop_reason_removes_completion(stop_reason)
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: true,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantTurnCompletedRecord {
            on input ResolveAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && stop_reason_records_completion(stop_reason)
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: true,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: true,
                materialize_ready_items: true
            }
        }

        transition ResolveAssistantTurnInterruptedInvalid {
            on input ResolveAssistantTurnInterrupted { response_id_valid }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid == false
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: false
            }
        }

        transition ResolveAssistantTurnInterruptedValid {
            on input ResolveAssistantTurnInterrupted { response_id_valid }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
            }
            update {}
            to Ready
            emit RealtimeTranscriptEventResolved {
                observe_item: false,
                observe_skipped: false,
                write_user_segment: false,
                append_assistant_segment: false,
                replace_assistant_segment: false,
                promote_lane: false,
                mark_item_ready: false,
                record_delta_id: false,
                remove_completion: false,
                record_completion: true,
                discard_response: false,
                discard_response_by_lane: true,
                mark_response_ready: true,
                materialize_ready_items: true
            }
        }

        transition ResolveMaterializeAlreadyDone {
            on input ResolveMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized
            }
            update {}
            to Ready
            emit MaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition ResolveMaterializeWaitForPredecessor {
            on input ResolveMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized == false
            }
            update {}
            to Ready
            emit MaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition ResolveMaterializeSkipped {
            on input ResolveMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized
                && item_skipped
            }
            update {}
            to Ready
            emit MaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::MarkSkipped,
                consume_usage: false
            }
        }

        transition ResolveMaterializeWaitForReadyText {
            on input ResolveMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized
                && item_skipped == false
                && (item_ready == false || item_text_present == false)
            }
            update {}
            to Ready
            emit MaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition ResolveMaterializeUser {
            on input ResolveMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized
                && item_skipped == false
                && item_ready
                && item_text_present
                && role == RealtimeTranscriptRoleKind::User
            }
            update {}
            to Ready
            emit MaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::MaterializeUser,
                consume_usage: false
            }
        }

        transition ResolveMaterializeAssistant {
            on input ResolveMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized
                && item_skipped == false
                && item_ready
                && item_text_present
                && role == RealtimeTranscriptRoleKind::Assistant
                && response_id_present
                && completion_present
            }
            update {}
            to Ready
            emit MaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::MaterializeAssistant,
                consume_usage: completion_usage_consumed == false
            }
        }

        transition ResolveMaterializeAssistantMissingCompletion {
            on input ResolveMaterializeCandidate {
                item_materialized,
                predecessor_materialized,
                item_skipped,
                item_ready,
                item_text_present,
                role,
                response_id_present,
                completion_present,
                completion_usage_consumed
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_materialized == false
                && predecessor_materialized
                && item_skipped == false
                && item_ready
                && item_text_present
                && role == RealtimeTranscriptRoleKind::Assistant
                && (response_id_present == false || completion_present == false)
            }
            update {}
            to Ready
            emit MaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition AuthorizeRestoreRealtimeTranscriptState {
            on input RestoreRealtimeTranscriptState {
                item_count,
                first_seen_count,
                first_seen_unique_count,
                every_item_has_order_entry,
                every_order_entry_has_item,
                all_identity_fields_valid,
                all_delta_ids_valid,
                all_completion_response_ids_valid,
                all_discarded_response_ids_valid,
                all_materialized_items_were_ready_or_skipped,
                all_assistant_items_have_response_unless_skipped,
                all_ready_assistant_items_have_completion_or_are_skipped,
                all_materialized_assistant_completions_consumed,
                all_completed_assistant_text_items_are_ready_or_materialized_or_skipped,
                all_discarded_assistant_items_are_skipped_or_materialized
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && item_count == first_seen_count
                && first_seen_count == first_seen_unique_count
                && every_item_has_order_entry
                && every_order_entry_has_item
                && all_identity_fields_valid
                && all_delta_ids_valid
                && all_completion_response_ids_valid
                && all_discarded_response_ids_valid
                && all_materialized_items_were_ready_or_skipped
                && all_assistant_items_have_response_unless_skipped
                && all_ready_assistant_items_have_completion_or_are_skipped
                && all_materialized_assistant_completions_consumed
                && all_completed_assistant_text_items_are_ready_or_materialized_or_skipped
                && all_discarded_assistant_items_are_skipped_or_materialized
            }
            update {}
            to Ready
            emit SnapshotRestoreAuthorized
        }
    }
}
