//! Target-side kennel/direct control composition.
//!
//! Wraps the attachment leaf machine and makes loop-exit / re-register intent
//! explicit so the binary cannot skip semantically required effects.

use std::fmt;

use crate::LeaseRef;

use super::target_attachment;

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub attachment: target_attachment::State,
}

impl State {
    pub fn idle() -> Self {
        Self {
            attachment: target_attachment::State::Idle,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Adopted {
        target_id: String,
        lease_id: String,
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
    },
    LeaseRebound {
        target_id: String,
        lease_id: String,
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
    },
    DirectAttachAck {
        lease_id: String,
    },
    KennelReleased {
        lease_ref: LeaseRef,
    },
    KennelDisconnected,
    KennelHeartbeatFailed,
    DirectLinkLost,
    AttachSendFailed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Attachment(target_attachment::Effect),
    ReturnToRegisterLoop,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransitionError {
    pub state: &'static str,
    pub event: String,
    pub reason: String,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid transition: {} in state {} ({})",
            self.event, self.state, self.reason
        )
    }
}

fn err(state: &'static str, event: &str, reason: &str) -> TransitionError {
    TransitionError {
        state,
        event: event.to_string(),
        reason: reason.to_string(),
    }
}

fn lift(
    attachment: target_attachment::State,
    effects: Vec<target_attachment::Effect>,
) -> (State, Vec<Effect>) {
    let mut out: Vec<Effect> = effects.into_iter().map(Effect::Attachment).collect();
    if attachment.is_terminal() {
        out.push(Effect::ReturnToRegisterLoop);
    }
    (State { attachment }, out)
}

pub fn transition(state: State, event: Event) -> Result<(State, Vec<Effect>), TransitionError> {
    let attachment_event = match event {
        Event::Adopted {
            target_id,
            lease_id,
            tux_id,
            tux_pubkey,
            tux_direct_addr,
        } => target_attachment::Event::Adopted {
            target_id,
            lease_id,
            tux_id,
            tux_pubkey,
            tux_direct_addr,
        },
        Event::LeaseRebound {
            target_id,
            lease_id,
            tux_id,
            tux_pubkey,
            tux_direct_addr,
        } => target_attachment::Event::LeaseRebound {
            target_id,
            lease_id,
            tux_id,
            tux_pubkey,
            tux_direct_addr,
        },
        Event::DirectAttachAck { lease_id } => {
            target_attachment::Event::DirectAttachAck { lease_id }
        }
        Event::KennelReleased { lease_ref } => {
            target_attachment::Event::KennelReleased { lease_ref }
        }
        Event::KennelDisconnected => target_attachment::Event::KennelDisconnected,
        Event::KennelHeartbeatFailed => target_attachment::Event::KennelHeartbeatFailed,
        Event::DirectLinkLost => target_attachment::Event::DirectLinkLost,
        Event::AttachSendFailed => target_attachment::Event::AttachSendFailed,
    };

    let (next, effects) = target_attachment::transition(state.attachment, attachment_event)
        .map_err(|e| err("State", "TargetKennelControl", &e.reason))?;
    Ok(lift(next, effects))
}
