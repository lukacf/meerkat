use super::OptionValueExt;
use meerkat_machine_dsl::machine;

machine! {
    machine OccurrenceLifecycleMachine {
        version: 2,
        rust: "self" / "catalog::dsl::occurrence_lifecycle",

        state {
            lifecycle_phase: OccurrenceLifecycleState,
            occurrence_id: OccurrenceId,
            schedule_id: ScheduleId,
            schedule_revision: u64,
            occurrence_ordinal: u64,
            trigger_key: String,
            target_binding_key: String,
            misfire_policy: Enum<MisfirePolicy>,
            misfire_policy_key: String,
            overlap_policy: Enum<OverlapPolicy>,
            overlap_policy_key: String,
            missing_target_policy: Enum<MissingTargetPolicy>,
            missing_target_policy_key: String,
            due_at_utc_ms: u64,
            misfire_deadline_utc_ms: u64,
            claimed_by: Option<String>,
            lease_expires_at_utc_ms: Option<u64>,
            claimed_at_utc_ms: Option<u64>,
            claim_token: Option<ClaimToken>,
            delivery_correlation_id: Option<String>,
            last_receipt: Option<DeliveryReceipt>,
            runtime_outcome_key: Option<String>,
            receipt_stage: Option<Enum<DeliveryReceiptStage>>,
            receipt_failure_class: Option<Enum<OccurrenceFailureClass>>,
            receipt_detail: Option<String>,
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
            last_receipt = None,
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
                trigger_key: String,
                target_binding_key: String,
                misfire_policy: Enum<MisfirePolicy>,
                misfire_policy_key: String,
                overlap_policy: Enum<OverlapPolicy>,
                overlap_policy_key: String,
                missing_target_policy: Enum<MissingTargetPolicy>,
                missing_target_policy_key: String,
                due_at_utc_ms: u64,
                misfire_deadline_utc_ms: u64,
            },
            SyncTargetSnapshot { target_binding_key: String },
            RecordReceipt {
                receipt: DeliveryReceipt,
                stage: Enum<DeliveryReceiptStage>,
                failure_class: Option<Enum<OccurrenceFailureClass>>,
                detail: Option<String>,
                runtime_outcome_key: Option<String>
            },
            ClassifyDue { now_utc_ms: u64 },
            Claim { owner_id: String, at_utc_ms: u64, lease_expires_at_utc_ms: u64, claim_token: ClaimToken },
            DispatchStarted { correlation_id: Option<String>, at_utc_ms: u64 },
            AwaitCompletion { at_utc_ms: u64 },
            Complete { at_utc_ms: u64 },
            Skip { detail: Option<String>, failure_class: Option<Enum<OccurrenceFailureClass>>, at_utc_ms: u64 },
            Misfire { detail: Option<String>, failure_class: Option<Enum<OccurrenceFailureClass>>, at_utc_ms: u64 },
            Supersede { superseded_by_revision: u64, at_utc_ms: u64 },
            DeliveryFailed { failure_class: Enum<OccurrenceFailureClass>, detail: Option<String>, at_utc_ms: u64 },
            LeaseExpired { at_utc_ms: u64 },
            ReleaseLeaseForPausedSchedule { at_utc_ms: u64 },
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

        invariant misfire_deadline_not_before_due {
            self.misfire_deadline_utc_ms >= self.due_at_utc_ms
        }

        disposition Claimed => external,
        disposition DispatchStarted => external,
        disposition AwaitingCompletion => external,
        disposition Completed => external,
        disposition Skipped => external,
        disposition Misfired => external,
        disposition Superseded => external,
        disposition OccurrencesSuperseded => routed [ScheduleLifecycleMachine],
        disposition DueNoAction => local,
        disposition DueClaimEligible => local,
        disposition DueMisfireRequired => local,
        disposition DueLeaseExpired => local,
        disposition DeliveryFailed => external,
        disposition LeaseExpired => external,

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
                due_at_utc_ms,
                misfire_deadline_utc_ms
            }
            guard {
                self.lifecycle_phase == Phase::Pending
                && self.attempt_count == 0
                && self.claimed_by == None
                && self.claim_token == None
                && self.delivery_correlation_id == None
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
                self.due_at_utc_ms = due_at_utc_ms;
                self.misfire_deadline_utc_ms = misfire_deadline_utc_ms;
                self.claimed_by = None;
                self.lease_expires_at_utc_ms = None;
                self.claimed_at_utc_ms = None;
                self.claim_token = None;
                self.delivery_correlation_id = None;
                self.last_receipt = None;
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

        // --- Target snapshot sync ---
        //
        // Materialized session binding changes the dispatch target. Keep that
        // target-binding fact behind generated occurrence authority; the shell
        // writes the full target snapshot only after this input is accepted.

        transition SyncTargetSnapshotPending {
            on input SyncTargetSnapshot { target_binding_key }
            guard { self.lifecycle_phase == Phase::Pending }
            update {
                self.target_binding_key = target_binding_key;
            }
            to Pending
        }

        transition SyncTargetSnapshotClaimed {
            on input SyncTargetSnapshot { target_binding_key }
            guard { self.lifecycle_phase == Phase::Claimed }
            update {
                self.target_binding_key = target_binding_key;
            }
            to Claimed
        }

        // --- Receipt/result projection ---

        transition RecordReceiptPending {
            on input RecordReceipt { receipt, stage, failure_class, detail, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Pending
                && self.receipt_stage == Some(stage)
                && self.receipt_failure_class == failure_class
                && self.receipt_detail == detail
            }
            update {
                self.last_receipt = Some(receipt);
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Pending
        }

        transition RecordReceiptClaimed {
            on input RecordReceipt { receipt, stage, failure_class, detail, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Claimed
                && self.receipt_stage == Some(stage)
                && self.receipt_failure_class == failure_class
                && self.receipt_detail == detail
            }
            update {
                self.last_receipt = Some(receipt);
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Claimed
        }

        transition RecordReceiptDispatching {
            on input RecordReceipt { receipt, stage, failure_class, detail, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Dispatching
                && self.receipt_stage == Some(stage)
                && self.receipt_failure_class == failure_class
                && self.receipt_detail == detail
            }
            update {
                self.last_receipt = Some(receipt);
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Dispatching
        }

        transition RecordReceiptAwaitingCompletion {
            on input RecordReceipt { receipt, stage, failure_class, detail, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::AwaitingCompletion
                && self.receipt_stage == Some(stage)
                && self.receipt_failure_class == failure_class
                && self.receipt_detail == detail
            }
            update {
                self.last_receipt = Some(receipt);
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to AwaitingCompletion
        }

        transition RecordReceiptCompleted {
            on input RecordReceipt { receipt, stage, failure_class, detail, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Completed
                && self.receipt_stage == Some(stage)
                && self.receipt_failure_class == failure_class
                && self.receipt_detail == detail
            }
            update {
                self.last_receipt = Some(receipt);
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Completed
        }

        transition RecordReceiptSkipped {
            on input RecordReceipt { receipt, stage, failure_class, detail, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Skipped
                && self.receipt_stage == Some(stage)
                && self.receipt_failure_class == failure_class
                && self.receipt_detail == detail
            }
            update {
                self.last_receipt = Some(receipt);
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Skipped
        }

        transition RecordReceiptMisfired {
            on input RecordReceipt { receipt, stage, failure_class, detail, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Misfired
                && self.receipt_stage == Some(stage)
                && self.receipt_failure_class == failure_class
                && self.receipt_detail == detail
            }
            update {
                self.last_receipt = Some(receipt);
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Misfired
        }

        transition RecordReceiptSuperseded {
            on input RecordReceipt { receipt, stage, failure_class, detail, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::Superseded
                && self.receipt_stage == Some(stage)
                && self.receipt_failure_class == failure_class
                && self.receipt_detail == detail
            }
            update {
                self.last_receipt = Some(receipt);
                self.runtime_outcome_key = runtime_outcome_key;
            }
            to Superseded
        }

        transition RecordReceiptDeliveryFailed {
            on input RecordReceipt { receipt, stage, failure_class, detail, runtime_outcome_key }
            guard {
                self.lifecycle_phase == Phase::DeliveryFailed
                && self.receipt_stage == Some(stage)
                && self.receipt_failure_class == failure_class
                && self.receipt_detail == detail
            }
            update {
                self.last_receipt = Some(receipt);
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
                self.last_receipt = None;
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

        // --- Complete ---

        transition CompleteFromDispatchingOrAwaiting {
            on input Complete { at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            update {
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::Completed);
                self.receipt_failure_class = None;
                self.receipt_detail = None;
            }
            to Completed
            emit Completed
        }

        // --- Skip (from Pending or live claim phases) ---

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
                self.receipt_stage = Some(DeliveryReceiptStage::Skipped);
                self.receipt_failure_class = failure_class;
                self.receipt_detail = detail;
                self.claimed_by = None;
                self.lease_expires_at_utc_ms = None;
                self.claim_token = None;
                self.delivery_correlation_id = None;
            }
            to Skipped
            emit Skipped
        }

        // --- Misfire ---

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
                self.receipt_stage = Some(DeliveryReceiptStage::Misfired);
                self.receipt_failure_class = failure_class;
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
                self.receipt_stage = Some(DeliveryReceiptStage::Superseded);
                self.receipt_failure_class = None;
                self.receipt_detail = None;
            }
            to Superseded
            emit Superseded
            emit OccurrencesSuperseded { occurrence_id: self.occurrence_id, superseding_revision: superseded_by_revision }
        }

        // --- Delivery failed ---

        transition DeliveryFailedFromClaimedOrLive {
            on input DeliveryFailed { failure_class, detail, at_utc_ms }
            guard {
                self.lifecycle_phase == Phase::Claimed
                || self.lifecycle_phase == Phase::Dispatching
                || self.lifecycle_phase == Phase::AwaitingCompletion
            }
            update {
                self.failure_class = Some(failure_class);
                self.failure_detail = detail;
                self.completed_at_utc_ms = Some(at_utc_ms);
                self.receipt_stage = Some(DeliveryReceiptStage::DeliveryFailed);
                self.receipt_failure_class = Some(failure_class);
                self.receipt_detail = detail;
            }
            to DeliveryFailed
            emit DeliveryFailed
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
                self.receipt_stage = Some(DeliveryReceiptStage::LeaseExpired);
                self.receipt_failure_class = Some(OccurrenceFailureClass::LeaseLost);
                self.receipt_detail = Some("lease released because schedule was paused before dispatch");
            }
            to Pending
            emit LeaseExpired
        }
    }
}

// Stub types for compilation — in the real port these would come from meerkat-schedule
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceId(pub String);
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
pub struct DeliveryReceipt(pub String);
impl<T: Into<String>> From<T> for DeliveryReceipt {
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
