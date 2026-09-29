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
//! machine-owned key-to-input binding, the input's terminal receipt (the
//! terminal batch the runtime finalized it in: the run that committed that
//! batch and the inputs finalized with it), and the batch's finalized
//! outcome. The result is projected through the same bounded projection as
//! [`super::WorkTurnHandle::wait_bounded`]. Nothing here records or decides a
//! lifecycle fact, and no mob actor command is sent.

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
    /// The deadline elapsed before the member lifecycle or the member's
    /// runtime could be read even once, so nothing is known about the
    /// delivery. Waiting again with a later deadline is safe.
    NotObservedByDeadline,
}

/// Why an admitted delivery had no terminal by the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeliveryNotTerminalCause {
    /// The live runtime still owed the input a terminal when the deadline
    /// came: the wait ran until the deadline less the 100 ms evidence-read
    /// floor, and the final read in that last slice still found the input
    /// pending. Waiting again later is safe: waiting cancels nothing.
    DeadlineElapsed,
    /// The final evidence read did not finish before the deadline (it can
    /// wait on the session driver, which a run's terminal commit holds), so
    /// the facts are those of the last read before it, which found the input
    /// pending. They say nothing about the input now: it may have reached its
    /// terminal since. Waiting again later is safe: waiting cancels nothing.
    EvidenceReadTimedOut,
    /// The deadline elapsed while the member's session had no live runtime
    /// registration, so the input could not advance (for example between a
    /// restart and the member's recovery).
    RuntimeDetached,
}

/// How an admitted delivery reached its terminal.
#[derive(Debug)]
#[non_exhaustive]
pub enum DeliveryTerminalResolution {
    /// The runtime finalized a terminal receipt for the input: the terminal
    /// batch it finalized the input in, with one outcome shared by every
    /// input of that batch.
    ///
    /// The receipt proves which inputs share `result`; it does not prove
    /// that a run's answer covers every input the run touched. A delivery
    /// joined into a running run at a live boundary (a steer injected as
    /// turn context) is finalized alone in its own batch of that run, with
    /// `result` = `Err(BoundedTurnFailure::CompletedWithoutResult)`: the
    /// run's text answers the run's own batch, not this delivery.
    Receipt {
        /// The run whose terminal transaction committed the input's batch;
        /// `None` when the runtime terminalized the input outside any run
        /// (stop, reset, retire, destroy, executor replacement, or
        /// cancellation while queued).
        runtime_run_id: Option<RuntimeRunId>,
        /// The canonical owner input of the batch (its first recipient).
        owner_input_id: InputId,
        /// Every input finalized in the same batch, in canonical (input-id)
        /// order, including this one; all of them share `result`. More than
        /// one entry means one terminal transaction finalized several
        /// deliveries together, for example queued deliveries one run
        /// consumed as one batch. It is the batch, not every input the run
        /// touched.
        recipient_input_ids: Vec<InputId>,
        /// The batch outcome through the bounded result projection.
        result: Result<BoundedTurnResult, BoundedTurnFailure>,
    },
    /// The input reached a terminal through a runtime transition that stages
    /// no receipt: superseded or coalesced by a later admission, consumed on
    /// accept, cancelled by member-host boot revival, or abandoned at the
    /// stage-attempt cap after a failed batch start. No run answered it;
    /// `last_run_id` is the last run it was staged into, if any.
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
    member: Option<DurableBoundedMemberState>,
    work: DeliveryTerminalWait,
}

impl DeliveryTerminalWaitReport {
    /// The member lifecycle observed when the wait started. `None` only when
    /// the deadline elapsed before it could be read; `work` is then
    /// `Unknown { cause: NotObservedByDeadline }`.
    #[must_use]
    pub fn member(&self) -> Option<&DurableBoundedMemberState> {
        self.member.as_ref()
    }

    /// What the wait observed for the delivery.
    #[must_use]
    pub fn work(&self) -> &DeliveryTerminalWait {
        &self.work
    }

