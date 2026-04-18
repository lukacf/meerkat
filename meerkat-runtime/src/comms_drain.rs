//! Comms inbox drain task.
//!
//! Standalone tokio task that drains `CommsRuntime` inbox and feeds typed
//! `Input` values into `MeerkatMachine`. Replaces the old
//! `RuntimeCommsBridge` (comms_sink.rs) which implemented the now-removed
//! `RuntimeInputSink` trait on `meerkat-core`.

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::agent::CommsRuntime;
use meerkat_core::comms::{CommsCommand, PeerName, TrustedPeerSpec};
use meerkat_core::event::AgentEvent;
use meerkat_core::interaction::{InteractionContent, PeerInputCandidate, PeerInputClass};
use meerkat_core::lifecycle::RunControlCommand;
use meerkat_core::types::SessionId;

use meerkat_contracts::wire::supervisor_bridge::{
    BridgeAck, BridgeBindResponse, BridgeCapabilities, BridgeCommand, BridgeDeliveryOutcome,
    BridgeDeliveryPayload, BridgeDeliveryResponse, BridgeDestroyResponse, BridgeMemberRuntimeState,
    BridgeObservationResponse, BridgePeerConnectivity, BridgePeerSpec, BridgeRejectionCause,
    BridgeReply, BridgeRetireResponse, BridgeSupervisorPayload,
    SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM, SUPERVISOR_BRIDGE_INTENT,
    SUPERVISOR_BRIDGE_PROTOCOL_VERSION, canonicalize_bridge_address,
};

use crate::comms_bridge::classified_interaction_to_runtime_input;
use crate::completion::CompletionOutcome;
use crate::identifiers::IdempotencyKey;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
};
use crate::meerkat_machine::{DrainExitReason, MeerkatMachine};
use crate::service_ext::SessionServiceRuntimeExt as _;
use crate::tokio::sync::mpsc;
use crate::traits::RuntimeControlPlane;

/// Default idle timeout for session-backed comms drains.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Per-drain supervisor authorization state.
///
/// Each comms drain task is per-session, so this is local to the task —
/// no global mutable state needed.
#[derive(Debug, Clone)]
struct AuthorizedSupervisorState {
    supervisor: TrustedPeerSpec,
    epoch: u64,
}

/// Spawn a background task that drains the comms inbox and routes
/// classified interactions through the runtime adapter.
///
/// The task runs until the comms runtime signals DISMISS or the returned
/// `JoinHandle` is aborted by the drain lifecycle authority.
pub fn spawn_comms_drain(
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
    comms_runtime: Arc<dyn CommsRuntime>,
    idle_timeout: Option<Duration>,
) -> crate::tokio::task::JoinHandle<()> {
    let timeout_dur = idle_timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT);
    let runtime_id = LogicalRuntimeId::new(session_id.to_string());

    crate::tokio::spawn(async move {
        let inbox_notify = comms_runtime.inbox_notify();
        // Per-drain supervisor state — no global registry needed.
        let mut supervisor_state: Option<AuthorizedSupervisorState> = None;

        loop {
            // Register BEFORE drain — notify_waiters() guarantees wakeup
            // from creation, so a message arriving between drain-returns-empty
            // and the await cannot be lost. No enable() needed.
            // (Mirrors the pattern in CommsRuntime::recv_message.)
            let notified = inbox_notify.notified();

            let candidates = comms_runtime.drain_peer_input_candidates().await;
            if candidates.is_empty() {
                // Check DISMISS on empty drain.
                if comms_runtime.dismiss_received() {
                    tracing::info!("comms_drain: DISMISS received, stopping");
                    let _ = adapter
                        .stop_runtime_executor(
                            &session_id,
                            RunControlCommand::StopRuntimeExecutor {
                                reason: "peer DISMISS".to_string(),
                            },
                        )
                        .await;
                    adapter
                        .notify_comms_drain_exited(&session_id, DrainExitReason::Dismissed)
                        .await;
                    return;
                }
                if crate::tokio::time::timeout(timeout_dur, notified)
                    .await
                    .is_err()
                {
                    tracing::info!("comms_drain: idle timeout expired, stopping");
                    adapter
                        .notify_comms_drain_exited(&session_id, DrainExitReason::IdleTimeout)
                        .await;
                    return;
                }
                continue;
            }

            // Route each classified interaction through the adapter.
            for candidate in candidates {
                if try_handle_supervisor_bridge_command(
                    &adapter,
                    &session_id,
                    &comms_runtime,
                    &candidate,
                    &mut supervisor_state,
                )
                .await
                {
                    continue;
                }
                match candidate.class {
                    PeerInputClass::Ack => {
                        // Ack envelopes are filtered at ingress. Skip here.
                    }
                    PeerInputClass::PeerLifecycleAdded
                    | PeerInputClass::PeerLifecycleRetired
                    | PeerInputClass::PeerLifecycleUnwired => {
                        // Lifecycle events must be injected as session context
                        // so the LLM knows when peers connect/disconnect.
                        // comms_drain is the sole keep-alive inbox consumer.
                        let input =
                            classified_interaction_to_runtime_input(&candidate, &runtime_id);
                        if let Err(err) = adapter.accept_input(&session_id, input).await {
                            tracing::warn!(
                                error = %err,
                                "comms_drain: failed to inject peer lifecycle context"
                            );
                        }
                    }
                    PeerInputClass::Response => {
                        // Distinguish progress responses from terminal responses.
                        let is_terminal = matches!(
                            &candidate.interaction.content,
                            meerkat_core::interaction::InteractionContent::Response {
                                status,
                                ..
                            } if matches!(
                                status,
                                meerkat_core::interaction::ResponseStatus::Completed
                                    | meerkat_core::interaction::ResponseStatus::Failed
                            )
                        );

                        if is_terminal {
                            // Terminal response — single admission with
                            // completion tracking. The PeerInput already
                            // carries ResponseTerminal convention which the
                            // policy table maps to WakeIfIdle/Steer as needed.
                            // No synthetic Continuation required.
                            let interaction_id = candidate.interaction.id;
                            let subscriber = comms_runtime.interaction_subscriber(&interaction_id);
                            let content_input =
                                classified_interaction_to_runtime_input(&candidate, &runtime_id);
                            let result = adapter
                                .accept_input_with_completion(&session_id, content_input)
                                .await;
                            match result {
                                Ok((_outcome, handle)) => {
                                    if subscriber.is_some() || handle.is_some() {
                                        spawn_completion_bridge(
                                            Some(comms_runtime.clone()),
                                            interaction_id,
                                            subscriber,
                                            handle,
                                        );
                                    } else {
                                        comms_runtime.mark_interaction_complete(&interaction_id);
                                    }
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        "comms_drain: failed to inject terminal response"
                                    );
                                    comms_runtime.mark_interaction_complete(&interaction_id);
                                }
                            }
                        } else {
                            // Progress response — route as peer input for checkpoint-style handling.
                            let input =
                                classified_interaction_to_runtime_input(&candidate, &runtime_id);
                            if let Err(err) = adapter.accept_input(&session_id, input).await {
                                tracing::warn!(
                                    error = %err,
                                    "comms_drain: failed to inject progress response"
                                );
                            }
                        }
                    }
                    PeerInputClass::SilentRequest
                    | PeerInputClass::PeerLifecycleKickoffFailed
                    | PeerInputClass::PeerLifecycleKickoffCancelled
                    | PeerInputClass::ActionableMessage
                    | PeerInputClass::ActionableRequest
                    | PeerInputClass::PlainEvent => {
                        // Route through the adapter as a peer input.
                        let interaction_id = candidate.interaction.id;
                        let subscriber = comms_runtime.interaction_subscriber(&interaction_id);
                        let input =
                            classified_interaction_to_runtime_input(&candidate, &runtime_id);
                        let result = adapter
                            .accept_input_with_completion(&session_id, input)
                            .await;

                        match result {
                            Ok((_outcome, handle)) => {
                                if subscriber.is_some() || handle.is_some() {
                                    spawn_completion_bridge(
                                        Some(comms_runtime.clone()),
                                        interaction_id,
                                        subscriber,
                                        handle,
                                    );
                                } else {
                                    comms_runtime.mark_interaction_complete(&interaction_id);
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    error = %err,
                                    "comms_drain: failed to accept peer input"
                                );
                                comms_runtime.mark_interaction_complete(&interaction_id);
                            }
                        }
                    }
                }
            }
        }
    })
}

fn sender_matches_supervisor(sender: &str, supervisor: &TrustedPeerSpec) -> bool {
    sender == supervisor.name || sender == supervisor.peer_id
}

fn sender_matches_bridge_peer(sender: &str, peer: &BridgePeerSpec) -> bool {
    sender == peer.name || sender == peer.peer_id
}

