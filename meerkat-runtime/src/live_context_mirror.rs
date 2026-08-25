//! Sealed bridge from exact committed session rows into generated live-context authority.

use meerkat_core::generated::session_document::{
    LiveContextCommittedRowDisposition, LiveContextCommittedRowKind,
    LiveContextCommittedTextProvenance, SessionDocumentEffect, SessionDocumentKey,
    SessionDocumentMachineAuthority,
};
use meerkat_core::{AssistantBlock, Message, SessionId};
use sha2::{Digest, Sha256};

use crate::live_execution::{
    LiveContextAmbiguityRecoveryAuthority, LiveContextAppendAuthority,
    LiveDelegationResultAmbiguityRecoveryAuthority,
};

/// Provider-neutral shell seam for canonical context delivery and generated
/// ambiguity recovery. Implementations may perform I/O, but receive no power
/// to select rows, cursor edges, or recovery channel identity.
#[async_trait::async_trait]
pub trait LiveContextMirrorHost: Send + Sync {
    async fn append_context(
        &self,
        authority: LiveContextAppendAuthority,
        context: String,
    ) -> Result<
        (
            LiveContextAppendAuthority,
            meerkat_core::LiveAppendDeliveryOutcome,
        ),
        String,
    >;

    async fn recover_ambiguous_append(
        &self,
        authority: LiveContextAmbiguityRecoveryAuthority,
    ) -> Result<(), String>;

    async fn recover_ambiguous_delegation_result(
        &self,
        authority: LiveDelegationResultAmbiguityRecoveryAuthority,
    ) -> Result<(), String>;
}

/// One exact store-committed canonical row classified by SessionDocument.
///
/// Construction is crate-private so a surface cannot manufacture commit or
/// provenance evidence. The provider payload is retained only for the one
/// generated disposition that authorizes mirroring.
#[derive(Debug, Clone)]
pub struct CommittedLiveContextRow {
    session_id: SessionId,
    canonical_row_sequence: u64,
    content_digest: String,
    store_commit_authority: String,
    disposition: LiveContextCommittedRowDisposition,
    provider_context: Option<String>,
}

impl CommittedLiveContextRow {
    pub(crate) fn classify(
        session_id: &SessionId,
        canonical_row_sequence: u64,
        serialized_row: &[u8],
        message: &Message,
        provenance: LiveContextCommittedTextProvenance,
        store_commit_authority: &str,
    ) -> Result<Self, String> {
        let (row_kind, provider_context) = context_projection(message)?;
        let content_digest = format!("{:x}", Sha256::digest(serialized_row));
        let mut authority = SessionDocumentMachineAuthority::new();
        let effects = authority
            .classify_live_context_committed_row(
                SessionDocumentKey::new(session_id.to_string()),
                canonical_row_sequence,
                row_kind,
                provenance,
                content_digest.clone(),
                store_commit_authority.to_string(),
            )
            .map_err(|error| error.to_string())?;
        let classified = effects.into_iter().find_map(|effect| match effect {
            SessionDocumentEffect::LiveContextCommittedRowClassified {
                session_id: effect_session,
                canonical_row_sequence: effect_sequence,
                row_kind: effect_kind,
                provenance: effect_provenance,
                disposition,
                content_digest: effect_digest,
                store_commit_authority: effect_commit_authority,
            } if effect_session == SessionDocumentKey::new(session_id.to_string())
                && effect_sequence == canonical_row_sequence
                && effect_kind == row_kind
                && effect_provenance == provenance
                && effect_digest == content_digest
                && effect_commit_authority == store_commit_authority =>
            {
                Some(disposition)
            }
            _ => None,
        });
        let disposition = classified.ok_or_else(|| {
            "SessionDocument emitted no exact live-context row classification".to_string()
        })?;
        let provider_context = matches!(
            disposition,
            LiveContextCommittedRowDisposition::MirrorParentText
        )
        .then_some(provider_context)
        .flatten();
        Ok(Self {
            session_id: session_id.clone(),
            canonical_row_sequence,
            content_digest,
            store_commit_authority: store_commit_authority.to_string(),
            disposition,
            provider_context,
        })
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn canonical_row_sequence(&self) -> u64 {
        self.canonical_row_sequence
    }

    #[must_use]
    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }

    #[must_use]
    pub fn store_commit_authority(&self) -> &str {
        &self.store_commit_authority
    }

    #[must_use]
    pub const fn disposition(&self) -> LiveContextCommittedRowDisposition {
        self.disposition
    }

    #[must_use]
    pub fn provider_context(&self) -> Option<&str> {
        self.provider_context.as_deref()
    }
}

