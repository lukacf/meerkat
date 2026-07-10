//! Realtime-transcript revision shell — NON-generated MECHANISM.
//!
//! This module is the mechanical companion to the realtime-transcript region
//! of the canonical [`crate::generated::session_document::SessionDocumentMachine`].
//!
//! ## DECISION vs MECHANISM split
//!
//! Every SEMANTIC decision in the realtime-transcript flow is owned by the
//! `SessionDocumentMachine` DSL (its realtime-transcript region):
//!   * the per-event **action vector** (observe/append/replace/promote/
//!     mark-ready/record/discard/materialize),
//!   * the per-item **materialize verdict** (`Wait` / `MarkSkipped` /
//!     `MaterializeUser` / `MaterializeAssistant` + `consume_usage`),
//!   * the snapshot-restore **consistency authorization**.
//!
//! This file performs only the MECHANICAL work the DSL expr language cannot
//! express and which the task scopes as shell helpers:
//!   * the per-session item registry storage ([`SessionRealtimeTranscriptState`]
//!     and its content-segment maps),
//!   * `item.text()` string assembly over `content_segments`,
//!   * the causal topological ordering ([`realtime_transcript_order`]),
//!   * the materialize loop / batching / `AssistantBlock`/`Message` assembly,
//!   * `in_flight` filtering + dedup,
//!   * the restore-consistency observation computations.
//!
//! It DECIDES nothing: it computes typed RAW observations from the bulky
//! registry, hands them to the machine, and mirrors the machine's resolved
//! action vector / verdict back onto the registry.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use sha2::{Digest, Sha256};

use crate::generated::session_document::{
    RealtimeTranscriptLaneKind, RealtimeTranscriptMaterializeDecision, RealtimeTranscriptRoleKind,
    RealtimeTranscriptStopReasonKind, RealtimeUserContentBlobFinalizeDisposition,
    RealtimeUserContentBlobRecoveryDisposition, RealtimeUserContentBlobStageDisposition,
    RealtimeUserContentIdentityDisposition, SessionDocumentEffect, SessionDocumentError,
    SessionDocumentMachineAuthority,
};
use crate::realtime_transcript::{
    PendingRealtimeUserContentBlob, RealtimeTranscriptApplyOutcome, RealtimeTranscriptEvent,
    RealtimeTranscriptMaterializedMessage, RealtimeTranscriptRole, RealtimeUserContentApplyOutcome,
    RealtimeUserContentIdentity, RealtimeUserContentTombstone, TranscriptLane,
};
use crate::types::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, ContentInput, Message, StopReason,
    TranscriptSource, Usage, UserMessage,
};

/// Error surfaced when the canonical SessionDocument authority rejects a
/// realtime-transcript operation. Carries the rejected op label for fail-closed
/// diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeTranscriptShellError {
    op: &'static str,
}

impl std::fmt::Display for RealtimeTranscriptShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "session document authority rejected realtime-transcript {}",
            self.op
        )
    }
}

impl std::error::Error for RealtimeTranscriptShellError {}

impl From<SessionDocumentError> for RealtimeTranscriptShellError {
    fn from(err: SessionDocumentError) -> Self {
        // The machine error label is stable; re-tag it under a realtime op so
        // the caller's fail-closed path keeps a transcript-scoped message.
        let _ = err;
        Self {
            op: "decision_rejected",
        }
    }
}

// ---------------------------------------------------------------------------
// DECISION mirrors: typed projections of the machine's emitted action vectors.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct RealtimeTranscriptEventDecision {
    observe_item: bool,
    observe_skipped: bool,
    write_user_segment: bool,
    append_assistant_segment: bool,
    replace_assistant_segment: bool,
    promote_lane: bool,
    mark_item_ready: bool,
    record_delta_id: bool,
    remove_completion: bool,
    record_completion: bool,
    discard_response: bool,
    discard_response_by_lane: bool,
    mark_response_ready: bool,
    materialize_ready_items: bool,
}

#[derive(Debug, Clone, Copy)]
struct MaterializeCandidateDecision {
    decision: RealtimeTranscriptMaterializeDecision,
    consume_usage: bool,
}

fn document_authority() -> SessionDocumentMachineAuthority {
    SessionDocumentMachineAuthority::new()
}

/// Drive the machine's realtime-transcript event region and mirror the emitted
/// action vector. The machine decides; this never does.
fn resolve_realtime_event(
    apply: impl FnOnce(
        &mut SessionDocumentMachineAuthority,
    ) -> Result<Vec<SessionDocumentEffect>, SessionDocumentError>,
) -> Result<RealtimeTranscriptEventDecision, RealtimeTranscriptShellError> {
    let mut authority = document_authority();
    let effects = apply(&mut authority)?;
    effects
        .into_iter()
        .find_map(|effect| match effect {
            SessionDocumentEffect::RealtimeTranscriptEventResolved {
                observe_item,
                observe_skipped,
                write_user_segment,
                append_assistant_segment,
                replace_assistant_segment,
                promote_lane,
                mark_item_ready,
                record_delta_id,
                remove_completion,
                record_completion,
                discard_response,
                discard_response_by_lane,
                mark_response_ready,
                materialize_ready_items,
            } => Some(RealtimeTranscriptEventDecision {
                observe_item,
                observe_skipped,
                write_user_segment,
                append_assistant_segment,
                replace_assistant_segment,
                promote_lane,
                mark_item_ready,
                record_delta_id,
                remove_completion,
                record_completion,
                discard_response,
                discard_response_by_lane,
                mark_response_ready,
                materialize_ready_items,
            }),
            _ => None,
        })
        .ok_or(RealtimeTranscriptShellError {
            op: "event_resolved",
        })
}

fn resolve_materialize_candidate(
    state: &SessionRealtimeTranscriptState,
    item: &RealtimeTranscriptItemState,
    text_present: bool,
    completion: Option<&RealtimeAssistantCompletion>,
) -> Result<MaterializeCandidateDecision, RealtimeTranscriptShellError> {
    let mut authority = document_authority();
    let effects = authority.resolve_realtime_materialize_candidate(
        item.materialized,
        realtime_predecessor_materialized(state, item.previous_item_id.as_deref()),
        item.skipped,
        item.ready,
        text_present,
        role_kind(item.role),
        item.response_id.is_some(),
        completion.is_some(),
        completion.is_some_and(|completion| completion.usage_consumed),
    )?;
    effects
        .into_iter()
        .find_map(|effect| match effect {
            SessionDocumentEffect::RealtimeMaterializeCandidateResolved {
                decision,
                consume_usage,
            } => Some(MaterializeCandidateDecision {
                decision,
                consume_usage,
            }),
            _ => None,
        })
        .ok_or(RealtimeTranscriptShellError {
            op: "materialize_candidate_resolved",
        })
}

#[derive(Debug, Clone, Copy)]
struct RealtimeUserContentIdentityFacts {
    identity_fields_valid: bool,
    key_tombstoned: bool,
    predecessor_materialized: bool,
    existing_identity_present: bool,
    existing_payload_matches: bool,
    target_item_id_available: bool,
    reducer_commit_proof_required: bool,
    reducer_commit_proof_present: bool,
}

fn resolve_realtime_user_content_identity(
    facts: RealtimeUserContentIdentityFacts,
) -> Result<RealtimeUserContentIdentityDisposition, RealtimeTranscriptShellError> {
    let RealtimeUserContentIdentityFacts {
        identity_fields_valid,
        key_tombstoned,
        predecessor_materialized,
        existing_identity_present,
        existing_payload_matches,
        target_item_id_available,
        reducer_commit_proof_required,
        reducer_commit_proof_present,
    } = facts;
    let mut authority = document_authority();
    let effects = authority.resolve_realtime_user_content_identity(
        identity_fields_valid,
        key_tombstoned,
        predecessor_materialized,
        existing_identity_present,
        existing_payload_matches,
        target_item_id_available,
        reducer_commit_proof_required,
        reducer_commit_proof_present,
    )?;
    effects
        .into_iter()
        .find_map(|effect| match effect {
            SessionDocumentEffect::RealtimeUserContentIdentityResolved { disposition } => {
                Some(disposition)
            }
            _ => None,
        })
        .ok_or(RealtimeTranscriptShellError {
            op: "user_content_identity_resolved",
        })
}

fn resolve_realtime_user_content_blob_stage(
    pending_present: bool,
    pending_matches_request: bool,
) -> Result<RealtimeUserContentBlobStageDisposition, RealtimeTranscriptShellError> {
    let mut authority = document_authority();
    authority
        .resolve_realtime_user_content_blob_stage(pending_present, pending_matches_request)?
        .into_iter()
        .find_map(|effect| match effect {
            SessionDocumentEffect::RealtimeUserContentBlobStageResolved { disposition } => {
                Some(disposition)
            }
            _ => None,
        })
        .ok_or(RealtimeTranscriptShellError {
            op: "user_content_blob_stage_resolved",
        })
}