fn require_authorized_supervisor(
    sender: &str,
    payload: &BridgeSupervisorPayload,
    current: &Option<AuthorizedSupervisorState>,
) -> Result<AuthorizedSupervisorState, (BridgeRejectionCause, String)> {
    if payload.protocol_version != SUPERVISOR_BRIDGE_PROTOCOL_VERSION {
        return Err((
            BridgeRejectionCause::UnsupportedProtocolVersion,
            format!(
                "unsupported bridge protocol version {} (expected {})",
                payload.protocol_version, SUPERVISOR_BRIDGE_PROTOCOL_VERSION
            ),
        ));
    }
    let Some(current) = current else {
        return Err((
            BridgeRejectionCause::NotBound,
            "no authorized supervisor registered".to_string(),
        ));
    };
    if payload.epoch < current.epoch {
        return Err((
            BridgeRejectionCause::StaleSupervisor,
            format!(
                "stale supervisor epoch {} (current {})",
                payload.epoch, current.epoch
            ),
        ));
    }
    if payload.epoch != current.epoch {
        return Err((
            BridgeRejectionCause::StaleSupervisor,
            format!(
                "unexpected supervisor epoch {} (current {})",
                payload.epoch, current.epoch
            ),
        ));
    }
    if payload.supervisor.peer_id != current.supervisor.peer_id {
        return Err((
            BridgeRejectionCause::StaleSupervisor,
            format!(
                "stale supervisor peer '{}' (current '{}')",
                payload.supervisor.peer_id, current.supervisor.peer_id
            ),
        ));
    }
    if !sender_matches_supervisor(sender, &current.supervisor) {
        return Err((
            BridgeRejectionCause::SenderMismatch,
            format!(
                "request sender '{sender}' does not match authorized supervisor '{}'",
                current.supervisor.peer_id
            ),
        ));
    }
    Ok(current.clone())
}

fn bridge_capabilities() -> BridgeCapabilities {
    BridgeCapabilities {
        deliver_member_input: true,
        observe_member: true,
        interrupt_member: true,
        retire_member: true,
        destroy_member: true,
        wire_member: true,
        unwire_member: true,
    }
}

fn peer_input_from_delivery_payload(
    session_id: &SessionId,
    sender: &str,
    request_id: meerkat_core::interaction::InteractionId,
    payload: BridgeDeliveryPayload,
) -> Input {
    let (body, blocks) = match payload.content {
        meerkat_core::types::ContentInput::Text(body) => (body, None),
        meerkat_core::types::ContentInput::Blocks(blocks) => {
            let body = meerkat_core::types::text_content(&blocks);
            (body, Some(blocks))
        }
    };

    Input::Peer(PeerInput {
        header: InputHeader {
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::Peer {
                peer_id: sender.to_string(),
                runtime_id: Some(LogicalRuntimeId::new(session_id.to_string())),
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility {
                transcript_eligible: true,
                operator_eligible: true,
            },
            idempotency_key: Some(IdempotencyKey::new(payload.input_id)),
            supersession_key: None,
            correlation_id: Some(crate::identifiers::CorrelationId::from_uuid(request_id.0)),
        },
        convention: Some(PeerConvention::Message),
        body,
        payload: None,
        blocks,
        handling_mode: match payload.handling_mode {
            meerkat_core::types::HandlingMode::Queue => None,
            mode => Some(mode),
        },
    })
}

fn advertised_bind_bootstrap_token(
    comms_runtime: &Arc<dyn CommsRuntime>,
) -> Result<String, (BridgeRejectionCause, String)> {
    if let Some(token) = comms_runtime.bridge_bootstrap_token()
        && !token.is_empty()
    {
        return Ok(token);
    }
    let address = comms_runtime.advertised_address().ok_or_else(|| {
        (
            BridgeRejectionCause::Internal,
            "runtime does not expose an advertised address for bind bootstrap".to_string(),
        )
    })?;
    let query = address
        .split_once('?')
        .map(|(_, query)| query)
        .ok_or_else(|| {
            (
                BridgeRejectionCause::InvalidBootstrapToken,
                format!(
                    "runtime advertised address '{address}' is missing '{SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM}' query param"
                ),
            )
        })?;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key == SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM && !value.is_empty() {
            return Ok(value.to_string());
        }
    }
    Err((
        BridgeRejectionCause::InvalidBootstrapToken,
        format!(
            "runtime advertised address '{address}' is missing '{SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM}' query param"
        ),
    ))
}

fn validate_bind_request(
    comms_runtime: &Arc<dyn CommsRuntime>,
    sender: &str,
    payload: &meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload,
) -> Result<(TrustedPeerSpec, String), (BridgeRejectionCause, String)> {
    if payload.protocol_version != SUPERVISOR_BRIDGE_PROTOCOL_VERSION {
        return Err((
            BridgeRejectionCause::UnsupportedProtocolVersion,
            format!(
                "unsupported bridge protocol version {} (expected {})",
                payload.protocol_version, SUPERVISOR_BRIDGE_PROTOCOL_VERSION
            ),
        ));
    }
    let expected_bootstrap_token = advertised_bind_bootstrap_token(comms_runtime)?;
    let advertised_address = comms_runtime.advertised_address().ok_or_else(|| {
        (
            BridgeRejectionCause::Internal,
            "runtime does not expose an advertised address for bind bootstrap".to_string(),
        )
    })?;
    if canonicalize_bridge_address(&payload.expected_address)
        != canonicalize_bridge_address(&advertised_address)
    {
        return Err((
            BridgeRejectionCause::AddressMismatch,
            format!(
                "bind address mismatch: expected '{}', actual '{}'",
                payload.expected_address, advertised_address
            ),
        ));
    }
    if !sender_matches_bridge_peer(sender, &payload.supervisor) {
        return Err((
            BridgeRejectionCause::SenderMismatch,
            format!(
                "request sender '{sender}' does not match supervisor '{}'",
                payload.supervisor.peer_id
            ),
        ));
    }
    if let Some(actual_peer_id) = comms_runtime.public_key()
        && actual_peer_id != payload.expected_peer_id
    {
        return Err((
            BridgeRejectionCause::InvalidPeerSpec,
            format!(
                "bind peer_id mismatch: expected '{}', actual '{actual_peer_id}'",
                payload.expected_peer_id
            ),
        ));
    }
    if payload.bootstrap_token.as_str() != expected_bootstrap_token {
        return Err((
            BridgeRejectionCause::InvalidBootstrapToken,
            "bind member failed: invalid bootstrap token".to_string(),
        ));
    }
    let supervisor = TrustedPeerSpec::try_from(payload.supervisor.clone()).map_err(|error| {
        (
            BridgeRejectionCause::InvalidSupervisorSpec,
            format!("bind member failed: invalid supervisor peer spec: {error}"),
        )
    })?;
    Ok((supervisor, advertised_address))
}

#[derive(Debug)]
enum AuthorizeSupervisorGate {
    IdempotentAck,
    Proceed,
}

/// Gate decision for `BindMember` when a supervisor is already bound.
///
/// Once `supervisor_state` is populated, the only safe outcome is an
/// idempotent acknowledgement for the exact same supervisor + epoch +
/// authenticated sender. Anything else is rejected so that a caller who
/// still knows the bootstrap token cannot seize authority by re-binding
/// with a different supervisor or lower epoch — rotation must go through
/// `AuthorizeSupervisor` (which requires the *current* supervisor to
/// sign).
#[derive(Debug)]
enum BindMemberGate {
    /// No supervisor bound yet — the caller may proceed with the full
    /// bootstrap validation (`validate_bind_request`).
    Bootstrap,
    /// A supervisor is already bound and the request exactly matches it
    /// (same peer id, same epoch, authenticated sender). Reply with the
    /// same `BridgeBindResponse` we would have returned originally; do
    /// not mutate `supervisor_state` or re-trust the peer.
    IdempotentAck,
}

fn validate_bind_request_against_state(
    sender: &str,
    payload: &meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload,
    supervisor_state: &Option<AuthorizedSupervisorState>,
) -> Result<BindMemberGate, (BridgeRejectionCause, String)> {
    if payload.protocol_version != SUPERVISOR_BRIDGE_PROTOCOL_VERSION {
        return Err((
            BridgeRejectionCause::UnsupportedProtocolVersion,
            format!(
                "unsupported bridge protocol version {} (expected {})",
                payload.protocol_version, SUPERVISOR_BRIDGE_PROTOCOL_VERSION
            ),
        ));
    }
    let Some(current) = supervisor_state.as_ref() else {
        return Ok(BindMemberGate::Bootstrap);
    };
    if payload.supervisor.peer_id != current.supervisor.peer_id {
        return Err((
            BridgeRejectionCause::AlreadyBound,
            format!(
                "bind member failed: supervisor already bound as '{}'; use authorize_supervisor to rotate",
                current.supervisor.peer_id
            ),
        ));
    }
    if payload.epoch != current.epoch {
        return Err((
            BridgeRejectionCause::AlreadyBound,
            format!(
                "bind member failed: epoch {} does not match bound supervisor epoch {}; use authorize_supervisor to rotate",
                payload.epoch, current.epoch
            ),
        ));
    }
    if !sender_matches_supervisor(sender, &current.supervisor) {
        return Err((
            BridgeRejectionCause::SenderMismatch,
            format!(
                "bind member failed: request sender '{sender}' does not match authorized supervisor '{}'",
                current.supervisor.peer_id
            ),
        ));
    }
    Ok(BindMemberGate::IdempotentAck)
}

