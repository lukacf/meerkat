//! Session-owned bridge from generated SessionDocument effects to sealed live
//! final-user-input evidence.

use meerkat_core::generated::session_document::SessionDocumentEffect;
use meerkat_core::generated::session_document::{
    LiveTranscriptReconciliation, SessionDocumentKey, SessionDocumentMachineAuthority,
};
use meerkat_core::{
    FinalLiveUserTranscriptCommitError, FinalLiveUserTranscriptCommitEvidence, InteractionId,
    LiveAssistantPlaybackEvidence, LiveAssistantPlaybackTruncationDisposition,
    LiveAssistantPlaybackTruncationError, LiveAssistantPlaybackTruncationEvidence, LiveChannelId,
    NormalizedLiveUserInputDigest, ProvisionalLiveHandoff, RealtimeTranscriptEvent,
    RealtimeTranscriptMaterializedMessage, SessionId,
};
use sha2::{Digest, Sha256};

use crate::ephemeral::SessionAgent;

struct LiveUserTranscriptGeneratedAuthorityBridgeToken;

static LIVE_USER_TRANSCRIPT_GENERATED_AUTHORITY_BRIDGE_TOKEN:
    LiveUserTranscriptGeneratedAuthorityBridgeToken =
    LiveUserTranscriptGeneratedAuthorityBridgeToken;

fn live_user_transcript_generated_authority_bridge_token()
-> &'static (dyn std::any::Any + Send + Sync) {
    &LIVE_USER_TRANSCRIPT_GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_session_generated_authority_bridge_token_is_valid_v1_live_user_transcript_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub extern "Rust" fn live_user_transcript_generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<LiveUserTranscriptGeneratedAuthorityBridgeToken>()
}

pub(crate) fn seal_final_live_user_transcript_commit(
    session_id: SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    normalized_final_input_digest: Option<NormalizedLiveUserInputDigest>,
    committed_message_count: Option<usize>,
    effect: &SessionDocumentEffect,
) -> Result<FinalLiveUserTranscriptCommitEvidence, FinalLiveUserTranscriptCommitError> {
    #[allow(improper_ctypes_definitions, unsafe_code)]
    unsafe extern "Rust" {
        #[link_name = concat!(
            "__meerkat_core_session_generated_live_user_transcript_commit_build_v2_",
            env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
        )]
        fn core_session_generated_live_user_transcript_commit_build(
            token: &'static (dyn std::any::Any + Send + Sync),
            session_id: SessionId,
            channel_id: LiveChannelId,
            interaction_id: InteractionId,
            normalized_final_input_digest: Option<NormalizedLiveUserInputDigest>,
            committed_message_count: Option<usize>,
            effect: &SessionDocumentEffect,
        ) -> Result<FinalLiveUserTranscriptCommitEvidence, FinalLiveUserTranscriptCommitError>;
    }

    #[allow(unsafe_code)]
    unsafe {
        core_session_generated_live_user_transcript_commit_build(
            live_user_transcript_generated_authority_bridge_token(),
            session_id,
            channel_id,
            interaction_id,
            normalized_final_input_digest,
            committed_message_count,
            effect,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn seal_live_assistant_playback_truncation(
    session_id: SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    response_id: &str,
    item_id: &str,
    content_index: u32,
    evidence: &LiveAssistantPlaybackEvidence,
    effect: &SessionDocumentEffect,
) -> Result<LiveAssistantPlaybackTruncationEvidence, LiveAssistantPlaybackTruncationError> {
    #[allow(improper_ctypes_definitions, unsafe_code)]
    unsafe extern "Rust" {
        #[link_name = concat!(
            "__meerkat_core_session_generated_live_playback_truncation_build_v1_",
            env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
        )]
        fn core_session_generated_live_playback_truncation_build(
            token: &'static (dyn std::any::Any + Send + Sync),
            session_id: SessionId,
            channel_id: LiveChannelId,
            interaction_id: InteractionId,
            response_id: &str,
            item_id: &str,
            content_index: u32,
            evidence: &LiveAssistantPlaybackEvidence,
            effect: &SessionDocumentEffect,
        ) -> Result<LiveAssistantPlaybackTruncationEvidence, LiveAssistantPlaybackTruncationError>;
    }

    #[allow(unsafe_code)]
    unsafe {
        core_session_generated_live_playback_truncation_build(
            live_user_transcript_generated_authority_bridge_token(),
            session_id,
            channel_id,
            interaction_id,
            response_id,
            item_id,
            content_index,
            evidence,
            effect,
        )
    }
}

struct PreparedLiveUserTranscriptCommit {
    authority: SessionDocumentMachineAuthority,
    session_key: SessionDocumentKey,
    session_id: SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
}

impl PreparedLiveUserTranscriptCommit {
    fn prepare(
        session_id: &SessionId,
        provisional: &ProvisionalLiveHandoff,
    ) -> Result<Self, meerkat_core::error::AgentError> {
        let correlation = provisional.correlation();
        let session_key = SessionDocumentKey::new(session_id.to_string());
        let channel_id = correlation.channel_id().clone();
        let interaction_id = correlation.interaction_id();
        let mut authority = SessionDocumentMachineAuthority::new();
        authority
            .admit_live_interaction_transcript(
                session_key.clone(),
                channel_id.to_string(),
                interaction_id.to_string(),
            )
            .map_err(session_document_error)?;
        authority
            .stage_live_provisional_user_transcript(
                session_key.clone(),
                channel_id.to_string(),
                interaction_id.to_string(),
                !provisional.executor_input().is_empty(),
            )
            .map_err(session_document_error)?;
        Ok(Self {
            authority,
            session_key,
            session_id: session_id.clone(),
            channel_id,
            interaction_id,
        })
    }

