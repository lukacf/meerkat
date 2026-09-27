//! Terminal wait for one caller-identified delivery to a local member.
//!
//! Autonomous (inbox-driven) members cannot hand out a completion-bearing
//! admission handle: a delivery reaches the member's comms inbox first and
//! becomes a runtime input only when the member drains it. This module lets a
//! host observe that input's own terminal after the fact, keyed by the stable
//! [`MobDeliveryIdentity`](crate::store::MobDeliveryIdentity) the delivery
//! carried, without changing how it was delivered.
//!
//! Every fact comes from the member runtime's own input lifecycle: the
//! machine-owned key-to-input binding, the input's terminal receipt (the run
//! that answered it and every input that run answered), and the batch's
//! finalized outcome. The result is projected through the same bounded
//! projection as [`super::WorkTurnHandle::wait_bounded`]. Nothing here records
//! or decides a lifecycle fact, and no mob actor command is sent.

use super::handle::{
    BoundedResultSpec, BoundedTurnFailure, BoundedTurnResult, DurableBoundedMemberState,
};
use crate::ids::AgentIdentity;
use crate::store::MobStoreError;
use meerkat_core::lifecycle::{InputId, RunId as RuntimeRunId};
use meerkat_runtime::terminal_status::TerminalWitnessSource;
use meerkat_runtime::{InputLifecycleState, InputTerminalOutcome, RuntimeDriverError};

/// Why a delivery could not be matched to an admitted runtime input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeliveryUnknownCause {
    /// The member's runtime held no input for the delivery identity when the
    /// deadline elapsed. A returned send only proves the delivery reached the
    /// member's inbox; resending with the same identity is safe, because the
    /// runtime keeps the first input admitted for a key.
    NotAdmittedByDeadline,
    /// The member has no bridge session whose runtime could hold the input.
    MemberHasNoSession,
    /// The member is retired and its runtime holds no input for the delivery
    /// identity, so none can arrive.
    MemberRetired,
}

/// Why an admitted delivery had no terminal by the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeliveryNotTerminalCause {
    /// The deadline elapsed while the live runtime still owed the input a
    /// terminal. Waiting again later is safe: waiting cancels nothing.
    DeadlineElapsed,
    /// The deadline elapsed while the member's session had no live runtime
    /// registration, so the input could not advance (for example between a
    /// restart and the member's recovery).
    RuntimeDetached,
}

/// How an admitted delivery reached its terminal.
#[derive(Debug)]
#[non_exhaustive]
pub enum DeliveryTerminalResolution {
    /// The runtime finalized a terminal receipt for the input.
    Receipt {
        /// The run that answered the input; `None` when the runtime stopped,
        /// retired or was destroyed with the input pending.
        runtime_run_id: Option<RuntimeRunId>,
        /// The canonical owner input of the batch (its first recipient).
        owner_input_id: InputId,
        /// Every input the same batch answered, in canonical (input-id)
        /// order, including this one. More than one entry means one run
        /// answered several deliveries together and `result` is that run's
        /// shared answer.
        recipient_input_ids: Vec<InputId>,
        /// The batch outcome through the bounded result projection.
        result: Result<BoundedTurnResult, BoundedTurnFailure>,
    },
    /// The input reached a terminal the runtime never stages a receipt for
    /// (superseded, coalesced, or consumed on accept); no run answered it.
    WithoutRun { last_run_id: Option<RuntimeRunId> },
}

/// Terminal record of one admitted delivery.
#[derive(Debug)]
#[non_exhaustive]
pub struct DeliveryTerminalRecord {
    input_id: InputId,
    terminal: InputTerminalOutcome,
    attempt_count: u32,
    witness_source: TerminalWitnessSource,
    resolution: DeliveryTerminalResolution,
}

impl DeliveryTerminalRecord {
    /// The runtime input the delivery was admitted as.
    #[must_use]
    pub fn input_id(&self) -> &InputId {
        &self.input_id
    }

    /// The input's machine-owned terminal outcome.
    #[must_use]
    pub fn terminal(&self) -> &InputTerminalOutcome {
        &self.terminal
    }

