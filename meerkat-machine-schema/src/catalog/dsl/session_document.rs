//! SessionDocumentMachine — canonical session-document registry authority.
//!
//! This machine owns per-session "session document" lifecycle facts that are
//! consumed by every session path (`meerkat-core` session/recovery and
//! `meerkat-session` ephemeral service, including the runtime-less WASM
//! path). It is a true per-session REGISTRY keyed by `SessionId`, not a
//! stateless classifier: the canonical phase truth lives in the machine's
//! own `Map` state, and transitions compute from and mutate that map.
//!
//! For now it models only the FIRST-TURN region (ported verbatim from the
//! retired `SessionDeferredTurnAuthorityMachine`). The machine is named and
//! scoped for the broader session-document domain so later folds
//! (system-context, realtime-transcript, durable-config) can join the same
//! canonical machine.

use meerkat_machine_dsl::machine;

use super::OptionValueExt;

/// Bridging key type for session identity. Maps to `meerkat_core::SessionId`.
///
/// The DSL needs `Ord + Hash + Clone` for `Map` keys; this newtype satisfies
/// that while staying a thin wrapper over the session id string.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct SessionId(pub String);

impl<T: Into<String>> From<T> for SessionId {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Per-session first-turn lifecycle phase.
///
/// `Inactive` is the default (and the value for any session id absent from the
/// `session_first_turn_phase` map), `Pending` means the deferred first turn is
/// staged but not yet started, and `Consumed` is the absorbing terminal phase
/// once the first turn has started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionFirstTurnPhase {
    #[default]
    Inactive,
    Pending,
    Consumed,
}

/// Disposition for an initial-prompt staging decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionInitialPromptStageDecision {
    #[default]
    Clear,
    Store,
}

/// Disposition for a runtime system-context append-staging decision.
///
/// Ported verbatim from the retired `SessionSystemContextAuthorityMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SystemContextAppendDecision {
    #[default]
    Staged,
    Duplicate,
    RejectEmpty,
    RejectConflict,
}

/// Typed provenance class for a runtime system-context append.
///
/// This is the canonical replacement for the retired `runtime:steer:` string
/// prefix folklore: the producer of a runtime-steer append constructs it with
/// [`SystemContextSource::RuntimeSteer`]; everything else is
/// [`SystemContextSource::Normal`]. The machine guards the typed field — no
/// generated or shell code reclassifies a source string into this fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SystemContextSource {
    #[default]
    Normal,
    RuntimeSteer,
}

// ---------------------------------------------------------------------------
// Realtime-transcript region typed vocabulary (folded from the retired
// SessionRealtimeTranscriptAuthorityMachine).
//
// These are the SAME typed observation/decision enums the retired machine
// carried. The bulky per-item registry (`SessionRealtimeTranscriptState`,
// the content-segment maps, the causal ordering, message assembly) stays a
// NON-generated shell helper in meerkat-core: the DSL has no string-content
// op, no topological-order op, and no materialize-loop construct, so the
// shell computes those mechanical facts and feeds them as typed RAW
// observations. The machine decides the action vector / materialize verdict
// from those observations — never the other way around.
// ---------------------------------------------------------------------------

/// Provider-neutral role for a realtime transcript item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptRoleKind {
    #[default]
    User,
    Assistant,
}

/// Output lane carried by an assistant realtime transcript item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptLaneKind {
    #[default]
    Display,
    Spoken,
}

/// Terminal-boundary stop-reason class observed for a realtime assistant turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptStopReasonKind {
    Cancelled,
    ToolUse,
    #[default]
    Other,
}

/// Per-item materialization verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeTranscriptMaterializeDecision {
    #[default]
    Wait,
    MarkSkipped,
    MaterializeUser,
    MaterializeAssistant,
}