    fn finish(
        mut self,
        normalized_final_input_digest: Option<NormalizedLiveUserInputDigest>,
        committed_message_count: Option<usize>,
    ) -> Result<FinalLiveUserTranscriptCommitEvidence, meerkat_core::error::AgentError> {
        let reconciliation = if normalized_final_input_digest.is_some() {
            LiveTranscriptReconciliation::Committed
        } else {
            LiveTranscriptReconciliation::Missing
        };
        let effects = self
            .authority
            .reconcile_live_final_user_transcript(
                self.session_key,
                self.channel_id.to_string(),
                self.interaction_id.to_string(),
                reconciliation,
            )
            .map_err(session_document_error)?;
        let effect = effects
            .iter()
            .find(|effect| {
                matches!(
                    effect,
                    SessionDocumentEffect::LiveFinalUserTranscriptReconciled { .. }
                )
            })
            .ok_or_else(|| {
                meerkat_core::error::AgentError::InternalError(
                    "SessionDocument final live transcript emitted no terminal effect".to_string(),
                )
            })?;
        seal_final_live_user_transcript_commit(
            self.session_id,
            self.channel_id,
            self.interaction_id,
            normalized_final_input_digest,
            committed_message_count,
            effect,
        )
        .map_err(|error| meerkat_core::error::AgentError::InternalError(error.to_string()))
    }
}

fn session_document_error(
    error: meerkat_core::generated::session_document::SessionDocumentError,
) -> meerkat_core::error::AgentError {
    meerkat_core::error::AgentError::InternalError(format!(
        "SessionDocument live transcript authority rejected transition: {error}"
    ))
}

pub enum LiveAssistantPlaybackObservationResult {
    Pending,
    Resolved(LiveAssistantPlaybackTruncationEvidence),
}

impl LiveAssistantPlaybackObservationResult {
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved(_))
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn admit_live_assistant_playback_target(
    agent: &mut dyn SessionAgent,
    session_id: &SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    response_id: String,
    item_id: String,
    content_index: u32,
) -> Result<meerkat_core::LiveAssistantPlaybackTarget, meerkat_core::error::AgentError> {
    if let Some(existing) =
        agent.live_assistant_playback_target(&channel_id, &item_id, content_index)
    {
        if existing.interaction_id() == interaction_id && existing.response_id() == response_id {
            return Ok(existing);
        }
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live assistant playback target admission mismatch".to_string(),
        ));
    }
    let session_key = SessionDocumentKey::new(session_id.to_string());
    let mut authority = SessionDocumentMachineAuthority::new();
    authority
        .admit_live_interaction_transcript(
            session_key.clone(),
            channel_id.to_string(),
            interaction_id.to_string(),
        )
        .map_err(session_document_error)?;
    let effects = authority
        .admit_live_assistant_playback_target(
            session_key,
            channel_id.to_string(),
            interaction_id.to_string(),
            response_id.clone(),
            item_id.clone(),
            u64::from(content_index),
        )
        .map_err(session_document_error)?;
    if !effects.iter().any(|effect| {
        matches!(
            effect,
            SessionDocumentEffect::LiveAssistantPlaybackTargetAdmitted {
                channel_id: effect_channel,
                interaction_id: effect_interaction,
                response_id: effect_response,
                item_id: effect_item,
                content_index: effect_index,
                ..
            } if effect_channel == channel_id.as_str()
                && effect_interaction == &interaction_id.to_string()
                && effect_response == &response_id
                && effect_item == &item_id
                && *effect_index == u64::from(content_index)
        )
    }) {
        return Err(meerkat_core::error::AgentError::InternalError(
            "SessionDocument emitted no exact playback target admission".to_string(),
        ));
    }
    agent.admit_live_assistant_playback_target(
        &channel_id,
        interaction_id,
        &response_id,
        &item_id,
        content_index,
    )
}

pub(crate) fn commit_final_live_user_transcript(
    agent: &mut dyn SessionAgent,
    session_id: &SessionId,
    provisional: ProvisionalLiveHandoff,
    final_event: Option<RealtimeTranscriptEvent>,
) -> Result<FinalLiveUserTranscriptCommitEvidence, meerkat_core::error::AgentError> {
    let prepared = PreparedLiveUserTranscriptCommit::prepare(session_id, &provisional)?;
    let Some(final_event) = final_event else {
        return prepared.finish(None, None);
    };
    let RealtimeTranscriptEvent::UserTranscriptFinal { item_id, text, .. } = &final_event else {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live final-user commit requires UserTranscriptFinal".to_string(),
        ));
    };
    let item_id = item_id.clone();
    let text = text.clone();
    if item_id != provisional.correlation().provider().user_turn_id() {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live final-user commit provider correlation mismatch".to_string(),
        ));
    }
    let digest = NormalizedLiveUserInputDigest::derive(&text)
        .map_err(|error| meerkat_core::error::AgentError::ConfigError(error.to_string()))?;
    let outcome = agent.append_realtime_transcript_event(final_event)?;
    let canonical_commit_observed = outcome.materialized_messages.iter().any(|materialized| {
        matches!(
            materialized,
            RealtimeTranscriptMaterializedMessage::User {
                item_id: committed_item_id,
                text: committed_text,
            } if committed_item_id == &item_id && committed_text == &text
        )
    });
    if !canonical_commit_observed {
        return Err(meerkat_core::error::AgentError::InternalError(
            "live final-user transcript did not become canonical in this commit".to_string(),
        ));
    }
    let committed_message_count = agent.snapshot().message_count;
    prepared.finish(Some(digest), Some(committed_message_count))
}

