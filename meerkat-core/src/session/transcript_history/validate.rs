//! Whole-graph and per-record transcript validation.
//!
//! Extracted verbatim from `session.rs`; the extraction commit changes
//! no behaviour, only where the code lives.

use super::graph::{
    TRANSCRIPT_DIGEST_FORMAT_CURRENT, TRANSCRIPT_HISTORY_FORMAT_CURRENT, TranscriptEndpointWitness,
    TranscriptHistoryState, TranscriptParentAdvance, TranscriptRevisionBody,
    TranscriptRevisionEdge, TranscriptRewriteCommit, TranscriptRewritePrefixAccumulator,
};
use crate::session::{TranscriptEditError, TranscriptRewriteSemantic, transcript_messages_digest};
use crate::types::{AssistantBlock, Message};
use std::collections::BTreeSet;

pub(crate) fn message_role_name(message: &Message) -> &'static str {
    match message {
        Message::System(_) => "system",
        Message::SystemNotice(_) => "system_notice",
        Message::User(_) => "user",
        Message::BlockAssistant(_) => "block_assistant",
        Message::ToolResults { .. } => "tool_results",
    }
}

pub(crate) fn assistant_tool_use_ids(message: &Message) -> Vec<&str> {
    match message {
        Message::BlockAssistant(assistant) => assistant
            .blocks
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn validate_transcript_tool_result_shape(
    messages: &[Message],
) -> Result<(), TranscriptEditError> {
    for (index, message) in messages.iter().enumerate() {
        if let Message::ToolResults { results, .. } = message {
            let Some(previous) = index
                .checked_sub(1)
                .and_then(|previous| messages.get(previous))
            else {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "tool_results at message {index} has no preceding assistant tool-use message"
                )));
            };
            let expected = assistant_tool_use_ids(previous);
            if expected.is_empty() {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "tool_results at message {index} follows {}, not an assistant tool-use message",
                    message_role_name(previous)
                )));
            }
            let actual = results
                .iter()
                .map(|result| result.tool_use_id.as_str())
                .collect::<Vec<_>>();
            let actual_set = actual.iter().copied().collect::<BTreeSet<_>>();
            let expected_set = expected.iter().copied().collect::<BTreeSet<_>>();
            if actual.len() != actual_set.len() {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "tool_results at message {index} contains duplicate tool ids"
                )));
            }
            if expected.len() != expected_set.len() {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "assistant tool-use message before tool_results at message {index} contains duplicate tool ids"
                )));
            }
            if actual_set != expected_set {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "tool_results at message {index} resolve tool ids {actual_set:?}, expected {expected_set:?}"
                )));
            }
        }

        let tool_use_ids = assistant_tool_use_ids(message);
        if tool_use_ids.is_empty() {
            continue;
        }
        let Some(next) = messages.get(index + 1) else {
            return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                "assistant tool-use message {index} has no following tool_results"
            )));
        };
        if !matches!(next, Message::ToolResults { .. }) {
            return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                "assistant tool-use message {index} is followed by {}, not tool_results",
                message_role_name(next)
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_transcript_rewrite_record(
    commit: &TranscriptRewriteCommit,
    parent_body: &TranscriptRevisionBody,
    revision_body: &TranscriptRevisionBody,
) -> Result<(), TranscriptEditError> {
    validate_transcript_rewrite_record_with_digest(
        commit,
        parent_body,
        revision_body,
        transcript_messages_digest,
    )
}

pub(super) fn validate_released_0810_transcript_rewrite_record(
    commit: &TranscriptRewriteCommit,
    parent_body: &TranscriptRevisionBody,
    revision_body: &TranscriptRevisionBody,
) -> Result<(), TranscriptEditError> {
    validate_released_0810_transcript_rewrite_structure(commit, parent_body, revision_body)
}

fn validate_instruction_activation_revision_transition(
    commit: &TranscriptRewriteCommit,
    parent_body: &TranscriptRevisionBody,
    revision_body: &TranscriptRevisionBody,
) -> Result<(), TranscriptEditError> {
    for (role, body) in [("parent", parent_body), ("revision", revision_body)] {
        crate::session::validate_instruction_activation_messages(&body.messages).map_err(
            |error| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "{role} transcript revision carries malformed instruction activation history: {error}"
                ))
            },
        )?;
    }
    let activation_rows = |messages: &[Message], preserve_offsets: bool| {
        messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| {
                let Message::System(system) = message else {
                    return None;
                };
                system.instruction_activation.as_ref().map(|_| {
                    (
                        preserve_offsets.then_some(index),
                        Message::System(system.clone()),
                    )
                })
            })
            .collect::<Vec<_>>()
    };
    let preserve_offsets = commit.selection.semantic() != TranscriptRewriteSemantic::Compaction;
    if activation_rows(&parent_body.messages, preserve_offsets)
        != activation_rows(&revision_body.messages, preserve_offsets)
    {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "transcript rewrite changed an instruction activation boundary".to_string(),
        ));
    }
    Ok(())
}

