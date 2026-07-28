//! Machine-authorized durable-tail recovery.
//!
//! When a durable store head is a verified strict descendant of the committed
//! runtime snapshot but carries intra-turn provenance, its tail is real turn
//! content whose boundary commit lost a race with shutdown. The recovery rule:
//! every verified durable descendant is preserved — recovery either commits it
//! as completed, closes it as interrupted, or holds it intact. It never rolls
//! back and never falsely marks an incomplete turn completed.
//!
//! Ownership split (see the recovery spec):
//! - `SessionDocumentMachine` CLASSIFIES the tail (in `meerkat-session`). Its
//!   `DurableTailClassified` effect is the only way to mint a
//!   [`DurableTailRecoveryRequest`].
//! - `MeerkatMachine` AUTHORIZES recovery — here, by driving the production
//!   generated authority with `AuthorizeDurableTailRecovery`, whose guards
//!   judge typed projections of the PERSISTED lifecycle row, the durably
//!   committed receipts, and the input-lifecycle rows, and whose commit arms
//!   mint the recovery boundary sequence.
//! - `RuntimeStore::atomic_apply_with_machine_lifecycle` REALIZES the
//!   boundary: recovered snapshot, recovered receipt, quiescent lifecycle
//!   re-commit (fenced on the exact observed row version), and fenced
//!   input-lifecycle terminalization in one atomic commit.
//! - No shell promotes, discards, or downgrades the tail: every disposition
//!   here is mirrored from an emitted machine verdict.
//!
//! Input identity is durable evidence only: a record is terminalized when the
//! persisted machine facts bound it to the candidate run, or when a durably
//! committed receipt for that run names it. Content matching is NOT identity —
//! two identical prompts are indistinguishable by text — so an unbound,
//! non-terminal, content-carrying input is reported to the machine as
//! unattributable evidence, and the machine holds the recovery intact.
//!
//! The one machine-owned exception is the LEGACY-ERA retain-inputs commit: a
//! pre-witness-v3 writer persisted staged run bindings only inside the
//! boundary commit, so its lost boundary routinely leaves the executed
//! turn's own input durably unbound — holding that shape wedges every
//! upgraded legacy session forever. For a clean COMPLETED candidate with
//! pre-witness-v3 stamp evidence the machine commits the digest-proven
//! transcript and terminalizes NOTHING: the unbound input stays in its own
//! lifecycle and redelivers normally (the legacy fleet's own restart
//! semantics — a possible duplicate turn, never a dropped input, never
//! fabricated consumption; content-matching a row as "consumed" could mark a
//! genuinely new identical prompt consumed, which silently drops user input
//! and is strictly worse than one duplicate reply).

use std::collections::BTreeSet;

use crate::identifiers::LogicalRuntimeId;
use crate::input_state::{InputLifecycleState, InputStatePersistenceRecord, StoredInputState};
use crate::meerkat_machine::dsl as mm_dsl;
use crate::runtime_state::RuntimeState;
use crate::store::{
    MachineLifecycleBindingFacts, MachineLifecycleCommit, MachineLifecycleExpectedVersion,
    MachineLifecycleObservation, MachineLifecycleRunFacts, RuntimeStore, RuntimeStoreError,
    SessionDelta, SupervisorAuthoritySnapshot,
};
use meerkat_core::lifecycle::InputId;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::types::SessionId;

pub use mm_dsl::{
    DurableRecoveryWriterEra, DurableTailRecoveryClass, DurableTailRecoveryDisposition,
};

/// One sealed recovery commit request.
///
/// The fields are private and the only constructor consumes the
/// SessionDocumentMachine's own `DurableTailClassified` effect: the class and
/// the candidate identity are READ OUT of the classifier verdict rather than
/// asserted by the caller, so a `RuntimeStore` holder cannot claim
/// `CompletedCandidate` for a candidate no classifier ever judged.
#[derive(Debug)]
pub struct DurableTailRecoveryRequest {
    session_id: SessionId,
    /// Opaque candidate identity binding the exact evidence (session,
    /// authority stamp, store-head digest, CAS token, observed run identity),
    /// taken from the classifier verdict.
    candidate_id: String,
    /// The run identity observed on the durable tail.
    candidate_run_id: meerkat_core::RunId,
    /// SessionDocumentMachine's classification of the tail.
    class: DurableTailRecoveryClass,
    /// Serialized recovered session document, already restamped with the
    /// recovered provenance anchored to the last committed authority.
    recovered_snapshot: Vec<u8>,
    /// Content digest of the recovered transcript, for the receipt.
    conversation_digest: String,
    /// Message count of the recovered transcript, for the receipt.
    message_count: usize,
    /// Stamp-schema era of the candidate head row's fully re-verified stamp,
    /// observed by the classifier shell under the recovery fence. Selects
    /// whether an unbound content input holds the commit (modern writers
    /// persist staged run bindings before execution) or is retained for
    /// ordinary redelivery (legacy writers bound inputs only inside the
    /// boundary commit this candidate lost).
    writer_era: DurableRecoveryWriterEra,
}