fn resolve_realtime_user_content_blob_recovery(
    pending_present: bool,
    request_matches_pending: bool,
    pending_blob_valid: bool,
) -> Result<RealtimeUserContentBlobRecoveryDisposition, RealtimeTranscriptShellError> {
    let mut authority = document_authority();
    authority
        .resolve_realtime_user_content_blob_recovery(
            pending_present,
            request_matches_pending,
            pending_blob_valid,
        )?
        .into_iter()
        .find_map(|effect| match effect {
            SessionDocumentEffect::RealtimeUserContentBlobRecoveryResolved { disposition } => {
                Some(disposition)
            }
            _ => None,
        })
        .ok_or(RealtimeTranscriptShellError {
            op: "user_content_blob_recovery_resolved",
        })
}

fn resolve_realtime_user_content_blob_finalize(
    pending_present: bool,
    pending_matches_committed: bool,
) -> Result<RealtimeUserContentBlobFinalizeDisposition, RealtimeTranscriptShellError> {
    let mut authority = document_authority();
    authority
        .resolve_realtime_user_content_blob_finalize(pending_present, pending_matches_committed)?
        .into_iter()
        .find_map(|effect| match effect {
            SessionDocumentEffect::RealtimeUserContentBlobFinalizeResolved { disposition } => {
                Some(disposition)
            }
            _ => None,
        })
        .ok_or(RealtimeTranscriptShellError {
            op: "user_content_blob_finalize_resolved",
        })
}

// ---------------------------------------------------------------------------
// Per-session registry storage (MECHANISM).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionRealtimeTranscriptState {
    #[serde(default)]
    items: BTreeMap<String, RealtimeTranscriptItemState>,
    #[serde(default)]
    first_seen_order: Vec<String>,
    #[serde(default)]
    seen_delta_ids: BTreeSet<String>,
    #[serde(default)]
    assistant_completions: BTreeMap<String, RealtimeAssistantCompletion>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    discarded_assistant_response_ids: BTreeSet<String>,
    /// Session-scoped idempotency bindings for committed non-text user input.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    user_content_identities: BTreeMap<String, RealtimeUserContentIdentity>,
    /// Durable conflict markers for keys whose canonical image was removed by
    /// a same-session transcript rewrite.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    user_content_tombstones: BTreeSet<RealtimeUserContentTombstone>,
    /// Bounded metadata-only recovery anchor for an image blob whose reducer
    /// commit and durable object are not yet known to be jointly committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_user_content_blob: Option<PendingRealtimeUserContentBlob>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct RealtimeTranscriptItemState {
    role: RealtimeTranscriptRole,
    #[serde(default)]
    previous_item_id: Option<String>,
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    content_segments: BTreeMap<u32, String>,
    /// Typed non-text user content staged until canonical materialization.
    /// Cleared as soon as the user message materializes so inline image bytes
    /// never remain duplicated in durable transcript metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    user_content_segments: BTreeMap<u32, Vec<ContentBlock>>,
    /// Stable replay identities retained after typed blocks move into the
    /// canonical message. This preserves idempotency without retaining a
    /// second copy of media bytes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    user_content_segment_fingerprints: BTreeMap<u32, String>,
    #[serde(default)]
    skipped: bool,
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    materialized: bool,
    #[serde(default)]
    lane: TranscriptLane,
}

impl RealtimeTranscriptItemState {
    fn new(
        role: RealtimeTranscriptRole,
        previous_item_id: Option<String>,
        response_id: Option<String>,
    ) -> Self {
        Self {
            role,
            previous_item_id,
            response_id,
            content_segments: BTreeMap::new(),
            user_content_segments: BTreeMap::new(),
            user_content_segment_fingerprints: BTreeMap::new(),
            skipped: false,
            ready: false,
            materialized: false,
            lane: TranscriptLane::Display,
        }
    }

    fn skipped(previous_item_id: Option<String>) -> Self {
        Self {
            role: RealtimeTranscriptRole::Assistant,
            previous_item_id,
            response_id: None,
            content_segments: BTreeMap::new(),
            user_content_segments: BTreeMap::new(),
            user_content_segment_fingerprints: BTreeMap::new(),
            skipped: true,
            ready: true,
            materialized: false,
            lane: TranscriptLane::Display,
        }
    }

    fn text(&self) -> String {
        self.content_segments.values().cloned().collect()
    }

    fn materializable_content_present(&self) -> bool {
        !self.text().is_empty()
            || self
                .user_content_segments
                .values()
                .any(|blocks| !blocks.is_empty())
    }