/// Validate the exact structure still observable after released 0.8.10
/// metadata buffering.
///
/// That release could mint a format-2 label before `RawValue` object spelling
/// was normalized into `serde_json::Value`, irreversibly losing the bytes
/// needed to re-prove the label. The one-time importer therefore proves
/// topology and exact retained message relations here, under the enclosing
/// checkpoint/store source authority, then rebinds every semantic id. Current
/// graph ingress never calls this relaxed validator.
fn validate_released_0810_transcript_rewrite_structure(
    commit: &TranscriptRewriteCommit,
    parent_body: &TranscriptRevisionBody,
    revision_body: &TranscriptRevisionBody,
) -> Result<(), TranscriptEditError> {
    validate_instruction_activation_revision_transition(commit, parent_body, revision_body)?;
    if parent_body.revision != commit.parent_revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "parent body revision {} does not match commit parent {}",
            parent_body.revision, commit.parent_revision
        )));
    }
    if revision_body.revision != commit.revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "revision body {} does not match commit revision {}",
            revision_body.revision, commit.revision
        )));
    }
    if commit.parent_revision == commit.revision {
        return Err(TranscriptEditError::NoOpRewrite {
            revision: commit.revision.clone(),
        });
    }
    let (start, end) = commit.selection.bounds();
    if start > end || end > parent_body.messages.len() {
        return Err(TranscriptEditError::InvalidRewriteRange {
            start,
            end,
            message_count: parent_body.messages.len(),
        });
    }
    if commit.messages_before != parent_body.messages.len()
        || commit.messages_after != revision_body.messages.len()
    {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "commit message counts {} -> {} do not match revision bodies {} -> {}",
            commit.messages_before,
            commit.messages_after,
            parent_body.messages.len(),
            revision_body.messages.len()
        )));
    }
    let removed_len = end - start;
    let retained_len = commit
        .messages_before
        .checked_sub(removed_len)
        .ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "commit removed more messages than it recorded before rewrite".to_string(),
            )
        })?;
    let replacement_len = commit
        .messages_after
        .checked_sub(retained_len)
        .ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "commit message counts cannot describe a replacement span".to_string(),
            )
        })?;
    let replacement_end = start.checked_add(replacement_len).ok_or_else(|| {
        TranscriptEditError::HistoryStateMalformed("replacement span end overflowed".to_string())
    })?;
    if replacement_end > revision_body.messages.len() {
        return Err(TranscriptEditError::InvalidRewriteRange {
            start,
            end: replacement_end,
            message_count: revision_body.messages.len(),
        });
    }
    let parent_prefix = transcript_messages_digest(&parent_body.messages[..start])
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    let revision_prefix = transcript_messages_digest(&revision_body.messages[..start])
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    let parent_suffix = transcript_messages_digest(&parent_body.messages[end..])
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    let revision_suffix = transcript_messages_digest(&revision_body.messages[replacement_end..])
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    if parent_prefix != revision_prefix || parent_suffix != revision_suffix {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "released 0.8.10 rewrite changed retained messages outside its selected span"
                .to_string(),
        ));
    }
    if commit.selection.semantic() == TranscriptRewriteSemantic::Compaction {
        let summary_count = revision_body.messages[start..replacement_end]
            .iter()
            .filter(|message| {
                matches!(message, Message::User(user) if user.transcript_role.is_compaction_summary())
            })
            .count();
        if start != 0
            || end != commit.messages_before
            || commit.messages_after >= commit.messages_before
            || summary_count != 1
        {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "typed compaction rewrite must shrink the full transcript and carry exactly one CompactionSummary"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_transcript_rewrite_record_with_digest(
    commit: &TranscriptRewriteCommit,
    parent_body: &TranscriptRevisionBody,
    revision_body: &TranscriptRevisionBody,
    digest: fn(&[Message]) -> Result<String, serde_json::Error>,
) -> Result<(), TranscriptEditError> {
    validate_instruction_activation_revision_transition(commit, parent_body, revision_body)?;
    if parent_body.revision != commit.parent_revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "parent body revision {} does not match commit parent {}",
            parent_body.revision, commit.parent_revision
        )));
    }
    if revision_body.revision != commit.revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "revision body {} does not match commit revision {}",
            revision_body.revision, commit.revision
        )));
    }
    if commit.parent_revision == commit.revision {
        return Err(TranscriptEditError::NoOpRewrite {
            revision: commit.revision.clone(),
        });
    }
    let parent_digest = digest(&parent_body.messages)
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if parent_digest != commit.parent_revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "parent body digest {parent_digest} does not match commit parent {}",
            commit.parent_revision
        )));
    }
    let revision_digest = digest(&revision_body.messages)
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if revision_digest != commit.revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "revision body digest {revision_digest} does not match commit revision {}",
            commit.revision
        )));
    }
    let (start, end) = commit.selection.bounds();
    if start > end || end > parent_body.messages.len() {
        return Err(TranscriptEditError::InvalidRewriteRange {
            start,
            end,
            message_count: parent_body.messages.len(),
        });
    }
    if commit.messages_before != parent_body.messages.len()
        || commit.messages_after != revision_body.messages.len()
    {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "commit message counts {} -> {} do not match revision bodies {} -> {}",
            commit.messages_before,
            commit.messages_after,
            parent_body.messages.len(),
            revision_body.messages.len()
        )));
    }
    let original_span_digest = digest(&parent_body.messages[start..end])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if original_span_digest != commit.original_span_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "original span digest {original_span_digest} does not match commit digest {}",
            commit.original_span_digest
        )));
    }
    let removed_len = end - start;
    let retained_len = commit
        .messages_before
        .checked_sub(removed_len)
        .ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "commit removed more messages than it recorded before rewrite".to_string(),
            )
        })?;
    let replacement_len = commit
        .messages_after
        .checked_sub(retained_len)
        .ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "commit message counts cannot describe a replacement span".to_string(),
            )
        })?;
    let replacement_end = start.checked_add(replacement_len).ok_or_else(|| {
        TranscriptEditError::HistoryStateMalformed("replacement span end overflowed".to_string())
    })?;
    if replacement_end > revision_body.messages.len() {
        return Err(TranscriptEditError::InvalidRewriteRange {
            start,
            end: replacement_end,
            message_count: revision_body.messages.len(),
        });
    }
    if commit.selection.semantic() == TranscriptRewriteSemantic::Compaction {
        let summary_count = revision_body.messages[start..replacement_end]
            .iter()
            .filter(|message| {
                matches!(message, Message::User(user) if user.transcript_role.is_compaction_summary())
            })
            .count();
        if start != 0
            || end != commit.messages_before
            || commit.messages_after >= commit.messages_before
            || summary_count != 1
        {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "typed compaction rewrite must shrink the full transcript and carry exactly one CompactionSummary"
                    .to_string(),
            ));
        }
    }
    let parent_prefix_digest = digest(&parent_body.messages[..start])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    let revision_prefix_digest = digest(&revision_body.messages[..start])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if parent_prefix_digest != revision_prefix_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "rewrite revision changed messages before the selected span".to_string(),
        ));
    }
    let parent_suffix_digest = digest(&parent_body.messages[end..])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    let revision_suffix_digest = digest(&revision_body.messages[replacement_end..])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if parent_suffix_digest != revision_suffix_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "rewrite revision changed messages after the selected span".to_string(),
        ));
    }
    let replacement_digest = digest(&revision_body.messages[start..replacement_end])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if replacement_digest != commit.replacement_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "replacement span digest {replacement_digest} does not match commit digest {}",
            commit.replacement_digest
        )));
    }
    Ok(())
}

