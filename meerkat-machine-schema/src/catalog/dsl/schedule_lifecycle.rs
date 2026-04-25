use meerkat_machine_dsl::machine;

machine! {
    machine ScheduleLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::schedule_lifecycle",

        state {
            lifecycle_phase: ScheduleLifecycleState,
            revision: u64,
            trigger_key: String,
            target_binding_key: String,
            misfire_policy: Enum<MisfirePolicy>,
            overlap_policy: Enum<OverlapPolicy>,
            missing_target_policy: Enum<MissingTargetPolicy>,
            planning_cursor_utc_ms: Option<u64>,
            next_occurrence_ordinal: u64,
            // Reciprocal-ack accumulator (wave-d D-f): occurrence ids
            // whose supersession the occurrence authority has confirmed.
            // The schedule side observes the completion of the
            // SupersedePendingOccurrences route by counting acks rather
            // than assuming every in-flight Supersede landed.
            superseded_ack_ids: Set<OccurrenceId>,
        }

        init(Active) {
            revision = 1,
            trigger_key = "trigger-0",
            target_binding_key = "target-0",
            misfire_policy = MisfirePolicy::Skip,
            overlap_policy = OverlapPolicy::SkipIfRunning,
            missing_target_policy = MissingTargetPolicy::MarkMisfired,
            planning_cursor_utc_ms = None,
            next_occurrence_ordinal = 0,
            superseded_ack_ids = EmptySet,
        }

        terminal [Deleted]

        phase ScheduleLifecycleState {
            Active,
            Paused,
            Deleted,
        }

        input ScheduleLifecycleInput {
            Create {
                trigger_key: String,
                target_binding_key: String,
                misfire_policy: Enum<MisfirePolicy>,
                overlap_policy: Enum<OverlapPolicy>,
                missing_target_policy: Enum<MissingTargetPolicy>,
            },
            Revise {
                trigger_key: String,
                target_binding_key: String,
                misfire_policy: Enum<MisfirePolicy>,
                overlap_policy: Enum<OverlapPolicy>,
                missing_target_policy: Enum<MissingTargetPolicy>,
            },
            RecordPlanningWindow {
                planning_cursor_utc_ms: u64,
                next_occurrence_ordinal: u64,
            },
            Pause { at_utc_ms: u64 },
            Resume { at_utc_ms: u64 },
            Delete { at_utc_ms: u64 },
            // Reciprocal ack (wave-d D-f): OccurrenceLifecycleMachine
            // reports back the occurrence_id it superseded along with
            // the revision that requested the supersession. The schedule
            // side records the ack so the SupersedePendingOccurrences
            // route has an observable completion signal.
            ConfirmOccurrencesSuperseded { occurrence_id: OccurrenceId, superseding_revision: u64 },
        }

        effect ScheduleLifecycleEffect {
            EmitScheduleNotice { new_state: ScheduleLifecycleState, revision: u64 },
            SupersedePendingOccurrences { superseding_revision: u64 },
            PlanningWindowRecorded { planning_cursor_utc_ms: u64, next_occurrence_ordinal: u64 },
        }

        invariant revision_is_positive {
            self.revision > 0
        }

        invariant deleted_has_no_planning_cursor {
            self.lifecycle_phase != Phase::Deleted || self.planning_cursor_utc_ms == None
        }

        invariant planning_cursor_requires_occurrence_progress {
            self.planning_cursor_utc_ms == None || self.next_occurrence_ordinal > 0
        }

        disposition EmitScheduleNotice => external,
        disposition SupersedePendingOccurrences => routed [OccurrenceLifecycleMachine],
        disposition PlanningWindowRecorded => local,

        // --- Create (only from Active, self-loop) ---

        transition CreateSchedule {
            on input Create { trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy }
            guard { self.lifecycle_phase == Phase::Active }
            update {
                self.trigger_key = trigger_key;
                self.target_binding_key = target_binding_key;
                self.misfire_policy = misfire_policy;
                self.overlap_policy = overlap_policy;
                self.missing_target_policy = missing_target_policy;
            }
            to Active
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
        }

        // --- Revise (per-phase, bumps revision) ---

        transition ReviseActive {
            on input Revise { trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy }
            guard { self.lifecycle_phase == Phase::Active }
            update {
                self.trigger_key = trigger_key;
                self.target_binding_key = target_binding_key;
                self.misfire_policy = misfire_policy;
                self.overlap_policy = overlap_policy;
                self.missing_target_policy = missing_target_policy;
                self.revision += 1;
                self.planning_cursor_utc_ms = None;
            }
            to Active
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            emit SupersedePendingOccurrences { superseding_revision: self.revision }
        }

        transition RevisePaused {
            on input Revise { trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy }
            guard { self.lifecycle_phase == Phase::Paused }
            update {
                self.trigger_key = trigger_key;
                self.target_binding_key = target_binding_key;
                self.misfire_policy = misfire_policy;
                self.overlap_policy = overlap_policy;
                self.missing_target_policy = missing_target_policy;
                self.revision += 1;
                self.planning_cursor_utc_ms = None;
            }
            to Paused
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            emit SupersedePendingOccurrences { superseding_revision: self.revision }
        }

        // --- Record planning window (per-phase, with guard) ---

        transition RecordPlanningWindowActive {
            on input RecordPlanningWindow { planning_cursor_utc_ms, next_occurrence_ordinal }
            guard "planning_window_advances_ordinal" { self.lifecycle_phase == Phase::Active && next_occurrence_ordinal > 0 }
            update {
                self.planning_cursor_utc_ms = Some(planning_cursor_utc_ms);
                self.next_occurrence_ordinal = next_occurrence_ordinal;
            }
            to Active
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            emit PlanningWindowRecorded { planning_cursor_utc_ms: planning_cursor_utc_ms, next_occurrence_ordinal: next_occurrence_ordinal }
        }

        // NB: no `RecordPlanningWindowPaused` — planning only advances while
        // the schedule is Active. Paused schedules MUST reject
        // `RecordPlanningWindow` as an invalid transition; this closes the
        // race where a driver tick could race with `Pause` and silently
        // advance the planning cursor against a paused schedule.

        // --- Pause / Resume (from Active or Paused) ---

        transition PauseActiveOrPaused {
            on input Pause { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Active || self.lifecycle_phase == Phase::Paused }
            update {}
            to Paused
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
        }

        transition ResumeActiveOrPaused {
            on input Resume { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Active || self.lifecycle_phase == Phase::Paused }
            update {}
            to Active
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
        }

        // --- Delete (per-phase, bumps revision) ---

        transition DeleteActive {
            on input Delete { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Active }
            update {
                self.revision += 1;
                self.planning_cursor_utc_ms = None;
            }
            to Deleted
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            emit SupersedePendingOccurrences { superseding_revision: self.revision }
        }

        transition DeletePaused {
            on input Delete { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Paused }
            update {
                self.revision += 1;
                self.planning_cursor_utc_ms = None;
            }
            to Deleted
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            emit SupersedePendingOccurrences { superseding_revision: self.revision }
        }

        // Idempotent no-op: Delete applied to an already-Deleted schedule
        // leaves state unchanged and emits zero effects. Without this the
        // authority would return NoMatchingTransition and shell callers
        // would be forced to re-derive the phase before firing Delete.
        transition DeleteDeleted {
            on input Delete { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Deleted }
            update {}
            to Deleted
        }

        // --- Reciprocal ack (wave-d D-f) ---
        //
        // The occurrence authority absorbs Supersede and reports its own
        // occurrence_id back to the schedule. We accept the ack in every
        // phase — a revision-affecting transition (Revise*, Delete*) can
        // leave the schedule in Deleted while acks are still arriving,
        // and the acks must land regardless of the schedule's onward
        // trajectory.

        transition ConfirmOccurrencesSupersededActive {
            on input ConfirmOccurrencesSuperseded { occurrence_id, superseding_revision }
            guard { self.lifecycle_phase == Phase::Active }
            update {
                self.superseded_ack_ids.insert(occurrence_id);
            }
            to Active
        }

        transition ConfirmOccurrencesSupersededPaused {
            on input ConfirmOccurrencesSuperseded { occurrence_id, superseding_revision }
            guard { self.lifecycle_phase == Phase::Paused }
            update {
                self.superseded_ack_ids.insert(occurrence_id);
            }
            to Paused
        }

        transition ConfirmOccurrencesSupersededDeleted {
            on input ConfirmOccurrencesSuperseded { occurrence_id, superseding_revision }
            guard { self.lifecycle_phase == Phase::Deleted }
            update {
                self.superseded_ack_ids.insert(occurrence_id);
            }
            to Deleted
        }
    }
}

