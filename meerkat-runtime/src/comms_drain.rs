//! Comms inbox drain task.
//!
//! Standalone tokio task that drains `CommsRuntime` inbox and feeds typed
//! `Input` values into `MeerkatMachine`. Replaces the old
//! `RuntimeCommsBridge` (comms_sink.rs) which implemented the now-removed
//! `RuntimeInputSink` trait on `meerkat-core`.

#![allow(clippy::large_futures)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::agent::CommsRuntime;
#[allow(unused_imports)]
use meerkat_core::comms::{
    CommsCommand, CommsTrustMutation, CommsTrustMutationAuthority, CommsTrustMutationResult,
    PeerId, PeerRoute, SendError, TrustedPeerDescriptor,
};
use meerkat_core::event::{AgentEvent, InteractionFailureReason};
use meerkat_core::interaction::{
    InteractionContent, PeerIngressFact, PeerInputCandidate, PeerInputClass,
};
use meerkat_core::types::SessionId;

use meerkat_contracts::wire::supervisor_bridge::{
    BridgeAck, BridgeBindResponse, BridgeCapabilities, BridgeCommand, BridgeDeliveryOutcome,
    BridgeDeliveryPayload, BridgeDeliveryRejectionCause, BridgeDeliveryResponse,
    BridgeDestroyResponse, BridgeMemberRuntimeState, BridgeMobPeerOverlayHandoff,
    BridgeObservationResponse, BridgePeerConnectivity, BridgePeerIdentity, BridgePeerSpec,
    BridgeProtocolVersion, BridgeRejectionCause, BridgeReply, BridgeRetireResponse,
    BridgeSupervisorPayload, SUPERVISOR_BRIDGE_INTENT, canonicalize_bridge_address,
    decode_bridge_command,
};
#[cfg(test)]
use meerkat_contracts::wire::supervisor_bridge::{
    SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
};

use crate::comms_bridge::classified_interaction_to_runtime_input;
use crate::completion::CompletionOutcome;
use crate::identifiers::IdempotencyKey;
#[cfg(test)]
use crate::identifiers::LogicalRuntimeId;
use crate::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
};

use crate::meerkat_machine::SupervisorBinding;
use crate::meerkat_machine::{
    DrainExitReason, MeerkatMachine, SupervisorAuthorizeAdmission, SupervisorBindAdmission,
    SupervisorBindingStageError,
};
use crate::service_ext::SessionServiceRuntimeExt as _;
use crate::tokio::sync::mpsc;
use crate::traits::RuntimeControlPlane;

/// Default idle timeout for session-backed comms drains.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Spawn a background task that drains the comms inbox and routes
/// classified interactions through the runtime adapter.
///
/// The task runs until its idle timeout expires or the returned `JoinHandle`
/// is aborted by the drain lifecycle authority. Lifecycle dismissal is a typed
/// signal owned by that authority, never a peer message body.
pub fn spawn_comms_drain(
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
    comms_runtime: Arc<dyn CommsRuntime>,
    idle_timeout: Option<Duration>,
) -> crate::tokio::task::JoinHandle<()> {
    let timeout_dur = idle_timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT);
    let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);

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
        // Bridge-command dispatch resolves binding-sensitive admission
        // through generated MeerkatMachine feedback before staging DSL
        // transitions; no shell-local mirror.

        loop {
            // Register the waiter BEFORE draining. `notify_waiters()` only wakes
            // waiters that are already registered, and a `Notified` future is not
            // registered until it is first polled — so we pin and `enable()` it
            // here. Without the `enable()`, an inbox admission whose
            // `notify_waiters()` fires in the window between the drain returning
            // empty and the await first polling `notified` is lost; with the mob
            // `Duration::MAX` idle timeout that stalls the drain forever (a peer
            // message reported "sent" but never drained, e.g. a readout leg that
            // arrives after the recipient went idle).
            let mut notified = std::pin::pin!(inbox_notify.notified());
            notified.as_mut().enable();

            // Drain the CLASSIFIED path directly so a classification fault stays
            // a typed error instead of being laundered into an empty candidate
            // vec (the old `drain_peer_input_candidates().await` collapsed the
            // fallible drain with `.unwrap_or_default()`, making an error
            // indistinguishable from an idle inbox and triggering the
            // idle/dismiss lifecycle branch below). Empty-because-error must be
            // distinguishable from empty-because-idle before deciding to idle.
            let candidates = match comms_runtime.drain_classified_inbox_interactions().await {
                Ok(candidates) => candidates,
                Err(err) => {
                    // A classified-drain fault is NOT an idle inbox. Fail closed
                    // with the typed `Failed` terminal owned by the drain
                    // lifecycle authority rather than silently idling/dismissing.
                    tracing::error!(
                        session_id = %session_id,
                        error = %err,
                        "comms_drain: classified inbox drain failed; exiting via typed Failed terminal (not idle/dismiss)"
                    );
                    if let Err(notify_err) = adapter
                        .notify_comms_drain_exited(&session_id, DrainExitReason::Failed)
                        .await
                    {
                        // Detached-task boundary: the typed control fault has
                        // nowhere left to propagate; surface it explicitly and
                        // run the projection safety net so slot mechanics
                        // cannot silently diverge from the drain authority.
                        tracing::error!(
                            session_id = %session_id,
                            error = %notify_err,
                            "comms_drain: NotifyDrainExited(Failed) rejected by machine authority"
                        );
                        adapter
                            .project_comms_drain_failed_safety_net(&session_id)
                            .await;
                    }
                    return;
                }
            };
            if candidates.is_empty() {
                // Lifecycle dismissal is NOT driven by a peer message body. A
                // peer-controlled string must never stop this executor; the
                // typed dismissal path is owned by the runtime drain-lifecycle
                // authority, which fires `DrainExitReason::Dismissed` through
                // the handle seam (see `handles::comms_drain`). The drain loop
                // here only honors the idle-timeout terminal below. This branch
                // is reached only on a genuinely empty (Ok) drain, never on a
                // drain error.
                if crate::tokio::time::timeout(timeout_dur, notified.as_mut())
                    .await
                    .is_err()
                {
                    tracing::info!("comms_drain: idle timeout expired, stopping");
                    if let Err(notify_err) = adapter
                        .notify_comms_drain_exited(&session_id, DrainExitReason::IdleTimeout)
                        .await
                    {
                        // Detached-task boundary: surface the typed fault and
                        // run the projection safety net so slot mechanics
                        // cannot silently diverge from the drain authority.
                        tracing::error!(
                            session_id = %session_id,
                            error = %notify_err,
                            "comms_drain: NotifyDrainExited(IdleTimeout) rejected by machine authority"
                        );
                        adapter
                            .project_comms_drain_failed_safety_net(&session_id)
                            .await;
                    }
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
                let candidate_class = candidate.class();
                match candidate_class {
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
                            class = ?candidate_class,
                            lifecycle_peer = ?candidate.lifecycle_peer,
                            "comms_drain: consumed silent peer lifecycle notice"
                        );
                    }
                    PeerInputClass::ResponseProgress | PeerInputClass::ResponseTerminal => {
                        // Response progress/terminal status is fixed by the
                        // ingress classifier and carried on the candidate. Do
                        // not re-match raw `ResponseStatus` here.
                        let terminal_status = match (
                            candidate_class,
                            candidate.response_terminality,
                        ) {
                            (
                                PeerInputClass::ResponseTerminal,
                                Some(meerkat_core::interaction::TerminalityClass::Terminal {
                                    disposition,
                                }),
                            ) => match disposition {
                                meerkat_core::interaction::TerminalDisposition::Completed => {
                                    Some(meerkat_core::handles::PeerTerminalDisposition::Completed)
                                }
                                meerkat_core::interaction::TerminalDisposition::Failed => {
                                    Some(meerkat_core::handles::PeerTerminalDisposition::Failed)
                                }
                                _ => {
                                    reject_peer_response_observation_via_authority(
                                        &comms_runtime,
                                        &candidate,
                                        "unsupported terminal disposition",
                                    );
                                    tracing::warn!(
                                        class = ?candidate_class,
                                        disposition = ?disposition,
                                        interaction_id = %candidate.interaction.id,
                                        "comms_drain: rejected peer response with unsupported terminal disposition"
                                    );
                                    continue;
                                }
                            },
                            (
                                PeerInputClass::ResponseProgress,
                                Some(meerkat_core::interaction::TerminalityClass::Progress),
                            ) => None,
                            (class, terminality) => {
                                reject_peer_response_observation_via_authority(
                                    &comms_runtime,
                                    &candidate,
                                    "missing or inconsistent machine terminality",
                                );
                                tracing::warn!(
                                    class = ?class,
                                    terminality = ?terminality,
                                    interaction_id = %candidate.interaction.id,
                                    "comms_drain: rejected peer response with missing or inconsistent machine terminality"
                                );
                                continue;
                            }
                        };
                        let is_terminal = terminal_status.is_some();

                        if is_terminal {
                            // Terminal response — single admission with
                            // completion tracking. The PeerInput already
                            // carries ResponseTerminal convention which the
                            // generated admission maps to the machine-owned WakeIfIdle
                            // terminal policy.
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
                            // the now-idle stream registry entry. Without a
                            // peer-interaction handle, this drain path is
                            // transport-only for lifecycle purposes and does
                            // not synthesize semantic cleanup.
                            let subscriber = comms_runtime.interaction_subscriber(&interaction_id);
                            let peer_interaction_handle = comms_runtime.peer_interaction_handle();
                            let dsl_installed = peer_interaction_handle.is_some();
                            if let (Some(handle), Some(disposition)) =
                                (peer_interaction_handle.as_ref(), terminal_status)
                            {
                                let corr_id =
                                    meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0);
                                if let Err(err) = handle.response_terminal(corr_id, disposition) {
                                    tracing::warn!(
                                        error = %err,
                                        corr_id = %corr_id,
                                        "PeerInteractionHandle::response_terminal rejected (no DSL entry — classified drain saw an unknown corr_id)"
                                    );
                                    reject_peer_response_observation_via_authority(
                                        &comms_runtime,
                                        &candidate,
                                        "generated peer terminal transition rejected",
                                    );
                                    continue;
                                }
                            }

                            let content_input = match classified_interaction_to_runtime_input(
                                &candidate,
                                &runtime_id,
                            ) {
                                Ok(input) => input,
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        interaction_id = %interaction_id,
                                        "comms_drain: rejected malformed terminal peer ingress"
                                    );
                                    if !dsl_installed {
                                        comms_runtime.mark_interaction_complete(&interaction_id);
                                    }
                                    continue;
                                }
                            };
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
                                    reject_peer_response_observation_via_authority(
                                        &comms_runtime,
                                        &candidate,
                                        "generated peer progress transition rejected",
                                    );
                                    continue;
                                }
                            }

                            let input = match classified_interaction_to_runtime_input(
                                &candidate,
                                &runtime_id,
                            ) {
                                Ok(input) => input,
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        interaction_id = %candidate.interaction.id,
                                        "comms_drain: rejected malformed progress peer ingress"
                                    );
                                    continue;
                                }
                            };
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
                            candidate_class,
                            PeerInputClass::SilentRequest | PeerInputClass::ActionableRequest
                        );
                        if is_inbound_peer_request {
                            let Some(handle) =
                                comms_runtime.peer_request_response_authority_handle()
                            else {
                                tracing::warn!(
                                    interaction_id = %interaction_id,
                                    class = ?candidate_class,
                                    "comms_drain: rejected inbound peer request without complete peer request authority"
                                );
                                comms_runtime.mark_interaction_complete(&interaction_id);
                                continue;
                            };
                            let corr_id =
                                meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0);
                            if handle.inbound_state(corr_id).is_none()
                                && let Err(err) = handle
                                    .request_received(corr_id, candidate.interaction.handling_mode)
                            {
                                tracing::warn!(
                                    error = %err,
                                    corr_id = %corr_id,
                                    "PeerInteractionHandle::request_received rejected"
                                );
                                comms_runtime.mark_interaction_complete(&interaction_id);
                                continue;
                            }
                        }

                        let subscriber = comms_runtime.interaction_subscriber(&interaction_id);
                        let input = match classified_interaction_to_runtime_input(
                            &candidate,
                            &runtime_id,
                        ) {
                            Ok(input) => input,
                            Err(err) => {
                                tracing::warn!(
                                    error = %err,
                                    interaction_id = %interaction_id,
                                    "comms_drain: rejected malformed peer ingress"
                                );
                                comms_runtime.mark_interaction_complete(&interaction_id);
                                continue;
                            }
                        };
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

fn reject_peer_response_observation_via_authority(
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
    reason: &'static str,
) {
    let InteractionContent::Response { in_reply_to, .. } = &candidate.interaction.content else {
        tracing::warn!(
            reason = reason,
            interaction_id = %candidate.interaction.id,
            "comms_drain: cannot reject malformed peer response observation without response correlation"
        );
        return;
    };
    let corr_id = meerkat_core::PeerCorrelationId::from_uuid(in_reply_to.0);
    let Some(handle) = comms_runtime.peer_interaction_handle() else {
        tracing::warn!(
            reason = reason,
            corr_id = %corr_id,
            interaction_id = %candidate.interaction.id,
            "comms_drain: malformed peer response observation has no peer-interaction authority"
        );
        return;
    };
    if let Err(err) = handle.response_rejected(corr_id) {
        tracing::warn!(
            error = %err,
            reason = reason,
            corr_id = %corr_id,
            interaction_id = %candidate.interaction.id,
            "PeerInteractionHandle::response_rejected rejected malformed peer response observation"
        );
    }
}

fn bridge_peer_identity(
    peer: &BridgePeerSpec,
    context: &str,
) -> Result<BridgePeerIdentity, (BridgeRejectionCause, String)> {
    BridgePeerIdentity::try_from(peer).map_err(|error| {
        (
            BridgeRejectionCause::InvalidSupervisorSpec,
            format!("{context}: invalid supervisor peer spec: {error}"),
        )
    })
}

fn sender_peer_label(sender: &PeerIngressFact) -> String {
    sender.diagnostic_label()
}

fn sender_matches_bridge_peer(sender: &PeerIngressFact, peer: &BridgePeerIdentity) -> bool {
    sender
        .canonical_peer_id
        .is_some_and(|sender_peer_id| sender_peer_id == peer.peer_id)
        || (!peer.pubkey.is_zero() && sender.signing_pubkey == Some(*peer.pubkey.as_bytes()))
}

fn generated_sender_peer_id(sender: &PeerIngressFact) -> Option<String> {
    sender.canonical_peer_id.map(|peer_id| peer_id.as_str())
}

fn supervisor_bind_admission_rejection_cause(
    rejection: crate::meerkat_machine::dsl::SupervisorBindRejectionKind,
) -> BridgeRejectionCause {
    match rejection {
        crate::meerkat_machine::dsl::SupervisorBindRejectionKind::AlreadyBound => {
            BridgeRejectionCause::AlreadyBound
        }
        crate::meerkat_machine::dsl::SupervisorBindRejectionKind::SenderMismatch => {
            BridgeRejectionCause::SenderMismatch
        }
    }
}

fn supervisor_bind_admission_rejection_reason(
    rejection: crate::meerkat_machine::dsl::SupervisorBindRejectionKind,
    sender: &PeerIngressFact,
    supervisor: &BridgePeerIdentity,
) -> String {
    match rejection {
        crate::meerkat_machine::dsl::SupervisorBindRejectionKind::AlreadyBound => {
            "bind member admission rejected by MeerkatMachine: supervisor already bound; use authorize_supervisor to rotate"
                .to_string()
        }
        crate::meerkat_machine::dsl::SupervisorBindRejectionKind::SenderMismatch => {
            let sender_label = sender_peer_label(sender);
            format!(
                "bind member admission rejected by MeerkatMachine: request sender '{sender_label}' does not match authorized supervisor '{}'",
                supervisor.peer_id
            )
        }
    }
}

fn supervisor_authorize_admission_rejection_cause(
    rejection: crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind,
) -> BridgeRejectionCause {
    match rejection {
        crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind::NotBound => {
            BridgeRejectionCause::NotBound
        }
        crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind::StaleSupervisor => {
            BridgeRejectionCause::StaleSupervisor
        }
        crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind::SenderMismatch => {
            BridgeRejectionCause::SenderMismatch
        }
    }
}

fn supervisor_authorize_admission_rejection_reason(
    rejection: crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind,
    sender: &PeerIngressFact,
    supervisor: &BridgePeerIdentity,
) -> String {
    match rejection {
        crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind::NotBound => {
            "authorize supervisor admission rejected by MeerkatMachine: use bind_member to establish initial supervisor authority"
                .to_string()
        }
        crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind::StaleSupervisor => {
            "authorize supervisor admission rejected by MeerkatMachine: stale supervisor binding"
                .to_string()
        }
        crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind::SenderMismatch => {
            let sender_label = sender_peer_label(sender);
            format!(
                "authorize supervisor admission rejected by MeerkatMachine: request sender '{sender_label}' does not match authorized supervisor '{}'",
                supervisor.peer_id
            )
        }
    }
}

fn supervisor_bridge_admission_rejection_cause(
    rejection: crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind,
) -> BridgeRejectionCause {
    match rejection {
        crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind::NotBound => {
            BridgeRejectionCause::NotBound
        }
        crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind::StaleSupervisor => {
            BridgeRejectionCause::StaleSupervisor
        }
        crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind::SenderMismatch => {
            BridgeRejectionCause::SenderMismatch
        }
    }
}

fn supervisor_bridge_admission_rejection_reason(
    rejection: crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind,
    sender: &PeerIngressFact,
    supervisor: &BridgePeerIdentity,
) -> String {
    match rejection {
        crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind::NotBound => {
            "supervisor bridge admission rejected by MeerkatMachine: no authorized supervisor registered"
                .to_string()
        }
        crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind::StaleSupervisor => {
            "supervisor bridge admission rejected by MeerkatMachine: stale supervisor binding"
                .to_string()
        }
        crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind::SenderMismatch => {
            let sender_label = sender_peer_label(sender);
            format!(
                "supervisor bridge admission rejected by MeerkatMachine: request sender '{sender_label}' does not match authorized supervisor '{}'",
                supervisor.peer_id
            )
        }
    }
}

struct BridgeMobPeerOverlay {
    endpoints: BTreeSet<crate::meerkat_machine::dsl::PeerEndpoint>,
    endpoint_count: u64,
}

fn bridge_mob_peer_overlay(
    handoff: &BridgeMobPeerOverlayHandoff,
) -> Result<BridgeMobPeerOverlay, String> {
    let mut endpoints = BTreeSet::new();
    let endpoint_count = u64::try_from(handoff.peer_specs.len())
        .map_err(|_| "MobMachine peer overlay contains too many endpoints".to_string())?;
    for peer_spec in &handoff.peer_specs {
        let peer = TrustedPeerDescriptor::try_from(peer_spec)?;
        let endpoint = crate::meerkat_machine::dsl::PeerEndpoint::from(&peer);
        endpoints.insert(endpoint);
    }
    Ok(BridgeMobPeerOverlay {
        endpoints,
        endpoint_count,
    })
}

/// Resolve supervisor bridge command admission through generated
/// MeerkatMachine authority. The shell parses the wire peer spec into typed
/// atoms, but binding/epoch/sender admission and public rejection class come
/// from `SupervisorBridgeCommandAdmissionResolved`.
async fn resolve_authorized_supervisor(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    sender: &PeerIngressFact,
    payload: &BridgeSupervisorPayload,
) -> Result<BridgePeerIdentity, (BridgeRejectionCause, String)> {
    let supervisor = bridge_peer_identity(&payload.supervisor, "supervisor bridge request")?;
    let sender_peer_id = sender
        .canonical_peer_id
        .map(|sender_peer_id| sender_peer_id.to_string());
    match adapter
        .resolve_supervisor_bridge_command_admission(
            session_id,
            supervisor.peer_id.to_string(),
            payload.epoch,
            sender_peer_id,
        )
        .await
    {
        Ok(crate::meerkat_machine::SupervisorBridgeCommandAdmission::Accepted) => Ok(supervisor),
        Ok(crate::meerkat_machine::SupervisorBridgeCommandAdmission::Rejected(rejection)) => Err((
            supervisor_bridge_admission_rejection_cause(rejection),
            supervisor_bridge_admission_rejection_reason(rejection, sender, &supervisor),
        )),
        Err(error) => Err((
            BridgeRejectionCause::Internal,
            format!("supervisor bridge admission failed: {error}"),
        )),
    }
}

/// Gen-authority supervisor admission (PR #714) plus the PR #750 reply-route
/// install: authorize the command through `MeerkatMachine`, then ensure the
/// bound supervisor's response route exists so the `PeerResponse` ack can be
/// routed back to the authenticated sender.
async fn resolve_authorized_supervisor_with_response_route(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    sender: &PeerIngressFact,
    payload: &BridgeSupervisorPayload,
    context: &str,
) -> Result<BridgePeerIdentity, (BridgeRejectionCause, String)> {
    let supervisor = resolve_authorized_supervisor(adapter, session_id, sender, payload).await?;
    let current = adapter.supervisor_binding(session_id).await;
    let binding = BoundSupervisorState::from_binding(&current).ok_or_else(|| {
        (
            BridgeRejectionCause::Internal,
            format!("{context}: missing bound supervisor response state"),
        )
    })?;
    let response_peer = bound_supervisor_response_descriptor(sender, &binding, context)?;
    ensure_bridge_response_route_for_descriptor(
        adapter,
        session_id,
        comms_runtime,
        response_peer,
        context,
    )
    .await?;
    Ok(supervisor)
}

fn bridge_capabilities() -> BridgeCapabilities {
    BridgeCapabilities {
        deliver_member_input: true,
        observe_member: true,
        interrupt_member: true,
        hard_cancel_member: false,
        retire_member: true,
        destroy_member: true,
        wire_member: true,
        unwire_member: true,
        ..BridgeCapabilities::default()
    }
}

fn peer_input_from_delivery_payload(
    session_id: &SessionId,
    sender_peer_id: PeerId,
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
                peer_id: sender_peer_id.as_str(),
                display_identity: Some(sender_peer_id.as_str()),
                runtime_id: Some(MeerkatMachine::logical_runtime_id(session_id)),
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
        handling_mode: Some(payload.handling_mode),
    })
}

