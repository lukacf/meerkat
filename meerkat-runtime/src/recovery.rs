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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::identifiers::LogicalRuntimeId;
use crate::input_state::{InputLifecycleState, InputStatePersistenceRecord, StoredInputState};
use crate::meerkat_machine::dsl as mm_dsl;
use crate::runtime_state::RuntimeState;
use crate::store::{
    CommittedRecoveryBoundary, CommittedWholeBlobProvisionalTail, CommittedWholeBlobSnapshot,
    MachineLifecycleBindingFacts, MachineLifecycleCommit, MachineLifecycleExpectedVersion,
    MachineLifecycleObservation, MachineLifecycleRunFacts, PreparedDurableTailRecoverySource,
    PreparedRecoveryEvidence, PreparedRecoveryReceiptDigestEnrichment,
    PreparedRecoveryReceiptSource, PreparedRuntimeSessionCommit, RecoveryInputSetRevision,
    RuntimeSessionPersistenceProfile, RuntimeStore, RuntimeStoreError, SupervisorAuthoritySnapshot,
};
use meerkat_core::lifecycle::InputId;
use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::session_document::{
    DurableHeadRelation, DurableTailRecoveryClass as ClassifiedRecoveryClass,
    DurableTailStopReason, RunIdCardinality, SessionDocumentEffect, SessionDocumentKey,
    SessionDocumentMachineAuthority,
};
use meerkat_core::session_store::PreparedHeadCanonicalMutation;
use meerkat_core::types::SessionId;
use meerkat_core::{Message, RunId, Session, StopReason, SystemNoticeKind, SystemNoticeMessage};
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
}

enum RecoveryCandidatePreparation {
    Prepared(PreparedRecoveryCandidate),
    AlreadyAligned(Arc<Session>),
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
                match assistant.identity.run_id.as_ref() {
                    Some(run_id) => {
                        run_ids.insert(run_id.to_string());
                        tail_run_id = Some(run_id.clone());
                    }
                    None => assistant_without_run_id = true,
                }
                open_call_ids.extend(assistant.tool_calls().map(|call| call.id.to_string()));
                let effective_stop = match assistant.stop_reason {
                    StopReason::EndTurn => DurableTailStopReason::EndTurn,
                    StopReason::ToolUse if assistant.has_tool_calls() => {
                        DurableTailStopReason::ToolUse
                    }
                    // The live agent decides the tool phase from actual call
                    // blocks, not the provider's stop label. A ToolUse label
                    // with no calls is operationally terminal and recovery
                    // must mirror that exact decision.
                    StopReason::ToolUse => DurableTailStopReason::EndTurn,
                    _ => DurableTailStopReason::Other,
                };
                last_assistant_stop = Some(effective_stop);
                // ToolUse is an intermediate provider boundary: durable tool
                // results and a later assistant continuation can belong to
                // the same run. Every other stop reason closes that run, so
                // any later message makes the candidate ambiguous even when
                // a later assistant reuses the same run id and ends cleanly.
                terminal_seen = effective_stop != DurableTailStopReason::ToolUse;
            }
            Message::ToolResults { results, .. } => {
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
    }
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
    format!("sha256:{:x}", hasher.finalize())
}

