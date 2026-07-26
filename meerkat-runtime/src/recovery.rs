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
//! - `SessionDocumentMachine` CLASSIFIES the tail (in `meerkat-session`).
//! - `MeerkatMachine` AUTHORIZES recovery — here, by driving the production
//!   generated authority with `AuthorizeDurableTailRecovery`.
//! - `RuntimeStore::atomic_apply` REALIZES the boundary: recovered snapshot,
//!   recovered receipt, and input-lifecycle terminalization in one atomic
//!   commit.
//! - No shell promotes or discards the tail.

use crate::identifiers::LogicalRuntimeId;
use crate::input_state::{InputLifecycleState, InputStatePersistenceRecord};
use crate::meerkat_machine::dsl as mm_dsl;
use crate::runtime_state::RuntimeState;
use crate::store::{RuntimeStore, RuntimeStoreError, SessionDelta};
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::types::SessionId;

pub use mm_dsl::{DurableTailRecoveryClass, DurableTailRecoveryDisposition};

/// One sealed recovery commit request. The caller (meerkat-session) has
/// already classified the tail and stamped the recovered document; this
/// module owns authorization and the atomic boundary.
#[derive(Debug)]
pub struct DurableTailRecoveryRequest {
    pub session_id: SessionId,
    /// Opaque candidate identity binding the exact evidence (session,
    /// authority stamp, store-head digest, CAS token, observed run identity).
    pub candidate_id: String,
    /// The run identity observed on the durable tail.
    pub candidate_run_id: meerkat_core::RunId,
    /// SessionDocumentMachine's classification of the tail.
    pub class: DurableTailRecoveryClass,
    /// Serialized recovered session document, already restamped with the
    /// recovered provenance anchored to the last committed authority.
    pub recovered_snapshot: Vec<u8>,
    /// Content digest of the recovered transcript, for the receipt.
    pub conversation_digest: String,
    /// Message count of the recovered transcript, for the receipt.
    pub message_count: usize,
    /// Text content of the USER messages inside the recovered tail, in tail
    /// order. Consumption evidence: the recovered boundary proves these
    /// inputs executed, so matching durable input records are terminalized
    /// with the commit — otherwise the delivery layer redelivers them and the
    /// turn runs twice (the exactly-once violation the cold-restart suite
    /// pins).
    pub tail_user_texts: Vec<String>,
}

/// Outcome of one authorization + commit attempt. Refusal and hold both
/// retain the durable tail intact; nothing here deletes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableTailRecoveryOutcome {
    /// The machine authorized the commit and `atomic_apply` succeeded.
    Committed(DurableTailRecoveryDisposition),
    /// The machine held the candidate intact (ambiguous evidence). Autonomy
    /// stays blocked; the tail clears only through reconciliation.
    Held,
    /// The machine refused (non-quiescent runtime or conflicting run facts).
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

/// Authorize a classified durable-tail recovery against the machine and, if
/// authorized, realize the atomic recovered boundary.
///
/// The persisted runtime lifecycle for a cold session is quiescent by
/// construction of the caller (`session_is_live == false` gated the
/// `RecoveryRequired` disposition), but the machine still owns the verdict: a
/// freshly registered authority (phase `Idle`, no current run, no recorded
/// terminal) is the faithful seed for a session with no live runtime, and the
/// generated guards decide from there. A non-quiescent persisted lifecycle is
/// mirrored into the machine drive being skipped and reported as `Refused` —
/// the same verdict `AuthorizeDurableTailRecoveryRefuseNonQuiescent` encodes
/// for a live machine.
pub async fn authorize_and_commit_durable_tail_recovery(
    store: &dyn RuntimeStore,
    persisted_lifecycle: Option<RuntimeState>,
    request: DurableTailRecoveryRequest,
) -> Result<DurableTailRecoveryOutcome, DurableTailRecoveryError> {
    // Non-quiescent persisted lifecycle: the machine's own
    // RefuseNonQuiescent transition answers this for a live instance; a cold
    // load with such a record must not fabricate a quiescent seed to get a
    // different answer.
    if let Some(state) = persisted_lifecycle
        && !matches!(state, RuntimeState::Idle | RuntimeState::Retired)
    {
        tracing::warn!(
            session_id = %request.session_id,
            persisted_lifecycle = %state,
            "durable-tail recovery refused: persisted runtime lifecycle is not quiescent"
        );
        return Ok(DurableTailRecoveryOutcome::Refused);
    }

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
        },
    )
    .map_err(|error| DurableTailRecoveryError::Authority(error.to_string()))?;
    let disposition = transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            mm_dsl::MeerkatMachineEffect::DurableTailRecoveryAuthorized {
                candidate_id,
                disposition,
            } if *candidate_id == request.candidate_id => Some(*disposition),
            _ => None,
        })
        .ok_or_else(|| {
            DurableTailRecoveryError::Authority(
                "generated machine returned no recovery disposition for the exact candidate"
                    .to_string(),
            )
        })?;

    match disposition {
        DurableTailRecoveryDisposition::RefuseRecovery => Ok(DurableTailRecoveryOutcome::Refused),
        DurableTailRecoveryDisposition::HoldIntact => Ok(DurableTailRecoveryOutcome::Held),
        DurableTailRecoveryDisposition::CommitCompleted
        | DurableTailRecoveryDisposition::RepairAndCommitInterrupted => {
            let runtime_id = LogicalRuntimeId::for_session(&request.session_id);
            // Terminalize input-lifecycle records the machine had already
            // bound to the candidate's run. Records bound to OTHER runs (or
            // not yet bound) are untouched: terminalizing an unrelated queued
            // input would silently drop work, and the recovered run's input —
            // if its record was never bound — cannot be distinguished from a
            // later queued input without receipt evidence that was never
            // written.
            let input_updates = terminalize_candidate_run_inputs(
                store,
                &runtime_id,
                &request.candidate_run_id,
                &request.tail_user_texts,
            )
            .await?;
            let receipt = RunBoundaryReceipt {
                run_id: request.candidate_run_id.clone(),
                // The recovered boundary applies at commit time; no live run
                // exists to carry a checkpoint position.
                boundary: RunApplyBoundary::Immediate,
                // Never written for the lost boundary, and unrecoverable from
                // the tail alone: contributing input ids live only in the
                // receipt that failed to commit.
                contributing_input_ids: Vec::new(),
                conversation_digest: Some(request.conversation_digest.clone()),
                message_count: request.message_count,
                // A recovered run has no committed receipts (atomic_apply
                // cannot half-commit), so sequence 1 cannot collide; the
                // (runtime_id, run_id, sequence) key also makes a concurrent
                // duplicate recovery attempt fail loudly instead of
                // double-committing.
                sequence: 1,
            };
            store
                .atomic_apply(
                    &runtime_id,
                    Some(SessionDelta {
                        session_snapshot: request.recovered_snapshot,
                    }),
                    receipt,
                    input_updates,
                    Some(request.session_id.clone()),
                )
                .await?;
            tracing::info!(
                session_id = %request.session_id,
                candidate_run_id = %request.candidate_run_id,
                ?disposition,
                message_count = request.message_count,
                "durable-tail recovery committed as a recovered runtime boundary"
            );
            Ok(DurableTailRecoveryOutcome::Committed(disposition))
        }
    }
}

