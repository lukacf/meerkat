use meerkat_core::{
    HookObservation, HookPeerIngressCommitted, HookRuntimeInputAccepted,
    HookRuntimeInputDeduplicated, HookRuntimeInputKind, HookRuntimeInputRejected,
    HookRuntimeInputRejection, HookRuntimeState, PostCommitHookDispatcher,
};

use crate::accept::{AcceptOutcome, RejectReason, handling_mode_from_policy};
use crate::identifiers::InputKind;
use crate::input::{Input, InputOrigin, PeerConvention};
use crate::runtime_state::RuntimeState;
use crate::traits::RuntimeDriverError;

pub(crate) struct RuntimeInputHookFacts {
    input_id: meerkat_core::lifecycle::InputId,
    input_kind: HookRuntimeInputKind,
    peer_ingress: Option<HookPeerIngressCommitted>,
}

impl RuntimeInputHookFacts {
    pub(crate) fn from_input(input: &Input) -> Self {
        Self {
            input_id: input.id().clone(),
            input_kind: input.kind().into(),
            peer_ingress: peer_ingress_observation(input),
        }
    }
}

fn peer_ingress_observation(input: &Input) -> Option<HookPeerIngressCommitted> {
    let Input::Peer(peer_input) = input else {
        return None;
    };
    let InputOrigin::Peer {
        peer_id,
        display_identity,
        ..
    } = &peer_input.header.source
    else {
        return None;
    };
    let peer = match meerkat_core::comms::PeerId::parse(peer_id) {
        Ok(id) => Some(meerkat_core::types::SystemNoticePeer {
            id,
            display_name: display_identity.clone(),
        }),
        Err(error) => {
            tracing::warn!(
                %peer_id,
                %error,
                "committed peer ingress carried no parseable canonical peer identity"
            );
            None
        }
    };
    let (kind, request_id) = match &peer_input.convention {
        Some(PeerConvention::Request { request_id, .. }) => (
            meerkat_core::types::CommsNoticeKind::Request,
            Some(request_id.clone()),
        ),
        Some(PeerConvention::ResponseProgress { request_id, .. }) => (
            meerkat_core::types::CommsNoticeKind::ResponseProgress,
            Some(request_id.clone()),
        ),
        Some(PeerConvention::ResponseTerminal { request_id, .. }) => (
            meerkat_core::types::CommsNoticeKind::ResponseTerminal,
            Some(request_id.clone()),
        ),
        Some(PeerConvention::Message) | None => {
            (meerkat_core::types::CommsNoticeKind::Message, None)
        }
    };
    Some(HookPeerIngressCommitted {
        kind,
        peer,
        request_id,
        sender_taint: peer_input.sender_taint,
    })
}

impl From<InputKind> for HookRuntimeInputKind {
    fn from(kind: InputKind) -> Self {
        match kind {
            InputKind::Prompt => Self::Prompt,
            InputKind::PeerMessage => Self::PeerMessage,
            InputKind::PeerRequest => Self::PeerRequest,
            InputKind::PeerResponseProgress => Self::PeerResponseProgress,
            InputKind::PeerResponseTerminal => Self::PeerResponseTerminal,
            InputKind::FlowStep => Self::FlowStep,
            InputKind::ExternalEvent => Self::ExternalEvent,
            InputKind::Continuation => Self::Continuation,
            InputKind::Operation => Self::Operation,
        }
    }
}

impl From<RuntimeState> for HookRuntimeState {
    fn from(state: RuntimeState) -> Self {
        match state {
            RuntimeState::Initializing => Self::Initializing,
            RuntimeState::Idle => Self::Idle,
            RuntimeState::Attached => Self::Attached,
            RuntimeState::Running => Self::Running,
            RuntimeState::Retired => Self::Retired,
            RuntimeState::Stopped => Self::Stopped,
            RuntimeState::Destroyed => Self::Destroyed,
        }
    }
}

impl From<&RejectReason> for HookRuntimeInputRejection {
    fn from(reason: &RejectReason) -> Self {
        match reason {
            RejectReason::NotReady { state } => Self::NotReady {
                state: (*state).into(),
            },
            RejectReason::DurabilityViolation { detail } => Self::DurabilityViolation {
                detail: detail.clone(),
            },
            RejectReason::PeerHandlingModeInvalid { detail } => Self::PeerHandlingModeInvalid {
                detail: detail.clone(),
            },
            RejectReason::PeerResponseTerminalInvalid { detail } => {
                Self::PeerResponseTerminalInvalid {
                    detail: detail.clone(),
                }
            }
        }
    }
}

pub(crate) fn dispatch_runtime_input_outcome(
    dispatcher: &PostCommitHookDispatcher,
    input: &RuntimeInputHookFacts,
    outcome: &AcceptOutcome,
) {
    let observation = match outcome {
        AcceptOutcome::Accepted {
            input_id, policy, ..
        } => HookObservation::RuntimeInputAccepted(HookRuntimeInputAccepted {
            input_id: input_id.clone(),
            input_kind: input.input_kind,
            handling_mode: handling_mode_from_policy(policy),
        }),
        AcceptOutcome::Deduplicated {
            input_id,
            existing_id,
            ..
        } => HookObservation::RuntimeInputDeduplicated(HookRuntimeInputDeduplicated {
            input_id: input_id.clone(),
            input_kind: input.input_kind,
            existing_input_id: existing_id.clone(),
        }),
        AcceptOutcome::Rejected { reason } => {
            HookObservation::RuntimeInputRejected(HookRuntimeInputRejected {
                input_id: input.input_id.clone(),
                input_kind: input.input_kind,
                reason: reason.into(),
            })
        }
    };
    let accepted = matches!(outcome, AcceptOutcome::Accepted { .. });
    dispatcher.dispatch(observation);
    if accepted && let Some(peer_ingress) = input.peer_ingress.clone() {
        dispatcher.dispatch(HookObservation::PeerIngressCommitted(peer_ingress));
    }
}

pub(crate) fn dispatch_runtime_input_error(
    dispatcher: &PostCommitHookDispatcher,
    input: &RuntimeInputHookFacts,
    error: &RuntimeDriverError,
) {
    let reason = match error {
        RuntimeDriverError::NotReady { state } => HookRuntimeInputRejection::NotReady {
            state: (*state).into(),
        },
        RuntimeDriverError::Destroyed => HookRuntimeInputRejection::NotReady {
            state: HookRuntimeState::Destroyed,
        },
        RuntimeDriverError::ValidationFailed { reason } => {
            HookRuntimeInputRejection::ValidationFailed {
                detail: reason.clone(),
            }
        }
        _ => return,
    };
    dispatcher.dispatch(HookObservation::RuntimeInputRejected(
        HookRuntimeInputRejected {
            input_id: input.input_id.clone(),
            input_kind: input.input_kind,
            reason,
        },
    ));
}
