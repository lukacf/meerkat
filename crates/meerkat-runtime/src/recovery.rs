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
//! - this module proves the exact committed authority, exact physical store
//!   head, strict descendant relation, and tail structure, then drives
//!   `SessionDocumentMachine` itself. Generated effects never cross the public
//!   API as caller-assembled authority.
//! - `MeerkatMachine` AUTHORIZES recovery — here, by driving the production
//!   generated authority with `AuthorizeDurableTailRecovery`, whose guards
//!   judge typed projections of the PERSISTED lifecycle row, the durably
//!   committed receipts, and the input-lifecycle rows, and whose commit arms
//!   mint the recovery boundary sequence.
//! - one opaque prepared-session boundary REALIZES the recovered document,
//!   exact receipt witness, quiescent lifecycle re-commit, physical
//!   `SessionStore` head CAS, runtime predecessor CAS, and fenced input
//!   terminalization in one atomic commit.
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
//! The one machine-owned exception is the retain-inputs commit: for a clean
//! COMPLETED candidate with an unbound content input the machine commits the
//! proved transcript and RETAINS the unbound row in its own lifecycle for
//! ordinary redelivery, terminalizing only rows the observation proved bound
//! to the recovered run. At the supported 0.8.10 state floor, execution is
//! fenced by a durable run binding first, so an unbound content row never
//! started and redelivering it is correct. Content matching must never be used
//! to manufacture consumption identity.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::identifiers::LogicalRuntimeId;
use crate::input_state::{InputLifecycleState, InputStatePersistenceRecord, StoredInputState};
use crate::meerkat_machine::dsl as mm_dsl;
use crate::runtime_state::RuntimeState;
use crate::store::{
    CommittedRecoveryBoundary, CommittedWholeBlobProvisionalTail, CommittedWholeBlobSnapshot,
    DurableTailSourceRelation, MachineLifecycleBindingFacts, MachineLifecycleCommit,
    MachineLifecycleExpectedVersion, MachineLifecycleObservation, MachineLifecycleRunFacts,
    PreparedDurableTailRecoverySource, PreparedRecoveryEvidence,
    PreparedRecoveryReceiptDigestEnrichment, PreparedRecoveryReceiptSource,
    PreparedRuntimeSessionCommit, RecoveryInputSetRevision, RuntimeSessionPersistenceProfile,
    RuntimeStore, RuntimeStoreError, SupervisorAuthoritySnapshot,
};
use meerkat_core::lifecycle::InputId;
use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::session_document::{
    DurableHeadRelation, DurableTailRecoveryClass as ClassifiedRecoveryClass, DurableTailRunCursor,
    DurableTailStopReason, RunIdCardinality, SessionDocumentEffect, SessionDocumentKey,
    SessionDocumentMachineAuthority,
};
use meerkat_core::session_store::PreparedHeadCanonicalMutation;
use meerkat_core::types::SessionId;
use meerkat_core::types::{BlockAssistantMessage, MessageTimestamp, ToolResult};
use meerkat_core::{
    Message, RunCheckpointAuthority, RunCheckpointReceipt, RunId, RunResumePoint, Session,
    StopReason, SystemNoticeKind, SystemNoticeMessage, ToolError,
};
use sha2::{Digest, Sha256};

pub use mm_dsl::{DurableTailRecoveryClass, DurableTailRecoveryDisposition};

/// Candidate hash sentinel for a current tail that carries no observed run
/// identity. It authorizes nothing: the generated classifier sees
/// `RunIdCardinality::NoRunId` and emits the Ambiguous/Hold verdict. The
/// sentinel only binds that exact absence into the candidate identity.
const NO_OBSERVED_RUN_IDENTITY: &str = "current:no-observed-run-identity";

/// Structural observation of exactly the messages after committed authority.
/// This is private because callers must not be able to assert the shape a
/// generated classifier judges.
#[derive(Debug)]
struct DurableTailObservation {
    tail_run_id: Option<RunId>,
    run_id_cardinality: RunIdCardinality,
    terminal_stop_reason: DurableTailStopReason,
    dangling_tool_use_ids: Vec<String>,
    orphan_tool_result_count: u64,
    messages_after_terminal: bool,
    /// What the tail's run cursor says against its transcript; `Absent`
    /// until the source preparation observes the cursor.
    run_cursor: DurableTailRunCursor,
    /// Whether every input the run cursor binds is still bound to its run.
    cursor_inputs_open: bool,
}

/// Fully evidence-bound recovery candidate.
///
/// Every field is derived in one private constructor from the exact committed
/// authority and exact observed physical head. In particular, no public seam
/// accepts a generated effect, class, run id, recovered document, store token,
/// digest, or receipt fact independently.
#[derive(Debug)]
enum PreparedRecoveryStoreTransition {
    WholeBlob {
        base_store_revision: u64,
        base_blob_sha256: String,
        provisional_candidate_blob_sha256: String,
        provisional_candidate_sequence: u64,
        recovered_blob_sha256: String,
    },
    HeadCanonical {
        committed_store_revision: u64,
        committed_head_token: String,
        physical_store_revision: u64,
        physical_head_cas_token: String,
        recovered_head_token: String,
    },
}

#[derive(Debug)]
struct PreparedRecoveryCandidate {
    session_id: SessionId,
    candidate_id: String,
    candidate_run_id: RunId,
    class: DurableTailRecoveryClass,
    store_transition: PreparedRecoveryStoreTransition,
    recovered: Arc<Session>,
    document: Option<BoundSessionCommit>,
    conversation_digest: String,
    message_count: usize,
    /// For a ContinuableCandidate only: what continuing its run needs.
    /// `recovered` and `document` then hold the interrupted repair a retired
    /// runtime commits instead.
    continuation: Option<PreparedContinuation>,
    /// The classifier's observations of the tail, as an operator reads them.
    tail_facts: HeldTailFacts,
    /// Messages the session holds before the recovered document's additions.
    physical_message_count: usize,
    /// The calls the recovered document answers as outcome unknown.
    closed_call_ids: Vec<String>,
    /// The inputs the tail's run cursor binds to its run, when the cursor
    /// agrees with the tail; `None` for a tail without a run cursor or with
    /// one that disagrees (a stale or unreadable one, which does not show
    /// what the tail holds). An operator's requeue of another run's stranded
    /// inputs needs an agreeing cursor and never touches one of these (see
    /// [`observe_stranded_run_inputs`]).
    cursor_bound_inputs: Option<Vec<InputId>>,
}

/// Which verdict a prepared candidate serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparationMode {
    /// Ordinary recovery: an ambiguous tail is held.
    Recovery,
    /// An operator's `resolve-held-tail`: an ambiguous tail is prepared as
    /// the interrupted run the operator attests it to be.
    Operator,
}

/// Who closes an interrupted run; its notice and results name the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InterruptedRunCloser<'a> {
    /// Crash recovery, and the settle of a run that ends without resuming.
    Recovery,
    /// An operator's `resolve-held-tail` on a held tail.
    Operator { candidate_id: &'a str },
}

/// What the durable-tail classifier observed in a tail: the facts that make
/// a held tail ambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HeldTailFacts {
    /// Run identity of the tail's rows: `single_run_id`, `no_run_id`,
    /// `multiple_run_ids` or `no_run_rows`.
    pub run_rows: &'static str,
    /// The last assistant row's stop: `end_turn`, `tool_use`, `other` or
    /// `absent`.
    pub last_stop: &'static str,
    /// Calls the transcript shows without a result.
    pub dangling_calls: Vec<String>,
    /// Results that answer no earlier call.
    pub orphan_results: u64,
    /// Whether rows follow an assistant row that ended the run.
    pub rows_after_terminal_reply: bool,
    /// The run cursor against the transcript: `agrees`, `disagrees` or
    /// `absent`.
    pub run_cursor: &'static str,
    /// Whether every input the run cursor binds is still bound to its run.
    pub cursor_inputs_bound: bool,
}

impl HeldTailFacts {
    fn observed(observation: &DurableTailObservation) -> Self {
        Self {
            run_rows: run_id_cardinality_name(observation.run_id_cardinality),
            last_stop: durable_tail_stop_reason_name(observation.terminal_stop_reason),
            dangling_calls: observation.dangling_tool_use_ids.clone(),
            orphan_results: observation.orphan_tool_result_count,
            rows_after_terminal_reply: observation.messages_after_terminal,
            run_cursor: durable_tail_run_cursor_name(observation.run_cursor),
            cursor_inputs_bound: observation.cursor_inputs_open,
        }
    }
}

/// What [`resolve_held_durable_tail`] found or did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HeldTailResolution {
    /// The store holds no provisional tail for the session: nothing is held.
    NoTail,
    /// The tail is not held: ordinary recovery commits or continues it when
    /// the session next resumes. Nothing was written.
    NotHeld { class: DurableTailRecoveryClass },
    /// The tail is held or refused for a reason this verb does not resolve,
    /// and nothing was written:
    /// - ordinary recovery holds it because its input rows cannot be
    ///   attributed to its run or fenced, or refuses it (a non-quiescent
    ///   runtime or a current run, or a prior commit that covers or
    ///   contradicts it);
    /// - the store cannot prove an exact recovery source or boundary, or the
    ///   tail's descent from the committed head (the operator attests a run,
    ///   never ancestry);
    /// - an incomplete HeadCanonical intent, which ordinary recovery rolls
    ///   back;
    /// - a recovery of this exact tail already committed.
    HeldOtherwise { reason: String },
    /// An ambiguous held tail and the operator repair, planned or applied.
    Ambiguous(Box<HeldTailRepair>),
    /// An interrupted tail that ordinary recovery holds only because content
    /// inputs of another run stand beside it, and the operator repair,
    /// planned or applied. Each such input is staged for, or queued after, a
    /// run that never committed (no provisional tail and no boundary
    /// receipt in the store), and the tail's run cursor does not bind it: that
    /// run binding is the proof that the input is not the tail's own. An
    /// input the cursor binds is the tail's own, whatever run it names now:
    /// the repair consumes it with the tail
    /// ([`HeldTailRepair::terminalized_inputs`]). An input that names no run
    /// is not in the tail and stays queued. The repair commits the tail as
    /// interrupted, as ordinary recovery commits an interrupted tail, and
    /// requeues the stranded inputs with their content
    /// ([`HeldTailRepair::requeued_inputs`]), so each runs once after it.
    /// Only a pre-release build of 0.9.5, whose cancelled run kept its tail,
    /// or a store fault, leaves this shape.
    StrandedInputs(Box<HeldTailRepair>),
}

/// The operator repair of a held tail: an ambiguous one
/// ([`HeldTailResolution::Ambiguous`]), or an interrupted one held beside
/// another run's stranded inputs ([`HeldTailResolution::StrandedInputs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HeldTailRepair {
    /// The exact candidate the repair commits, as the recovery row records it.
    pub candidate_id: String,
    /// The run the tail belongs to.
    pub run_id: RunId,
    /// Why the tail is held.
    pub facts: HeldTailFacts,
    /// Messages the session holds now; every one is kept.
    pub messages_before: usize,
    /// Messages after the repair: the kept rows plus the outcome-unknown
    /// results and the notice.
    pub messages_after: usize,
    /// The calls the repair answers as outcome unknown.
    pub closed_calls: Vec<String>,
    /// The inputs the repair terminalizes: those proved bound to the run
    /// (beside a run cursor, the inputs it binds). Other inputs stay for
    /// ordinary redelivery.
    pub terminalized_inputs: Vec<InputId>,
    /// The inputs the repair requeues, with their content, to run once after
    /// the repaired run: another run's stranded inputs
    /// ([`HeldTailResolution::StrandedInputs`]). Empty for any other repair.
    pub requeued_inputs: Vec<InputId>,
    /// Whether the persisted lifecycle row is quiescent (missing, Idle or
    /// Retired, or a Stopped row this repair normalizes) with no current
    /// run. The repair is refused otherwise. A live host whose runtime is
    /// idle is not detected, so stop every host first.
    pub runtime_quiescent: bool,
    /// Whether the persisted lifecycle row is `Stopped` with no current run
    /// and no unregister in progress: a host stopped the runtime and ended
    /// before it unregistered. The verb reads it as the clean `Idle` row a
    /// start's registration replaces it with (unbound, its supervisor,
    /// unregister-progress and live-bridge facts kept); a dry run writes
    /// nothing, and an applied repair writes that row in its own commit. A
    /// `Stopped` row with a current run or an unregister in progress is not
    /// quiescent, and the repair is refused.
    pub normalizes_stopped_lifecycle: bool,
    /// The machine's verdict.
    pub verdict: HeldTailRepairVerdict,
}

/// The machine's verdict on an operator repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HeldTailRepairVerdict {
    /// A dry run: the machine authorizes the repair, and nothing was written.
    WouldRepair,
    /// The machine refused the repair, and nothing was written.
    Refused,
    /// The repair committed at this recovered boundary sequence.
    Repaired { boundary_sequence: u64 },
}

/// What continuing a ContinuableCandidate's run needs: the provisional tail
/// itself, run cursor included, and the receipt its next checkpoint chains
/// from.
#[derive(Debug)]
struct PreparedContinuation {
    candidate: Arc<Session>,
    receipt: RunCheckpointReceipt,
}

enum RecoveryCandidatePreparation {
    Prepared(PreparedRecoveryCandidate),
    AlreadyAligned {
        recovered: Arc<Session>,
        /// The exact verified committed WholeBlob snapshot `recovered` was
        /// decoded from; `None` for HeadCanonical sources.
        committed_whole_blob: Option<crate::store::CommittedWholeBlobSnapshot>,
    },
    IncompleteHeadCanonicalIntent {
        provisional: crate::store::HeadCanonicalProvisionalTailAuthority,
    },
    Held,
}

fn observe_durable_tail(authority_len: usize, head: &Session) -> DurableTailObservation {
    let tail = &head.messages()[authority_len.min(head.messages().len())..];
    let mut run_ids = BTreeSet::<String>::new();
    let mut assistant_without_run_id = false;
    let mut tail_run_id = None;
    // Ordered multiset pairing. A result consumes one earlier call with the
    // same id; a result before its call is an orphan and duplicated ids retain
    // distinct obligations.
    let mut open_call_ids = Vec::<String>::new();
    let mut orphan_tool_result_count = 0_u64;
    let mut last_assistant_stop: Option<DurableTailStopReason> = None;
    let mut terminal_seen = false;
    let mut messages_after_terminal = false;
    let mut run_rows = false;
    for message in tail {
        // No typed transcript fact distinguishes a generated structured-output
        // continuation prompt from an ordinary User message. Fail closed:
        // once a non-ToolUse assistant has ended, every later message makes
        // the candidate ambiguous. A future extraction-phase identity can
        // narrow this without inferring authority from prompt text.
        if terminal_seen {
            messages_after_terminal = true;
        }
        match message {
            Message::BlockAssistant(assistant) => {
                run_rows = true;
                match assistant.identity.run_id.as_ref() {
                    Some(run_id) => {
                        run_ids.insert(run_id.to_string());
                        tail_run_id = Some(run_id.clone());
                    }
                    None => assistant_without_run_id = true,
                }
                open_call_ids.extend(assistant.tool_calls().map(|call| call.id.to_string()));
                let effective_stop = match assistant.stop_reason {
                    Some(StopReason::EndTurn) => DurableTailStopReason::EndTurn,
                    Some(StopReason::ToolUse) if assistant.has_tool_calls() => {
                        DurableTailStopReason::ToolUse
                    }
                    // The live agent decides the tool phase from actual call
                    // blocks, not the provider's stop label. A ToolUse label
                    // with no calls is operationally terminal and recovery
                    // must mirror that exact decision.
                    Some(StopReason::ToolUse) => DurableTailStopReason::EndTurn,
                    Some(_) => DurableTailStopReason::Other,
                    None => DurableTailStopReason::Absent,
                };
                last_assistant_stop = Some(effective_stop);
                // ToolUse is an intermediate provider boundary: durable tool
                // results and a later assistant continuation can belong to
                // the same run. Every other stop reason closes that run, so
                // any later message makes the candidate ambiguous even when
                // a later assistant reuses the same run id and ends cleanly.
                terminal_seen = assistant.stop_reason.is_some()
                    && effective_stop != DurableTailStopReason::ToolUse;
            }
            Message::ToolResults { results, .. } => {
                run_rows = true;
                for result in results {
                    if let Some(position) = open_call_ids
                        .iter()
                        .position(|call_id| *call_id == result.tool_use_id)
                    {
                        open_call_ids.remove(position);
                    } else {
                        orphan_tool_result_count += 1;
                    }
                }
            }
            _ => {}
        }
    }
    let terminal_stop_reason = last_assistant_stop.unwrap_or(DurableTailStopReason::Absent);
    let run_id_cardinality = match (run_ids.len(), assistant_without_run_id) {
        // No assistant message and no tool result: no row could carry a run
        // id. The classifier decides whether the store-issued provisional
        // binding, or an agreeing run cursor, attributes the tail.
        (0, _) if !run_rows => RunIdCardinality::NoRunRows,
        (0, _) => RunIdCardinality::NoRunId,
        (1, false) => RunIdCardinality::SingleRunId,
        _ => RunIdCardinality::MultipleRunIds,
    };
    DurableTailObservation {
        tail_run_id,
        run_id_cardinality,
        terminal_stop_reason,
        dangling_tool_use_ids: open_call_ids,
        orphan_tool_result_count,
        messages_after_terminal,
        run_cursor: DurableTailRunCursor::Absent,
        cursor_inputs_open: false,
    }
}

/// Mechanical run-cursor evidence of a durable tail: whether the tail
/// carries a run cursor, whether that cursor agrees with the transcript, and
/// whether every input it binds is still bound to its run. The generated
/// classifier assigns the meaning.
///
/// The provisional tail and its cursor are written by one checkpoint of one
/// run, so a readable cursor that names another run than the store-issued
/// provisional run contradicts the store and is invalid evidence.
///
/// A cursor agrees when it names the provisional run and, while its round's
/// tool calls are pending, the staged assistant message is absent from the
/// tail (the second commit of a round puts the message and its results into
/// the transcript together). The staged message is matched by its own
/// assistant message id, never by its call ids: providers repeat call ids
/// across responses (Gemini numbers each response's calls from `fc_0`), so an
/// earlier round of the same run can carry the staged calls' ids. A staged
/// message without an id cannot be told apart and disagrees. A cursor
/// awaiting the model agrees by its run alone: resuming it asks the model
/// again and dispatches nothing. An unreadable cursor disagrees.
///
/// Its inputs are bound when the cursor binds at least one input and every
/// one of them is Staged, Applied or AppliedPendingConsumption in the store
/// with that run as its last run. An empty bound set never resumes (direct and
/// machine-service turns), and a requeued or terminal input ends the run's
/// continuation.
fn observe_run_cursor(
    head: &Session,
    tail_base: usize,
    candidate_run_id: &RunId,
    bound_input_runs: &HashMap<InputId, RunId>,
) -> Result<(DurableTailRunCursor, bool), DurableTailRecoveryError> {
    let cursor = match head.try_run_cursor() {
        Ok(None) => return Ok((DurableTailRunCursor::Absent, false)),
        Ok(Some(cursor)) => cursor,
        Err(_) => return Ok((DurableTailRunCursor::Disagrees, false)),
    };
    if &cursor.run_id != candidate_run_id {
        return Err(DurableTailRecoveryError::InvalidEvidence(format!(
            "run cursor names run {}, not the store-issued provisional run {candidate_run_id}",
            cursor.run_id
        )));
    }
    if let RunResumePoint::ToolsPending { assistant_message } = &cursor.resume_point {
        let Some(staged_message_id) = assistant_message.assistant_message_id.as_ref() else {
            return Ok((DurableTailRunCursor::Disagrees, false));
        };
        if !assistant_message.has_tool_calls() {
            return Ok((DurableTailRunCursor::Disagrees, false));
        }
        let tail = &head.messages()[tail_base.min(head.messages().len())..];
        let staged_in_tail = tail.iter().any(|message| {
            matches!(
                message,
                Message::BlockAssistant(assistant)
                    if assistant.assistant_message_id.as_ref() == Some(staged_message_id)
            )
        });
        if staged_in_tail {
            return Ok((DurableTailRunCursor::Disagrees, false));
        }
    }
    let inputs_bound = !cursor.bound_input_ids.is_empty()
        && cursor
            .bound_input_ids
            .iter()
            .all(|input_id| bound_input_runs.get(input_id) == Some(&cursor.run_id));
    Ok((DurableTailRunCursor::Agrees, inputs_bound))
}

/// The inputs a tail's run cursor binds to its run, when the classifier's
/// cursor verdict `run_cursor` is that the cursor agrees with the tail:
/// only an agreeing cursor binds exactly the inputs whose content the tail
/// holds. `None` for a tail without a cursor, and for one whose cursor
/// disagrees (unreadable, or a round's staged message that is already in
/// the tail or cannot be told apart): that tail keeps attribution by run
/// association.
fn cursor_bound_inputs(head: &Session, run_cursor: DurableTailRunCursor) -> Option<Vec<InputId>> {
    if run_cursor != DurableTailRunCursor::Agrees {
        return None;
    }
    match head.try_run_cursor() {
        Ok(Some(cursor)) => Some(cursor.bound_input_ids),
        Ok(None) | Err(_) => None,
    }
}

fn durable_tail_run_cursor_name(cursor: DurableTailRunCursor) -> &'static str {
    match cursor {
        DurableTailRunCursor::Absent => "absent",
        DurableTailRunCursor::Agrees => "agrees",
        DurableTailRunCursor::Disagrees => "disagrees",
    }
}

/// Bind the run-cursor evidence into a candidate id. A tail without a cursor
/// adds nothing, so the candidate ids of tails written without run cursors
/// do not change.
fn hash_run_cursor_observation(hasher: &mut Sha256, observation: &DurableTailObservation) {
    if observation.run_cursor == DurableTailRunCursor::Absent {
        return;
    }
    hash_part(
        hasher,
        "run_cursor",
        durable_tail_run_cursor_name(observation.run_cursor).as_bytes(),
    );
    hash_part(
        hasher,
        "cursor_inputs_open",
        if observation.cursor_inputs_open {
            b"true"
        } else {
            b"false"
        },
    );
}

fn hash_part(hasher: &mut Sha256, label: &str, value: &[u8]) {
    hasher.update((label.len() as u64).to_be_bytes());
    hasher.update(label.as_bytes());
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn run_id_cardinality_name(cardinality: RunIdCardinality) -> &'static str {
    match cardinality {
        RunIdCardinality::NoRunId => "no_run_id",
        RunIdCardinality::SingleRunId => "single_run_id",
        RunIdCardinality::MultipleRunIds => "multiple_run_ids",
        RunIdCardinality::NoRunRows => "no_run_rows",
    }
}

fn durable_tail_stop_reason_name(reason: DurableTailStopReason) -> &'static str {
    match reason {
        DurableTailStopReason::Absent => "absent",
        DurableTailStopReason::EndTurn => "end_turn",
        DurableTailStopReason::ToolUse => "tool_use",
        DurableTailStopReason::Other => "other",
    }
}

fn exact_candidate_id(
    session_id: &SessionId,
    committed_store_revision: u64,
    committed_head_token: &str,
    physical_store_revision: u64,
    physical_head_cas_token: &str,
    provisional_run_id: &RunId,
    observation: &DurableTailObservation,
) -> String {
    let observed_run = observation
        .tail_run_id
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| NO_OBSERVED_RUN_IDENTITY.to_string());
    let mut hasher = Sha256::new();
    hash_part(
        &mut hasher,
        "domain",
        b"meerkat:durable-tail-recovery-candidate:v5",
    );
    hash_part(&mut hasher, "session", session_id.to_string().as_bytes());
    hash_part(
        &mut hasher,
        "committed_store_revision",
        &committed_store_revision.to_be_bytes(),
    );
    hash_part(
        &mut hasher,
        "committed_head_token",
        committed_head_token.as_bytes(),
    );
    hash_part(
        &mut hasher,
        "physical_store_revision",
        &physical_store_revision.to_be_bytes(),
    );
    hash_part(
        &mut hasher,
        "physical_head_token",
        physical_head_cas_token.as_bytes(),
    );
    hash_part(
        &mut hasher,
        "provisional_run",
        provisional_run_id.to_string().as_bytes(),
    );
    hash_part(&mut hasher, "observed_run", observed_run.as_bytes());
    hash_part(
        &mut hasher,
        "run_cardinality",
        run_id_cardinality_name(observation.run_id_cardinality).as_bytes(),
    );
    hash_part(
        &mut hasher,
        "stop_reason",
        durable_tail_stop_reason_name(observation.terminal_stop_reason).as_bytes(),
    );
    hash_part(
        &mut hasher,
        "dangling_call_count",
        observation
            .dangling_tool_use_ids
            .len()
            .to_string()
            .as_bytes(),
    );
    for call_id in &observation.dangling_tool_use_ids {
        hash_part(&mut hasher, "dangling_call", call_id.as_bytes());
    }
    hash_part(
        &mut hasher,
        "orphan_results",
        observation.orphan_tool_result_count.to_string().as_bytes(),
    );
    hash_part(
        &mut hasher,
        "after_terminal",
        if observation.messages_after_terminal {
            b"true"
        } else {
            b"false"
        },
    );
    hash_run_cursor_observation(&mut hasher, observation);
    format!("sha256:{:x}", hasher.finalize())
}