/// Terminalize stored input-state bundles the recovered boundary consumed.
///
/// Two evidence classes, both fail-closed toward leaving records pending:
/// 1. Records already BOUND to the candidate run (`seed.last_run_id`).
/// 2. Records whose durable payload text equals a USER message in the
///    recovered tail — the tail is digest-verified proof those inputs
///    executed. Matching is count-bounded (each tail message consumes at
///    most one record), text-only (non-text payloads never match), and only
///    non-terminal records participate, so an identical input queued AFTER
///    the cut stays pending and is legitimately redelivered.
async fn terminalize_candidate_run_inputs(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    candidate_run_id: &meerkat_core::RunId,
    tail_user_texts: &[String],
) -> Result<Vec<InputStatePersistenceRecord>, DurableTailRecoveryError> {
    let stored = match store.load_input_states_strict(runtime_id).await {
        Ok(stored) => stored,
        Err(RuntimeStoreError::Unsupported(_)) => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut unconsumed_tail_texts: Vec<&String> = tail_user_texts.iter().collect();
    let mut updates = Vec::new();
    for mut bundle in stored {
        let terminal = matches!(
            bundle.seed.phase,
            InputLifecycleState::Consumed
                | InputLifecycleState::Superseded
                | InputLifecycleState::Coalesced
                | InputLifecycleState::Abandoned
        );
        if terminal {
            continue;
        }
        let bound_to_candidate = bundle.seed.last_run_id.as_ref() == Some(candidate_run_id);
        let consumed_by_tail = !bound_to_candidate
            && bundle
                .state
                .persisted_input
                .as_ref()
                .and_then(input_text_content)
                .is_some_and(|text| {
                    if let Some(position) =
                        unconsumed_tail_texts.iter().position(|tail| **tail == text)
                    {
                        unconsumed_tail_texts.remove(position);
                        true
                    } else {
                        false
                    }
                });
        if !bound_to_candidate && !consumed_by_tail {
            continue;
        }
        bundle.seed.phase = InputLifecycleState::Consumed;
        bundle.seed.terminal_outcome = Some(crate::input_state::InputTerminalOutcome::Consumed);
        // Terminal seeds carry no recovery lane — the generated authority
        // refuses a Consumed seed that still claims one.
        bundle.seed.recovery_lane = None;
        bundle.seed.last_run_id = Some(candidate_run_id.clone());
        bundle.seed.last_boundary_sequence = Some(1);
        let record = InputStatePersistenceRecord::from_machine_snapshot(bundle)
            .map_err(DurableTailRecoveryError::Authority)?;
        updates.push(record);
    }
    Ok(updates)
}

/// Text projection of a content-carrying input, `None` for non-content or
/// non-text inputs (which therefore never match tail evidence).
fn input_text_content(input: &crate::input::Input) -> Option<String> {
    match input {
        crate::input::Input::Prompt(prompt) => Some(prompt.content.text_content()),
        crate::input::Input::FlowStep(step) => Some(step.content.text_content()),
        crate::input::Input::Peer(peer) => Some(peer.content.text_content()),
        crate::input::Input::ExternalEvent(_)
        | crate::input::Input::Continuation(_)
        | crate::input::Input::Operation(_) => None,
    }
}
