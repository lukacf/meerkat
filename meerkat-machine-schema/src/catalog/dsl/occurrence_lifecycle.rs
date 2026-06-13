use super::OptionValueExt;
use meerkat_machine_dsl::machine;

machine! {
    machine OccurrenceLifecycleMachine {
        version: 8,
        rust: "self" / "catalog::dsl::occurrence_lifecycle",

        state {
            lifecycle_phase: OccurrenceLifecycleState,
            occurrence_id: OccurrenceId,
            schedule_id: ScheduleId,
            schedule_revision: u64,
            occurrence_ordinal: u64,
            trigger_key: TriggerKey,
            target_binding_key: TargetBindingId,
            misfire_policy: Enum<MisfirePolicy>,
            misfire_policy_key: String,
            overlap_policy: Enum<OverlapPolicy>,
            overlap_policy_key: String,
            missing_target_policy: Enum<MissingTargetPolicy>,
            missing_target_policy_key: String,
            due_at_utc_ms: u64,
            misfire_deadline_utc_ms: u64,
            claimed_by: Option<ClaimOwner>,
            lease_expires_at_utc_ms: Option<u64>,
            claimed_at_utc_ms: Option<u64>,
            claim_token: Option<ClaimToken>,
            delivery_correlation_id: Option<CorrelationId>,
            target_materialized_session_id: Option<SessionId>,
            receipt_recorded_at_utc_ms: Option<u64>,
            last_receipt_recorded_at_utc_ms: Option<u64>,
            last_receipt_attempt: Option<u64>,
            last_receipt_stage: Option<Enum<DeliveryReceiptStage>>,
            last_receipt_failure_class: Option<Enum<OccurrenceFailureClass>>,
            last_receipt_detail: Option<String>,
            last_receipt_correlation_id: Option<CorrelationId>,
            last_receipt_materialized_session_id: Option<SessionId>,
            runtime_outcome_key: Option<RuntimeOutcomeKey>,
            receipt_stage: Option<Enum<DeliveryReceiptStage>>,
            receipt_failure_class: Option<Enum<OccurrenceFailureClass>>,
            receipt_detail: Option<String>,
            failure_class: Option<Enum<OccurrenceFailureClass>>,
            failure_detail: Option<String>,
            dispatched_at_utc_ms: Option<u64>,
            completed_at_utc_ms: Option<u64>,
            attempt_count: u64,
            superseded_by_revision: Option<u64>,
            // 0.7.2 D2a (disciplined shell inputs): typed late-arrival record.
            // A delivery resolution (Complete / ResolveRuntimeCompletion /
            // ResolveDeliveryCompletionFailure / ResolveDeliveryFailure) that
            // arrives after this occurrence was superseded is a legitimate
            // interleaving, not an error. The machine accepts it as a
            // self-loop in Superseded and records what the delivery actually
            // did as a machine-owned fact instead of guard-rejecting the
            // completion waiter.
            late_completion_recorded_at_utc_ms: Option<u64>,
            late_completion_resolution: Option<Enum<LateCompletionResolutionClass>>,
            late_completion_detail: Option<String>,
            // 0.7.2 D2a: count of completion-shaped arrivals that bore a
            // stale claim (attempt/claim-token no longer current). The store
            // shell screens those arrivals before they can mutate lifecycle
            // state; the screen verdict is fed back here as a typed
            // classification so the abandonment is a machine fact, never a
            // silent drop.
            stale_completion_arrivals: u64,
        }

        init(Pending) {
            occurrence_id = "occurrence-0",
            schedule_id = "schedule-0",
            schedule_revision = 1,
            occurrence_ordinal = 0,
            trigger_key = "trigger-0",
            target_binding_key = "target-0",
            misfire_policy = MisfirePolicy::Skip,
            misfire_policy_key = "misfire:skip",
            overlap_policy = OverlapPolicy::SkipIfRunning,
            overlap_policy_key = "overlap:skip_if_running",
            missing_target_policy = MissingTargetPolicy::MarkMisfired,
            missing_target_policy_key = "missing_target:mark_misfired",
            due_at_utc_ms = 1,
            misfire_deadline_utc_ms = 1,
            claimed_by = None,
            lease_expires_at_utc_ms = None,
            claimed_at_utc_ms = None,
            claim_token = None,
            delivery_correlation_id = None,
            target_materialized_session_id = None,
            receipt_recorded_at_utc_ms = None,
            last_receipt_recorded_at_utc_ms = None,
            last_receipt_attempt = None,
            last_receipt_stage = None,
            last_receipt_failure_class = None,
            last_receipt_detail = None,
            last_receipt_correlation_id = None,
            last_receipt_materialized_session_id = None,
            runtime_outcome_key = None,
            receipt_stage = None,
            receipt_failure_class = None,
            receipt_detail = None,
            failure_class = None,
            failure_detail = None,
            dispatched_at_utc_ms = None,
            completed_at_utc_ms = None,
            attempt_count = 0,
            superseded_by_revision = None,
            late_completion_recorded_at_utc_ms = None,
            late_completion_resolution = None,
            late_completion_detail = None,
            stale_completion_arrivals = 0,
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
            PlanOccurrence {
                occurrence_id: OccurrenceId,
                schedule_id: ScheduleId,
                schedule_revision: u64,
                occurrence_ordinal: u64,
                trigger_key: TriggerKey,
                target_binding_key: TargetBindingId,
                misfire_policy: Enum<MisfirePolicy>,
                misfire_policy_key: String,
                overlap_policy: Enum<OverlapPolicy>,
                overlap_policy_key: String,
                missing_target_policy: Enum<MissingTargetPolicy>,
                missing_target_policy_key: String,
                target_materialized_session_id: Option<SessionId>,
                due_at_utc_ms: u64,
                misfire_deadline_utc_ms: u64,
            },
            SyncTargetSnapshot { target_binding_key: TargetBindingId, target_materialized_session_id: Option<SessionId> },
            RecordReceipt {
                correlation_id: Option<CorrelationId>,
                detail: Option<String>,
                materialized_session_id: Option<SessionId>,
                runtime_outcome_key: Option<RuntimeOutcomeKey>
            },
            ClassifyDue { now_utc_ms: u64 },
            // Terminality classification. This machine owns the lifecycle_phase
            // and the terminal-phase SET (`terminal [Completed, Skipped,
            // Misfired, Superseded, DeliveryFailed]` above). The shell extracts
            // no fact — it drives this input over the occurrence's recovered
            // machine state and mirrors the emitted
            // OccurrenceTerminalityClassified.terminal, failing closed. Each
            // transition self-loops in its phase (classification never mutates
            // lifecycle state).
            ClassifyOccurrenceTerminality {},
            // Claimed-occurrence pre-dispatch reconciliation. The driver shell
            // observes the owning schedule's current phase and revision (pure
            // observations) and feeds them here together with the occurrence's
            // own claimed schedule_revision (already machine-owned state). The
            // occurrence authority — not the driver — classifies the dispatch
            // disposition: a paused schedule freezes the claim, a deleted
            // schedule or a stale claimed revision supersedes it, a claimed
            // revision ahead of the schedule is an impossible/corrupt fact, and
            // otherwise the claim is ready to dispatch.
            ClassifyClaimedDispatchDisposition {
                schedule_phase: Enum<ClaimedDispatchSchedulePhase>,
                current_schedule_revision: u64
            },
            // Post-completion supersession reconciliation. After a dispatched
            // occurrence's delivery completes, the driver shell observes the
            // owning schedule's current phase and revision (pure observations)
            // and feeds them here against the occurrence's own claimed
            // schedule_revision (already machine-owned state). The occurrence
            // authority — not the driver — classifies whether the completed
            // delivery is superseded (the schedule was deleted, or the claim is
            // for a stale revision behind the schedule's current revision) or
            // should proceed to terminalize on its delivery result. Unlike the
            // pre-dispatch disposition, a paused schedule does NOT supersede a
            // delivery that has already completed.
            ClassifyCompletionSupersession {
                schedule_phase: Enum<ClaimedDispatchSchedulePhase>,
                current_schedule_revision: u64
            },
            Claim { owner_id: ClaimOwner, at_utc_ms: u64, lease_expires_at_utc_ms: u64, claim_token: ClaimToken },
            DispatchStarted { correlation_id: Option<CorrelationId>, at_utc_ms: u64 },
            AwaitCompletion { at_utc_ms: u64 },
            Complete { at_utc_ms: u64 },
            ResolveRuntimeCompletion {
                outcome: Enum<RuntimeCompletionOutcome>,
                detail: Option<String>,
                at_utc_ms: u64
            },
            ResolveDeliveryCompletionFailure {
                reason: Enum<DeliveryCompletionFailureReason>,
                detail: Option<String>,
                at_utc_ms: u64
            },
            ResolveDeliveryFailure {
                reason: Enum<DeliveryFailureReason>,
                detail: Option<String>,
                at_utc_ms: u64
            },
            ResolveTargetProbe {
                outcome: Enum<OccurrenceTargetProbeOutcome>,
                detail: Option<String>,
                at_utc_ms: u64
            },
            ResolveDueMisfire { detail: Option<String>, at_utc_ms: u64 },
            Supersede { superseded_by_revision: u64, at_utc_ms: u64 },
            LeaseExpired { at_utc_ms: u64 },
            ReleaseLeaseForPausedSchedule { at_utc_ms: u64 },
            ClassifyTransitionFailure {
                refusal_kind: Enum<OccurrenceTransitionFailureRefusalKind>,
                trigger: Enum<OccurrenceLifecycleInputVariant>
            },
            // 0.7.2 D2a: typed classification of a completion-shaped arrival
            // whose claim evidence (attempt count / claim token) is no longer
            // current. The store shell screens such arrivals before they can
            // reach lifecycle transitions; instead of dropping the screened
            // arrival silently (`Ok(None)` → `Ok(false)`), the shell feeds the
            // screen verdict back through this input so the occurrence
            // authority records the abandonment as a machine fact. Total in
            // every phase for completion-shaped triggers: stale arrivals can
            // race any later lifecycle state (released lease, reclaim, pause,
            // delete, terminal outcomes).
            ClassifyStaleCompletionArrival {
                trigger: Enum<OccurrenceLifecycleInputVariant>
            },
        }

        effect OccurrenceLifecycleEffect {
            Claimed,
            DispatchStarted,
            AwaitingCompletion,
            Completed,
            Skipped,
            Misfired,
            Superseded,
            // Reciprocal-ack effect (wave-d D-f): after the occurrence
            // absorbs a Supersede it reports the superseding revision and
            // its own occurrence_id back to the schedule authority so the
            // schedule side observes which occurrences it actually
            // superseded (instead of firing a one-way Supersede and
            // hoping).
            OccurrencesSuperseded { occurrence_id: OccurrenceId, superseding_revision: u64 },
            DueNoAction,
            DueClaimEligible,
            DueMisfireRequired,
            DueLeaseExpired,
            // Terminality verdict over the machine-owned lifecycle_phase. The
            // shell mirrors `terminal`; it decides nothing.
            OccurrenceTerminalityClassified { terminal: bool },
            // Claimed-occurrence pre-dispatch disposition decided by the
            // occurrence authority. The driver shell mirrors `disposition`:
            // Frozen releases the lease for the paused schedule, Supersede
            // terminalizes the occurrence against `superseded_by_revision`,
            // Ready continues to dispatch, and FutureRevision is an impossible
            // fact the driver surfaces as an internal error.
            ClaimedDispatchDispositionClassified {
                disposition: Enum<ClaimedDispatchDisposition>,
                superseded_by_revision: Option<u64>,
            },
            // Post-completion supersession disposition decided by the
            // occurrence authority. The driver shell mirrors `disposition`:
            // Supersede terminalizes the completed occurrence against
            // `superseded_by_revision`, and Proceed continues to terminalize on
            // the occurrence's delivery result.
            CompletionSupersessionClassified {
                disposition: Enum<CompletionSupersessionDisposition>,
                superseded_by_revision: Option<u64>,
            },
            DeliveryFailed,
            LeaseExpired,
            TransitionFailureClassified {
                phase: Enum<OccurrenceLifecycleState>,
                refusal_kind: Enum<OccurrenceTransitionFailureRefusalKind>,
                trigger: Enum<OccurrenceLifecycleInputVariant>,
                public_class: Enum<OccurrenceTransitionFailureClassKind>,
            },
            // 0.7.2 D2a: a delivery resolution arrived after this occurrence
            // was superseded and was recorded as a typed late-arrival fact.
            // The driver shell mirrors this to know the arrival landed as a
            // late record (no fresh terminal receipt is minted for it) instead
            // of treating the machine's answer as a guard rejection.
            LateCompletionResolutionRecorded {
                resolution: Enum<LateCompletionResolutionClass>,
            },
            // 0.7.2 D2a: a completion-shaped arrival bearing stale claim
            // evidence was classified. Pure observability for the shell
            // (debug-level, never ERROR); the lifecycle state is untouched.
            StaleCompletionArrivalClassified {
                phase: Enum<OccurrenceLifecycleState>,
                trigger: Enum<OccurrenceLifecycleInputVariant>,
            },
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

        invariant misfire_deadline_not_before_due {
            self.misfire_deadline_utc_ms >= self.due_at_utc_ms
        }

        // 0.7.2 D2a: late-arrival records only exist on superseded
        // occurrences (Superseded is terminal, so once recorded the pairing
        // holds forever).
        invariant late_completion_only_after_supersession {
            self.late_completion_resolution == None || self.lifecycle_phase == Phase::Superseded
        }

        invariant late_completion_resolution_requires_timestamp {
            self.late_completion_resolution == None || self.late_completion_recorded_at_utc_ms != None
        }

        invariant late_completion_timestamp_requires_resolution {
            self.late_completion_recorded_at_utc_ms == None || self.late_completion_resolution != None
        }

        invariant late_completion_detail_requires_resolution {
            self.late_completion_detail == None || self.late_completion_resolution != None
        }

        disposition Claimed => external seam SurfaceResultAlignment,
        disposition DispatchStarted => external seam SurfaceResultAlignment,
        disposition AwaitingCompletion => external seam SurfaceResultAlignment,
        disposition Completed => external seam SurfaceResultAlignment,
        disposition Skipped => external seam SurfaceResultAlignment,
        disposition Misfired => external seam SurfaceResultAlignment,
        disposition Superseded => external seam SurfaceResultAlignment,
        disposition OccurrencesSuperseded => routed [ScheduleLifecycleMachine] seam NoOwnerRealization,
        disposition DueNoAction => local seam NoOwnerRealization,
        disposition DueClaimEligible => local seam NoOwnerRealization,
        disposition DueMisfireRequired => local seam NoOwnerRealization,
        disposition DueLeaseExpired => local seam NoOwnerRealization,
        disposition OccurrenceTerminalityClassified => local seam NoOwnerRealization,
        disposition ClaimedDispatchDispositionClassified => local seam NoOwnerRealization,
        disposition CompletionSupersessionClassified => local seam NoOwnerRealization,
        disposition DeliveryFailed => external seam SurfaceResultAlignment,
        disposition LeaseExpired => external seam SurfaceResultAlignment,
        disposition TransitionFailureClassified => local seam SurfaceResultAlignment,
        // Late-arrival record: the driver mirrors it to align its receipt /
        // waiter-resolution behavior with the machine's verdict (the arrival
        // landed as a late record, not a fresh terminal transition).
        disposition LateCompletionResolutionRecorded => local seam SurfaceResultAlignment,
        // Stale-arrival classification: pure observability; no owner realizes
        // it beyond debug-level logging in the driver shell.
        disposition StaleCompletionArrivalClassified => local seam NoOwnerRealization,

        // --- Transition failure classification ---
        //
        // Public occurrence lifecycle error class is a machine fact. Shells
        // feed back the typed refusal evidence emitted by this generated
        // authority, and the occurrence machine owns the mapping from that
        // refusal to the public class exposed by the domain API.

        transition ClassifyTransitionFailurePlanRejected {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "plan_rejected" {
                trigger == OccurrenceLifecycleInputVariant::PlanOccurrence
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::PlanRejected
            }
        }

        transition ClassifyTransitionFailureTargetSyncRejected {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "target_sync_rejected" {
                trigger == OccurrenceLifecycleInputVariant::SyncTargetSnapshot
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::TargetSyncRejected
            }
        }

        transition ClassifyTransitionFailureReceiptRecordRejected {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "receipt_record_rejected" {
                trigger == OccurrenceLifecycleInputVariant::RecordReceipt
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::ReceiptRecordRejected
            }
        }

        transition ClassifyTransitionFailureDueClassificationRejected {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "due_classification_rejected" {
                trigger == OccurrenceLifecycleInputVariant::ClassifyDue
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::DueClassificationRejected
            }
        }

        transition ClassifyTransitionFailureClaimedDispatchDispositionRejected {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "claimed_dispatch_disposition_rejected" {
                trigger == OccurrenceLifecycleInputVariant::ClassifyClaimedDispatchDisposition
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::ClaimedDispatchClassificationRejected
            }
        }

        transition ClassifyTransitionFailureCompletionSupersessionRejected {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "completion_supersession_rejected" {
                trigger == OccurrenceLifecycleInputVariant::ClassifyCompletionSupersession
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::CompletionSupersessionClassificationRejected
            }
        }

        transition ClassifyTransitionFailureClaimRejectedPending {
            per_phase [Pending]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "claim_rejected_pending" {
                trigger == OccurrenceLifecycleInputVariant::Claim
                && refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::ClaimRejected
            }
        }

        transition ClassifyTransitionFailureNotPendingForClaim {
            per_phase [Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "not_pending_for_claim" {
                trigger == OccurrenceLifecycleInputVariant::Claim
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::NotPendingForClaim
            }
        }

        transition ClassifyTransitionFailureNotClaimed {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "not_claimed" {
                trigger == OccurrenceLifecycleInputVariant::DispatchStarted
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::NotClaimed
            }
        }

        transition ClassifyTransitionFailureNotDispatching {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "not_dispatching" {
                trigger == OccurrenceLifecycleInputVariant::AwaitCompletion
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::NotDispatching
            }
        }

        transition ClassifyTransitionFailureNotLeaseHolding {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "not_lease_holding" {
                (
                    trigger == OccurrenceLifecycleInputVariant::LeaseExpired
                    || trigger == OccurrenceLifecycleInputVariant::ReleaseLeaseForPausedSchedule
                )
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::NotLeaseHolding
            }
        }

        transition ClassifyTransitionFailureNotLiveForTerminal {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "not_live_for_terminal" {
                (
                    trigger == OccurrenceLifecycleInputVariant::Complete
                    || trigger == OccurrenceLifecycleInputVariant::ResolveRuntimeCompletion
                    || trigger == OccurrenceLifecycleInputVariant::ResolveDeliveryCompletionFailure
                    || trigger == OccurrenceLifecycleInputVariant::ResolveDeliveryFailure
                    || trigger == OccurrenceLifecycleInputVariant::ResolveTargetProbe
                    || trigger == OccurrenceLifecycleInputVariant::ResolveDueMisfire
                    || trigger == OccurrenceLifecycleInputVariant::Supersede
                )
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::NotLiveForTerminal
            }
        }

        transition ClassifyTransitionFailureStaleCompletionArrivalRejected {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyTransitionFailure { refusal_kind, trigger }
            guard "stale_completion_arrival_rejected" {
                trigger == OccurrenceLifecycleInputVariant::ClassifyStaleCompletionArrival
                && (
                    refusal_kind == OccurrenceTransitionFailureRefusalKind::GuardRejected
                    || refusal_kind == OccurrenceTransitionFailureRefusalKind::NoMatchingTransition
                )
            }
            update {}
            to Pending
            emit TransitionFailureClassified {
                phase: self.lifecycle_phase,
                refusal_kind: refusal_kind,
                trigger: trigger,
                public_class: OccurrenceTransitionFailureClassKind::StaleCompletionArrivalClassificationRejected
            }
        }

        // --- Stale completion arrival classification (0.7.2 D2a) ---
        //
        // The store shell screens completion-shaped arrivals against the
        // occurrence's current claim evidence (attempt count + claim token)
        // before letting them mutate lifecycle state. When the screen finds
        // the arrival stale (the claim was released for a paused schedule,
        // expired and reclaimed, or the occurrence was superseded by a newer
        // attempt), the shell feeds the verdict back through this input. The
        // occurrence authority records the abandonment as a typed machine
        // fact and emits a classification effect; lifecycle state is never
        // mutated by a stale arrival. Total in every phase for
        // completion-shaped triggers — stale arrivals can race anything.

        transition ClassifyStaleCompletionArrivalObserved {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion, Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyStaleCompletionArrival { trigger }
            guard "completion_shaped_trigger" {
                trigger == OccurrenceLifecycleInputVariant::Complete
                || trigger == OccurrenceLifecycleInputVariant::ResolveRuntimeCompletion
                || trigger == OccurrenceLifecycleInputVariant::ResolveDeliveryCompletionFailure
                || trigger == OccurrenceLifecycleInputVariant::ResolveDeliveryFailure
                || trigger == OccurrenceLifecycleInputVariant::Supersede
            }
            update {
                self.stale_completion_arrivals += 1;
            }
            to Pending
            emit StaleCompletionArrivalClassified {
                phase: self.lifecycle_phase,
                trigger: trigger
            }
        }

        // --- Plan occurrence ---
        //
        // Occurrence creation is a semantic fact: id, schedule revision,
        // ordinal, due time, and target binding determine later dispatch
        // behavior. The shell may allocate opaque ids and carry full target
        // snapshots, but the machine owns accepting the planned occurrence
        // facts before the domain row is materialized.

        transition PlanOccurrenceFromPending {
            on input PlanOccurrence {
                occurrence_id,
                schedule_id,
                schedule_revision,
                occurrence_ordinal,
                trigger_key,
                target_binding_key,
                misfire_policy,
                misfire_policy_key,
                overlap_policy,
                overlap_policy_key,
                missing_target_policy,
                missing_target_policy_key,
                target_materialized_session_id,
                due_at_utc_ms,
                misfire_deadline_utc_ms
            }
            guard {
                self.lifecycle_phase == Phase::Pending
                && self.attempt_count == 0
                && self.claimed_by == None
                && self.claim_token == None
                && self.delivery_correlation_id == None
                && self.target_materialized_session_id == None
                && self.completed_at_utc_ms == None
                && self.superseded_by_revision == None
                && misfire_deadline_utc_ms >= due_at_utc_ms
            }
            update {
                self.occurrence_id = occurrence_id;
                self.schedule_id = schedule_id;
                self.schedule_revision = schedule_revision;
                self.occurrence_ordinal = occurrence_ordinal;
                self.trigger_key = trigger_key;
                self.target_binding_key = target_binding_key;
                self.misfire_policy = misfire_policy;
                self.misfire_policy_key = misfire_policy_key;
                self.overlap_policy = overlap_policy;
                self.overlap_policy_key = overlap_policy_key;
                self.missing_target_policy = missing_target_policy;
                self.missing_target_policy_key = missing_target_policy_key;
                self.target_materialized_session_id = target_materialized_session_id;
                self.due_at_utc_ms = due_at_utc_ms;
                self.misfire_deadline_utc_ms = misfire_deadline_utc_ms;
                self.claimed_by = None;
                self.lease_expires_at_utc_ms = None;
                self.claimed_at_utc_ms = None;
                self.claim_token = None;
                self.delivery_correlation_id = None;
                self.receipt_recorded_at_utc_ms = None;
                self.last_receipt_recorded_at_utc_ms = None;
                self.last_receipt_attempt = None;
                self.last_receipt_stage = None;
                self.last_receipt_failure_class = None;
                self.last_receipt_detail = None;
                self.last_receipt_correlation_id = None;
                self.last_receipt_materialized_session_id = None;
                self.runtime_outcome_key = None;
                self.receipt_stage = None;
                self.receipt_failure_class = None;
                self.receipt_detail = None;
                self.failure_class = None;
                self.failure_detail = None;
                self.dispatched_at_utc_ms = None;
                self.completed_at_utc_ms = None;
                self.attempt_count = 0;
                self.superseded_by_revision = None;
                self.late_completion_recorded_at_utc_ms = None;
                self.late_completion_resolution = None;
                self.late_completion_detail = None;
                self.stale_completion_arrivals = 0;
            }
            to Pending
        }

        // --- Due classification ---
        //
        // Store shells observe time and durable ordering, but occurrence
        // authority classifies due admission, pending misfire, and expired
        // lease reclaimability.

        transition ClassifyDuePendingFuture {
            on input ClassifyDue { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Pending && now_utc_ms < self.due_at_utc_ms }
            to Pending
            emit DueNoAction
        }

        transition ClassifyDuePendingMisfire {
            on input ClassifyDue { now_utc_ms }
            guard {
                self.lifecycle_phase == Phase::Pending
                && self.due_at_utc_ms <= now_utc_ms
                && self.misfire_deadline_utc_ms < now_utc_ms
            }
            to Pending
            emit DueMisfireRequired
        }

        transition ClassifyDuePendingClaimEligible {
            on input ClassifyDue { now_utc_ms }
            guard {
                self.lifecycle_phase == Phase::Pending
                && self.due_at_utc_ms <= now_utc_ms
                && now_utc_ms <= self.misfire_deadline_utc_ms
            }
            to Pending
            emit DueClaimEligible
        }

        transition ClassifyDueClaimedLeaseExpired {
            on input ClassifyDue { now_utc_ms }
            guard {
                self.lifecycle_phase == Phase::Claimed
                && self.lease_expires_at_utc_ms != None
                && self.lease_expires_at_utc_ms.get("value") <= now_utc_ms
            }
            to Claimed
            emit DueLeaseExpired
        }

        transition ClassifyDueDispatchingLeaseExpired {
            on input ClassifyDue { now_utc_ms }
            guard {
                self.lifecycle_phase == Phase::Dispatching
                && self.lease_expires_at_utc_ms != None
                && self.lease_expires_at_utc_ms.get("value") <= now_utc_ms
            }
            to Dispatching
            emit DueLeaseExpired
        }

        transition ClassifyDueAwaitingCompletionLeaseExpired {
            on input ClassifyDue { now_utc_ms }
            guard {
                self.lifecycle_phase == Phase::AwaitingCompletion
                && self.lease_expires_at_utc_ms != None
                && self.lease_expires_at_utc_ms.get("value") <= now_utc_ms
            }
            to AwaitingCompletion
            emit DueLeaseExpired
        }

        transition ClassifyDueClaimedLeaseCurrent {
            on input ClassifyDue { now_utc_ms }
            guard {
                self.lifecycle_phase == Phase::Claimed
                && (
                    self.lease_expires_at_utc_ms == None
                    || now_utc_ms < self.lease_expires_at_utc_ms.get("value")
                )
            }
            to Claimed
            emit DueNoAction
        }

        transition ClassifyDueDispatchingLeaseCurrent {
            on input ClassifyDue { now_utc_ms }
            guard {
                self.lifecycle_phase == Phase::Dispatching
                && (
                    self.lease_expires_at_utc_ms == None
                    || now_utc_ms < self.lease_expires_at_utc_ms.get("value")
                )
            }
            to Dispatching
            emit DueNoAction
        }

        transition ClassifyDueAwaitingCompletionLeaseCurrent {
            on input ClassifyDue { now_utc_ms }
            guard {
                self.lifecycle_phase == Phase::AwaitingCompletion
                && (
                    self.lease_expires_at_utc_ms == None
                    || now_utc_ms < self.lease_expires_at_utc_ms.get("value")
                )
            }
            to AwaitingCompletion
            emit DueNoAction
        }

        transition ClassifyDueCompletedNoAction {
            on input ClassifyDue { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Completed }
            to Completed
            emit DueNoAction
        }

        transition ClassifyDueSkippedNoAction {
            on input ClassifyDue { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Skipped }
            to Skipped
            emit DueNoAction
        }

        transition ClassifyDueMisfiredNoAction {
            on input ClassifyDue { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Misfired }
            to Misfired
            emit DueNoAction
        }

        transition ClassifyDueSupersededNoAction {
            on input ClassifyDue { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            to Superseded
            emit DueNoAction
        }

        transition ClassifyDueDeliveryFailedNoAction {
            on input ClassifyDue { now_utc_ms }
            guard { self.lifecycle_phase == Phase::DeliveryFailed }
            to DeliveryFailed
            emit DueNoAction
        }

        // --- Terminality classification ---
        //
        // The terminality verdict over the machine-owned lifecycle_phase is a
        // machine fact. The shell drives this input over the occurrence's
        // recovered state and mirrors the emitted `terminal`. Each transition
        // self-loops in its phase (classification never mutates lifecycle
        // state). The terminal phase set here is exactly `terminal [Completed,
        // Skipped, Misfired, Superseded, DeliveryFailed]` declared above.

        transition ClassifyOccurrenceTerminalityTerminal {
            per_phase [Completed, Skipped, Misfired, Superseded, DeliveryFailed]
            on input ClassifyOccurrenceTerminality {}
            update {}
            to Pending
            emit OccurrenceTerminalityClassified { terminal: true }
        }

        transition ClassifyOccurrenceTerminalityLive {
            per_phase [Pending, Claimed, Dispatching, AwaitingCompletion]
            on input ClassifyOccurrenceTerminality {}
            update {}
            to Pending
            emit OccurrenceTerminalityClassified { terminal: false }
        }

        // --- Claimed-occurrence pre-dispatch disposition ---
        //
        // The driver claims due occurrences and then, before dispatching,
        // reconciles each claim against the latest owning-schedule facts. The
        // schedule's current phase and revision are pure observations the
        // driver extracts; the occurrence authority owns whether the claim is
        // frozen (schedule paused), superseded (schedule deleted or the claim's
        // revision is stale), ready, or references an impossible future
        // revision. These are pure classifications in the Claimed phase.

        transition ClassifyClaimedDispatchDispositionFutureRevision {
            on input ClassifyClaimedDispatchDisposition { schedule_phase, current_schedule_revision }
            guard {
                self.lifecycle_phase == Phase::Claimed
                && current_schedule_revision < self.schedule_revision
            }
            to Claimed
            emit ClaimedDispatchDispositionClassified {
                disposition: ClaimedDispatchDisposition::FutureRevision,
                superseded_by_revision: None
            }
        }

        transition ClassifyClaimedDispatchDispositionFrozen {
            on input ClassifyClaimedDispatchDisposition { schedule_phase, current_schedule_revision }
            guard {
                self.lifecycle_phase == Phase::Claimed
                && current_schedule_revision >= self.schedule_revision
                && schedule_phase == ClaimedDispatchSchedulePhase::Paused
            }
            to Claimed
            emit ClaimedDispatchDispositionClassified {
                disposition: ClaimedDispatchDisposition::Frozen,
                superseded_by_revision: None
            }
        }

        transition ClassifyClaimedDispatchDispositionSupersedeDeleted {
            on input ClassifyClaimedDispatchDisposition { schedule_phase, current_schedule_revision }
            guard {
                self.lifecycle_phase == Phase::Claimed
                && current_schedule_revision >= self.schedule_revision
                && schedule_phase == ClaimedDispatchSchedulePhase::Deleted
            }
            to Claimed
            emit ClaimedDispatchDispositionClassified {
                disposition: ClaimedDispatchDisposition::Supersede,
                superseded_by_revision: Some(current_schedule_revision)
            }
        }

        transition ClassifyClaimedDispatchDispositionSupersedeStale {
            on input ClassifyClaimedDispatchDisposition { schedule_phase, current_schedule_revision }
            guard {
                self.lifecycle_phase == Phase::Claimed
                && schedule_phase == ClaimedDispatchSchedulePhase::Active
                && self.schedule_revision < current_schedule_revision
            }
            to Claimed
            emit ClaimedDispatchDispositionClassified {
                disposition: ClaimedDispatchDisposition::Supersede,
                superseded_by_revision: Some(current_schedule_revision)
            }
        }

        transition ClassifyClaimedDispatchDispositionReady {
            on input ClassifyClaimedDispatchDisposition { schedule_phase, current_schedule_revision }
            guard {
                self.lifecycle_phase == Phase::Claimed
                && schedule_phase == ClaimedDispatchSchedulePhase::Active
                && self.schedule_revision == current_schedule_revision
            }
            to Claimed
            emit ClaimedDispatchDispositionClassified {
                disposition: ClaimedDispatchDisposition::Ready,
                superseded_by_revision: None
            }
        }

        // --- Post-completion supersession disposition ---
        //
        // After a dispatched occurrence's delivery completes, the driver
        // reconciles it against the latest owning-schedule facts before
        // terminalizing on the delivery result. The schedule's current phase and
        // revision are pure observations the driver extracts; the occurrence
        // authority owns whether the completed delivery is superseded (the
        // schedule was deleted, or the claim is for a revision behind the
        // schedule's current revision) or should proceed. Unlike the pre-dispatch
        // disposition, a paused schedule does NOT supersede an already-completed
        // delivery. These are pure classifications in the live post-dispatch
        // phases (the occurrence is in AwaitingCompletion when delivery resolves).

        transition ClassifyCompletionSupersessionDeleted {
            on input ClassifyCompletionSupersession { schedule_phase, current_schedule_revision }
            guard {
                self.lifecycle_phase == Phase::AwaitingCompletion
                && schedule_phase == ClaimedDispatchSchedulePhase::Deleted
            }
            to AwaitingCompletion
            emit CompletionSupersessionClassified {
                disposition: CompletionSupersessionDisposition::Supersede,
                superseded_by_revision: Some(current_schedule_revision)
            }
        }

        transition ClassifyCompletionSupersessionStale {
            on input ClassifyCompletionSupersession { schedule_phase, current_schedule_revision }
            guard {
                self.lifecycle_phase == Phase::AwaitingCompletion
                && schedule_phase != ClaimedDispatchSchedulePhase::Deleted
                && self.schedule_revision < current_schedule_revision
            }
            to AwaitingCompletion
            emit CompletionSupersessionClassified {
                disposition: CompletionSupersessionDisposition::Supersede,
                superseded_by_revision: Some(current_schedule_revision)
            }
        }

        transition ClassifyCompletionSupersessionProceed {
            on input ClassifyCompletionSupersession { schedule_phase, current_schedule_revision }
            guard {
                self.lifecycle_phase == Phase::AwaitingCompletion
                && schedule_phase != ClaimedDispatchSchedulePhase::Deleted
                && self.schedule_revision >= current_schedule_revision
            }
            to AwaitingCompletion
            emit CompletionSupersessionClassified {
                disposition: CompletionSupersessionDisposition::Proceed,
                superseded_by_revision: None
            }
        }

        // 0.7.2 D2a: the waiter's occurrence snapshot can already be
        // Superseded when delete/update supersession landed between dispatch
        // and completion. The classification stays total: the authority
        // answers AlreadySuperseded (no superseding revision — the recorded
        // supersession on this occurrence is the truth) instead of refusing
        // to classify. The driver then delivers the completion outcome as a
        // late-arrival record rather than a fresh terminal transition.
        transition ClassifyCompletionSupersessionAlreadySuperseded {
            on input ClassifyCompletionSupersession { schedule_phase, current_schedule_revision }
            guard { self.lifecycle_phase == Phase::Superseded }
            update {}
            to Superseded
            emit CompletionSupersessionClassified {
                disposition: CompletionSupersessionDisposition::AlreadySuperseded,
                superseded_by_revision: None
            }
        }

        // --- Target snapshot sync ---
        //
        // Materialized session binding changes the dispatch target. Keep that
        // target-binding fact behind generated occurrence authority; the shell
        // writes the full target snapshot only after this input is accepted.

        transition SyncTargetSnapshotPending {
            on input SyncTargetSnapshot { target_binding_key, target_materialized_session_id }
            guard { self.lifecycle_phase == Phase::Pending }
            update {
                self.target_binding_key = target_binding_key;
                self.target_materialized_session_id = target_materialized_session_id;
            }
            to Pending
        }

        transition SyncTargetSnapshotClaimed {
            on input SyncTargetSnapshot { target_binding_key, target_materialized_session_id }
            guard { self.lifecycle_phase == Phase::Claimed }
            update {
                self.target_binding_key = target_binding_key;
                self.target_materialized_session_id = target_materialized_session_id;
            }
            to Claimed
        }

        // --- Receipt/result projection ---

        transition RecordReceiptPending {
            on input RecordReceipt { correlation_id, detail, materialized_session_id, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Pending
                && self.receipt_stage != None
                && self.receipt_recorded_at_utc_ms != None
                && self.receipt_detail == detail
                && self.delivery_correlation_id == correlation_id
                && self.target_materialized_session_id == materialized_session_id
            }
            update {
                self.last_receipt_recorded_at_utc_ms = self.receipt_recorded_at_utc_ms;
                self.last_receipt_attempt = Some(self.attempt_count);
                self.last_receipt_stage = self.receipt_stage;
                self.last_receipt_failure_class = self.receipt_failure_class;
                self.last_receipt_detail = detail;
                self.last_receipt_correlation_id = correlation_id;
                self.last_receipt_materialized_session_id = materialized_session_id;
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Pending
        }

        transition RecordReceiptClaimed {
            on input RecordReceipt { correlation_id, detail, materialized_session_id, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Claimed
                && self.receipt_stage != None
                && self.receipt_recorded_at_utc_ms != None
                && self.receipt_detail == detail
                && self.delivery_correlation_id == correlation_id
                && self.target_materialized_session_id == materialized_session_id
            }
            update {
                self.last_receipt_recorded_at_utc_ms = self.receipt_recorded_at_utc_ms;
                self.last_receipt_attempt = Some(self.attempt_count);
                self.last_receipt_stage = self.receipt_stage;
                self.last_receipt_failure_class = self.receipt_failure_class;
                self.last_receipt_detail = detail;
                self.last_receipt_correlation_id = correlation_id;
                self.last_receipt_materialized_session_id = materialized_session_id;
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Claimed
        }

        transition RecordReceiptDispatching {
            on input RecordReceipt { correlation_id, detail, materialized_session_id, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Dispatching
                && self.receipt_stage != None
                && self.receipt_recorded_at_utc_ms != None
                && self.receipt_detail == detail
                && self.delivery_correlation_id == correlation_id
                && self.target_materialized_session_id == materialized_session_id
            }
            update {
                self.last_receipt_recorded_at_utc_ms = self.receipt_recorded_at_utc_ms;
                self.last_receipt_attempt = Some(self.attempt_count);
                self.last_receipt_stage = self.receipt_stage;
                self.last_receipt_failure_class = self.receipt_failure_class;
                self.last_receipt_detail = detail;
                self.last_receipt_correlation_id = correlation_id;
                self.last_receipt_materialized_session_id = materialized_session_id;
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Dispatching
        }

        transition RecordReceiptAwaitingCompletion {
            on input RecordReceipt { correlation_id, detail, materialized_session_id, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::AwaitingCompletion
                && self.receipt_stage != None
                && self.receipt_recorded_at_utc_ms != None
                && self.receipt_detail == detail
                && self.delivery_correlation_id == correlation_id
                && self.target_materialized_session_id == materialized_session_id
            }
            update {
                self.last_receipt_recorded_at_utc_ms = self.receipt_recorded_at_utc_ms;
                self.last_receipt_attempt = Some(self.attempt_count);
                self.last_receipt_stage = self.receipt_stage;
                self.last_receipt_failure_class = self.receipt_failure_class;
                self.last_receipt_detail = detail;
                self.last_receipt_correlation_id = correlation_id;
                self.last_receipt_materialized_session_id = materialized_session_id;
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to AwaitingCompletion
        }

        transition RecordReceiptCompleted {
            on input RecordReceipt { correlation_id, detail, materialized_session_id, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Completed
                && self.receipt_stage != None
                && self.receipt_recorded_at_utc_ms != None
                && self.receipt_detail == detail
                && self.delivery_correlation_id == correlation_id
                && self.target_materialized_session_id == materialized_session_id
            }
            update {
                self.last_receipt_recorded_at_utc_ms = self.receipt_recorded_at_utc_ms;
                self.last_receipt_attempt = Some(self.attempt_count);
                self.last_receipt_stage = self.receipt_stage;
                self.last_receipt_failure_class = self.receipt_failure_class;
                self.last_receipt_detail = detail;
                self.last_receipt_correlation_id = correlation_id;
                self.last_receipt_materialized_session_id = materialized_session_id;
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Completed
        }

        transition RecordReceiptSkipped {
            on input RecordReceipt { correlation_id, detail, materialized_session_id, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Skipped
                && self.receipt_stage != None
                && self.receipt_recorded_at_utc_ms != None
                && self.receipt_detail == detail
                && self.delivery_correlation_id == correlation_id
                && self.target_materialized_session_id == materialized_session_id
            }
            update {
                self.last_receipt_recorded_at_utc_ms = self.receipt_recorded_at_utc_ms;
                self.last_receipt_attempt = Some(self.attempt_count);
                self.last_receipt_stage = self.receipt_stage;
                self.last_receipt_failure_class = self.receipt_failure_class;
                self.last_receipt_detail = detail;
                self.last_receipt_correlation_id = correlation_id;
                self.last_receipt_materialized_session_id = materialized_session_id;
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Skipped
        }

        transition RecordReceiptMisfired {
            on input RecordReceipt { correlation_id, detail, materialized_session_id, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Misfired
                && self.receipt_stage != None
                && self.receipt_recorded_at_utc_ms != None
                && self.receipt_detail == detail
                && self.delivery_correlation_id == correlation_id
                && self.target_materialized_session_id == materialized_session_id
            }
            update {
                self.last_receipt_recorded_at_utc_ms = self.receipt_recorded_at_utc_ms;
                self.last_receipt_attempt = Some(self.attempt_count);
                self.last_receipt_stage = self.receipt_stage;
                self.last_receipt_failure_class = self.receipt_failure_class;
                self.last_receipt_detail = detail;
                self.last_receipt_correlation_id = correlation_id;
                self.last_receipt_materialized_session_id = materialized_session_id;
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Misfired
        }

        transition RecordReceiptSuperseded {
            on input RecordReceipt { correlation_id, detail, materialized_session_id, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Superseded
                && self.receipt_stage != None
                && self.receipt_recorded_at_utc_ms != None
                && self.receipt_detail == detail
                && self.delivery_correlation_id == correlation_id
                && self.target_materialized_session_id == materialized_session_id
            }
            update {
                self.last_receipt_recorded_at_utc_ms = self.receipt_recorded_at_utc_ms;
                self.last_receipt_attempt = Some(self.attempt_count);
                self.last_receipt_stage = self.receipt_stage;
                self.last_receipt_failure_class = self.receipt_failure_class;
                self.last_receipt_detail = detail;
                self.last_receipt_correlation_id = correlation_id;
                self.last_receipt_materialized_session_id = materialized_session_id;
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Superseded
        }

        transition RecordReceiptDeliveryFailed {
            on input RecordReceipt { correlation_id, detail, materialized_session_id, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::DeliveryFailed
                && self.receipt_stage != None
                && self.receipt_recorded_at_utc_ms != None
                && self.receipt_detail == detail
                && self.delivery_correlation_id == correlation_id
                && self.target_materialized_session_id == materialized_session_id
            }
            update {
                self.last_receipt_recorded_at_utc_ms = self.receipt_recorded_at_utc_ms;
                self.last_receipt_attempt = Some(self.attempt_count);
                self.last_receipt_stage = self.receipt_stage;
                self.last_receipt_failure_class = self.receipt_failure_class;
                self.last_receipt_detail = detail;
                self.last_receipt_correlation_id = correlation_id;
                self.last_receipt_materialized_session_id = materialized_session_id;
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to DeliveryFailed
        }

        // --- Claim ---

        transition ClaimPending {
            on input Claim { owner_id, at_utc_ms, lease_expires_at_utc_ms, claim_token }
            guard {
                self.lifecycle_phase == Phase::Pending
                && self.due_at_utc_ms <= at_utc_ms
                && at_utc_ms <= self.misfire_deadline_utc_ms
            }
            update {
                self.claimed_by = Some(owner_id);
                self.lease_expires_at_utc_ms = Some(lease_expires_at_utc_ms);
                self.claimed_at_utc_ms = Some(at_utc_ms);
                self.claim_token = Some(claim_token);
                self.delivery_correlation_id = None;
                self.receipt_recorded_at_utc_ms = None;
                self.last_receipt_recorded_at_utc_ms = None;
                self.last_receipt_attempt = None;
                self.last_receipt_stage = None;
                self.last_receipt_failure_class = None;
                self.last_receipt_detail = None;
                self.last_receipt_correlation_id = None;
                self.last_receipt_materialized_session_id = None;
                self.runtime_outcome_key = None;
                self.receipt_stage = None;
                self.receipt_failure_class = None;
                self.receipt_detail = None;
                self.failure_class = None;
                self.failure_detail = None;
                self.dispatched_at_utc_ms = None;
                self.completed_at_utc_ms = None;
                self.attempt_count += 1;
            }
            to Claimed
            emit Claimed
        }

        // --- Dispatch ---

        transition DispatchStartedFromClaimed {
            on input DispatchStarted { correlation_id, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed }
            update {
                self.delivery_correlation_id = correlation_id;
                self.dispatched_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DispatchStarted);
                self.receipt_failure_class = None;
                self.receipt_detail = None;
            }
            to Dispatching
            emit DispatchStarted
        }

        // --- Await completion ---

        transition AwaitCompletionFromDispatching {
            on input AwaitCompletion { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching }
            update {
                self.dispatched_at_utc_ms = Some(at_utc_ms);
            }
            to AwaitingCompletion
            emit AwaitingCompletion
        }

        // 0.7.2 D2a: a Supersede can land between the driver's
        // DispatchStarted commit and its AwaitCompletion commit (schedule
        // delete/update revokes the in-flight claim at commit time). The
        // dispatch already happened; the await marker on an
        // already-superseded occurrence is a benign no-op, not a guard
        // rejection. The waiter still runs and its resolution lands as a
        // typed late-arrival record below.
        transition AwaitCompletionAfterSupersession {
            on input AwaitCompletion { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            update {}
            to Superseded
        }

        // --- Complete ---

        transition CompleteFromDispatchingOrAwaiting {
            on input Complete { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            update {
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::Completed);
                self.receipt_failure_class = None;
                self.receipt_detail = None;
            }
            to Completed
            emit Completed
        }

        transition RuntimeCompletionCompleted {
            on input ResolveRuntimeCompletion { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "runtime_outcome_completed" { outcome == RuntimeCompletionOutcome::Completed }
            update {
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::Completed);
                self.receipt_failure_class = None;
                self.receipt_detail = None;
            }
            to Completed
            emit Completed
        }

        transition RuntimeCompletionRuntimeRejected {
            on input ResolveRuntimeCompletion { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "runtime_outcome_rejected" {
                outcome == RuntimeCompletionOutcome::CallbackPending
                || outcome == RuntimeCompletionOutcome::Cancelled
                || outcome == RuntimeCompletionOutcome::Abandoned
            }
            update {
                self.failure_class = Some(OccurrenceFailureClass::RuntimeRejected);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::RuntimeRejected);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition RuntimeCompletionTransportError {
            on input ResolveRuntimeCompletion { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "runtime_outcome_transport_error" { outcome == RuntimeCompletionOutcome::RuntimeTerminated }
            update {
                self.failure_class = Some(OccurrenceFailureClass::TransportError);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::TransportError);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition RuntimeCompletionInternalError {
            on input ResolveRuntimeCompletion { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "runtime_outcome_internal_error" { outcome == RuntimeCompletionOutcome::FinalizationFailed }
            update {
                self.failure_class = Some(OccurrenceFailureClass::InternalError);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::InternalError);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition DeliveryCompletionFailureTransportError {
            on input ResolveDeliveryCompletionFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "completion_future_failed" {
                reason == DeliveryCompletionFailureReason::CompletionFutureFailed
            }
            update {
                self.failure_class = Some(OccurrenceFailureClass::TransportError);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::TransportError);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition DeliveryCompletionFailureInternalError {
            on input ResolveDeliveryCompletionFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "runtime_completion_authority_absent" {
                reason == DeliveryCompletionFailureReason::RuntimeCompletionChannelClosed
                || reason == DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable
                || reason == DeliveryCompletionFailureReason::RuntimeCompletionHandleMissing
            }
            update {
                self.failure_class = Some(OccurrenceFailureClass::InternalError);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::InternalError);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition DeliveryFailureTargetMaterializationFailed {
            on input ResolveDeliveryFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed || self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "target_materialization_failed" { reason == DeliveryFailureReason::TargetMaterializationFailed }
            update {
                self.failure_class = Some(OccurrenceFailureClass::TargetMaterializationFailed);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::TargetMaterializationFailed);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition DeliveryFailureTargetMissing {
            on input ResolveDeliveryFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed || self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "target_missing" { reason == DeliveryFailureReason::TargetMissing }
            update {
                self.failure_class = Some(OccurrenceFailureClass::TargetMissing);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::TargetMissing);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition DeliveryFailureTargetBusy {
            on input ResolveDeliveryFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed || self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "target_busy" { reason == DeliveryFailureReason::TargetBusy }
            update {
                self.failure_class = Some(OccurrenceFailureClass::TargetBusy);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::TargetBusy);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition DeliveryFailureRuntimeRejected {
            on input ResolveDeliveryFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed || self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "runtime_rejected" { reason == DeliveryFailureReason::RuntimeRejected }
            update {
                self.failure_class = Some(OccurrenceFailureClass::RuntimeRejected);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::RuntimeRejected);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition DeliveryFailureMobRejected {
            on input ResolveDeliveryFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed || self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "mob_rejected" { reason == DeliveryFailureReason::MobRejected }
            update {
                self.failure_class = Some(OccurrenceFailureClass::MobRejected);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::MobRejected);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition DeliveryFailureTransportError {
            on input ResolveDeliveryFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed || self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "transport_error" { reason == DeliveryFailureReason::TransportError }
            update {
                self.failure_class = Some(OccurrenceFailureClass::TransportError);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::TransportError);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        transition DeliveryFailureInternalError {
            on input ResolveDeliveryFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed || self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            guard "internal_error" { reason == DeliveryFailureReason::InternalError }
            update {
                self.failure_class = Some(OccurrenceFailureClass::InternalError);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(OccurrenceFailureClass::InternalError);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
        }

        // --- Target probe resolution ---
        //
        // Shells report typed target observations; occurrence authority owns
        // the policy decision that turns them into delivery admission,
        // skipped terminality, or misfired terminality.

        transition TargetProbeReadyClaimed {
            on input ResolveTargetProbe { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed }
            guard "ready" { outcome == OccurrenceTargetProbeOutcome::Ready }
            to Claimed
        }

        transition TargetProbeBusyAllowedByPolicy {
            on input ResolveTargetProbe { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed }
            guard "busy" { outcome == OccurrenceTargetProbeOutcome::Busy }
            guard "allow_concurrent" { self.overlap_policy == OverlapPolicy::AllowConcurrent }
            to Claimed
        }

        transition TargetProbeBusySkipByPolicy {
            on input ResolveTargetProbe { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed }
            guard "busy" { outcome == OccurrenceTargetProbeOutcome::Busy }
            guard "skip_if_running" { self.overlap_policy == OverlapPolicy::SkipIfRunning }
            update {
                self.failure_detail = detail;
                self.failure_class = Some(OccurrenceFailureClass::TargetBusy);
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::Skipped);
                self.receipt_failure_class = Some(OccurrenceFailureClass::TargetBusy);
                self.receipt_detail = detail;
                self.claimed_by = None;
                self.lease_expires_at_utc_ms = None;
                self.claim_token = None;
                self.delivery_correlation_id = None;
            }
            to Skipped
            emit Skipped
        }

        transition TargetProbeMissingSkipByPolicy {
            on input ResolveTargetProbe { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed }
            guard "missing" { outcome == OccurrenceTargetProbeOutcome::Missing }
            guard "skip_missing_target" { self.missing_target_policy == MissingTargetPolicy::Skip }
            update {
                self.failure_detail = detail;
                self.failure_class = Some(OccurrenceFailureClass::TargetMissing);
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::Skipped);
                self.receipt_failure_class = Some(OccurrenceFailureClass::TargetMissing);
                self.receipt_detail = detail;
                self.claimed_by = None;
                self.lease_expires_at_utc_ms = None;
                self.claim_token = None;
                self.delivery_correlation_id = None;
            }
            to Skipped
            emit Skipped
        }

        transition TargetProbeMissingMisfireByPolicy {
            on input ResolveTargetProbe { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed }
            guard "missing" { outcome == OccurrenceTargetProbeOutcome::Missing }
            guard "mark_misfired_missing_target" { self.missing_target_policy == MissingTargetPolicy::MarkMisfired }
            update {
                self.failure_detail = detail;
                self.failure_class = Some(OccurrenceFailureClass::TargetMissing);
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::Misfired);
                self.receipt_failure_class = Some(OccurrenceFailureClass::TargetMissing);
                self.receipt_detail = detail;
                self.claimed_by = None;
                self.lease_expires_at_utc_ms = None;
                self.claim_token = None;
                self.delivery_correlation_id = None;
            }
            to Misfired
            emit Misfired
        }

        // --- Due misfire ---

        transition DueMisfirePending {
            on input ResolveDueMisfire { detail, at_utc_ms }
            guard {
                self.lifecycle_phase == Phase::Pending
                && self.due_at_utc_ms <= at_utc_ms
                && self.misfire_deadline_utc_ms < at_utc_ms
            }
            update {
                self.failure_detail = detail;
                self.failure_class = None;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::Misfired);
                self.receipt_failure_class = None;
                self.receipt_detail = detail;
                self.claimed_by = None;
                self.lease_expires_at_utc_ms = None;
                self.claim_token = None;
                self.delivery_correlation_id = None;
            }
            to Misfired
            emit Misfired
        }

        // --- Supersede ---

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
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::Superseded);
                self.receipt_failure_class = None;
                self.receipt_detail = None;
            }
            to Superseded
            emit Superseded
            emit OccurrencesSuperseded { occurrence_id: self.occurrence_id, superseding_revision: superseded_by_revision }
        }

        // 0.7.2 D2a: idempotent no-op (precedent: DeleteDeleted in
        // ScheduleLifecycleMachine). A driver's supersession verdict can race
        // the schedule-commit supersession sweep; the second Supersede landing
        // on an already-superseded occurrence is a benign duplicate. The first
        // recorded supersession wins — state unchanged, zero effects, so the
        // shell mints no duplicate receipt for it.
        transition SupersedeAlreadySuperseded {
            on input Supersede { superseded_by_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            update {}
            to Superseded
        }

        // --- Late delivery resolutions after supersession (0.7.2 D2a) ---
        //
        // Schedule delete (and revision-bumping update) revokes in-flight
        // claims by superseding the occurrence at commit time. The completion
        // waiter spawned for the dispatched delivery cannot be quiesced by
        // that commit — it legitimately resolves afterwards. These inputs are
        // therefore total in Superseded: each self-loops, leaves the recorded
        // supersession (phase, receipt authority, claim evidence,
        // superseded_by_revision) untouched, and records what the delivery
        // actually did as the typed late-arrival fact. "Not allowed" stays
        // meaningful everywhere else: the same inputs in Completed / Skipped /
        // Misfired / DeliveryFailed are still refused and classified
        // NotLiveForTerminal.

        transition LateCompleteAfterSupersession {
            on input Complete { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            update {
                self.late_completion_recorded_at_utc_ms = Some(at_utc_ms);
                self.late_completion_resolution = Some(LateCompletionResolutionClass::DeliveryCompleted);
                self.late_completion_detail = None;
            }
            to Superseded
            emit LateCompletionResolutionRecorded {
                resolution: LateCompletionResolutionClass::DeliveryCompleted
            }
        }

        transition LateRuntimeCompletionCompletedAfterSupersession {
            on input ResolveRuntimeCompletion { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            guard "runtime_outcome_completed" { outcome == RuntimeCompletionOutcome::Completed }
            update {
                self.late_completion_recorded_at_utc_ms = Some(at_utc_ms);
                self.late_completion_resolution = Some(LateCompletionResolutionClass::RuntimeCompleted);
                self.late_completion_detail = detail;
            }
            to Superseded
            emit LateCompletionResolutionRecorded {
                resolution: LateCompletionResolutionClass::RuntimeCompleted
            }
        }

        transition LateRuntimeCompletionRejectedAfterSupersession {
            on input ResolveRuntimeCompletion { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            guard "runtime_outcome_rejected" {
                outcome == RuntimeCompletionOutcome::CallbackPending
                || outcome == RuntimeCompletionOutcome::Cancelled
                || outcome == RuntimeCompletionOutcome::Abandoned
            }
            update {
                self.late_completion_recorded_at_utc_ms = Some(at_utc_ms);
                self.late_completion_resolution = Some(LateCompletionResolutionClass::RuntimeRejected);
                self.late_completion_detail = detail;
            }
            to Superseded
            emit LateCompletionResolutionRecorded {
                resolution: LateCompletionResolutionClass::RuntimeRejected
            }
        }

        transition LateRuntimeCompletionTransportErrorAfterSupersession {
            on input ResolveRuntimeCompletion { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            guard "runtime_outcome_transport_error" { outcome == RuntimeCompletionOutcome::RuntimeTerminated }
            update {
                self.late_completion_recorded_at_utc_ms = Some(at_utc_ms);
                self.late_completion_resolution = Some(LateCompletionResolutionClass::RuntimeTransportError);
                self.late_completion_detail = detail;
            }
            to Superseded
            emit LateCompletionResolutionRecorded {
                resolution: LateCompletionResolutionClass::RuntimeTransportError
            }
        }

        transition LateRuntimeCompletionInternalErrorAfterSupersession {
            on input ResolveRuntimeCompletion { outcome, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            guard "runtime_outcome_internal_error" { outcome == RuntimeCompletionOutcome::FinalizationFailed }
            update {
                self.late_completion_recorded_at_utc_ms = Some(at_utc_ms);
                self.late_completion_resolution = Some(LateCompletionResolutionClass::RuntimeInternalError);
                self.late_completion_detail = detail;
            }
            to Superseded
            emit LateCompletionResolutionRecorded {
                resolution: LateCompletionResolutionClass::RuntimeInternalError
            }
        }

        transition LateDeliveryCompletionFailureAfterSupersession {
            on input ResolveDeliveryCompletionFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            update {
                self.late_completion_recorded_at_utc_ms = Some(at_utc_ms);
                self.late_completion_resolution = Some(LateCompletionResolutionClass::DeliveryCompletionFailed);
                self.late_completion_detail = detail;
            }
            to Superseded
            emit LateCompletionResolutionRecorded {
                resolution: LateCompletionResolutionClass::DeliveryCompletionFailed
            }
        }

        transition LateDeliveryFailureAfterSupersession {
            on input ResolveDeliveryFailure { reason, detail, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            update {
                self.late_completion_recorded_at_utc_ms = Some(at_utc_ms);
                self.late_completion_resolution = Some(LateCompletionResolutionClass::DeliveryFailed);
                self.late_completion_detail = detail;
            }
            to Superseded
            emit LateCompletionResolutionRecorded {
                resolution: LateCompletionResolutionClass::DeliveryFailed
            }
        }

        // --- Lease expired (one per source phase, returns to Pending) ---

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
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::LeaseExpired);
                self.receipt_failure_class = Some(OccurrenceFailureClass::LeaseLost);
                self.receipt_detail = Some("lease expired before completion");
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
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::LeaseExpired);
                self.receipt_failure_class = Some(OccurrenceFailureClass::LeaseLost);
                self.receipt_detail = Some("lease expired before completion");
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
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::LeaseExpired);
                self.receipt_failure_class = Some(OccurrenceFailureClass::LeaseLost);
                self.receipt_detail = Some("lease expired before completion");
            }
            to Pending
            emit LeaseExpired
        }

        transition ReleaseLeaseForPausedScheduleFromClaimed {
            on input ReleaseLeaseForPausedSchedule { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Claimed }
            update {
                self.claimed_by = None;
                self.lease_expires_at_utc_ms = None;
                self.claim_token = None;
                self.delivery_correlation_id = None;
                self.claimed_at_utc_ms = None;
                self.dispatched_at_utc_ms = None;
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::LeaseExpired);
                self.receipt_failure_class = Some(OccurrenceFailureClass::LeaseLost);
                self.receipt_detail = Some("lease released because schedule was paused before dispatch");
            }
            to Pending
            emit LeaseExpired
        }

        transition ReleaseLeaseForPausedScheduleFromDispatching {
            on input ReleaseLeaseForPausedSchedule { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching }
            update {
                self.claimed_by = None;
                self.lease_expires_at_utc_ms = None;
                self.claim_token = None;
                self.delivery_correlation_id = None;
                self.claimed_at_utc_ms = None;
                self.dispatched_at_utc_ms = None;
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::LeaseExpired);
                self.receipt_failure_class = Some(OccurrenceFailureClass::LeaseLost);
                self.receipt_detail = Some("lease released because schedule was paused before dispatch");
            }
            to Pending
            emit LeaseExpired
        }

        transition ReleaseLeaseForPausedScheduleFromAwaitingCompletion {
            on input ReleaseLeaseForPausedSchedule { at_utc_ms }
            guard { self.lifecycle_phase == Phase::AwaitingCompletion }
            update {
                self.claimed_by = None;
                self.lease_expires_at_utc_ms = None;
                self.claim_token = None;
                self.delivery_correlation_id = None;
                self.claimed_at_utc_ms = None;
                self.dispatched_at_utc_ms = None;
                self.receipt_recorded_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::LeaseExpired);
                self.receipt_failure_class = Some(OccurrenceFailureClass::LeaseLost);
                self.receipt_detail = Some("lease released because schedule was paused before dispatch");
            }
            to Pending
            emit LeaseExpired
        }

        // 0.7.2 D2a: the driver's Frozen disposition (observed under a paused
        // schedule) can race a delete/update supersession that lands before
        // the lease release applies. Releasing the lease of an
        // already-superseded occurrence is a benign no-op — the supersession
        // already revoked the claim's meaning. State unchanged, zero effects,
        // so the shell mints no lease-expired receipt for it.
        transition ReleaseLeaseForPausedScheduleAfterSupersession {
            on input ReleaseLeaseForPausedSchedule { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Superseded }
            update {}
            to Superseded
        }
    }
}

// Stub types for compilation — in the real port these would come from meerkat-schedule
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceId(pub String);

/// Typed trigger identity an occurrence was planned from. String-backed
/// newtype so the machine cannot confuse it with policy keys or binding ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerKey(pub String);
impl<T: Into<String>> From<T> for TriggerKey {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Typed target-binding identity that determines delivery target resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetBindingId(pub String);
impl<T: Into<String>> From<T> for TargetBindingId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Typed claim-owner identity that determines lease ownership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimOwner(pub String);
impl<T: Into<String>> From<T> for ClaimOwner {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Typed delivery correlation identity that ties receipts to dispatches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrelationId(pub String);
impl<T: Into<String>> From<T> for CorrelationId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}
impl<T: Into<String>> From<T> for OccurrenceId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleId(pub String);
impl<T: Into<String>> From<T> for ScheduleId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimToken(pub String);
impl<T: Into<String>> From<T> for ClaimToken {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(pub String);
impl<T: Into<String>> From<T> for SessionId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Typed runtime-outcome receipt key (UUID-backed at the shell seam). Ties an
/// occurrence's recorded receipt to the runtime completion outcome it
/// projects; never a raw `String` the machine could confuse with detail text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOutcomeKey(pub String);
impl<T: Into<String>> From<T> for RuntimeOutcomeKey {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceFailureClass {
    TargetMaterializationFailed,
    TargetMissing,
    TargetBusy,
    RuntimeRejected,
    MobRejected,
    LeaseLost,
    TransportError,
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeCompletionOutcome {
    Completed,
    CallbackPending,
    Cancelled,
    Abandoned,
    FinalizationFailed,
    RuntimeTerminated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryCompletionFailureReason {
    CompletionFutureFailed,
    RuntimeCompletionChannelClosed,
    RuntimeCompletionAuthorityUnavailable,
    RuntimeCompletionHandleMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryFailureReason {
    TargetMaterializationFailed,
    TargetMissing,
    TargetBusy,
    RuntimeRejected,
    MobRejected,
    TransportError,
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceTargetProbeOutcome {
    Ready,
    Busy,
    Missing,
}

/// Pure observation of the owning schedule's lifecycle phase, extracted by the
/// driver shell and fed to the occurrence authority during claimed-occurrence
/// pre-dispatch reconciliation. Mirrors the schedule-domain `SchedulePhase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimedDispatchSchedulePhase {
    Active,
    Paused,
    Deleted,
}

/// Machine-owned disposition verdict for a claimed occurrence awaiting
/// dispatch. The driver shell mirrors this rather than deciding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimedDispatchDisposition {
    /// Schedule is paused: release the lease and leave the occurrence pending.
    Frozen,
    /// Schedule was deleted or the claim is for a stale revision: terminalize
    /// the occurrence as superseded by `superseded_by_revision`.
    Supersede,
    /// Schedule is active and the claimed revision is current: dispatch.
    Ready,
    /// The claimed revision is ahead of the schedule's current revision — an
    /// impossible/corrupt fact the driver surfaces as an internal error.
    FutureRevision,
}

/// Machine-owned disposition verdict for a dispatched occurrence whose delivery
/// has completed. The driver shell mirrors this rather than deciding it. Unlike
/// the pre-dispatch disposition, a paused schedule does not supersede an
/// already-completed delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionSupersessionDisposition {
    /// Schedule was deleted or the claim is for a revision behind the
    /// schedule's current revision: terminalize the completed occurrence as
    /// superseded by `superseded_by_revision`.
    Supersede,
    /// Terminalize the completed occurrence on its delivery result.
    Proceed,
    /// The occurrence is already Superseded (the schedule-commit supersession
    /// sweep landed between dispatch and completion). The driver delivers the
    /// completion outcome as a typed late-arrival record; no fresh terminal
    /// transition or receipt is minted.
    AlreadySuperseded,
}

/// Typed class of a delivery resolution that arrived after the occurrence
/// was superseded (0.7.2 D2a). Mirrors the outcome classes the live terminal
/// transitions distinguish, so a superseded occurrence still records what its
/// dispatched delivery actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LateCompletionResolutionClass {
    /// `Complete` — the delivery finished successfully.
    DeliveryCompleted,
    /// `ResolveRuntimeCompletion` with `Completed`.
    RuntimeCompleted,
    /// `ResolveRuntimeCompletion` with `CallbackPending`/`Cancelled`/`Abandoned`.
    RuntimeRejected,
    /// `ResolveRuntimeCompletion` with `RuntimeTerminated`.
    RuntimeTransportError,
    /// `ResolveRuntimeCompletion` with `FinalizationFailed`.
    RuntimeInternalError,
    /// `ResolveDeliveryCompletionFailure` — the completion future failed.
    DeliveryCompletionFailed,
    /// `ResolveDeliveryFailure` — the delivery adapter reported failure.
    DeliveryFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryReceiptStage {
    Planned,
    Claimed,
    DispatchStarted,
    DispatchAccepted,
    AwaitingCompletion,
    Completed,
    Skipped,
    Misfired,
    Superseded,
    DeliveryFailed,
    LeaseExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceTransitionFailureRefusalKind {
    NoMatchingTransition,
    GuardRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceTransitionFailureClassKind {
    PlanRejected,
    TargetSyncRejected,
    ReceiptRecordRejected,
    DueClassificationRejected,
    ClaimedDispatchClassificationRejected,
    CompletionSupersessionClassificationRejected,
    ClaimRejected,
    NotPendingForClaim,
    NotClaimed,
    NotDispatching,
    NotLeaseHolding,
    NotLiveForTerminal,
    /// `ClassifyStaleCompletionArrival` was refused (its trigger was not a
    /// completion-shaped input variant).
    StaleCompletionArrivalClassificationRejected,
}