fn validate_authorize_supervisor_request(
    sender: &str,
    payload: &BridgeSupervisorPayload,
    supervisor_state: &Option<AuthorizedSupervisorState>,
) -> Result<AuthorizeSupervisorGate, (BridgeRejectionCause, String)> {
    if payload.protocol_version != SUPERVISOR_BRIDGE_PROTOCOL_VERSION {
        return Err((
            BridgeRejectionCause::UnsupportedProtocolVersion,
            format!(
                "authorize supervisor failed: unsupported bridge protocol version {} (expected {})",
                payload.protocol_version, SUPERVISOR_BRIDGE_PROTOCOL_VERSION
            ),
        ));
    }
    if let Some(current) = supervisor_state.as_ref() {
        if payload.epoch < current.epoch {
            return Err((
                BridgeRejectionCause::StaleSupervisor,
                format!(
                    "authorize supervisor failed: stale supervisor epoch {} (current {})",
                    payload.epoch, current.epoch
                ),
            ));
        }
        if !sender_matches_supervisor(sender, &current.supervisor) {
            return Err((
                BridgeRejectionCause::SenderMismatch,
                format!(
                    "authorize supervisor failed: request sender '{sender}' does not match authorized supervisor '{}'",
                    current.supervisor.peer_id
                ),
            ));
        }
        if payload.epoch == current.epoch
            && payload.supervisor.peer_id == current.supervisor.peer_id
        {
            return Ok(AuthorizeSupervisorGate::IdempotentAck);
        }
        return Ok(AuthorizeSupervisorGate::Proceed);
    }

    if !sender_matches_bridge_peer(sender, &payload.supervisor) {
        return Err((
            BridgeRejectionCause::SenderMismatch,
            format!(
                "authorize supervisor failed: request sender '{sender}' does not match supervisor '{}'",
                payload.supervisor.peer_id
            ),
        ));
    }

    Err((
        BridgeRejectionCause::NotBound,
        "authorize supervisor failed: use bind_member to establish initial supervisor authority"
            .to_string(),
    ))
}

async fn send_bridge_response(
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
    status: meerkat_core::interaction::ResponseStatus,
    reply: BridgeReply,
) {
    let result = serde_json::to_value(&reply).unwrap_or_else(|error| {
        tracing::error!(
            interaction_id = %candidate.interaction.id,
            error = %error,
            "comms_drain: BridgeReply serialization failed; falling back to minimal rejection"
        );
        serde_json::json!({
            "result": "rejected",
            "cause": "internal",
            "reason": "bridge reply serialization failed",
        })
    });
    let to = match PeerName::new(candidate.interaction.from.clone()) {
        Ok(name) => name,
        Err(error) => {
            tracing::warn!(
                from = %candidate.interaction.from,
                interaction_id = %candidate.interaction.id,
                error = %error,
                "comms_drain: failed to route bridge response"
            );
            comms_runtime.mark_interaction_complete(&candidate.interaction.id);
            return;
        }
    };

    if let Err(error) = comms_runtime
        .send(CommsCommand::PeerResponse {
            to,
            in_reply_to: candidate.interaction.id,
            status,
            result,
            handling_mode: None,
        })
        .await
    {
        tracing::warn!(
            from = %candidate.interaction.from,
            interaction_id = %candidate.interaction.id,
            error = %error,
            "comms_drain: failed to send bridge response"
        );
    }
    comms_runtime.mark_interaction_complete(&candidate.interaction.id);
}

async fn send_bridge_failure(
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
    cause: BridgeRejectionCause,
    message: impl Into<String>,
) {
    send_bridge_response(
        comms_runtime,
        candidate,
        meerkat_core::interaction::ResponseStatus::Failed,
        BridgeReply::Rejected {
            cause,
            reason: message.into(),
        },
    )
    .await;
}

