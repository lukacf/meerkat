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

use meerkat_core::comms_drain_lifecycle_authority::DrainExitReason;

use meerkat_contracts::wire::supervisor_bridge::{
    BridgeAck, BridgeBindResponse, BridgeCapabilities, BridgeCommand, BridgeDeliveryOutcome,
    BridgeDeliveryPayload, BridgeDeliveryResponse, BridgeDestroyResponse, BridgeMemberRuntimeState,
    BridgeObservationResponse, BridgePeerConnectivity, BridgePeerSpec, BridgeRetireResponse,
    BridgeSupervisorPayload, SUPERVISOR_BRIDGE_INTENT,
};

use crate::comms_bridge::classified_interaction_to_runtime_input;
use crate::completion::CompletionOutcome;
use crate::identifiers::IdempotencyKey;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
};
use crate::meerkat_machine::MeerkatMachine;
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
                        reconcile_lifecycle_peer_trust(&comms_runtime, &candidate).await;
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
) -> Result<AuthorizedSupervisorState, String> {
    let Some(current) = current else {
        return Err("no authorized supervisor registered".to_string());
    };
    if payload.epoch < current.epoch {
        return Err(format!(
            "stale supervisor epoch {} (current {})",
            payload.epoch, current.epoch
        ));
    }
    if payload.epoch != current.epoch {
        return Err(format!(
            "unexpected supervisor epoch {} (current {})",
            payload.epoch, current.epoch
        ));
    }
    if payload.supervisor.peer_id != current.supervisor.peer_id {
        return Err(format!(
            "stale supervisor peer '{}' (current '{}')",
            payload.supervisor.peer_id, current.supervisor.peer_id
        ));
    }
    if !sender_matches_supervisor(sender, &current.supervisor) {
        return Err(format!(
            "request sender '{sender}' does not match authorized supervisor '{}'",
            current.supervisor.peer_id
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
        blocks,
        handling_mode: match payload.handling_mode {
            meerkat_core::types::HandlingMode::Queue => None,
            mode => Some(mode),
        },
    })
}

fn lifecycle_peer_spec_from_params(params: &serde_json::Value) -> Result<TrustedPeerSpec, String> {
    if let Some(spec_value) = params.get("peer_spec") {
        return serde_json::from_value(spec_value.clone())
            .map_err(|error| format!("invalid peer_spec in lifecycle request: {error}"));
    }

    let peer_name = params
        .get("peer_name")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing peer_name in lifecycle request".to_string())?;
    let peer_id = params
        .get("peer_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing peer_id in lifecycle request".to_string())?;
    let address = params
        .get("address")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing address in lifecycle request".to_string())?;

    TrustedPeerSpec::new(
        peer_name.to_string(),
        peer_id.to_string(),
        address.to_string(),
    )
    .map_err(|error| format!("invalid lifecycle peer spec: {error}"))
}

async fn reconcile_lifecycle_peer_trust(
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
) {
    let InteractionContent::Request { params, .. } = &candidate.interaction.content else {
        return;
    };

    let peer_spec = match lifecycle_peer_spec_from_params(params) {
        Ok(spec) => spec,
        Err(error) => {
            tracing::warn!(
                interaction_id = %candidate.interaction.id,
                class = ?candidate.class,
                error = %error,
                "comms_drain: failed to parse lifecycle peer spec"
            );
            return;
        }
    };

    match candidate.class {
        PeerInputClass::PeerLifecycleAdded => {
            if let Err(error) = comms_runtime.add_trusted_peer(peer_spec).await {
                tracing::warn!(
                    interaction_id = %candidate.interaction.id,
                    error = %error,
                    "comms_drain: failed to add trusted peer from lifecycle notice"
                );
            }
        }
        PeerInputClass::PeerLifecycleRetired | PeerInputClass::PeerLifecycleUnwired => {
            if let Err(error) = comms_runtime.remove_trusted_peer(&peer_spec.peer_id).await {
                tracing::warn!(
                    interaction_id = %candidate.interaction.id,
                    error = %error,
                    "comms_drain: failed to remove trusted peer from lifecycle notice"
                );
            }
        }
        _ => {}
    }
}

async fn send_bridge_response(
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
    status: meerkat_core::interaction::ResponseStatus,
    result: serde_json::Value,
) {
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
    message: impl Into<String>,
) {
    send_bridge_response(
        comms_runtime,
        candidate,
        meerkat_core::interaction::ResponseStatus::Failed,
        serde_json::Value::String(message.into()),
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

    // Accept both the canonical typed intent and legacy per-command intents.
    let is_bridge = intent == SUPERVISOR_BRIDGE_INTENT;
    if !is_bridge {
        return false;
    }

    let command: BridgeCommand = match serde_json::from_value(params.clone()) {
        Ok(cmd) => cmd,
        Err(error) => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                format!("invalid bridge command: {error}"),
            )
            .await;
            return true;
        }
    };

    let sender = &candidate.interaction.from;

    match command {
        BridgeCommand::BindMember(payload) => {
            if !sender_matches_bridge_peer(sender, &payload.supervisor) {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    format!(
                        "request sender '{sender}' does not match supervisor '{}'",
                        payload.supervisor.peer_id
                    ),
                )
                .await;
                return true;
            }
            if let Some(actual_peer_id) = comms_runtime.public_key()
                && actual_peer_id != payload.expected_peer_id
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    format!(
                        "bind peer_id mismatch: expected '{}', actual '{actual_peer_id}'",
                        payload.expected_peer_id
                    ),
                )
                .await;
                return true;
            }
            let supervisor_spec: TrustedPeerSpec = payload.supervisor.clone().into();
            match comms_runtime.add_trusted_peer(supervisor_spec).await {
                Ok(()) => {
                    *supervisor_state = Some(AuthorizedSupervisorState {
                        supervisor: payload.supervisor.clone().into(),
                        epoch: payload.epoch,
                    });
                    let response = serde_json::to_value(BridgeBindResponse {
                        peer_id: comms_runtime
                            .public_key()
                            .unwrap_or(payload.expected_peer_id),
                        address: payload.expected_address,
                        capabilities: bridge_capabilities(),
                    })
                    .unwrap_or_else(|_| serde_json::json!({ "ok": true }));
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        response,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        format!("bind member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::AuthorizeSupervisor(payload) => {
            if let Some(current) = supervisor_state.as_ref()
                && payload.epoch < current.epoch
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    format!(
                        "authorize supervisor failed: stale supervisor epoch {} (current {})",
                        payload.epoch, current.epoch
                    ),
                )
                .await;
                return true;
            }
            if let Some(current) = supervisor_state.as_ref() {
                if !sender_matches_supervisor(sender, &current.supervisor) {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        format!(
                            "authorize supervisor failed: request sender '{sender}' does not match authorized supervisor '{}'",
                            current.supervisor.peer_id
                        ),
                    )
                    .await;
                    return true;
                }
                if payload.epoch == current.epoch
                    && payload.supervisor.peer_id == current.supervisor.peer_id
                {
                    let response = serde_json::to_value(BridgeAck { ok: true })
                        .unwrap_or(serde_json::Value::Bool(true));
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        response,
                    )
                    .await;
                    return true;
                }
            } else if !sender_matches_bridge_peer(sender, &payload.supervisor) {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    format!(
                        "authorize supervisor failed: request sender '{sender}' does not match supervisor '{}'",
                        payload.supervisor.peer_id
                    ),
                )
                .await;
                return true;
            }

            let old_supervisor = supervisor_state.as_ref().map(|s| s.supervisor.clone());
            let supervisor_spec: TrustedPeerSpec = payload.supervisor.clone().into();
            let response = match comms_runtime.add_trusted_peer(supervisor_spec).await {
                Ok(()) => {
                    if let Some(old_supervisor) = old_supervisor
                        && old_supervisor.peer_id != payload.supervisor.peer_id
                    {
                        let _ = comms_runtime
                            .remove_trusted_peer(&old_supervisor.peer_id)
                            .await;
                    }
                    *supervisor_state = Some(AuthorizedSupervisorState {
                        supervisor: payload.supervisor.into(),
                        epoch: payload.epoch,
                    });
                    serde_json::to_value(BridgeAck { ok: true })
                        .unwrap_or(serde_json::Value::Bool(true))
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        format!("authorize supervisor failed: {error}"),
                    )
                    .await;
                    return true;
                }
            };
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                response,
            )
            .await;
            true
        }
        BridgeCommand::RevokeSupervisor(payload) => {
            if let Err(error) = require_authorized_supervisor(sender, &payload, supervisor_state) {
                send_bridge_failure(comms_runtime, candidate, error).await;
                return true;
            }
            let response = serde_json::to_value(BridgeAck { ok: true })
                .unwrap_or(serde_json::Value::Bool(true));
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                response,
            )
            .await;
            if let Err(error) = comms_runtime
                .remove_trusted_peer(&payload.supervisor.peer_id)
                .await
            {
                tracing::warn!(
                    interaction_id = %candidate.interaction.id,
                    error = %error,
                    "comms_drain: revoke supervisor cleanup failed"
                );
            }
            *supervisor_state = None;
            true
        }
        BridgeCommand::DeliverMemberInput(payload) => {
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err(error) =
                require_authorized_supervisor(sender, &sup_payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, error).await;
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
                    let response = serde_json::to_value(response)
                        .unwrap_or_else(|_| serde_json::json!({ "ok": true }));
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        response,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        format!("deliver member input failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::InterruptMember(payload) => {
            if let Err(error) = require_authorized_supervisor(sender, &payload, supervisor_state) {
                send_bridge_failure(comms_runtime, candidate, error).await;
                return true;
            }
            match adapter.interrupt_current_run(session_id).await {
                Ok(()) => {
                    let response = serde_json::to_value(BridgeAck { ok: true })
                        .unwrap_or(serde_json::Value::Bool(true));
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        response,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        format!("interrupt member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::RetireMember(payload) => {
            if let Err(error) = require_authorized_supervisor(sender, &payload, supervisor_state) {
                send_bridge_failure(comms_runtime, candidate, error).await;
                return true;
            }
            match adapter.retire_runtime(session_id).await {
                Ok(report) => {
                    let response = serde_json::to_value(BridgeRetireResponse {
                        inputs_abandoned: report.inputs_abandoned,
                        inputs_pending_drain: report.inputs_pending_drain,
                    })
                    .unwrap_or_else(|_| serde_json::json!({ "ok": true }));
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        response,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        format!("retire member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::DestroyMember(payload) => {
            if let Err(error) = require_authorized_supervisor(sender, &payload, supervisor_state) {
                send_bridge_failure(comms_runtime, candidate, error).await;
                return true;
            }
            let runtime_id = LogicalRuntimeId::new(session_id.to_string());
            match RuntimeControlPlane::destroy(adapter.as_ref(), &runtime_id).await {
                Ok(report) => {
                    let response = serde_json::to_value(BridgeDestroyResponse {
                        inputs_abandoned: report.inputs_abandoned,
                    })
                    .unwrap_or_else(|_| serde_json::json!({ "ok": true }));
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        response,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        format!("destroy member failed: {error}"),
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::ObserveMember(payload) => {
            if let Err(error) = require_authorized_supervisor(sender, &payload, supervisor_state) {
                send_bridge_failure(comms_runtime, candidate, error).await;
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
                    let response = serde_json::to_value(BridgeObservationResponse {
                        phase: bridge_state,
                        state: bridge_state,
                        accepting_inputs: Some(state.can_accept_input()),
                        current_run: current_run_id.clone(),
                        current_run_id,
                        peer_connectivity: Some(BridgePeerConnectivity::Reachable),
                        last_error: None,
                        observed_at: chrono::Utc::now().to_rfc3339(),
                    })
                    .unwrap_or_else(|_| serde_json::json!({ "state": bridge_state.to_string() }));
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        response,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
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
            if let Err(error) =
                require_authorized_supervisor(sender, &sup_payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, error).await;
                return true;
            }
            let peer_spec: TrustedPeerSpec = payload.peer_spec.into();
            match comms_runtime.add_trusted_peer(peer_spec).await {
                Ok(()) => {
                    let response = serde_json::to_value(BridgeAck { ok: true })
                        .unwrap_or(serde_json::Value::Bool(true));
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        response,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
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
            if let Err(error) =
                require_authorized_supervisor(sender, &sup_payload, supervisor_state)
            {
                send_bridge_failure(comms_runtime, candidate, error).await;
                return true;
            }
            match comms_runtime
                .remove_trusted_peer(&payload.peer_spec.peer_id)
                .await
            {
                Ok(_) => {
                    let response = serde_json::to_value(BridgeAck { ok: true })
                        .unwrap_or(serde_json::Value::Bool(true));
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        response,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
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
    use meerkat_core::InteractionId;
    use meerkat_core::interaction::InboxInteraction;
    use meerkat_core::types::HandlingMode;
    use serde_json::json;
    use uuid::Uuid;

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
    async fn peer_lifecycle_added_syncs_trust_to_comms_runtime() {
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

        reconcile_lifecycle_peer_trust(&runtime, &candidate).await;

        let peers = runtime.peers().await;
        assert!(
            peers
                .iter()
                .any(|entry| entry.name.as_str() == "peer-added"),
            "peer lifecycle add should materialize comms trust"
        );
    }

    #[tokio::test]
    async fn peer_lifecycle_unwired_and_retired_remove_trust_from_comms_runtime() {
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
        reconcile_lifecycle_peer_trust(&runtime, &unwired).await;
        assert!(
            !runtime
                .peers()
                .await
                .iter()
                .any(|entry| entry.name.as_str() == "peer-removed"),
            "peer lifecycle unwire should revoke comms trust"
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
        reconcile_lifecycle_peer_trust(&runtime, &retired).await;
        assert!(
            !runtime
                .peers()
                .await
                .iter()
                .any(|entry| entry.name.as_str() == "peer-removed"),
            "peer lifecycle retire should revoke comms trust"
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
