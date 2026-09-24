//! Bounded, typed dispatch contract for runtime submissions.
//!
//! A host that hands work to the runtime needs three things the raw accept
//! surface does not give it:
//!
//! 1. a bound it supplies itself, so an admission that never answers cannot
//!    masquerade as normal operation (an `await` that never returns never
//!    raises, so a caller's bounded-retry handler never fires);
//! 2. a typed outcome for every fate - durable acceptance, typed refusal, or a
//!    typed timeout that says whether the work is durably queued (retry is
//!    safe) or unknown (retry is not);
//! 3. a mandatory idempotency key, so nobody can add a retry without first
//!    deciding what makes the delivery unique.
//!
//! The third point is not optional garnish. A bounded wait *without* an
//! idempotency seam creates a strictly worse failure than the hang it removes:
//! a slow-but-successful delivery whose caller's bound expired, retried, and
//! delivered twice. The bound here is on the CALLER'S OBSERVATION, never on the
//! work: an expired bound never cancels an in-flight admission, and the
//! mandatory key means a retry collapses at the durable admission point instead
//! of executing a second time.
//!
//! # Scope
//!
//! This contract is admission-scoped. Its success arm means "the runtime
//! admitted the work and it will run", not "the turn finished". Turn results
//! are served by
//! [`SessionServiceRuntimeExt::input_terminal_completion`] and
//! [`SessionServiceRuntimeExt::interaction_terminal_status`].
//!
//! # Collapse guarantee, stated honestly
//!
//! Collapse is owned by the generated admission machine
//! (`ResolveAdmissionIdempotency` / `RegisterAcceptedIdempotency`) plus, on a
//! persistent runtime, the store-owned input index that the persistent driver
//! consults before the live map. That combination is what makes a duplicate
//! submission collapse rather than execute twice.
//!
//! On a runtime with no store, collapse holds only while the admission is
//! retained in live machine state: the machine drops an input's key binding
//! when the terminal input is archived, and nothing survives a restart. That is
//! why [`AdmissionDurability`] and [`SubmitTimeoutDisposition`] are separate
//! typed facts instead of an assumed guarantee.
//!
//! # A durable row is not the same fact as work that will run
//!
//! The key binding outlives the work: a cancelled, abandoned, superseded, or
//! coalesced input keeps its committed row and stays indexed under its key
//! forever, and a later submission under that key collapses onto it. Reporting
//! that as "durably queued" would re-create the very failure this contract
//! exists to remove - a host marking its message dispatched and waiting for a
//! reply that is never coming. So every outcome that names an input also
//! carries [`AdmittedWorkState`], resolved by the same generated MeerkatMachine
//! authority every other surface publishes input state with, and
//! [`BoundedSubmitReport::is_durably_queued`] is true only for work that is
//! both durable AND still owed a run.

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::lifecycle::InputId;
use meerkat_core::types::SessionId;

use crate::accept::{AcceptOutcome, RejectReason};
use crate::identifiers::IdempotencyKey;
use crate::input::Input;
use crate::input_state::InputStateSeed;
use crate::meerkat_machine::dsl::InputPublicTerminalOutcome;
use crate::service_ext::SessionServiceRuntimeExt;
use crate::traits::RuntimeDriverError;

/// Bound applied when a caller supplies none.
///
/// Matches the transport budget the runtime already applies to a peer bridge
/// request, so a host that does not choose gets the same order of magnitude the
/// rest of the runtime already treats as "too long to still be waiting".
pub const DEFAULT_SUBMIT_BOUND: Duration = Duration::from_secs(30);

/// Longest slice of a caller's bound reserved for classifying an expiry.
const MAX_EVIDENCE_READ_BOUND: Duration = Duration::from_secs(5);

/// Shortest classification slice. A caller may ask for a bound too small to
/// carve a useful slice out of; classification still gets this much, so a tiny
/// bound returns an evidence-backed disposition instead of a reflexive
/// "unknown".
const MIN_EVIDENCE_READ_BOUND: Duration = Duration::from_millis(100);

/// Caller-supplied bound on how long a submission may go unanswered.
///
/// There is deliberately no unbounded variant: an unbounded submit is the
/// defect this type exists to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubmitBound(Duration);

impl SubmitBound {
    /// Bound this submission by `bound`.
    ///
    /// A zero bound is legal and means "answer from durable evidence, do not
    /// wait": the admission is still started and still runs, but the caller
    /// never observes it, so the outcome is always
    /// [`BoundedSubmitOutcome::TimedOut`] classified from the store-owned
    /// index alone - including when the admission would have answered
    /// instantly. A host that computes its bound must therefore not treat a
    /// zero-bound timeout as a runtime fault.
    #[must_use]
    pub const fn after(bound: Duration) -> Self {
        Self(bound)
    }

    /// The caller-supplied duration.
    #[must_use]
    pub const fn as_duration(self) -> Duration {
        self.0
    }

    /// Split the bound into the slice the admission may consume and the slice
    /// reserved for classifying an expiry.
    ///
    /// The classification slice is a quarter of the bound, floored at
    /// [`MIN_EVIDENCE_READ_BOUND`] and capped at [`MAX_EVIDENCE_READ_BOUND`];
    /// the admission gets whatever remains. Only a bound below that floor
    /// cannot fund both, and such a call still returns within
    /// [`MIN_EVIDENCE_READ_BOUND`] in total. At or above the floor the two
    /// slices always sum to exactly the caller's bound.
    fn split(self) -> (Duration, Duration) {
        let evidence = (self.0 / 4).clamp(MIN_EVIDENCE_READ_BOUND, MAX_EVIDENCE_READ_BOUND);
        (self.0.saturating_sub(evidence), evidence)
    }
}

impl Default for SubmitBound {
    fn default() -> Self {
        Self(DEFAULT_SUBMIT_BOUND)
    }
}

/// One retryable hand-off to the runtime.
///
/// The idempotency key is not optional. A submission that can be retried must
/// name what makes it unique, at the point the retry is introduced rather than
/// after a duplicate reaches a recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedSubmission {
    idempotency_key: IdempotencyKey,
    bound: SubmitBound,
}

impl BoundedSubmission {
    /// Submit under `idempotency_key` with the documented default bound.
    #[must_use]
    pub fn new(idempotency_key: IdempotencyKey) -> Self {
        Self {
            idempotency_key,
            bound: SubmitBound::default(),
        }
    }

    /// Replace the default bound with a caller-supplied one.
    #[must_use]
    pub const fn with_bound(mut self, bound: SubmitBound) -> Self {
        self.bound = bound;
        self
    }

    /// The key this submission collapses on.
    #[must_use]
    pub const fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    /// The bound this submission is observed under.
    #[must_use]
    pub const fn bound(&self) -> SubmitBound {
        self.bound
    }
}

