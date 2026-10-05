//! Sealed bridge from exact committed session rows into generated live-context authority.

#[cfg(all(target_arch = "wasm32", feature = "live"))]
use crate::tokio;
use meerkat_core::SessionId;
use meerkat_core::generated::session_document::LiveContextCommittedRowDisposition;
#[cfg(feature = "live")]
use meerkat_core::generated::session_document::{
    LiveContextCommittedRowKind, LiveContextCommittedTextProvenance, SessionDocumentEffect,
    SessionDocumentKey, SessionDocumentMachineAuthority,
};
#[cfg(feature = "live")]
use meerkat_core::{AssistantBlock, Message};
#[cfg(feature = "live")]
use sha2::{Digest, Sha256};

use crate::live_execution::{
    LiveContextAmbiguityRecoveryAuthority, LiveContextAppendAuthority,
    LiveDelegationResultAmbiguityRecoveryAuthority,
};

/// Owned observation of one drain realization, independent of the canonical
/// commit caller. The generated outbox remains the delivery authority.
#[cfg(feature = "live")]
#[derive(Default)]
pub(crate) struct LiveContextDrainTask {
    pub generation: std::sync::atomic::AtomicU64,
    pub outcome: std::sync::Mutex<Option<Result<(), std::sync::Arc<crate::RuntimeDriverError>>>>,
    pub changed: tokio::sync::Notify,
    pub task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// A stopped delivery worker, not evidence that the provider consumed context.
#[cfg(feature = "live")]
#[derive(Debug, Clone)]
pub enum LiveContextDrainCompletion {
    Succeeded,
    Failed(std::sync::Arc<crate::RuntimeDriverError>),
}

#[cfg(feature = "live")]
impl LiveContextDrainTask {
    pub fn new() -> Self {
        Self {
            generation: std::sync::atomic::AtomicU64::new(0),
            outcome: std::sync::Mutex::new(None),
            changed: tokio::sync::Notify::new(),
            task: std::sync::Mutex::new(None),
        }
    }

    pub async fn wait(&self) -> Result<(), crate::RuntimeDriverError> {
        match self.wait_completion().await {
            LiveContextDrainCompletion::Succeeded => Ok(()),
            LiveContextDrainCompletion::Failed(error) => Err(crate::RuntimeDriverError::Internal(
                format!("owned live context delivery failed: {error}"),
            )),
        }
    }

