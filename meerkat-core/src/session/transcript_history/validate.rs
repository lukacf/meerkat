//! Whole-graph and per-record transcript validation.
//!
//! Extracted verbatim from `session.rs`; the extraction commit changes
//! no behaviour, only where the code lives.

use super::graph::{TranscriptHistoryState, TranscriptRevisionBody, TranscriptRewriteCommit};
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
    let parent_digest = transcript_messages_digest(&parent_body.messages)
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if parent_digest != commit.parent_revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "parent body digest {parent_digest} does not match commit parent {}",
            commit.parent_revision
        )));
    }
    let revision_digest = transcript_messages_digest(&revision_body.messages)
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
    let original_span_digest = transcript_messages_digest(&parent_body.messages[start..end])
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
    let parent_prefix_digest = transcript_messages_digest(&parent_body.messages[..start])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    let revision_prefix_digest = transcript_messages_digest(&revision_body.messages[..start])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if parent_prefix_digest != revision_prefix_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "rewrite revision changed messages before the selected span".to_string(),
        ));
    }
    let parent_suffix_digest = transcript_messages_digest(&parent_body.messages[end..])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    let revision_suffix_digest =
        transcript_messages_digest(&revision_body.messages[replacement_end..])
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if parent_suffix_digest != revision_suffix_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "rewrite revision changed messages after the selected span".to_string(),
        ));
    }
    let replacement_digest =
        transcript_messages_digest(&revision_body.messages[start..replacement_end])
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
    if state
        .revisions
        .iter()
        .all(|body| body.revision != state.head)
    {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "missing transcript head body {}",
            state.head
        )));
    }
    for body in &state.revisions {
        let digest = transcript_messages_digest(&body.messages)
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        if digest != body.revision {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "transcript revision body {} has digest {digest}",
                body.revision
            )));
        }
    }
    for commit in &state.commits {
        let parent_body = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.parent_revision)
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "missing parent transcript body {}",
                    commit.parent_revision
                ))
            })?;
        let revision_body = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.revision)
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "missing transcript revision body {}",
                    commit.revision
                ))
            })?;
        validate_transcript_rewrite_record(commit, parent_body, revision_body)?;
    }
    let Some(first_commit) = state.commits.first() else {
        return Ok(());
    };
    let mut expected_head = first_commit.parent_revision.clone();
    for commit in &state.commits {
        let parent_body = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.parent_revision)
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "missing parent transcript body {}",
                    commit.parent_revision
                ))
            })?;
        if commit.parent_revision != expected_head
            && !revision_body_extends_head(parent_body, &state.revisions, &expected_head)?
        {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite commit parent {} does not extend transcript head {}",
                commit.parent_revision, expected_head
            )));
        }
        expected_head = commit.revision.clone();
    }
    let head_is_audited_endpoint = state
        .commits
        .iter()
        .any(|commit| commit.parent_revision == state.head || commit.revision == state.head);
    let head_extends_latest_commit = if head_is_audited_endpoint {
        let Some(head_body) = state
            .revisions
            .iter()
            .find(|body| body.revision == state.head)
        else {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "missing transcript head body {}",
                state.head
            )));
        };
        revision_body_extends_head(head_body, &state.revisions, &expected_head)?
    } else {
        let mut cursor = state.head.as_str();
        let mut visited = BTreeSet::new();
        while cursor != expected_head {
            if !visited.insert(cursor.to_string()) {
                break;
            }
            let Some(head_body) = state.revisions.iter().find(|body| body.revision == cursor)
            else {
                break;
            };
            let Some(parent) = head_body.parent_revision.as_deref() else {
                break;
            };
            cursor = parent;
        }
        cursor == expected_head
    };
    if !head_extends_latest_commit {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "transcript head {} does not extend the rewrite chain",
            state.head
        )));
    }
    Ok(())
}

pub(super) fn revision_body_extends_head(
    candidate: &TranscriptRevisionBody,
    revisions: &[TranscriptRevisionBody],
    head: &str,
) -> Result<bool, TranscriptEditError> {
    let Some(head_body) = revisions.iter().find(|body| body.revision == head) else {
        return Ok(false);
    };
    if candidate.revision == head {
        return Ok(true);
    }
    if candidate.messages.len() < head_body.messages.len() {
        return Ok(false);
    }
    let prefix_digest = transcript_messages_digest(&candidate.messages[..head_body.messages.len()])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if prefix_digest == head {
        return Ok(true);
    }

    // A resume-time system refresh may replace the single leading System
    // projection while preserving (and possibly appending to) the exact
    // conversation tail. Prove that content shape directly; a historical
    // parent_revision pointer is not occurrence identity and must never, by
    // itself, authorize a later commit after a digest has recurred.
    let (Some(Message::System(_)), Some(Message::System(_))) =
        (candidate.messages.first(), head_body.messages.first())
    else {
        return Ok(false);
    };
    let head_tail_len = head_body.messages.len().saturating_sub(1);
    if head_tail_len == 0 {
        return Ok(true);
    }
    let candidate_tail = &candidate.messages[1..];
    if candidate_tail.len() < head_tail_len {
        return Ok(false);
    }
    let head_tail_digest = transcript_messages_digest(&head_body.messages[1..])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    let candidate_tail_prefix_digest = transcript_messages_digest(&candidate_tail[..head_tail_len])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    Ok(candidate_tail_prefix_digest == head_tail_digest)
}