/// Try to handle a supervisor bridge command from an incoming comms request.
///
/// Returns `true` if the candidate was a bridge command (handled or rejected),
/// `false` if it was not a bridge command and should be processed normally.
async fn try_handle_supervisor_bridge_command(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
    supervisor_state: &mut Option<AuthorizedSupervisorState>,
) -> bool {
    let InteractionContent::Request { intent, params } = &candidate.interaction.content else {
        return false;
    };

    // All supervisor bridge commands arrive under a single typed intent
    // (`SUPERVISOR_BRIDGE_INTENT`); per-command intents were never part of
    // the wire contract. Any other intent is left for other dispatchers.
    if intent != SUPERVISOR_BRIDGE_INTENT {
        return false;
    }

    let command: BridgeCommand = match serde_json::from_value(params.clone()) {
        Ok(cmd) => cmd,
        Err(error) => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                BridgeRejectionCause::Unsupported,
                format!("invalid bridge command: {error}"),
            )
            .await;
            return true;
        }
    };

    let sender = &candidate.interaction.from;

    match command {
        BridgeCommand::BindMember(payload) => {
            // Reject rebind attempts once a supervisor is already bound;
            // only exact-match retries from the authorized sender get an
            // idempotent ack. This prevents any holder of the bootstrap
            // token from seizing authority without going through
            // AuthorizeSupervisor/rotation (which requires the current
            // supervisor to sign).
            let gate = match validate_bind_request_against_state(sender, &payload, supervisor_state)
            {
                Ok(gate) => gate,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                    return true;
                }
            };
            match gate {
                BindMemberGate::IdempotentAck => {
                    // Idempotent-ack replies use canonical runtime identity,
                    // never the caller-supplied `payload.expected_*`. If the
                    // runtime cannot produce its own identity at this point
                    // something upstream broke our invariant (a prior bind
                    // would have failed without these): surface an internal
                    // rejection and do NOT echo attacker-controlled fields.
                    let Some(advertised) = comms_runtime.advertised_address() else {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            "idempotent ack invariant violated",
                        )
                        .await;
                        return true;
                    };
                    let Some(peer_id) = comms_runtime.public_key() else {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            "idempotent ack invariant violated",
                        )
                        .await;
                        return true;
                    };
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::BindMember(BridgeBindResponse {
                            peer_id,
                            address: canonicalize_bridge_address(&advertised),
                            capabilities: bridge_capabilities(),
                        }),
                    )
                    .await;
                    return true;
                }
                BindMemberGate::Bootstrap => {}
            }
            let (supervisor_spec, advertised_address) =
                match validate_bind_request(comms_runtime, sender, &payload) {
                    Ok(binding) => binding,
                    Err((cause, reason)) => {
                        send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                        return true;
                    }
                };
            match comms_runtime
                .add_trusted_peer(supervisor_spec.clone())
                .await
            {
                Ok(()) => {
                    *supervisor_state = Some(AuthorizedSupervisorState {
                        supervisor: supervisor_spec.clone(),
                        epoch: payload.epoch,
                    });
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::BindMember(BridgeBindResponse {
                            peer_id: comms_runtime
                                .public_key()
                                .unwrap_or(payload.expected_peer_id),
                            address: canonicalize_bridge_address(&advertised_address),
                            capabilities: bridge_capabilities(),
                        }),
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("bind member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::AuthorizeSupervisor(payload) => {
            match validate_authorize_supervisor_request(sender, &payload, supervisor_state) {
                Ok(AuthorizeSupervisorGate::IdempotentAck) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                    )
                    .await;
                    return true;
                }
                Ok(AuthorizeSupervisorGate::Proceed) => {}
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                    return true;
                }
            }

            let old_supervisor = supervisor_state.as_ref().map(|s| s.supervisor.clone());
            let supervisor_spec = match TrustedPeerSpec::try_from(payload.supervisor.clone()) {
                Ok(spec) => spec,
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::InvalidSupervisorSpec,
                        format!(
                            "authorize supervisor failed: invalid supervisor peer spec: {error}"
                        ),
                    )
                    .await;
                    return true;
                }
            };
            match comms_runtime
                .add_trusted_peer(supervisor_spec.clone())
                .await
            {
                Ok(()) => {
                    if let Some(old_supervisor) = old_supervisor
                        && old_supervisor.peer_id != payload.supervisor.peer_id
                        && let Err(error) = comms_runtime
                            .remove_trusted_peer(&old_supervisor.peer_id)
                            .await
                    {
                        // Not a hard failure: the new supervisor is already trusted and
                        // the state cutover has happened. But leaving the old supervisor
                        // trusted is a cleanup regression we must surface — otherwise the
                        // old supervisor can still spend comms bandwidth until the next
                        // boundary.
                        tracing::warn!(
                            old_supervisor = %old_supervisor.peer_id,
                            error = %error,
                            "authorize supervisor: failed to remove previous supervisor trust"
                        );
                    }
                    *supervisor_state = Some(AuthorizedSupervisorState {
                        supervisor: supervisor_spec,
                        epoch: payload.epoch,
                    });
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("authorize supervisor failed: {error}"),
                    )
                    .await;
                    return true;
                }
            }
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::Ack(BridgeAck { ok: true }),
            )
            .await;
            true
        }
        BridgeCommand::RevokeSupervisor(payload) => {
            if let Err((cause, reason)) =
                require_authorized_supervisor(sender, &payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            if let Err(error) = comms_runtime
                .remove_trusted_peer(&payload.supervisor.peer_id)
                .await
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("revoke supervisor failed: {error}"),
                )
                .await;
                return true;
            }
            *supervisor_state = None;
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::Ack(BridgeAck { ok: true }),
            )
            .await;
            true
        }
        BridgeCommand::DeliverMemberInput(payload) => {
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) =
                require_authorized_supervisor(sender, &sup_payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            let request_input_id = payload.input_id.clone();
            let input = peer_input_from_delivery_payload(
                session_id,
                sender,
                candidate.interaction.id,
                payload,
            );
            match adapter.accept_input(session_id, input).await {
                Ok(outcome) => {
                    let response = match outcome {
                        crate::accept::AcceptOutcome::Accepted { input_id, .. } => {
                            BridgeDeliveryResponse {
                                input_id: request_input_id,
                                canonical_input_id: Some(input_id.to_string()),
                                outcome: BridgeDeliveryOutcome::Accepted,
                            }
                        }
                        crate::accept::AcceptOutcome::Deduplicated { existing_id, .. } => {
                            let existing_id = existing_id.to_string();
                            BridgeDeliveryResponse {
                                input_id: request_input_id,
                                canonical_input_id: Some(existing_id.clone()),
                                outcome: BridgeDeliveryOutcome::Deduplicated {
                                    existing_input_id: existing_id,
                                },
                            }
                        }
                        crate::accept::AcceptOutcome::Rejected { reason } => {
                            BridgeDeliveryResponse {
                                input_id: request_input_id,
                                canonical_input_id: None,
                                outcome: BridgeDeliveryOutcome::Rejected {
                                    reason: reason.to_string(),
                                },
                            }
                        }
                    };
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Delivery(response),
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("deliver member input failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::InterruptMember(payload) => {
            if let Err((cause, reason)) =
                require_authorized_supervisor(sender, &payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            match adapter.interrupt_current_run(session_id).await {
                Ok(()) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("interrupt member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::RetireMember(payload) => {
            if let Err((cause, reason)) =
                require_authorized_supervisor(sender, &payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            match adapter.retire_runtime(session_id).await {
                Ok(report) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Retire(BridgeRetireResponse {
                            inputs_abandoned: report.inputs_abandoned,
                            inputs_pending_drain: report.inputs_pending_drain,
                        }),
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("retire member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::DestroyMember(payload) => {
            if let Err((cause, reason)) =
                require_authorized_supervisor(sender, &payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            let runtime_id = LogicalRuntimeId::new(session_id.to_string());
            match RuntimeControlPlane::destroy(adapter.as_ref(), &runtime_id).await {
                Ok(report) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Destroy(BridgeDestroyResponse {
                            inputs_abandoned: report.inputs_abandoned,
                        }),
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("destroy member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::ObserveMember(payload) => {
            if let Err((cause, reason)) =
                require_authorized_supervisor(sender, &payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            match crate::service_ext::SessionServiceRuntimeExt::runtime_state(
                adapter.as_ref(),
                session_id,
            )
            .await
            {
                Ok(state) => {
                    let current_run_id = adapter
                        .meerkat_machine_spine_snapshot(session_id)
                        .await
                        .and_then(|snapshot| {
                            snapshot
                                .control
                                .current_run_id
                                .map(|run_id| run_id.to_string())
                        });
                    let bridge_state = runtime_state_to_bridge(state);
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Observation(BridgeObservationResponse::new(
                            bridge_state,
                            Some(state.can_accept_input()),
                            current_run_id,
                            Some(BridgePeerConnectivity::Reachable),
                            None,
                            chrono::Utc::now().to_rfc3339(),
                        )),
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("observe member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::WireMember(payload) => {
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) =
                require_authorized_supervisor(sender, &sup_payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            let peer_spec = match TrustedPeerSpec::try_from(payload.peer_spec) {
                Ok(spec) => spec,
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::InvalidPeerSpec,
                        format!("wire member failed: invalid trusted peer spec: {error}"),
                    )
                    .await;
                    return true;
                }
            };
            match comms_runtime.add_trusted_peer(peer_spec).await {
                Ok(()) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("wire member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::UnwireMember(payload) => {
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) =
                require_authorized_supervisor(sender, &sup_payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            match comms_runtime
                .remove_trusted_peer(&payload.peer_spec.peer_id)
                .await
            {
                Ok(_) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("unwire member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        _ => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                BridgeRejectionCause::Unsupported,
                "unsupported supervisor bridge command".to_string(),
            )
            .await;
            true
        }
    }
}

/// Map internal `RuntimeState` to the wire `BridgeMemberRuntimeState`.
fn runtime_state_to_bridge(state: crate::RuntimeState) -> BridgeMemberRuntimeState {
    match state {
        crate::RuntimeState::Initializing => BridgeMemberRuntimeState::Initializing,
        crate::RuntimeState::Idle => BridgeMemberRuntimeState::Idle,
        crate::RuntimeState::Attached => BridgeMemberRuntimeState::Attached,
        crate::RuntimeState::Running => BridgeMemberRuntimeState::Running,
        crate::RuntimeState::Retired => BridgeMemberRuntimeState::Retired,
        crate::RuntimeState::Stopped => BridgeMemberRuntimeState::Stopped,
        crate::RuntimeState::Destroyed => BridgeMemberRuntimeState::Destroyed,
    }
}

fn interaction_terminal_event(
    interaction_id: meerkat_core::interaction::InteractionId,
    outcome: CompletionOutcome,
) -> AgentEvent {
    match outcome {
        CompletionOutcome::Completed(result) => AgentEvent::InteractionComplete {
            interaction_id,
            result: result.text,
        },
        CompletionOutcome::CompletedWithoutResult => AgentEvent::InteractionComplete {
            interaction_id,
            result: String::new(),
        },
        CompletionOutcome::CallbackPending { tool_name, args } => {
            AgentEvent::InteractionCallbackPending {
                interaction_id,
                tool_name,
                args,
            }
        }
        CompletionOutcome::Abandoned(reason) | CompletionOutcome::RuntimeTerminated(reason) => {
            AgentEvent::InteractionFailed {
                interaction_id,
                error: reason,
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_contracts::BridgePeerWiringPayload;
    use meerkat_core::InteractionId;
    use meerkat_core::SendError;
    use meerkat_core::interaction::InboxInteraction;
    use meerkat_core::types::HandlingMode;
    use serde_json::json;
    use uuid::Uuid;

    struct BootstrapRuntime {
        peer_id: String,
        address: String,
        bootstrap_token: Option<String>,
        inbox_notify: Arc<tokio::sync::Notify>,
        remove_trusted_peer_error: Option<String>,
    }

    #[async_trait::async_trait]
    impl CommsRuntime for BootstrapRuntime {
        fn public_key(&self) -> Option<String> {
            Some(self.peer_id.clone())
        }

        fn advertised_address(&self) -> Option<String> {
            Some(self.address.clone())
        }

        fn bridge_bootstrap_token(&self) -> Option<String> {
            self.bootstrap_token.clone()
        }

        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            self.inbox_notify.clone()
        }

        async fn add_trusted_peer(&self, _peer: TrustedPeerSpec) -> Result<(), SendError> {
            Ok(())
        }

        async fn remove_trusted_peer(&self, _peer_id: &str) -> Result<bool, SendError> {
            match &self.remove_trusted_peer_error {
                Some(message) => Err(SendError::Internal(message.clone())),
                None => Ok(true),
            }
        }
    }

    fn lifecycle_candidate(
        class: PeerInputClass,
        intent: &str,
        params: serde_json::Value,
    ) -> PeerInputCandidate {
        PeerInputCandidate {
            interaction: InboxInteraction {
                id: InteractionId(Uuid::new_v4()),
                from: "test-mob/__mob_supervisor__".to_string(),
                content: InteractionContent::Request {
                    intent: intent.to_string(),
                    params,
                },
                rendered_text: String::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
            },
            class,
            lifecycle_peer: Some("peer-1".to_string()),
        }
    }

    #[test]
    fn callback_pending_maps_to_interaction_callback_pending_terminal_event() {
        let interaction_id = InteractionId(Uuid::new_v4());
        let event = interaction_terminal_event(
            interaction_id,
            CompletionOutcome::CallbackPending {
                tool_name: "external_mock".to_string(),
                args: json!({ "value": "browser" }),
            },
        );

        assert!(
            matches!(event, AgentEvent::InteractionCallbackPending { .. }),
            "expected callback-pending interaction event"
        );
        if let AgentEvent::InteractionCallbackPending {
            interaction_id: actual_id,
            tool_name,
            args,
        } = event
        {
            assert_eq!(actual_id, interaction_id);
            assert_eq!(tool_name, "external_mock");
            assert_eq!(args, json!({ "value": "browser" }));
        }
    }

    #[tokio::test]
    async fn peer_lifecycle_added_does_not_change_comms_trust() {
        let runtime: Arc<dyn CommsRuntime> =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("receiver-added").unwrap());
        let peer = meerkat_comms::CommsRuntime::inproc_only("peer-added").unwrap();
        let peer_spec = TrustedPeerSpec::new(
            "peer-added".to_string(),
            peer.public_key().to_peer_id(),
            "inproc://peer-added".to_string(),
        )
        .unwrap();
        let candidate = lifecycle_candidate(
            PeerInputClass::PeerLifecycleAdded,
            "mob.peer_added",
            json!({
                "peer": "peer-1",
                "peer_spec": peer_spec,
            }),
        );

        let peers_before = runtime.peers().await;
        assert!(
            peers_before.is_empty(),
            "test runtime should start without trust"
        );
        let input =
            classified_interaction_to_runtime_input(&candidate, &LogicalRuntimeId::new("s-1"));
        assert!(
            matches!(input, Input::Peer(_)),
            "lifecycle candidate should still route as peer input"
        );
        let peers = runtime.peers().await;
        assert!(
            peers.is_empty(),
            "peer lifecycle add must not materialize comms trust before topology validation"
        );
    }

    #[tokio::test]
    async fn peer_lifecycle_unwired_and_retired_do_not_revoke_comms_trust() {
        let runtime: Arc<dyn CommsRuntime> =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("receiver-removed").unwrap());
        let peer = meerkat_comms::CommsRuntime::inproc_only("peer-removed").unwrap();
        let peer_spec = TrustedPeerSpec::new(
            "peer-removed".to_string(),
            peer.public_key().to_peer_id(),
            "inproc://peer-removed".to_string(),
        )
        .unwrap();
        runtime.add_trusted_peer(peer_spec.clone()).await.unwrap();

        let unwired = lifecycle_candidate(
            PeerInputClass::PeerLifecycleUnwired,
            "mob.peer_unwired",
            json!({
                "peer": "peer-1",
                "peer_spec": peer_spec.clone(),
            }),
        );
        let _ = classified_interaction_to_runtime_input(&unwired, &LogicalRuntimeId::new("s-1"));
        assert!(
            runtime
                .peers()
                .await
                .iter()
                .any(|entry| entry.name.as_str() == "peer-removed"),
            "peer lifecycle unwire must not revoke comms trust before topology validation"
        );

        runtime.add_trusted_peer(peer_spec.clone()).await.unwrap();
        let retired = lifecycle_candidate(
            PeerInputClass::PeerLifecycleRetired,
            "mob.peer_retired",
            json!({
                "peer": "peer-1",
                "peer_spec": peer_spec,
            }),
        );
        let _ = classified_interaction_to_runtime_input(&retired, &LogicalRuntimeId::new("s-1"));
        assert!(
            runtime
                .peers()
                .await
                .iter()
                .any(|entry| entry.name.as_str() == "peer-removed"),
            "peer lifecycle retire must not revoke comms trust before topology validation"
        );
    }

    #[test]
    fn validate_bind_request_rejects_missing_or_wrong_bootstrap_token() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(BootstrapRuntime {
            peer_id: "ed25519:receiver".to_string(),
            address: "inproc://receiver".to_string(),
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            remove_trusted_peer_error: None,
        });
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: "ed25519:receiver".to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "wrong-token".into(),
        };

        let (cause, error) = validate_bind_request(&runtime, &supervisor.peer_id, &payload)
            .expect_err("bind must reject incorrect bootstrap token");
        assert_eq!(cause, BridgeRejectionCause::InvalidBootstrapToken);
        assert!(
            error.contains("invalid bootstrap token"),
            "bind rejection should explain the bootstrap proof failure, got: {error}"
        );
    }

    #[test]
    fn validate_bind_request_accepts_matching_bootstrap_token() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(BootstrapRuntime {
            peer_id: "ed25519:receiver".to_string(),
            address: "inproc://receiver".to_string(),
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            remove_trusted_peer_error: None,
        });
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: "ed25519:receiver".to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "expected-token".into(),
        };

        let (authorized, advertised_address) =
            validate_bind_request(&runtime, &supervisor.peer_id, &payload)
                .expect("bind should accept the configured bootstrap token");
        assert_eq!(authorized.peer_id, supervisor.peer_id);
        assert_eq!(advertised_address, runtime.advertised_address().unwrap());
    }

    #[test]
    fn validate_bind_request_returns_runtime_advertised_address() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(BootstrapRuntime {
            peer_id: "ed25519:receiver".to_string(),
            address: format!(
                "inproc://receiver-real?{SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM}=expected-token"
            ),
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            remove_trusted_peer_error: None,
        });
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: "ed25519:receiver".to_string(),
            expected_address: "inproc://receiver-real".to_string(),
            bootstrap_token: "expected-token".into(),
        };

        let (_, advertised_address) =
            validate_bind_request(&runtime, &supervisor.peer_id, &payload)
                .expect("bind should canonicalize to the callee's advertised address");
        assert_eq!(advertised_address, runtime.advertised_address().unwrap());
    }

    #[test]
    fn validate_bind_request_rejects_mismatched_expected_address() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(BootstrapRuntime {
            peer_id: "ed25519:receiver".to_string(),
            address: "inproc://receiver-real".to_string(),
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            remove_trusted_peer_error: None,
        });
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: "ed25519:receiver".to_string(),
            expected_address: "inproc://receiver-stale".to_string(),
            bootstrap_token: "expected-token".into(),
        };

        let (cause, error) = validate_bind_request(&runtime, &supervisor.peer_id, &payload)
            .expect_err("bind should reject mismatched expected addresses");
        assert_eq!(cause, BridgeRejectionCause::AddressMismatch);
        assert!(
            error.contains("bind address mismatch"),
            "bind rejection should explain the address mismatch, got: {error}"
        );
    }

    #[test]
    fn validate_bind_request_rejects_protocol_version_mismatch() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(BootstrapRuntime {
            peer_id: "ed25519:receiver".to_string(),
            address: "inproc://receiver".to_string(),
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            remove_trusted_peer_error: None,
        });
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION + 1,
            expected_peer_id: "ed25519:receiver".to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "expected-token".into(),
        };

        let (cause, error) = validate_bind_request(&runtime, &supervisor.peer_id, &payload)
            .expect_err("bind should reject unsupported protocol versions");
        assert_eq!(cause, BridgeRejectionCause::UnsupportedProtocolVersion);
        assert!(
            error.contains("unsupported bridge protocol version"),
            "bind rejection should explain the protocol mismatch, got: {error}"
        );
    }

    #[test]
    fn validate_bind_request_rejects_explicit_v1_protocol_under_v2() {
        // Pin the v1→v2 upgrade contract: a v1 payload MUST be rejected by a
        // v2 runtime at the protocol-version gate, *not* proceed with defaulted
        // fields. Uses the literal `1` (not `SUPERVISOR_BRIDGE_PROTOCOL_VERSION
        // - 1`) so that a future v3 bump re-confirms v1 stays rejected.
        let runtime: Arc<dyn CommsRuntime> = Arc::new(BootstrapRuntime {
            peer_id: "ed25519:receiver".to_string(),
            address: "inproc://receiver".to_string(),
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            remove_trusted_peer_error: None,
        });
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: 1,
            expected_peer_id: "ed25519:receiver".to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "expected-token".into(),
        };

        let (cause, _error) = validate_bind_request(&runtime, &supervisor.peer_id, &payload)
            .expect_err("v1 bind must be rejected under v2+");
        assert_eq!(cause, BridgeRejectionCause::UnsupportedProtocolVersion);
    }

    #[test]
    fn validate_bind_request_rejects_invalid_supervisor_peer_name() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(BootstrapRuntime {
            peer_id: "ed25519:receiver".to_string(),
            address: "inproc://receiver".to_string(),
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            remove_trusted_peer_error: None,
        });
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: BridgePeerSpec {
                name: "".to_string(),
                peer_id: "ed25519:supervisor".to_string(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
            },
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: "ed25519:receiver".to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "expected-token".into(),
        };

        let (cause, error) = validate_bind_request(&runtime, &payload.supervisor.peer_id, &payload)
            .expect_err("bind should reject invalid supervisor peer names");
        assert_eq!(cause, BridgeRejectionCause::InvalidSupervisorSpec);
        assert!(
            error.contains("invalid supervisor peer spec"),
            "bind rejection should explain invalid supervisor identity, got: {error}"
        );
    }

    fn sample_bind_payload() -> meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
        meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: BridgePeerSpec {
                name: "mob/__mob_supervisor__".to_string(),
                peer_id: "ed25519:supervisor".to_string(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
            },
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: "ed25519:receiver".to_string(),
            expected_address: "inproc://receiver".to_string(),
            bootstrap_token: "expected-token".into(),
        }
    }

    fn authorized_state_for(
        payload: &meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload,
    ) -> Option<AuthorizedSupervisorState> {
        Some(AuthorizedSupervisorState {
            supervisor: TrustedPeerSpec::try_from(payload.supervisor.clone())
                .expect("valid supervisor spec"),
            epoch: payload.epoch,
        })
    }

    #[test]
    fn validate_bind_request_against_state_allows_bootstrap_when_unbound() {
        let payload = sample_bind_payload();
        let gate =
            validate_bind_request_against_state(&payload.supervisor.peer_id, &payload, &None)
                .expect("unbound state should accept bootstrap bind");
        assert!(matches!(gate, BindMemberGate::Bootstrap));
    }

    #[test]
    fn validate_bind_request_against_state_rejects_different_supervisor_takeover() {
        // Scenario: a stale bootstrap-token holder tries to seize authority
        // by calling BindMember with a DIFFERENT supervisor peer_id. This is
        // exactly the review vulnerability and must be rejected.
        let current_payload = sample_bind_payload();
        let state = authorized_state_for(&current_payload);
        let mut takeover = sample_bind_payload();
        takeover.supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:adversary".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let (cause, error) =
            validate_bind_request_against_state(&takeover.supervisor.peer_id, &takeover, &state)
                .expect_err("rebind with a different supervisor must be rejected");
        assert_eq!(cause, BridgeRejectionCause::AlreadyBound);
        assert!(
            error.contains("supervisor already bound"),
            "rejection should call out the already-bound supervisor, got: {error}"
        );
        assert!(
            error.contains("authorize_supervisor"),
            "rejection should direct callers to authorize_supervisor for rotation, got: {error}"
        );
    }

    #[test]
    fn validate_bind_request_against_state_rejects_lower_epoch_replay() {
        // Scenario: replay an earlier bind with a lower epoch to regress
        // member state. Must be rejected even when the supervisor peer_id
        // matches — otherwise an attacker can downgrade the member epoch.
        let current_payload = sample_bind_payload();
        let state = authorized_state_for(&current_payload);
        let mut replay = sample_bind_payload();
        replay.epoch = current_payload.epoch - 1;
        let (cause, error) =
            validate_bind_request_against_state(&replay.supervisor.peer_id, &replay, &state)
                .expect_err("lower-epoch rebind must be rejected as a stale replay");
        assert_eq!(cause, BridgeRejectionCause::AlreadyBound);
        assert!(
            error.contains("does not match bound supervisor epoch"),
            "rejection should explain the epoch mismatch, got: {error}"
        );
    }

    #[test]
    fn validate_bind_request_against_state_rejects_higher_epoch_same_supervisor_rebind() {
        // Scenario: same supervisor tries to self-bump epoch via BindMember.
        // Rotation must go through AuthorizeSupervisor — BindMember is only
        // for initial bootstrap after member restart.
        let current_payload = sample_bind_payload();
        let state = authorized_state_for(&current_payload);
        let mut advance = sample_bind_payload();
        advance.epoch = current_payload.epoch + 5;
        let (cause, error) =
            validate_bind_request_against_state(&advance.supervisor.peer_id, &advance, &state)
                .expect_err("higher-epoch rebind with same supervisor must be rejected");
        assert_eq!(cause, BridgeRejectionCause::AlreadyBound);
        assert!(
            error.contains("does not match bound supervisor epoch"),
            "rejection should explain the epoch mismatch, got: {error}"
        );
    }

    #[test]
    fn validate_bind_request_against_state_rejects_spoofed_sender() {
        // Scenario: an attacker forges a BindMember that matches the
        // current supervisor spec but signs it with their own key.
        // sender != authorized supervisor → must reject.
        let current_payload = sample_bind_payload();
        let state = authorized_state_for(&current_payload);
        let retry = sample_bind_payload();
        let (cause, error) =
            validate_bind_request_against_state("ed25519:attacker", &retry, &state)
                .expect_err("bind from an unauthorized sender must be rejected");
        assert_eq!(cause, BridgeRejectionCause::SenderMismatch);
        assert!(
            error.contains("request sender"),
            "rejection should surface the sender mismatch, got: {error}"
        );
    }

    #[test]
    fn validate_bind_request_against_state_idempotently_acks_retry_from_current_supervisor() {
        // Scenario: the authorized supervisor retries BindMember with the
        // exact same payload (e.g., transport-level retry). Must return
        // idempotent ack without mutating state.
        let current_payload = sample_bind_payload();
        let state = authorized_state_for(&current_payload);
        let retry = sample_bind_payload();
        let gate = validate_bind_request_against_state(&retry.supervisor.peer_id, &retry, &state)
            .expect("exact-match retry should be idempotent");
        assert!(matches!(gate, BindMemberGate::IdempotentAck));
    }

    #[test]
    fn validate_bind_request_against_state_rejects_protocol_version_mismatch_even_when_bound() {
        // A stale protocol version cannot coast on idempotent-ack — protocol
        // version is checked before state to surface clear incompatibility.
        let current_payload = sample_bind_payload();
        let state = authorized_state_for(&current_payload);
        let mut stale = sample_bind_payload();
        stale.protocol_version = SUPERVISOR_BRIDGE_PROTOCOL_VERSION + 1;
        let (cause, error) =
            validate_bind_request_against_state(&stale.supervisor.peer_id, &stale, &state)
                .expect_err("stale protocol version must be rejected");
        assert_eq!(cause, BridgeRejectionCause::UnsupportedProtocolVersion);
        assert!(
            error.contains("unsupported bridge protocol version"),
            "rejection should explain the protocol mismatch, got: {error}"
        );
    }

    #[tokio::test]
    async fn bind_member_handler_rejects_rebind_after_supervisor_bound() {
        // End-to-end handler test: drive BridgeCommand::BindMember through the
        // dispatch and assert that a different supervisor is NOT overwritten
        // into supervisor_state.
        let runtime: Arc<dyn CommsRuntime> = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("bind-rebind-receiver")
                .expect("receiver runtime"),
        );
        let supervisor_runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("mob/__mob_supervisor__")
                .expect("supervisor runtime"),
        );
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let current_supervisor = TrustedPeerSpec::new(
            "mob/__mob_supervisor__",
            supervisor_runtime.public_key().to_peer_id(),
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid supervisor spec");
        let mut supervisor_state = Some(AuthorizedSupervisorState {
            supervisor: current_supervisor.clone(),
            epoch: 1,
        });
        let adversary_supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:adversary".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                supervisor: adversary_supervisor,
                epoch: 2,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                expected_peer_id: runtime
                    .public_key()
                    .unwrap_or_else(|| "ed25519:receiver".to_string()),
                expected_address: runtime
                    .advertised_address()
                    .unwrap_or_else(|| "inproc://bind-rebind-receiver".to_string()),
                // Even with a *valid* bootstrap token, the rebind must fail.
                bootstrap_token: runtime.bridge_bootstrap_token().unwrap_or_default().into(),
            },
        );
        let candidate = bridge_candidate("ed25519:adversary", &command);

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &runtime,
                &candidate,
                &mut supervisor_state,
            )
            .await,
            "bridge handler must own the BindMember command"
        );
        let state = supervisor_state.expect("supervisor state must be preserved");
        assert_eq!(
            state.supervisor.peer_id, current_supervisor.peer_id,
            "rebind attempt must not replace the authorized supervisor"
        );
        assert_eq!(
            state.epoch, 1,
            "rebind attempt must not advance the authorized epoch"
        );
    }

    // -----------------------------------------------------------------------
    // 10. Strict gate production path: the BindMember rebind is rejected
    //     with a *typed* `AlreadyBound` cause on the wire, not just by
    //     state preservation. Complements
    //     `bind_member_handler_rejects_rebind_after_supervisor_bound`,
    //     which asserts the state-side invariant.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn bind_member_handler_rebind_reply_is_typed_already_bound() {
        let sent: Arc<tokio::sync::Mutex<Vec<CommsCommand>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn CommsRuntime> = Arc::new(CapturingRuntime {
            peer_id: "ed25519:receiver".to_string(),
            advertised_address: Some("inproc://receiver".to_string()),
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            sent: sent.clone(),
        });
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let current = TrustedPeerSpec::new(
            "mob/__mob_supervisor__",
            "ed25519:current-supervisor",
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid supervisor spec");
        let mut supervisor_state = Some(AuthorizedSupervisorState {
            supervisor: current.clone(),
            epoch: 1,
        });

        // A different supervisor tries to rebind. Under the strict gate this
        // must be rejected with typed `AlreadyBound` — the mob-side bridge
        // fallback logic branches on the typed cause, not on reason text.
        let adversary = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:different-supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                supervisor: adversary,
                epoch: 2,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                expected_peer_id: "ed25519:receiver".to_string(),
                expected_address: "inproc://receiver".to_string(),
                bootstrap_token: "expected-token".into(),
            },
        );
        let candidate = bridge_candidate("ed25519:different-supervisor", &command);

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &runtime,
                &candidate,
                &mut supervisor_state,
            )
            .await,
            "bridge handler must own the BindMember command"
        );

        // State is preserved (already covered elsewhere, pinned again here).
        let state = supervisor_state.expect("state preserved");
        assert_eq!(state.supervisor.peer_id, current.peer_id);

        // The reply on the wire carries a typed `AlreadyBound` cause.
        let (result, status) = sent
            .lock()
            .await
            .iter()
            .find_map(|cmd| match cmd {
                CommsCommand::PeerResponse { result, status, .. } => {
                    Some((result.clone(), *status))
                }
                _ => None,
            })
            .expect("handler must send a PeerResponse for the rejection");
        assert!(
            matches!(status, meerkat_core::interaction::ResponseStatus::Failed),
            "rebind rejection must surface as Failed status"
        );
        let reply: BridgeReply = serde_json::from_value(result).expect("typed bridge reply");
        match reply {
            BridgeReply::Rejected { cause, .. } => {
                assert_eq!(
                    cause,
                    BridgeRejectionCause::AlreadyBound,
                    "different-supervisor rebind must be rejected as AlreadyBound",
                );
            }
            other => unreachable!("expected Rejected reply, got {other:?}"),
        }
    }

    /// Capturing runtime that lets a test inspect every `CommsCommand` sent
    /// through the bridge handler. Also deliberately returns `None` for
    /// `advertised_address` so we can exercise the idempotent-ack invariant
    /// path without wiring a full inproc transport.
    struct CapturingRuntime {
        peer_id: String,
        advertised_address: Option<String>,
        bootstrap_token: Option<String>,
        inbox_notify: Arc<tokio::sync::Notify>,
        sent: Arc<tokio::sync::Mutex<Vec<CommsCommand>>>,
    }

    #[async_trait::async_trait]
    impl CommsRuntime for CapturingRuntime {
        fn public_key(&self) -> Option<String> {
            Some(self.peer_id.clone())
        }

        fn advertised_address(&self) -> Option<String> {
            self.advertised_address.clone()
        }

        fn bridge_bootstrap_token(&self) -> Option<String> {
            self.bootstrap_token.clone()
        }

        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            self.inbox_notify.clone()
        }

        async fn add_trusted_peer(&self, _peer: TrustedPeerSpec) -> Result<(), SendError> {
            Ok(())
        }

        async fn remove_trusted_peer(&self, _peer_id: &str) -> Result<bool, SendError> {
            Ok(true)
        }

        async fn send(
            &self,
            cmd: CommsCommand,
        ) -> Result<meerkat_core::comms::SendReceipt, SendError> {
            let receipt = match &cmd {
                CommsCommand::PeerResponse { in_reply_to, .. } => {
                    meerkat_core::comms::SendReceipt::PeerResponseSent {
                        envelope_id: Uuid::new_v4(),
                        in_reply_to: *in_reply_to,
                    }
                }
                _ => meerkat_core::comms::SendReceipt::PeerMessageSent {
                    envelope_id: Uuid::new_v4(),
                    acked: true,
                },
            };
            self.sent.lock().await.push(cmd);
            Ok(receipt)
        }
    }

    #[tokio::test]
    async fn idempotent_ack_invariant_rejects_without_echoing_attacker_fields() {
        // Invariant: when the strict BindMember gate hits IdempotentAck we MUST
        // reply with the runtime's canonical identity. If the runtime cannot
        // produce it (e.g. advertised_address disappeared under us), the
        // handler must NOT fall back to payload.expected_* — those are
        // attacker-controlled. Instead, reply with a typed Internal rejection
        // whose reason does not include any attacker input.
        let sent: Arc<tokio::sync::Mutex<Vec<CommsCommand>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let runtime: Arc<dyn CommsRuntime> = Arc::new(CapturingRuntime {
            peer_id: "ed25519:receiver".to_string(),
            // Simulate the invariant violation: no advertised address at the
            // moment of idempotent ack.
            advertised_address: None,
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            sent: sent.clone(),
        });
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let authorized = TrustedPeerSpec::new(
            "mob/__mob_supervisor__",
            "ed25519:supervisor",
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid supervisor spec");
        let mut supervisor_state = Some(AuthorizedSupervisorState {
            supervisor: authorized.clone(),
            epoch: 7,
        });
        // Attacker sets expected_address/expected_peer_id to unique tokens we
        // can grep for in the reply to prove they are NOT echoed back.
        let attacker_address = "inproc://ATTACKER-ADDRESS-DO-NOT-ECHO".to_string();
        let attacker_peer_id = "ed25519:ATTACKER-PEER-ID-DO-NOT-ECHO".to_string();
        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                // Sender/supervisor match stored state so validation returns
                // IdempotentAck; the invariant then fires because the runtime
                // cannot produce its own canonical identity.
                supervisor: BridgePeerSpec {
                    name: authorized.name.clone(),
                    peer_id: authorized.peer_id.clone(),
                    address: authorized.address.clone(),
                },
                epoch: 7,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                expected_peer_id: attacker_peer_id.clone(),
                expected_address: attacker_address.clone(),
                bootstrap_token: "expected-token".into(),
            },
        );
        let candidate = bridge_candidate(&authorized.peer_id, &command);

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &runtime,
                &candidate,
                &mut supervisor_state,
            )
            .await,
            "bridge handler must own the BindMember command"
        );

        // The runtime stored state must be preserved — an invariant-violation
        // reply must not mutate authority.
        let state = supervisor_state.expect("supervisor state must survive invariant failure");
        assert_eq!(state.supervisor.peer_id, authorized.peer_id);
        assert_eq!(state.epoch, 7);

        // Pull the PeerResponse and assert its shape + that attacker fields
        // are NOT echoed anywhere in the reply.
        let sent_commands = sent.lock().await.clone();
        let (result, status) = sent_commands
            .into_iter()
            .find_map(|cmd| match cmd {
                CommsCommand::PeerResponse { result, status, .. } => Some((result, status)),
                _ => None,
            })
            .expect("handler must send a PeerResponse for the invariant violation");
        assert!(
            matches!(status, meerkat_core::interaction::ResponseStatus::Failed),
            "invariant violation must surface as Failed status"
        );
        let reply: BridgeReply =
            serde_json::from_value(result.clone()).expect("typed bridge reply");
        assert!(
            matches!(reply, BridgeReply::Rejected { .. }),
            "expected Rejected reply for invariant violation, got: {reply:?}"
        );
        if let BridgeReply::Rejected { cause, reason } = reply {
            assert_eq!(cause, BridgeRejectionCause::Internal);
            assert!(
                !reason.contains(&attacker_address),
                "rejection reason must not echo attacker-supplied address: {reason}"
            );
            assert!(
                !reason.contains(&attacker_peer_id),
                "rejection reason must not echo attacker-supplied peer_id: {reason}"
            );
        }
        let raw = serde_json::to_string(&result).expect("serialize reply for attacker-field check");
        assert!(
            !raw.contains(&attacker_address),
            "reply payload must not contain attacker-supplied address: {raw}"
        );
        assert!(
            !raw.contains(&attacker_peer_id),
            "reply payload must not contain attacker-supplied peer_id: {raw}"
        );
    }

    #[test]
    fn validate_authorize_supervisor_rejects_initial_claim_without_bind() {
        let payload = BridgeSupervisorPayload {
            supervisor: BridgePeerSpec {
                name: "mob/__mob_supervisor__".to_string(),
                peer_id: "ed25519:supervisor".to_string(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
            },
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        };

        let (cause, error) =
            validate_authorize_supervisor_request(&payload.supervisor.peer_id, &payload, &None)
                .expect_err("first supervisor claim must go through bind_member");
        assert_eq!(cause, BridgeRejectionCause::NotBound);
        assert!(
            error.contains("bind_member"),
            "initial authorize rejection should direct callers to bind_member, got: {error}"
        );
    }

    #[test]
    fn validate_authorize_supervisor_rejects_protocol_version_mismatch() {
        let payload = BridgeSupervisorPayload {
            supervisor: BridgePeerSpec {
                name: "mob/__mob_supervisor__".to_string(),
                peer_id: "ed25519:supervisor".to_string(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
            },
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION + 1,
        };

        let (cause, error) =
            validate_authorize_supervisor_request(&payload.supervisor.peer_id, &payload, &None)
                .expect_err("authorize should reject unsupported protocol versions");
        assert_eq!(cause, BridgeRejectionCause::UnsupportedProtocolVersion);
        assert!(
            error.contains("unsupported bridge protocol version"),
            "authorize rejection should explain the protocol mismatch, got: {error}"
        );
    }

    // -----------------------------------------------------------------------
    // 18. Empty bootstrap token must be rejected with a typed cause.
    // -----------------------------------------------------------------------
    //
    // `advertised_bind_bootstrap_token` is the single point where the
    // runtime asserts the bootstrap token is non-empty. An empty token
    // would make the initial bind handshake unverifiable, so pin the
    // typed rejection.

    #[test]
    fn validate_bind_request_rejects_empty_bootstrap_token_at_runtime() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(BootstrapRuntime {
            peer_id: "ed25519:receiver".to_string(),
            address: "inproc://receiver".to_string(),
            bootstrap_token: Some(String::new()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            remove_trusted_peer_error: None,
        });
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: "ed25519:receiver".to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "whatever".into(),
        };

        let (cause, _error) = validate_bind_request(&runtime, &supervisor.peer_id, &payload)
            .expect_err("runtime with empty token must refuse to validate");
        assert_eq!(cause, BridgeRejectionCause::InvalidBootstrapToken);
    }

    // -----------------------------------------------------------------------
    // 19. Protocol v1 downgrade: every command's handler must reject a v1
    //     payload at the version check, never proceed with defaulted fields.
    // -----------------------------------------------------------------------
    //
    // `validate_bind_request_rejects_explicit_v1_protocol_under_v2` already
    // covers the bind path. Extend the contract to AuthorizeSupervisor
    // (gating rotation) and the broader require_authorized_supervisor flow
    // that backs revoke/observe/interrupt/retire/destroy/deliver and
    // wire/unwire.

    #[test]
    fn validate_authorize_supervisor_rejects_explicit_v1_protocol_under_v2() {
        let payload = BridgeSupervisorPayload {
            supervisor: BridgePeerSpec {
                name: "mob/__mob_supervisor__".to_string(),
                peer_id: "ed25519:supervisor".to_string(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
            },
            epoch: 0,
            protocol_version: 1,
        };
        let (cause, _error) =
            validate_authorize_supervisor_request(&payload.supervisor.peer_id, &payload, &None)
                .expect_err("v1 authorize must be rejected under v2+");
        assert_eq!(cause, BridgeRejectionCause::UnsupportedProtocolVersion);
    }

    #[test]
    fn require_authorized_supervisor_rejects_explicit_v1_protocol_under_v2() {
        // Back-stops every command that calls `require_authorized_supervisor`
        // (revoke/observe/interrupt/retire/destroy/deliver/wire/unwire). A v1
        // payload must not coast on idempotent-ack or sender-match.
        let supervisor = TrustedPeerSpec::new(
            "mob/__mob_supervisor__",
            "ed25519:supervisor",
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid supervisor spec");
        let state = Some(AuthorizedSupervisorState {
            supervisor: supervisor.clone(),
            epoch: 3,
        });
        let payload = BridgeSupervisorPayload {
            supervisor: BridgePeerSpec {
                name: supervisor.name.clone(),
                peer_id: supervisor.peer_id.clone(),
                address: supervisor.address.clone(),
            },
            epoch: 3,
            protocol_version: 1,
        };
        let (cause, _error) = require_authorized_supervisor(&supervisor.peer_id, &payload, &state)
            .expect_err("v1 authorized-supervisor payload must be rejected");
        assert_eq!(cause, BridgeRejectionCause::UnsupportedProtocolVersion);
    }

    fn bridge_candidate(sender: &str, command: &BridgeCommand) -> PeerInputCandidate {
        PeerInputCandidate {
            interaction: InboxInteraction {
                id: InteractionId(Uuid::new_v4()),
                from: sender.to_string(),
                content: InteractionContent::Request {
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                    params: serde_json::to_value(command).expect("serialize bridge command"),
                },
                rendered_text: String::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
            },
            class: PeerInputClass::ActionableRequest,
            lifecycle_peer: None,
        }
    }

    #[tokio::test]
    async fn revoke_supervisor_keeps_authority_when_trust_removal_fails() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(BootstrapRuntime {
            peer_id: "ed25519:receiver".to_string(),
            address: "inproc://receiver".to_string(),
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            remove_trusted_peer_error: Some("boom".to_string()),
        });
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
        };
        let payload = BridgeSupervisorPayload {
            supervisor: supervisor.clone(),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        };
        let candidate = bridge_candidate(
            &supervisor.peer_id,
            &BridgeCommand::RevokeSupervisor(payload.clone()),
        );
        let mut supervisor_state = Some(AuthorizedSupervisorState {
            supervisor: TrustedPeerSpec::try_from(supervisor).expect("valid supervisor spec"),
            epoch: payload.epoch,
        });

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &runtime,
                &candidate,
                &mut supervisor_state,
            )
            .await,
            "revoke command should be handled"
        );
        assert!(
            supervisor_state.is_some(),
            "failed revoke must preserve supervisor authority until trust removal succeeds"
        );
    }

    #[tokio::test]
    async fn wire_member_rejects_invalid_peer_spec_before_trusting_it() {
        let runtime = Arc::new(meerkat_comms::CommsRuntime::inproc_only("receiver-wire").unwrap());
        let supervisor_runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("mob/__mob_supervisor__").unwrap());
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let supervisor = TrustedPeerSpec::new(
            "mob/__mob_supervisor__",
            supervisor_runtime.public_key().to_peer_id(),
            "inproc://mob/__mob_supervisor__",
        )
        .unwrap();
        runtime
            .add_trusted_peer(supervisor.clone())
            .await
            .expect("trust supervisor");
        let mut supervisor_state = Some(AuthorizedSupervisorState {
            supervisor: supervisor.clone(),
            epoch: 1,
        });
        let candidate = bridge_candidate(
            &supervisor.peer_id,
            &BridgeCommand::WireMember(BridgePeerWiringPayload {
                supervisor: supervisor.clone().into(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                peer_spec: BridgePeerSpec {
                    name: "".to_string(),
                    peer_id: "ed25519:peer".to_string(),
                    address: "inproc://peer".to_string(),
                },
            }),
        );

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
                &mut supervisor_state,
            )
            .await,
            "wire bridge command should be handled"
        );
        let peers = runtime.peers().await;
        assert!(
            peers.iter().all(|entry| !entry.name.as_str().is_empty()),
            "invalid wire peer specs must not be materialized in comms trust"
        );
    }
}

/// Bridge between a completion handle and the comms interaction lifecycle.
fn spawn_completion_bridge(
    comms_runtime: Option<Arc<dyn CommsRuntime>>,
    interaction_id: meerkat_core::interaction::InteractionId,
    subscriber: Option<mpsc::Sender<AgentEvent>>,
    handle: Option<crate::completion::CompletionHandle>,
) {
    crate::tokio::spawn(async move {
        let outcome = match handle {
            Some(handle) => handle.wait().await,
            None => CompletionOutcome::CompletedWithoutResult,
        };

        if let Some(tx) = subscriber {
            let event = interaction_terminal_event(interaction_id, outcome);

            if crate::tokio::time::timeout(std::time::Duration::from_secs(5), tx.send(event))
                .await
                .is_err()
            {
                tracing::warn!(
                    %interaction_id,
                    "completion bridge dropped terminal event: subscriber send timed out after 5s"
                );
            }
        }

        if let Some(runtime) = comms_runtime {
            runtime.mark_interaction_complete(&interaction_id);
        }
    });
}