    pub async fn wait_completion(&self) -> LiveContextDrainCompletion {
        loop {
            let changed = self.changed.notified();
            let outcome = self
                .outcome
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(outcome) = outcome {
                return match outcome {
                    Ok(()) => LiveContextDrainCompletion::Succeeded,
                    Err(error) => LiveContextDrainCompletion::Failed(error),
                };
            }
            changed.await;
        }
    }
}

#[cfg(feature = "live")]
impl Drop for LiveContextDrainTask {
    fn drop(&mut self) {
        if let Some(task) = self
            .task
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

/// Provider-neutral shell seam for canonical context delivery and generated
/// ambiguity recovery. Implementations may perform I/O, but receive no power
/// to select rows, cursor edges, or recovery channel identity.
#[async_trait::async_trait]
pub trait LiveContextMirrorHost: Send + Sync {
    /// Read canonical store-owned content after a runtime commit notification.
    /// The notification is only a wake signal, never transcript content.
    async fn committed_boundary(
        &self,
        _session_id: &SessionId,
    ) -> Result<
        (
            meerkat_core::lifecycle::core_executor::BoundSessionCommit,
            String,
        ),
        String,
    > {
        Err("live context host cannot export committed session authority".to_string())
    }

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

    /// Join the exact provider-control handoff before the source owner
    /// revalidates a prepared summary. This is resource readiness, not ACK.
    async fn wait_bootstrap_control_ready(
        &self,
        _lease: &crate::live_execution::LiveContextPreparationLease,
    ) -> Result<(), String> {
        Err("live context host cannot confirm bootstrap control readiness".into())
    }

    /// Quiet historical bootstrap. The authority binds the exact captured
    /// prefix, summary digest, channel, and cancellation fence.
    async fn append_bootstrap_context(
        &self,
        authority: crate::live_execution::LiveContextBootstrapAppendAuthority,
        _context: String,
    ) -> Result<
        (
            crate::live_execution::LiveContextBootstrapAppendAuthority,
            meerkat_core::LiveAppendDeliveryOutcome,
        ),
        String,
    > {
        Ok((authority, meerkat_core::LiveAppendDeliveryOutcome::Rejected))
    }

    async fn recover_ambiguous_append(
        &self,
        authority: LiveContextAmbiguityRecoveryAuthority,
    ) -> Result<(), String>;

    async fn recover_ambiguous_delegation_result(
        &self,
        authority: LiveDelegationResultAmbiguityRecoveryAuthority,
    ) -> Result<(), String>;

    /// Release publication custody for this exact generated close, including
    /// replacements that never activated. This does not authorize closing.
    #[cfg(feature = "live")]
    async fn retire_closed_channel(
        &self,
        _session_id: &SessionId,
        _authority: &meerkat_live::LiveChannelCloseCommitAuthority,
    ) {
    }
}

/// One exact store-committed canonical row classified by SessionDocument.
///
/// Construction is crate-private so a surface cannot manufacture commit or
/// provenance evidence. Retained payload availability is a sealed observation;
/// generated runtime authority decides ordinary delivery, causal reassertion,
/// or coverage without a provider send.
#[derive(Debug, Clone)]
pub struct CommittedLiveContextRow {
    session_id: SessionId,
    canonical_row_sequence: u64,
    content_digest: String,
    store_commit_authority: String,
    disposition: LiveContextCommittedRowDisposition,
    #[cfg(feature = "live")]
    source: crate::meerkat_machine::dsl::LiveContextRowSource,
    #[cfg(feature = "live")]
    author: crate::meerkat_machine::dsl::LiveContextRowAuthor,
    provider_context: Option<String>,
    causal_context: Option<String>,
    #[cfg(feature = "live")]
    observation_id: Option<meerkat_core::LiveContextObservationId>,
    /// The interaction of the turn that committed the row (its transcript
    /// identity): for a post-close merge reply, the voice delegation's own
    /// interaction, the typed key of its title.
    #[cfg(feature = "live")]
    interaction_id: Option<meerkat_core::interaction::InteractionId>,
}

impl CommittedLiveContextRow {
    #[cfg(feature = "live")]
    pub(crate) fn classify(
        session_id: &SessionId,
        canonical_row_sequence: u64,
        serialized_row: &[u8],
        message: &Message,
        provenance: LiveContextCommittedTextProvenance,
        store_commit_authority: &str,
    ) -> Result<Self, String> {
        let (ordinary_row_kind, provider_context) = context_projection(message)?;
        let observation_id = if provenance
            == LiveContextCommittedTextProvenance::LiveRealtimeTranscript
        {
            let origin = match message {
                Message::User(user) => user.identity.realtime_origin.as_ref(),
                Message::BlockAssistant(assistant) => assistant.identity.realtime_origin.as_ref(),
                _ => None,
            };
            let claim = origin
                .and_then(|origin| origin.context_observation_id())
                .cloned();
            if claim.is_none() {
                // An unsequenced live transcript row is never reasserted after
                // a bootstrap summary. That is the honest default for legacy
                // history; for a fresh row it means the channel stopped
                // admitting observations, which is worth seeing in logs.
                tracing::debug!(
                    "live transcript row carries no observation claim and will not be reasserted"
                );
            }
            claim
        } else {
            None
        };
        let causal_context = causal_context_projection(message)?;
        let row_kind = if ordinary_row_kind == LiveContextCommittedRowKind::NonText
            && causal_context.is_some()
            && provenance != LiveContextCommittedTextProvenance::LiveRealtimeTranscript
        {
            LiveContextCommittedRowKind::AssistantTranscript
        } else {
            ordinary_row_kind
        };
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
                | LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel
                | LiveContextCommittedRowDisposition::AssistantObservation
        )
        .then_some(provider_context)
        .flatten();
        let causal_context = matches!(
            disposition,
            LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel
                | LiveContextCommittedRowDisposition::AssistantObservation
        )
        .then_some(causal_context)
        .flatten();
        Ok(Self {
            session_id: session_id.clone(),
            canonical_row_sequence,
            content_digest,
            store_commit_authority: store_commit_authority.to_string(),
            disposition,
            source: crate::meerkat_machine::dsl::LiveContextRowSource::Conversation,
            author: row_author(message),
            provider_context,
            causal_context,
            observation_id,
            interaction_id: match message {
                Message::User(user) => user.identity.interaction_id,
                Message::BlockAssistant(assistant) => assistant.identity.interaction_id,
                _ => None,
            },
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
        matches!(
            self.disposition,
            LiveContextCommittedRowDisposition::MirrorParentText
        )
        .then_some(self.provider_context.as_deref())
        .flatten()
    }

    pub(crate) fn causal_context(&self) -> Option<&str> {
        self.causal_context
            .as_deref()
            .or(self.provider_context.as_deref())
    }

    #[cfg(feature = "live")]
    pub(crate) fn payload_availability(
        &self,
    ) -> crate::meerkat_machine::dsl::LiveContextPayloadAvailability {
        if self.causal_context().is_some() {
            crate::meerkat_machine::dsl::LiveContextPayloadAvailability::Materializable
        } else {
            crate::meerkat_machine::dsl::LiveContextPayloadAvailability::NoPayload
        }
    }

    /// Who authored this row (user input, assistant output, runtime context).
    #[cfg(feature = "live")]
    pub(crate) const fn author(&self) -> crate::meerkat_machine::dsl::LiveContextRowAuthor {
        self.author
    }

    /// The interaction of the turn that committed this row.
    #[cfg(feature = "live")]
    pub(crate) fn interaction_id(&self) -> Option<meerkat_core::interaction::InteractionId> {
        self.interaction_id
    }

    /// What drove the turn that committed this row (see
    /// [`classify_committed_boundary_rows_after`]).
    #[cfg(feature = "live")]
    pub(crate) fn source(&self) -> crate::meerkat_machine::dsl::LiveContextRowSource {
        self.source
    }

    #[cfg(feature = "live")]
    pub(crate) fn observation_id(&self) -> Option<&meerkat_core::LiveContextObservationId> {
        self.observation_id.as_ref()
    }
}

/// Author of a committed row as the live-context authority sees it: only a
/// conversational user row is the user's own input. Injected context and a
/// compaction summary ride a user message but are runtime-authored.
#[cfg(feature = "live")]
fn row_author(message: &Message) -> crate::meerkat_machine::dsl::LiveContextRowAuthor {
    use crate::meerkat_machine::dsl::LiveContextRowAuthor;
    match message {
        Message::User(user) if user.transcript_role.is_conversational() => {
            LiveContextRowAuthor::User
        }
        Message::BlockAssistant(_) => LiveContextRowAuthor::Assistant,
        _ => LiveContextRowAuthor::Runtime,
    }
}

#[cfg(feature = "live")]
pub(crate) fn classify_committed_boundary_rows_after(
    session_id: &SessionId,
    committed: &meerkat_core::lifecycle::core_executor::BoundSessionCommit,
    canonical_cursor: u64,
    channel_id: &meerkat_core::LiveChannelId,
    store_commit_authority: &str,
    existing_member_interactions: &std::collections::BTreeSet<String>,
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
            let source = row_source(&message);
            let origin = match &message {
                Message::User(user) => user.identity.realtime_origin.as_ref(),
                Message::BlockAssistant(assistant) => assistant.identity.realtime_origin.as_ref(),
                _ => None,
            };
            let provenance =
                if origin.is_some_and(|origin| origin.matches(session_id, channel_id, sequence)) {
                    LiveContextCommittedTextProvenance::LiveRealtimeTranscript
                } else if matches!(&message, Message::BlockAssistant(assistant)
                    if assistant.identity.interaction_id.is_some_and(|interaction|
                        existing_member_interactions.contains(&interaction.to_string())))
                {
                    LiveContextCommittedTextProvenance::ExecutorTrace
                } else {
                    LiveContextCommittedTextProvenance::ParentSessionServiceTurn
                };
            CommittedLiveContextRow::classify(
                session_id,
                sequence,
                &serialized,
                &message,
                provenance,
                store_commit_authority,
            )
            .map(|row| CommittedLiveContextRow { source, ..row })
        })
        .collect()
}

/// What drove the turn that committed a row. The member's reply to
/// runtime-authored injected execution context (such as a post-close result
/// merge) carries `TranscriptTurnInput::RuntimeAuthored` on its identity,
/// stamped at admission from the turn's work attribution; it is runtime work
/// output, replayed quietly instead of voiced. A host-typed turn (the text
/// chat) stamps `TranscriptTurnInput::TypedText` on its user row and its
/// reply: the user typed and read them in the chat, so they are the
/// `TextChat` source, which updates what the voice knows on the quiet lane and
/// is never voiced (#1614, S105 R3: a voiced typed correction made the model
/// replay stale results aloud). They stay typed rows to the machine: newer
/// heard speech supersedes them (S99).
/// Every other row (spoken, or a peer-driven reply such as the answer to a
/// delegated question) is conversation and is voiced. Transcript position is
/// never consulted: a mid-turn steer can follow a typed row, and a reply can
/// commit in a later boundary than its input.
#[cfg(feature = "live")]
fn row_source(message: &Message) -> crate::meerkat_machine::dsl::LiveContextRowSource {
    use crate::meerkat_machine::dsl::LiveContextRowSource;
    use meerkat_core::types::TranscriptTurnInput;
    let turn_input = match message {
        Message::BlockAssistant(assistant) => assistant.identity.turn_input,
        Message::User(user) => user.identity.turn_input,
        _ => None,
    };
    match (message, turn_input) {
        (Message::BlockAssistant(_), Some(TranscriptTurnInput::RuntimeAuthored)) => {
            LiveContextRowSource::RuntimeWork
        }
        (Message::BlockAssistant(_) | Message::User(_), Some(TranscriptTurnInput::TypedText)) => {
            LiveContextRowSource::TextChat
        }
        _ => LiveContextRowSource::Conversation,
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg(feature = "live")]
enum LiveAssistantContextBlock<'a> {
    Text {
        text: &'a str,
    },
    Transcript {
        text: std::borrow::Cow<'a, str>,
        source: meerkat_core::types::TranscriptSource,
    },
}

#[derive(serde::Serialize)]
#[cfg(feature = "live")]
struct LiveAssistantContext<'a> {
    role: &'static str,
    blocks: Vec<LiveAssistantContextBlock<'a>>,
}

#[cfg(feature = "live")]
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

#[cfg(feature = "live")]
fn causal_context_projection(message: &Message) -> Result<Option<String>, String> {
    let Message::BlockAssistant(assistant) = message else {
        return Ok(None);
    };
    if !assistant.blocks.iter().any(|block| {
        matches!(
            block, AssistantBlock::Transcript { text, .. } if !text.trim().is_empty()
        )
    }) {
        return Ok(None);
    }
    let blocks = assistant
        .blocks
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::Text { text, .. } if !text.trim().is_empty() => {
                Some(LiveAssistantContextBlock::Text { text })
            }
            AssistantBlock::Transcript { text, source, .. } if !text.trim().is_empty() => {
                Some(LiveAssistantContextBlock::Transcript {
                    text: source.text_for_model(text),
                    source: *source,
                })
            }
            _ => None,
        })
        .collect();
    serde_json::to_string(&LiveAssistantContext {
        role: "assistant",
        blocks,
    })
    .map(Some)
    .map_err(|error| error.to_string())
}