/// Classify one exact playback-prefix observation through SessionDocument
/// authority and, only for an authorized reported prefix, apply the existing
/// canonical assistant truncation event in the same session actor.
#[allow(clippy::too_many_arguments)]
pub(crate) fn commit_live_assistant_playback_truncation(
    agent: &mut dyn SessionAgent,
    session_id: &SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    response_id: String,
    item_id: String,
    content_index: u32,
    evidence: LiveAssistantPlaybackEvidence,
) -> Result<LiveAssistantPlaybackTruncationEvidence, meerkat_core::error::AgentError> {
    match observe_live_assistant_playback_terminal(
        agent,
        session_id,
        channel_id,
        interaction_id,
        response_id,
        item_id,
        content_index,
        evidence,
        None,
    )? {
        LiveAssistantPlaybackObservationResult::Resolved(receipt) => Ok(receipt),
        LiveAssistantPlaybackObservationResult::Pending => {
            Err(meerkat_core::error::AgentError::ConfigError(
                "playback truncation arrived before final without completion custody".to_string(),
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn commit_live_assistant_playback_complete(
    agent: &mut dyn SessionAgent,
    session_id: &SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    response_id: String,
    item_id: String,
    content_index: u32,
    stop_reason: meerkat_core::StopReason,
    usage: meerkat_core::TurnUsage,
) -> Result<LiveAssistantPlaybackTruncationEvidence, meerkat_core::error::AgentError> {
    match observe_live_assistant_playback_terminal(
        agent,
        session_id,
        channel_id,
        interaction_id,
        response_id,
        item_id,
        content_index,
        LiveAssistantPlaybackEvidence::PlaybackComplete,
        Some((stop_reason, usage)),
    )? {
        LiveAssistantPlaybackObservationResult::Resolved(receipt) => Ok(receipt),
        LiveAssistantPlaybackObservationResult::Pending => {
            Err(meerkat_core::error::AgentError::ConfigError(
                "legacy playback completion cannot retain a pre-final terminal".to_string(),
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn observe_live_assistant_playback_terminal_with_completion(
    agent: &mut dyn SessionAgent,
    session_id: &SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    response_id: String,
    item_id: String,
    content_index: u32,
    evidence: LiveAssistantPlaybackEvidence,
    stop_reason: meerkat_core::StopReason,
    usage: meerkat_core::TurnUsage,
) -> Result<LiveAssistantPlaybackObservationResult, meerkat_core::error::AgentError> {
    observe_live_assistant_playback_terminal(
        agent,
        session_id,
        channel_id,
        interaction_id,
        response_id,
        item_id,
        content_index,
        evidence,
        Some((stop_reason, usage)),
    )
}

/// Resolve the one durable playback target on channel close. This uses the
/// close-specific SessionDocument transition, emits Unmeasured with no
/// canonical text or hearing claim, discards the staged response, and clears
/// the session target before channel terminality can commit.
pub(crate) fn resolve_live_assistant_playback_on_channel_close(
    agent: &mut dyn SessionAgent,
    session_id: &SessionId,
    channel_id: LiveChannelId,
) -> Result<Option<LiveAssistantPlaybackTruncationEvidence>, meerkat_core::error::AgentError> {
    let Some(target) = agent.live_assistant_playback_target_for_channel(&channel_id) else {
        return Ok(None);
    };
    let session_key = SessionDocumentKey::new(session_id.to_string());
    let mut authority = SessionDocumentMachineAuthority::new();
    authority
        .recover_live_assistant_playback_target(
            session_key.clone(),
            channel_id.to_string(),
            target.interaction_id().to_string(),
            target.response_id().to_string(),
            target.item_id().to_string(),
            u64::from(target.content_index()),
        )
        .map_err(session_document_error)?;
    let effects = authority
        .resolve_live_assistant_playback_on_channel_close(
            session_key,
            channel_id.to_string(),
            target.interaction_id().to_string(),
            target.response_id().to_string(),
            target.item_id().to_string(),
            u64::from(target.content_index()),
        )
        .map_err(session_document_error)?;
    let effect = effects
        .iter()
        .find(|effect| {
            matches!(
                effect,
                SessionDocumentEffect::LiveAssistantPlaybackTerminalResolved { .. }
            )
        })
        .ok_or_else(|| {
            meerkat_core::error::AgentError::InternalError(
                "SessionDocument close emitted no playback terminal effect".to_string(),
            )
        })?;
    let evidence = LiveAssistantPlaybackEvidence::Unmeasured;
    let receipt = seal_live_assistant_playback_truncation(
        session_id.clone(),
        channel_id.clone(),
        target.interaction_id(),
        target.response_id(),
        target.item_id(),
        target.content_index(),
        &evidence,
        effect,
    )
    .map_err(|error| meerkat_core::error::AgentError::InternalError(error.to_string()))?;
    if receipt.disposition() != LiveAssistantPlaybackTruncationDisposition::Unmeasured
        || receipt.biological_hearing_claimed()
        || receipt.canonical_prefix_chars().is_some()
    {
        return Err(meerkat_core::error::AgentError::InternalError(
            "channel-close playback authority made a canonical or hearing claim".to_string(),
        ));
    }
    if agent
        .staged_realtime_assistant_segment_text(
            target.response_id(),
            target.item_id(),
            target.content_index(),
        )
        .is_some()
    {
        let _ = agent.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnInterrupted {
                response_id: target.response_id().to_string(),
            },
        )?;
    }
    agent.resolve_live_assistant_playback_target(
        &channel_id,
        target.interaction_id(),
        target.response_id(),
        target.item_id(),
        target.content_index(),
    )?;
    Ok(Some(receipt))
}

#[allow(clippy::too_many_arguments)]
fn observe_live_assistant_playback_terminal(
    agent: &mut dyn SessionAgent,
    session_id: &SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    response_id: String,
    item_id: String,
    content_index: u32,
    evidence: LiveAssistantPlaybackEvidence,
    completion: Option<(meerkat_core::StopReason, meerkat_core::TurnUsage)>,
) -> Result<LiveAssistantPlaybackObservationResult, meerkat_core::error::AgentError> {
    if response_id.trim().is_empty() || item_id.trim().is_empty() {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live playback truncation requires non-empty response and item identity".to_string(),
        ));
    }

    let target = agent
        .live_assistant_playback_target(&channel_id, &item_id, content_index)
        .ok_or_else(|| {
            meerkat_core::error::AgentError::ConfigError(
                "live playback terminal has no exact active target".to_string(),
            )
        })?;
    if target.interaction_id() != interaction_id || target.response_id() != response_id {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live playback terminal target identity mismatch".to_string(),
        ));
    }
    if target.pending_terminal().is_some() {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live playback terminal is already retained for this target".to_string(),
        ));
    }

    let authoritative_text = agent
        .staged_realtime_assistant_segment_text(&response_id, &item_id, content_index)
        .unwrap_or_default();
    let authoritative_final =
        agent.staged_realtime_assistant_segment_is_final(&response_id, &item_id, content_index);
    let authoritative_chars = authoritative_text.chars().count() as u64;
    let authoritative_digest = if authoritative_final {
        text_digest(&authoritative_text)
    } else {
        String::new()
    };
    let (observation, reported_prefix_chars, reported_prefix_digest, prefix_matches) = match &evidence {
        LiveAssistantPlaybackEvidence::PlaybackComplete => (
            meerkat_core::generated::session_document::LiveAssistantPlaybackTerminalObservation::PlaybackComplete,
            0,
            String::new(),
            false,
        ),
        LiveAssistantPlaybackEvidence::ReportedPrefix(prefix) => {
            (
                meerkat_core::generated::session_document::LiveAssistantPlaybackTerminalObservation::ReportedPrefix,
                prefix.chars().count() as u64,
                text_digest(prefix),
                authoritative_text.starts_with(prefix),
            )
        }
        LiveAssistantPlaybackEvidence::Unmeasured => (
            meerkat_core::generated::session_document::LiveAssistantPlaybackTerminalObservation::Unmeasured,
            0,
            String::new(),
            false,
        ),
    };

    let session_key = SessionDocumentKey::new(session_id.to_string());
    let mut authority = SessionDocumentMachineAuthority::new();
    authority
        .recover_live_assistant_playback_target(
            session_key.clone(),
            channel_id.to_string(),
            interaction_id.to_string(),
            response_id.clone(),
            item_id.clone(),
            u64::from(content_index),
        )
        .map_err(session_document_error)?;
    if authoritative_final {
        authority
            .recover_live_assistant_playback_final(
                session_key.clone(),
                channel_id.to_string(),
                interaction_id.to_string(),
                response_id.clone(),
                item_id.clone(),
                u64::from(content_index),
                authoritative_chars,
                authoritative_digest.clone(),
            )
            .map_err(session_document_error)?;
    }
    let effects = authority
        .observe_live_assistant_playback_terminal(
            session_key,
            channel_id.to_string(),
            interaction_id.to_string(),
            response_id.clone(),
            item_id.clone(),
            u64::from(content_index),
            observation,
            reported_prefix_chars,
            reported_prefix_digest,
            authoritative_chars,
            authoritative_digest,
            authoritative_final,
            prefix_matches,
        )
        .map_err(session_document_error)?;
    if effects.iter().any(|effect| {
        matches!(
            effect,
            SessionDocumentEffect::LiveAssistantPlaybackTerminalObserved { .. }
        )
    }) {
        let (stop_reason, usage) = completion.ok_or_else(|| {
            meerkat_core::error::AgentError::ConfigError(
                "pre-final playback terminal requires retained completion facts".to_string(),
            )
        })?;
        agent.observe_live_assistant_playback_terminal(
            &channel_id,
            interaction_id,
            &response_id,
            &item_id,
            content_index,
            evidence,
            stop_reason,
            usage,
        )?;
        return Ok(LiveAssistantPlaybackObservationResult::Pending);
    }
    let effect = effects
        .iter()
        .find(|effect| {
            matches!(
                effect,
                SessionDocumentEffect::LiveAssistantPlaybackTerminalResolved { .. }
            )
        })
        .ok_or_else(|| {
            meerkat_core::error::AgentError::InternalError(
                "SessionDocument live playback authority emitted no terminal effect".to_string(),
            )
        })?;
    let receipt = seal_live_assistant_playback_truncation(
        session_id.clone(),
        channel_id.clone(),
        interaction_id,
        &response_id,
        &item_id,
        content_index,
        &evidence,
        effect,
    )
    .map_err(|error| meerkat_core::error::AgentError::InternalError(error.to_string()))?;

    match (&evidence, receipt.disposition()) {
        (
            LiveAssistantPlaybackEvidence::PlaybackComplete,
            LiveAssistantPlaybackTruncationDisposition::PlaybackComplete,
        ) => {
            let (stop_reason, usage) = completion.ok_or_else(|| {
                meerkat_core::error::AgentError::InternalError(
                    "playback-complete terminal omitted completion facts".to_string(),
                )
            })?;
            let _ = agent.append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTurnCompleted {
                    response_id: response_id.clone(),
                    stop_reason,
                    usage,
                },
            )?;
        }
        (
            LiveAssistantPlaybackEvidence::ReportedPrefix(prefix),
            LiveAssistantPlaybackTruncationDisposition::CommittedReportedPrefix,
        ) => {
            let event = RealtimeTranscriptEvent::AssistantTranscriptTruncated {
                response_id: response_id.clone(),
                item_id: item_id.clone(),
                content_index,
                text: prefix.clone(),
            };
            let _ = agent.append_realtime_transcript_event(event)?;
            if agent
                .staged_realtime_assistant_segment_text(&response_id, &item_id, content_index)
                .as_deref()
                != Some(prefix.as_str())
            {
                return Err(meerkat_core::error::AgentError::InternalError(
                    "authorized live playback prefix did not replace the exact staged segment"
                        .to_string(),
                ));
            }
            if let Some((stop_reason, usage)) = completion {
                let _ = agent.append_realtime_transcript_event(
                    RealtimeTranscriptEvent::AssistantTurnCompleted {
                        response_id: response_id.clone(),
                        stop_reason,
                        usage,
                    },
                )?;
            }
        }
        (
            LiveAssistantPlaybackEvidence::Unmeasured,
            LiveAssistantPlaybackTruncationDisposition::Unmeasured,
        ) => {}
        _ => {
            return Err(meerkat_core::error::AgentError::InternalError(
                "generated live playback disposition disagreed with input evidence".to_string(),
            ));
        }
    }

    agent.resolve_live_assistant_playback_target(
        &channel_id,
        interaction_id,
        &response_id,
        &item_id,
        content_index,
    )?;

    Ok(LiveAssistantPlaybackObservationResult::Resolved(receipt))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn observe_live_assistant_playback_final(
    agent: &mut dyn SessionAgent,
    session_id: &SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    response_id: String,
    item_id: String,
    content_index: u32,
) -> Result<Option<LiveAssistantPlaybackTruncationEvidence>, meerkat_core::error::AgentError> {
    let Some(target) = agent.live_assistant_playback_target(&channel_id, &item_id, content_index)
    else {
        return Ok(None);
    };
    if target.interaction_id() != interaction_id || target.response_id() != response_id {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live assistant final target identity mismatch".to_string(),
        ));
    }
    let Some(pending) = target.pending_terminal().cloned() else {
        return Ok(None);
    };
    let authoritative_text = agent
        .staged_realtime_assistant_segment_text(&response_id, &item_id, content_index)
        .unwrap_or_default();
    if !agent.staged_realtime_assistant_segment_is_final(&response_id, &item_id, content_index) {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live assistant final observation did not stage an exact final segment".to_string(),
        ));
    }
    let authoritative_chars = authoritative_text.chars().count() as u64;
    let authoritative_digest = text_digest(&authoritative_text);
    let (observation, prefix_chars, prefix_digest, prefix_matches) = match pending.evidence() {
        LiveAssistantPlaybackEvidence::PlaybackComplete => (
            meerkat_core::generated::session_document::LiveAssistantPlaybackTerminalObservation::PlaybackComplete,
            0,
            String::new(),
            false,
        ),
        LiveAssistantPlaybackEvidence::ReportedPrefix(prefix) => (
            meerkat_core::generated::session_document::LiveAssistantPlaybackTerminalObservation::ReportedPrefix,
            prefix.chars().count() as u64,
            text_digest(prefix),
            authoritative_text.starts_with(prefix),
        ),
        LiveAssistantPlaybackEvidence::Unmeasured => {
            return Err(meerkat_core::error::AgentError::InternalError(
                "Unmeasured terminal must resolve immediately".to_string(),
            ));
        }
    };
    let session_key = SessionDocumentKey::new(session_id.to_string());
    let mut authority = SessionDocumentMachineAuthority::new();
    authority
        .recover_live_assistant_playback_target(
            session_key.clone(),
            channel_id.to_string(),
            interaction_id.to_string(),
            response_id.clone(),
            item_id.clone(),
            u64::from(content_index),
        )
        .map_err(session_document_error)?;
    authority
        .recover_live_assistant_playback_terminal(
            session_key.clone(),
            channel_id.to_string(),
            interaction_id.to_string(),
            response_id.clone(),
            item_id.clone(),
            u64::from(content_index),
            observation,
            prefix_chars,
            prefix_digest.clone(),
        )
        .map_err(session_document_error)?;
    let effects = authority
        .observe_live_assistant_playback_final(
            session_key,
            channel_id.to_string(),
            interaction_id.to_string(),
            response_id.clone(),
            item_id.clone(),
            u64::from(content_index),
            authoritative_chars,
            authoritative_digest,
            observation,
            prefix_chars,
            prefix_digest,
            prefix_matches,
        )
        .map_err(session_document_error)?;
    let effect = effects
        .iter()
        .find(|effect| {
            matches!(
                effect,
                SessionDocumentEffect::LiveAssistantPlaybackTerminalResolved { .. }
            )
        })
        .ok_or_else(|| {
            meerkat_core::error::AgentError::InternalError(
                "late final did not resolve retained playback terminal".to_string(),
            )
        })?;
    let evidence = pending.evidence().clone();
    let receipt = seal_live_assistant_playback_truncation(
        session_id.clone(),
        channel_id.clone(),
        interaction_id,
        &response_id,
        &item_id,
        content_index,
        &evidence,
        effect,
    )
    .map_err(|error| meerkat_core::error::AgentError::InternalError(error.to_string()))?;
    match &evidence {
        LiveAssistantPlaybackEvidence::PlaybackComplete => {
            let _ = agent.append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTurnCompleted {
                    response_id: response_id.clone(),
                    stop_reason: pending.stop_reason(),
                    usage: pending.usage().clone(),
                },
            )?;
        }
        LiveAssistantPlaybackEvidence::ReportedPrefix(prefix) => {
            let _ = agent.append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTranscriptTruncated {
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    content_index,
                    text: prefix.clone(),
                },
            )?;
            let _ = agent.append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTurnCompleted {
                    response_id: response_id.clone(),
                    stop_reason: pending.stop_reason(),
                    usage: pending.usage().clone(),
                },
            )?;
        }
        LiveAssistantPlaybackEvidence::Unmeasured => unreachable!(),
    }
    agent.resolve_live_assistant_playback_target(
        &channel_id,
        interaction_id,
        &response_id,
        &item_id,
        content_index,
    )?;
    Ok(Some(receipt))
}