fn exact_whole_blob_candidate_id(
    session_id: &SessionId,
    base_store_revision: u64,
    base_blob_sha256: &str,
    candidate_blob_sha256: &str,
    candidate_sequence: u64,
    provisional_run_id: &RunId,
    observation: &DurableTailObservation,
) -> String {
    let mut hasher = Sha256::new();
    hash_part(
        &mut hasher,
        "domain",
        b"meerkat:whole-blob-durable-tail-recovery-candidate:v2",
    );
    hash_part(&mut hasher, "session", session_id.to_string().as_bytes());
    hash_part(
        &mut hasher,
        "base_store_revision",
        &base_store_revision.to_be_bytes(),
    );
    hash_part(&mut hasher, "base_blob_sha256", base_blob_sha256.as_bytes());
    hash_part(
        &mut hasher,
        "candidate_blob_sha256",
        candidate_blob_sha256.as_bytes(),
    );
    hash_part(
        &mut hasher,
        "candidate_sequence",
        &candidate_sequence.to_be_bytes(),
    );
    hash_part(
        &mut hasher,
        "provisional_run",
        provisional_run_id.to_string().as_bytes(),
    );
    hash_part(
        &mut hasher,
        "run_cardinality",
        run_id_cardinality_name(observation.run_id_cardinality).as_bytes(),
    );
    hash_part(
        &mut hasher,
        "stop_reason",
        durable_tail_stop_reason_name(observation.terminal_stop_reason).as_bytes(),
    );
    for call_id in &observation.dangling_tool_use_ids {
        hash_part(&mut hasher, "dangling_call", call_id.as_bytes());
    }
    hash_part(
        &mut hasher,
        "orphan_results",
        &observation.orphan_tool_result_count.to_be_bytes(),
    );
    hash_part(
        &mut hasher,
        "after_terminal",
        if observation.messages_after_terminal {
            b"true"
        } else {
            b"false"
        },
    );
    hash_run_cursor_observation(&mut hasher, observation);
    format!("sha256:{:x}", hasher.finalize())
}

fn generated_recovery_classification(
    session_id: &SessionId,
    candidate_id: &str,
    relation: DurableHeadRelation,
    observation: &DurableTailObservation,
) -> Result<ClassifiedRecoveryClass, DurableTailRecoveryError> {
    // The generated machine is invoked and consumed inside this function.
    // There is intentionally no parameter through which a caller can inject a
    // `SessionDocumentEffect` or any of the shape projections below.
    let mut classifier = SessionDocumentMachineAuthority::new();
    let effects = classifier
        .classify_durable_tail(
            SessionDocumentKey::new(session_id.to_string()),
            candidate_id.to_string(),
            relation,
            observation.run_id_cardinality,
            observation.terminal_stop_reason,
            observation.dangling_tool_use_ids.len() as u64,
            observation.orphan_tool_result_count,
            observation.messages_after_terminal,
            observation.run_cursor,
            observation.cursor_inputs_open,
        )
        .map_err(|error| {
            DurableTailRecoveryError::Authority(format!(
                "durable-tail classification rejected: {error}"
            ))
        })?;
    let mut matching = effects.iter().filter_map(|effect| match effect {
        SessionDocumentEffect::DurableTailClassified {
            candidate_id: emitted_candidate,
            class,
        } if emitted_candidate == candidate_id => Some(*class),
        _ => None,
    });
    let Some(class) = matching.next() else {
        return Err(DurableTailRecoveryError::Authority(
            "classifier emitted no verdict for the exact evidence-bound candidate".to_string(),
        ));
    };
    if matching.next().is_some() {
        return Err(DurableTailRecoveryError::Authority(
            "classifier emitted more than one verdict for the same candidate".to_string(),
        ));
    }
    Ok(class)
}

fn runtime_recovery_class(
    class: ClassifiedRecoveryClass,
) -> Result<DurableTailRecoveryClass, DurableTailRecoveryError> {
    Ok(match class {
        ClassifiedRecoveryClass::CompletedCandidate => DurableTailRecoveryClass::CompletedCandidate,
        ClassifiedRecoveryClass::InterruptedRepairableCandidate => {
            DurableTailRecoveryClass::InterruptedRepairableCandidate
        }
        ClassifiedRecoveryClass::Ambiguous => DurableTailRecoveryClass::Ambiguous,
        ClassifiedRecoveryClass::ContinuableCandidate => {
            DurableTailRecoveryClass::ContinuableCandidate
        }
        // Fail closed against a stale generated artifact while the 0.8.10
        // floor removal propagates through generated code. This is not a
        // mapping or adoption path; the variant disappears at regeneration.
        #[allow(unreachable_patterns)]
        _ => {
            return Err(DurableTailRecoveryError::Authority(
                "session-document classifier emitted unsupported recovery vocabulary".to_string(),
            ));
        }
    })
}

fn message_timestamp(message: &Message) -> meerkat_core::types::MessageTimestamp {
    match message {
        Message::System(message) => message.created_at,
        Message::SystemNotice(message) => message.created_at,
        Message::User(message) => message.created_at,
        Message::BlockAssistant(message) => message.created_at,
        Message::ToolResults { created_at, .. } => *created_at,
    }
}

/// The reason an outcome-unknown result gives for a call of an interrupted
/// run that recovery closes.
const INTERRUPTED_CALL_REASON: &str = "the run was interrupted after this call was issued and \
     did not resume; the call was not run again, and whether it took effect is unknown";

/// The staged message of a ToolsPending run cursor that agrees with the
/// tail: the round's first commit put it in the cursor, and its second commit
/// never put it in the transcript. `None` for any other cursor.
fn agreeing_staged_message(
    head: &Session,
    run_cursor: DurableTailRunCursor,
) -> Result<Option<BlockAssistantMessage>, DurableTailRecoveryError> {
    if run_cursor != DurableTailRunCursor::Agrees {
        return Ok(None);
    }
    let cursor = head
        .try_run_cursor()
        .map_err(|error| {
            DurableTailRecoveryError::InvalidEvidence(format!(
                "an agreeing run cursor is unreadable: {error}"
            ))
        })?
        .ok_or_else(|| {
            DurableTailRecoveryError::InvalidEvidence(
                "an agreeing run cursor is missing from the head".to_string(),
            )
        })?;
    Ok(match cursor.resume_point {
        RunResumePoint::ToolsPending { assistant_message } => Some(assistant_message),
        RunResumePoint::AwaitingModel => None,
    })
}

/// The operator's reason on the calls `resolve-held-tail` answers.
fn operator_closed_call_reason(candidate_id: &str) -> String {
    format!(
        "the run was interrupted after this call was issued, and an operator resolved its held \
         tail with resolve-held-tail (candidate {candidate_id}); the call was not run again, and \
         whether it took effect is unknown"
    )
}

/// The outcome-unknown result for one call.
fn outcome_unknown_result(call_id: &str, tool_name: &str, reason: &str) -> ToolResult {
    ToolResult::new(
        call_id.to_string(),
        ToolError::outcome_uncertain(tool_name, reason).to_transcript_content(),
        true,
    )
}

/// The results an operator's repair gives the dangling transcript calls.
///
/// Only the calls of the tail's last assistant message can be answered in
/// place: the repair appends, and a result must follow its call with nothing
/// but other results between. A dangling call anywhere else refuses.
fn trailing_dangling_results(
    recovered: &Session,
    dangling_tool_use_ids: &[String],
    reason: &str,
) -> Result<Vec<ToolResult>, DurableTailRecoveryError> {
    if dangling_tool_use_ids.is_empty() {
        return Ok(Vec::new());
    }
    let messages = recovered.messages();
    let last_assistant = messages
        .iter()
        .rposition(|message| matches!(message, Message::BlockAssistant(_)));
    let not_trailing = || {
        DurableTailRecoveryError::InvalidEvidence(format!(
            "resolve-held-tail answers only the calls of the tail's last assistant message, \
             followed by nothing but tool results; dangling call(s) {} are not",
            dangling_tool_use_ids.join(", ")
        ))
    };
    let Some(position) = last_assistant else {
        return Err(not_trailing());
    };
    let Message::BlockAssistant(assistant) = &messages[position] else {
        return Err(not_trailing());
    };
    if !messages[position + 1..]
        .iter()
        .all(|message| matches!(message, Message::ToolResults { .. }))
    {
        return Err(not_trailing());
    }
    // Pair the last message's own calls with the results after it, by
    // position: providers repeat call ids across responses (Gemini numbers
    // each response's calls from `fc_0`), so the tail-wide dangling ids alone
    // cannot say which message a call belongs to. The calls left open must be
    // exactly the dangling set; otherwise a dangling call belongs to an
    // earlier message.
    let mut own_open: Vec<(&str, &str)> = assistant
        .tool_calls()
        .map(|call| (call.id, call.name))
        .collect();
    for message in &messages[position + 1..] {
        if let Message::ToolResults { results, .. } = message {
            for result in results {
                if let Some(index) = own_open
                    .iter()
                    .position(|(id, _)| *id == result.tool_use_id.as_str())
                {
                    own_open.remove(index);
                }
            }
        }
    }
    let mut expected: Vec<&str> = dangling_tool_use_ids.iter().map(String::as_str).collect();
    let mut own_ids: Vec<&str> = own_open.iter().map(|(id, _)| *id).collect();
    expected.sort_unstable();
    own_ids.sort_unstable();
    if expected != own_ids {
        return Err(not_trailing());
    }
    // Answer the open calls in call order.
    Ok(own_open
        .into_iter()
        .map(|(id, name)| outcome_unknown_result(id, name, reason))
        .collect())
}

/// An operator's repair must not drop calls a run cursor may stage: refuse
/// a cursor it cannot read, and a staged message with calls but no id, which
/// the classifier cannot place in or out of the tail. Evidence that cannot be
/// verified stays held.
fn operator_staged_calls_placeable(head: &Session) -> Result<(), DurableTailRecoveryError> {
    let cursor = head.try_run_cursor().map_err(|error| {
        DurableTailRecoveryError::InvalidEvidence(format!(
            "resolve-held-tail cannot read the tail's run cursor, so it cannot place the calls \
             the cursor may stage: {error}"
        ))
    })?;
    match cursor.map(|cursor| cursor.resume_point) {
        Some(RunResumePoint::ToolsPending { assistant_message })
            if assistant_message.has_tool_calls()
                && assistant_message.assistant_message_id.is_none() =>
        {
            Err(DurableTailRecoveryError::InvalidEvidence(
                "resolve-held-tail cannot place the calls of the run cursor's staged message: \
                 the message has no id"
                    .to_string(),
            ))
        }
        _ => Ok(()),
    }
}

/// Close an interrupted run's tail in place, keeping every durable row.
///
/// The calls of a staged message may have run, so dropping the cursor alone
/// would hide them and the next turn could repeat their side effects. The
/// staged message is written into the transcript, followed by one error
/// result per call: `ToolError::OutcomeUncertain`, the result a resumed round
/// gives a call it does not run again. Then the interrupted notice is
/// appended. Every added row is stamped from durable rows only, so every
/// process builds the same document; the caller's adoption of the head state
/// then drops the run cursor.
///
/// An operator's repair also answers the transcript's dangling calls the same
/// way, before any staged message, and its results and notice name the verb
/// and the candidate. Returns the calls answered as outcome unknown.
fn close_interrupted_run(
    recovered: &mut Session,
    staged: Option<&BlockAssistantMessage>,
    dangling_tool_use_ids: &[String],
    durable_tail_timestamp: MessageTimestamp,
    closer: InterruptedRunCloser<'_>,
) -> Result<Vec<String>, DurableTailRecoveryError> {
    let operator_reason = match closer {
        InterruptedRunCloser::Recovery => None,
        InterruptedRunCloser::Operator { candidate_id } => {
            Some(operator_closed_call_reason(candidate_id))
        }
    };
    let reason = operator_reason
        .as_deref()
        .unwrap_or(INTERRUPTED_CALL_REASON);
    let mut closed_call_ids = Vec::new();
    let dangling_results = match closer {
        // The classifier admits repair only with zero dangling transcript
        // calls. Recheck in release code: a call the transcript shows without
        // its result was not written by a round's two commits, so its history
        // is not this run's to settle; only an operator answers it.
        InterruptedRunCloser::Recovery => {
            if !dangling_tool_use_ids.is_empty() {
                return Err(DurableTailRecoveryError::InvalidEvidence(format!(
                    "interrupted-tail repair was classified despite {} dangling tool call(s): {}",
                    dangling_tool_use_ids.len(),
                    dangling_tool_use_ids.join(", ")
                )));
            }
            Vec::new()
        }
        InterruptedRunCloser::Operator { .. } => {
            trailing_dangling_results(recovered, dangling_tool_use_ids, reason)?
        }
    };
    let mut closed_at = durable_tail_timestamp;
    if !dangling_results.is_empty() {
        closed_call_ids.extend(
            dangling_results
                .iter()
                .map(|result| result.tool_use_id.clone()),
        );
        recovered.push(Message::ToolResults {
            results: dangling_results,
            created_at: closed_at,
        });
    }
    if let Some(staged) = staged {
        let results: Vec<ToolResult> = staged
            .tool_calls()
            .map(|call| outcome_unknown_result(call.id, call.name, reason))
            .collect();
        if results.is_empty() {
            return Err(DurableTailRecoveryError::InvalidEvidence(
                "a ToolsPending run cursor stages a message without tool calls".to_string(),
            ));
        }
        closed_at = closed_at.max(staged.created_at);
        closed_call_ids.extend(results.iter().map(|result| result.tool_use_id.clone()));
        recovered.push(Message::BlockAssistant(staged.clone()));
        recovered.push(Message::ToolResults {
            results,
            created_at: closed_at,
        });
    }
    let notice = match closer {
        InterruptedRunCloser::Recovery => "A previous run was interrupted before its boundary \
             committed. Recovery preserved every durable message, closed the run as \
             InterruptedByRecovery, and did not requeue its input. Continue from a new turn."
            .to_string(),
        InterruptedRunCloser::Operator { candidate_id } => format!(
            "A previous run was held for recovery because its durable evidence was ambiguous. \
             An operator resolved it with resolve-held-tail (candidate {candidate_id}): every \
             durable message was kept, calls without a result were answered as outcome \
             unknown, and the run was closed as interrupted. Its input was not requeued. \
             Continue from a new turn."
        ),
    };
    recovered.push(Message::SystemNotice(SystemNoticeMessage {
        runtime_origin: None,
        kind: SystemNoticeKind::Generic,
        body: Some(notice),
        blocks: Vec::new(),
        // Recovery of the same exact durable head must be byte-identical in
        // every process. Wall-clock `now` would make the recovered boundary
        // and idempotency witness race-dependent.
        created_at: closed_at,
    }));
    Ok(closed_call_ids)
}

/// The relation the durable-tail classifier receives: the store's proof,
/// mapped one to one. The relation is never assumed: an unproven rewrite is
/// classified as unverifiable, never as a strict continuation.
fn classifier_relation(source: DurableTailSourceRelation) -> DurableHeadRelation {
    match source {
        DurableTailSourceRelation::StrictContinuation => {
            DurableHeadRelation::VerifiedStrictDescendant
        }
        DurableTailSourceRelation::RewriteDescendant { .. } => {
            DurableHeadRelation::VerifiedRewriteDescendant
        }
        DurableTailSourceRelation::UnprovenRewrite => DurableHeadRelation::Unverifiable,
    }
}

/// How a recovery source's transcript history differs from the rewrite
/// witness of the store-issued physical head it was materialized from, if it
/// does. The head is the authority: values are never derived from the
/// session itself.
fn physical_history_witness_mismatch(
    history: Option<&meerkat_core::ValidatedTranscriptHistory>,
    head: &meerkat_core::session_store::SessionHead,
) -> Option<&'static str> {
    let Some(history) = history else {
        return (head.rewrite_count != 0)
            .then_some("the head has rewrite occurrences but no graph");
    };
    if head.rewrite_count == 0 {
        return Some("the head has no rewrite occurrences but the source carries a graph");
    }
    if u64::try_from(history.commit_count()).ok() != Some(head.rewrite_count) {
        return Some("the graph's occurrence count differs from the head's rewrite count");
    }
    if history.rewrite_prefix() != &head.rewrite_prefix {
        return Some("the graph's rewrite prefix differs from the head's");
    }
    if head.graph_prefix.as_ref() != Some(history.graph_prefix()) {
        return Some("the graph's prefix differs from the head's graph prefix");
    }
    None
}

fn prepare_head_canonical_recovery_candidate_in(
    source: &PreparedDurableTailRecoverySource,
    bound_input_runs: &HashMap<InputId, RunId>,
    mode: PreparationMode,
) -> Result<RecoveryCandidatePreparation, DurableTailRecoveryError> {
    let runtime_authority = source.runtime_authority();
    let committed_authority = source.committed_session().as_ref();
    let observed_physical_head = source.physical_head();
    let physical_head = source.physical_session().as_ref();
    let session_id = committed_authority.id().clone();
    if runtime_authority.profile() != RuntimeSessionPersistenceProfile::HeadCanonicalV1 {
        return Err(DurableTailRecoveryError::InvalidEvidence(format!(
            "runtime persistence profile {} cannot atomically recover an external physical head",
            runtime_authority.profile()
        )));
    }
    let committed_store_authority = runtime_authority.head_canonical().ok_or_else(|| {
        DurableTailRecoveryError::InvalidEvidence(
            "HeadCanonical recovery received a different store authority profile".to_string(),
        )
    })?;
    if runtime_authority.session_id() != &session_id {
        return Err(DurableTailRecoveryError::InvalidEvidence(format!(
            "runtime authority belongs to session {}, not committed document {session_id}",
            runtime_authority.session_id()
        )));
    }
    if observed_physical_head.id != session_id {
        return Err(DurableTailRecoveryError::InvalidEvidence(format!(
            "observed physical head belongs to session {}, not committed authority {session_id}",
            observed_physical_head.id
        )));
    }
    if physical_head.id() != &session_id {
        return Err(DurableTailRecoveryError::InvalidEvidence(format!(
            "physical head belongs to session {}, not committed authority {session_id}",
            physical_head.id()
        )));
    }
    if committed_authority.version() != physical_head.version()
        || committed_authority.created_at() != physical_head.created_at()
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "physical head changes immutable session envelope identity".to_string(),
        ));
    }
    let boundary_head = committed_store_authority.boundary_head();
    let authority_head_cas_token = committed_store_authority.committed_head_token();

    if let Some(provisional) = source.provisional_authority()
        && !source.provisional_target_applied()
    {
        return Ok(
            RecoveryCandidatePreparation::IncompleteHeadCanonicalIntent {
                provisional: provisional.clone(),
            },
        );
    }

    // Benign convergence: another process may have committed recovery after
    // the caller observed A/H but before this store-owned snapshot. Equality
    // is not a hold and not a recovery candidate. Return the exact
    // committed document the source paired with the authoritative head.
    if observed_physical_head == boundary_head {
        if source.physical_head_cas_token() != authority_head_cas_token {
            return Err(DurableTailRecoveryError::InvalidEvidence(
                "equal runtime and physical heads carry contradictory store authority".to_string(),
            ));
        }
        return Ok(RecoveryCandidatePreparation::AlreadyAligned {
            recovered: Arc::clone(source.physical_session()),
            committed_whole_blob: None,
        });
    }
    let provisional = source.provisional_authority().ok_or_else(|| {
        DurableTailRecoveryError::InvalidEvidence(
            "newer physical head has no store-issued provisional authority".to_string(),
        )
    })?;
    if provisional.session_id() != &session_id
        || provisional.base_store_revision() != committed_store_authority.store_revision()
        || provisional.base_committed_head_token() != authority_head_cas_token
        || provisional.physical_head_token() != source.physical_head_cas_token()
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "provisional tail does not name the exact committed parent and physical head"
                .to_string(),
        ));
    }
    // The relation is the store's proof, never assumed here: a strict
    // continuation keeps the boundary's rows as a prefix; a run that
    // compacted rewrote them, and its rows after the final rewrite are the
    // tail the classifier observes.
    let relation = classifier_relation(source.relation());
    let tail_base = match source.relation() {
        DurableTailSourceRelation::StrictContinuation => {
            // Equal transcripts are a metadata-only tail (a round's first
            // commit with no new row, or a run's closing save); the tail
            // observation below decides whether anything attributes it.
            if physical_head.messages().len() < committed_authority.messages().len()
                || !physical_head
                    .messages()
                    .starts_with(committed_authority.messages())
            {
                return Err(DurableTailRecoveryError::InvalidEvidence(
                    "physical recovery head is not an exact strict transcript continuation"
                        .to_string(),
                ));
            }
            committed_authority.messages().len()
        }
        DurableTailSourceRelation::RewriteDescendant { live_tail_base } => {
            if live_tail_base > physical_head.messages().len() {
                return Err(DurableTailRecoveryError::InvalidEvidence(
                    "rewrite descent live-tail base exceeds the physical transcript".to_string(),
                ));
            }
            live_tail_base
        }
        DurableTailSourceRelation::UnprovenRewrite => physical_head.messages().len(),
    };

    let derived_physical_head_token = source.physical_head_cas_token().to_string();

    let mut observation = observe_durable_tail(tail_base, physical_head);
    let (run_cursor, cursor_inputs_open) = observe_run_cursor(
        physical_head,
        tail_base,
        provisional.run_id(),
        bound_input_runs,
    )?;
    observation.run_cursor = run_cursor;
    observation.cursor_inputs_open = cursor_inputs_open;
    // Rewrite occurrences carry no run identity, so a rewritten tail with no
    // run-stamped assistant row after its final rewrite has no transcript
    // evidence of its run: the machine classifies it, and holds it. A tail
    // with no run rows at all is the machine's to attribute too: the
    // store-issued provisional binding of this exact physical head does on a
    // strict continuation, and only an agreeing run cursor does on a rewrite.
    let unattributed_rewrite = relation != DurableHeadRelation::VerifiedStrictDescendant
        && observation.run_id_cardinality == RunIdCardinality::NoRunId;
    let no_run_rows = observation.run_id_cardinality == RunIdCardinality::NoRunRows;
    if !unattributed_rewrite
        && !no_run_rows
        && (observation.tail_run_id.as_ref() != Some(provisional.run_id())
            || observation.run_id_cardinality != RunIdCardinality::SingleRunId)
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "physical tail transcript contradicts its store-issued provisional run identity"
                .to_string(),
        ));
    }
    let candidate_id = exact_candidate_id(
        &session_id,
        committed_store_authority.store_revision(),
        authority_head_cas_token,
        provisional.physical_store_revision(),
        source.physical_head_cas_token(),
        provisional.run_id(),
        &observation,
    );
    let classified =
        generated_recovery_classification(&session_id, &candidate_id, relation, &observation)?;
    let class = runtime_recovery_class(classified)?;
    // An operator attests a held tail as an interrupted run, never its
    // ancestry: a tail whose descent from the committed head the store
    // cannot prove stays held (quarantined evidence has no operator path).
    if class == DurableTailRecoveryClass::Ambiguous
        && (mode == PreparationMode::Recovery || relation == DurableHeadRelation::Unverifiable)
    {
        return Ok(RecoveryCandidatePreparation::Held);
    }
    let candidate_run_id = provisional.run_id().clone();

    // The recovered document starts from the canonical physical
    // materialization, which is already the exact durable successor content.
    // Its rewrite authority is the store-issued physical SessionHead's
    // witness (rewrite count, rewrite prefix, graph prefix). A head with
    // rewrite occurrences materializes with exactly the graph the store
    // replayed from its rewrite rows and bound to that witness; the
    // recovered document keeps that graph as recovery-owned state and never
    // extends it. With the witness exact, the recovered mutation proves an
    // empty pending rewrite suffix, so recovery commits no rewrite; appending
    // the recovery notice leaves the graph untouched, and the recovered
    // envelope is re-adopted from the physical head, so every process mints
    // the same recovered boundary from the same durable rows. A graph the
    // head does not describe is invalid evidence.
    let physical_history = physical_head
        .validated_transcript_history_state()
        .map_err(|error| {
            DurableTailRecoveryError::InvalidEvidence(format!(
                "physical-head transcript history is malformed: {error}"
            ))
        })?;
    if let Some(mismatch) =
        physical_history_witness_mismatch(physical_history.as_ref(), observed_physical_head)
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(format!(
            "head-canonical recovery source carries a transcript history its physical head does not describe: {mismatch}"
        )));
    }
    let mut recovered = physical_head.clone();
    let closer = match class {
        // The exact physical transcript is already the completed boundary.
        DurableTailRecoveryClass::CompletedCandidate => None,
        DurableTailRecoveryClass::InterruptedRepairableCandidate
        | DurableTailRecoveryClass::ContinuableCandidate => Some(InterruptedRunCloser::Recovery),
        // Only an operator's resolve-held-tail prepares an ambiguous tail.
        DurableTailRecoveryClass::Ambiguous => Some(InterruptedRunCloser::Operator {
            candidate_id: &candidate_id,
        }),
    };
    let closed_call_ids = match closer {
        None => Vec::new(),
        Some(closer) => {
            let durable_tail_timestamp = physical_head
                .messages()
                .last()
                .map(message_timestamp)
                .ok_or_else(|| {
                    DurableTailRecoveryError::InvalidEvidence(
                        "interrupted recovery has no durable tail timestamp".to_string(),
                    )
                })?;
            if matches!(closer, InterruptedRunCloser::Operator { .. }) {
                operator_staged_calls_placeable(physical_head)?;
            }
            let staged = agreeing_staged_message(physical_head, observation.run_cursor)?;
            close_interrupted_run(
                &mut recovered,
                staged.as_ref(),
                &observation.dangling_tool_use_ids,
                durable_tail_timestamp,
                closer,
            )?
        }
    };
    // Synthetic repair necessarily advances the outer updated_at with
    // wall-clock time. Re-adopting the exact physical envelope restores the
    // durable timestamp and all non-recovery-owned state. Completed recovery
    // is already byte-for-byte the physical materialization; applying this
    // uniformly keeps the field-ownership rule in one place.
    // Adopting the head also drops its run cursor: a committed body never
    // carries one (D3), so every recovery commit closes the run it
    // attributes, whatever the class.
    recovered
        .adopt_recovered_head_state(physical_head)
        .map_err(DurableTailRecoveryError::InvalidEvidence)?;
    if recovered.messages().len() < physical_head.messages().len() {
        return Err(DurableTailRecoveryError::InvalidEvidence(format!(
            "internally recovered document lost durable content: {} < {} messages",
            recovered.messages().len(),
            physical_head.messages().len()
        )));
    }
    let recovered = Arc::new(recovered);
    let recovered_mutation = PreparedHeadCanonicalMutation::prepare(
        recovered.as_ref(),
        Some(observed_physical_head.clone()),
    )
    .map_err(|error| {
        DurableTailRecoveryError::InvalidEvidence(format!(
            "recovered HeadCanonical mutation preparation failed: {error}"
        ))
    })?;
    if recovered_mutation.predecessor_head_token() != Some(derived_physical_head_token.as_str()) {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "recovered mutation changed the provisional physical-head token".to_string(),
        ));
    }
    let recovered_head_token = meerkat_core::session_head_cas_token(
        recovered_mutation.successor_head(),
    )
    .map_err(|error| {
        DurableTailRecoveryError::InvalidEvidence(format!(
            "recovered successor head token is invalid: {error}"
        ))
    })?;
    let document = BoundSessionCommit::sealed(Arc::clone(&recovered))
        .map_err(|error| {
            DurableTailRecoveryError::InvalidEvidence(format!(
                "failed to seal recovered HeadCanonical document: {error}"
            ))
        })?
        .with_head_canonical_mutation(recovered_mutation)
        .map_err(|error| {
            DurableTailRecoveryError::InvalidEvidence(format!(
                "failed to bind recovered HeadCanonical mutation: {error}"
            ))
        })?;
    let conversation_digest = recovered.transcript_content_digest().map_err(|error| {
        DurableTailRecoveryError::InvalidEvidence(format!(
            "recovered transcript digest failed: {error}"
        ))
    })?;
    let message_count = recovered.messages().len();
    let continuation = if class == DurableTailRecoveryClass::ContinuableCandidate {
        Some(prepared_continuation(
            source.physical_session(),
            RunCheckpointAuthority::HeadCanonical(provisional.clone()),
        )?)
    } else {
        None
    };

    Ok(RecoveryCandidatePreparation::Prepared(
        PreparedRecoveryCandidate {
            session_id,
            candidate_id,
            candidate_run_id,
            class,
            store_transition: PreparedRecoveryStoreTransition::HeadCanonical {
                committed_store_revision: committed_store_authority.store_revision(),
                committed_head_token: authority_head_cas_token.to_string(),
                physical_store_revision: provisional.physical_store_revision(),
                physical_head_cas_token: derived_physical_head_token,
                recovered_head_token,
            },
            recovered,
            document: Some(document),
            conversation_digest,
            message_count,
            continuation,
            tail_facts: HeldTailFacts::observed(&observation),
            physical_message_count: physical_head.messages().len(),
            closed_call_ids,
            cursor_bound_inputs: cursor_bound_inputs(physical_head, observation.run_cursor),
        },
    ))
}