machine! {
    machine SessionDocumentMachine {
        version: 1,
        rust: "self" / "catalog::dsl::session_document",

        state {
            lifecycle_phase: SessionDocumentPhase,
            session_first_turn_phase: Map<SessionId, Enum<SessionFirstTurnPhase>>,
            session_pending_initial_prompt_present: Map<SessionId, bool>,
            session_pending_tool_results_count: Map<SessionId, u64>,
        }

        init(Ready) {
            session_first_turn_phase = EmptyMap,
            session_pending_initial_prompt_present = EmptyMap,
            session_pending_tool_results_count = EmptyMap,
        }

        terminal []

        phase SessionDocumentPhase {
            Ready,
        }

        input SessionDocumentInput {
            MarkSessionInitialTurnPending { session_id: SessionId },
            StartSessionInitialTurn { session_id: SessionId },
            StageSessionInitialPrompt { session_id: SessionId, prompt_has_content: bool },
            StageSessionToolResults { session_id: SessionId, result_count: u64 },
            ConsumeSessionDeferredInputs { session_id: SessionId },
            RestoreSessionConsumedInputs {
                session_id: SessionId,
                restore_first_turn_pending: bool,
                pending_initial_prompt_present: bool,
                pending_tool_result_message_count: u64,
            },
            RecoverSessionFirstTurnPhase {
                session_id: SessionId,
                phase: Enum<SessionFirstTurnPhase>,
                pending_initial_prompt_present: bool,
                pending_tool_result_message_count: u64,
            },
            ResolveSessionFirstTurnOverridesAllowed { session_id: SessionId },

            // -----------------------------------------------------------
            // System-context region (folded from the retired
            // SessionSystemContextAuthorityMachine).
            //
            // The bulky append payloads (text/source strings, pending/applied
            // vectors, the seen map) stay in the shell's
            // `SessionSystemContextState`. The machine owns the per-append
            // SEMANTIC decisions: append disposition (RejectEmpty / Conflict /
            // Duplicate / Staged) and the runtime-steer apply/discard
            // disposition, which guards the TYPED `SystemContextSource` field
            // instead of a `runtime:steer:` string prefix.
            // -----------------------------------------------------------
            ResolveSystemContextAppend {
                trimmed_text_byte_count: u64,
                idempotency_key_present: bool,
                existing_key_matches: bool,
                existing_key_conflicts: bool,
                active_turn_scoped: bool,
            },
            // Per-pending-append decision for `mark_pending_applied`: a
            // runtime-steer append is dropped (and its seen entry removed); a
            // normal append is promoted to applied and its seen entry marked
            // applied. The machine guards the typed `source_kind`.
            ResolveSystemContextPendingApplyItem {
                source_kind: Enum<SystemContextSource>,
            },
            // Per-item decision for transient runtime-steer cleanup: discard
            // iff the typed `source_kind` is `RuntimeSteer`.
            ResolveSystemContextSteerCleanupItem {
                source_kind: Enum<SystemContextSource>,
            },
            // Snapshot-restore consistency authorization, ported verbatim.
            RestoreSystemContextSnapshot {
                active_keys_have_known_pending_or_seen: bool,
                seen_keys_match_known_appends: bool,
            },

            // -----------------------------------------------------------
            // Realtime-transcript region (folded from the retired
            // SessionRealtimeTranscriptAuthorityMachine).
            //
            // Each input carries only typed RAW observations the shell
            // computes mechanically against its bulky
            // `SessionRealtimeTranscriptState` (set membership, segment
            // concat emptiness, per-item flags). NONE carries a pre-decided
            // action. The machine resolves the action vector / materialize
            // verdict below.
            // -----------------------------------------------------------
            ResolveRealtimeItemObserved {
                role: Enum<RealtimeTranscriptRoleKind>,
                response_discarded: bool,
            },
            ResolveRealtimeItemSkipped,
            ResolveRealtimeUserTranscriptFinal {
                text_present: bool,
                segment_empty: bool,
                segment_matches: bool,
            },
            ResolveRealtimeAssistantDelta {
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
            ResolveRealtimeAssistantTextReplacement {
                response_id_valid: bool,
                response_discarded: bool,
                item_materialized: bool,
                item_has_text: bool,
                current_lane: Enum<RealtimeTranscriptLaneKind>,
                requested_lane: Enum<RealtimeTranscriptLaneKind>,
                response_completed: bool,
                text_after_replace_present: bool,
            },
            ResolveRealtimeAssistantTurnCompleted {
                response_id_valid: bool,
                response_discarded: bool,
                stop_reason: Enum<RealtimeTranscriptStopReasonKind>,
            },
            ResolveRealtimeAssistantTurnInterrupted {
                response_id_valid: bool,
            },
            ResolveRealtimeMaterializeCandidate {
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

        effect SessionDocumentEffect {
            SessionFirstTurnPhaseResolved {
                phase: Enum<SessionFirstTurnPhase>,
                was_pending: bool,
            },
            SessionFirstTurnOverridesResolved { allowed: bool },
            SessionInitialPromptStageResolved { decision: Enum<SessionInitialPromptStageDecision> },
            SessionToolResultsStageResolved { accepted_count: u64 },
            SessionConsumedInputsRestoreResolved {
                restore_first_turn_pending: bool,
                restore_initial_prompt: bool,
                restore_tool_results: bool,
            },
            SessionFirstTurnPhaseRecovered,

            // System-context region effects.
            SystemContextAppendResolved {
                decision: Enum<SystemContextAppendDecision>,
                active_turn_scoped: bool,
            },
            // `promote_to_applied`/`mark_seen_applied` are emitted for normal
            // appends; `remove_seen` for runtime-steer appends. The shell
            // mirrors these onto its bulky pending/applied/seen collections.
            SystemContextPendingApplyItemResolved {
                promote_to_applied: bool,
                mark_seen_applied: bool,
                remove_seen: bool,
            },
            // `discard` is emitted true for runtime-steer items.
            SystemContextSteerCleanupItemResolved {
                discard: bool,
            },
            SystemContextSnapshotRestoreAuthorized,

            // Realtime-transcript region effects. The action vector is the
            // machine's decision; the shell mirrors each flag onto its bulky
            // `SessionRealtimeTranscriptState` and decides nothing.
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
            RealtimeMaterializeCandidateResolved {
                decision: Enum<RealtimeTranscriptMaterializeDecision>,
                consume_usage: bool,
            },
            RealtimeTranscriptSnapshotRestoreAuthorized,
        }

        helper phase_allows_initial_turn_overrides(phase: Enum<SessionFirstTurnPhase>) -> bool {
            phase == SessionFirstTurnPhase::Pending
        }

        helper should_store_initial_prompt(
            phase: Enum<SessionFirstTurnPhase>,
            prompt_has_content: bool
        ) -> bool {
            phase == SessionFirstTurnPhase::Pending && prompt_has_content
        }

        // System-context append classification helpers (ported verbatim from
        // the retired SessionSystemContextAuthorityMachine).
        helper append_is_empty(trimmed_text_byte_count: u64) -> bool {
            trimmed_text_byte_count == 0
        }

        helper append_is_conflict(
            idempotency_key_present: bool,
            existing_key_conflicts: bool
        ) -> bool {
            idempotency_key_present && existing_key_conflicts
        }

        helper append_is_duplicate(
            idempotency_key_present: bool,
            existing_key_matches: bool,
            existing_key_conflicts: bool
        ) -> bool {
            idempotency_key_present && existing_key_matches && existing_key_conflicts == false
        }

        helper append_is_new(
            idempotency_key_present: bool,
            existing_key_matches: bool,
            existing_key_conflicts: bool
        ) -> bool {
            idempotency_key_present == false
                || (existing_key_matches == false && existing_key_conflicts == false)
        }

        // Realtime-transcript region classification helpers (ported verbatim
        // from the retired SessionRealtimeTranscriptAuthorityMachine).
        helper realtime_delta_is_duplicate(delta_id_present: bool, delta_id_seen: bool) -> bool {
            delta_id_present && delta_id_seen
        }

        helper realtime_lane_accepts(
            item_has_text: bool,
            current_lane: Enum<RealtimeTranscriptLaneKind>,
            requested_lane: Enum<RealtimeTranscriptLaneKind>
        ) -> bool {
            current_lane == requested_lane || item_has_text == false
        }

        helper realtime_should_mark_ready_after_write(
            response_completed: bool,
            text_after_write_present: bool
        ) -> bool {
            response_completed && text_after_write_present
        }

        helper realtime_stop_reason_discards(
            stop_reason: Enum<RealtimeTranscriptStopReasonKind>
        ) -> bool {
            stop_reason == RealtimeTranscriptStopReasonKind::Cancelled
        }

        helper realtime_stop_reason_removes_completion(
            stop_reason: Enum<RealtimeTranscriptStopReasonKind>
        ) -> bool {
            stop_reason == RealtimeTranscriptStopReasonKind::ToolUse
        }

        helper realtime_stop_reason_records_completion(
            stop_reason: Enum<RealtimeTranscriptStopReasonKind>
        ) -> bool {
            stop_reason == RealtimeTranscriptStopReasonKind::Other
        }

        disposition SessionFirstTurnPhaseResolved => local,
        disposition SessionFirstTurnOverridesResolved => local,
        disposition SessionInitialPromptStageResolved => local,
        disposition SessionToolResultsStageResolved => local,
        disposition SessionConsumedInputsRestoreResolved => local,
        disposition SessionFirstTurnPhaseRecovered => local,
        disposition SystemContextAppendResolved => local,
        disposition SystemContextPendingApplyItemResolved => local,
        disposition SystemContextSteerCleanupItemResolved => local,
        disposition SystemContextSnapshotRestoreAuthorized => local,
        disposition RealtimeTranscriptEventResolved => local,
        disposition RealtimeMaterializeCandidateResolved => local,
        disposition RealtimeTranscriptSnapshotRestoreAuthorized => local,

        // ---------------------------------------------------------------
        // MarkSessionInitialTurnPending
        //
        // Old legality (MarkInitialTurnPending): Inactive|Pending -> Pending
        // emit was_pending=false; Consumed stays Consumed emit was_pending=false.
        // ---------------------------------------------------------------
        transition MarkSessionInitialTurnPendingInactiveOrPending {
            on input MarkSessionInitialTurnPending { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.session_first_turn_phase.get_cloned(session_id).get("value")
                        == SessionFirstTurnPhase::Inactive
                    || self.session_first_turn_phase.get_cloned(session_id).get("value")
                        == SessionFirstTurnPhase::Pending)
            }
            update {
                self.session_first_turn_phase.insert(session_id, SessionFirstTurnPhase::Pending);
            }
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Pending,
                was_pending: false
            }
        }

        transition MarkSessionInitialTurnPendingConsumed {
            on input MarkSessionInitialTurnPending { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Consumed
            }
            update {}
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Consumed,
                was_pending: false
            }
        }

        // ---------------------------------------------------------------
        // StartSessionInitialTurn
        //
        // Old legality (StartInitialTurn): Pending -> Consumed emit
        // was_pending=true (load-bearing for rollback); Inactive stays
        // Inactive emit was_pending=false; Consumed stays Consumed emit
        // was_pending=false. `Consumed` is the absorbing phase.
        // ---------------------------------------------------------------
        transition StartSessionInitialTurnPending {
            on input StartSessionInitialTurn { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Pending
            }
            update {
                self.session_first_turn_phase.insert(session_id, SessionFirstTurnPhase::Consumed);
            }
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Consumed,
                was_pending: true
            }
        }

        transition StartSessionInitialTurnInactive {
            on input StartSessionInitialTurn { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Inactive
            }
            update {}
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Inactive,
                was_pending: false
            }
        }

        transition StartSessionInitialTurnConsumed {
            on input StartSessionInitialTurn { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Consumed
            }
            update {}
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Consumed,
                was_pending: false
            }
        }

        // ---------------------------------------------------------------
        // ResolveSessionFirstTurnOverridesAllowed
        //
        // Old legality (AllowsInitialTurnOverrides): allowed == (phase == Pending).
        // ---------------------------------------------------------------
        transition ResolveSessionFirstTurnOverridesAllowed {
            on input ResolveSessionFirstTurnOverridesAllowed { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && phase_allows_initial_turn_overrides(
                    self.session_first_turn_phase.get_cloned(session_id).get("value")
                )
            }
            update {}
            to Ready
            emit SessionFirstTurnOverridesResolved { allowed: true }
        }

        transition ResolveSessionFirstTurnOverridesDenied {
            on input ResolveSessionFirstTurnOverridesAllowed { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && phase_allows_initial_turn_overrides(
                    self.session_first_turn_phase.get_cloned(session_id).get("value")
                ) == false
            }
            update {}
            to Ready
            emit SessionFirstTurnOverridesResolved { allowed: false }
        }

        // ---------------------------------------------------------------
        // StageSessionInitialPrompt
        //
        // Old legality (ResolveInitialPromptStage): Store iff phase == Pending
        // && prompt_has_content; else Clear. The machine also tracks
        // presence in its own state for recovery legality.
        // ---------------------------------------------------------------
        transition StageSessionInitialPromptStore {
            on input StageSessionInitialPrompt { session_id, prompt_has_content }
            guard {
                self.lifecycle_phase == Phase::Ready
                && should_store_initial_prompt(
                    self.session_first_turn_phase.get_cloned(session_id).get("value"),
                    prompt_has_content
                )
            }
            update {
                self.session_pending_initial_prompt_present.insert(session_id, true);
            }
            to Ready
            emit SessionInitialPromptStageResolved {
                decision: SessionInitialPromptStageDecision::Store
            }
        }

        transition StageSessionInitialPromptClear {
            on input StageSessionInitialPrompt { session_id, prompt_has_content }
            guard {
                self.lifecycle_phase == Phase::Ready
                && should_store_initial_prompt(
                    self.session_first_turn_phase.get_cloned(session_id).get("value"),
                    prompt_has_content
                ) == false
            }
            update {
                self.session_pending_initial_prompt_present.insert(session_id, false);
            }
            to Ready
            emit SessionInitialPromptStageResolved {
                decision: SessionInitialPromptStageDecision::Clear
            }
        }

        // ---------------------------------------------------------------
        // StageSessionToolResults
        //
        // Old legality (ResolveToolResultsStage): vacuously accepts in every
        // phase, emitting accepted_count == result_count. The machine tracks
        // the count in its own state for recovery legality.
        // ---------------------------------------------------------------
        transition StageSessionToolResults {
            on input StageSessionToolResults { session_id, result_count }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.session_first_turn_phase.get_cloned(session_id).get("value")
                        == SessionFirstTurnPhase::Inactive
                    || self.session_first_turn_phase.get_cloned(session_id).get("value")
                        == SessionFirstTurnPhase::Pending
                    || self.session_first_turn_phase.get_cloned(session_id).get("value")
                        == SessionFirstTurnPhase::Consumed)
            }
            update {
                self.session_pending_tool_results_count.insert(session_id, result_count);
            }
            to Ready
            emit SessionToolResultsStageResolved { accepted_count: result_count }
        }

        // ---------------------------------------------------------------
        // ConsumeSessionDeferredInputs
        //
        // Old legality: consuming a started turn is `StartInitialTurn`
        // (Pending -> Consumed emit was_pending=true) followed by clearing
        // the bulky payload mirrors. The shell takes the actual payloads;
        // the machine clears its presence/count mirrors and absorbs the
        // first-turn phase.
        // ---------------------------------------------------------------
        transition ConsumeSessionDeferredInputsPending {
            on input ConsumeSessionDeferredInputs { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Pending
            }
            update {
                self.session_first_turn_phase.insert(session_id, SessionFirstTurnPhase::Consumed);
                self.session_pending_initial_prompt_present.insert(session_id, false);
                self.session_pending_tool_results_count.insert(session_id, 0);
            }
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Consumed,
                was_pending: true
            }
        }

        transition ConsumeSessionDeferredInputsInactive {
            on input ConsumeSessionDeferredInputs { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Inactive
            }
            update {
                self.session_pending_initial_prompt_present.insert(session_id, false);
                self.session_pending_tool_results_count.insert(session_id, 0);
            }
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Inactive,
                was_pending: false
            }
        }

        transition ConsumeSessionDeferredInputsConsumed {
            on input ConsumeSessionDeferredInputs { session_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.session_first_turn_phase.get_cloned(session_id).get("value")
                    == SessionFirstTurnPhase::Consumed
            }
            update {
                self.session_pending_initial_prompt_present.insert(session_id, false);
                self.session_pending_tool_results_count.insert(session_id, 0);
            }
            to Ready
            emit SessionFirstTurnPhaseResolved {
                phase: SessionFirstTurnPhase::Consumed,
                was_pending: false
            }
        }

        // ---------------------------------------------------------------
        // RestoreSessionConsumedInputs
        //
        // Old legality (ResolveConsumedInputsRestore): vacuously authorizes,
        // emitting restore_initial_prompt == pending_initial_prompt_present
        // and restore_tool_results == (pending_tool_result_message_count > 0).
        // On a Consumed session that is being rolled back to Pending, restore
        // the machine-owned phase + presence/count mirrors.
        // ---------------------------------------------------------------
        transition RestoreSessionConsumedInputs {
            on input RestoreSessionConsumedInputs {
                session_id,
                restore_first_turn_pending,
                pending_initial_prompt_present,
                pending_tool_result_message_count
            }
            guard { self.lifecycle_phase == Phase::Ready && restore_first_turn_pending }
            update {
                self.session_first_turn_phase.insert(session_id, SessionFirstTurnPhase::Pending);
                self.session_pending_initial_prompt_present
                    .insert(session_id, pending_initial_prompt_present);
                self.session_pending_tool_results_count
                    .insert(session_id, pending_tool_result_message_count);
            }
            to Ready
            emit SessionConsumedInputsRestoreResolved {
                restore_first_turn_pending: restore_first_turn_pending,
                restore_initial_prompt: pending_initial_prompt_present,
                restore_tool_results: pending_tool_result_message_count > 0
            }
        }

        transition RestoreSessionConsumedInputsNoPhaseRollback {
            on input RestoreSessionConsumedInputs {
                session_id,
                restore_first_turn_pending,
                pending_initial_prompt_present,
                pending_tool_result_message_count
            }
            guard { self.lifecycle_phase == Phase::Ready && restore_first_turn_pending == false }
            update {
                self.session_pending_initial_prompt_present
                    .insert(session_id, pending_initial_prompt_present);
                self.session_pending_tool_results_count
                    .insert(session_id, pending_tool_result_message_count);
            }
            to Ready
            emit SessionConsumedInputsRestoreResolved {
                restore_first_turn_pending: restore_first_turn_pending,
                restore_initial_prompt: pending_initial_prompt_present,
                restore_tool_results: pending_tool_result_message_count > 0
            }
        }

        // ---------------------------------------------------------------
        // RecoverSessionFirstTurnPhase
        //
        // Old legality (RestoreDeferredTurnState): authorize a durable
        // snapshot restore for any first-turn phase, then adopt the restored
        // phase + presence/count into the machine-owned registry.
        // ---------------------------------------------------------------
        transition RecoverSessionFirstTurnPhase {
            on input RecoverSessionFirstTurnPhase {
                session_id,
                phase,
                pending_initial_prompt_present,
                pending_tool_result_message_count
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (phase == SessionFirstTurnPhase::Inactive
                    || phase == SessionFirstTurnPhase::Pending
                    || phase == SessionFirstTurnPhase::Consumed)
            }
            update {
                self.session_first_turn_phase.insert(session_id, phase);
                self.session_pending_initial_prompt_present
                    .insert(session_id, pending_initial_prompt_present);
                self.session_pending_tool_results_count
                    .insert(session_id, pending_tool_result_message_count);
            }
            to Ready
            emit SessionFirstTurnPhaseRecovered
        }

        // ===============================================================
        // System-context region (folded from the retired
        // SessionSystemContextAuthorityMachine).
        // ===============================================================

        // ---------------------------------------------------------------
        // ResolveSystemContextAppend — four-way append disposition.
        //
        // Ported verbatim from the retired ResolveAppend transitions. The
        // observations (key present / matches / conflicts) are mechanical
        // string-equality facts the shell computes against its bulky `seen`
        // map; the SEMANTIC disposition is decided here from those typed
        // observations via the append classification helpers.
        // ---------------------------------------------------------------
        transition ResolveSystemContextAppendEmpty {
            on input ResolveSystemContextAppend {
                trimmed_text_byte_count,
                idempotency_key_present,
                existing_key_matches,
                existing_key_conflicts,
                active_turn_scoped
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && append_is_empty(trimmed_text_byte_count)
            }
            update {}
            to Ready
            emit SystemContextAppendResolved {
                decision: SystemContextAppendDecision::RejectEmpty,
                active_turn_scoped: active_turn_scoped
            }
        }

        transition ResolveSystemContextAppendConflict {
            on input ResolveSystemContextAppend {
                trimmed_text_byte_count,
                idempotency_key_present,
                existing_key_matches,
                existing_key_conflicts,
                active_turn_scoped
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && append_is_empty(trimmed_text_byte_count) == false
                && append_is_conflict(idempotency_key_present, existing_key_conflicts)
            }
            update {}
            to Ready
            emit SystemContextAppendResolved {
                decision: SystemContextAppendDecision::RejectConflict,
                active_turn_scoped: active_turn_scoped
            }
        }

        transition ResolveSystemContextAppendDuplicate {
            on input ResolveSystemContextAppend {
                trimmed_text_byte_count,
                idempotency_key_present,
                existing_key_matches,
                existing_key_conflicts,
                active_turn_scoped
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && append_is_empty(trimmed_text_byte_count) == false
                && append_is_duplicate(
                    idempotency_key_present,
                    existing_key_matches,
                    existing_key_conflicts)
            }
            update {}
            to Ready
            emit SystemContextAppendResolved {
                decision: SystemContextAppendDecision::Duplicate,
                active_turn_scoped: active_turn_scoped
            }
        }

        transition ResolveSystemContextAppendNew {
            on input ResolveSystemContextAppend {
                trimmed_text_byte_count,
                idempotency_key_present,
                existing_key_matches,
                existing_key_conflicts,
                active_turn_scoped
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && append_is_empty(trimmed_text_byte_count) == false
                && append_is_new(
                    idempotency_key_present,
                    existing_key_matches,
                    existing_key_conflicts)
            }
            update {}
            to Ready
            emit SystemContextAppendResolved {
                decision: SystemContextAppendDecision::Staged,
                active_turn_scoped: active_turn_scoped
            }
        }

        // ---------------------------------------------------------------
        // ResolveSystemContextPendingApplyItem — per-pending-append apply
        // decision. Guards the TYPED source_kind: a runtime-steer append is
        // dropped from the applied set and its seen entry removed; a normal
        // append is promoted to applied and its seen entry marked applied.
        //
        // This replaces the retired `is_runtime_steer_append` string-prefix
        // classification (`source.starts_with("runtime:steer:")`).
        // ---------------------------------------------------------------
        transition ResolveSystemContextPendingApplyItemRuntimeSteer {
            on input ResolveSystemContextPendingApplyItem { source_kind }
            guard {
                self.lifecycle_phase == Phase::Ready
                && source_kind == SystemContextSource::RuntimeSteer
            }
            update {}
            to Ready
            emit SystemContextPendingApplyItemResolved {
                promote_to_applied: false,
                mark_seen_applied: false,
                remove_seen: true
            }
        }

        transition ResolveSystemContextPendingApplyItemNormal {
            on input ResolveSystemContextPendingApplyItem { source_kind }
            guard {
                self.lifecycle_phase == Phase::Ready
                && source_kind == SystemContextSource::Normal
            }
            update {}
            to Ready
            emit SystemContextPendingApplyItemResolved {
                promote_to_applied: true,
                mark_seen_applied: true,
                remove_seen: false
            }
        }

        // ---------------------------------------------------------------
        // ResolveSystemContextSteerCleanupItem — per-item transient-steer
        // discard decision, guarding the typed source_kind.
        // ---------------------------------------------------------------
        transition ResolveSystemContextSteerCleanupItemRuntimeSteer {
            on input ResolveSystemContextSteerCleanupItem { source_kind }
            guard {
                self.lifecycle_phase == Phase::Ready
                && source_kind == SystemContextSource::RuntimeSteer
            }
            update {}
            to Ready
            emit SystemContextSteerCleanupItemResolved { discard: true }
        }

        transition ResolveSystemContextSteerCleanupItemNormal {
            on input ResolveSystemContextSteerCleanupItem { source_kind }
            guard {
                self.lifecycle_phase == Phase::Ready
                && source_kind == SystemContextSource::Normal
            }
            update {}
            to Ready
            emit SystemContextSteerCleanupItemResolved { discard: false }
        }

        // ---------------------------------------------------------------
        // RestoreSystemContextSnapshot — snapshot-restore consistency
        // authorization, ported verbatim from AuthorizeRestoreSystemContextState.
        // ---------------------------------------------------------------
        transition RestoreSystemContextSnapshot {
            on input RestoreSystemContextSnapshot {
                active_keys_have_known_pending_or_seen,
                seen_keys_match_known_appends
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && active_keys_have_known_pending_or_seen
                && seen_keys_match_known_appends
            }
            update {}
            to Ready
            emit SystemContextSnapshotRestoreAuthorized
        }

        // ===============================================================
        // Realtime-transcript region (folded from the retired
        // SessionRealtimeTranscriptAuthorityMachine). Each transition is a
        // verbatim port: it reads only the typed RAW observations carried on
        // the input and resolves the action vector / materialize verdict.
        // The shell mirrors the emitted decision onto its bulky
        // `SessionRealtimeTranscriptState` and decides nothing.
        // ===============================================================

        transition ResolveRealtimeItemObservedDiscardedAssistant {
            on input ResolveRealtimeItemObserved { role, response_discarded }
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

        transition ResolveRealtimeItemObservedPresent {
            on input ResolveRealtimeItemObserved { role, response_discarded }
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

        transition ResolveRealtimeItemSkipped {
            on input ResolveRealtimeItemSkipped
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

        transition ResolveRealtimeUserTranscriptFinalEmpty {
            on input ResolveRealtimeUserTranscriptFinal { text_present, segment_empty, segment_matches }
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

        transition ResolveRealtimeUserTranscriptFinalStore {
            on input ResolveRealtimeUserTranscriptFinal { text_present, segment_empty, segment_matches }
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

        transition ResolveRealtimeUserTranscriptFinalReplayOrConflict {
            on input ResolveRealtimeUserTranscriptFinal { text_present, segment_empty, segment_matches }
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

        transition ResolveRealtimeAssistantDeltaInvalidOrDuplicate {
            on input ResolveRealtimeAssistantDelta {
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
                && (response_id_valid == false
                    || realtime_delta_is_duplicate(delta_id_present, delta_id_seen))
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

        transition ResolveRealtimeAssistantDeltaDiscarded {
            on input ResolveRealtimeAssistantDelta {
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

        transition ResolveRealtimeAssistantDeltaLaneConflict {
            on input ResolveRealtimeAssistantDelta {
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
                && realtime_delta_is_duplicate(delta_id_present, delta_id_seen) == false
                && realtime_lane_accepts(item_has_text, current_lane, requested_lane) == false
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

        transition ResolveRealtimeAssistantDeltaAccepted {
            on input ResolveRealtimeAssistantDelta {
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
                && realtime_delta_is_duplicate(delta_id_present, delta_id_seen) == false
                && realtime_lane_accepts(item_has_text, current_lane, requested_lane)
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
                mark_item_ready: realtime_should_mark_ready_after_write(response_completed, text_after_write_present),
                record_delta_id: delta_id_present,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantReplacementInvalid {
            on input ResolveRealtimeAssistantTextReplacement {
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

        transition ResolveRealtimeAssistantReplacementDiscarded {
            on input ResolveRealtimeAssistantTextReplacement {
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

        transition ResolveRealtimeAssistantReplacementLocked {
            on input ResolveRealtimeAssistantTextReplacement {
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

        transition ResolveRealtimeAssistantReplacementLaneConflict {
            on input ResolveRealtimeAssistantTextReplacement {
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
                && realtime_lane_accepts(item_has_text, current_lane, requested_lane) == false
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

        transition ResolveRealtimeAssistantReplacementAccepted {
            on input ResolveRealtimeAssistantTextReplacement {
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
                && realtime_lane_accepts(item_has_text, current_lane, requested_lane)
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
                mark_item_ready: realtime_should_mark_ready_after_write(response_completed, text_after_replace_present),
                record_delta_id: false,
                remove_completion: false,
                record_completion: false,
                discard_response: false,
                discard_response_by_lane: false,
                mark_response_ready: false,
                materialize_ready_items: true
            }
        }

        transition ResolveRealtimeAssistantTurnCompletedInvalid {
            on input ResolveRealtimeAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
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

        transition ResolveRealtimeAssistantTurnCompletedDiscard {
            on input ResolveRealtimeAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && (response_discarded || realtime_stop_reason_discards(stop_reason))
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

        transition ResolveRealtimeAssistantTurnCompletedToolUse {
            on input ResolveRealtimeAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && realtime_stop_reason_removes_completion(stop_reason)
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

        transition ResolveRealtimeAssistantTurnCompletedRecord {
            on input ResolveRealtimeAssistantTurnCompleted { response_id_valid, response_discarded, stop_reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && response_id_valid
                && response_discarded == false
                && realtime_stop_reason_records_completion(stop_reason)
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

        transition ResolveRealtimeAssistantTurnInterruptedInvalid {
            on input ResolveRealtimeAssistantTurnInterrupted { response_id_valid }
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

        transition ResolveRealtimeAssistantTurnInterruptedValid {
            on input ResolveRealtimeAssistantTurnInterrupted { response_id_valid }
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

        transition ResolveRealtimeMaterializeAlreadyDone {
            on input ResolveRealtimeMaterializeCandidate {
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
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition ResolveRealtimeMaterializeWaitForPredecessor {
            on input ResolveRealtimeMaterializeCandidate {
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
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition ResolveRealtimeMaterializeSkipped {
            on input ResolveRealtimeMaterializeCandidate {
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
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::MarkSkipped,
                consume_usage: false
            }
        }

        transition ResolveRealtimeMaterializeWaitForReadyText {
            on input ResolveRealtimeMaterializeCandidate {
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
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::Wait,
                consume_usage: false
            }
        }

        transition ResolveRealtimeMaterializeUser {
            on input ResolveRealtimeMaterializeCandidate {
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
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::MaterializeUser,
                consume_usage: false
            }
        }

        transition ResolveRealtimeMaterializeAssistant {
            on input ResolveRealtimeMaterializeCandidate {
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
            emit RealtimeMaterializeCandidateResolved {
                decision: RealtimeTranscriptMaterializeDecision::MaterializeAssistant,
                consume_usage: completion_usage_consumed == false
            }
        }

        transition ResolveRealtimeMaterializeAssistantMissingCompletion {
            on input ResolveRealtimeMaterializeCandidate {
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
            emit RealtimeMaterializeCandidateResolved {
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
            emit RealtimeTranscriptSnapshotRestoreAuthorized
        }
    }
}