/// Whether an admission survives a process restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdmissionDurability {
    /// A committed row for this key exists in the runtime's store-owned input
    /// index. The admission survives restart and a retry under the same key
    /// collapses onto it even on a fresh process.
    Durable,
    /// The runtime admitted the work, but no durable witness is observable:
    /// either the runtime retains nothing across restart, or its index could
    /// not answer. The work runs in this process; a retry under the same key
    /// collapses only while the admission is retained in live machine state.
    ProcessLocalOnly,
}

/// What generated MeerkatMachine authority says about the work behind one
/// admitted input right now.
///
/// Durability answers "does this survive a restart"; this answers "is anything
/// still going to happen". They are independent: the terminal row of a
/// cancelled input is durable and dead at the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdmittedWorkState {
    /// Not terminal: the runtime still owes this input a run, so a caller
    /// waiting on its reply is waiting for something still coming.
    Pending,
    /// Terminal because a run consumed it: the work happened. A retry under
    /// the same key collapses onto the finished row and changes nothing.
    Delivered,
    /// Terminal without this input ever being consumed by a run. Nothing more
    /// will happen for it, and a retry under the SAME key collapses onto this
    /// same dead row instead of re-running the work: a host that still needs
    /// the work done must submit it under a NEW key.
    ///
    /// `outcome` keeps the sub-cases the machine distinguishes rather than
    /// flattening them: `Superseded` and `Coalesced` mean a successor input
    /// carries this work, while `Cancelled` and `Abandoned` mean nothing does.
    TerminalWithoutDelivery { outcome: InputPublicTerminalOutcome },
    /// The machine declined to classify this input's seed, so neither "still
    /// coming" nor "already terminal" is proven. Treated as not-queued
    /// everywhere: a row nobody can classify must never read as a live claim.
    Unclassified,
}

impl AdmittedWorkState {
    /// Whether the runtime still owes this input a run.
    #[must_use]
    pub const fn is_pending(self) -> bool {
        matches!(self, Self::Pending)
    }

    /// Whether this input reached a terminal state without ever running.
    #[must_use]
    pub const fn is_terminal_without_delivery(self) -> bool {
        matches!(self, Self::TerminalWithoutDelivery { .. })
    }
}

/// Why a submission's fate is unknown after its bound expired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubmitUnknownCause {
    /// The store-owned index held no committed row for this key.
    NoDurableWitness,
    /// The durable evidence read failed or could not answer in time, so the
    /// absence of a witness is not evidence of absence.
    EvidenceUnavailable,
}

/// What durable evidence says about work whose submission went unanswered.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubmitTimeoutDisposition {
    /// A committed durable row exists for this key, and `state` says what the
    /// machine makes of it: still owed a run, already run, or terminal without
    /// ever running. Retrying under the same key is safe (it collapses onto
    /// this row) but changes nothing, in any of the three cases.
    DurablyAdmitted {
        input_id: InputId,
        state: AdmittedWorkState,
    },
    /// No durable witness was observable. The submission may still commit, may
    /// have been refused, or may never have reached the runtime. Retry only
    /// under the SAME idempotency key, and note that exactly-once is not
    /// guaranteed if this process dies before the admission commits.
    Unknown { cause: SubmitUnknownCause },
}

/// Why a submission produced no definite admission answer.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SubmitTimeoutCause {
    /// The caller-supplied bound expired first. The admission was NOT
    /// cancelled: the bound is on the caller's observation, never on the work.
    BoundExpired,
    /// The runtime failed without a definite admission answer, which leaves
    /// this submission in the same unknown state a bound expiry does.
    RuntimeIndeterminate { error: RuntimeDriverError },
}

/// Typed reason a submission was declined.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SubmitRefusal {
    /// The input already carried a different idempotency key, so the
    /// submission has no single unique identity. Nothing was sent to the
    /// runtime.
    IdempotencyKeyConflict {
        supplied: IdempotencyKey,
        carried: IdempotencyKey,
    },
    /// The runtime rejected the input at its admission boundary and named the
    /// reason.
    Admission { reason: RejectReason },
    /// The runtime declined the submission before admission. Nothing was
    /// admitted by this call.
    Runtime { error: RuntimeDriverError },
}

/// Total typed fate of one bounded submission.
///
/// Every path through [`submit_bounded`] lands on exactly one of these; there
/// is no untyped escape hatch and no unbounded wait.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BoundedSubmitOutcome {
    /// The runtime admitted the work within the bound. `state` is what the
    /// machine says about it: normally [`AdmittedWorkState::Pending`], but an
    /// admission can land already terminal, and this reports that rather than
    /// promising a run.
    Admitted {
        input_id: InputId,
        durability: AdmissionDurability,
        state: AdmittedWorkState,
    },
    /// This submission collapsed onto an admission already made under the same
    /// idempotency key. There is exactly one admitted input, and therefore at
    /// most one execution; which call created it is not distinguished because
    /// it does not change that fact.
    ///
    /// `state` is the half a bare collapse cannot tell you: the key outlives
    /// the work, so the row collapsed onto may be pending, already run, or
    /// terminal without ever having run.
    Collapsed {
        input_id: InputId,
        durability: AdmissionDurability,
        state: AdmittedWorkState,
    },
    /// The runtime declined the work. Nothing was admitted by this call.
    Refused { reason: SubmitRefusal },
    /// The submission went unanswered. `disposition` states what durable
    /// evidence says about the work right now.
    TimedOut {
        cause: SubmitTimeoutCause,
        disposition: SubmitTimeoutDisposition,
    },
}

/// One bounded submission and what came of it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BoundedSubmitReport {
    /// The key this submission collapsed on.
    pub idempotency_key: IdempotencyKey,
    /// The bound the caller supplied (or the documented default).
    pub bound: SubmitBound,
    /// The typed fate.
    pub outcome: BoundedSubmitOutcome,
}

impl BoundedSubmitReport {
    /// The input the runtime is holding for this key, when one is known.
    ///
    /// `None` for refusals and for a timeout with no durable witness.
    #[must_use]
    pub const fn admitted_input_id(&self) -> Option<&InputId> {
        match &self.outcome {
            BoundedSubmitOutcome::Admitted { input_id, .. }
            | BoundedSubmitOutcome::Collapsed { input_id, .. }
            | BoundedSubmitOutcome::TimedOut {
                disposition: SubmitTimeoutDisposition::DurablyAdmitted { input_id, .. },
                ..
            } => Some(input_id),
            BoundedSubmitOutcome::Refused { .. } | BoundedSubmitOutcome::TimedOut { .. } => None,
        }
    }

    /// What generated machine authority says about the work behind this key.
    ///
    /// `None` when there is no input to say anything about: a refusal, or a
    /// timeout with no durable witness.
    #[must_use]
    pub const fn work_state(&self) -> Option<AdmittedWorkState> {
        match &self.outcome {
            BoundedSubmitOutcome::Admitted { state, .. }
            | BoundedSubmitOutcome::Collapsed { state, .. }
            | BoundedSubmitOutcome::TimedOut {
                disposition: SubmitTimeoutDisposition::DurablyAdmitted { state, .. },
                ..
            } => Some(*state),
            BoundedSubmitOutcome::Refused { .. } | BoundedSubmitOutcome::TimedOut { .. } => None,
        }
    }