/// The continuation of a ContinuableCandidate: the provisional tail itself
/// and the receipt its next checkpoint chains from, bound to the same
/// store-issued provisional authority the tail was written under.
fn prepared_continuation(
    candidate: &Arc<Session>,
    authority: RunCheckpointAuthority,
) -> Result<PreparedContinuation, DurableTailRecoveryError> {
    let conversation_digest = candidate.transcript_content_digest().map_err(|error| {
        DurableTailRecoveryError::InvalidEvidence(format!(
            "continuable tail transcript digest failed: {error}"
        ))
    })?;
    let message_count = u64::try_from(candidate.messages().len()).map_err(|_| {
        DurableTailRecoveryError::InvalidEvidence(
            "continuable tail message count exceeds the receipt range".to_string(),
        )
    })?;
    let receipt = RunCheckpointReceipt::issued(authority, conversation_digest, message_count)
        .map_err(|error| {
            DurableTailRecoveryError::InvalidEvidence(format!(
                "continuable tail checkpoint receipt is invalid: {error}"
            ))
        })?;
    Ok(PreparedContinuation {
        candidate: Arc::clone(candidate),
        receipt,
    })
}

/// Ordinary recovery's WholeBlob preparation, as the rows call it.
#[cfg(test)]
fn prepare_whole_blob_recovery_candidate(
    committed: CommittedWholeBlobSnapshot,
    provisional: Option<CommittedWholeBlobProvisionalTail>,
    bound_input_runs: &HashMap<InputId, RunId>,
) -> Result<RecoveryCandidatePreparation, DurableTailRecoveryError> {
    prepare_whole_blob_recovery_candidate_in(
        committed,
        provisional,
        bound_input_runs,
        PreparationMode::Recovery,
    )
}

fn prepare_whole_blob_recovery_candidate_in(
    committed: CommittedWholeBlobSnapshot,
    provisional: Option<CommittedWholeBlobProvisionalTail>,
    bound_input_runs: &HashMap<InputId, RunId>,
    mode: PreparationMode,
) -> Result<RecoveryCandidatePreparation, DurableTailRecoveryError> {
    let committed_snapshot = committed.clone();
    let (committed_session, _committed_bytes, committed_authority) = committed.into_parts();
    if committed_session.id() != committed_authority.session_id() {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "committed WholeBlob payload identity differs from store authority".to_string(),
        ));
    }
    let Some(provisional) = provisional else {
        return Ok(RecoveryCandidatePreparation::AlreadyAligned {
            recovered: committed_session,
            committed_whole_blob: Some(committed_snapshot),
        });
    };
    let provisional_authority = provisional.authority();
    if provisional_authority.session_id() != committed_authority.session_id()
        || provisional_authority.base_store_revision() != committed_authority.store_revision()
        || provisional_authority.base_blob_sha256() != committed_authority.blob_sha256()
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "WholeBlob provisional tail does not name the exact committed base".to_string(),
        ));
    }
    // Decode the candidate exactly as the committed body is decoded
    // (`CommittedWholeBlobSnapshot::new`): `decode_whole_blob_document` binds
    // the decoded transcript to the exact rows of these bytes, the row-lineage
    // origin every later transcript rewrite extends. A run continued from this
    // tail is seeded from this session, so without that lineage none of its
    // compactions could commit. The bytes must be the ones the store issued
    // the provisional authority for; anything else is invalid evidence, as an
    // undecodable payload is.
    let decoded_candidate = Session::decode_whole_blob_document(provisional.candidate_bytes())
        .map_err(|error| {
            DurableTailRecoveryError::InvalidEvidence(format!(
                "provisional WholeBlob payload is invalid: {error}"
            ))
        })?;
    if decoded_candidate.row_sha256_token() != provisional_authority.candidate_blob_sha256() {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "provisional WholeBlob payload digest differs from its store-issued candidate authority"
                .to_string(),
        ));
    }
    let candidate_session = decoded_candidate.into_session();
    if candidate_session.id() != committed_session.id()
        || candidate_session.version() != committed_session.version()
        || candidate_session.created_at() != committed_session.created_at()
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "WholeBlob provisional tail changes immutable session identity".to_string(),
        ));
    }
    // Equal transcripts are a metadata-only tail: a round's first commit
    // with no new row, a run's closing save, or the tail a run re-base
    // restarts on its compacted snapshot.
    if candidate_session.messages().len() < committed_session.messages().len()
        || !candidate_session
            .messages()
            .starts_with(committed_session.messages())
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "WholeBlob provisional tail is not an exact strict transcript continuation".to_string(),
        ));
    }
    let mut observation =
        observe_durable_tail(committed_session.messages().len(), &candidate_session);
    let (run_cursor, cursor_inputs_open) = observe_run_cursor(
        &candidate_session,
        committed_session.messages().len(),
        provisional_authority.run_id(),
        bound_input_runs,
    )?;
    observation.run_cursor = run_cursor;
    observation.cursor_inputs_open = cursor_inputs_open;
    // A tail with no run rows is attributed by the store-issued provisional
    // binding of this exact candidate; the classifier decides.
    if observation.run_id_cardinality != RunIdCardinality::NoRunRows
        && (observation.tail_run_id.as_ref() != Some(provisional_authority.run_id())
            || observation.run_id_cardinality != RunIdCardinality::SingleRunId)
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "WholeBlob provisional transcript contradicts its store-issued run identity"
                .to_string(),
        ));
    }
    let candidate_id = exact_whole_blob_candidate_id(
        committed_authority.session_id(),
        committed_authority.store_revision(),
        committed_authority.blob_sha256(),
        provisional_authority.candidate_blob_sha256(),
        provisional_authority.candidate_sequence(),
        provisional_authority.run_id(),
        &observation,
    );
    // The exact prefix check above is this source's descent proof.
    let class = runtime_recovery_class(generated_recovery_classification(
        committed_authority.session_id(),
        &candidate_id,
        DurableHeadRelation::VerifiedStrictDescendant,
        &observation,
    )?)?;
    if class == DurableTailRecoveryClass::Ambiguous && mode == PreparationMode::Recovery {
        return Ok(RecoveryCandidatePreparation::Held);
    }
    let physical_envelope = candidate_session.clone();
    let physical_message_count = candidate_session.messages().len();
    let candidate_cursor_bound_inputs =
        cursor_bound_inputs(&candidate_session, observation.run_cursor);
    let mut recovered = candidate_session;
    let closer = match class {
        DurableTailRecoveryClass::CompletedCandidate => None,
        DurableTailRecoveryClass::InterruptedRepairableCandidate
        | DurableTailRecoveryClass::ContinuableCandidate => Some(InterruptedRunCloser::Recovery),
        // Only an operator's resolve-held-tail prepares an ambiguous tail.
        DurableTailRecoveryClass::Ambiguous => Some(InterruptedRunCloser::Operator {
            candidate_id: &candidate_id,
        }),
    };
    let closed_call_ids = match closer {
        None => Vec::new(),
        Some(closer) => {
            let durable_tail_timestamp = recovered
                .messages()
                .last()
                .map(message_timestamp)
                .ok_or_else(|| {
                    DurableTailRecoveryError::InvalidEvidence(
                        "interrupted WholeBlob recovery has no durable tail timestamp".to_string(),
                    )
                })?;
            if matches!(closer, InterruptedRunCloser::Operator { .. }) {
                operator_staged_calls_placeable(&recovered)?;
            }
            let staged = agreeing_staged_message(&recovered, observation.run_cursor)?;
            close_interrupted_run(
                &mut recovered,
                staged.as_ref(),
                &observation.dangling_tool_use_ids,
                durable_tail_timestamp,
                closer,
            )?
        }
    };
    // Adopting the envelope also drops its run cursor (D3).
    recovered
        .adopt_recovered_head_state(&physical_envelope)
        .map_err(DurableTailRecoveryError::InvalidEvidence)?;
    let continuation = if class == DurableTailRecoveryClass::ContinuableCandidate {
        Some(prepared_continuation(
            &Arc::new(physical_envelope),
            RunCheckpointAuthority::WholeBlob(provisional_authority.clone()),
        )?)
    } else {
        None
    };
    let recovered = Arc::new(recovered);
    let (document, recovered_blob_sha256) = match class {
        DurableTailRecoveryClass::CompletedCandidate => (
            None,
            provisional_authority.candidate_blob_sha256().to_string(),
        ),
        DurableTailRecoveryClass::InterruptedRepairableCandidate
        | DurableTailRecoveryClass::ContinuableCandidate
        | DurableTailRecoveryClass::Ambiguous => {
            let document = BoundSessionCommit::sealed(Arc::clone(&recovered)).map_err(|error| {
                DurableTailRecoveryError::InvalidEvidence(format!(
                    "failed to seal repaired WholeBlob document: {error}"
                ))
            })?;
            let recovered_blob_sha256 = document
                .whole_blob_artifact()
                .map_err(|error| {
                    DurableTailRecoveryError::InvalidEvidence(format!(
                        "failed to serialize repaired WholeBlob document: {error}"
                    ))
                })?
                .row_sha256_token()
                .to_string();
            (Some(document), recovered_blob_sha256)
        }
    };
    let conversation_digest = recovered.transcript_content_digest().map_err(|error| {
        DurableTailRecoveryError::InvalidEvidence(format!(
            "recovered WholeBlob transcript digest failed: {error}"
        ))
    })?;
    let message_count = recovered.messages().len();
    Ok(RecoveryCandidatePreparation::Prepared(
        PreparedRecoveryCandidate {
            session_id: committed_authority.session_id().clone(),
            candidate_id,
            candidate_run_id: provisional_authority.run_id().clone(),
            class,
            store_transition: PreparedRecoveryStoreTransition::WholeBlob {
                base_store_revision: committed_authority.store_revision(),
                base_blob_sha256: committed_authority.blob_sha256().to_string(),
                provisional_candidate_blob_sha256: provisional_authority
                    .candidate_blob_sha256()
                    .to_string(),
                provisional_candidate_sequence: provisional_authority.candidate_sequence(),
                recovered_blob_sha256,
            },
            recovered,
            document,
            conversation_digest,
            message_count,
            continuation,
            tail_facts: HeldTailFacts::observed(&observation),
            physical_message_count,
            closed_call_ids,
            cursor_bound_inputs: candidate_cursor_bound_inputs,
        },
    ))
}

fn bind_recovered_store_document(
    candidate: &PreparedRecoveryCandidate,
) -> Result<Option<BoundSessionCommit>, DurableTailRecoveryError> {
    let document = candidate.document.clone();
    match &candidate.store_transition {
        PreparedRecoveryStoreTransition::WholeBlob {
            provisional_candidate_blob_sha256,
            recovered_blob_sha256,
            ..
        } => {
            if recovered_blob_sha256 == provisional_candidate_blob_sha256 {
                if candidate.class != DurableTailRecoveryClass::CompletedCandidate
                    || document.is_some()
                {
                    return Err(DurableTailRecoveryError::InvalidEvidence(
                        "metadata-only WholeBlob promotion carries a repaired document or \
                         non-completed class"
                            .to_string(),
                    ));
                }
                return Ok(None);
            }
            let document = document.ok_or_else(|| {
                DurableTailRecoveryError::InvalidEvidence(
                    "repaired WholeBlob recovery lost its sealed successor artifact".to_string(),
                )
            })?;
            let artifact = document.whole_blob_artifact().map_err(|error| {
                DurableTailRecoveryError::InvalidEvidence(format!(
                    "failed to materialize recovered WholeBlob artifact: {error}"
                ))
            })?;
            if artifact.row_sha256_token() != recovered_blob_sha256 {
                return Err(DurableTailRecoveryError::InvalidEvidence(
                    "sealed WholeBlob recovery document differs from its successor digest"
                        .to_string(),
                ));
            }
            Ok(Some(document))
        }
        PreparedRecoveryStoreTransition::HeadCanonical {
            physical_head_cas_token,
            recovered_head_token,
            ..
        } => {
            let document = document.ok_or_else(|| {
                DurableTailRecoveryError::InvalidEvidence(
                    "HeadCanonical recovery lost its prepared mutation".to_string(),
                )
            })?;
            let mutation = document
                .head_canonical()
                .ok_or_else(|| {
                    DurableTailRecoveryError::InvalidEvidence(
                        "sealed HeadCanonical recovery lost its prepared mutation".to_string(),
                    )
                })?
                .mutation();
            if mutation.predecessor_head_token() != Some(physical_head_cas_token.as_str()) {
                return Err(DurableTailRecoveryError::InvalidEvidence(
                    "prepared recovery mutation changed the observed physical-head CAS token"
                        .to_string(),
                ));
            }
            let successor_head_token = meerkat_core::session_head_cas_token(
                mutation.successor_head(),
            )
            .map_err(|error| {
                DurableTailRecoveryError::InvalidEvidence(format!(
                    "prepared recovery successor head token is invalid: {error}"
                ))
            })?;
            if successor_head_token != *recovered_head_token {
                return Err(DurableTailRecoveryError::InvalidEvidence(
                    "prepared recovery mutation changed the recovered store head".to_string(),
                ));
            }
            Ok(Some(document))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn seal_recovery_evidence(
    candidate: &PreparedRecoveryCandidate,
    document: Option<&BoundSessionCommit>,
    disposition: DurableTailRecoveryDisposition,
    receipt_digest_enrichments: Vec<PreparedRecoveryReceiptDigestEnrichment>,
    predecessor_nonterminal_input_set_revision: RecoveryInputSetRevision,
    predecessor_nonterminal_input_set_token: String,
    input_updates: Vec<InputStatePersistenceRecord>,
    receipt: &RunBoundaryReceipt,
    lifecycle: &MachineLifecycleCommit,
) -> Result<PreparedRecoveryEvidence, RuntimeStoreError> {
    match &candidate.store_transition {
        PreparedRecoveryStoreTransition::WholeBlob {
            base_store_revision,
            base_blob_sha256,
            provisional_candidate_blob_sha256,
            provisional_candidate_sequence,
            recovered_blob_sha256,
        } => PreparedRecoveryEvidence::seal_whole_blob(
            candidate.recovered.as_ref(),
            document,
            candidate.session_id.clone(),
            candidate.candidate_id.clone(),
            candidate.candidate_run_id.clone(),
            candidate.class,
            disposition,
            *base_store_revision,
            base_blob_sha256.clone(),
            provisional_candidate_blob_sha256.clone(),
            *provisional_candidate_sequence,
            recovered_blob_sha256.clone(),
            receipt_digest_enrichments,
            predecessor_nonterminal_input_set_revision,
            predecessor_nonterminal_input_set_token,
            input_updates,
            receipt,
            lifecycle,
        ),
        PreparedRecoveryStoreTransition::HeadCanonical {
            committed_store_revision,
            committed_head_token,
            physical_store_revision,
            physical_head_cas_token,
            recovered_head_token,
            ..
        } => {
            let document =
                document.ok_or_else(|| RuntimeStoreError::SessionPersistenceAuthorityConflict {
                    runtime_id: candidate.session_id.to_string(),
                    detail: "HeadCanonical recovery evidence lost its prepared mutation"
                        .to_string(),
                })?;
            PreparedRecoveryEvidence::seal_head_canonical(
                candidate.recovered.as_ref(),
                document,
                candidate.session_id.clone(),
                candidate.candidate_id.clone(),
                candidate.candidate_run_id.clone(),
                candidate.class,
                disposition,
                *committed_store_revision,
                committed_head_token.clone(),
                *physical_store_revision,
                physical_head_cas_token.clone(),
                recovered_head_token.clone(),
                receipt_digest_enrichments,
                predecessor_nonterminal_input_set_revision,
                predecessor_nonterminal_input_set_token,
                input_updates,
                receipt,
                lifecycle,
            )
        }
    }
}

fn verify_exact_committed_recovery(
    candidate: &PreparedRecoveryCandidate,
    committed: &CommittedRecoveryBoundary,
    lifecycle: &MachineLifecycleCommit,
) -> Result<(DurableTailRecoveryDisposition, u64), DurableTailRecoveryError> {
    let receipt = committed.receipt();
    if receipt.boundary != RunApplyBoundary::Immediate {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "committed recovery witness carries a non-immediate boundary".to_string(),
        ));
    }
    let document = bind_recovered_store_document(candidate)?;
    let expected_evidence = seal_recovery_evidence(
        candidate,
        document.as_ref(),
        committed.evidence().disposition(),
        committed.evidence().receipt_digest_enrichments().to_vec(),
        committed
            .evidence()
            .predecessor_nonterminal_input_set_revision(),
        committed
            .evidence()
            .predecessor_nonterminal_input_set_token()
            .to_owned(),
        committed.evidence().cloned_input_updates(),
        receipt,
        lifecycle,
    )?;
    if &expected_evidence != committed.evidence() {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "committed recovery candidate id exists with a different exact source, recovered \
             store authority, receipt, lifecycle target, migration witness, or evidence witness"
                .to_string(),
        ));
    }
    Ok((committed.evidence().disposition(), receipt.sequence))
}

async fn load_exact_committed_recovery(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    candidate: &PreparedRecoveryCandidate,
    lifecycle: &MachineLifecycleCommit,
) -> Result<Option<(DurableTailRecoveryDisposition, u64)>, DurableTailRecoveryError> {
    let committed = store
        .load_committed_recovery_boundary(runtime_id, &candidate.candidate_id)
        .await?;
    committed
        .as_ref()
        .map(|committed| verify_exact_committed_recovery(candidate, committed, lifecycle))
        .transpose()
}

fn committed_recovery_outcome(
    candidate: PreparedRecoveryCandidate,
    disposition: DurableTailRecoveryDisposition,
    boundary_sequence: u64,
) -> DurableTailRecoveryOutcome {
    let PreparedRecoveryCandidate {
        recovered,
        document,
        ..
    } = candidate;
    // Drop the sealed carrier before unwrapping its shared typed Session.
    // Otherwise every successful recovery deep-clones the accumulated
    // document solely because its own completed commit carrier still holds an
    // Arc at outcome construction.
    drop(document);
    let recovered = Arc::try_unwrap(recovered).unwrap_or_else(|shared| shared.as_ref().clone());
    DurableTailRecoveryOutcome::Committed {
        disposition,
        boundary_sequence,
        recovered: Box::new(recovered),
    }
}

/// Outcome of one authorization + commit attempt. Refusal and hold both
/// retain the durable tail intact; nothing here deletes anything.
#[derive(Debug)]
pub enum DurableTailRecoveryOutcome {
    /// The machine authorized the commit and the atomic boundary succeeded,
    /// with the machine-minted boundary sequence. The returned document is the
    /// exact internally built successor bound to the store authority the
    /// commit returned; callers do not rebuild it.
    Committed {
        disposition: DurableTailRecoveryDisposition,
        boundary_sequence: u64,
        recovered: Box<Session>,
    },
    /// The store-owned snapshot proved runtime authority and the physical
    /// canonical head were already byte-exactly aligned. This is benign
    /// convergence (typically another process won recovery before source
    /// loading), and the returned store-bound committed document is safe to
    /// serve directly.
    AlreadyAligned { recovered: Box<Session> },
    /// The candidate is held intact: ambiguous tail evidence, or input
    /// records whose identity cannot be proven durably. Autonomy stays
    /// blocked; the tail clears only through reconciliation.
    Held,
    /// The machine refused (non-quiescent persisted or in-process runtime,
    /// conflicting run facts, or durable receipts that already cover — or
    /// contradict — this candidate).
    Refused,
    /// The tail is a durable session's open run at a committed round, and
    /// the machine authorized continuing it. Nothing was written:
    /// `candidate` is the provisional tail itself, its run cursor included,
    /// `run_id` the run to resume, and `receipt` the provisional receipt the
    /// resumed run's next checkpoint chains from.
    Continue {
        candidate: Box<Session>,
        run_id: RunId,
        receipt: RunCheckpointReceipt,
    },
}

/// What registration needs to keep a resumable run's inputs bound: the run a
/// continuable tail's cursor names and the inputs the cursor binds to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumableRunObservation {
    /// The run the stored cursor names.
    pub run_id: RunId,
    /// The inputs the cursor binds to that run.
    pub bound_input_ids: Vec<InputId>,
}

