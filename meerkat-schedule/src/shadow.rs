//! Shadow dispatch: DSL-generated `apply()` runs in parallel with the
//! hand-written authority, comparing phase outcomes and effect counts.
//!
//! This module is `#[cfg(test)]` only — it adds zero runtime overhead.
//! Every existing test that calls `OccurrenceLifecycleAuthority::apply()`
//! or `ScheduleLifecycleAuthority::apply()` automatically exercises the
//! shadow comparison.
#![allow(
    dead_code,
    unused_variables,
    clippy::cmp_owned,
    clippy::assign_op_pattern,
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used
)]

// ============================================================
// Occurrence lifecycle shadow
// ============================================================

pub(crate) mod occurrence {
    use meerkat_machine_dsl::machine;

    // --- Bridging newtypes (satisfy DSL type constraints) ---

    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct OccurrenceId(pub String);
    impl<T: Into<String>> From<T> for OccurrenceId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct ScheduleId(pub String);
    impl<T: Into<String>> From<T> for ScheduleId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct ClaimToken(pub String);
    impl<T: Into<String>> From<T> for ClaimToken {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct DeliveryReceipt(pub String);
    impl<T: Into<String>> From<T> for DeliveryReceipt {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum OccurrenceFailureClass {
        Timeout,
        TargetUnavailable,
        InternalError,
    }

    // --- DSL machine definition (identical to catalog/dsl/occurrence_lifecycle.rs) ---

