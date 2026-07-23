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
#[cfg(test)]
use meerkat_core::comms::{PeerAddress, SendReceipt};
use meerkat_core::event::AgentEvent;
use meerkat_core::interaction::{
    InteractionContent, PeerIngressFact, PeerInputCandidate, PeerInputClass,
};
use meerkat_core::time_compat::Instant;
use meerkat_core::types::SessionId;

use meerkat_contracts::wire::WireMemberHistoryPageBody;
use meerkat_contracts::wire::supervisor_bridge::{
    BRIDGE_TURN_OUTCOME_ACK_MAX, BridgeAck, BridgeBindResponse, BridgeCapabilities, BridgeCommand,
    BridgeDeliveryOutcome, BridgeDeliveryPayload, BridgeDeliveryRejectionCause,
    BridgeDeliveryResponse, BridgeDestroyResponse, BridgeEventCursor, BridgeHardCancelPayload,
    BridgeLiveControlledResponse, BridgeLiveOpenedResponse, BridgeMemberEventsPage,
    BridgeMemberHistoryPage, BridgeMemberIncarnation, BridgeMemberRuntimeState,
    BridgeMobPeerOverlayHandoff, BridgeObservationResponse, BridgeOutboundTaintTarget,
    BridgeOutcomeTracking, BridgePeerConnectivity, BridgePeerIdentity, BridgePeerSpec,
    BridgeProtocolVersion, BridgeRejectionCause, BridgeReply, BridgeRetireResponse,
    BridgeSupervisorPayload, BridgeSupervisorRotationObservation,
    BridgeSupervisorRotationOperationReceipt, BridgeSupervisorRotationPendingPhase,
    BridgeSupervisorRotationRejectionCause, BridgeSupervisorRotationRejectionReceipt,
    BridgeSupervisorRotationState, BridgeSupervisorRotationTargetReceipt,
    BridgeTrackedInputCancelPayload, BridgeTrackedInputCancelResponse, SUPERVISOR_BRIDGE_INTENT,
    SupervisorRotationOperationId, WireEventRow, canonicalize_bridge_address,
    decode_bridge_command,
};
#[cfg(test)]
use meerkat_contracts::wire::supervisor_bridge::{
    BridgeHostRuntimeIncarnation, SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM,
    SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
};

use crate::comms_bridge::classified_interaction_to_runtime_input;
#[cfg(test)]
use crate::completion::CompletionOutcome;
use crate::completion::interaction_terminal_event;
use crate::identifiers::IdempotencyKey;
#[cfg(test)]
use crate::identifiers::LogicalRuntimeId;
use crate::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
};
use crate::member_live::{MemberLiveError, MemberLiveHost};
use crate::member_observation::{
    DirectedTurnAdmission, DirectedTurnTracking, DirectedTurnWindow, MemberEventsPollRequest,
    MemberEventsWindow, MemberHistoryWindow, MemberObservationCursor, MemberObservationError,
    MemberObservationHost,
};

use crate::meerkat_machine::SupervisorBinding;
use crate::meerkat_machine::{
    DrainExitReason, GeneratedSupervisorBinding, GeneratedSupervisorRotationSubmit, MeerkatMachine,
    SupervisorAuthorizeAdmission, SupervisorBindAdmission, SupervisorBindingStageError,
    SupervisorRotationObservation, SupervisorRotationSubmission,
};
use crate::service_ext::SessionServiceRuntimeExt as _;
use crate::tokio::sync::mpsc;
use crate::traits::RuntimeControlPlane;

