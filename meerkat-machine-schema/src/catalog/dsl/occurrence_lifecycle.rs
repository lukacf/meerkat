use meerkat_machine_dsl::machine;

machine! {
    machine OccurrenceLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::occurrence_lifecycle",

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
            // Reciprocal-ack effect (wave-d D-f): after the occurrence
            // absorbs a Supersede it reports the superseding revision and
            // its own occurrence_id back to the schedule authority so the
            // schedule side observes which occurrences it actually
            // superseded (instead of firing a one-way Supersede and
            // hoping).
            OccurrencesSuperseded { occurrence_id: OccurrenceId, superseding_revision: u64 },
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
        disposition OccurrencesSuperseded => routed [ScheduleLifecycleMachine],
        disposition DeliveryFailed => external,
        disposition LeaseExpired => external,

        // --- Claim ---

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

        // --- Dispatch ---

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
            on input Complete { receipt, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Dispatching || self.lifecycle_phase == Phase::AwaitingCompletion }
            update {
                self.last_receipt = Some(receipt);
                self.completed_at_utc_ms = Some(at_utc_ms);
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
            }
            to Superseded
            emit Superseded
            emit OccurrencesSuperseded { occurrence_id: self.occurrence_id, superseding_revision: superseded_by_revision }
        }

        // --- Delivery failed ---

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OccurrenceFailureClass {
    Timeout,
    TargetUnavailable,
    InternalError,
}