pub(crate) fn validate_transcript_history_state(
    state: &TranscriptHistoryState,
) -> Result<(), TranscriptEditError> {
    if state.format() != TRANSCRIPT_HISTORY_FORMAT_CURRENT {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "unsupported transcript graph format {}",
            state.format()
        )));
    }
    if state.digest_format() != TRANSCRIPT_DIGEST_FORMAT_CURRENT {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "unsupported transcript digest format {}",
            state.digest_format()
        )));
    }
    if state.edges().is_empty() {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "current transcript graph carries no rewrite occurrence".to_string(),
        ));
    }
    let anchor_digest = transcript_messages_digest(state.anchor().messages())
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    if anchor_digest != state.anchor().revision() {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "transcript graph anchor {} has digest {anchor_digest}",
            state.anchor().revision()
        )));
    }
    let anchor_row_prefix =
        crate::SessionMessageRowPrefixAccumulator::from_messages(state.anchor().messages())
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    if &anchor_row_prefix != state.anchor().row_prefix() {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "transcript graph anchor exact row prefix does not bind its messages".to_string(),
        ));
    }
    let mut expected_base = state.anchor().revision();
    let mut expected_witness = TranscriptEndpointWitness::from_messages(state.anchor().messages())
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    let mut rewrite_prefix = TranscriptRewritePrefixAccumulator::empty();
    for (index, edge) in state.edges().iter().enumerate() {
        let expected = u64::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "transcript rewrite generation exceeds u64".to_string(),
                )
            })?;
        if edge.rewrite_generation() != expected {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "transcript rewrite generation {} is not the expected contiguous occurrence {expected}",
                edge.rewrite_generation()
            )));
        }
        validate_transcript_revision_edge(expected_base, &expected_witness, edge.as_ref())?;
        rewrite_prefix = rewrite_prefix
            .extend(edge.commit())
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        if &rewrite_prefix != edge.rewrite_prefix() {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite occurrence {} cached prefix does not bind its commit",
                edge.rewrite_generation()
            )));
        }
        expected_base = edge.revision();
        expected_witness = edge.result_witness().clone();
    }
    if &rewrite_prefix != state.rewrite_prefix() {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite-prefix accumulator does not bind {} ordered commits",
            state.commit_count()
        )));
    }
    let graph_prefix = super::graph::TranscriptGraphPrefixAccumulator::from_graph(
        state.anchor(),
        state.edges().iter().map(AsRef::as_ref),
    )
    .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    if &graph_prefix != state.graph_prefix() {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "graph-prefix accumulator does not bind {} ordered occurrence edges",
            state.commit_count()
        )));
    }
    if state.head() != expected_base {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "derived transcript head {} does not match final occurrence {expected_base}",
            state.head()
        )));
    }
    Ok(())
}