    /// Execution attempts the runtime recorded for the input.
    #[must_use]
    pub const fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// Whether the live runtime or the durable store answered.
    #[must_use]
    pub const fn witness_source(&self) -> TerminalWitnessSource {
        self.witness_source
    }

    /// How the input reached its terminal.
    #[must_use]
    pub fn resolution(&self) -> &DeliveryTerminalResolution {
        &self.resolution
    }

    /// Take the resolution.
    #[must_use]
    pub fn into_resolution(self) -> DeliveryTerminalResolution {
        self.resolution
    }
}

/// What the wait observed for one delivery.
#[derive(Debug)]
#[non_exhaustive]
pub enum DeliveryTerminalWait {
    /// The delivery's input is terminal.
    Terminal(Box<DeliveryTerminalRecord>),
    /// The delivery was admitted as `input_id` but had no terminal by the
    /// deadline. The fields are the input's last observed lifecycle facts.
    NotTerminal {
        input_id: InputId,
        phase: InputLifecycleState,
        terminal: Option<InputTerminalOutcome>,
        last_run_id: Option<RuntimeRunId>,
        attempt_count: u32,
        cause: DeliveryNotTerminalCause,
    },
    /// No runtime input could be matched to the delivery.
    Unknown { cause: DeliveryUnknownCause },
}

/// Result of
/// [`super::MobHandle::wait_bounded_work_for_identity_with_delivery_identity`]:
/// the member lifecycle it read and what it observed for the delivery.
#[derive(Debug)]
#[non_exhaustive]
pub struct DeliveryTerminalWaitReport {
    member: DurableBoundedMemberState,
    work: DeliveryTerminalWait,
}

impl DeliveryTerminalWaitReport {
    /// The member lifecycle observed when the wait started.
    #[must_use]
    pub fn member(&self) -> &DurableBoundedMemberState {
        &self.member
    }

    /// What the wait observed for the delivery.
    #[must_use]
    pub fn work(&self) -> &DeliveryTerminalWait {
        &self.work
    }

    /// Take both parts.
    #[must_use]
    pub fn into_parts(self) -> (DurableBoundedMemberState, DeliveryTerminalWait) {
        (self.member, self.work)
    }
}

