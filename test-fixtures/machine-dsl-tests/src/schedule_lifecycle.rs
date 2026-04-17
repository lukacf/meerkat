use meerkat_machine_dsl::machine;

machine! {
    machine ScheduleLifecycleMachine {
        version: 1,
        rust: "meerkat-schedule" / "generated::schedule_lifecycle",

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

        transition RecordPlanningWindowPaused {
            on input RecordPlanningWindow { planning_cursor_utc_ms, next_occurrence_ordinal }
            guard "planning_window_advances_ordinal" { self.lifecycle_phase == Phase::Paused && next_occurrence_ordinal > 0 }
            update {
                self.planning_cursor_utc_ms = Some(planning_cursor_utc_ms);
                self.next_occurrence_ordinal = next_occurrence_ordinal;
            }
            to Paused
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            emit PlanningWindowRecorded { planning_cursor_utc_ms: planning_cursor_utc_ms, next_occurrence_ordinal: next_occurrence_ordinal }
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_is_active() {
        let auth = ScheduleLifecycleMachineAuthority::new();
        assert_eq!(auth.state.phase(), ScheduleLifecycleState::Active);
        assert_eq!(auth.state.revision, 1);
        assert_eq!(auth.state.misfire_policy, MisfirePolicy::Skip);
    }

    #[test]
    fn revise_bumps_revision() {
        let mut auth = ScheduleLifecycleMachineAuthority::new();
        ScheduleLifecycleMachineMutator::apply(
            &mut auth,
            ScheduleLifecycleInput::Revise {
                trigger_key: "t2".into(),
                target_binding_key: "tb2".into(),
                misfire_policy: MisfirePolicy::Execute,
                overlap_policy: OverlapPolicy::Queue,
                missing_target_policy: MissingTargetPolicy::Skip,
            },
        )
        .unwrap();
        assert_eq!(auth.state.revision, 2);
        assert_eq!(auth.state.misfire_policy, MisfirePolicy::Execute);
        assert_eq!(auth.state.planning_cursor_utc_ms, None); // cleared on revise
    }

    #[test]
    fn pause_and_resume() {
        let mut auth = ScheduleLifecycleMachineAuthority::new();
        let r = ScheduleLifecycleMachineMutator::apply(
            &mut auth,
            ScheduleLifecycleInput::Pause { at_utc_ms: 100 },
        )
        .unwrap();
        assert_eq!(r.to_phase, ScheduleLifecycleState::Paused);

        let r = ScheduleLifecycleMachineMutator::apply(
            &mut auth,
            ScheduleLifecycleInput::Resume { at_utc_ms: 200 },
        )
        .unwrap();
        assert_eq!(r.to_phase, ScheduleLifecycleState::Active);
    }

    #[test]
    fn delete_is_terminal() {
        let mut auth = ScheduleLifecycleMachineAuthority::new();
        let r = ScheduleLifecycleMachineMutator::apply(
            &mut auth,
            ScheduleLifecycleInput::Delete { at_utc_ms: 300 },
        )
        .unwrap();
        assert_eq!(r.to_phase, ScheduleLifecycleState::Deleted);
        assert_eq!(auth.state.revision, 2); // bumped on delete

        // Cannot act after delete
        let r = ScheduleLifecycleMachineMutator::apply(
            &mut auth,
            ScheduleLifecycleInput::Pause { at_utc_ms: 400 },
        );
        assert!(r.is_err());
    }

    #[test]
    fn planning_window_requires_positive_ordinal() {
        let mut auth = ScheduleLifecycleMachineAuthority::new();
        // ordinal > 0 should succeed
        let r = ScheduleLifecycleMachineMutator::apply(
            &mut auth,
            ScheduleLifecycleInput::RecordPlanningWindow {
                planning_cursor_utc_ms: 500,
                next_occurrence_ordinal: 1,
            },
        );
        assert!(r.is_ok());
        assert_eq!(auth.state.planning_cursor_utc_ms, Some(500));

        // ordinal == 0 should fail (guard)
        let r = ScheduleLifecycleMachineMutator::apply(
            &mut auth,
            ScheduleLifecycleInput::RecordPlanningWindow {
                planning_cursor_utc_ms: 600,
                next_occurrence_ordinal: 0,
            },
        );
        assert!(r.is_err());
    }

    // ---- Schema tests ----

    #[test]
    fn schema_validates() {
        let schema = ScheduleLifecycleMachineState::schema();
        schema
            .validate()
            .expect("schedule lifecycle schema should validate");
    }

    #[test]
    fn schema_renders_tla() {
        let schema = ScheduleLifecycleMachineState::schema();
        let tla = meerkat_machine_codegen::render_machine_module(&schema);
        assert!(tla.contains("ScheduleLifecycleMachine"));
        assert!(tla.contains("CreateSchedule"));
        assert!(tla.contains("DeleteActive"));
    }
}