    /// Take both parts.
    #[must_use]
    pub fn into_parts(self) -> (Option<DurableBoundedMemberState>, DeliveryTerminalWait) {
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
    /// A settle named an input the delivery was not admitted as (see
    /// [`super::MobHandle::settle_delivery_input_for_identity`]). Nothing was
    /// cancelled.
    #[error("input {input_id} is not the runtime input this delivery was admitted as")]
    InputNotOfDelivery { input_id: InputId },
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
    /// report carries that input's own terminal - the terminal batch the
    /// runtime finalized it in (the run that committed the batch and every
    /// input finalized with it, so a batched answer reports its batch), the
    /// input's terminal outcome, and the batch's result through the same
    /// bounded projection as [`super::WorkTurnHandle::wait_bounded`]. See
    /// [`DeliveryTerminalResolution::Receipt`] for what that proves.
    ///
    /// The method only reads. It never sends a mob actor command, spawns,
    /// submits, retries or cancels work, and waiting past the deadline cancels
    /// nothing; a later call may wait again. It reads the member's current
    /// session binding, like
    /// [`Self::recover_bounded_work_for_identity_with_delivery_identity`]; a
    /// delivery made to an earlier session of the member reads as unknown.
    ///
    /// The call returns by `deadline`, or within the 100 ms evidence-read
    /// floor for a deadline that has passed or is closer than that. Every
    /// step is bounded by that end: the member lifecycle read, the wait on
    /// the member runtime (which runs until the end less the floor), and one
    /// final read in the floor. A terminal the runtime has durably finalized
    /// is returned from the store even before the member's session is
    /// registered again after a restart.
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
        let end = observe::call_end(deadline);
        // For a member the machine state no longer (or never) knows this
        // replays the mob event log, so it is bounded like every other step.
        let Some(member) = observe::within(end, self.durable_bounded_member_state(identity)).await
        else {
            return Ok(DeliveryTerminalWaitReport {
                member: None,
                work: DeliveryTerminalWait::Unknown {
                    cause: DeliveryUnknownCause::NotObservedByDeadline,
                },
            });
        };
        let member = member.map_err(DeliveryTerminalWaitError::MemberState)?;
        let Some(session_id) = member.session_id().cloned() else {
            return Ok(DeliveryTerminalWaitReport {
                member: Some(member),
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
            end,
        )
        .await?;
        Ok(DeliveryTerminalWaitReport {
            member: Some(member),
            work,
        })
    }

    /// Settle the runtime input a caller-identified delivery to a local
    /// member was admitted as, then read the delivery's terminal.
    ///
    /// `input_id` is the exact input a [`DeliveryTerminalWait::NotTerminal`]
    /// reading of this delivery named; an input the member's runtime does not
    /// hold under this delivery's idempotency key is refused with
    /// [`DeliveryTerminalWaitError::InputNotOfDelivery`] before anything is
    /// cancelled. Unless the input already has a terminal, it is cancelled in
    /// the member's runtime: a queued input is abandoned alone, and a staged or
    /// applied one is cancelled through its exact run, never through the
    /// member's ambient current run. Cancelling that run also ends every other
    /// input batched into it, so a delivery that shares its run with other
    /// work ends that work too. A run that answers the input first wins, and
    /// its terminal is the one read. The read is
    /// [`Self::wait_bounded_work_for_identity_with_delivery_identity`] by
    /// `deadline`.
    ///
    /// This fences a delivery before its caller commits an outcome for it:
    /// once the report carries a terminal, no run can answer the input any
    /// more. A report without one (the member's runtime no longer holds the
    /// input live, or the read ended first) settled nothing.
    pub async fn settle_delivery_input_for_identity(
        &self,
        identity: &AgentIdentity,
        delivery_identity: &crate::store::MobDeliveryIdentity,
        input_id: &InputId,
        result_spec: &BoundedResultSpec,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<DeliveryTerminalWaitReport, DeliveryTerminalWaitError> {
        use meerkat_runtime::SessionServiceRuntimeExt as _;

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
                member: Some(member),
                work: DeliveryTerminalWait::Unknown {
                    cause: DeliveryUnknownCause::MemberHasNoSession,
                },
            });
        };
        let admitted = runtime
            .durable_input_state_by_idempotency_key(&session_id, &delivery_identity.idempotency_key)
            .await
            .map_err(DeliveryTerminalWaitError::RuntimeRead)?;
        if admitted.as_ref().map(|stored| &stored.state.input_id) != Some(input_id) {
            return Err(DeliveryTerminalWaitError::InputNotOfDelivery {
                input_id: input_id.clone(),
            });
        }
        #[cfg(any(test, feature = "test-support"))]
        if run_delivery_input_settle_test_gate(identity).await
            == DeliveryInputSettleTestRelease::FailCancellation
        {
            return Err(DeliveryTerminalWaitError::RuntimeRead(
                RuntimeDriverError::Internal(
                    "injected delivery input settle cancellation failure".to_string(),
                ),
            ));
        }
        runtime
            .cancel_input_if_present(&session_id, input_id, "delivery settled by its observer")
            .await
            .map_err(DeliveryTerminalWaitError::RuntimeRead)?;
        self.wait_bounded_work_for_identity_with_delivery_identity(
            identity,
            delivery_identity,
            result_spec,
            deadline,
        )
        .await
    }