    machine! {
        machine OccurrenceLifecycleMachine {
            version: 1,
            rust: "meerkat-schedule" / "shadow::occurrence",

            state {
                lifecycle_phase: OccurrenceLifecycleState,
                occurrence_id: OccurrenceId,
                schedule_id: ScheduleId,
                schedule_revision: u64,
                occurrence_ordinal: u64,
                target_binding_key: String,
                due_at_utc_ms: u64,
                claimed_by: Option<String>,
                lease_expires_at_utc_ms: Option<u64>,
                claimed_at_utc_ms: Option<u64>,
                claim_token: Option<ClaimToken>,
                delivery_correlation_id: Option<String>,
                last_receipt: Option<DeliveryReceipt>,
                failure_class: Option<Enum<OccurrenceFailureClass>>,
                failure_detail: Option<String>,
                dispatched_at_utc_ms: Option<u64>,
                completed_at_utc_ms: Option<u64>,
                attempt_count: u64,
                superseded_by_revision: Option<u64>,
            }

            init(Pending) {
                occurrence_id = "occurrence-0",
                schedule_id = "schedule-0",
                schedule_revision = 1,
                occurrence_ordinal = 0,
                target_binding_key = "target-0",
                due_at_utc_ms = 1,
                claimed_by = None,
                lease_expires_at_utc_ms = None,
                claimed_at_utc_ms = None,
                claim_token = None,
                delivery_correlation_id = None,
                last_receipt = None,
                failure_class = None,
                failure_detail = None,
                dispatched_at_utc_ms = None,
                completed_at_utc_ms = None,
                attempt_count = 0,
                superseded_by_revision = None,
            }

            terminal [Completed, Skipped, Misfired, Superseded, DeliveryFailed]

            phase OccurrenceLifecycleState {
                Pending,
                Claimed,
                Dispatching,
                AwaitingCompletion,
                Completed,
                Skipped,
                Misfired,
                Superseded,
                DeliveryFailed,
            }

            input OccurrenceLifecycleInput {
                Claim { owner_id: String, at_utc_ms: u64, lease_expires_at_utc_ms: u64, claim_token: ClaimToken },
                DispatchStarted { correlation_id: Option<String>, at_utc_ms: u64 },
                AwaitCompletion { at_utc_ms: u64 },
                Complete { receipt: DeliveryReceipt, at_utc_ms: u64 },
                Skip { detail: Option<String>, failure_class: Option<Enum<OccurrenceFailureClass>>, at_utc_ms: u64 },
                Misfire { detail: Option<String>, failure_class: Option<Enum<OccurrenceFailureClass>>, at_utc_ms: u64 },
                Supersede { superseded_by_revision: u64, at_utc_ms: u64 },
                DeliveryFailed { receipt: Option<DeliveryReceipt>, failure_class: Enum<OccurrenceFailureClass>, detail: Option<String>, at_utc_ms: u64 },
                LeaseExpired { at_utc_ms: u64 },
            }

            effect OccurrenceLifecycleEffect {
                Claimed,
                DispatchStarted,
                AwaitingCompletion,
                Completed,
                Skipped,
                Misfired,
                Superseded,
                DeliveryFailed,
                LeaseExpired,
            }

            helper is_live_claim_phase(phase: OccurrenceLifecycleState) -> bool {
                phase == Phase::Claimed || phase == Phase::Dispatching || phase == Phase::AwaitingCompletion
            }

            invariant live_claim_requires_owner {
                !is_live_claim_phase(self.lifecycle_phase) || self.claimed_by != None
            }

            invariant superseded_records_revision {
                self.lifecycle_phase != Phase::Superseded || self.superseded_by_revision != None
            }

            invariant delivery_failed_records_failure_class {
                self.lifecycle_phase != Phase::DeliveryFailed || self.failure_class != None
            }

            disposition Claimed => external,
            disposition DispatchStarted => external,
            disposition AwaitingCompletion => external,
            disposition Completed => external,
            disposition Skipped => external,
            disposition Misfired => external,
            disposition Superseded => external,
            disposition DeliveryFailed => external,
            disposition LeaseExpired => external,

            transition ClaimPending {
                on input Claim { owner_id, at_utc_ms, lease_expires_at_utc_ms, claim_token }
                guard { self.lifecycle_phase == Phase::Pending }
                update {
                    self.claimed_by = Some(owner_id);
                    self.lease_expires_at_utc_ms = Some(lease_expires_at_utc_ms);
                    self.claimed_at_utc_ms = Some(at_utc_ms);
                    self.claim_token = Some(claim_token);
                    self.delivery_correlation_id = None;
                    self.last_receipt = None;
                    self.failure_class = None;
                    self.failure_detail = None;
                    self.dispatched_at_utc_ms = None;
                    self.completed_at_utc_ms = None;
                    self.attempt_count += 1;
                }
                to Claimed
                emit Claimed
            }

            transition DispatchStartedFromClaimed {
                on input DispatchStarted { correlation_id, at_utc_ms }
                guard { self.lifecycle_phase == Phase::Claimed }
                update {
                    self.delivery_correlation_id = correlation_id;
                    self.dispatched_at_utc_ms = Some(at_utc_ms);
                }
                to Dispatching
                emit DispatchStarted
            }

            transition AwaitCompletionFromDispatching {
                on input AwaitCompletion { at_utc_ms }
                guard { self.lifecycle_phase == Phase::Dispatching }
                update {
                    self.dispatched_at_utc_ms = Some(at_utc_ms);
                }
                to AwaitingCompletion
                emit AwaitingCompletion
            }

            transition CompleteFromDispatchingOrAwaiting {
                on input Complete { receipt, at_utc_ms }
                guard { self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
                update {
                    self.last_receipt = Some(receipt);
                    self.completed_at_utc_ms = Some(at_utc_ms);
                }
                to Completed
                emit Completed
            }

            transition SkipFromPendingOrLive {
                on input Skip { detail, failure_class, at_utc_ms }
                guard {
                    self.lifecycle_phase == Phase::Pending
                    || self.lifecycle_phase == Phase::Claimed
                    || self.lifecycle_phase == Phase::Dispatching
                    || self.lifecycle_phase == Phase::AwaitingCompletion
                }
                update {
                    self.failure_detail = detail;
                    self.failure_class = failure_class;
                    self.completed_at_utc_ms = Some(at_utc_ms);
                    self.claimed_by = None;
                    self.lease_expires_at_utc_ms = None;
                    self.claim_token = None;
                    self.delivery_correlation_id = None;
                }
                to Skipped
                emit Skipped
            }

            transition MisfireFromPendingOrLive {
                on input Misfire { detail, failure_class, at_utc_ms }
                guard {
                    self.lifecycle_phase == Phase::Pending
                    || self.lifecycle_phase == Phase::Claimed
                    || self.lifecycle_phase == Phase::Dispatching
                    || self.lifecycle_phase == Phase::AwaitingCompletion
                }
                update {
                    self.failure_detail = detail;
                    self.failure_class = failure_class;
                    self.completed_at_utc_ms = Some(at_utc_ms);
                    self.claimed_by = None;
                    self.lease_expires_at_utc_ms = None;
                    self.claim_token = None;
                    self.delivery_correlation_id = None;
                }
                to Misfired
                emit Misfired
            }

            transition SupersedePendingOrLive {
                on input Supersede { superseded_by_revision, at_utc_ms }
                guard {
                    self.lifecycle_phase == Phase::Pending
                    || self.lifecycle_phase == Phase::Claimed
                    || self.lifecycle_phase == Phase::Dispatching
                    || self.lifecycle_phase == Phase::AwaitingCompletion
                }
                update {
                    self.superseded_by_revision = Some(superseded_by_revision);
                    self.completed_at_utc_ms = Some(at_utc_ms);
                }
                to Superseded
                emit Superseded
            }

            transition DeliveryFailedFromClaimedOrLive {
                on input DeliveryFailed { receipt, failure_class, detail, at_utc_ms }
                guard {
                    self.lifecycle_phase == Phase::Claimed
                    || self.lifecycle_phase == Phase::Dispatching
                    || self.lifecycle_phase == Phase::AwaitingCompletion
                }
                update {
                    self.last_receipt = receipt;
                    self.failure_class = Some(failure_class);
                    self.failure_detail = detail;
                    self.completed_at_utc_ms = Some(at_utc_ms);
                }
                to DeliveryFailed
                emit DeliveryFailed
            }

            transition LeaseExpiredFromClaimed {
                on input LeaseExpired { at_utc_ms }
                guard { self.lifecycle_phase == Phase::Claimed }
                update {
                    self.claimed_by = None;
                    self.lease_expires_at_utc_ms = None;
                    self.claim_token = None;
                    self.delivery_correlation_id = None;
                    self.claimed_at_utc_ms = None;
                    self.dispatched_at_utc_ms = None;
                }
                to Pending
                emit LeaseExpired
            }

            transition LeaseExpiredFromDispatching {
                on input LeaseExpired { at_utc_ms }
                guard { self.lifecycle_phase == Phase::Dispatching }
                update {
                    self.claimed_by = None;
                    self.lease_expires_at_utc_ms = None;
                    self.claim_token = None;
                    self.delivery_correlation_id = None;
                    self.claimed_at_utc_ms = None;
                    self.dispatched_at_utc_ms = None;
                }
                to Pending
                emit LeaseExpired
            }

            transition LeaseExpiredFromAwaitingCompletion {
                on input LeaseExpired { at_utc_ms }
                guard { self.lifecycle_phase == Phase::AwaitingCompletion }
                update {
                    self.claimed_by = None;
                    self.lease_expires_at_utc_ms = None;
                    self.claim_token = None;
                    self.delivery_correlation_id = None;
                    self.claimed_at_utc_ms = None;
                    self.dispatched_at_utc_ms = None;
                }
                to Pending
                emit LeaseExpired
            }
        }
    }

    // --- Projection: Occurrence → DSL state ---

    fn dt_to_ms(dt: chrono::DateTime<chrono::Utc>) -> u64 {
        dt.timestamp_millis() as u64
    }

    fn map_failure_class(fc: &crate::types::OccurrenceFailureClass) -> OccurrenceFailureClass {
        use crate::types::OccurrenceFailureClass as Real;
        match fc {
            Real::TargetMaterializationFailed | Real::TargetMissing | Real::TargetBusy => {
                OccurrenceFailureClass::TargetUnavailable
            }
            Real::RuntimeRejected
            | Real::MobRejected
            | Real::LeaseLost
            | Real::TransportError
            | Real::InternalError => OccurrenceFailureClass::InternalError,
        }
    }

    fn project_occurrence(occ: &crate::types::Occurrence) -> OccurrenceLifecycleMachineState {
        use crate::types::OccurrencePhase as P;
        let phase = match occ.phase {
            P::Pending => OccurrenceLifecycleState::Pending,
            P::Claimed => OccurrenceLifecycleState::Claimed,
            P::Dispatching => OccurrenceLifecycleState::Dispatching,
            P::AwaitingCompletion => OccurrenceLifecycleState::AwaitingCompletion,
            P::Completed => OccurrenceLifecycleState::Completed,
            P::Skipped => OccurrenceLifecycleState::Skipped,
            P::Misfired => OccurrenceLifecycleState::Misfired,
            P::Superseded => OccurrenceLifecycleState::Superseded,
            P::DeliveryFailed => OccurrenceLifecycleState::DeliveryFailed,
        };

        OccurrenceLifecycleMachineState {
            lifecycle_phase: phase,
            occurrence_id: OccurrenceId(occ.occurrence_id.0.to_string()),
            schedule_id: ScheduleId(occ.schedule_id.0.to_string()),
            schedule_revision: occ.schedule_revision.0,
            occurrence_ordinal: occ.occurrence_ordinal.0,
            target_binding_key: occ.context.target_snapshot.stable_key(),
            due_at_utc_ms: dt_to_ms(occ.due_at_utc),
            claimed_by: occ.claimed_by.clone(),
            lease_expires_at_utc_ms: occ.lease_expires_at_utc.map(dt_to_ms),
            claimed_at_utc_ms: occ.claimed_at_utc.map(dt_to_ms),
            claim_token: occ.claim_token.map(|u| ClaimToken(u.to_string())),
            delivery_correlation_id: occ.delivery_correlation_id.clone(),
            last_receipt: occ
                .last_receipt
                .as_ref()
                .map(|r| DeliveryReceipt(format!("{:?}", r.stage))),
            failure_class: occ.failure_class.as_ref().map(map_failure_class),
            failure_detail: occ.failure_detail.clone(),
            dispatched_at_utc_ms: occ.dispatched_at_utc.map(dt_to_ms),
            completed_at_utc_ms: occ.completed_at_utc.map(dt_to_ms),
            attempt_count: u64::from(occ.attempt_count),
            superseded_by_revision: occ.superseded_by_revision.map(|r| r.0),
        }
    }

    // --- Input conversion ---

    fn convert_input(
        input: &crate::authority::OccurrenceLifecycleInput,
    ) -> OccurrenceLifecycleInput {
        use crate::authority::OccurrenceLifecycleInput as Real;
        match input {
            Real::Claim {
                owner_id,
                at_utc,
                lease_expires_at_utc,
                claim_token,
            } => OccurrenceLifecycleInput::Claim {
                owner_id: owner_id.clone(),
                at_utc_ms: dt_to_ms(*at_utc),
                lease_expires_at_utc_ms: dt_to_ms(*lease_expires_at_utc),
                claim_token: ClaimToken(claim_token.to_string()),
            },
            Real::DispatchStarted {
                correlation_id,
                at_utc,
            } => OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: correlation_id.clone(),
                at_utc_ms: dt_to_ms(*at_utc),
            },
            Real::AwaitCompletion { at_utc } => OccurrenceLifecycleInput::AwaitCompletion {
                at_utc_ms: dt_to_ms(*at_utc),
            },
            Real::Complete { receipt, at_utc } => OccurrenceLifecycleInput::Complete {
                receipt: DeliveryReceipt(format!("{:?}", receipt.stage)),
                at_utc_ms: dt_to_ms(*at_utc),
            },
            Real::Skip {
                detail,
                failure_class,
                at_utc,
            } => OccurrenceLifecycleInput::Skip {
                detail: detail.clone(),
                failure_class: failure_class.as_ref().map(map_failure_class),
                at_utc_ms: dt_to_ms(*at_utc),
            },
            Real::Misfire {
                detail,
                failure_class,
                at_utc,
            } => OccurrenceLifecycleInput::Misfire {
                detail: detail.clone(),
                failure_class: failure_class.as_ref().map(map_failure_class),
                at_utc_ms: dt_to_ms(*at_utc),
            },
            Real::Supersede {
                superseded_by_revision,
                at_utc,
            } => OccurrenceLifecycleInput::Supersede {
                superseded_by_revision: superseded_by_revision.0,
                at_utc_ms: dt_to_ms(*at_utc),
            },
            Real::DeliveryFailed {
                receipt,
                failure_class,
                detail,
                at_utc,
            } => OccurrenceLifecycleInput::DeliveryFailed {
                receipt: receipt
                    .as_ref()
                    .map(|r| DeliveryReceipt(format!("{:?}", r.stage))),
                failure_class: map_failure_class(failure_class),
                detail: detail.clone(),
                at_utc_ms: dt_to_ms(*at_utc),
            },
            Real::LeaseExpired { at_utc } => OccurrenceLifecycleInput::LeaseExpired {
                at_utc_ms: dt_to_ms(*at_utc),
            },
        }
    }