/// Default idle timeout for session-backed comms drains.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const SUPERVISOR_RESPONSE_ROUTE_IO_TIMEOUT: Duration = Duration::from_secs(5);

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
        match adapter
            .peer_ingress_runtime_is_current(&session_id, &comms_runtime)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(%session_id, "stale comms drain refused startup");
                return;
            }
            Err(error) => {
                tracing::warn!(%session_id, %error, "comms drain lost session before startup");
                return;
            }
        }
        if let Err(error) = async {
            adapter
                .stage_local_endpoint_for_comms_runtime(&session_id, comms_runtime.as_ref())
                .await
                .map_err(|error| error.to_string())?;
            hydrate_or_resume_supervisor_rotation_runtime(&adapter, &session_id, &comms_runtime)
                .await
        }
        .await
        {
            tracing::warn!(%session_id, %error, "unable to hydrate supervisor authority on comms runtime");
        }
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
                            from_peer_id = ?candidate.from_peer_id(),
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
                            // terminal transition. `PeerInteractionCleanup`
                            // owns only peer-correlation mechanics; the
                            // independent stream remains claimable when this
                            // response wins the race with caller attachment.
                            // Stream completion is committed when its consumer
                            // observes the terminal event. Without a
                            // peer-interaction handle, this drain path remains
                            // transport-only for lifecycle purposes.
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

                            // Member-upcall waiter seam: a registered
                            // bridge-reply waiter consumes the terminal
                            // Response BEFORE session injection. The reply is
                            // a mid-turn tool result, not session input —
                            // injecting it would create a shadow message. The
                            // DSL lifecycle above stays honest either way. A
                            // tombstoned waiter (member-side timeout already
                            // surfaced) consumes and discards the late reply
                            // so bridge JSON never leaks into the transcript.
                            if comms_runtime.has_bridge_reply_waiter(&interaction_id) {
                                match comms_runtime.take_bridge_reply_waiter(&interaction_id) {
                                    Some(waiter) => {
                                        let _ = waiter.send(candidate.clone());
                                    }
                                    None => {
                                        tracing::debug!(
                                            interaction_id = %interaction_id,
                                            "comms_drain: discarded late bridge reply for tombstoned waiter"
                                        );
                                    }
                                }
                                comms_runtime.mark_interaction_complete(&interaction_id);
                                continue;
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
                                    if dsl_installed {
                                        comms_runtime.abandon_interaction_stream(
                                            &interaction_id,
                                            meerkat_core::InteractionStreamAbandonReason::ResponseRejected,
                                        );
                                    } else {
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
                                    if dsl_installed {
                                        comms_runtime.abandon_interaction_stream(
                                            &interaction_id,
                                            meerkat_core::InteractionStreamAbandonReason::ResponseRejected,
                                        );
                                    } else {
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

                            // A registered bridge-reply waiter suppresses
                            // Progress injection too: `response_progress` was
                            // recorded above (the DSL lifecycle stays honest),
                            // but the payload must not become session input.
                            // The controlling responder never sends Progress —
                            // this is belt-and-braces against a misbehaving
                            // peer.
                            if let meerkat_core::interaction::InteractionContent::Response {
                                in_reply_to,
                                ..
                            } = &candidate.interaction.content
                                && comms_runtime.has_bridge_reply_waiter(in_reply_to)
                            {
                                continue;
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

                        let fenced_member = if let meerkat_core::interaction::InteractionContent::IncarnationFencedMessage {
                            expected_recipient,
                            ..
                        } = &candidate.interaction.content
                        {
                            let expected_member = BridgeMemberIncarnation {
                                mob_id: expected_recipient.mob_id.clone(),
                                agent_identity: expected_recipient.agent_identity.clone(),
                                host_id: expected_recipient.host_id.clone(),
                                binding_generation: expected_recipient.binding_generation,
                                member_session_id: expected_recipient.member_session_id.clone(),
                                generation: expected_recipient.generation,
                                fence_token: expected_recipient.fence_token,
                            };
                            Some(expected_member)
                        } else {
                            None
                        };

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
                                comms_runtime.abandon_interaction_stream(
                                    &interaction_id,
                                    meerkat_core::InteractionStreamAbandonReason::AdmissionRejected,
                                );
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
                                comms_runtime.abandon_interaction_stream(
                                    &interaction_id,
                                    meerkat_core::InteractionStreamAbandonReason::AdmissionRejected,
                                );
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
                                comms_runtime.abandon_interaction_stream(
                                    &interaction_id,
                                    meerkat_core::InteractionStreamAbandonReason::AdmissionRejected,
                                );
                                continue;
                            }
                        };
                        let result = match fenced_member.as_ref() {
                            Some(expected_member) => {
                                adapter
                                    .accept_input_with_completion_for_member_residency(
                                        &session_id,
                                        input,
                                        Some(expected_member),
                                    )
                                    .await
                            }
                            None => {
                                adapter
                                    .accept_input_with_completion(&session_id, input)
                                    .await
                            }
                        };

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
                                comms_runtime.abandon_interaction_stream(
                                    &interaction_id,
                                    meerkat_core::InteractionStreamAbandonReason::AdmissionRejected,
                                );
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
        return;
    }
    comms_runtime.abandon_interaction_stream(
        in_reply_to,
        meerkat_core::InteractionStreamAbandonReason::ResponseRejected,
    );
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

fn generated_sender_signing_public_key(sender: &PeerIngressFact) -> Option<String> {
    sender
        .signing_pubkey
        .map(encode_supervisor_signing_public_key)
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
        crate::meerkat_machine::dsl::SupervisorBindRejectionKind::RevocationPending => {
            BridgeRejectionCause::AlreadyBound
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
        crate::meerkat_machine::dsl::SupervisorBindRejectionKind::RevocationPending => {
            "bind member admission rejected by MeerkatMachine: prior supervisor revocation is still pending; retry revoke_supervisor before binding"
                .to_string()
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
        crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind::RotationNotAllowed => {
            BridgeRejectionCause::Unsupported
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
        crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind::RotationNotAllowed => {
            "authorize supervisor admission rejected by MeerkatMachine: supervisor rotation is not allowed after the member runtime has stopped accepting ordinary commands"
                .to_string()
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
        crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind::CommandNotAllowed => {
            BridgeRejectionCause::Unsupported
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
        crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind::CommandNotAllowed => {
            "supervisor bridge admission rejected by MeerkatMachine: command is not allowed in the current runtime phase"
                .to_string()
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
        Ok(crate::meerkat_machine::SupervisorBridgeCommandAdmission::ResumePendingRevoke) => Err((
            BridgeRejectionCause::Internal,
            "ordinary supervisor admission returned a cleanup-only pending-revoke disposition"
                .to_string(),
        )),
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

fn supervisor_cleanup_command_label(
    command_kind: crate::meerkat_machine::dsl::SupervisorCleanupCommandKind,
) -> &'static str {
    match command_kind {
        crate::meerkat_machine::dsl::SupervisorCleanupCommandKind::Retire => "retire_member",
        crate::meerkat_machine::dsl::SupervisorCleanupCommandKind::Observe => "observe_member",
        crate::meerkat_machine::dsl::SupervisorCleanupCommandKind::Destroy => "destroy_member",
        crate::meerkat_machine::dsl::SupervisorCleanupCommandKind::Revoke => "revoke_supervisor",
    }
}

/// Resolve one of the four lifecycle cleanup commands through the dedicated
/// generated phase policy. Ordinary bridge commands continue to use
/// `resolve_authorized_supervisor` and therefore remain active-phase-only.
struct AuthorizedSupervisorCleanup {
    supervisor: BridgePeerIdentity,
    resume_pending_revoke: bool,
}

async fn resolve_authorized_supervisor_cleanup(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    sender: &PeerIngressFact,
    payload: &BridgeSupervisorPayload,
    command_kind: crate::meerkat_machine::dsl::SupervisorCleanupCommandKind,
) -> Result<AuthorizedSupervisorCleanup, (BridgeRejectionCause, String)> {
    let supervisor = bridge_peer_identity(&payload.supervisor, "supervisor cleanup request")?;
    let sender_peer_id = sender
        .canonical_peer_id
        .map(|sender_peer_id| sender_peer_id.to_string());
    match adapter
        .resolve_supervisor_cleanup_command_admission(
            session_id,
            command_kind,
            supervisor.peer_id.to_string(),
            payload.epoch,
            sender_peer_id,
        )
        .await
    {
        Ok((crate::meerkat_machine::SupervisorBridgeCommandAdmission::Accepted, _)) => {
            Ok(AuthorizedSupervisorCleanup {
                supervisor,
                resume_pending_revoke: false,
            })
        }
        Ok((crate::meerkat_machine::SupervisorBridgeCommandAdmission::ResumePendingRevoke, _)) => {
            Ok(AuthorizedSupervisorCleanup {
                supervisor,
                resume_pending_revoke: true,
            })
        }
        Ok((
            crate::meerkat_machine::SupervisorBridgeCommandAdmission::Rejected(rejection),
            phase,
        )) => {
            let reason = if rejection
                == crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind::CommandNotAllowed
            {
                format!(
                    "supervisor cleanup command '{}' is not allowed while the member runtime is {phase:?}",
                    supervisor_cleanup_command_label(command_kind)
                )
            } else {
                supervisor_bridge_admission_rejection_reason(rejection, sender, &supervisor)
            };
            Err((
                supervisor_bridge_admission_rejection_cause(rejection),
                reason,
            ))
        }
        Err(error) => Err((
            BridgeRejectionCause::Internal,
            format!("supervisor cleanup admission failed: {error}"),
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
    let response_peer = supervisor.clone().into_trusted_peer_descriptor();
    refresh_and_publish_bound_supervisor_route(
        adapter,
        session_id,
        comms_runtime.as_ref(),
        &response_peer,
        payload.epoch,
        generated_sender_peer_id(sender),
        context,
    )
    .await
    .map_err(|reason| (BridgeRejectionCause::Internal, reason))?;
    Ok(supervisor)
}

async fn resolve_authorized_supervisor_cleanup_with_response_route(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    sender: &PeerIngressFact,
    payload: &BridgeSupervisorPayload,
    command_kind: crate::meerkat_machine::dsl::SupervisorCleanupCommandKind,
    context: &str,
) -> Result<BridgePeerIdentity, (BridgeRejectionCause, String)> {
    let supervisor =
        resolve_authorized_supervisor_cleanup(adapter, session_id, sender, payload, command_kind)
            .await?;
    if supervisor.resume_pending_revoke {
        return Err((
            BridgeRejectionCause::Internal,
            format!("{context}: pending revoke disposition is invalid for this command"),
        ));
    }
    let response_peer = supervisor.supervisor.clone().into_trusted_peer_descriptor();
    refresh_and_publish_bound_supervisor_route(
        adapter,
        session_id,
        comms_runtime.as_ref(),
        &response_peer,
        payload.epoch,
        generated_sender_peer_id(sender),
        context,
    )
    .await
    .map_err(|reason| (BridgeRejectionCause::Internal, reason))?;
    Ok(supervisor.supervisor)
}

fn bridge_capabilities(
    origin_protocol: meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion,
) -> BridgeCapabilities {
    let mut capabilities = BridgeCapabilities {
        deliver_member_input: true,
        observe_member: true,
        interrupt_member: true,
        // Phase 6 (DEC-P6E-7): the member drain serves the machine-admitted
        // exact-run hard-cancel arm, which is V4-only.
        hard_cancel_member: origin_protocol.supports_multi_host(),
        tracked_input_cancel: origin_protocol.supports_multi_host(),
        retire_member: true,
        destroy_member: true,
        wire_member: true,
        unwire_member: true,
        ..BridgeCapabilities::default()
    };
    if !origin_protocol.supports_multi_host() {
        capabilities.current_protocol_version = origin_protocol;
        capabilities.default_protocol_version = origin_protocol;
        capabilities.supported_protocol_versions = match origin_protocol {
            version
                if version
                    == meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V2 =>
            {
                vec![meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V2]
            }
            _ => vec![
                meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V2,
                meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V3,
            ],
        };
    }
    capabilities
}

fn peer_input_from_delivery_payload(
    session_id: &SessionId,
    sender_peer_id: PeerId,
    payload: BridgeDeliveryPayload,
) -> Result<Input, String> {
    if payload.turn.is_some() {
        return Err(
            "directive-bearing member delivery must use the tracked flow-step lowering path"
                .to_string(),
        );
    }
    let stable_uuid = canonical_non_nil_delivery_uuid("input_id", &payload.input_id)?;
    let transcript_uuid = payload
        .transcript_interaction_id
        .as_deref()
        .map(|raw| canonical_non_nil_delivery_uuid("transcript_interaction_id", raw))
        .transpose()?;
    let is_placed = payload.expected_member.is_some();
    let directed_interaction_id = delivery_requests_tracked_interaction(&payload)
        .then_some(meerkat_core::interaction::InteractionId(stable_uuid));
    Ok(Input::Peer(PeerInput {
        directed_interaction_id,
        objective_id: payload.objective_id,
        header: InputHeader {
            id: meerkat_core::lifecycle::InputId::from_uuid(stable_uuid),
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
            // Placed ingress separates the retry-stable transport key from
            // transcript identity. Absence stays absent so the ordinary
            // runtime lane retains its own minting semantics. Legacy
            // peer-only delivery preserves its pre-field input-id carrier.
            correlation_id: transcript_uuid
                .or_else(|| (!is_placed).then_some(stable_uuid))
                .map(crate::identifiers::CorrelationId::from_uuid),
        },
        convention: Some(PeerConvention::Message),
        content: payload.content,
        payload: None,
        handling_mode: Some(payload.handling_mode),
        // Bridge delivery payloads are supervisor-authored work deliveries,
        // not peer comms envelopes: no sender taint declaration exists.
        sender_taint: None,
        // Supervisor-attached injected context rides the delivery payload
        // and lowers as InjectedContext-role transcript appends immediately
        // before this input's peer append.
        injected_context: payload.injected_context,
    }))
}

/// Tracked terminal custody is explicit. Flow-step directives inherently own
/// it; plain peer-shaped deliveries opt in with the V4 `outcome_tracking`
/// marker. `expected_member` alone is only a residency fence and must never
/// acquire terminal-publication semantics by implication.
fn delivery_requests_tracked_interaction(payload: &BridgeDeliveryPayload) -> bool {
    match payload.outcome_tracking {
        None => false,
        Some(BridgeOutcomeTracking::Interaction) => true,
    }
}

fn delivery_requires_tracked_turn_custody(payload: &BridgeDeliveryPayload) -> bool {
    payload.turn.is_some() || delivery_requests_tracked_interaction(payload)
}

fn validate_delivery_tracking_request(payload: &BridgeDeliveryPayload) -> Result<(), String> {
    canonical_non_nil_delivery_uuid("input_id", &payload.input_id)?;
    if payload.turn.is_some() && payload.outcome_tracking.is_some() {
        return Err(
            "turn and outcome_tracking are mutually exclusive terminal-custody requests"
                .to_string(),
        );
    }
    if payload.outcome_tracking.is_some() && !payload.protocol_version.supports_multi_host() {
        return Err(
            "outcome_tracking requires a V4 supervisor bridge protocol version".to_string(),
        );
    }
    if payload.outcome_tracking.is_some() && payload.expected_member.is_none() {
        return Err("outcome_tracking requires an exact placed-member incarnation".to_string());
    }
    if let Some(transcript_id) = payload.transcript_interaction_id.as_deref() {
        if !payload.protocol_version.supports_multi_host() {
            return Err(
                "transcript_interaction_id requires a V4 supervisor bridge protocol version"
                    .to_string(),
            );
        }
        if payload.expected_member.is_none() {
            return Err(
                "transcript_interaction_id requires an exact placed-member incarnation".to_string(),
            );
        }
        if payload.turn.is_some() {
            return Err(
                "turn and transcript_interaction_id are mutually exclusive identity carriers"
                    .to_string(),
            );
        }
        canonical_non_nil_delivery_uuid("transcript_interaction_id", transcript_id)?;
    }
    if payload.outcome_tracking == Some(BridgeOutcomeTracking::Interaction)
        && payload.transcript_interaction_id.as_deref() != Some(payload.input_id.as_str())
    {
        return Err(
            "tracked interaction requires transcript_interaction_id to equal input_id".to_string(),
        );
    }
    Ok(())
}

fn canonical_non_nil_delivery_uuid(field: &str, raw: &str) -> Result<uuid::Uuid, String> {
    let parsed = uuid::Uuid::parse_str(raw)
        .map_err(|error| format!("delivery {field} '{raw}' is not a UUID: {error}"))?;
    if parsed.is_nil() || parsed.to_string() != raw {
        return Err(format!(
            "delivery {field} '{raw}' must be a canonical non-nil UUID"
        ));
    }
    Ok(parsed)
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
    adapter: &MeerkatMachine,
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
    comms_runtime: &dyn CommsRuntime,
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
    comms_runtime: &dyn CommsRuntime,
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
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    comms_runtime: &dyn CommsRuntime,
    obligation: &crate::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation,
) -> Result<(), String> {
    let trusted_peer = trusted_peer_descriptor_from_supervisor_publish_obligation(obligation)?;
    // A same-peer/key authorize may legitimately advance only the route
    // metadata. The source-attributed trust store deliberately rejects a
    // divergent Add from an existing source, so the generated publish
    // protocol owns the update as Remove(old source row) -> Add(new row).
    // Mint fail-closed cleanup from a second exact restatement before
    // spending the first removal; generated obligations allow each mutation
    // kind only once. This cleanup removes the newly-added row if the machine
    // rejects its publish ACK. It deliberately does not restore the obsolete
    // descriptor; the pending publish remains retryable with no stale trust.
    let failure_cleanup_obligation = supervisor_route_publish_obligation(
        adapter,
        session_id,
        &trusted_peer,
        "supervisor trust route replacement failure cleanup",
    )
    .await?;
    let replacement_cleanup_authority = supervisor_publish_cleanup_authority(obligation)?;
    let failure_cleanup_authority =
        supervisor_publish_cleanup_authority(&failure_cleanup_obligation)?;
    apply_generated_private_trust_remove(
        comms_runtime,
        obligation.peer_id().clone(),
        replacement_cleanup_authority,
    )
    .await?;
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
            failure_cleanup_authority,
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

async fn refresh_and_publish_bound_supervisor_route(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    comms_runtime: &dyn CommsRuntime,
    descriptor: &TrustedPeerDescriptor,
    epoch: u64,
    sender_peer_id: Option<String>,
    context: &str,
) -> Result<(), String> {
    adapter
        .stage_local_endpoint_for_comms_runtime(session_id, comms_runtime)
        .await
        .map_err(|error| format!("{context}: local endpoint rejected: {error}"))?;
    let transition = adapter
        .stage_supervisor_binding_route_refresh(
            session_id,
            GeneratedSupervisorBinding {
                name: descriptor.name.as_str().to_owned(),
                peer_id: descriptor.peer_id.as_str(),
                address: descriptor.address.to_string(),
                signing_public_key: encode_supervisor_signing_public_key(descriptor.pubkey),
                epoch,
            },
            sender_peer_id,
        )
        .await
        .map_err(|error| format!("{context}: route refresh rejected: {error}"))?;
    let obligation =
        single_supervisor_publish_obligation(adapter, session_id, &transition, context).await?;
    validate_supervisor_publish_obligation(&obligation, descriptor, epoch, context)?;
    publish_supervisor_trust_from_generated_obligation(
        adapter,
        session_id,
        comms_runtime,
        &obligation,
    )
    .await
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
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    comms_runtime: &dyn CommsRuntime,
    supervisor: &TrustedPeerDescriptor,
    epoch: u64,
    sender_peer_id: Option<String>,
) -> Result<(), String> {
    refresh_and_publish_bound_supervisor_route(
        adapter,
        session_id,
        comms_runtime,
        supervisor,
        epoch,
        sender_peer_id,
        "bind member idempotent ack trust repair",
    )
    .await
}

async fn rollback_bind_after_trust_publication_failure(
    adapter: &MeerkatMachine,
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

/// Bootstrap-path supervisor-bind commit with rollback — the `BindMember`
/// drain arm body, extracted so the wire drain and the host materializer
/// ([`bind_supervisor_for_materialized_session`]) share one implementation.
///
/// Stages the local endpoint, applies `BindSupervisor`, validates the single
/// generated `PublishSupervisorTrustEdge` obligation against the staged
/// binding, and spends it as a PRIVATE trust add. Any failure after the DSL
/// commit rolls the staged bind back via `SupervisorRevoke` (rollback
/// failures are reported inside the returned reason, never swallowed).
async fn commit_supervisor_bind_with_rollback(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    comms_runtime: &dyn CommsRuntime,
    supervisor_spec: &TrustedPeerDescriptor,
    epoch: u64,
    context: &str,
) -> Result<(), String> {
    adapter
        .stage_local_endpoint_for_comms_runtime(session_id, comms_runtime)
        .await
        .map_err(|error| format!("{context} failed: local endpoint rejected: {error}"))?;
    // DSL owns the authorization discriminant + identity + epoch. Generated
    // bind admission already emitted `Bootstrap`, so this stage should never
    // be rejected; if the DSL rejects anyway, surface it typed.
    let bind_transition = adapter
        .stage_supervisor_bind(
            session_id,
            supervisor_spec.name.as_str().to_owned(),
            supervisor_spec.peer_id.as_str(),
            supervisor_spec.address.to_string(),
            encode_supervisor_signing_public_key(supervisor_spec.pubkey),
            epoch,
        )
        .await
        .map_err(|error| format!("{context} failed: DSL rejected binding: {error}"))?;
    let publish_obligation =
        match single_supervisor_publish_obligation(adapter, session_id, &bind_transition, context)
            .await
        {
            Ok(obligation) => obligation,
            Err(error) => {
                let _ = rollback_bind_after_trust_publication_failure(
                    adapter,
                    session_id,
                    &supervisor_spec.peer_id.as_str(),
                    epoch,
                )
                .await;
                return Err(error);
            }
        };
    if let Err(error) =
        validate_supervisor_publish_obligation(&publish_obligation, supervisor_spec, epoch, context)
    {
        let _ = rollback_bind_after_trust_publication_failure(
            adapter,
            session_id,
            publish_obligation.peer_id(),
            publish_obligation.epoch(),
        )
        .await;
        return Err(error);
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
            Ok(()) => {
                format!("{context} failed: trust publication failed after DSL commit: {error}")
            }
            Err(rollback_error) => format!(
                "{context} failed: trust publication failed after DSL commit: {error}; rollback failed: {rollback_error}"
            ),
        };
        return Err(reason);
    }
    Ok(())
}

/// Typed failure surface for [`bind_supervisor_for_materialized_session`].
///
/// Variants carry the inner reason strings from the shared bind stages so
/// the member host can persist/report exactly what the drain path would
/// have replied on the wire.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorMaterializeBindError {
    /// Generated bind admission could not be staged (session not registered,
    /// DSL rejection, missing or malformed admission feedback).
    #[error("supervisor bind admission failed: {0}")]
    Admission(String),
    /// MeerkatMachine admission rejected the bind (fail-closed verdict).
    #[error("supervisor bind admission rejected: {0}")]
    AdmissionRejected(String),
    /// The binding is already current (idempotent rebind) but the private
    /// admission trust edge could not be re-published.
    #[error("supervisor trust repair failed for idempotent rebind: {0}")]
    TrustRepair(String),
    /// The bind commit failed; any staged bind was rolled back (rollback
    /// failures are reported inside the message).
    #[error("supervisor bind commit failed: {0}")]
    Commit(String),
}

/// Bind the recorded supervisor for a host-materialized member session
/// (multi-host mobs DEC-P3H-4 / ADJ-18).
///
/// AUTHORITY PATH: `MobHostBindingAuthority` has already adjudicated the
/// admitted `MaterializeMember` command (sender == recorded supervisor,
/// epoch currency), so no wire-shape validation is repeated here. Machine
/// admission is NOT skipped: this drives the member session's MeerkatMachine
/// through the SAME generated seams as the member drain's `BindMember` arm —
/// `ResolveSupervisorBindAdmission` → `PublishLocalEndpoint` →
/// `BindSupervisor` (emitting the `PublishSupervisorTrustEdge` obligation) →
/// obligation validation → obligation spend as a PRIVATE trust add
/// (`SupervisorTrustPublished` ack), with `SupervisorRevoke` rollback when
/// publication fails after the DSL commit. The sender/epoch/token facts are
/// the host-authority-adjudicated payload facts, not wire observations. An
/// `IdempotentAck` admission (ensure-on-replay, ADJ-9) repairs the private
/// trust edge and succeeds without re-binding.
pub async fn bind_supervisor_for_materialized_session(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    comms_runtime: &dyn CommsRuntime,
    supervisor: &TrustedPeerDescriptor,
    epoch: u64,
) -> Result<(), SupervisorMaterializeBindError> {
    let admission = adapter
        .resolve_supervisor_bind_admission(
            session_id,
            supervisor.peer_id.as_str(),
            epoch,
            // Host command admission has already validated
            // sender == recorded supervisor; the supervisor identity IS the
            // adjudicated sender fact on this path.
            Some(supervisor.peer_id.as_str()),
        )
        .await
        .map_err(|error| SupervisorMaterializeBindError::Admission(error.to_string()))?;
    match admission {
        SupervisorBindAdmission::Bootstrap => commit_supervisor_bind_with_rollback(
            adapter,
            session_id,
            comms_runtime,
            supervisor,
            epoch,
            "materialized member supervisor bind",
        )
        .await
        .map_err(SupervisorMaterializeBindError::Commit),
        SupervisorBindAdmission::IdempotentAck => republish_private_supervisor_trust(
            adapter,
            session_id,
            comms_runtime,
            supervisor,
            epoch,
            Some(supervisor.peer_id.as_str()),
        )
        .await
        .map_err(SupervisorMaterializeBindError::TrustRepair),
        SupervisorBindAdmission::Rejected(rejection) => Err(
            SupervisorMaterializeBindError::AdmissionRejected(format!("{rejection:?}")),
        ),
    }
}

/// Typed failure surface for
/// [`authorize_supervisor_for_materialized_session`].
#[derive(Debug, thiserror::Error)]
pub enum SupervisorMaterializeAuthorizeError {
    /// Generated admission rejected the claimed same-authority version
    /// advance (peer/key mismatch, stale epoch, or disallowed phase).
    #[error("supervisor authorize admission rejected: {0}")]
    AdmissionRejected(String),
    /// Generated admission, version commit, or private trust publication
    /// failed before the refreshed route became usable.
    #[error("supervisor authorize apply failed: {0}")]
    Apply(String),
}

/// Advance the already-bound supervisor version for a host-materialized
/// member after `MobHostBindingAuthority` has authenticated a same-identity
/// host rebind. This is the host-seeded sibling of wire
/// `AuthorizeSupervisor`: generated admission requires the same peer and
/// signing key plus a monotonic epoch, and the refreshed private trust edge
/// is published before success returns.
pub async fn authorize_supervisor_for_materialized_session(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    comms_runtime: &dyn CommsRuntime,
    supervisor: &TrustedPeerDescriptor,
    epoch: u64,
) -> Result<(), SupervisorMaterializeAuthorizeError> {
    apply_supervisor_authorization(
        adapter,
        session_id,
        comms_runtime,
        supervisor,
        epoch,
        Some(supervisor.peer_id.as_str()),
        Some(encode_supervisor_signing_public_key(supervisor.pubkey)),
        "materialized member supervisor authorize",
    )
    .await
    .map_err(|error| match error {
        SupervisorAuthorizeApplyError::Rejected(rejection) => {
            SupervisorMaterializeAuthorizeError::AdmissionRejected(format!("{rejection:?}"))
        }
        SupervisorAuthorizeApplyError::Internal(reason) => {
            SupervisorMaterializeAuthorizeError::Apply(reason)
        }
    })
}

#[cfg(test)]
async fn register_member_incarnation(
    adapter: &Arc<MeerkatMachine>,
    session_id: SessionId,
    incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
) -> Result<(), crate::traits::RuntimeDriverError> {
    let update = adapter.begin_member_residency_update(session_id).await;
    let publication = update.commit(incarnation, None).await?;
    drop(publication);
    Ok(())
}

/// Validate one direct member command against the exact host residency that
/// the controller admitted before detaching transport. Member generation,
/// fence, session, and key can all be reused after host replacement, so the
/// incarnation's binding generation is part of this equality.
fn require_registered_member_incarnation(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    expected: &BridgeMemberIncarnation,
    context: &str,
) -> Result<(), (BridgeRejectionCause, String)> {
    match adapter.member_incarnation(session_id) {
        Some(current) if &current == expected => Ok(()),
        Some(current) => Err((
            BridgeRejectionCause::StaleFence,
            format!("{context}: expected member incarnation {expected:?}; current is {current:?}"),
        )),
        None => Err((
            BridgeRejectionCause::Unavailable,
            format!(
                "{context}: target session '{session_id}' has no registered host-member incarnation"
            ),
        )),
    }
}

#[cfg(test)]
fn peer_recipient_incarnation_matches(
    expected: &meerkat_core::comms::PeerRecipientIncarnation,
    current: &BridgeMemberIncarnation,
) -> bool {
    expected.mob_id == current.mob_id
        && expected.agent_identity == current.agent_identity
        && expected.host_id == current.host_id
        && expected.binding_generation == current.binding_generation
        && expected.member_session_id == current.member_session_id
        && expected.generation == current.generation
        && expected.fence_token == current.fence_token
}

fn require_optional_registered_member_incarnation(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    expected: Option<&BridgeMemberIncarnation>,
    context: &str,
) -> Result<(), (BridgeRejectionCause, String)> {
    match expected {
        Some(expected) => {
            require_registered_member_incarnation(adapter, session_id, expected, context)
        }
        None if adapter.member_incarnation(session_id).is_some() => Err((
            BridgeRejectionCause::StaleFence,
            format!(
                "{context}: host-materialized target requires an exact expected_member incarnation"
            ),
        )),
        None => Ok(()),
    }
}

async fn acquire_registered_member_effect_authority(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    expected: &BridgeMemberIncarnation,
    context: &str,
) -> Result<crate::meerkat_machine::MemberEffectAuthorityGuard, (BridgeRejectionCause, String)> {
    adapter
        .lock_member_effect_authority(session_id, expected)
        .await
        .map_err(|error| match error {
            crate::traits::RuntimeDriverError::StaleAuthority { reason } => (
                BridgeRejectionCause::StaleFence,
                format!("{context}: {reason}"),
            ),
            other => (
                BridgeRejectionCause::Unavailable,
                format!("{context}: failed to acquire member effect authority: {other}"),
            ),
        })
}

async fn acquire_optional_member_effect_authority(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    expected: Option<&BridgeMemberIncarnation>,
    context: &str,
) -> Result<crate::meerkat_machine::MemberEffectAuthorityGuard, (BridgeRejectionCause, String)> {
    adapter
        .lock_optional_member_effect_authority(session_id, expected)
        .await
        .map_err(|error| match error {
            crate::traits::RuntimeDriverError::StaleAuthority { reason } => (
                BridgeRejectionCause::StaleFence,
                format!("{context}: {reason}"),
            ),
            other => (
                BridgeRejectionCause::Unavailable,
                format!("{context}: failed to acquire member effect authority: {other}"),
            ),
        })
}

/// Long-poll wait ceiling for `PollMemberEvents` (DEC-P6E-3): bounded well
/// under the bridge transport budget (the pump's request timeout is 30s),
/// so a long-poll never masquerades as a dead peer.
const MEMBER_POLL_WAIT_MAX_MS: u32 = 20_000;

/// Default / ceiling row counts for one `PollMemberEvents` page.
const MEMBER_POLL_DEFAULT_MAX_ROWS: u32 = 256;
const MEMBER_POLL_MAX_ROWS: u32 = 1024;

/// Independent terminal-sidecar page/ack bounds. The count cap limits actor
/// work; the serialized reply cap below enforces the transport invariant.
const MEMBER_POLL_DEFAULT_MAX_OUTCOMES: u32 = 8;
const MEMBER_POLL_MAX_OUTCOMES: u32 = 64;
/// Leave ample room for the peer-response envelope, routing metadata, and
/// transport encoding around the serialized `BridgeReply` value.
// The comms transport's wire ceiling is 1 MiB. Keep this core runtime free
// of an inverted production dependency on `meerkat-comms` while reserving
// half that envelope for peer-response metadata and encoding overhead.
const MEMBER_EVENTS_REPLY_MAX_BYTES: usize = 512 * 1024;
const MEMBER_HISTORY_REPLY_MAX_BYTES: usize = 512 * 1024;

/// A hard-cancel RPC is a bounded level assertion, not an edge notification.
/// Keep the receiver deadline below the controlling bridge's per-attempt 5s
/// budget so a still-bound run returns a typed `Unavailable` rather than a
/// transport timeout. A transient resend reuses the same operation/run tuple.
const HARD_CANCEL_SETTLE_TIMEOUT: Duration = Duration::from_secs(4);
const HARD_CANCEL_REASSERT_INTERVAL: Duration = Duration::from_millis(10);

/// Map a typed observation failure to its wire rejection (DEC-P6E-4).
fn observation_error_to_bridge_rejection(
    error: &MemberObservationError,
) -> (BridgeRejectionCause, String) {
    match error {
        MemberObservationError::StaleIncarnation { .. } => {
            (BridgeRejectionCause::StaleFence, error.to_string())
        }
        MemberObservationError::StaleCursor {
            watermark,
            generation,
        } => (
            BridgeRejectionCause::StaleCursor {
                watermark: *watermark,
                generation: *generation,
            },
            error.to_string(),
        ),
        // Future-generation cursors are a restore-from-backup / split-brain
        // shape: fail closed as Internal (FLAG-P6E-12).
        MemberObservationError::FutureGenerationCursor { .. } => {
            (BridgeRejectionCause::Internal, error.to_string())
        }
        MemberObservationError::Unavailable { .. } => {
            (BridgeRejectionCause::Unavailable, error.to_string())
        }
        MemberObservationError::OutcomeRecordTooLarge { .. } => {
            (BridgeRejectionCause::Internal, error.to_string())
        }
        MemberObservationError::Internal { .. } => {
            (BridgeRejectionCause::Internal, error.to_string())
        }
    }
}

/// Resolve the injected observation host or produce the typed absent-substrate
/// rejection (ADJ-P4-7 vocabulary: `Unavailable`, never `Unsupported` — the
/// command IS served on this engine; the composition lacks the substrate).
fn resolve_member_observation_host(
    adapter: &Arc<MeerkatMachine>,
) -> Result<Arc<dyn MemberObservationHost>, (BridgeRejectionCause, String)> {
    adapter.member_observation_host().ok_or_else(|| {
        (
            BridgeRejectionCause::Unavailable,
            "member host serves no observation substrate".to_string(),
        )
    })
}

/// Resolve the injected member live host or produce the typed
/// absent-substrate rejection (DEC-P6B-L7): `LiveTransportUnavailable` —
/// the live-specific sibling of the observation host's `Unavailable`; the
/// landed cause names precisely "this host serves no live substrate" and
/// §16.6 uses it for live-incapable hosts.
fn resolve_member_live_host(
    adapter: &Arc<MeerkatMachine>,
) -> Result<Arc<dyn MemberLiveHost>, (BridgeRejectionCause, String)> {
    // S3b: every live arm resolves exactly once, so this is the single
    // counting site for "a live command reached this member's drain" —
    // absent-slot rejects count (the command arrived; only the reach fails).
    adapter.note_live_command_served();
    adapter.member_live_host().ok_or_else(|| {
        (
            BridgeRejectionCause::LiveTransportUnavailable,
            "member host serves no live substrate".to_string(),
        )
    })
}

/// Map a typed member live failure to its wire rejection. The cause comes
/// from the ONE total `MemberLiveError::to_bridge_rejection` conversion
/// (ADJ-P6B-1 — shared with meerkat-mob's local branch); the reason is the
/// error's display text. The drain does zero semantic interpretation.
fn member_live_error_to_bridge_rejection(
    error: &MemberLiveError,
) -> (BridgeRejectionCause, String) {
    (error.to_bridge_rejection(), error.to_string())
}

/// Stage source-confined reply transport for this exact bridge response.
///
/// The only callback authority is the endpoint projected at ingress from a
/// signature-verified TCP Request: the classifier discards the sender-chosen
/// host and reconstructs `observed-source-ip:declared-port`. Payload peer
/// addresses are identity descriptors, never callback capabilities. Without
/// this ingress fact, replies must use an already-authorized durable route or
/// fail closed.
async fn stage_correlated_reply_endpoint(
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
) {
    if let (Some(endpoint), Some(sender_peer_id), Some(signing_pubkey)) = (
        candidate.ingress.declared_reply_endpoint.clone(),
        candidate.ingress.canonical_peer_id,
        candidate.ingress.signing_pubkey,
    ) {
        match comms_runtime
            .stage_correlated_reply_endpoint(
                sender_peer_id,
                candidate.interaction.id,
                signing_pubkey,
                endpoint,
            )
            .await
        {
            Ok(()) | Err(SendError::Unsupported(_)) => {}
            Err(error) => {
                tracing::warn!(
                    interaction_id = %candidate.interaction.id,
                    error = %error,
                    "comms_drain: failed to stage authenticated correlated reply endpoint"
                );
            }
        }
    }
}

async fn send_bridge_response(
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
    status: meerkat_core::interaction::ResponseStatus,
    reply: BridgeReply,
    // Kept temporarily as a compatibility argument for the large bridge
    // dispatcher call surface. Payload addresses are deliberately ignored:
    // they are identity descriptors, not callback authority.
    _declared_reply_address: Option<&str>,
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

    stage_correlated_reply_endpoint(comms_runtime, candidate).await;

    if let Err(error) = comms_runtime
        .send(CommsCommand::PeerResponse {
            to,
            in_reply_to: candidate.interaction.id,
            status,
            result,
            blocks: None,
            content_taint: None,
            handling_mode: None,
            objective_id: candidate.interaction.objective_id,
        })
        .await
    {
        if let Some(sender_peer_id) = candidate.ingress.canonical_peer_id
            && candidate.ingress.declared_reply_endpoint.is_some()
            && let Err(cleanup_error) = comms_runtime
                .unstage_correlated_reply_endpoint(sender_peer_id, candidate.interaction.id)
                .await
            && !matches!(cleanup_error, SendError::Unsupported(_))
        {
            tracing::warn!(
                from = %candidate.ingress.diagnostic_label(),
                interaction_id = %candidate.interaction.id,
                error = %cleanup_error,
                "comms_drain: failed to clear correlated reply endpoint after response failure"
            );
        }
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
    declared_reply_address: Option<&str>,
) {
    send_bridge_response(
        comms_runtime,
        candidate,
        meerkat_core::interaction::ResponseStatus::Failed,
        BridgeReply::Rejected {
            cause,
            reason: message.into(),
        },
        declared_reply_address,
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
    apply_generated_private_trust_add(comms_runtime.as_ref(), descriptor, authority)
        .await
        .map_err(|error| {
            (
                BridgeRejectionCause::Internal,
                format!("{context}: failed to install supervisor response route: {error}"),
            )
        })
}

/// Stage a generated `RequestSupervisorTrustPublish` restating the CURRENT
/// binding and return the single publish obligation it emits. The obligation's
/// authority owner token derives from the session dsl authority, so a
/// trust-mutation authority minted from it validates against a runtime carrying
/// the same MeerkatMachine owner token.
async fn supervisor_route_publish_obligation(
    adapter: &MeerkatMachine,
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

/// Try to handle a supervisor bridge command from an incoming comms request.
///
/// Returns `true` if the candidate was a bridge command (handled or rejected),
/// `false` if it was not a bridge command and should be processed normally.
///
/// Wave 3 D Row 21: the authorized-supervisor binding and bridge admission
/// classes live in DSL state. Bridge dispatch routes binding-sensitive
/// decisions through generated admission inputs before emitting wire replies
/// or staging supervisor binding transitions.
/// DEC-P6E-7 hard-cancel serving arm — deliberately EXTRACTED: this fn is
/// the single effect-authority-allowlisted reach from the member drain into
/// hard interrupt authority (`hard_cancel_run_if_current` → generated
/// `InterruptCurrentRun`). The dispatcher arm stays a thin call so the gate
/// ban keeps covering every other drain path.
struct SupervisorRotationDriveContext<'a> {
    operation_id: &'a str,
    previous: &'a BoundSupervisorState,
    next: &'a TrustedPeerDescriptor,
    next_epoch: u64,
    phase: crate::meerkat_machine::dsl::SupervisorRotationPhase,
}

async fn drive_supervisor_rotation(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    context: SupervisorRotationDriveContext<'_>,
) -> Result<(), String> {
    use crate::meerkat_machine::dsl::SupervisorRotationPhase;
    let SupervisorRotationDriveContext {
        operation_id,
        previous,
        next,
        next_epoch,
        mut phase,
    } = context;

    if matches!(
        phase,
        SupervisorRotationPhase::Completed | SupervisorRotationPhase::Rejected
    ) {
        return Ok(());
    }
    let mut transition = adapter
        .stage_supervisor_rotation_resume(session_id, operation_id.to_string())
        .await
        .map_err(|error| format!("rotation resume rejected: {error}"))?;

    loop {
        match phase {
            SupervisorRotationPhase::PreviousRevokePending => {
                let obligation = matching_supervisor_revoke_obligation(
                    adapter,
                    session_id,
                    &transition,
                    &previous.peer_id,
                    previous.epoch,
                    "supervisor rotation operation",
                )
                .await?;
                let authority = supervisor_revoke_authority(&obligation)?;
                let removal = crate::tokio::time::timeout(
                    SUPERVISOR_RESPONSE_ROUTE_IO_TIMEOUT,
                    apply_generated_private_trust_remove(
                        comms_runtime.as_ref(),
                        obligation.peer_id().clone(),
                        authority,
                    ),
                )
                .await;
                if let Err(error) = match removal {
                    Ok(result) => result.map(|_| ()),
                    Err(_) => Err("previous supervisor trust removal timed out".to_string()),
                } {
                    let _ = adapter
                        .stage_supervisor_trust_revoke_failed(
                            session_id,
                            obligation.peer_id().clone(),
                            obligation.epoch(),
                            error.clone(),
                        )
                        .await;
                    return Err(format!(
                        "supervisor rotation operation left previous revoke pending: {error}"
                    ));
                }
                transition = adapter
                    .stage_supervisor_rotation_previous_revoked(
                        session_id,
                        operation_id.to_string(),
                        obligation.peer_id().clone(),
                        obligation.epoch(),
                    )
                    .await
                    .map_err(|error| {
                        format!(
                            "previous supervisor route is absent but rotation checkpoint persistence failed: {error}"
                        )
                    })?;
                phase = SupervisorRotationPhase::NextPublishPending;
            }
            SupervisorRotationPhase::NextPublishPending => {
                let obligation = single_supervisor_publish_obligation(
                    adapter,
                    session_id,
                    &transition,
                    "supervisor rotation operation",
                )
                .await?;
                validate_supervisor_publish_obligation(
                    &obligation,
                    next,
                    next_epoch,
                    "supervisor rotation operation",
                )?;
                let authority = supervisor_publish_authority(&obligation)?;
                let publication = crate::tokio::time::timeout(
                    SUPERVISOR_RESPONSE_ROUTE_IO_TIMEOUT,
                    apply_generated_private_trust_add(
                        comms_runtime.as_ref(),
                        next.clone(),
                        authority,
                    ),
                )
                .await;
                if let Err(error) = match publication {
                    Ok(result) => result,
                    Err(_) => Err("next supervisor trust publication timed out".to_string()),
                } {
                    let _ = adapter
                        .stage_supervisor_trust_publish_failed(
                            session_id,
                            obligation.peer_id().clone(),
                            obligation.epoch(),
                            error.clone(),
                        )
                        .await;
                    return Err(format!(
                        "supervisor rotation operation left next publication pending: {error}"
                    ));
                }
                // Do not compensate the live add if this durable checkpoint
                // fails. Recovery resumes NextPublishPending and idempotently
                // republishes the same generated source before retrying the
                // checkpoint.
                adapter
                    .stage_supervisor_rotation_next_published(
                        session_id,
                        operation_id.to_string(),
                        obligation.peer_id().clone(),
                        obligation.epoch(),
                    )
                    .await
                    .map_err(|error| {
                        format!(
                            "next supervisor route is live but rotation checkpoint persistence failed: {error}"
                        )
                    })?;
                phase = SupervisorRotationPhase::Completed;
            }
            SupervisorRotationPhase::Completed | SupervisorRotationPhase::Rejected => return Ok(()),
        }
    }
}

fn trusted_peer_from_generated_binding(
    binding: &crate::meerkat_machine::GeneratedSupervisorBinding,
) -> Result<TrustedPeerDescriptor, String> {
    TrustedPeerDescriptor::unsigned_with_pubkey(
        binding.name.clone(),
        &binding.peer_id,
        decode_supervisor_signing_public_key(&binding.signing_public_key)?,
        &binding.address,
    )
}

async fn start_supervisor_rotation_worker_if_pending(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
) -> Result<bool, String> {
    let Some(receipt) = adapter
        .active_supervisor_rotation(session_id)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(false);
    };
    if matches!(
        receipt.phase,
        crate::meerkat_machine::dsl::SupervisorRotationPhase::Completed
            | crate::meerkat_machine::dsl::SupervisorRotationPhase::Rejected
    ) {
        return Ok(false);
    }
    let operation_id = receipt.operation_id.clone();
    let worker_adapter = Arc::clone(adapter);
    let worker_session_id = session_id.clone();
    let worker_runtime = Arc::clone(comms_runtime);
    let slot_runtime = Arc::clone(comms_runtime);
    let task_operation_id = operation_id.clone();
    let (start_tx, start_rx) = crate::tokio::sync::oneshot::channel::<()>();
    let task = crate::tokio::spawn(async move {
        if start_rx.await.is_err() {
            return;
        }
        let mut retry_delay = std::time::Duration::from_millis(25);
        loop {
            match worker_adapter
                .peer_ingress_runtime_is_current(&worker_session_id, &worker_runtime)
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(
                        session_id = %worker_session_id,
                        operation_id = %task_operation_id,
                        "supervisor rotation worker refused stale runtime retry"
                    );
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        session_id = %worker_session_id,
                        operation_id = %task_operation_id,
                        %error,
                        "supervisor rotation worker lost runtime authority before retry"
                    );
                    return;
                }
            }
            let receipt = match worker_adapter
                .active_supervisor_rotation(&worker_session_id)
                .await
            {
                Ok(Some(receipt)) if receipt.operation_id == task_operation_id => receipt,
                Ok(_) => return,
                Err(error) => {
                    tracing::warn!(
                        session_id = %worker_session_id,
                        operation_id = %task_operation_id,
                        %error,
                        "supervisor rotation worker lost its registered session"
                    );
                    return;
                }
            };
            if matches!(
                receipt.phase,
                crate::meerkat_machine::dsl::SupervisorRotationPhase::Completed
                    | crate::meerkat_machine::dsl::SupervisorRotationPhase::Rejected
            ) {
                return;
            }
            let next = match trusted_peer_from_generated_binding(&receipt.next) {
                Ok(next) => next,
                Err(error) => {
                    tracing::error!(
                        session_id = %worker_session_id,
                        operation_id = %task_operation_id,
                        %error,
                        "supervisor rotation receipt carried an invalid target"
                    );
                    return;
                }
            };
            let previous = BoundSupervisorState {
                name: receipt.previous.name,
                peer_id: receipt.previous.peer_id,
                address: receipt.previous.address,
                signing_public_key: receipt.previous.signing_public_key,
                epoch: receipt.previous.epoch,
            };
            match drive_supervisor_rotation(
                &worker_adapter,
                &worker_session_id,
                &worker_runtime,
                SupervisorRotationDriveContext {
                    operation_id: &task_operation_id,
                    previous: &previous,
                    next: &next,
                    next_epoch: receipt.next.epoch,
                    phase: receipt.phase,
                },
            )
            .await
            {
                Ok(()) => return,
                Err(error) => {
                    tracing::warn!(
                        session_id = %worker_session_id,
                        operation_id = %task_operation_id,
                        %error,
                        ?retry_delay,
                        "supervisor rotation remains pending; session-owned worker will retry"
                    );
                    crate::tokio::time::sleep(retry_delay).await;
                    retry_delay = std::cmp::min(
                        retry_delay.saturating_mul(2),
                        std::time::Duration::from_secs(1),
                    );
                }
            }
        }
    });
    let installed = adapter
        .install_supervisor_rotation_task_if_current(session_id, operation_id, slot_runtime, task)
        .await
        .map_err(|error| error.to_string())?;
    if installed {
        start_tx
            .send(())
            .map_err(|()| "supervisor rotation worker exited before start".to_string())?;
    }
    Ok(installed)
}

async fn hydrate_current_bound_supervisor_route(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
) -> Result<bool, String> {
    let binding = adapter.supervisor_binding(session_id).await;
    let Some(bound) = BoundSupervisorState::from_binding(&binding) else {
        return Ok(false);
    };
    let descriptor = TrustedPeerDescriptor::unsigned_with_pubkey(
        bound.name,
        &bound.peer_id,
        decode_supervisor_signing_public_key(&bound.signing_public_key)?,
        &bound.address,
    )?;
    ensure_bridge_response_route_for_descriptor(
        adapter,
        session_id,
        comms_runtime,
        descriptor,
        "supervisor runtime hydration",
    )
    .await
    .map_err(|(_, reason)| reason)?;
    Ok(true)
}

async fn hydrate_or_resume_supervisor_rotation_runtime(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
) -> Result<(), String> {
    match adapter
        .active_supervisor_rotation(session_id)
        .await
        .map_err(|error| error.to_string())?
        .map(|receipt| receipt.phase)
    {
        Some(
            crate::meerkat_machine::dsl::SupervisorRotationPhase::PreviousRevokePending
            | crate::meerkat_machine::dsl::SupervisorRotationPhase::NextPublishPending,
        ) => {
            start_supervisor_rotation_worker_if_pending(adapter, session_id, comms_runtime).await?;
        }
        Some(
            crate::meerkat_machine::dsl::SupervisorRotationPhase::Completed
            | crate::meerkat_machine::dsl::SupervisorRotationPhase::Rejected,
        )
        | None => {
            // Terminal receipts retain a Bound authority (next on Completed,
            // previous on Rejected). A replacement CommsRuntime has an empty
            // mechanical trust store, so hydrate that generated current route.
            // Pending phases are deliberately excluded: their binding is
            // Unbound and only the existing rotation obligation may publish.
            let _ =
                hydrate_current_bound_supervisor_route(adapter, session_id, comms_runtime).await?;
        }
    }
    Ok(())
}

fn bridge_supervisor_rotation_state(
    receipt: crate::meerkat_machine::GeneratedSupervisorRotationReceipt,
) -> Result<BridgeSupervisorRotationState, String> {
    let operation_id = receipt
        .operation_id
        .parse()
        .map_err(|error| format!("stored supervisor rotation operation id is invalid: {error}"))?;
    let target = BridgePeerSpec {
        name: receipt.next.name,
        peer_id: receipt.next.peer_id,
        address: receipt.next.address,
        pubkey: decode_supervisor_signing_public_key(&receipt.next.signing_public_key)?,
    };
    let operation = BridgeSupervisorRotationOperationReceipt {
        operation_id,
        target: BridgeSupervisorRotationTargetReceipt {
            target,
            target_epoch: receipt.next.epoch,
        },
    };
    match receipt.phase {
        crate::meerkat_machine::dsl::SupervisorRotationPhase::PreviousRevokePending => {
            Ok(BridgeSupervisorRotationState::Pending {
                operation,
                phase: BridgeSupervisorRotationPendingPhase::PreviousRevokePending,
            })
        }
        crate::meerkat_machine::dsl::SupervisorRotationPhase::NextPublishPending => {
            Ok(BridgeSupervisorRotationState::Pending {
                operation,
                phase: BridgeSupervisorRotationPendingPhase::NextPublishPending,
            })
        }
        crate::meerkat_machine::dsl::SupervisorRotationPhase::Completed => {
            Ok(BridgeSupervisorRotationState::Completed { receipt: operation })
        }
        crate::meerkat_machine::dsl::SupervisorRotationPhase::Rejected => {
            let rejection = receipt.rejection.ok_or_else(|| {
                "stored rejected supervisor rotation omitted its typed cause".to_string()
            })?;
            let cause = match rejection {
                crate::meerkat_machine::dsl::SupervisorRotationRejectionKind::OperationConflict => {
                    BridgeSupervisorRotationRejectionCause::OperationConflict
                }
                crate::meerkat_machine::dsl::SupervisorRotationRejectionKind::NotBound => {
                    BridgeSupervisorRotationRejectionCause::Internal
                }
                crate::meerkat_machine::dsl::SupervisorRotationRejectionKind::SenderMismatch => {
                    BridgeSupervisorRotationRejectionCause::SenderMismatch
                }
                crate::meerkat_machine::dsl::SupervisorRotationRejectionKind::TargetEpochNotAdvanced => {
                    BridgeSupervisorRotationRejectionCause::StaleTargetEpoch
                }
                crate::meerkat_machine::dsl::SupervisorRotationRejectionKind::InvalidTarget => {
                    BridgeSupervisorRotationRejectionCause::InvalidTarget
                }
                crate::meerkat_machine::dsl::SupervisorRotationRejectionKind::UnsupportedProtocolVersion => {
                    BridgeSupervisorRotationRejectionCause::UnsupportedProtocolVersion
                }
            };
            Ok(BridgeSupervisorRotationState::Rejected {
                receipt: BridgeSupervisorRotationRejectionReceipt {
                    operation,
                    cause,
                    reason: format!("supervisor rotation rejected: {rejection:?}"),
                },
            })
        }
    }
}

#[derive(Debug)]
enum SupervisorAuthorizeApplyError {
    Rejected(crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind),
    Internal(String),
}

/// Apply one generated `AuthorizeSupervisor` decision and publish its private
/// trust edge before returning. Both wire ingress and host-seeded member
/// refresh use this owner so monotonic admission cannot drift between paths.
#[allow(clippy::too_many_arguments)]
async fn apply_supervisor_authorization(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    comms_runtime: &dyn CommsRuntime,
    supervisor: &TrustedPeerDescriptor,
    epoch: u64,
    sender_peer_id: Option<String>,
    sender_signing_public_key: Option<String>,
    context: &str,
) -> Result<(), SupervisorAuthorizeApplyError> {
    adapter
        .stage_local_endpoint_for_comms_runtime(session_id, comms_runtime)
        .await
        .map_err(|error| {
            SupervisorAuthorizeApplyError::Internal(format!(
                "{context}: local endpoint rejected: {error}"
            ))
        })?;

    let admission = adapter
        .resolve_supervisor_authorize_admission(
            session_id,
            GeneratedSupervisorBinding {
                name: supervisor.name.as_str().to_owned(),
                peer_id: supervisor.peer_id.as_str(),
                address: supervisor.address.to_string(),
                signing_public_key: encode_supervisor_signing_public_key(supervisor.pubkey),
                epoch,
            },
            sender_peer_id.clone(),
            sender_signing_public_key,
        )
        .await
        .map_err(|error| {
            SupervisorAuthorizeApplyError::Internal(format!(
                "{context}: generated admission failed: {error}"
            ))
        })?;

    match admission {
        SupervisorAuthorizeAdmission::IdempotentAck => refresh_and_publish_bound_supervisor_route(
            adapter,
            session_id,
            comms_runtime,
            supervisor,
            epoch,
            sender_peer_id,
            context,
        )
        .await
        .map_err(SupervisorAuthorizeApplyError::Internal),
        SupervisorAuthorizeAdmission::Proceed(_) => {
            let transition = adapter
                .stage_supervisor_authorize(
                    session_id,
                    supervisor.name.as_str().to_owned(),
                    supervisor.peer_id.as_str(),
                    supervisor.address.to_string(),
                    encode_supervisor_signing_public_key(supervisor.pubkey),
                    epoch,
                )
                .await
                .map_err(|error| {
                    SupervisorAuthorizeApplyError::Internal(format!(
                        "{context}: version advance failed: {error}"
                    ))
                })?;
            let obligation =
                single_supervisor_publish_obligation(adapter, session_id, &transition, context)
                    .await
                    .map_err(SupervisorAuthorizeApplyError::Internal)?;
            validate_supervisor_publish_obligation(&obligation, supervisor, epoch, context)
                .map_err(SupervisorAuthorizeApplyError::Internal)?;
            publish_supervisor_trust_from_generated_obligation(
                adapter,
                session_id,
                comms_runtime,
                &obligation,
            )
            .await
            .map_err(|error| {
                SupervisorAuthorizeApplyError::Internal(format!(
                    "{context}: version publish failed: {error}"
                ))
            })
        }
        SupervisorAuthorizeAdmission::Rejected(rejection) => {
            Err(SupervisorAuthorizeApplyError::Rejected(rejection))
        }
    }
}

async fn handle_authorize_supervisor_command(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
    sender: &PeerIngressFact,
    payload: BridgeSupervisorPayload,
) -> bool {
    let supervisor = match bridge_peer_identity(&payload.supervisor, "authorize supervisor failed")
    {
        Ok(supervisor) => supervisor,
        Err((cause, reason)) => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                cause,
                reason,
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return true;
        }
    };
    let supervisor_spec = match TrustedPeerDescriptor::try_from(payload.supervisor.clone()) {
        Ok(supervisor_spec) => supervisor_spec,
        Err(error) => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                BridgeRejectionCause::InvalidSupervisorSpec,
                format!("authorize supervisor failed: invalid supervisor peer spec: {error}"),
                Some(payload.supervisor.address.as_str()),
            )
            .await;
            return true;
        }
    };
    match apply_supervisor_authorization(
        adapter,
        session_id,
        comms_runtime.as_ref(),
        &supervisor_spec,
        payload.epoch,
        generated_sender_peer_id(sender),
        generated_sender_signing_public_key(sender),
        "authorize supervisor",
    )
    .await
    {
        Ok(()) => {
            if crate::tokio::time::timeout(
                SUPERVISOR_RESPONSE_ROUTE_IO_TIMEOUT,
                send_bridge_response(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Ack(BridgeAck { ok: true }),
                    Some(payload.supervisor.address.as_str()),
                ),
            )
            .await
            .is_err()
            {
                comms_runtime.mark_interaction_complete(&candidate.interaction.id);
            }
        }
        Err(SupervisorAuthorizeApplyError::Rejected(rejection)) => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                supervisor_authorize_admission_rejection_cause(rejection),
                supervisor_authorize_admission_rejection_reason(rejection, sender, &supervisor),
                Some(payload.supervisor.address.as_str()),
            )
            .await;
        }
        Err(SupervisorAuthorizeApplyError::Internal(error)) => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                BridgeRejectionCause::Internal,
                error,
                Some(payload.supervisor.address.as_str()),
            )
            .await;
        }
    }
    true
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum SupervisorDeliveryMarker {
    SubmitSupervisorRotation,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSupervisorRotationSubmitDelivery {
    #[serde(rename = "delivery")]
    _delivery: SupervisorDeliveryMarker,
    operation_id: SupervisorRotationOperationId,
    target: BridgePeerSpec,
    target_epoch: u64,
    protocol_version: u32,
}

#[derive(Default)]
struct SupervisorDeliveryMarkerProbe {
    delivery_key_count: usize,
    reserved_rotation_marker_seen: bool,
}

impl<'de> serde::Deserialize<'de> for SupervisorDeliveryMarkerProbe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ProbeVisitor;

        impl<'de> serde::de::Visitor<'de> for ProbeVisitor {
            type Value = SupervisorDeliveryMarkerProbe;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a supervisor delivery JSON object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut probe = SupervisorDeliveryMarkerProbe::default();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "delivery" {
                        probe.delivery_key_count += 1;
                        let value = map.next_value::<serde_json::Value>()?;
                        if value.as_str() == Some("submit_supervisor_rotation") {
                            probe.reserved_rotation_marker_seen = true;
                        }
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(probe)
            }
        }

        deserializer.deserialize_map(ProbeVisitor)
    }
}

async fn try_handle_supervisor_bridge_delivery(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
) -> bool {
    let InteractionContent::Message { body, .. } = &candidate.interaction.content else {
        return false;
    };
    let Ok(marker_probe) = serde_json::from_str::<SupervisorDeliveryMarkerProbe>(body) else {
        return false;
    };
    if !marker_probe.reserved_rotation_marker_seen {
        return false;
    }
    if marker_probe.delivery_key_count != 1 {
        tracing::warn!(
            %session_id,
            delivery_key_count = marker_probe.delivery_key_count,
            "duplicate/conflicting supervisor rotation delivery markers consumed fail-closed"
        );
        comms_runtime.mark_interaction_complete(&candidate.interaction.id);
        return true;
    }
    let delivery = match serde_json::from_str::<RawSupervisorRotationSubmitDelivery>(body) {
        Ok(delivery) => delivery,
        Err(error) => {
            tracing::warn!(%session_id, %error, "malformed supervisor rotation delivery consumed fail-closed");
            comms_runtime.mark_interaction_complete(&candidate.interaction.id);
            return true;
        }
    };
    let sender_peer_id = generated_sender_peer_id(&candidate.ingress);
    let sender_signing_public_key = generated_sender_signing_public_key(&candidate.ingress);
    let raw_target = delivery.target;
    let (next, preflight_rejection) = if BridgeProtocolVersion::try_from(delivery.protocol_version)
        .ok()
        == Some(BridgeProtocolVersion::V4)
    {
        match TrustedPeerDescriptor::try_from(raw_target.clone()) {
            Ok(target) => (
                GeneratedSupervisorBinding {
                    name: target.name.as_str().to_owned(),
                    peer_id: target.peer_id.as_str(),
                    address: target.address.to_string(),
                    signing_public_key: encode_supervisor_signing_public_key(target.pubkey),
                    epoch: delivery.target_epoch,
                },
                None,
            ),
            Err(error) => {
                tracing::warn!(%session_id, %error, "durably rejecting one-way supervisor rotation with invalid target");
                (
                    GeneratedSupervisorBinding {
                        name: raw_target.name,
                        peer_id: raw_target.peer_id,
                        address: raw_target.address,
                        signing_public_key: encode_supervisor_signing_public_key(raw_target.pubkey),
                        epoch: delivery.target_epoch,
                    },
                    Some(
                        crate::meerkat_machine::dsl::SupervisorRotationRejectionKind::InvalidTarget,
                    ),
                )
            }
        }
    } else {
        (
            GeneratedSupervisorBinding {
                name: raw_target.name,
                peer_id: raw_target.peer_id,
                address: raw_target.address,
                signing_public_key: encode_supervisor_signing_public_key(raw_target.pubkey),
                epoch: delivery.target_epoch,
            },
            Some(
                crate::meerkat_machine::dsl::SupervisorRotationRejectionKind::UnsupportedProtocolVersion,
            ),
        )
    };
    if preflight_rejection.is_none()
        && let Err(error) = adapter
            .stage_local_endpoint_for_comms_runtime(session_id, comms_runtime.as_ref())
            .await
    {
        tracing::warn!(%session_id, %error, "unable to stage local endpoint for supervisor rotation submission");
        comms_runtime.mark_interaction_complete(&candidate.interaction.id);
        return true;
    }
    let submission = GeneratedSupervisorRotationSubmit {
        operation_id: delivery.operation_id.to_string(),
        next,
        preflight_rejection,
        sender_peer_id,
        sender_signing_public_key,
    };
    match adapter
        .submit_supervisor_rotation(session_id, submission)
        .await
    {
        Ok(
            SupervisorRotationSubmission::New(_) | SupervisorRotationSubmission::ExistingPending(_),
        ) => {
            if let Err(error) =
                start_supervisor_rotation_worker_if_pending(adapter, session_id, comms_runtime)
                    .await
            {
                tracing::warn!(%session_id, %error, "supervisor rotation was durably accepted but worker start failed");
            }
        }
        Ok(SupervisorRotationSubmission::ExistingTerminal) => {}
        Ok(
            SupervisorRotationSubmission::Rejected(rejection)
            | SupervisorRotationSubmission::Conflict(rejection),
        ) => {
            tracing::warn!(%session_id, ?rejection, "one-way supervisor rotation submission rejected");
        }
        Err(error) => {
            tracing::warn!(%session_id, %error, "one-way supervisor rotation submission failed");
        }
    }
    // Submission acceptance is one-way. Completing local ingress is not a
    // semantic supervisor response and cannot be observed as an operation
    // terminal; callers use ObserveSupervisorRotation.
    comms_runtime.mark_interaction_complete(&candidate.interaction.id);
    true
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
async fn serve_hard_cancel_member(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    sender: &PeerIngressFact,
    candidate: &PeerInputCandidate,
    payload: &BridgeHardCancelPayload,
) {
    // DEC-P6E-7: mirror of the machine-admitted local hard cancel, with the
    // bridge's exact-run fence folded into the same session mutation gate.
    // Reassert until that run is unbound/terminal so a cancel signal landing
    // before the executor arms its edge-triggered listener is not lost.
    let sup_payload = BridgeSupervisorPayload {
        supervisor: payload.supervisor.clone(),
        epoch: payload.epoch,
        protocol_version: payload.protocol_version,
    };
    if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
        adapter,
        session_id,
        comms_runtime,
        sender,
        &sup_payload,
        "hard cancel member failed",
    )
    .await
    {
        send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
        return;
    }
    if let Err((cause, reason)) = require_registered_member_incarnation(
        adapter,
        session_id,
        &payload.expected_member,
        "hard cancel member",
    ) {
        send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
        return;
    }
    let deadline = Instant::now() + HARD_CANCEL_SETTLE_TIMEOUT;
    loop {
        match adapter
            .hard_cancel_run_if_current_for_member_incarnation(
                session_id,
                &payload.expected_run_id,
                &payload.expected_member,
                payload.reason.clone(),
            )
            .await
        {
            // The exact expected run is no longer current (or its lifecycle is
            // terminal). This is the ONLY successful reply point. In
            // particular, a newer current run also lands here without seeing
            // an interrupt.
            Ok(false) => {
                if let Err((cause, reason)) = require_registered_member_incarnation(
                    adapter,
                    session_id,
                    &payload.expected_member,
                    "hard cancel member completion",
                ) {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return;
                }
                tracing::debug!(
                    operation_id = %payload.operation_id,
                    expected_run_id = %payload.expected_run_id,
                    "hard-cancel operation reached exact-run terminal truth"
                );
                send_bridge_response(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Ack(BridgeAck { ok: true }),
                    None,
                )
                .await;
                return;
            }
            Ok(true) => {}
            Err(crate::traits::RuntimeDriverError::StaleAuthority { reason }) => {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::StaleFence,
                    reason,
                    None,
                )
                .await;
                return;
            }
            // ADJ-P4-7: `Unavailable` for not-live/not-ready;
            // `Unsupported` stays reserved for unserved commands.
            Err(crate::traits::RuntimeDriverError::NotReady { state }) => {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Unavailable,
                    format!(
                        "hard-cancel operation {} could not cancel expected run {} in state {state:?}",
                        payload.operation_id, payload.expected_run_id
                    ),
                    None,
                )
                .await;
                return;
            }
            Err(crate::traits::RuntimeDriverError::Destroyed) => {
                // A destroyed lifecycle is terminal for every prior run. The
                // run-fenced helper normally classifies this as Ok(false), but
                // preserve terminal ACK truth if destruction races its staged
                // interrupt realization.
                send_bridge_response(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Ack(BridgeAck { ok: true }),
                    None,
                )
                .await;
                return;
            }
            Err(error) => {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!(
                        "hard-cancel operation {} for expected run {} failed: {error}",
                        payload.operation_id, payload.expected_run_id
                    ),
                    None,
                )
                .await;
                return;
            }
        }

        if Instant::now() >= deadline {
            send_bridge_failure(
                comms_runtime,
                candidate,
                BridgeRejectionCause::Unavailable,
                format!(
                    "hard-cancel operation {} timed out waiting for expected run {} to become terminal",
                    payload.operation_id, payload.expected_run_id
                ),
                None,
            )
            .await;
            return;
        }
        crate::tokio::time::sleep(HARD_CANCEL_REASSERT_INTERVAL).await;
    }
}