impl DurableTailRecoveryRequest {
    /// Mint a request from the classifier's own verdict.
    ///
    /// `verdict` must be the `DurableTailClassified` effect the
    /// SessionDocumentMachine emitted for this tail; the candidate id and the
    /// recovery class are taken from it, never from the caller. Any other
    /// effect is a typed authority error.
    ///
    /// Residual: the verdict binds CLASS to CANDIDATE ID, not to the recovered
    /// bytes — the effect carries no content digest. Closing that requires the
    /// classification effect itself to carry the head digest the classifier
    /// judged.
    #[allow(clippy::too_many_arguments)]
    pub fn from_classification(
        verdict: &meerkat_core::session_document::SessionDocumentEffect,
        session_id: SessionId,
        candidate_run_id: meerkat_core::RunId,
        recovered_snapshot: Vec<u8>,
        conversation_digest: String,
        message_count: usize,
        writer_era: DurableRecoveryWriterEra,
    ) -> Result<Self, DurableTailRecoveryError> {
        use meerkat_core::session_document::{
            DurableTailRecoveryClass as ClassifiedClass, SessionDocumentEffect,
        };

        let SessionDocumentEffect::DurableTailClassified {
            candidate_id,
            class,
        } = verdict
        else {
            return Err(DurableTailRecoveryError::Authority(
                "durable-tail recovery requires the SessionDocumentMachine's \
                 DurableTailClassified verdict"
                    .to_string(),
            ));
        };
        let class = match class {
            ClassifiedClass::CompletedCandidate => DurableTailRecoveryClass::CompletedCandidate,
            ClassifiedClass::InterruptedRepairableCandidate => {
                DurableTailRecoveryClass::InterruptedRepairableCandidate
            }
            // Legacy adoption: the candidate run identity is the shell's
            // domain-separated deterministic legacy run id (the tail itself
            // carries none), judged by the machine under the same receipt
            // and input-evidence guards as every other commit class.
            ClassifiedClass::LegacyCompletedCandidate => {
                DurableTailRecoveryClass::LegacyCompletedCandidate
            }
            ClassifiedClass::Ambiguous => DurableTailRecoveryClass::Ambiguous,
        };
        Ok(Self {
            session_id,
            candidate_id: candidate_id.clone(),
            candidate_run_id,
            class,
            recovered_snapshot,
            conversation_digest,
            message_count,
            writer_era,
        })
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn candidate_id(&self) -> &str {
        &self.candidate_id
    }

    pub fn candidate_run_id(&self) -> &meerkat_core::RunId {
        &self.candidate_run_id
    }

    pub fn class(&self) -> DurableTailRecoveryClass {
        self.class
    }
}

/// Outcome of one authorization + commit attempt. Refusal and hold both
/// retain the durable tail intact; nothing here deletes anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableTailRecoveryOutcome {
    /// The machine authorized the commit and the atomic boundary succeeded,
    /// with the machine-minted boundary sequence.
    Committed {
        disposition: DurableTailRecoveryDisposition,
        boundary_sequence: u64,
    },
    /// The candidate is held intact: ambiguous tail evidence, or input
    /// records whose identity cannot be proven durably. Autonomy stays
    /// blocked; the tail clears only through reconciliation.
    Held,
    /// The machine refused (non-quiescent persisted or in-process runtime,
    /// conflicting run facts, or durable receipts that already cover — or
    /// contradict — this candidate).
    Refused,
}

