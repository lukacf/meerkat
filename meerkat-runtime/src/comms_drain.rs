//! Comms inbox drain task.
//!
//! Standalone tokio task that drains `CommsRuntime` inbox and feeds typed
//! `Input` values into `MeerkatMachine`. Replaces the old
//! `RuntimeCommsBridge` (comms_sink.rs) which implemented the now-removed
//! `RuntimeInputSink` trait on `meerkat-core`.

#![allow(clippy::large_futures)]

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::agent::CommsRuntime;
#[allow(unused_imports)]
use meerkat_core::comms::{CommsCommand, PeerId, PeerName, PeerRoute, TrustedPeerDescriptor};
use meerkat_core::event::AgentEvent;
use meerkat_core::interaction::{InteractionContent, PeerInputCandidate, PeerInputClass};
use meerkat_core::lifecycle::RunControlCommand;
use meerkat_core::types::SessionId;

use meerkat_contracts::wire::supervisor_bridge::{
    BridgeAck, BridgeBindResponse, BridgeCapabilities, BridgeCommand, BridgeDeliveryOutcome,
    BridgeDeliveryPayload, BridgeDeliveryRejectionCause, BridgeDeliveryResponse,
    BridgeDestroyResponse, BridgeMemberRuntimeState, BridgeObservationResponse,
    BridgePeerConnectivity, BridgePeerSpec, BridgeRejectionCause, BridgeReply,
    BridgeRetireResponse, BridgeSupervisorPayload, SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM,
    SUPERVISOR_BRIDGE_INTENT, SUPERVISOR_BRIDGE_PROTOCOL_VERSION, canonicalize_bridge_address,
};

use crate::comms_bridge::classified_interaction_to_runtime_input;
use crate::completion::CompletionOutcome;
use crate::identifiers::IdempotencyKey;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
};
use crate::meerkat_machine::{
    DrainExitReason, MeerkatMachine, SupervisorBinding, SupervisorBindingStageError,
};
use crate::service_ext::SessionServiceRuntimeExt as _;
use crate::tokio::sync::mpsc;
use crate::traits::RuntimeControlPlane;

/// Default idle timeout for session-backed comms drains.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

const ED25519_PUBKEY_PREFIX: &str = "ed25519:";
const PEER_ID_UUID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d65_6572_6b61_7450_6565_7249_6430_0001);

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
        if std::env::var_os("RKAT_TRACE_COMMS_DRAIN_BIND").is_some() {
            tracing::info!(
                %session_id,
                comms_ptr = ?Arc::as_ptr(&comms_runtime),
                "comms_drain task started"
            );
        }
        let inbox_notify = comms_runtime.inbox_notify();
        // Supervisor authorization is DSL-owned (Wave 3 D Row 21).
        // Each bridge-command dispatch reads the binding via
        // `adapter.supervisor_binding(session_id)` and stages DSL
        // transitions; no shell-local mirror.

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
                        // Silent mob lifecycle notices are control-plane
                        // mechanics, not model-facing work. The canonical
                        // topology truth already lives in MobMachine + the
                        // comms trust directory; replaying these as prompt
                        // text creates shadow semantics and breaks instruction
                        // following in mixed-provider mobs.
                        tracing::debug!(
                            session_id = %session_id,
                            class = ?candidate.class,
                            lifecycle_peer = ?candidate.lifecycle_peer,
                            "comms_drain: consumed silent peer lifecycle notice"
                        );
                    }
                    PeerInputClass::Response => {
                        // Distinguish progress responses from terminal responses
                        // via the canonical classifier in meerkat-core. Raw
                        // `ResponseStatus` matching here is forbidden — all
                        // consumers must read `TerminalityClass` so "terminal"
                        // stays lock-stepped across the codebase.
                        //
                        // W1-A also needs to route terminal status → the DSL
                        // peer-interaction handle's `PeerTerminalDisposition`.
                        // Compute both from the single classifier call so the
                        // "terminal" and "disposition" facts never drift.
                        let terminal_status = match &candidate.interaction.content {
                            meerkat_core::interaction::InteractionContent::Response {
                                status,
                                ..
                            } => match meerkat_core::interaction::classify_response_terminality(
                                *status,
                            ) {
                                meerkat_core::interaction::TerminalityClass::Terminal {
                                    disposition,
                                } => match disposition {
                                    meerkat_core::interaction::TerminalDisposition::Completed => {
                                        Some(meerkat_core::handles::PeerTerminalDisposition::Completed)
                                    }
                                    meerkat_core::interaction::TerminalDisposition::Failed => {
                                        Some(meerkat_core::handles::PeerTerminalDisposition::Failed)
                                    }
                                    _ => None,
                                },
                                meerkat_core::interaction::TerminalityClass::Progress => None,
                                _ => None,
                            },
                            _ => None,
                        };
                        let is_terminal = terminal_status.is_some();

                        if is_terminal {
                            // Terminal response — single admission with
                            // completion tracking. The PeerInput already
                            // carries ResponseTerminal convention which the
                            // policy table maps to WakeIfIdle/Steer as needed.
                            // No synthetic Continuation required.
                            let interaction_id = match &candidate.interaction.content {
                                meerkat_core::interaction::InteractionContent::Response {
                                    in_reply_to,
                                    ..
                                } => *in_reply_to,
                                other => {
                                    tracing::warn!(
                                        content = ?other,
                                        "comms_drain: terminal response candidate missing response content"
                                    );
                                    continue;
                                }
                            };

                            // W1-A ordering: pull the subscriber FIRST (it
                            // is the one-shot the completion bridge hands
                            // the terminal event to), THEN fire the DSL
                            // terminal transition. The transition emits
                            // `PeerInteractionCleanup`; the observer drops
                            // the now-idle stream registry entry. If the
                            // DSL is not installed (standalone / WASM),
                            // `mark_interaction_complete` below handles
                            // cleanup; when the DSL IS installed, the
                            // effect drives cleanup and `mark_interaction_complete`
                            // becomes a no-op.
                            let subscriber = comms_runtime.interaction_subscriber(&interaction_id);
                            let dsl_installed = comms_runtime.peer_interaction_handle().is_some();
                            if let (Some(handle), Some(disposition)) =
                                (comms_runtime.peer_interaction_handle(), terminal_status)
                            {
                                let corr_id =
                                    meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0);
                                if let Err(err) = handle.response_terminal(corr_id, disposition) {
                                    tracing::warn!(
                                        error = %err,
                                        corr_id = %corr_id,
                                        "PeerInteractionHandle::response_terminal rejected (no DSL entry — classified drain saw an unknown corr_id)"
                                    );
                                }
                            }

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
                                    } else if !dsl_installed {
                                        comms_runtime.mark_interaction_complete(&interaction_id);
                                    }
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        "comms_drain: failed to inject terminal response"
                                    );
                                    if !dsl_installed {
                                        comms_runtime.mark_interaction_complete(&interaction_id);
                                    }
                                }
                            }
                        } else {
                            // Progress response — route as peer input for checkpoint-style handling.
                            // W1-A: also record the progress signal in the
                            // DSL so downstream reads see state transition
                            // `Sent → AcceptedProgress`.
                            if let Some(handle) = comms_runtime.peer_interaction_handle()
                                && let meerkat_core::interaction::InteractionContent::Response {
                                    in_reply_to,
                                    ..
                                } = &candidate.interaction.content
                            {
                                let corr_id =
                                    meerkat_core::PeerCorrelationId::from_uuid(in_reply_to.0);
                                if let Err(err) = handle.response_progress(corr_id) {
                                    tracing::warn!(
                                        error = %err,
                                        corr_id = %corr_id,
                                        "PeerInteractionHandle::response_progress rejected"
                                    );
                                }
                            }

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

                        // W1-A: inbound peer-request admission is the
                        // canonical fire site for `PeerRequestReceived`.
                        // Classes that correspond to actual peer requests
                        // (SilentRequest, ActionableRequest) populate the
                        // DSL's `inbound_peer_requests` map; the
                        // `CommsRuntime::send(PeerResponse)` path closes the
                        // lifecycle with `PeerResponseReplied` on terminal
                        // reply. Other classes (messages, lifecycle, plain
                        // events) are not requests and skip the fire.
                        let is_inbound_peer_request = matches!(
                            candidate.class,
                            PeerInputClass::SilentRequest | PeerInputClass::ActionableRequest
                        );
                        if is_inbound_peer_request
                            && let Some(handle) = comms_runtime.peer_interaction_handle()
                        {
                            let corr_id =
                                meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0);
                            if let Err(err) = handle.request_received(corr_id) {
                                tracing::warn!(
                                    error = %err,
                                    corr_id = %corr_id,
                                    "PeerInteractionHandle::request_received rejected"
                                );
                            }
                        }

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

