//! Durable-format healing of legacy transcript revision strings.
//!
//! Extracted verbatim from `session.rs`; the extraction commit changes
//! no behaviour, only where the code lives.

use super::graph::{TranscriptRevisionBody, TranscriptRewriteCommit};
use crate::session::{
    TranscriptRewriteSelection, canonicalize_message_images_for_digest, sha256_json_digest,
    transcript_messages_digest,
};
use crate::types::Message;
use std::collections::BTreeMap;

/// Re-derive pre-0.7.14 (bookkeeping-inclusive) transcript revision strings to
/// the current content-addressed format at the durable-format parse boundary.
///
/// Retained revision bodies carry their full message lists, so every legacy
/// string can be re-verified against the bytes it was computed from. Only
/// strings that verify under the legacy digest of their own retained body are
/// rewritten; anything else is left untouched for the validators to reject
/// exactly as they would have before.
pub(super) fn heal_legacy_revision_strings(
    revisions: &mut [TranscriptRevisionBody],
    commits: &mut [TranscriptRewriteCommit],
    head: Option<&mut String>,
) -> Result<(), serde_json::Error> {
    let mut remap: BTreeMap<String, String> = BTreeMap::new();
    for body in revisions.iter() {
        let content = transcript_messages_digest(&body.messages)?;
        if body.revision == content {
            continue;
        }
        if body.revision == legacy_transcript_messages_digest(&body.messages)? {
            remap.insert(body.revision.clone(), content);
        }
    }
    if remap.is_empty() {
        return Ok(());
    }
    for body in revisions.iter_mut() {
        if let Some(current) = remap.get(&body.revision) {
            body.revision = current.clone();
        }
        if let Some(parent) = body.parent_revision.as_ref()
            && let Some(current) = remap.get(parent)
        {
            body.parent_revision = Some(current.clone());
        }
    }
    for commit in commits.iter_mut() {
        if let Some(current) = remap.get(&commit.parent_revision) {
            commit.parent_revision = current.clone();
        }
        if let Some(current) = remap.get(&commit.revision) {
            commit.revision = current.clone();
        }
        heal_legacy_commit_span_digests(commit, revisions)?;
    }
    if let Some(head) = head
        && let Some(current) = remap.get(head.as_str())
    {
        *head = current.clone();
    }
    Ok(())
}

/// Re-derive a legacy commit's span digests from its retained bodies.
///
/// Span digests are only rewritten when the stored value verifies under the
/// legacy digest of the same span; malformed commits keep their stored bytes
/// so [`validate_transcript_rewrite_record`] rejects them unchanged.
fn heal_legacy_commit_span_digests(
    commit: &mut TranscriptRewriteCommit,
    revisions: &[TranscriptRevisionBody],
) -> Result<(), serde_json::Error> {
    let Some(parent_body) = revisions
        .iter()
        .rev()
        .find(|body| body.revision == commit.parent_revision)
    else {
        return Ok(());
    };
    let Some(revision_body) = revisions
        .iter()
        .rev()
        .find(|body| body.revision == commit.revision)
    else {
        return Ok(());
    };
    let (start, end) = commit.selection.bounds();
    if start > end || end > parent_body.messages.len() {
        return Ok(());
    }
    let removed_len = end - start;
    let Some(retained_len) = commit.messages_before.checked_sub(removed_len) else {
        return Ok(());
    };
    let Some(replacement_len) = commit.messages_after.checked_sub(retained_len) else {
        return Ok(());
    };
    let Some(replacement_end) = start.checked_add(replacement_len) else {
        return Ok(());
    };
    if replacement_end > revision_body.messages.len() {
        return Ok(());
    }
    let original_span = &parent_body.messages[start..end];
    if commit.original_span_digest == legacy_transcript_messages_digest(original_span)? {
        commit.original_span_digest = transcript_messages_digest(original_span)?;
    }
    let replacement_span = &revision_body.messages[start..replacement_end];
    if commit.replacement_digest == legacy_transcript_messages_digest(replacement_span)? {
        commit.replacement_digest = transcript_messages_digest(replacement_span)?;
    }
    Ok(())
}

/// Upgrade pre-semantic-field compaction records from retained typed transcript
/// evidence, never from the free-form audit reason.
///
/// Old compaction commits used the generic `message_range` selection, but their
/// revision body already carries the runtime-minted `CompactionSummary` role.
/// A full-transcript, shrinking rewrite with exactly one such summary is the
/// complete legacy witness. Other edits remain ordinary edits even when their
/// display reason happens to say "compaction".
pub(super) fn heal_legacy_compaction_rewrite_semantics(
    commits: &mut [TranscriptRewriteCommit],
    revisions: &[TranscriptRevisionBody],
) -> bool {
    let mut changed = false;
    for commit in commits {
        if !commit.selection.is_legacy_untyped() {
            continue;
        }
        let (start, end) = commit.selection.bounds();
        if start != 0
            || end != commit.messages_before
            || commit.messages_after >= commit.messages_before
        {
            continue;
        }
        let Some(parent) = revisions
            .iter()
            .rev()
            .find(|body| body.revision == commit.parent_revision)
        else {
            continue;
        };
        let Some(revision) = revisions
            .iter()
            .rev()
            .find(|body| body.revision == commit.revision)
        else {
            continue;
        };
        if parent.messages.len() != commit.messages_before
            || revision.messages.len() != commit.messages_after
        {
            continue;
        }
        let summary_count = revision
            .messages
            .iter()
            .filter(|message| {
                matches!(message, Message::User(user) if user.transcript_role.is_compaction_summary())
            })
            .count();
        if summary_count == 1 {
            commit.selection = TranscriptRewriteSelection::migrated_legacy_compaction(start, end);
            changed = true;
        }
    }
    changed
}

/// Digest format used by pre-0.7.14 transcript revision strings.
///
/// The legacy canonicalization only normalized image payloads, so persisted
/// revision strings from older stores include construction bookkeeping
/// (`identity`, `created_at`). This is a durable-format decoder: it exists
/// solely so [`heal_legacy_revision_strings`] can verify a stored string
/// against its retained body before re-deriving it to the current
/// content-addressed format. Never mint new revisions with it.
pub(crate) fn legacy_transcript_messages_digest(
    messages: &[Message],
) -> Result<String, serde_json::Error> {
    sha256_json_digest(&canonicalize_message_images_for_digest(messages))
}