pub(crate) fn classify_committed_boundary_rows_after(
    session_id: &SessionId,
    committed: &meerkat_core::lifecycle::core_executor::BoundSessionCommit,
    canonical_cursor: u64,
    provenance: LiveContextCommittedTextProvenance,
    store_commit_authority: &str,
) -> Result<Vec<CommittedLiveContextRow>, String> {
    let raw_rows = if let Some(session) = committed.session() {
        session
            .messages()
            .iter()
            .enumerate()
            .filter_map(|(index, message)| {
                let sequence = index as u64 + 1;
                (sequence > canonical_cursor).then_some((sequence, message))
            })
            .map(|(sequence, message)| {
                serde_json::to_vec(message)
                    .map(|serialized| (sequence, message.clone(), serialized))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?
    } else if let Some(ordinary) = committed
        .head_canonical()
        .and_then(|boundary| boundary.mutation().ordinary())
    {
        ordinary
            .serialized_suffix()
            .iter()
            .enumerate()
            .filter_map(|(index, serialized)| {
                let sequence = ordinary.base_seq() + index as u64 + 1;
                if sequence <= canonical_cursor {
                    return None;
                }
                Some(
                    serde_json::from_slice::<Message>(serialized)
                        .map(|message| (sequence, message, serialized.clone()))
                        .map_err(|error| {
                            format!("committed canonical row {sequence} cannot be decoded: {error}")
                        }),
                )
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };

    raw_rows
        .into_iter()
        .map(|(sequence, message, serialized)| {
            CommittedLiveContextRow::classify(
                session_id,
                sequence,
                &serialized,
                &message,
                provenance,
                store_commit_authority,
            )
        })
        .collect()
}

fn context_projection(
    message: &Message,
) -> Result<(LiveContextCommittedRowKind, Option<String>), String> {
    match message {
        Message::User(user) if user.transcript_role.is_conversational() => {
            let text = user.text_content();
            if text.trim().is_empty() {
                Ok((LiveContextCommittedRowKind::NonText, None))
            } else {
                let context = serde_json::to_string(&serde_json::json!({
                    "role": "user",
                    "text": text,
                }))
                .map_err(|error| error.to_string())?;
                Ok((LiveContextCommittedRowKind::UserText, Some(context)))
            }
        }
        Message::BlockAssistant(assistant) => {
            let text = assistant
                .blocks
                .iter()
                .filter_map(|block| match block {
                    AssistantBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            if text.trim().is_empty() {
                Ok((LiveContextCommittedRowKind::NonText, None))
            } else {
                let context = serde_json::to_string(&serde_json::json!({
                    "role": "assistant",
                    "text": text,
                }))
                .map_err(|error| error.to_string())?;
                Ok((LiveContextCommittedRowKind::AssistantText, Some(context)))
            }
        }
        Message::System(_)
        | Message::SystemNotice(_)
        | Message::ToolResults { .. }
        | Message::User(_) => Ok((LiveContextCommittedRowKind::NonText, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::UserMessage;

    fn classify(
        message: &Message,
        provenance: LiveContextCommittedTextProvenance,
    ) -> CommittedLiveContextRow {
        let serialized = serde_json::to_vec(message).expect("message serializes");
        CommittedLiveContextRow::classify(
            &SessionId::new(),
            1,
            &serialized,
            message,
            provenance,
            "store-authority",
        )
        .expect("generated classification succeeds")
    }

    #[test]
    fn ordinary_parent_text_is_context_only() {
        let row = classify(
            &Message::User(UserMessage::text("committed parent text")),
            LiveContextCommittedTextProvenance::ParentSessionServiceTurn,
        );
        assert_eq!(
            row.disposition(),
            LiveContextCommittedRowDisposition::MirrorParentText
        );
        assert_eq!(
            row.provider_context(),
            Some(r#"{"role":"user","text":"committed parent text"}"#)
        );
    }

    #[test]
    fn identical_live_transcript_text_is_never_echoed() {
        let row = classify(
            &Message::User(UserMessage::text("already in live channel")),
            LiveContextCommittedTextProvenance::LiveRealtimeTranscript,
        );
        assert_eq!(
            row.disposition(),
            LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel
        );
        assert_eq!(row.provider_context(), None);
    }

    #[test]
    fn executor_trace_is_excluded_even_when_text_shaped() {
        let row = classify(
            &Message::User(UserMessage::text("executor progress")),
            LiveContextCommittedTextProvenance::ExecutorTrace,
        );
        assert_eq!(
            row.disposition(),
            LiveContextCommittedRowDisposition::ExcludedFromLiveContext
        );
        assert_eq!(row.provider_context(), None);
    }
}