fn sender_matches_bound_supervisor(sender: &str, name: &str, peer_id: &str) -> bool {
    sender == name || sender == peer_id
}

fn sender_matches_bridge_peer(sender: &str, peer: &BridgePeerSpec) -> bool {
    sender == peer.name || sender == peer.peer_id
}

/// Require the caller to be the currently authorized supervisor for the
/// session. Shared validation gate for all post-bind bridge commands
/// (`AuthorizeSupervisor` / `RevokeSupervisor` / `DeliverMemberInput` /
/// other member-control commands).
///
/// The current binding snapshot is read from DSL state by the caller and
/// passed in; the validator itself is pure so it remains test-friendly.
fn require_authorized_supervisor(
    sender: &str,
    payload: &BridgeSupervisorPayload,
    current: &SupervisorBinding,
) -> Result<(), (BridgeRejectionCause, String)> {
    if payload.protocol_version != SUPERVISOR_BRIDGE_PROTOCOL_VERSION {
        return Err((
            BridgeRejectionCause::UnsupportedProtocolVersion,
            format!(
                "unsupported bridge protocol version {} (expected {})",
                payload.protocol_version, SUPERVISOR_BRIDGE_PROTOCOL_VERSION
            ),
        ));
    }
    let SupervisorBinding::Bound {
        name: current_name,
        peer_id: current_peer_id,
        epoch: current_epoch,
        ..
    } = current
    else {
        return Err((
            BridgeRejectionCause::NotBound,
            "no authorized supervisor registered".to_string(),
        ));
    };
    if payload.epoch < *current_epoch {
        return Err((
            BridgeRejectionCause::StaleSupervisor,
            format!(
                "stale supervisor epoch {} (current {})",
                payload.epoch, current_epoch
            ),
        ));
    }
    if payload.epoch != *current_epoch {
        return Err((
            BridgeRejectionCause::StaleSupervisor,
            format!(
                "unexpected supervisor epoch {} (current {})",
                payload.epoch, current_epoch
            ),
        ));
    }
    if payload.supervisor.peer_id != *current_peer_id {
        return Err((
            BridgeRejectionCause::StaleSupervisor,
            format!(
                "stale supervisor peer '{}' (current '{current_peer_id}')",
                payload.supervisor.peer_id
            ),
        ));
    }
    if !sender_matches_bound_supervisor(sender, current_name, current_peer_id) {
        return Err((
            BridgeRejectionCause::SenderMismatch,
            format!(
                "request sender '{sender}' does not match authorized supervisor '{current_peer_id}'"
            ),
        ));
    }
    Ok(())
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

fn bridge_delivery_rejection_cause(
    reason: &crate::accept::RejectReason,
) -> BridgeDeliveryRejectionCause {
    match reason {
        crate::accept::RejectReason::NotReady { state } => BridgeDeliveryRejectionCause::NotReady {
            state: state.clone(),
        },
        crate::accept::RejectReason::DurabilityViolation { detail } => {
            BridgeDeliveryRejectionCause::DurabilityViolation {
                detail: detail.clone(),
            }
        }
        crate::accept::RejectReason::PeerHandlingModeInvalid { detail } => {
            BridgeDeliveryRejectionCause::PeerHandlingModeInvalid {
                detail: detail.clone(),
            }
        }
    }
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
) -> Result<(TrustedPeerDescriptor, String), (BridgeRejectionCause, String)> {
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
    // Canonical identity must be present to validate the request's
    // expected_peer_id. Missing runtime identity is a typed internal
    // invariant failure, not a silent "skip the check" path — otherwise
    // caller-supplied `expected_peer_id` would never be challenged.
    let Some(actual_peer_id) = comms_runtime.public_key() else {
        return Err((
            BridgeRejectionCause::Internal,
            "bind member failed: runtime public key unavailable".to_string(),
        ));
    };
    if actual_peer_id != payload.expected_peer_id {
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
    let supervisor =
        TrustedPeerDescriptor::try_from(payload.supervisor.clone()).map_err(|error| {
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

#[derive(Clone, Debug)]
struct BoundSupervisorState {
    name: String,
    peer_id: String,
    address: String,
    epoch: u64,
}

impl BoundSupervisorState {
    fn from_binding(binding: &SupervisorBinding) -> Option<Self> {
        let SupervisorBinding::Bound {
            name,
            peer_id,
            address,
            epoch,
        } = binding
        else {
            return None;
        };
        Some(Self {
            name: name.clone(),
            peer_id: peer_id.clone(),
            address: address.clone(),
            epoch: *epoch,
        })
    }
}

async fn rollback_bind_after_trust_publication_failure(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    peer_id: &str,
    epoch: u64,
) -> Result<(), SupervisorBindingStageError> {
    adapter
        .stage_supervisor_revoke(session_id, peer_id.to_string(), epoch)
        .await
}

async fn rollback_authorize_after_trust_publication_failure(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    previous: &BoundSupervisorState,
) -> Result<(), SupervisorBindingStageError> {
    adapter
        .stage_supervisor_authorize(
            session_id,
            previous.name.clone(),
            previous.peer_id.clone(),
            previous.address.clone(),
            previous.epoch,
        )
        .await
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
    current: &SupervisorBinding,
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
    let SupervisorBinding::Bound {
        name: current_name,
        peer_id: current_peer_id,
        epoch: current_epoch,
        ..
    } = current
    else {
        return Ok(BindMemberGate::Bootstrap);
    };
    if payload.supervisor.peer_id != *current_peer_id {
        return Err((
            BridgeRejectionCause::AlreadyBound,
            format!(
                "bind member failed: supervisor already bound as '{current_peer_id}'; use authorize_supervisor to rotate"
            ),
        ));
    }
    if payload.epoch != *current_epoch {
        return Err((
            BridgeRejectionCause::AlreadyBound,
            format!(
                "bind member failed: epoch {} does not match bound supervisor epoch {current_epoch}; use authorize_supervisor to rotate",
                payload.epoch
            ),
        ));
    }
    if !sender_matches_bound_supervisor(sender, current_name, current_peer_id) {
        return Err((
            BridgeRejectionCause::SenderMismatch,
            format!(
                "bind member failed: request sender '{sender}' does not match authorized supervisor '{current_peer_id}'"
            ),
        ));
    }
    Ok(BindMemberGate::IdempotentAck)
}

fn validate_authorize_supervisor_request(
    sender: &str,
    payload: &BridgeSupervisorPayload,
    current: &SupervisorBinding,
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
    if let SupervisorBinding::Bound {
        name: current_name,
        peer_id: current_peer_id,
        epoch: current_epoch,
        ..
    } = current
    {
        if payload.epoch < *current_epoch {
            return Err((
                BridgeRejectionCause::StaleSupervisor,
                format!(
                    "authorize supervisor failed: stale supervisor epoch {} (current {current_epoch})",
                    payload.epoch
                ),
            ));
        }
        if !sender_matches_bound_supervisor(sender, current_name, current_peer_id) {
            return Err((
                BridgeRejectionCause::SenderMismatch,
                format!(
                    "authorize supervisor failed: request sender '{sender}' does not match authorized supervisor '{current_peer_id}'"
                ),
            ));
        }
        if payload.epoch == *current_epoch && payload.supervisor.peer_id == *current_peer_id {
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
    let to = match resolve_peer_route(comms_runtime, &candidate.interaction.from).await {
        Some(route) => route,
        None => {
            tracing::warn!(
                from = %candidate.interaction.from,
                interaction_id = %candidate.interaction.id,
                "comms_drain: failed to resolve bridge response peer route"
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

async fn resolve_peer_route(
    comms_runtime: &Arc<dyn CommsRuntime>,
    from: &str,
) -> Option<PeerRoute> {
    let peers = comms_runtime.peers().await;
    peers
        .iter()
        .find(|entry| entry.peer_id.as_str() == from)
        .or_else(|| peers.iter().find(|entry| entry.name.as_str() == from))
        .map(|entry| PeerRoute::with_display_name(entry.peer_id, entry.name.clone()))
        .or_else(|| peer_route_from_pubkey_string(from))
        .or_else(|| {
            meerkat_core::comms::PeerId::parse(from)
                .ok()
                .map(PeerRoute::new)
        })
        .or_else(|| {
            PeerName::new(from.to_string())
                .ok()
                .map(|name| PeerRoute::with_display_name(meerkat_core::comms::PeerId::new(), name))
        })
}

fn peer_route_from_pubkey_string(from: &str) -> Option<PeerRoute> {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    let encoded = from.strip_prefix(ED25519_PUBKEY_PREFIX)?;
    let decoded = BASE64.decode(encoded).ok()?;
    if decoded.len() != 32 {
        return None;
    }
    Some(PeerRoute::new(PeerId::from_uuid(uuid::Uuid::new_v5(
        &PEER_ID_UUID_NAMESPACE,
        &decoded,
    ))))
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
///
/// Wave 3 D Row 21: the authorized-supervisor binding lives in DSL state,
/// not in a helper-local variable. Each dispatch reads the current
/// binding via `adapter.supervisor_binding(session_id)` and stages DSL
/// transitions through `adapter.stage_supervisor_{bind,authorize,revoke}`.
async fn try_handle_supervisor_bridge_command(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
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

    // Snapshot the DSL-owned supervisor binding for this dispatch.
    // Every bridge command reads through this snapshot; within a single
    // drain task the dispatch is sequential so no read-modify-write race
    // is possible. The snapshot is re-read after any `stage_supervisor_*`
    // call when we need to report the new binding.
    let current_binding = adapter.supervisor_binding(session_id).await;

    match command {
        BridgeCommand::BindMember(payload) => {
            // Reject rebind attempts once a supervisor is already bound;
            // only exact-match retries from the authorized sender get an
            // idempotent ack. This prevents any holder of the bootstrap
            // token from seizing authority without going through
            // AuthorizeSupervisor/rotation (which requires the current
            // supervisor to sign).
            let gate = match validate_bind_request_against_state(sender, &payload, &current_binding)
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
            // Canonical identity only: the BindMember reply echoes
            // the runtime's own public key, never `payload.expected_peer_id`.
            // A caller-supplied identity can't cross the canonical
            // boundary — if the runtime cannot produce its own
            // identity, that is a typed internal invariant failure,
            // not a silent fallback.
            let Some(peer_id) = comms_runtime.public_key() else {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    "bind member failed: runtime public key unavailable",
                )
                .await;
                return true;
            };
            // DSL owns the authorization discriminant + identity + epoch.
            // `validate_bind_request_against_state` already confirmed the
            // binding is `Unbound`, so this stage should never be
            // rejected; if the DSL rejects anyway, treat as internal.
            if let Err(error) = adapter
                .stage_supervisor_bind(
                    session_id,
                    supervisor_spec.name.as_str().to_owned(),
                    supervisor_spec.peer_id.as_str(),
                    advertised_address.clone(),
                    payload.epoch,
                )
                // Wave-c C-6r V5: typed `PeerName` / `PeerId` carry the
                // trust-edge identity across the core seam. The `stage_*`
                // helpers still consume `String` on their way into the
                // DSL input payload (the DSL schema is stringly for
                // supervisor identity today); typed → string conversion
                // happens at this single callsite so the typed atoms
                // are preserved everywhere above the DSL boundary.
                .await
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("bind member failed: DSL rejected binding: {error}"),
                )
                .await;
                return true;
            }
            if let Err(error) = comms_runtime
                .add_trusted_peer(supervisor_spec.clone())
                .await
            {
                let peer_id_str = supervisor_spec.peer_id.as_str();
                let reason = match rollback_bind_after_trust_publication_failure(
                    adapter,
                    session_id,
                    &peer_id_str,
                    payload.epoch,
                )
                .await
                {
                    Ok(()) => format!(
                        "bind member failed: trust publication failed after DSL commit: {error}"
                    ),
                    Err(rollback_error) => format!(
                        "bind member failed: trust publication failed after DSL commit: {error}; rollback failed: {rollback_error}"
                    ),
                };
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    reason,
                )
                .await;
                return true;
            }
            // Wave-d D-d: close the `supervisor_trust_publish` obligation
            // on the DSL side with the epoch observed on the producer
            // effect. The DSL guard enforces `epoch == supervisor_bound_epoch`
            // so a stale ack for a superseded epoch is rejected without
            // mutating state. Rejection here is non-fatal: the rollback
            // path above already returned if trust publication failed,
            // so a guard failure would indicate the binding has rotated
            // between the `stage_supervisor_bind` commit and this ack
            // staging — which means the ack is stale and correctly
            // dropped without closing the (now-superseded) obligation.
            if let Err(error) = adapter
                .stage_supervisor_trust_published(
                    session_id,
                    supervisor_spec.peer_id.as_str(),
                    payload.epoch,
                )
                .await
            {
                tracing::debug!(
                    %session_id,
                    epoch = payload.epoch,
                    %error,
                    "supervisor_trust_publish ack rejected by DSL (binding rotated?)"
                );
            }
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::BindMember(BridgeBindResponse {
                    peer_id,
                    address: canonicalize_bridge_address(&advertised_address),
                    capabilities: bridge_capabilities(),
                }),
            )
            .await;
            true
        }
        BridgeCommand::AuthorizeSupervisor(payload) => {
            match validate_authorize_supervisor_request(sender, &payload, &current_binding) {
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

            // Reached `Proceed`, so `validate_authorize_supervisor_request`
            // already confirmed `Bound`; extract the old identity for
            // rollback-safe trust publication if the post-commit runtime
            // trust changes fail.
            let Some(previous_binding) = BoundSupervisorState::from_binding(&current_binding)
            else {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    "authorize supervisor failed: missing current binding",
                )
                .await;
                return true;
            };
            let supervisor_spec = match TrustedPeerDescriptor::try_from(payload.supervisor.clone())
            {
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
            if let Err(error) = adapter
                .stage_supervisor_authorize(
                    session_id,
                    supervisor_spec.name.as_str().to_owned(),
                    supervisor_spec.peer_id.as_str(),
                    supervisor_spec.address.to_string(),
                    payload.epoch,
                )
                .await
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("authorize supervisor failed: DSL rejected rotation: {error}"),
                )
                .await;
                return true;
            }
            if let Err(error) = comms_runtime
                .add_trusted_peer(supervisor_spec.clone())
                .await
            {
                let reason = match rollback_authorize_after_trust_publication_failure(
                    adapter,
                    session_id,
                    &previous_binding,
                )
                .await
                {
                    Ok(()) => format!(
                        "authorize supervisor failed: trust publication failed after DSL commit: {error}"
                    ),
                    Err(rollback_error) => format!(
                        "authorize supervisor failed: trust publication failed after DSL commit: {error}; rollback failed: {rollback_error}"
                    ),
                };
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    reason,
                )
                .await;
                return true;
            }
            // Wave-d D-d: close the `supervisor_trust_publish` obligation
            // for the rotated binding. `AuthorizeSupervisor` above has
            // already committed the DSL rotation to the new epoch, so
            // the guard matches `(new_peer_id, new_epoch)` and the ack
            // closes the obligation for the current binding. A stale
            // ack (observed from a superseded rotation) would have a
            // lower epoch and be rejected.
            if let Err(error) = adapter
                .stage_supervisor_trust_published(
                    session_id,
                    supervisor_spec.peer_id.as_str(),
                    payload.epoch,
                )
                .await
            {
                tracing::debug!(
                    %session_id,
                    epoch = payload.epoch,
                    %error,
                    "supervisor_trust_publish ack rejected by DSL (binding rotated?)"
                );
            }
            if previous_binding.peer_id != payload.supervisor.peer_id
                && let Err(error) = comms_runtime
                    .remove_trusted_peer(&previous_binding.peer_id)
                    .await
            {
                let rollback_result = rollback_authorize_after_trust_publication_failure(
                    adapter,
                    session_id,
                    &previous_binding,
                )
                .await;
                let supervisor_peer_id_str = supervisor_spec.peer_id.as_str();
                let cleanup_result = comms_runtime
                    .remove_trusted_peer(&supervisor_peer_id_str)
                    .await;
                let mut reason = format!(
                    "authorize supervisor failed: previous supervisor trust removal failed after DSL commit: {error}"
                );
                if let Err(rollback_error) = rollback_result {
                    reason.push_str(&format!("; rollback failed: {rollback_error}"));
                }
                if let Err(cleanup_error) = cleanup_result {
                    reason.push_str(&format!(
                        "; cleanup failed while removing new supervisor trust: {cleanup_error}"
                    ));
                }
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    reason,
                )
                .await;
                return true;
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
                require_authorized_supervisor(sender, &payload, &current_binding)
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
            // Wave-d D-d: close the `supervisor_trust_revoke` obligation
            // with the epoch observed on the producer effect. Staged
            // before the `RevokeSupervisor` transition flips the binding
            // to `Unbound` — the DSL guard matches against the still-
            // `Bound` binding. A stale revoke ack (epoch mismatch) would
            // be rejected here, leaving the obligation open and the
            // subsequent `stage_supervisor_revoke` below also rejecting
            // (its guards are identical modulo the unbound-transition).
            if let Err(error) = adapter
                .stage_supervisor_trust_revoked(
                    session_id,
                    payload.supervisor.peer_id.clone(),
                    payload.epoch,
                )
                .await
            {
                tracing::debug!(
                    %session_id,
                    epoch = payload.epoch,
                    %error,
                    "supervisor_trust_revoke ack rejected by DSL (binding rotated?)"
                );
            }
            if let Err(error) = adapter
                .stage_supervisor_revoke(
                    session_id,
                    payload.supervisor.peer_id.clone(),
                    payload.epoch,
                )
                .await
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("revoke supervisor failed: DSL rejected revoke: {error}"),
                )
                .await;
                return true;
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
        BridgeCommand::DeliverMemberInput(payload) => {
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) =
                require_authorized_supervisor(sender, &sup_payload, &current_binding)
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
                            let cause = bridge_delivery_rejection_cause(&reason);
                            BridgeDeliveryResponse {
                                input_id: request_input_id,
                                canonical_input_id: None,
                                outcome: BridgeDeliveryOutcome::Rejected {
                                    cause,
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
                require_authorized_supervisor(sender, &payload, &current_binding)
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
                require_authorized_supervisor(sender, &payload, &current_binding)
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
                require_authorized_supervisor(sender, &payload, &current_binding)
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
                require_authorized_supervisor(sender, &payload, &current_binding)
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
                require_authorized_supervisor(sender, &sup_payload, &current_binding)
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            // D-track-b: route WireMember through the DSL authority's
            // peer-projection state. The stager helper applies
            // `AddDirectPeerEndpoint`, emits
            // `CommsTrustReconcileRequested`, and drives the session's
            // `CommsTrustReconciler` to register the peer against the
            // `CommsRuntime`'s trust store — no direct `add_trusted_peer`
            // call here (closes the emitter→consumer gap from
            // docs/wave-d-prep/track-b-producer-wiring.md).
            let peer_spec = match TrustedPeerDescriptor::try_from(payload.peer_spec) {
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
            let endpoint = crate::meerkat_machine::dsl::PeerEndpoint::from(&peer_spec);
            match adapter
                .stage_add_direct_peer_endpoint(session_id, endpoint, Arc::clone(comms_runtime))
                .await
            {
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
                require_authorized_supervisor(sender, &sup_payload, &current_binding)
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            // D-track-b: mirror of WireMember — route UnwireMember
            // through `RemoveDirectPeerEndpoint`. The DSL authority's
            // peer-projection state is the source of truth; the trust
            // store is a derived projection maintained by the
            // reconciler.
            let peer_spec = match TrustedPeerDescriptor::try_from(payload.peer_spec) {
                Ok(spec) => spec,
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::InvalidPeerSpec,
                        format!("unwire member failed: invalid trusted peer spec: {error}"),
                    )
                    .await;
                    return true;
                }
            };
            let endpoint = crate::meerkat_machine::dsl::PeerEndpoint::from(&peer_spec);
            match adapter
                .stage_remove_direct_peer_endpoint(session_id, endpoint, Arc::clone(comms_runtime))
                .await
            {
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
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_contracts::BridgePeerWiringPayload;
    use meerkat_core::InteractionId;
    use meerkat_core::SendError;
    use meerkat_core::interaction::InboxInteraction;
    use meerkat_core::types::HandlingMode;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};
    use uuid::Uuid;

    // Post-#24 `PeerId` is a typed UUID and `PeerId::parse` rejects the
    // legacy `ed25519:<alias>` form. Tests in this module previously fed
    // hardcoded `PEER_ID_RECEIVER` / `PEER_ID_SUPERVISOR` literals
    // into `BridgePeerSpec.peer_id` / `BootstrapRuntime.peer_id` /
    // `TrustedPeerDescriptor::test_only_unsigned(...)`; downstream
    // `PeerId::parse` rejected. These stable UUID-string constants
    // substitute for the aliases so sender-vs-receiver comparisons still
    // match (same string across sites) while the string is a valid UUID.
    // Chosen pattern: zero-padded hex `-` UUIDs with an alias hint in the
    // last hex group so commit diffs stay readable.
    const PEER_ID_RECEIVER: &str = "00000000-0000-0000-0000-00000000aaaa"; // "receiver"
    const PEER_ID_SUPERVISOR: &str = "00000000-0000-0000-0000-00000000bbbb"; // "supervisor"
    const PEER_ID_CURRENT_SUPERVISOR: &str = "00000000-0000-0000-0000-00000000cccc"; // "current-supervisor"
    const PEER_ID_OLD_SUPERVISOR: &str = "00000000-0000-0000-0000-00000000dddd"; // "old-supervisor"

    #[tokio::test]
    async fn bridge_response_route_accepts_pubkey_string_sender() {
        let sender = meerkat_comms::Keypair::generate();
        let sender_pubkey = sender.public_key();
        let runtime: Arc<dyn CommsRuntime> = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("bridge-response-route").expect("runtime"),
        );

        let route = resolve_peer_route(&runtime, &sender_pubkey.to_pubkey_string())
            .await
            .expect("pubkey string sender should resolve to derived peer route");

        assert_eq!(route.peer_id, sender_pubkey.to_peer_id());
    }

    struct BootstrapRuntime {
        peer_id: String,
        address: String,
        bootstrap_token: Option<String>,
        inbox_notify: Arc<tokio::sync::Notify>,
        add_trusted_peer_errors: HashMap<String, String>,
        remove_trusted_peer_errors: HashMap<String, String>,
        trusted_peer_ids: Arc<tokio::sync::Mutex<HashSet<String>>>,
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

        async fn add_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
            let peer_id_str = peer.peer_id.as_str();
            if let Some(message) = self.add_trusted_peer_errors.get(&peer_id_str) {
                return Err(SendError::Internal(message.clone()));
            }
            self.trusted_peer_ids.lock().await.insert(peer_id_str);
            Ok(())
        }

        async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
            match self.remove_trusted_peer_errors.get(peer_id) {
                Some(message) => Err(SendError::Internal(message.clone())),
                None => Ok(self.trusted_peer_ids.lock().await.remove(peer_id)),
            }
        }
    }

    fn bootstrap_runtime(
        peer_id: &str,
        address: &str,
        bootstrap_token: Option<&str>,
    ) -> BootstrapRuntime {
        BootstrapRuntime {
            peer_id: peer_id.to_string(),
            address: address.to_string(),
            bootstrap_token: bootstrap_token.map(ToString::to_string),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            add_trusted_peer_errors: HashMap::new(),
            remove_trusted_peer_errors: HashMap::new(),
            trusted_peer_ids: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
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
            auth: Some(meerkat_core::PeerIngressAuthDecision::Required),
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
        let peer_spec = TrustedPeerDescriptor::test_only_unsigned(
            "peer-added".to_string(),
            peer.public_key().to_peer_id().as_str(),
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
        let peer_spec = TrustedPeerDescriptor::test_only_unsigned(
            "peer-removed".to_string(),
            peer.public_key().to_peer_id().as_str(),
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
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: PEER_ID_SUPERVISOR.to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
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
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: PEER_ID_SUPERVISOR.to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "expected-token".into(),
        };

        let (authorized, advertised_address) =
            validate_bind_request(&runtime, &supervisor.peer_id, &payload)
                .expect("bind should accept the configured bootstrap token");
        assert_eq!(authorized.peer_id.as_str(), supervisor.peer_id);
        assert_eq!(advertised_address, runtime.advertised_address().unwrap());
    }

    #[test]
    fn validate_bind_request_returns_runtime_advertised_address() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            &format!(
                "inproc://receiver-real?{SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM}=expected-token"
            ),
            Some("expected-token"),
        ));
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: PEER_ID_SUPERVISOR.to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
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
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver-real",
            Some("expected-token"),
        ));
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: PEER_ID_SUPERVISOR.to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
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
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: PEER_ID_SUPERVISOR.to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION + 1,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
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
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: PEER_ID_SUPERVISOR.to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: 1,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "expected-token".into(),
        };

        let (cause, _error) = validate_bind_request(&runtime, &supervisor.peer_id, &payload)
            .expect_err("v1 bind must be rejected under v2+");
        assert_eq!(cause, BridgeRejectionCause::UnsupportedProtocolVersion);
    }

    #[test]
    fn validate_bind_request_rejects_invalid_supervisor_peer_name() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: BridgePeerSpec {
                name: "".to_string(),
                peer_id: PEER_ID_SUPERVISOR.to_string(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
                pubkey: [0u8; 32],
            },
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
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
                peer_id: PEER_ID_SUPERVISOR.to_string(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
                pubkey: [0u8; 32],
            },
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: "inproc://receiver".to_string(),
            bootstrap_token: "expected-token".into(),
        }
    }

    fn authorized_state_for(
        payload: &meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload,
    ) -> SupervisorBinding {
        let spec = TrustedPeerDescriptor::try_from(payload.supervisor.clone())
            .expect("valid supervisor spec");
        SupervisorBinding::Bound {
            name: spec.name.to_string(),
            peer_id: spec.peer_id.as_str(),
            address: spec.address.to_string(),
            epoch: payload.epoch,
        }
    }

    #[test]
    fn validate_bind_request_against_state_allows_bootstrap_when_unbound() {
        let payload = sample_bind_payload();
        let gate = validate_bind_request_against_state(
            &payload.supervisor.peer_id,
            &payload,
            &SupervisorBinding::Unbound,
        )
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
            pubkey: [0u8; 32],
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
        // into the DSL-owned supervisor binding.
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
        adapter.register_session(session_id.clone()).await;
        let current_supervisor = TrustedPeerDescriptor::test_only_unsigned(
            "mob/__mob_supervisor__",
            supervisor_runtime.public_key().to_peer_id().as_str(),
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid supervisor spec");
        adapter
            .stage_supervisor_bind(
                &session_id,
                current_supervisor.name.to_string(),
                current_supervisor.peer_id.as_str(),
                current_supervisor.address.to_string(),
                1,
            )
            .await
            .expect("initial bind must succeed");
        let adversary_supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:adversary".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                supervisor: adversary_supervisor,
                epoch: 2,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                expected_peer_id: runtime
                    .public_key()
                    .unwrap_or_else(|| PEER_ID_RECEIVER.to_string()),
                expected_address: runtime
                    .advertised_address()
                    .unwrap_or_else(|| "inproc://bind-rebind-receiver".to_string()),
                // Even with a *valid* bootstrap token, the rebind must fail.
                bootstrap_token: runtime.bridge_bootstrap_token().unwrap_or_default().into(),
            },
        );
        let candidate = bridge_candidate("ed25519:adversary", &command);

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate,)
                .await,
            "bridge handler must own the BindMember command"
        );
        let binding = adapter.supervisor_binding(&session_id).await;
        let SupervisorBinding::Bound { peer_id, epoch, .. } = binding else {
            panic!("supervisor binding must be preserved as Bound");
        };
        assert_eq!(
            peer_id,
            current_supervisor.peer_id.as_str(),
            "rebind attempt must not replace the authorized supervisor"
        );
        assert_eq!(
            epoch, 1,
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
            peer_id: PEER_ID_RECEIVER.to_string(),
            advertised_address: Some("inproc://receiver".to_string()),
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            sent: sent.clone(),
        });
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;
        let current = TrustedPeerDescriptor::test_only_unsigned_typed(
            "mob/__mob_supervisor__",
            PeerId::new(),
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid supervisor spec");
        adapter
            .stage_supervisor_bind(
                &session_id,
                current.name.to_string(),
                current.peer_id.as_str(),
                current.address.to_string(),
                1,
            )
            .await
            .expect("initial bind must succeed");

        // A different supervisor tries to rebind. Under the strict gate this
        // must be rejected with typed `AlreadyBound` — the mob-side bridge
        // fallback logic branches on the typed cause, not on reason text.
        let adversary = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:different-supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                supervisor: adversary,
                epoch: 2,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                expected_peer_id: PEER_ID_RECEIVER.to_string(),
                expected_address: "inproc://receiver".to_string(),
                bootstrap_token: "expected-token".into(),
            },
        );
        let candidate = bridge_candidate("ed25519:different-supervisor", &command);

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate,)
                .await,
            "bridge handler must own the BindMember command"
        );

        // DSL binding is preserved (already covered elsewhere, pinned again here).
        let binding = adapter.supervisor_binding(&session_id).await;
        let SupervisorBinding::Bound { peer_id, .. } = binding else {
            panic!("binding preserved");
        };
        assert_eq!(peer_id, current.peer_id.as_str());

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

        async fn add_trusted_peer(&self, _peer: TrustedPeerDescriptor) -> Result<(), SendError> {
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
            peer_id: PEER_ID_RECEIVER.to_string(),
            // Simulate the invariant violation: no advertised address at the
            // moment of idempotent ack.
            advertised_address: None,
            bootstrap_token: Some("expected-token".to_string()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            sent: sent.clone(),
        });
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;
        let authorized = TrustedPeerDescriptor::test_only_unsigned_typed(
            "mob/__mob_supervisor__",
            PeerId::new(),
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid supervisor spec");
        adapter
            .stage_supervisor_bind(
                &session_id,
                authorized.name.to_string(),
                authorized.peer_id.as_str(),
                authorized.address.to_string(),
                7,
            )
            .await
            .expect("pre-bind supervisor");
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
                    name: authorized.name.to_string(),
                    peer_id: authorized.peer_id.as_str(),
                    address: authorized.address.to_string(),
                    pubkey: [0u8; 32],
                },
                epoch: 7,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                expected_peer_id: attacker_peer_id.clone(),
                expected_address: attacker_address.clone(),
                bootstrap_token: "expected-token".into(),
            },
        );
        let candidate = bridge_candidate(&authorized.peer_id.as_str(), &command);

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate,)
                .await,
            "bridge handler must own the BindMember command"
        );

        // The DSL-owned binding must be preserved — an invariant-violation
        // reply must not mutate authority.
        let binding = adapter.supervisor_binding(&session_id).await;
        let SupervisorBinding::Bound {
            peer_id: stored_peer_id,
            epoch: stored_epoch,
            ..
        } = binding
        else {
            panic!("binding must survive invariant failure");
        };
        assert_eq!(stored_peer_id, authorized.peer_id.as_str());
        assert_eq!(stored_epoch, 7);

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
                peer_id: PEER_ID_SUPERVISOR.to_string(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
                pubkey: [0u8; 32],
            },
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        };

        let (cause, error) = validate_authorize_supervisor_request(
            &payload.supervisor.peer_id,
            &payload,
            &SupervisorBinding::Unbound,
        )
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
                peer_id: PEER_ID_SUPERVISOR.to_string(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
                pubkey: [0u8; 32],
            },
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION + 1,
        };

        let (cause, error) = validate_authorize_supervisor_request(
            &payload.supervisor.peer_id,
            &payload,
            &SupervisorBinding::Unbound,
        )
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
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some(""),
        ));
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: PEER_ID_SUPERVISOR.to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
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
                peer_id: PEER_ID_SUPERVISOR.to_string(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
                pubkey: [0u8; 32],
            },
            epoch: 0,
            protocol_version: 1,
        };
        let (cause, _error) = validate_authorize_supervisor_request(
            &payload.supervisor.peer_id,
            &payload,
            &SupervisorBinding::Unbound,
        )
        .expect_err("v1 authorize must be rejected under v2+");
        assert_eq!(cause, BridgeRejectionCause::UnsupportedProtocolVersion);
    }

    #[test]
    fn require_authorized_supervisor_rejects_explicit_v1_protocol_under_v2() {
        // Back-stops every command that calls `require_authorized_supervisor`
        // (revoke/observe/interrupt/retire/destroy/deliver/wire/unwire). A v1
        // payload must not coast on idempotent-ack or sender-match.
        let supervisor = TrustedPeerDescriptor::test_only_unsigned_typed(
            "mob/__mob_supervisor__",
            PeerId::new(),
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid supervisor spec");
        let state = SupervisorBinding::Bound {
            name: supervisor.name.to_string(),
            peer_id: supervisor.peer_id.as_str(),
            address: supervisor.address.to_string(),
            epoch: 3,
        };
        let payload = BridgeSupervisorPayload {
            supervisor: BridgePeerSpec {
                name: supervisor.name.to_string(),
                peer_id: supervisor.peer_id.as_str(),
                address: supervisor.address.to_string(),
                pubkey: [0u8; 32],
            },
            epoch: 3,
            protocol_version: 1,
        };
        let (cause, _error) =
            require_authorized_supervisor(&supervisor.peer_id.as_str(), &payload, &state)
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
            auth: Some(meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
            )),
            lifecycle_peer: None,
        }
    }

    #[tokio::test]
    async fn bind_member_rolls_back_binding_when_trust_publication_fails() {
        // DOGMA-19 defensive scan: a post-bind trust publication failure must
        // not leave the supervisor bound in the DSL.
        let payload = sample_bind_payload();
        let sender = payload.supervisor.peer_id.clone();
        let mut runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        runtime_impl
            .add_trusted_peer_errors
            .insert(payload.supervisor.peer_id.clone(), "boom".to_string());
        let trusted_peer_ids = runtime_impl.trusted_peer_ids.clone();
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;
        let candidate = bridge_candidate(&sender, &BridgeCommand::BindMember(payload));

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate).await
        );
        assert!(
            matches!(
                adapter.supervisor_binding(&session_id).await,
                SupervisorBinding::Unbound
            ),
            "failed trust publication must roll BindMember back to Unbound"
        );
        assert!(
            trusted_peer_ids.lock().await.is_empty(),
            "failed trust publication must not leave the supervisor trusted"
        );
    }

    #[tokio::test]
    async fn authorize_supervisor_restores_old_binding_when_new_trust_publish_fails() {
        // DOGMA-19 defensive scan: the old supervisor remains authoritative
        // until the new supervisor trust publishes successfully.
        let old_supervisor = TrustedPeerDescriptor::test_only_unsigned_typed(
            "mob/__mob_supervisor__",
            PeerId::new(),
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid old supervisor");
        let new_supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:new-supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let payload = BridgeSupervisorPayload {
            supervisor: new_supervisor.clone(),
            epoch: 2,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        };
        let mut runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        runtime_impl
            .add_trusted_peer_errors
            .insert(new_supervisor.peer_id.clone(), "boom".to_string());
        let trusted_peer_ids = runtime_impl.trusted_peer_ids.clone();
        trusted_peer_ids
            .lock()
            .await
            .insert(old_supervisor.peer_id.as_str());
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;
        adapter
            .stage_supervisor_bind(
                &session_id,
                old_supervisor.name.to_string(),
                old_supervisor.peer_id.as_str(),
                old_supervisor.address.to_string(),
                1,
            )
            .await
            .expect("pre-bind old supervisor");
        let candidate = bridge_candidate(
            &old_supervisor.peer_id.as_str(),
            &BridgeCommand::AuthorizeSupervisor(payload),
        );

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate).await
        );
        match adapter.supervisor_binding(&session_id).await {
            SupervisorBinding::Bound { peer_id, epoch, .. } => {
                assert_eq!(peer_id, old_supervisor.peer_id.as_str());
                assert_eq!(epoch, 1);
            }
            SupervisorBinding::Unbound => panic!("old supervisor must remain bound"),
        }
        let trusted = trusted_peer_ids.lock().await.clone();
        assert!(
            trusted.contains(&old_supervisor.peer_id.as_str()),
            "old supervisor trust must stay active on failed rotation"
        );
        assert!(
            !trusted.contains(&new_supervisor.peer_id),
            "new supervisor trust must not publish on failed rotation"
        );
    }

    #[tokio::test]
    async fn authorize_supervisor_rolls_back_when_old_trust_removal_fails() {
        // DOGMA-19 defensive scan: if the old trust cannot be retired after
        // the DSL rotates, restore the old binding and clean the new trust up.
        let old_supervisor = TrustedPeerDescriptor::test_only_unsigned_typed(
            "mob/__mob_supervisor__",
            PeerId::new(),
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid old supervisor");
        let new_supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: "ed25519:new-supervisor".to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
        };
        let payload = BridgeSupervisorPayload {
            supervisor: new_supervisor.clone(),
            epoch: 2,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        };
        let mut runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        runtime_impl
            .remove_trusted_peer_errors
            .insert(old_supervisor.peer_id.as_str(), "boom".to_string());
        let trusted_peer_ids = runtime_impl.trusted_peer_ids.clone();
        trusted_peer_ids
            .lock()
            .await
            .insert(old_supervisor.peer_id.as_str());
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;
        adapter
            .stage_supervisor_bind(
                &session_id,
                old_supervisor.name.to_string(),
                old_supervisor.peer_id.as_str(),
                old_supervisor.address.to_string(),
                1,
            )
            .await
            .expect("pre-bind old supervisor");
        let candidate = bridge_candidate(
            &old_supervisor.peer_id.as_str(),
            &BridgeCommand::AuthorizeSupervisor(payload),
        );

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate).await
        );
        match adapter.supervisor_binding(&session_id).await {
            SupervisorBinding::Bound { peer_id, epoch, .. } => {
                assert_eq!(peer_id, old_supervisor.peer_id.as_str());
                assert_eq!(epoch, 1);
            }
            SupervisorBinding::Unbound => panic!("old supervisor must be restored after rollback"),
        }
        let trusted = trusted_peer_ids.lock().await.clone();
        assert!(
            trusted.contains(&old_supervisor.peer_id.as_str()),
            "old supervisor trust must remain after rollback"
        );
        assert!(
            !trusted.contains(&new_supervisor.peer_id),
            "new supervisor trust must be cleaned up after rollback"
        );
    }

    #[tokio::test]
    async fn revoke_supervisor_keeps_authority_when_trust_removal_fails() {
        let mut runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        runtime_impl
            .remove_trusted_peer_errors
            .insert(PEER_ID_SUPERVISOR.to_string(), "boom".to_string());
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: PEER_ID_SUPERVISOR.to_string(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: [0u8; 32],
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
        let spec =
            TrustedPeerDescriptor::try_from(supervisor.clone()).expect("valid supervisor spec");
        adapter
            .stage_supervisor_bind(
                &session_id,
                spec.name.to_string(),
                spec.peer_id.as_str(),
                spec.address.to_string(),
                payload.epoch,
            )
            .await
            .expect("pre-bind supervisor");

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate,)
                .await,
            "revoke command should be handled"
        );
        assert!(
            matches!(
                adapter.supervisor_binding(&session_id).await,
                SupervisorBinding::Bound { .. }
            ),
            "failed revoke must preserve supervisor authority until trust removal succeeds"
        );
    }

    // Wave 3 D Row 21 defensive test: the DSL supervisor-binding guards
    // structurally prevent double-bind and stale-epoch revoke. If someone
    // accidentally broadens the guards in a future refactor (e.g. allowing
    // `Bound → Bound` on `BindSupervisor`, or a revoke with a mismatched
    // epoch), this test fails, catching the regression at the DSL layer
    // before any shell-side validator runs.
    #[tokio::test]
    async fn dsl_supervisor_guards_block_rebind_and_stale_revoke() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;

        // Start Unbound → Bound.
        adapter
            .stage_supervisor_bind(
                &session_id,
                "super-a".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a".to_string(),
                1,
            )
            .await
            .expect("initial bind");

        // Attempting a second `BindSupervisor` must be DSL-rejected — the
        // guard requires `Unbound`. Rotation goes through AuthorizeSupervisor.
        let rebind = adapter
            .stage_supervisor_bind(
                &session_id,
                "super-b".to_string(),
                "ed25519:super-b".to_string(),
                "inproc://super-b".to_string(),
                2,
            )
            .await;
        assert!(
            matches!(rebind, Err(SupervisorBindingStageError::Dsl(_))),
            "double bind must be rejected by DSL guard, got: {rebind:?}"
        );

        // Revoke with a non-matching peer_id must be DSL-rejected.
        let stale_peer_revoke = adapter
            .stage_supervisor_revoke(&session_id, "ed25519:super-b".to_string(), 1)
            .await;
        assert!(
            matches!(stale_peer_revoke, Err(SupervisorBindingStageError::Dsl(_))),
            "revoke with mismatched peer_id must be DSL-rejected, got: {stale_peer_revoke:?}"
        );

        // Revoke with mismatched epoch must be DSL-rejected.
        let stale_epoch_revoke = adapter
            .stage_supervisor_revoke(&session_id, "ed25519:super-a".to_string(), 99)
            .await;
        assert!(
            matches!(stale_epoch_revoke, Err(SupervisorBindingStageError::Dsl(_))),
            "revoke with mismatched epoch must be DSL-rejected, got: {stale_epoch_revoke:?}"
        );

        // Proper rotation via AuthorizeSupervisor must succeed.
        adapter
            .stage_supervisor_authorize(
                &session_id,
                "super-c".to_string(),
                "ed25519:super-c".to_string(),
                "inproc://super-c".to_string(),
                2,
            )
            .await
            .expect("rotation must succeed");

        // Read back: binding is `Bound { super-c, 2 }`.
        match adapter.supervisor_binding(&session_id).await {
            SupervisorBinding::Bound { peer_id, epoch, .. } => {
                assert_eq!(peer_id, "ed25519:super-c");
                assert_eq!(epoch, 2);
            }
            SupervisorBinding::Unbound => panic!("expected Bound after rotation"),
        }

        // Proper revoke with matching identity + epoch must succeed and
        // return to Unbound.
        adapter
            .stage_supervisor_revoke(&session_id, "ed25519:super-c".to_string(), 2)
            .await
            .expect("matching revoke must succeed");
        assert!(
            matches!(
                adapter.supervisor_binding(&session_id).await,
                SupervisorBinding::Unbound
            ),
            "binding must be Unbound after matching revoke"
        );
    }

    // Wave-d D-d: supervisor-trust-edge feedback acks carry the epoch
    // observed on the producer effect. The DSL guard rejects a stale-
    // epoch ack arriving after the binding has rotated forward — the
    // outstanding obligation stays open, the stale ack does not close
    // it. Mirrors `correlation_fields = [peer_id, epoch]` on the
    // `supervisor_trust_publish` / `supervisor_trust_revoke` protocols.
    #[tokio::test]
    async fn dsl_supervisor_trust_publish_ack_stale_epoch_is_rejected() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;

        // Bind at epoch 1.
        adapter
            .stage_supervisor_bind(
                &session_id,
                "super-a".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a".to_string(),
                1,
            )
            .await
            .expect("initial bind");

        // Rotate forward to epoch 2 before the ack for epoch 1 arrives.
        adapter
            .stage_supervisor_authorize(
                &session_id,
                "super-a".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a".to_string(),
                2,
            )
            .await
            .expect("rotation to epoch 2");

        // Stale ack for epoch 1 — must be DSL-rejected. The guard
        // checks `self.supervisor_bound_epoch == Some(epoch)`, and the
        // binding is now at epoch 2.
        let stale = adapter
            .stage_supervisor_trust_published(&session_id, "ed25519:super-a".to_string(), 1)
            .await;
        assert!(
            matches!(stale, Err(SupervisorBindingStageError::Dsl(_))),
            "stale-epoch publish ack must be DSL-rejected, got: {stale:?}"
        );
        // State must be unchanged — still bound at epoch 2.
        match adapter.supervisor_binding(&session_id).await {
            SupervisorBinding::Bound { epoch, .. } => {
                assert_eq!(epoch, 2, "binding must still be at epoch 2");
            }
            SupervisorBinding::Unbound => panic!("stale ack must not unbind"),
        }

        // Matching-epoch ack closes the obligation — transition
        // accepted, no state mutation (phase-preserving self-loop).
        adapter
            .stage_supervisor_trust_published(&session_id, "ed25519:super-a".to_string(), 2)
            .await
            .expect("matching-epoch ack must be accepted");
    }

    #[tokio::test]
    async fn dsl_supervisor_trust_revoke_ack_stale_epoch_is_rejected() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;

        // Bind at epoch 1.
        adapter
            .stage_supervisor_bind(
                &session_id,
                "super-a".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a".to_string(),
                1,
            )
            .await
            .expect("initial bind");

        // Rotate forward to epoch 2.
        adapter
            .stage_supervisor_authorize(
                &session_id,
                "super-a".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a".to_string(),
                2,
            )
            .await
            .expect("rotation to epoch 2");

        // Stale revoke ack for epoch 1 — DSL-rejected.
        let stale = adapter
            .stage_supervisor_trust_revoked(&session_id, "ed25519:super-a".to_string(), 1)
            .await;
        assert!(
            matches!(stale, Err(SupervisorBindingStageError::Dsl(_))),
            "stale-epoch revoke ack must be DSL-rejected, got: {stale:?}"
        );
        match adapter.supervisor_binding(&session_id).await {
            SupervisorBinding::Bound { epoch, .. } => {
                assert_eq!(epoch, 2, "binding must still be at epoch 2");
            }
            SupervisorBinding::Unbound => panic!("stale revoke ack must not unbind"),
        }

        // Matching-epoch revoke ack accepted.
        adapter
            .stage_supervisor_trust_revoked(&session_id, "ed25519:super-a".to_string(), 2)
            .await
            .expect("matching-epoch revoke ack must be accepted");
    }

    #[tokio::test]
    async fn dsl_supervisor_trust_ack_rejects_mismatched_peer_id() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;

        adapter
            .stage_supervisor_bind(
                &session_id,
                "super-a".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a".to_string(),
                7,
            )
            .await
            .expect("initial bind");

        // Correct epoch but wrong peer_id — must be rejected.
        let wrong_peer = adapter
            .stage_supervisor_trust_published(&session_id, "ed25519:unrelated".to_string(), 7)
            .await;
        assert!(
            matches!(wrong_peer, Err(SupervisorBindingStageError::Dsl(_))),
            "mismatched-peer ack must be DSL-rejected, got: {wrong_peer:?}"
        );
    }

    #[tokio::test]
    async fn dsl_supervisor_trust_ack_rejected_when_unbound() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;

        // Unbound from the start — any ack must be rejected.
        let unbound = adapter
            .stage_supervisor_trust_published(&session_id, "ed25519:super-a".to_string(), 1)
            .await;
        assert!(
            matches!(unbound, Err(SupervisorBindingStageError::Dsl(_))),
            "publish ack must be rejected when Unbound, got: {unbound:?}"
        );
    }

    #[tokio::test]
    async fn wire_member_rejects_invalid_peer_spec_before_trusting_it() {
        let runtime = Arc::new(meerkat_comms::CommsRuntime::inproc_only("receiver-wire").unwrap());
        let supervisor_runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("mob/__mob_supervisor__").unwrap());
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;
        let supervisor = TrustedPeerDescriptor::test_only_unsigned(
            "mob/__mob_supervisor__",
            supervisor_runtime.public_key().to_peer_id().as_str(),
            "inproc://mob/__mob_supervisor__",
        )
        .unwrap();
        runtime
            .add_trusted_peer(supervisor.clone())
            .await
            .expect("trust supervisor");
        adapter
            .stage_supervisor_bind(
                &session_id,
                supervisor.name.to_string(),
                supervisor.peer_id.as_str(),
                supervisor.address.to_string(),
                1,
            )
            .await
            .expect("pre-bind supervisor");
        let candidate = bridge_candidate(
            &supervisor.peer_id.as_str(),
            &BridgeCommand::WireMember(BridgePeerWiringPayload {
                supervisor: supervisor.clone().into(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                peer_spec: BridgePeerSpec {
                    name: "".to_string(),
                    peer_id: "ed25519:peer".to_string(),
                    address: "inproc://peer".to_string(),
                    pubkey: [0u8; 32],
                },
            }),
        );

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
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