/// Observe whether the session's durable tail is a continuable open run,
/// with the same source preparation and classifier the recovery drive uses.
///
/// This only observes. The drive's machine verdict at materialization
/// decides; a run marked resumable from this observation is released to
/// ordinary requeue when that verdict is not Continue.
pub async fn observe_resumable_run(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
) -> Result<Option<ResumableRunObservation>, DurableTailRecoveryError> {
    let runtime_id = LogicalRuntimeId::for_session(session_id);
    // Most registrations find no provisional tail: answer them without
    // loading the committed document and the input rows.
    // A store that cannot load provisional tails holds none to continue.
    let has_provisional_tail = match store.session_persistence_profile() {
        RuntimeSessionPersistenceProfile::HeadCanonicalV1 => store
            .load_head_canonical_provisional_tail(&runtime_id)
            .await
            .map(|tail| tail.is_some()),
        RuntimeSessionPersistenceProfile::WholeBlobV1 => store
            .load_whole_blob_provisional_tail(&runtime_id)
            .await
            .map(|tail| tail.is_some()),
    };
    match has_provisional_tail {
        Ok(true) => {}
        Ok(false) | Err(RuntimeStoreError::Unsupported(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let RecoveryCandidatePreparation::Prepared(candidate) =
        load_recovery_preparation(store, &runtime_id, session_id, PreparationMode::Recovery)
            .await?
    else {
        return Ok(None);
    };
    if candidate.class != DurableTailRecoveryClass::ContinuableCandidate {
        return Ok(None);
    }
    let Some(continuation) = candidate.continuation.as_ref() else {
        return Ok(None);
    };
    let cursor = continuation.candidate.try_run_cursor().map_err(|error| {
        DurableTailRecoveryError::InvalidEvidence(format!(
            "continuable tail run cursor is unreadable: {error}"
        ))
    })?;
    Ok(cursor.map(|cursor| ResumableRunObservation {
        run_id: candidate.candidate_run_id.clone(),
        bound_input_ids: cursor.bound_input_ids,
    }))
}

/// A durable session's open run as its provisional tail holds it: the tail
/// itself, run cursor included, the run, and the receipt the run's next
/// checkpoint chains from (its digest the tail's transcript content digest),
/// as a registration's actor would be seeded with them.
#[derive(Debug, Clone)]
pub struct OpenRunContinuation {
    /// The provisional tail, run cursor included.
    pub candidate: Box<Session>,
    /// The run the tail continues.
    pub run_id: RunId,
    /// The receipt the run's next checkpoint chains from.
    pub receipt: RunCheckpointReceipt,
}

/// Observe the continuation of the session's open run, with the same source
/// preparation and classifier as [`observe_resumable_run`]: `Some` only when
/// the durable tail is a continuable open run. This only observes; nothing is
/// written.
///
/// A host that resumes, in the same process, a run it suspended (a store
/// fence gap dropped the run at its dispatch wait) seeds the run's live actor
/// from it again, as a restart's registration seeds a new actor: the dropped
/// attempt's uncommitted content is gone, and the run continues from its last
/// round commit.
pub async fn observe_open_run_continuation(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
) -> Result<Option<OpenRunContinuation>, DurableTailRecoveryError> {
    let runtime_id = LogicalRuntimeId::for_session(session_id);
    let RecoveryCandidatePreparation::Prepared(candidate) =
        load_recovery_preparation(store, &runtime_id, session_id, PreparationMode::Recovery)
            .await?
    else {
        return Ok(None);
    };
    if candidate.class != DurableTailRecoveryClass::ContinuableCandidate {
        return Ok(None);
    }
    let Some(continuation) = candidate.continuation else {
        return Ok(None);
    };
    Ok(Some(OpenRunContinuation {
        candidate: Box::new(continuation.candidate.as_ref().clone()),
        run_id: candidate.candidate_run_id,
        receipt: continuation.receipt,
    }))
}

/// The session's last committed snapshot while a HeadCanonical provisional
/// tail sits ahead of it.
///
/// A durable run's round commits apply its provisional tail to the physical
/// head, so from the run's first tool round on the physical head is past the
/// runtime boundary, and a committed read that materializes the boundary as
/// the current head refuses with `TranscriptRevisionConflict`. The store
/// retains the boundary for recovery: this reads it through the store-owned
/// durable-tail source, which verifies the boundary head from its own anchor,
/// rows and runtime-boundary metadata in the one transaction that also proves
/// the provisional tail binds that boundary and the physical head. The
/// snapshot is the boundary's exact content and metadata, whether the run
/// appended to it or rewrote it, and carries nothing of the run.
///
/// `Ok(None)` when the profile is not HeadCanonical, the store keeps no
/// durable-tail source, or no applied provisional tail sits on the boundary:
/// the caller's refusal then stands. It only reads.
///
/// The snapshot is evidence of the committed boundary only. It proves nothing
/// about the tail: a source whose rewrites the store could not prove descend
/// from the boundary still answers `Some`, so a caller never uses the answer
/// as tail evidence (recovery owns that judgment). A round's intent written
/// but not yet applied also answers `None`; a reader in another task than
/// the run's re-observes then. Under a tail the store verifies both the
/// boundary and the physical head, so a call costs about two committed
/// reads.
///
/// The session service reads its fork source through it when the fork comes
/// from the source's own running turn
/// ([`meerkat_core::DurableForkSourceAdmission::CallerTurn`]).
#[doc(hidden)]
pub async fn load_committed_snapshot_under_provisional_tail(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
) -> Result<Option<Session>, DurableTailRecoveryError> {
    if store.session_persistence_profile() != RuntimeSessionPersistenceProfile::HeadCanonicalV1 {
        return Ok(None);
    }
    let runtime_id = LogicalRuntimeId::for_session(session_id);
    let source = match store.load_durable_tail_recovery_source(&runtime_id).await {
        Ok(Some(source)) => source,
        Ok(None)
        | Err(
            RuntimeStoreError::Unsupported(_)
            | RuntimeStoreError::PreparedRecoveryRequiresAtomicPhysicalHeadCas { .. },
        ) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if source.runtime_authority().session_id() != session_id {
        return Err(DurableTailRecoveryError::InvalidEvidence(format!(
            "store-owned durable-tail source belongs to session {}, not requested {session_id}",
            source.runtime_authority().session_id()
        )));
    }
    if source.provisional_authority().is_none() || !source.provisional_target_applied() {
        return Ok(None);
    }
    Ok(Some(source.committed_session().as_ref().clone()))
}

/// Typed error: authorization/commit mechanics failed (as opposed to the
/// machine refusing, which is an [`DurableTailRecoveryOutcome`]).
#[derive(Debug, thiserror::Error)]
pub enum DurableTailRecoveryError {
    #[error("recovery authorization could not be driven: {0}")]
    Authority(String),
    /// Exact store authorities, physical rows, tail shape, or the internally
    /// built recovered successor failed validation.
    #[error("recovery evidence is invalid: {0}")]
    InvalidEvidence(String),
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
    /// session document); a Stopped row an operator's repair normalizes
    /// becomes the clean Idle row a start's registration writes.
    reassert_state: RuntimeState,
    binding: MachineLifecycleBindingFacts,
    supervisor_authority: SupervisorAuthoritySnapshot,
    unregister_progress: Option<crate::store::MachineUnregisterProgressSnapshot>,
    live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage,
    /// The placement witness the row carries (record version 7), which the
    /// commit re-asserts with the rest of the observed record.
    placement_witness: Option<MachineLifecycleBindingFacts>,
    /// An operator reads this `Stopped` row as the clean `Idle` row a
    /// start's registration replaces it with (`observe_persisted_lifecycle`).
    normalizes_stopped: bool,
}

/// Project the persisted lifecycle row into what the recovery machine judges.
///
/// The archive of a session that is ending (`DriveMode::SessionArchive`)
/// reads every quiescent persisted runtime as `Retired`: a missing row,
/// `Idle`, `Stopped` (its executor exited) and `Retired` itself. The session
/// ends with that archive and no later start resumes its run, so the machine
/// never continues a run there: it repairs a continuable tail and commits it
/// as interrupted. The re-asserted phase stays the observed one, and the
/// persisted current run keeps its own guard. Every other drive reads each
/// phase as persisted; a `Stopped` runtime is not quiescent there, because a
/// later start may resume its run.
///
/// An operator's drive (`resolve-held-tail`) runs under an exclusive open of
/// the runtime database, which proves no live process owns the session, and
/// never registers the runtime. A `Stopped` row with no current run and no
/// unregister in progress (a host stopped the runtime and ended before it
/// unregistered) reads there as the clean `Idle` row a start's registration
/// replaces it with, on the same generated reconcile verdict
/// (`NormalizeOrReplace`): unbound, no run facts, its supervisor,
/// unregister-progress and live-bridge facts kept. Only an applied repair
/// writes that row, in its own commit fenced on the observed row version; a
/// `Stopped` row with a current run or an unregister in progress is not
/// quiescent there either.
fn observe_persisted_lifecycle(
    observation: MachineLifecycleObservation,
    candidate_run_id: &meerkat_core::RunId,
    mode: DriveMode,
) -> ObservedPersistedLifecycle {
    let ending_session = mode == DriveMode::SessionArchive;
    let normalizes_stopped = matches!(mode, DriveMode::Operator { .. })
        && matches!(
            &observation,
            MachineLifecycleObservation::Decoded { record, .. }
                if record.runtime_state() == Some(RuntimeState::Stopped)
                    && record.run().current_run_id().is_none()
                    && record.unregister_progress().is_none()
        )
        && matches!(
            crate::meerkat_machine::driver::runtime_authority_reconcile_decision_for_observation(
                &observation
            ),
            Ok(mm_dsl::RuntimeAuthorityReconcileDecision::NormalizeOrReplace)
        );
    let mut observed = match observation {
        MachineLifecycleObservation::Missing => ObservedPersistedLifecycle {
            lifecycle: mm_dsl::DurableRecoveryObservedLifecycle::MissingRow,
            current_run: mm_dsl::DurableRecoveryObservedRun::NoRun,
            expected_version: MachineLifecycleExpectedVersion::Missing,
            reassert_state: RuntimeState::Idle,
            binding: MachineLifecycleBindingFacts::default(),
            supervisor_authority: SupervisorAuthoritySnapshot::UnboundNoReceipt,
            unregister_progress: None,
            live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage::default(),
            placement_witness: None,
            normalizes_stopped: false,
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
                Some(RuntimeState::Stopped) if ending_session => (
                    mm_dsl::DurableRecoveryObservedLifecycle::Retired,
                    RuntimeState::Stopped,
                ),
                Some(RuntimeState::Stopped) if normalizes_stopped => (
                    mm_dsl::DurableRecoveryObservedLifecycle::Idle,
                    RuntimeState::Idle,
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
                // The normalized row is unbound, as registration's replacement.
                binding: if normalizes_stopped {
                    MachineLifecycleBindingFacts::default()
                } else {
                    record.binding().clone()
                },
                supervisor_authority: record.supervisor_authority().clone(),
                unregister_progress: record.unregister_progress().cloned(),
                live_bridge_recovery: record.live_bridge_recovery().clone(),
                placement_witness: record.placement_witness().cloned(),
                normalizes_stopped,
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
            live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage::default(),
            placement_witness: None,
            normalizes_stopped: false,
        },
    };
    if ending_session
        && matches!(
            observed.lifecycle,
            mm_dsl::DurableRecoveryObservedLifecycle::MissingRow
                | mm_dsl::DurableRecoveryObservedLifecycle::Idle
        )
    {
        observed.lifecycle = mm_dsl::DurableRecoveryObservedLifecycle::Retired;
    }
    observed
}

fn durable_tail_lifecycle_reassertion(
    observed: &ObservedPersistedLifecycle,
) -> MachineLifecycleCommit {
    MachineLifecycleCommit::new_with_binding_run_unregister_progress_and_live_bridge(
        observed.reassert_state,
        observed.binding.clone(),
        MachineLifecycleRunFacts::default(),
        observed.supervisor_authority.clone(),
        observed.unregister_progress.clone(),
        observed.live_bridge_recovery.clone(),
    )
    .with_placement_witness(observed.placement_witness.clone())
    .with_expected_version(observed.expected_version.clone())
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
struct PriorCommitObservation {
    last_committed_sequence: u64,
    classification: mm_dsl::DurableRecoveryPriorCommit,
    receipt_bound_inputs: BTreeSet<String>,
}

/// Materialize the exact receipts already proved by the store-owned source,
/// applying only the sealed one-time digest enrichments for 0.8.10 rows.
///
/// Pairing is by both sequence and exact original row token. A migration for
/// another row can never donate transcript ancestry to this candidate.
fn materialize_verified_receipts(
    sources: &[PreparedRecoveryReceiptSource],
    enrichments: &[PreparedRecoveryReceiptDigestEnrichment],
) -> Result<Vec<RunBoundaryReceipt>, DurableTailRecoveryError> {
    let mut by_sequence = BTreeMap::new();
    for enrichment in enrichments {
        if by_sequence
            .insert(enrichment.original_receipt().sequence, enrichment)
            .is_some()
        {
            return Err(DurableTailRecoveryError::InvalidEvidence(
                "store-owned receipt migration contains duplicate sequences".to_string(),
            ));
        }
    }

    let mut materialized = Vec::with_capacity(sources.len());
    let mut consumed_enrichments = 0_usize;
    for source in sources {
        let receipt = source.receipt();
        if receipt.conversation_digest.is_some() {
            if by_sequence.contains_key(&receipt.sequence) {
                return Err(DurableTailRecoveryError::InvalidEvidence(format!(
                    "receipt sequence {} is already digest-bound but also carries a migration",
                    receipt.sequence
                )));
            }
            materialized.push(receipt.clone());
            continue;
        }
        let enrichment = by_sequence.get(&receipt.sequence).ok_or_else(|| {
            DurableTailRecoveryError::InvalidEvidence(format!(
                "digestless receipt sequence {} has no sealed store migration",
                receipt.sequence
            ))
        })?;
        if enrichment.original_receipt() != receipt
            || enrichment.original_exact_row_token() != source.exact_row_token()
        {
            return Err(DurableTailRecoveryError::InvalidEvidence(format!(
                "receipt migration sequence {} does not bind the exact original row",
                receipt.sequence
            )));
        }
        materialized.push(enrichment.enriched_receipt());
        consumed_enrichments += 1;
    }
    if consumed_enrichments != enrichments.len() {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "store-owned receipt migration contains a row absent from the exact receipt read"
                .to_string(),
        ));
    }
    Ok(materialized)
}

/// Check the candidate run's committed receipts (run identity, dense
/// sequence, counts within the candidate) and pair each digestless one with
/// the digest of the candidate prefix it counts.
///
/// A crashed run's committed receipts are its mid-run boundary receipts,
/// which carry message count 0 and no digest
/// (`mid_run_receipts_carry_no_transcript_count_or_digest`): for them this
/// flat-prefix check is vacuous and the enrichment is the empty transcript's
/// digest. A writer that mints a counted receipt before its run's terminal
/// boundary must first move this check, and [`observe_prior_commits`], to
/// commit-chain ancestry: an in-run compaction rewrites the prefix it counts.
fn prepare_receipt_digest_enrichments(
    candidate: &PreparedRecoveryCandidate,
    receipts: &[PreparedRecoveryReceiptSource],
) -> Result<Vec<PreparedRecoveryReceiptDigestEnrichment>, DurableTailRecoveryError> {
    let mut expected_sequence = 1_u64;
    let mut previous_message_count = 0_usize;
    let mut enrichments = Vec::new();
    for source in receipts {
        let receipt = source.receipt();
        if receipt.run_id != candidate.candidate_run_id {
            return Err(DurableTailRecoveryError::InvalidEvidence(
                "recovery receipt source contains a different run identity".to_string(),
            ));
        }
        if receipt.sequence != expected_sequence {
            return Err(DurableTailRecoveryError::InvalidEvidence(format!(
                "recovery receipt sequence {} is not the required dense sequence {expected_sequence}",
                receipt.sequence
            )));
        }
        if receipt.message_count < previous_message_count
            || receipt.message_count > candidate.recovered.messages().len()
        {
            return Err(DurableTailRecoveryError::InvalidEvidence(
                "recovery receipt message counts are not monotonic within the store-bound transcript"
                    .to_string(),
            ));
        }
        let derived_digest = candidate
            .recovered
            .transcript_prefix_digest(receipt.message_count)
            .map_err(|error| {
                DurableTailRecoveryError::InvalidEvidence(format!(
                    "failed to derive recovery receipt transcript prefix: {error}"
                ))
            })?;
        match receipt.conversation_digest.as_deref() {
            Some(existing) if existing != derived_digest => {
                return Err(DurableTailRecoveryError::InvalidEvidence(format!(
                    "receipt sequence {} digest differs from the store-bound transcript prefix",
                    receipt.sequence
                )));
            }
            Some(_) => {}
            None => enrichments.push(
                PreparedRecoveryReceiptDigestEnrichment::new(source, derived_digest)
                    .map_err(DurableTailRecoveryError::Store)?,
            ),
        }
        expected_sequence = expected_sequence.checked_add(1).ok_or_else(|| {
            DurableTailRecoveryError::InvalidEvidence(
                "recovery receipt sequence overflow".to_string(),
            )
        })?;
        previous_message_count = receipt.message_count;
    }
    Ok(enrichments)
}

/// Verify every same-run receipt against the exact candidate transcript prefix
/// it claims. Message count alone is not ancestry, and a digest of some other
/// shorter conversation is not "precedes". Supported-floor digestless rows
/// reach this function only after [`materialize_verified_receipts`] has paired
/// them with a sealed store-owned enrichment.
///
/// A crashed run's committed receipts are its mid-run boundary receipts
/// (message count 0, no digest), so this prefix check is vacuous for them; see
/// [`prepare_receipt_digest_enrichments`] before minting a counted one.
fn observe_prior_commits(
    committed: &[RunBoundaryReceipt],
    candidate: &Session,
    candidate_run_id: &RunId,
) -> PriorCommitObservation {
    let last_committed_sequence = committed
        .iter()
        .map(|receipt| receipt.sequence)
        .max()
        .unwrap_or(0);
    if committed.is_empty() {
        return PriorCommitObservation {
            last_committed_sequence,
            classification: mm_dsl::DurableRecoveryPriorCommit::NoPriorCommit,
            receipt_bound_inputs: BTreeSet::new(),
        };
    }

    let mut ordered = committed.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|receipt| receipt.sequence);
    let mut previous: Option<&RunBoundaryReceipt> = None;
    let mut receipt_bound_inputs = BTreeSet::new();
    for receipt in &ordered {
        let expected_sequence = previous
            .and_then(|prior| prior.sequence.checked_add(1))
            .unwrap_or(1);
        let structurally_monotonic = receipt.sequence == expected_sequence
            && previous.is_none_or(|prior| receipt.message_count >= prior.message_count);
        let prefix_matches = receipt.run_id == *candidate_run_id
            && receipt.message_count <= candidate.messages().len()
            && receipt
                .conversation_digest
                .as_deref()
                .is_some_and(|digest| {
                    candidate
                        .transcript_prefix_digest(receipt.message_count)
                        .is_ok_and(|prefix| prefix == digest)
                });
        if !structurally_monotonic || !prefix_matches {
            return PriorCommitObservation {
                last_committed_sequence,
                classification: mm_dsl::DurableRecoveryPriorCommit::DivergesFromCandidate,
                // A divergent receipt cannot contribute input identity to
                // this candidate. The refusal path must never terminalize its
                // named rows even if later code is rearranged.
                receipt_bound_inputs: BTreeSet::new(),
            };
        }
        receipt_bound_inputs.extend(
            receipt
                .contributing_input_ids
                .iter()
                .map(ToString::to_string),
        );
        previous = Some(receipt);
    }

    PriorCommitObservation {
        last_committed_sequence,
        classification: ordered.last().map_or(
            mm_dsl::DurableRecoveryPriorCommit::NoPriorCommit,
            |highest| {
                if highest.message_count == candidate.messages().len() {
                    mm_dsl::DurableRecoveryPriorCommit::MatchesCandidate
                } else {
                    mm_dsl::DurableRecoveryPriorCommit::PrecedesCandidate
                }
            },
        ),
        receipt_bound_inputs,
    }
}

/// The run each input is still bound to, for every non-terminal input the
/// store holds in Staged, Applied or AppliedPendingConsumption with a last
/// run. A store that cannot list its input rows proves nothing bound: a cursor
/// tail then classifies as an ordinary interrupted tail, never as continuable.
async fn load_bound_input_runs(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<HashMap<InputId, RunId>, DurableTailRecoveryError> {
    let snapshot = match store.load_input_states_with_versions(runtime_id).await {
        Ok(snapshot) => snapshot,
        Err(RuntimeStoreError::Unsupported(_)) => return Ok(HashMap::new()),
        Err(error) => return Err(error.into()),
    };
    if snapshot.runtime_id() != runtime_id {
        return Err(DurableTailRecoveryError::Authority(format!(
            "store returned an input snapshot for {}, not {runtime_id}",
            snapshot.runtime_id()
        )));
    }
    let (rows, _revision, _token) = snapshot.into_parts();
    Ok(rows
        .into_iter()
        .filter_map(|(bundle, _digest)| {
            let bound_phase = matches!(
                bundle.seed.phase,
                InputLifecycleState::Staged
                    | InputLifecycleState::Applied
                    | InputLifecycleState::AppliedPendingConsumption
            );
            let run_id = bundle.seed.last_run_id?;
            bound_phase.then_some((bundle.state.input_id, run_id))
        })
        .collect())
}

async fn load_recovery_preparation(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    session_id: &SessionId,
    mode: PreparationMode,
) -> Result<RecoveryCandidatePreparation, DurableTailRecoveryError> {
    let bound_input_runs = load_bound_input_runs(store, runtime_id).await?;
    match store.session_persistence_profile() {
        RuntimeSessionPersistenceProfile::HeadCanonicalV1 => {
            let source = match store.load_durable_tail_recovery_source(runtime_id).await {
                Ok(Some(source)) => source,
                Ok(None) => return Ok(RecoveryCandidatePreparation::Held),
                Err(RuntimeStoreError::PreparedRecoveryRequiresAtomicPhysicalHeadCas {
                    ..
                }) => {
                    return Ok(RecoveryCandidatePreparation::Held);
                }
                Err(error) => return Err(error.into()),
            };
            if source.runtime_authority().session_id() != session_id {
                return Err(DurableTailRecoveryError::InvalidEvidence(format!(
                    "store-owned recovery source belongs to session {}, not requested {session_id}",
                    source.runtime_authority().session_id()
                )));
            }
            prepare_head_canonical_recovery_candidate_in(&source, &bound_input_runs, mode)
        }
        RuntimeSessionPersistenceProfile::WholeBlobV1 => {
            let committed = store
                .load_committed_whole_blob_snapshot(runtime_id)
                .await?
                .ok_or_else(|| {
                    DurableTailRecoveryError::InvalidEvidence(
                        "WholeBlob recovery has no committed base snapshot".to_string(),
                    )
                })?;
            if committed.authority().session_id() != session_id {
                return Err(DurableTailRecoveryError::InvalidEvidence(format!(
                    "WholeBlob committed authority belongs to {}, not requested {session_id}",
                    committed.authority().session_id()
                )));
            }
            let provisional = store.load_whole_blob_provisional_tail(runtime_id).await?;
            prepare_whole_blob_recovery_candidate_in(
                committed,
                provisional,
                &bound_input_runs,
                mode,
            )
        }
    }
}

/// Prove, classify, authorize, and atomically commit one durable-tail recovery.
///
/// This is the only public preparation seam. The caller supplies only the
/// store and stable session identity. A capable store loads one opaque,
/// store-bound source from its own runtime-authority and physical-session
/// rows. The caller cannot supply or mix a committed document, physical head,
/// materialization, class, candidate/run identity, recovered snapshot,
/// store authority, receipt fact, or CAS token. Those are derived and sealed
/// internally before the generated machines are driven. Generated classifier
/// DTOs remain private observations, never public recovery capabilities.
///
/// Machine authorization judges persisted lifecycle, receipts, and
/// input-lifecycle rows. A freshly registered in-process authority alone
/// would be vacuously quiescent, so every durable observation is taken before
/// the drive; afterwards this module only realizes the emitted verdict.
pub async fn recover_durable_tail(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
) -> Result<DurableTailRecoveryOutcome, DurableTailRecoveryError> {
    recover_durable_tail_capturing(
        store,
        session_id,
        &mut None,
        None,
        DriveMode::Recovery,
        mm_dsl::DurableRecoveryOwnership::NotProven,
    )
    .await?
    .into_outcome()
}

/// [`recover_durable_tail`] for the tail of one run: the run that the caller
/// settled without resuming it.
///
/// Its bound inputs are terminal by then, so the tail classifies as an
/// interrupted run and commits as interrupted, exactly as crash recovery
/// commits it. `Ok(None)`, with nothing touched, when the store's provisional
/// tail (or an incomplete HeadCanonical intent) belongs to another run. A
/// tail held as ambiguous answers `Held` whichever run it belongs to, and
/// nothing is touched then either.
///
/// The caller holds the session's driver lock across the call, as the
/// runtime loop's settle of a refused resume and of a cancelled run do (the
/// loop never takes the registration gate here: an unregister holds it while
/// it waits for the loop). The lock orders the call against the loop's own
/// writes, and the store's fences order it against any other process.
pub async fn recover_durable_tail_for_run(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
    run_id: &RunId,
) -> Result<Option<DurableTailRecoveryOutcome>, DurableTailRecoveryError> {
    match recover_durable_tail_capturing(
        store,
        session_id,
        &mut None,
        Some(run_id),
        DriveMode::Recovery,
        mm_dsl::DurableRecoveryOwnership::NotProven,
    )
    .await?
    {
        DriveAnswer::OtherRun => Ok(None),
        answer => answer.into_outcome().map(Some),
    }
}

/// Resolve a session's held durable tail as an operator (`rkat session
/// resolve-held-tail`), through the store traits alone, so a host store is
/// served as a built-in one is.
///
/// Two held shapes qualify. A tail the recovery drive holds as ambiguous
/// takes its machine verdict from the operator-attested
/// `AuthorizeOperatorDurableTailRepair`, which repairs only under the
/// admissible core of an ordinary commit (a quiescent persisted runtime with
/// no current run, no prior commit that covers or contradicts the tail,
/// attributable inputs) and refuses otherwise. An interrupted tail ordinary
/// recovery holds only beside another run's stranded inputs
/// ([`HeldTailResolution::StrandedInputs`]) takes ordinary recovery's own
/// `AuthorizeDurableTailRecovery`, with those inputs attributed to their own
/// run by their run binding; its repair commits the tail as ordinary recovery
/// commits an interrupted tail and requeues those inputs in the same commit.
///
/// The caller opens the store exclusively, so no live process owns the
/// session, and nothing registers its runtime. A lifecycle row a host left
/// `Stopped` when it ended before unregistering (no current run, no
/// unregister in progress) therefore counts as the clean `Idle` row a
/// start's registration would normalize it to: the plan reports it
/// ([`HeldTailRepair::normalizes_stopped_lifecycle`]), and an applied repair
/// writes that row with its commit, fenced on the observed row version.
///
/// With `apply` false (the dry run) nothing is written: the answer reports
/// what the repair keeps, answers, terminalizes and requeues, and the
/// verdict. With `apply` true an authorized repair commits the tail as
/// interrupted through the ordinary recovery commit: every durable row kept,
/// the open calls answered as outcome unknown, the interrupted notice (for an
/// ambiguous tail it names the verb and the candidate), the inputs proved
/// bound to the run terminalized (beside a run cursor, every input it binds
/// that has not ended: their content is in the tail), a stranded-input
/// repair's inputs requeued, and a recovery row recording the candidate id
/// and its disposition (`operator_repair_and_commit_interrupted` for an
/// ambiguous tail, ordinary recovery's `repair_and_commit_interrupted` for
/// stranded inputs). Evidence that cannot be verified is an error, and the
/// tail stays held.
pub async fn resolve_held_durable_tail(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
    apply: bool,
) -> Result<HeldTailResolution, DurableTailRecoveryError> {
    let answer = recover_durable_tail_capturing(
        store,
        session_id,
        &mut None,
        None,
        DriveMode::Operator { apply },
        mm_dsl::DurableRecoveryOwnership::NotProven,
    )
    .await?;
    Ok(match answer {
        DriveAnswer::Outcome(DurableTailRecoveryOutcome::AlreadyAligned { .. }) => {
            HeldTailResolution::NoTail
        }
        DriveAnswer::Outcome(DurableTailRecoveryOutcome::Held) => {
            HeldTailResolution::HeldOtherwise {
                reason: "the store cannot prove an exact recovery source or boundary, or the \
                         tail's descent from the committed head"
                    .to_string(),
            }
        }
        DriveAnswer::NotHeld(class) => HeldTailResolution::NotHeld { class },
        DriveAnswer::HeldOtherwise(reason) => HeldTailResolution::HeldOtherwise { reason },
        DriveAnswer::Operator(repair) => HeldTailResolution::Ambiguous(repair),
        DriveAnswer::Stranded(repair) => HeldTailResolution::StrandedInputs(repair),
        DriveAnswer::Outcome(_) | DriveAnswer::OtherRun => {
            return Err(DurableTailRecoveryError::Authority(
                "the operator recovery drive answered an ordinary recovery outcome".to_string(),
            ));
        }
    })
}

/// Who drives a recovery: ordinary recovery, the archive of a session that is
/// ending, or an operator resolving a held tail (a dry run unless `apply`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriveMode {
    Recovery,
    /// Ordinary recovery for the archive of a session that is ending; it
    /// reads the persisted runtime as retired (`observe_persisted_lifecycle`).
    SessionArchive,
    Operator {
        apply: bool,
    },
}

impl DriveMode {
    fn preparation(self) -> PreparationMode {
        match self {
            Self::Recovery | Self::SessionArchive => PreparationMode::Recovery,
            Self::Operator { .. } => PreparationMode::Operator,
        }
    }
}

/// What one recovery drive answers.
enum DriveAnswer {
    Outcome(DurableTailRecoveryOutcome),
    /// A run guard named a run whose tail the store does not hold.
    OtherRun,
    /// Operator mode: the tail is not held as ambiguous.
    NotHeld(DurableTailRecoveryClass),
    /// Operator mode: held for a reason the operator verb does not resolve.
    HeldOtherwise(String),
    /// Operator mode: the repair, planned or applied.
    Operator(Box<HeldTailRepair>),
    /// Operator mode: the repair of an interrupted tail held beside another
    /// run's stranded inputs, planned or applied.
    Stranded(Box<HeldTailRepair>),
}

impl DriveAnswer {
    fn into_outcome(self) -> Result<DurableTailRecoveryOutcome, DurableTailRecoveryError> {
        match self {
            Self::Outcome(outcome) => Ok(outcome),
            Self::OtherRun
            | Self::NotHeld(_)
            | Self::HeldOtherwise(_)
            | Self::Operator(_)
            | Self::Stranded(_) => Err(DurableTailRecoveryError::Authority(
                "an ordinary recovery drive answered for another run or an operator".to_string(),
            )),
        }
    }
}

/// The operator repair a held candidate plans (an ambiguous one, or an
/// interrupted one beside `stranded` inputs), with the machine's verdict.
fn operator_plan(
    candidate: &PreparedRecoveryCandidate,
    inputs: &CandidateInputObservation,
    stranded: Option<&[(StoredInputState, String)]>,
    observed: &ObservedPersistedLifecycle,
    verdict: HeldTailRepairVerdict,
) -> HeldTailRepair {
    HeldTailRepair {
        candidate_id: candidate.candidate_id.clone(),
        run_id: candidate.candidate_run_id.clone(),
        facts: candidate.tail_facts.clone(),
        messages_before: candidate.physical_message_count,
        messages_after: candidate.message_count,
        closed_calls: candidate.closed_call_ids.clone(),
        terminalized_inputs: inputs
            .attributed
            .iter()
            .map(|(bundle, _)| bundle.state.input_id.clone())
            .collect(),
        requeued_inputs: stranded
            .unwrap_or_default()
            .iter()
            .map(|(bundle, _)| bundle.state.input_id.clone())
            .collect(),
        runtime_quiescent: matches!(
            observed.lifecycle,
            mm_dsl::DurableRecoveryObservedLifecycle::MissingRow
                | mm_dsl::DurableRecoveryObservedLifecycle::Idle
                | mm_dsl::DurableRecoveryObservedLifecycle::Retired
        ) && observed.current_run == mm_dsl::DurableRecoveryObservedRun::NoRun,
        normalizes_stopped_lifecycle: observed.normalizes_stopped,
        verdict,
    }
}

/// [`recover_durable_tail`], additionally returning the exact committed
/// WholeBlob snapshot an `AlreadyAligned` outcome's body was decoded from.
///
/// The snapshot is the store's own verified read (its bytes hash to the
/// store-issued authority it carries). A session owner can therefore adopt
/// it as the committed body for that exact authority instead of reading and
/// decoding the same document again. `None` for every other outcome and for
/// HeadCanonical sources.
pub async fn recover_durable_tail_retaining_committed_whole_blob(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
) -> Result<
    (
        DurableTailRecoveryOutcome,
        Option<crate::store::CommittedWholeBlobSnapshot>,
    ),
    DurableTailRecoveryError,
> {
    let mut committed_whole_blob = None;
    let outcome = recover_durable_tail_capturing(
        store,
        session_id,
        &mut committed_whole_blob,
        None,
        DriveMode::Recovery,
        mm_dsl::DurableRecoveryOwnership::NotProven,
    )
    .await?
    .into_outcome()?;
    Ok((outcome, committed_whole_blob))
}

/// [`recover_durable_tail_retaining_committed_whole_blob`] for the archive
/// of a session that is ending.
///
/// The caller is an archive that holds the session's turn boundary, its
/// recovery gate and its runtime archive lease, and the session ends with
/// that archive: no later start resumes its run. The persisted runtime is
/// therefore judged as retired here whenever it is quiescent (a missing row,
/// `Idle`, `Stopped` or `Retired`), and the recovery re-asserts the observed
/// phase. The machine never continues a run then: a continuable tail is
/// repaired and committed as interrupted, and its bound inputs are
/// terminalized in the same commit, so this never answers
/// [`DurableTailRecoveryOutcome::Continue`]. Ordinary recovery keeps
/// refusing a `Stopped` runtime, and continues a continuable tail of a
/// runtime that is not retired, because a later start may resume its run.
/// Every other guard is unchanged: a persisted current run still refuses,
/// and the commit is fenced on the exact lifecycle row version the machine
/// judged.
pub async fn recover_durable_tail_for_session_archive(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
) -> Result<
    (
        DurableTailRecoveryOutcome,
        Option<crate::store::CommittedWholeBlobSnapshot>,
    ),
    DurableTailRecoveryError,
> {
    let mut committed_whole_blob = None;
    let outcome = recover_durable_tail_capturing(
        store,
        session_id,
        &mut committed_whole_blob,
        None,
        DriveMode::SessionArchive,
        mm_dsl::DurableRecoveryOwnership::NotProven,
    )
    .await?
    .into_outcome()?;
    Ok((outcome, committed_whole_blob))
}

/// What a recovery drive under the session's hosting claim is for
/// ([`recover_durable_tail_under_hosting_claim`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DurableTailDriveRole {
    /// A start, a resume or a cold attach: the caller's runtime owner hosts
    /// the session next, so the tail is recovered as a restart recovers it,
    /// and a continuable run resumes.
    Host,
    /// A store-only write (a System message appended to a session this
    /// runtime owner does not host): the caller cannot run the session's
    /// run, so an open run is committed as interrupted before the write.
    Writer,
}

/// [`recover_durable_tail_retaining_committed_whole_blob`] under the
/// session's hosting claim, which the caller holds for the whole call.
///
/// When typed store state proves that the caller holds the session's
/// exclusive write authority, the persisted lifecycle row of an earlier
/// incarnation no longer gates the recovery: that incarnation can no longer
/// commit. The proof is never inferred from timing or from the absence of
/// activity. It is one of:
/// - the claim is a cross-process OS lock held by the caller's lineage: the
///   kernel releases it only when its holder dies, and a live holder never
///   loses it, so no earlier holder can still write the session;
/// - the store is a host store realm whose fence this process holds (its
///   self-fence window is open): the open waited out any superseded holder's
///   lease before it served anything, and a fenced-out holder commits
///   nothing.
///
/// Without either proof the persisted lifecycle gates exactly as for
/// [`recover_durable_tail`]. With it, `role` decides what happens to an open
/// run: a host continues a continuable run, as at a restart; a writer
/// commits it as interrupted. A persisted current run that names another
/// run, or a lifecycle row that does not decode, still refuses.
///
/// `owner` is the runtime owner the caller drives for, and the proof holds
/// only for a claim of its own lineage, taken while it does not have the
/// session registered. The drive is NotProven otherwise:
/// - `claim` is another runtime owner's: it proves nothing about `owner`;
/// - `owner` has the session registered: its lineage shares the claim, so
///   the claim proves nothing against that registration, which may still
///   write the lifecycle row while its run is live.
///
/// The registration check is a point-in-time read; a registration that
/// lands before the recovery's commit changes the lifecycle row the commit
/// is fenced on, so the commit fails typed instead of overwriting it.
///
/// After a host store takeover, a superseded holder may still be finishing
/// a tool call it dispatched before its window closed. A writer's
/// interrupted commit answers every open call as outcome unknown, and a
/// resumed run follows the documented at-least-once tool contract.
pub async fn recover_durable_tail_under_hosting_claim(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
    owner: &crate::meerkat_machine::MeerkatMachine,
    claim: &meerkat_core::session_hosting::HostingClaim,
    role: DurableTailDriveRole,
) -> Result<
    (
        DurableTailRecoveryOutcome,
        Option<crate::store::CommittedWholeBlobSnapshot>,
    ),
    DurableTailRecoveryError,
> {
    if claim.session_id() != session_id {
        return Err(DurableTailRecoveryError::InvalidEvidence(format!(
            "durable-tail recovery of session {session_id} was offered the hosting claim of session {}",
            claim.session_id()
        )));
    }
    let ownership = if claim.owner() != owner.hosting_authority().owner()
        || owner.contains_session(session_id).await
    {
        mm_dsl::DurableRecoveryOwnership::NotProven
    } else {
        proven_ownership(store, claim, role)
    };
    let mut committed_whole_blob = None;
    let outcome = recover_durable_tail_capturing(
        store,
        session_id,
        &mut committed_whole_blob,
        None,
        DriveMode::Recovery,
        ownership,
    )
    .await?
    .into_outcome()?;
    Ok((outcome, committed_whole_blob))
}

/// The ownership a drive under `claim` proves (see
/// [`recover_durable_tail_under_hosting_claim`]).
fn proven_ownership(
    store: &dyn RuntimeStore,
    claim: &meerkat_core::session_hosting::HostingClaim,
    role: DurableTailDriveRole,
) -> mm_dsl::DurableRecoveryOwnership {
    let exclusive = claim.is_cross_process()
        || store
            .self_fence()
            .is_some_and(|fence| fence.window().is_open());
    match (exclusive, role) {
        (false, _) => mm_dsl::DurableRecoveryOwnership::NotProven,
        (true, DurableTailDriveRole::Host) => mm_dsl::DurableRecoveryOwnership::ExclusiveHost,
        (true, DurableTailDriveRole::Writer) => mm_dsl::DurableRecoveryOwnership::ExclusiveWriter,
    }
}

/// The recovery drive: ordinary, an archive's, or an operator's. It answers
/// `OtherRun` only when `expected_run` names a run whose tail the store does
/// not hold, before anything is written. In operator mode it never rolls back
/// an intent, plans only an ambiguous candidate or an interrupted one held
/// beside another run's stranded inputs, and writes only when `apply` is set.
async fn recover_durable_tail_capturing(
    store: &dyn RuntimeStore,
    session_id: &SessionId,
    aligned_committed_whole_blob: &mut Option<crate::store::CommittedWholeBlobSnapshot>,
    expected_run: Option<&RunId>,
    mode: DriveMode,
    ownership: mm_dsl::DurableRecoveryOwnership,
) -> Result<DriveAnswer, DurableTailRecoveryError> {
    let runtime_id = LogicalRuntimeId::for_session(session_id);
    let mut preparation =
        load_recovery_preparation(store, &runtime_id, session_id, mode.preparation()).await?;
    let mut rolled_back_incomplete_intent = false;
    let candidate = loop {
        match preparation {
            RecoveryCandidatePreparation::Prepared(candidate) => break candidate,
            RecoveryCandidatePreparation::AlreadyAligned {
                recovered,
                committed_whole_blob,
            } => {
                *aligned_committed_whole_blob = committed_whole_blob;
                tracing::info!(
                    %session_id,
                    "durable-tail recovery source is already exactly aligned"
                );
                let recovered =
                    Arc::try_unwrap(recovered).unwrap_or_else(|shared| shared.as_ref().clone());
                return Ok(DriveAnswer::Outcome(
                    DurableTailRecoveryOutcome::AlreadyAligned {
                        recovered: Box::new(recovered),
                    },
                ));
            }
            RecoveryCandidatePreparation::IncompleteHeadCanonicalIntent { provisional } => {
                if expected_run.is_some_and(|run| run != provisional.run_id()) {
                    return Ok(DriveAnswer::OtherRun);
                }
                if let DriveMode::Operator { .. } = mode {
                    return Ok(DriveAnswer::HeldOtherwise(
                        "an incomplete HeadCanonical intent, which ordinary recovery rolls back \
                         when the session next resumes"
                            .to_string(),
                    ));
                }
                if rolled_back_incomplete_intent {
                    return Err(DurableTailRecoveryError::InvalidEvidence(
                        "HeadCanonical provisional rollback did not expose a stable physical authority"
                            .to_string(),
                    ));
                }
                if !store
                    .discard_head_canonical_provisional_tail(&runtime_id, &provisional)
                    .await?
                {
                    tracing::warn!(
                        %session_id,
                        "durable-tail recovery held: incomplete HeadCanonical intent changed before exact rollback"
                    );
                    return Ok(DriveAnswer::Outcome(DurableTailRecoveryOutcome::Held));
                }
                tracing::info!(
                    %session_id,
                    physical_revision = provisional.physical_store_revision(),
                    "rolled back incomplete HeadCanonical provisional intent"
                );
                rolled_back_incomplete_intent = true;
                preparation =
                    load_recovery_preparation(store, &runtime_id, session_id, mode.preparation())
                        .await?;
            }
            RecoveryCandidatePreparation::Held => {
                return Ok(DriveAnswer::Outcome(DurableTailRecoveryOutcome::Held));
            }
        }
    };

    if expected_run.is_some_and(|run| *run != candidate.candidate_run_id) {
        return Ok(DriveAnswer::OtherRun);
    }
    // An operator's drive answers for a candidate it does not repair with
    // ordinary recovery's own verdict, and writes nothing: such a tail may be
    // held or refused for a reason the operator verb does not resolve.
    let ordinary_probe = matches!(mode, DriveMode::Operator { .. })
        && candidate.class != DurableTailRecoveryClass::Ambiguous;

    let observed = match store.observe_machine_lifecycle(&runtime_id).await {
        Ok(observation) => {
            observe_persisted_lifecycle(observation, &candidate.candidate_run_id, mode)
        }
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
            live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage::default(),
            placement_witness: None,
            normalizes_stopped: false,
        },
        Err(error) => return Err(error.into()),
    };
    // Re-assert the observed quiescent lifecycle (never a new phase), fenced
    // on the exact row version the machine judged. The expected version is a
    // first-apply CAS fence; the target record itself is the durable outcome
    // identity used by exact retry convergence.
    let lifecycle = durable_tail_lifecycle_reassertion(&observed);

    let prior_recovery = match load_exact_committed_recovery(
        store,
        &runtime_id,
        &candidate,
        &lifecycle,
    )
    .await
    {
        Ok(boundary) => boundary,
        Err(DurableTailRecoveryError::Store(
            RuntimeStoreError::PreparedRecoveryRequiresAtomicPhysicalHeadCas { profile },
        )) => {
            tracing::warn!(
                session_id = %candidate.session_id,
                %profile,
                "durable-tail recovery held: store cannot prove an exact atomic recovery boundary"
            );
            return Ok(DriveAnswer::Outcome(DurableTailRecoveryOutcome::Held));
        }
        Err(error) => return Err(error),
    };
    if let Some((disposition, boundary_sequence)) = prior_recovery {
        tracing::info!(
            session_id = %candidate.session_id,
            candidate_run_id = %candidate.candidate_run_id,
            ?disposition,
            boundary_sequence,
            "durable-tail recovery converged on an exact committed witness"
        );
        if ordinary_probe {
            // Ordinary recovery converges on that commit at the next resume.
            return Ok(DriveAnswer::NotHeld(candidate.class));
        }
        if let DriveMode::Operator { .. } = mode {
            return Ok(DriveAnswer::HeldOtherwise(format!(
                "a recovery of this exact tail already committed at boundary sequence \
                 {boundary_sequence}"
            )));
        }
        return Ok(DriveAnswer::Outcome(committed_recovery_outcome(
            candidate,
            disposition,
            boundary_sequence,
        )));
    }

    // Durably committed receipts for the candidate run: an interrupted tool
    // loop can have committed BoundaryContinue receipts before losing only
    // its final boundary. They carry (a) the last committed sequence the
    // machine mints past, (b) exact contributing input identities, and (c)
    // whether this exact recovery already landed.
    let committed_receipt_sources = match store
        .load_durable_tail_recovery_receipts(&runtime_id, &candidate.candidate_run_id)
        .await
    {
        Ok(receipts) => receipts,
        Err(RuntimeStoreError::PreparedRecoveryRequiresAtomicPhysicalHeadCas { profile }) => {
            tracing::warn!(
                session_id = %candidate.session_id,
                %profile,
                "durable-tail recovery held: store cannot load exact receipt rows"
            );
            return Ok(DriveAnswer::Outcome(DurableTailRecoveryOutcome::Held));
        }
        Err(error) => return Err(error.into()),
    };
    let receipt_digest_enrichments =
        prepare_receipt_digest_enrichments(&candidate, &committed_receipt_sources)?;
    let committed_receipts =
        materialize_verified_receipts(&committed_receipt_sources, &receipt_digest_enrichments)?;
    let prior = observe_prior_commits(
        &committed_receipts,
        candidate.recovered.as_ref(),
        &candidate.candidate_run_id,
    );

    let inputs = observe_candidate_run_inputs(
        store,
        &runtime_id,
        &candidate.candidate_run_id,
        &prior.receipt_bound_inputs,
        candidate.cursor_bound_inputs.as_deref(),
    )
    .await?;
    // An operator's drive also resolves one held shape that is not
    // ambiguous: an interrupted tail held only beside another run's stranded
    // inputs. The run binding of each such input proves it is not the tail's
    // own, so the operator attests the input evidence as attributed, and the
    // repair requeues those inputs. Ordinary recovery never takes this path.
    let stranded = if ordinary_probe
        && candidate.class == DurableTailRecoveryClass::InterruptedRepairableCandidate
        && inputs.evidence == mm_dsl::DurableRecoveryInputEvidence::UnboundContentInput
    {
        observe_stranded_run_inputs(
            store,
            &runtime_id,
            &candidate,
            &prior.receipt_bound_inputs,
            inputs.predecessor_nonterminal_input_set_revision,
        )
        .await?
    } else {
        None
    };
    let input_evidence = if stranded.is_some() {
        mm_dsl::DurableRecoveryInputEvidence::AllBoundOrInert
    } else {
        inputs.evidence
    };

    let mut authority =
        crate::meerkat_machine::dsl_authority::new_registered_authority_without_runtime_entry(
            &candidate.session_id,
        )
        .map_err(|error| DurableTailRecoveryError::Authority(error.to_string()))?;
    // Only an operator's ambiguous candidate is attested; everything else
    // takes ordinary recovery's own input.
    let operator_attestation = matches!(mode, DriveMode::Operator { .. }) && !ordinary_probe;
    let authorization = if operator_attestation {
        mm_dsl::MeerkatMachineInput::AuthorizeOperatorDurableTailRepair {
            session_id: mm_dsl::SessionId::from_domain(&candidate.session_id),
            candidate_id: candidate.candidate_id.clone(),
            candidate_run_id: mm_dsl::RunId(candidate.candidate_run_id.to_string()),
            class: candidate.class,
            observed_lifecycle: observed.lifecycle,
            observed_current_run: observed.current_run,
            last_committed_sequence: prior.last_committed_sequence,
            prior_commit: prior.classification,
            input_evidence,
        }
    } else {
        mm_dsl::MeerkatMachineInput::AuthorizeDurableTailRecovery {
            session_id: mm_dsl::SessionId::from_domain(&candidate.session_id),
            candidate_id: candidate.candidate_id.clone(),
            candidate_run_id: mm_dsl::RunId(candidate.candidate_run_id.to_string()),
            class: candidate.class,
            observed_lifecycle: observed.lifecycle,
            observed_current_run: observed.current_run,
            last_committed_sequence: prior.last_committed_sequence,
            prior_commit: prior.classification,
            input_evidence,
            ownership,
        }
    };
    let transition = mm_dsl::MeerkatMachineMutator::apply(&mut authority, authorization)
        .map_err(|error| DurableTailRecoveryError::Authority(error.to_string()))?;

    let mut commit_verdict: Option<(DurableTailRecoveryDisposition, u64)> = None;
    let mut non_commit_verdict: Option<DurableTailRecoveryDisposition> = None;
    for effect in transition.effects() {
        match effect {
            mm_dsl::MeerkatMachineEffect::DurableTailRecoveryCommitAuthorized {
                candidate_id,
                disposition,
                boundary_sequence,
            } if *candidate_id == candidate.candidate_id => {
                commit_verdict = Some((*disposition, *boundary_sequence));
            }
            mm_dsl::MeerkatMachineEffect::DurableTailRecoveryAuthorized {
                candidate_id,
                disposition,
            } if *candidate_id == candidate.candidate_id => {
                non_commit_verdict = Some(*disposition);
            }
            _ => {}
        }
    }

    if ordinary_probe && stranded.is_none() {
        return match (commit_verdict, non_commit_verdict) {
            (Some(_), _) | (None, Some(DurableTailRecoveryDisposition::ContinueRun)) => {
                Ok(DriveAnswer::NotHeld(candidate.class))
            }
            (None, Some(DurableTailRecoveryDisposition::HoldIntact)) => {
                Ok(DriveAnswer::HeldOtherwise(
                    "ordinary recovery holds this tail because its input rows cannot be \
                     attributed to its run or fenced (an unbound content row next to an \
                     interrupted run, or rows the store cannot fence)"
                        .to_string(),
                ))
            }
            (None, Some(DurableTailRecoveryDisposition::RefuseRecovery)) => {
                Ok(DriveAnswer::HeldOtherwise(
                    "ordinary recovery refuses this tail: the persisted runtime is not \
                     quiescent or has a current run (retry once every host is stopped), or \
                     a prior commit covers or contradicts it"
                        .to_string(),
                ))
            }
            (None, other) => Err(DurableTailRecoveryError::Authority(format!(
                "ordinary recovery answered the probe with {other:?}"
            ))),
        };
    }

    let (disposition, boundary_sequence) = match (commit_verdict, non_commit_verdict) {
        (Some((disposition, sequence)), _) => (disposition, sequence),
        (
            None,
            Some(
                non_commit @ (DurableTailRecoveryDisposition::HoldIntact
                | DurableTailRecoveryDisposition::RefuseRecovery),
            ),
        ) => {
            // A competing process can commit after our initial witness read
            // but before receipt/input observation. Never collapse that race
            // into a hold/refusal: re-read the durable recovery boundary and
            // converge only after reconstructing its exact source, recovered
            // store authority, receipt, lifecycle target, and migration witness.
            if let Some((disposition, boundary_sequence)) =
                load_exact_committed_recovery(store, &runtime_id, &candidate, &lifecycle).await?
            {
                tracing::info!(
                    session_id = %candidate.session_id,
                    candidate_run_id = %candidate.candidate_run_id,
                    ?disposition,
                    boundary_sequence,
                    "durable-tail recovery converged after a competing exact commit"
                );
                if let DriveMode::Operator { .. } = mode {
                    return Ok(DriveAnswer::HeldOtherwise(format!(
                        "a recovery of this exact tail already committed at boundary sequence \
                         {boundary_sequence}"
                    )));
                }
                return Ok(DriveAnswer::Outcome(committed_recovery_outcome(
                    candidate,
                    disposition,
                    boundary_sequence,
                )));
            }
            match non_commit {
                // Defensive: no operator verdict holds. The operator
                // transitions answer a commit or a refusal, and the ordinary
                // verdict a stranded-input drive takes holds an interrupted
                // tail only on the input evidence that drive replaces.
                DurableTailRecoveryDisposition::HoldIntact
                    if matches!(mode, DriveMode::Operator { .. }) =>
                {
                    return Ok(DriveAnswer::HeldOtherwise(
                        "ordinary recovery holds this tail for a reason other than the inputs \
                         another run left beside it"
                            .to_string(),
                    ));
                }
                DurableTailRecoveryDisposition::HoldIntact => {
                    tracing::warn!(
                        session_id = %candidate.session_id,
                        candidate_run_id = %candidate.candidate_run_id,
                        class = ?candidate.class,
                        prior_commit = ?prior.classification,
                        input_evidence = ?inputs.evidence,
                        "durable-tail recovery held intact by machine verdict"
                    );
                    return Ok(DriveAnswer::Outcome(DurableTailRecoveryOutcome::Held));
                }
                DurableTailRecoveryDisposition::RefuseRecovery => {
                    if let DriveMode::Operator { .. } = mode {
                        let plan = Box::new(operator_plan(
                            &candidate,
                            &inputs,
                            stranded.as_deref(),
                            &observed,
                            HeldTailRepairVerdict::Refused,
                        ));
                        return Ok(if stranded.is_some() {
                            DriveAnswer::Stranded(plan)
                        } else {
                            DriveAnswer::Operator(plan)
                        });
                    }
                    return Ok(DriveAnswer::Outcome(DurableTailRecoveryOutcome::Refused));
                }
                other => {
                    return Err(DurableTailRecoveryError::Authority(format!(
                        "generated machine emitted unexpected non-commit disposition {other:?}"
                    )));
                }
            }
        }
        (None, Some(DurableTailRecoveryDisposition::ContinueRun)) => {
            let PreparedRecoveryCandidate {
                candidate_run_id,
                recovered,
                document,
                continuation,
                ..
            } = candidate;
            drop(document);
            drop(recovered);
            let continuation = continuation.ok_or_else(|| {
                DurableTailRecoveryError::Authority(
                    "machine authorized continuing a candidate prepared without its continuation"
                        .to_string(),
                )
            })?;
            tracing::info!(
                %session_id,
                %candidate_run_id,
                "durable-tail recovery continues the open run from its last committed round"
            );
            let PreparedContinuation {
                candidate: continued,
                receipt,
            } = continuation;
            let continued =
                Arc::try_unwrap(continued).unwrap_or_else(|shared| shared.as_ref().clone());
            return Ok(DriveAnswer::Outcome(DurableTailRecoveryOutcome::Continue {
                candidate: Box::new(continued),
                run_id: candidate_run_id,
                receipt,
            }));
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

    let operator_repair = match mode {
        DriveMode::Recovery | DriveMode::SessionArchive => None,
        DriveMode::Operator { apply } => {
            let plan = operator_plan(
                &candidate,
                &inputs,
                stranded.as_deref(),
                &observed,
                HeldTailRepairVerdict::WouldRepair,
            );
            if !apply {
                return Ok(if stranded.is_some() {
                    DriveAnswer::Stranded(Box::new(plan))
                } else {
                    DriveAnswer::Operator(Box::new(plan))
                });
            }
            Some(plan)
        }
    };
    let stranded_shape = stranded.is_some();

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
    let CandidateInputObservation {
        evidence: _,
        predecessor_nonterminal_input_set_revision,
        predecessor_nonterminal_input_set_token,
        attributed,
    } = inputs;
    let predecessor_nonterminal_input_set_revision = predecessor_nonterminal_input_set_revision
        .ok_or_else(|| {
            DurableTailRecoveryError::Authority(
                "machine authorized recovery without an exact input-set revision".to_string(),
            )
        })?;
    let predecessor_nonterminal_input_set_token = predecessor_nonterminal_input_set_token
        .ok_or_else(|| {
            DurableTailRecoveryError::Authority(
                "machine authorized recovery without an exact nonterminal input-set witness"
                    .to_string(),
            )
        })?;
    // The contributors this boundary consumes get their terminal in the same
    // fenced commit: one run-scoped terminal-completion batch, and a directed
    // outbox for each directed contributor, which the next drain finalizes
    // and publishes. Without it a consumed contributor has no receipt to read
    // and its requester no terminal.
    let terminal_batch = recovered_boundary_terminal_batch(
        store,
        &runtime_id,
        &candidate,
        disposition,
        &attributed,
        crate::input_state::InteractionTerminalPlacement::from_lifecycle_row(
            &observed.binding,
            observed.placement_witness.as_ref(),
        ),
    )
    .await?;
    let mut input_updates = terminalize_attributed_inputs(
        attributed,
        &candidate.candidate_run_id,
        boundary_sequence,
        terminal_batch,
    )?;
    let contributing_input_ids: Vec<InputId> = input_updates
        .iter()
        .map(|record| record.as_stored().state.input_id.clone())
        .collect();
    // Another run's stranded inputs are no contributors of the recovered
    // boundary: they go back to their lane in the same fenced commit.
    if let Some(rows) = stranded {
        input_updates.extend(requeue_stranded_inputs(rows)?);
    }
    // Retained rows are deliberately not rewritten. The store-owned input-set
    // revision fences every mutation since observation, while the exact set
    // token records the classified identities and bytes in durable evidence.
    // Re-upserting every retained row would turn one recovered delta back into
    // O(current outstanding work) writes without strengthening the proof.
    input_updates.sort_by_key(|record| record.as_stored().state.input_id.to_string());

    let receipt = RunBoundaryReceipt {
        run_id: candidate.candidate_run_id.clone(),
        // The recovered boundary applies at commit time; no live run exists
        // to carry a checkpoint position.
        boundary: RunApplyBoundary::Immediate,
        contributing_input_ids,
        conversation_digest: Some(candidate.conversation_digest.clone()),
        message_count: candidate.message_count,
        // Machine-minted: one past the last durably committed receipt for
        // this run. The (runtime_id, run_id, sequence) key fences only a
        // SAME-sequence race; a recovery that already landed is fenced by the
        // machine's prior-commit guard, not by this key.
        sequence: boundary_sequence,
        owner_contributions: Vec::new(),
    };
    let document = bind_recovered_store_document(&candidate)?;
    let evidence = seal_recovery_evidence(
        &candidate,
        document.as_ref(),
        disposition,
        receipt_digest_enrichments,
        predecessor_nonterminal_input_set_revision,
        predecessor_nonterminal_input_set_token,
        input_updates,
        &receipt,
        &lifecycle,
    )?;
    let request = match &candidate.store_transition {
        PreparedRecoveryStoreTransition::WholeBlob { .. } => {
            PreparedRuntimeSessionCommit::machine_terminal_whole_blob_recovery(
                document,
                evidence,
                receipt,
                lifecycle,
                candidate.session_id.clone(),
            )
        }
        PreparedRecoveryStoreTransition::HeadCanonical { .. } => {
            let document = document.ok_or_else(|| {
                DurableTailRecoveryError::Authority(
                    "HeadCanonical recovery lost its prepared mutation before commit".to_string(),
                )
            })?;
            PreparedRuntimeSessionCommit::machine_terminal_recovery(
                document,
                evidence,
                receipt,
                lifecycle,
                candidate.session_id.clone(),
            )
        }
    }?;
    let committed = match store
        .commit_prepared_session_boundary(&runtime_id, request)
        .await
    {
        Ok(result) => result,
        Err(RuntimeStoreError::PreparedRecoveryRequiresAtomicPhysicalHeadCas { profile }) => {
            tracing::warn!(
                session_id = %candidate.session_id,
                %profile,
                "durable-tail recovery held: store lacks atomic physical-head CAS"
            );
            return Ok(DriveAnswer::Outcome(DurableTailRecoveryOutcome::Held));
        }
        Err(error) => return Err(error.into()),
    };
    let status = committed.recovery_status().ok_or_else(|| {
        DurableTailRecoveryError::Authority(
            "prepared recovery commit returned no exact recovery status".to_string(),
        )
    })?;
    let committed_authority = committed.authority().ok_or_else(|| {
        DurableTailRecoveryError::Authority(
            "prepared recovery commit returned no successor session authority".to_string(),
        )
    })?;
    let successor_matches = match &candidate.store_transition {
        PreparedRecoveryStoreTransition::WholeBlob {
            base_store_revision,
            recovered_blob_sha256,
            ..
        } => committed_authority.whole_blob().is_some_and(|authority| {
            authority.session_id() == &candidate.session_id
                && base_store_revision
                    .checked_add(1)
                    .is_some_and(|expected| authority.store_revision() == expected)
                && authority.blob_sha256() == recovered_blob_sha256
        }),
        PreparedRecoveryStoreTransition::HeadCanonical {
            physical_store_revision,
            recovered_head_token,
            ..
        } => committed_authority
            .head_canonical()
            .is_some_and(|authority| {
                authority.session_id() == &candidate.session_id
                    && physical_store_revision
                        .checked_add(1)
                        .is_some_and(|expected| authority.store_revision() == expected)
                    && authority.committed_head_token() == recovered_head_token
            }),
    };
    if !successor_matches {
        return Err(DurableTailRecoveryError::Authority(
            "prepared recovery commit returned a different successor authority".to_string(),
        ));
    }
    tracing::info!(
        session_id = %candidate.session_id,
        candidate_run_id = %candidate.candidate_run_id,
        ?disposition,
        ?status,
        boundary_sequence,
        message_count = candidate.message_count,
        "durable-tail recovery committed as a recovered runtime boundary"
    );
    let outcome = committed_recovery_outcome(candidate, disposition, boundary_sequence);
    Ok(match operator_repair {
        Some(mut repair) => {
            repair.verdict = HeldTailRepairVerdict::Repaired { boundary_sequence };
            if stranded_shape {
                DriveAnswer::Stranded(Box::new(repair))
            } else {
                DriveAnswer::Operator(Box::new(repair))
            }
        }
        None => DriveAnswer::Outcome(outcome),
    })
}

/// What the input-lifecycle rows say about the candidate run, plus the rows a
/// commit would terminalize. The evidence class is the machine's input; the
/// attributed rows are realized only after a commit verdict.
struct CandidateInputObservation {
    evidence: mm_dsl::DurableRecoveryInputEvidence,
    /// Store-owned revision observed in the same backend snapshot as the
    /// classified rows. First apply compares it in O(1) inside the write
    /// transaction, including for the empty-set absence case.
    predecessor_nonterminal_input_set_revision: Option<RecoveryInputSetRevision>,
    /// Exact store-owned set/absence token over every nonterminal row that
    /// could affect this classification. `None` means the store could not
    /// provide an atomically enforceable complete-set witness.
    predecessor_nonterminal_input_set_token: Option<String>,
    /// Non-terminal rows durable identity attributed to the candidate run,
    /// each paired with the row digest the commit fences on. Populated
    /// whenever the rows could be scanned — including under
    /// `UnboundContentInput`, where the retain-inputs commit still
    /// terminalizes exactly these proven-bound rows.
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
/// A tail whose run cursor agrees with it (a round commit's) names its own
/// inputs: every round commit lists in the cursor each input the run wrote
/// into its turn, its batch at run start and each join at the boundary that
/// appended it. So the cursor's inputs, and those a committed receipt for the
/// candidate run names, are attributed to the candidate; nothing else is,
/// even a row naming the candidate (a join the cancel requeued before any
/// round commit listed it is not in the tail, so it is redelivered, not
/// consumed with the tail). A content row the cursor does not list is inert
/// when it names no run or the candidate: it cannot be in the tail. One
/// naming another run is reported as `UnboundContentInput`.
///
/// A tail without an agreeing cursor (a closing save, an older shape, or a
/// cursor the classifier judged to disagree with the tail) keeps the durable
/// run association:
/// 1. Records the persisted machine facts already bound to the candidate run
///    (`seed.last_run_id`).
/// 2. Records a durably committed receipt for the candidate run names in its
///    `contributing_input_ids`.
///
/// Without an agreeing cursor, anything else that is non-terminal and carries
/// redeliverable content might be the tail's own input whose binding never
/// became durable (text equality is content evidence, never identity), and
/// is reported as `UnboundContentInput`. A store that cannot version input
/// rows while blocking rows exist is reported as `Unfenceable`. The
/// DISPOSITION for both belongs to the machine.
async fn observe_candidate_run_inputs(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    candidate_run_id: &meerkat_core::RunId,
    receipt_bound_inputs: &BTreeSet<String>,
    cursor_bound_inputs: Option<&[InputId]>,
) -> Result<CandidateInputObservation, DurableTailRecoveryError> {
    let snapshot = match store.load_input_states_with_versions(runtime_id).await {
        Ok(snapshot) => snapshot,
        // An unversioned row scan cannot be carried into the atomic recovery
        // commit: even an empty result has no range/absence fence and can race
        // a durable input insertion. Unsupported therefore proves no
        // observation capability, never "this store has no input state".
        // A future no-input store needs an explicit sealed capability.
        Err(RuntimeStoreError::Unsupported(_)) => {
            return Ok(CandidateInputObservation {
                evidence: mm_dsl::DurableRecoveryInputEvidence::Unfenceable,
                predecessor_nonterminal_input_set_revision: None,
                predecessor_nonterminal_input_set_token: None,
                attributed: Vec::new(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    if snapshot.runtime_id() != runtime_id {
        return Err(DurableTailRecoveryError::Authority(format!(
            "store returned a recovery input snapshot for {}, not {runtime_id}",
            snapshot.runtime_id()
        )));
    }
    let (rows, predecessor_nonterminal_input_set_revision, predecessor_nonterminal_input_set_token) =
        snapshot.into_parts();

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
    let cursor_bound_inputs = cursor_bound_inputs.map(|inputs| {
        inputs
            .iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>()
    });
    let mut attributed = Vec::new();
    let mut unbound_content_input = false;
    for (bundle, row_digest) in rows {
        if is_terminal(bundle.seed.phase) {
            continue;
        }
        let input_id = bundle.state.input_id.to_string();
        let receipt_bound = receipt_bound_inputs.contains(&input_id);
        let bound_to_candidate = match cursor_bound_inputs.as_ref() {
            Some(cursor_bound) => cursor_bound.contains(&input_id) || receipt_bound,
            None => bundle.seed.last_run_id.as_ref() == Some(candidate_run_id) || receipt_bound,
        };
        if bound_to_candidate {
            attributed.push((bundle, row_digest));
            continue;
        }
        if !carries_redeliverable_content(bundle.state.persisted_input.as_ref()) {
            continue;
        }
        // Beside an agreeing cursor, a row the cursor does not list that
        // names no run, or the candidate, is not in the tail and stays for
        // redelivery. One naming another run is not proven either way, so it
        // holds. Without an agreeing cursor every such row holds.
        let holds = match cursor_bound_inputs {
            Some(_) => bundle
                .seed
                .last_run_id
                .as_ref()
                .is_some_and(|run_id| run_id != candidate_run_id),
            None => true,
        };
        if holds {
            unbound_content_input = true;
        }
    }
    Ok(CandidateInputObservation {
        evidence: if unbound_content_input {
            mm_dsl::DurableRecoveryInputEvidence::UnboundContentInput
        } else {
            mm_dsl::DurableRecoveryInputEvidence::AllBoundOrInert
        },
        predecessor_nonterminal_input_set_revision: Some(
            predecessor_nonterminal_input_set_revision,
        ),
        predecessor_nonterminal_input_set_token: Some(predecessor_nonterminal_input_set_token),
        attributed,
    })
}

/// Realize pass: terminalize the observed rows the recovered boundary
/// consumed, fenced on the exact row bytes the observation read. Called only
/// after a commit verdict, and only over rows the observation attributed.
/// Each row carries its part of `terminal_batch` when the commit stages one.
fn terminalize_attributed_inputs(
    attributed: Vec<(StoredInputState, String)>,
    candidate_run_id: &meerkat_core::RunId,
    boundary_sequence: u64,
    terminal_batch: Option<RecoveredBoundaryTerminalBatch>,
) -> Result<Vec<InputStatePersistenceRecord>, DurableTailRecoveryError> {
    let (mut completions, mut outboxes) = match terminal_batch {
        Some(batch) => (batch.completions, batch.outboxes),
        None => (HashMap::new(), HashMap::new()),
    };
    let mut updates = Vec::with_capacity(attributed.len());
    for (mut bundle, row_digest) in attributed {
        bundle.seed.phase = InputLifecycleState::Consumed;
        bundle.seed.terminal_outcome = Some(crate::input_state::InputTerminalOutcome::Consumed);
        // Terminal seeds carry no recovery lane — the generated authority
        // refuses a Consumed seed that still claims one.
        bundle.seed.recovery_lane = None;
        bundle.seed.last_run_id = Some(candidate_run_id.clone());
        bundle.seed.last_boundary_sequence = Some(boundary_sequence);
        let input_id = bundle.state.input_id.clone();
        if let Some(completion) = completions.remove(&input_id) {
            bundle.state.terminal_completion = Some(completion);
        }
        if let Some(outbox) = outboxes.remove(&input_id) {
            bundle.state.interaction_terminal_outbox = Some(outbox);
        }
        // The same fenced recovery transaction installs the recovered
        // boundary receipt and this terminal row. Once attributed, ordinary
        // content is neither redeliverable nor needed to identify a future
        // durable tail. A row whose staged terminal is still pending keeps it
        // until its drain closes it, and a directed input without its
        // terminal outbox is kept fail-closed because its payload still owns
        // the Interaction identity.
        if crate::store::input_state_payload_is_retirable(&bundle) {
            bundle.state.persisted_input = None;
        }
        let record = InputStatePersistenceRecord::from_machine_snapshot(bundle)
            .map_err(DurableTailRecoveryError::Authority)?
            .with_expected_row_digest(row_digest);
        updates.push(record);
    }
    if !completions.is_empty() || !outboxes.is_empty() {
        return Err(DurableTailRecoveryError::Authority(
            "a recovered boundary's terminal batch names an input the boundary does not consume"
                .to_string(),
        ));
    }
    Ok(updates)
}

/// The terminal a recovered boundary stages for the contributors it consumes:
/// one run-scoped terminal-completion batch over all of them and, for each
/// directed contributor, a directed outbox row of that batch, every row keyed
/// by its input.
struct RecoveredBoundaryTerminalBatch {
    completions: HashMap<InputId, crate::input_state::InputTerminalCompletion>,
    outboxes: HashMap<InputId, crate::input_state::InteractionTerminalOutbox>,
}

/// The candidate a recovered boundary's terminal batch carries.
///
/// A completed tail answers its contributors `CompletedWithoutResult`: their
/// answer is the committed history, and no result payload is rebuilt. An
/// interrupted tail (including a continuable run a writer or an archive
/// repairs) answers them `Cancelled`, the class the live path gives a run
/// that ends before it completes. A disposition that commits nothing stages
/// nothing.
fn recovered_boundary_terminal_candidate(
    disposition: DurableTailRecoveryDisposition,
) -> Option<crate::input_state::InteractionTerminalCandidate> {
    match disposition {
        DurableTailRecoveryDisposition::CommitCompleted
        | DurableTailRecoveryDisposition::CommitCompletedRetainInputs => {
            Some(crate::input_state::InteractionTerminalCandidate::CompletedWithoutResult)
        }
        DurableTailRecoveryDisposition::RepairAndCommitInterrupted
        | DurableTailRecoveryDisposition::OperatorRepairAndCommitInterrupted => {
            Some(crate::input_state::InteractionTerminalCandidate::Cancelled)
        }
        DurableTailRecoveryDisposition::RefuseRecovery
        | DurableTailRecoveryDisposition::HoldIntact
        | DurableTailRecoveryDisposition::ContinueRun => None,
    }
}

/// Build the terminal batch a recovered boundary stages for `attributed`, the
/// rows it consumes, in its own fenced commit.
///
/// The batch is the one the live path stages for a run's contributors: key
/// `Run { candidate run }`, owner the first contributor in canonical order,
/// no session checkpoint (the boundary and its document commit in this same
/// transaction, so no later checkpoint exists). A directed contributor gets a
/// `Candidate` outbox naming `placement`, the runtime placement the row the
/// drive observed records (its complete binding, else its placement witness):
/// a later drain adopts it as it adopts any outbox an earlier incarnation
/// left.
///
/// `None` stages nothing, which leaves the contributors as every earlier
/// release left them (consumed, with no receipt and no requester terminal,
/// for a predecessor settlement to answer):
/// - nothing is consumed, or the disposition commits nothing;
/// - the contributors exceed one batch, or a row already carries terminal
///   work or is a migrated row without completion evidence;
/// - a directed contributor's payload does not validate;
/// - another run-scoped batch of the session has not finished (its
///   completion pending or its directed outbox unpublished): the generated
///   completion authority recovers one pending run-scoped batch at a time, so
///   a second one would block that batch's recovery;
/// - the store cannot say whether such a batch is pending.
///
/// Without a complete placement no directed outbox can be staged, so the
/// batch holds the nondirected contributors only, and each directed
/// contributor is left as before, keeping the payload that owns its
/// interaction. Its receipt is never finalized without its outbox: the drain
/// would compact the candidate a later outbox binds to, and its requester
/// could never be answered. With no nondirected contributor nothing is
/// staged.
async fn recovered_boundary_terminal_batch(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    candidate: &PreparedRecoveryCandidate,
    disposition: DurableTailRecoveryDisposition,
    attributed: &[(StoredInputState, String)],
    placement: Option<crate::input_state::InteractionTerminalPlacement>,
) -> Result<Option<RecoveredBoundaryTerminalBatch>, DurableTailRecoveryError> {
    if attributed.is_empty() {
        return Ok(None);
    }
    let Some(terminal_candidate) = recovered_boundary_terminal_candidate(disposition) else {
        return Ok(None);
    };
    let session_id = &candidate.session_id;
    let run_id = &candidate.candidate_run_id;
    if attributed.len() > crate::input_state::TERMINAL_COMPLETION_BATCH_MAX_INPUTS {
        tracing::warn!(
            %session_id,
            %run_id,
            contributors = attributed.len(),
            "a recovered boundary's contributors exceed one terminal batch; they keep no receipt"
        );
        return Ok(None);
    }
    let mut directed = Vec::new();
    let mut nondirected = Vec::new();
    for (bundle, _) in attributed {
        let input_id = &bundle.state.input_id;
        if bundle.state.terminal_completion.is_some()
            || bundle.state.interaction_terminal_outbox.is_some()
            || bundle.state.terminal_completion_unavailable
        {
            tracing::warn!(
                %session_id,
                %run_id,
                %input_id,
                "a recovered boundary's contributor already carries terminal work; the boundary \
                 stages no terminal batch"
            );
            return Ok(None);
        }
        let Some(input) = bundle.state.persisted_input.as_ref() else {
            nondirected.push(input_id.clone());
            continue;
        };
        match crate::input::validated_directed_interaction_id(input) {
            Ok(None) => nondirected.push(input_id.clone()),
            Ok(Some(interaction_id)) if interaction_id.0 == input_id.0 => {
                directed.push((input_id.clone(), interaction_id));
            }
            Ok(Some(_)) => {
                tracing::warn!(
                    %session_id,
                    %run_id,
                    %input_id,
                    "a recovered boundary's directed contributor names another interaction; the \
                     boundary stages no terminal batch"
                );
                return Ok(None);
            }
            Err(reason) => {
                tracing::warn!(
                    %session_id,
                    %run_id,
                    %input_id,
                    %reason,
                    "a recovered boundary's directed contributor does not validate; the boundary \
                     stages no terminal batch"
                );
                return Ok(None);
            }
        }
    }
    match run_scoped_terminal_batch_pending(store, runtime_id).await? {
        Some(false) => {}
        Some(true) => {
            tracing::info!(
                %session_id,
                %run_id,
                "another run's terminal batch is still pending; the recovered boundary stages \
                 none beside it"
            );
            return Ok(None);
        }
        None => {
            tracing::info!(
                %session_id,
                %run_id,
                "the store cannot report pending terminal batches; the recovered boundary stages \
                 none"
            );
            return Ok(None);
        }
    }
    let input_ids = match placement {
        Some(_) => attributed
            .iter()
            .map(|(bundle, _)| bundle.state.input_id.clone())
            .collect::<Vec<_>>(),
        None => {
            if !directed.is_empty() {
                tracing::info!(
                    %session_id,
                    %run_id,
                    directed = directed.len(),
                    "the session's lifecycle row records no runtime placement; the recovered \
                     boundary stages its nondirected contributors' receipts only and leaves its \
                     directed contributors without one"
                );
                directed.clear();
            }
            nondirected
        }
    };
    if input_ids.is_empty() {
        return Ok(None);
    }
    let rows = crate::meerkat_machine::driver::build_input_terminal_completion_rows(
        crate::meerkat_machine::driver::InteractionTerminalBatchScope::Run(run_id),
        &input_ids,
        terminal_candidate.clone(),
        false,
    )
    .map_err(|error| {
        DurableTailRecoveryError::Authority(format!(
            "a recovered boundary's terminal batch could not be built: {error}"
        ))
    })?;
    let owner = rows.first().ok_or_else(|| {
        DurableTailRecoveryError::Authority(
            "a recovered boundary's terminal batch lost its owner row".to_string(),
        )
    })?;
    let candidate_digest = owner.candidate_digest.clone();
    let completion_input_ids_digest = owner.completion_input_ids_digest.clone();
    let mut outboxes = HashMap::new();
    if let (Some(placement), false) = (placement.as_ref(), directed.is_empty()) {
        directed.sort_by_key(|(input_id, _)| input_id.0);
        let directed_rows = recovered_boundary_directed_outboxes(
            session_id,
            run_id,
            &directed,
            placement,
            &candidate_digest,
            &completion_input_ids_digest,
            &terminal_candidate,
        )?;
        for outbox in directed_rows {
            outboxes.insert(outbox.input_id.clone(), outbox);
        }
    }
    Ok(Some(RecoveredBoundaryTerminalBatch {
        completions: rows
            .into_iter()
            .map(|completion| (completion.input_id.clone(), completion))
            .collect(),
        outboxes,
    }))
}

/// The `Candidate` outbox rows of a recovered boundary's directed
/// contributors (`directed`, in canonical order): ordinal zero owns the
/// batch's candidate, every row names `placement` and binds back to the
/// completion batch by its two digests, as the live path's rows do.
fn recovered_boundary_directed_outboxes(
    session_id: &SessionId,
    run_id: &RunId,
    directed: &[(InputId, meerkat_core::interaction::InteractionId)],
    placement: &crate::input_state::InteractionTerminalPlacement,
    candidate_digest: &str,
    completion_input_ids_digest: &str,
    terminal_candidate: &crate::input_state::InteractionTerminalCandidate,
) -> Result<Vec<crate::input_state::InteractionTerminalOutbox>, DurableTailRecoveryError> {
    let candidate_owner_input_id = directed
        .first()
        .map(|(input_id, _)| input_id.clone())
        .ok_or_else(|| {
            DurableTailRecoveryError::Authority(
                "a recovered boundary's directed batch lost its owner".to_string(),
            )
        })?;
    let batch_key = crate::input_state::InteractionTerminalBatchKey::Run {
        run_id: run_id.clone(),
    };
    let outboxes = directed
        .iter()
        .enumerate()
        .map(|(ordinal, (input_id, interaction_id))| {
            let batch_ordinal = u16::try_from(ordinal).map_err(|_| {
                DurableTailRecoveryError::Authority(
                    "a recovered boundary's directed batch exceeds one terminal batch".to_string(),
                )
            })?;
            Ok(crate::input_state::InteractionTerminalOutbox {
                interaction_id: *interaction_id,
                input_id: input_id.clone(),
                batch_ordinal,
                batch_key: batch_key.clone(),
                owner_session_id: session_id.clone(),
                owner_agent_runtime_id: Some(placement.agent_runtime_id.clone()),
                owner_fence_token: Some(placement.fence_token),
                owner_runtime_generation: Some(placement.runtime_generation),
                owner_runtime_epoch_id: placement.runtime_epoch_id.clone(),
                candidate_owner_input_id: candidate_owner_input_id.clone(),
                released_0831_candidate: None,
                candidate_digest: candidate_digest.to_string(),
                released_0831_completion_input_ids: None,
                completion_input_ids_digest: completion_input_ids_digest.to_string(),
                phase: crate::input_state::InteractionTerminalOutboxPhase::Candidate,
            })
        })
        .collect::<Result<Vec<_>, DurableTailRecoveryError>>()?;
    crate::input_state::validate_unpublished_interaction_terminal_outbox_batch(&outboxes)
        .map_err(DurableTailRecoveryError::Authority)?;
    crate::input_state::validate_interaction_terminal_outbox_batch_against_candidate(
        &outboxes,
        terminal_candidate,
    )
    .map_err(DurableTailRecoveryError::Authority)?;
    Ok(outboxes)
}

/// Whether the session holds a run-scoped terminal batch that has not
/// finished: a pending completion, or an unpublished directed outbox, of any
/// run. Read from the store's pending-owner index, after the recovery's input
/// observation, so a batch staged after that observation fails the
/// recovery's input-set fence instead. `None` when the store cannot answer.
async fn run_scoped_terminal_batch_pending(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Option<bool>, DurableTailRecoveryError> {
    let limit = crate::store::MAX_PENDING_TERMINAL_OWNER_PAGE;
    let mut after: Option<InputId> = None;
    loop {
        let page = match store
            .load_pending_terminal_owner_ids_page(runtime_id, after.as_ref(), limit)
            .await
        {
            Ok(page) => page,
            Err(RuntimeStoreError::Unsupported(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if page.is_empty() {
            return Ok(Some(false));
        }
        let owners = match store.load_input_states_by_ids(runtime_id, &page).await {
            Ok(owners) => owners,
            Err(RuntimeStoreError::Unsupported(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        for owner in owners.into_iter().flatten() {
            let completion_pending =
                owner
                    .state
                    .terminal_completion
                    .as_ref()
                    .is_some_and(|completion| {
                        completion.batch_key.run_id().is_some()
                            && matches!(
                                completion.phase,
                                crate::input_state::InputTerminalCompletionPhase::Pending
                            )
                    });
            let outbox_unpublished = owner
                .state
                .interaction_terminal_outbox
                .as_ref()
                .is_some_and(|outbox| {
                    outbox.batch_key.run_id().is_some()
                        && !matches!(
                            outbox.phase,
                            crate::input_state::InteractionTerminalOutboxPhase::Published { .. }
                        )
                });
            if completion_pending || outbox_unpublished {
                return Ok(Some(true));
            }
        }
        if page.len() < limit {
            return Ok(Some(false));
        }
        after = page.last().cloned();
    }
}

/// Operator pass: the inputs of another run stranded beside an interrupted
/// tail, when they are the only reason ordinary recovery holds it.
///
/// Every row is read again from a snapshot at the same input-set revision the
/// observation read (a moved set answers `None`). The tail must carry a run
/// cursor that agrees with it, whose bound inputs are exactly the inputs
/// whose content the tail holds. Each non-terminal row is then judged as the
/// observation attributes it ([`observe_candidate_run_inputs`]):
/// - a row the cursor binds is the tail's own, whatever run it names now (a
///   join the cancel requeued and a later run staged again, for one): the
///   observation attributed it, so the repair commit consumes it with the
///   tail, once, and it is never requeued;
/// - a content row that names no run is not in the tail: it stays queued for
///   ordinary delivery;
/// - a content row that names another run, and is Staged or Queued, is
///   stranded, when that run has no boundary receipt in the store. The store
///   keeps one provisional tail per session, the candidate's, so such a run
///   has no tail either: it never committed.
///
/// Any other non-terminal row answers `None`: one naming the candidate run
/// that the cursor does not bind, one a receipt binds, and a content row in
/// another phase. That binding is the proof that an input is not the tail's
/// own: nothing is inferred from order or timing. Rows without content
/// (operations, continuations, external events) are left as they are, as
/// ordinary recovery leaves them. `None` leaves the tail to today's verdict.
async fn observe_stranded_run_inputs(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    candidate: &PreparedRecoveryCandidate,
    receipt_bound_inputs: &BTreeSet<String>,
    observed_revision: Option<RecoveryInputSetRevision>,
) -> Result<Option<Vec<(StoredInputState, String)>>, DurableTailRecoveryError> {
    let snapshot = match store.load_input_states_with_versions(runtime_id).await {
        Ok(snapshot) => snapshot,
        Err(RuntimeStoreError::Unsupported(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if snapshot.runtime_id() != runtime_id {
        return Err(DurableTailRecoveryError::Authority(format!(
            "store returned a recovery input snapshot for {}, not {runtime_id}",
            snapshot.runtime_id()
        )));
    }
    let (rows, revision, _token) = snapshot.into_parts();
    if observed_revision != Some(revision) {
        return Ok(None);
    }
    let Some(cursor_bound_inputs) = candidate.cursor_bound_inputs.as_deref() else {
        return Ok(None);
    };
    let mut stranded = Vec::new();
    let mut stranding_runs: BTreeMap<String, RunId> = BTreeMap::new();
    for (bundle, row_digest) in rows {
        if is_terminal(bundle.seed.phase) {
            continue;
        }
        // The tail's own content: attributed, so the repair consumes it.
        if cursor_bound_inputs.contains(&bundle.state.input_id) {
            continue;
        }
        if bundle.seed.last_run_id.as_ref() == Some(&candidate.candidate_run_id)
            || receipt_bound_inputs.contains(&bundle.state.input_id.to_string())
        {
            return Ok(None);
        }
        if !carries_redeliverable_content(bundle.state.persisted_input.as_ref()) {
            continue;
        }
        // Not in the tail, and no run's: it stays queued.
        let Some(run_id) = bundle.seed.last_run_id.clone() else {
            continue;
        };
        if !matches!(
            bundle.seed.phase,
            InputLifecycleState::Staged | InputLifecycleState::Queued
        ) {
            return Ok(None);
        }
        stranding_runs.insert(run_id.to_string(), run_id);
        stranded.push((bundle, row_digest));
    }
    if stranded.is_empty() {
        return Ok(None);
    }
    for run_id in stranding_runs.values() {
        match store
            .load_durable_tail_recovery_receipts(runtime_id, run_id)
            .await
        {
            Ok(receipts) if receipts.is_empty() => {}
            Ok(_)
            | Err(
                RuntimeStoreError::Unsupported(_)
                | RuntimeStoreError::PreparedRecoveryRequiresAtomicPhysicalHeadCas { .. },
            ) => return Ok(None),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(Some(stranded))
}

/// Realize pass of an operator's stranded-input repair: each input goes back
/// to its lane with its content, as a staged rollback puts it back (the run
/// association ends, the attempt count and lane stay), fenced on the exact row
/// bytes the observation read.
fn requeue_stranded_inputs(
    rows: Vec<(StoredInputState, String)>,
) -> Result<Vec<InputStatePersistenceRecord>, DurableTailRecoveryError> {
    let now = chrono::Utc::now();
    let mut updates = Vec::with_capacity(rows.len());
    for (mut bundle, row_digest) in rows {
        let from = bundle.seed.phase;
        bundle.seed.phase = InputLifecycleState::Queued;
        bundle.seed.last_run_id = None;
        if from != InputLifecycleState::Queued {
            bundle
                .state
                .history
                .push(crate::input_state::InputStateHistoryEntry {
                    timestamp: now,
                    from,
                    to: InputLifecycleState::Queued,
                    reason: Some("ResolveHeldTailRequeue".to_string()),
                });
        }
        bundle.state.updated_at = now;
        let record = InputStatePersistenceRecord::from_machine_snapshot(bundle)
            .map_err(DurableTailRecoveryError::Authority)?
            .with_expected_row_digest(row_digest);
        updates.push(record);
    }
    Ok(updates)
}

#[cfg(test)]
#[path = "recovery_boundary_batch_tests.rs"]
mod recovery_boundary_batch_tests;

#[cfg(test)]
mod store_authority_tests {
    use super::*;
    use crate::store::{
        CommittedWholeBlobProvisionalTail, CommittedWholeBlobSnapshot, WholeBlobStoreAuthority,
    };
    use meerkat_core::WholeBlobProvisionalTailAuthority;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, TranscriptMessageIdentity, UserMessage,
        message_timestamp_now,
    };

    #[test]
    fn nonterminal_transcript_snapshot_is_not_a_recovered_turn_end() {
        let mut session = Session::new();
        session.push(Message::BlockAssistant(BlockAssistantMessage::snapshot(
            vec![AssistantBlock::Transcript {
                text: "observed speech".into(),
                source: meerkat_core::TranscriptSource::SpokenUnmeasured,
                meta: None,
            }],
        )));
        session.push(Message::User(UserMessage::text("continue")));
        let observed = observe_durable_tail(0, &session);
        assert_eq!(observed.terminal_stop_reason, DurableTailStopReason::Absent);
        assert!(!observed.messages_after_terminal);
    }

    fn observation(run_id: &RunId) -> DurableTailObservation {
        DurableTailObservation {
            tail_run_id: Some(run_id.clone()),
            run_id_cardinality: RunIdCardinality::SingleRunId,
            terminal_stop_reason: DurableTailStopReason::EndTurn,
            dangling_tool_use_ids: Vec::new(),
            orphan_tool_result_count: 0,
            messages_after_terminal: false,
            // A tail written without a run cursor, as observe_durable_tail
            // records it before the cursor is read: the candidate id binds
            // nothing for it, so these rows keep their pre-cursor identity.
            run_cursor: DurableTailRunCursor::Absent,
            cursor_inputs_open: false,
        }
    }

    #[test]
    fn durable_tail_lifecycle_reassertion_preserves_live_bridge_recovery_image() {
        let mut state = crate::meerkat_machine::dsl::MeerkatMachineState::default();
        let operation_id = crate::meerkat_machine::dsl::OperationId(
            "\"00000000-0000-0000-0000-000000000001\"".to_string(),
        );
        let channel_id = "live:durable-tail-reassert".to_string();
        state
            .live_bridge_operation_by_channel
            .insert(channel_id.clone(), operation_id.clone());
        state
            .live_bridge_channel_by_operation
            .insert(operation_id.clone(), channel_id);
        state.live_bridge_interaction_by_operation.insert(
            operation_id.clone(),
            "00000000-0000-0000-0000-000000000002".to_string(),
        );
        state.live_bridge_provider_turn_by_operation.insert(
            operation_id.clone(),
            "provider-turn:durable-tail".to_string(),
        );
        state.live_bridge_provider_delegation_by_operation.insert(
            operation_id.clone(),
            "provider-delegation:durable-tail".to_string(),
        );
        state.live_bridge_provider_call_by_operation.insert(
            operation_id.clone(),
            "provider-call:durable-tail".to_string(),
        );
        state.live_bridge_agent_identity_by_operation.insert(
            operation_id.clone(),
            crate::meerkat_machine::dsl::AgentIdentity("source:durable-tail".to_string()),
        );
        state.live_bridge_context_revision_by_operation.insert(
            operation_id.clone(),
            "sha256:durable-tail-context".to_string(),
        );
        state.live_bridge_request_digest_by_operation.insert(
            operation_id.clone(),
            "sha256:durable-tail-request".to_string(),
        );
        state.live_bridge_phase_by_operation.insert(
            operation_id.clone(),
            crate::meerkat_machine::dsl::LiveBridgeOperationPhase::ExecutionRunning,
        );
        state
            .live_bridge_execution_started_operations
            .insert(operation_id.clone());
        state
            .live_bridge_outcome_receipt_required_operations
            .insert(operation_id);
        let live_bridge_recovery = crate::live_execution::LiveBridgeRecoveryImage::capture(&state)
            .expect("capture one durable live bridge operation");
        let seed = MachineLifecycleCommit::new_with_binding_run_unregister_progress_and_live_bridge(
            RuntimeState::Idle,
            MachineLifecycleBindingFacts::default(),
            MachineLifecycleRunFacts::default(),
            SupervisorAuthoritySnapshot::UnboundNoReceipt,
            None,
            live_bridge_recovery.clone(),
        );
        let encoded = seed
            .store_record()
            .encode()
            .expect("encode V5 lifecycle image");
        let observed = observe_persisted_lifecycle(
            MachineLifecycleObservation::from_raw_record(&encoded),
            &RunId::new(),
            DriveMode::Recovery,
        );
        let reassertion = durable_tail_lifecycle_reassertion(&observed);
        assert_eq!(
            reassertion.snapshot().live_bridge_recovery(),
            &live_bridge_recovery,
            "durable-tail lifecycle reassertion must not erase V5 bridge truth"
        );
    }

    /// The archive of a session that is ending reads every quiescent
    /// persisted runtime (a missing row, `Idle`, `Stopped`, `Retired`) as
    /// `Retired` and re-asserts the observed phase, so the machine never
    /// continues a run there. Ordinary recovery reads each phase as
    /// persisted, `Stopped` as not quiescent; the operator verb reads a clean
    /// `Stopped` row as the `Idle` row a registration normalizes it to. A
    /// running runtime is not quiescent for any drive. RED before: an
    /// archive's recovery read an `Idle` runtime as `Idle`, so a continuable
    /// tail answered `Continue` inside the archive. Unrun: written while
    /// Cargo is not run.
    #[test]
    fn a_session_archive_reads_every_quiescent_persisted_runtime_as_retired() {
        use crate::meerkat_machine::dsl::DurableRecoveryObservedLifecycle as Observed;

        fn persisted(state: RuntimeState) -> MachineLifecycleObservation {
            let encoded =
                MachineLifecycleCommit::new_with_binding_run_unregister_progress_and_live_bridge(
                    state,
                    MachineLifecycleBindingFacts::default(),
                    MachineLifecycleRunFacts::default(),
                    SupervisorAuthoritySnapshot::UnboundNoReceipt,
                    None,
                    crate::live_execution::LiveBridgeRecoveryImage::default(),
                )
                .store_record()
                .encode()
                .expect("encode a lifecycle image");
            MachineLifecycleObservation::from_raw_record(&encoded)
        }

        let run_id = RunId::new();
        for (state, as_persisted, for_operator) in [
            (RuntimeState::Idle, Observed::Idle, Observed::Idle),
            (
                RuntimeState::Stopped,
                Observed::NonQuiescent,
                Observed::Idle,
            ),
            (RuntimeState::Retired, Observed::Retired, Observed::Retired),
        ] {
            let archive =
                observe_persisted_lifecycle(persisted(state), &run_id, DriveMode::SessionArchive);
            assert_eq!(archive.lifecycle, Observed::Retired, "{state}");
            assert_eq!(archive.reassert_state, state, "{state}");
            assert_eq!(
                archive.current_run,
                crate::meerkat_machine::dsl::DurableRecoveryObservedRun::NoRun,
                "{state}"
            );
            assert!(
                matches!(
                    archive.expected_version,
                    MachineLifecycleExpectedVersion::Version(_)
                ),
                "{state}"
            );
            assert_eq!(
                observe_persisted_lifecycle(persisted(state), &run_id, DriveMode::Recovery)
                    .lifecycle,
                as_persisted,
                "{state}"
            );
            assert_eq!(
                observe_persisted_lifecycle(
                    persisted(state),
                    &run_id,
                    DriveMode::Operator { apply: true },
                )
                .lifecycle,
                for_operator,
                "{state}"
            );
        }

        let archive = observe_persisted_lifecycle(
            MachineLifecycleObservation::Missing,
            &run_id,
            DriveMode::SessionArchive,
        );
        assert_eq!(archive.lifecycle, Observed::Retired);
        assert_eq!(archive.reassert_state, RuntimeState::Idle);
        assert!(matches!(
            archive.expected_version,
            MachineLifecycleExpectedVersion::Missing
        ));
        assert_eq!(
            observe_persisted_lifecycle(
                MachineLifecycleObservation::Missing,
                &run_id,
                DriveMode::Recovery,
            )
            .lifecycle,
            Observed::MissingRow
        );

        for mode in [
            DriveMode::Recovery,
            DriveMode::SessionArchive,
            DriveMode::Operator { apply: true },
        ] {
            assert_eq!(
                observe_persisted_lifecycle(persisted(RuntimeState::Running), &run_id, mode)
                    .lifecycle,
                Observed::NonQuiescent,
                "{mode:?}"
            );
        }
    }

    /// The operator verb runs under an exclusive open and never registers
    /// the runtime, so it reads a `Stopped` row a host left when it ended
    /// before unregistering (no current run, no unregister in progress) as
    /// the clean `Idle` row a start's registration replaces it with: Idle
    /// re-asserted, unbound, no run facts, the supervisor and live-bridge
    /// facts kept, fenced on the observed row version. A `Stopped` row with
    /// a current run, or with an unregister in progress, stays not
    /// quiescent, and ordinary recovery never normalizes. RED before: the
    /// operator read every `Stopped` row as not quiescent, so the verb
    /// refused a session whose host ended between its stop and its
    /// unregister. Unrun: written while Cargo is not run.
    #[test]
    fn an_operator_reads_a_clean_stopped_row_as_the_idle_row_registration_writes() {
        use crate::meerkat_machine::dsl::DurableRecoveryObservedLifecycle as Observed;

        fn stopped(
            run: MachineLifecycleRunFacts,
            unregister_progress: Option<crate::store::MachineUnregisterProgressSnapshot>,
        ) -> MachineLifecycleObservation {
            let encoded =
                MachineLifecycleCommit::new_with_binding_run_unregister_progress_and_live_bridge(
                    RuntimeState::Stopped,
                    MachineLifecycleBindingFacts::new(
                        Some("agent-runtime".to_string()),
                        Some(7),
                        Some(2),
                        Some("epoch-of-the-ended-process".to_string()),
                    ),
                    run,
                    SupervisorAuthoritySnapshot::UnboundNoReceipt,
                    unregister_progress,
                    crate::live_execution::LiveBridgeRecoveryImage::default(),
                )
                .store_record()
                .encode()
                .expect("encode a stopped lifecycle image");
            MachineLifecycleObservation::from_raw_record(&encoded)
        }

        let run_id = RunId::new();
        let operator = DriveMode::Operator { apply: false };
        let clean = observe_persisted_lifecycle(
            stopped(MachineLifecycleRunFacts::default(), None),
            &run_id,
            operator,
        );
        assert_eq!(clean.lifecycle, Observed::Idle);
        assert!(clean.normalizes_stopped);
        assert_eq!(clean.reassert_state, RuntimeState::Idle);
        assert_eq!(clean.binding, MachineLifecycleBindingFacts::default());
        assert_eq!(
            clean.current_run,
            crate::meerkat_machine::dsl::DurableRecoveryObservedRun::NoRun
        );
        assert!(matches!(
            clean.expected_version,
            MachineLifecycleExpectedVersion::Version(_)
        ));
        let reassertion = durable_tail_lifecycle_reassertion(&clean);
        assert_eq!(reassertion.snapshot().runtime_state(), RuntimeState::Idle);
        assert_eq!(
            reassertion.snapshot().binding(),
            &MachineLifecycleBindingFacts::default()
        );
        assert!(reassertion.snapshot().run().current_run_id().is_none());
        assert!(reassertion.expected_version().is_some());

        let with_run = observe_persisted_lifecycle(
            stopped(
                MachineLifecycleRunFacts::new(Some(run_id.clone()), None),
                None,
            ),
            &run_id,
            operator,
        );
        assert_eq!(with_run.lifecycle, Observed::NonQuiescent);
        assert!(!with_run.normalizes_stopped);

        let unregistering = observe_persisted_lifecycle(
            stopped(
                MachineLifecycleRunFacts::default(),
                Some(crate::store::MachineUnregisterProgressSnapshot::new(
                    true, true, true, false, false,
                )),
            ),
            &run_id,
            operator,
        );
        assert_eq!(unregistering.lifecycle, Observed::NonQuiescent);
        assert!(!unregistering.normalizes_stopped);

        let ordinary = observe_persisted_lifecycle(
            stopped(MachineLifecycleRunFacts::default(), None),
            &run_id,
            DriveMode::Recovery,
        );
        assert_eq!(ordinary.lifecycle, Observed::NonQuiescent);
        assert!(!ordinary.normalizes_stopped);
    }

    #[test]
    fn whole_blob_candidate_identity_binds_exact_store_base_and_run() {
        let session_id = SessionId::new();
        let first_run = RunId::new();
        let second_run = RunId::new();
        let first = exact_whole_blob_candidate_id(
            &session_id,
            7,
            "row-sha256:base",
            "row-sha256:candidate",
            1,
            &first_run,
            &observation(&first_run),
        );
        let different_run = exact_whole_blob_candidate_id(
            &session_id,
            7,
            "row-sha256:base",
            "row-sha256:candidate",
            1,
            &second_run,
            &observation(&second_run),
        );
        let different_base = exact_whole_blob_candidate_id(
            &session_id,
            8,
            "row-sha256:other-base",
            "row-sha256:candidate",
            1,
            &first_run,
            &observation(&first_run),
        );
        let different_sequence = exact_whole_blob_candidate_id(
            &session_id,
            7,
            "row-sha256:base",
            "row-sha256:candidate",
            2,
            &first_run,
            &observation(&first_run),
        );
        assert_ne!(first, different_run);
        assert_ne!(first, different_base);
        assert_ne!(first, different_sequence);
    }

    fn whole_blob_recovery_fixture(
        authority_run: RunId,
        transcript_run: RunId,
    ) -> (
        CommittedWholeBlobSnapshot,
        CommittedWholeBlobProvisionalTail,
    ) {
        let mut committed = Session::new();
        committed.push(Message::User(UserMessage::text("committed input")));
        let session_id = committed.id().clone();
        let committed_bytes = Arc::new(committed.to_persisted_bytes().unwrap());
        let committed_sha = format!("row-sha256:{:x}", Sha256::digest(committed_bytes.as_ref()));
        let committed_authority =
            WholeBlobStoreAuthority::issued(session_id.clone(), 7, committed_sha.clone()).unwrap();

        let mut candidate = committed;
        candidate.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "durable reply".to_string(),
                meta: None,
            }],
            stop_reason: Some(StopReason::EndTurn),
            identity: TranscriptMessageIdentity::default().with_run_id(transcript_run),
            created_at: message_timestamp_now(),
            assistant_message_id: None,
        }));
        let candidate_bytes = Arc::new(candidate.to_persisted_bytes().unwrap());
        let candidate_sha = format!("row-sha256:{:x}", Sha256::digest(candidate_bytes.as_ref()));
        let provisional_authority = WholeBlobProvisionalTailAuthority::issued(
            session_id,
            7,
            committed_sha,
            authority_run,
            candidate_sha,
            1,
        )
        .unwrap();
        (
            CommittedWholeBlobSnapshot::new(committed_bytes, committed_authority).unwrap(),
            CommittedWholeBlobProvisionalTail::new(provisional_authority, candidate_bytes),
        )
    }

    #[test]
    fn whole_blob_provisional_recovery_uses_store_authority_without_session_stamp() {
        let run_id = RunId::new();
        let (committed, provisional) = whole_blob_recovery_fixture(run_id.clone(), run_id.clone());

        let RecoveryCandidatePreparation::Prepared(candidate) =
            prepare_whole_blob_recovery_candidate(committed, Some(provisional), &HashMap::new())
                .expect("store-issued WholeBlob tail prepares")
        else {
            panic!("valid WholeBlob provisional tail must prepare");
        };
        assert_eq!(candidate.candidate_run_id, run_id);
        assert_eq!(
            candidate.class,
            DurableTailRecoveryClass::CompletedCandidate
        );
        assert!(
            candidate.document.is_none(),
            "completed WholeBlob recovery is metadata-only and must not materialize a successor"
        );
        assert!(matches!(
            candidate.store_transition,
            PreparedRecoveryStoreTransition::WholeBlob {
                base_store_revision: 7,
                ..
            }
        ));
    }

    #[test]
    fn whole_blob_provisional_recovery_refuses_wrong_run_authority() {
        let (committed, provisional) = whole_blob_recovery_fixture(RunId::new(), RunId::new());

        assert!(matches!(
            prepare_whole_blob_recovery_candidate(committed, Some(provisional), &HashMap::new()),
            Err(DurableTailRecoveryError::InvalidEvidence(detail))
                if detail.contains("store-issued run identity")
        ));
    }

    // Run-cursor rows.

    /// A committed WholeBlob document, and a provisional candidate that is
    /// that document plus `rows`, carrying `cursor` when one is given.
    fn whole_blob_cursor_fixture(
        authority_run: RunId,
        rows: Vec<Message>,
        cursor: Option<meerkat_core::SessionRunCursor>,
    ) -> (
        CommittedWholeBlobSnapshot,
        CommittedWholeBlobProvisionalTail,
    ) {
        let mut committed = Session::new();
        committed.push(Message::User(UserMessage::text("committed input")));
        let session_id = committed.id().clone();
        let committed_bytes = Arc::new(committed.to_persisted_bytes().unwrap());
        let committed_sha = format!("row-sha256:{:x}", Sha256::digest(committed_bytes.as_ref()));
        let committed_authority =
            WholeBlobStoreAuthority::issued(session_id.clone(), 7, committed_sha.clone()).unwrap();

        let mut candidate = committed;
        for row in rows {
            candidate.push(row);
        }
        if let Some(cursor) = cursor.as_ref() {
            candidate.set_run_cursor(cursor).unwrap();
        }
        let candidate_bytes = Arc::new(candidate.to_persisted_bytes().unwrap());
        let candidate_sha = format!("row-sha256:{:x}", Sha256::digest(candidate_bytes.as_ref()));
        let provisional_authority = WholeBlobProvisionalTailAuthority::issued(
            session_id,
            7,
            committed_sha,
            authority_run,
            candidate_sha,
            1,
        )
        .unwrap();
        (
            CommittedWholeBlobSnapshot::new(committed_bytes, committed_authority).unwrap(),
            CommittedWholeBlobProvisionalTail::new(provisional_authority, candidate_bytes),
        )
    }

    fn assistant_message_id(uuid: &str) -> meerkat_core::types::AssistantMessageId {
        serde_json::from_value(serde_json::Value::String(uuid.to_string())).unwrap()
    }

    fn tool_call_message(run_id: &RunId, call_id: &str, message_id: &str) -> BlockAssistantMessage {
        BlockAssistantMessage {
            blocks: vec![AssistantBlock::ToolUse {
                id: call_id.to_string(),
                name: "search".to_string(),
                args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                meta: None,
            }],
            stop_reason: Some(StopReason::ToolUse),
            identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
            created_at: message_timestamp_now(),
            assistant_message_id: Some(assistant_message_id(message_id)),
        }
    }

    fn staged_tool_call_message(run_id: &RunId) -> BlockAssistantMessage {
        tool_call_message(
            run_id,
            "call-staged",
            "0190b5f0-0000-7000-8000-000000000002",
        )
    }

    fn cursor(
        run_id: &RunId,
        bound_input_ids: Vec<InputId>,
        resume_point: RunResumePoint,
    ) -> meerkat_core::SessionRunCursor {
        meerkat_core::SessionRunCursor::new(run_id.clone(), bound_input_ids, 0, resume_point)
    }

    fn bound(run_id: &RunId, input_ids: &[InputId]) -> HashMap<InputId, RunId> {
        input_ids
            .iter()
            .map(|input_id| (input_id.clone(), run_id.clone()))
            .collect()
    }

    /// Round zero's first commit with no new row: the staged message lives
    /// only in the cursor, the store binding attributes the tail, and the
    /// run continues from the provisional tail itself.
    #[test]
    fn metadata_only_tools_pending_tail_continues() {
        let run_id = RunId::new();
        let input_id = InputId::new();
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            Vec::new(),
            Some(cursor(
                &run_id,
                vec![input_id.clone()],
                RunResumePoint::ToolsPending {
                    assistant_message: staged_tool_call_message(&run_id),
                },
            )),
        );
        let RecoveryCandidatePreparation::Prepared(candidate) =
            prepare_whole_blob_recovery_candidate(
                committed,
                Some(provisional),
                &bound(&run_id, &[input_id]),
            )
            .expect("a metadata-only cursor tail prepares")
        else {
            panic!("a metadata-only cursor tail must prepare a candidate");
        };
        assert_eq!(
            candidate.class,
            DurableTailRecoveryClass::ContinuableCandidate
        );
        let continuation = candidate
            .continuation
            .as_ref()
            .expect("a continuable candidate carries its continuation");
        assert!(continuation.candidate.try_run_cursor().unwrap().is_some());
        assert!(
            candidate.recovered.try_run_cursor().unwrap().is_none(),
            "the interrupted repair a retired runtime would commit carries no cursor"
        );
        assert_staged_calls_closed(&candidate.recovered, &staged_tool_call_message(&run_id));
    }

    /// The interrupted document of a ToolsPending tail ends with the staged
    /// message, one outcome-unknown error result per staged call, and the
    /// interrupted notice.
    fn assert_staged_calls_closed(recovered: &Session, staged: &BlockAssistantMessage) {
        let messages = recovered.messages();
        let [
            ..,
            Message::BlockAssistant(written),
            Message::ToolResults { results, .. },
            Message::SystemNotice(_),
        ] = messages
        else {
            panic!("the staged message, its results and the notice end the document");
        };
        assert_eq!(
            written.assistant_message_id, staged.assistant_message_id,
            "the staged message is written as the model returned it"
        );
        assert_eq!(written.blocks.len(), staged.blocks.len());
        let staged_ids: Vec<&str> = staged.tool_calls().map(|call| call.id).collect();
        let answered_ids: Vec<&str> = results
            .iter()
            .map(|result| result.tool_use_id.as_str())
            .collect();
        assert_eq!(
            answered_ids, staged_ids,
            "one result per staged call, in order"
        );
        for result in results {
            assert!(result.is_error);
            let payload: serde_json::Value =
                serde_json::from_str(&result.text_content()).expect("a typed tool error payload");
            assert_eq!(payload["error"], "outcome_uncertain");
        }
    }

    /// The operator's preparation of an ambiguous tail answers a trailing
    /// dangling call in place, after its assistant message.
    #[test]
    fn operator_prepares_a_trailing_dangling_call_as_outcome_unknown() {
        let run_id = RunId::new();
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            vec![Message::BlockAssistant(tool_call_message(
                &run_id,
                "call-dangling",
                "0190b5f0-0000-7000-8000-000000000011",
            ))],
            None,
        );
        let RecoveryCandidatePreparation::Prepared(candidate) =
            prepare_whole_blob_recovery_candidate_in(
                committed,
                Some(provisional),
                &HashMap::new(),
                PreparationMode::Operator,
            )
            .expect("an operator prepares an ambiguous tail")
        else {
            panic!("an operator's preparation must prepare the ambiguous candidate");
        };
        assert_eq!(candidate.class, DurableTailRecoveryClass::Ambiguous);
        assert_eq!(candidate.closed_call_ids, vec!["call-dangling".to_string()]);
        let [
            ..,
            Message::BlockAssistant(_),
            Message::ToolResults { results, .. },
            Message::SystemNotice(notice),
        ] = candidate.recovered.messages()
        else {
            panic!("the call's result follows its call, then the operator's notice");
        };
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_use_id, "call-dangling");
        assert!(notice.body.as_deref().is_some_and(
            |body| body.contains("resolve-held-tail") && body.contains(&candidate.candidate_id)
        ));
    }

    /// A dangling call with another row after it cannot be answered in
    /// place: the operator's preparation refuses it. Ordinary recovery holds
    /// the same tail.
    #[test]
    fn operator_refuses_a_dangling_call_that_is_not_trailing() {
        let run_id = RunId::new();
        let rows = || {
            vec![
                Message::BlockAssistant(tool_call_message(
                    &run_id,
                    "call-dangling",
                    "0190b5f0-0000-7000-8000-000000000012",
                )),
                Message::User(UserMessage::text("a later row")),
            ]
        };
        let (committed, provisional) = whole_blob_cursor_fixture(run_id.clone(), rows(), None);
        assert!(matches!(
            prepare_whole_blob_recovery_candidate(committed, Some(provisional), &HashMap::new()),
            Ok(RecoveryCandidatePreparation::Held)
        ));
        let (committed, provisional) = whole_blob_cursor_fixture(run_id.clone(), rows(), None);
        assert!(matches!(
            prepare_whole_blob_recovery_candidate_in(
                committed,
                Some(provisional),
                &HashMap::new(),
                PreparationMode::Operator,
            ),
            Err(DurableTailRecoveryError::InvalidEvidence(detail))
                if detail.contains("resolve-held-tail")
        ));
    }

    /// Providers repeat call ids across responses (Gemini numbers each from
    /// `fc_0`). A dangling call of an earlier message is not the last
    /// message's, even when the ids match: the operator refuses instead of
    /// answering the last message's call a second time.
    #[test]
    fn operator_pairs_dangling_calls_by_position_not_by_id() {
        let run_id = RunId::new();
        let two_calls = |message_id: &str| BlockAssistantMessage {
            blocks: ["fc_0", "fc_1"]
                .into_iter()
                .map(|id| AssistantBlock::ToolUse {
                    id: id.to_string(),
                    name: "search".to_string(),
                    args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                    meta: None,
                })
                .collect(),
            stop_reason: Some(StopReason::ToolUse),
            identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
            created_at: message_timestamp_now(),
            assistant_message_id: Some(assistant_message_id(message_id)),
        };
        let results = |ids: &[&str]| Message::ToolResults {
            results: ids
                .iter()
                .map(|id| {
                    meerkat_core::types::ToolResult::new((*id).to_string(), "ok".to_string(), false)
                })
                .collect(),
            created_at: message_timestamp_now(),
        };
        // A1[fc_0, fc_1] R[fc_0] A2[fc_0, fc_1] R[fc_0, fc_1]: A1's fc_1 is open.
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            vec![
                Message::BlockAssistant(two_calls("0190b5f0-0000-7000-8000-000000000021")),
                results(&["fc_0"]),
                Message::BlockAssistant(two_calls("0190b5f0-0000-7000-8000-000000000022")),
                results(&["fc_0", "fc_1"]),
            ],
            None,
        );
        assert!(matches!(
            prepare_whole_blob_recovery_candidate_in(
                committed,
                Some(provisional),
                &HashMap::new(),
                PreparationMode::Operator,
            ),
            Err(DurableTailRecoveryError::InvalidEvidence(detail))
                if detail.contains("resolve-held-tail")
        ));
    }

    /// A ToolsPending tail whose inputs are no longer bound (the run was
    /// settled without resuming) is repaired with its staged calls answered
    /// outcome-unknown: dropping the cursor alone would hide calls that may
    /// have run.
    #[test]
    fn repaired_tools_pending_tail_answers_its_staged_calls_outcome_unknown() {
        let run_id = RunId::new();
        let input_id = InputId::new();
        let staged = staged_tool_call_message(&run_id);
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            vec![Message::User(UserMessage::text("started the run"))],
            Some(cursor(
                &run_id,
                vec![input_id],
                RunResumePoint::ToolsPending {
                    assistant_message: staged.clone(),
                },
            )),
        );
        let RecoveryCandidatePreparation::Prepared(candidate) =
            prepare_whole_blob_recovery_candidate(committed, Some(provisional), &HashMap::new())
                .expect("a settled ToolsPending tail prepares")
        else {
            panic!("a settled ToolsPending tail must prepare a candidate");
        };
        assert_eq!(
            candidate.class,
            DurableTailRecoveryClass::InterruptedRepairableCandidate
        );
        assert!(candidate.recovered.try_run_cursor().unwrap().is_none());
        assert_eq!(
            candidate.recovered.messages().len(),
            5,
            "the committed input, the run's row, the staged message, its results, the notice"
        );
        assert_staged_calls_closed(&candidate.recovered, &staged);
    }

    /// A provider that numbers each response's calls from `fc_0` (Gemini)
    /// repeats call ids across rounds: an earlier committed round's `fc_0`
    /// in the tail must not make the staged round's `fc_0` disagree. The
    /// staged message is matched by its message id.
    #[test]
    fn repeated_provider_call_ids_across_rounds_still_agree() {
        let run_id = RunId::new();
        let input_id = InputId::new();
        let earlier = tool_call_message(&run_id, "fc_0", "0190b5f0-0000-7000-8000-000000000001");
        let earlier_results = Message::ToolResults {
            results: vec![meerkat_core::types::ToolResult::new(
                "fc_0".to_string(),
                "done".to_string(),
                false,
            )],
            created_at: message_timestamp_now(),
        };
        let staged = tool_call_message(&run_id, "fc_0", "0190b5f0-0000-7000-8000-000000000003");
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            vec![Message::BlockAssistant(earlier), earlier_results],
            Some(cursor(
                &run_id,
                vec![input_id.clone()],
                RunResumePoint::ToolsPending {
                    assistant_message: staged,
                },
            )),
        );
        let RecoveryCandidatePreparation::Prepared(candidate) =
            prepare_whole_blob_recovery_candidate(
                committed,
                Some(provisional),
                &bound(&run_id, &[input_id]),
            )
            .expect("a two-round tail prepares")
        else {
            panic!("a two-round tail must prepare a candidate");
        };
        assert_eq!(
            candidate.class,
            DurableTailRecoveryClass::ContinuableCandidate
        );
    }

    /// The staged message already in the tail means the round's second
    /// commit landed: a ToolsPending cursor then disagrees and holds.
    #[test]
    fn staged_message_already_in_the_tail_disagrees() {
        let run_id = RunId::new();
        let input_id = InputId::new();
        let staged = staged_tool_call_message(&run_id);
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            vec![Message::BlockAssistant(staged.clone())],
            Some(cursor(
                &run_id,
                vec![input_id.clone()],
                RunResumePoint::ToolsPending {
                    assistant_message: staged,
                },
            )),
        );
        assert!(matches!(
            prepare_whole_blob_recovery_candidate(
                committed,
                Some(provisional),
                &bound(&run_id, &[input_id]),
            ),
            Ok(RecoveryCandidatePreparation::Held)
        ));
    }

    /// The tail a WholeBlob run re-base restarts on its compacted snapshot:
    /// metadata only, with a cursor awaiting the model.
    #[test]
    fn rebase_tail_awaiting_the_model_continues() {
        let run_id = RunId::new();
        let input_id = InputId::new();
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            Vec::new(),
            Some(cursor(
                &run_id,
                vec![input_id.clone()],
                RunResumePoint::AwaitingModel,
            )),
        );
        let RecoveryCandidatePreparation::Prepared(candidate) =
            prepare_whole_blob_recovery_candidate(
                committed,
                Some(provisional),
                &bound(&run_id, &[input_id]),
            )
            .expect("a re-base tail prepares")
        else {
            panic!("a re-base tail must prepare a candidate");
        };
        assert_eq!(
            candidate.class,
            DurableTailRecoveryClass::ContinuableCandidate
        );
    }

    /// A cursor naming another run than the store-issued provisional run
    /// contradicts the store.
    #[test]
    fn cursor_naming_another_run_is_invalid_evidence() {
        let run_id = RunId::new();
        let other_run = RunId::new();
        let input_id = InputId::new();
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            Vec::new(),
            Some(cursor(
                &other_run,
                vec![input_id.clone()],
                RunResumePoint::AwaitingModel,
            )),
        );
        assert!(matches!(
            prepare_whole_blob_recovery_candidate(
                committed,
                Some(provisional),
                &bound(&other_run, &[input_id]),
            ),
            Err(DurableTailRecoveryError::InvalidEvidence(detail))
                if detail.contains("run cursor names run")
        ));
    }

    /// Direct and machine-service turns write cursors with no bound inputs:
    /// such a tail never continues, it is repaired as interrupted (C3).
    #[test]
    fn cursor_with_no_bound_inputs_is_repaired_not_continued() {
        let run_id = RunId::new();
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            vec![Message::User(UserMessage::text("direct turn"))],
            Some(cursor(&run_id, Vec::new(), RunResumePoint::AwaitingModel)),
        );
        let RecoveryCandidatePreparation::Prepared(candidate) =
            prepare_whole_blob_recovery_candidate(committed, Some(provisional), &HashMap::new())
                .expect("an empty-set cursor tail prepares")
        else {
            panic!("an empty-set cursor tail must prepare a candidate");
        };
        assert_eq!(
            candidate.class,
            DurableTailRecoveryClass::InterruptedRepairableCandidate
        );
        assert!(candidate.continuation.is_none());
        assert!(candidate.recovered.try_run_cursor().unwrap().is_none());
    }

    /// A bound input that is no longer bound to the run (requeued, or
    /// terminal) ends the continuation: the tail is repaired (C2).
    #[test]
    fn cursor_whose_inputs_are_no_longer_bound_is_repaired() {
        let run_id = RunId::new();
        let input_id = InputId::new();
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            vec![Message::User(UserMessage::text("started the run"))],
            Some(cursor(
                &run_id,
                vec![input_id],
                RunResumePoint::AwaitingModel,
            )),
        );
        let RecoveryCandidatePreparation::Prepared(candidate) =
            prepare_whole_blob_recovery_candidate(committed, Some(provisional), &HashMap::new())
                .expect("a cursor tail with settled inputs prepares")
        else {
            panic!("a cursor tail with settled inputs must prepare a candidate");
        };
        assert_eq!(
            candidate.class,
            DurableTailRecoveryClass::InterruptedRepairableCandidate
        );
    }

    /// A run's closing save before any assistant row (no run rows, no
    /// cursor) is attributed by the store binding and repaired.
    #[test]
    fn closing_save_without_run_rows_is_repaired() {
        let run_id = RunId::new();
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id,
            vec![Message::User(UserMessage::text("the run's input"))],
            None,
        );
        let RecoveryCandidatePreparation::Prepared(candidate) =
            prepare_whole_blob_recovery_candidate(committed, Some(provisional), &HashMap::new())
                .expect("a closing save prepares")
        else {
            panic!("a closing save must prepare a candidate");
        };
        assert_eq!(
            candidate.class,
            DurableTailRecoveryClass::InterruptedRepairableCandidate
        );
    }

    /// An unstamped assistant row is not the no-run-rows shape: it stays
    /// invalid evidence on a strict continuation, cursor or not.
    #[test]
    fn unstamped_assistant_row_with_a_cursor_is_invalid_evidence() {
        let run_id = RunId::new();
        let input_id = InputId::new();
        let mut unstamped = staged_tool_call_message(&run_id);
        unstamped.identity = TranscriptMessageIdentity::default();
        unstamped.blocks = vec![AssistantBlock::Text {
            text: "unstamped".to_string(),
            meta: None,
        }];
        unstamped.stop_reason = None;
        let (committed, provisional) = whole_blob_cursor_fixture(
            run_id.clone(),
            vec![Message::BlockAssistant(unstamped)],
            Some(cursor(
                &run_id,
                vec![input_id.clone()],
                RunResumePoint::AwaitingModel,
            )),
        );
        assert!(matches!(
            prepare_whole_blob_recovery_candidate(
                committed,
                Some(provisional),
                &bound(&run_id, &[input_id]),
            ),
            Err(DurableTailRecoveryError::InvalidEvidence(detail))
                if detail.contains("store-issued run identity")
        ));
    }

    /// A committed body never makes a run resumable (D3): with no
    /// provisional tail the source is aligned, whatever its metadata holds.
    #[test]
    fn committed_body_with_a_stale_cursor_is_aligned_not_continued() {
        let run_id = RunId::new();
        let mut committed = Session::new();
        committed.push(Message::User(UserMessage::text("committed input")));
        committed
            .set_run_cursor(&cursor(
                &run_id,
                vec![InputId::new()],
                RunResumePoint::AwaitingModel,
            ))
            .unwrap();
        let session_id = committed.id().clone();
        let committed_bytes = Arc::new(committed.to_persisted_bytes().unwrap());
        let committed_sha = format!("row-sha256:{:x}", Sha256::digest(committed_bytes.as_ref()));
        let committed_authority =
            WholeBlobStoreAuthority::issued(session_id, 7, committed_sha).unwrap();
        let snapshot =
            CommittedWholeBlobSnapshot::new(committed_bytes, committed_authority).unwrap();
        assert!(matches!(
            prepare_whole_blob_recovery_candidate(snapshot, None, &HashMap::new()),
            Ok(RecoveryCandidatePreparation::AlreadyAligned { .. })
        ));
    }
}

#[cfg(test)]
mod physical_history_witness_tests {
    use super::physical_history_witness_mismatch;
    use meerkat_core::session_store::{SessionHead, TranscriptStrandId};
    use meerkat_core::types::UserMessage;
    use meerkat_core::{
        Message, Session, TranscriptRewriteReason, TranscriptRewriteSelection,
        ValidatedTranscriptHistory,
    };

    /// A session after `compactions` compaction-shaped rewrites, its graph
    /// and the head a store writes for it.
    #[allow(clippy::expect_used)]
    fn compacted(compactions: u64, label: &str) -> (ValidatedTranscriptHistory, SessionHead) {
        let mut session = Session::new();
        for compaction in 0..compactions {
            session.push(Message::User(UserMessage::text(format!(
                "{label} row {compaction}"
            ))));
            let end = session.messages().len();
            session
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 0, end },
                    vec![Message::User(UserMessage::text(format!(
                        "{label} summary {compaction}"
                    )))],
                    TranscriptRewriteReason::new("test compaction"),
                    Some("unit-test".to_string()),
                    None,
                )
                .expect("compaction-shaped rewrite");
        }
        let history = session
            .validated_transcript_history_state()
            .expect("graph decodes")
            .expect("graph present");
        let head = SessionHead::from_session(&session, TranscriptStrandId::root(), compactions)
            .expect("head");
        (history, head)
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn a_physical_graph_matching_its_head_is_its_witness() {
        let (history, head) = compacted(2, "own");
        assert_eq!(
            physical_history_witness_mismatch(Some(&history), &head),
            None
        );
        let plain = SessionHead::from_session(&Session::new(), TranscriptStrandId::root(), 0)
            .expect("head without rewrites");
        assert_eq!(physical_history_witness_mismatch(None, &plain), None);
    }

    #[test]
    fn a_graph_whose_occurrence_count_is_not_the_heads_is_refused() {
        let (history, mut head) = compacted(2, "own");
        head.rewrite_count = 3;
        assert_eq!(
            physical_history_witness_mismatch(Some(&history), &head),
            Some("the graph's occurrence count differs from the head's rewrite count")
        );
    }

    #[test]
    fn a_graph_whose_rewrite_prefix_is_not_the_heads_is_refused() {
        let (history, mut head) = compacted(2, "own");
        let (_, foreign) = compacted(2, "foreign");
        head.rewrite_prefix = foreign.rewrite_prefix;
        assert_eq!(
            physical_history_witness_mismatch(Some(&history), &head),
            Some("the graph's rewrite prefix differs from the head's")
        );
    }

    #[test]
    fn a_graph_whose_prefix_is_not_the_heads_graph_prefix_is_refused() {
        let (history, mut head) = compacted(2, "own");
        let (_, foreign) = compacted(2, "foreign");
        head.graph_prefix = foreign.graph_prefix;
        assert_eq!(
            physical_history_witness_mismatch(Some(&history), &head),
            Some("the graph's prefix differs from the head's graph prefix")
        );
    }

    #[test]
    fn a_head_with_rewrites_and_no_graph_is_refused() {
        let (_, head) = compacted(1, "own");
        assert_eq!(
            physical_history_witness_mismatch(None, &head),
            Some("the head has rewrite occurrences but no graph")
        );
    }

    #[test]
    fn a_head_without_rewrites_carrying_a_graph_is_refused() {
        let (history, mut head) = compacted(1, "own");
        head.rewrite_count = 0;
        assert_eq!(
            physical_history_witness_mismatch(Some(&history), &head),
            Some("the head has no rewrite occurrences but the source carries a graph")
        );
    }

    #[test]
    fn the_classifier_relation_is_the_stores_proof() {
        use super::{DurableTailSourceRelation, classifier_relation};
        use meerkat_core::session_document::DurableHeadRelation;
        assert_eq!(
            classifier_relation(DurableTailSourceRelation::StrictContinuation),
            DurableHeadRelation::VerifiedStrictDescendant
        );
        assert_eq!(
            classifier_relation(DurableTailSourceRelation::RewriteDescendant { live_tail_base: 3 }),
            DurableHeadRelation::VerifiedRewriteDescendant
        );
        assert_eq!(
            classifier_relation(DurableTailSourceRelation::UnprovenRewrite),
            DurableHeadRelation::Unverifiable,
            "an unproven rewrite is never classified as a strict continuation"
        );
    }
}
