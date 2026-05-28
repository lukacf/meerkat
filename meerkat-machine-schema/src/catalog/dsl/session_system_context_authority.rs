use meerkat_machine_dsl::machine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SystemContextAppendDecision {
    #[default]
    Staged,
    Duplicate,
    RejectEmpty,
    RejectConflict,
}

machine! {
    machine SessionSystemContextAuthorityMachine {
        version: 1,
        rust: "self" / "catalog::dsl::session_system_context_authority",

        state {
            lifecycle_phase: SessionSystemContextAuthorityPhase,
        }

        init(Ready) {}

        terminal []

        phase SessionSystemContextAuthorityPhase {
            Ready,
        }

        input SessionSystemContextAuthorityInput {
            ResolveAppend {
                trimmed_text_byte_count: u64,
                idempotency_key_present: bool,
                existing_key_matches: bool,
                existing_key_conflicts: bool,
                active_turn_scoped: bool,
            },
            MarkPendingApplied { pending_count: u64 },
            DiscardUnappliedActiveTurnPending { active_turn_pending_key_count: u64 },
            DiscardActiveTurnPendingByKeys { requested_key_count: u64, active_turn_pending_key_count: u64 },
            RecordAppliedSystemContextBlocks { append_count: u64 },
            DiscardTransientRuntimeSteerState { observed_entry_count: u64 },
            RemoveRuntimeSteerPromptBlocks { observed_part_count: u64 },
            RestoreSystemContextState {
                pending_count: u64,
                applied_count: u64,
                seen_count: u64,
                active_turn_pending_key_count: u64,
                active_keys_have_known_pending_or_seen: bool,
                seen_keys_match_known_appends: bool,
            },
        }

        effect SessionSystemContextAuthorityEffect {
            AppendResolved {
                decision: Enum<SystemContextAppendDecision>,
                active_turn_scoped: bool,
            },
            PendingApplyResolved {
                apply_non_runtime_steer_appends: bool,
                mark_seen_applied: bool,
                remove_runtime_steer_seen: bool,
                clear_pending: bool,
                clear_active_turn_pending_keys: bool,
            },
            ActiveTurnPendingDiscardResolved {
                discard_matching_active_turn_keys: bool,
                remove_pending_seen_for_discarded: bool,
            },
            ActiveTurnKeyedDiscardResolved {
                discard_requested_active_turn_keys: bool,
                remove_pending_seen_for_discarded: bool,
            },
            AppliedBlocksRecordResolved {
                record_new_appends: bool,
                mark_seen_applied: bool,
            },
            TransientRuntimeSteerDiscardResolved {
                discard_runtime_steer_appends: bool,
                discard_runtime_steer_seen: bool,
                discard_runtime_steer_active_keys: bool,
            },
            RuntimeSteerPromptCleanupResolved {
                remove_runtime_steer_blocks: bool,
            },
            SnapshotRestoreAuthorized,
        }

        helper append_is_empty(trimmed_text_byte_count: u64) -> bool {
            trimmed_text_byte_count == 0
        }

        helper append_is_conflict(idempotency_key_present: bool, existing_key_conflicts: bool) -> bool {
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

        disposition AppendResolved => local,
        disposition PendingApplyResolved => local,
        disposition ActiveTurnPendingDiscardResolved => local,
        disposition ActiveTurnKeyedDiscardResolved => local,
        disposition AppliedBlocksRecordResolved => local,
        disposition TransientRuntimeSteerDiscardResolved => local,
        disposition RuntimeSteerPromptCleanupResolved => local,
        disposition SnapshotRestoreAuthorized => local,

        transition ResolveAppendEmpty {
            on input ResolveAppend {
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
            emit AppendResolved {
                decision: SystemContextAppendDecision::RejectEmpty,
                active_turn_scoped: active_turn_scoped
            }
        }

        transition ResolveAppendConflict {
            on input ResolveAppend {
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
            emit AppendResolved {
                decision: SystemContextAppendDecision::RejectConflict,
                active_turn_scoped: active_turn_scoped
            }
        }

        transition ResolveAppendDuplicate {
            on input ResolveAppend {
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
            emit AppendResolved {
                decision: SystemContextAppendDecision::Duplicate,
                active_turn_scoped: active_turn_scoped
            }
        }

        transition ResolveAppendNew {
            on input ResolveAppend {
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
            emit AppendResolved {
                decision: SystemContextAppendDecision::Staged,
                active_turn_scoped: active_turn_scoped
            }
        }

        transition AuthorizePendingApplied {
            on input MarkPendingApplied { pending_count }
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit PendingApplyResolved {
                apply_non_runtime_steer_appends: true,
                mark_seen_applied: true,
                remove_runtime_steer_seen: true,
                clear_pending: true,
                clear_active_turn_pending_keys: true
            }
        }

        transition AuthorizeDiscardUnappliedActiveTurnPending {
            on input DiscardUnappliedActiveTurnPending { active_turn_pending_key_count }
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit ActiveTurnPendingDiscardResolved {
                discard_matching_active_turn_keys: true,
                remove_pending_seen_for_discarded: true
            }
        }

        transition AuthorizeDiscardActiveTurnPendingByKeys {
            on input DiscardActiveTurnPendingByKeys { requested_key_count, active_turn_pending_key_count }
            guard {
                self.lifecycle_phase == Phase::Ready
            }
            update {}
            to Ready
            emit ActiveTurnKeyedDiscardResolved {
                discard_requested_active_turn_keys: true,
                remove_pending_seen_for_discarded: true
            }
        }

        transition AuthorizeRecordAppliedSystemContextBlocks {
            on input RecordAppliedSystemContextBlocks { append_count }
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit AppliedBlocksRecordResolved {
                record_new_appends: true,
                mark_seen_applied: true
            }
        }

        transition AuthorizeDiscardTransientRuntimeSteerState {
            on input DiscardTransientRuntimeSteerState { observed_entry_count }
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit TransientRuntimeSteerDiscardResolved {
                discard_runtime_steer_appends: true,
                discard_runtime_steer_seen: true,
                discard_runtime_steer_active_keys: true
            }
        }

        transition AuthorizeRemoveRuntimeSteerPromptBlocks {
            on input RemoveRuntimeSteerPromptBlocks { observed_part_count }
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit RuntimeSteerPromptCleanupResolved {
                remove_runtime_steer_blocks: true
            }
        }

        transition AuthorizeRestoreSystemContextState {
            on input RestoreSystemContextState {
                pending_count,
                applied_count,
                seen_count,
                active_turn_pending_key_count,
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
            emit SnapshotRestoreAuthorized
        }
    }
}