async fn serve_cancel_tracked_member_input(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    sender: &PeerIngressFact,
    candidate: &PeerInputCandidate,
    payload: &BridgeTrackedInputCancelPayload,
) {
    let sup_payload = BridgeSupervisorPayload {
        supervisor: payload.supervisor.clone(),
        epoch: payload.epoch,
        protocol_version: payload.protocol_version,
    };
    if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
        adapter,
        session_id,
        comms_runtime,
        sender,
        &sup_payload,
        "cancel tracked member input failed",
    )
    .await
    {
        send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
        return;
    }
    if let Err((cause, reason)) = require_registered_member_incarnation(
        adapter,
        session_id,
        &payload.expected_member,
        "cancel tracked member input",
    ) {
        send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
        return;
    }
    match uuid::Uuid::parse_str(&payload.input_id) {
        Ok(input_uuid) if !input_uuid.is_nil() && input_uuid.to_string() == payload.input_id => {}
        Ok(_) => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                BridgeRejectionCause::Unsupported,
                "tracked-input cancel input_id must be a canonical non-nil UUID".to_string(),
                None,
            )
            .await;
            return;
        }
        Err(error) => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                BridgeRejectionCause::Unsupported,
                format!("tracked-input cancel input_id is not a UUID: {error}"),
                None,
            )
            .await;
            return;
        }
    }
    let observation = match resolve_member_observation_host(adapter) {
        Ok(observation) => observation,
        Err((cause, reason)) => {
            send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
            return;
        }
    };
    match observation
        .cancel_tracked_member_input(session_id, &payload.expected_member, &payload.input_id)
        .await
    {
        Ok(outcome) => {
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::TrackedInputCancelled(BridgeTrackedInputCancelResponse {
                    expected_member: payload.expected_member.clone(),
                    input_id: payload.input_id.clone(),
                    outcome,
                }),
                None,
            )
            .await;
        }
        Err(error) => {
            let (cause, reason) = observation_error_to_bridge_rejection(&error);
            send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
        }
    }
}

async fn try_handle_supervisor_bridge_command(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
) -> bool {
    if try_handle_supervisor_bridge_delivery(adapter, session_id, comms_runtime, candidate).await {
        return true;
    }
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
                None,
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
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        cause,
                        reason,
                        Some(payload.supervisor.address.as_str()),
                    )
                    .await;
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
                        Some(payload.supervisor.address.as_str()),
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
                            Some(payload.supervisor.address.as_str()),
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
                            Some(payload.supervisor.address.as_str()),
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
                                    Some(payload.supervisor.address.as_str()),
                                )
                                .await;
                                return true;
                            }
                        };
                    if let Err(error) = republish_private_supervisor_trust(
                        adapter,
                        session_id,
                        comms_runtime.as_ref(),
                        &supervisor_spec,
                        payload.epoch,
                        generated_sender_peer_id(sender),
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
                            Some(payload.supervisor.address.as_str()),
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
                            capabilities: bridge_capabilities(payload.protocol_version),
                        }),
                        None,
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
                        Some(payload.supervisor.address.as_str()),
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
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            cause,
                            reason,
                            Some(payload.supervisor.address.as_str()),
                        )
                        .await;
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
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return true;
            };
            // Bootstrap commit — shared with the host materializer
            // (`bind_supervisor_for_materialized_session`): local endpoint,
            // `BindSupervisor` (typed PeerName/PeerId atoms convert to the
            // stringly DSL payload at that single seam), single generated
            // publish obligation validated against the staged binding, and
            // the obligation spend as a PRIVATE trust add — with rollback
            // when publication fails after the DSL commit.
            if let Err(reason) = commit_supervisor_bind_with_rollback(
                adapter,
                session_id,
                comms_runtime.as_ref(),
                &supervisor_spec,
                payload.epoch,
                "bind member",
            )
            .await
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    reason,
                    Some(payload.supervisor.address.as_str()),
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
                    capabilities: bridge_capabilities(payload.protocol_version),
                }),
                None,
            )
            .await;
            true
        }
        BridgeCommand::AuthorizeSupervisor(payload) => {
            handle_authorize_supervisor_command(
                adapter,
                session_id,
                comms_runtime,
                candidate,
                sender,
                payload,
            )
            .await
        }
        BridgeCommand::RevokeSupervisor(payload) => {
            let authorized_supervisor = match resolve_authorized_supervisor_cleanup(
                adapter,
                session_id,
                sender,
                &payload,
                crate::meerkat_machine::dsl::SupervisorCleanupCommandKind::Revoke,
            )
            .await
            {
                Ok(supervisor) => supervisor,
                Err((BridgeRejectionCause::NotBound, reason)) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::NotBound,
                        reason,
                        Some(payload.supervisor.address.as_str()),
                    )
                    .await;
                    return true;
                }
                Err((cause, reason)) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        cause,
                        reason,
                        Some(payload.supervisor.address.as_str()),
                    )
                    .await;
                    return true;
                }
            };
            let resume_pending_revoke = authorized_supervisor.resume_pending_revoke;
            let authorized_supervisor = authorized_supervisor.supervisor;
            if let Err(error) = adapter
                .stage_local_endpoint_for_comms_runtime(session_id, comms_runtime.as_ref())
                .await
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("revoke supervisor failed: local endpoint rejected: {error}"),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
                return true;
            }
            let supervisor_peer_id = authorized_supervisor.peer_id.as_str();
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
                        Some(payload.supervisor.address.as_str()),
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
                        Some(payload.supervisor.address.as_str()),
                    )
                    .await;
                    return true;
                }
            };
            let revoke_authority = match supervisor_revoke_authority(&revoke_obligation) {
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
                        Some(payload.supervisor.address.as_str()),
                    )
                    .await;
                    return true;
                }
            };
            // Revoke acknowledgement is acceptance of the durable cleanup,
            // not proof that a reply route survived trust removal. Send it
            // before spending the one-shot remove authority; never re-trust a
            // revoked supervisor solely to deliver a terminal response.
            if resume_pending_revoke {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::NotBound,
                    "supervisor revoke cleanup was already pending".to_string(),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
            } else {
                send_bridge_response(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Ack(BridgeAck { ok: true }),
                    Some(payload.supervisor.address.as_str()),
                )
                .await;
            }
            if let Err(error) = apply_generated_private_trust_remove(
                comms_runtime.as_ref(),
                revoke_obligation.peer_id().clone(),
                revoke_authority,
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
                tracing::warn!(%session_id, %reason, "accepted supervisor revoke remains pending");
                return true;
            }
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
                tracing::warn!(%session_id, %error, "accepted supervisor revoke cleanup checkpoint failed");
                return true;
            }
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
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            let request_input_id = payload.input_id.clone();
            let registered_incarnation = adapter.member_incarnation(session_id);
            match (&registered_incarnation, &payload.expected_member) {
                (Some(current), Some(expected)) if current == expected => {}
                (Some(current), _) => {
                    let response = BridgeDeliveryResponse {
                        input_id: request_input_id,
                        canonical_input_id: None,
                        outcome: BridgeDeliveryOutcome::Rejected {
                            cause: BridgeDeliveryRejectionCause::StaleMemberIncarnation {
                                current: current.clone(),
                            },
                            reason: format!(
                                "delivery expected member incarnation {:?}; current is {:?}",
                                payload.expected_member, current
                            ),
                        },
                    };
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Delivery(response),
                        None,
                    )
                    .await;
                    return true;
                }
                (None, Some(expected)) => {
                    let response = BridgeDeliveryResponse {
                        input_id: request_input_id,
                        canonical_input_id: None,
                        outcome: BridgeDeliveryOutcome::Rejected {
                            cause: BridgeDeliveryRejectionCause::Internal {
                                detail: "target session has no registered host-member incarnation"
                                    .to_string(),
                            },
                            reason: format!(
                                "delivery addressed host member {expected:?}, but target session has no host journal"
                            ),
                        },
                    };
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Delivery(response),
                        None,
                    )
                    .await;
                    return true;
                }
                (None, None) => {}
            }
            // Validate the common delivery identity contract before the
            // tracked/untracked fork. In particular, an untracked placed
            // admission may carry an independent transcript id, so limiting
            // this check to terminal-custody requests would let under-V4 or
            // unscoped identity carriers bypass semantic validation.
            if let Err(detail) = validate_delivery_tracking_request(&payload) {
                let response = BridgeDeliveryResponse {
                    input_id: request_input_id,
                    canonical_input_id: None,
                    outcome: BridgeDeliveryOutcome::Rejected {
                        cause: BridgeDeliveryRejectionCause::TurnDirectiveUnsupported {
                            detail: detail.clone(),
                        },
                        reason: detail,
                    },
                };
                send_bridge_response(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Delivery(response),
                    None,
                )
                .await;
                return true;
            }
            // §18 O1/O2 (DEC-P6E-16, DEC-P6F-9): directive-bearing work and
            // an explicitly outcome-tracked plain interaction are admitted as
            // TRACKED turns — completion handle retained, terminal journaled —
            // or rejected typed at admission. Turn-bearing flow work keeps its
            // FlowStep lowering; tracked plain work keeps its Peer lowering.
            if delivery_requires_tracked_turn_custody(&payload) {
                serve_tracked_member_delivery(
                    adapter,
                    session_id,
                    comms_runtime,
                    candidate,
                    authorized_supervisor.peer_id,
                    payload,
                )
                .await;
                return true;
            }
            let expected_member = payload.expected_member.clone();
            let input = match peer_input_from_delivery_payload(
                session_id,
                authorized_supervisor.peer_id,
                payload,
            ) {
                Ok(input) => input,
                Err(reason) => {
                    let response = BridgeDeliveryResponse {
                        input_id: request_input_id,
                        canonical_input_id: None,
                        outcome: BridgeDeliveryOutcome::Rejected {
                            cause: BridgeDeliveryRejectionCause::Internal {
                                detail: reason.clone(),
                            },
                            reason,
                        },
                    };
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Delivery(response),
                        None,
                    )
                    .await;
                    return true;
                }
            };
            let accept_result = adapter
                .accept_input_with_completion_for_member_residency(
                    session_id,
                    input,
                    expected_member.as_ref(),
                )
                .await;
            match accept_result {
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
                        None,
                    )
                    .await;
                }
                Err(crate::traits::RuntimeDriverError::StaleAuthority { reason }) => {
                    if let Some(current) = adapter.member_incarnation(session_id) {
                        let response = BridgeDeliveryResponse {
                            input_id: request_input_id,
                            canonical_input_id: None,
                            outcome: BridgeDeliveryOutcome::Rejected {
                                cause: BridgeDeliveryRejectionCause::StaleMemberIncarnation {
                                    current,
                                },
                                reason,
                            },
                        };
                        send_bridge_response(
                            comms_runtime,
                            candidate,
                            meerkat_core::interaction::ResponseStatus::Completed,
                            BridgeReply::Delivery(response),
                            None,
                        )
                        .await;
                    } else {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::StaleFence,
                            reason,
                            None,
                        )
                        .await;
                    }
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("deliver member input failed: {error}"),
                        None,
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::InterruptMember(payload) => {
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "interrupt member failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            if let Err((cause, reason)) = require_optional_registered_member_incarnation(
                adapter,
                session_id,
                payload.expected_member.as_ref(),
                "interrupt member",
            ) {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            let result = adapter
                .cancel_after_boundary_for_optional_member_incarnation(
                    session_id,
                    payload.expected_member.as_ref(),
                )
                .await;
            match result {
                Ok(()) => {
                    if let Err((cause, reason)) = require_optional_registered_member_incarnation(
                        adapter,
                        session_id,
                        payload.expected_member.as_ref(),
                        "interrupt member completion",
                    ) {
                        send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                        return true;
                    }
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                        None,
                    )
                    .await;
                }
                Err(crate::traits::RuntimeDriverError::StaleAuthority { reason }) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::StaleFence,
                        reason,
                        None,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("interrupt member failed: {error}"),
                        None,
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::RetireMember(payload) => {
            if let Err((cause, reason)) = resolve_authorized_supervisor_cleanup_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &payload,
                crate::meerkat_machine::dsl::SupervisorCleanupCommandKind::Retire,
                "retire member failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
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
                        None,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("retire member failed: {error}"),
                        None,
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::DestroyMember(payload) => {
            if let Err((cause, reason)) = resolve_authorized_supervisor_cleanup_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &payload,
                crate::meerkat_machine::dsl::SupervisorCleanupCommandKind::Destroy,
                "destroy member failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
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
                        None,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("destroy member failed: {error}"),
                        None,
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::ObserveMember(payload) => {
            if let Err((cause, reason)) = resolve_authorized_supervisor_cleanup_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &payload,
                crate::meerkat_machine::dsl::SupervisorCleanupCommandKind::Observe,
                "observe member failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
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
                                    None,
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
                        None,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("observe member failed: {error}"),
                        None,
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
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
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
                        None,
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
                        None,
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
                        None,
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
                        send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                        return true;
                    }
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                        None,
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
                            send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                            return true;
                        }
                        send_bridge_response(
                            comms_runtime,
                            candidate,
                            meerkat_core::interaction::ResponseStatus::Completed,
                            BridgeReply::Ack(BridgeAck { ok: true }),
                            None,
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
                            None,
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
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
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
                        None,
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
                        None,
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
                        None,
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
                        send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                        return true;
                    }
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                        None,
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
                            send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                            return true;
                        }
                        send_bridge_response(
                            comms_runtime,
                            candidate,
                            meerkat_core::interaction::ResponseStatus::Completed,
                            BridgeReply::Ack(BridgeAck { ok: true }),
                            None,
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
                            None,
                        )
                        .await;
                    }
                }
            }
            true
        }
        BridgeCommand::ObserveSupervisorRotation(payload) => {
            if payload.protocol_version != BridgeProtocolVersion::V4 {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::UnsupportedProtocolVersion,
                    format!(
                        "observe supervisor rotation requires protocol v4, received {:?}",
                        payload.protocol_version
                    ),
                    Some(payload.observer.address.as_str()),
                )
                .await;
                return true;
            }
            let observer =
                match bridge_peer_identity(&payload.observer, "observe supervisor rotation failed")
                {
                    Ok(observer) => observer,
                    Err((cause, reason)) => {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            cause,
                            reason,
                            Some(payload.observer.address.as_str()),
                        )
                        .await;
                        return true;
                    }
                };
            let authenticated_peer_id = generated_sender_peer_id(sender);
            let authenticated_key = generated_sender_signing_public_key(sender);
            let observer_peer_id = observer.peer_id.as_str();
            let observer_key = encode_supervisor_signing_public_key(observer.pubkey.into_bytes());
            if authenticated_peer_id.as_deref() != Some(observer_peer_id.as_str())
                || authenticated_key.as_deref() != Some(observer_key.as_str())
            {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::SenderMismatch,
                    "observe supervisor rotation failed: observer does not match authenticated sender"
                        .to_string(),
                    Some(payload.observer.address.as_str()),
                )
                .await;
                return true;
            }
            let operation_id = payload.operation_id;
            let observer = GeneratedSupervisorBinding {
                name: observer.name.as_str().to_owned(),
                peer_id: observer_peer_id,
                address: observer.address.to_string(),
                signing_public_key: observer_key,
                epoch: payload.observer_epoch,
            };
            let reply =
                match adapter
                    .observe_supervisor_rotation(session_id, operation_id.to_string(), &observer)
                    .await
                {
                    Ok(SupervisorRotationObservation::Found(receipt)) => {
                        match bridge_supervisor_rotation_state(receipt) {
                            Ok(state) => BridgeReply::SupervisorRotation(
                                BridgeSupervisorRotationObservation::Found { state },
                            ),
                            Err(error) => {
                                send_bridge_failure(
                                    comms_runtime,
                                    candidate,
                                    BridgeRejectionCause::Internal,
                                    error,
                                    Some(payload.observer.address.as_str()),
                                )
                                .await;
                                return true;
                            }
                        }
                    }
                    Ok(SupervisorRotationObservation::NotFound) => BridgeReply::SupervisorRotation(
                        BridgeSupervisorRotationObservation::NotFound { operation_id },
                    ),
                    Err(error) => {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            format!("observe supervisor rotation failed: {error}"),
                            Some(payload.observer.address.as_str()),
                        )
                        .await;
                        return true;
                    }
                };
            // Observation reports machine-owned operation truth; it does not
            // mint a temporary trust edge. During PreviousRevokePending after
            // the old edge is gone, or NextPublishPending before the new edge
            // exists, no participant may be routable yet. The response may
            // therefore time out at the interaction layer until the existing
            // rotation obligation publishes the next route. Never retrust the
            // revoked supervisor merely to acknowledge or observe progress.
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                reply,
                Some(payload.observer.address.as_str()),
            )
            .await;
            true
        }
        BridgeCommand::DeclareMemberOutboundTaint(payload) => {
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "declare member outbound taint failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            let incarnation_check = match payload.target.as_ref() {
                Some(BridgeOutboundTaintTarget::Placed(expected)) => {
                    require_registered_member_incarnation(
                        adapter,
                        session_id,
                        expected,
                        "declare member outbound taint failed",
                    )
                }
                Some(BridgeOutboundTaintTarget::PeerOnly) | None => {
                    if let Some(current) = adapter.member_incarnation(session_id) {
                        Err((
                            BridgeRejectionCause::StaleFence,
                            format!(
                                "declare member outbound taint failed: peer-only or legacy target cannot address registered incarnation {current:?}"
                            ),
                        ))
                    } else {
                        Ok(())
                    }
                }
            };
            if let Err((cause, reason)) = incarnation_check {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            let expected_member = match payload.target.as_ref() {
                Some(BridgeOutboundTaintTarget::Placed(expected)) => Some(expected),
                Some(BridgeOutboundTaintTarget::PeerOnly) | None => None,
            };
            let effect_authority = match acquire_optional_member_effect_authority(
                adapter,
                session_id,
                expected_member,
                "declare member outbound taint",
            )
            .await
            {
                Ok(authority) => authority,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            // The host's declaration, relayed by the supervisor, installs on
            // THIS member runtime's outbound carrier config. Fail-closed: an
            // unsupported runtime rejects rather than silently dropping a
            // security declaration.
            let result = comms_runtime.set_outbound_content_taint(payload.taint);
            drop(effect_authority);
            match result {
                Ok(()) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::Ack(BridgeAck { ok: true }),
                        None,
                    )
                    .await;
                }
                Err(error) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        format!("declare member outbound taint failed: {error}"),
                        None,
                    )
                    .await;
                }
            }
            true
        }
        BridgeCommand::HardCancelMember(payload) => {
            serve_hard_cancel_member(
                adapter,
                session_id,
                comms_runtime,
                sender,
                candidate,
                &payload,
            )
            .await;
            true
        }
        BridgeCommand::CancelTrackedMemberInput(payload) => {
            serve_cancel_tracked_member_input(
                adapter,
                session_id,
                comms_runtime,
                sender,
                candidate,
                &payload,
            )
            .await;
            true
        }
        BridgeCommand::ReadMemberHistory(payload) => {
            // DEC-P6E-6: one page projection, shared with the local path
            // via `WireMemberHistoryPageBody::try_from_history_page`.
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "read member history failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            let observation = match resolve_member_observation_host(adapter) {
                Ok(observation) => observation,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            if let Err((cause, reason)) = require_registered_member_incarnation(
                adapter,
                session_id,
                &payload.expected_member,
                "read member history",
            ) {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            match observation
                .read_history(session_id, payload.from_index, payload.limit)
                .await
            {
                Ok(window) => {
                    if let Err((cause, reason)) = require_registered_member_incarnation(
                        adapter,
                        session_id,
                        &payload.expected_member,
                        "read member history completion",
                    ) {
                        send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                        return true;
                    }
                    match bounded_member_history_reply(window) {
                        Ok(reply) => {
                            send_bridge_response(
                                comms_runtime,
                                candidate,
                                meerkat_core::interaction::ResponseStatus::Completed,
                                reply,
                                None,
                            )
                            .await;
                        }
                        Err(MemberHistoryReplyError::RowTooLarge {
                            index,
                            encoded_bytes,
                        }) => {
                            send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::HistoryRowTooLarge {
                                index,
                                encoded_bytes: u64::try_from(encoded_bytes).unwrap_or(u64::MAX),
                                max_bytes: MEMBER_HISTORY_REPLY_MAX_BYTES as u64,
                            },
                            format!(
                                "member history row at index {index} is {encoded_bytes} bytes (maximum bridge reply budget {MEMBER_HISTORY_REPLY_MAX_BYTES})"
                            ),
                            None,
                        )
                        .await;
                        }
                        Err(MemberHistoryReplyError::Internal(reason)) => {
                            send_bridge_failure(
                                comms_runtime,
                                candidate,
                                BridgeRejectionCause::Internal,
                                reason,
                                None,
                            )
                            .await;
                        }
                    }
                }
                Err(error) => {
                    let (cause, reason) = observation_error_to_bridge_rejection(&error);
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                }
            }
            true
        }
        BridgeCommand::PollMemberEvents(payload) => {
            // DEC-P6E-3: authorize + resolve the substrate INLINE; the
            // bounded long-poll wait runs on a DETACHED responder task so a
            // poll never serializes `InterruptMember`/`RetireMember` for the
            // same member behind its wait window (ADJ-P4-12 mirrored
            // receiver-side). Exact outcome acknowledgements may prune
            // durable terminal rows, but are independently idempotent and do
            // not advance the event cursor (correlation is envelope-id).
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "poll member events failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            if payload.outcome_acks.len() > BRIDGE_TURN_OUTCOME_ACK_MAX {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!(
                        "turn-outcome acknowledgement batch has {} rows (maximum {})",
                        payload.outcome_acks.len(),
                        BRIDGE_TURN_OUTCOME_ACK_MAX
                    ),
                    None,
                )
                .await;
                return true;
            }
            let observation = match resolve_member_observation_host(adapter) {
                Ok(observation) => observation,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            let cursor = match payload.cursor {
                BridgeEventCursor::Tail => MemberObservationCursor::Tail,
                BridgeEventCursor::At { generation, seq } => {
                    MemberObservationCursor::At { generation, seq }
                }
            };
            let max = payload
                .max
                .unwrap_or(MEMBER_POLL_DEFAULT_MAX_ROWS)
                .clamp(1, MEMBER_POLL_MAX_ROWS);
            let outcome_acks = payload.outcome_acks;
            let max_outcomes = payload
                .max_outcomes
                .unwrap_or(MEMBER_POLL_DEFAULT_MAX_OUTCOMES)
                .clamp(1, MEMBER_POLL_MAX_OUTCOMES);
            let wait_ms = payload.wait_ms.unwrap_or(0).min(MEMBER_POLL_WAIT_MAX_MS);
            let wait = Duration::from_millis(u64::from(wait_ms));
            let request = MemberEventsPageRequest {
                cursor,
                max,
                wait,
                outcome_acks,
                max_outcomes,
                expected_member: payload.expected_member,
            };
            if wait_ms == 0 {
                // Fast path: serve and reply inline.
                serve_member_events_page(
                    adapter,
                    &observation,
                    session_id,
                    comms_runtime,
                    candidate,
                    request,
                )
                .await;
                return true;
            }
            let observation = Arc::clone(&observation);
            let adapter = Arc::clone(adapter);
            let session_id = session_id.clone();
            let comms_runtime = Arc::clone(comms_runtime);
            let candidate = candidate.clone();
            crate::tokio::spawn(async move {
                serve_member_events_page(
                    adapter.as_ref(),
                    &observation,
                    &session_id,
                    &comms_runtime,
                    &candidate,
                    request,
                )
                .await;
            });
            true
        }
        BridgeCommand::OpenMemberLiveChannel(payload) => {
            // ADJ-P6B-1 serving arm: authorize → resolve host → reach →
            // reply, the `serve_hard_cancel_member` order. Steps 1-2 run
            // inline; reach+reply run on a DETACHED responder task (the
            // `PollMemberEvents` precedent): a provider-adapter open is
            // seconds-scale network I/O and must not head-of-line-block
            // `InterruptMember`/`HardCancelMember` on the same drain.
            // Correlation is envelope-id; a control verb racing an
            // in-flight open honestly rejects `LiveChannelNotFound` (the
            // console cannot know the channel id before the open reply).
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "open member live channel failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            let live = match resolve_member_live_host(adapter) {
                Ok(live) => live,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            let session_id = session_id.clone();
            let adapter = Arc::clone(adapter);
            let comms_runtime = Arc::clone(comms_runtime);
            let candidate = candidate.clone();
            let expected_member = payload.expected_member;
            let turning_mode = payload.turning_mode;
            let transport = payload.transport;
            crate::tokio::spawn(async move {
                let effect_authority = match acquire_registered_member_effect_authority(
                    adapter.as_ref(),
                    &session_id,
                    &expected_member,
                    "open member live channel",
                )
                .await
                {
                    Ok(authority) => authority,
                    Err((cause, reason)) => {
                        send_bridge_failure(&comms_runtime, &candidate, cause, reason, None).await;
                        return;
                    }
                };
                let result = live.open(&session_id, turning_mode, transport).await;
                drop(effect_authority);
                match result {
                    Ok(open) => {
                        send_bridge_response(
                            &comms_runtime,
                            &candidate,
                            meerkat_core::interaction::ResponseStatus::Completed,
                            BridgeReply::MemberLiveChannelOpened(BridgeLiveOpenedResponse { open }),
                            None,
                        )
                        .await;
                    }
                    Err(error) => {
                        let (cause, reason) = member_live_error_to_bridge_rejection(&error);
                        send_bridge_failure(&comms_runtime, &candidate, cause, reason, None).await;
                    }
                }
            });
            true
        }
        BridgeCommand::CloseMemberLiveChannel(payload) => {
            // Inline: close is a local host/machine operation (no provider
            // round-trip). Close-what-you-name: an unknown channel is a
            // typed `LiveChannelNotFound` — idempotent-safe for the
            // caller-driven reply-loss reconciliation (DEC-P6B-C9).
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "close member live channel failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            let live = match resolve_member_live_host(adapter) {
                Ok(live) => live,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            let effect_authority = match acquire_registered_member_effect_authority(
                adapter,
                session_id,
                &payload.expected_member,
                "close member live channel",
            )
            .await
            {
                Ok(authority) => authority,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            let result = live.close(session_id, &payload.channel_id).await;
            drop(effect_authority);
            match result {
                Ok(status) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::MemberLiveChannelClosed { status },
                        None,
                    )
                    .await;
                }
                Err(error) => {
                    let (cause, reason) = member_live_error_to_bridge_rejection(&error);
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                }
            }
            true
        }
        BridgeCommand::MemberLiveChannelStatus(payload) => {
            // Inline read-only point read. Absent `channel_id` ⇒ the host
            // resolves `live_active_channel_by_session` for this bound
            // session; none active ⇒ typed `LiveChannelNotFound`
            // (ADJ-P6B-2 — the reply-loss discovery primitive).
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "member live channel status failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            let live = match resolve_member_live_host(adapter) {
                Ok(live) => live,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            let effect_authority = match acquire_registered_member_effect_authority(
                adapter,
                session_id,
                &payload.expected_member,
                "member live channel status",
            )
            .await
            {
                Ok(authority) => authority,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            let result = live.status(session_id, payload.channel_id).await;
            drop(effect_authority);
            match result {
                Ok(report) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::MemberLiveChannelStatusReport {
                            channel_id: report.channel_id,
                            status: report.status,
                        },
                        None,
                    )
                    .await;
                }
                Err(error) => {
                    let (cause, reason) = member_live_error_to_bridge_rejection(&error);
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                }
            }
            true
        }
        BridgeCommand::ControlMemberLiveChannel(payload) => {
            // Inline: turn-level control verbs are local host/machine
            // operations (DL10's closed verb set). Live `Interrupt` is
            // media-plane barge-in through the facade pipeline's
            // `LiveAdapterHost::send_command_observed` — NOT
            // `hard_cancel_current_run`; no effect-authority reach is added
            // here (DEC-P6B-L7).
            let sup_payload = BridgeSupervisorPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
            };
            if let Err((cause, reason)) = resolve_authorized_supervisor_with_response_route(
                adapter,
                session_id,
                comms_runtime,
                sender,
                &sup_payload,
                "control member live channel failed",
            )
            .await
            {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return true;
            }
            let live = match resolve_member_live_host(adapter) {
                Ok(live) => live,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            let effect_authority = match acquire_registered_member_effect_authority(
                adapter,
                session_id,
                &payload.expected_member,
                "control member live channel",
            )
            .await
            {
                Ok(authority) => authority,
                Err((cause, reason)) => {
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                    return true;
                }
            };
            let result = live
                .control(session_id, &payload.channel_id, payload.verb)
                .await;
            drop(effect_authority);
            match result {
                Ok(outcome) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        BridgeReply::MemberLiveChannelControlled(BridgeLiveControlledResponse {
                            outcome,
                        }),
                        None,
                    )
                    .await;
                }
                Err(error) => {
                    let (cause, reason) = member_live_error_to_bridge_rejection(&error);
                    send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
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
                None,
            )
            .await;
            true
        }
    }
}

