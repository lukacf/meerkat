//! Facade-owned four-role live projection over `SessionService` (DEC-P2-8).
//!
//! `rkat mob host` composes a live listener without an RPC runtime, so the
//! `LiveProjectionSink + LiveChannelCloseFeedback + LiveChannelStatusFeedback
//! + LiveWsTokenAuthority` roles are implemented here directly over the
//! daemon's `PersistentSessionService` + `MeerkatMachine` — the same seams
//! `meerkat_rpc::live_projection_sink::SessionServiceProjectionSink` reaches
//! through its `SessionRuntime` (which delegates every one of these calls to
//! the service/machine). rkat-rpc keeps its own impl unchanged; the
//! `LiveOrchestrator` open-pipeline extraction remains phase 6b pre-work.
//!
//! Semantics are a faithful port of the RPC sink:
//! - user transcripts route through the realtime staging seam when a stable
//!   `provider_item_id` exists, else commit directly as external user
//!   content;
//! - assistant deltas stage via typed `RealtimeTranscriptEvent`s (identity
//!   plumbed end-to-end, fail-closed on missing required ids);
//! - display-text finals buffer per `(SessionId, response_id)` until the
//!   authoritative `TurnCompleted` flushes them (P1#1/R6/T6);
//! - barge-in fans `AssistantTurnInterrupted` to every staged response and
//!   interrupts through machine authority (CC4/G4);
//! - close/status/token authority calls route into the generated
//!   MeerkatMachine live authorities.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use meerkat_core::RealtimeTranscriptEvent;
use meerkat_core::live_adapter::LiveAdapterErrorCode;
use meerkat_core::service::SessionService as _;
use meerkat_core::types::{AssistantBlock, ContentInput, SessionId, StopReason, Usage};
use meerkat_live::{
    LiveChannelCloseFeedback, LiveChannelCloseObservation, LiveChannelId,
    LiveChannelStatusFeedback, LiveChannelStatusObservation, LiveProjectionError,
    LiveProjectionSink, LiveTokenString, LiveTranscriptIdentity, LiveTranscriptIdentityError,
    LiveWsTokenAdmission, LiveWsTokenAdmissionPublicErrorClass, LiveWsTokenAdmissionRejection,
    LiveWsTokenAuthority, LiveWsTokenIssue,
};
use meerkat_runtime::MeerkatMachine;

use crate::{FactoryAgentBuilder, PersistentSessionService};

/// One buffered assistant **display-text** fragment awaiting an
/// authoritative `signal_turn_completed` flush (text lane only; the spoken
/// lane commits via the realtime-staging materializer).
#[derive(Debug, Clone)]
enum PendingAssistantContent {
    Text(String),
}

#[derive(Debug, Default)]
struct PendingTurn {
    blocks: Vec<PendingAssistantContent>,
}

/// Four-role live projection over the daemon's session service + machine.
pub struct ServiceLiveProjection {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    machine: Arc<MeerkatMachine>,
    /// Per-(session, response_id) buffer of assistant finals awaiting an
    /// authoritative `TurnCompleted`. Held only for short sync sections —
    /// never across `.await`.
    pending_turns: StdMutex<HashMap<(SessionId, Option<String>), PendingTurn>>,
}

impl ServiceLiveProjection {
    pub fn new(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        machine: Arc<MeerkatMachine>,
    ) -> Self {
        Self {
            service,
            machine,
            pending_turns: StdMutex::new(HashMap::new()),
        }
    }

    fn buffer_assistant_content(
        &self,
        session_id: &SessionId,
        response_id: Option<&str>,
        content: PendingAssistantContent,
    ) {
        let Ok(mut pending) = self.pending_turns.lock() else {
            // Lock poisoning would mean a prior panic inside this struct's
            // short sync sections; preserving the buffer beats breaking the
            // in-flight turn.
            return;
        };
        pending
            .entry((session_id.clone(), response_id.map(|s| s.to_string())))
            .or_default()
            .blocks
            .push(content);
    }