    /// Whether the runtime is durably holding work that still owes a run.
    ///
    /// Both halves are load-bearing. Durability comes from the store-owned
    /// index, so the work survives a restart; the state comes from machine
    /// authority, so a terminal row - completed, cancelled, abandoned,
    /// superseded, coalesced - answers false. A durable row for a cancelled
    /// input is exactly the "accepted, never moves" shape, and answering true
    /// for one is how a host is told to stop waiting on nothing.
    ///
    /// False is therefore not one situation but three, and they need opposite
    /// responses: work that already ran (do nothing), work that will never run
    /// (resubmit under a NEW key), and an unknown fate (retry under the SAME
    /// key). Branch on [`Self::work_state`] before acting on a false here;
    /// treating them alike either loses the work or delivers it twice.
    #[must_use]
    pub const fn is_durably_queued(&self) -> bool {
        match &self.outcome {
            BoundedSubmitOutcome::Admitted {
                durability, state, ..
            }
            | BoundedSubmitOutcome::Collapsed {
                durability, state, ..
            } => matches!(durability, AdmissionDurability::Durable) && state.is_pending(),
            BoundedSubmitOutcome::TimedOut { disposition, .. } => matches!(
                disposition,
                SubmitTimeoutDisposition::DurablyAdmitted {
                    state: AdmittedWorkState::Pending,
                    ..
                }
            ),
            BoundedSubmitOutcome::Refused { .. } => false,
        }
    }

    /// Whether this submission resolved onto an input that reached a terminal
    /// state without ever running.
    ///
    /// The host-visible consequence: no reply is coming, and retrying under
    /// this same idempotency key collapses onto the same dead row. Work that
    /// still has to happen needs a new key.
    #[must_use]
    pub const fn is_terminal_without_delivery(&self) -> bool {
        matches!(
            self.work_state(),
            Some(AdmittedWorkState::TerminalWithoutDelivery { .. })
        )
    }
}

/// Submit `input` under a caller-supplied bound and a mandatory idempotency
/// key, and return a typed fate for every path.
///
/// The bound is on the caller's observation, never on the work. The admission
/// runs on its own task, so an expired bound cannot drop-cancel a durable
/// admission mid-commit (which would strand the runtime's own compensation) and
/// cannot turn a slow-but-successful delivery into a lost one. A retry after an
/// expired bound must reuse the same [`BoundedSubmission::idempotency_key`]; it
/// then collapses at the durable admission point instead of delivering twice.
///
/// The call returns within [`BoundedSubmission::bound`], except for a bound
/// below [`MIN_EVIDENCE_READ_BOUND`], which cannot fund the classification
/// slice and so returns within [`MIN_EVIDENCE_READ_BOUND`] instead.
#[must_use = "a bounded submission's typed fate is the point of calling it"]
pub async fn submit_bounded<R>(
    runtime: Arc<R>,
    session_id: &SessionId,
    input: Input,
    submission: BoundedSubmission,
) -> BoundedSubmitReport
where
    R: SessionServiceRuntimeExt + ?Sized + 'static,
{
    let BoundedSubmission {
        idempotency_key,
        bound,
    } = submission;

    let input = match stamp_idempotency_key(input, &idempotency_key) {
        Ok(input) => input,
        Err(carried) => {
            let outcome = BoundedSubmitOutcome::Refused {
                reason: SubmitRefusal::IdempotencyKeyConflict {
                    supplied: idempotency_key.clone(),
                    carried,
                },
            };
            log_submit_outcome(session_id, &idempotency_key, bound, &outcome);
            return BoundedSubmitReport {
                idempotency_key,
                bound,
                outcome,
            };
        }
    };

    let (admission_bound, evidence_bound) = bound.split();
    let mut admission = {
        let runtime = Arc::clone(&runtime);
        let admitting_session_id = session_id.clone();
        crate::tokio::spawn(async move { runtime.accept_input(&admitting_session_id, input).await })
    };

    // A zero admission slice means the caller asked not to wait at all, and it
    // is answered without touching the timer. Racing a zero-length timer
    // against a task that may or may not have been scheduled yet would make a
    // "do not wait" call report whichever won, which is a scheduling detail no
    // caller can act on.
    if admission_bound.is_zero() {
        observe_abandoned_admission(admission, session_id.clone(), idempotency_key.clone());
        let outcome = BoundedSubmitOutcome::TimedOut {
            cause: SubmitTimeoutCause::BoundExpired,
            disposition: timeout_disposition(
                durable_evidence(
                    runtime.as_ref(),
                    session_id,
                    &idempotency_key,
                    evidence_bound,
                )
                .await,
            ),
        };
        log_submit_outcome(session_id, &idempotency_key, bound, &outcome);
        return BoundedSubmitReport {
            idempotency_key,
            bound,
            outcome,
        };
    }

    // The bound is observed through `&mut`, so an expiry leaves the handle
    // intact: the task is never aborted, and its answer stays reachable for an
    // observer instead of being dropped on the floor.
    let outcome = match crate::tokio::time::timeout(admission_bound, &mut admission).await {
        Ok(Ok(Ok(accepted))) => {
            classify_admission(
                runtime.as_ref(),
                session_id,
                &idempotency_key,
                evidence_bound,
                accepted,
            )
            .await
        }
        Ok(Ok(Err(error))) => {
            classify_admission_error(
                runtime.as_ref(),
                session_id,
                &idempotency_key,
                evidence_bound,
                error,
            )
            .await
        }
        Ok(Err(join_error)) => {
            classify_admission_error(
                runtime.as_ref(),
                session_id,
                &idempotency_key,
                evidence_bound,
                RuntimeDriverError::Internal(format!(
                    "runtime admission task did not complete: {join_error}"
                )),
            )
            .await
        }
        Err(_elapsed) => {
            observe_abandoned_admission(admission, session_id.clone(), idempotency_key.clone());
            BoundedSubmitOutcome::TimedOut {
                cause: SubmitTimeoutCause::BoundExpired,
                disposition: timeout_disposition(
                    durable_evidence(
                        runtime.as_ref(),
                        session_id,
                        &idempotency_key,
                        evidence_bound,
                    )
                    .await,
                ),
            }
        }
    };

    log_submit_outcome(session_id, &idempotency_key, bound, &outcome);
    BoundedSubmitReport {
        idempotency_key,
        bound,
        outcome,
    }
}