#[derive(Debug)]
enum MemberHistoryReplyError {
    RowTooLarge { index: u64, encoded_bytes: usize },
    Internal(String),
}

/// Project and byte-bound one history page below the comms transport ceiling.
/// A multi-row page returns the maximal fitting prefix with an exact
/// `next_index`. A single poison row fails typed: returning an empty success
/// page would make every continuation retry the same index forever, while
/// silently skipping it would corrupt fork context.
fn bounded_member_history_reply(
    window: MemberHistoryWindow,
) -> Result<BridgeReply, MemberHistoryReplyError> {
    let mut page = BridgeMemberHistoryPage {
        generation: window.generation,
        page: WireMemberHistoryPageBody::try_from_history_page(&window.page).map_err(|error| {
            MemberHistoryReplyError::Internal(format!(
                "member history page projection failed: {error}"
            ))
        })?,
    };
    let encoded_len = |candidate: &BridgeMemberHistoryPage| {
        serde_json::to_vec(&BridgeReply::MemberHistoryPage(candidate.clone()))
            .map(|bytes| bytes.len())
            .map_err(|error| {
                MemberHistoryReplyError::Internal(format!(
                    "member history reply serialization failed: {error}"
                ))
            })
    };
    let full_bytes = encoded_len(&page)?;
    if full_bytes <= MEMBER_HISTORY_REPLY_MAX_BYTES {
        return Ok(BridgeReply::MemberHistoryPage(page));
    }

    let original_len = page.page.messages.len();
    if original_len == 0 {
        return Err(MemberHistoryReplyError::Internal(format!(
            "empty member history reply exceeds the {MEMBER_HISTORY_REPLY_MAX_BYTES} byte bridge budget"
        )));
    }
    if original_len == 1 {
        return Err(MemberHistoryReplyError::RowTooLarge {
            index: page.page.from_index,
            encoded_bytes: full_bytes,
        });
    }

    let candidate_with_prefix = |prefix_len: usize| {
        let mut candidate = page.clone();
        candidate.page.messages.truncate(prefix_len);
        if prefix_len < original_len {
            let served = u64::try_from(prefix_len).map_err(|_| {
                MemberHistoryReplyError::Internal(format!(
                    "member history prefix length {prefix_len} exceeds the u64 cursor domain"
                ))
            })?;
            candidate.page.next_index = Some(
                candidate
                    .page
                    .from_index
                    .checked_add(served)
                    .ok_or_else(|| {
                        MemberHistoryReplyError::Internal(format!(
                            "member history continuation cursor exhausts the u64 domain at {} + {served}",
                            candidate.page.from_index
                        ))
                    })?,
            );
            candidate.page.complete = false;
        }
        Ok(candidate)
    };

    // Prove the cursor-blocking first row itself is transportable before
    // searching for a larger prefix.
    let first = candidate_with_prefix(1)?;
    let first_bytes = encoded_len(&first)?;
    if first_bytes > MEMBER_HISTORY_REPLY_MAX_BYTES {
        return Err(MemberHistoryReplyError::RowTooLarge {
            index: page.page.from_index,
            encoded_bytes: first_bytes,
        });
    }

    // Maximal prefix in O(log page_rows) serializations; a pop-and-reserialize
    // loop would do quadratic work on large transcript rows.
    let mut low = 1usize;
    let mut high = original_len - 1;
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let candidate = candidate_with_prefix(mid)?;
        if encoded_len(&candidate)? <= MEMBER_HISTORY_REPLY_MAX_BYTES {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    page = candidate_with_prefix(low)?;
    Ok(BridgeReply::MemberHistoryPage(page))
}

/// Serve one `PollMemberEvents` page and send the reply (inline fast path
/// or from the detached long-poll responder — DEC-P6E-3/4).
#[derive(Debug)]
enum MemberEventsReplyError {
    OversizedEvent {
        generation: u64,
        durable_seq: u64,
        encoded_bytes: usize,
    },
    Internal(String),
}

fn member_event_frontier_after(sequence: u64) -> Result<u64, MemberEventsReplyError> {
    sequence.checked_add(1).ok_or_else(|| {
        MemberEventsReplyError::Internal(
            "member event sequence space is exhausted; reply cursor cannot advance past u64::MAX"
                .to_string(),
        )
    })
}

fn bounded_member_events_reply(
    window: MemberEventsWindow,
) -> Result<BridgeReply, MemberEventsReplyError> {
    let MemberEventsWindow {
        runtime_incarnation,
        generation,
        fence_token,
        rows,
        from_seq,
        next_seq,
        watermark,
        turn_outcomes,
        outcomes_complete,
    } = window;
    let mut page = BridgeMemberEventsPage {
        runtime_incarnation,
        generation,
        fence_token,
        events: rows
            .into_iter()
            .map(|(durable_seq, envelope)| WireEventRow {
                durable_seq,
                envelope,
            })
            .collect(),
        from_seq,
        next_seq,
        watermark,
        turn_outcomes,
        outcomes_complete,
    };

    // Defense in depth for alternate MemberObservationHost implementations:
    // a row at MAX has no representable resume cursor. Emitting MAX (or zero)
    // would replay or wrap that row forever, so reject the page before even
    // the within-budget fast path can serialize it.
    if let Some(last) = page.events.last() {
        let expected_next = member_event_frontier_after(last.durable_seq)?;
        if page.next_seq != expected_next {
            return Err(MemberEventsReplyError::Internal(format!(
                "member event page next_seq {} does not exactly follow last durable seq {}",
                page.next_seq, last.durable_seq
            )));
        }
    } else if page.watermark == u64::MAX && page.next_seq == u64::MAX {
        return Err(MemberEventsReplyError::Internal(
            "member event sequence space is exhausted at an empty MAX frontier".to_string(),
        ));
    }

    let encoded_len = |candidate: &BridgeMemberEventsPage| {
        serde_json::to_vec(&BridgeReply::MemberEventsPage(candidate.clone()))
            .map(|bytes| bytes.len())
            .map_err(|error| {
                MemberEventsReplyError::Internal(format!(
                    "member events reply serialization failed: {error}"
                ))
            })
    };
    if encoded_len(&page)? <= MEMBER_EVENTS_REPLY_MAX_BYTES {
        return Ok(BridgeReply::MemberEventsPage(page));
    }

    // The first row is the cursor blocker. If it cannot fit even without the
    // outcome sidecar, return its typed disposition immediately; no empty-page
    // replay loop and no silent cursor skip.
    if !page.events.is_empty() {
        let mut first_only = page.clone();
        first_only.events.truncate(1);
        first_only.next_seq = member_event_frontier_after(first_only.events[0].durable_seq)?;
        first_only.turn_outcomes.clear();
        first_only.outcomes_complete = true;
        let event_only_bytes = encoded_len(&first_only)?;
        if event_only_bytes > MEMBER_EVENTS_REPLY_MAX_BYTES {
            return Err(MemberEventsReplyError::OversizedEvent {
                generation: page.generation,
                durable_seq: page.events[0].durable_seq,
                encoded_bytes: event_only_bytes,
            });
        }
    }

    // Events own the cursor frontier. First find the maximal event prefix
    // that fits without sidecars; unlike the old outcome-first fallback, an
    // outcome can never crowd out the row needed to advance the fold.
    let mut low = 0usize;
    let mut high = page.events.len();
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let mut candidate = page.clone();
        candidate.events.truncate(mid);
        candidate.next_seq = match candidate.events.last() {
            Some(row) => member_event_frontier_after(row.durable_seq)?,
            None => from_seq,
        };
        candidate.turn_outcomes.clear();
        candidate.outcomes_complete = false;
        if encoded_len(&candidate)? <= MEMBER_EVENTS_REPLY_MAX_BYTES {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    page.events.truncate(low);
    page.next_seq = match page.events.last() {
        Some(row) => member_event_frontier_after(row.durable_seq)?,
        None => from_seq,
    };

    // A sidecar is eligible only when its terminal row is strictly below the
    // actual (possibly byte-trimmed) event frontier. Then fit the maximal
    // eligible outcome prefix alongside that fixed event prefix.
    let original_outcome_len = page.turn_outcomes.len();
    page.turn_outcomes
        .retain(|record| record.terminal_seq < page.next_seq);
    let eligible_outcome_len = page.turn_outcomes.len();
    page.outcomes_complete = page.outcomes_complete && eligible_outcome_len == original_outcome_len;
    low = 0;
    high = eligible_outcome_len;
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let mut candidate = page.clone();
        candidate.turn_outcomes.truncate(mid);
        candidate.outcomes_complete = page.outcomes_complete && mid == eligible_outcome_len;
        if encoded_len(&candidate)? <= MEMBER_EVENTS_REPLY_MAX_BYTES {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    if eligible_outcome_len > 0 && low == 0 && page.events.is_empty() {
        return Err(MemberEventsReplyError::Internal(format!(
            "one bounded turn outcome cannot fit the {MEMBER_EVENTS_REPLY_MAX_BYTES} byte bridge budget"
        )));
    }
    page.turn_outcomes.truncate(low);
    if low < eligible_outcome_len {
        page.outcomes_complete = false;
    }
    Ok(BridgeReply::MemberEventsPage(page))
}

struct MemberEventsPageRequest {
    cursor: MemberObservationCursor,
    max: u32,
    wait: Duration,
    outcome_acks: Vec<meerkat_contracts::wire::supervisor_bridge::BridgeTurnOutcomeAck>,
    max_outcomes: u32,
    expected_member: BridgeMemberIncarnation,
}

async fn serve_member_events_page(
    adapter: &MeerkatMachine,
    observation: &Arc<dyn MemberObservationHost>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
    request: MemberEventsPageRequest,
) {
    let MemberEventsPageRequest {
        cursor,
        max,
        wait,
        outcome_acks,
        max_outcomes,
        expected_member,
    } = request;
    if let Err((cause, reason)) = require_registered_member_incarnation(
        adapter,
        session_id,
        &expected_member,
        "poll member events",
    ) {
        send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
        return;
    }
    match observation
        .poll_events(
            session_id,
            MemberEventsPollRequest {
                expected_member: &expected_member,
                cursor,
                max,
                wait,
                outcome_acks: &outcome_acks,
                max_outcomes,
            },
        )
        .await
    {
        Ok(window) => {
            if let Err((cause, reason)) = require_registered_member_incarnation(
                adapter,
                session_id,
                &expected_member,
                "poll member events completion",
            ) {
                send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
                return;
            }
            match bounded_member_events_reply(window) {
                Ok(reply) => {
                    send_bridge_response(
                        comms_runtime,
                        candidate,
                        meerkat_core::interaction::ResponseStatus::Completed,
                        reply,
                        None,
                    )
                    .await;
                }
                Err(MemberEventsReplyError::OversizedEvent {
                    generation,
                    durable_seq,
                    encoded_bytes,
                }) => {
                    let Some(next_seq) = durable_seq.checked_add(1) else {
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            "member event sequence space is exhausted; oversized-row disposition cannot advance the cursor".to_string(),
                            None,
                        )
                        .await;
                        return;
                    };
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::OversizedEvent {
                            generation,
                            durable_seq,
                            next_seq,
                            encoded_bytes: u64::try_from(encoded_bytes).unwrap_or(u64::MAX),
                            max_bytes: MEMBER_EVENTS_REPLY_MAX_BYTES as u64,
                        },
                        format!(
                            "single member event row at durable seq {durable_seq} is {encoded_bytes} bytes (maximum bridge reply budget {MEMBER_EVENTS_REPLY_MAX_BYTES})"
                        ),
                        None,
                    )
                    .await;
                }
                Err(MemberEventsReplyError::Internal(reason)) => {
                    send_bridge_failure(
                        comms_runtime,
                        candidate,
                        BridgeRejectionCause::Internal,
                        reason,
                        None,
                    )
                    .await;
                }
            }
        }
        Err(error) => {
            let (cause, reason) = observation_error_to_bridge_rejection(&error);
            send_bridge_failure(comms_runtime, candidate, cause, reason, None).await;
        }
    }
}

/// Serve one tracked `DeliverMemberInput` (DEC-P6E-16, realizing DEC-P6F-9's
/// contract). A delivery is tracked when it carries `turn` or
/// `outcome_tracking: interaction`; either form also requires an exact
/// `expected_member`:
///
/// 1. Capability gate BEFORE accept: a registered [`TrackedTurnJournal`] AND
///    the observation host are the structural substrate; either absent ⇒
///    typed `TurnDirectiveUnsupported` (supersedes the mid-run
///    `UnsupportedForMode` shape — §18.8:1001).
/// 2. The turn-window bracket (session-events subscription + admission
///    watermark) opens BEFORE `accept_input_with_completion`, so no terminal
///    can slip between accept and watch (the atomicity letter).
/// 3. Turn-bearing work lowers as the flow-step shape
///    (`create_tracked_flow_step_input`); plain work lowers through
///    `peer_input_from_delivery_payload`, preserving objective and injected
///    context. Both preserve `idempotency_key = payload.input_id` EXACTLY, so
///    redelivery deduplicates; `Deduplicated` spawns NO second watcher — the
///    journal's Fresh/Replay transitions converge redelivery on one row.
/// 4. The completion handle is KEPT and handed to the observation host's
///    watcher (classify via the shared core classifier → bind `terminal_seq`
///    → `RecordTurnOutcome` through the journal seam).
/// 5. The delivery REPLY stays at admission and carries no turn payload
///    (§7.4 — completion rides the journal + pump sidecar).
fn directed_rejection_after_pending_cancel(
    input_id: &str,
    cause: BridgeDeliveryRejectionCause,
    reason: String,
    cancellation: Result<(), crate::member_observation::DirectedTurnReject>,
) -> Result<BridgeDeliveryResponse, String> {
    cancellation.map_err(|error| {
        format!(
            "runtime proved no admission, but durable Pending cancellation failed: {}",
            error.detail
        )
    })?;
    Ok(BridgeDeliveryResponse {
        input_id: input_id.to_string(),
        canonical_input_id: None,
        outcome: BridgeDeliveryOutcome::Rejected { cause, reason },
    })
}

fn definite_no_effect_delivery_rejection(
    error: &crate::traits::RuntimeDriverError,
    current_member: Option<BridgeMemberIncarnation>,
    expected_member: &BridgeMemberIncarnation,
) -> Option<BridgeDeliveryRejectionCause> {
    match error {
        crate::traits::RuntimeDriverError::ValidationFailed { reason } => {
            Some(BridgeDeliveryRejectionCause::Internal {
                detail: reason.clone(),
            })
        }
        crate::traits::RuntimeDriverError::StaleAuthority { .. } => {
            Some(BridgeDeliveryRejectionCause::StaleMemberResidency {
                expected: expected_member.clone(),
                current: current_member,
            })
        }
        _ => None,
    }
}

/// Keep the turn-window/Pending reservation as the mandatory gate before the
/// runtime accept call. Besides making the ordering explicit in production,
/// this seam lets boundary tests prove a rejected event-sequence frontier
/// cannot reach runtime admission.
#[cfg(test)]
async fn open_directed_turn_before_runtime_accept<T, F, Fut>(
    observation: &Arc<dyn MemberObservationHost>,
    session_id: &SessionId,
    expected_member: &BridgeMemberIncarnation,
    input_id: &str,
    accept: F,
) -> Result<
    (crate::member_observation::DirectedTurnWindow, T),
    crate::member_observation::DirectedTurnReject,
>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let window = observation
        .open_directed_turn_window(session_id, expected_member, input_id)
        .await?;
    Ok((window, accept().await))
}

enum DirectedTurnPreAcceptDecision {
    SubmitToRuntime,
    TerminalReplay(BridgeDeliveryResponse),
}

/// Let the durable host journal dominate volatile runtime idempotency on an
/// exact terminal replay. Re-submitting after the journal already contains a
/// terminal can execute the effect a second time if the runtime ledger is
/// missing or divergent; it would also have no watcher because terminal
/// replay needs none. The exact reservation is sufficient dedup authority.
fn directed_turn_pre_accept_decision(
    request_input_id: &str,
    expected_member: &BridgeMemberIncarnation,
    window: &DirectedTurnWindow,
) -> Result<DirectedTurnPreAcceptDecision, String> {
    if matches!(
        window.tracking,
        DirectedTurnTracking::PendingFresh | DirectedTurnTracking::PendingReplay
    ) {
        return Ok(DirectedTurnPreAcceptDecision::SubmitToRuntime);
    }
    if window.input_id != request_input_id || &window.expected_member != expected_member {
        return Err(format!(
            "terminal replay reservation does not match request input/residency: request input '{request_input_id}', reserved input '{}', request member {expected_member:?}, reserved member {:?}",
            window.input_id, window.expected_member
        ));
    }
    Ok(DirectedTurnPreAcceptDecision::TerminalReplay(
        BridgeDeliveryResponse {
            input_id: request_input_id.to_string(),
            canonical_input_id: Some(request_input_id.to_string()),
            outcome: BridgeDeliveryOutcome::Deduplicated {
                existing_input_id: request_input_id.to_string(),
            },
        },
    ))
}