fn generated_recovery_classification(
    session_id: &SessionId,
    candidate_id: &str,
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
            DurableHeadRelation::VerifiedStrictDescendant,
            observation.run_id_cardinality,
            observation.terminal_stop_reason,
            observation.dangling_tool_use_ids.len() as u64,
            observation.orphan_tool_result_count,
            observation.messages_after_terminal,
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

fn repair_interrupted_tail(
    recovered: &mut Session,
    dangling_tool_use_ids: &[String],
    durable_tail_timestamp: meerkat_core::types::MessageTimestamp,
) -> Result<(), DurableTailRecoveryError> {
    // The classifier admits repair only with zero dangling calls. Recheck in
    // release code: manufacturing tool results would invent external
    // execution truth for a call whose side effect may already have fired.
    if !dangling_tool_use_ids.is_empty() {
        return Err(DurableTailRecoveryError::InvalidEvidence(format!(
            "interrupted-tail repair was classified despite {} dangling tool call(s): {}",
            dangling_tool_use_ids.len(),
            dangling_tool_use_ids.join(", ")
        )));
    }
    recovered.push(Message::SystemNotice(SystemNoticeMessage {
        kind: SystemNoticeKind::Generic,
        body: Some(
            "A previous run was interrupted before its boundary committed. Recovery preserved \
             every durable message, closed the run as InterruptedByRecovery, and did not requeue \
             its input. Continue from a new turn."
                .to_string(),
        ),
        blocks: Vec::new(),
        // Recovery of the same exact durable head must be byte-identical in
        // every process. Wall-clock `now` would make the recovered boundary
        // and idempotency witness race-dependent.
        created_at: durable_tail_timestamp,
    }));
    Ok(())
}

fn prepare_head_canonical_recovery_candidate(
    source: &PreparedDurableTailRecoverySource,
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
        return Ok(RecoveryCandidatePreparation::AlreadyAligned(Arc::clone(
            source.physical_session(),
        )));
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
    if physical_head.messages().len() <= committed_authority.messages().len()
        || !physical_head
            .messages()
            .starts_with(committed_authority.messages())
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "physical recovery head is not an exact strict transcript continuation".to_string(),
        ));
    }

    let derived_physical_head_token = source.physical_head_cas_token().to_string();

    let observation = observe_durable_tail(committed_authority.messages().len(), physical_head);
    if observation.tail_run_id.as_ref() != Some(provisional.run_id())
        || observation.run_id_cardinality != RunIdCardinality::SingleRunId
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
    let classified = generated_recovery_classification(&session_id, &candidate_id, &observation)?;
    let class = runtime_recovery_class(classified)?;
    if class == DurableTailRecoveryClass::Ambiguous {
        return Ok(RecoveryCandidatePreparation::Held);
    }
    let candidate_run_id = provisional.run_id().clone();

    // The canonical physical materialization is already the exact durable
    // successor content. Starting from it is both simpler and deterministic:
    // replaying the suffix onto an inline transcript graph manufactures fresh
    // revision-body timestamps in `Session::push`, so two processes would
    // mint different recovered boundaries for the same durable rows.
    //
    // Head-canonical materialization must be slim. Retained rewrite bodies are
    // an out-of-line store concern and the carried rewrite-prefix/history
    // witness in SessionHead is the recovery document's authority.
    if physical_head
        .validated_transcript_history_state()
        .map_err(|error| {
            DurableTailRecoveryError::InvalidEvidence(format!(
                "physical-head transcript history is malformed: {error}"
            ))
        })?
        .is_some()
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "head-canonical recovery source unexpectedly contains inline transcript history"
                .to_string(),
        ));
    }
    let mut recovered = physical_head.clone();
    match class {
        DurableTailRecoveryClass::CompletedCandidate => {
            // The exact physical transcript is already the completed boundary.
        }
        DurableTailRecoveryClass::InterruptedRepairableCandidate => {
            let durable_tail_timestamp = physical_head
                .messages()
                .last()
                .map(message_timestamp)
                .ok_or_else(|| {
                    DurableTailRecoveryError::InvalidEvidence(
                        "interrupted recovery has no durable tail timestamp".to_string(),
                    )
                })?;
            repair_interrupted_tail(
                &mut recovered,
                &observation.dangling_tool_use_ids,
                durable_tail_timestamp,
            )?;
        }
        DurableTailRecoveryClass::Ambiguous => {
            return Err(DurableTailRecoveryError::Authority(
                "ambiguous recovery candidate reached document preparation after hold".to_string(),
            ));
        }
    }
    // Synthetic repair necessarily advances the outer updated_at with
    // wall-clock time. Re-adopting the exact physical envelope restores the
    // durable timestamp and all non-recovery-owned state. Completed recovery
    // is already byte-for-byte the physical materialization; applying this
    // uniformly keeps the field-ownership rule in one place.
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
        },
    ))
}