    /// Pause the next [`Self::settle_delivery_input_for_identity`] of
    /// `identity`, through any handle in this process, after it matched the
    /// input to the delivery and before it cancels anything. The first
    /// receiver resolves when a settle reaches the gate; the returned sender
    /// lets it go on as told (dropping it proceeds). Exposed only by test
    /// builds.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_delivery_input_settle_test_gate(
        identity: AgentIdentity,
    ) -> (
        crate::tokio::sync::oneshot::Receiver<()>,
        crate::tokio::sync::oneshot::Sender<DeliveryInputSettleTestRelease>,
    ) {
        let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
        let mut gate = DELIVERY_INPUT_SETTLE_TEST_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            gate.is_none(),
            "delivery input settle test gate already armed"
        );
        *gate = Some((identity, entered_tx, release_rx));
        (entered_rx, release_tx)
    }
}

/// How a test releases a held delivery input settle (see
/// [`super::MobHandle::arm_delivery_input_settle_test_gate`]).
#[cfg(all(feature = "runtime-adapter", any(test, feature = "test-support")))]
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryInputSettleTestRelease {
    /// Go on and cancel the input.
    Proceed,
    /// Fail as a cancellation that could not settle the input.
    FailCancellation,
}

/// One-shot gate a test arms to hold a delivery input settle (see
/// [`super::MobHandle::arm_delivery_input_settle_test_gate`]).
#[cfg(all(feature = "runtime-adapter", any(test, feature = "test-support")))]
type DeliveryInputSettleTestGate = (
    AgentIdentity,
    crate::tokio::sync::oneshot::Sender<()>,
    crate::tokio::sync::oneshot::Receiver<DeliveryInputSettleTestRelease>,
);

#[cfg(all(feature = "runtime-adapter", any(test, feature = "test-support")))]
static DELIVERY_INPUT_SETTLE_TEST_GATE: std::sync::Mutex<Option<DeliveryInputSettleTestGate>> =
    std::sync::Mutex::new(None);