    // --- Shadow comparison ---

    fn phase_name(phase: &OccurrenceLifecycleState) -> &'static str {
        match phase {
            OccurrenceLifecycleState::Pending => "Pending",
            OccurrenceLifecycleState::Claimed => "Claimed",
            OccurrenceLifecycleState::Dispatching => "Dispatching",
            OccurrenceLifecycleState::AwaitingCompletion => "AwaitingCompletion",
            OccurrenceLifecycleState::Completed => "Completed",
            OccurrenceLifecycleState::Skipped => "Skipped",
            OccurrenceLifecycleState::Misfired => "Misfired",
            OccurrenceLifecycleState::Superseded => "Superseded",
            OccurrenceLifecycleState::DeliveryFailed => "DeliveryFailed",
        }
    }

    fn real_phase_name(phase: crate::types::OccurrencePhase) -> &'static str {
        use crate::types::OccurrencePhase as P;
        match phase {
            P::Pending => "Pending",
            P::Claimed => "Claimed",
            P::Dispatching => "Dispatching",
            P::AwaitingCompletion => "AwaitingCompletion",
            P::Completed => "Completed",
            P::Skipped => "Skipped",
            P::Misfired => "Misfired",
            P::Superseded => "Superseded",
            P::DeliveryFailed => "DeliveryFailed",
        }
    }