/// Why a delivery terminal wait could not run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DeliveryTerminalWaitError {
    #[error("invalid delivery identity: {0}")]
    InvalidDeliveryIdentity(#[source] MobStoreError),
    #[error(
        "member '{identity}' is placed on a remote host; its runtime inputs are not observable here"
    )]
    RemotelyHostedMember { identity: AgentIdentity },
    #[error("the runtime adapter is unavailable for delivery terminal observation")]
    RuntimeAdapterUnavailable,
    #[error("member lifecycle read failed: {0}")]
    MemberState(#[source] crate::error::MobError),
    #[error("runtime terminal receipt read failed: {0}")]
    RuntimeRead(#[source] RuntimeDriverError),
}

#[cfg(feature = "runtime-adapter")]
impl super::MobHandle {
    /// Wait for the terminal of one caller-identified delivery to a local
    /// member, as the member's runtime recorded it.
    ///
    /// This is the observation counterpart of the delivery-identity submit
    /// paths for members that cannot hand out a completion-bearing admission
    /// (`MobRuntimeMode::AutonomousHost`): the delivery is found by its stable
    /// idempotency key in the member runtime's own admission map, and the
    /// report carries that input's own terminal - the run that answered it,
    /// every input that run answered (so a batched answer reports its batch),
    /// the input's terminal outcome, and the run's result through the same
    /// bounded projection as [`super::WorkTurnHandle::wait_bounded`].
    ///
    /// The method only reads. It never sends a mob actor command, spawns,
    /// submits, retries or cancels work, and waiting past the deadline cancels
    /// nothing; a later call may wait again. It reads the member's current
    /// session binding, like
    /// [`Self::recover_bounded_work_for_identity_with_delivery_identity`]; a
    /// delivery made to an earlier session of the member reads as unknown.
    ///
    /// The call returns by `deadline`: the last quarter of the remaining time
    /// (at least 100 ms, at most 5 s) is reserved for one final bounded read,
    /// so a deadline that has passed or is closer than 100 ms takes one
    /// snapshot read bounded by 100 ms. A terminal the runtime has durably
    /// finalized is returned from the store even before the member's session
    /// is registered again after a restart.
    pub async fn wait_bounded_work_for_identity_with_delivery_identity(
        &self,
        identity: &AgentIdentity,
        delivery_identity: &crate::store::MobDeliveryIdentity,
        result_spec: &BoundedResultSpec,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<DeliveryTerminalWaitReport, DeliveryTerminalWaitError> {
        delivery_identity
            .validate()
            .map_err(DeliveryTerminalWaitError::InvalidDeliveryIdentity)?;
        if self.member_placement_present(identity) {
            return Err(DeliveryTerminalWaitError::RemotelyHostedMember {
                identity: identity.clone(),
            });
        }
        let runtime = self
            .runtime_adapter
            .as_ref()
            .ok_or(DeliveryTerminalWaitError::RuntimeAdapterUnavailable)?;
        let member = self
            .durable_bounded_member_state(identity)
            .await
            .map_err(DeliveryTerminalWaitError::MemberState)?;
        let Some(session_id) = member.session_id().cloned() else {
            return Ok(DeliveryTerminalWaitReport {
                member,
                work: DeliveryTerminalWait::Unknown {
                    cause: DeliveryUnknownCause::MemberHasNoSession,
                },
            });
        };
        let work = observe::observe_delivery_terminal(
            runtime,
            &session_id,
            &delivery_identity.idempotency_key,
            result_spec,
            matches!(member, DurableBoundedMemberState::Retired { .. }),
            deadline,
        )
        .await?;
        Ok(DeliveryTerminalWaitReport { member, work })
    }
}

#[cfg(feature = "runtime-adapter")]
mod observe {
    use super::*;
    use crate::tokio;
    use meerkat_core::time_compat::Instant;
    use meerkat_core::types::SessionId;
    use meerkat_runtime::terminal_status::{
        InputTerminalReceiptRead, InputTerminalReceiptWait, InteractionSelector, Sourced,
    };
    use std::time::Duration;

    /// The evidence slice follows `meerkat_runtime::SubmitBound`: a quarter
    /// of the caller's bound, floored and capped, so the final read cannot run
    /// the call past its deadline.
    const MIN_EVIDENCE_READ_BOUND: Duration = Duration::from_millis(100);
    const MAX_EVIDENCE_READ_BOUND: Duration = Duration::from_secs(5);
    /// Polling covers only what the runtime cannot notify: a key not yet
    /// bound (the inbox gap) and a session without a live registration.
    const POLL_START: Duration = Duration::from_millis(10);
    const POLL_MAX: Duration = Duration::from_millis(250);
    /// While armed on the runtime's waiter, re-read at least this often.
    /// Transitions that never deliver a completion (supersession or
    /// coalescing by a later admission) are observed by that read.
    const REREAD_INTERVAL: Duration = Duration::from_secs(1);

    fn split(bound: Duration) -> (Duration, Duration) {
        let evidence = (bound / 4).clamp(MIN_EVIDENCE_READ_BOUND, MAX_EVIDENCE_READ_BOUND);
        (bound.saturating_sub(evidence), evidence)
    }

    struct PendingFacts {
        input_id: InputId,
        phase: InputLifecycleState,
        terminal: Option<InputTerminalOutcome>,
        last_run_id: Option<RuntimeRunId>,
        attempt_count: u32,
        live: bool,
    }

    enum Observation {
        /// No input is bound to the key yet, or the session's runtime cannot
        /// be read (never admitted, or unregistered on a store-less machine).
        Unobservable,
        Pending(PendingFacts),
        Terminal(Box<DeliveryTerminalRecord>),
    }

    struct DeliveryObserver<'a> {
        runtime: &'a meerkat_runtime::MeerkatMachine,
        session_id: &'a SessionId,
        idempotency_key: &'a str,
        result_spec: &'a BoundedResultSpec,
    }

    impl DeliveryObserver<'_> {
        async fn read(
            &self,
            input_id: Option<&InputId>,
        ) -> Result<Observation, DeliveryTerminalWaitError> {
            let selector = input_id.map_or_else(
                || InteractionSelector::IdempotencyKey(self.idempotency_key.to_string()),
                |input_id| InteractionSelector::InputId(input_id.clone()),
            );
            match self
                .runtime
                .input_terminal_receipt(self.session_id, selector)
                .await
            {
                Ok(Some(read)) => self.classify(read),
                Ok(None)
                | Err(RuntimeDriverError::NotFound { .. } | RuntimeDriverError::NotReady { .. }) => {
                    Ok(Observation::Unobservable)
                }
                Err(error) => Err(DeliveryTerminalWaitError::RuntimeRead(error)),
            }
        }

        fn classify(
            &self,
            read: Sourced<InputTerminalReceiptRead>,
        ) -> Result<Observation, DeliveryTerminalWaitError> {
            let witness_source = read.source;
            match read.report {
                InputTerminalReceiptRead::Pending {
                    input_id,
                    phase,
                    terminal,
                    last_run_id,
                    attempt_count,
                } => Ok(Observation::Pending(PendingFacts {
                    input_id,
                    phase,
                    terminal,
                    last_run_id,
                    attempt_count,
                    live: witness_source == TerminalWitnessSource::LiveRuntime,
                })),
                InputTerminalReceiptRead::Finalized(receipt) => {
                    let receipt = *receipt;
                    let input_id = receipt.input_id().clone();
                    let terminal = receipt.terminal().clone();
                    let attempt_count = receipt.attempt_count();
                    let runtime_run_id = receipt.run_id().cloned();
                    let owner_input_id = receipt.owner_input_id().clone();
                    let recipient_input_ids = receipt.recipient_input_ids().to_vec();
                    let result = super::super::handle::bounded_runtime_turn_result(
                        receipt.into_outcome(),
                        self.session_id,
                        self.session_id.clone(),
                        self.result_spec,
                    );
                    Ok(Observation::Terminal(Box::new(DeliveryTerminalRecord {
                        input_id,
                        terminal,
                        attempt_count,
                        witness_source,
                        resolution: DeliveryTerminalResolution::Receipt {
                            runtime_run_id,
                            owner_input_id,
                            recipient_input_ids,
                            result,
                        },
                    })))
                }
                InputTerminalReceiptRead::TerminalWithoutReceipt {
                    input_id,
                    terminal,
                    last_run_id,
                    attempt_count,
                } => Ok(Observation::Terminal(Box::new(DeliveryTerminalRecord {
                    input_id,
                    terminal,
                    attempt_count,
                    witness_source,
                    resolution: DeliveryTerminalResolution::WithoutRun { last_run_id },
                }))),
                other => Err(DeliveryTerminalWaitError::RuntimeRead(
                    RuntimeDriverError::Internal(format!(
                        "unrecognized terminal receipt read {other:?}"
                    )),
                )),
            }
        }
    }

    fn not_terminal(facts: PendingFacts, cause: DeliveryNotTerminalCause) -> DeliveryTerminalWait {
        DeliveryTerminalWait::NotTerminal {
            input_id: facts.input_id,
            phase: facts.phase,
            terminal: facts.terminal,
            last_run_id: facts.last_run_id,
            attempt_count: facts.attempt_count,
            cause,
        }
    }

    /// Observe one delivery's terminal on one member session until
    /// `deadline`, returning by the deadline (or within the evidence-read
    /// floor for a nearly elapsed one).
    pub(super) async fn observe_delivery_terminal(
        runtime: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
        idempotency_key: &str,
        result_spec: &BoundedResultSpec,
        member_retired: bool,
        deadline: Instant,
    ) -> Result<DeliveryTerminalWait, DeliveryTerminalWaitError> {
        let observer = DeliveryObserver {
            runtime,
            session_id,
            idempotency_key,
            result_spec,
        };
        let started = Instant::now();
        let (wait_budget, evidence_budget) = split(deadline.saturating_duration_since(started));
        let wait_until = started + wait_budget;
        let mut input_id: Option<InputId> = None;
        let mut last_pending: Option<PendingFacts> = None;
        let mut poll = POLL_START;

        loop {
            let remaining = wait_until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let Ok(observation) =
                tokio::time::timeout(remaining, observer.read(input_id.as_ref())).await
            else {
                break;
            };
            let armed_on = match observation? {
                Observation::Terminal(record) => return Ok(DeliveryTerminalWait::Terminal(record)),
                Observation::Pending(facts) => {
                    let armed_on = facts.live.then(|| facts.input_id.clone());
                    input_id = Some(facts.input_id.clone());
                    last_pending = Some(facts);
                    armed_on
                }
                Observation::Unobservable => {
                    if member_retired && input_id.is_none() {
                        return Ok(DeliveryTerminalWait::Unknown {
                            cause: DeliveryUnknownCause::MemberRetired,
                        });
                    }
                    None
                }
            };
            let remaining = wait_until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            if let Some(armed_on) = armed_on {
                match tokio::time::timeout(
                    remaining.min(REREAD_INTERVAL),
                    runtime.wait_input_terminal_receipt(session_id, &armed_on),
                )
                .await
                {
                    // Re-read on expiry; the loop re-arms if still pending.
                    Err(_elapsed) => continue,
                    Ok(Ok(InputTerminalReceiptWait::Resolved(read))) => {
                        if let Observation::Terminal(record) = observer.classify(read)? {
                            return Ok(DeliveryTerminalWait::Terminal(record));
                        }
                        continue;
                    }
                    // The session lost its live registration; poll below.
                    Ok(Ok(_detached)) => {}
                    Ok(Err(
                        RuntimeDriverError::NotFound { .. } | RuntimeDriverError::NotReady { .. },
                    )) => {}
                    Ok(Err(error)) => return Err(DeliveryTerminalWaitError::RuntimeRead(error)),
                }
            }
            let remaining = wait_until.saturating_duration_since(Instant::now());
            tokio::time::sleep(poll.min(remaining)).await;
            poll = (poll * 2).min(POLL_MAX);
        }

        // Final evidence read, bounded by the reserved slice.
        let observation =
            match tokio::time::timeout(evidence_budget, observer.read(input_id.as_ref())).await {
                Ok(observation) => Some(observation?),
                Err(_elapsed) => None,
            };
        Ok(match observation {
            Some(Observation::Terminal(record)) => DeliveryTerminalWait::Terminal(record),
            Some(Observation::Pending(facts)) => {
                let cause = if facts.live {
                    DeliveryNotTerminalCause::DeadlineElapsed
                } else {
                    DeliveryNotTerminalCause::RuntimeDetached
                };
                not_terminal(facts, cause)
            }
            Some(Observation::Unobservable) => match last_pending {
                Some(facts) => not_terminal(facts, DeliveryNotTerminalCause::RuntimeDetached),
                None if member_retired => DeliveryTerminalWait::Unknown {
                    cause: DeliveryUnknownCause::MemberRetired,
                },
                None => DeliveryTerminalWait::Unknown {
                    cause: DeliveryUnknownCause::NotAdmittedByDeadline,
                },
            },
            // The evidence read itself ran out: report the last observation.
            None => match last_pending {
                Some(facts) => {
                    let cause = if facts.live {
                        DeliveryNotTerminalCause::DeadlineElapsed
                    } else {
                        DeliveryNotTerminalCause::RuntimeDetached
                    };
                    not_terminal(facts, cause)
                }
                None => DeliveryTerminalWait::Unknown {
                    cause: DeliveryUnknownCause::NotAdmittedByDeadline,
                },
            },
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn split_reserves_an_evidence_slice_inside_the_bound() {
            assert_eq!(
                split(Duration::from_secs(4)),
                (Duration::from_secs(3), Duration::from_secs(1))
            );
            assert_eq!(
                split(Duration::from_secs(60)),
                (Duration::from_secs(55), MAX_EVIDENCE_READ_BOUND)
            );
            assert_eq!(
                split(Duration::ZERO),
                (Duration::ZERO, MIN_EVIDENCE_READ_BOUND)
            );
        }
    }
}