    fn drain_pending_turn(&self, session_id: &SessionId, response_id: Option<&str>) -> PendingTurn {
        let Ok(mut pending) = self.pending_turns.lock() else {
            return PendingTurn::default();
        };
        pending
            .remove(&(session_id.clone(), response_id.map(|s| s.to_string())))
            .unwrap_or_default()
    }

    fn drain_all_pending_turns(&self, session_id: &SessionId) {
        let Ok(mut pending) = self.pending_turns.lock() else {
            return;
        };
        pending.retain(|(sid, _resp), _| sid != session_id);
    }

    async fn in_flight_realtime_assistant_response_ids(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<String>, meerkat_core::SessionError> {
        let Some(session) = self.service.load_authoritative_session(session_id).await? else {
            return Ok(Vec::new());
        };
        Ok(session.in_flight_realtime_assistant_response_ids())
    }
}

fn build_assistant_text_delta_event(
    delta: &str,
    identity: LiveTranscriptIdentity<'_>,
) -> Result<RealtimeTranscriptEvent, LiveTranscriptIdentityError> {
    let resolved = identity.require_delta_identity()?;
    Ok(RealtimeTranscriptEvent::AssistantTextDelta {
        response_id: resolved.response_id.to_string(),
        delta_id: resolved.delta_id.to_string(),
        item_id: resolved.item_id.to_string(),
        previous_item_id: resolved.previous_item_id.map(|s| s.to_string()),
        content_index: resolved.content_index.unwrap_or(0),
        delta: delta.to_string(),
    })
}

fn build_assistant_transcript_delta_event(
    delta: &str,
    identity: LiveTranscriptIdentity<'_>,
) -> Result<RealtimeTranscriptEvent, LiveTranscriptIdentityError> {
    let resolved = identity.require_delta_identity()?;
    Ok(RealtimeTranscriptEvent::AssistantTranscriptDelta {
        response_id: resolved.response_id.to_string(),
        delta_id: resolved.delta_id.to_string(),
        item_id: resolved.item_id.to_string(),
        previous_item_id: resolved.previous_item_id.map(|s| s.to_string()),
        content_index: resolved.content_index.unwrap_or(0),
        delta: delta.to_string(),
    })
}

fn identity_error_to_projection(err: LiveTranscriptIdentityError) -> LiveProjectionError {
    LiveProjectionError::Rejected(err.to_string())
}

fn collapse_pending_blocks(buffered: Vec<PendingAssistantContent>) -> Vec<AssistantBlock> {
    if buffered.is_empty() {
        return Vec::new();
    }
    let mut acc = String::new();
    for PendingAssistantContent::Text(fragment) in buffered {
        acc.push_str(&fragment);
    }
    vec![AssistantBlock::Text {
        text: acc,
        meta: None,
    }]
}

fn session_error_to_projection(
    err: meerkat_core::SessionError,
    id: &SessionId,
) -> LiveProjectionError {
    LiveProjectionError::from_session_error(id, err)
}

#[async_trait]
impl LiveChannelCloseFeedback for ServiceLiveProjection {
    async fn record_live_channel_closed(
        &self,
        channel_id: &LiveChannelId,
        observation: &LiveChannelCloseObservation,
    ) -> Result<meerkat_live::LiveChannelCloseCommitAuthority, String> {
        let session_id = self
            .machine
            .live_session_for_active_channel(channel_id)
            .await
            .ok_or_else(|| {
                format!("generated live active-channel authority absent for channel {channel_id}")
            })?;
        self.machine
            .resolve_live_close_result(&session_id, observation)
            .await
            .map_err(|err| err.to_string())?
            .into_channel_close_commit_authority()
            .ok_or_else(|| {
                format!(
                    "generated live close authority omitted host commit handoff for channel {channel_id}"
                )
            })
    }
}

#[async_trait]
impl LiveChannelStatusFeedback for ServiceLiveProjection {
    async fn record_live_channel_status(
        &self,
        channel_id: &LiveChannelId,
        observation: &LiveChannelStatusObservation,
    ) -> Result<meerkat_live::LiveChannelStatusCommitAuthority, String> {
        if observation.channel_id() != channel_id.as_str() {
            return Err(format!(
                "generated live status observation channel mismatch: observed {}, requested {}",
                observation.channel_id(),
                channel_id
            ));
        }
        let session_id = self
            .machine
            .live_session_for_status_channel(channel_id)
            .await
            .ok_or_else(|| {
                format!("generated live status-channel authority absent for channel {channel_id}")
            })?;
        self.machine
            .resolve_live_channel_status_result(&session_id, observation)
            .await
            .map_err(|err| err.to_string())?
            .into_channel_status_commit_authority()
            .ok_or_else(|| {
                format!(
                    "generated live status authority omitted host commit handoff for channel {channel_id}"
                )
            })
    }
}

#[async_trait]
impl LiveWsTokenAuthority for ServiceLiveProjection {
    async fn record_live_ws_token_issued(
        &self,
        session_id: &SessionId,
        channel_id: &LiveChannelId,
        token: &LiveTokenString,
        issued_at_ms: u64,
        ttl_ms: u64,
    ) -> Result<LiveWsTokenIssue, String> {
        let authority = self
            .machine
            .record_live_websocket_token_issued(
                session_id,
                channel_id,
                token.as_str(),
                issued_at_ms,
                ttl_ms,
            )
            .await
            .map_err(|err| err.to_string())?;
        let token = LiveTokenString::new(authority.token).map_err(|err| err.to_string())?;
        Ok(LiveWsTokenIssue {
            token,
            expires_at_ms: authority.expires_at_ms,
            sequence: authority.sequence,
        })
    }