async fn serve_tracked_member_delivery(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    comms_runtime: &Arc<dyn CommsRuntime>,
    candidate: &PeerInputCandidate,
    sender_peer_id: PeerId,
    payload: BridgeDeliveryPayload,
) {
    let request_input_id = payload.input_id.clone();
    let reject = |detail: String| BridgeDeliveryResponse {
        input_id: request_input_id.clone(),
        canonical_input_id: None,
        outcome: BridgeDeliveryOutcome::Rejected {
            cause: BridgeDeliveryRejectionCause::TurnDirectiveUnsupported {
                detail: detail.clone(),
            },
            reason: detail,
        },
    };
    if let Err(detail) = validate_delivery_tracking_request(&payload) {
        let response = reject(detail);
        send_bridge_response(
            comms_runtime,
            candidate,
            meerkat_core::interaction::ResponseStatus::Completed,
            BridgeReply::Delivery(response),
            None,
        )
        .await;
        return;
    }
    let Some(expected_member) = payload.expected_member.clone() else {
        let response =
            reject("tracked delivery requires an exact placed-member incarnation".to_string());
        send_bridge_response(
            comms_runtime,
            candidate,
            meerkat_core::interaction::ResponseStatus::Completed,
            BridgeReply::Delivery(response),
            None,
        )
        .await;
        return;
    };
    let input = if let Some(directive) = payload.turn.clone() {
        if !payload.injected_context.is_empty() {
            // The flow-step input shape has no injected-context carrier; a
            // silent drop would launder supervisor-authored context. Fail
            // closed typed until a carrier exists.
            let response = reject(
                "injected_context is not supported on directive-bearing deliveries".to_string(),
            );
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::Delivery(response),
                None,
            )
            .await;
            return;
        }
        if payload.objective_id.is_some() {
            // Directed flow-step inputs do not yet own an objective carrier.
            // The durable objective introduced by V4 must never disappear
            // merely because the same delivery also carries a turn directive.
            let response =
                reject("objective_id is not supported on directive-bearing deliveries".to_string());
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::Delivery(response),
                None,
            )
            .await;
            return;
        }
        let turn_metadata = crate::runtime_loop::for_bridge_turn_directive(
            payload.handling_mode,
            directive.tool_overlay.clone().map(Into::into),
        );
        match crate::mob_adapter::create_tracked_flow_step_input(
            &directive.correlation.step_id,
            payload.content.clone(),
            &directive.correlation.run_id,
            Some(turn_metadata),
            &payload.input_id,
        ) {
            Ok(input) => input,
            Err(detail) => {
                let response = reject(detail);
                send_bridge_response(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Delivery(response),
                    None,
                )
                .await;
                return;
            }
        }
    } else {
        match peer_input_from_delivery_payload(session_id, sender_peer_id, payload) {
            Ok(input) => input,
            Err(reason) => {
                let response = BridgeDeliveryResponse {
                    input_id: request_input_id.clone(),
                    canonical_input_id: None,
                    outcome: BridgeDeliveryOutcome::Rejected {
                        cause: BridgeDeliveryRejectionCause::Internal {
                            detail: reason.clone(),
                        },
                        reason,
                    },
                };
                send_bridge_response(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Delivery(response),
                    None,
                )
                .await;
                return;
            }
        }
    };
    let Some(observation) = adapter.member_observation_host() else {
        let response = reject("member host serves no observation substrate".to_string());
        send_bridge_response(
            comms_runtime,
            candidate,
            meerkat_core::interaction::ResponseStatus::Completed,
            BridgeReply::Delivery(response),
            None,
        )
        .await;
        return;
    };
    // Reserve the exact durable turn window before acquiring the residency
    // lease. The host actor owns that reservation channel; acquiring the slot
    // first would deadlock against a concurrent materialize/release command
    // that already owns the actor and is waiting for the same slot.
    let window = match observation
        .open_directed_turn_window(session_id, &expected_member, &request_input_id)
        .await
    {
        Ok(window) => window,
        Err(rejected) => {
            if !rejected.definite_no_effect {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    rejected.detail,
                    None,
                )
                .await;
                return;
            }
            let response = BridgeDeliveryResponse {
                input_id: request_input_id.clone(),
                canonical_input_id: None,
                outcome: BridgeDeliveryOutcome::Rejected {
                    cause: rejected.cause,
                    reason: rejected.detail,
                },
            };
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::Delivery(response),
                None,
            )
            .await;
            return;
        }
    };
    if window.expected_member != expected_member {
        let cancellation = observation
            .cancel_directed_turn_window(session_id, window)
            .await;
        let reason = format!(
            "directed-turn residency changed before accept; controller expected {expected_member:?}"
        );
        match directed_rejection_after_pending_cancel(
            &request_input_id,
            BridgeDeliveryRejectionCause::StaleMemberResidency {
                expected: expected_member.clone(),
                current: adapter.member_incarnation(session_id),
            },
            reason,
            cancellation,
        ) {
            Ok(response) => {
                send_bridge_response(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Delivery(response),
                    None,
                )
                .await;
            }
            Err(detail) => {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    detail,
                    None,
                )
                .await;
            }
        }
        return;
    }
    match directed_turn_pre_accept_decision(&request_input_id, &expected_member, &window) {
        Ok(DirectedTurnPreAcceptDecision::SubmitToRuntime) => {}
        Ok(DirectedTurnPreAcceptDecision::TerminalReplay(response)) => {
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::Delivery(response),
                None,
            )
            .await;
            return;
        }
        Err(detail) => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                BridgeRejectionCause::Internal,
                detail,
                None,
            )
            .await;
            return;
        }
    }
    let Some(journal) = adapter.tracked_turn_journal(session_id) else {
        let cancellation = observation
            .cancel_directed_turn_window(session_id, window)
            .await;
        let response = match directed_rejection_after_pending_cancel(
            &request_input_id,
            BridgeDeliveryRejectionCause::TurnDirectiveUnsupported {
                detail: "member runtime has no tracked-turn journal registration".to_string(),
            },
            "member runtime has no tracked-turn journal registration".to_string(),
            cancellation,
        ) {
            Ok(response) => response,
            Err(detail) => {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    detail,
                    None,
                )
                .await;
                return;
            }
        };
        send_bridge_response(
            comms_runtime,
            candidate,
            meerkat_core::interaction::ResponseStatus::Completed,
            BridgeReply::Delivery(response),
            None,
        )
        .await;
        return;
    };
    if journal.member_incarnation() != &expected_member {
        let cancellation = observation
            .cancel_directed_turn_window(session_id, window)
            .await;
        let detail = "tracked-turn journal incarnation differs from reserved residency".to_string();
        match directed_rejection_after_pending_cancel(
            &request_input_id,
            BridgeDeliveryRejectionCause::TurnDirectiveUnsupported {
                detail: detail.clone(),
            },
            detail,
            cancellation,
        ) {
            Ok(response) => {
                send_bridge_response(
                    comms_runtime,
                    candidate,
                    meerkat_core::interaction::ResponseStatus::Completed,
                    BridgeReply::Delivery(response),
                    None,
                )
                .await;
            }
            Err(detail) => {
                send_bridge_failure(
                    comms_runtime,
                    candidate,
                    BridgeRejectionCause::Internal,
                    detail,
                    None,
                )
                .await;
            }
        }
        return;
    }
    // Serialize the final runtime-accept interval against the exact-key
    // cancel command, then re-probe durable host authority while that lock is
    // held. The initial Pending reservation alone is not a permit: cancel may
    // have installed a durable tombstone after `open_directed_turn_window`
    // returned but before runtime admission.
    let _admission_permit = match observation
        .lock_and_revalidate_directed_turn_admission(session_id, window.admission_request())
        .await
    {
        Ok(crate::member_observation::DirectedTurnAdmissionDecision::Admit(permit)) => permit,
        Ok(crate::member_observation::DirectedTurnAdmissionDecision::TerminalReplay) => {
            let response = BridgeDeliveryResponse {
                input_id: request_input_id.clone(),
                canonical_input_id: Some(request_input_id.clone()),
                outcome: BridgeDeliveryOutcome::Deduplicated {
                    existing_input_id: request_input_id.clone(),
                },
            };
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::Delivery(response),
                None,
            )
            .await;
            return;
        }
        Err(error) => {
            send_bridge_failure(
                comms_runtime,
                candidate,
                BridgeRejectionCause::Internal,
                format!(
                    "directed-turn final admission revalidation was ambiguous: {}",
                    error.detail
                ),
                None,
            )
            .await;
            return;
        }
    };
    let accept_result = adapter
        .accept_input_with_completion_for_member_residency(
            session_id,
            input,
            Some(&expected_member),
        )
        .await;
    match accept_result {
        Ok((accept_outcome, completion_handle)) => {
            let response = match accept_outcome {
                crate::accept::AcceptOutcome::Accepted { input_id, .. } => {
                    let canonical_input_id = input_id.to_string();
                    if let Err(rejected) = observation
                        .admit_directed_turn(
                            session_id,
                            DirectedTurnAdmission {
                                input_id: request_input_id.clone(),
                                window,
                                completion_handle,
                                journal,
                            },
                        )
                        .await
                    {
                        // The turn IS accepted; refusing here would lie.
                        // Pending remains durable so host recovery can query
                        // the runtime idempotency ledger and reattach.
                        tracing::error!(
                            session_id = %session_id,
                            input_id = %request_input_id,
                            detail = %rejected.detail,
                            "directed turn accepted but watcher admission failed; durable Pending retained for recovery"
                        );
                    }
                    BridgeDeliveryResponse {
                        input_id: request_input_id.clone(),
                        canonical_input_id: Some(canonical_input_id),
                        outcome: BridgeDeliveryOutcome::Accepted,
                    }
                }
                crate::accept::AcceptOutcome::Deduplicated { existing_id, .. } => {
                    let existing_id = existing_id.to_string();
                    if let Err(rejected) = observation
                        .admit_directed_turn(
                            session_id,
                            DirectedTurnAdmission {
                                input_id: request_input_id.clone(),
                                window,
                                completion_handle,
                                journal,
                            },
                        )
                        .await
                    {
                        tracing::error!(
                            session_id = %session_id,
                            input_id = %request_input_id,
                            detail = %rejected.detail,
                            "deduplicated directed turn could not reattach watcher; durable Pending retained for recovery"
                        );
                    }
                    BridgeDeliveryResponse {
                        input_id: request_input_id.clone(),
                        canonical_input_id: Some(existing_id.clone()),
                        outcome: BridgeDeliveryOutcome::Deduplicated {
                            existing_input_id: existing_id,
                        },
                    }
                }
                crate::accept::AcceptOutcome::Rejected { reason } => {
                    let cause = bridge_delivery_rejection_cause(&reason);
                    let cancellation = observation
                        .cancel_directed_turn_window(session_id, window)
                        .await;
                    match directed_rejection_after_pending_cancel(
                        &request_input_id,
                        cause,
                        reason.to_string(),
                        cancellation,
                    ) {
                        Ok(response) => response,
                        Err(detail) => {
                            tracing::error!(
                                session_id = %session_id,
                                input_id = %request_input_id,
                                detail = %detail,
                                "proven-rejected directed turn could not durably certify no-effect"
                            );
                            send_bridge_failure(
                                comms_runtime,
                                candidate,
                                BridgeRejectionCause::Internal,
                                detail,
                                None,
                            )
                            .await;
                            return;
                        }
                    }
                }
            };
            send_bridge_response(
                comms_runtime,
                candidate,
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::Delivery(response),
                None,
            )
            .await;
        }
        Err(error) => {
            // ValidationFailed and StaleAuthority are pre-effect admission
            // failures. Every other error may have happened after durable
            // acceptance, so retain Pending.
            if let Some(cause) = definite_no_effect_delivery_rejection(
                &error,
                adapter.member_incarnation(session_id),
                &expected_member,
            ) {
                let cancellation = observation
                    .cancel_directed_turn_window(session_id, window)
                    .await;
                match directed_rejection_after_pending_cancel(
                    &request_input_id,
                    cause,
                    error.to_string(),
                    cancellation,
                ) {
                    Ok(response) => {
                        send_bridge_response(
                            comms_runtime,
                            candidate,
                            meerkat_core::interaction::ResponseStatus::Completed,
                            BridgeReply::Delivery(response),
                            None,
                        )
                        .await;
                    }
                    Err(detail) => {
                        tracing::error!(
                            session_id = %session_id,
                            input_id = %request_input_id,
                            detail = %detail,
                            "proven validation rejection could not durably certify no-effect"
                        );
                        send_bridge_failure(
                            comms_runtime,
                            candidate,
                            BridgeRejectionCause::Internal,
                            detail,
                            None,
                        )
                        .await;
                    }
                }
                return;
            }
            send_bridge_failure(
                comms_runtime,
                candidate,
                BridgeRejectionCause::Internal,
                format!("deliver member input failed: {error}"),
                None,
            )
            .await;
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

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_contracts::wire::supervisor_bridge::{
        supervisor_bridge_current_protocol_version, supervisor_bridge_default_protocol_version,
        supervisor_bridge_supported_protocol_versions,
    };
    use meerkat_contracts::{
        BridgeMobPeerOverlayHandoff, BridgeOutboundTaintPayload, BridgePeerWiringPayload,
    };
    use meerkat_core::InteractionId;
    use meerkat_core::SendError;
    use meerkat_core::interaction::InboxInteraction;
    use meerkat_core::interaction::{PeerIngressConvention, PeerIngressIdentity};
    use meerkat_core::types::HandlingMode;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};
    use uuid::Uuid;

    struct ExhaustedDirectedTurnObservation;

    #[async_trait::async_trait]
    impl MemberObservationHost for ExhaustedDirectedTurnObservation {
        async fn member_generation(
            &self,
            _session: &SessionId,
        ) -> Result<u64, MemberObservationError> {
            Err(MemberObservationError::Internal {
                reason: "unused test seam".to_string(),
            })
        }

        async fn read_history(
            &self,
            _session: &SessionId,
            _from_index: Option<u64>,
            _limit: Option<u32>,
        ) -> Result<MemberHistoryWindow, MemberObservationError> {
            Err(MemberObservationError::Internal {
                reason: "unused test seam".to_string(),
            })
        }

        async fn poll_events(
            &self,
            _session: &SessionId,
            _request: MemberEventsPollRequest<'_>,
        ) -> Result<MemberEventsWindow, MemberObservationError> {
            Err(MemberObservationError::Internal {
                reason: "unused test seam".to_string(),
            })
        }

        async fn open_directed_turn_window(
            &self,
            _session: &SessionId,
            _expected_member: &BridgeMemberIncarnation,
            _input_id: &str,
        ) -> Result<
            crate::member_observation::DirectedTurnWindow,
            crate::member_observation::DirectedTurnReject,
        > {
            Err(crate::member_observation::DirectedTurnReject::unsupported(
                "member event sequence space is exhausted before Pending reservation",
            ))
        }

        async fn cancel_directed_turn_window(
            &self,
            _session: &SessionId,
            _window: crate::member_observation::DirectedTurnWindow,
        ) -> Result<(), crate::member_observation::DirectedTurnReject> {
            Err(crate::member_observation::DirectedTurnReject::unsupported(
                "unused test seam",
            ))
        }

        async fn admit_directed_turn(
            &self,
            _session: &SessionId,
            _admission: DirectedTurnAdmission,
        ) -> Result<(), crate::member_observation::DirectedTurnReject> {
            Err(crate::member_observation::DirectedTurnReject::unsupported(
                "unused test seam",
            ))
        }
    }

    #[tokio::test]
    async fn rejected_exhausted_window_never_invokes_runtime_accept() {
        let observation: Arc<dyn MemberObservationHost> =
            Arc::new(ExhaustedDirectedTurnObservation);
        let session_id = SessionId::new();
        let accept_calls = std::sync::atomic::AtomicUsize::new(0);
        let result = open_directed_turn_before_runtime_accept(
            &observation,
            &session_id,
            &BridgeMemberIncarnation::default(),
            "input-exhausted",
            || async {
                accept_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            accept_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "runtime acceptance must remain strictly downstream of window reservation"
        );
    }

    #[test]
    fn exact_terminal_replay_never_invokes_runtime_accept() {
        let expected_member = BridgeMemberIncarnation {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            host_id: "host".to_string(),
            binding_generation: 3,
            member_session_id: SessionId::new().to_string(),
            generation: 7,
            fence_token: 11,
        };
        let input_id = Uuid::new_v4().to_string();
        let window = DirectedTurnWindow {
            expected_member: expected_member.clone(),
            subscription: Box::pin(futures::stream::empty()),
            window_start: 42,
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            input_id: input_id.clone(),
            tracking: DirectedTurnTracking::TerminalReplay,
        };
        let accept_calls = std::sync::atomic::AtomicUsize::new(0);
        let response = match directed_turn_pre_accept_decision(&input_id, &expected_member, &window)
            .expect("exact terminal replay is valid")
        {
            DirectedTurnPreAcceptDecision::SubmitToRuntime => {
                accept_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                panic!("durable terminal replay must dominate runtime admission")
            }
            DirectedTurnPreAcceptDecision::TerminalReplay(response) => response,
        };

        assert_eq!(
            accept_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "terminal replay must never reach the runtime accept closure"
        );
        assert_eq!(response.input_id, input_id);
        assert_eq!(
            response.canonical_input_id.as_deref(),
            Some(input_id.as_str())
        );
        assert!(matches!(
            response.outcome,
            BridgeDeliveryOutcome::Deduplicated {
                existing_input_id
            } if existing_input_id == input_id
        ));
    }

    #[test]
    fn terminal_replay_with_different_request_identity_fails_closed() {
        let expected_member = BridgeMemberIncarnation {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            host_id: "host".to_string(),
            binding_generation: 3,
            member_session_id: SessionId::new().to_string(),
            generation: 7,
            fence_token: 11,
        };
        let window = DirectedTurnWindow {
            expected_member: expected_member.clone(),
            subscription: Box::pin(futures::stream::empty()),
            window_start: 42,
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            input_id: Uuid::new_v4().to_string(),
            tracking: DirectedTurnTracking::TerminalReplay,
        };
        assert!(
            directed_turn_pre_accept_decision(
                &Uuid::new_v4().to_string(),
                &expected_member,
                &window,
            )
            .is_err(),
            "a terminal row for another input must never suppress or submit this request"
        );
    }

    #[test]
    fn directed_no_effect_rejection_requires_durable_pending_cancel() {
        let cause = BridgeDeliveryRejectionCause::Internal {
            detail: "validation failed".to_string(),
        };
        let response = directed_rejection_after_pending_cancel(
            "input-1",
            cause.clone(),
            "Input validation failed: invalid".to_string(),
            Ok(()),
        )
        .expect("durably canceled Pending certifies definite no-effect");
        assert!(matches!(
            response.outcome,
            BridgeDeliveryOutcome::Rejected {
                cause: actual,
                ..
            } if actual == cause
        ));

        let ambiguous = directed_rejection_after_pending_cancel(
            "input-1",
            cause,
            "Input validation failed: invalid".to_string(),
            Err(crate::member_observation::DirectedTurnReject::unsupported(
                "host actor cancellation unavailable",
            )),
        )
        .expect_err("cancel failure must not mint a definite Delivery Rejected");
        assert!(ambiguous.contains("durable Pending cancellation failed"));
        assert!(ambiguous.contains("host actor cancellation unavailable"));
    }

    #[test]
    fn stale_fenced_accept_maps_to_typed_stale_member_rejection() {
        let expected = BridgeMemberIncarnation {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            host_id: "host".to_string(),
            binding_generation: 1,
            member_session_id: "session-g1".to_string(),
            generation: 1,
            fence_token: 11,
        };
        let current = BridgeMemberIncarnation {
            binding_generation: 2,
            member_session_id: "session-g2".to_string(),
            generation: 2,
            fence_token: 12,
            ..expected.clone()
        };
        let cause = definite_no_effect_delivery_rejection(
            &crate::traits::RuntimeDriverError::StaleAuthority {
                reason: "G1 lost the residency cutover".to_string(),
            },
            Some(current.clone()),
            &expected,
        )
        .expect("stale authority is a definite no-effect rejection");
        assert_eq!(
            cause,
            BridgeDeliveryRejectionCause::StaleMemberResidency {
                expected,
                current: Some(current),
            }
        );
    }

    #[test]
    fn stale_fenced_accept_without_replacement_never_fabricates_current_member() {
        let expected = BridgeMemberIncarnation {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            host_id: "host".to_string(),
            binding_generation: 1,
            member_session_id: "session-g1".to_string(),
            generation: 1,
            fence_token: 11,
        };
        let cause = definite_no_effect_delivery_rejection(
            &crate::traits::RuntimeDriverError::StaleAuthority {
                reason: "G1 lost to vacate".to_string(),
            },
            None,
            &expected,
        )
        .expect("vacated stale authority remains a typed no-effect rejection");
        assert_eq!(
            cause,
            BridgeDeliveryRejectionCause::StaleMemberResidency {
                expected,
                current: None,
            }
        );
    }

    #[test]
    fn member_events_reply_trims_outcomes_below_transport_budget() {
        let outcomes = (0..16)
            .map(|index| {
                meerkat_contracts::wire::supervisor_bridge::BridgeTurnOutcomeRecord {
                    input_id: format!("input-{index}"),
                    generation: 1,
                    fence_token: 1,
                    terminal_seq: index,
                    outcome: meerkat_contracts::wire::supervisor_bridge::WireFlowTurnOutcome::RunFailed {
                        detail: meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail::complete(
                            "x".repeat(60_000),
                        ),
                    },
                }
            })
            .collect();
        let reply = bounded_member_events_reply(MemberEventsWindow {
            runtime_incarnation: BridgeHostRuntimeIncarnation::new(),
            generation: 1,
            fence_token: 1,
            rows: Vec::new(),
            from_seq: 7,
            next_seq: 7,
            watermark: 6,
            turn_outcomes: outcomes,
            outcomes_complete: true,
        })
        .expect("bounded outcome page");
        let encoded = serde_json::to_vec(&reply).expect("serialize bounded reply");
        assert!(encoded.len() <= MEMBER_EVENTS_REPLY_MAX_BYTES);
        let BridgeReply::MemberEventsPage(page) = reply else {
            panic!("expected member events page")
        };
        assert_eq!(
            page.from_seq, 7,
            "the wire page preserves the host-resolved read floor even when no event row fits"
        );
        assert_eq!(page.next_seq, page.from_seq);
        assert!(page.turn_outcomes.len() < 16);
        assert!(!page.outcomes_complete);
    }

    #[test]
    fn member_events_reply_rejects_one_oversized_event_without_cursor_skip() {
        let session_id =
            SessionId::parse("0195e9a0-0000-7000-8000-000000000001").expect("session id");
        let envelope = meerkat_core::event::EventEnvelope::new(
            "worker",
            1,
            Some("mob-size".to_string()),
            AgentEvent::ExtractionFailed {
                session_id,
                last_output: "x".repeat(MEMBER_EVENTS_REPLY_MAX_BYTES + 1024),
                attempts: 1,
                reason: "oversized".to_string(),
            },
        );
        let error = bounded_member_events_reply(MemberEventsWindow {
            runtime_incarnation: BridgeHostRuntimeIncarnation::new(),
            generation: 1,
            fence_token: 1,
            rows: vec![(7, envelope)],
            from_seq: 7,
            next_seq: 8,
            watermark: 7,
            turn_outcomes: Vec::new(),
            outcomes_complete: true,
        })
        .expect_err("single oversized event must reject instead of replay-looping empty pages");
        assert!(matches!(
            error,
            MemberEventsReplyError::OversizedEvent {
                generation: 1,
                durable_seq: 7,
                ..
            }
        ));
    }

    #[test]
    fn member_events_reply_rejects_exhausted_row_without_max_replay_or_zero_wrap() {
        let envelope = meerkat_core::event::EventEnvelope::new(
            "worker",
            1,
            Some("mob-exhausted".to_string()),
            AgentEvent::TurnStarted { turn_number: 1 },
        );
        let error = bounded_member_events_reply(MemberEventsWindow {
            runtime_incarnation: BridgeHostRuntimeIncarnation::new(),
            generation: 1,
            fence_token: 1,
            rows: vec![(u64::MAX, envelope)],
            from_seq: u64::MAX,
            next_seq: u64::MAX,
            watermark: u64::MAX,
            turn_outcomes: Vec::new(),
            outcomes_complete: true,
        })
        .expect_err("a MAX row has no representable successor cursor");
        assert!(matches!(
            error,
            MemberEventsReplyError::Internal(reason)
                if reason.contains("sequence space is exhausted")
        ));
    }

    #[test]
    fn member_events_reply_rejects_empty_exhausted_frontier_instead_of_spinning() {
        let error = bounded_member_events_reply(MemberEventsWindow {
            runtime_incarnation: BridgeHostRuntimeIncarnation::new(),
            generation: 1,
            fence_token: 1,
            rows: Vec::new(),
            from_seq: u64::MAX,
            next_seq: u64::MAX,
            watermark: u64::MAX,
            turn_outcomes: Vec::new(),
            outcomes_complete: true,
        })
        .expect_err("an empty MAX frontier cannot make progress");
        assert!(matches!(
            error,
            MemberEventsReplyError::Internal(reason)
                if reason.contains("empty MAX frontier")
        ));
    }

    fn member_history_window(
        offset: usize,
        message_count: usize,
        rows: impl IntoIterator<Item = String>,
    ) -> MemberHistoryWindow {
        let messages: Vec<_> = rows
            .into_iter()
            .map(|text| {
                meerkat_core::types::Message::User(meerkat_core::types::UserMessage::text(text))
            })
            .collect();
        MemberHistoryWindow {
            generation: 4,
            page: meerkat_core::service::SessionHistoryPage {
                session_id: SessionId::parse("0195e9a0-0000-7000-8000-000000000001")
                    .expect("session id"),
                message_count,
                offset,
                limit: Some(messages.len()),
                has_more: offset + messages.len() < message_count,
                messages,
            },
        }
    }

    #[test]
    fn member_history_reply_returns_maximal_transport_safe_prefix() {
        let reply = bounded_member_history_reply(member_history_window(
            7,
            10,
            [
                "a".repeat(200_000),
                "b".repeat(200_000),
                "c".repeat(200_000),
            ],
        ))
        .expect("bounded history page");
        let encoded = serde_json::to_vec(&reply).expect("serialize bounded history reply");
        assert!(encoded.len() <= MEMBER_HISTORY_REPLY_MAX_BYTES);
        let BridgeReply::MemberHistoryPage(page) = reply else {
            panic!("expected member history page")
        };
        assert_eq!(page.page.from_index, 7);
        assert_eq!(page.page.messages.len(), 2);
        assert_eq!(page.page.next_index, Some(9));
        assert!(!page.page.complete);
        assert_eq!(page.page.message_count, 10);
    }

    #[test]
    fn member_history_reply_rejects_single_poison_row_at_exact_index() {
        let error = bounded_member_history_reply(member_history_window(
            9,
            10,
            ["x".repeat(MEMBER_HISTORY_REPLY_MAX_BYTES + 64 * 1024)],
        ))
        .expect_err("one oversized history row must fail typed");
        assert!(matches!(
            error,
            MemberHistoryReplyError::RowTooLarge { index: 9, .. }
        ));
    }

    #[test]
    fn member_history_reply_rejects_non_advancing_domain_page() {
        let error = bounded_member_history_reply(MemberHistoryWindow {
            generation: 4,
            page: meerkat_core::service::SessionHistoryPage {
                session_id: SessionId::parse("0195e9a0-0000-7000-8000-000000000001")
                    .expect("session id"),
                message_count: 1,
                offset: 0,
                limit: Some(1),
                has_more: true,
                messages: Vec::new(),
            },
        })
        .expect_err("a non-advancing page must fail before it reaches the bridge");
        assert!(matches!(
            error,
            MemberHistoryReplyError::Internal(reason)
                if reason.contains("page projection failed") && reason.contains("serves none")
        ));
    }

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
            runtime_dyn.as_ref(),
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
        abandonments:
            std::sync::Mutex<Vec<(InteractionId, meerkat_core::InteractionStreamAbandonReason)>>,
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
                abandonments: std::sync::Mutex::new(Vec::new()),
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
                abandonments: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn completed_count(&self) -> usize {
            self.completed_count
                .load(std::sync::atomic::Ordering::SeqCst)
        }

        fn abandonment_reasons(&self) -> Vec<meerkat_core::InteractionStreamAbandonReason> {
            self.abandonments
                .lock()
                .expect("abandonments mutex")
                .iter()
                .map(|(_, reason)| *reason)
                .collect()
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

        fn abandon_interaction_stream(
            &self,
            id: &InteractionId,
            reason: meerkat_core::InteractionStreamAbandonReason,
        ) {
            self.abandonments
                .lock()
                .expect("abandonments mutex")
                .push((*id, reason));
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
                objective_id: None,
                sender_taint: None,
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
                objective_id: None,
                sender_taint: None,
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

    async fn spawn_authorized_test_comms_drain(
        adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
        runtime: Arc<dyn CommsRuntime>,
        idle_timeout: Duration,
    ) -> crate::tokio::task::JoinHandle<()> {
        // Production launch publishes generated peer-ingress ownership and
        // installs the task runtime before the spawned drain can acquire the
        // session gate. Direct unit tests must establish the same fence; a
        // finished placeholder handle supplies only the slot identity while
        // the returned handle remains available to the assertion.
        adapter
            .test_authorize_direct_comms_drain_runtime(&session_id, Arc::clone(&runtime))
            .await
            .expect("direct-drain test authority should accept");
        spawn_comms_drain(adapter, session_id, runtime, Some(idle_timeout))
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
        let drain = spawn_authorized_test_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Duration::from_secs(60),
        )
        .await;

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
        let drain = spawn_authorized_test_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Duration::from_secs(60),
        )
        .await;

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
        let drain = spawn_authorized_test_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Duration::from_millis(10),
        )
        .await;

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
        let drain = spawn_authorized_test_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Duration::from_millis(10),
        )
        .await;

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
            let drain = spawn_authorized_test_comms_drain(
                adapter.clone(),
                session_id.clone(),
                runtime.clone(),
                Duration::from_millis(10),
            )
            .await;

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
            let drain = spawn_authorized_test_comms_drain(
                adapter.clone(),
                session_id.clone(),
                runtime.clone(),
                Duration::from_millis(10),
            )
            .await;

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
            assert_eq!(runtime.completed_count(), 0);
            assert_eq!(
                runtime.abandonment_reasons(),
                vec![meerkat_core::InteractionStreamAbandonReason::AdmissionRejected],
                "rejected {class:?} must use the typed abandonment path"
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
            let drain = spawn_authorized_test_comms_drain(
                adapter.clone(),
                session_id.clone(),
                runtime.clone(),
                Duration::from_millis(10),
            )
            .await;

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
            assert_eq!(runtime.completed_count(), 0);
            assert_eq!(
                runtime.abandonment_reasons(),
                vec![meerkat_core::InteractionStreamAbandonReason::AdmissionRejected],
                "rejected {class:?} must use the typed abandonment path"
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
            let drain = spawn_authorized_test_comms_drain(
                adapter.clone(),
                session_id.clone(),
                runtime.clone(),
                Duration::from_millis(10),
            )
            .await;

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
            assert_eq!(runtime.completed_count(), 0);
            assert_eq!(
                runtime.abandonment_reasons(),
                vec![meerkat_core::InteractionStreamAbandonReason::AdmissionRejected],
                "rejected {class:?} must use the typed abandonment path"
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
                objective_id: None,
                sender_taint: None,
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
                content_taint: None,
                to: PeerRoute::with_display_name(member_spec.peer_id, member_spec.name.clone()),
                intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::to_value(&command).expect("serialize bridge command"),
                blocks: None,
                handling_mode: HandlingMode::Queue,
                stream: meerkat_core::comms::InputStreamMode::None,
                objective_id: None,
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
                content_taint: None,
                to: PeerRoute::with_display_name(member_spec.peer_id, member_spec.name.clone()),
                intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::to_value(&command).expect("serialize bridge command"),
                blocks: None,
                handling_mode: HandlingMode::Queue,
                stream: meerkat_core::comms::InputStreamMode::None,
                objective_id: None,
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
    async fn supervisor_rotation_v4_is_one_way_then_observed_from_new_authority() {
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

        let new_supervisor_keypair = meerkat_comms::Keypair::generate();
        let mut new_supervisor_runtime =
            meerkat_comms::CommsRuntime::inproc_only_with_keypair_and_silent_intents(
                &new_supervisor_name,
                None,
                new_supervisor_keypair.clone(),
                Arc::new(HashSet::new()),
            )
            .expect("new supervisor runtime");
        new_supervisor_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure new supervisor TCP listener");
        new_supervisor_runtime
            .start_listeners()
            .await
            .expect("start new supervisor TCP listener");
        let new_supervisor_runtime = Arc::new(new_supervisor_runtime);
        install_test_peer_request_response_authority(
            &new_supervisor_runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );

        let member_spec = trusted_tcp_peer_from_runtime(&member_name, &member_runtime);
        let old_supervisor_dsl =
            install_running_test_peer_comms_handle(old_supervisor_runtime.as_ref());
        add_test_projection_trust_with_dsl(
            old_supervisor_runtime.as_ref(),
            Arc::clone(&old_supervisor_dsl),
            member_spec.clone(),
            1,
            "old supervisor trusts member target",
        )
        .await;
        let new_supervisor_dsl =
            install_running_test_peer_comms_handle(new_supervisor_runtime.as_ref());
        add_test_projection_trust_with_dsl(
            new_supervisor_runtime.as_ref(),
            Arc::clone(&new_supervisor_dsl),
            member_spec,
            1,
            "new supervisor trusts member target",
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
        adapter
            .test_authorize_direct_comms_drain_runtime(
                &session_id,
                member_runtime.clone() as Arc<dyn CommsRuntime>,
            )
            .await
            .expect("install generated current member comms runtime fence");
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

        let operation_id = SupervisorRotationOperationId::new();
        let delivery = serde_json::to_value(
            meerkat_contracts::wire::supervisor_bridge::BridgeSupervisorDelivery::SubmitSupervisorRotation(
                meerkat_contracts::wire::supervisor_bridge::BridgeSupervisorRotationSubmit {
                    operation_id,
                    target: BridgePeerSpec::from(new_supervisor_spec.clone()),
                    target_epoch: 2,
                    protocol_version: BridgeProtocolVersion::V4,
                },
            ),
        )
        .expect("serialize supervisor rotation delivery");
        let interaction_id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableMessage,
            meerkat_core::PeerIngressKind::Message,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                old_supervisor_spec.peer_id,
                old_supervisor_spec.name.as_str(),
                meerkat_core::PeerIngressConvention::Message,
            )
            .with_signing_pubkey(old_supervisor_spec.pubkey),
        );
        let candidate = bridge_delivery_candidate_with_ingress(
            old_supervisor_spec.name.as_str(),
            &delivery,
            ingress,
        );

        assert!(
            try_handle_supervisor_bridge_delivery(
                &adapter,
                &session_id,
                &(member_runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
            )
            .await,
            "delivery handler must own SubmitSupervisorRotation"
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let receipt = adapter
                    .active_supervisor_rotation(&session_id)
                    .await
                    .expect("read rotation receipt")
                    .expect("rotation receipt exists");
                if receipt.phase == crate::meerkat_machine::dsl::SupervisorRotationPhase::Completed
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("rotation worker reaches terminal completion");

        let observe = BridgeCommand::ObserveSupervisorRotation(
            meerkat_contracts::wire::supervisor_bridge::BridgeSupervisorRotationObserve {
                operation_id,
                observer: BridgePeerSpec::from(new_supervisor_spec.clone()),
                observer_epoch: 2,
                protocol_version: BridgeProtocolVersion::V4,
            },
        );
        // The durable next-supervisor binding points at `new_supervisor_runtime`
        // (P1). Exercise the production non-fixed authority-probe shape with a
        // second listener under the SAME signing identity at P2. The signed
        // request-level reply endpoint must make the exact Response choose P2
        // even though member trust still resolves this peer id to P1.
        let probe_name = format!("{new_supervisor_name}/pending-supervisor-probe");
        let mut probe_runtime =
            meerkat_comms::CommsRuntime::inproc_only_with_keypair_and_silent_intents(
                &probe_name,
                None,
                new_supervisor_keypair,
                Arc::new(HashSet::new()),
            )
            .expect("alternate-authority probe runtime");
        probe_runtime
            .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("configure alternate-authority probe TCP listener");
        probe_runtime
            .start_listeners()
            .await
            .expect("start alternate-authority probe TCP listener");
        let probe_runtime = Arc::new(probe_runtime);
        install_test_peer_request_response_authority(
            &probe_runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );
        let probe_dsl = install_running_test_peer_comms_handle(probe_runtime.as_ref());
        let member_spec = trusted_tcp_peer_from_runtime(&member_name, &member_runtime);
        add_test_projection_trust_with_dsl(
            probe_runtime.as_ref(),
            Arc::clone(&probe_dsl),
            member_spec.clone(),
            1,
            "alternate-authority probe trusts member target",
        )
        .await;
        let reply_endpoint = PeerAddress::parse(probe_runtime.advertised_address())
            .expect("probe advertises a typed TCP reply endpoint");
        assert_ne!(
            reply_endpoint.to_string(),
            new_supervisor_spec.address.to_string(),
            "regression requires the durable authority and probe listeners to differ"
        );
        let receipt = probe_runtime
            .send_peer_request_at_endpoint(
                PeerRoute::with_display_name(member_spec.peer_id, member_spec.name.clone()),
                SUPERVISOR_BRIDGE_INTENT,
                serde_json::to_value(&observe).expect("serialize observe command"),
                reply_endpoint.clone(),
            )
            .await
            .expect("send signed observation from alternate-authority probe");
        let SendReceipt::PeerRequestSent { envelope_id, .. } = receipt else {
            panic!("expected peer request receipt, got {receipt:?}");
        };
        let observe_id = InteractionId(envelope_id);
        let observe_candidate =
            drain_one_candidate(&member_runtime, "alternate-authority observation request").await;
        assert_eq!(observe_candidate.interaction.id, observe_id);
        assert_eq!(
            observe_candidate.ingress.declared_reply_endpoint,
            Some(reply_endpoint),
            "the authenticated request reply endpoint must survive ingress classification"
        );
        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(member_runtime.clone() as Arc<dyn CommsRuntime>),
                &observe_candidate,
            )
            .await,
            "bridge handler must own ObserveSupervisorRotation"
        );

        let response = drain_one_candidate(
            &probe_runtime,
            "alternate-authority supervisor rotation observation",
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
        assert_eq!(in_reply_to, observe_id);
        assert_eq!(status, meerkat_core::interaction::ResponseStatus::Completed);
        assert!(matches!(
            serde_json::from_value::<BridgeReply>(result).expect("typed bridge reply"),
            BridgeReply::SupervisorRotation(
                BridgeSupervisorRotationObservation::Found {
                    state: BridgeSupervisorRotationState::Completed { receipt }
                }
            ) if receipt.operation_id == operation_id
                && receipt.target.target.peer_id == new_supervisor_spec.peer_id.as_str()
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
        add_mutates_then_hangs: Arc<std::sync::atomic::AtomicBool>,
        add_mutates_then_hangs_once_for: Arc<tokio::sync::Mutex<HashSet<String>>>,
        remove_hangs_remaining: Arc<std::sync::atomic::AtomicUsize>,
        remove_mutates_then_hangs_once_for: Arc<tokio::sync::Mutex<HashSet<String>>>,
        remove_failures_remaining: Arc<std::sync::atomic::AtomicUsize>,
        remove_attempts: Arc<std::sync::atomic::AtomicUsize>,
        staged_reply_endpoints: Arc<std::sync::Mutex<Vec<(PeerId, [u8; 32], String)>>>,
        staged_correlated_reply_endpoints:
            Arc<std::sync::Mutex<Vec<(PeerId, InteractionId, [u8; 32], PeerAddress)>>>,
        unstaged_correlated_reply_endpoints: Arc<std::sync::Mutex<Vec<(PeerId, InteractionId)>>>,
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

        async fn stage_declared_reply_endpoint(
            &self,
            dest: PeerId,
            signer_pubkey: [u8; 32],
            declared_address: String,
        ) -> Result<(), SendError> {
            self.staged_reply_endpoints
                .lock()
                .expect("staged reply endpoints mutex")
                .push((dest, signer_pubkey, declared_address));
            Ok(())
        }

        async fn stage_correlated_reply_endpoint(
            &self,
            dest: PeerId,
            in_reply_to: InteractionId,
            signer_pubkey: [u8; 32],
            declared_endpoint: PeerAddress,
        ) -> Result<(), SendError> {
            self.staged_correlated_reply_endpoints
                .lock()
                .expect("staged correlated reply endpoints mutex")
                .push((dest, in_reply_to, signer_pubkey, declared_endpoint));
            Ok(())
        }

        async fn unstage_correlated_reply_endpoint(
            &self,
            dest: PeerId,
            in_reply_to: InteractionId,
        ) -> Result<(), SendError> {
            self.unstaged_correlated_reply_endpoints
                .lock()
                .expect("unstaged correlated reply endpoints mutex")
                .push((dest, in_reply_to));
            Ok(())
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
                    let mut mutate_then_hang = self.add_mutates_then_hangs_once_for.lock().await;
                    let targeted_mutate_then_hang = mutate_then_hang.remove(&peer.peer_id.as_str());
                    drop(mutate_then_hang);
                    if self
                        .add_mutates_then_hangs
                        .load(std::sync::atomic::Ordering::SeqCst)
                        || targeted_mutate_then_hang
                    {
                        std::future::pending::<()>().await;
                    }
                    Ok(CommsTrustMutationResult::Added { created })
                }
                CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority } => {
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|error| SendError::Validation(error.to_string()))?;
                    authority
                        .validate_private_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    self.remove_attempts
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let mut mutate_then_hang = self.remove_mutates_then_hangs_once_for.lock().await;
                    let targeted_mutate_then_hang = mutate_then_hang.remove(&peer_id);
                    drop(mutate_then_hang);
                    if targeted_mutate_then_hang {
                        self.trusted_peer_ids.lock().await.remove(&peer_id);
                        std::future::pending::<()>().await;
                    }
                    if self
                        .remove_hangs_remaining
                        .fetch_update(
                            std::sync::atomic::Ordering::SeqCst,
                            std::sync::atomic::Ordering::SeqCst,
                            |remaining| remaining.checked_sub(1),
                        )
                        .is_ok()
                    {
                        std::future::pending::<()>().await;
                    }
                    if self
                        .remove_failures_remaining
                        .fetch_update(
                            std::sync::atomic::Ordering::SeqCst,
                            std::sync::atomic::Ordering::SeqCst,
                            |remaining| remaining.checked_sub(1),
                        )
                        .is_ok()
                    {
                        return Err(SendError::Internal(
                            "synthetic one-shot remove failure".to_string(),
                        ));
                    }
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
            add_mutates_then_hangs: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            add_mutates_then_hangs_once_for: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            remove_hangs_remaining: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            remove_mutates_then_hangs_once_for: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            remove_failures_remaining: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            remove_attempts: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            staged_reply_endpoints: Arc::new(std::sync::Mutex::new(Vec::new())),
            staged_correlated_reply_endpoints: Arc::new(std::sync::Mutex::new(Vec::new())),
            unstaged_correlated_reply_endpoints: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    #[tokio::test]
    async fn supervisor_rotation_task_slot_rejects_live_replacement_carrier() {
        struct DropSignal(Arc<std::sync::atomic::AtomicBool>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let slot = crate::meerkat_machine::SupervisorRotationTaskSlot::new();
        let runtime_a: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://rotation-worker-a",
            Some("token"),
        ));
        let runtime_b: Arc<dyn CommsRuntime> = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://rotation-worker-b",
            Some("token"),
        ));
        let first_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first = tokio::spawn({
            let first_dropped = Arc::clone(&first_dropped);
            let first_started = Arc::clone(&first_started);
            async move {
                let _drop_signal = DropSignal(first_dropped);
                first_started.notify_one();
                std::future::pending::<()>().await;
            }
        });
        first_started.notified().await;
        assert!(
            slot.install("operation-a".to_string(), runtime_a, first)
                .await
        );

        let replacement_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let replacement = tokio::spawn({
            let drop_signal = DropSignal(Arc::clone(&replacement_dropped));
            async move {
                let _drop_signal = drop_signal;
                std::future::pending::<()>().await;
            }
        });
        assert!(
            !slot
                .install("operation-b".to_string(), runtime_b, replacement)
                .await,
            "a live carrier must never be displaced by last-writer-wins installation"
        );
        assert!(
            !first_dropped.load(std::sync::atomic::Ordering::SeqCst),
            "the incumbent carrier remains authoritative until its owning runtime is quiesced"
        );
        assert!(
            replacement_dropped.load(std::sync::atomic::Ordering::SeqCst),
            "the rejected candidate carrier must be aborted and joined"
        );
        slot.abort_and_wait().await;
        assert!(
            first_dropped.load(std::sync::atomic::Ordering::SeqCst),
            "explicit quiescence must abort and join the incumbent carrier"
        );
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
                objective_id: None,
                sender_taint: None,
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
    fn extraction_failure_maps_to_typed_interaction_failure() {
        let interaction_id = InteractionId(Uuid::new_v4());
        let event = interaction_terminal_event(
            interaction_id,
            CompletionOutcome::Completed(Box::new(meerkat_core::RunResult {
                text: "main output".to_string(),
                session_id: meerkat_core::SessionId::new(),
                usage: meerkat_core::Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: Some(meerkat_core::types::ExtractionError {
                    last_output: "main output".to_string(),
                    attempts: 3,
                    reason: "invalid schema".to_string(),
                }),
                schema_warnings: None,
                skill_diagnostics: None,
            })),
        );

        assert!(matches!(
            event,
            AgentEvent::InteractionFailed {
                interaction_id: actual_id,
                reason: meerkat_core::event::InteractionFailureReason::ExtractionFailed {
                    ref last_output,
                    attempts: 3,
                    ref reason,
                },
            } if actual_id == interaction_id
                && last_output == "main output"
                && reason == "invalid schema"
        ));
    }

    #[test]
    fn peer_turn_metadata_matches_live_interaction_complete_identity() {
        let interaction_id = InteractionId(Uuid::new_v4());
        let input = crate::input::Input::Peer(crate::input::PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            injected_context: Vec::new(),
            header: crate::input::InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: crate::input::InputOrigin::Peer {
                    peer_id: PEER_ID_RECEIVER.to_string(),
                    display_identity: Some("keepalive-peer".to_string()),
                    runtime_id: None,
                },
                durability: crate::input::InputDurability::Durable,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: Some(crate::CorrelationId::from_uuid(interaction_id.0)),
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: "poke".to_string().into(),
            payload: None,
            handling_mode: None,
            sender_taint: None,
        });
        let semantics =
            crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(&input, true)
                .expect("peer message should admit as a runtime turn");
        let metadata = crate::runtime_loop::for_input(&input, semantics);

        let event =
            interaction_terminal_event(interaction_id, CompletionOutcome::CompletedWithoutResult);
        let AgentEvent::InteractionComplete {
            interaction_id: live_interaction_id,
            ..
        } = event
        else {
            panic!("expected InteractionComplete");
        };

        assert_eq!(
            metadata.transcript_identity.interaction_id,
            Some(live_interaction_id),
            "runtime transcript identity must use the same peer interaction id as the live terminal event"
        );
    }

    #[test]
    fn callback_pending_maps_to_interaction_callback_pending_terminal_event() {
        let interaction_id = InteractionId(Uuid::new_v4());
        let event = interaction_terminal_event(
            interaction_id,
            CompletionOutcome::CallbackPending {
                tool_use_id: "call-1".to_string(),
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
            ..
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
        let capabilities = bridge_capabilities(
            meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V4,
        );
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
            capabilities.hard_cancel_member,
            "phase 6 serves HardCancelMember: the bridge MUST advertise it — \
             the controlling side capability-gates dispatch on this fact \
             (ADJ-P6-5), so a false report would typed-reject every hard \
             cancel without bridge traffic"
        );
        assert!(
            capabilities.tracked_input_cancel,
            "V4 serves exact durable tracked-input cancellation and must advertise it"
        );
    }

    #[test]
    fn v3_bind_capabilities_omit_v4_extensions_and_hard_cancel() {
        let capabilities = bridge_capabilities(
            meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V3,
        );
        assert_eq!(
            capabilities.current_protocol_version,
            meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V3
        );
        assert!(!capabilities.hard_cancel_member);
        assert!(!capabilities.tracked_input_cancel);
        assert_eq!(
            capabilities.supported_protocol_versions,
            vec![
                meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V2,
                meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V3,
            ]
        );
        let encoded = serde_json::to_value(&capabilities).expect("serialize V3 capabilities");
        for v4_field in [
            "durable_sessions",
            "autonomous_members",
            "tracked_input_cancel",
            "memory_store",
            "mcp",
            "engine_version",
            "approval_forwarding",
            "resolvable_providers",
        ] {
            assert!(
                encoded.get(v4_field).is_none(),
                "V3 bind response leaked V4 capability field {v4_field}: {encoded}"
            );
        }
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
        let stable_input_id = Uuid::new_v4();
        let input = peer_input_from_delivery_payload(
            &SessionId::new(),
            PeerId::parse(PEER_ID_SUPERVISOR).expect("valid supervisor peer id"),
            BridgeDeliveryPayload {
                objective_id: None,
                injected_context: Vec::new(),
                supervisor: supervisor_bridge_spec(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                input_id: stable_input_id.to_string(),
                transcript_interaction_id: None,
                content: meerkat_core::types::ContentInput::Text("queued follow-up".to_string()),
                handling_mode: HandlingMode::Queue,
                expected_member: None,
                turn: None,
                outcome_tracking: None,
            },
        )
        .expect("UUID payload input id lowers");

        let Input::Peer(peer) = &input else {
            panic!("bridge delivery must project to peer input");
        };
        assert_eq!(
            peer.handling_mode,
            Some(HandlingMode::Queue),
            "bridge delivery explicit queue must survive into MeerkatMachine admission"
        );
        assert_eq!(peer.header.id.0, stable_input_id);
        assert_eq!(
            peer.header.correlation_id.as_ref().map(|id| id.0),
            Some(stable_input_id),
            "payload input_id is the stable terminal correlation, not the transient envelope id"
        );
        assert_eq!(
            peer.directed_interaction_id, None,
            "peer-only legacy delivery must not mint terminal-publication authority"
        );
        assert_eq!(
            crate::input::validated_directed_interaction_id(&input)
                .expect("legacy peer shape validates"),
            None
        );
        let semantics =
            crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(&input, true)
                .expect("legacy peer admission semantics");
        assert!(
            crate::runtime_loop::for_input(&input, semantics)
                .directed_interaction_ids
                .is_empty(),
            "correlation alone must not opt legacy peer traffic into directed terminals"
        );
    }

    #[test]
    fn explicitly_tracked_plain_bridge_delivery_mints_exact_directed_interaction() {
        let session_id = SessionId::new();
        let stable_input_id = Uuid::new_v4();
        let objective_id = meerkat_core::interaction::ObjectiveId::new();
        let payload = BridgeDeliveryPayload {
            objective_id: Some(objective_id),
            injected_context: vec![meerkat_core::types::ContentInput::Text(
                "tracked ambient context".to_string(),
            )],
            supervisor: supervisor_bridge_spec(),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            input_id: stable_input_id.to_string(),
            transcript_interaction_id: Some(stable_input_id.to_string()),
            content: meerkat_core::types::ContentInput::Text("placed work".to_string()),
            handling_mode: HandlingMode::Queue,
            expected_member: Some(BridgeMemberIncarnation {
                mob_id: "mob".to_string(),
                agent_identity: "worker".to_string(),
                host_id: "host".to_string(),
                binding_generation: 1,
                member_session_id: session_id.to_string(),
                generation: 1,
                fence_token: 11,
            }),
            turn: None,
            outcome_tracking: Some(BridgeOutcomeTracking::Interaction),
        };
        assert!(delivery_requires_tracked_turn_custody(&payload));
        let input = peer_input_from_delivery_payload(
            &session_id,
            PeerId::parse(PEER_ID_SUPERVISOR).expect("valid supervisor peer id"),
            payload,
        )
        .expect("placed UUID payload lowers");

        let expected = meerkat_core::interaction::InteractionId(stable_input_id);
        let Input::Peer(peer) = &input else {
            panic!("placed bridge delivery must project to peer input");
        };
        assert_eq!(peer.directed_interaction_id, Some(expected));
        assert_eq!(peer.objective_id, Some(objective_id));
        assert_eq!(
            peer.injected_context,
            vec![meerkat_core::types::ContentInput::Text(
                "tracked ambient context".to_string()
            )]
        );
        assert_eq!(
            crate::input::validated_directed_interaction_id(&input)
                .expect("placed peer shape validates"),
            Some(expected)
        );
        let semantics =
            crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(&input, true)
                .expect("placed peer admission semantics");
        assert_eq!(
            crate::runtime_loop::for_input(&input, semantics).directed_interaction_ids,
            vec![expected]
        );
    }

    #[test]
    fn placed_delivery_without_explicit_tracking_remains_untracked() {
        let session_id = SessionId::new();
        let stable_input_id = Uuid::new_v4();
        let transcript_interaction_id = Uuid::new_v4();
        let payload = BridgeDeliveryPayload {
            objective_id: None,
            injected_context: Vec::new(),
            supervisor: supervisor_bridge_spec(),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            input_id: stable_input_id.to_string(),
            transcript_interaction_id: Some(transcript_interaction_id.to_string()),
            content: meerkat_core::types::ContentInput::Text("ordinary placed work".to_string()),
            handling_mode: HandlingMode::Queue,
            expected_member: Some(BridgeMemberIncarnation {
                mob_id: "mob".to_string(),
                agent_identity: "worker".to_string(),
                host_id: "host".to_string(),
                binding_generation: 1,
                member_session_id: session_id.to_string(),
                generation: 1,
                fence_token: 11,
            }),
            turn: None,
            outcome_tracking: None,
        };
        assert!(!delivery_requires_tracked_turn_custody(&payload));
        validate_delivery_tracking_request(&payload)
            .expect("untracked placed admission accepts independent transcript identity");
        let mut unscoped = payload.clone();
        unscoped.expected_member = None;
        assert!(
            validate_delivery_tracking_request(&unscoped)
                .expect_err("transcript identity without placed residency must reject")
                .contains("exact placed-member incarnation")
        );
        let mut under_versioned = payload.clone();
        under_versioned.protocol_version = BridgeProtocolVersion::V3;
        assert!(
            validate_delivery_tracking_request(&under_versioned)
                .expect_err("placed transcript identity requires V4")
                .contains("V4")
        );
        let input = peer_input_from_delivery_payload(
            &session_id,
            PeerId::parse(PEER_ID_SUPERVISOR).expect("valid supervisor peer id"),
            payload,
        )
        .expect("ordinary placed payload lowers");
        let Input::Peer(peer) = &input else {
            panic!("ordinary placed bridge delivery must project to peer input");
        };
        assert_eq!(peer.directed_interaction_id, None);
        let expected = meerkat_core::interaction::InteractionId(transcript_interaction_id);
        let semantics =
            crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(&input, true)
                .expect("ordinary placed peer admission semantics");
        let metadata = crate::runtime_loop::for_input(&input, semantics);
        assert_eq!(
            metadata.transcript_identity.interaction_id,
            Some(expected),
            "the independent transcript carrier survives without terminal tracking"
        );
        assert!(
            metadata.directed_interaction_ids.is_empty(),
            "IngressAccepted must not acquire terminal-publication authority"
        );
        assert_eq!(
            crate::input::validated_directed_interaction_id(&input)
                .expect("ordinary placed peer shape validates"),
            None
        );
    }

    #[test]
    fn placed_admission_without_caller_transcript_id_keeps_identity_absent() {
        let session_id = SessionId::new();
        let input = peer_input_from_delivery_payload(
            &session_id,
            PeerId::parse(PEER_ID_SUPERVISOR).expect("valid supervisor peer id"),
            BridgeDeliveryPayload {
                objective_id: None,
                injected_context: Vec::new(),
                supervisor: supervisor_bridge_spec(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                input_id: Uuid::new_v4().to_string(),
                transcript_interaction_id: None,
                content: meerkat_core::types::ContentInput::Text(
                    "ordinary placed work".to_string(),
                ),
                handling_mode: HandlingMode::Queue,
                expected_member: Some(BridgeMemberIncarnation {
                    mob_id: "mob".to_string(),
                    agent_identity: "worker".to_string(),
                    host_id: "host".to_string(),
                    binding_generation: 1,
                    member_session_id: session_id.to_string(),
                    generation: 1,
                    fence_token: 11,
                }),
                turn: None,
                outcome_tracking: None,
            },
        )
        .expect("ordinary placed payload lowers");
        assert!(input.header().correlation_id.is_none());
        let semantics =
            crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(&input, true)
                .expect("ordinary placed peer admission semantics");
        assert!(
            crate::runtime_loop::for_input(&input, semantics)
                .transcript_identity
                .interaction_id
                .is_none(),
            "absent caller identity must remain absent for runtime-side minting"
        );
    }

    #[test]
    fn tracked_interaction_validation_is_v4_exact_member_and_mutually_exclusive() {
        let session_id = SessionId::new();
        let mut payload = BridgeDeliveryPayload {
            objective_id: None,
            injected_context: Vec::new(),
            supervisor: supervisor_bridge_spec(),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            input_id: Uuid::new_v4().to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("tracked work".to_string()),
            handling_mode: HandlingMode::Queue,
            expected_member: Some(BridgeMemberIncarnation {
                mob_id: "mob".to_string(),
                agent_identity: "worker".to_string(),
                host_id: "host".to_string(),
                binding_generation: 1,
                member_session_id: session_id.to_string(),
                generation: 1,
                fence_token: 11,
            }),
            turn: None,
            outcome_tracking: Some(BridgeOutcomeTracking::Interaction),
        };
        payload.transcript_interaction_id = Some(payload.input_id.clone());
        validate_delivery_tracking_request(&payload).expect("valid tracked interaction");

        payload.transcript_interaction_id = Some(Uuid::new_v4().to_string());
        assert!(
            validate_delivery_tracking_request(&payload)
                .expect_err("tracked terminal identity must not split from its sidecar key")
                .contains("equal input_id")
        );
        payload.transcript_interaction_id =
            Some("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA".to_string());
        assert!(
            validate_delivery_tracking_request(&payload)
                .expect_err("non-canonical transcript UUID must reject")
                .contains("canonical non-nil UUID")
        );
        payload.transcript_interaction_id = Some(payload.input_id.clone());

        let canonical_input_id = payload.input_id.clone();
        payload.input_id = Uuid::nil().to_string();
        payload.transcript_interaction_id = Some(payload.input_id.clone());
        assert!(
            validate_delivery_tracking_request(&payload)
                .expect_err("tracked peer nil UUID must reject")
                .contains("nil UUID")
        );
        assert!(
            peer_input_from_delivery_payload(
                &session_id,
                PeerId::parse(PEER_ID_SUPERVISOR).expect("valid supervisor peer id"),
                payload.clone(),
            )
            .expect_err("tracked peer lowering must independently reject nil UUID")
            .contains("nil UUID")
        );
        payload.input_id = canonical_input_id;
        payload.transcript_interaction_id = Some(payload.input_id.clone());

        payload.protocol_version = BridgeProtocolVersion::V3;
        assert!(
            validate_delivery_tracking_request(&payload)
                .expect_err("pre-V4 tracking marker must reject")
                .contains("V4")
        );
        payload.protocol_version = SUPERVISOR_BRIDGE_PROTOCOL_VERSION;
        payload.expected_member = None;
        assert!(
            validate_delivery_tracking_request(&payload)
                .expect_err("tracking without exact residency must reject")
                .contains("exact placed-member incarnation")
        );
        payload.expected_member = Some(BridgeMemberIncarnation {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            host_id: "host".to_string(),
            binding_generation: 1,
            member_session_id: session_id.to_string(),
            generation: 1,
            fence_token: 11,
        });
        payload.turn = Some(
            meerkat_contracts::wire::supervisor_bridge::BridgeTurnDirective {
                correlation: meerkat_contracts::wire::supervisor_bridge::BridgeTurnCorrelation {
                    run_id: "run".to_string(),
                    step_id: "step".to_string(),
                },
                tool_overlay: None,
            },
        );
        assert!(
            validate_delivery_tracking_request(&payload)
                .expect_err("turn and interaction marker must reject")
                .contains("mutually exclusive")
        );

        payload.outcome_tracking = None;
        payload.transcript_interaction_id = None;
        payload.input_id = Uuid::nil().to_string();
        assert!(
            validate_delivery_tracking_request(&payload)
                .expect_err("tracked flow-step nil UUID must reject")
                .contains("nil UUID")
        );
    }

    #[test]
    fn forged_tracked_peer_interaction_identity_is_rejected() {
        let session_id = SessionId::new();
        let tracked_id = Uuid::new_v4();
        let mut input = peer_input_from_delivery_payload(
            &session_id,
            PeerId::parse(PEER_ID_SUPERVISOR).expect("valid supervisor peer id"),
            BridgeDeliveryPayload {
                objective_id: None,
                injected_context: Vec::new(),
                supervisor: supervisor_bridge_spec(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                input_id: tracked_id.to_string(),
                transcript_interaction_id: Some(tracked_id.to_string()),
                content: meerkat_core::types::ContentInput::Text("placed work".to_string()),
                handling_mode: HandlingMode::Queue,
                expected_member: Some(BridgeMemberIncarnation {
                    mob_id: "mob".to_string(),
                    agent_identity: "worker".to_string(),
                    host_id: "host".to_string(),
                    binding_generation: 1,
                    member_session_id: session_id.to_string(),
                    generation: 1,
                    fence_token: 11,
                }),
                turn: None,
                outcome_tracking: Some(BridgeOutcomeTracking::Interaction),
            },
        )
        .expect("placed UUID payload lowers");
        let Input::Peer(peer) = &mut input else {
            panic!("placed bridge delivery must project to peer input");
        };
        peer.directed_interaction_id =
            Some(meerkat_core::interaction::InteractionId(Uuid::new_v4()));

        assert!(crate::input::validated_directed_interaction_id(&input).is_err());
    }

    #[test]
    fn bridge_delivery_payload_rejects_non_uuid_stable_input_id() {
        let lowered = peer_input_from_delivery_payload(
            &SessionId::new(),
            PeerId::parse(PEER_ID_SUPERVISOR).expect("valid supervisor peer id"),
            BridgeDeliveryPayload {
                objective_id: None,
                injected_context: Vec::new(),
                supervisor: supervisor_bridge_spec(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                input_id: "transport-envelope-alias".to_string(),
                transcript_interaction_id: None,
                content: meerkat_core::types::ContentInput::Text("work".to_string()),
                handling_mode: HandlingMode::Queue,
                expected_member: None,
                turn: None,
                outcome_tracking: None,
            },
        );
        assert!(lowered.is_err());
    }

    /// Remote-member receiver lowering: injected context on a bridge
    /// delivery payload reaches the peer-work input's typed slot, and the
    /// peer projection places the InjectedContext-role appends before the
    /// peer work append.
    #[test]
    fn bridge_delivery_payload_injected_context_projects_before_peer_append() {
        let objective_id = meerkat_core::interaction::ObjectiveId::new();
        let input = peer_input_from_delivery_payload(
            &SessionId::new(),
            PeerId::parse(PEER_ID_SUPERVISOR).expect("valid supervisor peer id"),
            BridgeDeliveryPayload {
                objective_id: Some(objective_id),
                injected_context: vec![
                    meerkat_core::types::ContentInput::Text("ambient alpha".to_string()),
                    meerkat_core::types::ContentInput::Text("ambient beta".to_string()),
                ],
                supervisor: supervisor_bridge_spec(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                input_id: Uuid::new_v4().to_string(),
                transcript_interaction_id: None,
                content: meerkat_core::types::ContentInput::Text("work content".to_string()),
                handling_mode: HandlingMode::Queue,
                expected_member: None,
                turn: None,
                outcome_tracking: None,
            },
        )
        .expect("UUID payload input id lowers");

        let Input::Peer(peer) = &input else {
            panic!("bridge delivery must project to peer input");
        };
        assert_eq!(
            peer.injected_context,
            vec![
                meerkat_core::types::ContentInput::Text("ambient alpha".to_string()),
                meerkat_core::types::ContentInput::Text("ambient beta".to_string()),
            ],
        );
        assert_eq!(peer.objective_id, Some(objective_id));

        let projection = crate::input::runtime_input_projection_for_machine_batch(&input);
        assert_eq!(
            projection
                .injected_context_appends
                .iter()
                .map(|append| (append.role, append.content.render_text()))
                .collect::<Vec<_>>(),
            vec![
                (
                    meerkat_core::lifecycle::run_primitive::ConversationAppendRole::InjectedContext,
                    "ambient alpha".to_string()
                ),
                (
                    meerkat_core::lifecycle::run_primitive::ConversationAppendRole::InjectedContext,
                    "ambient beta".to_string()
                ),
            ],
        );
        assert!(
            projection.append.is_some(),
            "peer work append must survive alongside injected context"
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
                objective_id: None,
                sender_taint: None,
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

    async fn stage_test_supervisor_rotation(
        adapter: &MeerkatMachine,
        session_id: &SessionId,
        next: crate::meerkat_machine::GeneratedSupervisorBinding,
    ) {
        let SupervisorBinding::Bound {
            peer_id: previous_peer_id,
            signing_public_key: previous_signing_public_key,
            epoch: previous_epoch,
            ..
        } = adapter.supervisor_binding(session_id).await
        else {
            panic!("test rotation requires a bound previous supervisor");
        };
        let operation_id = uuid::Uuid::new_v4().to_string();
        let submission = adapter
            .submit_supervisor_rotation(
                session_id,
                GeneratedSupervisorRotationSubmit {
                    operation_id: operation_id.clone(),
                    next: next.clone(),
                    preflight_rejection: None,
                    sender_peer_id: Some(previous_peer_id.clone()),
                    sender_signing_public_key: Some(previous_signing_public_key),
                },
            )
            .await
            .expect("stage test rotation submit");
        assert!(matches!(submission, SupervisorRotationSubmission::New(_)));
        adapter
            .stage_supervisor_rotation_previous_revoked(
                session_id,
                operation_id.clone(),
                previous_peer_id,
                previous_epoch,
            )
            .await
            .expect("stage test rotation previous-revoked checkpoint");
        adapter
            .stage_supervisor_rotation_next_published(
                session_id,
                operation_id,
                next.peer_id,
                next.epoch,
            )
            .await
            .expect("stage test rotation bound checkpoint");
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
                    delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked,
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
                protocol_version: supervisor_bridge_current_protocol_version(),
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
            response.capabilities.hard_cancel_member,
            "bind response advertises the phase-6 hard-cancel arm"
        );
        assert!(
            response.capabilities.tracked_input_cancel,
            "bind response advertises exact durable tracked-input cancellation"
        );
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
                        GeneratedSupervisorBinding {
                            name: supervisor.name.to_string(),
                            peer_id: supervisor.peer_id.as_str().clone(),
                            address: supervisor.address.to_string(),
                            signing_public_key: encode_supervisor_signing_public_key(
                                *supervisor.pubkey.as_bytes(),
                            ),
                            epoch: payload.epoch,
                        },
                        generated_sender_peer_id(&sender),
                        generated_sender_signing_public_key(&sender),
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
                        GeneratedSupervisorBinding {
                            name: supervisor.name.to_string(),
                            peer_id: supervisor.peer_id.as_str().clone(),
                            address: supervisor.address.to_string(),
                            signing_public_key: encode_supervisor_signing_public_key(
                                *supervisor.pubkey.as_bytes(),
                            ),
                            epoch: payload.epoch,
                        },
                        generated_sender_peer_id(&sender),
                        generated_sender_signing_public_key(&sender),
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
                objective_id: None,
                sender_taint: None,
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

    fn bridge_delivery_candidate_with_ingress(
        sender: &str,
        delivery: &serde_json::Value,
        ingress: PeerIngressFact,
    ) -> PeerInputCandidate {
        bridge_delivery_raw_candidate_with_ingress(
            sender,
            serde_json::to_string(delivery).expect("serialize bridge delivery body"),
            ingress,
        )
    }

    fn bridge_delivery_raw_candidate_with_ingress(
        sender: &str,
        body: String,
        ingress: PeerIngressFact,
    ) -> PeerInputCandidate {
        let id = ingress.interaction_id;
        PeerInputCandidate {
            interaction: InboxInteraction {
                objective_id: None,
                sender_taint: None,
                id,
                from_route: None,
                from: sender.to_string(),
                content: InteractionContent::Message { body, blocks: None },
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
    async fn raw_rotation_marker_versions_reject_durably_and_malformed_markers_fail_closed() {
        let current_spec = current_supervisor_bridge_spec();
        let current = TrustedPeerDescriptor::try_from(current_spec.clone())
            .expect("valid current supervisor");
        let target = supervisor_bridge_spec();

        let runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://raw-rotation-version",
            Some("token"),
        );
        let completed = Arc::clone(&runtime_impl.completed_count);
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
                current.name.to_string(),
                current.peer_id.as_str(),
                current.address.to_string(),
                signing_public_key_for_descriptor(&current),
                1,
            )
            .await
            .expect("bind current supervisor");

        let operation_id = SupervisorRotationOperationId::new();
        let raw_unsupported = json!({
            "delivery": "submit_supervisor_rotation",
            "operation_id": operation_id,
            "target": target,
            "target_epoch": 2,
            "protocol_version": 99,
        });
        let interaction_id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableMessage,
            meerkat_core::PeerIngressKind::Message,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                current.peer_id,
                current.name.as_str(),
                meerkat_core::PeerIngressConvention::Message,
            )
            .with_signing_pubkey(current.pubkey),
        );
        let candidate = bridge_delivery_candidate_with_ingress(
            current.name.as_str(),
            &raw_unsupported,
            ingress,
        );
        assert!(
            try_handle_supervisor_bridge_delivery(&adapter, &session_id, &runtime, &candidate,)
                .await,
            "exact delivery marker must be consumed before typed version decode"
        );
        let receipt = adapter
            .active_supervisor_rotation(&session_id)
            .await
            .expect("read rejected rotation")
            .expect("unsupported rotation is durably recorded");
        assert_eq!(
            receipt.phase,
            crate::meerkat_machine::dsl::SupervisorRotationPhase::Rejected
        );
        assert_eq!(
            receipt.rejection,
            Some(
                crate::meerkat_machine::dsl::SupervisorRotationRejectionKind::UnsupportedProtocolVersion
            )
        );
        assert_eq!(
            completed.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "one-way unsupported submission must close local ingress"
        );

        let malformed_runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://raw-rotation-malformed",
            Some("token"),
        );
        let malformed_completed = Arc::clone(&malformed_runtime_impl.completed_count);
        let malformed_runtime: Arc<dyn CommsRuntime> = Arc::new(malformed_runtime_impl);
        let malformed_adapter = Arc::new(MeerkatMachine::ephemeral());
        let malformed_session = SessionId::new();
        malformed_adapter
            .register_session(malformed_session.clone())
            .await
            .expect("register malformed-marker session");
        let malformed_id = InteractionId(Uuid::new_v4());
        let malformed_ingress = PeerIngressFact::peer(
            malformed_id,
            PeerInputClass::ActionableMessage,
            meerkat_core::PeerIngressKind::Message,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                current.peer_id,
                current.name.as_str(),
                meerkat_core::PeerIngressConvention::Message,
            )
            .with_signing_pubkey(current.pubkey),
        );
        let malformed = json!({
            "delivery": "submit_supervisor_rotation",
            "protocol_version": 4,
        });
        let malformed_candidate = bridge_delivery_candidate_with_ingress(
            current.name.as_str(),
            &malformed,
            malformed_ingress,
        );
        assert!(
            try_handle_supervisor_bridge_delivery(
                &malformed_adapter,
                &malformed_session,
                &malformed_runtime,
                &malformed_candidate,
            )
            .await,
            "malformed exact marker must be consumed fail-closed"
        );
        assert!(
            malformed_adapter
                .active_supervisor_rotation(&malformed_session)
                .await
                .expect("read malformed session")
                .is_none(),
            "malformed marker must not create partial durable operation state"
        );
        assert_eq!(
            malformed_completed.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "malformed marker must be completed locally instead of reaching the user prompt"
        );

        let duplicate_base = json!({
            "delivery": "submit_supervisor_rotation",
            "operation_id": SupervisorRotationOperationId::new(),
            "target": supervisor_bridge_spec(),
            "target_epoch": 2,
            "protocol_version": 4,
        })
        .to_string();
        let duplicate_body = format!(
            "{},\"delivery\":\"chat\"}}",
            duplicate_base
                .strip_suffix('}')
                .expect("delivery object has closing brace")
        );
        let duplicate_id = InteractionId(Uuid::new_v4());
        let duplicate_ingress = PeerIngressFact::peer(
            duplicate_id,
            PeerInputClass::ActionableMessage,
            meerkat_core::PeerIngressKind::Message,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                current.peer_id,
                current.name.as_str(),
                meerkat_core::PeerIngressConvention::Message,
            )
            .with_signing_pubkey(current.pubkey),
        );
        let duplicate_candidate = bridge_delivery_raw_candidate_with_ingress(
            current.name.as_str(),
            duplicate_body,
            duplicate_ingress,
        );
        assert!(
            try_handle_supervisor_bridge_delivery(
                &malformed_adapter,
                &malformed_session,
                &malformed_runtime,
                &duplicate_candidate,
            )
            .await,
            "reserved-first duplicate delivery marker must remain reserved and fail closed"
        );
        assert_eq!(
            malformed_completed.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "duplicate/conflicting delivery keys must be consumed instead of reaching the user prompt"
        );
        assert!(
            malformed_adapter
                .active_supervisor_rotation(&malformed_session)
                .await
                .expect("read duplicate-marker session")
                .is_none(),
            "duplicate marker must not create durable operation state"
        );
    }

    #[tokio::test]
    async fn pending_rotation_observe_does_not_mint_temporary_supervisor_trust() {
        let previous_spec = current_supervisor_bridge_spec();
        let previous = TrustedPeerDescriptor::try_from(previous_spec.clone())
            .expect("valid previous supervisor");
        let next_spec = supervisor_bridge_spec();
        let runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://pending-observe-no-retrust",
            Some("token"),
        );
        let trusted_peer_ids = Arc::clone(&runtime_impl.trusted_peer_ids);
        let sent_commands = Arc::clone(&runtime_impl.sent_commands);
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
                previous.name.to_string(),
                previous.peer_id.as_str(),
                previous.address.to_string(),
                signing_public_key_for_descriptor(&previous),
                1,
            )
            .await
            .expect("bind previous supervisor");

        let operation_id = SupervisorRotationOperationId::new();
        let next =
            TrustedPeerDescriptor::try_from(next_spec.clone()).expect("valid next supervisor");
        let submission = adapter
            .submit_supervisor_rotation(
                &session_id,
                GeneratedSupervisorRotationSubmit {
                    operation_id: operation_id.to_string(),
                    next: GeneratedSupervisorBinding {
                        name: next.name.to_string(),
                        peer_id: next.peer_id.as_str(),
                        address: next.address.to_string(),
                        signing_public_key: signing_public_key_for_descriptor(&next),
                        epoch: 2,
                    },
                    preflight_rejection: None,
                    sender_peer_id: Some(previous.peer_id.as_str()),
                    sender_signing_public_key: Some(signing_public_key_for_descriptor(&previous)),
                },
            )
            .await
            .expect("submit pending rotation");
        assert!(matches!(submission, SupervisorRotationSubmission::New(_)));

        let command = BridgeCommand::ObserveSupervisorRotation(
            meerkat_contracts::wire::supervisor_bridge::BridgeSupervisorRotationObserve {
                operation_id,
                observer: previous_spec,
                observer_epoch: 1,
                protocol_version: BridgeProtocolVersion::V4,
            },
        );
        let interaction_id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                previous.peer_id,
                previous.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: interaction_id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(previous.pubkey),
        );
        let candidate = bridge_candidate_with_ingress(previous.name.as_str(), &command, ingress);
        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate).await
        );
        assert!(
            trusted_peer_ids.lock().await.is_empty(),
            "pending observation must not temporarily retrust either rotation participant"
        );
        let replies = recorded_bridge_replies(&sent_commands).await;
        assert!(matches!(
            replies.as_slice(),
            [(
                meerkat_core::interaction::ResponseStatus::Completed,
                BridgeReply::SupervisorRotation(BridgeSupervisorRotationObservation::Found {
                    state: BridgeSupervisorRotationState::Pending { .. }
                })
            )]
        ));
    }

    #[tokio::test]
    async fn replacement_runtime_hydrates_completed_and_rejected_rotation_authority() {
        let previous = trusted_supervisor_descriptor(0xcc);
        let next = trusted_supervisor_descriptor(0xbb);

        let completed_runtime_name = format!("completed-rotation-hydration-{}", Uuid::new_v4());
        let completed_runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only(&completed_runtime_name)
                .expect("completed replacement runtime"),
        );
        let completed_runtime_dyn: Arc<dyn CommsRuntime> = completed_runtime.clone();
        let completed_adapter = Arc::new(MeerkatMachine::ephemeral());
        let completed_session = SessionId::new();
        completed_adapter
            .register_session(completed_session.clone())
            .await
            .expect("register completed session");
        completed_adapter
            .stage_supervisor_bind(
                &completed_session,
                previous.name.to_string(),
                previous.peer_id.as_str(),
                previous.address.to_string(),
                signing_public_key_for_descriptor(&previous),
                1,
            )
            .await
            .expect("bind previous completed supervisor");
        completed_adapter
            .test_install_session_peer_comms_handle_on_runtime(
                &completed_session,
                completed_runtime.as_ref(),
            )
            .await
            .expect("install completed runtime owner token");
        completed_adapter
            .stage_local_endpoint_for_comms_runtime(
                &completed_session,
                completed_runtime_dyn.as_ref(),
            )
            .await
            .expect("stage completed runtime local endpoint");

        let completed_operation = SupervisorRotationOperationId::new();
        completed_adapter
            .submit_supervisor_rotation(
                &completed_session,
                GeneratedSupervisorRotationSubmit {
                    operation_id: completed_operation.to_string(),
                    next: GeneratedSupervisorBinding {
                        name: next.name.to_string(),
                        peer_id: next.peer_id.as_str(),
                        address: next.address.to_string(),
                        signing_public_key: signing_public_key_for_descriptor(&next),
                        epoch: 2,
                    },
                    preflight_rejection: None,
                    sender_peer_id: Some(previous.peer_id.as_str()),
                    sender_signing_public_key: Some(signing_public_key_for_descriptor(&previous)),
                },
            )
            .await
            .expect("submit completed rotation");
        completed_adapter
            .stage_supervisor_rotation_resume(&completed_session, completed_operation.to_string())
            .await
            .expect("resume completed rotation");
        completed_adapter
            .stage_supervisor_rotation_previous_revoked(
                &completed_session,
                completed_operation.to_string(),
                previous.peer_id.as_str(),
                1,
            )
            .await
            .expect("checkpoint previous revoke");
        completed_adapter
            .stage_supervisor_rotation_next_published(
                &completed_session,
                completed_operation.to_string(),
                next.peer_id.as_str(),
                2,
            )
            .await
            .expect("checkpoint next publish");
        assert!(
            !completed_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(next.pubkey))
        );
        assert!(
            hydrate_current_bound_supervisor_route(
                &completed_adapter,
                &completed_session,
                &completed_runtime_dyn,
            )
            .await
            .expect("hydrate completed authority")
        );
        assert!(
            completed_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(next.pubkey)),
            "Completed replacement runtime must hydrate only the generated next authority"
        );
        assert!(
            !completed_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(previous.pubkey)),
            "Completed replacement runtime must not hydrate the revoked authority"
        );

        let rejected_runtime_name = format!("rejected-rotation-hydration-{}", Uuid::new_v4());
        let rejected_runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only(&rejected_runtime_name)
                .expect("rejected replacement runtime"),
        );
        let rejected_runtime_dyn: Arc<dyn CommsRuntime> = rejected_runtime.clone();
        let rejected_adapter = Arc::new(MeerkatMachine::ephemeral());
        let rejected_session = SessionId::new();
        rejected_adapter
            .register_session(rejected_session.clone())
            .await
            .expect("register rejected session");
        rejected_adapter
            .stage_supervisor_bind(
                &rejected_session,
                previous.name.to_string(),
                previous.peer_id.as_str(),
                previous.address.to_string(),
                signing_public_key_for_descriptor(&previous),
                1,
            )
            .await
            .expect("bind previous rejected supervisor");
        rejected_adapter
            .test_install_session_peer_comms_handle_on_runtime(
                &rejected_session,
                rejected_runtime.as_ref(),
            )
            .await
            .expect("install rejected runtime owner token");
        rejected_adapter
            .stage_local_endpoint_for_comms_runtime(
                &rejected_session,
                rejected_runtime_dyn.as_ref(),
            )
            .await
            .expect("stage rejected runtime local endpoint");
        rejected_adapter
            .submit_supervisor_rotation(
                &rejected_session,
                GeneratedSupervisorRotationSubmit {
                    operation_id: SupervisorRotationOperationId::new().to_string(),
                    next: GeneratedSupervisorBinding {
                        name: next.name.to_string(),
                        peer_id: next.peer_id.as_str(),
                        address: next.address.to_string(),
                        signing_public_key: signing_public_key_for_descriptor(&next),
                        epoch: 2,
                    },
                    preflight_rejection: Some(
                        crate::meerkat_machine::dsl::SupervisorRotationRejectionKind::UnsupportedProtocolVersion,
                    ),
                    sender_peer_id: Some(previous.peer_id.as_str()),
                    sender_signing_public_key: Some(signing_public_key_for_descriptor(&previous)),
                },
            )
            .await
            .expect("durably reject rotation");
        assert!(
            hydrate_current_bound_supervisor_route(
                &rejected_adapter,
                &rejected_session,
                &rejected_runtime_dyn,
            )
            .await
            .expect("hydrate rejected authority")
        );
        assert!(
            rejected_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(previous.pubkey)),
            "Rejected replacement runtime must hydrate only the retained previous authority"
        );
        assert!(
            !rejected_runtime
                .router()
                .is_private(&meerkat_comms::PubKey::new(next.pubkey)),
            "Rejected replacement runtime must not hydrate the rejected target"
        );
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
    async fn revoke_supervisor_failure_leaves_pending_checkpoint_and_retry_completes() {
        let supervisor = supervisor_bridge_spec();
        let runtime = Arc::new(bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        ));
        runtime
            .remove_failures_remaining
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let runtime_dyn: Arc<dyn CommsRuntime> = Arc::clone(&runtime) as Arc<dyn CommsRuntime>;
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
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime_dyn, &candidate,)
                .await,
            "revoke command should be handled"
        );
        assert!(
            matches!(
                adapter.supervisor_binding(&session_id).await,
                SupervisorBinding::Unbound
            ),
            "failed removal must leave durable revoke pending, not restore a live binding"
        );
        let retry = bridge_candidate(
            &supervisor.peer_id,
            &BridgeCommand::RevokeSupervisor(payload),
        );
        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime_dyn, &retry).await,
            "exact revoke retry should complete the pending checkpoint"
        );
        assert!(
            runtime
                .remove_attempts
                .load(std::sync::atomic::Ordering::SeqCst)
                >= 2,
            "retry must re-drive generated trust removal with fresh authority"
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
        // guard requires `Unbound`. Identity rotation uses the durable
        // supervisor-rotation operation.
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

        // AuthorizeSupervisor may advance only the same authority's version.
        adapter
            .stage_supervisor_authorize(
                &session_id,
                "super-a-refreshed".to_string(),
                "ed25519:super-a".to_string(),
                "inproc://super-a-refreshed".to_string(),
                test_supervisor_signing_public_key(0xaa),
                2,
            )
            .await
            .expect("same-authority version advance must succeed");

        // Read back: identity is unchanged and version advanced.
        match adapter.supervisor_binding(&session_id).await {
            SupervisorBinding::Bound { peer_id, epoch, .. } => {
                assert_eq!(peer_id, "ed25519:super-a");
                assert_eq!(epoch, 2);
            }
            SupervisorBinding::Unbound => panic!("expected Bound after rotation"),
        }

        // Proper revoke with matching identity + epoch must succeed and
        // return to Unbound.
        adapter
            .stage_supervisor_revoke(&session_id, "ed25519:super-a".to_string(), 2)
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
            .stage_supervisor_binding_route_refresh(
                &session_id,
                GeneratedSupervisorBinding {
                    name: "super-a-renamed".to_string(),
                    peer_id: supervisor_peer_id.clone(),
                    address: "inproc://super-new".to_string(),
                    signing_public_key: test_supervisor_signing_public_key(0xaa),
                    epoch: 7,
                },
                Some(supervisor_peer_id),
            )
            .await
            .expect("same-authority same-epoch route refresh");

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
    async fn declare_member_outbound_taint_from_bound_supervisor_installs_declaration() {
        let runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("receiver-taint-install").unwrap());
        // The peer request/response authority is load-bearing for supervisor
        // command authorization (response-route resolution), not just acks.
        install_test_peer_request_response_authority(
            &runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );
        let supervisor_runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("mob/__mob_supervisor_taint__").unwrap(),
        );
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let supervisor =
            trusted_peer_from_runtime("mob/__mob_supervisor_taint__", &supervisor_runtime);
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
        // The supervisor response route comes from the handler's bound-
        // supervisor route repair (no member-side projection trust needed).
        adapter
            .test_install_session_peer_comms_handle_on_runtime(&session_id, runtime.as_ref())
            .await
            .expect("install adapter session peer-comms handle on member runtime");

        let command = BridgeCommand::DeclareMemberOutboundTaint(BridgeOutboundTaintPayload {
            supervisor: supervisor.clone().into(),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            target: Some(BridgeOutboundTaintTarget::PeerOnly),
            taint: Some(meerkat_core::comms::SenderContentTaint::Tainted),
        });
        let interaction_id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                supervisor.peer_id,
                supervisor.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: interaction_id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(supervisor.pubkey),
        );
        let candidate = bridge_candidate_with_ingress(supervisor.name.as_str(), &command, ingress);

        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
            )
            .await,
            "declare-outbound-taint bridge command should be handled"
        );
        assert_eq!(
            runtime.outbound_content_taint(),
            Some(meerkat_core::comms::SenderContentTaint::Tainted),
            "the relayed host declaration must install on the member runtime"
        );

        runtime.set_outbound_content_taint(None);
        let legacy_command =
            BridgeCommand::DeclareMemberOutboundTaint(BridgeOutboundTaintPayload {
                supervisor: supervisor.clone().into(),
                epoch: 1,
                protocol_version: BridgeProtocolVersion::V3,
                target: None,
                taint: Some(meerkat_core::comms::SenderContentTaint::Tainted),
            });
        let legacy_id = InteractionId(Uuid::new_v4());
        let legacy_ingress = PeerIngressFact::peer(
            legacy_id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                supervisor.peer_id,
                supervisor.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: legacy_id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(supervisor.pubkey),
        );
        let legacy_candidate = bridge_candidate_with_ingress(
            supervisor.name.as_str(),
            &legacy_command,
            legacy_ingress,
        );
        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(runtime.clone() as Arc<dyn CommsRuntime>),
                &legacy_candidate,
            )
            .await
        );
        assert_eq!(
            runtime.outbound_content_taint(),
            Some(meerkat_core::comms::SenderContentTaint::Tainted),
            "an omitted-target V3 declaration remains valid for a truly peer-only session"
        );
    }

    #[tokio::test]
    async fn delayed_g1_outbound_taint_cannot_mutate_revived_g2_member() {
        let runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only("receiver-taint-host-g2").unwrap());
        install_test_peer_request_response_authority(
            &runtime,
            Arc::new(CountingPeerInteractionHandle::default()),
        );
        let supervisor_runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("mob/__mob_supervisor_taint_g2__").unwrap(),
        );
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let supervisor =
            trusted_peer_from_runtime("mob/__mob_supervisor_taint_g2__", &supervisor_runtime);
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
        adapter
            .test_install_session_peer_comms_handle_on_runtime(&session_id, runtime.as_ref())
            .await
            .expect("install adapter session peer-comms handle");

        let current = BridgeMemberIncarnation {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            host_id: "host".to_string(),
            binding_generation: 2,
            member_session_id: session_id.to_string(),
            generation: 1,
            fence_token: 7,
        };
        register_member_incarnation(&adapter, session_id.clone(), current.clone())
            .await
            .expect("register current member incarnation");
        let mut delayed = current;
        delayed.binding_generation = 1;
        let command = BridgeCommand::DeclareMemberOutboundTaint(BridgeOutboundTaintPayload {
            supervisor: supervisor.clone().into(),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            target: Some(BridgeOutboundTaintTarget::Placed(delayed)),
            taint: Some(meerkat_core::comms::SenderContentTaint::Tainted),
        });
        let interaction_id = InteractionId(Uuid::new_v4());
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                supervisor.peer_id,
                supervisor.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: interaction_id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(supervisor.pubkey),
        );
        let candidate = bridge_candidate_with_ingress(supervisor.name.as_str(), &command, ingress);
        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(runtime.clone() as Arc<dyn CommsRuntime>),
                &candidate,
            )
            .await
        );
        assert_eq!(
            runtime.outbound_content_taint(),
            None,
            "a delayed G1 declaration must be rejected before mutating the revived G2 runtime"
        );

        let legacy_command =
            BridgeCommand::DeclareMemberOutboundTaint(BridgeOutboundTaintPayload {
                supervisor: supervisor.clone().into(),
                epoch: 1,
                protocol_version: BridgeProtocolVersion::V3,
                target: None,
                taint: Some(meerkat_core::comms::SenderContentTaint::Tainted),
            });
        let legacy_id = InteractionId(Uuid::new_v4());
        let legacy_ingress = PeerIngressFact::peer(
            legacy_id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(
                supervisor.peer_id,
                supervisor.name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: legacy_id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(supervisor.pubkey),
        );
        let legacy_candidate = bridge_candidate_with_ingress(
            supervisor.name.as_str(),
            &legacy_command,
            legacy_ingress,
        );
        assert!(
            try_handle_supervisor_bridge_command(
                &adapter,
                &session_id,
                &(runtime.clone() as Arc<dyn CommsRuntime>),
                &legacy_candidate,
            )
            .await
        );
        assert_eq!(
            runtime.outbound_content_taint(),
            None,
            "legacy omitted-target declarations must not address any registered placed incarnation"
        );
    }

    #[test]
    fn delayed_g1_peer_message_fence_does_not_match_revived_g2_recipient() {
        let current = BridgeMemberIncarnation {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            host_id: "host".to_string(),
            binding_generation: 2,
            member_session_id: "session".to_string(),
            generation: 1,
            fence_token: 7,
        };
        let mut delayed = meerkat_core::comms::PeerRecipientIncarnation {
            mob_id: current.mob_id.clone(),
            agent_identity: current.agent_identity.clone(),
            host_id: current.host_id.clone(),
            binding_generation: 1,
            member_session_id: current.member_session_id.clone(),
            generation: current.generation,
            fence_token: current.fence_token,
        };
        assert!(
            !peer_recipient_incarnation_matches(&delayed, &current),
            "a signed G1 envelope delayed across revoke/rebind cannot address G2"
        );
        delayed.binding_generation = 2;
        assert!(peer_recipient_incarnation_matches(&delayed, &current));
    }

    #[tokio::test]
    async fn declare_member_outbound_taint_from_unauthorized_sender_installs_nothing() {
        let runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("receiver-taint-unauthorized").unwrap(),
        );
        let supervisor_runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only("mob/__mob_supervisor_taint_real__").unwrap(),
        );
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let supervisor =
            trusted_peer_from_runtime("mob/__mob_supervisor_taint_real__", &supervisor_runtime);
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
        // The sender is NOT the bound supervisor: the machine-owned
        // supervisor admission must reject the command and the declaration
        // must not install.
        let impostor_peer_id = PeerId::new();
        let candidate = bridge_candidate(
            &impostor_peer_id.as_str(),
            &BridgeCommand::DeclareMemberOutboundTaint(BridgeOutboundTaintPayload {
                supervisor: supervisor.clone().into(),
                epoch: 1,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                target: Some(BridgeOutboundTaintTarget::PeerOnly),
                taint: Some(meerkat_core::comms::SenderContentTaint::Tainted),
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
            "unauthorized declare must still be handled (and rejected)"
        );
        assert_eq!(
            runtime.outbound_content_taint(),
            None,
            "an unauthorized sender must not install an outbound taint declaration"
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

    // ---- Member-upcall waiter seam (T-11) ----

    enum TestWaiterEntry {
        Waiting(tokio::sync::oneshot::Sender<PeerInputCandidate>),
        TimedOut,
    }

    /// One-shot drain fixture with a bridge-reply waiter registry, mirroring
    /// the concrete runtime's take/has semantics (tombstone answers `has`,
    /// yields no sender on take, and is consumed by take).
    struct WaiterReplyRuntime {
        notify: Arc<tokio::sync::Notify>,
        candidate: std::sync::Mutex<Option<PeerInputCandidate>>,
        peer_handle: Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>>,
        completed_count: std::sync::atomic::AtomicUsize,
        waiters: std::sync::Mutex<HashMap<Uuid, TestWaiterEntry>>,
    }

    impl WaiterReplyRuntime {
        fn new(
            candidate: PeerInputCandidate,
            peer_handle: Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>>,
            in_reply_to: InteractionId,
            entry: TestWaiterEntry,
        ) -> Self {
            let mut waiters = HashMap::new();
            waiters.insert(in_reply_to.0, entry);
            Self {
                notify: Arc::new(tokio::sync::Notify::new()),
                candidate: std::sync::Mutex::new(Some(candidate)),
                peer_handle,
                completed_count: std::sync::atomic::AtomicUsize::new(0),
                waiters: std::sync::Mutex::new(waiters),
            }
        }

        fn completed_count(&self) -> usize {
            self.completed_count
                .load(std::sync::atomic::Ordering::SeqCst)
        }

        fn waiter_entry_count(&self) -> usize {
            self.waiters.lock().expect("waiters mutex").len()
        }
    }

    #[async_trait::async_trait]
    impl CommsRuntime for WaiterReplyRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            self.notify.clone()
        }

        fn peer_interaction_handle(
            &self,
        ) -> Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>> {
            self.peer_handle.clone()
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

        fn take_bridge_reply_waiter(
            &self,
            in_reply_to: &InteractionId,
        ) -> Option<tokio::sync::oneshot::Sender<PeerInputCandidate>> {
            match self
                .waiters
                .lock()
                .expect("waiters mutex")
                .remove(&in_reply_to.0)
            {
                Some(TestWaiterEntry::Waiting(sender)) => Some(sender),
                Some(TestWaiterEntry::TimedOut) | None => None,
            }
        }

        fn has_bridge_reply_waiter(&self, in_reply_to: &InteractionId) -> bool {
            self.waiters
                .lock()
                .expect("waiters mutex")
                .contains_key(&in_reply_to.0)
        }
    }

    fn response_candidate_replying_to(
        class: PeerInputClass,
        response_terminality: Option<meerkat_core::interaction::TerminalityClass>,
        in_reply_to: InteractionId,
    ) -> PeerInputCandidate {
        let mut candidate = response_candidate(class, response_terminality);
        if let InteractionContent::Response {
            in_reply_to: reply_slot,
            ..
        } = &mut candidate.interaction.content
        {
            *reply_slot = in_reply_to;
        }
        candidate
    }

    /// T-11: a registered waiter receives the terminal Response candidate
    /// (typed terminality intact), the interaction is marked complete, and
    /// the candidate is NEVER injected into the session (input ledger stays
    /// empty).
    #[tokio::test]
    async fn terminal_response_with_registered_waiter_is_delivered_not_injected() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let in_reply_to = InteractionId(Uuid::new_v4());
        let candidate = response_candidate_replying_to(
            PeerInputClass::ResponseTerminal,
            Some(meerkat_core::interaction::TerminalityClass::Terminal {
                disposition: meerkat_core::interaction::TerminalDisposition::Completed,
            }),
            in_reply_to,
        );
        let (waiter_tx, waiter_rx) = tokio::sync::oneshot::channel();
        let runtime = Arc::new(WaiterReplyRuntime::new(
            candidate,
            None,
            in_reply_to,
            TestWaiterEntry::Waiting(waiter_tx),
        ));

        let drain = spawn_authorized_test_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Duration::from_millis(10),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("drain should exit after one candidate")
            .expect("drain task should not panic");

        let delivered = waiter_rx.await.expect("waiter must receive the candidate");
        assert!(
            matches!(
                delivered.response_terminality,
                Some(meerkat_core::interaction::TerminalityClass::Terminal {
                    disposition: meerkat_core::interaction::TerminalDisposition::Completed,
                })
            ),
            "typed terminality must reach the waiter intact"
        );
        assert!(
            matches!(
                &delivered.interaction.content,
                InteractionContent::Response { in_reply_to: got, .. } if *got == in_reply_to
            ),
            "the waiter must receive the correlated Response"
        );
        assert!(
            runtime.completed_count() >= 1,
            "waiter delivery must mark the interaction complete"
        );
        assert_eq!(
            runtime.waiter_entry_count(),
            0,
            "take must consume the waiter"
        );
        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("registered session snapshot");
        assert_eq!(
            snapshot.ledger.input_count, 0,
            "a waiter-consumed reply must never become session input"
        );
    }

    /// T-11: a tombstoned waiter (member-side timeout already surfaced)
    /// consumes and discards the late terminal reply — no session injection,
    /// tombstone removed, interaction marked complete.
    #[tokio::test]
    async fn tombstoned_waiter_consumes_late_terminal_reply_without_injection() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let in_reply_to = InteractionId(Uuid::new_v4());
        let candidate = response_candidate_replying_to(
            PeerInputClass::ResponseTerminal,
            Some(meerkat_core::interaction::TerminalityClass::Terminal {
                disposition: meerkat_core::interaction::TerminalDisposition::Completed,
            }),
            in_reply_to,
        );
        let runtime = Arc::new(WaiterReplyRuntime::new(
            candidate,
            None,
            in_reply_to,
            TestWaiterEntry::TimedOut,
        ));

        let drain = spawn_authorized_test_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Duration::from_millis(10),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("drain should exit after one candidate")
            .expect("drain task should not panic");

        assert_eq!(
            runtime.waiter_entry_count(),
            0,
            "the tombstone must be consumed by the late reply"
        );
        assert!(
            runtime.completed_count() >= 1,
            "late-reply discard must mark the interaction complete"
        );
        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("registered session snapshot");
        assert_eq!(
            snapshot.ledger.input_count, 0,
            "a discarded late reply must never leak into the session"
        );
    }

    /// T-11: a Progress response with a registered waiter is suppressed —
    /// never session input — while the waiter stays registered for the
    /// terminal reply.
    #[tokio::test]
    async fn progress_response_with_registered_waiter_is_suppressed() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let in_reply_to = InteractionId(Uuid::new_v4());
        let candidate = response_candidate_replying_to(
            PeerInputClass::ResponseProgress,
            Some(meerkat_core::interaction::TerminalityClass::Progress),
            in_reply_to,
        );
        let (waiter_tx, _waiter_rx) = tokio::sync::oneshot::channel();
        let runtime = Arc::new(WaiterReplyRuntime::new(
            candidate,
            None,
            in_reply_to,
            TestWaiterEntry::Waiting(waiter_tx),
        ));

        let drain = spawn_authorized_test_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Duration::from_millis(10),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("drain should exit after one candidate")
            .expect("drain task should not panic");

        assert_eq!(
            runtime.waiter_entry_count(),
            1,
            "a Progress reply must not consume the waiter"
        );
        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("registered session snapshot");
        assert_eq!(
            snapshot.ledger.input_count, 0,
            "a Progress reply for a waited interaction must not become session input"
        );
    }

    /// T-11 regression row: with NO waiter registered the terminal Response
    /// path is byte-identical to the pre-waiter behavior — the candidate is
    /// injected into the session as before.
    #[tokio::test]
    async fn terminal_response_without_waiter_keeps_existing_injection_path() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let runtime = Arc::new(OneShotPeerRequestRuntime::new(
            response_candidate(
                PeerInputClass::ResponseTerminal,
                Some(meerkat_core::interaction::TerminalityClass::Terminal {
                    disposition: meerkat_core::interaction::TerminalDisposition::Completed,
                }),
            ),
            None,
        ));
        let drain = spawn_authorized_test_comms_drain(
            adapter.clone(),
            session_id.clone(),
            runtime.clone(),
            Duration::from_millis(10),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("drain should exit after one candidate")
            .expect("drain task should not panic");

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("registered session snapshot");
        assert_eq!(
            snapshot.ledger.input_count, 1,
            "without a waiter the terminal response must still inject as session input"
        );
    }

    // ---- Reject-reply parity (T-14) ----

    /// A decoded bridge payload's peer address is an identity descriptor, not
    /// callback authority. Without an authenticated/source-confined endpoint
    /// on the Request envelope, even a typed rejection to a signed-but-never-
    /// trusted sender must not stage or dial payload-selected TCP/UDS targets.
    #[tokio::test]
    async fn bind_member_invalid_bootstrap_never_stages_payload_reply_address() {
        let runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        let staged = runtime_impl.staged_reply_endpoints.clone();
        let staged_correlated = runtime_impl.staged_correlated_reply_endpoints.clone();
        let sent = runtime_impl.sent_commands.clone();
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        for payload_address in [
            "tcp://127.0.0.1:9",
            "uds:///tmp/meerkat-payload-selected-reply.sock",
        ] {
            let mut payload = sample_bind_payload();
            payload.bootstrap_token = "wrong-token".into();
            payload.supervisor.address = payload_address.to_string();
            // Pubkey-string sender: this is the strongest legacy case, with an
            // ingress-authenticated signer but no source-confined callback.
            let sender = pubkey_sender_for_bridge_spec(&payload.supervisor);
            let candidate = bridge_candidate(&sender, &BridgeCommand::BindMember(payload));

            assert!(
                try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate,)
                    .await
            );
        }

        // Open-auth-shaped ingress carries an auth exemption and canonical
        // identity but no verified signing key. It must not recover callback
        // authority from the decoded payload either.
        let mut open_payload = sample_bind_payload();
        open_payload.bootstrap_token = "wrong-token".into();
        open_payload.supervisor.address = "tcp://127.0.0.1:19".to_string();
        let open_sender = open_payload.supervisor.name.clone();
        let open_sender_peer_id =
            meerkat_comms::PubKey::new(open_payload.supervisor.pubkey).to_peer_id();
        let open_ingress =
            bridge_sender_fact_with_display(open_sender_peer_id, open_sender.as_str());
        let open_candidate = bridge_candidate_with_ingress(
            open_sender.as_str(),
            &BridgeCommand::BindMember(open_payload),
            open_ingress,
        );
        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &open_candidate,)
                .await
        );

        let staged = staged.lock().expect("staged reply endpoints mutex").clone();
        assert!(
            staged.is_empty(),
            "payload-selected TCP/UDS addresses must never become callback routes: {staged:?}"
        );
        let staged_correlated = staged_correlated
            .lock()
            .expect("staged correlated reply endpoints mutex")
            .clone();
        assert!(
            staged_correlated.is_empty(),
            "payload-selected TCP/UDS/open-auth addresses must never become correlated callback routes: {staged_correlated:?}"
        );
        let replies = recorded_bridge_replies(&sent).await;
        assert_eq!(
            replies.len(),
            3,
            "one rejection attempt per malicious payload"
        );
        for (status, reply) in replies {
            assert_eq!(status, meerkat_core::interaction::ResponseStatus::Failed);
            assert!(
                matches!(&reply, BridgeReply::Rejected { .. }),
                "invalid bootstrap must produce a typed rejection attempt, got {reply:?}"
            );
        }
    }

    /// A classifier-produced source-confined endpoint takes the real bridge
    /// response staging path and preserves the complete correlation tuple.
    /// This is deliberately separate from the payload-address rejection test:
    /// the decoded supervisor address remains irrelevant even when a valid
    /// signed TCP callback capability is present on ingress.
    #[tokio::test]
    async fn invalid_bootstrap_stages_exact_source_confined_callback_tuple() {
        let runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        let staged_legacy = runtime_impl.staged_reply_endpoints.clone();
        let staged_correlated = runtime_impl.staged_correlated_reply_endpoints.clone();
        let unstaged_correlated = runtime_impl.unstaged_correlated_reply_endpoints.clone();
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let mut payload = sample_bind_payload();
        payload.bootstrap_token = "wrong-token".into();
        payload.supervisor.address =
            "uds:///tmp/payload-address-is-not-callback-authority.sock".to_string();
        let sender_name = payload.supervisor.name.clone();
        let signer_pubkey = payload.supervisor.pubkey;
        let sender_peer_id = meerkat_comms::PubKey::new(signer_pubkey).to_peer_id();
        let interaction_id = InteractionId(Uuid::new_v4());
        let callback = PeerAddress::parse("tcp://192.0.2.44:4311")
            .expect("source-confined callback endpoint parses");
        let ingress = PeerIngressFact::peer(
            interaction_id,
            PeerInputClass::ActionableRequest,
            meerkat_core::PeerIngressKind::Request,
            Some(meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
            )),
            PeerIngressIdentity::new(
                sender_peer_id,
                sender_name.as_str(),
                meerkat_core::PeerIngressConvention::Request {
                    request_id: interaction_id.to_string(),
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                },
            )
            .with_signing_pubkey(signer_pubkey),
        )
        .with_declared_reply_endpoint(Some(callback.clone()));
        let candidate = bridge_candidate_with_ingress(
            sender_name.as_str(),
            &BridgeCommand::BindMember(payload),
            ingress,
        );

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate,)
                .await
        );
        assert!(
            staged_legacy
                .lock()
                .expect("staged legacy reply endpoints mutex")
                .is_empty(),
            "a correlated ingress callback must never use the legacy staging seam"
        );
        assert_eq!(
            staged_correlated
                .lock()
                .expect("staged correlated reply endpoints mutex")
                .as_slice(),
            &[(sender_peer_id, interaction_id, signer_pubkey, callback)],
            "bridge response staging must preserve peer, correlation, signer, and source-confined endpoint"
        );
        assert!(
            unstaged_correlated
                .lock()
                .expect("unstaged correlated reply endpoints mutex")
                .is_empty(),
            "a successfully recorded response must leave cleanup to the one-shot Router send"
        );
    }

    /// T-14: pre-decode failures carry NO declared reply address (the payload
    /// never decoded), so nothing is staged.
    #[tokio::test]
    async fn undecodable_bridge_command_stages_no_reply_endpoint() {
        let runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        let staged = runtime_impl.staged_reply_endpoints.clone();
        let sent = runtime_impl.sent_commands.clone();
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let id = InteractionId(Uuid::new_v4());
        let sender_key = meerkat_comms::PubKey::new([0xbb; 32]).to_pubkey_string();
        let ingress = bridge_sender_fact_with_id(id, &sender_key);
        let candidate = PeerInputCandidate {
            interaction: InboxInteraction {
                sender_taint: None,
                id,
                from_route: None,
                from: sender_key.clone(),
                content: InteractionContent::Request {
                    intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                    params: json!({ "not": "a bridge command" }),
                    blocks: None,
                },
                rendered_text: String::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
                objective_id: None,
            },
            ingress,
            lifecycle_peer: None,
            response_terminality: None,
        };

        assert!(
            try_handle_supervisor_bridge_command(&adapter, &session_id, &runtime, &candidate).await
        );
        assert!(
            staged
                .lock()
                .expect("staged reply endpoints mutex")
                .is_empty(),
            "pre-decode failures must stage nothing"
        );
        let replies = recorded_bridge_replies(&sent).await;
        assert_eq!(replies.len(), 1, "exactly one rejection reply");
        assert!(matches!(&replies[0].1, BridgeReply::Rejected { .. }));
    }

    /// T-14: the accept path never stages — supervisor trust is installed
    /// before the reply, so the trusted route serves it.
    #[tokio::test]
    async fn bind_member_accept_path_stages_no_reply_endpoint() {
        let payload = sample_bind_payload();
        let sender = pubkey_sender_for_bridge_spec(&payload.supervisor);
        let runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        let staged = runtime_impl.staged_reply_endpoints.clone();
        let sent = runtime_impl.sent_commands.clone();
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
        let replies = recorded_bridge_replies(&sent).await;
        assert_eq!(replies.len(), 1);
        assert!(
            matches!(&replies[0].1, BridgeReply::BindMember(_)),
            "expected a successful bind reply, got {:?}",
            replies[0].1
        );
        assert!(
            staged
                .lock()
                .expect("staged reply endpoints mutex")
                .is_empty(),
            "the accept path must not stage a declared reply endpoint"
        );
    }

    // ---- Host-seeded supervisor bind (DEC-P3H-4 / ADJ-18, T24) ----

    /// Happy path: the composed API installs the supervisor binding and the
    /// PRIVATE admission trust edge on a scratch runtime through the same
    /// generated seams as the BindMember drain arm.
    #[tokio::test]
    async fn bind_supervisor_for_materialized_session_installs_binding_and_private_trust() {
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
        let supervisor = trusted_supervisor_descriptor(0xbb);

        bind_supervisor_for_materialized_session(
            &adapter,
            &session_id,
            runtime.as_ref(),
            &supervisor,
            1,
        )
        .await
        .expect("host-seeded supervisor bind must succeed");

        assert!(
            !matches!(
                adapter.supervisor_binding(&session_id).await,
                SupervisorBinding::Unbound
            ),
            "the supervisor binding must be installed"
        );
        assert!(
            trusted_peer_ids
                .lock()
                .await
                .contains(&supervisor.peer_id.to_string()),
            "the private supervisor admission edge must be installed"
        );

        // Ensure-on-replay (ADJ-9): a second identical bind is an idempotent
        // rebind that repairs trust and succeeds.
        bind_supervisor_for_materialized_session(
            &adapter,
            &session_id,
            runtime.as_ref(),
            &supervisor,
            1,
        )
        .await
        .expect("idempotent host-seeded rebind must succeed");
        assert!(
            !matches!(
                adapter.supervisor_binding(&session_id).await,
                SupervisorBinding::Unbound
            ),
            "the binding must survive the idempotent rebind"
        );
    }

    /// Host rebind advances the same supervisor authority version and route;
    /// it must use generated AuthorizeSupervisor admission rather than a
    /// second bootstrap BindSupervisor.
    #[tokio::test]
    async fn authorize_supervisor_for_materialized_session_advances_same_peer_and_key() {
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
        let original = trusted_supervisor_descriptor(0xbb);
        let original_peer_id = original.peer_id.to_string();
        bind_supervisor_for_materialized_session(
            &adapter,
            &session_id,
            runtime.as_ref(),
            &original,
            7,
        )
        .await
        .expect("initial host-seeded supervisor bind");
        let refreshed_address = "inproc://mob/__mob_supervisor__/restarted";
        let refreshed = TrustedPeerDescriptor::unsigned_with_pubkey(
            original.name.clone(),
            original.peer_id.to_string(),
            original.pubkey,
            refreshed_address,
        )
        .expect("same-authority refreshed descriptor");

        authorize_supervisor_for_materialized_session(
            &adapter,
            &session_id,
            runtime.as_ref(),
            &refreshed,
            8,
        )
        .await
        .expect("same-peer/key monotonic host refresh succeeds");

        assert!(matches!(
            adapter.supervisor_binding(&session_id).await,
            SupervisorBinding::Bound {
                ref peer_id,
                ref address,
                epoch: 8,
                ..
            } if peer_id == &original_peer_id && address == refreshed_address
        ));
        assert!(
            trusted_peer_ids.lock().await.contains(&original_peer_id),
            "the refreshed private supervisor trust edge is published before success"
        );
    }

    /// A route-version commit whose private trust publication fails must
    /// leave no stale route trusted. The committed same-authority version is
    /// intentionally retryable: an exact retry re-drives generated route
    /// publication without another version advance.
    #[tokio::test]
    async fn authorize_supervisor_for_materialized_session_publication_failure_is_fail_closed_and_retryable()
     {
        let original = trusted_supervisor_descriptor(0xbb);
        let original_peer_id = original.peer_id.to_string();
        let refreshed_address = "inproc://mob/__mob_supervisor__/restarted";
        let refreshed = TrustedPeerDescriptor::unsigned_with_pubkey(
            original.name.clone(),
            original.peer_id.to_string(),
            original.pubkey,
            refreshed_address,
        )
        .expect("same-authority refreshed descriptor");

        let mut failing_runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        failing_runtime_impl
            .add_trusted_peer_errors
            .insert(original_peer_id.clone(), "boom".to_string());
        let failing_trusted_peer_ids = failing_runtime_impl.trusted_peer_ids.clone();
        let failing_runtime: Arc<dyn CommsRuntime> = Arc::new(failing_runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");
        adapter
            .stage_local_endpoint_for_comms_runtime(&session_id, failing_runtime.as_ref())
            .await
            .expect("stage local endpoint");
        adapter
            .stage_supervisor_bind(
                &session_id,
                original.name.to_string(),
                original.peer_id.as_str(),
                original.address.to_string(),
                signing_public_key_for_descriptor(&original),
                7,
            )
            .await
            .expect("seed current binding");
        adapter
            .stage_supervisor_trust_published(&session_id, original_peer_id.clone(), 7)
            .await
            .expect("close seeded publication");
        failing_trusted_peer_ids
            .lock()
            .await
            .insert(original_peer_id.clone());

        let error = authorize_supervisor_for_materialized_session(
            &adapter,
            &session_id,
            failing_runtime.as_ref(),
            &refreshed,
            8,
        )
        .await
        .expect_err("synthetic trust publication failure must surface");
        assert!(matches!(
            error,
            SupervisorMaterializeAuthorizeError::Apply(_)
        ));
        assert!(matches!(
            adapter.supervisor_binding(&session_id).await,
            SupervisorBinding::Bound {
                ref peer_id,
                ref address,
                epoch: 8,
                ..
            } if peer_id == &original_peer_id && address == refreshed_address
        ));
        assert!(
            failing_trusted_peer_ids.lock().await.is_empty(),
            "failed replacement publication must remove the stale route"
        );

        let retry_runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        let retry_trusted_peer_ids = retry_runtime_impl.trusted_peer_ids.clone();
        let retry_runtime: Arc<dyn CommsRuntime> = Arc::new(retry_runtime_impl);
        authorize_supervisor_for_materialized_session(
            &adapter,
            &session_id,
            retry_runtime.as_ref(),
            &refreshed,
            8,
        )
        .await
        .expect("exact retry re-drives the pending private trust publication");
        assert!(
            retry_trusted_peer_ids
                .lock()
                .await
                .contains(&original_peer_id),
            "retry must publish the refreshed private route"
        );
    }

    /// T24 rollback twin: a trust-publication failure after the DSL commit
    /// rolls the staged bind back to Unbound and surfaces a typed Commit
    /// error.
    #[tokio::test]
    async fn bind_supervisor_for_materialized_session_rolls_back_on_publication_failure() {
        let supervisor = trusted_supervisor_descriptor(0xbb);
        let mut runtime_impl = bootstrap_runtime(
            PEER_ID_RECEIVER,
            "inproc://receiver",
            Some("expected-token"),
        );
        runtime_impl
            .add_trusted_peer_errors
            .insert(supervisor.peer_id.to_string(), "boom".to_string());
        let trusted_peer_ids = runtime_impl.trusted_peer_ids.clone();
        let runtime: Arc<dyn CommsRuntime> = Arc::new(runtime_impl);
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let error = bind_supervisor_for_materialized_session(
            &adapter,
            &session_id,
            runtime.as_ref(),
            &supervisor,
            1,
        )
        .await
        .expect_err("trust publication failure must fail the bind");
        assert!(
            matches!(error, SupervisorMaterializeBindError::Commit(_)),
            "expected a Commit error, got {error:?}"
        );
        assert!(
            matches!(
                adapter.supervisor_binding(&session_id).await,
                SupervisorBinding::Unbound
            ),
            "a failed publication must roll the staged bind back to Unbound"
        );
        assert!(
            trusted_peer_ids.lock().await.is_empty(),
            "no trust edge may survive the rollback"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 6b — member live serving arms (T-L15..T-L19 + ADJ-P6B-9 pin)
    // -----------------------------------------------------------------------

    use crate::member_live::{MemberLiveError, MemberLiveHost, MemberLiveStatus};
    use meerkat_contracts::wire::supervisor_bridge::{
        BridgeLiveChannelPayload, BridgeLiveControlOutcome, BridgeLiveControlPayload,
        BridgeLiveControlVerb, BridgeLiveOpenPayload, BridgeLiveStatusPayload,
    };
    use meerkat_contracts::{
        LiveCloseStatus, LiveCommitInputStatus, LiveInterruptStatus, LiveOpenResult,
        LiveOpenTransport, LiveRefreshStatus, LiveTruncateStatus, RealtimeTurningMode,
        WireLiveAdapterStatus, WireLiveContinuityMode, WireLiveTransportBootstrap,
    };

    /// ADJ-P6B-9 pin: NO live boolean/endpoint ever rides the member-drain
    /// `bridge_capabilities()` report. The member-addressed live arms plus
    /// the `host_live_endpoints` bind fact are the whole capability truth
    /// (DL5 — a member-report boolean would be dual truth).
    #[test]
    fn member_bridge_capabilities_stay_live_free() {}

    #[derive(Debug, Clone, PartialEq)]
    enum ScriptedLiveCall {
        Open {
            session: SessionId,
            turning_mode: Option<RealtimeTurningMode>,
            transport: Option<LiveOpenTransport>,
        },
        Close {
            session: SessionId,
            channel_id: String,
        },
        Status {
            session: SessionId,
            channel_id: Option<String>,
        },
        Control {
            session: SessionId,
            channel_id: String,
            verb: BridgeLiveControlVerb,
        },
    }

    fn scripted_open_result() -> LiveOpenResult {
        LiveOpenResult {
            channel_id: "chan-scripted".to_string(),
            transport: WireLiveTransportBootstrap::Websocket {
                url: "wss://advertised.example:8443/live/ws?token=tok-1&channel=chan-scripted&format=pcm_24k_mono"
                    .to_string(),
                token: "tok-1".to_string(),
            },
            capabilities: meerkat_core::live_adapter::LiveChannelCapabilities::default().into(),
            continuity: WireLiveContinuityMode::Fresh,
        }
    }

    /// Scripted `MemberLiveHost` fake: records every reach and answers
    /// canned typed values. `open_gate` (when armed) parks the open until
    /// the test releases a permit — the T-L18 detachment probe.
    #[derive(Default)]
    struct ScriptedMemberLive {
        calls: std::sync::Mutex<Vec<ScriptedLiveCall>>,
        open_gate: Option<Arc<tokio::sync::Semaphore>>,
    }

    impl ScriptedMemberLive {
        fn recorded(&self) -> Vec<ScriptedLiveCall> {
            self.calls.lock().expect("scripted live calls lock").clone()
        }

        fn record(&self, call: ScriptedLiveCall) {
            self.calls
                .lock()
                .expect("scripted live calls lock")
                .push(call);
        }
    }

    #[async_trait::async_trait]
    impl MemberLiveHost for ScriptedMemberLive {
        async fn open(
            &self,
            session: &SessionId,
            turning_mode: Option<RealtimeTurningMode>,
            transport: Option<LiveOpenTransport>,
        ) -> Result<LiveOpenResult, MemberLiveError> {
            self.record(ScriptedLiveCall::Open {
                session: session.clone(),
                turning_mode,
                transport,
            });
            if let Some(gate) = &self.open_gate {
                let _permit = gate.acquire().await.expect("open gate semaphore");
            }
            Ok(scripted_open_result())
        }

        async fn close(
            &self,
            session: &SessionId,
            channel_id: &str,
        ) -> Result<LiveCloseStatus, MemberLiveError> {
            self.record(ScriptedLiveCall::Close {
                session: session.clone(),
                channel_id: channel_id.to_string(),
            });
            Ok(LiveCloseStatus::Closed)
        }

        async fn status(
            &self,
            session: &SessionId,
            channel_id: Option<String>,
        ) -> Result<MemberLiveStatus, MemberLiveError> {
            self.record(ScriptedLiveCall::Status {
                session: session.clone(),
                channel_id: channel_id.clone(),
            });
            Ok(MemberLiveStatus {
                channel_id: channel_id.unwrap_or_else(|| "chan-scripted".to_string()),
                status: WireLiveAdapterStatus::Ready,
            })
        }

        async fn control(
            &self,
            session: &SessionId,
            channel_id: &str,
            verb: BridgeLiveControlVerb,
        ) -> Result<BridgeLiveControlOutcome, MemberLiveError> {
            self.record(ScriptedLiveCall::Control {
                session: session.clone(),
                channel_id: channel_id.to_string(),
                verb: verb.clone(),
            });
            Ok(match verb {
                BridgeLiveControlVerb::CommitInput => BridgeLiveControlOutcome::CommitInput {
                    status: LiveCommitInputStatus::Committed,
                },
                BridgeLiveControlVerb::Interrupt => BridgeLiveControlOutcome::Interrupt {
                    status: LiveInterruptStatus::Interrupted,
                },
                BridgeLiveControlVerb::Truncate { .. } => BridgeLiveControlOutcome::Truncate {
                    status: LiveTruncateStatus::Truncated,
                },
                BridgeLiveControlVerb::Refresh => BridgeLiveControlOutcome::Refresh {
                    status: LiveRefreshStatus::Queued,
                },
            })
        }
    }

    struct LiveArmFixture {
        adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
        member_runtime: Arc<meerkat_comms::CommsRuntime>,
        supervisor_runtime: Arc<meerkat_comms::CommsRuntime>,
        supervisor_spec: TrustedPeerDescriptor,
        incarnation: BridgeMemberIncarnation,
    }

    impl LiveArmFixture {
        /// Two TCP comms runtimes + a machine with a pre-staged supervisor
        /// bind at epoch 1 — the `authorized_bridge_command_repairs_...`
        /// harness shape, reused for the live arms.
        async fn bound(label: &str) -> Self {
            let suffix = Uuid::new_v4().simple().to_string();
            let member_name = format!("live-{label}-member-{suffix}");
            let supervisor_name = format!("live-{label}-supervisor-{suffix}");

            let mut member_runtime =
                meerkat_comms::CommsRuntime::inproc_only(&member_name).expect("member runtime");
            member_runtime
                .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    0,
                )))
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

            let mut supervisor_runtime = meerkat_comms::CommsRuntime::inproc_only(&supervisor_name)
                .expect("supervisor runtime");
            supervisor_runtime
                .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    0,
                )))
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
            let supervisor_dsl =
                install_running_test_peer_comms_handle(supervisor_runtime.as_ref());
            add_test_projection_trust_with_dsl(
                supervisor_runtime.as_ref(),
                Arc::clone(&supervisor_dsl),
                member_spec.clone(),
                1,
                "supervisor trusts live member target",
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
                .test_install_session_peer_comms_handle_on_runtime(
                    &session_id,
                    member_runtime.as_ref(),
                )
                .await
                .expect("install adapter session peer-comms handle on member runtime");
            // Production members hold the supervisor route from
            // materialization — failure-path rejects (stale epoch) reply
            // over it without the success path's just-in-time ensure.
            add_member_private_supervisor_trust_via_adapter(
                &adapter,
                &session_id,
                &member_runtime,
                &supervisor_spec,
                1,
                "member starts with supervisor private route",
            )
            .await;
            let incarnation = BridgeMemberIncarnation {
                mob_id: "mob".to_string(),
                agent_identity: format!("live-{label}-member"),
                host_id: "host-a".to_string(),
                binding_generation: 1,
                member_session_id: session_id.to_string(),
                generation: 1,
                fence_token: 1,
            };
            register_member_incarnation(&adapter, session_id.clone(), incarnation.clone())
                .await
                .expect("register live fixture member incarnation");

            Self {
                adapter,
                session_id,
                member_runtime,
                supervisor_runtime,
                supervisor_spec,
                incarnation,
            }
        }

        fn candidate_for(&self, command: &BridgeCommand) -> PeerInputCandidate {
            let interaction_id = InteractionId(Uuid::new_v4());
            let ingress = PeerIngressFact::peer(
                interaction_id,
                PeerInputClass::ActionableRequest,
                meerkat_core::PeerIngressKind::Request,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                PeerIngressIdentity::new(
                    self.supervisor_spec.peer_id,
                    self.supervisor_spec.name.as_str(),
                    meerkat_core::PeerIngressConvention::Request {
                        request_id: interaction_id.to_string(),
                        intent: SUPERVISOR_BRIDGE_INTENT.to_string(),
                    },
                )
                .with_signing_pubkey(self.supervisor_spec.pubkey),
            );
            bridge_candidate_with_ingress(self.supervisor_spec.name.as_str(), command, ingress)
        }

        fn supervisor_payload(&self) -> (BridgePeerSpec, u64, BridgeProtocolVersion) {
            (
                BridgePeerSpec::from(self.supervisor_spec.clone()),
                1,
                SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            )
        }

        async fn serve(&self, command: &BridgeCommand) {
            let candidate = self.candidate_for(command);
            assert!(
                try_handle_supervisor_bridge_command(
                    &self.adapter,
                    &self.session_id,
                    &(Arc::clone(&self.member_runtime) as Arc<dyn CommsRuntime>),
                    &candidate,
                )
                .await,
                "bridge handler must own the live command"
            );
        }

        async fn next_reply(&self, label: &str) -> BridgeReply {
            let response = drain_one_candidate(&self.supervisor_runtime, label).await;
            let InteractionContent::Response { result, .. } = response.interaction.content else {
                panic!("expected bridge response for {label}");
            };
            serde_json::from_value::<BridgeReply>(result).expect("typed bridge reply")
        }
    }

    /// T-L17: all four verbs round-trip payload → trait → reply, with
    /// `turning_mode: None` passthrough, the verb enum mapping, the
    /// `status(None)` active-channel probe shape (ADJ-P6B-2), and the
    /// session pin argument = the drain's bound member session.
    #[tokio::test]
    async fn member_live_verbs_round_trip_payload_to_trait_to_reply() {
        let fixture = LiveArmFixture::bound("roundtrip").await;
        let live = Arc::new(ScriptedMemberLive::default());
        fixture
            .adapter
            .set_member_live_host(Arc::clone(&live) as Arc<dyn MemberLiveHost>);
        let (supervisor, epoch, protocol_version) = fixture.supervisor_payload();

        fixture
            .serve(&BridgeCommand::OpenMemberLiveChannel(
                BridgeLiveOpenPayload {
                    supervisor: supervisor.clone(),
                    epoch,
                    protocol_version,
                    expected_member: fixture.incarnation.clone(),
                    turning_mode: None,
                    transport: None,
                },
            ))
            .await;
        match fixture.next_reply("open reply").await {
            BridgeReply::MemberLiveChannelOpened(response) => {
                assert_eq!(
                    response.open,
                    scripted_open_result(),
                    "LiveOpenResult must embed VERBATIM (gotcha #14): URL + token untouched"
                );
            }
            other => panic!("expected MemberLiveChannelOpened, got {other:?}"),
        }

        fixture
            .serve(&BridgeCommand::MemberLiveChannelStatus(
                BridgeLiveStatusPayload {
                    supervisor: supervisor.clone(),
                    epoch,
                    protocol_version,
                    expected_member: fixture.incarnation.clone(),
                    channel_id: None,
                },
            ))
            .await;
        match fixture.next_reply("status reply").await {
            BridgeReply::MemberLiveChannelStatusReport { channel_id, status } => {
                assert_eq!(channel_id, "chan-scripted");
                assert_eq!(status, WireLiveAdapterStatus::Ready);
            }
            other => panic!("expected MemberLiveChannelStatusReport, got {other:?}"),
        }

        fixture
            .serve(&BridgeCommand::ControlMemberLiveChannel(
                BridgeLiveControlPayload {
                    supervisor: supervisor.clone(),
                    epoch,
                    protocol_version,
                    expected_member: fixture.incarnation.clone(),
                    channel_id: "chan-scripted".to_string(),
                    verb: BridgeLiveControlVerb::CommitInput,
                },
            ))
            .await;
        match fixture.next_reply("control reply").await {
            BridgeReply::MemberLiveChannelControlled(response) => {
                assert_eq!(
                    response.outcome,
                    BridgeLiveControlOutcome::CommitInput {
                        status: LiveCommitInputStatus::Committed,
                    }
                );
            }
            other => panic!("expected MemberLiveChannelControlled, got {other:?}"),
        }

        fixture
            .serve(&BridgeCommand::CloseMemberLiveChannel(
                BridgeLiveChannelPayload {
                    supervisor,
                    epoch,
                    protocol_version,
                    expected_member: fixture.incarnation.clone(),
                    channel_id: "chan-scripted".to_string(),
                },
            ))
            .await;
        match fixture.next_reply("close reply").await {
            BridgeReply::MemberLiveChannelClosed { status } => {
                assert_eq!(status, LiveCloseStatus::Closed);
            }
            other => panic!("expected MemberLiveChannelClosed, got {other:?}"),
        }

        let calls = live.recorded();
        assert_eq!(
            calls,
            vec![
                ScriptedLiveCall::Open {
                    session: fixture.session_id.clone(),
                    turning_mode: None,
                    transport: None,
                },
                ScriptedLiveCall::Status {
                    session: fixture.session_id.clone(),
                    channel_id: None,
                },
                ScriptedLiveCall::Control {
                    session: fixture.session_id.clone(),
                    channel_id: "chan-scripted".to_string(),
                    verb: BridgeLiveControlVerb::CommitInput,
                },
                ScriptedLiveCall::Close {
                    session: fixture.session_id.clone(),
                    channel_id: "chan-scripted".to_string(),
                },
            ],
            "every arm must reach the trait with the drain's bound member session"
        );
    }

    /// T-L15: an unauthorized supervisor (no bound authority) is rejected
    /// typed BEFORE the live host is reached.
    #[tokio::test]
    async fn unauthorized_supervisor_never_reaches_member_live_host() {
        let fixture = LiveArmFixture::bound("unauthorized").await;
        let live = Arc::new(ScriptedMemberLive::default());
        fixture
            .adapter
            .set_member_live_host(Arc::clone(&live) as Arc<dyn MemberLiveHost>);
        let (supervisor, _epoch, protocol_version) = fixture.supervisor_payload();

        // Stale epoch: the bound authority is epoch 1; epoch 0 must reject.
        fixture
            .serve(&BridgeCommand::CloseMemberLiveChannel(
                BridgeLiveChannelPayload {
                    supervisor,
                    epoch: 0,
                    protocol_version,
                    expected_member: fixture.incarnation.clone(),
                    channel_id: "chan-any".to_string(),
                },
            ))
            .await;
        match fixture.next_reply("unauthorized close reply").await {
            BridgeReply::Rejected { .. } => {}
            other => panic!("expected typed rejection, got {other:?}"),
        }
        assert!(
            live.recorded().is_empty(),
            "an unauthorized command must never reach the member live host"
        );
    }

    /// T-L16: absent slot ⇒ typed `LiveTransportUnavailable` — the
    /// fail-closed backstop behind the controlling host's
    /// `host_live_endpoints` gate (§16.6 row 1).
    #[tokio::test]
    async fn absent_member_live_slot_rejects_live_transport_unavailable() {
        let fixture = LiveArmFixture::bound("absent-slot").await;
        let (supervisor, epoch, protocol_version) = fixture.supervisor_payload();

        fixture
            .serve(&BridgeCommand::CloseMemberLiveChannel(
                BridgeLiveChannelPayload {
                    supervisor,
                    epoch,
                    protocol_version,
                    expected_member: fixture.incarnation.clone(),
                    channel_id: "chan-any".to_string(),
                },
            ))
            .await;
        match fixture.next_reply("absent-slot close reply").await {
            BridgeReply::Rejected { cause, reason } => {
                assert_eq!(cause, BridgeRejectionCause::LiveTransportUnavailable);
                assert_eq!(reason, "member host serves no live substrate");
            }
            other => panic!("expected typed rejection, got {other:?}"),
        }
    }

    /// T-L16 (helper form): the resolver itself produces the typed absent
    /// rejection without touching any machine state.
    #[test]
    fn resolve_member_live_host_absent_is_live_transport_unavailable() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let Err((cause, reason)) = resolve_member_live_host(&adapter) else {
            panic!("empty slot must reject");
        };
        assert_eq!(cause, BridgeRejectionCause::LiveTransportUnavailable);
        assert_eq!(reason, "member host serves no live substrate");
    }

    /// T-L19: every `MemberLiveError` variant projects onto its §3 cause
    /// through the ONE shared conversion; the drain adds only the display
    /// reason. (The exhaustive per-variant cause table is pinned beside the
    /// conversion impl in `member_live.rs`; this row pins the drain seam.)
    #[test]
    fn member_live_error_rejection_uses_shared_conversion_and_display_reason() {
        let error = MemberLiveError::TransportUnsupported {
            requested: "webrtc".to_string(),
        };
        let (cause, reason) = member_live_error_to_bridge_rejection(&error);
        assert_eq!(cause, error.to_bridge_rejection());
        assert_eq!(reason, error.to_string());

        let internal = MemberLiveError::Internal {
            reason: "invariant".to_string(),
        };
        let (cause, reason) = member_live_error_to_bridge_rejection(&internal);
        assert_eq!(cause, BridgeRejectionCause::Internal);
        assert_eq!(reason, "member live internal fault: invariant");
    }

    /// T-L18: `OpenMemberLiveChannel` serves on a DETACHED responder task —
    /// a slow scripted open must not delay a subsequent inline status on
    /// the same drain (the PollMemberEvents precedent).
    #[tokio::test]
    async fn slow_member_live_open_does_not_block_inline_status() {
        let fixture = LiveArmFixture::bound("detached-open").await;
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let live = Arc::new(ScriptedMemberLive {
            calls: std::sync::Mutex::new(Vec::new()),
            open_gate: Some(Arc::clone(&gate)),
        });
        fixture
            .adapter
            .set_member_live_host(Arc::clone(&live) as Arc<dyn MemberLiveHost>);
        let (supervisor, epoch, protocol_version) = fixture.supervisor_payload();

        // The open arm must RETURN while the scripted open is still parked
        // on the gate; the reach+reply continues on the detached task.
        fixture
            .serve(&BridgeCommand::OpenMemberLiveChannel(
                BridgeLiveOpenPayload {
                    supervisor: supervisor.clone(),
                    epoch,
                    protocol_version,
                    expected_member: fixture.incarnation.clone(),
                    turning_mode: None,
                    transport: None,
                },
            ))
            .await;

        // A subsequent status on the SAME drain serves inline and replies
        // while the open is still gated.
        fixture
            .serve(&BridgeCommand::MemberLiveChannelStatus(
                BridgeLiveStatusPayload {
                    supervisor,
                    epoch,
                    protocol_version,
                    expected_member: fixture.incarnation.clone(),
                    channel_id: None,
                },
            ))
            .await;
        match fixture.next_reply("status while open parked").await {
            BridgeReply::MemberLiveChannelStatusReport { .. } => {}
            other => panic!("expected status report before the gated open reply, got {other:?}"),
        }

        // Release the gate: the detached open completes and replies.
        gate.add_permits(1);
        match fixture.next_reply("gated open reply").await {
            BridgeReply::MemberLiveChannelOpened(response) => {
                assert_eq!(response.open, scripted_open_result());
            }
            other => panic!("expected MemberLiveChannelOpened, got {other:?}"),
        }
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
        let delivered_terminal = if let Some(handle) = handle {
            match handle.try_wait().await {
                Ok(outcome) => {
                    if let Some(tx) = subscriber {
                        let event = interaction_terminal_event(interaction_id, outcome);
                        match crate::tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            tx.send(event),
                        )
                        .await
                        {
                            Ok(Ok(())) => true,
                            Ok(Err(_)) => {
                                tracing::warn!(
                                    %interaction_id,
                                    "completion bridge could not deliver terminal event: subscriber closed"
                                );
                                false
                            }
                            Err(_) => {
                                tracing::warn!(
                                    %interaction_id,
                                    "completion bridge could not deliver terminal event: subscriber send timed out after 5s"
                                );
                                false
                            }
                        }
                    } else {
                        tracing::warn!(
                            %interaction_id,
                            "completion bridge has no interaction subscriber for terminal event"
                        );
                        false
                    }
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

        if !delivered_terminal && let Some(runtime) = comms_runtime {
            runtime.abandon_interaction_stream(
                &interaction_id,
                meerkat_core::InteractionStreamAbandonReason::TerminalDeliveryFailed,
            );
        }
    });
}