/// Typed error: authorization/commit mechanics failed (as opposed to the
/// machine refusing, which is an [`DurableTailRecoveryOutcome`]).
#[derive(Debug, thiserror::Error)]
pub enum DurableTailRecoveryError {
    #[error("recovery authorization could not be driven: {0}")]
    Authority(String),
    #[error("recovery commit failed: {0}")]
    Store(#[from] RuntimeStoreError),
}

/// Typed projections of the persisted machine-lifecycle row that recovery
/// observed, carried alongside the exact row version so the eventual commit
/// can fence on precisely the evidence the machine judged.
struct ObservedPersistedLifecycle {
    lifecycle: mm_dsl::DurableRecoveryObservedLifecycle,
    current_run: mm_dsl::DurableRecoveryObservedRun,
    expected_version: MachineLifecycleExpectedVersion,
    /// The lifecycle phase to re-assert on commit. Recovery never invents a
    /// phase: Missing rows create quiescent Idle; Idle stays Idle; Retired
    /// stays Retired (a retired runtime is not resurrected by recovering its
    /// session document).
    reassert_state: RuntimeState,
    binding: MachineLifecycleBindingFacts,
    supervisor_authority: SupervisorAuthoritySnapshot,
    unregister_progress: Option<crate::store::MachineUnregisterProgressSnapshot>,
}

fn observe_persisted_lifecycle(
    observation: MachineLifecycleObservation,
    candidate_run_id: &meerkat_core::RunId,
) -> ObservedPersistedLifecycle {
    match observation {
        MachineLifecycleObservation::Missing => ObservedPersistedLifecycle {
            lifecycle: mm_dsl::DurableRecoveryObservedLifecycle::MissingRow,
            current_run: mm_dsl::DurableRecoveryObservedRun::NoRun,
            expected_version: MachineLifecycleExpectedVersion::Missing,
            reassert_state: RuntimeState::Idle,
            binding: MachineLifecycleBindingFacts::default(),
            supervisor_authority: SupervisorAuthoritySnapshot::UnboundNoReceipt,
            unregister_progress: None,
        },
        MachineLifecycleObservation::Decoded { record, version } => {
            let (lifecycle, reassert_state) = match record.runtime_state() {
                Some(RuntimeState::Idle) => (
                    mm_dsl::DurableRecoveryObservedLifecycle::Idle,
                    RuntimeState::Idle,
                ),
                Some(RuntimeState::Retired) => (
                    mm_dsl::DurableRecoveryObservedLifecycle::Retired,
                    RuntimeState::Retired,
                ),
                Some(_) => (
                    mm_dsl::DurableRecoveryObservedLifecycle::NonQuiescent,
                    RuntimeState::Idle,
                ),
                // A decoded row without a lifecycle phase is a torn shape;
                // fail closed as undecodable evidence.
                None => (
                    mm_dsl::DurableRecoveryObservedLifecycle::Undecodable,
                    RuntimeState::Idle,
                ),
            };
            let current_run = match record.run().current_run_id() {
                None => mm_dsl::DurableRecoveryObservedRun::NoRun,
                Some(run_id) if run_id == candidate_run_id => {
                    mm_dsl::DurableRecoveryObservedRun::CandidateRun
                }
                Some(_) => mm_dsl::DurableRecoveryObservedRun::OtherRun,
            };
            ObservedPersistedLifecycle {
                lifecycle,
                current_run,
                expected_version: MachineLifecycleExpectedVersion::Version(version),
                reassert_state,
                binding: record.binding().clone(),
                supervisor_authority: record.supervisor_authority().clone(),
                unregister_progress: record.unregister_progress().cloned(),
            }
        }
        MachineLifecycleObservation::Unsupported { version, .. }
        | MachineLifecycleObservation::Malformed { version, .. } => ObservedPersistedLifecycle {
            lifecycle: mm_dsl::DurableRecoveryObservedLifecycle::Undecodable,
            current_run: mm_dsl::DurableRecoveryObservedRun::OtherRun,
            expected_version: MachineLifecycleExpectedVersion::Version(version),
            reassert_state: RuntimeState::Idle,
            binding: MachineLifecycleBindingFacts::default(),
            supervisor_authority: SupervisorAuthoritySnapshot::UnboundNoReceipt,
            unregister_progress: None,
        },
    }
}

/// Classify the highest durably committed boundary receipt for the candidate
/// run against the candidate transcript itself.
///
/// This is the only observation that can see a PRIOR SUCCESS of this same
/// recovery. The in-process `turn_terminal_run_id` is vacuous on cold recovery
/// (a freshly registered authority is driven), and receipt-key uniqueness
/// `(runtime_id, run_id, sequence)` fences only a SAME-sequence race: a second
/// process that observes the first recovery's receipt mints one past it and
/// would commit a phantom recovered boundary.
///
/// The message count carries the safety property on its own: a boundary
/// recording strictly fewer messages than the candidate cannot be a completed
/// recovery of the candidate, which commits the candidate's own count. The
/// digest, when the receipt recorded one, strengthens the classification.
fn classify_prior_commit(
    highest_committed: Option<&RunBoundaryReceipt>,
    conversation_digest: &str,
    message_count: usize,
) -> mm_dsl::DurableRecoveryPriorCommit {
    let Some(highest) = highest_committed else {
        return mm_dsl::DurableRecoveryPriorCommit::NoPriorCommit;
    };
    match highest.conversation_digest.as_deref() {
        Some(digest) if digest == conversation_digest && highest.message_count == message_count => {
            mm_dsl::DurableRecoveryPriorCommit::MatchesCandidate
        }
        Some(digest) if digest != conversation_digest && highest.message_count < message_count => {
            mm_dsl::DurableRecoveryPriorCommit::PrecedesCandidate
        }
        // Digest recorded but contradictory: the candidate's digest at a
        // different length, or different content at or past the candidate's
        // length. Neither ancestor nor equal — unattributable.
        Some(_) => mm_dsl::DurableRecoveryPriorCommit::DivergesFromCandidate,
        None if highest.message_count < message_count => {
            mm_dsl::DurableRecoveryPriorCommit::PrecedesCandidate
        }
        None if highest.message_count == message_count => {
            mm_dsl::DurableRecoveryPriorCommit::MatchesCandidate
        }
        None => mm_dsl::DurableRecoveryPriorCommit::DivergesFromCandidate,
    }
}

/// Authorize a classified durable-tail recovery against the machine and, if
/// authorized, realize the atomic recovered boundary.
///
/// The machine judges the DURABLE facts: this function observes the lifecycle
/// row, the committed receipts, and the input-lifecycle rows for the candidate
/// run, and feeds all three as typed inputs to the generated guards. A freshly
/// registered in-process authority alone would be vacuously quiescent — its
/// guards would authorize against an empty machine instead of durable truth.
/// Every observation is taken BEFORE the drive; afterwards the shell only
/// realizes the emitted verdict.
pub async fn authorize_and_commit_durable_tail_recovery(
    store: &dyn RuntimeStore,
    request: DurableTailRecoveryRequest,
) -> Result<DurableTailRecoveryOutcome, DurableTailRecoveryError> {
    let runtime_id = LogicalRuntimeId::for_session(&request.session_id);

    let observed = match store.observe_machine_lifecycle(&runtime_id).await {
        Ok(observation) => observe_persisted_lifecycle(observation, &request.candidate_run_id),
        // A store that cannot observe its lifecycle row cannot prove
        // quiescence; the machine refuses undecodable evidence.
        Err(RuntimeStoreError::Unsupported(_)) => ObservedPersistedLifecycle {
            lifecycle: mm_dsl::DurableRecoveryObservedLifecycle::Undecodable,
            current_run: mm_dsl::DurableRecoveryObservedRun::OtherRun,
            expected_version: MachineLifecycleExpectedVersion::Missing,
            reassert_state: RuntimeState::Idle,
            binding: MachineLifecycleBindingFacts::default(),
            supervisor_authority: SupervisorAuthoritySnapshot::UnboundNoReceipt,
            unregister_progress: None,
        },
        Err(error) => return Err(error.into()),
    };

    // Durably committed receipts for the candidate run: an interrupted tool
    // loop can have committed BoundaryContinue receipts before losing only
    // its final boundary. They carry (a) the last committed sequence the
    // machine mints past, (b) exact contributing input identities, and (c)
    // whether this exact recovery already landed.
    let committed_receipts = store
        .load_committed_boundary_receipts(&runtime_id, &request.candidate_run_id)
        .await?;
    let highest_committed = committed_receipts
        .iter()
        .max_by_key(|receipt| receipt.sequence);
    let last_committed_sequence = highest_committed
        .map(|receipt| receipt.sequence)
        .unwrap_or(0);
    let prior_commit = classify_prior_commit(
        highest_committed,
        &request.conversation_digest,
        request.message_count,
    );
    // Keyed by the id's string form: `InputId` itself is not `Ord`.
    let receipt_bound_inputs: BTreeSet<String> = committed_receipts
        .iter()
        .flat_map(|receipt| {
            receipt
                .contributing_input_ids
                .iter()
                .map(|input_id| input_id.to_string())
        })
        .collect();

    let inputs = observe_candidate_run_inputs(
        store,
        &runtime_id,
        &request.candidate_run_id,
        &receipt_bound_inputs,
    )
    .await?;

    let mut authority =
        crate::meerkat_machine::dsl_authority::new_registered_authority(&request.session_id)
            .map_err(|error| DurableTailRecoveryError::Authority(error.to_string()))?;
    let transition = mm_dsl::MeerkatMachineMutator::apply(
        &mut authority,
        mm_dsl::MeerkatMachineInput::AuthorizeDurableTailRecovery {
            session_id: mm_dsl::SessionId::from_domain(&request.session_id),
            candidate_id: request.candidate_id.clone(),
            candidate_run_id: mm_dsl::RunId(request.candidate_run_id.to_string()),
            class: request.class,
            observed_lifecycle: observed.lifecycle,
            observed_current_run: observed.current_run,
            last_committed_sequence,
            prior_commit,
            input_evidence: inputs.evidence,
            writer_era: request.writer_era,
        },
    )
    .map_err(|error| DurableTailRecoveryError::Authority(error.to_string()))?;

    let mut commit_verdict: Option<(DurableTailRecoveryDisposition, u64)> = None;
    let mut non_commit_verdict: Option<DurableTailRecoveryDisposition> = None;
    for effect in transition.effects() {
        match effect {
            mm_dsl::MeerkatMachineEffect::DurableTailRecoveryCommitAuthorized {
                candidate_id,
                disposition,
                boundary_sequence,
            } if *candidate_id == request.candidate_id => {
                commit_verdict = Some((*disposition, *boundary_sequence));
            }
            mm_dsl::MeerkatMachineEffect::DurableTailRecoveryAuthorized {
                candidate_id,
                disposition,
            } if *candidate_id == request.candidate_id => {
                non_commit_verdict = Some(*disposition);
            }
            _ => {}
        }
    }

    let (disposition, boundary_sequence) = match (commit_verdict, non_commit_verdict) {
        (Some((disposition, sequence)), _) => (disposition, sequence),
        (None, Some(DurableTailRecoveryDisposition::HoldIntact)) => {
            tracing::warn!(
                session_id = %request.session_id,
                candidate_run_id = %request.candidate_run_id,
                class = ?request.class,
                ?prior_commit,
                input_evidence = ?inputs.evidence,
                writer_era = ?request.writer_era,
                "durable-tail recovery held intact by machine verdict"
            );
            return Ok(DurableTailRecoveryOutcome::Held);
        }
        (None, Some(DurableTailRecoveryDisposition::RefuseRecovery)) => {
            return Ok(DurableTailRecoveryOutcome::Refused);
        }
        (None, Some(other)) => {
            return Err(DurableTailRecoveryError::Authority(format!(
                "generated machine emitted commit disposition {other:?} without a commit \
                 authorization effect"
            )));
        }
        (None, None) => {
            return Err(DurableTailRecoveryError::Authority(
                "generated machine returned no recovery disposition for the exact candidate"
                    .to_string(),
            ));
        }
    };

    // Realize-only pass: terminalize exactly the rows the observation PROVED
    // bound to the candidate run — durable staging bindings or a committed
    // boundary receipt naming them. Never more.
    //
    // This holds for the retain-inputs disposition too, and that is the point
    // of the word "retain": the unbound rows that set its evidence class are
    // retained for ordinary redelivery, while rows the same scan proved
    // consumed by the adopted tail are closed out. Clearing the attribution
    // wholesale here (as this once did) does not make the pass safer — it
    // strands proven-consumed rows non-terminal, and redelivery then
    // re-executes a turn the boundary just committed. Attribution is the
    // safety property; it is enforced by the observation, which only ever
    // attributes a row on durable run-binding evidence.
    let input_updates = terminalize_attributed_inputs(
        inputs.attributed,
        &request.candidate_run_id,
        boundary_sequence,
    )?;
    let contributing_input_ids: Vec<InputId> = input_updates
        .iter()
        .map(|record| record.as_stored().state.input_id.clone())
        .collect();

    let receipt = RunBoundaryReceipt {
        run_id: request.candidate_run_id.clone(),
        // The recovered boundary applies at commit time; no live run exists
        // to carry a checkpoint position.
        boundary: RunApplyBoundary::Immediate,
        contributing_input_ids,
        conversation_digest: Some(request.conversation_digest.clone()),
        message_count: request.message_count,
        // Machine-minted: one past the last durably committed receipt for
        // this run. The (runtime_id, run_id, sequence) key fences only a
        // SAME-sequence race; a recovery that already landed is fenced by the
        // machine's prior-commit guard, not by this key.
        sequence: boundary_sequence,
    };
    // Re-assert the observed quiescent lifecycle (never a new phase), fenced
    // on the exact row version the machine judged: if another process
    // registered or advanced the runtime since observation, the whole
    // boundary fails stale.
    let lifecycle = MachineLifecycleCommit::new_with_binding_run_and_unregister_progress(
        observed.reassert_state,
        observed.binding,
        MachineLifecycleRunFacts::default(),
        observed.supervisor_authority,
        observed.unregister_progress,
    )
    .with_expected_version(observed.expected_version);

    // Legacy-upgrade note: this recovery commit never needs the
    // caller-threaded history evidence of the one-time 0.8.8 -> 0.8.9
    // boundary (`atomic_apply_with_machine_lifecycle_and_legacy_history_evidence`).
    // The recovered document is BUILT FROM the committed runtime snapshot —
    // the durable tail is pushed onto a clone of that snapshot through the
    // session's own mutation seams (see `build_recovered` in
    // meerkat-session's durable-tail recovery) — so it always carries the
    // stored row's own transcript-history representation: inline over
    // inline for a pre-0.8.9 row, slim over slim after migration.
    store
        .atomic_apply_with_machine_lifecycle(
            &runtime_id,
            SessionDelta {
                session_snapshot: request.recovered_snapshot,
            },
            receipt,
            lifecycle,
            input_updates,
            request.session_id.clone(),
        )
        .await?;
    tracing::info!(
        session_id = %request.session_id,
        candidate_run_id = %request.candidate_run_id,
        ?disposition,
        boundary_sequence,
        message_count = request.message_count,
        "durable-tail recovery committed as a recovered runtime boundary"
    );
    Ok(DurableTailRecoveryOutcome::Committed {
        disposition,
        boundary_sequence,
    })
}

/// What the input-lifecycle rows say about the candidate run, plus the rows a
/// commit would terminalize. The evidence class is the machine's input; the
/// attributed rows are realized only after a commit verdict.
struct CandidateInputObservation {
    evidence: mm_dsl::DurableRecoveryInputEvidence,
    /// Non-terminal rows durable identity attributed to the candidate run,
    /// each paired with the row digest the commit fences on. Empty unless the
    /// evidence class is `AllBoundOrInert`.
    attributed: Vec<(StoredInputState, String)>,
}

fn is_terminal(phase: InputLifecycleState) -> bool {
    matches!(
        phase,
        InputLifecycleState::Consumed
            | InputLifecycleState::Superseded
            | InputLifecycleState::Coalesced
            | InputLifecycleState::Abandoned
    )
}

/// Does this input carry redeliverable content? A content input the delivery
/// layer would re-run duplicates the recovered turn if it was actually the
/// tail's input; a non-content input (operation, continuation, external
/// event) does not re-execute turn content.
fn carries_redeliverable_content(input: Option<&crate::input::Input>) -> bool {
    matches!(
        input,
        Some(
            crate::input::Input::Prompt(_)
                | crate::input::Input::FlowStep(_)
                | crate::input::Input::Peer(_)
        )
    )
}

/// Observation pass: classify the input-lifecycle evidence the machine judges,
/// on durable identity only.
///
/// Attribution classes:
/// 1. Records the persisted machine facts already bound to the candidate run
///    (`seed.last_run_id`).
/// 2. Records a durably committed receipt for the candidate run names in its
///    `contributing_input_ids`.
///
/// Anything else that is non-terminal and carries redeliverable content might
/// be the tail's own input whose binding never became durable — text equality
/// is content evidence, never identity — and is reported as
/// `UnboundContentInput`. A store that cannot version input rows while
/// blocking rows exist is reported as `Unfenceable`. The DISPOSITION for both
/// belongs to the machine.
async fn observe_candidate_run_inputs(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    candidate_run_id: &meerkat_core::RunId,
    receipt_bound_inputs: &BTreeSet<String>,
) -> Result<CandidateInputObservation, DurableTailRecoveryError> {
    let rows = match store.load_input_states_with_versions(runtime_id).await {
        Ok(rows) => rows,
        Err(RuntimeStoreError::Unsupported(_)) => {
            // No fenceable load: the strict read only distinguishes "nothing
            // the delivery layer could redeliver" from "blocking rows exist
            // that no commit here could fence".
            return match store.load_input_states_strict(runtime_id).await {
                Ok(rows) => {
                    let blocking = rows.iter().any(|bundle| {
                        !is_terminal(bundle.seed.phase)
                            && (bundle.seed.last_run_id.as_ref() == Some(candidate_run_id)
                                || carries_redeliverable_content(
                                    bundle.state.persisted_input.as_ref(),
                                ))
                    });
                    Ok(CandidateInputObservation {
                        evidence: if blocking {
                            mm_dsl::DurableRecoveryInputEvidence::Unfenceable
                        } else {
                            mm_dsl::DurableRecoveryInputEvidence::AllBoundOrInert
                        },
                        attributed: Vec::new(),
                    })
                }
                // A store that tracks no input states at all has nothing the
                // delivery layer could redeliver.
                Err(RuntimeStoreError::Unsupported(_)) => Ok(CandidateInputObservation {
                    evidence: mm_dsl::DurableRecoveryInputEvidence::AllBoundOrInert,
                    attributed: Vec::new(),
                }),
                Err(error) => Err(error.into()),
            };
        }
        Err(error) => return Err(error.into()),
    };

    // Scan EVERY row before deciding. An unbound content row sets the
    // evidence class, but it must not erase the rows this same scan proved
    // bound to the candidate run: those were consumed by the very tail being
    // adopted (durable staging bindings, or a committed boundary receipt
    // naming them). Returning early with an empty attribution — as this did
    // — strands them non-terminal, and the input lifecycle then rolls
    // Staged back to Queued and re-admits them, re-executing an
    // already-committed turn: a duplicate provider call with re-fired tool
    // side effects. Proven-bound rows are terminalized; only genuinely
    // unbound rows are retained for redelivery.
    //
    // Legacy documents are unaffected by construction: a pre-0.8.9 writer
    // persisted its staging bindings in memory only, so its rows carry no
    // `last_run_id` and no receipt binding, attribute nothing, and take the
    // identical retain-everything path they always did.
    let mut attributed = Vec::new();
    let mut unbound_content_input = false;
    for (bundle, row_digest) in rows {
        if is_terminal(bundle.seed.phase) {
            continue;
        }
        let bound_to_candidate = bundle.seed.last_run_id.as_ref() == Some(candidate_run_id)
            || receipt_bound_inputs.contains(&bundle.state.input_id.to_string());
        if !bound_to_candidate {
            if carries_redeliverable_content(bundle.state.persisted_input.as_ref()) {
                unbound_content_input = true;
            }
            continue;
        }
        attributed.push((bundle, row_digest));
    }
    Ok(CandidateInputObservation {
        evidence: if unbound_content_input {
            mm_dsl::DurableRecoveryInputEvidence::UnboundContentInput
        } else {
            mm_dsl::DurableRecoveryInputEvidence::AllBoundOrInert
        },
        attributed,
    })
}

/// Realize pass: terminalize the observed rows the recovered boundary
/// consumed, fenced on the exact row bytes the observation read. Called only
/// after a commit verdict, and only over rows the observation attributed.
fn terminalize_attributed_inputs(
    attributed: Vec<(StoredInputState, String)>,
    candidate_run_id: &meerkat_core::RunId,
    boundary_sequence: u64,
) -> Result<Vec<InputStatePersistenceRecord>, DurableTailRecoveryError> {
    let mut updates = Vec::with_capacity(attributed.len());
    for (mut bundle, row_digest) in attributed {
        bundle.seed.phase = InputLifecycleState::Consumed;
        bundle.seed.terminal_outcome = Some(crate::input_state::InputTerminalOutcome::Consumed);
        // Terminal seeds carry no recovery lane — the generated authority
        // refuses a Consumed seed that still claims one.
        bundle.seed.recovery_lane = None;
        bundle.seed.last_run_id = Some(candidate_run_id.clone());
        bundle.seed.last_boundary_sequence = Some(boundary_sequence);
        let record = InputStatePersistenceRecord::from_machine_snapshot(bundle)
            .map_err(DurableTailRecoveryError::Authority)?
            .with_expected_row_digest(row_digest);
        updates.push(record);
    }
    Ok(updates)
}
