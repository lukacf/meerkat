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
    if let Some(superseded) = agent.live_assistant_playback_target_for_channel(&channel_id)
        && (superseded.item_id() != item_id || superseded.content_index() != content_index)
    {
        // The provider started a new response before the caller reported
        // playback of the previous one: a barge-in, or a follow-up the model
        // volunteers. The generated machine admits the new output, so the
        // earlier target must not keep the transcript's single active slot.
        // It is retired as Unmeasured (no playback evidence was ever
        // reported); a later caller report for it lands on this settlement as
        // a replay and changes nothing. Before this, the reducer rejected the
        // admission and the session task aborted mid-conversation.
        if superseded.pending_terminal().is_some() {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "live assistant playback target admission while the previous target awaits its provider final"
                    .to_string(),
            ));
        }
        tracing::debug!(
            channel_id = %channel_id,
            superseded_item_id = superseded.item_id(),
            new_item_id = %item_id,
            "retiring an unreported live assistant playback target superseded by new provider output"
        );
        match observe_live_assistant_playback_terminal(
            agent,
            session_id,
            channel_id.clone(),
            superseded.interaction_id(),
            superseded.response_id().to_string(),
            superseded.item_id().to_string(),
            superseded.content_index(),
            LiveAssistantPlaybackEvidence::Unmeasured,
            None,
        )? {
            LiveAssistantPlaybackObservationResult::Resolved(_) => {}
            LiveAssistantPlaybackObservationResult::Pending => {
                return Err(meerkat_core::error::AgentError::InternalError(
                    "superseded live assistant playback target did not retire as Unmeasured"
                        .to_string(),
                ));
            }
        }
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
        channel_id,
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
    record_playback_settlement(agent, &receipt, evidence, None, None)?;
    Ok(Some(receipt))
}