// These tests exercise the live-only classification path. The crate's
// self dev-dependency enables `live`, so package-scoped test lanes run them.
#[cfg(all(test, feature = "live"))]
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

    /// A committed row keeps the interaction of the turn that committed it:
    /// the typed key a post-close merge reply's title is recorded under.
    #[test]
    fn a_committed_row_keeps_its_turn_interaction() {
        let interaction = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());
        let mut reply =
            meerkat_core::types::BlockAssistantMessage::snapshot(vec![AssistantBlock::Text {
                text: "The completed ode reads: O coffee".into(),
                meta: None,
            }]);
        reply.identity.interaction_id = Some(interaction);
        let row = classify(
            &Message::BlockAssistant(reply),
            LiveContextCommittedTextProvenance::ParentSessionServiceTurn,
        );
        assert_eq!(row.interaction_id(), Some(interaction));
        let plain = classify(
            &Message::User(UserMessage::text("no interaction")),
            LiveContextCommittedTextProvenance::ParentSessionServiceTurn,
        );
        assert_eq!(plain.interaction_id(), None);
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

    /// The source is read from the reply's stamped turn authorship, never
    /// from transcript position.
    #[test]
    fn row_source_reads_the_stamped_turn_authorship() {
        use crate::meerkat_machine::dsl::LiveContextRowSource::{Conversation, RuntimeWork};
        let reply = |text: &str, runtime_authored: bool| {
            let mut message =
                meerkat_core::types::BlockAssistantMessage::snapshot(vec![AssistantBlock::Text {
                    text: text.into(),
                    meta: None,
                }]);
            if runtime_authored {
                message.identity.turn_input =
                    Some(meerkat_core::types::TranscriptTurnInput::RuntimeAuthored);
            }
            Message::BlockAssistant(message)
        };
        let mut steer = UserMessage::text("a durable mid-turn steer");
        steer.transcript_role = meerkat_core::types::TranscriptUserRole::InjectedContext;

        // (a) A typed turn followed by a durable mid-turn steer (injected
        // context) and the reply: the reply stays conversation.
        let typed_with_steer = [
            Message::User(UserMessage::text("typed turn")),
            Message::User(steer.clone()),
            reply("Noted.", false),
        ];
        assert_eq!(
            typed_with_steer.iter().map(row_source).collect::<Vec<_>>(),
            vec![Conversation, Conversation, Conversation]
        );

        // (b) A merge reply committed alone, in a later boundary than its
        // injected input, stays runtime work.
        assert_eq!(
            row_source(&reply("Done, the file is written.", true)),
            RuntimeWork
        );

        let merged = classify(
            &reply("Done, the file is written.", true),
            LiveContextCommittedTextProvenance::ParentSessionServiceTurn,
        );
        assert_eq!(
            merged.disposition(),
            LiveContextCommittedRowDisposition::MirrorParentText
        );
        assert_eq!(
            classify(
                &Message::User(steer),
                LiveContextCommittedTextProvenance::ParentSessionServiceTurn
            )
            .disposition(),
            LiveContextCommittedRowDisposition::ExcludedFromLiveContext
        );
    }

    /// #1614 (S105 R3): a host-typed turn's user row and reply carry
    /// `TypedText` and are the text-chat source, replayed quietly; an
    /// unstamped reply, such as the executor's answer to a peer's
    /// response, stays conversation and is voiced.
    #[test]
    fn typed_text_turn_rows_are_quiet_text_chat_and_peer_replies_stay_voiced() {
        use crate::meerkat_machine::dsl::LiveContextRowSource::{Conversation, TextChat};
        use meerkat_core::types::TranscriptTurnInput;
        let mut typed = UserMessage::text("Correction: the numbers are 21 and 42.");
        typed.identity.turn_input = Some(TranscriptTurnInput::TypedText);
        let reply = |text: &str, turn_input: Option<TranscriptTurnInput>| {
            let mut message =
                meerkat_core::types::BlockAssistantMessage::snapshot(vec![AssistantBlock::Text {
                    text: text.into(),
                    meta: None,
                }]);
            message.identity.turn_input = turn_input;
            Message::BlockAssistant(message)
        };
        let typed_reply = reply(
            "The numbers are 21 and 42.",
            Some(TranscriptTurnInput::TypedText),
        );
        let peer_reply = reply("Pemberton replied: 13 UTC.", None);

        assert_eq!(row_source(&Message::User(typed.clone())), TextChat);
        assert_eq!(row_source(&typed_reply), TextChat);
        assert_eq!(row_source(&peer_reply), Conversation);

        for message in [Message::User(typed), typed_reply, peer_reply] {
            let row = classify(
                &message,
                LiveContextCommittedTextProvenance::ParentSessionServiceTurn,
            );
            assert_eq!(
                row.disposition(),
                LiveContextCommittedRowDisposition::MirrorParentText,
                "a materializable parent text row either way; the generated \
                 enqueue maps the text-chat source to ReplayTextChat"
            );
        }
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
    fn causal_assistant_snapshot_retains_typed_unmeasured_provenance_without_strict_echo() {
        let message =
            Message::BlockAssistant(meerkat_core::types::BlockAssistantMessage::snapshot(vec![
                AssistantBlock::Transcript {
                    text: "earlier unmeasured assistant observation".into(),
                    source: meerkat_core::types::TranscriptSource::SpokenUnmeasured,
                    meta: None,
                },
            ]));
        assert_eq!(
            context_projection(&message).expect("ordinary projection"),
            (LiveContextCommittedRowKind::NonText, None)
        );
        let row = classify(
            &message,
            LiveContextCommittedTextProvenance::LiveRealtimeTranscript,
        );
        assert_eq!(
            row.disposition(),
            LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel
        );
        assert!(
            row.provider_context().is_none(),
            "strict delivery still does not echo live speech"
        );
        let causal = row
            .causal_context()
            .expect("causal payload retained for generated reassertion");
        assert!(causal.contains("earlier unmeasured assistant observation"));
        assert!(causal.contains("spoken_unmeasured"));
        assert!(causal.contains("Not proof of"));
        let ordinary = classify(
            &message,
            LiveContextCommittedTextProvenance::ParentSessionServiceTurn,
        );
        assert_eq!(
            ordinary.disposition(),
            LiveContextCommittedRowDisposition::AssistantObservation
        );
        assert!(ordinary.provider_context().is_none());
    }

    #[test]
    fn mixed_assistant_rows_keep_ordinary_text_and_separate_observational_context() {
        let message =
            Message::BlockAssistant(meerkat_core::types::BlockAssistantMessage::snapshot(vec![
                AssistantBlock::Text {
                    text: "written text".into(),
                    meta: None,
                },
                AssistantBlock::Transcript {
                    text: "unmeasured audio".into(),
                    source: meerkat_core::types::TranscriptSource::SpokenUnmeasured,
                    meta: None,
                },
            ]));
        let legacy = r#"{"role":"assistant","text":"written text"}"#;
        assert_eq!(
            context_projection(&message).expect("ordinary projection"),
            (
                LiveContextCommittedRowKind::AssistantText,
                Some(legacy.into())
            )
        );
        let ordinary = classify(
            &message,
            LiveContextCommittedTextProvenance::ParentSessionServiceTurn,
        );
        assert_eq!(ordinary.provider_context(), Some(legacy));
        assert_eq!(
            ordinary.causal_context(),
            Some(legacy),
            "foreign/ordinary rows expose no extra speech payload"
        );
        let live = classify(
            &message,
            LiveContextCommittedTextProvenance::LiveRealtimeTranscript,
        );
        assert_eq!(
            live.disposition(),
            LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel
        );
        assert!(
            live.provider_context().is_none(),
            "strict live no-echo is unchanged"
        );
        let causal = live.causal_context().expect("observational causal payload");
        assert!(causal.contains("written text"));
        assert!(causal.contains("unmeasured audio"));
        assert!(causal.contains("spoken_unmeasured"));
        assert!(causal.contains("Not proof of"));
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

    #[test]
    fn non_text_live_rows_never_create_unrealizable_causal_appends() {
        let row = classify(
            &Message::User(UserMessage::text("   ")),
            LiveContextCommittedTextProvenance::LiveRealtimeTranscript,
        );
        assert_eq!(
            row.disposition(),
            LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel
        );
        assert!(row.causal_context().is_none());
        assert_eq!(
            row.payload_availability(),
            crate::meerkat_machine::dsl::LiveContextPayloadAvailability::NoPayload
        );
    }

    #[test]
    fn same_channel_assistant_speech_without_an_observation_is_present_not_reasserted() {
        // An assistant row committed from the live channel itself (realtime
        // materialization, no context observation) now carries a channel
        // origin. Its disposition: already present in the channel, so it is
        // never mirrored back, and with no observation claim it is not
        // reasserted after a bootstrap summary either; the summary path is
        // what carries it forward.
        let session_id = SessionId::new();
        let channel_id = meerkat_core::LiveChannelId::new("voice-origin-test");
        let mut session = meerkat_core::Session::with_id(session_id.clone());
        let mut assistant = meerkat_core::types::BlockAssistantMessage::new(
            vec![AssistantBlock::Transcript {
                text: "spoken on this call".to_string(),
                source: meerkat_core::types::TranscriptSource::Spoken,
                meta: None,
            }],
            meerkat_core::StopReason::EndTurn,
        );
        assistant.identity.realtime_origin = Some(
            serde_json::from_value(serde_json::json!({
                "session_id": session_id,
                "channel_id": channel_id,
                "canonical_row_sequence": 1,
                "provider_item_ids": ["item_spoken"],
            }))
            .expect("realtime origin"),
        );
        session.push(Message::BlockAssistant(assistant));
        let committed = meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(
            std::sync::Arc::new(session),
        )
        .expect("seal exact committed messages");
        let rows = classify_committed_boundary_rows_after(
            &session_id,
            &committed,
            0,
            &channel_id,
            "store-receipt",
            &std::collections::BTreeSet::new(),
        )
        .expect("classify same-channel assistant speech");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].disposition(),
            LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel
        );
        assert!(rows[0].provider_context().is_none());
        assert!(
            rows[0].observation_id().is_none(),
            "no observation claim: the row is not reasserted after a bootstrap summary"
        );
    }

    #[test]
    fn existing_voice_assistant_is_excluded_but_later_ordinary_assistant_is_mirrored() {
        let session_id = SessionId::new();
        let voice_interaction = meerkat_core::InteractionId::new();
        let ordinary_interaction = meerkat_core::InteractionId::new();
        let mut session = meerkat_core::Session::with_id(session_id.clone());
        for interaction in [voice_interaction, ordinary_interaction] {
            let mut assistant = meerkat_core::types::BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: "same factual result".to_string(),
                    meta: None,
                }],
                meerkat_core::StopReason::EndTurn,
            );
            assistant.identity.interaction_id = Some(interaction);
            session.push(Message::BlockAssistant(assistant));
        }
        let committed = meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(
            std::sync::Arc::new(session),
        )
        .expect("seal exact committed messages");
        let rows = classify_committed_boundary_rows_after(
            &session_id,
            &committed,
            0,
            &meerkat_core::LiveChannelId::new("voice-origin-test"),
            "store-receipt",
            &std::collections::BTreeSet::from([voice_interaction.to_string()]),
        )
        .expect("classify source-owned interaction provenance");
        assert_eq!(
            rows[0].disposition(),
            LiveContextCommittedRowDisposition::ExcludedFromLiveContext
        );
        assert!(rows[0].provider_context().is_none());
        assert_eq!(
            rows[1].disposition(),
            LiveContextCommittedRowDisposition::MirrorParentText
        );
        assert!(rows[1].provider_context().is_some());
    }
}