    async fn resolve_live_ws_token_admission(
        &self,
        channel_id: &LiveChannelId,
        token: &str,
        observed_at_ms: u64,
    ) -> Result<LiveWsTokenAdmission, String> {
        let token_owner = self.machine.live_session_for_websocket_token(token).await;
        let authority = match token_owner {
            Some(session_id) => {
                self.machine
                    .resolve_live_websocket_token_admission(
                        &session_id,
                        channel_id,
                        token,
                        observed_at_ms,
                    )
                    .await
            }
            None => match self
                .machine
                .live_session_for_active_channel(channel_id)
                .await
            {
                Some(session_id) => {
                    self.machine
                        .resolve_live_websocket_token_admission(
                            &session_id,
                            channel_id,
                            token,
                            observed_at_ms,
                        )
                        .await
                }
                None => {
                    self.machine
                        .resolve_unbound_live_websocket_token_admission(
                            channel_id,
                            token,
                            observed_at_ms,
                        )
                        .await
                }
            },
        }
        .map_err(|err| err.to_string())?;

        Ok(LiveWsTokenAdmission {
            channel_id: channel_id.clone(),
            admitted: authority.admitted,
            rejection: authority
                .rejection
                .map(live_ws_token_admission_rejection_from_machine),
            public_error_class: authority
                .public_error_class
                .map(live_ws_token_public_error_class_from_machine),
            sequence: authority.sequence,
        })
    }
}

fn live_ws_token_admission_rejection_from_machine(
    rejection: meerkat_runtime::meerkat_machine::dsl::LiveWebsocketTokenAdmissionRejection,
) -> LiveWsTokenAdmissionRejection {
    use meerkat_runtime::meerkat_machine::dsl::LiveWebsocketTokenAdmissionRejection as Dsl;
    match rejection {
        Dsl::TokenNotFound => LiveWsTokenAdmissionRejection::TokenNotFound,
        Dsl::TokenExpired => LiveWsTokenAdmissionRejection::TokenExpired,
        Dsl::TokenChannelMismatch => LiveWsTokenAdmissionRejection::TokenChannelMismatch,
        Dsl::TokenAlreadyConsumed => LiveWsTokenAdmissionRejection::TokenAlreadyConsumed,
        Dsl::ChannelNotBound => LiveWsTokenAdmissionRejection::ChannelNotBound,
    }
}

fn live_ws_token_public_error_class_from_machine(
    public_error_class: meerkat_runtime::meerkat_machine::dsl::LiveWebsocketTokenAdmissionPublicErrorClass,
) -> LiveWsTokenAdmissionPublicErrorClass {
    use meerkat_runtime::meerkat_machine::dsl::LiveWebsocketTokenAdmissionPublicErrorClass as Dsl;
    match public_error_class {
        Dsl::InvalidToken => LiveWsTokenAdmissionPublicErrorClass::InvalidToken,
    }
}

#[async_trait]
impl LiveProjectionSink for ServiceLiveProjection {
    async fn append_user_transcript(
        &self,
        session_id: &SessionId,
        text: &str,
        identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError> {
        // Stable provider item id → typed realtime seam owns idempotent
        // ordering/dedup; legacy partial-final shape commits directly.
        if let Some(item_id) = identity.provider_item_id {
            let event = RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: item_id.to_string(),
                previous_item_id: identity.previous_item_id.map(|s| s.to_string()),
                content_index: identity.content_index.unwrap_or(0),
                text: text.to_string(),
            };
            return self
                .service
                .append_realtime_transcript_event(session_id, event)
                .await
                .map(|_outcome| ())
                .map_err(|err| session_error_to_projection(err, session_id));
        }

        self.service
            .append_external_user_content(session_id, ContentInput::Text(text.to_string()))
            .await
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn append_assistant_text_delta(
        &self,
        session_id: &SessionId,
        delta: &str,
        identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError> {
        let event = build_assistant_text_delta_event(delta, identity)
            .map_err(identity_error_to_projection)?;
        self.service
            .append_realtime_transcript_event(session_id, event)
            .await
            .map(|_outcome| ())
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn append_assistant_transcript_delta(
        &self,
        session_id: &SessionId,
        delta: &str,
        identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError> {
        let event = build_assistant_transcript_delta_event(delta, identity)
            .map_err(identity_error_to_projection)?;
        self.service
            .append_realtime_transcript_event(session_id, event)
            .await
            .map(|_outcome| ())
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn append_assistant_text_final(
        &self,
        session_id: &SessionId,
        text: &str,
        _identity: LiveTranscriptIdentity<'_>,
        _stop_reason: StopReason,
        _usage: Usage,
        response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        // P1#1 + T6: display-text finals buffer until the authoritative
        // TurnCompleted carries stop_reason/usage; text survives barge-in.
        self.buffer_assistant_content(
            session_id,
            response_id,
            PendingAssistantContent::Text(text.to_string()),
        );
        Ok(())
    }

    async fn append_assistant_transcript_final(
        &self,
        session_id: &SessionId,
        text: &str,
        identity: LiveTranscriptIdentity<'_>,
        _stop_reason: StopReason,
        _usage: Usage,
        response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        // R5-7: authoritative final transcript text feeds the staging
        // pipeline; stop_reason/usage arrive atomically with TurnCompleted.
        let response_id = identity
            .response_id
            .map(|s| s.to_string())
            .or_else(|| response_id.map(|s| s.to_string()))
            .unwrap_or_default();
        let event = RealtimeTranscriptEvent::AssistantTranscriptFinalText {
            response_id,
            item_id: identity
                .provider_item_id
                .map(|s| s.to_string())
                .unwrap_or_default(),
            content_index: identity.content_index.unwrap_or(0),
            text: text.to_string(),
        };
        self.service
            .append_realtime_transcript_event(session_id, event)
            .await
            .map(|_outcome| ())
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn truncate_assistant_transcript(
        &self,
        session_id: &SessionId,
        provider_item_id: Option<&str>,
        _previous_item_id: Option<&str>,
        content_index: Option<u32>,
        response_id: Option<&str>,
        text: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        // P1#3: never fabricate empty identity — a missing id is a typed
        // rejection, not a silently inert projection.
        let Some(response_id) = response_id else {
            return Err(LiveProjectionError::Rejected(
                "AssistantTranscriptTruncated missing response_id from adapter".to_string(),
            ));
        };
        let Some(item_id) = provider_item_id else {
            return Err(LiveProjectionError::Rejected(
                "AssistantTranscriptTruncated missing provider_item_id from adapter".to_string(),
            ));
        };
        let event = RealtimeTranscriptEvent::AssistantTranscriptTruncated {
            response_id: response_id.to_string(),
            item_id: item_id.to_string(),
            content_index: content_index.unwrap_or(0),
            text: text.unwrap_or_default().to_string(),
        };
        self.service
            .append_realtime_transcript_event(session_id, event)
            .await
            .map(|_outcome| ())
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn signal_turn_interrupt(
        &self,
        session_id: &SessionId,
        response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        // CC4/G4: fan AssistantTurnInterrupted to the observed response id
        // plus every staged in-flight response, then interrupt through
        // machine authority. NotRunning is tolerated (late barge-in).
        let mut response_ids: Vec<String> = Vec::new();
        if let Some(rid) = response_id.filter(|s| !s.is_empty()) {
            response_ids.push(rid.to_string());
        }
        match self
            .in_flight_realtime_assistant_response_ids(session_id)
            .await
        {
            Ok(ids) => {
                for id in ids {
                    if !response_ids.contains(&id) {
                        response_ids.push(id);
                    }
                }
            }
            Err(meerkat_core::SessionError::NotFound { .. }) => {}
            Err(meerkat_core::SessionError::Unsupported(_)) => {}
            Err(err) => return Err(session_error_to_projection(err, session_id)),
        }
        for rid in response_ids {
            let event = RealtimeTranscriptEvent::AssistantTurnInterrupted { response_id: rid };
            match self
                .service
                .append_realtime_transcript_event(session_id, event)
                .await
            {
                Ok(_) => {}
                Err(
                    meerkat_core::SessionError::NotFound { .. }
                    | meerkat_core::SessionError::Unsupported(_),
                ) => {}
                Err(err) => return Err(session_error_to_projection(err, session_id)),
            }
        }

        match self
            .service
            .interrupt_with_machine_authority(session_id, self.machine.session_control_authority())
            .await
        {
            Ok(()) => Ok(()),
            Err(meerkat_core::SessionError::NotRunning { .. }) => Ok(()),
            Err(err) => Err(session_error_to_projection(err, session_id)),
        }
    }

    async fn signal_turn_completed(
        &self,
        session_id: &SessionId,
        stop_reason: StopReason,
        usage: Usage,
        response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        // CC2: synthesize AssistantTurnCompleted so the staging materializer
        // commits staged transcript items; then drain the display-text
        // buffer with single-counted usage.
        let mut realtime_materialized = false;
        if let Some(rid) = response_id.filter(|s| !s.is_empty()) {
            let event = RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: rid.to_string(),
                stop_reason,
                usage: usage.clone(),
            };
            match self
                .service
                .append_realtime_transcript_event(session_id, event)
                .await
            {
                Ok(outcome) => {
                    realtime_materialized = !outcome.is_inert();
                }
                Err(
                    meerkat_core::SessionError::NotFound { .. }
                    | meerkat_core::SessionError::Unsupported(_),
                ) => {}
                Err(err) => return Err(session_error_to_projection(err, session_id)),
            }
        }

        let pending = self.drain_pending_turn(session_id, response_id);
        let blocks = collapse_pending_blocks(pending.blocks);
        if realtime_materialized && blocks.is_empty() {
            return Ok(());
        }
        let usage_for_drain = if realtime_materialized {
            Usage::default()
        } else {
            usage
        };
        self.service
            .append_external_assistant_output(session_id, blocks, stop_reason, usage_for_drain)
            .await
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn signal_terminal_error(
        &self,
        session_id: &SessionId,
        code: LiveAdapterErrorCode,
        message: &str,
    ) -> Result<(), LiveProjectionError> {
        self.drain_all_pending_turns(session_id);
        tracing::warn!(
            target: "meerkat::surface::live_projection",
            session_id = %session_id,
            ?code,
            message,
            "live adapter terminal error",
        );
        self.service
            .record_live_terminal_error(session_id, code)
            .await
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn signal_output_audio_degraded(
        &self,
        session_id: &SessionId,
        dropped: u64,
    ) -> Result<(), LiveProjectionError> {
        self.service
            .record_live_output_audio_degraded(session_id, dropped)
            .await
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn append_realtime_transcript(
        &self,
        session_id: &SessionId,
        event: &RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::RealtimeTranscriptApplyOutcome, LiveProjectionError> {
        self.service
            .append_realtime_transcript_event(session_id, event.clone())
            .await
            .map_err(|err| session_error_to_projection(err, session_id))
    }
}