/// Watch an admission whose caller stopped observing.
///
/// The caller was answered at its bound and will never look at this task
/// again, so without an observer a slow admission that eventually FAILS
/// produces no log line, no event, and no metric: the work was never admitted
/// and nobody in the process can see that. Silence is the failure mode this
/// contract exists to remove, so the fate is logged even though no caller
/// wants it.
fn observe_abandoned_admission(
    admission: crate::tokio::task::JoinHandle<Result<AcceptOutcome, RuntimeDriverError>>,
    session_id: SessionId,
    idempotency_key: IdempotencyKey,
) {
    crate::tokio::spawn(async move {
        match admission.await {
            Ok(Ok(outcome)) => tracing::debug!(
                %session_id,
                %idempotency_key,
                ?outcome,
                "runtime admission answered after the caller's bound expired"
            ),
            Ok(Err(error)) => tracing::error!(
                %session_id,
                %idempotency_key,
                %error,
                "runtime admission failed after the caller's bound expired; the work was never \
                 admitted and no caller is waiting for this answer"
            ),
            Err(join_error) => tracing::error!(
                %session_id,
                %idempotency_key,
                %join_error,
                "runtime admission task ended without an answer after the caller's bound expired; \
                 whether the work was admitted is unknown"
            ),
        }
    });
}

/// Emit one line per bounded submission, loudest for the fates a host cannot
/// otherwise see.
///
/// A typed outcome only helps the caller that received it. The states that
/// stranded the field event - no definite answer, an expired bound, and a key
/// that resolves onto work which will never run - are the ones an operator has
/// to be able to find afterwards without the caller's cooperation.
fn log_submit_outcome(
    session_id: &SessionId,
    idempotency_key: &IdempotencyKey,
    bound: SubmitBound,
    outcome: &BoundedSubmitOutcome,
) {
    let bound_ms = bound.as_duration().as_millis();
    match outcome {
        BoundedSubmitOutcome::TimedOut {
            cause: SubmitTimeoutCause::RuntimeIndeterminate { error },
            disposition,
        } => tracing::error!(
            %session_id,
            %idempotency_key,
            bound_ms,
            ?disposition,
            %error,
            "bounded submit got no definite admission answer"
        ),
        BoundedSubmitOutcome::TimedOut { cause, disposition } => tracing::warn!(
            %session_id,
            %idempotency_key,
            bound_ms,
            ?cause,
            ?disposition,
            "bounded submit exceeded the caller-supplied bound without an admission answer"
        ),
        BoundedSubmitOutcome::Admitted {
            input_id,
            durability,
            state,
        }
        | BoundedSubmitOutcome::Collapsed {
            input_id,
            durability,
            state,
        } => match state {
            AdmittedWorkState::Pending | AdmittedWorkState::Delivered => tracing::debug!(
                %session_id,
                %idempotency_key,
                %input_id,
                ?durability,
                ?state,
                "bounded submit resolved to an admitted input"
            ),
            AdmittedWorkState::TerminalWithoutDelivery { .. } => {
                tracing::warn!(
                    %session_id,
                    %idempotency_key,
                    %input_id,
                    ?durability,
                    ?state,
                    "bounded submit resolved onto an input that will never run; a retry under \
                     this key collapses onto the same row, so work that still has to happen \
                     needs a new key"
                );
            }
            // Deliberately not the line above: an unclassifiable seed is an
            // unknown fate, not a dead one, and a log that says otherwise is
            // the same over-claim in a different place.
            AdmittedWorkState::Unclassified => {
                tracing::warn!(
                    %session_id,
                    %idempotency_key,
                    %input_id,
                    ?durability,
                    "bounded submit resolved onto an input the machine could not classify; \
                     whether its work is still coming is unknown"
                );
            }
        },
        BoundedSubmitOutcome::Refused { reason } => tracing::debug!(
            %session_id,
            %idempotency_key,
            ?reason,
            "bounded submit was refused"
        ),
    }
}

/// Durable evidence for one key, read from the store-owned input index.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DurableEvidence {
    /// A committed row exists, classified by machine authority. The row alone
    /// proves durability; only its state says whether anything will happen.
    Committed {
        input_id: InputId,
        state: AdmittedWorkState,
    },
    /// The index answered and held no row.
    Absent,
    /// The index could not answer, so absence proves nothing.
    Unavailable,
}

/// Classify one machine-owned input seed through generated authority.
///
/// This is the same generated projection every other surface publishes input
/// state with, so the bounded contract cannot drift from what the machine says,
/// and the cancelled-vs-abandoned distinction stays owned by the DSL rather
/// than re-derived here.
///
/// A projection failure means the seed's own facts are inconsistent - a
/// terminal phase with no terminal kind, say. That is reported loudly and
/// under-claimed as [`AdmittedWorkState::Unclassified`] rather than guessed in
/// either direction.
fn classify_work_state(input_id: &InputId, seed: &InputStateSeed) -> AdmittedWorkState {
    match crate::meerkat_machine::resolve_input_public_state_projection(input_id, seed) {
        Ok(projection) => match projection.terminal_outcome {
            None => AdmittedWorkState::Pending,
            Some(InputPublicTerminalOutcome::Completed) => AdmittedWorkState::Delivered,
            Some(outcome) => AdmittedWorkState::TerminalWithoutDelivery { outcome },
        },
        Err(error) => {
            tracing::error!(
                %input_id,
                %error,
                "MeerkatMachine could not classify an admitted input; whether its work is still \
                 coming is unknown"
            );
            AdmittedWorkState::Unclassified
        }
    }
}

/// Read durable evidence without ever failing the caller.
///
/// A classifier that propagates its own read error would hand the host back an
/// untyped fate at exactly the moment the contract exists to prevent one. An
/// uncertain or corrupt index is what [`DurableEvidence::Unavailable`] is for.
async fn durable_evidence<R>(
    runtime: &R,
    session_id: &SessionId,
    idempotency_key: &IdempotencyKey,
    bound: Duration,
) -> DurableEvidence
where
    R: SessionServiceRuntimeExt + ?Sized,
{
    match crate::tokio::time::timeout(
        bound,
        runtime.durable_input_state_by_idempotency_key(session_id, idempotency_key.0.as_str()),
    )
    .await
    {
        Ok(Ok(Some(stored))) => DurableEvidence::Committed {
            state: classify_work_state(&stored.state.input_id, &stored.seed),
            input_id: stored.state.input_id,
        },
        Ok(Ok(None)) => DurableEvidence::Absent,
        Ok(Err(_)) | Err(_) => DurableEvidence::Unavailable,
    }
}

fn admission_durability(evidence: &DurableEvidence) -> AdmissionDurability {
    match evidence {
        DurableEvidence::Committed { .. } => AdmissionDurability::Durable,
        DurableEvidence::Absent | DurableEvidence::Unavailable => {
            AdmissionDurability::ProcessLocalOnly
        }
    }
}

fn timeout_disposition(evidence: DurableEvidence) -> SubmitTimeoutDisposition {
    match evidence {
        DurableEvidence::Committed { input_id, state } => {
            SubmitTimeoutDisposition::DurablyAdmitted { input_id, state }
        }
        DurableEvidence::Absent => SubmitTimeoutDisposition::Unknown {
            cause: SubmitUnknownCause::NoDurableWitness,
        },
        DurableEvidence::Unavailable => SubmitTimeoutDisposition::Unknown {
            cause: SubmitUnknownCause::EvidenceUnavailable,
        },
    }
}

