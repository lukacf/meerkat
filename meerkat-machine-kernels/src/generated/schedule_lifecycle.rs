mod source {
    #![allow(dead_code, clippy::expect_used, clippy::assign_op_pattern)]
    use meerkat_machine_dsl::machine;

    machine! {
        machine ScheduleLifecycleMachine {
            version: 1,
            rust: "meerkat-schedule" / "machines::schedule_lifecycle",

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

            // NB: no `RecordPlanningWindowPaused` — planning only advances while
            // the schedule is Active. Paused schedules MUST reject
            // `RecordPlanningWindow` as an invalid transition; this closes the
            // race where a driver tick could race with `Pause` and silently
            // advance the planning cursor against a paused schedule.

            // --- Pause / Resume (from Active or Paused) ---

            transition PauseActiveOrPaused {
                on input Pause { at_utc_ms }
                guard { self.lifecycle_phase == Phase::Active || self.lifecycle_phase == Phase::Paused }
                guard "pause_timestamp_present" { at_utc_ms == at_utc_ms }
                update {}
                to Paused
                emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            }

            transition ResumeActiveOrPaused {
                on input Resume { at_utc_ms }
                guard { self.lifecycle_phase == Phase::Active || self.lifecycle_phase == Phase::Paused }
                guard "resume_timestamp_present" { at_utc_ms == at_utc_ms }
                update {}
                to Active
                emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            }

            // --- Delete (per-phase, bumps revision) ---

            transition DeleteActive {
                on input Delete { at_utc_ms }
                guard { self.lifecycle_phase == Phase::Active }
                guard "delete_timestamp_present" { at_utc_ms == at_utc_ms }
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
                guard "delete_timestamp_present" { at_utc_ms == at_utc_ms }
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

    // DSL proxy types — must match the real policy enums 1:1

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MisfirePolicy {
        Skip,
        CatchUpWithin,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum OverlapPolicy {
        AllowConcurrent,
        SkipIfRunning,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MissingTargetPolicy {
        Skip,
        MarkMisfired,
    }
}
pub use source::*;

pub fn schema() -> meerkat_machine_schema::MachineSchema {
    meerkat_machine_schema::catalog::dsl::dsl_schedule_lifecycle_machine()
}