// Stub types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MisfirePolicy {
    Skip,
    Execute,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapPolicy {
    SkipIfRunning,
    Queue,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingTargetPolicy {
    MarkMisfired,
    Skip,
}

// OccurrenceId is defined alongside OccurrenceLifecycleMachine; redeclare
// locally so the schedule-lifecycle DSL macro can compile its generated
// struct. The typed atom (`NamedTypeBinding::string("OccurrenceId")`)
// registered in `dsl/mod.rs` keeps schema emission consistent with the
// occurrence side.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct OccurrenceId(pub String);
impl<T: Into<String>> From<T> for OccurrenceId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn delete_from_deleted_is_noop() {
        let mut auth = ScheduleLifecycleMachineAuthority::new();

        let first = ScheduleLifecycleMachineMutator::apply(
            &mut auth,
            ScheduleLifecycleInput::Delete { at_utc_ms: 100 },
        )
        .expect("first Delete from Active must succeed");
        assert_eq!(first.to_phase, ScheduleLifecycleState::Deleted);
        let revision_after_delete = auth.state.revision;
        let planning_after_delete = auth.state.planning_cursor_utc_ms;
        let ordinal_after_delete = auth.state.next_occurrence_ordinal;

        let second = ScheduleLifecycleMachineMutator::apply(
            &mut auth,
            ScheduleLifecycleInput::Delete { at_utc_ms: 200 },
        )
        .expect("Delete from Deleted must be idempotent, not a transition error");
        assert_eq!(second.from_phase, ScheduleLifecycleState::Deleted);
        assert_eq!(second.to_phase, ScheduleLifecycleState::Deleted);
        assert!(
            second.effects.is_empty(),
            "Delete from Deleted must emit zero effects, got {:?}",
            second.effects
        );
        assert_eq!(auth.state.revision, revision_after_delete);
        assert_eq!(auth.state.planning_cursor_utc_ms, planning_after_delete);
        assert_eq!(auth.state.next_occurrence_ordinal, ordinal_after_delete);
    }
}