async fn classify_admission<R>(
    runtime: &R,
    session_id: &SessionId,
    idempotency_key: &IdempotencyKey,
    evidence_bound: Duration,
    accepted: AcceptOutcome,
) -> BoundedSubmitOutcome
where
    R: SessionServiceRuntimeExt + ?Sized,
{
    match accepted {
        AcceptOutcome::Accepted {
            input_id, ref seed, ..
        } => {
            let evidence =
                durable_evidence(runtime, session_id, idempotency_key, evidence_bound).await;
            BoundedSubmitOutcome::Admitted {
                durability: admission_durability(&evidence),
                state: classify_work_state(&input_id, seed),
                input_id,
            }
        }
        // The retained receipt, not a fresh read: a persistent runtime may
        // already have archived a terminal collapse target out of live state,
        // and this seed is the exact fact the admission decided on.
        AcceptOutcome::Deduplicated {
            existing_id,
            ref existing_seed,
            ..
        } => {
            let evidence =
                durable_evidence(runtime, session_id, idempotency_key, evidence_bound).await;
            BoundedSubmitOutcome::Collapsed {
                durability: admission_durability(&evidence),
                state: classify_work_state(&existing_id, existing_seed),
                input_id: existing_id,
            }
        }
        AcceptOutcome::Rejected { reason } => BoundedSubmitOutcome::Refused {
            reason: SubmitRefusal::Admission { reason },
        },
    }
}

/// Classify a failed admission against durable evidence FIRST.
///
/// An error raised after a durable claim already exists must never be reported
/// as a refusal: that is the restart-retry case, where the host resubmits under
/// the same key against a runtime that has since been torn down and the
/// committed row is the answer it needs.
///
/// The row's state is carried through for the same reason it is everywhere
/// else. A post-admission failure whose own compensation terminalized the row
/// it just wrote leaves committed evidence behind; reporting that as queued
/// work would tell the host to wait for a reply its runtime already gave up
/// on.
async fn classify_admission_error<R>(
    runtime: &R,
    session_id: &SessionId,
    idempotency_key: &IdempotencyKey,
    evidence_bound: Duration,
    error: RuntimeDriverError,
) -> BoundedSubmitOutcome
where
    R: SessionServiceRuntimeExt + ?Sized,
{
    let evidence = durable_evidence(runtime, session_id, idempotency_key, evidence_bound).await;
    if let DurableEvidence::Committed { input_id, state } = evidence {
        return BoundedSubmitOutcome::Collapsed {
            input_id,
            durability: AdmissionDurability::Durable,
            state,
        };
    }
    if is_definite_refusal(&error) {
        return BoundedSubmitOutcome::Refused {
            reason: SubmitRefusal::Runtime { error },
        };
    }
    BoundedSubmitOutcome::TimedOut {
        cause: SubmitTimeoutCause::RuntimeIndeterminate { error },
        disposition: timeout_disposition(evidence),
    }
}

/// Whether this error means the runtime evaluated the request and declined it
/// without attempting admission.
///
/// Conservative on purpose: only the pre-admission gates qualify. Everything
/// else (internal faults, recovery states, in-progress sagas whose own
/// documentation tells callers to retry and join) leaves the submission's fate
/// genuinely unknown, and must not be dressed up as a decision.
fn is_definite_refusal(error: &RuntimeDriverError) -> bool {
    matches!(
        error,
        RuntimeDriverError::ValidationFailed { .. }
            | RuntimeDriverError::NotReady { .. }
            | RuntimeDriverError::NotFound { .. }
            | RuntimeDriverError::Destroyed
            | RuntimeDriverError::StaleAuthority { .. }
    )
}

