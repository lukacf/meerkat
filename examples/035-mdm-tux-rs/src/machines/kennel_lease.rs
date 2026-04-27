//! Kennel lease state machine — per-target lifecycle on the kennel side.
//!
//! Simplified: no AwaitingAttach state, no AttachConfirmed event.
//! After ClaimAck the lease goes directly to Claimed.

use std::fmt;

use crate::{LeaseRef, LeaseTerminationReason};

#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryLease {
    PendingRebind,
    Assigned(String),
}

impl RecoveryLease {
    fn to_lease_ref(&self) -> LeaseRef {
        match self {
            RecoveryLease::PendingRebind => LeaseRef::PendingRebind,
            RecoveryLease::Assigned(lease_id) => LeaseRef::Known {
                lease_id: lease_id.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Available {
        target_id: String,
    },
    AwaitingAck {
        target_id: String,
        lease_id: String,
        tux_id: String,
        expires_at_ms: i64,
        ack_deadline_ms: i64,
    },
    Claimed {
        target_id: String,
        lease_id: String,
        tux_id: String,
        expires_at_ms: i64,
    },
    RecoveringClaim {
        target_id: String,
        lease: RecoveryLease,
        tux_id: String,
        expires_at_ms: i64,
        recover_deadline_ms: i64,
        target_connected: bool,
    },
}

#[derive(Debug, Clone)]
pub enum Event {
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
    TargetReregisteredWithAttachment {
        target_id: String,
        tux_id: String,
        now_ms: i64,
        recovery_window_ms: i64,
    },
    TargetAddressChanged,
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
    SendTargetReleased {
        target_id: String,
        lease_ref: LeaseRef,
        reason: LeaseTerminationReason,
    },
    SendClaimReleasedToTux {
        target_id: String,
        lease_ref: LeaseRef,
        tux_id: String,
        reason: LeaseTerminationReason,
    },
    SendTargetLostToTux {
        target_id: String,
        tux_id: String,
        lease_ref: LeaseRef,
    },
    SendLeaseRebound {
        target_id: String,
        lease_id: String,
        tux_id: String,
        expires_at_ms: i64,
    },
    RemoveLease {
        lease_id: String,
    },
    DropTargetRecord {
        target_id: String,
    },
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

fn recovery_deadline(expires_at_ms: i64, now_ms: i64, recovery_window_ms: i64) -> i64 {
    expires_at_ms.min(now_ms + recovery_window_ms)
}

pub fn transition(state: State, event: Event) -> Result<(State, Vec<Effect>), TransitionError> {
    match (state, event) {
        (
            State::Available { target_id },
            Event::ClaimRequested {
                target_id: claim_target_id,
                lease_id,
                tux_id,
                expires_at_ms,
                ack_deadline_ms,
            },
        ) => {
            if claim_target_id != target_id {
                return Err(err("Available", "ClaimRequested", "target_id mismatch"));
            }
            Ok((
                State::AwaitingAck {
                    target_id: claim_target_id,
                    lease_id,
                    tux_id,
                    expires_at_ms,
                    ack_deadline_ms,
                },
                vec![],
            ))
        }
        (
            State::Available {
                target_id: available_target_id,
            },
            Event::TargetReregisteredWithAttachment {
                target_id,
                tux_id,
                now_ms,
                recovery_window_ms,
            },
        ) => {
            if target_id != available_target_id {
                return Err(err(
                    "Available",
                    "TargetReregisteredWithAttachment",
                    "target_id mismatch",
                ));
            }
            let expires_at_ms = now_ms + recovery_window_ms;
            Ok((
                State::RecoveringClaim {
                    target_id,
                    lease: RecoveryLease::PendingRebind,
                    tux_id,
                    expires_at_ms,
                    recover_deadline_ms: expires_at_ms,
                    target_connected: true,
                },
                vec![],
            ))
        }
        (State::Available { target_id }, Event::TargetDisconnected { .. }) => Ok((
            State::Available {
                target_id: target_id.clone(),
            },
            vec![Effect::DropTargetRecord { target_id }],
        )),
        (State::Available { target_id }, Event::Tick { .. }) => {
            Ok((State::Available { target_id }, vec![]))
        }
        (
            State::Claimed {
                target_id: claimed_target_id,
                lease_id,
                tux_id: claimed_tux_id,
                expires_at_ms,
            },
            Event::TargetReregisteredWithAttachment {
                target_id, tux_id, ..
            },
        ) => {
            if target_id != claimed_target_id {
                return Err(err(
                    "Claimed",
                    "TargetReregisteredWithAttachment",
                    "target_id mismatch",
                ));
            }
            if tux_id != claimed_tux_id {
                return Err(err(
                    "Claimed",
                    "TargetReregisteredWithAttachment",
                    "tux_id mismatch",
                ));
            }
            Ok((
                State::Claimed {
                    target_id: claimed_target_id.clone(),
                    lease_id: lease_id.clone(),
                    tux_id: claimed_tux_id.clone(),
                    expires_at_ms,
                },
                vec![Effect::SendLeaseRebound {
                    target_id: claimed_target_id,
                    lease_id,
                    tux_id: claimed_tux_id,
                    expires_at_ms,
                }],
            ))
        }

        // AwaitingAck -> Claimed (directly, no attach window)
        (
            State::AwaitingAck {
                target_id,
                lease_id,
                tux_id,
                expires_at_ms,
                ..
            },
            Event::ClaimAcked {
                lease_id: ack_lid,
                tux_id: ack_tid,
            },
        ) => {
            if ack_lid != lease_id {
                return Err(err("AwaitingAck", "ClaimAcked", "lease_id mismatch"));
            }
            if ack_tid != tux_id {
                return Err(err("AwaitingAck", "ClaimAcked", "tux_id mismatch"));
            }
            Ok((
                State::Claimed {
                    target_id,
                    lease_id,
                    tux_id,
                    expires_at_ms,
                },
                vec![],
            ))
        }
        (
            State::AwaitingAck {
                target_id,
                lease_id,
                tux_id,
                ..
            },
            Event::Released { reason },
        ) => Ok((
            State::Available {
                target_id: target_id.clone(),
            },
            vec![
                Effect::SendTargetReleased {
                    target_id: target_id.clone(),
                    lease_ref: LeaseRef::Known {
                        lease_id: lease_id.clone(),
                    },
                    reason,
                },
                Effect::SendClaimReleasedToTux {
                    target_id,
                    lease_ref: LeaseRef::Known {
                        lease_id: lease_id.clone(),
                    },
                    tux_id,
                    reason,
                },
                Effect::RemoveLease { lease_id },
            ],
        )),
        (
            State::AwaitingAck {
                target_id,
                lease_id,
                tux_id,
                expires_at_ms,
                ack_deadline_ms,
            },
            Event::Tick { now_ms },
        ) => {
            if now_ms >= ack_deadline_ms {
                Ok((
                    State::Available {
                        target_id: target_id.clone(),
                    },
                    vec![
                        Effect::SendClaimReleasedToTux {
                            target_id,
                            lease_ref: LeaseRef::Known {
                                lease_id: lease_id.clone(),
                            },
                            tux_id,
                            reason: LeaseTerminationReason::ClaimAckTimeout,
                        },
                        Effect::RemoveLease { lease_id },
                    ],
                ))
            } else {
                Ok((
                    State::AwaitingAck {
                        target_id,
                        lease_id,
                        tux_id,
                        expires_at_ms,
                        ack_deadline_ms,
                    },
                    vec![],
                ))
            }
        }
        (
            State::AwaitingAck {
                target_id,
                lease_id,
                tux_id,
                expires_at_ms,
                ack_deadline_ms,
            },
            Event::TuxDisconnected {
                tux_id: disc_tid, ..
            },
        ) => {
            if disc_tid != tux_id {
                return Ok((
                    State::AwaitingAck {
                        target_id,
                        lease_id,
                        tux_id,
                        expires_at_ms,
                        ack_deadline_ms,
                    },
                    vec![],
                ));
            }
            Ok((
                State::Available {
                    target_id: target_id.clone(),
                },
                vec![
                    Effect::SendTargetReleased {
                        target_id,
                        lease_ref: LeaseRef::Known {
                            lease_id: lease_id.clone(),
                        },
                        reason: LeaseTerminationReason::TuxDisconnected,
                    },
                    Effect::RemoveLease { lease_id },
                ],
            ))
        }
        (
            State::AwaitingAck {
                target_id,
                lease_id,
                tux_id,
                ..
            },
            Event::TargetDisconnected { .. },
        ) => Ok((
            State::Available {
                target_id: target_id.clone(),
            },
            vec![
                Effect::SendTargetLostToTux {
                    target_id: target_id.clone(),
                    tux_id,
                    lease_ref: LeaseRef::Known {
                        lease_id: lease_id.clone(),
                    },
                },
                Effect::RemoveLease { lease_id },
                Effect::DropTargetRecord { target_id },
            ],
        )),

        (
            State::Claimed {
                target_id,
                lease_id,
                tux_id,
                ..
            },
            Event::LeaseRenewed {
                lease_id: ren_lid,
                tux_id: ren_tid,
                new_expires_at_ms,
            },
        ) => {
            if ren_lid != lease_id || ren_tid != tux_id {
                return Err(err(
                    "Claimed",
                    "LeaseRenewed",
                    "lease_id or tux_id mismatch",
                ));
            }
            Ok((
                State::Claimed {
                    target_id,
                    lease_id,
                    tux_id,
                    expires_at_ms: new_expires_at_ms,
                },
                vec![],
            ))
        }
        (
            State::Claimed {
                target_id,
                lease_id,
                tux_id,
                ..
            },
            Event::Released { reason },
        ) => Ok((
            State::Available {
                target_id: target_id.clone(),
            },
            vec![
                Effect::SendTargetReleased {
                    target_id: target_id.clone(),
                    lease_ref: LeaseRef::Known {
                        lease_id: lease_id.clone(),
                    },
                    reason,
                },
                Effect::SendClaimReleasedToTux {
                    target_id,
                    lease_ref: LeaseRef::Known {
                        lease_id: lease_id.clone(),
                    },
                    tux_id,
                    reason,
                },
                Effect::RemoveLease { lease_id },
            ],
        )),
        (
            State::Claimed {
                target_id,
                lease_id,
                tux_id,
                expires_at_ms,
            },
            Event::TuxDisconnected {
                tux_id: disc_tid,
                now_ms,
                recovery_window_ms,
            },
        ) => {
            if disc_tid != tux_id {
                return Ok((
                    State::Claimed {
                        target_id,
                        lease_id,
                        tux_id,
                        expires_at_ms,
                    },
                    vec![],
                ));
            }
            Ok((
                State::RecoveringClaim {
                    target_id,
                    lease: RecoveryLease::Assigned(lease_id),
                    tux_id,
                    expires_at_ms,
                    recover_deadline_ms: recovery_deadline(
                        expires_at_ms,
                        now_ms,
                        recovery_window_ms,
                    ),
                    target_connected: true,
                },
                vec![],
            ))
        }
        (
            State::Claimed {
                target_id,
                lease_id,
                tux_id,
                expires_at_ms,
            },
            Event::TargetDisconnected {
                now_ms,
                recovery_window_ms,
            },
        ) => Ok((
            State::RecoveringClaim {
                target_id: target_id.clone(),
                lease: RecoveryLease::Assigned(lease_id.clone()),
                tux_id: tux_id.clone(),
                expires_at_ms,
                recover_deadline_ms: recovery_deadline(expires_at_ms, now_ms, recovery_window_ms),
                target_connected: false,
            },
            vec![Effect::SendTargetLostToTux {
                target_id,
                tux_id,
                lease_ref: LeaseRef::Known { lease_id },
            }],
        )),
        (
            State::Claimed {
                target_id,
                lease_id,
                tux_id,
                expires_at_ms,
            },
            Event::Tick { .. },
        ) => Ok((
            State::Claimed {
                target_id,
                lease_id,
                tux_id,
                expires_at_ms,
            },
            vec![],
        )),

        (
            State::RecoveringClaim {
                target_id,
                lease,
                tux_id,
                expires_at_ms,
                recover_deadline_ms: _,
                target_connected: _,
            },
            Event::TargetReregisteredWithAttachment {
                target_id: ev_target_id,
                tux_id: ev_tux_id,
                now_ms,
                recovery_window_ms,
            },
        ) => {
            if ev_target_id != target_id {
                return Err(err(
                    "RecoveringClaim",
                    "TargetReregisteredWithAttachment",
                    "target_id mismatch",
                ));
            }
            if ev_tux_id != tux_id {
                return Err(err(
                    "RecoveringClaim",
                    "TargetReregisteredWithAttachment",
                    "tux_id mismatch",
                ));
            }
            let rebound_effect = match &lease {
                RecoveryLease::Assigned(lease_id) => Some(Effect::SendLeaseRebound {
                    target_id: target_id.clone(),
                    lease_id: lease_id.clone(),
                    tux_id: tux_id.clone(),
                    expires_at_ms,
                }),
                RecoveryLease::PendingRebind => None,
            };
            Ok((
                State::RecoveringClaim {
                    target_id,
                    lease,
                    tux_id,
                    expires_at_ms,
                    recover_deadline_ms: recovery_deadline(
                        expires_at_ms,
                        now_ms,
                        recovery_window_ms,
                    ),
                    target_connected: true,
                },
                rebound_effect.into_iter().collect(),
            ))
        }
        (
            State::RecoveringClaim {
                target_id,
                lease,
                tux_id,
                expires_at_ms,
                recover_deadline_ms,
                target_connected,
            },
            Event::Rebound {
                new_lease_id,
                tux_id: rebound_tid,
                new_expires_at_ms,
            },
        ) => {
            if rebound_tid != tux_id {
                return Err(err("RecoveringClaim", "Rebound", "tux_id mismatch"));
            }
            if !target_connected {
                return Ok((
                    State::RecoveringClaim {
                        target_id,
                        lease,
                        tux_id,
                        expires_at_ms,
                        recover_deadline_ms,
                        target_connected,
                    },
                    vec![],
                ));
            }
            let mut effects = vec![Effect::SendLeaseRebound {
                target_id: target_id.clone(),
                lease_id: new_lease_id.clone(),
                tux_id: tux_id.clone(),
                expires_at_ms: new_expires_at_ms,
            }];
            if let RecoveryLease::Assigned(old_lease_id) = &lease {
                effects.push(Effect::RemoveLease {
                    lease_id: old_lease_id.clone(),
                });
            }
            let _ = expires_at_ms;
            Ok((
                State::Claimed {
                    target_id,
                    lease_id: new_lease_id,
                    tux_id,
                    expires_at_ms: new_expires_at_ms,
                },
                effects,
            ))
        }
        (
            State::RecoveringClaim {
                target_id,
                lease,
                tux_id,
                expires_at_ms,
                recover_deadline_ms,
                target_connected,
            },
            Event::Tick { now_ms },
        ) => {
            if now_ms < recover_deadline_ms {
                return Ok((
                    State::RecoveringClaim {
                        target_id,
                        lease,
                        tux_id,
                        expires_at_ms,
                        recover_deadline_ms,
                        target_connected,
                    },
                    vec![],
                ));
            }
            let mut effects = vec![];
            if let RecoveryLease::Assigned(lid) = &lease {
                effects.push(Effect::SendClaimReleasedToTux {
                    target_id: target_id.clone(),
                    lease_ref: LeaseRef::Known {
                        lease_id: lid.clone(),
                    },
                    tux_id: tux_id.clone(),
                    reason: LeaseTerminationReason::RecoveryExpired,
                });
                effects.push(Effect::RemoveLease {
                    lease_id: lid.clone(),
                });
            }
            if target_connected {
                effects.push(Effect::SendTargetReleased {
                    target_id: target_id.clone(),
                    lease_ref: lease.to_lease_ref(),
                    reason: LeaseTerminationReason::RecoveryExpired,
                });
                Ok((State::Available { target_id }, effects))
            } else {
                effects.push(Effect::DropTargetRecord {
                    target_id: target_id.clone(),
                });
                Ok((State::Available { target_id }, effects))
            }
        }
        (
            State::RecoveringClaim {
                target_id,
                lease,
                tux_id,
                expires_at_ms,
                recover_deadline_ms,
                ..
            },
            Event::TargetDisconnected { .. },
        ) => Ok((
            State::RecoveringClaim {
                target_id,
                lease,
                tux_id,
                expires_at_ms,
                recover_deadline_ms,
                target_connected: false,
            },
            vec![],
        )),
        (
            State::RecoveringClaim {
                target_id,
                lease,
                tux_id,
                expires_at_ms,
                recover_deadline_ms,
                target_connected,
            },
            Event::TuxDisconnected {
                tux_id: disc_tid, ..
            },
        ) => {
            if disc_tid != tux_id {
                return Ok((
                    State::RecoveringClaim {
                        target_id,
                        lease,
                        tux_id,
                        expires_at_ms,
                        recover_deadline_ms,
                        target_connected,
                    },
                    vec![],
                ));
            }
            let mut effects = vec![];
            if let RecoveryLease::Assigned(lid) = &lease {
                effects.push(Effect::RemoveLease {
                    lease_id: lid.clone(),
                });
                effects.push(Effect::SendClaimReleasedToTux {
                    target_id: target_id.clone(),
                    lease_ref: LeaseRef::Known {
                        lease_id: lid.clone(),
                    },
                    tux_id,
                    reason: LeaseTerminationReason::TuxDisconnected,
                });
            }
            if matches!(lease, RecoveryLease::Assigned(_)) {
                effects.push(Effect::SendTargetReleased {
                    target_id: target_id.clone(),
                    lease_ref: lease.to_lease_ref(),
                    reason: LeaseTerminationReason::TuxDisconnected,
                });
            }
            Ok((State::Available { target_id }, effects))
        }
        (state, event) => {
            let state_name = match &state {
                State::Available { .. } => "Available",
                State::AwaitingAck { .. } => "AwaitingAck",
                State::Claimed { .. } => "Claimed",
                State::RecoveringClaim { .. } => "RecoveringClaim",
            };
            Err(err(
                state_name,
                &format!("{event:?}"),
                "unhandled event in this state",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_ack_goes_directly_to_claimed() {
        let state = State::AwaitingAck {
            target_id: "t".into(),
            lease_id: "l".into(),
            tux_id: "u".into(),
            expires_at_ms: 10_000,
            ack_deadline_ms: 5_000,
        };
        let (state, effects) = transition(
            state,
            Event::ClaimAcked {
                lease_id: "l".into(),
                tux_id: "u".into(),
            },
        )
        .unwrap();
        assert!(matches!(
            state,
            State::Claimed {
                target_id,
                lease_id,
                tux_id,
                expires_at_ms: 10_000,
            } if target_id == "t" && lease_id == "l" && tux_id == "u"
        ));
        assert!(effects.is_empty());
    }

    #[test]
    fn recovery_deadline_is_bounded_by_ttl() {
        let state = State::Claimed {
            target_id: "t".into(),
            lease_id: "l".into(),
            tux_id: "u".into(),
            expires_at_ms: 1_000,
        };
        let (state, _) = transition(
            state,
            Event::TuxDisconnected {
                tux_id: "u".into(),
                now_ms: 100,
                recovery_window_ms: 5_000,
            },
        )
        .unwrap();
        match state {
            State::RecoveringClaim {
                recover_deadline_ms,
                ..
            } => assert_eq!(recover_deadline_ms, 1_000),
            _ => panic!("expected recovering claim"),
        }
    }

    #[test]
    fn recovery_expiry_drops_disconnected_target() {
        let state = State::RecoveringClaim {
            target_id: "t".into(),
            lease: RecoveryLease::Assigned("l".into()),
            tux_id: "u".into(),
            expires_at_ms: 1_000,
            recover_deadline_ms: 1_000,
            target_connected: false,
        };
        let (_, effects) = transition(state, Event::Tick { now_ms: 1_001 }).unwrap();
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::DropTargetRecord { target_id } if target_id == "t"))
        );
    }

    #[test]
    fn rebound_does_not_revive_disconnected_target() {
        let state = State::RecoveringClaim {
            target_id: "t".into(),
            lease: RecoveryLease::Assigned("lease-old".into()),
            tux_id: "u".into(),
            expires_at_ms: 1_000,
            recover_deadline_ms: 1_000,
            target_connected: false,
        };
        let (state, effects) = transition(
            state,
            Event::Rebound {
                new_lease_id: "lease-new".into(),
                tux_id: "u".into(),
                new_expires_at_ms: 2_000,
            },
        )
        .unwrap();
        assert!(matches!(
            state,
            State::RecoveringClaim {
                lease: RecoveryLease::Assigned(ref lease_id),
                target_connected: false,
                ..
            } if lease_id == "lease-old"
        ));
        assert!(effects.is_empty());
    }
}
