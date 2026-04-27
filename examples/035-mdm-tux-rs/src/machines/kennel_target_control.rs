//! Kennel-side per-target control composition.
//!
//! Owns the control-plane seam around a target record:
//! - kennel control socket connected / disconnected
//! - registration / re-registration
//! - routing all lease lifecycle events through the leaf lease machine
//!
//! The kennel binary should not infer disconnect or re-register semantics on
//! its own. It feeds typed events here and executes the returned effects.

use std::fmt;

use crate::LeaseTerminationReason;

use super::kennel_lease;

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub connected: bool,
    pub lease: kennel_lease::State,
}

impl State {
    pub fn available(target_id: String) -> Self {
        Self {
            connected: true,
            lease: kennel_lease::State::Available { target_id },
        }
    }

    pub fn target_id(&self) -> &str {
        match &self.lease {
            kennel_lease::State::Available { target_id }
            | kennel_lease::State::AwaitingAck { target_id, .. }
            | kennel_lease::State::Claimed { target_id, .. }
            | kennel_lease::State::RecoveringClaim { target_id, .. } => target_id,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Registered {
        attached_tux_id: Option<String>,
        now_ms: i64,
        recovery_window_ms: i64,
    },
    ClaimRequested {
        target_id: String,
        lease_id: String,
        tux_id: String,
        expires_at_ms: i64,
        ack_deadline_ms: i64,
    },
    ClaimAcked {
        lease_id: String,
        tux_id: String,
    },
    LeaseRenewed {
        lease_id: String,
        tux_id: String,
        new_expires_at_ms: i64,
    },
    Released {
        reason: LeaseTerminationReason,
    },
    Rebound {
        new_lease_id: String,
        tux_id: String,
        new_expires_at_ms: i64,
    },
    Tick {
        now_ms: i64,
    },
    TargetDisconnected {
        now_ms: i64,
        recovery_window_ms: i64,
    },
    TuxDisconnected {
        tux_id: String,
        now_ms: i64,
        recovery_window_ms: i64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Lease(kennel_lease::Effect),
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
    next: kennel_lease::State,
    connected: bool,
    effects: Vec<kennel_lease::Effect>,
) -> (State, Vec<Effect>) {
    (
        State {
            connected,
            lease: next,
        },
        effects.into_iter().map(Effect::Lease).collect(),
    )
}

pub fn transition(state: State, event: Event) -> Result<(State, Vec<Effect>), TransitionError> {
    let State { connected, lease } = state;
    match event {
        Event::Registered {
            attached_tux_id,
            now_ms,
            recovery_window_ms,
        } => {
            let connected = true;
            if let Some(tux_id) = attached_tux_id {
                let target_id = match &lease {
                    kennel_lease::State::Available { target_id }
                    | kennel_lease::State::AwaitingAck { target_id, .. }
                    | kennel_lease::State::Claimed { target_id, .. }
                    | kennel_lease::State::RecoveringClaim { target_id, .. } => target_id.clone(),
                };
                let (next, effects) = kennel_lease::transition(
                    lease,
                    kennel_lease::Event::TargetReregisteredWithAttachment {
                        target_id,
                        tux_id,
                        now_ms,
                        recovery_window_ms,
                    },
                )
                .map_err(|e| err("State", "Registered", &e.reason))?;
                Ok(lift(next, connected, effects))
            } else {
                Ok((State { connected, lease }, vec![]))
            }
        }
        Event::ClaimRequested {
            target_id,
            lease_id,
            tux_id,
            expires_at_ms,
            ack_deadline_ms,
        } => {
            let (next, effects) = kennel_lease::transition(
                lease,
                kennel_lease::Event::ClaimRequested {
                    target_id,
                    lease_id,
                    tux_id,
                    expires_at_ms,
                    ack_deadline_ms,
                },
            )
            .map_err(|e| err("State", "ClaimRequested", &e.reason))?;
            Ok(lift(next, connected, effects))
        }
        Event::ClaimAcked { lease_id, tux_id } => {
            let (next, effects) = kennel_lease::transition(
                lease,
                kennel_lease::Event::ClaimAcked { lease_id, tux_id },
            )
            .map_err(|e| err("State", "ClaimAcked", &e.reason))?;
            Ok(lift(next, connected, effects))
        }
        Event::LeaseRenewed {
            lease_id,
            tux_id,
            new_expires_at_ms,
        } => {
            let (next, effects) = kennel_lease::transition(
                lease,
                kennel_lease::Event::LeaseRenewed {
                    lease_id,
                    tux_id,
                    new_expires_at_ms,
                },
            )
            .map_err(|e| err("State", "LeaseRenewed", &e.reason))?;
            Ok(lift(next, connected, effects))
        }
        Event::Released { reason } => {
            let (next, effects) =
                kennel_lease::transition(lease, kennel_lease::Event::Released { reason })
                    .map_err(|e| err("State", "Released", &e.reason))?;
            Ok(lift(next, connected, effects))
        }
        Event::Rebound {
            new_lease_id,
            tux_id,
            new_expires_at_ms,
        } => {
            let (next, effects) = kennel_lease::transition(
                lease,
                kennel_lease::Event::Rebound {
                    new_lease_id,
                    tux_id,
                    new_expires_at_ms,
                },
            )
            .map_err(|e| err("State", "Rebound", &e.reason))?;
            Ok(lift(next, connected, effects))
        }
        Event::Tick { now_ms } => {
            let (next, effects) =
                kennel_lease::transition(lease, kennel_lease::Event::Tick { now_ms })
                    .map_err(|e| err("State", "Tick", &e.reason))?;
            Ok(lift(next, connected, effects))
        }
        Event::TargetDisconnected {
            now_ms,
            recovery_window_ms,
        } => {
            let (next, effects) = kennel_lease::transition(
                lease,
                kennel_lease::Event::TargetDisconnected {
                    now_ms,
                    recovery_window_ms,
                },
            )
            .map_err(|e| err("State", "TargetDisconnected", &e.reason))?;
            Ok(lift(next, false, effects))
        }
        Event::TuxDisconnected {
            tux_id,
            now_ms,
            recovery_window_ms,
        } => {
            let (next, effects) = kennel_lease::transition(
                lease,
                kennel_lease::Event::TuxDisconnected {
                    tux_id,
                    now_ms,
                    recovery_window_ms,
                },
            )
            .map_err(|e| err("State", "TuxDisconnected", &e.reason))?;
            Ok(lift(next, connected, effects))
        }
    }
}
