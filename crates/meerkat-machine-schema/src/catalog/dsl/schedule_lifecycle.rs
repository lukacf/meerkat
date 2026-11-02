use super::OptionValueExt;
use meerkat_machine_dsl::machine;

machine! {
    machine ScheduleLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::schedule_lifecycle",

        state {
            schedule_id: ScheduleId,
            lifecycle_phase: ScheduleLifecycleState,
            revision: u64,
            trigger_key: TriggerKey,
            target_binding_key: TargetBindingId,
            misfire_policy: Enum<MisfirePolicy>,
            overlap_policy: Enum<OverlapPolicy>,
            missing_target_policy: Enum<MissingTargetPolicy>,
            planning_horizon_days: u64,
            planning_horizon_occurrences: u64,
            planning_cursor_utc_ms: Option<u64>,
            next_occurrence_ordinal: u64,
            // Reciprocal-ack accumulator (wave-d D-f): occurrence ids
            // whose supersession the occurrence authority has confirmed.
            // The schedule side observes the completion of the
            // SupersedePendingOccurrences route by counting acks rather
            // than assuming every in-flight Supersede landed.
            //
            // Bounded retention: the set holds only the acknowledgements of
            // the latest supersession, the one at `revision`. Revise* and
            // Delete* (the only arms that bump `revision` and emit
            // SupersedePendingOccurrences) drop every recorded id before the
            // new supersession's acknowledgements arrive, since each of them
            // answers an earlier revision; ConfirmOccurrencesSuperseded*
            // record an acknowledgement only when its `superseding_revision`
            // is `revision`. A set stored before this bound (any size, ids of
            // any earlier revision) recovers unchanged, since no invariant
            // reads it, and the next Revise*, DeleteActive or DeletePaused of
            // a live schedule drops it. A schedule deleted before the bound
            // keeps the set it was deleted with: no Revise* arm runs from
            // Deleted, and DeleteDeleted changes nothing.
            superseded_ack_ids: Set<OccurrenceId>,
        }

        init(Active) {
            schedule_id = "schedule-0",
            revision = 1,
            trigger_key = "trigger-0",
            target_binding_key = "target-0",
            misfire_policy = MisfirePolicy::Skip,
            overlap_policy = OverlapPolicy::SkipIfRunning,
            missing_target_policy = MissingTargetPolicy::MarkMisfired,
            planning_horizon_days = 30,
            planning_horizon_occurrences = 64,
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
                schedule_id: ScheduleId,
                trigger_key: TriggerKey,
                target_binding_key: TargetBindingId,
                misfire_policy: Enum<MisfirePolicy>,
                overlap_policy: Enum<OverlapPolicy>,
                missing_target_policy: Enum<MissingTargetPolicy>,
                planning_horizon_days: Option<u64>,
                planning_horizon_occurrences: Option<u64>,
            },
            Revise {
                trigger_key: TriggerKey,
                target_binding_key: TargetBindingId,
                misfire_policy: Enum<MisfirePolicy>,
                overlap_policy: Enum<OverlapPolicy>,
                missing_target_policy: Enum<MissingTargetPolicy>,
                planning_horizon_days: u64,
                planning_horizon_occurrences: u64,
                at_utc_ms: u64,
            },
            UpdatePlanningConfig { planning_horizon_days: u64, planning_horizon_occurrences: u64 },
            RecordPlanningWindow {
                planning_cursor_utc_ms: u64,
                next_occurrence_ordinal: u64,
            },
            SyncTargetSnapshot { target_binding_key: TargetBindingId },
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
            // 0.7.2 D1 (disciplined shell inputs): this effect's sweep covers
            // ALL outstanding (non-terminal) occurrences of the schedule —
            // Pending AND driver-claimed in-flight ones (Claimed /
            // Dispatching / AwaitingCompletion). A revision-affecting commit
            // (Revise*, Delete*) revokes in-flight claims by superseding them
            // through the occurrence authority's typed Supersede transition
            // at commit time, so delete never leaves the driver holding a
            // claim whose completion inputs get guard-rejected; late
            // resolutions land as typed late-arrival facts on the occurrence
            // machine. Each swept occurrence acks back via
            // OccurrencesSuperseded → ConfirmOccurrencesSuperseded into
            // `superseded_ack_ids`, which is the machine-owned account of the
            // claims this commit revoked. (Effect name predates the broadened
            // sweep — "Pending" reads historically; kept because the effect is
            // routed in compositions.rs and a rename buys no semantics.)
            SupersedePendingOccurrences { superseding_revision: u64, at_utc_ms: u64 },
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

        // A post-commit return/log projection, not a replayable external
        // effect. The committed Schedule row is the authority.
        disposition EmitScheduleNotice => local seam SurfaceResultAlignment,
        disposition SupersedePendingOccurrences => routed [OccurrenceLifecycleMachine] seam NoOwnerRealization,
        disposition PlanningWindowRecorded => local seam NoOwnerRealization,

        // --- Create (only from Active, self-loop) ---

        transition CreateSchedule {
            on input Create {
                schedule_id,
                trigger_key,
                target_binding_key,
                misfire_policy,
                overlap_policy,
                missing_target_policy,
                planning_horizon_days,
                planning_horizon_occurrences
            }
            guard { self.lifecycle_phase == Phase::Active }
            update {
                self.schedule_id = schedule_id;
                self.trigger_key = trigger_key;
                self.target_binding_key = target_binding_key;
                self.misfire_policy = misfire_policy;
                self.overlap_policy = overlap_policy;
                self.missing_target_policy = missing_target_policy;
                if planning_horizon_days != None {
                    self.planning_horizon_days = planning_horizon_days.get("value");
                }
                if planning_horizon_occurrences != None {
                    self.planning_horizon_occurrences = planning_horizon_occurrences.get("value");
                }
            }
            to Active
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
        }

        // --- Revise (per-phase, bumps revision) ---
        //
        // A revise supersedes at its new revision, so every acknowledgement
        // recorded so far answers an earlier one: the update drops them all
        // before this supersession's acknowledgements arrive.

        transition ReviseActive {
            on input Revise {
                trigger_key,
                target_binding_key,
                misfire_policy,
                overlap_policy,
                missing_target_policy,
                planning_horizon_days,
                planning_horizon_occurrences,
                at_utc_ms
            }
            guard { self.lifecycle_phase == Phase::Active }
            update {
                self.trigger_key = trigger_key;
                self.target_binding_key = target_binding_key;
                self.misfire_policy = misfire_policy;
                self.overlap_policy = overlap_policy;
                self.missing_target_policy = missing_target_policy;
                self.planning_horizon_days = planning_horizon_days;
                self.planning_horizon_occurrences = planning_horizon_occurrences;
                self.revision += 1;
                self.planning_cursor_utc_ms = None;
                self.superseded_ack_ids = EmptySet;
            }
            to Active
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            emit SupersedePendingOccurrences { superseding_revision: self.revision, at_utc_ms: at_utc_ms }
        }

        transition RevisePaused {
            on input Revise {
                trigger_key,
                target_binding_key,
                misfire_policy,
                overlap_policy,
                missing_target_policy,
                planning_horizon_days,
                planning_horizon_occurrences,
                at_utc_ms
            }
            guard { self.lifecycle_phase == Phase::Paused }
            update {
                self.trigger_key = trigger_key;
                self.target_binding_key = target_binding_key;
                self.misfire_policy = misfire_policy;
                self.overlap_policy = overlap_policy;
                self.missing_target_policy = missing_target_policy;
                self.planning_horizon_days = planning_horizon_days;
                self.planning_horizon_occurrences = planning_horizon_occurrences;
                self.revision += 1;
                self.planning_cursor_utc_ms = None;
                self.superseded_ack_ids = EmptySet;
            }
            to Paused
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            emit SupersedePendingOccurrences { superseding_revision: self.revision, at_utc_ms: at_utc_ms }
        }

        // --- Planning config update ---

        transition UpdatePlanningConfigActive {
            on input UpdatePlanningConfig { planning_horizon_days, planning_horizon_occurrences }
            guard { self.lifecycle_phase == Phase::Active }
            update {
                self.planning_horizon_days = planning_horizon_days;
                self.planning_horizon_occurrences = planning_horizon_occurrences;
            }
            to Active
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
        }

        transition UpdatePlanningConfigPaused {
            on input UpdatePlanningConfig { planning_horizon_days, planning_horizon_occurrences }
            guard { self.lifecycle_phase == Phase::Paused }
            update {
                self.planning_horizon_days = planning_horizon_days;
                self.planning_horizon_occurrences = planning_horizon_occurrences;
            }
            to Paused
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
        }

        // --- Record planning window (per-phase, with guard) ---

        transition RecordPlanningWindowActive {
            on input RecordPlanningWindow { planning_cursor_utc_ms, next_occurrence_ordinal }
            guard "planning_window_advances_ordinal" { self.lifecycle_phase == Phase::Active && next_occurrence_ordinal > 0 }
            // Machine-owned convergence invariant (ask 22): planning is
            // MONOTONE. A replan that does not advance the cursor is the
            // regeneration-runaway shape (re-planning an already-covered
            // due); the machine refuses it, so even a future planner or
            // representation bug converges as a visible per-tick refill
            // fault instead of generating occurrences unboundedly.
            guard "planning_cursor_advances" {
                self.planning_cursor_utc_ms == None
                || planning_cursor_utc_ms > self.planning_cursor_utc_ms.get("value")
            }
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

        // --- Target snapshot sync ---
        //
        // Materialized on-demand sessions update the schedule target binding
        // without revising schedule authoring intent. The generated authority
        // owns that target binding key before the shell persists the full
        // target snapshot.

        transition SyncTargetSnapshotActive {
            on input SyncTargetSnapshot { target_binding_key }
            guard { self.lifecycle_phase == Phase::Active }
            update {
                self.target_binding_key = target_binding_key;
            }
            to Active
        }

        transition SyncTargetSnapshotPaused {
            on input SyncTargetSnapshot { target_binding_key }
            guard { self.lifecycle_phase == Phase::Paused }
            update {
                self.target_binding_key = target_binding_key;
            }
            to Paused
        }

        // --- Pause / Resume (from Active or Paused) ---
        //
        // 0.7.2 D1: pause deliberately emits NO supersession. Pause is
        // resumable: pre-dispatch claims are frozen and released through the
        // existing machine-owned reconcile path (the occurrence authority's
        // ClaimedDispatchDisposition::Frozen verdict →
        // ReleaseLeaseForPausedSchedule typed transition), and in-flight
        // dispatched deliveries legitimately complete under a paused schedule
        // (the completion-supersession classification treats Paused as
        // Proceed). A late resolution of a claim that was released (and
        // possibly reclaimed) is screened by the store's claim-evidence check
        // and fed back as the occurrence authority's
        // ClassifyStaleCompletionArrival typed fact — never a silent drop,
        // never an ERROR.

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
        //
        // Delete supersedes like a revise: it drops every recorded
        // acknowledgement, all of them of earlier revisions, and records its
        // own supersession's acknowledgements, which arrive in Deleted.

        transition DeleteActive {
            on input Delete { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Active }
            update {
                self.revision += 1;
                self.planning_cursor_utc_ms = None;
                self.superseded_ack_ids = EmptySet;
            }
            to Deleted
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            emit SupersedePendingOccurrences { superseding_revision: self.revision, at_utc_ms: at_utc_ms }
        }

        transition DeletePaused {
            on input Delete { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Paused }
            update {
                self.revision += 1;
                self.planning_cursor_utc_ms = None;
                self.superseded_ack_ids = EmptySet;
            }
            to Deleted
            emit EmitScheduleNotice { new_state: self.lifecycle_phase, revision: self.revision }
            emit SupersedePendingOccurrences { superseding_revision: self.revision, at_utc_ms: at_utc_ms }
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
        //
        // Bounded retention: an ack is recorded only when it answers the
        // latest supersession (`superseding_revision == revision`). Any other
        // ack is accepted in every phase and changes nothing, so it keeps
        // the answer it always had (accepted, no effect) without growing the
        // set: a late ack of an earlier supersession, whose id a later Revise*
        // or Delete* dropped, and an ack of a revision the schedule never
        // reached. A repeated ack of the latest supersession re-inserts an id
        // already recorded.

        transition ConfirmOccurrencesSupersededActive {
            on input ConfirmOccurrencesSuperseded { occurrence_id, superseding_revision }
            guard { self.lifecycle_phase == Phase::Active }
            update {
                if superseding_revision == self.revision {
                    self.superseded_ack_ids.insert(occurrence_id);
                }
            }
            to Active
        }

        transition ConfirmOccurrencesSupersededPaused {
            on input ConfirmOccurrencesSuperseded { occurrence_id, superseding_revision }
            guard { self.lifecycle_phase == Phase::Paused }
            update {
                if superseding_revision == self.revision {
                    self.superseded_ack_ids.insert(occurrence_id);
                }
            }
            to Paused
        }

        transition ConfirmOccurrencesSupersededDeleted {
            on input ConfirmOccurrencesSuperseded { occurrence_id, superseding_revision }
            guard { self.lifecycle_phase == Phase::Deleted }
            update {
                if superseding_revision == self.revision {
                    self.superseded_ack_ids.insert(occurrence_id);
                }
            }
            to Deleted
        }
    }
}

// Stub types
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
    MarkMisfired,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleId(pub String);
impl<T: Into<String>> From<T> for ScheduleId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

// Opaque trigger handle. Replaces the prior reuse of `TargetBindingId` for
// the `trigger_key` field: trigger identity and target-binding identity are
// DIFFERENT semantic facts, and sharing one newtype across them re-creates
// the index-confusion the newtypes exist to prevent. The typed atom
// (`NamedTypeBinding::string("TriggerKey")`) is registered in `dsl/mod.rs`
// for both the schedule and occurrence machines.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct TriggerKey(pub String);
impl<T: Into<String>> From<T> for TriggerKey {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

// Opaque target-binding handle. Replaces the prior raw `String`
// `target_binding_key` schedule state field so the schedule authority owns a
// typed identifier rather than ferrying a bare string. The typed atom
// (`NamedTypeBinding::string("TargetBindingId")`) registered in `dsl/mod.rs`
// keeps schema emission consistent across the schedule and occurrence
// machines that share this binding key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct TargetBindingId(pub String);
impl<T: Into<String>> From<T> for TargetBindingId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

// Opaque claim-owner handle. The occurrence authority's `claimed_by` field is
// retyped from raw `Option<String>` to `Option<ClaimOwner>`; the newtype is
// declared here so both the schedule and occurrence DSL modules can reference
// the shared binding atom (`NamedTypeBinding::string("ClaimOwner")`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ClaimOwner(pub String);
impl<T: Into<String>> From<T> for ClaimOwner {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

// Opaque delivery/receipt correlation handle. The occurrence authority's
// `delivery_correlation_id`/`last_receipt_correlation_id`/`runtime_outcome_key`
// fields are retyped from raw `Option<String>` to `Option<CorrelationId>`; the
// newtype is declared here so both DSL modules share the binding atom
// (`NamedTypeBinding::string("CorrelationId")`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct CorrelationId(pub String);
impl<T: Into<String>> From<T> for CorrelationId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
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
        let revision_after_delete = auth.state().revision;
        let planning_after_delete = auth.state().planning_cursor_utc_ms;
        let ordinal_after_delete = auth.state().next_occurrence_ordinal;

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
        assert_eq!(auth.state().revision, revision_after_delete);
        assert_eq!(auth.state().planning_cursor_utc_ms, planning_after_delete);
        assert_eq!(auth.state().next_occurrence_ordinal, ordinal_after_delete);
    }

    fn revise(auth: &mut ScheduleLifecycleMachineAuthority, at_utc_ms: u64) {
        ScheduleLifecycleMachineMutator::apply(
            auth,
            ScheduleLifecycleInput::Revise {
                trigger_key: "trigger-0".into(),
                target_binding_key: "target-0".into(),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                planning_horizon_days: 30,
                planning_horizon_occurrences: 64,
                at_utc_ms,
            },
        )
        .expect("Revise from Active or Paused must succeed");
    }

    fn confirm(
        auth: &mut ScheduleLifecycleMachineAuthority,
        occurrence_id: &str,
        superseding_revision: u64,
    ) -> ScheduleLifecycleMachineTransition {
        ScheduleLifecycleMachineMutator::apply(
            auth,
            ScheduleLifecycleInput::ConfirmOccurrencesSuperseded {
                occurrence_id: occurrence_id.into(),
                superseding_revision,
            },
        )
        .expect("ConfirmOccurrencesSuperseded is accepted in every phase")
    }

    fn ack_ids(auth: &ScheduleLifecycleMachineAuthority) -> std::collections::BTreeSet<String> {
        auth.state()
            .superseded_ack_ids
            .iter()
            .map(|id| id.0.clone())
            .collect()
    }

    fn ids(values: &[&str]) -> std::collections::BTreeSet<String> {
        values.iter().map(ToString::to_string).collect()
    }

    /// Each revise supersedes at its new revision, and its acknowledgements
    /// replace the earlier supersessions': after any number of revises, from
    /// Active or Paused, the set holds only the latest supersession's ids.
    #[test]
    fn revise_keeps_only_the_latest_supersessions_acks() {
        let mut auth = ScheduleLifecycleMachineAuthority::new();
        for round in 1..=6_u64 {
            if round == 4 {
                ScheduleLifecycleMachineMutator::apply(
                    &mut auth,
                    ScheduleLifecycleInput::Pause { at_utc_ms: 0 },
                )
                .expect("Pause from Active must succeed");
            }
            revise(&mut auth, round);
            assert!(
                auth.state().superseded_ack_ids.is_empty(),
                "revise {round} must drop every acknowledgement of an earlier revision, found {:?}",
                ack_ids(&auth)
            );
            let revision = auth.state().revision;
            let occurrence = format!("occurrence-{round}");
            confirm(&mut auth, &occurrence, revision);
            assert_eq!(
                ack_ids(&auth),
                ids(&[occurrence.as_str()]),
                "after revise {round} the set holds the latest supersession's acknowledgement only"
            );
        }
        assert_eq!(auth.state().lifecycle_phase, ScheduleLifecycleState::Paused);
    }

    /// An acknowledgement that does not answer the latest supersession (a late
    /// one of an earlier revision, whose id a later revise dropped, or one for
    /// a revision the schedule never reached) is accepted and changes nothing;
    /// a repeated acknowledgement of the latest supersession keeps its id.
    #[test]
    fn acks_that_do_not_answer_the_latest_supersession_are_idempotent_noops() {
        let mut auth = ScheduleLifecycleMachineAuthority::new();
        revise(&mut auth, 1);
        confirm(&mut auth, "occurrence-a", 2);
        revise(&mut auth, 2);
        confirm(&mut auth, "occurrence-b", 3);
        revise(&mut auth, 3);
        confirm(&mut auth, "occurrence-c", 4);
        assert_eq!(auth.state().revision, 4);
        assert_eq!(ack_ids(&auth), ids(&["occurrence-c"]));
        let latest = auth.state().clone();

        for (occurrence, superseding_revision) in [
            // Two revisions back, its id dropped by the later revises.
            ("occurrence-a", 2),
            // One revision back.
            ("occurrence-b", 3),
            // Never recorded, from an earlier revision.
            ("occurrence-z", 1),
            // A revision the schedule never reached.
            ("occurrence-y", 5),
            // The latest supersession's own acknowledgement again.
            ("occurrence-c", 4),
        ] {
            let late = confirm(&mut auth, occurrence, superseding_revision);
            assert_eq!(late.from_phase, ScheduleLifecycleState::Active);
            assert_eq!(late.to_phase, ScheduleLifecycleState::Active);
            assert!(
                late.effects().is_empty(),
                "an acknowledgement emits nothing, got {:?}",
                late.effects()
            );
            assert_eq!(
                auth.state(),
                &latest,
                "acknowledgement of {occurrence} at revision {superseding_revision} must leave \
                 the latest supersession's set as it was"
            );
        }
    }

    /// Delete supersedes like a revise: it drops the earlier supersessions'
    /// ids and records its own acknowledgements, which a repeated delete keeps.
    #[test]
    fn delete_keeps_only_its_own_supersessions_acks() {
        for pause_first in [false, true] {
            let mut auth = ScheduleLifecycleMachineAuthority::new();
            revise(&mut auth, 1);
            confirm(&mut auth, "occurrence-a", 2);
            if pause_first {
                ScheduleLifecycleMachineMutator::apply(
                    &mut auth,
                    ScheduleLifecycleInput::Pause { at_utc_ms: 2 },
                )
                .expect("Pause from Active must succeed");
            }
            ScheduleLifecycleMachineMutator::apply(
                &mut auth,
                ScheduleLifecycleInput::Delete { at_utc_ms: 3 },
            )
            .expect("Delete from Active or Paused must succeed");
            assert_eq!(
                auth.state().lifecycle_phase,
                ScheduleLifecycleState::Deleted
            );
            assert!(
                auth.state().superseded_ack_ids.is_empty(),
                "delete (paused first: {pause_first}) must drop the revise's acknowledgements, found {:?}",
                ack_ids(&auth)
            );
            confirm(&mut auth, "occurrence-b", 3);
            confirm(&mut auth, "occurrence-a", 2);
            assert_eq!(ack_ids(&auth), ids(&["occurrence-b"]));

            let repeated = ScheduleLifecycleMachineMutator::apply(
                &mut auth,
                ScheduleLifecycleInput::Delete { at_utc_ms: 4 },
            )
            .expect("Delete from Deleted is the idempotent no-op");
            assert!(repeated.effects().is_empty());
            assert_eq!(
                ack_ids(&auth),
                ids(&["occurrence-b"]),
                "a repeated delete keeps the delete's own acknowledgements"
            );
        }
    }

    /// A stored state from before the bound (any number of acknowledgements of
    /// earlier revisions) recovers unchanged, and its next revise drops them.
    #[test]
    fn a_stored_large_ack_set_recovers_and_the_next_revise_drops_it() {
        let mut stored = ScheduleLifecycleMachineAuthority::new().state().clone();
        stored.revision = 41;
        stored.superseded_ack_ids = (0..512)
            .map(|n| OccurrenceId(format!("occurrence-{n}")))
            .collect();

        let mut auth = ScheduleLifecycleMachineAuthority::recover_from_state(stored.clone())
            .expect("a stored set of any size recovers: no invariant reads its size");
        assert_eq!(auth.state(), &stored);

        revise(&mut auth, 1);
        assert_eq!(auth.state().revision, 42);
        assert!(
            auth.state().superseded_ack_ids.is_empty(),
            "the next revise must drop the stored acknowledgements, found {}",
            auth.state().superseded_ack_ids.len()
        );

        let mut auth = ScheduleLifecycleMachineAuthority::recover_from_state(stored)
            .expect("a stored set of any size recovers");
        ScheduleLifecycleMachineMutator::apply(
            &mut auth,
            ScheduleLifecycleInput::Delete { at_utc_ms: 1 },
        )
        .expect("Delete from Active must succeed");
        assert!(
            auth.state().superseded_ack_ids.is_empty(),
            "the next delete must drop the stored acknowledgements, found {}",
            auth.state().superseded_ack_ids.len()
        );
    }
}