    pub(crate) fn shadow_assert(
        pre_occurrence: &crate::types::Occurrence,
        input: &crate::authority::OccurrenceLifecycleInput,
        result: &Result<
            crate::authority::OccurrenceLifecycleMutator,
            crate::authority::OccurrenceLifecycleError,
        >,
    ) {
        let dsl_state = project_occurrence(pre_occurrence);
        let dsl_input = convert_input(input);

        let mut authority = OccurrenceLifecycleMachineAuthority::from_state(dsl_state);
        let dsl_result = OccurrenceLifecycleMachineMutator::apply(&mut authority, dsl_input);

        match (result, &dsl_result) {
            (Ok(mutator), Ok(transition)) => {
                let authority_phase = real_phase_name(mutator.occurrence.phase);
                let dsl_phase = phase_name(&authority.state.lifecycle_phase);
                assert_eq!(
                    authority_phase, dsl_phase,
                    "shadow occurrence: phase mismatch: authority={authority_phase}, dsl={dsl_phase}",
                );

                assert_eq!(
                    mutator.effects.len(),
                    transition.effects.len(),
                    "shadow occurrence: effect count mismatch: authority={}, dsl={}",
                    mutator.effects.len(),
                    transition.effects.len(),
                );
            }
            (Err(_), Err(_)) => {
                // Both rejected — consistent
            }
            (Ok(mutator), Err(e)) => {
                panic!(
                    "shadow occurrence: authority accepted (phase={}) but DSL rejected: {e:?}",
                    real_phase_name(mutator.occurrence.phase),
                );
            }
            (Err(e), Ok(_transition)) => {
                let dsl_phase = phase_name(&authority.state.lifecycle_phase);
                panic!(
                    "shadow occurrence: authority rejected ({e:?}) but DSL accepted (phase={dsl_phase})",
                );
            }
        }
    }
}