    fn user_content_blocks(&self) -> Vec<ContentBlock> {
        let indices = self
            .content_segments
            .keys()
            .chain(self.user_content_segments.keys())
            .copied()
            .collect::<BTreeSet<_>>();
        let mut blocks = Vec::new();
        for index in indices {
            if let Some(content) = self.user_content_segments.get(&index) {
                blocks.extend(content.clone());
            } else if let Some(text) = self.content_segments.get(&index)
                && !text.is_empty()
            {
                blocks.push(ContentBlock::Text { text: text.clone() });
            }
        }
        blocks
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct RealtimeAssistantCompletion {
    stop_reason: StopReason,
    usage: Usage,
    usage_consumed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RealtimeTranscriptApplyCommit {
    pub outcome: RealtimeTranscriptApplyOutcome,
    pub messages: Vec<Message>,
    pub usage: Usage,
}

/// Authorize a durable snapshot through the canonical SessionDocument
/// realtime-transcript restore transition. The shell computes the consistency
/// observations mechanically; the machine decides legality.
pub fn restore_realtime_transcript_state(
    state: SessionRealtimeTranscriptState,
) -> Result<SessionRealtimeTranscriptState, RealtimeTranscriptShellError> {
    let first_seen_ids = state
        .first_seen_order
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let first_seen_unique_count = first_seen_ids.len();
    let causal = realtime_transcript_causal_graph_observations(&state);
    let mut authority = document_authority();
    authority.restore_realtime_transcript_state(
        usize_to_u64(state.items.len()),
        usize_to_u64(state.first_seen_order.len()),
        usize_to_u64(first_seen_unique_count),
        state
            .items
            .keys()
            .all(|item_id| first_seen_ids.contains(item_id)),
        state
            .first_seen_order
            .iter()
            .all(|item_id| state.items.contains_key(item_id)),
        causal.all_materialized_predecessor_references_exist,
        causal.no_self_predecessor_references,
        causal.acyclic,
        causal.all_materialized_items_have_materialized_ancestry,
        realtime_transcript_state_identity_fields_valid(&state),
        realtime_user_content_identity_keys_match(&state),
        realtime_user_content_identity_fields_valid(&state),
        realtime_user_content_identity_item_ids_unique(&state),
        realtime_user_content_identities_reference_materialized_user_items(&state),
        realtime_user_content_tombstones_valid(&state),
        realtime_user_content_identities_and_tombstones_disjoint(&state),
        realtime_pending_user_content_blob_fields_valid(&state),
        realtime_pending_user_content_blob_is_uncommitted(&state),
        realtime_transcript_delta_ids_valid(&state),
        realtime_transcript_completion_ids_valid(&state),
        realtime_transcript_discarded_ids_valid(&state),
        realtime_transcript_materialized_items_were_ready_or_skipped(&state),
        realtime_transcript_assistant_items_have_response_unless_skipped(&state),
        realtime_transcript_ready_assistant_items_have_completion_or_are_skipped(&state),
        realtime_transcript_materialized_assistant_completions_consumed(&state),
        realtime_transcript_completed_assistant_text_items_are_ready_or_materialized_or_skipped(
            &state,
        ),
        realtime_transcript_discarded_assistant_items_are_skipped_or_materialized(&state),
    )?;
    Ok(state)
}

#[must_use]
pub fn pending_realtime_user_content_blob(
    state: &SessionRealtimeTranscriptState,
) -> Option<PendingRealtimeUserContentBlob> {
    state.pending_user_content_blob.clone()
}

pub fn stage_pending_realtime_user_content_blob(
    state: &mut SessionRealtimeTranscriptState,
    pending: PendingRealtimeUserContentBlob,
) -> Result<RealtimeUserContentBlobStageDisposition, RealtimeTranscriptShellError> {
    if !realtime_pending_user_content_blob_value_fields_valid(&pending)
        || !realtime_pending_user_content_blob_value_is_uncommitted(state, &pending)
    {
        return Err(RealtimeTranscriptShellError {
            op: "user_content_blob_stage_invalid",
        });
    }
    let disposition = resolve_realtime_user_content_blob_stage(
        state.pending_user_content_blob.is_some(),
        state.pending_user_content_blob.as_ref() == Some(&pending),
    )?;
    match disposition {
        RealtimeUserContentBlobStageDisposition::StageNew => {
            state.pending_user_content_blob = Some(pending);
        }
        RealtimeUserContentBlobStageDisposition::ReuseExact
        | RealtimeUserContentBlobStageDisposition::RejectOccupied => {}
    }
    Ok(disposition)
}

pub fn resolve_pending_realtime_user_content_blob_recovery(
    state: &SessionRealtimeTranscriptState,
    request: Option<&PendingRealtimeUserContentBlob>,
    pending_blob_valid: bool,
) -> Result<RealtimeUserContentBlobRecoveryDisposition, RealtimeTranscriptShellError> {
    resolve_realtime_user_content_blob_recovery(
        state.pending_user_content_blob.is_some(),
        state
            .pending_user_content_blob
            .as_ref()
            .is_some_and(|pending| request == Some(pending)),
        pending_blob_valid,
    )
}

pub fn clear_invalid_pending_realtime_user_content_blob(
    state: &mut SessionRealtimeTranscriptState,
    request: Option<&PendingRealtimeUserContentBlob>,
) -> Result<(), RealtimeTranscriptShellError> {
    match resolve_pending_realtime_user_content_blob_recovery(state, request, false)? {
        RealtimeUserContentBlobRecoveryDisposition::ClearInvalidBeforeCurrent => {
            state.pending_user_content_blob = None;
            Ok(())
        }
        _ => Err(RealtimeTranscriptShellError {
            op: "user_content_blob_clear_not_authorized",
        }),
    }
}

fn finalize_pending_realtime_user_content_blob(
    state: &mut SessionRealtimeTranscriptState,
    identity: &RealtimeUserContentIdentity,
) -> Result<(), RealtimeTranscriptShellError> {
    let disposition = resolve_realtime_user_content_blob_finalize(
        state.pending_user_content_blob.is_some(),
        state
            .pending_user_content_blob
            .as_ref()
            .is_some_and(|pending| pending.matches_identity(identity)),
    )?;
    match disposition {
        RealtimeUserContentBlobFinalizeDisposition::ClearCommitted => {
            state.pending_user_content_blob = None;
            Ok(())
        }
        RealtimeUserContentBlobFinalizeDisposition::NoPending => Ok(()),
        RealtimeUserContentBlobFinalizeDisposition::RejectMismatch => {
            Err(RealtimeTranscriptShellError {
                op: "user_content_blob_finalize_mismatch",
            })
        }
    }
}

pub fn apply_realtime_transcript_event(
    state: &mut SessionRealtimeTranscriptState,
    event: RealtimeTranscriptEvent,
) -> Result<RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError> {
    let commit = match event {
        RealtimeTranscriptEvent::ItemObserved {
            item_id,
            previous_item_id,
            role,
            response_id,
        } => {
            let response_id = normalize_realtime_optional_response_id(response_id);
            let response_discarded = role == RealtimeTranscriptRole::Assistant
                && response_id
                    .as_ref()
                    .is_some_and(|id| state.discarded_assistant_response_ids.contains(id));
            let decision = resolve_realtime_event(|authority| {
                authority.resolve_realtime_item_observed(role_kind(role), response_discarded)
            })?;
            apply_item_observation_decision(
                state,
                decision,
                item_id,
                previous_item_id,
                role,
                response_id,
            )?
        }
        RealtimeTranscriptEvent::ItemSkipped {
            item_id,
            previous_item_id,
        } => {
            let decision =
                resolve_realtime_event(|authority| authority.resolve_realtime_item_skipped())?;
            apply_item_observation_decision(
                state,
                decision,
                item_id,
                previous_item_id,
                RealtimeTranscriptRole::Assistant,
                None,
            )?
        }
        RealtimeTranscriptEvent::UserTranscriptFinal {
            item_id,
            previous_item_id,
            content_index,
            text,
        } => apply_user_transcript_final(state, item_id, previous_item_id, content_index, text)?,
        RealtimeTranscriptEvent::UserContentFinal {
            idempotency_key,
            item_id,
            previous_item_id,
            content_index,
            content,
        } => apply_user_content_final(
            state,
            idempotency_key,
            item_id,
            previous_item_id,
            content_index,
            content,
        )?,
        RealtimeTranscriptEvent::AssistantTextDelta {
            response_id,
            delta_id,
            item_id,
            previous_item_id,
            content_index,
            delta,
        } => apply_assistant_delta(
            state,
            response_id,
            delta_id,
            item_id,
            previous_item_id,
            content_index,
            delta,
            TranscriptLane::Display,
        )?,
        RealtimeTranscriptEvent::AssistantTranscriptDelta {
            response_id,
            delta_id,
            item_id,
            previous_item_id,
            content_index,
            delta,
        } => apply_assistant_delta(
            state,
            response_id,
            delta_id,
            item_id,
            previous_item_id,
            content_index,
            delta,
            TranscriptLane::Spoken,
        )?,
        RealtimeTranscriptEvent::AssistantTranscriptTruncated {
            response_id,
            item_id,
            content_index,
            text,
        } => apply_assistant_text_replacement(
            state,
            response_id,
            item_id,
            content_index,
            text,
            "AssistantTranscriptTruncated",
        )?,
        RealtimeTranscriptEvent::AssistantTranscriptFinalText {
            response_id,
            item_id,
            content_index,
            text,
        } => apply_assistant_text_replacement(
            state,
            response_id,
            item_id,
            content_index,
            text,
            "AssistantTranscriptFinalText",
        )?,
        RealtimeTranscriptEvent::AssistantTurnCompleted {
            response_id,
            stop_reason,
            usage,
        } => apply_assistant_turn_completed(state, response_id, stop_reason, usage)?,
        RealtimeTranscriptEvent::AssistantTurnInterrupted { response_id } => {
            apply_assistant_turn_interrupted(state, response_id)?
        }
    };
    Ok(commit)
}

fn apply_item_observation_decision(
    state: &mut SessionRealtimeTranscriptState,
    decision: RealtimeTranscriptEventDecision,
    item_id: String,
    previous_item_id: Option<String>,
    role: RealtimeTranscriptRole,
    response_id: Option<String>,
) -> Result<RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError> {
    if decision.observe_skipped {
        observe_realtime_skipped_item(state, item_id, previous_item_id);
    } else if decision.observe_item {
        observe_realtime_item(state, item_id, previous_item_id, role, response_id);
    }
    finish_realtime_event(state, decision)
}

fn apply_user_transcript_final(
    state: &mut SessionRealtimeTranscriptState,
    item_id: String,
    previous_item_id: Option<String>,
    content_index: u32,
    text: String,
) -> Result<RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError> {
    let existing_segment = state
        .items
        .get(&item_id)
        .and_then(|item| item.content_segments.get(&content_index));
    let segment_empty = existing_segment.is_none_or(String::is_empty);
    let segment_matches = existing_segment.is_some_and(|segment| segment == &text);
    let decision = resolve_realtime_event(|authority| {
        authority.resolve_realtime_user_transcript_final(
            !text.is_empty(),
            segment_empty,
            segment_matches,
        )
    })?;

    if let Some(item) = if decision.observe_item {
        observe_realtime_item(
            state,
            item_id,
            previous_item_id,
            RealtimeTranscriptRole::User,
            None,
        )
    } else {
        None
    } {
        if decision.write_user_segment {
            item.content_segments.insert(content_index, text);
        } else if !text.is_empty() && !segment_empty && !segment_matches {
            tracing::warn!(
                content_index,
                "ignoring conflicting realtime user transcript segment replay"
            );
        }
        if decision.mark_item_ready {
            item.ready = true;
        }
    }
    finish_realtime_event(state, decision)
}

fn apply_user_content_final(
    state: &mut SessionRealtimeTranscriptState,
    idempotency_key: String,
    item_id: String,
    previous_item_id: Option<String>,
    content_index: u32,
    content: Vec<ContentBlock>,
) -> Result<RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError> {
    let identity_image = match content.as_slice() {
        [ContentBlock::Image { media_type, data }] => {
            let blob_id = match data {
                crate::types::ImageData::Inline { data } => {
                    crate::blob::content_blob_id(media_type, data)
                }
                crate::types::ImageData::Blob { blob_id } => blob_id.clone(),
            };
            Some((
                crate::image_generation::MediaType::canonical_str(media_type),
                blob_id,
                data.clone(),
            ))
        }
        _ => None,
    };
    let (media_type, blob_id) = identity_image
        .as_ref()
        .map(|(media_type, blob_id, _)| (media_type.clone(), blob_id.clone()))
        .unwrap_or_else(|| (String::new(), crate::blob::BlobId::new("")));
    let content = identity_image
        .as_ref()
        .map(|(media_type, _, data)| {
            vec![ContentBlock::Image {
                media_type: media_type.clone(),
                data: data.clone(),
            }]
        })
        .unwrap_or(content);
    let existing_identity = state.user_content_identities.get(&idempotency_key).cloned();
    let reducer_blob_backed = identity_image
        .as_ref()
        .is_some_and(|(_, _, data)| matches!(data, crate::types::ImageData::Blob { .. }));
    let pending_matches_candidate =
        state
            .pending_user_content_blob
            .as_ref()
            .is_some_and(|pending| {
                pending
                    == &PendingRealtimeUserContentBlob {
                        idempotency_key: idempotency_key.clone(),
                        item_id: item_id.clone(),
                        previous_item_id: previous_item_id.clone(),
                        content_index,
                        blob_id: blob_id.clone(),
                        media_type: media_type.clone(),
                    }
            });
    let identity_fields_valid = identity_image.is_some()
        && crate::live_adapter::live_image_idempotency_key_is_valid(&idempotency_key)
        && !item_id.trim().is_empty()
        && previous_item_id
            .as_ref()
            .is_none_or(|previous| !previous.trim().is_empty())
        && canonical_supported_image_media_type(&media_type)
        && blob_id.is_canonical_sha256();
    let predecessor_materialized =
        realtime_predecessor_materialized(state, previous_item_id.as_deref());
    let existing_payload_matches = existing_identity.as_ref().is_some_and(|identity| {
        identity.blob_id == blob_id
            && identity.media_type == media_type
            && identity.content_index == content_index
    });
    let disposition = resolve_realtime_user_content_identity(RealtimeUserContentIdentityFacts {
        identity_fields_valid,
        key_tombstoned: realtime_user_content_key_tombstoned(state, &idempotency_key),
        predecessor_materialized,
        existing_identity_present: existing_identity.is_some(),
        existing_payload_matches,
        target_item_id_available: !state.items.contains_key(&item_id),
        reducer_commit_proof_required: true,
        reducer_commit_proof_present: reducer_blob_backed && pending_matches_candidate,
    })?;

    match disposition {
        RealtimeUserContentIdentityDisposition::RejectInvalidIdentity => {
            return Ok(RealtimeTranscriptApplyCommit {
                outcome: RealtimeTranscriptApplyOutcome {
                    user_content: Some(RealtimeUserContentApplyOutcome::RejectedInvalidIdentity {
                        idempotency_key,
                    }),
                    ..RealtimeTranscriptApplyOutcome::default()
                },
                ..RealtimeTranscriptApplyCommit::default()
            });
        }
        RealtimeUserContentIdentityDisposition::RejectUnmaterializedPredecessor => {
            return Ok(RealtimeTranscriptApplyCommit {
                outcome: RealtimeTranscriptApplyOutcome {
                    user_content: Some(
                        RealtimeUserContentApplyOutcome::RejectedUnmaterializedPredecessor {
                            idempotency_key,
                            previous_item_id,
                        },
                    ),
                    ..RealtimeTranscriptApplyOutcome::default()
                },
                ..RealtimeTranscriptApplyCommit::default()
            });
        }
        RealtimeUserContentIdentityDisposition::RejectConflict => {
            return Ok(RealtimeTranscriptApplyCommit {
                outcome: RealtimeTranscriptApplyOutcome {
                    user_content: Some(RealtimeUserContentApplyOutcome::RejectedConflict {
                        idempotency_key,
                    }),
                    ..RealtimeTranscriptApplyOutcome::default()
                },
                ..RealtimeTranscriptApplyCommit::default()
            });
        }
        RealtimeUserContentIdentityDisposition::AlreadyCommitted => {
            let identity = existing_identity.ok_or(RealtimeTranscriptShellError {
                op: "user_content_identity_replay_missing",
            })?;
            finalize_pending_realtime_user_content_blob(state, &identity)?;
            return Ok(RealtimeTranscriptApplyCommit {
                outcome: RealtimeTranscriptApplyOutcome {
                    user_content: Some(RealtimeUserContentApplyOutcome::AlreadyCommitted(identity)),
                    ..RealtimeTranscriptApplyOutcome::default()
                },
                ..RealtimeTranscriptApplyCommit::default()
            });
        }
        RealtimeUserContentIdentityDisposition::CommitNew => {}
    }

    let fingerprint = user_content_fingerprint(&content)?;
    let existing_fingerprint = state
        .items
        .get(&item_id)
        .and_then(|item| item.user_content_segment_fingerprints.get(&content_index));
    let segment_empty = existing_fingerprint.is_none();
    let segment_matches = existing_fingerprint.is_some_and(|existing| existing == &fingerprint);
    let content_present = !content.is_empty();
    let decision = resolve_realtime_event(|authority| {
        authority.resolve_realtime_user_content_final(
            content_present,
            segment_empty,
            segment_matches,
        )
    })?;

    if let Some(item) = if decision.observe_item {
        observe_realtime_item(
            state,
            item_id.clone(),
            previous_item_id.clone(),
            RealtimeTranscriptRole::User,
            None,
        )
    } else {
        None
    } {
        if decision.write_user_segment {
            item.user_content_segments.insert(content_index, content);
            item.user_content_segment_fingerprints
                .insert(content_index, fingerprint);
        } else if content_present && !segment_empty && !segment_matches {
            tracing::warn!(
                content_index,
                "ignoring conflicting realtime user content segment replay"
            );
        }
        if decision.mark_item_ready {
            item.ready = true;
        }
    }
    let mut commit = finish_realtime_event(state, decision)?;
    let materialized = state
        .items
        .get(&item_id)
        .is_some_and(|item| item.role == RealtimeTranscriptRole::User && item.materialized);
    if !materialized {
        return Err(RealtimeTranscriptShellError {
            op: "user_content_commit_not_materialized",
        });
    }
    let identity = RealtimeUserContentIdentity {
        idempotency_key: idempotency_key.clone(),
        item_id,
        previous_item_id,
        content_index,
        blob_id,
        media_type,
    };
    state
        .user_content_identities
        .insert(idempotency_key, identity.clone());
    finalize_pending_realtime_user_content_blob(state, &identity)?;
    commit.outcome.user_content = Some(RealtimeUserContentApplyOutcome::Committed(identity));
    Ok(commit)
}

/// Return the durable user-content idempotency registry in deterministic key
/// order for provider-session reconstruction.
#[must_use]
pub fn realtime_user_content_identities(
    state: &SessionRealtimeTranscriptState,
) -> Vec<RealtimeUserContentIdentity> {
    state.user_content_identities.values().cloned().collect()
}

/// Return durable removed-key markers in deterministic order for provider
/// open/reconnect/refresh admission.
#[must_use]
pub fn realtime_user_content_tombstones(
    state: &SessionRealtimeTranscriptState,
) -> Vec<RealtimeUserContentTombstone> {
    state.user_content_tombstones.iter().cloned().collect()
}

/// Reconcile realtime transcript metadata after a canonical same-session
/// history rewrite.
///
/// The rewritten messages are the sole authority for whether an old committed
/// image still exists. Matching is an ordered, deterministic multiset over
/// canonical `(media_type, blob_id)` identities, so one image occurrence can
/// retain at most one caller key. Retained bindings are rebased into a fresh
/// materialized user-item projection; removed bindings become durable
/// tombstones. All response/delta/causal staging from the pre-rewrite history
/// is discarded.
pub fn reconcile_realtime_transcript_state_after_rewrite(
    mut state: SessionRealtimeTranscriptState,
    canonical_messages: &[Message],
) -> Result<SessionRealtimeTranscriptState, RealtimeTranscriptShellError> {
    if state.pending_user_content_blob.is_some() {
        return Err(RealtimeTranscriptShellError {
            op: "history_rewrite_pending_user_content_blob",
        });
    }
    let mut canonical_images = canonical_user_image_multiset(canonical_messages);
    let previous_identities = std::mem::take(&mut state.user_content_identities);
    let mut retained_identities = BTreeMap::new();
    let mut rebuilt_items = BTreeMap::new();
    let mut rebuilt_order = Vec::new();

    for (key, mut identity) in previous_identities {
        let image_key = (identity.media_type.clone(), identity.blob_id.clone());
        let already_tombstoned = realtime_user_content_key_tombstoned(&state, &key);
        let retained = !already_tombstoned
            && canonical_images
                .get_mut(&image_key)
                .is_some_and(|remaining| {
                    if *remaining == 0 {
                        false
                    } else {
                        *remaining -= 1;
                        true
                    }
                });
        if !retained {
            state
                .user_content_tombstones
                .insert(RealtimeUserContentTombstone {
                    idempotency_key: key,
                });
            continue;
        }

        // A rewrite creates a new causal projection. Provider-native
        // predecessor ids from the old projection are not authoritative in
        // the rewritten history, so retained canonical images become roots.
        identity.previous_item_id = None;
        let expected_content = vec![ContentBlock::Image {
            media_type: identity.media_type.clone(),
            data: crate::types::ImageData::Blob {
                blob_id: identity.blob_id.clone(),
            },
        }];
        let fingerprint = user_content_fingerprint(&expected_content)?;
        let mut item = RealtimeTranscriptItemState::new(RealtimeTranscriptRole::User, None, None);
        item.user_content_segment_fingerprints
            .insert(identity.content_index, fingerprint);
        item.ready = true;
        item.materialized = true;
        rebuilt_order.push(identity.item_id.clone());
        rebuilt_items.insert(identity.item_id.clone(), item);
        retained_identities.insert(key, identity);
    }

    state.items = rebuilt_items;
    state.first_seen_order = rebuilt_order;
    state.seen_delta_ids.clear();
    state.assistant_completions.clear();
    state.discarded_assistant_response_ids.clear();
    state.user_content_identities = retained_identities;
    restore_realtime_transcript_state(state)
}

fn canonical_user_image_multiset(
    messages: &[Message],
) -> BTreeMap<(String, crate::blob::BlobId), usize> {
    let mut images = BTreeMap::new();
    for message in messages {
        let Message::User(user) = message else {
            continue;
        };
        for block in &user.content {
            let ContentBlock::Image { media_type, data } = block else {
                continue;
            };
            let media_type = crate::image_generation::MediaType::canonical_str(media_type);
            let blob_id = match data {
                crate::types::ImageData::Inline { data } => {
                    crate::blob::content_blob_id(&media_type, data)
                }
                crate::types::ImageData::Blob { blob_id } => blob_id.clone(),
            };
            *images.entry((media_type, blob_id)).or_insert(0) += 1;
        }
    }
    images
}

fn realtime_user_content_key_tombstoned(
    state: &SessionRealtimeTranscriptState,
    idempotency_key: &str,
) -> bool {
    state
        .user_content_tombstones
        .iter()
        .any(|tombstone| tombstone.idempotency_key == idempotency_key)
}

/// Resolve expected replay/rejection outcomes without mutating transcript
/// state. Persistent services use this before writing a new blob so conflict,
/// malformed identity, and unmaterialized-predecessor attempts cannot create
/// orphan storage. `None` means the event is a valid new commit and must flow
/// through the normal externalize + reducer path.
pub fn preflight_realtime_user_content_event(
    state: &SessionRealtimeTranscriptState,
    event: &RealtimeTranscriptEvent,
) -> Result<Option<RealtimeUserContentApplyOutcome>, RealtimeTranscriptShellError> {
    let RealtimeTranscriptEvent::UserContentFinal {
        idempotency_key,
        item_id,
        previous_item_id,
        content_index,
        content,
    } = event
    else {
        return Ok(None);
    };
    let identity_image = match content.as_slice() {
        [ContentBlock::Image { media_type, data }] => {
            let blob_id = match data {
                crate::types::ImageData::Inline { data } => {
                    crate::blob::content_blob_id(media_type, data)
                }
                crate::types::ImageData::Blob { blob_id } => blob_id.clone(),
            };
            Some((
                crate::image_generation::MediaType::canonical_str(media_type),
                blob_id,
            ))
        }
        _ => None,
    };
    let (media_type, blob_id) = identity_image
        .clone()
        .unwrap_or_else(|| (String::new(), crate::blob::BlobId::new("")));
    let existing_identity = state.user_content_identities.get(idempotency_key).cloned();
    let identity_fields_valid = identity_image.is_some()
        && crate::live_adapter::live_image_idempotency_key_is_valid(idempotency_key)
        && !item_id.trim().is_empty()
        && previous_item_id
            .as_ref()
            .is_none_or(|previous| !previous.trim().is_empty())
        && canonical_supported_image_media_type(&media_type)
        && blob_id.is_canonical_sha256();
    let existing_payload_matches = existing_identity.as_ref().is_some_and(|identity| {
        identity.blob_id == blob_id
            && identity.media_type == media_type
            && identity.content_index == *content_index
    });
    let disposition = resolve_realtime_user_content_identity(RealtimeUserContentIdentityFacts {
        identity_fields_valid,
        key_tombstoned: realtime_user_content_key_tombstoned(state, idempotency_key),
        predecessor_materialized: realtime_predecessor_materialized(
            state,
            previous_item_id.as_deref(),
        ),
        existing_identity_present: existing_identity.is_some(),
        existing_payload_matches,
        target_item_id_available: !state.items.contains_key(item_id),
        reducer_commit_proof_required: false,
        reducer_commit_proof_present: false,
    })?;
    Ok(match disposition {
        RealtimeUserContentIdentityDisposition::CommitNew => None,
        RealtimeUserContentIdentityDisposition::AlreadyCommitted => {
            Some(RealtimeUserContentApplyOutcome::AlreadyCommitted(
                existing_identity.ok_or(RealtimeTranscriptShellError {
                    op: "user_content_preflight_replay_missing",
                })?,
            ))
        }
        RealtimeUserContentIdentityDisposition::RejectInvalidIdentity => {
            Some(RealtimeUserContentApplyOutcome::RejectedInvalidIdentity {
                idempotency_key: idempotency_key.clone(),
            })
        }
        RealtimeUserContentIdentityDisposition::RejectUnmaterializedPredecessor => Some(
            RealtimeUserContentApplyOutcome::RejectedUnmaterializedPredecessor {
                idempotency_key: idempotency_key.clone(),
                previous_item_id: previous_item_id.clone(),
            },
        ),
        RealtimeUserContentIdentityDisposition::RejectConflict => {
            Some(RealtimeUserContentApplyOutcome::RejectedConflict {
                idempotency_key: idempotency_key.clone(),
            })
        }
    })
}

fn user_content_fingerprint(
    content: &[ContentBlock],
) -> Result<String, RealtimeTranscriptShellError> {
    let canonical = content
        .iter()
        .map(|block| match block {
            ContentBlock::Image {
                media_type,
                data: crate::types::ImageData::Inline { data },
            } => ContentBlock::Image {
                media_type: crate::image_generation::MediaType::canonical_str(media_type),
                data: crate::types::ImageData::Blob {
                    blob_id: crate::blob::content_blob_id(media_type, data),
                },
            },
            ContentBlock::Image { media_type, data } => ContentBlock::Image {
                media_type: crate::image_generation::MediaType::canonical_str(media_type),
                data: data.clone(),
            },
            other => other.clone(),
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&canonical).map_err(|_| RealtimeTranscriptShellError {
        op: "user_content_fingerprint",
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

#[allow(clippy::too_many_arguments)]
fn apply_assistant_delta(
    state: &mut SessionRealtimeTranscriptState,
    response_id: String,
    delta_id: String,
    item_id: String,
    previous_item_id: Option<String>,
    content_index: u32,
    delta: String,
    requested_lane: TranscriptLane,
) -> Result<RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError> {
    let response_id = normalize_realtime_response_id(response_id);
    let delta_id_present = !delta_id.trim().is_empty();
    let delta_id_seen = delta_id_present && state.seen_delta_ids.contains(&delta_id);
    let response_discarded = response_id
        .as_ref()
        .is_some_and(|id| state.discarded_assistant_response_ids.contains(id));
    let item_has_text = state
        .items
        .get(&item_id)
        .is_some_and(|item| !item.text().is_empty());
    let current_lane = state
        .items
        .get(&item_id)
        .map(|item| item.lane)
        .unwrap_or_default();
    let text_after_write_present = item_has_text || !delta.is_empty();
    let response_completed = response_id
        .as_ref()
        .is_some_and(|id| state.assistant_completions.contains_key(id));
    let decision = resolve_realtime_event(|authority| {
        authority.resolve_realtime_assistant_delta(
            response_id.is_some(),
            response_discarded,
            delta_id_present,
            delta_id_seen,
            item_has_text,
            lane_kind(current_lane),
            lane_kind(requested_lane),
            response_completed,
            text_after_write_present,
        )
    })?;

    if decision.observe_skipped {
        observe_realtime_skipped_item(state, item_id, previous_item_id);
    } else if let Some(response_id) = response_id {
        if decision.record_delta_id {
            state.seen_delta_ids.insert(delta_id);
        }
        let item = if decision.observe_item {
            observe_realtime_item(
                state,
                item_id,
                previous_item_id,
                RealtimeTranscriptRole::Assistant,
                Some(response_id),
            )
        } else {
            None
        };
        if let Some(item) = item {
            if decision.promote_lane {
                item.lane = requested_lane;
            }
            if decision.append_assistant_segment {
                item.content_segments
                    .entry(content_index)
                    .or_default()
                    .push_str(&delta);
            } else if decision.observe_item && current_lane != requested_lane && item_has_text {
                tracing::warn!(
                    "realtime assistant delta routed to an incompatible transcript lane; dropping delta to preserve generated lane decision"
                );
            }
            if decision.mark_item_ready {
                item.ready = true;
            }
        }
    }
    finish_realtime_event(state, decision)
}

fn apply_assistant_text_replacement(
    state: &mut SessionRealtimeTranscriptState,
    response_id: String,
    item_id: String,
    content_index: u32,
    text: String,
    op: &'static str,
) -> Result<RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError> {
    let response_id = normalize_realtime_response_id(response_id);
    let response_discarded = response_id
        .as_ref()
        .is_some_and(|id| state.discarded_assistant_response_ids.contains(id));
    let item_has_text = state
        .items
        .get(&item_id)
        .is_some_and(|item| !item.text().is_empty());
    let current_lane = state
        .items
        .get(&item_id)
        .map(|item| item.lane)
        .unwrap_or_default();
    let item_materialized = state
        .items
        .get(&item_id)
        .is_some_and(|item| item.materialized);
    let text_after_replace_present =
        text_after_replacing_segment_present(state.items.get(&item_id), content_index, &text);
    let response_completed = response_id
        .as_ref()
        .is_some_and(|id| state.assistant_completions.contains_key(id));
    let decision = resolve_realtime_event(|authority| {
        authority.resolve_realtime_assistant_text_replacement(
            response_id.is_some(),
            response_discarded,
            item_materialized,
            item_has_text,
            lane_kind(current_lane),
            lane_kind(TranscriptLane::Spoken),
            response_completed,
            text_after_replace_present,
        )
    })?;

    if decision.observe_skipped {
        observe_realtime_skipped_item(state, item_id, None);
    } else if let Some(response_id) = response_id {
        let item_id_for_log = item_id.clone();
        let item = if decision.observe_item {
            observe_realtime_item(
                state,
                item_id,
                None,
                RealtimeTranscriptRole::Assistant,
                Some(response_id.clone()),
            )
        } else {
            None
        };
        if let Some(item) = item {
            if item_materialized {
                tracing::warn!(
                    target: "meerkat::session",
                    item_id = %item_id_for_log,
                    response_id = %response_id,
                    "{op} arrived after item already materialized; canonical message is locked, late replacement dropped",
                );
            } else if decision.promote_lane {
                item.lane = TranscriptLane::Spoken;
            } else if decision.observe_item
                && current_lane != TranscriptLane::Spoken
                && item_has_text
            {
                tracing::warn!(
                    "{op} routed to a Display-lane item; dropping replacement to preserve generated lane decision"
                );
            }
            if decision.replace_assistant_segment {
                item.content_segments.insert(content_index, text);
            }
            if decision.mark_item_ready {
                item.ready = true;
            }
        }
    }
    finish_realtime_event(state, decision)
}

fn apply_assistant_turn_completed(
    state: &mut SessionRealtimeTranscriptState,
    response_id: String,
    stop_reason: StopReason,
    usage: Usage,
) -> Result<RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError> {
    let response_id = normalize_realtime_response_id(response_id);
    let response_discarded = response_id
        .as_ref()
        .is_some_and(|id| state.discarded_assistant_response_ids.contains(id));
    let decision = resolve_realtime_event(|authority| {
        authority.resolve_realtime_assistant_turn_completed(
            response_id.is_some(),
            response_discarded,
            stop_reason_kind(stop_reason),
        )
    })?;

    if let Some(response_id) = response_id {
        if decision.discard_response {
            discard_realtime_assistant_response(state, &response_id);
        }
        if decision.remove_completion {
            state.assistant_completions.remove(&response_id);
        }
        if decision.record_completion {
            state
                .assistant_completions
                .entry(response_id.clone())
                .or_insert(RealtimeAssistantCompletion {
                    stop_reason,
                    usage,
                    usage_consumed: false,
                });
        }
        if decision.mark_response_ready {
            mark_realtime_assistant_response_ready(state, &response_id);
        }
    }
    finish_realtime_event(state, decision)
}

fn apply_assistant_turn_interrupted(
    state: &mut SessionRealtimeTranscriptState,
    response_id: String,
) -> Result<RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError> {
    let response_id = normalize_realtime_response_id(response_id);
    let decision = resolve_realtime_event(|authority| {
        authority.resolve_realtime_assistant_turn_interrupted(response_id.is_some())
    })?;

    if let Some(response_id) = response_id {
        if decision.discard_response_by_lane {
            discard_realtime_assistant_response_by_lane(state, &response_id);
        }
        if decision.record_completion {
            state
                .assistant_completions
                .entry(response_id.clone())
                .or_insert(RealtimeAssistantCompletion {
                    stop_reason: StopReason::Cancelled,
                    usage: Usage::default(),
                    usage_consumed: false,
                });
        }
        if decision.mark_response_ready {
            mark_realtime_assistant_response_ready(state, &response_id);
        }
    }
    finish_realtime_event(state, decision)
}

fn finish_realtime_event(
    state: &mut SessionRealtimeTranscriptState,
    decision: RealtimeTranscriptEventDecision,
) -> Result<RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError> {
    if decision.materialize_ready_items {
        materialize_realtime_transcript_ready_items(state)
    } else {
        Ok(RealtimeTranscriptApplyCommit::default())
    }
}

#[must_use]
pub fn in_flight_realtime_assistant_response_ids(
    state: &SessionRealtimeTranscriptState,
) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for item_id in &state.first_seen_order {
        let Some(item) = state.items.get(item_id) else {
            continue;
        };
        if item.role != RealtimeTranscriptRole::Assistant {
            continue;
        }
        if item.materialized || item.skipped {
            continue;
        }
        let Some(response_id) = item.response_id.as_ref() else {
            continue;
        };
        if state.discarded_assistant_response_ids.contains(response_id) {
            continue;
        }
        if seen.insert(response_id.clone()) {
            out.push(response_id.clone());
        }
    }
    out
}

fn materialize_realtime_transcript_ready_items(
    state: &mut SessionRealtimeTranscriptState,
) -> Result<RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError> {
    let mut materialized = Vec::new();
    let mut messages = Vec::new();
    let mut committed_usage = Usage::default();
    let mut pending_blocks: Vec<AssistantBlock> = Vec::new();
    let mut pending_response_id: Option<String> = None;
    let mut pending_stop_reason: StopReason = StopReason::EndTurn;
    let mut pending_usage: Usage = Usage::default();

    loop {
        let order = realtime_transcript_order(state);
        let mut skipped_batch = Vec::new();
        let mut batch = Vec::new();
        for item_id in order {
            let Some(item) = state.items.get(&item_id) else {
                continue;
            };
            let text = item.text();
            let completion = item
                .response_id
                .as_ref()
                .and_then(|response_id| state.assistant_completions.get(response_id));
            let decision = resolve_materialize_candidate(
                state,
                item,
                item.materializable_content_present(),
                completion,
            )?;
            match decision.decision {
                RealtimeTranscriptMaterializeDecision::Wait => {}
                RealtimeTranscriptMaterializeDecision::MarkSkipped => {
                    skipped_batch.push(item_id.clone());
                }
                RealtimeTranscriptMaterializeDecision::MaterializeUser => {
                    batch.push(ResolvedMaterialization::User {
                        item_id: item_id.clone(),
                        content: item.user_content_blocks(),
                    });
                }
                RealtimeTranscriptMaterializeDecision::MaterializeAssistant => {
                    let Some(response_id) = item.response_id.as_ref() else {
                        continue;
                    };
                    let Some(completion) = completion else {
                        continue;
                    };
                    let usage = if decision.consume_usage {
                        completion.usage.clone()
                    } else {
                        Usage::default()
                    };
                    batch.push(ResolvedMaterialization::Assistant {
                        item_id: item_id.clone(),
                        response_id: response_id.clone(),
                        text,
                        stop_reason: completion.stop_reason,
                        usage,
                        lane: item.lane,
                        consume_usage: decision.consume_usage,
                    });
                }
            }
        }
        if skipped_batch.is_empty() && batch.is_empty() {
            break;
        }
        for item_id in skipped_batch {
            if let Some(item) = state.items.get_mut(&item_id) {
                item.materialized = true;
            }
        }

        for resolved in batch {
            match resolved {
                ResolvedMaterialization::User { item_id, content } => {
                    flush_pending_assistant_blocks(
                        &mut messages,
                        &mut committed_usage,
                        &mut pending_blocks,
                        pending_stop_reason,
                        &mut pending_usage,
                    );
                    pending_response_id = None;
                    if let Some(item) = state.items.get_mut(&item_id) {
                        item.materialized = true;
                        item.user_content_segments.clear();
                    }
                    let text = ContentInput::Blocks(content.clone()).text_content();
                    messages.push(Message::User(UserMessage::with_blocks(content)));
                    materialized
                        .push(RealtimeTranscriptMaterializedMessage::User { item_id, text });
                }
                ResolvedMaterialization::Assistant {
                    item_id,
                    response_id,
                    text,
                    stop_reason,
                    usage,
                    lane,
                    consume_usage,
                } => {
                    if pending_response_id
                        .as_ref()
                        .is_some_and(|existing| existing != &response_id)
                        && !pending_blocks.is_empty()
                    {
                        flush_pending_assistant_blocks(
                            &mut messages,
                            &mut committed_usage,
                            &mut pending_blocks,
                            pending_stop_reason,
                            &mut pending_usage,
                        );
                        pending_response_id = None;
                    }
                    if let Some(item) = state.items.get_mut(&item_id) {
                        item.materialized = true;
                    }
                    if consume_usage
                        && let Some(completion) = state.assistant_completions.get_mut(&response_id)
                    {
                        completion.usage_consumed = true;
                    }
                    let block = match lane {
                        TranscriptLane::Display => AssistantBlock::Text {
                            text: text.clone(),
                            meta: None,
                        },
                        TranscriptLane::Spoken => AssistantBlock::Transcript {
                            text: text.clone(),
                            source: TranscriptSource::Spoken,
                            meta: None,
                        },
                    };
                    if pending_response_id.is_none() {
                        pending_response_id = Some(response_id.clone());
                        pending_stop_reason = stop_reason;
                        pending_usage = usage.clone();
                    }
                    pending_blocks.push(block);
                    materialized.push(RealtimeTranscriptMaterializedMessage::Assistant {
                        item_id,
                        response_id,
                        text,
                        stop_reason,
                        usage,
                        lane,
                    });
                }
            }
        }
    }

    flush_pending_assistant_blocks(
        &mut messages,
        &mut committed_usage,
        &mut pending_blocks,
        pending_stop_reason,
        &mut pending_usage,
    );

    Ok(RealtimeTranscriptApplyCommit {
        outcome: RealtimeTranscriptApplyOutcome {
            materialized_messages: materialized,
            ..RealtimeTranscriptApplyOutcome::default()
        },
        messages,
        usage: committed_usage,
    })
}

enum ResolvedMaterialization {
    User {
        item_id: String,
        content: Vec<ContentBlock>,
    },
    Assistant {
        item_id: String,
        response_id: String,
        text: String,
        stop_reason: StopReason,
        usage: Usage,
        lane: TranscriptLane,
        consume_usage: bool,
    },
}

fn flush_pending_assistant_blocks(
    messages: &mut Vec<Message>,
    committed_usage: &mut Usage,
    pending_blocks: &mut Vec<AssistantBlock>,
    pending_stop_reason: StopReason,
    pending_usage: &mut Usage,
) {
    if pending_blocks.is_empty() {
        *pending_usage = Usage::default();
        return;
    }
    let blocks = std::mem::take(pending_blocks);
    messages.push(Message::BlockAssistant(BlockAssistantMessage::new(
        blocks,
        pending_stop_reason,
    )));
    if *pending_usage != Usage::default() {
        committed_usage.add(pending_usage);
        *pending_usage = Usage::default();
    }
}

fn observe_realtime_item(
    state: &mut SessionRealtimeTranscriptState,
    item_id: String,
    previous_item_id: Option<String>,
    role: RealtimeTranscriptRole,
    response_id: Option<String>,
) -> Option<&mut RealtimeTranscriptItemState> {
    if item_id.trim().is_empty() {
        return None;
    }
    if !state.items.contains_key(&item_id) {
        state.first_seen_order.push(item_id.clone());
        state.items.insert(
            item_id.clone(),
            RealtimeTranscriptItemState::new(role, previous_item_id, response_id),
        );
    } else if let Some(item) = state.items.get_mut(&item_id) {
        if item.previous_item_id.is_none() {
            item.previous_item_id = previous_item_id;
        }
        if item.response_id.is_none() {
            item.response_id = response_id;
        }
        if item.role != role {
            tracing::warn!("ignoring conflicting realtime item role replay");
        }
    }
    state.items.get_mut(&item_id)
}

fn observe_realtime_skipped_item(
    state: &mut SessionRealtimeTranscriptState,
    item_id: String,
    previous_item_id: Option<String>,
) {
    if item_id.trim().is_empty() {
        return;
    }
    if !state.items.contains_key(&item_id) {
        state.first_seen_order.push(item_id.clone());
        state.items.insert(
            item_id,
            RealtimeTranscriptItemState::skipped(previous_item_id),
        );
    } else if let Some(item) = state.items.get_mut(&item_id) {
        if item.previous_item_id.is_none() {
            item.previous_item_id = previous_item_id;
        }
        item.skipped = true;
        item.ready = true;
    }
}

fn mark_realtime_assistant_response_ready(
    state: &mut SessionRealtimeTranscriptState,
    response_id: &str,
) {
    for item in state.items.values_mut() {
        if item.role == RealtimeTranscriptRole::Assistant
            && item.response_id.as_deref() == Some(response_id)
            && !item.text().is_empty()
        {
            item.ready = true;
        }
    }
}

fn discard_realtime_assistant_response(
    state: &mut SessionRealtimeTranscriptState,
    response_id: &str,
) {
    state
        .discarded_assistant_response_ids
        .insert(response_id.to_string());
    for item in state.items.values_mut() {
        if item.role == RealtimeTranscriptRole::Assistant
            && item.response_id.as_deref() == Some(response_id)
            && !item.materialized
        {
            item.skipped = true;
            item.ready = true;
        }
    }
}

fn discard_realtime_assistant_response_by_lane(
    state: &mut SessionRealtimeTranscriptState,
    response_id: &str,
) {
    state
        .discarded_assistant_response_ids
        .insert(response_id.to_string());
    for item in state.items.values_mut() {
        if item.role != RealtimeTranscriptRole::Assistant
            || item.response_id.as_deref() != Some(response_id)
            || item.materialized
        {
            continue;
        }
        let has_content = item
            .content_segments
            .values()
            .any(|segment| !segment.is_empty());
        if item.lane == TranscriptLane::Display && has_content {
            continue;
        }
        item.content_segments.clear();
        item.skipped = true;
        item.ready = true;
    }
}

fn realtime_transcript_order(state: &SessionRealtimeTranscriptState) -> Vec<String> {
    let mut out = Vec::new();
    let mut emitted = BTreeSet::new();
    loop {
        let mut progressed = false;
        for item_id in &state.first_seen_order {
            if emitted.contains(item_id) {
                continue;
            }
            let Some(item) = state.items.get(item_id) else {
                continue;
            };
            if let Some(previous_item_id) = item.previous_item_id.as_ref()
                && (!state.items.contains_key(previous_item_id)
                    || !emitted.contains(previous_item_id))
            {
                continue;
            }
            emitted.insert(item_id.clone());
            out.push(item_id.clone());
            progressed = true;
        }
        if !progressed {
            break;
        }
        if emitted.len() >= state.items.len() {
            break;
        }
    }
    out
}

fn realtime_predecessor_materialized(
    state: &SessionRealtimeTranscriptState,
    previous_item_id: Option<&str>,
) -> bool {
    match previous_item_id {
        None => true,
        Some(previous_item_id) => state
            .items
            .get(previous_item_id)
            .is_some_and(|item| item.materialized),
    }
}

fn text_after_replacing_segment_present(
    item: Option<&RealtimeTranscriptItemState>,
    content_index: u32,
    text: &str,
) -> bool {
    if !text.is_empty() {
        return true;
    }
    item.is_some_and(|item| {
        item.content_segments
            .iter()
            .any(|(index, segment)| *index != content_index && !segment.is_empty())
    })
}

fn normalize_realtime_response_id(response_id: String) -> Option<String> {
    let trimmed = response_id.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_realtime_optional_response_id(response_id: Option<String>) -> Option<String> {
    response_id.and_then(normalize_realtime_response_id)
}

fn role_kind(role: RealtimeTranscriptRole) -> RealtimeTranscriptRoleKind {
    match role {
        RealtimeTranscriptRole::User => RealtimeTranscriptRoleKind::User,
        RealtimeTranscriptRole::Assistant => RealtimeTranscriptRoleKind::Assistant,
    }
}

fn lane_kind(lane: TranscriptLane) -> RealtimeTranscriptLaneKind {
    match lane {
        TranscriptLane::Display => RealtimeTranscriptLaneKind::Display,
        TranscriptLane::Spoken => RealtimeTranscriptLaneKind::Spoken,
    }
}

fn stop_reason_kind(stop_reason: StopReason) -> RealtimeTranscriptStopReasonKind {
    match stop_reason {
        StopReason::Cancelled => RealtimeTranscriptStopReasonKind::Cancelled,
        StopReason::ToolUse => RealtimeTranscriptStopReasonKind::ToolUse,
        _ => RealtimeTranscriptStopReasonKind::Other,
    }
}

fn realtime_transcript_state_identity_fields_valid(state: &SessionRealtimeTranscriptState) -> bool {
    state.items.iter().all(|(item_id, item)| {
        !item_id.trim().is_empty()
            && item
                .previous_item_id
                .as_ref()
                .is_none_or(|previous| !previous.trim().is_empty())
            && item
                .response_id
                .as_ref()
                .is_none_or(|response| !response.trim().is_empty())
    })
}

#[derive(Debug, Clone, Copy)]
struct RealtimeTranscriptCausalGraphObservations {
    all_materialized_predecessor_references_exist: bool,
    no_self_predecessor_references: bool,
    acyclic: bool,
    all_materialized_items_have_materialized_ancestry: bool,
}

/// Compute every causal-graph restore observation in one bounded Kahn walk.
/// Each node and predecessor edge is visited a constant number of times; a
/// long valid transcript cannot trigger an independent ancestry walk per item.
fn realtime_transcript_causal_graph_observations(
    state: &SessionRealtimeTranscriptState,
) -> RealtimeTranscriptCausalGraphObservations {
    let mut indegree = HashMap::with_capacity(state.items.len());
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for item_id in state.items.keys() {
        indegree.insert(item_id.as_str(), 0_u8);
    }

    let mut all_materialized_predecessor_references_exist = true;
    let mut no_self_predecessor_references = true;
    let mut all_materialized_items_have_materialized_ancestry = true;
    for (item_id, item) in &state.items {
        let Some(previous_item_id) = item.previous_item_id.as_deref() else {
            continue;
        };
        if previous_item_id == item_id {
            no_self_predecessor_references = false;
        }
        let Some(previous) = state.items.get(previous_item_id) else {
            if item.materialized {
                all_materialized_predecessor_references_exist = false;
                all_materialized_items_have_materialized_ancestry = false;
            }
            continue;
        };
        if let Some(degree) = indegree.get_mut(item_id.as_str()) {
            *degree = 1;
        }
        children
            .entry(previous_item_id)
            .or_default()
            .push(item_id.as_str());
        // Immediate predecessor materialization is sufficient for the
        // transitive property because every materialized predecessor is
        // subject to the same observation.
        if item.materialized && !previous.materialized {
            all_materialized_items_have_materialized_ancestry = false;
        }
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(item_id, degree)| (*degree == 0).then_some(*item_id))
        .collect::<VecDeque<_>>();
    let mut visited = 0_usize;
    while let Some(item_id) = ready.pop_front() {
        visited = visited.saturating_add(1);
        if let Some(item_children) = children.get(item_id) {
            for child in item_children {
                if let Some(degree) = indegree.get_mut(child) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        ready.push_back(child);
                    }
                }
            }
        }
    }

    RealtimeTranscriptCausalGraphObservations {
        all_materialized_predecessor_references_exist,
        no_self_predecessor_references,
        acyclic: visited == state.items.len(),
        all_materialized_items_have_materialized_ancestry,
    }
}

fn realtime_user_content_identity_keys_match(state: &SessionRealtimeTranscriptState) -> bool {
    state
        .user_content_identities
        .iter()
        .all(|(key, identity)| key == &identity.idempotency_key)
}

fn realtime_user_content_identity_fields_valid(state: &SessionRealtimeTranscriptState) -> bool {
    state.user_content_identities.values().all(|identity| {
        crate::live_adapter::live_image_idempotency_key_is_valid(&identity.idempotency_key)
            && !identity.item_id.trim().is_empty()
            && identity
                .previous_item_id
                .as_ref()
                .is_none_or(|previous| !previous.trim().is_empty())
            && identity.blob_id.is_canonical_sha256()
            && canonical_supported_image_media_type(&identity.media_type)
    })
}

fn realtime_user_content_identity_item_ids_unique(state: &SessionRealtimeTranscriptState) -> bool {
    let item_ids = state
        .user_content_identities
        .values()
        .map(|identity| identity.item_id.as_str())
        .collect::<BTreeSet<_>>();
    item_ids.len() == state.user_content_identities.len()
}

fn realtime_user_content_identities_reference_materialized_user_items(
    state: &SessionRealtimeTranscriptState,
) -> bool {
    state.user_content_identities.values().all(|identity| {
        state.items.get(&identity.item_id).is_some_and(|item| {
            let expected_content = vec![ContentBlock::Image {
                media_type: identity.media_type.clone(),
                data: crate::types::ImageData::Blob {
                    blob_id: identity.blob_id.clone(),
                },
            }];
            let fingerprint_matches =
                user_content_fingerprint(&expected_content).is_ok_and(|expected| {
                    item.user_content_segment_fingerprints
                        .get(&identity.content_index)
                        == Some(&expected)
                });
            item.role == RealtimeTranscriptRole::User
                && item.materialized
                && item.previous_item_id == identity.previous_item_id
                && realtime_predecessor_materialized(state, identity.previous_item_id.as_deref())
                && fingerprint_matches
        })
    })
}

fn realtime_user_content_tombstones_valid(state: &SessionRealtimeTranscriptState) -> bool {
    state.user_content_tombstones.iter().all(|tombstone| {
        crate::live_adapter::live_image_idempotency_key_is_valid(&tombstone.idempotency_key)
    })
}

fn realtime_user_content_identities_and_tombstones_disjoint(
    state: &SessionRealtimeTranscriptState,
) -> bool {
    state.user_content_tombstones.iter().all(|tombstone| {
        !state
            .user_content_identities
            .contains_key(&tombstone.idempotency_key)
    })
}

fn canonical_supported_image_media_type(media_type: &str) -> bool {
    matches!(media_type, "image/png" | "image/jpeg")
        && crate::image_generation::MediaType::canonical_str(media_type) == media_type
}

fn realtime_pending_user_content_blob_value_fields_valid(
    pending: &PendingRealtimeUserContentBlob,
) -> bool {
    crate::live_adapter::live_image_idempotency_key_is_valid(&pending.idempotency_key)
        && !pending.item_id.trim().is_empty()
        && pending
            .previous_item_id
            .as_ref()
            .is_none_or(|previous| !previous.trim().is_empty())
        && pending.blob_id.is_canonical_sha256()
        && canonical_supported_image_media_type(&pending.media_type)
}

fn realtime_pending_user_content_blob_fields_valid(state: &SessionRealtimeTranscriptState) -> bool {
    state
        .pending_user_content_blob
        .as_ref()
        .is_none_or(realtime_pending_user_content_blob_value_fields_valid)
}

fn realtime_pending_user_content_blob_is_uncommitted(
    state: &SessionRealtimeTranscriptState,
) -> bool {
    state
        .pending_user_content_blob
        .as_ref()
        .is_none_or(|pending| {
            realtime_pending_user_content_blob_value_is_uncommitted(state, pending)
        })
}

fn realtime_pending_user_content_blob_value_is_uncommitted(
    state: &SessionRealtimeTranscriptState,
    pending: &PendingRealtimeUserContentBlob,
) -> bool {
    !state
        .user_content_identities
        .contains_key(&pending.idempotency_key)
        && !state
            .user_content_tombstones
            .iter()
            .any(|tombstone| tombstone.idempotency_key == pending.idempotency_key)
        && !state.items.contains_key(&pending.item_id)
        && realtime_predecessor_materialized(state, pending.previous_item_id.as_deref())
}

fn realtime_transcript_delta_ids_valid(state: &SessionRealtimeTranscriptState) -> bool {
    state
        .seen_delta_ids
        .iter()
        .all(|delta_id| !delta_id.trim().is_empty())
}

fn realtime_transcript_completion_ids_valid(state: &SessionRealtimeTranscriptState) -> bool {
    state
        .assistant_completions
        .keys()
        .all(|response_id| !response_id.trim().is_empty())
}

fn realtime_transcript_discarded_ids_valid(state: &SessionRealtimeTranscriptState) -> bool {
    state
        .discarded_assistant_response_ids
        .iter()
        .all(|response_id| !response_id.trim().is_empty())
}

fn realtime_transcript_materialized_items_were_ready_or_skipped(
    state: &SessionRealtimeTranscriptState,
) -> bool {
    state
        .items
        .values()
        .all(|item| !item.materialized || item.ready || item.skipped)
}

fn realtime_transcript_assistant_items_have_response_unless_skipped(
    state: &SessionRealtimeTranscriptState,
) -> bool {
    state.items.values().all(|item| {
        item.role != RealtimeTranscriptRole::Assistant
            || item.skipped
            || item.response_id.is_some()
            || (!item.ready && !item.materialized && item.text().is_empty())
    })
}

fn realtime_transcript_ready_assistant_items_have_completion_or_are_skipped(
    state: &SessionRealtimeTranscriptState,
) -> bool {
    state.items.values().all(|item| {
        if item.role != RealtimeTranscriptRole::Assistant || item.skipped || !item.ready {
            return true;
        }
        item.response_id
            .as_ref()
            .is_some_and(|response_id| state.assistant_completions.contains_key(response_id))
    })
}

fn realtime_transcript_materialized_assistant_completions_consumed(
    state: &SessionRealtimeTranscriptState,
) -> bool {
    state.items.values().all(|item| {
        if item.role != RealtimeTranscriptRole::Assistant || !item.materialized || item.skipped {
            return true;
        }
        item.response_id.as_ref().is_some_and(|response_id| {
            state
                .assistant_completions
                .get(response_id)
                .is_some_and(|completion| completion.usage_consumed)
        })
    })
}

fn realtime_transcript_completed_assistant_text_items_are_ready_or_materialized_or_skipped(
    state: &SessionRealtimeTranscriptState,
) -> bool {
    state.items.values().all(|item| {
        if item.role != RealtimeTranscriptRole::Assistant || item.text().is_empty() {
            return true;
        }
        let Some(response_id) = item.response_id.as_ref() else {
            return false;
        };
        if !state.assistant_completions.contains_key(response_id) {
            return true;
        }
        item.ready || item.materialized || item.skipped
    })
}

fn realtime_transcript_discarded_assistant_items_are_skipped_or_materialized(
    state: &SessionRealtimeTranscriptState,
) -> bool {
    state.items.values().all(|item| {
        if item.role != RealtimeTranscriptRole::Assistant {
            return true;
        }
        let Some(response_id) = item.response_id.as_ref() else {
            return true;
        };
        if !state.discarded_assistant_response_ids.contains(response_id) {
            return true;
        }
        item.skipped || item.materialized || item.lane == TranscriptLane::Display
    })
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