fn record_playback_settlement(
    agent: &mut dyn SessionAgent,
    receipt: &LiveAssistantPlaybackTruncationEvidence,
    evidence: LiveAssistantPlaybackEvidence,
    authoritative_final: Option<String>,
    completion: Option<(meerkat_core::StopReason, meerkat_core::TurnUsage)>,
) -> Result<(), meerkat_core::error::AgentError> {
    agent.append_realtime_transcript_event(
        RealtimeTranscriptEvent::AssistantPlaybackTerminalSettled {
            channel_id: receipt.channel_id().to_string(),
            interaction_id: receipt.interaction_id(),
            response_id: receipt.response_id().to_string(),
            item_id: receipt.item_id().to_string(),
            content_index: receipt.content_index(),
            settlement: Box::new(meerkat_core::LiveAssistantPlaybackSettlement {
                evidence,
                authoritative_final,
                completion,
            }),
        },
    )?;
    Ok(())
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
    // Observation-only releases carry no provider completion or usage facts.
    let observation_only = matches!(
        evidence,
        LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(_)
    );
    let completion = if observation_only { None } else { completion };
    if response_id.trim().is_empty() || item_id.trim().is_empty() {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live playback truncation requires non-empty response and item identity".to_string(),
        ));
    }

    let prior_settlement = agent.live_assistant_playback_settlement(
        &channel_id,
        interaction_id,
        &response_id,
        &item_id,
        content_index,
    );
    // An output retired as Unmeasured because newer provider output superseded
    // it before anyone reported playback keeps that record; such a retirement
    // carries no completion, unlike a caller's own Unmeasured truncate, which
    // arrives with the provider's stop reason and usage. A caller report that
    // lands afterwards cannot extend the record; it is replayed as the
    // settlement that exists so the caller gets the committed disposition
    // rather than an error. A conflicting report against a caller-made
    // settlement is still rejected below.
    let (evidence, completion) = match prior_settlement.as_ref() {
        Some(prior)
            if prior.evidence == LiveAssistantPlaybackEvidence::Unmeasured
                && prior.completion.is_none() =>
        {
            tracing::debug!(
                channel_id = %channel_id,
                item_id = %item_id,
                "late caller playback report for an output already retired as Unmeasured; replaying that settlement"
            );
            (
                LiveAssistantPlaybackEvidence::Unmeasured,
                prior.completion.clone(),
            )
        }
        _ => (evidence, completion),
    };
    if prior_settlement.as_ref().is_some_and(|prior| {
        prior.evidence != evidence
            || (evidence.snapshot_cut().is_none() && prior.completion != completion)
    }) {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "playback retry conflicts with the committed settlement evidence".to_string(),
        ));
    }
    let target = agent.live_assistant_playback_target(&channel_id, &item_id, content_index);
    if prior_settlement.is_none() && target.is_none() {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live playback terminal has no exact active target".to_string(),
        ));
    }
    if target.as_ref().is_some_and(|target| {
        target.interaction_id() != interaction_id || target.response_id() != response_id
    }) {
        return Err(meerkat_core::error::AgentError::ConfigError(
            "live playback terminal target identity mismatch".to_string(),
        ));
    }
    if prior_settlement.is_none()
        && let Some(pending) = target.as_ref().and_then(|target| target.pending_terminal())
    {
        if pending.evidence() != &evidence
            || !completion.as_ref().is_some_and(|(reason, usage)| {
                *reason == pending.stop_reason() && usage == pending.usage()
            })
        {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "playback retry conflicts with the retained terminal evidence".to_string(),
            ));
        }
        if agent.staged_realtime_assistant_segment_is_final(&response_id, &item_id, content_index) {
            return observe_live_assistant_playback_final(
                agent,
                session_id,
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
            )
            .map(|receipt| {
                receipt.map_or(
                    LiveAssistantPlaybackObservationResult::Pending,
                    LiveAssistantPlaybackObservationResult::Resolved,
                )
            });
        }
        return Ok(LiveAssistantPlaybackObservationResult::Pending);
    }
    if let Some((snapshot, canonical)) = evidence.snapshot_cut().or({
        if let LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(snapshot) = &evidence {
            Some((snapshot.as_str(), ""))
        } else {
            None
        }
    }) {
        if snapshot.is_empty() || !snapshot.starts_with(canonical) {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "playback snapshot cut has no observed transcript".to_string(),
            ));
        }
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
        let effects = authority
            .observe_live_assistant_playback_snapshot(
                session_key,
                channel_id.to_string(),
                interaction_id.to_string(),
                response_id.clone(),
                item_id.clone(),
                u64::from(content_index),
                snapshot.chars().count() as u64,
                text_digest(snapshot),
                canonical.chars().count() as u64,
                if observation_only {
                    String::new()
                } else {
                    text_digest(canonical)
                },
                !observation_only && snapshot.starts_with(canonical),
                observation_only,
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
                    "snapshot cut emitted no exact playback receipt".to_string(),
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
        if prior_settlement.is_none() {
            let event = if observation_only {
                RealtimeTranscriptEvent::AssistantUnmeasuredSnapshotCommitted {
                    channel_id: channel_id.to_string(),
                    interaction_id,
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    content_index,
                    text: snapshot.to_string(),
                    evidence: evidence.clone(),
                }
            } else {
                RealtimeTranscriptEvent::AssistantPlaybackSnapshotCommitted {
                    channel_id: channel_id.to_string(),
                    interaction_id,
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    content_index,
                    text: canonical.to_string(),
                    evidence: evidence.clone(),
                }
            };
            agent.append_realtime_transcript_event(event)?;
        }
        if target.is_some() {
            agent.resolve_live_assistant_playback_target(
                &channel_id,
                interaction_id,
                &response_id,
                &item_id,
                content_index,
            )?;
        }
        return Ok(LiveAssistantPlaybackObservationResult::Resolved(receipt));
    }

    let (authoritative_text, authoritative_final) = if let Some(prior) = &prior_settlement {
        (
            prior.authoritative_final.clone().unwrap_or_default(),
            prior.authoritative_final.is_some(),
        )
    } else {
        (
            agent
                .staged_realtime_assistant_segment_text(&response_id, &item_id, content_index)
                .unwrap_or_default(),
            agent.staged_realtime_assistant_segment_is_final(&response_id, &item_id, content_index),
        )
    };
    // Only a provider final is authoritative text. Before it, the staged
    // segment is an ordered snapshot the public path lowers as deltas arrive;
    // the generated authority models the pre-final state as "no authoritative
    // text" (`authoritative_assistant_final == false && chars == 0 && digest
    // == ""`). Counting staged characters here made an unmeasured truncate
    // before the final fall through every terminal transition, so its target
    // stayed active and the next assistant output's admission was rejected.
    let authoritative_chars = if authoritative_final {
        authoritative_text.chars().count() as u64
    } else {
        0
    };
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
        LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(_)
        | LiveAssistantPlaybackEvidence::CallerConfirmedSnapshot(_)
        | LiveAssistantPlaybackEvidence::CallerConfirmedPrefix { .. } => {
            return Err(meerkat_core::error::AgentError::InternalError(
                "snapshot cut escaped its generated settlement path".to_string(),
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

    if prior_settlement.is_some() {
        return Ok(LiveAssistantPlaybackObservationResult::Resolved(receipt));
    }
    let recorded_completion = completion.clone();
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
        ) => {
            // Release staging, not audio. This internal discard cannot
            // canonicalize text or signal provider interruption/completion.
            if agent
                .staged_realtime_assistant_segment_text(&response_id, &item_id, content_index)
                .is_some()
            {
                agent.append_realtime_transcript_event(
                    RealtimeTranscriptEvent::AssistantTurnInterrupted {
                        response_id: response_id.clone(),
                    },
                )?;
            }
        }
        _ => {
            return Err(meerkat_core::error::AgentError::InternalError(
                "generated live playback disposition disagreed with input evidence".to_string(),
            ));
        }
    }

    record_playback_settlement(
        agent,
        &receipt,
        evidence,
        authoritative_final.then_some(authoritative_text),
        recorded_completion,
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
    if let Some(prior) = agent.live_assistant_playback_settlement(
        &channel_id,
        interaction_id,
        &response_id,
        &item_id,
        content_index,
    ) {
        return match observe_live_assistant_playback_terminal(
            agent,
            session_id,
            channel_id,
            interaction_id,
            response_id,
            item_id,
            content_index,
            prior.evidence,
            prior.completion,
        )? {
            LiveAssistantPlaybackObservationResult::Resolved(receipt) => Ok(Some(receipt)),
            LiveAssistantPlaybackObservationResult::Pending => {
                Err(meerkat_core::error::AgentError::InternalError(
                    "settled playback became pending during replay".to_string(),
                ))
            }
        };
    }
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
        LiveAssistantPlaybackEvidence::Unmeasured
        | LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(_)
        | LiveAssistantPlaybackEvidence::CallerConfirmedSnapshot(_)
        | LiveAssistantPlaybackEvidence::CallerConfirmedPrefix { .. } => {
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
        channel_id,
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
                    response_id,
                    stop_reason: pending.stop_reason(),
                    usage: pending.usage().clone(),
                },
            )?;
        }
        LiveAssistantPlaybackEvidence::ReportedPrefix(prefix) => {
            let _ = agent.append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTranscriptTruncated {
                    response_id: response_id.clone(),
                    item_id,
                    content_index,
                    text: prefix.clone(),
                },
            )?;
            let _ = agent.append_realtime_transcript_event(
                RealtimeTranscriptEvent::AssistantTurnCompleted {
                    response_id,
                    stop_reason: pending.stop_reason(),
                    usage: pending.usage().clone(),
                },
            )?;
        }
        LiveAssistantPlaybackEvidence::Unmeasured
        | LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(_)
        | LiveAssistantPlaybackEvidence::CallerConfirmedSnapshot(_)
        | LiveAssistantPlaybackEvidence::CallerConfirmedPrefix { .. } => {
            return Err(meerkat_core::error::AgentError::InternalError(
                "non-pending playback evidence reached a final join".to_string(),
            ));
        }
    }
    record_playback_settlement(
        agent,
        &receipt,
        evidence,
        Some(authoritative_text),
        Some((pending.stop_reason(), pending.usage().clone())),
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
        completed_events: usize,
    }

    impl PlaybackTestAgent {
        fn new() -> Self {
            Self {
                session: Session::new(),
                transient: meerkat_core::TransientTurnContextStateHandle::new(),
                completed_events: 0,
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
            if matches!(
                &event,
                RealtimeTranscriptEvent::AssistantTurnCompleted { .. }
            ) {
                self.completed_events += 1;
            }
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

        fn live_assistant_playback_settlement(
            &self,
            channel_id: &LiveChannelId,
            interaction_id: InteractionId,
            response_id: &str,
            item_id: &str,
            content_index: u32,
        ) -> Option<meerkat_core::LiveAssistantPlaybackSettlement> {
            self.session.live_assistant_playback_settlement(
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
            )
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
        assert!(
            !unmeasured.continues_provider_group(),
            "ordinary unmeasured truncation must not acquire provider-managed continuation"
        );
        assert_eq!(
            unmeasured_agent
                .staged_realtime_assistant_segment_text(
                    "response-unmeasured",
                    "item-unmeasured",
                    0,
                )
                .as_deref(),
            None
        );
        assert!(unmeasured_agent.session.messages().is_empty());
    }

    #[test]
    fn all_terminal_settlements_survive_commit_loss_and_replay_exactly() {
        for (evidence, expected) in [
            (
                LiveAssistantPlaybackEvidence::Unmeasured,
                LiveAssistantPlaybackTruncationDisposition::Unmeasured,
            ),
            (
                LiveAssistantPlaybackEvidence::PlaybackComplete,
                LiveAssistantPlaybackTruncationDisposition::PlaybackComplete,
            ),
            (
                LiveAssistantPlaybackEvidence::ReportedPrefix("played".to_string()),
                LiveAssistantPlaybackTruncationDisposition::CommittedReportedPrefix,
            ),
        ] {
            for final_first in [false, true] {
                let mut agent = PlaybackTestAgent::new();
                let session_id = agent.session_id();
                let channel = LiveChannelId::new("terminal-settlement-replay");
                let interaction = InteractionId::new();
                let usage = meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::Other,
                    "playback-test",
                    meerkat_core::Usage::default(),
                );
                let stage_final = |agent: &mut PlaybackTestAgent| {
                    agent
                        .append_realtime_transcript_event(
                            RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                                response_id: "response".to_string(),
                                item_id: "item".to_string(),
                                content_index: 0,
                                text: "played and remaining".to_string(),
                            },
                        )
                        .expect("stage final");
                };
                admit_live_assistant_playback_target(
                    &mut agent,
                    &session_id,
                    channel.clone(),
                    interaction,
                    "response".to_string(),
                    "item".to_string(),
                    0,
                )
                .expect("admit target");
                if final_first {
                    stage_final(&mut agent);
                }
                let first = observe_live_assistant_playback_terminal_with_completion(
                    &mut agent,
                    &session_id,
                    channel.clone(),
                    interaction,
                    "response".to_string(),
                    "item".to_string(),
                    0,
                    evidence.clone(),
                    meerkat_core::StopReason::EndTurn,
                    usage.clone(),
                )
                .expect("observe terminal");
                if !first.is_resolved() {
                    let encoded = serde_json::to_vec(&agent.session).expect("persist pending fact");
                    agent.session = serde_json::from_slice(&encoded).expect("recover pending fact");
                    assert!(
                        !observe_live_assistant_playback_terminal_with_completion(
                            &mut agent,
                            &session_id,
                            channel.clone(),
                            interaction,
                            "response".to_string(),
                            "item".to_string(),
                            0,
                            evidence.clone(),
                            meerkat_core::StopReason::EndTurn,
                            usage.clone(),
                        )
                        .expect("identical pending replay")
                        .is_resolved()
                    );
                    stage_final(&mut agent);
                    observe_live_assistant_playback_final(
                        &mut agent,
                        &session_id,
                        channel.clone(),
                        interaction,
                        "response".to_string(),
                        "item".to_string(),
                        0,
                    )
                    .expect("join final")
                    .expect("resolved receipt");
                }
                let committed =
                    serde_json::to_vec(&agent.session).expect("persist completed settlement");
                agent.session =
                    serde_json::from_slice(&committed).expect("recover after lost acknowledgement");
                let retry = observe_live_assistant_playback_terminal_with_completion(
                    &mut agent,
                    &session_id,
                    channel.clone(),
                    interaction,
                    "response".to_string(),
                    "item".to_string(),
                    0,
                    evidence.clone(),
                    meerkat_core::StopReason::EndTurn,
                    usage.clone(),
                )
                .expect("exact internal replay after durable commit");
                let receipt = match retry {
                    LiveAssistantPlaybackObservationResult::Resolved(receipt) => Some(receipt),
                    LiveAssistantPlaybackObservationResult::Pending => None,
                }
                .expect("settlement cannot become pending again");
                assert_eq!(receipt.disposition(), expected);
                assert!(!receipt.biological_hearing_claimed());
                assert_eq!(
                    serde_json::to_vec(&agent.session).expect("unchanged state"),
                    committed
                );
                let conflicting = if evidence == LiveAssistantPlaybackEvidence::Unmeasured {
                    LiveAssistantPlaybackEvidence::PlaybackComplete
                } else {
                    LiveAssistantPlaybackEvidence::Unmeasured
                };
                assert!(
                    observe_live_assistant_playback_terminal_with_completion(
                        &mut agent,
                        &session_id,
                        channel.clone(),
                        interaction,
                        "response".to_string(),
                        "item".to_string(),
                        0,
                        conflicting,
                        meerkat_core::StopReason::EndTurn,
                        usage.clone(),
                    )
                    .is_err()
                );
                assert!(
                    observe_live_assistant_playback_terminal_with_completion(
                        &mut agent,
                        &session_id,
                        channel,
                        interaction,
                        "response".to_string(),
                        "item".to_string(),
                        0,
                        evidence.clone(),
                        meerkat_core::StopReason::Cancelled,
                        usage,
                    )
                    .is_err(),
                    "changed completion facts are not an identical replay"
                );
                if expected == LiveAssistantPlaybackTruncationDisposition::Unmeasured {
                    assert!(agent.session.messages().is_empty());
                }
            }
        }
    }

    #[test]
    fn new_provider_output_retires_an_unreported_target_and_accepts_the_late_report() {
        let mut agent = PlaybackTestAgent::new();
        let session_id = agent.session_id();
        let channel = LiveChannelId::new("superseded");
        let interaction = InteractionId::new();
        let usage = || {
            meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                "gpt-live-1",
                meerkat_core::Usage::default(),
            )
        };
        admit_live_assistant_playback_target(
            &mut agent,
            &session_id,
            channel.clone(),
            interaction,
            "response-0".to_string(),
            "item-0".to_string(),
            0,
        )
        .expect("first output admitted");
        // The provider starts its next response (barge-in answer) before the
        // caller reported playback of the first one.
        let second = admit_live_assistant_playback_target(
            &mut agent,
            &session_id,
            channel.clone(),
            interaction,
            "response-1".to_string(),
            "item-1".to_string(),
            0,
        )
        .expect("new provider output is admitted while the previous one is unreported");
        assert_eq!(second.item_id(), "item-1");
        assert!(
            agent
                .live_assistant_playback_target(&channel, "item-0", 0)
                .is_none(),
            "the superseded target must be retired, not left active"
        );
        let settlement = agent
            .live_assistant_playback_settlement(&channel, interaction, "response-0", "item-0", 0)
            .expect("the superseded output settles as Unmeasured");
        assert!(matches!(
            settlement.evidence,
            LiveAssistantPlaybackEvidence::Unmeasured
        ));
        assert!(
            agent.session.messages().is_empty(),
            "an output nobody reported hearing commits no assistant text"
        );
        // The caller's late report for the superseded output is a replay
        // against that settlement: accepted, and it commits nothing new.
        let late = observe_live_assistant_playback_terminal_with_completion(
            &mut agent,
            &session_id,
            channel.clone(),
            interaction,
            "response-0".to_string(),
            "item-0".to_string(),
            0,
            LiveAssistantPlaybackEvidence::CallerConfirmedSnapshot(
                "said before barge-in".to_string(),
            ),
            meerkat_core::StopReason::EndTurn,
            usage(),
        )
        .expect("late caller report replays against the Unmeasured settlement");
        assert!(late.is_resolved());
        assert!(agent.session.messages().is_empty());
        // The new output still settles normally.
        let current = observe_live_assistant_playback_terminal_with_completion(
            &mut agent,
            &session_id,
            channel,
            interaction,
            "response-1".to_string(),
            "item-1".to_string(),
            0,
            LiveAssistantPlaybackEvidence::CallerConfirmedSnapshot("answer".to_string()),
            meerkat_core::StopReason::EndTurn,
            usage(),
        )
        .expect("the admitted output settles");
        assert!(current.is_resolved());
        assert_eq!(agent.session.messages().len(), 1);
    }

    #[test]
    fn provider_managed_unmeasured_releases_staging_without_completion_and_replays() {
        let mut agent = PlaybackTestAgent::new();
        let session_id = agent.session_id();
        let channel = LiveChannelId::new("continuous-observations");
        let interaction = InteractionId::new();
        for segment in 0..4 {
            let response = format!("unmeasured-response-{segment}");
            let item = format!("unmeasured-item-{segment}");
            admit_live_assistant_playback_target(
                &mut agent,
                &session_id,
                channel.clone(),
                interaction,
                response.clone(),
                item.clone(),
                0,
            )
            .expect("same group admits another observation segment");
            agent
                .append_realtime_transcript_event(
                    RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                        response_id: response.clone(),
                        item_id: item.clone(),
                        content_index: 0,
                        text: "observed, never measured as played".to_string(),
                    },
                )
                .expect("stage text independently of playback");
            let receipt = commit_live_assistant_playback_truncation(
                &mut agent,
                &session_id,
                channel.clone(),
                interaction,
                response.clone(),
                item.clone(),
                0,
                LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(
                    "observed, never measured as played".to_string(),
                ),
            )
            .expect("generated unmeasured release needs no playback-complete call");
            assert_eq!(
                agent.completed_events, 0,
                "observation snapshots cannot manufacture turn-completed events"
            );
            assert_eq!(
                receipt.disposition(),
                LiveAssistantPlaybackTruncationDisposition::Unmeasured
            );
            assert!(receipt.continues_provider_group());
            assert_eq!(receipt.canonical_prefix_chars(), None);
            assert!(!receipt.biological_hearing_claimed());
            assert!(
                agent
                    .staged_realtime_assistant_segment_text(&response, &item, 0)
                    .is_none()
            );
            assert!(
                agent
                    .live_assistant_playback_target_for_channel(&channel)
                    .is_none()
            );
            assert_eq!(agent.session.messages().len(), segment + 1);
            assert!(agent.session.messages().iter().all(|message| matches!(message,
                meerkat_core::Message::BlockAssistant(assistant) if assistant.stop_reason.is_none() && assistant.blocks.iter().all(|block| matches!(block,
                    meerkat_core::AssistantBlock::Transcript { text, source: meerkat_core::types::TranscriptSource::SpokenUnmeasured, .. }
                        if text == "observed, never measured as played"
                ))
            )));
            let bytes = serde_json::to_vec(&agent.session).expect("persist unmeasured receipt");
            agent.session = serde_json::from_slice(&bytes).expect("recover receipt");
            let replay = commit_live_assistant_playback_truncation(
                &mut agent,
                &session_id,
                channel.clone(),
                interaction,
                response.clone(),
                item.clone(),
                0,
                LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(
                    "observed, never measured as played".to_string(),
                ),
            )
            .expect("same unmeasured evidence replays after staging was discarded");
            assert_eq!(receipt, replay);
            assert_eq!(
                serde_json::to_vec(&agent.session).expect("replayed canonical snapshot"),
                bytes,
                "durable retry must not append another transcript row or change provenance"
            );
            assert_eq!(agent.session.messages().len(), segment + 1);
            assert!(
                commit_live_assistant_playback_truncation(
                    &mut agent,
                    &session_id,
                    channel.clone(),
                    interaction,
                    response,
                    item,
                    0,
                    LiveAssistantPlaybackEvidence::Unmeasured,
                )
                .is_err(),
                "ordinary unmeasured close is not same-group continuation authority"
            );
        }
        assert!(
            resolve_live_assistant_playback_on_channel_close(&mut agent, &session_id, channel,)
                .expect("released channel closes with no pending target")
                .is_none()
        );
    }

    #[test]
    fn snapshot_cut_commits_without_final_and_preserves_interaction_for_next_segment() {
        let mut agent = PlaybackTestAgent::new();
        let session_id = agent.session_id();
        let channel = LiveChannelId::new("snapshot-cut");
        let interaction = InteractionId::new();
        for (segment, text) in ["first checkpoint", "later continuation"]
            .into_iter()
            .enumerate()
        {
            let response = format!("response-{segment}");
            let item = format!("item-{segment}");
            admit_live_assistant_playback_target(
                &mut agent,
                &session_id,
                channel.clone(),
                interaction,
                response.clone(),
                item.clone(),
                0,
            )
            .expect("admit exact segment in the same interaction");
            let outcome = observe_live_assistant_playback_terminal_with_completion(
                &mut agent,
                &session_id,
                channel.clone(),
                interaction,
                response.clone(),
                item.clone(),
                0,
                LiveAssistantPlaybackEvidence::CallerConfirmedSnapshot(text.to_string()),
                meerkat_core::StopReason::EndTurn,
                meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    "gpt-live-1",
                    meerkat_core::Usage::default(),
                ),
            )
            .expect("cut does not require a provider final");
            let receipt = match outcome {
                LiveAssistantPlaybackObservationResult::Resolved(receipt) => Some(receipt),
                LiveAssistantPlaybackObservationResult::Pending => None,
            }
            .expect("explicit snapshot cut cannot remain pending on a provider final");
            assert_eq!(
                receipt.disposition(),
                LiveAssistantPlaybackTruncationDisposition::CommittedSnapshot
            );
            assert_eq!(receipt.interaction_id(), interaction);
            assert!(receipt.continues_provider_group());
            assert!(!receipt.biological_hearing_claimed());
            assert_eq!(agent.session.messages().len(), segment + 1);
            let bytes = serde_json::to_vec(&agent.session).expect("persist cut");
            agent.session =
                serde_json::from_slice(&bytes).expect("restore cut without losing history");
            let retry = observe_live_assistant_playback_terminal_with_completion(
                &mut agent,
                &session_id,
                channel.clone(),
                interaction,
                response,
                item,
                0,
                LiveAssistantPlaybackEvidence::CallerConfirmedSnapshot(text.to_string()),
                meerkat_core::StopReason::EndTurn,
                meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    "gpt-live-1",
                    meerkat_core::Usage::default(),
                ),
            )
            .expect("retry exact generated cut after durable commit and lost receipt");
            assert!(retry.is_resolved());
            assert_eq!(
                agent.session.messages().len(),
                segment + 1,
                "retry cannot duplicate history"
            );
        }
        let history = format!("{:?}", agent.session.messages());
        assert!(history.contains("first checkpoint"));
        assert!(history.contains("later continuation"));
    }

    #[test]
    fn snapshot_prefix_validates_exact_text_and_replays_only_identical_evidence() {
        for prefix in ["spoken", ""] {
            let mut agent = PlaybackTestAgent::new();
            let session_id = agent.session_id();
            let channel = LiveChannelId::new("snapshot-prefix-retry");
            let interaction = InteractionId::new();
            admit_live_assistant_playback_target(
                &mut agent,
                &session_id,
                channel.clone(),
                interaction,
                "response".to_string(),
                "item".to_string(),
                0,
            )
            .expect("target");
            let report = |prefix: &str| LiveAssistantPlaybackEvidence::CallerConfirmedPrefix {
                snapshot: "spoken but unplayed suffix".to_string(),
                prefix: prefix.to_string(),
            };
            let usage = meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                "gpt-live-1",
                meerkat_core::Usage::default(),
            );
            assert!(
                observe_live_assistant_playback_terminal_with_completion(
                    &mut agent,
                    &session_id,
                    channel.clone(),
                    interaction,
                    "response".to_string(),
                    "item".to_string(),
                    0,
                    report("wrong"),
                    meerkat_core::StopReason::EndTurn,
                    usage.clone(),
                )
                .is_err()
            );
            let expected = report(prefix);
            assert!(
                observe_live_assistant_playback_terminal_with_completion(
                    &mut agent,
                    &session_id,
                    channel.clone(),
                    interaction,
                    "response".to_string(),
                    "item".to_string(),
                    0,
                    expected.clone(),
                    meerkat_core::StopReason::EndTurn,
                    usage.clone(),
                )
                .expect("valid prefix resolves without final")
                .is_resolved()
            );
            let before_retry = agent.session.messages().to_vec();
            assert!(
                observe_live_assistant_playback_terminal_with_completion(
                    &mut agent,
                    &session_id,
                    channel.clone(),
                    interaction,
                    "response".to_string(),
                    "item".to_string(),
                    0,
                    expected,
                    meerkat_core::StopReason::EndTurn,
                    usage.clone(),
                )
                .expect("replay exact prefix receipt")
                .is_resolved()
            );
            assert_eq!(agent.session.messages(), &before_retry);
            assert!(
                observe_live_assistant_playback_terminal_with_completion(
                    &mut agent,
                    &session_id,
                    channel,
                    interaction,
                    "response".to_string(),
                    "item".to_string(),
                    0,
                    report("spoken but"),
                    meerkat_core::StopReason::EndTurn,
                    usage,
                )
                .is_err(),
                "conflicting retry cannot grow already-committed prefix"
            );
        }
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
            completed_events: 0,
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
    fn playback_complete_internal_replay_preserves_history_and_allows_next_turn() {
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
        let replay = commit_live_assistant_playback_complete(
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
        .expect("identical session-owner replay recovers the prior receipt");
        assert_eq!(replay, complete);
        assert_eq!(
            agent.session.messages().len(),
            1,
            "internal replay cannot append again"
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