// ============================================================
// Schedule lifecycle shadow
// ============================================================

pub(crate) mod schedule {
    use meerkat_machine_dsl::machine;

    // --- Bridging newtypes ---

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MisfirePolicy {
        Skip,
        Execute,
    }
    impl Default for MisfirePolicy {
        fn default() -> Self {
            Self::Skip
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum OverlapPolicy {
        SkipIfRunning,
        Queue,
    }
    impl Default for OverlapPolicy {
        fn default() -> Self {
            Self::SkipIfRunning
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MissingTargetPolicy {
        MarkMisfired,
        Skip,
    }
    impl Default for MissingTargetPolicy {
        fn default() -> Self {
            Self::MarkMisfired
        }
    }

    // --- DSL machine definition ---

    machine! {
        machine ScheduleLifecycleMachine {
            version: 1,
            rust: "meerkat-schedule" / "shadow::schedule",

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

    // --- Projection ---

    fn dt_to_ms(dt: chrono::DateTime<chrono::Utc>) -> u64 {
        dt.timestamp_millis() as u64
    }

    fn map_misfire(p: &crate::types::MisfirePolicy) -> MisfirePolicy {
        match p {
            crate::types::MisfirePolicy::Skip => MisfirePolicy::Skip,
            // CatchUpWithin maps to Execute for shadow purposes — both are "non-skip"
            crate::types::MisfirePolicy::CatchUpWithin { .. } => MisfirePolicy::Execute,
        }
    }

    fn map_overlap(p: &crate::types::OverlapPolicy) -> OverlapPolicy {
        match p {
            crate::types::OverlapPolicy::SkipIfRunning => OverlapPolicy::SkipIfRunning,
            crate::types::OverlapPolicy::AllowConcurrent => OverlapPolicy::Queue,
        }
    }

    fn map_missing(p: &crate::types::MissingTargetPolicy) -> MissingTargetPolicy {
        match p {
            crate::types::MissingTargetPolicy::MarkMisfired => MissingTargetPolicy::MarkMisfired,
            crate::types::MissingTargetPolicy::Skip => MissingTargetPolicy::Skip,
        }
    }

    fn project_schedule(sched: &crate::types::Schedule) -> ScheduleLifecycleMachineState {
        use crate::types::SchedulePhase as P;
        let phase = match sched.phase {
            P::Active => ScheduleLifecycleState::Active,
            P::Paused => ScheduleLifecycleState::Paused,
            P::Deleted => ScheduleLifecycleState::Deleted,
        };

        ScheduleLifecycleMachineState {
            lifecycle_phase: phase,
            revision: sched.revision.0,
            trigger_key: format!("{:?}", sched.trigger),
            target_binding_key: sched.target.stable_key(),
            misfire_policy: map_misfire(&sched.misfire_policy),
            overlap_policy: map_overlap(&sched.overlap_policy),
            missing_target_policy: map_missing(&sched.missing_target_policy),
            planning_cursor_utc_ms: sched.planning_cursor_utc.map(dt_to_ms),
            next_occurrence_ordinal: sched.next_occurrence_ordinal.0,
        }
    }

    // --- Shadow comparison ---

    fn phase_name(phase: &ScheduleLifecycleState) -> &'static str {
        match phase {
            ScheduleLifecycleState::Active => "Active",
            ScheduleLifecycleState::Paused => "Paused",
            ScheduleLifecycleState::Deleted => "Deleted",
        }
    }

    fn real_phase_name(phase: crate::types::SchedulePhase) -> &'static str {
        use crate::types::SchedulePhase as P;
        match phase {
            P::Active => "Active",
            P::Paused => "Paused",
            P::Deleted => "Deleted",
        }
    }

    pub(crate) fn shadow_assert(
        pre_schedule: Option<&crate::types::Schedule>,
        input: &crate::authority::ScheduleLifecycleInput,
        result: &Result<
            crate::authority::ScheduleLifecycleMutator,
            crate::authority::ScheduleLifecycleError,
        >,
    ) {
        use crate::authority::ScheduleLifecycleInput as Real;

        // Skip inputs the DSL doesn't model (Create from None, Update is conditional)
        let (dsl_state, dsl_input) = match input {
            Real::Create(_) => {
                // Create takes None schedule; DSL Create is a self-loop from Active.
                // The DSL's Create is structural, not the same as the authority's Create
                // (which constructs a new schedule). Skip the shadow for this case.
                return;
            }
            Real::Update(_) => {
                // Update does conditional revision bumping; DSL Revise always bumps.
                // Only shadow-compare when revision was bumped (semantic update).
                if let Ok(mutator) = result {
                    if !mutator.revision_bumped {
                        // Metadata-only update — not modeled by DSL
                        return;
                    }
                    // Revision was bumped — shadow as Revise
                    let sched = pre_schedule.as_ref().expect("Update requires pre-schedule");
                    let mut state = project_schedule(sched);
                    let dsl_in = ScheduleLifecycleInput::Revise {
                        trigger_key: format!("{:?}", mutator.schedule.trigger),
                        target_binding_key: mutator.schedule.target.stable_key(),
                        misfire_policy: map_misfire(&mutator.schedule.misfire_policy),
                        overlap_policy: map_overlap(&mutator.schedule.overlap_policy),
                        missing_target_policy: map_missing(&mutator.schedule.missing_target_policy),
                    };
                    (state, dsl_in)
                } else {
                    // Authority rejected — skip (DSL rejection model differs for Update)
                    return;
                }
            }
            Real::RecordPlanningWindow {
                planning_cursor_utc,
                next_occurrence_ordinal,
            } => {
                let sched = pre_schedule.expect("RecordPlanningWindow requires schedule");
                let state = project_schedule(sched);
                let dsl_in = ScheduleLifecycleInput::RecordPlanningWindow {
                    planning_cursor_utc_ms: dt_to_ms(*planning_cursor_utc),
                    next_occurrence_ordinal: next_occurrence_ordinal.0,
                };
                (state, dsl_in)
            }
            Real::Pause { at_utc } => {
                let sched = pre_schedule.expect("Pause requires schedule");
                let state = project_schedule(sched);
                let dsl_in = ScheduleLifecycleInput::Pause {
                    at_utc_ms: dt_to_ms(*at_utc),
                };
                (state, dsl_in)
            }
            Real::Resume { at_utc } => {
                let sched = pre_schedule.expect("Resume requires schedule");
                let state = project_schedule(sched);
                let dsl_in = ScheduleLifecycleInput::Resume {
                    at_utc_ms: dt_to_ms(*at_utc),
                };
                (state, dsl_in)
            }
            Real::Delete { at_utc } => {
                let sched = pre_schedule.expect("Delete requires schedule");
                let state = project_schedule(sched);
                let dsl_in = ScheduleLifecycleInput::Delete {
                    at_utc_ms: dt_to_ms(*at_utc),
                };
                (state, dsl_in)
            }
        };

        let mut dsl_authority = ScheduleLifecycleMachineAuthority::from_state(dsl_state);
        let dsl_result = ScheduleLifecycleMachineMutator::apply(&mut dsl_authority, dsl_input);

        match (result, &dsl_result) {
            (Ok(mutator), Ok(transition)) => {
                let authority_phase = real_phase_name(mutator.schedule.phase);
                let dsl_phase = phase_name(&dsl_authority.state.lifecycle_phase);
                assert_eq!(
                    authority_phase, dsl_phase,
                    "shadow schedule: phase mismatch: authority={authority_phase}, dsl={dsl_phase}",
                );

                assert_eq!(
                    mutator.effects.len(),
                    transition.effects.len(),
                    "shadow schedule: effect count mismatch: authority={}, dsl={}",
                    mutator.effects.len(),
                    transition.effects.len(),
                );
            }
            (Err(_), Err(_)) => {}
            (Ok(mutator), Err(e)) => {
                panic!(
                    "shadow schedule: authority accepted (phase={}) but DSL rejected: {e:?}",
                    real_phase_name(mutator.schedule.phase),
                );
            }
            (Err(e), Ok(_)) => {
                let dsl_phase = phase_name(&dsl_authority.state.lifecycle_phase);
                panic!(
                    "shadow schedule: authority rejected ({e:?}) but DSL accepted (phase={dsl_phase})",
                );
            }
        }
    }
}