#[cfg(all(feature = "runtime-adapter", any(test, feature = "test-support")))]
async fn run_delivery_input_settle_test_gate(
    identity: &AgentIdentity,
) -> DeliveryInputSettleTestRelease {
    let armed = {
        let mut gate = DELIVERY_INPUT_SETTLE_TEST_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if gate
            .as_ref()
            .is_some_and(|(armed_identity, _, _)| armed_identity == identity)
        {
            gate.take()
        } else {
            None
        }
    };
    let Some((_, entered_tx, release_rx)) = armed else {
        return DeliveryInputSettleTestRelease::Proceed;
    };
    let _ = entered_tx.send(());
    release_rx
        .await
        .unwrap_or(DeliveryInputSettleTestRelease::Proceed)
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

    /// Time kept back from the wait for one final evidence read, so the
    /// call reports fresh facts and still returns by its deadline. It is the
    /// floor `meerkat_runtime::SubmitBound` gives its own classification read,
    /// the shortest slice the runtime allots one evidence-backed read: here
    /// one driver-lock acquisition, or at most three store point reads for a
    /// receipt batch. It is also the whole budget of a call whose deadline has
    /// passed or is closer than this.
    const EVIDENCE_READ_FLOOR: Duration = Duration::from_millis(100);
    /// Polling covers only what the runtime cannot notify: a key not yet
    /// bound (the inbox gap) and a session without a live registration.
    const POLL_START: Duration = Duration::from_millis(10);
    const POLL_MAX: Duration = Duration::from_millis(250);
    /// While armed on the runtime's waiter, re-read at least this often.
    /// Every terminal transition wakes that waiter; this bounded re-read is
    /// defense in depth for a wake that lags receipt finalization (the
    /// runtime resolves waiters after finalizing, and a directed terminal can
    /// finalize before its publication succeeds).
    const REREAD_INTERVAL: Duration = Duration::from_secs(1);

    /// When the call must return: the caller's deadline, or one evidence
    /// floor from now for a deadline that has passed or is closer than that.
    pub(super) fn call_end(deadline: Instant) -> Instant {
        deadline.max(Instant::now() + EVIDENCE_READ_FLOOR)
    }

    /// Until when the wait may park before the final evidence read.
    fn wait_until(end: Instant) -> Instant {
        end.checked_sub(EVIDENCE_READ_FLOOR)
            .map_or_else(Instant::now, |until| until.max(Instant::now()))
    }

    /// Run `future` until `end`; `None` when `end` came first.
    pub(super) async fn within<F: std::future::Future>(
        end: Instant,
        future: F,
    ) -> Option<F::Output> {
        tokio::time::timeout(end.saturating_duration_since(Instant::now()), future)
            .await
            .ok()
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

    /// Observe one delivery's terminal on one member session, returning by
    /// `end` (see [`call_end`]): wait until `end` less the evidence floor,
    /// then take one final read bounded by the time left.
    pub(super) async fn observe_delivery_terminal(
        runtime: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
        idempotency_key: &str,
        result_spec: &BoundedResultSpec,
        member_retired: bool,
        end: Instant,
    ) -> Result<DeliveryTerminalWait, DeliveryTerminalWaitError> {
        let observer = DeliveryObserver {
            runtime,
            session_id,
            idempotency_key,
            result_spec,
        };
        let wait_until = wait_until(end);
        let mut input_id: Option<InputId> = None;
        let mut last_pending: Option<PendingFacts> = None;
        // Whether some read completed and found no input for the key.
        let mut observed_unadmitted = false;
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
                    observed_unadmitted = true;
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
                    Ok(Ok(Some(InputTerminalReceiptWait::Resolved(read)))) => {
                        if let Observation::Terminal(record) = observer.classify(read)? {
                            return Ok(DeliveryTerminalWait::Terminal(record));
                        }
                        continue;
                    }
                    // The session lost its live registration (or the row
                    // left it); poll below.
                    Ok(Ok(_detached_or_unknown)) => {}
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

        // Final evidence read in the floor kept back before `end`.
        let observation = match within(end, observer.read(input_id.as_ref())).await {
            Some(observation) => Some(observation?),
            None => None,
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
            // The evidence read itself ran out: report the last observation,
            // marked as an earlier reading rather than the state at the end.
            None => match last_pending {
                Some(facts) => not_terminal(facts, DeliveryNotTerminalCause::EvidenceReadTimedOut),
                None if observed_unadmitted => DeliveryTerminalWait::Unknown {
                    cause: DeliveryUnknownCause::NotAdmittedByDeadline,
                },
                None => DeliveryTerminalWait::Unknown {
                    cause: DeliveryUnknownCause::NotObservedByDeadline,
                },
            },
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Only the evidence floor is kept back from the wait, whatever the
        /// budget; a passed or too-close deadline gets exactly the floor.
        #[test]
        fn only_the_evidence_floor_is_kept_back_from_the_wait() {
            let now = Instant::now();
            for budget in [Duration::from_secs(4), Duration::from_secs(60)] {
                let end = call_end(now + budget);
                assert_eq!(end, now + budget);
                assert_eq!(Some(wait_until(end)), end.checked_sub(EVIDENCE_READ_FLOOR));
            }
            for deadline in [now, now + EVIDENCE_READ_FLOOR / 2] {
                let before = Instant::now();
                let end = call_end(deadline);
                assert!(end >= before + EVIDENCE_READ_FLOOR);
                assert!(end <= Instant::now() + EVIDENCE_READ_FLOOR);
                let until = wait_until(end);
                assert!(until <= end && end.duration_since(until) <= EVIDENCE_READ_FLOOR);
            }
        }
    }
}
