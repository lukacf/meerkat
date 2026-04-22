use meerkat_machine_dsl::machine;

machine! {
    machine OccurrenceLifecycleMachine {
        version: 1,
        rust: "meerkat-schedule" / "generated::occurrence_lifecycle",

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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Runtime dispatch tests ----

    #[test]
    fn initial_state_is_pending() {
        let auth = OccurrenceLifecycleMachineAuthority::new();
        assert_eq!(auth.state.phase(), OccurrenceLifecycleState::Pending);
    }

    #[test]
    fn full_happy_path() {
        let mut auth = OccurrenceLifecycleMachineAuthority::new();

        // Claim
        let r = OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::Claim {
                owner_id: "worker-1".into(),
                at_utc_ms: 1000,
                lease_expires_at_utc_ms: 2000,
                claim_token: ClaimToken::from("tok-1".to_string()),
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, OccurrenceLifecycleState::Claimed);
        assert_eq!(auth.state.attempt_count, 1);

        // Dispatch
        let r = OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-1".into()),
                at_utc_ms: 1100,
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, OccurrenceLifecycleState::Dispatching);

        // Await
        OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::AwaitCompletion { at_utc_ms: 1200 },
        )
        .unwrap();

        // Complete
        let r = OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::Complete {
                receipt: DeliveryReceipt::from("rcpt-1".to_string()),
                at_utc_ms: 1300,
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, OccurrenceLifecycleState::Completed);
    }

    #[test]
    fn skip_from_pending() {
        let mut auth = OccurrenceLifecycleMachineAuthority::new();
        let r = OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::Skip {
                detail: Some("not needed".into()),
                failure_class: None,
                at_utc_ms: 500,
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, OccurrenceLifecycleState::Skipped);
    }

    #[test]
    fn lease_expired_returns_to_pending() {
        let mut auth = OccurrenceLifecycleMachineAuthority::new();
        OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::Claim {
                owner_id: "w".into(),
                at_utc_ms: 1,
                lease_expires_at_utc_ms: 2,
                claim_token: ClaimToken::from("t".to_string()),
            },
        )
        .unwrap();

        let r = OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::LeaseExpired { at_utc_ms: 3 },
        )
        .unwrap();
        assert_eq!(r.to_phase, OccurrenceLifecycleState::Pending);
        assert_eq!(auth.state.claimed_by, None);
    }

    #[test]
    fn cannot_claim_from_completed() {
        let mut auth = OccurrenceLifecycleMachineAuthority::new();
        // Skip to terminal
        OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::Skip {
                detail: None,
                failure_class: None,
                at_utc_ms: 1,
            },
        )
        .unwrap();

        let r = OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::Claim {
                owner_id: "w".into(),
                at_utc_ms: 2,
                lease_expires_at_utc_ms: 3,
                claim_token: ClaimToken::from("t".to_string()),
            },
        );
        assert!(r.is_err());
    }

    // ---- Schema equivalence test ----

    #[test]
    fn schema_validates() {
        let schema = OccurrenceLifecycleMachineState::schema();
        schema
            .validate()
            .expect("occurrence lifecycle schema should validate");
    }

    // ---- TLA+ rendering ----

    #[test]
    fn schema_renders_tla() {
        let schema = OccurrenceLifecycleMachineState::schema();
        let tla = meerkat_machine_codegen::render_machine_module(&schema);
        assert!(tla.contains("OccurrenceLifecycleMachine"));
        assert!(tla.contains("ClaimPending"));
        assert!(tla.contains("LeaseExpiredFromClaimed"));
    }

    // ---- Kernel round-trip ----

    #[test]
    fn dsl_dispatch_matches_kernel() {
        let schema = OccurrenceLifecycleMachineState::schema();
        let kernel = meerkat_machine_kernels::test_oracle::GeneratedMachineKernel::new(schema);

        let mut auth = OccurrenceLifecycleMachineAuthority::new();
        let mut kernel_state = kernel.initial_state().unwrap();

        // Run Claim through both
        let dsl_r = OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::Claim {
                owner_id: "w".into(),
                at_utc_ms: 1,
                lease_expires_at_utc_ms: 2,
                claim_token: ClaimToken::from("t".to_string()),
            },
        )
        .unwrap();

        let kernel_input = meerkat_machine_kernels::test_oracle::KernelInput {
            variant: "Claim".into(),
            fields: std::collections::BTreeMap::from([
                (
                    "owner_id".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::String("w".into()),
                ),
                (
                    "at_utc_ms".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::U64(1),
                ),
                (
                    "lease_expires_at_utc_ms".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::U64(2),
                ),
                (
                    "claim_token".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::String("t".into()),
                ),
            ]),
        };
        let kernel_r = kernel.transition(&kernel_state, &kernel_input).unwrap();

        assert_eq!(format!("{:?}", dsl_r.to_phase), kernel_r.next_state.phase);
        assert_eq!(dsl_r.effects.len(), kernel_r.effects.len());

        kernel_state = kernel_r.next_state;

        // Run Skip through both
        let dsl_r = OccurrenceLifecycleMachineMutator::apply(
            &mut auth,
            OccurrenceLifecycleInput::Skip {
                detail: None,
                failure_class: None,
                at_utc_ms: 5,
            },
        );
        let kernel_input = meerkat_machine_kernels::test_oracle::KernelInput {
            variant: "Skip".into(),
            fields: std::collections::BTreeMap::from([
                (
                    "detail".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::None,
                ),
                (
                    "failure_class".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::None,
                ),
                (
                    "at_utc_ms".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::U64(5),
                ),
            ]),
        };
        let kernel_r = kernel.transition(&kernel_state, &kernel_input);

        assert_eq!(dsl_r.is_ok(), kernel_r.is_ok());
    }
}