fn prepare_whole_blob_recovery_candidate(
    committed: CommittedWholeBlobSnapshot,
    provisional: Option<CommittedWholeBlobProvisionalTail>,
) -> Result<RecoveryCandidatePreparation, DurableTailRecoveryError> {
    let (committed_session, _committed_bytes, committed_authority) = committed.into_parts();
    if committed_session.id() != committed_authority.session_id() {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "committed WholeBlob payload identity differs from store authority".to_string(),
        ));
    }
    let Some(provisional) = provisional else {
        return Ok(RecoveryCandidatePreparation::AlreadyAligned(
            committed_session,
        ));
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
    let candidate_session =
        Session::from_persisted_bytes(provisional.candidate_bytes()).map_err(|error| {
            DurableTailRecoveryError::InvalidEvidence(format!(
                "provisional WholeBlob payload is invalid: {error}"
            ))
        })?;
    if candidate_session.id() != committed_session.id()
        || candidate_session.version() != committed_session.version()
        || candidate_session.created_at() != committed_session.created_at()
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "WholeBlob provisional tail changes immutable session identity".to_string(),
        ));
    }
    if candidate_session.messages().len() <= committed_session.messages().len()
        || !candidate_session
            .messages()
            .starts_with(committed_session.messages())
    {
        return Err(DurableTailRecoveryError::InvalidEvidence(
            "WholeBlob provisional tail is not an exact strict transcript continuation".to_string(),
        ));
    }
    let observation = observe_durable_tail(committed_session.messages().len(), &candidate_session);
    if observation.tail_run_id.as_ref() != Some(provisional_authority.run_id())
        || observation.run_id_cardinality != RunIdCardinality::SingleRunId
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
    let class = runtime_recovery_class(generated_recovery_classification(
        committed_authority.session_id(),
        &candidate_id,
        &observation,
    )?)?;
    if class == DurableTailRecoveryClass::Ambiguous {
        return Ok(RecoveryCandidatePreparation::Held);
    }
    let physical_envelope = candidate_session.clone();
    let mut recovered = candidate_session;
    match class {
        DurableTailRecoveryClass::CompletedCandidate => {}
        DurableTailRecoveryClass::InterruptedRepairableCandidate => {
            let durable_tail_timestamp = recovered
                .messages()
                .last()
                .map(message_timestamp)
                .ok_or_else(|| {
                    DurableTailRecoveryError::InvalidEvidence(
                        "interrupted WholeBlob recovery has no durable tail timestamp".to_string(),
                    )
                })?;
            repair_interrupted_tail(
                &mut recovered,
                &observation.dangling_tool_use_ids,
                durable_tail_timestamp,
            )?;
        }
        DurableTailRecoveryClass::Ambiguous => {
            return Err(DurableTailRecoveryError::Authority(
                "ambiguous WholeBlob recovery reached document preparation".to_string(),
            ));
        }
    }
    recovered
        .adopt_recovered_head_state(&physical_envelope)
        .map_err(DurableTailRecoveryError::InvalidEvidence)?;
    let recovered = Arc::new(recovered);
    let (document, recovered_blob_sha256) = match class {
        DurableTailRecoveryClass::CompletedCandidate => (
            None,
            provisional_authority.candidate_blob_sha256().to_string(),
        ),
        DurableTailRecoveryClass::InterruptedRepairableCandidate => {
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
        DurableTailRecoveryClass::Ambiguous => {
            return Err(DurableTailRecoveryError::Authority(
                "ambiguous WholeBlob recovery reached artifact preparation".to_string(),
            ));
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
    /// session document).
    reassert_state: RuntimeState,
    binding: MachineLifecycleBindingFacts,
    supervisor_authority: SupervisorAuthoritySnapshot,
    unregister_progress: Option<crate::store::MachineUnregisterProgressSnapshot>,
    live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage,
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
            live_bridge_recovery: crate::live_execution::LiveBridgeRecoveryImage::default(),
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
                live_bridge_recovery: record.live_bridge_recovery().clone(),
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
        },
    }
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

async fn load_recovery_preparation(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    session_id: &SessionId,
) -> Result<RecoveryCandidatePreparation, DurableTailRecoveryError> {
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
            prepare_head_canonical_recovery_candidate(&source)
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
            prepare_whole_blob_recovery_candidate(committed, provisional)
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
    let runtime_id = LogicalRuntimeId::for_session(session_id);
    let mut preparation = load_recovery_preparation(store, &runtime_id, session_id).await?;
    let mut rolled_back_incomplete_intent = false;
    let candidate = loop {
        match preparation {
            RecoveryCandidatePreparation::Prepared(candidate) => break candidate,
            RecoveryCandidatePreparation::AlreadyAligned(recovered) => {
                tracing::info!(
                    %session_id,
                    "durable-tail recovery source is already exactly aligned"
                );
                let recovered =
                    Arc::try_unwrap(recovered).unwrap_or_else(|shared| shared.as_ref().clone());
                return Ok(DurableTailRecoveryOutcome::AlreadyAligned {
                    recovered: Box::new(recovered),
                });
            }
            RecoveryCandidatePreparation::IncompleteHeadCanonicalIntent { provisional } => {
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
                    return Ok(DurableTailRecoveryOutcome::Held);
                }
                tracing::info!(
                    %session_id,
                    physical_revision = provisional.physical_store_revision(),
                    "rolled back incomplete HeadCanonical provisional intent"
                );
                rolled_back_incomplete_intent = true;
                preparation = load_recovery_preparation(store, &runtime_id, session_id).await?;
            }
            RecoveryCandidatePreparation::Held => return Ok(DurableTailRecoveryOutcome::Held),
        }
    };

    let observed = match store.observe_machine_lifecycle(&runtime_id).await {
        Ok(observation) => observe_persisted_lifecycle(observation, &candidate.candidate_run_id),
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
            return Ok(DurableTailRecoveryOutcome::Held);
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
        return Ok(committed_recovery_outcome(
            candidate,
            disposition,
            boundary_sequence,
        ));
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
            return Ok(DurableTailRecoveryOutcome::Held);
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
    )
    .await?;

    let mut authority =
        crate::meerkat_machine::dsl_authority::new_registered_authority_without_runtime_entry(
            &candidate.session_id,
        )
        .map_err(|error| DurableTailRecoveryError::Authority(error.to_string()))?;
    let transition = mm_dsl::MeerkatMachineMutator::apply(
        &mut authority,
        mm_dsl::MeerkatMachineInput::AuthorizeDurableTailRecovery {
            session_id: mm_dsl::SessionId::from_domain(&candidate.session_id),
            candidate_id: candidate.candidate_id.clone(),
            candidate_run_id: mm_dsl::RunId(candidate.candidate_run_id.to_string()),
            class: candidate.class,
            observed_lifecycle: observed.lifecycle,
            observed_current_run: observed.current_run,
            last_committed_sequence: prior.last_committed_sequence,
            prior_commit: prior.classification,
            input_evidence: inputs.evidence,
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
                return Ok(committed_recovery_outcome(
                    candidate,
                    disposition,
                    boundary_sequence,
                ));
            }
            match non_commit {
                DurableTailRecoveryDisposition::HoldIntact => {
                    tracing::warn!(
                        session_id = %candidate.session_id,
                        candidate_run_id = %candidate.candidate_run_id,
                        class = ?candidate.class,
                        prior_commit = ?prior.classification,
                        input_evidence = ?inputs.evidence,
                        "durable-tail recovery held intact by machine verdict"
                    );
                    return Ok(DurableTailRecoveryOutcome::Held);
                }
                DurableTailRecoveryDisposition::RefuseRecovery => {
                    return Ok(DurableTailRecoveryOutcome::Refused);
                }
                other => {
                    return Err(DurableTailRecoveryError::Authority(format!(
                        "generated machine emitted unexpected non-commit disposition {other:?}"
                    )));
                }
            }
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
    let mut input_updates =
        terminalize_attributed_inputs(attributed, &candidate.candidate_run_id, boundary_sequence)?;
    let contributing_input_ids: Vec<InputId> = input_updates
        .iter()
        .map(|record| record.as_stored().state.input_id.clone())
        .collect();
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
            return Ok(DurableTailRecoveryOutcome::Held);
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
    if committed.downstream_projection_required() {
        return Err(DurableTailRecoveryError::Authority(
            "store-owned recovery unexpectedly requested a SessionStore projection".to_string(),
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
    Ok(committed_recovery_outcome(
        candidate,
        disposition,
        boundary_sequence,
    ))
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
        // The same fenced recovery transaction installs the recovered
        // boundary receipt and this terminal row. Once attributed, ordinary
        // content is neither redeliverable nor needed to identify a future
        // durable tail. A directed input without its terminal outbox is kept
        // fail-closed because its payload still owns the Interaction identity.
        if crate::store::input_state_payload_is_retirable(&bundle) {
            bundle.state.persisted_input = None;
        }
        let record = InputStatePersistenceRecord::from_machine_snapshot(bundle)
            .map_err(DurableTailRecoveryError::Authority)?
            .with_expected_row_digest(row_digest);
        updates.push(record);
    }
    Ok(updates)
}

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

    fn observation(run_id: &RunId) -> DurableTailObservation {
        DurableTailObservation {
            tail_run_id: Some(run_id.clone()),
            run_id_cardinality: RunIdCardinality::SingleRunId,
            terminal_stop_reason: DurableTailStopReason::EndTurn,
            dangling_tool_use_ids: Vec::new(),
            orphan_tool_result_count: 0,
            messages_after_terminal: false,
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
        );
        let reassertion = durable_tail_lifecycle_reassertion(&observed);
        assert_eq!(
            reassertion.snapshot().live_bridge_recovery(),
            &live_bridge_recovery,
            "durable-tail lifecycle reassertion must not erase V5 bridge truth"
        );
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
            stop_reason: StopReason::EndTurn,
            identity: TranscriptMessageIdentity::default().with_run_id(transcript_run),
            created_at: message_timestamp_now(),
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
            prepare_whole_blob_recovery_candidate(committed, Some(provisional))
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
            prepare_whole_blob_recovery_candidate(committed, Some(provisional)),
            Err(DurableTailRecoveryError::InvalidEvidence(detail))
                if detail.contains("store-issued run identity")
        ));
    }
}