/// Stamp the caller-supplied key, or report the one already carried.
///
/// An input that already names a different key has two candidate identities and
/// therefore no single one; collapsing on either would be a guess.
fn stamp_idempotency_key(
    mut input: Input,
    idempotency_key: &IdempotencyKey,
) -> Result<Input, IdempotencyKey> {
    let header = input.header_mut();
    match &header.idempotency_key {
        Some(carried) if carried != idempotency_key => Err(carried.clone()),
        Some(_) => Ok(input),
        None => {
            header.idempotency_key = Some(idempotency_key.clone());
            Ok(input)
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use chrono::Utc;
    use meerkat_core::lifecycle::{InputId, RunId};
    use meerkat_core::types::SessionId;

    use super::*;
    use crate::input::{InputDurability, InputHeader, InputOrigin, InputVisibility, PromptInput};
    use crate::input_state::{
        InputAbandonReason, InputLifecycleState, InputState, InputStateSeed, InputTerminalOutcome,
        StoredInputState,
    };
    use crate::runtime_state::RuntimeState;

    fn prompt(text: &str) -> Input {
        Input::Prompt(PromptInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            content: meerkat_core::types::ContentInput::Text(text.to_string()),
            typed_turn_appends: Vec::new(),
            injected_context: Vec::new(),
            turn_metadata: None,
        })
    }

    fn stored(input_id: &InputId) -> StoredInputState {
        StoredInputState {
            state: InputState::new_accepted(input_id.clone()),
            seed: InputStateSeed::new_accepted(),
        }
    }

    /// A committed row whose work is over: the shape a key keeps pointing at
    /// long after anything is going to happen for it.
    fn stored_terminal(
        input_id: &InputId,
        phase: InputLifecycleState,
        outcome: InputTerminalOutcome,
    ) -> StoredInputState {
        StoredInputState {
            state: InputState::new_accepted(input_id.clone()),
            seed: InputStateSeed {
                phase,
                terminal_outcome: Some(outcome),
                ..InputStateSeed::new_accepted()
            },
        }
    }

    fn cancelled_seed() -> InputStateSeed {
        InputStateSeed {
            phase: InputLifecycleState::Abandoned,
            terminal_outcome: Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Cancelled,
            }),
            ..InputStateSeed::new_accepted()
        }
    }

    /// Controllable stand-in for a runtime adapter.
    ///
    /// Only the two seams the bounded submit actually uses are programmable:
    /// how long admission takes and what it answers, and what the durable index
    /// says. Everything else refuses, so a test cannot silently depend on a
    /// path this contract does not touch.
    struct ScriptedRuntime {
        admission_delay: Duration,
        admission:
            Box<dyn Fn() -> Result<AcceptOutcome, RuntimeDriverError> + Send + Sync + 'static>,
        durable: Box<
            dyn Fn() -> Result<Option<StoredInputState>, RuntimeDriverError>
                + Send
                + Sync
                + 'static,
        >,
        admissions_started: AtomicUsize,
        admissions_finished: AtomicUsize,
    }

    impl ScriptedRuntime {
        fn new() -> Self {
            Self {
                admission_delay: Duration::ZERO,
                admission: Box::new(|| {
                    Ok(AcceptOutcome::Accepted {
                        input_id: InputId::new(),
                        policy: accepted_policy(),
                        state: InputState::new_accepted(InputId::new()),
                        seed: InputStateSeed::new_accepted(),
                    })
                }),
                durable: Box::new(|| Ok(None)),
                admissions_started: AtomicUsize::new(0),
                admissions_finished: AtomicUsize::new(0),
            }
        }

        fn with_admission_delay(mut self, delay: Duration) -> Self {
            self.admission_delay = delay;
            self
        }

        fn with_admission(
            mut self,
            admission: impl Fn() -> Result<AcceptOutcome, RuntimeDriverError> + Send + Sync + 'static,
        ) -> Self {
            self.admission = Box::new(admission);
            self
        }

        fn with_durable(
            mut self,
            durable: impl Fn() -> Result<Option<StoredInputState>, RuntimeDriverError>
            + Send
            + Sync
            + 'static,
        ) -> Self {
            self.durable = Box::new(durable);
            self
        }
    }

    fn accepted_policy() -> crate::policy::PolicyDecision {
        use crate::policy::{
            ApplyMode, ConsumePoint, DrainPolicy, PolicyDecision, QueueMode, RoutingDisposition,
            WakeMode,
        };
        PolicyDecision {
            apply_mode: ApplyMode::StageRunStart,
            wake_mode: WakeMode::WakeIfIdle,
            queue_mode: QueueMode::Fifo,
            consume_point: ConsumePoint::OnRunComplete,
            drain_policy: DrainPolicy::QueueNextTurn,
            routing_disposition: RoutingDisposition::Queue,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: crate::identifiers::PolicyVersion(1),
        }
    }

    fn unsupported(method: &str) -> RuntimeDriverError {
        RuntimeDriverError::Internal(format!("{method} is out of scope for ScriptedRuntime"))
    }

    #[async_trait::async_trait]
    impl SessionServiceRuntimeExt for ScriptedRuntime {
        async fn accept_input(
            &self,
            _session_id: &SessionId,
            _input: Input,
        ) -> Result<AcceptOutcome, RuntimeDriverError> {
            self.admissions_started.fetch_add(1, Ordering::SeqCst);
            if !self.admission_delay.is_zero() {
                tokio::time::sleep(self.admission_delay).await;
            }
            let outcome = (self.admission)();
            self.admissions_finished.fetch_add(1, Ordering::SeqCst);
            outcome
        }

        async fn accept_input_with_completion(
            &self,
            _session_id: &SessionId,
            _input: Input,
        ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
        {
            Err(unsupported("accept_input_with_completion"))
        }

        async fn runtime_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<RuntimeState, RuntimeDriverError> {
            Err(unsupported("runtime_state"))
        }

        async fn retire_runtime(
            &self,
            _session_id: &SessionId,
        ) -> Result<crate::traits::RetireReport, RuntimeDriverError> {
            Err(unsupported("retire_runtime"))
        }

        async fn reset_runtime(
            &self,
            _session_id: &SessionId,
        ) -> Result<crate::traits::ResetReport, RuntimeDriverError> {
            Err(unsupported("reset_runtime"))
        }

        async fn input_state(
            &self,
            _session_id: &SessionId,
            _input_id: &InputId,
        ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
            Err(unsupported("input_state"))
        }

        async fn input_terminal_completion(
            &self,
            _session_id: &SessionId,
            _input_id: &InputId,
        ) -> Result<Option<crate::completion::CompletionOutcome>, RuntimeDriverError> {
            Err(unsupported("input_terminal_completion"))
        }

        async fn input_state_by_idempotency_key(
            &self,
            _session_id: &SessionId,
            _idempotency_key: &str,
        ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
            Err(unsupported("input_state_by_idempotency_key"))
        }

        async fn durable_input_state_by_idempotency_key(
            &self,
            _session_id: &SessionId,
            _idempotency_key: &str,
        ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
            (self.durable)()
        }

        async fn interaction_terminal_status(
            &self,
            _session_id: &SessionId,
            _selector: crate::terminal_status::InteractionSelector,
        ) -> Result<
            Option<
                crate::terminal_status::Sourced<crate::terminal_status::InteractionTerminalReport>,
            >,
            RuntimeDriverError,
        > {
            Err(unsupported("interaction_terminal_status"))
        }

        async fn run_terminal_status(
            &self,
            _session_id: &SessionId,
            _run_id: &RunId,
        ) -> Result<
            crate::terminal_status::Sourced<crate::terminal_status::RunTerminalReport>,
            RuntimeDriverError,
        > {
            Err(unsupported("run_terminal_status"))
        }

        async fn list_active_inputs(
            &self,
            _session_id: &SessionId,
        ) -> Result<Vec<InputId>, RuntimeDriverError> {
            Err(unsupported("list_active_inputs"))
        }

        async fn reconfigure_session_llm_identity(
            &self,
            _session_id: &SessionId,
            _request: crate::meerkat_machine_types::SessionLlmReconfigureRequest,
        ) -> Result<crate::meerkat_machine_types::SessionLlmReconfigureReport, RuntimeDriverError>
        {
            Err(unsupported("reconfigure_session_llm_identity"))
        }
    }

    #[test]
    fn absent_bound_uses_the_documented_default() {
        let submission = BoundedSubmission::new(IdempotencyKey::new("k"));
        assert_eq!(submission.bound(), SubmitBound::default());
        assert_eq!(submission.bound().as_duration(), DEFAULT_SUBMIT_BOUND);
    }

    #[test]
    fn caller_supplied_bound_is_honored_and_reserves_a_classification_slice() {
        let bound = SubmitBound::after(Duration::from_secs(20));
        let (admission, evidence) = bound.split();
        assert_eq!(evidence, Duration::from_secs(5));
        assert_eq!(admission, Duration::from_secs(15));
        assert_eq!(admission + evidence, bound.as_duration());
    }

    #[test]
    fn a_bound_too_small_to_split_still_funds_classification() {
        let bound = SubmitBound::after(Duration::from_millis(10));
        let (admission, evidence) = bound.split();
        assert_eq!(admission, Duration::ZERO);
        assert_eq!(evidence, MIN_EVIDENCE_READ_BOUND);
    }

    #[tokio::test]
    async fn expired_bound_reports_durably_queued_from_the_store_index() {
        let queued = InputId::new();
        let witness = queued.clone();
        let runtime = Arc::new(
            ScriptedRuntime::new()
                .with_admission_delay(Duration::from_secs(30))
                .with_durable(move || Ok(Some(stored(&witness)))),
        );

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("slow admission"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-42"))
                .with_bound(SubmitBound::after(Duration::from_millis(20))),
        )
        .await;

        match &report.outcome {
            BoundedSubmitOutcome::TimedOut {
                cause: SubmitTimeoutCause::BoundExpired,
                disposition:
                    SubmitTimeoutDisposition::DurablyAdmitted {
                        input_id,
                        state: AdmittedWorkState::Pending,
                    },
            } => assert_eq!(input_id, &queued),
            other => panic!("expected a durably-queued timeout, got {other:?}"),
        }
        assert!(report.is_durably_queued());
    }

    #[tokio::test]
    async fn an_expired_bound_over_a_cancelled_row_is_not_reported_as_queued() {
        let cancelled = InputId::new();
        let witness = cancelled.clone();
        let runtime = Arc::new(
            ScriptedRuntime::new()
                .with_admission_delay(Duration::from_secs(30))
                .with_durable(move || {
                    Ok(Some(stored_terminal(
                        &witness,
                        InputLifecycleState::Abandoned,
                        InputTerminalOutcome::Abandoned {
                            reason: InputAbandonReason::Cancelled,
                        },
                    )))
                }),
        );

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("slow admission over a cancelled key"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-50"))
                .with_bound(SubmitBound::after(Duration::from_millis(20))),
        )
        .await;

        match &report.outcome {
            BoundedSubmitOutcome::TimedOut {
                cause: SubmitTimeoutCause::BoundExpired,
                disposition:
                    SubmitTimeoutDisposition::DurablyAdmitted {
                        input_id,
                        state:
                            AdmittedWorkState::TerminalWithoutDelivery {
                                outcome: InputPublicTerminalOutcome::Cancelled,
                            },
                    },
            } => assert_eq!(input_id, &cancelled),
            other => panic!("expected a cancelled durable witness, got {other:?}"),
        }
        assert!(
            !report.is_durably_queued(),
            "a durable row for cancelled work is not queued work"
        );
        assert!(report.is_terminal_without_delivery());
    }

    #[tokio::test]
    async fn a_collapse_onto_cancelled_work_is_not_reported_as_queued() {
        let cancelled = InputId::new();
        let dedup_target = cancelled.clone();
        let witness = cancelled.clone();
        let runtime = Arc::new(
            ScriptedRuntime::new()
                .with_admission(move || {
                    Ok(AcceptOutcome::Deduplicated {
                        input_id: InputId::new(),
                        existing_id: dedup_target.clone(),
                        existing_seed: cancelled_seed(),
                    })
                })
                .with_durable(move || {
                    Ok(Some(stored_terminal(
                        &witness,
                        InputLifecycleState::Abandoned,
                        InputTerminalOutcome::Abandoned {
                            reason: InputAbandonReason::Cancelled,
                        },
                    )))
                }),
        );

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("retry of a cancelled message"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-51")),
        )
        .await;

        match &report.outcome {
            BoundedSubmitOutcome::Collapsed {
                input_id,
                durability: AdmissionDurability::Durable,
                state:
                    AdmittedWorkState::TerminalWithoutDelivery {
                        outcome: InputPublicTerminalOutcome::Cancelled,
                    },
            } => assert_eq!(input_id, &cancelled),
            other => panic!("expected a collapse onto cancelled work, got {other:?}"),
        }
        assert!(
            !report.is_durably_queued(),
            "collapsing onto a cancelled input must not report queued work"
        );
        assert!(report.is_terminal_without_delivery());
    }

    #[tokio::test]
    async fn a_collapse_onto_completed_work_reports_delivery_not_queueing() {
        let completed = InputId::new();
        let dedup_target = completed.clone();
        let witness = completed.clone();
        let runtime = Arc::new(
            ScriptedRuntime::new()
                .with_admission(move || {
                    Ok(AcceptOutcome::Deduplicated {
                        input_id: InputId::new(),
                        existing_id: dedup_target.clone(),
                        existing_seed: InputStateSeed {
                            phase: InputLifecycleState::Consumed,
                            terminal_outcome: Some(InputTerminalOutcome::Consumed),
                            ..InputStateSeed::new_accepted()
                        },
                    })
                })
                .with_durable(move || {
                    Ok(Some(stored_terminal(
                        &witness,
                        InputLifecycleState::Consumed,
                        InputTerminalOutcome::Consumed,
                    )))
                }),
        );

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("retry of a message that already ran"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-52")),
        )
        .await;

        match &report.outcome {
            BoundedSubmitOutcome::Collapsed {
                input_id,
                durability: AdmissionDurability::Durable,
                state: AdmittedWorkState::Delivered,
            } => assert_eq!(input_id, &completed),
            other => panic!("expected a collapse onto completed work, got {other:?}"),
        }
        assert!(
            !report.is_durably_queued(),
            "work that already ran is not queued work"
        );
        assert!(!report.is_terminal_without_delivery());
    }

    #[tokio::test]
    async fn an_inconsistent_seed_is_unclassified_rather_than_assumed_queued() {
        let inconsistent = InputId::new();
        let dedup_target = inconsistent.clone();
        let runtime = Arc::new(ScriptedRuntime::new().with_admission(move || {
            Ok(AcceptOutcome::Deduplicated {
                input_id: InputId::new(),
                existing_id: dedup_target.clone(),
                // A terminal phase with no terminal outcome matches no
                // generated projection: the seed's own facts disagree.
                existing_seed: InputStateSeed {
                    phase: InputLifecycleState::Abandoned,
                    ..InputStateSeed::new_accepted()
                },
            })
        }));

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("unclassifiable collapse target"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-53")),
        )
        .await;

        match &report.outcome {
            BoundedSubmitOutcome::Collapsed {
                input_id,
                state: AdmittedWorkState::Unclassified,
                ..
            } => assert_eq!(input_id, &inconsistent),
            other => panic!("expected an unclassified collapse target, got {other:?}"),
        }
        assert!(!report.is_durably_queued());
    }

    #[tokio::test]
    async fn expired_bound_reports_unknown_without_a_durable_witness() {
        let runtime = Arc::new(
            ScriptedRuntime::new()
                .with_admission_delay(Duration::from_secs(30))
                .with_durable(|| Ok(None)),
        );

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("slow admission"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-43"))
                .with_bound(SubmitBound::after(Duration::from_millis(20))),
        )
        .await;

        assert!(matches!(
            report.outcome,
            BoundedSubmitOutcome::TimedOut {
                cause: SubmitTimeoutCause::BoundExpired,
                disposition: SubmitTimeoutDisposition::Unknown {
                    cause: SubmitUnknownCause::NoDurableWitness
                },
            }
        ));
        assert!(!report.is_durably_queued());
    }

    #[tokio::test]
    async fn a_failing_evidence_read_is_unknown_rather_than_an_untyped_error() {
        let runtime = Arc::new(
            ScriptedRuntime::new()
                .with_admission_delay(Duration::from_secs(30))
                .with_durable(|| {
                    Err(RuntimeDriverError::RecoveryRepairBlocked {
                        evidence_digest: None,
                        reason: "durable idempotency-index corruption".to_string(),
                    })
                }),
        );

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("slow admission"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-44"))
                .with_bound(SubmitBound::after(Duration::from_millis(20))),
        )
        .await;

        assert!(matches!(
            report.outcome,
            BoundedSubmitOutcome::TimedOut {
                disposition: SubmitTimeoutDisposition::Unknown {
                    cause: SubmitUnknownCause::EvidenceUnavailable
                },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn an_expired_bound_never_cancels_the_admission() {
        let runtime =
            Arc::new(ScriptedRuntime::new().with_admission_delay(Duration::from_millis(150)));

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("slow but successful"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-45"))
                .with_bound(SubmitBound::after(Duration::from_millis(20))),
        )
        .await;
        assert!(matches!(
            report.outcome,
            BoundedSubmitOutcome::TimedOut { .. }
        ));
        assert_eq!(runtime.admissions_finished.load(Ordering::SeqCst), 0);

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            runtime.admissions_finished.load(Ordering::SeqCst),
            1,
            "the detached admission must still complete after the caller's bound expired"
        );
    }

    /// A caller that asks not to wait must get the same answer whether or not
    /// the admission task happened to be scheduled first. Anything else makes
    /// the reported fate a scheduling detail.
    #[tokio::test]
    async fn a_zero_bound_never_reports_an_admission_answer() {
        let runtime = Arc::new(ScriptedRuntime::new());

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("instant admission, zero bound"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-55"))
                .with_bound(SubmitBound::after(Duration::ZERO)),
        )
        .await;

        assert!(
            matches!(
                report.outcome,
                BoundedSubmitOutcome::TimedOut {
                    cause: SubmitTimeoutCause::BoundExpired,
                    disposition: SubmitTimeoutDisposition::Unknown {
                        cause: SubmitUnknownCause::NoDurableWitness
                    },
                }
            ),
            "a zero bound must not observe the admission, got {:?}",
            report.outcome
        );

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            runtime.admissions_finished.load(Ordering::SeqCst),
            1,
            "not waiting for the answer must not stop the work"
        );
    }

    #[tokio::test]
    async fn a_conflicting_carried_key_is_refused_without_submitting() {
        let runtime = Arc::new(ScriptedRuntime::new());
        let mut input = prompt("already identified");
        input.header_mut().idempotency_key = Some(IdempotencyKey::new("original"));

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            input,
            BoundedSubmission::new(IdempotencyKey::new("different")),
        )
        .await;

        match &report.outcome {
            BoundedSubmitOutcome::Refused {
                reason: SubmitRefusal::IdempotencyKeyConflict { supplied, carried },
            } => {
                assert_eq!(supplied, &IdempotencyKey::new("different"));
                assert_eq!(carried, &IdempotencyKey::new("original"));
            }
            other => panic!("expected a key-conflict refusal, got {other:?}"),
        }
        assert_eq!(runtime.admissions_started.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_pre_admission_refusal_is_typed_and_definite() {
        let runtime = Arc::new(ScriptedRuntime::new().with_admission(|| {
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Stopped,
            })
        }));

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("refused"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-46")),
        )
        .await;

        assert!(matches!(
            report.outcome,
            BoundedSubmitOutcome::Refused {
                reason: SubmitRefusal::Runtime {
                    error: RuntimeDriverError::NotReady { .. }
                }
            }
        ));
    }

    #[tokio::test]
    async fn an_indeterminate_runtime_failure_is_not_dressed_up_as_a_refusal() {
        let runtime = Arc::new(
            ScriptedRuntime::new()
                .with_admission(|| Err(RuntimeDriverError::Internal("wedged".to_string()))),
        );

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("indeterminate"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-47")),
        )
        .await;

        assert!(matches!(
            report.outcome,
            BoundedSubmitOutcome::TimedOut {
                cause: SubmitTimeoutCause::RuntimeIndeterminate { .. },
                disposition: SubmitTimeoutDisposition::Unknown { .. },
            }
        ));
    }

    #[tokio::test]
    async fn a_failure_after_a_durable_claim_collapses_instead_of_refusing() {
        let queued = InputId::new();
        let witness = queued.clone();
        let runtime = Arc::new(
            ScriptedRuntime::new()
                .with_admission(|| {
                    Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    })
                })
                .with_durable(move || Ok(Some(stored(&witness)))),
        );

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("retry after restart"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-48")),
        )
        .await;

        match &report.outcome {
            BoundedSubmitOutcome::Collapsed {
                input_id,
                durability: AdmissionDurability::Durable,
                state: AdmittedWorkState::Pending,
            } => assert_eq!(input_id, &queued),
            other => panic!("expected a durable collapse, got {other:?}"),
        }
        assert!(report.is_durably_queued());
    }

    /// The compensation path: a post-admission failure whose own rollback
    /// terminalized the row it had already committed. Durable evidence exists,
    /// so this is not a refusal - but nothing is coming, and the report has to
    /// say so on the very first call.
    #[tokio::test]
    async fn a_failure_that_compensated_its_own_admission_reports_dead_work() {
        let compensated = InputId::new();
        let witness = compensated.clone();
        let runtime = Arc::new(
            ScriptedRuntime::new()
                .with_admission(|| {
                    Err(RuntimeDriverError::Internal(
                        "post-admission staging failed; accepted input terminalized".to_string(),
                    ))
                })
                .with_durable(move || {
                    Ok(Some(stored_terminal(
                        &witness,
                        InputLifecycleState::Abandoned,
                        InputTerminalOutcome::Abandoned {
                            reason: InputAbandonReason::Cancelled,
                        },
                    )))
                }),
        );

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("compensated admission"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-54")),
        )
        .await;

        match &report.outcome {
            BoundedSubmitOutcome::Collapsed {
                input_id,
                durability: AdmissionDurability::Durable,
                state:
                    AdmittedWorkState::TerminalWithoutDelivery {
                        outcome: InputPublicTerminalOutcome::Cancelled,
                    },
            } => assert_eq!(input_id, &compensated),
            other => panic!("expected a compensated-admission collapse, got {other:?}"),
        }
        assert!(!report.is_durably_queued());
        assert!(report.is_terminal_without_delivery());
    }

    #[tokio::test]
    async fn admission_without_durable_evidence_is_reported_process_local() {
        let admitted = InputId::new();
        let echoed = admitted.clone();
        let runtime = Arc::new(
            ScriptedRuntime::new()
                .with_admission(move || {
                    Ok(AcceptOutcome::Accepted {
                        input_id: echoed.clone(),
                        policy: accepted_policy(),
                        state: InputState::new_accepted(echoed.clone()),
                        seed: InputStateSeed::new_accepted(),
                    })
                })
                .with_durable(|| Ok(None)),
        );

        let report = submit_bounded(
            Arc::clone(&runtime),
            &SessionId::new(),
            prompt("ephemeral admission"),
            BoundedSubmission::new(IdempotencyKey::new("telegram-49")),
        )
        .await;

        match &report.outcome {
            BoundedSubmitOutcome::Admitted {
                input_id,
                durability: AdmissionDurability::ProcessLocalOnly,
                state: AdmittedWorkState::Pending,
            } => assert_eq!(input_id, &admitted),
            other => panic!("expected a process-local admission, got {other:?}"),
        }
        assert!(!report.is_durably_queued());
    }
}