fn bridge_delivery_rejection_cause(
    reason: &crate::accept::RejectReason,
) -> BridgeDeliveryRejectionCause {
    match reason {
        crate::accept::RejectReason::NotReady { state } => BridgeDeliveryRejectionCause::NotReady {
            state: runtime_state_to_bridge(*state),
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
        crate::accept::RejectReason::PeerResponseTerminalInvalid { detail } => {
            BridgeDeliveryRejectionCause::Internal {
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
    Err((
        BridgeRejectionCause::InvalidBootstrapToken,
        "runtime does not expose a typed bridge bootstrap token".to_string(),
    ))
}

/// Validate the material `BindMember` admission facts and mirror the
/// MeerkatMachine verdict.
///
/// The material admission verdict (advertised-address match, raw
/// supervisor-peer sender match, expected runtime peer-id match,
/// bootstrap-token match, in that precedence) is a MeerkatMachine-owned
/// wiring fact. The shell extracts the four pure boolean observations it
/// already computes, routes them through the machine, and mirrors the
/// emitted verdict onto the existing typed `(BridgeRejectionCause, reason)`
/// pairs (or the `Ok` payload on `Accept`).
///
/// The two `Internal` checks below — missing advertised bootstrap token and
/// missing advertised address / runtime peer id — are runtime self-integrity
/// invariants, NOT admission verdicts over a machine-owned fact, so they stay
/// shell-side and fail before any machine routing.
async fn validate_bind_request(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    sender: &PeerIngressFact,
    payload: &meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload,
) -> Result<(TrustedPeerDescriptor, String), (BridgeRejectionCause, String)> {
    let expected_bootstrap_token = advertised_bind_bootstrap_token(comms_runtime)?;
    let advertised_address = comms_runtime.advertised_address().ok_or_else(|| {
        (
            BridgeRejectionCause::Internal,
            "runtime does not expose an advertised address for bind bootstrap".to_string(),
        )
    })?;
    let supervisor = bridge_peer_identity(&payload.supervisor, "bind member failed")?;
    // Canonical peer identity must be present to validate the request's
    // expected_peer_id. Missing runtime identity is a typed internal
    // invariant failure, not a silent "skip the check" path — otherwise
    // caller-supplied `expected_peer_id` would never be challenged.
    let Some(actual_peer_id) = comms_runtime.peer_id().map(|peer_id| peer_id.as_str()) else {
        return Err((
            BridgeRejectionCause::Internal,
            "bind member failed: runtime peer_id unavailable".to_string(),
        ));
    };

    // Pure boolean observations the shell hands to the machine. The machine
    // owns the verdict + precedence; the shell only mirrors it.
    let address_matches = canonicalize_bridge_address(&payload.expected_address)
        == canonicalize_bridge_address(&advertised_address);
    let sender_matches_supervisor = sender_matches_bridge_peer(sender, &supervisor);
    let expected_peer_id_matches = actual_peer_id == payload.expected_peer_id;
    let bootstrap_token_matches = payload.bootstrap_token.as_str() == expected_bootstrap_token;

    let verdict = adapter
        .resolve_supervisor_bind_material_admission(
            session_id,
            address_matches,
            sender_matches_supervisor,
            expected_peer_id_matches,
            bootstrap_token_matches,
        )
        .await
        .map_err(|error| {
            (
                BridgeRejectionCause::Internal,
                format!("bind member material admission failed: {error}"),
            )
        })?;

    match verdict {
        crate::meerkat_machine::dsl::SupervisorBindMaterialAdmissionKind::Accept => Ok((
            supervisor.into_trusted_peer_descriptor(),
            advertised_address,
        )),
        crate::meerkat_machine::dsl::SupervisorBindMaterialAdmissionKind::AddressMismatch => Err((
            BridgeRejectionCause::AddressMismatch,
            format!(
                "bind address mismatch: expected '{}', actual '{}'",
                payload.expected_address, advertised_address
            ),
        )),
        crate::meerkat_machine::dsl::SupervisorBindMaterialAdmissionKind::SenderMismatch => {
            let sender_label = sender_peer_label(sender);
            Err((
                BridgeRejectionCause::SenderMismatch,
                format!(
                    "request sender '{sender_label}' does not match supervisor '{}'",
                    supervisor.peer_id
                ),
            ))
        }
        crate::meerkat_machine::dsl::SupervisorBindMaterialAdmissionKind::InvalidPeerSpec => Err((
            BridgeRejectionCause::InvalidPeerSpec,
            format!(
                "bind peer_id mismatch: expected '{}', actual '{actual_peer_id}'",
                payload.expected_peer_id
            ),
        )),
        crate::meerkat_machine::dsl::SupervisorBindMaterialAdmissionKind::InvalidBootstrapToken => {
            Err((
                BridgeRejectionCause::InvalidBootstrapToken,
                "bind member failed: invalid bootstrap token".to_string(),
            ))
        }
    }
}

#[derive(Clone, Debug)]
struct BoundSupervisorState {
    name: String,
    peer_id: String,
    address: String,
    signing_public_key: String,
    epoch: u64,
}

impl BoundSupervisorState {
    /// Project the canonical `SupervisorBinding` authority into the response
    /// descriptor state used to install a temporary bridge reply route.
    fn from_binding(binding: &SupervisorBinding) -> Option<Self> {
        let SupervisorBinding::Bound {
            name,
            peer_id,
            address,
            signing_public_key,
            epoch,
        } = binding
        else {
            return None;
        };
        Some(Self {
            name: name.clone(),
            peer_id: peer_id.clone(),
            address: address.clone(),
            signing_public_key: signing_public_key.clone(),
            epoch: *epoch,
        })
    }
}

pub fn encode_supervisor_signing_public_key(pubkey: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in pubkey {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

pub fn decode_supervisor_signing_public_key(encoded: &str) -> Result<[u8; 32], String> {
    fn hex_nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let bytes = encoded.as_bytes();
    if bytes.len() != 64 {
        return Err(format!(
            "supervisor signing public key must be 64 hex characters, got {}",
            bytes.len()
        ));
    }
    let mut decoded = [0u8; 32];
    for (index, chunk) in bytes.chunks_exact(2).enumerate() {
        let high = hex_nibble(chunk[0]).ok_or_else(|| {
            format!(
                "invalid supervisor signing public key hex at byte {}",
                index * 2
            )
        })?;
        let low = hex_nibble(chunk[1]).ok_or_else(|| {
            format!(
                "invalid supervisor signing public key hex at byte {}",
                index * 2 + 1
            )
        })?;
        decoded[index] = (high << 4) | low;
    }
    Ok(decoded)
}

pub fn trusted_peer_descriptor_from_supervisor_publish_obligation(
    obligation: &crate::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation,
) -> Result<TrustedPeerDescriptor, String> {
    let signing_public_key = obligation.signing_public_key().as_ref().ok_or_else(|| {
        "generated supervisor trust publish obligation omitted signing public key".to_string()
    })?;
    let pubkey = decode_supervisor_signing_public_key(signing_public_key)?;
    TrustedPeerDescriptor::unsigned_with_pubkey(
        obligation.name().clone(),
        obligation.peer_id().clone(),
        pubkey,
        obligation.address().clone(),
    )
}

async fn single_supervisor_publish_obligation(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    transition: &crate::meerkat_machine::dsl::MeerkatMachineTransition,
    context: &str,
) -> Result<crate::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation, String> {
    let freshness_authority = adapter
        .supervisor_trust_publish_freshness_authority(session_id)
        .await
        .map_err(|error| {
            format!("{context}: generated supervisor publish freshness unavailable: {error}")
        })?;
    let obligations = crate::protocol_supervisor_trust_publish::extract_obligations_with_freshness(
        transition,
        freshness_authority,
    );
    match obligations.as_slice() {
        [obligation] => Ok(obligation.clone()),
        [] => Err(format!("{context}: generated publish effect was absent")),
        _ => Err(format!(
            "{context}: generated multiple supervisor trust publish effects"
        )),
    }
}

async fn matching_supervisor_revoke_obligation(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    transition: &crate::meerkat_machine::dsl::MeerkatMachineTransition,
    peer_id: &str,
    epoch: u64,
    context: &str,
) -> Result<crate::protocol_supervisor_trust_revoke::SupervisorTrustRevokeObligation, String> {
    let freshness_authority = adapter
        .supervisor_trust_revoke_freshness_authority(session_id)
        .await
        .map_err(|error| {
            format!("{context}: generated supervisor revoke freshness unavailable: {error}")
        })?;
    crate::protocol_supervisor_trust_revoke::extract_obligations_with_freshness(
        transition,
        freshness_authority,
    )
    .into_iter()
    .find(|obligation| obligation.peer_id().as_str() == peer_id && obligation.epoch() == epoch)
    .ok_or_else(|| format!("{context}: generated revoke effect was absent"))
}

fn validate_supervisor_publish_obligation(
    obligation: &crate::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation,
    expected: &TrustedPeerDescriptor,
    expected_epoch: u64,
    context: &str,
) -> Result<(), String> {
    let expected_signing_key = encode_supervisor_signing_public_key(expected.pubkey);
    if obligation.name() != expected.name.as_str()
        || obligation.peer_id().as_str() != expected.peer_id.as_str()
        || obligation.address().as_str() != expected.address.to_string()
        || obligation.signing_public_key().as_deref() != Some(expected_signing_key.as_str())
        || obligation.epoch() != expected_epoch
    {
        return Err(format!(
            "{context}: generated publish obligation did not match the staged supervisor binding"
        ));
    }
    Ok(())
}

async fn apply_generated_private_trust_add(
    comms_runtime: &Arc<dyn CommsRuntime>,
    peer: TrustedPeerDescriptor,
    authority: CommsTrustMutationAuthority,
) -> Result<(), String> {
    match comms_runtime
        .apply_trust_mutation(CommsTrustMutation::AddPrivateTrustedPeer { peer, authority })
        .await
        .map_err(|error| error.to_string())?
    {
        CommsTrustMutationResult::Added { .. } => Ok(()),
        other => Err(format!(
            "generated trust add returned unexpected result: {other:?}"
        )),
    }
}

async fn apply_generated_private_trust_remove(
    comms_runtime: &Arc<dyn CommsRuntime>,
    peer_id: String,
    authority: CommsTrustMutationAuthority,
) -> Result<bool, String> {
    match comms_runtime
        .apply_trust_mutation(CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority })
        .await
        .map_err(|error| error.to_string())?
    {
        CommsTrustMutationResult::Removed { removed } => Ok(removed),
        other => Err(format!(
            "generated trust remove returned unexpected result: {other:?}"
        )),
    }
}

fn supervisor_publish_authority(
    obligation: &crate::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation,
) -> Result<CommsTrustMutationAuthority, String> {
    crate::protocol_supervisor_trust_publish::publish_authority_for_peer(
        obligation,
        obligation.peer_id(),
    )
}

fn supervisor_publish_cleanup_authority(
    obligation: &crate::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation,
) -> Result<CommsTrustMutationAuthority, String> {
    crate::protocol_supervisor_trust_publish::cleanup_authority_for_peer(
        obligation,
        obligation.peer_id(),
    )
}

fn supervisor_revoke_authority(
    obligation: &crate::protocol_supervisor_trust_revoke::SupervisorTrustRevokeObligation,
) -> Result<CommsTrustMutationAuthority, String> {
    crate::protocol_supervisor_trust_revoke::revoke_authority_for_peer(
        obligation,
        obligation.peer_id(),
    )
}

async fn publish_supervisor_trust_from_generated_obligation(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    obligation: &crate::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation,
) -> Result<(), String> {
    let trusted_peer = trusted_peer_descriptor_from_supervisor_publish_obligation(obligation)?;
    let cleanup_authority = supervisor_publish_cleanup_authority(obligation)?;
    apply_generated_private_trust_add(
        comms_runtime,
        trusted_peer,
        supervisor_publish_authority(obligation)?,
    )
    .await?;
    if let Err(error) = adapter
        .stage_supervisor_trust_published(
            session_id,
            obligation.peer_id().clone(),
            obligation.epoch(),
        )
        .await
    {
        let cleanup_result = apply_generated_private_trust_remove(
            comms_runtime,
            obligation.peer_id().clone(),
            cleanup_authority,
        )
        .await;
        let mut reason = format!("generated trust publish ack rejected: {error}");
        if let Err(cleanup_error) = cleanup_result {
            reason.push_str(&format!("; cleanup remove failed: {cleanup_error}"));
        }
        return Err(reason);
    }
    Ok(())
}

async fn bind_and_publish_supervisor_trust(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    supervisor: &TrustedPeerDescriptor,
    epoch: u64,
    context: &str,
) -> Result<(), String> {
    adapter
        .stage_local_endpoint_for_comms_runtime(session_id, comms_runtime.as_ref())
        .await
        .map_err(|error| format!("{context}: local endpoint rejected: {error}"))?;
    let transition = adapter
        .stage_supervisor_bind(
            session_id,
            supervisor.name.as_str().to_owned(),
            supervisor.peer_id.as_str(),
            supervisor.address.to_string(),
            encode_supervisor_signing_public_key(supervisor.pubkey),
            epoch,
        )
        .await
        .map_err(|error| format!("{context}: DSL rejected bind: {error}"))?;
    let obligation =
        single_supervisor_publish_obligation(adapter, session_id, &transition, context).await?;
    validate_supervisor_publish_obligation(&obligation, supervisor, epoch, context)?;
    publish_supervisor_trust_from_generated_obligation(
        adapter,
        session_id,
        comms_runtime,
        &obligation,
    )
    .await
    .map_err(|error| format!("{context}: trust publication failed: {error}"))
}

/// Re-install the PRIVATE supervisor admission trust edge for the current
/// binding, mirroring the Bootstrap path (`bind_and_publish_supervisor_trust`).
///
/// Used by the BindMember idempotent-ack repair: the DSL already holds the
/// supervisor binding, but the runtime's private admission edge may be missing
/// (e.g. after a transport rebind). `RequestSupervisorTrustPublish` restates the
/// current binding and emits a generated publish obligation whose authority
/// owner token derives from the session dsl authority; the obligation is spent
/// as a PRIVATE add. The supervisor is a control-plane edge and MUST NOT surface
/// in the public peer directory (see `add_private_trusted_peer` doc).
async fn republish_private_supervisor_trust(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    supervisor: &TrustedPeerDescriptor,
) -> Result<(), String> {
    adapter
        .stage_local_endpoint_for_comms_runtime(session_id, comms_runtime.as_ref())
        .await
        .map_err(|error| format!("local endpoint rejected: {error}"))?;
    let obligation = supervisor_route_publish_obligation(
        adapter,
        session_id,
        supervisor,
        "bind member idempotent ack trust repair",
    )
    .await?;
    publish_supervisor_trust_from_generated_obligation(
        adapter,
        session_id,
        comms_runtime,
        &obligation,
    )
    .await
}

fn trusted_peer_descriptor_from_bound_supervisor_state(
    previous: &BoundSupervisorState,
) -> Result<TrustedPeerDescriptor, String> {
    let pubkey = decode_supervisor_signing_public_key(&previous.signing_public_key)?;
    TrustedPeerDescriptor::unsigned_with_pubkey(
        previous.name.clone(),
        previous.peer_id.clone(),
        pubkey,
        previous.address.clone(),
    )
}

async fn rollback_bind_after_trust_publication_failure(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    peer_id: &str,
    epoch: u64,
) -> Result<(), SupervisorBindingStageError> {
    let transition = adapter
        .stage_supervisor_revoke(session_id, peer_id.to_string(), epoch)
        .await?;
    if let Some(obligation) =
        crate::protocol_supervisor_trust_revoke::extract_obligations(&transition)
            .into_iter()
            .find(|obligation| {
                obligation.peer_id().as_str() == peer_id && obligation.epoch() == epoch
            })
    {
        adapter
            .stage_supervisor_trust_revoked(
                session_id,
                obligation.peer_id().clone(),
                obligation.epoch(),
            )
            .await?;
    }
    Ok(())
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
            previous.signing_public_key.clone(),
            previous.epoch,
        )
        .await
        .map(|_| ())
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
    let to = match resolve_bridge_response_route(comms_runtime, candidate).await {
        Some(route) => route,
        None => {
            tracing::warn!(
                from = %candidate.ingress.diagnostic_label(),
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
            blocks: None,
            handling_mode: None,
        })
        .await
    {
        tracing::warn!(
            from = %candidate.ingress.diagnostic_label(),
            interaction_id = %candidate.interaction.id,
            error = %error,
            "comms_drain: failed to send bridge response"
        );
    }
    comms_runtime.mark_interaction_complete(&candidate.interaction.id);
}

async fn resolve_bridge_response_route(
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
) -> Option<PeerRoute> {
    if let Some(sender_route) = candidate.ingress.route.clone() {
        if let Some(route) = resolve_peer_route(comms_runtime, sender_route.peer_id).await {
            return Some(route);
        }
        return Some(sender_route);
    }

    if let Some(sender_peer_id) = candidate.ingress.canonical_peer_id {
        return resolve_peer_route(comms_runtime, sender_peer_id)
            .await
            .or_else(|| Some(PeerRoute::new(sender_peer_id)));
    }
    None
}

async fn resolve_peer_route(
    comms_runtime: &Arc<dyn CommsRuntime>,
    peer_id: PeerId,
) -> Option<PeerRoute> {
    let peers = comms_runtime.peers().await;
    peers
        .iter()
        .find(|entry| entry.peer_id == peer_id)
        .map(|entry| PeerRoute::with_display_name(entry.peer_id, entry.name.clone()))
}

fn record_bridge_inbound_peer_request(
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
) -> bool {
    let Some(handle) = comms_runtime.peer_request_response_authority_handle() else {
        tracing::warn!(
            interaction_id = %candidate.interaction.id,
            "comms_drain: rejected supervisor bridge request without complete peer request authority"
        );
        comms_runtime.mark_interaction_complete(&candidate.interaction.id);
        return false;
    };
    let corr_id = meerkat_core::PeerCorrelationId::from_uuid(candidate.interaction.id.0);
    if handle.inbound_state(corr_id).is_some() {
        return true;
    }
    if let Err(err) = handle.request_received(corr_id, candidate.interaction.handling_mode) {
        tracing::warn!(
            error = %err,
            corr_id = %corr_id,
            "PeerInteractionHandle::request_received rejected for supervisor bridge command"
        );
        comms_runtime.mark_interaction_complete(&candidate.interaction.id);
        return false;
    }
    true
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

async fn ensure_bridge_response_route_for_supervisor(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    supervisor: &BridgePeerIdentity,
    context: &str,
) -> Result<(), (BridgeRejectionCause, String)> {
    ensure_bridge_response_route_for_descriptor(
        adapter,
        session_id,
        comms_runtime,
        supervisor.clone().into_trusted_peer_descriptor(),
        context,
    )
    .await
}

async fn ensure_bridge_response_route_for_descriptor(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    descriptor: TrustedPeerDescriptor,
    context: &str,
) -> Result<(), (BridgeRejectionCause, String)> {
    let obligation = supervisor_route_publish_obligation(adapter, session_id, &descriptor, context)
        .await
        .map_err(|reason| (BridgeRejectionCause::Internal, reason))?;
    let authority = supervisor_publish_authority(&obligation)
        .map_err(|reason| (BridgeRejectionCause::Internal, reason))?;
    apply_generated_private_trust_add(comms_runtime, descriptor, authority)
        .await
        .map_err(|error| {
            (
                BridgeRejectionCause::Internal,
                format!("{context}: failed to install supervisor response route: {error}"),
            )
        })
}

fn bound_supervisor_response_descriptor(
    sender: &PeerIngressFact,
    binding: &BoundSupervisorState,
    context: &str,
) -> Result<TrustedPeerDescriptor, (BridgeRejectionCause, String)> {
    let pubkey = sender.signing_pubkey.ok_or_else(|| {
        (
            BridgeRejectionCause::Internal,
            format!("{context}: authenticated supervisor sender pubkey unavailable"),
        )
    })?;
    TrustedPeerDescriptor::unsigned_with_pubkey(
        binding.name.clone(),
        &binding.peer_id,
        pubkey,
        &binding.address,
    )
    .map_err(|error| {
        (
            BridgeRejectionCause::Internal,
            format!("{context}: invalid supervisor response route: {error}"),
        )
    })
}

/// Install a temporary PRIVATE supervisor response route, send the reply over
/// it, then remove it.
///
/// The supervisor's trust edge was revoked before the reply (revoke /
/// rotation-to-a-new-supervisor), so the router can no longer dial the original
/// supervisor. The caller captures a generated (add, remove) authority pair
/// from a single supervisor-publish obligation WHILE THE BINDING WAS STILL
/// PRESENT (the obligation's owner token derives from the session dsl
/// authority). This reinstalls the private admission edge just long enough to
/// route the terminal ack, then removes it so no stale control-plane edge
/// survives the reply.
async fn send_bridge_response_with_temporary_supervisor_route(
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
    status: meerkat_core::interaction::ResponseStatus,
    reply: BridgeReply,
    response_peer: TrustedPeerDescriptor,
    add_authority: CommsTrustMutationAuthority,
    remove_authority: CommsTrustMutationAuthority,
) {
    let removal_key = response_peer.peer_id.to_string();
    match apply_generated_private_trust_add(comms_runtime, response_peer, add_authority).await {
        Ok(()) => {
            send_bridge_response(comms_runtime, candidate, status, reply).await;
            if let Err(error) = apply_generated_private_trust_remove(
                comms_runtime,
                removal_key.clone(),
                remove_authority,
            )
            .await
            {
                tracing::warn!(
                    error = %error,
                    peer_id = %removal_key,
                    "comms_drain: failed to remove temporary supervisor response route"
                );
            }
        }
        Err(reason) => {
            tracing::warn!(
                reason = %reason,
                peer_id = %removal_key,
                "comms_drain: failed to install temporary supervisor response route"
            );
            comms_runtime.mark_interaction_complete(&candidate.interaction.id);
        }
    }
}

/// Stage a generated `RequestSupervisorTrustPublish` restating the CURRENT
/// binding and return the single publish obligation it emits. The obligation's
/// authority owner token derives from the session dsl authority, so a
/// trust-mutation authority minted from it validates against a runtime carrying
/// the same MeerkatMachine owner token.
async fn supervisor_route_publish_obligation(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    descriptor: &TrustedPeerDescriptor,
    context: &str,
) -> Result<crate::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation, String> {
    let binding = adapter.supervisor_binding(session_id).await;
    let bound = BoundSupervisorState::from_binding(&binding).ok_or_else(|| {
        format!("{context}: supervisor binding absent for response-route authority")
    })?;
    let transition = adapter
        .stage_supervisor_trust_publish_request(
            session_id,
            bound.name.clone(),
            bound.peer_id.clone(),
            bound.address.clone(),
            bound.signing_public_key.clone(),
            bound.epoch,
        )
        .await
        .map_err(|error| {
            format!("{context}: generated supervisor route publish request rejected: {error}")
        })?;
    let obligation =
        single_supervisor_publish_obligation(adapter, session_id, &transition, context).await?;
    if obligation.peer_id().as_str() != descriptor.peer_id.as_str() {
        return Err(format!(
            "{context}: generated supervisor route publish obligation peer_id mismatch"
        ));
    }
    Ok(obligation)
}

/// Mint a PRIVATE (add, remove) authority pair for a temporary supervisor
/// response route from a single generated publish obligation restating the
/// current binding. Must be called WHILE the binding is still present.
async fn generated_temporary_supervisor_route_authority_pair(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    descriptor: &TrustedPeerDescriptor,
    context: &str,
) -> Result<(CommsTrustMutationAuthority, CommsTrustMutationAuthority), String> {
    let obligation =
        supervisor_route_publish_obligation(adapter, session_id, descriptor, context).await?;
    let add = supervisor_publish_authority(&obligation)?;
    let remove = supervisor_publish_cleanup_authority(&obligation)?;
    Ok((add, remove))
}

/// Try to handle a supervisor bridge command from an incoming comms request.
///
/// Returns `true` if the candidate was a bridge command (handled or rejected),
/// `false` if it was not a bridge command and should be processed normally.
///
/// Wave 3 D Row 21: the authorized-supervisor binding and bridge admission
/// classes live in DSL state. Bridge dispatch routes binding-sensitive
/// decisions through generated admission inputs before emitting wire replies
/// or staging supervisor binding transitions.
async fn try_handle_supervisor_bridge_command(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
) -> bool {
    let InteractionContent::Request { intent, params, .. } = &candidate.interaction.content else {
        return false;
    };

    // All supervisor bridge commands arrive under a single typed intent
    // (`SUPERVISOR_BRIDGE_INTENT`); per-command intents were never part of
    // the wire contract. Any other intent is left for other dispatchers.
    if intent != SUPERVISOR_BRIDGE_INTENT {
        return false;
    }

    // Bridge commands are handled before the generic drain branch that records
    // inbound peer-request state, but their replies are semantic PeerResponses.
    // Treat the bridge pre-dispatch as the same authority boundary: if the
    // inbound request cannot be recorded under complete peer request/response
    // authority, do not decode or apply bridge side effects.
    if !record_bridge_inbound_peer_request(comms_runtime, candidate) {
        return true;
    }

    let command: BridgeCommand = match decode_bridge_command(params.clone()) {
        Ok(cmd) => cmd,
        Err(error) => {
            let cause = match &error {
                meerkat_contracts::wire::supervisor_bridge::BridgeCommandDecodeError::UnsupportedProtocolVersion(_) => {
                    BridgeRejectionCause::UnsupportedProtocolVersion
                }
                meerkat_contracts::wire::supervisor_bridge::BridgeCommandDecodeError::Invalid(_) => {
                    BridgeRejectionCause::Unsupported
                }
            };
            send_bridge_failure(
                comms_runtime,
                candidate,
                cause,
                format!("invalid bridge command: {error}"),
            )
            .await;
            return true;
        }
    };

    let sender = &candidate.ingress;

    match command {
        BridgeCommand::BindMember(payload) => {
            let supervisor = match bridge_peer_identity(&payload.supervisor, "bind member failed") {
                Ok(supervisor) => supervisor,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                    return true;
                }
            };
            let admission = match adapter
                .resolve_supervisor_bind_admission(
                    session_id,
                    supervisor.peer_id.as_str().clone(),
                    payload.epoch,
                    generated_sender_peer_id(sender),
                )
                .await
            {
                Ok(admission) => admission,
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("bind member admission failed: {error}"),
                    )
                    .await;
                    return true;
                }
            };
            match admission {
                SupervisorBindAdmission::IdempotentAck => {
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
                    let Some(peer_id) = comms_runtime.peer_id().map(|peer_id| peer_id.as_str())
                    else {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            "idempotent ack invariant violated",
                        )
                        .await;
                        return true;
                    };
                    let supervisor_spec =
                        match TrustedPeerDescriptor::try_from(payload.supervisor.clone()) {
                            Ok(spec) => spec,
                            Err(error) => {
                                send_bridge_failure(
                                    comms_runtime,
                                    candidate,
                                    BridgeRejectionCause::InvalidSupervisorSpec,
                                    format!(
                                        "bind member failed: invalid supervisor peer spec: {error}"
                                    ),
                                )
                                .await;
                                return true;
                            }
                        };
                    if let Err(error) = republish_private_supervisor_trust(
                        adapter,
                        session_id,
                        comms_runtime,
                        &supervisor_spec,
                    )
                    .await
                    {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!(
                                "bind member failed: trust repair failed before idempotent ack: {error}"
                            ),
                        )
                        .await;
                        return true;
                    }
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
                SupervisorBindAdmission::Bootstrap => {}
                SupervisorBindAdmission::Rejected(rejection) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        supervisor_bind_admission_rejection_cause(rejection),
                        supervisor_bind_admission_rejection_reason(rejection, sender, &supervisor),
                    )
                    .await;
                    return true;
                }
            }
            let (supervisor_spec, advertised_address) =
                match validate_bind_request(adapter, session_id, comms_runtime, sender, &payload)
                    .await
                {
                    Ok(binding) => binding,
                    Err((cause, reason)) => {
                        send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                        return true;
                    }
                };
            // Canonical identity only: the BindMember reply echoes
            // the runtime's own peer ID, never `payload.expected_peer_id`.
            // A caller-supplied identity can't cross the canonical
            // boundary — if the runtime cannot produce its own
            // identity, that is a typed internal invariant failure,
            // not a silent fallback.
            let Some(peer_id) = comms_runtime.peer_id().map(|peer_id| peer_id.as_str()) else {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    "bind member failed: runtime peer_id unavailable",
                )
                .await;
                return true;
            };
            if let Err(error) = adapter
                .stage_local_endpoint_for_comms_runtime(session_id, comms_runtime.as_ref())
                .await
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("bind member failed: local endpoint rejected: {error}"),
                )
                .await;
                return true;
            }
            // DSL owns the authorization discriminant + identity + epoch.
            // Generated bind admission already emitted `Bootstrap`, so this
            // stage should never be rejected; if the DSL rejects anyway,
            // treat as internal.
            let bind_transition = match adapter
                .stage_supervisor_bind(
                    session_id,
                    supervisor_spec.name.as_str().to_owned(),
                    supervisor_spec.peer_id.as_str(),
                    supervisor_spec.address.to_string(),
                    encode_supervisor_signing_public_key(supervisor_spec.pubkey),
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
                Ok(transition) => transition,
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("bind member failed: DSL rejected binding: {error}"),
                    )
                    .await;
                    return true;
                }
            };
            let publish_obligation = match single_supervisor_publish_obligation(
                adapter,
                session_id,
                &bind_transition,
                "bind member",
            )
            .await
            {
                Ok(obligation) => obligation,
                Err(error) => {
                    let _ = rollback_bind_after_trust_publication_failure(
                        adapter,
                        session_id,
                        &supervisor_spec.peer_id.as_str(),
                        payload.epoch,
                    )
                    .await;
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        error,
                    )
                    .await;
                    return true;
                }
            };
            if let Err(error) = validate_supervisor_publish_obligation(
                &publish_obligation,
                &supervisor_spec,
                payload.epoch,
                "bind member",
            ) {
                let _ = rollback_bind_after_trust_publication_failure(
                    adapter,
                    session_id,
                    publish_obligation.peer_id(),
                    publish_obligation.epoch(),
                )
                .await;
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    error,
                )
                .await;
                return true;
            }
            if let Err(error) = publish_supervisor_trust_from_generated_obligation(
                adapter,
                session_id,
                comms_runtime,
                &publish_obligation,
            )
            .await
            {
                let _ = adapter
                    .stage_supervisor_trust_publish_failed(
                        session_id,
                        publish_obligation.peer_id().clone(),
                        publish_obligation.epoch(),
                        error.clone(),
                    )
                    .await;
                let reason = match rollback_bind_after_trust_publication_failure(
                    adapter,
                    session_id,
                    publish_obligation.peer_id(),
                    publish_obligation.epoch(),
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
            let supervisor =
                match bridge_peer_identity(&payload.supervisor, "authorize supervisor failed") {
                    Ok(supervisor) => supervisor,
                    Err((cause, reason)) => {
                        send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                        return true;
                    }
                };
            let previous_binding = match adapter
                .resolve_supervisor_authorize_admission(
                    session_id,
                    supervisor.peer_id.as_str().clone(),
                    payload.epoch,
                    generated_sender_peer_id(sender),
                )
                .await
            {
                Ok(SupervisorAuthorizeAdmission::IdempotentAck) => {
                    // PR #750 response-route fix grafted onto gen-authority admission:
                    // install the supervisor response route before acking so the
                    // PeerResponse can be routed back to the bound supervisor.
                    if let Err((cause, reason)) = ensure_bridge_response_route_for_supervisor(
                        adapter,
                        session_id,
                        comms_runtime,
                        &supervisor,
                        "authorize supervisor failed",
                    )
                    .await
                    {
                        send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                        return true;
                    }
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                    )
                    .await;
                    return true;
                }
                Ok(SupervisorAuthorizeAdmission::Proceed(previous)) => BoundSupervisorState {
                    name: previous.name,
                    peer_id: previous.peer_id,
                    address: previous.address,
                    signing_public_key: previous.signing_public_key,
                    epoch: previous.epoch,
                },
                Ok(SupervisorAuthorizeAdmission::Rejected(rejection)) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        supervisor_authorize_admission_rejection_cause(rejection),
                        supervisor_authorize_admission_rejection_reason(
                            rejection,
                            sender,
                            &supervisor,
                        ),
                    )
                    .await;
                    return true;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("authorize supervisor admission failed: {error}"),
                    )
                    .await;
                    return true;
                }
            };
            let previous_response_peer = match bound_supervisor_response_descriptor(
                sender,
                &previous_binding,
                "authorize supervisor failed",
            ) {
                Ok(descriptor) => descriptor,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                    return true;
                }
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
                .stage_local_endpoint_for_comms_runtime(session_id, comms_runtime.as_ref())
                .await
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("authorize supervisor failed: local endpoint rejected: {error}"),
                )
                .await;
                return true;
            }
            let new_peer_id = supervisor_spec.peer_id.as_str();
            // For a supervisor REPLACEMENT (different peer), capture the generated
            // PRIVATE (add, remove) authority pair for the temporary reply route
            // to the PREVIOUS supervisor BEFORE the rotation revokes it —
            // `RequestSupervisorTrustPublish` requires a live binding, and the
            // router can only dial a peer that holds a trust-store address entry.
            // In-place rotation keeps the existing route, so no capture is needed.
            let rotation_reply_route_authority = if previous_binding.peer_id == new_peer_id {
                None
            } else {
                match generated_temporary_supervisor_route_authority_pair(
                    adapter,
                    session_id,
                    &previous_response_peer,
                    "authorize supervisor failed",
                )
                .await
                {
                    Ok(pair) => Some(pair),
                    Err(reason) => {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            reason,
                        )
                        .await;
                        return true;
                    }
                }
            };
            if previous_binding.peer_id == new_peer_id {
                let authorize_transition = match adapter
                    .stage_supervisor_authorize(
                        session_id,
                        supervisor_spec.name.as_str().to_owned(),
                        new_peer_id.clone(),
                        supervisor_spec.address.to_string(),
                        encode_supervisor_signing_public_key(supervisor_spec.pubkey),
                        payload.epoch,
                    )
                    .await
                {
                    Ok(transition) => transition,
                    Err(error) => {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!("authorize supervisor failed: DSL rejected rotation: {error}"),
                        )
                        .await;
                        return true;
                    }
                };
                let publish_obligation = match single_supervisor_publish_obligation(
                    adapter,
                    session_id,
                    &authorize_transition,
                    "authorize supervisor",
                )
                .await
                {
                    Ok(obligation) => obligation,
                    Err(error) => {
                        let _ = rollback_authorize_after_trust_publication_failure(
                            adapter,
                            session_id,
                            &previous_binding,
                        )
                        .await;
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            error,
                        )
                        .await;
                        return true;
                    }
                };
                if let Err(error) = validate_supervisor_publish_obligation(
                    &publish_obligation,
                    &supervisor_spec,
                    payload.epoch,
                    "authorize supervisor",
                )
                .and_then(|()| {
                    trusted_peer_descriptor_from_supervisor_publish_obligation(&publish_obligation)
                        .map(|_| ())
                }) {
                    let _ = rollback_authorize_after_trust_publication_failure(
                        adapter,
                        session_id,
                        &previous_binding,
                    )
                    .await;
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        error,
                    )
                    .await;
                    return true;
                }
                if let Err(error) = publish_supervisor_trust_from_generated_obligation(
                    adapter,
                    session_id,
                    comms_runtime,
                    &publish_obligation,
                )
                .await
                {
                    let _ = adapter
                        .stage_supervisor_trust_publish_failed(
                            session_id,
                            publish_obligation.peer_id().clone(),
                            publish_obligation.epoch(),
                            error.clone(),
                        )
                        .await;
                    let rollback = rollback_authorize_after_trust_publication_failure(
                        adapter,
                        session_id,
                        &previous_binding,
                    )
                    .await;
                    let mut reason =
                        format!("authorize supervisor failed: trust publication failed: {error}");
                    if let Err(rollback_error) = rollback {
                        reason.push_str(&format!("; rollback failed: {rollback_error}"));
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
            } else {
                let revoke_transition = match adapter
                    .stage_supervisor_revoke(
                        session_id,
                        previous_binding.peer_id.clone(),
                        previous_binding.epoch,
                    )
                    .await
                {
                    Ok(transition) => transition,
                    Err(error) => {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!("authorize supervisor failed: DSL rejected previous supervisor revoke: {error}"),
                        )
                        .await;
                        return true;
                    }
                };
                let revoke_obligation = match matching_supervisor_revoke_obligation(
                    adapter,
                    session_id,
                    &revoke_transition,
                    &previous_binding.peer_id,
                    previous_binding.epoch,
                    "authorize supervisor",
                )
                .await
                {
                    Ok(obligation) => obligation,
                    Err(error) => {
                        let _ = adapter
                            .stage_supervisor_trust_revoke_failed(
                                session_id,
                                previous_binding.peer_id.clone(),
                                previous_binding.epoch,
                                error.clone(),
                            )
                            .await;
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            error,
                        )
                        .await;
                        return true;
                    }
                };
                if let Err(error) = apply_generated_private_trust_remove(
                    comms_runtime,
                    revoke_obligation.peer_id().clone(),
                    match supervisor_revoke_authority(&revoke_obligation) {
                        Ok(authority) => authority,
                        Err(error) => {
                            let _ = adapter
                                .stage_supervisor_trust_revoke_failed(
                                    session_id,
                                    revoke_obligation.peer_id().clone(),
                                    revoke_obligation.epoch(),
                                    error.clone(),
                                )
                                .await;
                            send_bridge_failure(
                                comms_runtime,
                                candidate,
                                BridgeRejectionCause::Internal,
                                error,
                            )
                            .await;
                            return true;
                        }
                    },
                )
                .await
                {
                    let feedback = adapter
                        .stage_supervisor_trust_revoke_failed(
                            session_id,
                            revoke_obligation.peer_id().clone(),
                            revoke_obligation.epoch(),
                            error.clone(),
                        )
                        .await;
                    let mut reason = format!(
                        "authorize supervisor failed: previous supervisor trust removal failed: {error}"
                    );
                    if let Err(feedback_error) = feedback {
                        reason.push_str(&format!("; revoke feedback failed: {feedback_error}"));
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
                if let Err(error) = adapter
                    .stage_supervisor_trust_revoked(
                        session_id,
                        revoke_obligation.peer_id().clone(),
                        revoke_obligation.epoch(),
                    )
                    .await
                {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("authorize supervisor failed: generated revoke feedback rejected: {error}"),
                    )
                    .await;
                    return true;
                }
                if let Err(error) = bind_and_publish_supervisor_trust(
                    adapter,
                    session_id,
                    comms_runtime,
                    &supervisor_spec,
                    payload.epoch,
                    "authorize supervisor",
                )
                .await
                {
                    let _ = rollback_bind_after_trust_publication_failure(
                        adapter,
                        session_id,
                        &new_peer_id,
                        payload.epoch,
                    )
                    .await;
                    let restore = match trusted_peer_descriptor_from_bound_supervisor_state(
                        &previous_binding,
                    ) {
                        Ok(previous_spec) => {
                            bind_and_publish_supervisor_trust(
                                adapter,
                                session_id,
                                comms_runtime,
                                &previous_spec,
                                previous_binding.epoch,
                                "authorize supervisor rollback",
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    };
                    let mut reason =
                        format!("authorize supervisor failed: new supervisor bind failed: {error}");
                    if let Err(restore_error) = restore {
                        reason.push_str(&format!(
                            "; previous supervisor restore failed: {restore_error}"
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
            }
            // PR #750 response-route fix grafted onto gen-authority rotation:
            // reply over the same condition that drove the action. Rotating the
            // supervisor in place leaves the (private) response route intact;
            // replacing it revoked the previous route, so reply via a temporary
            // PRIVATE route built from the authenticated sender + previous binding
            // descriptor, using the authority pair captured before the revoke
            // unbound the previous supervisor.
            if previous_binding.peer_id == new_peer_id {
                send_bridge_response(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Ack(BridgeAck { ok: true }),
                )
                .await;
            } else {
                let Some((add_authority, remove_authority)) = rotation_reply_route_authority else {
                    // Unreachable: the authority pair is always captured for the
                    // different-peer branch above. Fail closed rather than panic.
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        "rotation reply route authority missing".to_string(),
                    )
                    .await;
                    return true;
                };
                send_bridge_response_with_temporary_supervisor_route(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Ack(BridgeAck { ok: true }),
                    previous_response_peer,
                    add_authority,
                    remove_authority,
                )
                .await;
            }
            true
        }
        BridgeCommand::RevokeSupervisor(payload) => {
            let authorized_supervisor =
                match resolve_authorized_supervisor(adapter, session_id, sender, &payload).await {
                    Ok(supervisor) => supervisor,
                    Err((cause, reason)) => {
                        send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                        return true;
                    }
                };
            if let Err(error) = adapter
                .stage_local_endpoint_for_comms_runtime(session_id, comms_runtime.as_ref())
                .await
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("revoke supervisor failed: local endpoint rejected: {error}"),
                )
                .await;
                return true;
            }
            let supervisor_peer_id = authorized_supervisor.peer_id.as_str();
            // Capture the generated PRIVATE (add, remove) authority pair for the
            // temporary reply route BEFORE the revoke unbinds the supervisor —
            // `RequestSupervisorTrustPublish` requires a live binding, and the
            // router can only dial a peer with a trust-store address entry.
            let revoke_reply_route_authority = {
                let response_descriptor =
                    authorized_supervisor.clone().into_trusted_peer_descriptor();
                match generated_temporary_supervisor_route_authority_pair(
                    adapter,
                    session_id,
                    &response_descriptor,
                    "revoke supervisor failed",
                )
                .await
                {
                    Ok(pair) => pair,
                    Err(reason) => {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            reason,
                        )
                        .await;
                        return true;
                    }
                }
            };
            let revoke_transition = match adapter
                .stage_supervisor_revoke(session_id, supervisor_peer_id.clone(), payload.epoch)
                .await
            {
                Ok(transition) => transition,
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("revoke supervisor failed: DSL rejected revoke: {error}"),
                    )
                    .await;
                    return true;
                }
            };
            let revoke_obligation = match matching_supervisor_revoke_obligation(
                adapter,
                session_id,
                &revoke_transition,
                supervisor_peer_id.as_str(),
                payload.epoch,
                "revoke supervisor",
            )
            .await
            {
                Ok(obligation) => obligation,
                Err(reason) => {
                    if let Err(error) = adapter
                        .stage_supervisor_trust_revoke_failed(
                            session_id,
                            supervisor_peer_id.clone(),
                            payload.epoch,
                            reason.clone(),
                        )
                        .await
                    {
                        tracing::warn!(
                            %session_id,
                            epoch = payload.epoch,
                            %error,
                            "failed to restore supervisor binding after missing revoke effect"
                        );
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
            };
            if let Err(error) = apply_generated_private_trust_remove(
                comms_runtime,
                revoke_obligation.peer_id().clone(),
                match supervisor_revoke_authority(&revoke_obligation) {
                    Ok(authority) => authority,
                    Err(error) => {
                        let _ = adapter
                            .stage_supervisor_trust_revoke_failed(
                                session_id,
                                revoke_obligation.peer_id().clone(),
                                revoke_obligation.epoch(),
                                error.clone(),
                            )
                            .await;
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            error,
                        )
                        .await;
                        return true;
                    }
                },
            )
            .await
            {
                let feedback_result = adapter
                    .stage_supervisor_trust_revoke_failed(
                        session_id,
                        revoke_obligation.peer_id().clone(),
                        revoke_obligation.epoch(),
                        error.clone(),
                    )
                    .await;
                let mut reason = format!(
                    "revoke supervisor failed: trust removal rejected after DSL commit: {error}"
                );
                if let Err(feedback_error) = feedback_result {
                    reason.push_str(&format!("; feedback failed: {feedback_error}"));
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
            // Reply-route fix (PR #750): capture the supervisor descriptor before
            // the generated revoke removes its trust, so the ack can be sent over
            // a temporary route. `supervisor_peer_id` is already bound above.
            let supervisor_response_peer =
                authorized_supervisor.clone().into_trusted_peer_descriptor();
            // Wave-d D-d: close the `supervisor_trust_revoke` obligation
            // with the epoch observed on the generated producer effect.
            // `RevokeSupervisor` first records the pending trust revoke,
            // then the shell mechanically removes live trust, then this
            // typed feedback clears the pending obligation.
            if let Err(error) = adapter
                .stage_supervisor_trust_revoked(
                    session_id,
                    revoke_obligation.peer_id().clone(),
                    revoke_obligation.epoch(),
                )
                .await
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!(
                        "revoke supervisor failed: DSL rejected trust revoke feedback: {error}"
                    ),
                )
                .await;
                return true;
            }
            let (add_authority, remove_authority) = revoke_reply_route_authority;
            send_bridge_response_with_temporary_supervisor_route(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::Ack(BridgeAck { ok: true }),
                supervisor_response_peer,
                add_authority,
                remove_authority,
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
            let authorized_supervisor = match resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "deliver member input failed",
            )
            .await
            {
                Ok(supervisor) => supervisor,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                    return true;
                }
            };
            let request_input_id = payload.input_id.clone();
            let input = peer_input_from_delivery_payload(
                session_id,
                authorized_supervisor.peer_id,
                candidate.interaction.id,
                payload,
            );
            match adapter
                .accept_input_with_completion(session_id, input)
                .await
            {
                Ok((accept_outcome, _completion_handle)) => {
                    let response = match accept_outcome {
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
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &payload,
                "interrupt member failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            match adapter.cancel_after_boundary(session_id).await {
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
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &payload,
                "retire member failed",
            )
            .await
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
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &payload,
                "destroy member failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                return true;
            }
            let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
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
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &payload,
                "observe member failed",
            )
            .await
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
                    let lifecycle_facts =
                        match crate::meerkat_machine::classify_runtime_lifecycle_state(state) {
                            Ok(facts) => facts,
                            Err(error) => {
                                send_bridge_failure(
                                    comms_runtime,
                                    candidate,
                                    BridgeRejectionCause::Internal,
                                    format!("runtime lifecycle classification failed: {error}"),
                                )
                                .await;
                                return true;
                            }
                        };
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Observation(BridgeObservationResponse::new(
                            bridge_state,
                            Some(lifecycle_facts.can_accept_input()),
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
            let authorized_supervisor = match resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "wire member failed",
            )
            .await
            {
                Ok(supervisor) => supervisor,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                    return true;
                }
            };
            // origin/main routed WireMember through the DSL peer-projection
            // state with NO direct `add_trusted_peer`; the gen-authority
            // branch preserves that property and stages the
            // supervisor-authorized MobMachine peer overlay
            // (`AuthorizeSupervisorMobPeerOverlay`), which folds the
            // direct-peer-endpoint projection and drives the session's
            // `CommsTrustReconciler` exclusively through generated authority.
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
            // Peer wiring requires the V3 MobMachine peer overlay. A V2 payload
            // (no overlay) is a clean cross-version rejection, not a serde error:
            // the field is optional on the wire so it deserializes, and we reject
            // here with a typed `UnsupportedProtocolVersion` cause.
            let mob_peer_overlay = match payload.mob_peer_overlay {
                Some(overlay) => overlay,
                None => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::UnsupportedProtocolVersion,
                        format!(
                            "wire member failed: peer wiring requires supervisor bridge protocol v{} (mob peer overlay absent); upgrade the supervisor",
                            BridgeProtocolVersion::V3
                        ),
                    )
                    .await;
                    return true;
                }
            };
            let overlay = match bridge_mob_peer_overlay(&mob_peer_overlay) {
                Ok(overlay) => overlay,
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::InvalidPeerSpec,
                        format!("wire member failed: invalid MobMachine peer overlay: {error}"),
                    )
                    .await;
                    return true;
                }
            };
            match adapter
                .stage_authorized_supervisor_mob_peer_overlay(
                    session_id,
                    payload.supervisor.peer_id,
                    payload.epoch,
                    mob_peer_overlay.recipient_peer_id,
                    mob_peer_overlay.topology_epoch,
                    overlay.endpoints,
                    overlay.endpoint_count,
                    peer_spec.peer_id.to_string(),
                    crate::meerkat_machine::dsl::PeerEndpoint::from(&peer_spec),
                    crate::meerkat_machine::dsl::MobPeerOverlayCommandKind::Wire,
                    Arc::clone(comms_runtime),
                )
                .await
            {
                Ok(()) => {
                    if let Err((cause, reason)) = ensure_bridge_response_route_for_supervisor(
                        adapter,
                        session_id,
                        comms_runtime,
                        &authorized_supervisor,
                        "wire member failed",
                    )
                    .await
                    {
                        send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                        return true;
                    }
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                    )
                    .await;
                }
                Err(error) => {
                    // origin/main idempotency tolerance, preserved on the
                    // gen-authority overlay path: a re-wire whose endpoint is
                    // already present in the DSL peer-projection state is a
                    // no-op success rather than a failure. `direct_peer_endpoint_contains`
                    // did not survive the adapter merge; mirror its semantics
                    // with the surviving `direct_peer_endpoints` accessor.
                    let endpoint = crate::meerkat_machine::dsl::PeerEndpoint::from(&peer_spec);
                    if matches!(
                        error,
                        crate::meerkat_machine::PeerEndpointStageError::Dsl(
                            crate::meerkat_machine::dsl::MeerkatMachineTransitionError::GuardRejected {
                                ..
                            }
                        )
                    ) && adapter
                        .direct_peer_endpoints(session_id)
                        .await
                        .map(|endpoints| endpoints.contains(&endpoint))
                        .unwrap_or(false)
                    {
                        if let Err((cause, reason)) = ensure_bridge_response_route_for_supervisor(
                            adapter,
                            session_id,
                            comms_runtime,
                            &authorized_supervisor,
                            "wire member failed",
                        )
                        .await
                        {
                            send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                            return true;
                        }
                        send_bridge_response(
                            comms_runtime,
                            candidate,
                            meerkat_core::interaction::ResponseStatus::Completed,
                            BridgeReply::Ack(BridgeAck { ok: true }),
                        )
                        .await;
                    } else {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!(
                                "wire member failed: generated peer overlay authority rejected: {error}"
                            ),
                        )
                        .await;
                    }
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
            let authorized_supervisor = match resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "unwire member failed",
            )
            .await
            {
                Ok(supervisor) => supervisor,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                    return true;
                }
            };
            // origin/main mirror of WireMember — route UnwireMember through
            // the DSL peer-projection state (trust store is a derived
            // projection maintained by the reconciler). The gen-authority
            // branch preserves that property and stages the
            // supervisor-authorized MobMachine peer overlay
            // (`AuthorizeSupervisorMobPeerOverlay` with `Unwire`).
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
            // Mirror WireMember: a V2 unwire payload (no overlay) is a clean
            // cross-version rejection, not a serde error.
            let mob_peer_overlay = match payload.mob_peer_overlay {
                Some(overlay) => overlay,
                None => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::UnsupportedProtocolVersion,
                        format!(
                            "unwire member failed: peer wiring requires supervisor bridge protocol v{} (mob peer overlay absent); upgrade the supervisor",
                            BridgeProtocolVersion::V3
                        ),
                    )
                    .await;
                    return true;
                }
            };
            let overlay = match bridge_mob_peer_overlay(&mob_peer_overlay) {
                Ok(overlay) => overlay,
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::InvalidPeerSpec,
                        format!("unwire member failed: invalid MobMachine peer overlay: {error}"),
                    )
                    .await;
                    return true;
                }
            };
            match adapter
                .stage_authorized_supervisor_mob_peer_overlay(
                    session_id,
                    payload.supervisor.peer_id,
                    payload.epoch,
                    mob_peer_overlay.recipient_peer_id,
                    mob_peer_overlay.topology_epoch,
                    overlay.endpoints,
                    overlay.endpoint_count,
                    peer_spec.peer_id.to_string(),
                    crate::meerkat_machine::dsl::PeerEndpoint::from(&peer_spec),
                    crate::meerkat_machine::dsl::MobPeerOverlayCommandKind::Unwire,
                    Arc::clone(comms_runtime),
                )
                .await
            {
                Ok(()) => {
                    if let Err((cause, reason)) = ensure_bridge_response_route_for_supervisor(
                        adapter,
                        session_id,
                        comms_runtime,
                        &authorized_supervisor,
                        "unwire member failed",
                    )
                    .await
                    {
                        send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                        return true;
                    }
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                    )
                    .await;
                }
                Err(error) => {
                    // origin/main idempotency tolerance, preserved on the
                    // gen-authority overlay path: an unwire whose endpoint is
                    // already ABSENT from the DSL peer-projection state is a
                    // no-op success. Mirror the dropped `direct_peer_endpoint_contains`
                    // with the surviving `direct_peer_endpoints` accessor.
                    let endpoint = crate::meerkat_machine::dsl::PeerEndpoint::from(&peer_spec);
                    if matches!(
                        error,
                        crate::meerkat_machine::PeerEndpointStageError::Dsl(
                            crate::meerkat_machine::dsl::MeerkatMachineTransitionError::GuardRejected {
                                ..
                            }
                        )
                    ) && !adapter
                        .direct_peer_endpoints(session_id)
                        .await
                        .map(|endpoints| endpoints.contains(&endpoint))
                        .unwrap_or(false)
                    {
                        if let Err((cause, reason)) = ensure_bridge_response_route_for_supervisor(
                            adapter,
                            session_id,
                            comms_runtime,
                            &authorized_supervisor,
                            "unwire member failed",
                        )
                        .await
                        {
                            send_bridge_failure(comms_runtime, candidate, cause, reason).await;
                            return true;
                        }
                        send_bridge_response(
                            comms_runtime,
                            candidate,
                            meerkat_core::interaction::ResponseStatus::Completed,
                            BridgeReply::Ack(BridgeAck { ok: true }),
                        )
                        .await;
                    } else {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!(
                                "unwire member failed: generated peer overlay authority rejected: {error}"
                            ),
                        )
                        .await;
                    }
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
            structured_output: result.structured_output,
        },
        CompletionOutcome::CompletedWithoutResult => AgentEvent::InteractionComplete {
            interaction_id,
            result: String::new(),
            structured_output: None,
        },
        CompletionOutcome::CallbackPending { tool_name, args } => {
            AgentEvent::InteractionCallbackPending {
                interaction_id,
                tool_name,
                args,
            }
        }
        CompletionOutcome::Cancelled => AgentEvent::InteractionFailed {
            interaction_id,
            reason: InteractionFailureReason::Cancelled,
        },
        CompletionOutcome::Abandoned { reason, .. }
        | CompletionOutcome::AbandonedWithError { reason, .. }
        | CompletionOutcome::RuntimeTerminated { reason, .. } => AgentEvent::InteractionFailed {
            interaction_id,
            reason: InteractionFailureReason::abandoned(reason),
        },
        CompletionOutcome::CompletedWithFinalizationFailure { error, .. } => {
            let detail = error
                .detail
                .unwrap_or_else(|| "turn finalization failed".to_string());
            AgentEvent::InteractionFailed {
                interaction_id,
                reason: InteractionFailureReason::finalization_failed(detail),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_contracts::wire::supervisor_bridge::{
        supervisor_bridge_current_protocol_version, supervisor_bridge_default_protocol_version,
        supervisor_bridge_supported_protocol_versions,
    };
    use meerkat_contracts::{BridgeMobPeerOverlayHandoff, BridgePeerWiringPayload};
    use meerkat_core::InteractionId;
    use meerkat_core::SendError;
    use meerkat_core::interaction::InboxInteraction;
    use meerkat_core::interaction::{PeerIngressConvention, PeerIngressIdentity};
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
    // These stable UUID strings are derived from the matching test pubkey
    // seed, so local endpoint staging can validate peer_id/pubkey coherence.
    const PEER_ID_RECEIVER: &str = "0b86269a-ed69-5e31-a1ce-9552506eebe3"; // seed 0xaa
    const PEER_ID_SUPERVISOR: &str = "0adb7b34-edb5-5898-bab4-3f7cb10ba4da"; // seed 0xbb
    const PEER_ID_OLD_SUPERVISOR: &str = "d15ad17e-57de-58da-a91a-466c87a3e0c4"; // seed 0xdd

    fn test_pubkey_bytes_for_peer_id(peer_id: &str) -> Option<[u8; 32]> {
        match peer_id {
            PEER_ID_RECEIVER => Some([0xaa; 32]),
            PEER_ID_SUPERVISOR => Some([0xbb; 32]),
            PEER_ID_OLD_SUPERVISOR => Some([0xdd; 32]),
            _ => None,
        }
    }

    fn test_public_key_for_peer_id(peer_id: &str) -> Option<String> {
        test_pubkey_bytes_for_peer_id(peer_id)
            .map(|bytes| meerkat_comms::PubKey::new(bytes).to_pubkey_string())
    }

    fn test_comms_name_from_address(address: &str) -> String {
        address
            .strip_prefix("inproc://")
            .unwrap_or(address)
            .to_string()
    }

    fn test_projection_trust_dsl(
        runtime: &meerkat_comms::CommsRuntime,
    ) -> Arc<crate::handles::HandleDslAuthority> {
        let dsl = Arc::new(crate::handles::HandleDslAuthority::ephemeral());
        dsl.apply_signal(
            crate::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
            "test_projection_trust_initialize",
        )
        .expect("Initialize signal");
        dsl.apply_input(
            crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: crate::meerkat_machine::dsl::SessionId::from(
                    "comms-drain-test-projection-trust",
                ),
            },
            "test_projection_trust_register",
        )
        .expect("RegisterSession input");
        crate::handles::RuntimePeerCommsHandle::install_generated_on(Arc::clone(&dsl), runtime)
            .expect("install generated peer-comms handle");
        dsl
    }

    async fn add_test_projection_trust(
        runtime: &meerkat_comms::CommsRuntime,
        peer: TrustedPeerDescriptor,
        context: &str,
    ) {
        let dsl = test_projection_trust_dsl(runtime);
        add_test_projection_trust_with_dsl(runtime, dsl, peer, 1, context).await;
    }

    async fn add_test_projection_trust_with_dsl(
        runtime: &meerkat_comms::CommsRuntime,
        dsl: Arc<crate::handles::HandleDslAuthority>,
        peer: TrustedPeerDescriptor,
        epoch: u64,
        context: &str,
    ) {
        let local_peer_id = runtime
            .peer_id()
            .unwrap_or_else(|| panic!("{context}: runtime peer_id unavailable"));
        let endpoint = crate::meerkat_machine::dsl::PeerEndpoint::from(&peer);
        dsl.apply_input(
            crate::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint {
                endpoint: crate::meerkat_machine::dsl::PeerEndpoint::new(
                    "local",
                    local_peer_id.to_string(),
                    "inproc://local",
                    [0x7f; 32],
                ),
            },
            "test_projection_trust_publish_local_endpoint",
        )
        .expect("PublishLocalEndpoint input");
        let transition = dsl
            .apply_input_with_transition(
                crate::meerkat_machine::dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
                    epoch,
                    endpoints: std::collections::BTreeSet::from([endpoint.clone()]),
                },
                "test_projection_trust_apply_overlay",
            )
            .expect("ApplyMobPeerOverlay input");
        let obligation = crate::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
            &transition,
            dsl.peer_projection_freshness_authority(),
        )
        .into_iter()
        .next()
        .expect("generated reconcile obligation");
        let authority =
            crate::protocol_comms_trust_reconcile::authority_for_endpoint(&obligation, &endpoint)
                .expect("generated authority");
        match runtime
            .apply_trust_mutation(CommsTrustMutation::AddTrustedPeer { peer, authority })
            .await
            .unwrap_or_else(|error| panic!("{context}: {error}"))
        {
            CommsTrustMutationResult::Added { .. } => {}
            other => panic!("{context}: unexpected trust mutation result {other:?}"),
        }
    }

    /// Seed a PRIVATE supervisor trust edge on `runtime` via the production
    /// generated supervisor-publish path against `adapter`'s session dsl. The
    /// runtime must already carry the adapter session's peer-comms owner token
    /// (via `test_install_session_peer_comms_handle_on_runtime`), and the
    /// supervisor must already be bound so `RequestSupervisorTrustPublish`
    /// restates the live binding.
    ///
    /// Seeding through `MeerkatMachineSupervisorPublish` (not a public
    /// projection) is required so the production PRIVATE revoke — which only
    /// removes rows owned by that same source kind — actually removes the edge.
    async fn add_member_private_supervisor_trust_via_adapter(
        adapter: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        supervisor: &TrustedPeerDescriptor,
        _epoch: u64,
        context: &str,
    ) {
        let runtime_dyn: Arc<dyn CommsRuntime> = Arc::clone(runtime) as Arc<dyn CommsRuntime>;
        adapter
            .stage_local_endpoint_for_comms_runtime(session_id, runtime_dyn.as_ref())
            .await
            .unwrap_or_else(|error| panic!("{context}: local endpoint rejected: {error}"));
        let obligation =
            supervisor_route_publish_obligation(adapter, session_id, supervisor, context)
                .await
                .unwrap_or_else(|error| panic!("{context}: {error}"));
        publish_supervisor_trust_from_generated_obligation(
            adapter,
            session_id,
            &runtime_dyn,
            &obligation,
        )
        .await
        .unwrap_or_else(|error| panic!("{context}: private trust publish failed: {error}"));
    }

    fn install_running_test_peer_comms_handle(
        runtime: &meerkat_comms::CommsRuntime,
    ) -> Arc<crate::handles::HandleDslAuthority> {
        let authority = Arc::new(crate::handles::HandleDslAuthority::ephemeral());
        let session_id = SessionId::new();
        authority
            .apply_signal(
                crate::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
                "running_test_peer_comms_handle_initialize",
            )
            .expect("test peer-comms authority should initialize");
        authority
            .apply_input(
                crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                },
                "running_test_peer_comms_handle_register",
            )
            .expect("test peer-comms authority should register a session");
        authority
            .apply_input(
                crate::meerkat_machine::dsl::MeerkatMachineInput::Prepare {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(
                        &meerkat_core::lifecycle::RunId::new(),
                    ),
                },
                "running_test_peer_comms_handle_prepare",
            )
            .expect("test peer-comms authority should enter Running");
        crate::handles::RuntimePeerCommsHandle::install_generated_on(
            Arc::clone(&authority),
            runtime,
        )
        .expect("install generated peer-comms handle");
        authority
    }

    fn test_pubkey(seed: u8) -> meerkat_comms::PubKey {
        assert_ne!(seed, 0, "test pubkey seed must be non-zero");
        meerkat_comms::PubKey::new([seed; 32])
    }

    fn test_supervisor_signing_public_key(seed: u8) -> String {
        encode_supervisor_signing_public_key([seed; 32])
    }

    fn signing_public_key_for_descriptor(peer: &TrustedPeerDescriptor) -> String {
        encode_supervisor_signing_public_key(peer.pubkey)
    }

    fn bridge_peer_spec_with_seed(name: &str, seed: u8, address: &str) -> BridgePeerSpec {
        let pubkey = test_pubkey(seed);
        BridgePeerSpec {
            name: name.to_string(),
            peer_id: pubkey.to_peer_id().as_str(),
            address: address.to_string(),
            pubkey: *pubkey.as_bytes(),
        }
    }

    fn supervisor_bridge_spec() -> BridgePeerSpec {
        bridge_peer_spec_with_seed(
            "mob/__mob_supervisor__",
            0xbb,
            "inproc://mob/__mob_supervisor__",
        )
    }

    fn current_supervisor_bridge_spec() -> BridgePeerSpec {
        bridge_peer_spec_with_seed(
            "mob/__mob_supervisor__",
            0xcc,
            "inproc://mob/__mob_supervisor__",
        )
    }

    fn pubkey_sender_for_bridge_spec(spec: &BridgePeerSpec) -> String {
        meerkat_comms::PubKey::new(spec.pubkey).to_pubkey_string()
    }

    fn old_supervisor_bridge_spec() -> BridgePeerSpec {
        bridge_peer_spec_with_seed(
            "mob/__mob_supervisor__",
            0xdd,
            "inproc://mob/__mob_supervisor__",
        )
    }

    fn trusted_supervisor_descriptor(seed: u8) -> TrustedPeerDescriptor {
        TrustedPeerDescriptor::try_from(bridge_peer_spec_with_seed(
            "mob/__mob_supervisor__",
            seed,
            "inproc://mob/__mob_supervisor__",
        ))
        .expect("valid supervisor spec")
    }

    fn trusted_peer_from_runtime(
        name: &str,
        runtime: &meerkat_comms::CommsRuntime,
    ) -> TrustedPeerDescriptor {
        let pubkey = runtime.public_key();
        TrustedPeerDescriptor::unsigned_with_pubkey(
            name,
            pubkey.to_peer_id().as_str(),
            *pubkey.as_bytes(),
            format!("inproc://{name}"),
        )
        .expect("valid non-zero trusted peer descriptor")
    }

    async fn recorded_bridge_replies(
        sent_commands: &Arc<tokio::sync::Mutex<Vec<CommsCommand>>>,
    ) -> Vec<(meerkat_core::interaction::ResponseStatus, BridgeReply)> {
        sent_commands
            .lock()
            .await
            .iter()
            .filter_map(|cmd| match cmd {
                CommsCommand::PeerResponse { status, result, .. } => Some((
                    *status,
                    serde_json::from_value::<BridgeReply>(result.clone())
                        .expect("recorded bridge response is typed"),
                )),
                _ => None,
            })
            .collect()
    }

    fn assert_completed_bridge_ack(
        reply: &(meerkat_core::interaction::ResponseStatus, BridgeReply),
    ) {
        assert_eq!(
            reply.0,
            meerkat_core::interaction::ResponseStatus::Completed
        );
        assert!(
            matches!(reply.1, BridgeReply::Ack(BridgeAck { ok: true })),
            "expected completed bridge ack, got {:?}",
            reply.1
        );
    }

    fn trusted_tcp_peer_from_runtime(
        name: &str,
        runtime: &meerkat_comms::CommsRuntime,
    ) -> TrustedPeerDescriptor {
        let pubkey = runtime.public_key();
        TrustedPeerDescriptor::unsigned_with_pubkey(
            name,
            pubkey.to_peer_id().as_str(),
            *pubkey.as_bytes(),
            runtime.advertised_address(),
        )
        .expect("valid non-zero TCP trusted peer descriptor")
    }

    fn install_test_peer_request_response_authority(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        peer_handle: Arc<CountingPeerInteractionHandle>,
    ) {
        runtime.install_peer_request_response_authority(
            meerkat_comms::PeerRequestResponseAuthority::new(
                peer_handle,
                Arc::new(crate::handles::RuntimeInteractionStreamHandle::ephemeral()),
            ),
        );
    }

    #[derive(Default)]
    struct CountingPeerInteractionHandle {
        inbound: std::sync::Mutex<HashSet<meerkat_core::PeerCorrelationId>>,
        inbound_modes: std::sync::Mutex<HashMap<meerkat_core::PeerCorrelationId, HandlingMode>>,
        request_received_count: std::sync::atomic::AtomicUsize,
        response_rejected_count: std::sync::atomic::AtomicUsize,
        response_replied_count: std::sync::atomic::AtomicUsize,
        reject_request_received: std::sync::atomic::AtomicBool,
        reject_response_progress: std::sync::atomic::AtomicBool,
        reject_response_terminal: std::sync::atomic::AtomicBool,
    }

    impl CountingPeerInteractionHandle {
        fn rejecting_request_received() -> Self {
            let handle = Self::default();
            handle
                .reject_request_received
                .store(true, std::sync::atomic::Ordering::SeqCst);
            handle
        }

        fn rejecting_response_progress() -> Self {
            let handle = Self::default();
            handle
                .reject_response_progress
                .store(true, std::sync::atomic::Ordering::SeqCst);
            handle
        }

        fn rejecting_response_terminal() -> Self {
            let handle = Self::default();
            handle
                .reject_response_terminal
                .store(true, std::sync::atomic::Ordering::SeqCst);
            handle
        }

        fn rejected(
            context: &'static str,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> meerkat_core::handles::DslTransitionError {
            meerkat_core::handles::DslTransitionError::guard_rejected(
                context,
                format!("test authority rejected corr_id {corr_id}"),
            )
        }

        fn request_received_count(&self) -> usize {
            self.request_received_count
                .load(std::sync::atomic::Ordering::SeqCst)
        }

        fn response_replied_count(&self) -> usize {
            self.response_replied_count
                .load(std::sync::atomic::Ordering::SeqCst)
        }

        fn response_rejected_count(&self) -> usize {
            self.response_rejected_count
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl meerkat_core::handles::PeerInteractionHandle for CountingPeerInteractionHandle {
        fn request_sent(
            &self,
            _corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Ok(())
        }

        fn response_progress(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            if self
                .reject_response_progress
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(Self::rejected(
                    "PeerInteractionHandle::response_progress",
                    corr_id,
                ));
            }
            Ok(())
        }

        fn response_terminal(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
            _disposition: meerkat_core::handles::PeerTerminalDisposition,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            if self
                .reject_response_terminal
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(Self::rejected(
                    "PeerInteractionHandle::response_terminal",
                    corr_id,
                ));
            }
            Ok(())
        }

        fn response_rejected(
            &self,
            _corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.response_rejected_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn request_timed_out(
            &self,
            _corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Ok(())
        }

        fn request_send_failed(
            &self,
            _corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Ok(())
        }

        fn classify_response_reply(
            &self,
            status: meerkat_core::ResponseStatus,
        ) -> Result<meerkat_core::TerminalityClass, meerkat_core::handles::DslTransitionError>
        {
            let generated = crate::handles::RuntimePeerInteractionHandle::ephemeral();
            meerkat_core::handles::PeerInteractionHandle::classify_response_reply(
                &generated, status,
            )
        }

        fn request_received(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
            handling_mode: HandlingMode,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            if self
                .reject_request_received
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(Self::rejected(
                    "PeerInteractionHandle::request_received",
                    corr_id,
                ));
            }
            let mut inbound = self.inbound.lock().expect("inbound mutex");
            if !inbound.insert(corr_id) {
                return Err(Self::rejected(
                    "PeerInteractionHandle::request_received",
                    corr_id,
                ));
            }
            self.inbound_modes
                .lock()
                .expect("inbound modes mutex")
                .insert(corr_id, handling_mode);
            self.request_received_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn response_replied(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            let mut inbound = self.inbound.lock().expect("inbound mutex");
            if !inbound.remove(&corr_id) {
                return Err(Self::rejected(
                    "PeerInteractionHandle::response_replied",
                    corr_id,
                ));
            }
            self.inbound_modes
                .lock()
                .expect("inbound modes mutex")
                .remove(&corr_id);
            self.response_replied_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn outbound_state(
            &self,
            _corr_id: meerkat_core::PeerCorrelationId,
        ) -> Option<meerkat_core::OutboundPeerRequestState> {
            None
        }

        fn inbound_state(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Option<meerkat_core::InboundPeerRequestState> {
            self.inbound
                .lock()
                .expect("inbound mutex")
                .contains(&corr_id)
                .then_some(meerkat_core::InboundPeerRequestState::Received)
        }

        fn inbound_handling_mode(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Option<HandlingMode> {
            self.inbound_modes
                .lock()
                .expect("inbound modes mutex")
                .get(&corr_id)
                .copied()
        }

        fn install_cleanup_observer(
            &self,
            _observer: Arc<dyn meerkat_core::handles::PeerInteractionCleanupObserver>,
        ) {
        }
    }

    struct OneShotPeerRequestRuntime {
        notify: Arc<tokio::sync::Notify>,
        candidate: std::sync::Mutex<Option<PeerInputCandidate>>,
        peer_handle: Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>>,
        peer_request_response_handle: Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>>,
        completed_count: std::sync::atomic::AtomicUsize,
    }

    impl OneShotPeerRequestRuntime {
        fn new(
            candidate: PeerInputCandidate,
            peer_handle: Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>>,
        ) -> Self {
            Self {
                notify: Arc::new(tokio::sync::Notify::new()),
                candidate: std::sync::Mutex::new(Some(candidate)),
                peer_handle,
                peer_request_response_handle: None,
                completed_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn with_complete_authority(
            candidate: PeerInputCandidate,
            peer_handle: Arc<dyn meerkat_core::handles::PeerInteractionHandle>,
        ) -> Self {
            Self {
                notify: Arc::new(tokio::sync::Notify::new()),
                candidate: std::sync::Mutex::new(Some(candidate)),
                peer_handle: Some(peer_handle.clone()),
                peer_request_response_handle: Some(peer_handle),
                completed_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn completed_count(&self) -> usize {
            self.completed_count
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl CommsRuntime for OneShotPeerRequestRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            self.notify.clone()
        }

        fn dismiss_received(&self) -> bool {
            self.candidate.lock().expect("candidate mutex").is_none()
        }

        fn peer_interaction_handle(
            &self,
        ) -> Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>> {
            self.peer_handle.clone()
        }

        fn peer_request_response_authority_handle(
            &self,
        ) -> Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>> {
            self.peer_request_response_handle.clone()
        }

        async fn drain_classified_inbox_interactions(
            &self,
        ) -> Result<
            Vec<meerkat_core::interaction::ClassifiedInboxInteraction>,
            meerkat_core::agent::CommsCapabilityError,
        > {
            Ok(self
                .candidate
                .lock()
                .expect("candidate mutex")
                .take()
                .into_iter()
                .collect())
        }

        fn mark_interaction_complete(&self, _id: &InteractionId) {
            self.completed_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn inbound_peer_request_candidate(class: PeerInputClass) -> PeerInputCandidate {
        assert!(
            matches!(
                class,
                PeerInputClass::SilentRequest | PeerInputClass::ActionableRequest
            ),
            "test helper only builds inbound peer requests"
        );
        let id = InteractionId(Uuid::new_v4());
        let intent = format!("test.inbound.{class:?}");
        let sender = "partial-authority-peer";
        PeerInputCandidate {
            interaction: InboxInteraction {
                id,
                from_route: None,
                from: sender.to_string(),
                content: InteractionContent::Request {
                    intent: intent.clone(),
                    params: json!({ "ok": true }),
                    blocks: None,
                },
                rendered_text: String::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
            },
            ingress: PeerIngressFact::peer(
                id,
                class,
                meerkat_core::PeerIngressKind::Request,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                PeerIngressIdentity::new(
                    PeerId::new(),
                    sender,
                    PeerIngressConvention::Request {
                        request_id: id.to_string(),
                        intent,
                    },
                ),
            ),
            lifecycle_peer: None,
            response_terminality: None,
        }
    }

    fn response_candidate(
        class: PeerInputClass,
        response_terminality: Option<meerkat_core::interaction::TerminalityClass>,
    ) -> PeerInputCandidate {
        assert!(
            matches!(
                class,
                PeerInputClass::ResponseProgress | PeerInputClass::ResponseTerminal
            ),
            "test helper only builds peer response candidates"
        );
        let id = InteractionId(Uuid::new_v4());
        let in_reply_to = InteractionId(Uuid::new_v4());
        PeerInputCandidate {
            interaction: InboxInteraction {
                id,
                from_route: Some(PeerId::new()),
                from: "response-peer".to_string(),
                content: InteractionContent::Response {
                    in_reply_to,
                    status: meerkat_core::interaction::ResponseStatus::Completed,
                    result: json!({ "ok": true }),
                    blocks: None,
                },
                rendered_text: String::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
            },
            ingress: PeerIngressFact::peer(
                id,
                class,
                meerkat_core::PeerIngressKind::Response,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                PeerIngressIdentity::new(
                    PeerId::new(),
                    "response-peer",
                    PeerIngressConvention::Response {
                        in_reply_to,
                        status: meerkat_core::interaction::ResponseStatus::Completed,
                    },
                ),
            ),
            lifecycle_peer: None,
            response_terminality,
        }
    }

    /// Test double whose classified inbox drain can be configured to either
    /// fail (typed classification fault) or return an empty (Ok) inbox.
    struct ClassifiedDrainOutcomeRuntime {
        notify: Arc<tokio::sync::Notify>,
        fail_classified_drain: bool,
    }

    impl ClassifiedDrainOutcomeRuntime {
        fn new(fail_classified_drain: bool) -> Self {
            Self {
                notify: Arc::new(tokio::sync::Notify::new()),
                fail_classified_drain,
            }
        }
    }

    #[async_trait::async_trait]
    impl CommsRuntime for ClassifiedDrainOutcomeRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            self.notify.clone()
        }

        async fn drain_classified_inbox_interactions(
            &self,
        ) -> Result<
            Vec<meerkat_core::interaction::ClassifiedInboxInteraction>,
            meerkat_core::agent::CommsCapabilityError,
        > {
            if self.fail_classified_drain {
                Err(meerkat_core::agent::CommsCapabilityError::Unsupported(
                    "synthetic classification fault".to_string(),
                ))
            } else {
                Ok(Vec::new())
            }
        }
    }

    /// Regression for #341: a classified-drain ERROR must NOT be laundered into
    /// an empty inbox and routed to the idle/dismiss lifecycle branch. With the
    /// old `drain_peer_input_candidates().await.unwrap_or_default()` collapse,
    /// an error became `Vec::new()` and the loop idled until the (long) idle
    /// timeout. The fix consumes the typed classified path and fails closed
    /// promptly with `DrainExitReason::Failed`, so the drain exits well before
    /// the long idle timeout could fire.
    #[tokio::test]
    async fn classified_drain_error_fails_closed_not_idle_timeout() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        // Long idle timeout: if the error were laundered to empty, the drain
        // would block here for 60s instead of exiting.
        let runtime = Arc::new(ClassifiedDrainOutcomeRuntime::new(true));
        let drain = spawn_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Some(Duration::from_secs(60)),
        );

        tokio::time::timeout(Duration::from_secs(2), drain)
            .await
            .expect(
                "a classified-drain error must fail closed promptly, NOT idle until the 60s timeout",
            )
            .expect("drain task should not panic");
    }

    /// Companion to #341: an actually-empty (Ok) inbox still idles — it must
    /// NOT exit promptly. With a long idle timeout the drain blocks on the
    /// inbox notify rather than terminating, proving empty-because-idle is
    /// distinguishable from empty-because-error.
    #[tokio::test]
    async fn empty_classified_drain_idles_rather_than_exiting() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let runtime = Arc::new(ClassifiedDrainOutcomeRuntime::new(false));
        let drain = spawn_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Some(Duration::from_secs(60)),
        );

        // An idle (empty Ok) inbox must keep the drain alive: it should still be
        // running well within the 60s idle window.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), drain)
                .await
                .is_err(),
            "an empty (Ok) inbox must idle, not exit promptly"
        );
    }

    #[tokio::test]
    async fn response_without_machine_terminality_is_rejected_before_progress_or_admission() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let runtime = Arc::new(OneShotPeerRequestRuntime::new(
            response_candidate(PeerInputClass::ResponseTerminal, None),
            None,
        ));
        let drain = spawn_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Some(Duration::from_millis(10)),
        );

        tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("drain should exit after one response candidate")
            .expect("drain task should not panic");

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("registered session snapshot");
        assert_eq!(
            snapshot.ledger.input_count, 0,
            "missing terminality must fail closed before runtime admission"
        );
    }

    #[tokio::test]
    async fn malformed_response_terminality_rejects_pending_peer_truth_via_authority() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let peer_handle = Arc::new(CountingPeerInteractionHandle::default());
        let peer_authority: Arc<dyn meerkat_core::handles::PeerInteractionHandle> =
            peer_handle.clone();
        let runtime = Arc::new(OneShotPeerRequestRuntime::new(
            response_candidate(PeerInputClass::ResponseTerminal, None),
            Some(peer_authority),
        ));
        let drain = spawn_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Some(Duration::from_millis(10)),
        );

        tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("drain should exit after one malformed response candidate")
            .expect("drain task should not panic");

        assert_eq!(
            peer_handle.response_rejected_count(),
            1,
            "malformed response observations must terminalize pending peer truth through generated authority"
        );
        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("registered session snapshot");
        assert_eq!(
            snapshot.ledger.input_count, 0,
            "malformed response terminality must fail closed before runtime admission"
        );
    }

    #[tokio::test]
    async fn rejected_peer_response_transition_fails_closed_before_machine_admission() {
        for (candidate, peer_handle) in [
            (
                response_candidate(
                    PeerInputClass::ResponseTerminal,
                    Some(meerkat_core::interaction::TerminalityClass::Terminal {
                        disposition: meerkat_core::interaction::TerminalDisposition::Completed,
                    }),
                ),
                Arc::new(CountingPeerInteractionHandle::rejecting_response_terminal()),
            ),
            (
                response_candidate(
                    PeerInputClass::ResponseProgress,
                    Some(meerkat_core::interaction::TerminalityClass::Progress),
                ),
                Arc::new(CountingPeerInteractionHandle::rejecting_response_progress()),
            ),
        ] {
            let adapter = Arc::new(MeerkatMachine::ephemeral());
            let session_id = SessionId::new();
            adapter
                .register_session(session_id.clone())
                .await
                .expect("register session");

            let runtime = Arc::new(OneShotPeerRequestRuntime::new(
                candidate,
                Some(peer_handle.clone()),
            ));
            let drain = spawn_comms_drain(
                adapter.clone(),
                session_id.clone(),
                runtime.clone(),
                Some(Duration::from_millis(10)),
            );

            tokio::time::timeout(Duration::from_secs(1), drain)
                .await
                .expect("drain should exit after one rejected response candidate")
                .expect("drain task should not panic");

            assert_eq!(
                peer_handle.response_rejected_count(),
                1,
                "rejected generated peer-response transition should terminalize peer truth through response rejection"
            );
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("registered session snapshot");
            assert_eq!(
                snapshot.ledger.input_count, 0,
                "rejected generated peer-response transition must fail closed before runtime admission"
            );
        }
    }

    #[tokio::test]
    async fn rejected_peer_request_recording_rejects_inbound_request_before_machine_admission() {
        for class in [
            PeerInputClass::SilentRequest,
            PeerInputClass::ActionableRequest,
        ] {
            let adapter = Arc::new(MeerkatMachine::ephemeral());
            let session_id = SessionId::new();
            adapter
                .register_session(session_id.clone())
                .await
                .expect("register session");

            let peer_handle = Arc::new(CountingPeerInteractionHandle::rejecting_request_received());
            let peer_authority: Arc<dyn meerkat_core::handles::PeerInteractionHandle> = peer_handle;
            let runtime = Arc::new(OneShotPeerRequestRuntime::with_complete_authority(
                inbound_peer_request_candidate(class),
                peer_authority,
            ));
            let drain = spawn_comms_drain(
                adapter.clone(),
                session_id.clone(),
                runtime.clone(),
                Some(Duration::from_millis(10)),
            );

            tokio::time::timeout(Duration::from_secs(1), drain)
                .await
                .expect("drain should exit after one candidate")
                .expect("drain task should not panic");

            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("registered session snapshot");
            assert_eq!(
                snapshot.ledger.input_count, 0,
                "rejected PeerRequestReceived must not admit {class:?} into machine work"
            );
            assert_eq!(
                runtime.completed_count(),
                1,
                "rejected {class:?} should be closed at the comms boundary"
            );
        }
    }

    #[tokio::test]
    async fn incomplete_peer_request_authority_rejects_inbound_request_before_machine_admission() {
        for class in [
            PeerInputClass::SilentRequest,
            PeerInputClass::ActionableRequest,
        ] {
            let adapter = Arc::new(MeerkatMachine::ephemeral());
            let session_id = SessionId::new();
            adapter
                .register_session(session_id.clone())
                .await
                .expect("register session");

            let peer_handle = Arc::new(CountingPeerInteractionHandle::default());
            let peer_authority: Arc<dyn meerkat_core::handles::PeerInteractionHandle> =
                peer_handle.clone();
            let runtime = Arc::new(OneShotPeerRequestRuntime::new(
                inbound_peer_request_candidate(class),
                Some(peer_authority),
            ));
            let drain = spawn_comms_drain(
                adapter.clone(),
                session_id.clone(),
                runtime.clone(),
                Some(Duration::from_millis(10)),
            );

            tokio::time::timeout(Duration::from_secs(1), drain)
                .await
                .expect("drain should exit after one candidate")
                .expect("drain task should not panic");

            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("registered session snapshot");
            assert_eq!(
                peer_handle.request_received_count(),
                0,
                "partial authority must not fire PeerRequestReceived for {class:?}"
            );
            assert_eq!(
                snapshot.ledger.input_count, 0,
                "partial authority must not admit {class:?} into machine work"
            );
            assert_eq!(
                runtime.completed_count(),
                1,
                "rejected {class:?} should be closed at the comms boundary"
            );
        }
    }

    #[tokio::test]
    async fn absent_peer_request_authority_rejects_inbound_request_before_machine_admission() {
        for class in [
            PeerInputClass::SilentRequest,
            PeerInputClass::ActionableRequest,
        ] {
            let adapter = Arc::new(MeerkatMachine::ephemeral());
            let session_id = SessionId::new();
            adapter
                .register_session(session_id.clone())
                .await
                .expect("register session");

            let runtime = Arc::new(OneShotPeerRequestRuntime::new(
                inbound_peer_request_candidate(class),
                None,
            ));
            let drain = spawn_comms_drain(
                adapter.clone(),
                session_id.clone(),
                runtime.clone(),
                Some(Duration::from_millis(10)),
            );

            tokio::time::timeout(Duration::from_secs(1), drain)
                .await
                .expect("drain should exit after one candidate")
                .expect("drain task should not panic");

            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("registered session snapshot");
            assert_eq!(
                snapshot.ledger.input_count, 0,
                "missing authority must not admit {class:?} into machine work"
            );
            assert_eq!(
                runtime.completed_count(),
                1,
                "rejected {class:?} should be closed at the comms boundary"
            );
        }
    }

    fn bridge_sender_fact(sender: &str) -> PeerIngressFact {
        let id = InteractionId(Uuid::new_v4());
        bridge_sender_fact_with_id(id, sender)
    }

    fn bridge_sender_fact_with_display(
        canonical_peer_id: PeerId,
        display_label: &str,
    ) -> PeerIngressFact {
        let id = InteractionId(Uuid::new_v4());
        PeerIngressFact::peer(
            id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
            )),
            PeerIngressIdentity::new(
                canonical_peer_id,
                display_label,
                meerkat_core::PeerIngressConvention::Request {
                    request_id: id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            ),
        )
    }

    fn bridge_sender_fact_with_id(id: InteractionId, sender: &str) -> PeerIngressFact {
        if let Ok(pubkey) = meerkat_comms::PubKey::from_pubkey_string(sender) {
            return PeerIngressFact::peer(
                id,
                PeerInputClass::ActionableRequest,
                meerkat_core::PeerIngressKind::Request,
                Some(meerkat_core::PeerIngressAuthDecision::Exempt(
                    meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
                )),
                PeerIngressIdentity::new(
                    pubkey.to_peer_id(),
                    sender,
                    meerkat_core::PeerIngressConvention::Request {
                        request_id: id.to_string(),
                        intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                    },
                )
                .with_signing_pubkey(*pubkey.as_bytes()),
            );
        }
        if let Ok(peer_id) = PeerId::parse(sender) {
            return PeerIngressFact::peer(
                id,
                PeerInputClass::ActionableRequest,
                meerkat_core::PeerIngressKind::Request,
                Some(meerkat_core::PeerIngressAuthDecision::Exempt(
                    meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
                )),
                PeerIngressIdentity::new(
                    peer_id,
                    sender,
                    meerkat_core::PeerIngressConvention::Request {
                        request_id: id.to_string(),
                        intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                    },
                ),
            );
        }
        PeerIngressFact::peer(
            id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
            )),
            PeerIngressIdentity::new(
                PeerId::new(),
                sender,
                meerkat_core::PeerIngressConvention::Request {
                    request_id: id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            ),
        )
    }

    #[test]
    fn generated_sender_peer_id_uses_canonical_peer_id_not_display_name() {
        let current_peer_id = PeerId::parse(PEER_ID_SUPERVISOR).expect("valid supervisor peer id");
        let attacker_peer_id =
            PeerId::parse(PEER_ID_OLD_SUPERVISOR).expect("valid attacker peer id");
        let sender = bridge_sender_fact_with_display(attacker_peer_id, "mob/__mob_supervisor__");

        assert_ne!(
            generated_sender_peer_id(&sender),
            Some(current_peer_id.as_str()),
            "display label must not become generated admission identity"
        );
    }

    #[test]
    fn sender_matches_bridge_peer_rejects_same_display_name_with_different_peer_id() {
        let attacker_peer_id =
            PeerId::parse(PEER_ID_OLD_SUPERVISOR).expect("valid attacker peer id");
        let peer = bridge_peer_identity(&supervisor_bridge_spec(), "test")
            .expect("valid bridge peer identity");
        let sender = bridge_sender_fact_with_display(attacker_peer_id, peer.name.as_str());

        assert!(
            !sender_matches_bridge_peer(&sender, &peer),
            "display label must not prove bridge peer authority"
        );
    }

    #[tokio::test]
    async fn bridge_response_route_requires_known_canonical_peer_id() {
        let peer = meerkat_comms::Keypair::generate();
        let peer_pubkey = peer.public_key();
        let peer_spec = TrustedPeerDescriptor::unsigned_with_pubkey(
            "bridge-response-peer",
            peer_pubkey.to_peer_id().as_str(),
            *peer_pubkey.as_bytes(),
            "inproc://bridge-response-peer",
        )
        .expect("valid peer spec");
        let runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("bridge-response-route").expect("runtime"),
        );
        let runtime_dyn: Arc<dyn CommsRuntime> = runtime.clone();
        add_test_projection_trust(runtime.as_ref(), peer_spec.clone(), "trust peer").await;

        let route = resolve_peer_route(&runtime_dyn, peer_spec.peer_id)
            .await
            .expect("canonical peer id should resolve through the peer directory");

        assert_eq!(route.peer_id, peer_spec.peer_id);
        let unknown_peer_id = PeerId::new();
        assert!(
            resolve_peer_route(&runtime_dyn, unknown_peer_id)
                .await
                .is_none(),
            "unknown PeerIds must not synthesize bridge response routes"
        );
    }

    #[tokio::test]
    async fn bridge_response_route_uses_pubkey_sender_not_spoofed_payload_peer_id() {
        let runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("bridge-response-spoof").expect("runtime"),
        );
        let runtime_dyn: Arc<dyn CommsRuntime> = runtime.clone();
        let spoofed_key = meerkat_comms::Keypair::generate();
        let spoofed_pubkey = spoofed_key.public_key();
        let spoofed_peer_id = spoofed_pubkey.to_peer_id();
        let spoofed_target = TrustedPeerDescriptor::unsigned_with_pubkey(
            "spoofed-target",
            spoofed_peer_id.as_str(),
            *spoofed_pubkey.as_bytes(),
            "inproc://spoofed-target",
        )
        .expect("valid spoofed target");
        add_test_projection_trust(runtime.as_ref(), spoofed_target, "trust spoofed target").await;
        let sender_key = meerkat_comms::Keypair::generate();
        let sender_pubkey = sender_key.public_key();
        let spoofed_peer_id = spoofed_peer_id.as_str();
        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                supervisor: BridgePeerSpec {
                    name: "mob/__mob_supervisor__".to_string(),
                    peer_id: spoofed_peer_id.clone(),
                    address: "inproc://mob/__mob_supervisor__".to_string(),
                    pubkey: *sender_pubkey.as_bytes(),
                },
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                expected_peer_id: PEER_ID_RECEIVER.to_string(),
                expected_address: "inproc://receiver".to_string(),
                bootstrap_token: "bootstrap".to_string().into(),
            },
        );
        let id = InteractionId(Uuid::new_v4());
        let sender_label = sender_pubkey.to_pubkey_string();
        let candidate = PeerInputCandidate {
            interaction: InboxInteraction {
                id,
                from_route: None,
                from: sender_pubkey.to_pubkey_string(),
                content: InteractionContent::Request {
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                    params: serde_json::to_value(command).expect("serialize bridge command"),
                    blocks: None,
                },
                rendered_text: String::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
            },
            ingress: bridge_sender_fact_with_id(id, &sender_label),
            lifecycle_peer: None,
            response_terminality: None,
        };

        let route = resolve_bridge_response_route(&runtime_dyn, &candidate)
            .await
            .expect("raw pubkey sender should resolve to its derived route");

        assert_eq!(route.peer_id, sender_pubkey.to_peer_id());
        assert_ne!(
            route.peer_id.as_str(),
            spoofed_peer_id,
            "bridge replies must not route to the caller-supplied supervisor.peer_id"
        );
    }

    #[tokio::test]
    async fn bridge_response_route_accepts_trusted_display_name_sender() {
        let peer = meerkat_comms::Keypair::generate();
        let peer_pubkey = peer.public_key();
        let peer_spec = TrustedPeerDescriptor::unsigned_with_pubkey(
            "mob/__mob_supervisor__",
            peer_pubkey.to_peer_id().as_str(),
            *peer_pubkey.as_bytes(),
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid peer spec");
        let runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("bridge-response-display").expect("runtime"),
        );
        let runtime_dyn: Arc<dyn CommsRuntime> = runtime.clone();
        add_test_projection_trust(runtime.as_ref(), peer_spec.clone(), "trust peer").await;

        let command = BridgeCommand::ObserveMember(BridgeSupervisorPayload {
            supervisor: BridgePeerSpec::from(peer_spec.clone()),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        });
        let id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
            )),
            PeerIngressIdentity::new(
                peer_pubkey.to_peer_id(),
                peer_spec.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(*peer_pubkey.as_bytes()),
        );
        let candidate = bridge_candidate_with_ingress(peer_spec.name.as_str(), &command, ingress);
        let route = resolve_bridge_response_route(&runtime_dyn, &candidate)
            .await
            .expect("trusted display-name sender should resolve through peer directory");

        assert_eq!(route.peer_id, peer_spec.peer_id);
        assert_eq!(
            route.display_name.as_ref().map(|name| name.as_str()),
            Some(peer_spec.name.as_str())
        );
    }

    #[tokio::test]
    async fn bridge_response_route_uses_typed_sender_peer_id_not_display_or_payload_identity() {
        let peer = meerkat_comms::Keypair::generate();
        let peer_pubkey = peer.public_key();
        let peer_spec = TrustedPeerDescriptor::unsigned_with_pubkey(
            "mob/__mob_supervisor__",
            peer_pubkey.to_peer_id().as_str(),
            *peer_pubkey.as_bytes(),
            "inproc://mob/__mob_supervisor__",
        )
        .expect("valid peer spec");
        let runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("bridge-response-typed-sender")
                .expect("runtime"),
        );
        let runtime_dyn: Arc<dyn CommsRuntime> = runtime.clone();
        add_test_projection_trust(runtime.as_ref(), peer_spec.clone(), "trust peer").await;

        let payload_pubkey = meerkat_comms::Keypair::generate().public_key();
        let spoofed_payload_peer_id = PeerId::new();
        let command = BridgeCommand::ObserveMember(BridgeSupervisorPayload {
            supervisor: BridgePeerSpec {
                name: peer_spec.name.to_string(),
                peer_id: spoofed_payload_peer_id.as_str(),
                address: "inproc://mob/__mob_supervisor__".to_string(),
                pubkey: *payload_pubkey.as_bytes(),
            },
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        });
        let id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
            )),
            PeerIngressIdentity::new(
                peer_pubkey.to_peer_id(),
                peer_spec.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(*peer_pubkey.as_bytes()),
        );
        let spoofed_candidate =
            bridge_candidate_with_ingress(peer_spec.name.as_str(), &command, ingress);
        let route = resolve_bridge_response_route(&runtime_dyn, &spoofed_candidate)
            .await
            .expect("typed ingress route should ignore spoofed payload peer_id");
        assert_eq!(route.peer_id, peer_spec.peer_id);
        assert_ne!(route.peer_id, spoofed_payload_peer_id);
        assert_eq!(
            route.display_name.as_ref().map(|name| name.as_str()),
            Some(peer_spec.name.as_str())
        );
    }

    #[tokio::test]
    async fn bridge_response_seeds_inbound_request_before_peer_response_send() {
        let suffix = Uuid::new_v4().simple().to_string();
        let member_name = format!("bridge-response-member-{suffix}");
        let supervisor_name = format!("bridge-response-supervisor-{suffix}");
        let member_runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only(&member_name).expect("member"));
        let supervisor_runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only(&supervisor_name).expect("supervisor"),
        );
        let peer_handle = Arc::new(CountingPeerInteractionHandle::default());
        let member_dsl = install_running_test_peer_comms_handle(member_runtime.as_ref());
        member_runtime.install_peer_request_response_authority(
            meerkat_comms::PeerRequestResponseAuthority::new(
                peer_handle.clone(),
                Arc::new(crate::handles::RuntimeInteractionStreamHandle::ephemeral()),
            ),
        );
        let supervisor_dsl = install_running_test_peer_comms_handle(supervisor_runtime.as_ref());

        let supervisor_pubkey = supervisor_runtime.public_key();
        let supervisor_spec = TrustedPeerDescriptor::unsigned_with_pubkey(
            &supervisor_name,
            supervisor_pubkey.to_peer_id().as_str(),
            *supervisor_pubkey.as_bytes(),
            format!("inproc://{supervisor_name}"),
        )
        .expect("valid supervisor spec");
        add_test_projection_trust_with_dsl(
            member_runtime.as_ref(),
            Arc::clone(&member_dsl),
            supervisor_spec.clone(),
            1,
            "member should trust supervisor",
        )
        .await;

        let member_pubkey = member_runtime.public_key();
        let member_spec = TrustedPeerDescriptor::unsigned_with_pubkey(
            &member_name,
            member_pubkey.to_peer_id().as_str(),
            *member_pubkey.as_bytes(),
            format!("inproc://{member_name}"),
        )
        .expect("valid member spec");
        add_test_projection_trust_with_dsl(
            supervisor_runtime.as_ref(),
            Arc::clone(&supervisor_dsl),
            member_spec,
            1,
            "supervisor should trust member",
        )
        .await;

        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        adapter
            .stage_supervisor_bind(
                &session_id,
                supervisor_spec.name.to_string(),
                supervisor_spec.peer_id.as_str(),
                supervisor_spec.address.to_string(),
                signing_public_key_for_descriptor(&supervisor_spec),
                1,
            )
            .await
            .expect("pre-bind supervisor");
        let command = BridgeCommand::ObserveMember(BridgeSupervisorPayload {
            supervisor: BridgePeerSpec::from(supervisor_spec.clone()),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        });
        let interaction_id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
            )),
            PeerIngressIdentity::new(
                supervisor_spec.peer_id,
                supervisor_spec.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: interaction_id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(supervisor_spec.pubkey),
        );
        let candidate =
            bridge_candidate_with_ingress(supervisor_spec.name.as_str(), &command, ingress);

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(member_runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
            )
            .await,
            "bridge handler must own ObserveMember"
        );

        assert_eq!(
            peer_handle.request_received_count(),
            1,
            "bridge handler must seed inbound peer request state before replying"
        );
        assert_eq!(
            peer_handle.response_replied_count(),
            1,
            "real CommsRuntime::send(PeerResponse) should pass the inbound-state guard"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn bind_member_bootstrap_stores_supervisor_address_not_member_address() {
        let suffix = Uuid::new_v4().simple().to_string();
        let member_name = format!("tcp-bootstrap-member-{suffix}");
        let supervisor_name = format!("tcp-bootstrap-supervisor-{suffix}");

        let mut member_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&member_name).expect("member runtime");
        member_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure member TCP listener");
        member_runtime
            .start_listeners()
            .await
            .expect("start member TCP listener");
        let member_runtime = Arc::new(member_runtime);
        install_test_peer_request_response_authority(
            &member_runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );

        let mut supervisor_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&supervisor_name).expect("supervisor runtime");
        supervisor_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure supervisor TCP listener");
        supervisor_runtime
            .start_listeners()
            .await
            .expect("start supervisor TCP listener");
        let supervisor_runtime = Arc::new(supervisor_runtime);
        install_test_peer_request_response_authority(
            &supervisor_runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );

        let member_spec = trusted_tcp_peer_from_runtime(&member_name, &member_runtime);
        let supervisor_dsl = install_running_test_peer_comms_handle(supervisor_runtime.as_ref());
        add_test_projection_trust_with_dsl(
            supervisor_runtime.as_ref(),
            Arc::clone(&supervisor_dsl),
            member_spec.clone(),
            1,
            "supervisor trusts member target",
        )
        .await;
        let supervisor_spec =
            trusted_tcp_peer_from_runtime("mob/__mob_supervisor__", &supervisor_runtime);

        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        adapter
            .test_install_session_peer_comms_handle_on_runtime(&session_id, member_runtime.as_ref())
            .await
            .expect("install adapter session peer-comms handle on member runtime");

        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                supervisor: BridgePeerSpec::from(supervisor_spec.clone()),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                expected_peer_id: member_spec.peer_id.as_str(),
                expected_address: member_spec.address.to_string(),
                bootstrap_token: member_runtime.bridge_bootstrap_token().to_owned().into(),
            },
        );
        let request_receipt = supervisor_runtime
            .send(CommsCommand::PeerRequest {
                to: PeerRoute::with_display_name(member_spec.peer_id, member_spec.name.clone()),
                intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::to_value(&command).expect("serialize bridge command"),
                blocks: None,
                handling_mode: HandlingMode::Queue,
                stream: meerkat_core::comms::InputStreamMode::None,
            })
            .await
            .expect("supervisor sends bootstrap BindMember over TCP");
        let meerkat_core::SendReceipt::PeerRequestSent { interaction_id, .. } = request_receipt
        else {
            panic!("expected PeerRequestSent receipt, got {request_receipt:?}");
        };

        let candidate = drain_one_candidate(&member_runtime, "member BindMember request").await;
        assert_eq!(
            candidate.interaction.id, interaction_id,
            "member must receive the correlated bridge request"
        );
        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(member_runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
            )
            .await,
            "bridge handler must own BindMember"
        );

        let binding = adapter.supervisor_binding(&session_id).await;
        let SupervisorBinding::Bound {
            address: bound_address,
            ..
        } = binding
        else {
            panic!("bootstrap bind should leave supervisor bound, got {binding:?}");
        };
        assert_eq!(
            bound_address,
            supervisor_spec.address.to_string(),
            "supervisor binding must store the supervisor route, not the member route"
        );
        assert_ne!(
            bound_address,
            member_spec.address.to_string(),
            "storing the member address under the supervisor pubkey makes later bridge replies loop back to the member"
        );

        let response =
            drain_one_candidate(&supervisor_runtime, "supervisor BindMember response").await;
        let InteractionContent::Response { result, .. } = response.interaction.content else {
            panic!(
                "expected bridge response, got {:?}",
                response.interaction.content
            );
        };
        let BridgeReply::BindMember(bind_response) =
            serde_json::from_value(result).expect("typed bridge reply")
        else {
            panic!("expected BindMember reply");
        };
        assert_eq!(bind_response.peer_id, member_spec.peer_id.as_str());
        assert_eq!(
            bind_response.address,
            canonicalize_bridge_address(&member_spec.address.to_string()),
            "BindMember reply still reports the member's canonical advertised address"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn bind_member_idempotent_ack_repairs_tcp_supervisor_trust_before_reply() {
        let suffix = Uuid::new_v4().simple().to_string();
        let member_name = format!("tcp-bind-member-{suffix}");
        let supervisor_name = format!("tcp-supervisor-{suffix}");

        let mut member_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&member_name).expect("member runtime");
        member_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure member TCP listener");
        member_runtime
            .start_listeners()
            .await
            .expect("start member TCP listener");
        let member_runtime = Arc::new(member_runtime);
        let member_peer_handle = Arc::new(CountingPeerInteractionHandle::default());
        install_test_peer_request_response_authority(&member_runtime, member_peer_handle.clone());

        let mut supervisor_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&supervisor_name).expect("supervisor runtime");
        supervisor_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure supervisor TCP listener");
        supervisor_runtime
            .start_listeners()
            .await
            .expect("start supervisor TCP listener");
        let supervisor_runtime = Arc::new(supervisor_runtime);
        install_test_peer_request_response_authority(
            &supervisor_runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );

        let member_spec = trusted_tcp_peer_from_runtime(&member_name, &member_runtime);
        let supervisor_dsl = install_running_test_peer_comms_handle(supervisor_runtime.as_ref());
        add_test_projection_trust_with_dsl(
            supervisor_runtime.as_ref(),
            Arc::clone(&supervisor_dsl),
            member_spec.clone(),
            1,
            "supervisor trusts member target",
        )
        .await;
        let supervisor_spec =
            trusted_tcp_peer_from_runtime("mob/__mob_supervisor__", &supervisor_runtime);

        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        adapter
            .stage_supervisor_bind(
                &session_id,
                supervisor_spec.name.to_string(),
                supervisor_spec.peer_id.to_string(),
                supervisor_spec.address.to_string(),
                signing_public_key_for_descriptor(&supervisor_spec),
                1,
            )
            .await
            .expect("pre-bind supervisor in DSL state");
        adapter
            .test_install_session_peer_comms_handle_on_runtime(&session_id, member_runtime.as_ref())
            .await
            .expect("install adapter session peer-comms handle on member runtime");
        assert!(
            member_runtime
                .peers()
                .await
                .iter()
                .all(|peer| peer.peer_id != supervisor_spec.peer_id),
            "fixture must start with DSL supervisor binding but no sendable runtime trust entry"
        );

        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                supervisor: BridgePeerSpec::from(supervisor_spec.clone()),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                expected_peer_id: member_spec.peer_id.as_str(),
                expected_address: member_spec.address.to_string(),
                bootstrap_token: member_runtime.bridge_bootstrap_token().to_owned().into(),
            },
        );
        let request_receipt = supervisor_runtime
            .send(CommsCommand::PeerRequest {
                to: PeerRoute::with_display_name(member_spec.peer_id, member_spec.name.clone()),
                intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::to_value(&command).expect("serialize bridge command"),
                blocks: None,
                handling_mode: HandlingMode::Queue,
                stream: meerkat_core::comms::InputStreamMode::None,
            })
            .await
            .expect("supervisor sends idempotent BindMember over TCP");
        let meerkat_core::SendReceipt::PeerRequestSent { interaction_id, .. } = request_receipt
        else {
            panic!("expected PeerRequestSent receipt, got {request_receipt:?}");
        };

        let candidate = drain_one_candidate(&member_runtime, "member BindMember request").await;
        assert_eq!(
            candidate.interaction.id, interaction_id,
            "member must receive the correlated bridge request"
        );
        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(member_runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
            )
            .await,
            "bridge handler must own BindMember"
        );

        let response =
            drain_one_candidate(&supervisor_runtime, "supervisor BindMember response").await;
        assert_eq!(
            member_peer_handle.response_replied_count(),
            1,
            "target must send the terminal bridge response after repairing trust"
        );
        assert!(
            member_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(supervisor_spec.pubkey)),
            "idempotent BindMember must repair the PRIVATE supervisor admission edge"
        );
        assert!(
            member_runtime
                .peers()
                .await
                .iter()
                .all(|peer| peer.peer_id != supervisor_spec.peer_id),
            "repaired supervisor edge is a control-plane route; it must not surface in public peers()"
        );
        let InteractionContent::Response {
            in_reply_to,
            status,
            result,
            ..
        } = response.interaction.content
        else {
            panic!(
                "expected bridge response, got {:?}",
                response.interaction.content
            );
        };
        assert_eq!(in_reply_to, interaction_id);
        assert_eq!(status, meerkat_core::interaction::ResponseStatus::Completed);
        let reply: BridgeReply = serde_json::from_value(result).expect("typed bridge reply");
        let BridgeReply::BindMember(bind_response) = reply else {
            panic!("expected BindMember reply, got {reply:?}");
        };
        assert_eq!(bind_response.peer_id, member_spec.peer_id.as_str());
        assert_eq!(
            bind_response.address,
            canonicalize_bridge_address(&member_spec.address.to_string())
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn authorized_bridge_command_repairs_tcp_supervisor_response_route_before_reply() {
        let suffix = Uuid::new_v4().simple().to_string();
        let member_name = format!("tcp-observe-member-{suffix}");
        let supervisor_name = format!("tcp-observe-supervisor-{suffix}");

        let mut member_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&member_name).expect("member runtime");
        member_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure member TCP listener");
        member_runtime
            .start_listeners()
            .await
            .expect("start member TCP listener");
        let member_runtime = Arc::new(member_runtime);
        let member_peer_handle = Arc::new(CountingPeerInteractionHandle::default());
        install_test_peer_request_response_authority(&member_runtime, member_peer_handle.clone());

        let mut supervisor_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&supervisor_name).expect("supervisor runtime");
        supervisor_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure supervisor TCP listener");
        supervisor_runtime
            .start_listeners()
            .await
            .expect("start supervisor TCP listener");
        let supervisor_runtime = Arc::new(supervisor_runtime);
        install_test_peer_request_response_authority(
            &supervisor_runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );

        let member_spec = trusted_tcp_peer_from_runtime(&member_name, &member_runtime);
        let supervisor_dsl = install_running_test_peer_comms_handle(supervisor_runtime.as_ref());
        add_test_projection_trust_with_dsl(
            supervisor_runtime.as_ref(),
            Arc::clone(&supervisor_dsl),
            member_spec.clone(),
            1,
            "supervisor trusts member target",
        )
        .await;
        let supervisor_spec =
            trusted_tcp_peer_from_runtime("mob/__mob_supervisor__", &supervisor_runtime);

        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        adapter
            .stage_supervisor_bind(
                &session_id,
                supervisor_spec.name.to_string(),
                supervisor_spec.peer_id.as_str(),
                supervisor_spec.address.to_string(),
                signing_public_key_for_descriptor(&supervisor_spec),
                1,
            )
            .await
            .expect("pre-bind supervisor in DSL state");
        adapter
            .test_install_session_peer_comms_handle_on_runtime(&session_id, member_runtime.as_ref())
            .await
            .expect("install adapter session peer-comms handle on member runtime");
        assert!(
            member_runtime
                .peers()
                .await
                .iter()
                .all(|peer| peer.peer_id != supervisor_spec.peer_id),
            "fixture must start with authorized supervisor state but no public runtime route"
        );

        let command = BridgeCommand::ObserveMember(BridgeSupervisorPayload {
            supervisor: BridgePeerSpec::from(supervisor_spec.clone()),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        });
        let interaction_id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                supervisor_spec.peer_id,
                supervisor_spec.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: interaction_id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(supervisor_spec.pubkey),
        );
        let candidate =
            bridge_candidate_with_ingress(supervisor_spec.name.as_str(), &command, ingress);

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(member_runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
            )
            .await,
            "bridge handler must own ObserveMember"
        );
        assert_eq!(
            member_peer_handle.response_replied_count(),
            1,
            "target must send the terminal bridge response after repairing supervisor route"
        );

        let supervisor_pubkey = meerkat_comms::PubKey::new(supervisor_spec.pubkey);
        assert!(
            member_runtime.router().is_private(&supervisor_pubkey),
            "repaired supervisor response route must remain private"
        );
        assert!(
            member_runtime
                .peers()
                .await
                .iter()
                .all(|peer| peer.peer_id != supervisor_spec.peer_id),
            "private supervisor response route must not leak through public peers()"
        );

        let response =
            drain_one_candidate(&supervisor_runtime, "supervisor ObserveMember response").await;
        let InteractionContent::Response {
            in_reply_to,
            status,
            result,
            ..
        } = response.interaction.content
        else {
            panic!(
                "expected bridge response, got {:?}",
                response.interaction.content
            );
        };
        assert_eq!(in_reply_to, interaction_id);
        assert_eq!(status, meerkat_core::interaction::ResponseStatus::Completed);
        assert!(
            matches!(
                serde_json::from_value::<BridgeReply>(result).expect("typed bridge reply"),
                BridgeReply::Observation(_)
            ),
            "expected ObserveMember response"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn authorize_supervisor_rotation_replies_after_removing_previous_route() {
        let suffix = Uuid::new_v4().simple().to_string();
        let member_name = format!("tcp-authorize-member-{suffix}");
        let old_supervisor_name = format!("tcp-authorize-old-supervisor-{suffix}");
        let new_supervisor_name = format!("tcp-authorize-new-supervisor-{suffix}");

        let mut member_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&member_name).expect("member runtime");
        member_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure member TCP listener");
        member_runtime
            .start_listeners()
            .await
            .expect("start member TCP listener");
        let member_runtime = Arc::new(member_runtime);
        install_test_peer_request_response_authority(
            &member_runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );

        let mut old_supervisor_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&old_supervisor_name)
                .expect("old supervisor runtime");
        old_supervisor_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure old supervisor TCP listener");
        old_supervisor_runtime
            .start_listeners()
            .await
            .expect("start old supervisor TCP listener");
        let old_supervisor_runtime = Arc::new(old_supervisor_runtime);
        install_test_peer_request_response_authority(
            &old_supervisor_runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );

        let mut new_supervisor_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&new_supervisor_name)
                .expect("new supervisor runtime");
        new_supervisor_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure new supervisor TCP listener");
        new_supervisor_runtime
            .start_listeners()
            .await
            .expect("start new supervisor TCP listener");
        let new_supervisor_runtime = Arc::new(new_supervisor_runtime);

        let member_spec = trusted_tcp_peer_from_runtime(&member_name, &member_runtime);
        let old_supervisor_dsl =
            install_running_test_peer_comms_handle(old_supervisor_runtime.as_ref());
        add_test_projection_trust_with_dsl(
            old_supervisor_runtime.as_ref(),
            Arc::clone(&old_supervisor_dsl),
            member_spec,
            1,
            "old supervisor trusts member target",
        )
        .await;
        let old_supervisor_spec =
            trusted_tcp_peer_from_runtime("mob/__mob_supervisor__", &old_supervisor_runtime);
        let new_supervisor_spec =
            trusted_tcp_peer_from_runtime("mob/__mob_supervisor__", &new_supervisor_runtime);
        // member_runtime's initial old-supervisor trust is seeded below via the
        // PRIVATE supervisor-publish path against the adapter session, after the
        // session is registered and the old supervisor is bound — so the
        // production PRIVATE revoke (MeerkatMachineSupervisorPublish) removes it.

        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        adapter
            .stage_supervisor_bind(
                &session_id,
                old_supervisor_spec.name.to_string(),
                old_supervisor_spec.peer_id.as_str(),
                old_supervisor_spec.address.to_string(),
                signing_public_key_for_descriptor(&old_supervisor_spec),
                1,
            )
            .await
            .expect("pre-bind old supervisor in DSL state");
        adapter
            .test_install_session_peer_comms_handle_on_runtime(&session_id, member_runtime.as_ref())
            .await
            .expect("install adapter session peer-comms handle on member runtime");
        add_member_private_supervisor_trust_via_adapter(
            &adapter,
            &session_id,
            &member_runtime,
            &old_supervisor_spec,
            1,
            "member starts with old supervisor private route",
        )
        .await;
        assert!(
            member_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(old_supervisor_spec.pubkey)),
            "member must start with a PRIVATE old supervisor route"
        );

        let command = BridgeCommand::AuthorizeSupervisor(BridgeSupervisorPayload {
            supervisor: BridgePeerSpec::from(new_supervisor_spec.clone()),
            epoch: 2,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        });
        let interaction_id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                old_supervisor_spec.peer_id,
                old_supervisor_spec.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: interaction_id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(old_supervisor_spec.pubkey),
        );
        let candidate =
            bridge_candidate_with_ingress(old_supervisor_spec.name.as_str(), &command, ingress);

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(member_runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
            )
            .await,
            "bridge handler must own AuthorizeSupervisor"
        );

        let response = drain_one_candidate(
            &old_supervisor_runtime,
            "old supervisor AuthorizeSupervisor response",
        )
        .await;
        let InteractionContent::Response {
            in_reply_to,
            status,
            result,
            ..
        } = response.interaction.content
        else {
            panic!(
                "expected authorize bridge response, got {:?}",
                response.interaction.content
            );
        };
        assert_eq!(in_reply_to, interaction_id);
        assert_eq!(status, meerkat_core::interaction::ResponseStatus::Completed);
        assert!(matches!(
            serde_json::from_value::<BridgeReply>(result).expect("typed bridge reply"),
            BridgeReply::Ack(BridgeAck { ok: true })
        ));
        let peers = member_runtime.peers().await;
        // Supervisor trust is a PRIVATE control-plane edge: neither old nor new
        // supervisor may appear in the public peer directory.
        assert!(
            peers
                .iter()
                .all(|peer| peer.peer_id != old_supervisor_spec.peer_id),
            "old supervisor route must be removed after rotation"
        );
        assert!(
            peers
                .iter()
                .all(|peer| peer.peer_id != new_supervisor_spec.peer_id),
            "new supervisor route must stay private (not in public peers())"
        );
        assert!(
            member_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(new_supervisor_spec.pubkey)),
            "new supervisor private route must remain installed after rotation"
        );
        assert!(
            !member_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(old_supervisor_spec.pubkey)),
            "old supervisor private route must be fully removed after rotation"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn revoke_supervisor_replies_after_removing_supervisor_route() {
        let suffix = Uuid::new_v4().simple().to_string();
        let member_name = format!("tcp-revoke-member-{suffix}");
        let supervisor_name = format!("tcp-revoke-supervisor-{suffix}");

        let mut member_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&member_name).expect("member runtime");
        member_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure member TCP listener");
        member_runtime
            .start_listeners()
            .await
            .expect("start member TCP listener");
        let member_runtime = Arc::new(member_runtime);
        install_test_peer_request_response_authority(
            &member_runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );

        let mut supervisor_runtime =
            meerkat_comms::CommsRuntime::inproc_only(&supervisor_name).expect("supervisor runtime");
        supervisor_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure supervisor TCP listener");
        supervisor_runtime
            .start_listeners()
            .await
            .expect("start supervisor TCP listener");
        let supervisor_runtime = Arc::new(supervisor_runtime);
        install_test_peer_request_response_authority(
            &supervisor_runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );

        let member_spec = trusted_tcp_peer_from_runtime(&member_name, &member_runtime);
        let supervisor_dsl = install_running_test_peer_comms_handle(supervisor_runtime.as_ref());
        add_test_projection_trust_with_dsl(
            supervisor_runtime.as_ref(),
            Arc::clone(&supervisor_dsl),
            member_spec,
            1,
            "supervisor trusts member target",
        )
        .await;
        let supervisor_spec =
            trusted_tcp_peer_from_runtime("mob/__mob_supervisor__", &supervisor_runtime);
        // member_runtime's supervisor trust is seeded below via the PRIVATE
        // supervisor-publish path against the adapter session, after register +
        // bind, so the production PRIVATE revoke removes it.

        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        adapter
            .stage_supervisor_bind(
                &session_id,
                supervisor_spec.name.to_string(),
                supervisor_spec.peer_id.as_str(),
                supervisor_spec.address.to_string(),
                signing_public_key_for_descriptor(&supervisor_spec),
                1,
            )
            .await
            .expect("pre-bind supervisor in DSL state");
        adapter
            .test_install_session_peer_comms_handle_on_runtime(&session_id, member_runtime.as_ref())
            .await
            .expect("install adapter session peer-comms handle on member runtime");
        add_member_private_supervisor_trust_via_adapter(
            &adapter,
            &session_id,
            &member_runtime,
            &supervisor_spec,
            1,
            "member starts with supervisor private route",
        )
        .await;
        assert!(
            member_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(supervisor_spec.pubkey)),
            "member must start with a PRIVATE supervisor route"
        );

        let command = BridgeCommand::RevokeSupervisor(BridgeSupervisorPayload {
            supervisor: BridgePeerSpec::from(supervisor_spec.clone()),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        });
        let interaction_id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                supervisor_spec.peer_id,
                supervisor_spec.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: interaction_id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(supervisor_spec.pubkey),
        );
        let candidate =
            bridge_candidate_with_ingress(supervisor_spec.name.as_str(), &command, ingress);

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(member_runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
            )
            .await,
            "bridge handler must own RevokeSupervisor"
        );

        let response =
            drain_one_candidate(&supervisor_runtime, "supervisor RevokeSupervisor response").await;
        let InteractionContent::Response {
            in_reply_to,
            status,
            result,
            ..
        } = response.interaction.content
        else {
            panic!(
                "expected revoke bridge response, got {:?}",
                response.interaction.content
            );
        };
        assert_eq!(in_reply_to, interaction_id);
        assert_eq!(status, meerkat_core::interaction::ResponseStatus::Completed);
        assert!(matches!(
            serde_json::from_value::<BridgeReply>(result).expect("typed bridge reply"),
            BridgeReply::Ack(BridgeAck { ok: true })
        ));
        assert!(
            member_runtime
                .peers()
                .await
                .iter()
                .all(|peer| peer.peer_id != supervisor_spec.peer_id),
            "supervisor route must be removed after revoke"
        );
        assert!(
            !member_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(supervisor_spec.pubkey)),
            "supervisor private admission edge must be fully removed after revoke"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn drain_one_candidate(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        label: &str,
    ) -> PeerInputCandidate {
        for _ in 0..40 {
            let candidates = runtime.drain_peer_input_candidates().await;
            if let Some(candidate) = candidates.into_iter().next() {
                return candidate;
            }
            let notify = runtime.inbox_notify();
            let notified = notify.notified();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(25), notified).await;
        }
        panic!("timed out waiting for {label}");
    }

    #[test]
    fn bridge_response_route_source_does_not_use_interaction_from_ladder() {
        let source = include_str!("comms_drain.rs");
        let route_start = source
            .find("async fn resolve_bridge_response_route")
            .expect("route function exists");
        let route_end = source[route_start..]
            .find("async fn resolve_peer_route")
            .map(|offset| route_start + offset)
            .expect("peer route function follows bridge response route");
        let route_source = &source[route_start..route_end];
        let forbidden_from = ["candidate", "interaction", "from"].join(".");
        assert!(
            !route_source.contains(&forbidden_from),
            "bridge response routing must use typed ingress facts, not candidate.interaction.from"
        );

        for removed_helper in [
            ["bridge_response", "route_from_sender"].join("_"),
            ["resolve_bridge", "display_name_sender_route"].join("_"),
        ] {
            assert!(
                !source.contains(&format!("fn {removed_helper}")),
                "bridge response routing must not revive {removed_helper}"
            );
        }
    }

    #[test]
    fn comms_drain_registers_inbox_waiter_before_draining() {
        // CONC-3: the drain MUST pin + `enable()` its inbox `notified` waiter
        // BEFORE calling the classified inbox drain, so a `notify_waiters()`
        // that fires in the drain-empty -> await window is observed rather than
        // lost. Under the mob `Duration::MAX` idle timeout, losing it stalls the
        // drain forever (a peer message reported "sent" but never drained). Pin
        // the ordering structurally so a refactor cannot silently drop or
        // reorder it.
        let source = include_str!("comms_drain.rs");
        let enable = source
            .find("notified.as_mut().enable();")
            .expect("drain must enable() its pinned inbox waiter");
        let drain = source
            .find("comms_runtime.drain_classified_inbox_interactions().await")
            .expect("drain must consume the typed classified inbox path");
        assert!(
            enable < drain,
            "the inbox waiter must be registered (enable) BEFORE the drain, not after"
        );
    }

    #[tokio::test]
    async fn notify_waiters_is_lost_without_prior_registration() {
        // The exact tokio::Notify failure mode the drain's pin+enable() avoids:
        // `notify_waiters()` only wakes ALREADY-registered waiters, and a
        // `Notified` future is not registered until first polled. So a
        // notification that fires before the waiter is enabled is lost — which
        // is precisely why the drain enables its waiter before draining.
        use std::sync::Arc;
        use tokio::sync::Notify;

        let notify = Arc::new(Notify::new());
        notify.notify_waiters(); // fired with no registered waiter -> lost
        let late = notify.notified();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), late)
                .await
                .is_err(),
            "a notify_waiters() with no prior registration must be lost"
        );

        // ...whereas a waiter enabled BEFORE the notify observes it (the drain's
        // ordering): this completes without hitting the timeout.
        let mut early = std::pin::pin!(notify.notified());
        early.as_mut().enable();
        notify.notify_waiters();
        tokio::time::timeout(std::time::Duration::from_millis(100), early)
            .await
            .expect("a waiter enabled before notify_waiters() must observe it");
    }

    struct BootstrapRuntime {
        peer_id: String,
        address: String,
        bootstrap_token: Option<String>,
        inbox_notify: Arc<tokio::sync::Notify>,
        add_trusted_peer_errors: HashMap<String, String>,
        remove_trusted_peer_errors: HashMap<String, String>,
        trusted_peer_ids: Arc<tokio::sync::Mutex<HashSet<String>>>,
        trusted_peers: Arc<tokio::sync::Mutex<HashMap<String, TrustedPeerDescriptor>>>,
        sent_commands: Arc<tokio::sync::Mutex<Vec<CommsCommand>>>,
        peer_handle: Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>>,
        peer_request_response_handle: Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>>,
        completed_count: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CommsRuntime for BootstrapRuntime {
        fn peer_id(&self) -> Option<PeerId> {
            PeerId::parse(&self.peer_id).ok()
        }

        fn public_key(&self) -> Option<String> {
            test_public_key_for_peer_id(&self.peer_id)
        }

        fn public_key_bytes(&self) -> Option<[u8; 32]> {
            test_pubkey_bytes_for_peer_id(&self.peer_id)
        }

        fn comms_name(&self) -> Option<String> {
            Some(test_comms_name_from_address(&self.address))
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

        fn peer_interaction_handle(
            &self,
        ) -> Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>> {
            self.peer_handle.clone()
        }

        fn peer_request_response_authority_handle(
            &self,
        ) -> Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>> {
            self.peer_request_response_handle.clone()
        }

        fn mark_interaction_complete(&self, _id: &InteractionId) {
            self.completed_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        async fn apply_trust_mutation(
            &self,
            mutation: CommsTrustMutation,
        ) -> Result<CommsTrustMutationResult, SendError> {
            match mutation {
                CommsTrustMutation::AddPrivateTrustedPeer { peer, authority } => {
                    authority
                        .validate_private_add(self.peer_id(), &peer)
                        .map_err(SendError::Validation)?;
                    let peer_id_str = peer.peer_id.as_str();
                    if let Some(message) = self.add_trusted_peer_errors.get(&peer_id_str) {
                        return Err(SendError::Internal(message.clone()));
                    }
                    let created = self.trusted_peer_ids.lock().await.insert(peer_id_str);
                    Ok(CommsTrustMutationResult::Added { created })
                }
                CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority } => {
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|error| SendError::Validation(error.to_string()))?;
                    authority
                        .validate_private_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    match self.remove_trusted_peer_errors.get(&peer_id) {
                        Some(message) => Err(SendError::Internal(message.clone())),
                        None => Ok(CommsTrustMutationResult::Removed {
                            removed: self.trusted_peer_ids.lock().await.remove(&peer_id),
                        }),
                    }
                }
                _ => Err(SendError::Unsupported(
                    "test runtime only supports generated private trust".into(),
                )),
            }
        }

        async fn add_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
            let peer_id_str = peer.peer_id.as_str();
            if let Some(message) = self.add_trusted_peer_errors.get(&peer_id_str) {
                return Err(SendError::Internal(message.clone()));
            }
            self.trusted_peer_ids
                .lock()
                .await
                .insert(peer_id_str.clone());
            self.trusted_peers.lock().await.insert(peer_id_str, peer);
            Ok(())
        }

        async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
            match self.remove_trusted_peer_errors.get(peer_id) {
                Some(message) => Err(SendError::Internal(message.clone())),
                None => {
                    let removed_id = self.trusted_peer_ids.lock().await.remove(peer_id);
                    let removed_descriptor =
                        self.trusted_peers.lock().await.remove(peer_id).is_some();
                    Ok(removed_id || removed_descriptor)
                }
            }
        }

        async fn send(&self, cmd: CommsCommand) -> Result<meerkat_core::SendReceipt, SendError> {
            let receipt = match &cmd {
                CommsCommand::PeerResponse { in_reply_to, .. } => {
                    meerkat_core::SendReceipt::PeerResponseSent {
                        envelope_id: Uuid::new_v4(),
                        in_reply_to: *in_reply_to,
                    }
                }
                other => {
                    return Err(SendError::Unsupported(format!(
                        "BootstrapRuntime only records peer responses, got {}",
                        other.command_kind()
                    )));
                }
            };
            self.sent_commands.lock().await.push(cmd);
            Ok(receipt)
        }

        async fn peer_ingress_runtime_snapshot(
            &self,
        ) -> Result<
            meerkat_core::interaction::PeerIngressRuntimeSnapshot,
            meerkat_core::CommsCapabilityError,
        > {
            Ok(meerkat_core::interaction::PeerIngressRuntimeSnapshot {
                self_peer_id: self
                    .peer_id()
                    .unwrap_or_else(|| PeerId::parse(PEER_ID_RECEIVER).expect("valid peer id")),
                auth_required: true,
                authority_phase: meerkat_core::interaction::PeerIngressAuthorityPhase::Received,
                trusted_peers: self.trusted_peers.lock().await.values().cloned().collect(),
                submission_queue_len: 0,
                queue: meerkat_core::interaction::PeerIngressQueueSnapshot::default(),
            })
        }
    }

    fn bootstrap_runtime(
        peer_id: &str,
        address: &str,
        bootstrap_token: Option<&str>,
    ) -> BootstrapRuntime {
        let peer_handle = Arc::new(CountingPeerInteractionHandle::default());
        let peer_handle: Arc<dyn meerkat_core::handles::PeerInteractionHandle> = peer_handle;
        BootstrapRuntime {
            peer_id: peer_id.to_string(),
            address: address.to_string(),
            bootstrap_token: bootstrap_token.map(ToString::to_string),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
            add_trusted_peer_errors: HashMap::new(),
            remove_trusted_peer_errors: HashMap::new(),
            trusted_peer_ids: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            trusted_peers: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            sent_commands: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            peer_handle: Some(peer_handle.clone()),
            peer_request_response_handle: Some(peer_handle),
            completed_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    fn lifecycle_candidate(
        class: PeerInputClass,
        intent: &str,
        params: serde_json::Value,
    ) -> PeerInputCandidate {
        let id = InteractionId(Uuid::new_v4());
        let lifecycle_kind = match class {
            PeerInputClass::PeerLifecycleRetired => {
                meerkat_core::comms::PeerLifecycleKind::PeerRetired
            }
            PeerInputClass::PeerLifecycleUnwired => {
                meerkat_core::comms::PeerLifecycleKind::PeerUnwired
            }
            _ => meerkat_core::comms::PeerLifecycleKind::PeerAdded,
        };
        PeerInputCandidate {
            interaction: InboxInteraction {
                id,
                from_route: None,
                from: "test-mob/__mob_supervisor__".to_string(),
                content: InteractionContent::Request {
                    intent: intent.to_string(),
                    params,
                    blocks: None,
                },
                rendered_text: String::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
            },
            ingress: PeerIngressFact::peer(
                id,
                class,
                meerkat_core::PeerIngressKind::Request,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                PeerIngressIdentity::new(
                    PeerId::new(),
                    "test-mob/__mob_supervisor__",
                    meerkat_core::PeerIngressConvention::Lifecycle {
                        kind: lifecycle_kind,
                        peer: "peer-1".to_string(),
                    },
                ),
            ),
            lifecycle_peer: Some("peer-1".to_string()),
            response_terminality: None,
        }
    }

    #[test]
    fn completed_outcome_maps_structured_output_to_interaction_complete() {
        let interaction_id = InteractionId(Uuid::new_v4());
        let event = interaction_terminal_event(
            interaction_id,
            CompletionOutcome::Completed(Box::new(meerkat_core::RunResult {
                text: "{\"answer\":42}".to_string(),
                session_id: meerkat_core::SessionId::new(),
                usage: meerkat_core::Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: Some(json!({"answer": 42})),
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })),
        );

        match event {
            AgentEvent::InteractionComplete {
                interaction_id: actual_id,
                result,
                structured_output,
            } => {
                assert_eq!(actual_id, interaction_id);
                assert_eq!(result, "{\"answer\":42}");
                assert_eq!(structured_output, Some(json!({"answer": 42})));
            }
            other => panic!("expected InteractionComplete, got {other:?}"),
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
        let peer_spec = trusted_peer_from_runtime("peer-added", &peer);
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
            classified_interaction_to_runtime_input(&candidate, &LogicalRuntimeId::new("s-1"))
                .expect("lifecycle candidate should project to runtime input");
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
        let runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("receiver-removed").unwrap());
        let peer = meerkat_comms::CommsRuntime::inproc_only("peer-removed").unwrap();
        let peer_spec = trusted_peer_from_runtime("peer-removed", &peer);
        let dsl = test_projection_trust_dsl(runtime.as_ref());
        add_test_projection_trust_with_dsl(
            runtime.as_ref(),
            Arc::clone(&dsl),
            peer_spec.clone(),
            1,
            "trust peer",
        )
        .await;

        let unwired = lifecycle_candidate(
            PeerInputClass::PeerLifecycleUnwired,
            "mob.peer_unwired",
            json!({
                "peer": "peer-1",
                "peer_spec": peer_spec.clone(),
            }),
        );
        let _ = classified_interaction_to_runtime_input(&unwired, &LogicalRuntimeId::new("s-1"))
            .expect("unwired lifecycle candidate should project");
        assert!(
            runtime
                .peers()
                .await
                .iter()
                .any(|entry| entry.name.as_str() == "peer-removed"),
            "peer lifecycle unwire must not revoke comms trust before topology validation"
        );

        add_test_projection_trust_with_dsl(
            runtime.as_ref(),
            dsl,
            peer_spec.clone(),
            2,
            "refresh peer trust",
        )
        .await;
        let retired = lifecycle_candidate(
            PeerInputClass::PeerLifecycleRetired,
            "mob.peer_retired",
            json!({
                "peer": "peer-1",
                "peer_spec": peer_spec,
            }),
        );
        let _ = classified_interaction_to_runtime_input(&retired, &LogicalRuntimeId::new("s-1"))
            .expect("retired lifecycle candidate should project");
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
    fn bridge_capabilities_report_canonical_protocol_versions() {
        let capabilities = bridge_capabilities();
        assert_eq!(
            capabilities.current_protocol_version,
            supervisor_bridge_current_protocol_version()
        );
        assert_eq!(
            capabilities.default_protocol_version,
            supervisor_bridge_default_protocol_version()
        );
        assert_eq!(
            capabilities.supported_protocol_versions,
            supervisor_bridge_supported_protocol_versions()
        );
        assert!(
            !capabilities.hard_cancel_member,
            "supervisor bridge must not advertise live hard-cancel authority"
        );
    }

    #[tokio::test]
    async fn validate_bind_request_rejects_missing_or_wrong_bootstrap_token() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        let (adapter, session_id) = bind_material_adapter().await;
        let supervisor = supervisor_bridge_spec();
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "wrong-token".into(),
        };

        let (cause, error) = validate_bind_request(
            &adapter,
            &session_id,
            &runtime,
            &bridge_sender_fact(&supervisor.peer_id),
            &payload,
        )
        .await
        .expect_err("bind must reject incorrect bootstrap token");
        assert_eq!(cause, BridgeRejectionCause::InvalidBootstrapToken);
        assert!(
            error.contains("invalid bootstrap token"),
            "bind rejection should explain the bootstrap proof failure, got: {error}"
        );
    }

    #[tokio::test]
    async fn validate_bind_request_accepts_matching_bootstrap_token() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        let (adapter, session_id) = bind_material_adapter().await;
        let supervisor = supervisor_bridge_spec();
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "expected-token".into(),
        };

        let (authorized, advertised_address) = validate_bind_request(
            &adapter,
            &session_id,
            &runtime,
            &bridge_sender_fact(&supervisor.peer_id),
            &payload,
        )
        .await
        .expect("bind should accept the configured bootstrap token");
        assert_eq!(authorized.peer_id.as_str(), supervisor.peer_id);
        assert_eq!(advertised_address, runtime.advertised_address().unwrap());
    }

    #[test]
    fn bridge_delivery_payload_preserves_explicit_queue_handling_mode() {
        let input = peer_input_from_delivery_payload(
            &SessionId::new(),
            PeerId::parse(PEER_ID_SUPERVISOR).expect("valid supervisor peer id"),
            InteractionId(Uuid::new_v4()),
            BridgeDeliveryPayload {
                supervisor: supervisor_bridge_spec(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                input_id: "bridge-delivery-queue-test".to_string(),
                content: meerkat_core::types::ContentInput::Text("queued follow-up".to_string()),
                handling_mode: HandlingMode::Queue,
            },
        );

        let Input::Peer(peer) = input else {
            panic!("bridge delivery must project to peer input");
        };
        assert_eq!(
            peer.handling_mode,
            Some(HandlingMode::Queue),
            "bridge delivery explicit queue must survive into MeerkatMachine admission"
        );
    }

    #[test]
    fn bridge_delivery_handler_does_not_wait_for_completion_inline() {
        let source = include_str!("comms_drain.rs");
        let deliver_start = source
            .find("BridgeCommand::DeliverMemberInput")
            .expect("deliver command branch exists");
        let deliver_end = source[deliver_start..]
            .find("fn bridge_runtime_state")
            .map(|offset| deliver_start + offset)
            .expect("bridge runtime state helper follows command match");
        let deliver_source = &source[deliver_start..deliver_end];
        assert!(
            !deliver_source.contains(".wait().await"),
            "bridge delivery admission must not block the comms drain waiting for completion"
        );
    }

    #[tokio::test]
    async fn validate_bind_request_rejects_same_display_name_different_canonical_sender() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        let (adapter, session_id) = bind_material_adapter().await;
        let supervisor = supervisor_bridge_spec();
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "expected-token".into(),
        };
        let attacker_peer_id =
            PeerId::parse(PEER_ID_OLD_SUPERVISOR).expect("valid attacker peer id");
        let sender = bridge_sender_fact_with_display(attacker_peer_id, &supervisor.name);

        let (cause, error) =
            validate_bind_request(&adapter, &session_id, &runtime, &sender, &payload)
                .await
                .expect_err("same display name with a different canonical sender must reject");
        assert_eq!(cause, BridgeRejectionCause::SenderMismatch);
        assert!(
            error.contains("does not match supervisor"),
            "bind rejection should explain the sender mismatch, got: {error}"
        );
    }

    #[tokio::test]
    async fn validate_bind_request_accepts_pubkey_sender_with_canonical_supervisor_peer_id() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        let (adapter, session_id) = bind_material_adapter().await;
        let supervisor_key = meerkat_comms::Keypair::generate();
        let supervisor_pubkey = supervisor_key.public_key();
        let supervisor = BridgePeerSpec {
            name: "mob/__mob_supervisor__".to_string(),
            peer_id: supervisor_pubkey.to_peer_id().as_str(),
            address: "inproc://mob/__mob_supervisor__".to_string(),
            pubkey: *supervisor_pubkey.as_bytes(),
        };
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "expected-token".into(),
        };

        let (authorized, advertised_address) = validate_bind_request(
            &adapter,
            &session_id,
            &runtime,
            &bridge_sender_fact(&supervisor_pubkey.to_pubkey_string()),
            &payload,
        )
        .await
        .expect("bind should accept raw transport sender when payload carries pubkey");
        assert_eq!(authorized.peer_id.as_str(), supervisor.peer_id);
        assert_eq!(authorized.pubkey, supervisor.pubkey);
        assert_eq!(advertised_address, runtime.advertised_address().unwrap());
    }

    #[tokio::test]
    async fn validate_bind_request_returns_runtime_advertised_address() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            &format!(
                "inproc://receiver-real?{SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM}=expected-token"
            ),
            Some("expected-token"),
        ));
        let (adapter, session_id) = bind_material_adapter().await;
        let supervisor = supervisor_bridge_spec();
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: "inproc://receiver-real".to_string(),
            bootstrap_token: "expected-token".into(),
        };

        let (_, advertised_address) = validate_bind_request(
            &adapter,
            &session_id,
            &runtime,
            &bridge_sender_fact(&supervisor.peer_id),
            &payload,
        )
        .await
        .expect("bind should canonicalize to the callee's advertised address");
        assert_eq!(advertised_address, runtime.advertised_address().unwrap());
    }

    #[tokio::test]
    async fn validate_bind_request_rejects_mismatched_expected_address() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver-real",
            Some("expected-token"),
        ));
        let (adapter, session_id) = bind_material_adapter().await;
        let supervisor = supervisor_bridge_spec();
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: "inproc://receiver-stale".to_string(),
            bootstrap_token: "expected-token".into(),
        };

        let (cause, error) = validate_bind_request(
            &adapter,
            &session_id,
            &runtime,
            &bridge_sender_fact(&supervisor.peer_id),
            &payload,
        )
        .await
        .expect_err("bind should reject mismatched expected addresses");
        assert_eq!(cause, BridgeRejectionCause::AddressMismatch);
        assert!(
            error.contains("bind address mismatch"),
            "bind rejection should explain the address mismatch, got: {error}"
        );
    }

    #[tokio::test]
    async fn bridge_handler_rejects_unsupported_protocol_version_before_bind_validation() {
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let params = json!({
            "command": "bind_member",
            "supervisor": {
                "name": "mob/__mob_supervisor__",
                "peer_id": PEER_ID_SUPERVISOR,
                "address": "inproc://mob/__mob_supervisor__",
                "pubkey": vec![0u8; 32],
            },
            "epoch": 0,
            "protocol_version": 999,
            "expected_peer_id": PEER_ID_RECEIVER,
            "expected_address": "inproc://receiver",
            "bootstrap_token": "expected-token",
        });
        let interaction_id = InteractionId(Uuid::new_v4());
        let candidate = PeerInputCandidate {
            interaction: InboxInteraction {
                id: interaction_id,
                from_route: None,
                from: PEER_ID_SUPERVISOR.to_string(),
                content: InteractionContent::Request {
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                    params,
                    blocks: None,
                },
                rendered_text: String::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
            },
            ingress: bridge_sender_fact_with_id(interaction_id, PEER_ID_SUPERVISOR),
            lifecycle_peer: None,
            response_terminality: None,
        };

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate).await,
            "bridge handler must own malformed supervisor bridge commands"
        );

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
            .expect("handler must send a typed rejection response");
        assert!(matches!(
            status,
            meerkat_core::interaction::ResponseStatus::Failed
        ));
        let reply: BridgeReply = serde_json::from_value(result).expect("typed bridge reply");
        match reply {
            BridgeReply::Rejected { cause, reason } => {
                assert_eq!(cause, BridgeRejectionCause::UnsupportedProtocolVersion);
                assert!(
                    reason.contains("unsupported supervisor bridge protocol version"),
                    "rejection should explain the typed version failure, got: {reason}"
                );
            }
            other => unreachable!("expected Rejected reply, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_bind_request_rejects_invalid_supervisor_peer_name() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        let (adapter, session_id) = bind_material_adapter().await;
        let mut supervisor = supervisor_bridge_spec();
        supervisor.name = String::new();
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor,
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "expected-token".into(),
        };

        let (cause, error) = validate_bind_request(
            &adapter,
            &session_id,
            &runtime,
            &bridge_sender_fact(&payload.supervisor.peer_id),
            &payload,
        )
        .await
        .expect_err("bind should reject invalid supervisor peer names");
        assert_eq!(cause, BridgeRejectionCause::InvalidSupervisorSpec);
        assert!(
            error.contains("invalid supervisor peer spec"),
            "bind rejection should explain invalid supervisor identity, got: {error}"
        );
    }

    fn sample_bind_payload() -> meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
        meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor_bridge_spec(),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: "inproc://receiver".to_string(),
            bootstrap_token: "expected-token".into(),
        }
    }

    /// Ephemeral MeerkatMachine with one registered (unbound) session, for
    /// exercising `validate_bind_request` through the machine-backed material
    /// admission seam. The material verdict is a pure boolean classifier
    /// independent of supervisor binding state, so an unbound session suffices.
    async fn bind_material_adapter() -> (Arc<MeerkatMachine>, SessionId) {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        (adapter, session_id)
    }

    /// The material bind-admission verdict is MeerkatMachine-owned: each of the
    /// five outcomes (Accept + four rejections) must be emitted from the four
    /// pure boolean observations, and the precedence (address → sender →
    /// peer-id → token) must hold so the FIRST failing check wins exactly as
    /// the shell short-circuited before the fold.
    #[tokio::test]
    async fn machine_owns_material_bind_admission_verdict_and_precedence() {
        use crate::meerkat_machine::dsl::SupervisorBindMaterialAdmissionKind as Verdict;

        let (adapter, session_id) = bind_material_adapter().await;

        let verdict = |address, sender, peer_id, token| {
            let adapter = adapter.clone();
            let session_id = session_id.clone();
            async move {
                adapter
                    .resolve_supervisor_bind_material_admission(
                        &session_id,
                        address,
                        sender,
                        peer_id,
                        token,
                    )
                    .await
                    .expect("machine must emit a material bind-admission verdict")
            }
        };

        // All four observations true -> Accept.
        assert_eq!(verdict(true, true, true, true).await, Verdict::Accept);

        // Single failures map to their typed verdict.
        assert_eq!(
            verdict(false, true, true, true).await,
            Verdict::AddressMismatch
        );
        assert_eq!(
            verdict(true, false, true, true).await,
            Verdict::SenderMismatch
        );
        assert_eq!(
            verdict(true, true, false, true).await,
            Verdict::InvalidPeerSpec
        );
        assert_eq!(
            verdict(true, true, true, false).await,
            Verdict::InvalidBootstrapToken
        );

        // Precedence: address beats every later check.
        assert_eq!(
            verdict(false, false, false, false).await,
            Verdict::AddressMismatch
        );
        // Sender beats peer-id and token once address matches.
        assert_eq!(
            verdict(true, false, false, false).await,
            Verdict::SenderMismatch
        );
        // Peer-id beats token once address + sender match.
        assert_eq!(
            verdict(true, true, false, false).await,
            Verdict::InvalidPeerSpec
        );
    }

    #[tokio::test]
    async fn bridge_request_without_complete_peer_authority_fails_before_dispatch() {
        for install_peer_handle in [false, true] {
            let payload = sample_bind_payload();
            let sender = payload.supervisor.peer_id.clone();
            let mut runtime_impl = bootstrap_runtime(
                PEER_ID_RECEIVER,
                "inproc://receiver",
                Some("expected-token"),
            );
            let peer_handle = Arc::new(CountingPeerInteractionHandle::default());
            if install_peer_handle {
                let peer_handle: Arc<dyn meerkat_core::handles::PeerInteractionHandle> =
                    peer_handle.clone();
                runtime_impl.peer_handle = Some(peer_handle);
            } else {
                runtime_impl.peer_handle = None;
            }
            runtime_impl.peer_request_response_handle = None;
            let completed_count = runtime_impl.completed_count.clone();
            let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
            let adapter = Arc::new(MeerkatMachine::ephemeral());
            let session_id = SessionId::new();
            adapter
                .register_session(session_id.clone())
                .await
                .expect("register session");
            let candidate = bridge_candidate(&sender, &BridgeCommand::BindMember(payload));

            assert!(
                try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate)
                    .await,
                "bridge handler must own bridge requests even when rejecting authority"
            );
            assert!(
                matches!(
                    adapter.supervisor_binding(&session_id).await,
                    SupervisorBinding::Unbound
                ),
                "bridge request must not mutate supervisor binding without complete authority"
            );
            assert_eq!(
                peer_handle.request_received_count(),
                0,
                "peer-only authority must not record PeerRequestReceived through bridge dispatch"
            );
            assert_eq!(
                completed_count.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "bridge request rejected at the authority boundary should be marked complete"
            );
        }
    }

    #[tokio::test]
    async fn bridge_request_rejected_by_peer_authority_fails_before_dispatch() {
        let payload = sample_bind_payload();
        let sender = payload.supervisor.peer_id.clone();
        let mut runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        let peer_handle = Arc::new(CountingPeerInteractionHandle::rejecting_request_received());
        let peer_handle: Arc<dyn meerkat_core::handles::PeerInteractionHandle> = peer_handle;
        runtime_impl.peer_handle = Some(peer_handle.clone());
        runtime_impl.peer_request_response_handle = Some(peer_handle);
        let completed_count = runtime_impl.completed_count.clone();
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let candidate = bridge_candidate(&sender, &BridgeCommand::BindMember(payload));

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate).await,
            "bridge handler must own bridge requests even when rejecting authority"
        );
        assert!(
            matches!(
                adapter.supervisor_binding(&session_id).await,
                SupervisorBinding::Unbound
            ),
            "bridge request must not mutate supervisor binding when PeerRequestReceived is rejected"
        );
        assert_eq!(
            completed_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "bridge request rejected by peer authority should be marked complete"
        );
    }

    async fn bind_sample_supervisor(
        adapter: &MeerkatMachine,
        session_id: &SessionId,
        payload: &meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload,
    ) {
        let spec = TrustedPeerDescriptor::try_from(payload.supervisor.clone())
            .expect("valid supervisor spec");
        adapter
            .stage_supervisor_bind(
                session_id,
                spec.name.to_string(),
                spec.peer_id.as_str(),
                spec.address.to_string(),
                signing_public_key_for_descriptor(&spec),
                payload.epoch,
            )
            .await
            .expect("supervisor bound");
    }

    #[tokio::test]
    async fn generated_bind_admission_allows_bootstrap_when_unbound() {
        let payload = sample_bind_payload();
        let supervisor = bridge_peer_identity(&payload.supervisor, "test").expect("valid peer");
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let sender = bridge_sender_fact(&payload.supervisor.peer_id);
        let admission = adapter
            .resolve_supervisor_bind_admission(
                &session_id,
                supervisor.peer_id.as_str().clone(),
                payload.epoch,
                generated_sender_peer_id(&sender),
            )
            .await
            .expect("generated bind admission");
        assert!(matches!(admission, SupervisorBindAdmission::Bootstrap));
    }

    #[tokio::test]
    async fn generated_bind_admission_rejects_different_supervisor_takeover() {
        // Scenario: a stale bootstrap-token holder tries to seize authority
        // by calling BindMember with a DIFFERENT supervisor peer_id. This is
        // exactly the review vulnerability and must be rejected.
        let current_payload = sample_bind_payload();
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_sample_supervisor(&adapter, &session_id, &current_payload).await;
        let mut takeover = sample_bind_payload();
        takeover.supervisor = old_supervisor_bridge_spec();
        let supervisor = bridge_peer_identity(&takeover.supervisor, "test").expect("valid peer");
        let sender = bridge_sender_fact(&takeover.supervisor.peer_id);
        let admission = adapter
            .resolve_supervisor_bind_admission(
                &session_id,
                supervisor.peer_id.as_str().clone(),
                takeover.epoch,
                generated_sender_peer_id(&sender),
            )
            .await
            .expect("generated bind admission");
        assert!(
            matches!(
                admission,
                SupervisorBindAdmission::Rejected(
                    crate::meerkat_machine::dsl::SupervisorBindRejectionKind::AlreadyBound
                )
            ),
            "different-supervisor bind must reject as already bound: {admission:?}"
        );
    }

    #[tokio::test]
    async fn generated_bind_admission_rejects_lower_epoch_replay() {
        // Scenario: replay an earlier bind with a lower epoch to regress
        // member state. Must be rejected even when the supervisor peer_id
        // matches — otherwise an attacker can downgrade the member epoch.
        let current_payload = sample_bind_payload();
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_sample_supervisor(&adapter, &session_id, &current_payload).await;
        let mut replay = sample_bind_payload();
        replay.epoch = current_payload.epoch - 1;
        let supervisor = bridge_peer_identity(&replay.supervisor, "test").expect("valid peer");
        let sender = bridge_sender_fact(&replay.supervisor.peer_id);
        let admission = adapter
            .resolve_supervisor_bind_admission(
                &session_id,
                supervisor.peer_id.as_str().clone(),
                replay.epoch,
                generated_sender_peer_id(&sender),
            )
            .await
            .expect("generated bind admission");
        assert!(
            matches!(
                admission,
                SupervisorBindAdmission::Rejected(
                    crate::meerkat_machine::dsl::SupervisorBindRejectionKind::AlreadyBound
                )
            ),
            "lower-epoch bind replay must reject as already bound: {admission:?}"
        );
    }

    #[tokio::test]
    async fn generated_bind_admission_rejects_higher_epoch_same_supervisor_rebind() {
        // Scenario: same supervisor tries to self-bump epoch via BindMember.
        // Rotation must go through AuthorizeSupervisor — BindMember is only
        // for initial bootstrap after member restart.
        let current_payload = sample_bind_payload();
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_sample_supervisor(&adapter, &session_id, &current_payload).await;
        let mut advance = sample_bind_payload();
        advance.epoch = current_payload.epoch + 5;
        let supervisor = bridge_peer_identity(&advance.supervisor, "test").expect("valid peer");
        let sender = bridge_sender_fact(&advance.supervisor.peer_id);
        let admission = adapter
            .resolve_supervisor_bind_admission(
                &session_id,
                supervisor.peer_id.as_str().clone(),
                advance.epoch,
                generated_sender_peer_id(&sender),
            )
            .await
            .expect("generated bind admission");
        assert!(
            matches!(
                admission,
                SupervisorBindAdmission::Rejected(
                    crate::meerkat_machine::dsl::SupervisorBindRejectionKind::AlreadyBound
                )
            ),
            "higher-epoch rebind must reject as already bound: {admission:?}"
        );
    }

    #[tokio::test]
    async fn generated_bind_admission_rejects_spoofed_sender() {
        // Scenario: an attacker forges a BindMember that matches the
        // current supervisor spec but signs it with their own key.
        // sender != authorized supervisor → must reject.
        let current_payload = sample_bind_payload();
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_sample_supervisor(&adapter, &session_id, &current_payload).await;
        let retry = sample_bind_payload();
        let supervisor = bridge_peer_identity(&retry.supervisor, "test").expect("valid peer");
        let attacker_peer_id = PeerId::new().as_str();
        let sender = bridge_sender_fact(&attacker_peer_id);
        let admission = adapter
            .resolve_supervisor_bind_admission(
                &session_id,
                supervisor.peer_id.as_str().clone(),
                retry.epoch,
                generated_sender_peer_id(&sender),
            )
            .await
            .expect("generated bind admission");
        assert!(
            matches!(
                admission,
                SupervisorBindAdmission::Rejected(
                    crate::meerkat_machine::dsl::SupervisorBindRejectionKind::SenderMismatch
                )
            ),
            "bind from unauthorized sender must reject as sender mismatch: {admission:?}"
        );
    }

    #[tokio::test]
    async fn generated_bind_admission_rejects_same_display_name_different_canonical_sender() {
        let current_payload = sample_bind_payload();
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_sample_supervisor(&adapter, &session_id, &current_payload).await;
        let retry = sample_bind_payload();
        let supervisor = bridge_peer_identity(&retry.supervisor, "test").expect("valid peer");
        let attacker_peer_id =
            PeerId::parse(PEER_ID_OLD_SUPERVISOR).expect("valid attacker peer id");
        let sender = bridge_sender_fact_with_display(attacker_peer_id, &retry.supervisor.name);

        let admission = adapter
            .resolve_supervisor_bind_admission(
                &session_id,
                supervisor.peer_id.as_str().clone(),
                retry.epoch,
                generated_sender_peer_id(&sender),
            )
            .await
            .expect("generated bind admission");
        assert!(
            matches!(
                admission,
                SupervisorBindAdmission::Rejected(
                    crate::meerkat_machine::dsl::SupervisorBindRejectionKind::SenderMismatch
                )
            ),
            "same display name with different canonical sender must reject: {admission:?}"
        );
    }

    #[tokio::test]
    async fn generated_bind_admission_idempotently_acks_retry_from_current_supervisor() {
        // Scenario: the authorized supervisor retries BindMember with the
        // exact same payload (e.g., transport-level retry). Must return
        // idempotent ack without mutating state.
        let current_payload = sample_bind_payload();
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_sample_supervisor(&adapter, &session_id, &current_payload).await;
        let retry = sample_bind_payload();
        let supervisor = bridge_peer_identity(&retry.supervisor, "test").expect("valid peer");
        let sender = bridge_sender_fact(&retry.supervisor.peer_id);
        let admission = adapter
            .resolve_supervisor_bind_admission(
                &session_id,
                supervisor.peer_id.as_str().clone(),
                retry.epoch,
                generated_sender_peer_id(&sender),
            )
            .await
            .expect("generated bind admission");
        assert!(matches!(admission, SupervisorBindAdmission::IdempotentAck));
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let current_supervisor =
            trusted_peer_from_runtime("mob/__mob_supervisor__", &supervisor_runtime);
        adapter
            .stage_supervisor_bind(
                &session_id,
                current_supervisor.name.to_string(),
                current_supervisor.peer_id.as_str(),
                current_supervisor.address.to_string(),
                signing_public_key_for_descriptor(&current_supervisor),
                1,
            )
            .await
            .expect("initial bind must succeed");
        let adversary_supervisor = old_supervisor_bridge_spec();
        let adversary_peer_id = adversary_supervisor.peer_id.clone();
        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                supervisor: adversary_supervisor,
                epoch: 2,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                expected_peer_id: runtime
                    .peer_id()
                    .map(|peer_id| peer_id.as_str())
                    .unwrap_or_else(|| PEER_ID_RECEIVER.to_string()),
                expected_address: runtime
                    .advertised_address()
                    .unwrap_or_else(|| "inproc://bind-rebind-receiver".to_string()),
                // Even with a *valid* bootstrap token, the rebind must fail.
                bootstrap_token: runtime.bridge_bootstrap_token().unwrap_or_default().into(),
            },
        );
        let candidate = bridge_candidate(&adversary_peer_id, &command);

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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let current = trusted_supervisor_descriptor(0xbb);
        adapter
            .stage_supervisor_bind(
                &session_id,
                current.name.to_string(),
                current.peer_id.as_str(),
                current.address.to_string(),
                signing_public_key_for_descriptor(&current),
                1,
            )
            .await
            .expect("initial bind must succeed");

        // A different supervisor tries to rebind. Under the strict gate this
        // must be rejected with typed `AlreadyBound` — the mob-side bridge
        // fallback logic branches on the typed cause, not on reason text.
        let adversary = old_supervisor_bridge_spec();
        let adversary_peer_id = adversary.peer_id.clone();
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
        let candidate = bridge_candidate(&adversary_peer_id, &command);

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
        fn peer_id(&self) -> Option<PeerId> {
            PeerId::parse(&self.peer_id).ok()
        }

        fn public_key(&self) -> Option<String> {
            test_public_key_for_peer_id(&self.peer_id)
        }

        fn public_key_bytes(&self) -> Option<[u8; 32]> {
            test_pubkey_bytes_for_peer_id(&self.peer_id)
        }

        fn comms_name(&self) -> Option<String> {
            self.advertised_address
                .as_deref()
                .map(test_comms_name_from_address)
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

        fn peer_request_response_authority_handle(
            &self,
        ) -> Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>> {
            Some(Arc::new(CountingPeerInteractionHandle::default()))
        }

        async fn add_trusted_peer(&self, _peer: TrustedPeerDescriptor) -> Result<(), SendError> {
            Ok(())
        }

        async fn remove_trusted_peer(&self, _peer_id: &str) -> Result<bool, SendError> {
            Ok(true)
        }

        async fn apply_trust_mutation(
            &self,
            mutation: CommsTrustMutation,
        ) -> Result<CommsTrustMutationResult, SendError> {
            match mutation {
                CommsTrustMutation::AddPrivateTrustedPeer { peer, authority } => {
                    authority
                        .validate_private_add(self.peer_id(), &peer)
                        .map_err(SendError::Validation)?;
                    Ok(CommsTrustMutationResult::Added { created: true })
                }
                CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority } => {
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|error| SendError::Validation(error.to_string()))?;
                    authority
                        .validate_private_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    Ok(CommsTrustMutationResult::Removed { removed: true })
                }
                _ => Err(SendError::Unsupported(
                    "test runtime only supports generated private trust".into(),
                )),
            }
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
    async fn bind_member_handler_response_reports_canonical_protocol_versions() {
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let supervisor = supervisor_bridge_spec();
        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                supervisor: supervisor.clone(),
                epoch: 0,
                protocol_version: supervisor_bridge_default_protocol_version(),
                expected_peer_id: PEER_ID_RECEIVER.to_string(),
                expected_address: "inproc://receiver".to_string(),
                bootstrap_token: "expected-token".into(),
            },
        );
        let candidate = bridge_candidate(&supervisor.peer_id, &command);

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate,)
                .await,
            "bridge handler must own the BindMember command"
        );

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
            .expect("handler must send a bind response");
        assert!(matches!(
            status,
            meerkat_core::interaction::ResponseStatus::Completed
        ));
        let reply: BridgeReply = serde_json::from_value(result).expect("typed bridge reply");
        let BridgeReply::BindMember(response) = reply else {
            panic!("expected bind response");
        };
        assert_eq!(
            response.capabilities.current_protocol_version,
            supervisor_bridge_current_protocol_version()
        );
        assert_eq!(
            response.capabilities.default_protocol_version,
            supervisor_bridge_default_protocol_version()
        );
        assert_eq!(
            response.capabilities.supported_protocol_versions,
            supervisor_bridge_supported_protocol_versions()
        );
        assert!(
            !response.capabilities.hard_cancel_member,
            "bind response must not advertise live hard-cancel authority"
        );
    }

    #[tokio::test]
    async fn hard_cancel_member_bridge_command_is_rejected_as_unsupported() {
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
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
                signing_public_key_for_descriptor(&authorized),
                11,
            )
            .await
            .expect("pre-bind supervisor");
        let command = BridgeCommand::HardCancelMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeHardCancelPayload {
                supervisor: BridgePeerSpec::from(authorized.clone()),
                epoch: 11,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                reason: "must not cross supervisor bridge".to_string(),
            },
        );
        let candidate = bridge_candidate(&authorized.peer_id.as_str(), &command);

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate,)
                .await,
            "bridge handler must reject the known but unsupported HardCancelMember command"
        );
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
            .expect("handler must send a hard-cancel rejection");
        assert!(
            matches!(status, meerkat_core::interaction::ResponseStatus::Failed),
            "unsupported hard-cancel bridge command must fail at the comms boundary"
        );
        let reply: BridgeReply = serde_json::from_value(result).expect("typed bridge reply");
        match reply {
            BridgeReply::Rejected { cause, .. } => {
                assert_eq!(cause, BridgeRejectionCause::Unsupported);
            }
            other => unreachable!("expected Rejected reply, got {other:?}"),
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let authorized = trusted_supervisor_descriptor(0xbb);
        adapter
            .stage_supervisor_bind(
                &session_id,
                authorized.name.to_string(),
                authorized.peer_id.as_str(),
                authorized.address.to_string(),
                signing_public_key_for_descriptor(&authorized),
                7,
            )
            .await
            .expect("pre-bind supervisor");
        // Attacker sets expected_address/expected_peer_id to unique tokens we
        // can grep for in the reply to prove they are NOT echoed back.
        let attacker_address = "inproc://ATTACKER-ADDRESS-DO-NOT-ECHO".to_string();
        let attacker_peer_id = format!("{}-ATTACKER-PEER-ID-DO-NOT-ECHO", PeerId::new().as_str());
        let command = BridgeCommand::BindMember(
            meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
                // Sender/supervisor match stored state so validation returns
                // IdempotentAck; the invariant then fires because the runtime
                // cannot produce its own canonical identity.
                supervisor: BridgePeerSpec::from(authorized.clone()),
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

    #[tokio::test]
    async fn generated_authorize_admission_rejects_initial_claim_without_bind() {
        let payload = BridgeSupervisorPayload {
            supervisor: supervisor_bridge_spec(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        };
        let supervisor = bridge_peer_identity(&payload.supervisor, "test").expect("valid peer");
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let sender = bridge_sender_fact(&payload.supervisor.peer_id);

        assert!(
            matches!(
                adapter
                    .resolve_supervisor_authorize_admission(
                        &session_id,
                        supervisor.peer_id.as_str().clone(),
                        payload.epoch,
                        generated_sender_peer_id(&sender),
                    )
                    .await
                    .expect("generated authorize admission"),
                SupervisorAuthorizeAdmission::Rejected(
                    crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind::NotBound
                )
            ),
            "first supervisor claim must go through bind_member"
        );
    }

    #[tokio::test]
    async fn generated_authorize_admission_rejects_same_display_name_different_canonical_sender() {
        let current_payload = sample_bind_payload();
        let payload = BridgeSupervisorPayload {
            supervisor: current_supervisor_bridge_spec(),
            epoch: current_payload.epoch + 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        };
        let supervisor = bridge_peer_identity(&payload.supervisor, "test").expect("valid peer");
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        bind_sample_supervisor(&adapter, &session_id, &current_payload).await;
        let attacker_peer_id =
            PeerId::parse(PEER_ID_OLD_SUPERVISOR).expect("valid attacker peer id");
        let sender =
            bridge_sender_fact_with_display(attacker_peer_id, &current_payload.supervisor.name);

        assert!(
            matches!(
                adapter
                    .resolve_supervisor_authorize_admission(
                        &session_id,
                        supervisor.peer_id.as_str().clone(),
                        payload.epoch,
                        generated_sender_peer_id(&sender),
                    )
                    .await
                    .expect("generated authorize admission"),
                SupervisorAuthorizeAdmission::Rejected(
                    crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind::SenderMismatch
                )
            ),
            "same display name with a different canonical sender must reject"
        );
    }

    #[tokio::test]
    async fn supervisor_bridge_admission_rejects_same_display_name_different_canonical_sender() {
        let current_payload = sample_bind_payload();
        let payload = BridgeSupervisorPayload {
            supervisor: current_payload.supervisor.clone(),
            epoch: current_payload.epoch,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        };
        let attacker_peer_id =
            PeerId::parse(PEER_ID_OLD_SUPERVISOR).expect("valid attacker peer id");
        let sender =
            bridge_sender_fact_with_display(attacker_peer_id, &current_payload.supervisor.name);

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let supervisor = TrustedPeerDescriptor::try_from(current_payload.supervisor.clone())
            .expect("valid supervisor spec");
        adapter
            .stage_supervisor_bind(
                &session_id,
                supervisor.name.to_string(),
                supervisor.peer_id.as_str(),
                supervisor.address.to_string(),
                signing_public_key_for_descriptor(&supervisor),
                current_payload.epoch,
            )
            .await
            .expect("supervisor bound");

        let (cause, error) =
            resolve_authorized_supervisor(&adapter, &session_id, &sender, &payload)
                .await
                .expect_err("same display name with a different canonical sender must reject");
        assert_eq!(cause, BridgeRejectionCause::SenderMismatch);
        assert!(
            error.contains("MeerkatMachine"),
            "supervisor command rejection should explain the sender mismatch, got: {error}"
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

    #[tokio::test]
    async fn validate_bind_request_rejects_empty_bootstrap_token_at_runtime() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some(""),
        ));
        let (adapter, session_id) = bind_material_adapter().await;
        let supervisor = supervisor_bridge_spec();
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: runtime.advertised_address().unwrap(),
            bootstrap_token: "whatever".into(),
        };

        let (cause, _error) = validate_bind_request(
            &adapter,
            &session_id,
            &runtime,
            &bridge_sender_fact(&supervisor.peer_id),
            &payload,
        )
        .await
        .expect_err("runtime with empty token must refuse to validate");
        assert_eq!(cause, BridgeRejectionCause::InvalidBootstrapToken);
    }

    #[tokio::test]
    async fn validate_bind_request_rejects_query_string_bootstrap_without_typed_runtime_token() {
        let runtime: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            &format!("inproc://receiver?{SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM}=expected-token"),
            None,
        ));
        let (adapter, session_id) = bind_material_adapter().await;
        let supervisor = supervisor_bridge_spec();
        let payload = meerkat_contracts::wire::supervisor_bridge::BridgeBindPayload {
            supervisor: supervisor.clone(),
            epoch: 0,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: PEER_ID_RECEIVER.to_string(),
            expected_address: "inproc://receiver".to_string(),
            bootstrap_token: "expected-token".into(),
        };

        let (cause, error) = validate_bind_request(
            &adapter,
            &session_id,
            &runtime,
            &bridge_sender_fact(&supervisor.peer_id),
            &payload,
        )
        .await
        .expect_err("query-string token must not satisfy typed bootstrap proof");
        assert_eq!(cause, BridgeRejectionCause::InvalidBootstrapToken);
        assert!(
            error.contains("typed bridge bootstrap token"),
            "runtime should explain that the typed token field is required, got: {error}"
        );
    }

    fn bridge_candidate(sender: &str, command: &BridgeCommand) -> PeerInputCandidate {
        let id = InteractionId(Uuid::new_v4());
        let ingress = bridge_sender_fact_with_id(id, sender);
        bridge_candidate_with_ingress(sender, command, ingress)
    }

    fn bridge_candidate_with_ingress(
        sender: &str,
        command: &BridgeCommand,
        ingress: PeerIngressFact,
    ) -> PeerInputCandidate {
        let id = ingress.interaction_id;
        PeerInputCandidate {
            interaction: InboxInteraction {
                id,
                from_route: None,
                from: sender.to_string(),
                content: InteractionContent::Request {
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                    params: serde_json::to_value(command).expect("serialize bridge command"),
                    blocks: None,
                },
                rendered_text: String::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
            },
            ingress,
            lifecycle_peer: None,
            response_terminality: None,
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
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
    async fn bind_member_invalid_bootstrap_does_not_publish_response_route() {
        let mut payload = sample_bind_payload();
        payload.bootstrap_token = "wrong-token".into();
        let sender = payload.supervisor.peer_id.clone();
        let runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        let trusted_peer_ids = runtime_impl.trusted_peer_ids.clone();
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let candidate = bridge_candidate(&sender, &BridgeCommand::BindMember(payload));

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate).await
        );
        assert!(
            matches!(
                adapter.supervisor_binding(&session_id).await,
                SupervisorBinding::Unbound
            ),
            "invalid bootstrap must not bind supervisor authority"
        );
        assert!(
            trusted_peer_ids.lock().await.is_empty(),
            "invalid bootstrap must not temporarily publish supervisor trust just to route rejection"
        );
    }

    #[tokio::test]
    async fn authorize_supervisor_restores_old_binding_when_new_trust_publish_fails() {
        // DOGMA-19 defensive scan: the old supervisor remains authoritative
        // until the new supervisor trust publishes successfully.
        let old_supervisor = trusted_supervisor_descriptor(0xbb);
        let new_supervisor = current_supervisor_bridge_spec();
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        adapter
            .stage_supervisor_bind(
                &session_id,
                old_supervisor.name.to_string(),
                old_supervisor.peer_id.as_str(),
                old_supervisor.address.to_string(),
                signing_public_key_for_descriptor(&old_supervisor),
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
        let old_supervisor = trusted_supervisor_descriptor(0xbb);
        let new_supervisor = current_supervisor_bridge_spec();
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        adapter
            .stage_supervisor_bind(
                &session_id,
                old_supervisor.name.to_string(),
                old_supervisor.peer_id.as_str(),
                old_supervisor.address.to_string(),
                signing_public_key_for_descriptor(&old_supervisor),
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
    async fn authorize_supervisor_same_peer_higher_epoch_failure_keeps_existing_response_route() {
        let supervisor = current_supervisor_bridge_spec();
        let payload = BridgeSupervisorPayload {
            supervisor: supervisor.clone(),
            epoch: 2,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        };
        let runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        let trusted_peer_ids = runtime_impl.trusted_peer_ids.clone();
        trusted_peer_ids
            .lock()
            .await
            .insert(supervisor.peer_id.clone());
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        adapter
            .stage_supervisor_bind(
                &session_id,
                supervisor.name.clone(),
                supervisor.peer_id.clone(),
                supervisor.address.clone(),
                encode_supervisor_signing_public_key(supervisor.pubkey),
                1,
            )
            .await
            .expect("pre-bind supervisor");
        let candidate = bridge_candidate(
            &supervisor.peer_id,
            &BridgeCommand::AuthorizeSupervisor(payload),
        );

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate).await
        );
        match adapter.supervisor_binding(&session_id).await {
            SupervisorBinding::Bound { peer_id, epoch, .. } => {
                assert_eq!(peer_id, supervisor.peer_id);
                assert_eq!(epoch, 1);
            }
            SupervisorBinding::Unbound => panic!("supervisor must remain bound"),
        }
        assert!(
            trusted_peer_ids.lock().await.contains(&supervisor.peer_id),
            "same-supervisor epoch update must keep the existing response route installed"
        );
    }

    #[tokio::test]
    async fn revoke_supervisor_keeps_authority_when_trust_removal_fails() {
        let supervisor = supervisor_bridge_spec();
        let mut runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        runtime_impl
            .remove_trusted_peer_errors
            .insert(supervisor.peer_id.clone(), "boom".to_string());
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
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
                signing_public_key_for_descriptor(&spec),
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        // Start Unbound → Bound.
        adapter
            .stage_supervisor_bind(
                &session_id,
                "super-a".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a".to_string(),
                test_supervisor_signing_public_key(0xaa),
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
                test_supervisor_signing_public_key(0xbb),
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
                test_supervisor_signing_public_key(0xcc),
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        // Bind at epoch 1.
        adapter
            .stage_supervisor_bind(
                &session_id,
                "super-a".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a".to_string(),
                test_supervisor_signing_public_key(0xaa),
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
                test_supervisor_signing_public_key(0xaa),
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
    async fn generated_supervisor_publish_authority_rejects_stale_descriptor_same_peer_epoch() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let runtime = bootstrap_runtime(PEER_ID_RECEIVER, "inproc://receiver", Some("token"));
        adapter
            .stage_local_endpoint_for_comms_runtime(&session_id, &runtime)
            .await
            .expect("stage local endpoint");
        let supervisor_peer_id = test_pubkey(0xaa).to_peer_id().as_str();

        let transition = adapter
            .stage_supervisor_bind(
                &session_id,
                "super-a".to_string(),
                supervisor_peer_id.clone(),
                "inproc://super-old".to_string(),
                test_supervisor_signing_public_key(0xaa),
                7,
            )
            .await
            .expect("initial bind");
        let freshness = adapter
            .supervisor_trust_publish_freshness_authority(&session_id)
            .await
            .expect("publish freshness");
        let stale_obligation =
            crate::protocol_supervisor_trust_publish::extract_obligations_with_freshness(
                &transition,
                freshness,
            )
            .into_iter()
            .next()
            .expect("generated publish obligation");

        adapter
            .stage_supervisor_authorize(
                &session_id,
                "super-a-renamed".to_string(),
                supervisor_peer_id,
                "inproc://super-new".to_string(),
                test_supervisor_signing_public_key(0xbb),
                7,
            )
            .await
            .expect("same-peer same-epoch descriptor rotation");

        let stale_authority = crate::protocol_supervisor_trust_publish::publish_authority_for_peer(
            &stale_obligation,
            stale_obligation.peer_id(),
        );
        assert!(
            matches!(stale_authority, Err(ref error) if error.contains("stale generated supervisor trust publish obligation")),
            "stale descriptor must not mint generated supervisor publish authority, got: {stale_authority:?}"
        );
    }

    #[tokio::test]
    async fn dsl_supervisor_trust_revoke_ack_stale_epoch_is_rejected() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        // Bind at epoch 1.
        adapter
            .stage_supervisor_bind(
                &session_id,
                "super-a".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a".to_string(),
                test_supervisor_signing_public_key(0xaa),
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
                test_supervisor_signing_public_key(0xaa),
                2,
            )
            .await
            .expect("rotation to epoch 2");

        // Revoke at epoch 2 records a generated pending trust-revoke
        // obligation before the shell removes live trust.
        adapter
            .stage_supervisor_revoke(&session_id, "ed25519:super-a".to_string(), 2)
            .await
            .expect("matching revoke must stage pending trust revoke");

        // Stale revoke ack for epoch 1 — DSL-rejected against the
        // pending generated revoke obligation.
        let stale = adapter
            .stage_supervisor_trust_revoked(&session_id, "ed25519:super-a".to_string(), 1)
            .await;
        assert!(
            matches!(stale, Err(SupervisorBindingStageError::Dsl(_))),
            "stale-epoch revoke ack must be DSL-rejected, got: {stale:?}"
        );
        match adapter.supervisor_binding(&session_id).await {
            SupervisorBinding::Unbound => {}
            SupervisorBinding::Bound { .. } => panic!("revoke input must unbind before feedback"),
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        adapter
            .stage_supervisor_bind(
                &session_id,
                "super-a".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a".to_string(),
                test_supervisor_signing_public_key(0xaa),
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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

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
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let supervisor = trusted_peer_from_runtime("mob/__mob_supervisor__", &supervisor_runtime);
        add_test_projection_trust(runtime.as_ref(), supervisor.clone(), "trust supervisor").await;
        adapter
            .stage_supervisor_bind(
                &session_id,
                supervisor.name.to_string(),
                supervisor.peer_id.as_str(),
                supervisor.address.to_string(),
                signing_public_key_for_descriptor(&supervisor),
                1,
            )
            .await
            .expect("pre-bind supervisor");
        let invalid_peer_id = PeerId::new().as_str();
        let candidate = bridge_candidate(
            &supervisor.peer_id.as_str(),
            &BridgeCommand::WireMember(BridgePeerWiringPayload {
                supervisor: supervisor.clone().into(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                peer_spec: BridgePeerSpec {
                    name: "".to_string(),
                    peer_id: invalid_peer_id.clone(),
                    address: "inproc://peer".to_string(),
                    pubkey: [0u8; 32],
                },
                mob_peer_overlay: Some(BridgeMobPeerOverlayHandoff {
                    recipient_peer_id: runtime.peer_id().expect("runtime peer_id").as_str(),
                    topology_epoch: 1,
                    peer_specs: Vec::new(),
                }),
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

    #[tokio::test]
    async fn wire_member_without_overlay_rejects_v2_payload_without_wiring() {
        // A pre-overlay (V2) supervisor emits WireMember with no
        // `mob_peer_overlay`. The optional wire field lets it deserialize, but
        // the receiver must reject it cleanly (typed UnsupportedProtocolVersion)
        // and must NOT materialize the peer into comms trust — peer wiring
        // requires the V3 overlay.
        let runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("receiver-wire-noverlay").unwrap());
        let supervisor_runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("mob/__mob_supervisor_noverlay__").unwrap(),
        );
        let peer_runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("peer-wire-noverlay").unwrap());
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let supervisor =
            trusted_peer_from_runtime("mob/__mob_supervisor_noverlay__", &supervisor_runtime);
        add_test_projection_trust(runtime.as_ref(), supervisor.clone(), "trust supervisor").await;
        adapter
            .stage_supervisor_bind(
                &session_id,
                supervisor.name.to_string(),
                supervisor.peer_id.as_str(),
                supervisor.address.to_string(),
                signing_public_key_for_descriptor(&supervisor),
                1,
            )
            .await
            .expect("pre-bind supervisor");
        let peer_spec = trusted_peer_from_runtime("peer-wire-noverlay", &peer_runtime);
        let candidate = bridge_candidate(
            &supervisor.peer_id.as_str(),
            &BridgeCommand::WireMember(BridgePeerWiringPayload {
                supervisor: supervisor.clone().into(),
                epoch: 1,
                protocol_version: BridgeProtocolVersion::V2,
                peer_spec: peer_spec.clone().into(),
                mob_peer_overlay: None,
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
            "wire bridge command should be handled (and cleanly rejected)"
        );
        let peers = runtime.peers().await;
        assert!(
            peers.iter().all(|entry| entry.peer_id != peer_spec.peer_id),
            "a V2 wiring payload without the overlay must not materialize peer trust"
        );
    }

    #[tokio::test]
    async fn wire_member_rejects_mob_overlay_for_different_recipient() {
        let runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("receiver-wire-mismatch").unwrap());
        let supervisor_runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("mob/__mob_supervisor_mismatch__").unwrap(),
        );
        let peer_runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("peer-wire-mismatch").unwrap());
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let supervisor =
            trusted_peer_from_runtime("mob/__mob_supervisor_mismatch__", &supervisor_runtime);
        add_test_projection_trust(runtime.as_ref(), supervisor.clone(), "trust supervisor").await;
        adapter
            .stage_supervisor_bind(
                &session_id,
                supervisor.name.to_string(),
                supervisor.peer_id.as_str(),
                supervisor.address.to_string(),
                signing_public_key_for_descriptor(&supervisor),
                1,
            )
            .await
            .expect("pre-bind supervisor");
        let peer_spec = trusted_peer_from_runtime("peer-wire-mismatch", &peer_runtime);
        let candidate = bridge_candidate(
            &supervisor.peer_id.as_str(),
            &BridgeCommand::WireMember(BridgePeerWiringPayload {
                supervisor: supervisor.clone().into(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                peer_spec: peer_spec.clone().into(),
                mob_peer_overlay: Some(BridgeMobPeerOverlayHandoff {
                    recipient_peer_id: PeerId::new().as_str(),
                    topology_epoch: 1,
                    peer_specs: vec![peer_spec.clone().into()],
                }),
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
            peers.iter().all(|entry| entry.peer_id != peer_spec.peer_id),
            "MobMachine overlay handoff for another recipient must not update local peer trust"
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
        let authorized_terminal = if let Some(handle) = handle {
            match handle.try_wait().await {
                Ok(outcome) => {
                    if let Some(tx) = subscriber {
                        let event = interaction_terminal_event(interaction_id, outcome);
                        if crate::tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            tx.send(event),
                        )
                        .await
                        .is_err()
                        {
                            tracing::warn!(
                                %interaction_id,
                                "completion bridge dropped terminal event: subscriber send timed out after 5s"
                            );
                        }
                    }
                    true
                }
                Err(error) => {
                    tracing::warn!(
                        %interaction_id,
                        error = %error,
                        "completion bridge waiter failed without generated completion authority; not fabricating terminal event"
                    );
                    false
                }
            }
        } else {
            tracing::debug!(
                %interaction_id,
                "completion bridge has no completion handle; not fabricating terminal event"
            );
            false
        };

        if authorized_terminal && let Some(runtime) = comms_runtime {
            runtime.mark_interaction_complete(&interaction_id);
        }
    });
}