pub(super) fn validate_transcript_revision_edge(
    expected_base_revision: &str,
    expected_base_witness: &TranscriptEndpointWitness,
    edge: &TranscriptRevisionEdge,
) -> Result<(), TranscriptEditError> {
    let commit = edge.commit();
    for (label, digest) in [
        ("parent revision", commit.parent_revision.as_str()),
        ("revision", commit.revision.as_str()),
        ("original span", commit.original_span_digest.as_str()),
        ("replacement", commit.replacement_digest.as_str()),
    ] {
        require_canonical_sha256(label, digest)?;
    }
    if edge.base_revision() != expected_base_revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {} base {} does not match preceding endpoint {expected_base_revision}",
            edge.rewrite_generation(),
            edge.base_revision()
        )));
    }
    if edge.parent_revision() != commit.parent_revision
        || edge.revision() != commit.revision
        || edge.rewrite_generation() != commit.rewrite_generation
    {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {} edge identity disagrees with embedded commit",
            edge.rewrite_generation()
        )));
    }
    if commit.parent_revision == commit.revision {
        return Err(TranscriptEditError::NoOpRewrite {
            revision: commit.revision.clone(),
        });
    }
    if edge.messages_before_base() != expected_base_witness.message_count() {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {} base count {} does not match endpoint witness {}",
            edge.rewrite_generation(),
            edge.messages_before_base(),
            expected_base_witness.message_count()
        )));
    }
    let appended = edge.parent_advance().appended().len();
    let expected_parent_count = edge
        .messages_before_base()
        .checked_add(appended)
        .ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "rewrite parent message count overflowed".to_string(),
            )
        })?;
    if expected_parent_count != commit.messages_before {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {} parent advance describes {} messages, commit records {}",
            edge.rewrite_generation(),
            expected_parent_count,
            commit.messages_before
        )));
    }
    if edge.parent_row_prefix().row_count() != commit.messages_before as u64 {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {} exact parent row prefix covers {} rows, commit records {}",
            edge.rewrite_generation(),
            edge.parent_row_prefix().row_count(),
            commit.messages_before
        )));
    }
    let (base_parent_prefix, appended) = match edge.parent_advance() {
        TranscriptParentAdvance::ExactAppend { appended } => {
            (Some(expected_base_witness.row_prefix().clone()), appended)
        }
        TranscriptParentAdvance::ContentAddressedAppend { appended } => (None, appended),
        TranscriptParentAdvance::ExactSplice {
            at,
            replacement,
            appended,
        } => {
            let end = at.checked_add(replacement.len()).ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "rewrite occurrence {} parent splice end overflowed",
                    edge.rewrite_generation()
                ))
            })?;
            if replacement.is_empty() || end > expected_base_witness.message_count() {
                return Err(TranscriptEditError::HistoryStateMalformed(format!(
                    "rewrite occurrence {} parent splice is empty or outside its base",
                    edge.rewrite_generation()
                )));
            }
            let replacement_rows = replacement
                .iter()
                .map(serde_json::to_vec)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
            let at = u64::try_from(*at).map_err(|_| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "rewrite occurrence {} parent splice start exceeds durable row coordinates",
                    edge.rewrite_generation()
                ))
            })?;
            let end = u64::try_from(end).map_err(|_| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "rewrite occurrence {} parent splice end exceeds durable row coordinates",
                    edge.rewrite_generation()
                ))
            })?;
            let spliced = expected_base_witness
                .row_prefix()
                .replace_serialized_range(at, end, &replacement_rows)
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
            (Some(spliced), appended)
        }
    };
    let serialized = appended
        .iter()
        .map(serde_json::to_vec)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    if let Some(base_parent_prefix) = base_parent_prefix {
        let expected_parent_prefix = base_parent_prefix
            .extend_serialized_rows(&serialized)
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        if &expected_parent_prefix != edge.parent_row_prefix() {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite occurrence {} parent row prefix does not exactly bind its typed advance",
                edge.rewrite_generation()
            )));
        }
    }
    let (start, end) = commit.selection.bounds();
    if start != edge.rewrite().at() || start > end || end > commit.messages_before {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {} patch bounds do not match selection",
            edge.rewrite_generation()
        )));
    }
    let expected_after = commit
        .messages_before
        .checked_sub(end - start)
        .and_then(|retained| retained.checked_add(edge.rewrite().replacement().len()))
        .ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "rewrite patch message count overflowed".to_string(),
            )
        })?;
    if expected_after != commit.messages_after {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {} patch produces {} messages, commit records {}",
            edge.rewrite_generation(),
            expected_after,
            commit.messages_after
        )));
    }
    if edge.result_witness().message_count() != commit.messages_after
        || edge.result_witness().row_prefix().row_count() != commit.messages_after as u64
    {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {} result endpoint witness is malformed",
            edge.rewrite_generation()
        )));
    }
    let replacement_digest = transcript_messages_digest(edge.rewrite().replacement())
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    if replacement_digest != commit.replacement_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {} replacement digest does not match commit",
            edge.rewrite_generation()
        )));
    }
    let replacement_rows = edge
        .rewrite()
        .replacement()
        .iter()
        .map(serde_json::to_vec)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    let durable_start = u64::try_from(start).map_err(|_| {
        TranscriptEditError::HistoryStateMalformed(
            "rewrite start exceeds durable u64 row coordinates".to_string(),
        )
    })?;
    let durable_end = u64::try_from(end).map_err(|_| {
        TranscriptEditError::HistoryStateMalformed(
            "rewrite end exceeds durable u64 row coordinates".to_string(),
        )
    })?;
    let expected_result_prefix = edge
        .parent_row_prefix()
        .replace_serialized_range(durable_start, durable_end, &replacement_rows)
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    if &expected_result_prefix != edge.result_witness().row_prefix() {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {} result row lineage is not derived from its exact parent and replacement: expected {} at {} rows, found {} at {} rows",
            edge.rewrite_generation(),
            expected_result_prefix.digest(),
            expected_result_prefix.row_count(),
            edge.result_witness().row_prefix().digest(),
            edge.result_witness().row_prefix().row_count(),
        )));
    }
    if commit.selection.semantic() == TranscriptRewriteSemantic::Compaction {
        let summaries = edge
            .rewrite()
            .replacement()
            .iter()
            .filter(|message| {
                matches!(message, Message::User(user) if user.transcript_role.is_compaction_summary())
            })
            .count();
        if start != 0
            || end != commit.messages_before
            || commit.messages_after >= commit.messages_before
            || summaries != 1
        {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "typed compaction edge must shrink the full transcript and carry one summary"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

fn require_canonical_sha256(label: &str, value: &str) -> Result<(), TranscriptEditError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "{label} digest is not canonical sha256"
        )));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "{label} digest is not canonical sha256"
        )));
    }
    Ok(())
}