fn text_digest(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"meerkat.live-assistant-playback-text.v1\0");
    hasher.update((text.len() as u64).to_be_bytes());
    hasher.update(text.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::{
        FinalLiveUserTranscriptDisposition, LiveHandoffInputProvenance, LiveUserTurnCorrelation,
        OpaqueProviderCorrelation, Session,
    };
    use std::time::SystemTime;

    fn session() -> SessionId {
        SessionId::new()
    }

    fn provisional() -> ProvisionalLiveHandoff {
        let correlation = LiveUserTurnCorrelation::new(
            LiveChannelId::new("channel-a"),
            InteractionId::new(),
            OpaqueProviderCorrelation::new("delegation-private", "turn-private")
                .expect("provider correlation"),
        )
        .expect("live correlation");
        ProvisionalLiveHandoff::new(
            correlation,
            "normalized final input",
            LiveHandoffInputProvenance::NormalizedHandoff,
        )
        .expect("provisional handoff")
    }

    struct PlaybackTestAgent {
        session: Session,
        transient: meerkat_core::TransientTurnContextStateHandle,
    }

    impl PlaybackTestAgent {
        fn new() -> Self {
            Self {
                session: Session::new(),
                transient: meerkat_core::TransientTurnContextStateHandle::new(),
            }
        }
    }

    #[async_trait]
    impl SessionAgent for PlaybackTestAgent {
        async fn run_with_events(
            &mut self,
            _prompt: meerkat_core::ContentInput,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>,
        ) -> Result<meerkat_core::RunResult, meerkat_core::error::AgentError> {
            Err(meerkat_core::error::AgentError::ConfigError(
                "playback test agent does not run turns".to_string(),
            ))
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_turn_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
            _identity: meerkat_core::SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {}

        fn session_id(&self) -> SessionId {
            self.session.id().clone()
        }

        fn snapshot(&self) -> crate::ephemeral::SessionSnapshot {
            crate::ephemeral::SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: self.session.messages().len(),
                total_tokens: 0,
                usage: meerkat_core::Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Result<Session, meerkat_core::error::AgentError> {
            Ok(self.session.clone())
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<
            crate::ephemeral::SessionTranscriptAuthoritySnapshot,
            meerkat_core::error::AgentError,
        > {
            crate::ephemeral::SessionTranscriptAuthoritySnapshot::from_session(&self.session)
        }

        fn observed_session_tail(&self) -> crate::ephemeral::ObservedSessionTailKind {
            crate::ephemeral::ObservedSessionTailKind::Empty
        }

        fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
            self.transient.clone()
        }

        fn append_realtime_transcript_event(
            &mut self,
            event: RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::RealtimeTranscriptApplyOutcome, meerkat_core::error::AgentError>
        {
            Ok(self.session.append_realtime_transcript_event(event))
        }

        fn staged_realtime_assistant_segment_text(
            &self,
            response_id: &str,
            item_id: &str,
            content_index: u32,
        ) -> Option<String> {
            self.session
                .staged_realtime_assistant_segment_text(response_id, item_id, content_index)
        }

        fn staged_realtime_assistant_segment_is_final(
            &self,
            response_id: &str,
            item_id: &str,
            content_index: u32,
        ) -> bool {
            self.session.staged_realtime_assistant_segment_is_final(
                response_id,
                item_id,
                content_index,
            )
        }

        fn admit_live_assistant_playback_target(
            &mut self,
            channel_id: &LiveChannelId,
            interaction_id: InteractionId,
            response_id: &str,
            item_id: &str,
            content_index: u32,
        ) -> Result<meerkat_core::LiveAssistantPlaybackTarget, meerkat_core::error::AgentError>
        {
            self.session.admit_live_assistant_playback_target(
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
            )
        }

        fn live_assistant_playback_target(
            &self,
            channel_id: &LiveChannelId,
            item_id: &str,
            content_index: u32,
        ) -> Option<meerkat_core::LiveAssistantPlaybackTarget> {
            self.session
                .live_assistant_playback_target(channel_id, item_id, content_index)
        }

        fn live_assistant_playback_target_for_channel(
            &self,
            channel_id: &LiveChannelId,
        ) -> Option<meerkat_core::LiveAssistantPlaybackTarget> {
            self.session
                .live_assistant_playback_target_for_channel(channel_id)
        }

        fn resolve_live_assistant_playback_target(
            &mut self,
            channel_id: &LiveChannelId,
            interaction_id: InteractionId,
            response_id: &str,
            item_id: &str,
            content_index: u32,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.session.resolve_live_assistant_playback_target(
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
            )
        }
    }

    #[test]
    fn generated_session_document_effect_seals_exact_committed_evidence() {
        let session_id = session();
        let evidence = PreparedLiveUserTranscriptCommit::prepare(&session_id, &provisional())
            .expect("prepare")
            .finish(
                Some(
                    NormalizedLiveUserInputDigest::derive("normalized final input")
                        .expect("digest"),
                ),
                Some(3),
            )
            .expect("seal evidence");

        assert_eq!(evidence.session_id(), &session_id);
        assert_eq!(evidence.channel_id().as_str(), "channel-a");
        assert_eq!(
            evidence.disposition(),
            FinalLiveUserTranscriptDisposition::Committed
        );
        assert!(evidence.normalized_final_input_digest().is_some());
        assert_eq!(evidence.committed_message_count(), Some(3));
    }

    #[test]
    fn generated_session_document_effect_seals_terminal_missing_evidence() {
        let session_id = session();
        let evidence = PreparedLiveUserTranscriptCommit::prepare(&session_id, &provisional())
            .expect("prepare")
            .finish(None, None)
            .expect("seal missing evidence");

        assert_eq!(
            evidence.disposition(),
            FinalLiveUserTranscriptDisposition::Missing
        );
        assert!(evidence.normalized_final_input_digest().is_none());
        assert_eq!(evidence.committed_message_count(), None);
    }

    #[test]
    fn seal_rejects_effect_from_another_session() {
        let requested_session = session();
        let effect_session = session();
        let provisional = provisional();
        let mut authority = SessionDocumentMachineAuthority::new();
        let effect_key = SessionDocumentKey::new(effect_session.to_string());
        authority
            .admit_live_interaction_transcript(
                effect_key.clone(),
                provisional.correlation().channel_id().to_string(),
                provisional.correlation().interaction_id().to_string(),
            )
            .expect("admit");
        authority
            .stage_live_provisional_user_transcript(
                effect_key.clone(),
                provisional.correlation().channel_id().to_string(),
                provisional.correlation().interaction_id().to_string(),
                true,
            )
            .expect("stage");
        let effects = authority
            .reconcile_live_final_user_transcript(
                effect_key,
                provisional.correlation().channel_id().to_string(),
                provisional.correlation().interaction_id().to_string(),
                LiveTranscriptReconciliation::Committed,
            )
            .expect("reconcile");

        assert!(matches!(
            seal_final_live_user_transcript_commit(
                requested_session,
                provisional.correlation().channel_id().clone(),
                provisional.correlation().interaction_id(),
                Some(
                    NormalizedLiveUserInputDigest::derive("normalized final input")
                        .expect("digest")
                ),
                Some(1),
                &effects[0],
            ),
            Err(FinalLiveUserTranscriptCommitError::Transition(_))
        ));
    }

    #[test]
    fn committed_live_user_evidence_freezes_the_exact_message_boundary() {
        let mut agent = PlaybackTestAgent::new();
        let session_id = agent.session_id();
        let evidence = commit_final_live_user_transcript(
            &mut agent,
            &session_id,
            provisional(),
            Some(RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "turn-private".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "normalized final input".to_string(),
            }),
        )
        .expect("commit exact final live input");

        let committed_boundary = evidence
            .committed_message_count()
            .expect("committed evidence carries exact boundary");
        assert_eq!(committed_boundary, 1);

        agent
            .append_realtime_transcript_event(RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "later-ordinary-turn".to_string(),
                previous_item_id: Some("turn-private".to_string()),
                content_index: 0,
                text: "later context must not enter the voice executor fork".to_string(),
            })
            .expect("append a later canonical turn");
        assert_eq!(agent.snapshot().message_count, 2);
        assert_eq!(
            evidence.committed_message_count(),
            Some(committed_boundary),
            "later canonical appends cannot move the sealed fork boundary"
        );
    }

    #[test]
    fn production_playback_path_commits_only_reported_prefix_and_unmeasured_never_claims_hearing() {
        let mut agent = PlaybackTestAgent::new();
        let session_id = agent.session_id();
        let channel_id = LiveChannelId::new("channel-playback");
        let interaction_id = InteractionId::new();
        let full_provider_output = "The full provider-authored answer continues beyond playback.";
        let reported_prefix = "The full provider-authored answer";

        admit_live_assistant_playback_target(
            &mut agent,
            &session_id,
            channel_id.clone(),
            interaction_id,
            "response-playback".to_string(),
            "item-playback".to_string(),
            0,
        )
        .expect("assistant start admits exact foreground target before final");

        agent
            .append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                    response_id: "response-playback".to_string(),
                    item_id: "item-playback".to_string(),
                    content_index: 0,
                    text: full_provider_output.to_string(),
                },
            )
            .expect("stage full provider output");

        let receipt = commit_live_assistant_playback_truncation(
            &mut agent,
            &session_id,
            channel_id.clone(),
            interaction_id,
            "response-playback".to_string(),
            "item-playback".to_string(),
            0,
            LiveAssistantPlaybackEvidence::ReportedPrefix(reported_prefix.to_string()),
        )
        .expect("generated playback authority should commit exact prefix");
        assert_eq!(
            receipt.disposition(),
            LiveAssistantPlaybackTruncationDisposition::CommittedReportedPrefix
        );
        assert!(!receipt.biological_hearing_claimed());

        agent
            .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "response-playback".to_string(),
                stop_reason: meerkat_core::StopReason::EndTurn,
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::Other,
                    "playback-test",
                    meerkat_core::Usage::default(),
                ),
            })
            .expect("materialize canonical prefix");
        let canonical = agent.session.messages();
        assert_eq!(canonical.len(), 1);
        let encoded = serde_json::to_string(canonical).expect("encode canonical transcript");
        assert!(encoded.contains(reported_prefix));
        assert!(!encoded.contains(full_provider_output));

        let mut unmeasured_agent = PlaybackTestAgent::new();
        let unmeasured_session_id = unmeasured_agent.session_id();
        let unmeasured_interaction_id = InteractionId::new();
        admit_live_assistant_playback_target(
            &mut unmeasured_agent,
            &unmeasured_session_id,
            channel_id.clone(),
            unmeasured_interaction_id,
            "response-unmeasured".to_string(),
            "item-unmeasured".to_string(),
            0,
        )
        .expect("unmeasured target is admitted before final");
        unmeasured_agent
            .append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                    response_id: "response-unmeasured".to_string(),
                    item_id: "item-unmeasured".to_string(),
                    content_index: 0,
                    text: full_provider_output.to_string(),
                },
            )
            .expect("stage unmeasured provider output");
        let unmeasured = commit_live_assistant_playback_truncation(
            &mut unmeasured_agent,
            &unmeasured_session_id,
            channel_id,
            unmeasured_interaction_id,
            "response-unmeasured".to_string(),
            "item-unmeasured".to_string(),
            0,
            LiveAssistantPlaybackEvidence::Unmeasured,
        )
        .expect("missing evidence should classify as unmeasured");
        assert_eq!(
            unmeasured.disposition(),
            LiveAssistantPlaybackTruncationDisposition::Unmeasured
        );
        assert_eq!(unmeasured.canonical_prefix_chars(), None);
        assert!(!unmeasured.biological_hearing_claimed());
        assert_eq!(
            unmeasured_agent
                .staged_realtime_assistant_segment_text(
                    "response-unmeasured",
                    "item-unmeasured",
                    0,
                )
                .as_deref(),
            Some(full_provider_output)
        );
        assert!(unmeasured_agent.session.messages().is_empty());
    }

    #[test]
    fn terminal_before_final_survives_session_recovery_and_commits_only_prefix() {
        let mut agent = PlaybackTestAgent::new();
        let session_id = agent.session_id();
        let channel_id = LiveChannelId::new("channel-early-terminal-recovery");
        let interaction_id = InteractionId::new();
        let response_id = "response-early-terminal";
        let item_id = "item-early-terminal";
        let prefix = "played prefix";
        let full = "played prefix followed by provider-only suffix";
        admit_live_assistant_playback_target(
            &mut agent,
            &session_id,
            channel_id.clone(),
            interaction_id,
            response_id.to_string(),
            item_id.to_string(),
            0,
        )
        .expect("exact target is admitted before either independent fact");
        let outcome = observe_live_assistant_playback_terminal_with_completion(
            &mut agent,
            &session_id,
            channel_id.clone(),
            interaction_id,
            response_id.to_string(),
            item_id.to_string(),
            0,
            LiveAssistantPlaybackEvidence::ReportedPrefix(prefix.to_string()),
            meerkat_core::StopReason::EndTurn,
            meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Other,
                "playback-test",
                meerkat_core::Usage::default(),
            ),
        )
        .expect("generated authority retains terminal while final is absent");
        assert!(matches!(
            outcome,
            LiveAssistantPlaybackObservationResult::Pending
        ));
        assert!(agent.session.messages().is_empty());

        let encoded = serde_json::to_vec(&agent.session).expect("serialize durable session");
        let restored_session: Session =
            serde_json::from_slice(&encoded).expect("restore durable session");
        let mut restored = PlaybackTestAgent {
            session: restored_session,
            transient: meerkat_core::TransientTurnContextStateHandle::new(),
        };
        restored
            .append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                    response_id: response_id.to_string(),
                    item_id: item_id.to_string(),
                    content_index: 0,
                    text: full.to_string(),
                },
            )
            .expect("late provider final stages after recovery");
        let receipt = observe_live_assistant_playback_final(
            &mut restored,
            &session_id,
            channel_id.clone(),
            interaction_id,
            response_id.to_string(),
            item_id.to_string(),
            0,
        )
        .expect("late final joins recovered terminal")
        .expect("join emits a terminal receipt");
        assert_eq!(
            receipt.disposition(),
            LiveAssistantPlaybackTruncationDisposition::CommittedReportedPrefix
        );
        assert!(!receipt.biological_hearing_claimed());
        let canonical = serde_json::to_string(restored.session.messages())
            .expect("encode canonical transcript");
        assert!(canonical.contains(prefix));
        assert!(!canonical.contains(full));
        assert!(
            restored
                .live_assistant_playback_target(&channel_id, item_id, 0)
                .is_none(),
            "generated join consumes the target exactly once"
        );
    }

    #[test]
    fn playback_complete_commits_full_final_and_exact_target_is_reusable_across_turns() {
        let mut agent = PlaybackTestAgent::new();
        let session_id = agent.session_id();
        let channel_id = LiveChannelId::new("channel-consecutive-playback");
        let first_interaction = InteractionId::new();
        let first_full = "First assistant response is fully played.";

        admit_live_assistant_playback_target(
            &mut agent,
            &session_id,
            channel_id.clone(),
            first_interaction,
            "response-first".to_string(),
            "item-first".to_string(),
            0,
        )
        .expect("first assistant start admits its exact target");
        agent
            .append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                    response_id: "response-first".to_string(),
                    item_id: "item-first".to_string(),
                    content_index: 0,
                    text: first_full.to_string(),
                },
            )
            .expect("provider final is staged before playback terminal");
        assert!(agent.session.messages().is_empty());

        let complete = commit_live_assistant_playback_complete(
            &mut agent,
            &session_id,
            channel_id.clone(),
            first_interaction,
            "response-first".to_string(),
            "item-first".to_string(),
            0,
            meerkat_core::StopReason::EndTurn,
            meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Other,
                "playback-test",
                meerkat_core::Usage::default(),
            ),
        )
        .expect("exact playback complete commits the full staged final");
        assert_eq!(
            complete.disposition(),
            LiveAssistantPlaybackTruncationDisposition::PlaybackComplete
        );
        assert!(!complete.biological_hearing_claimed());
        assert_eq!(agent.session.messages().len(), 1);
        assert!(
            serde_json::to_string(agent.session.messages())
                .expect("encode first canonical message")
                .contains(first_full)
        );
        assert!(
            commit_live_assistant_playback_complete(
                &mut agent,
                &session_id,
                channel_id.clone(),
                first_interaction,
                "response-first".to_string(),
                "item-first".to_string(),
                0,
                meerkat_core::StopReason::EndTurn,
                meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::Other,
                    "playback-test",
                    meerkat_core::Usage::default(),
                ),
            )
            .is_err(),
            "replaying a consumed playback terminal must fail"
        );

        let second_interaction = InteractionId::new();
        let second_full = "Second assistant response has an unheard tail.";
        let second_prefix = "Second assistant response";
        admit_live_assistant_playback_target(
            &mut agent,
            &session_id,
            channel_id.clone(),
            second_interaction,
            "response-second".to_string(),
            "item-second".to_string(),
            0,
        )
        .expect("consuming the first target permits a second assistant turn");
        agent
            .append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                    response_id: "response-second".to_string(),
                    item_id: "item-second".to_string(),
                    content_index: 0,
                    text: second_full.to_string(),
                },
            )
            .expect("second final stages without becoming canonical");
        commit_live_assistant_playback_truncation(
            &mut agent,
            &session_id,
            channel_id,
            second_interaction,
            "response-second".to_string(),
            "item-second".to_string(),
            0,
            LiveAssistantPlaybackEvidence::ReportedPrefix(second_prefix.to_string()),
        )
        .expect("second exact target commits only its reported prefix");
        agent
            .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "response-second".to_string(),
                stop_reason: meerkat_core::StopReason::EndTurn,
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::Other,
                    "playback-test",
                    meerkat_core::Usage::default(),
                ),
            })
            .expect("second prefix materializes after terminal resolution");
        let canonical = serde_json::to_string(agent.session.messages()).expect("encode messages");
        assert_eq!(agent.session.messages().len(), 2);
        assert!(canonical.contains(second_prefix));
        assert!(!canonical.contains(second_full));
    }

    #[test]
    fn channel_close_resolves_pending_target_unmeasured_and_allows_replacement_turn() {
        let mut agent = PlaybackTestAgent::new();
        let session_id = agent.session_id();
        let channel_id = LiveChannelId::new("channel-close-playback");
        let interaction_id = InteractionId::new();
        let authored = "Authored assistant output that was never measured as played.";
        admit_live_assistant_playback_target(
            &mut agent,
            &session_id,
            channel_id.clone(),
            interaction_id,
            "response-close".to_string(),
            "item-close".to_string(),
            0,
        )
        .expect("pending target admitted before close");
        agent
            .append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                    response_id: "response-close".to_string(),
                    item_id: "item-close".to_string(),
                    content_index: 0,
                    text: authored.to_string(),
                },
            )
            .expect("provider final remains staged");

        let receipt = resolve_live_assistant_playback_on_channel_close(
            &mut agent,
            &session_id,
            channel_id.clone(),
        )
        .expect("generated close resolves exact target")
        .expect("pending target emits a receipt");
        assert_eq!(
            receipt.disposition(),
            LiveAssistantPlaybackTruncationDisposition::Unmeasured
        );
        assert_eq!(receipt.canonical_prefix_chars(), None);
        assert!(!receipt.biological_hearing_claimed());
        assert!(agent.session.messages().is_empty());
        assert!(
            agent
                .live_assistant_playback_target_for_channel(&channel_id)
                .is_none()
        );

        admit_live_assistant_playback_target(
            &mut agent,
            &session_id,
            LiveChannelId::new("channel-replacement-playback"),
            InteractionId::new(),
            "response-replacement".to_string(),
            "item-replacement".to_string(),
            0,
        )
        .expect("replacement channel can admit the next exact target");
    }
}
