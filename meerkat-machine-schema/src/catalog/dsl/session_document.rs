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
    }
}
