//! Target attachment authority — target-side kennel-mode lifecycle.
#![allow(clippy::ptr_arg)] // machine! renders String helper params as borrowed authority inputs.

use crate::LeaseRef;
use meerkat_machine_dsl::machine;

machine! {
    machine TargetAttachment {
        version: 1,
        rust: "mdm-tux" / "machines::target_attachment",

        state {
            phase: TargetAttachmentPhase,
            target_id: String,
            lease_id: String,
            tux_id: String,
            tux_pubkey: String,
            tux_direct_addr: String,
            kennel_alive: bool,
        }

        init(Idle) {
            target_id = "",
            lease_id = "",
            tux_id = "",
            tux_pubkey = "",
            tux_direct_addr = "",
            kennel_alive = false,
        }

        terminal [Released, AttachFailed, DirectLost]

        phase TargetAttachmentPhase {
            Idle,
            Attaching,
            Attached,
            Released,
            AttachFailed,
            DirectLost,
        }

        input TargetAttachmentInput {
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
                release_lease_id: Option<String>,
                pending_rebind: bool,
            },
            KennelDisconnected,
            KennelHeartbeatFailed,
            DirectLinkLost,
            AttachSendFailed,
        }

        effect TargetAttachmentEffect {
            EnsureTuxPeer {
                tux_pubkey: String,
                tux_direct_addr: String,
            },
            PersistAttachmentHint {
                tux_id: String,
                lease_id: String,
            },
            ClearAttachmentHint,
            SendAttachRequest {
                tux_id: String,
                target_id: String,
                lease_id: String,
            },
            StartDirectHeartbeat {
                tux_id: String,
            },
            StopDirectHeartbeat,
            ReturnToRegisterClean,
            ReturnToRegisterWithHint {
                tux_id: String,
            },
        }

        helper release_matches(pending_rebind: bool, release_lease_id: Option<String>, current_lease_id: String) -> bool {
            pending_rebind == true || release_lease_id == Some(current_lease_id)
        }

        helper release_misses(pending_rebind: bool, release_lease_id: Option<String>, current_lease_id: String) -> bool {
            release_matches(pending_rebind, release_lease_id, current_lease_id) == false
        }

        helper rebound_material_same(
            lease_id: String,
            tux_pubkey: String,
            tux_direct_addr: String,
            current_lease_id: String,
            current_tux_pubkey: String,
            current_tux_direct_addr: String
        ) -> bool {
            lease_id == current_lease_id
                && tux_pubkey == current_tux_pubkey
                && tux_direct_addr == current_tux_direct_addr
        }

        helper rebound_material_changed(
            lease_id: String,
            tux_pubkey: String,
            tux_direct_addr: String,
            current_lease_id: String,
            current_tux_pubkey: String,
            current_tux_direct_addr: String
        ) -> bool {
            rebound_material_same(
                lease_id,
                tux_pubkey,
                tux_direct_addr,
                current_lease_id,
                current_tux_pubkey,
                current_tux_direct_addr
            ) == false
        }

        disposition EnsureTuxPeer => local,
        disposition PersistAttachmentHint => local,
        disposition ClearAttachmentHint => local,
        disposition SendAttachRequest => local,
        disposition StartDirectHeartbeat => local,
        disposition StopDirectHeartbeat => local,
        disposition ReturnToRegisterClean => local,
        disposition ReturnToRegisterWithHint => local,

        transition AdoptedIdle {
            on input Adopted { target_id, lease_id, tux_id, tux_pubkey, tux_direct_addr }
            guard { self.phase == Phase::Idle }
            update {
                self.target_id = target_id;
                self.lease_id = lease_id;
                self.tux_id = tux_id;
                self.tux_pubkey = tux_pubkey;
                self.tux_direct_addr = tux_direct_addr;
                self.kennel_alive = true;
            }
            to Attaching
            emit EnsureTuxPeer { tux_pubkey: self.tux_pubkey, tux_direct_addr: self.tux_direct_addr }
            emit SendAttachRequest { tux_id: self.tux_id, target_id: self.target_id, lease_id: self.lease_id }
        }

        transition LeaseReboundIdle {
            on input LeaseRebound { target_id, lease_id, tux_id, tux_pubkey, tux_direct_addr }
            guard { self.phase == Phase::Idle }
            update {
                self.target_id = target_id;
                self.lease_id = lease_id;
                self.tux_id = tux_id;
                self.tux_pubkey = tux_pubkey;
                self.tux_direct_addr = tux_direct_addr;
                self.kennel_alive = true;
            }
            to Attached
            emit EnsureTuxPeer { tux_pubkey: self.tux_pubkey, tux_direct_addr: self.tux_direct_addr }
            emit PersistAttachmentHint { tux_id: self.tux_id, lease_id: self.lease_id }
            emit StartDirectHeartbeat { tux_id: self.tux_id }
        }

        transition DirectAttachAckMatches {
            on input DirectAttachAck { lease_id }
            guard { self.phase == Phase::Attaching && lease_id == self.lease_id }
            update {
                self.kennel_alive = true;
            }
            to Attached
            emit PersistAttachmentHint { tux_id: self.tux_id, lease_id: self.lease_id }
            emit StartDirectHeartbeat { tux_id: self.tux_id }
        }

        transition KennelReleasedAttachingMatch {
            on input KennelReleased { release_lease_id, pending_rebind }
            guard {
                self.phase == Phase::Attaching
                    && release_matches(pending_rebind, release_lease_id, self.lease_id)
            }
            update {
                self.kennel_alive = true;
            }
            to Released
            emit ClearAttachmentHint
            emit ReturnToRegisterClean
        }

        transition KennelReleasedAttachingMiss {
            on input KennelReleased { release_lease_id, pending_rebind }
            guard {
                self.phase == Phase::Attaching
                    && release_misses(pending_rebind, release_lease_id, self.lease_id)
            }
            update {
                self.kennel_alive = true;
            }
            to Attaching
        }

        transition KennelDisconnectedAttaching {
            on input KennelDisconnected
            guard { self.phase == Phase::Attaching }
            update {
                self.kennel_alive = false;
            }
            to AttachFailed
            emit ClearAttachmentHint
            emit ReturnToRegisterClean
        }

        transition KennelHeartbeatFailedAttaching {
            on input KennelHeartbeatFailed
            guard { self.phase == Phase::Attaching }
            update {
                self.kennel_alive = false;
            }
            to AttachFailed
            emit ClearAttachmentHint
            emit ReturnToRegisterClean
        }

        transition AttachSendFailedAttaching {
            on input AttachSendFailed
            guard { self.phase == Phase::Attaching }
            update {
                self.kennel_alive = false;
            }
            to AttachFailed
            emit ClearAttachmentHint
            emit ReturnToRegisterClean
        }

        transition DirectLinkLostAttaching {
            on input DirectLinkLost
            guard { self.phase == Phase::Attaching }
            update {
                self.kennel_alive = false;
            }
            to AttachFailed
            emit ClearAttachmentHint
            emit ReturnToRegisterClean
        }

        transition KennelReleasedAttachedMatch {
            on input KennelReleased { release_lease_id, pending_rebind }
            guard {
                self.phase == Phase::Attached
                    && release_matches(pending_rebind, release_lease_id, self.lease_id)
            }
            update {
                self.kennel_alive = true;
            }
            to Released
            emit StopDirectHeartbeat
            emit ClearAttachmentHint
            emit ReturnToRegisterClean
        }

        transition KennelReleasedAttachedMiss {
            on input KennelReleased { release_lease_id, pending_rebind }
            guard {
                self.phase == Phase::Attached
                    && release_misses(pending_rebind, release_lease_id, self.lease_id)
            }
            update {
                self.kennel_alive = true;
            }
            to Attached
        }

        transition LeaseReboundAttachedSame {
            on input LeaseRebound { target_id, lease_id, tux_id, tux_pubkey, tux_direct_addr }
            guard {
                self.phase == Phase::Attached
                    && target_id == self.target_id
                    && tux_id == self.tux_id
                    && rebound_material_same(
                        lease_id,
                        tux_pubkey,
                        tux_direct_addr,
                        self.lease_id,
                        self.tux_pubkey,
                        self.tux_direct_addr
                    )
            }
            update {
                self.kennel_alive = true;
            }
            to Attached
        }

        transition LeaseReboundAttachedChanged {
            on input LeaseRebound { target_id, lease_id, tux_id, tux_pubkey, tux_direct_addr }
            guard {
                self.phase == Phase::Attached
                    && target_id == self.target_id
                    && tux_id == self.tux_id
                    && rebound_material_changed(
                        lease_id,
                        tux_pubkey,
                        tux_direct_addr,
                        self.lease_id,
                        self.tux_pubkey,
                        self.tux_direct_addr
                    )
            }
            update {
                self.lease_id = lease_id;
                self.tux_pubkey = tux_pubkey;
                self.tux_direct_addr = tux_direct_addr;
                self.kennel_alive = true;
            }
            to Attached
            emit EnsureTuxPeer { tux_pubkey: self.tux_pubkey, tux_direct_addr: self.tux_direct_addr }
        }

        transition KennelDisconnectedAttached {
            on input KennelDisconnected
            guard { self.phase == Phase::Attached }
            update {
                self.kennel_alive = false;
            }
            to DirectLost
            emit StopDirectHeartbeat
            emit ReturnToRegisterWithHint { tux_id: self.tux_id }
        }

        transition KennelHeartbeatFailedAttached {
            on input KennelHeartbeatFailed
            guard { self.phase == Phase::Attached }
            update {
                self.kennel_alive = false;
            }
            to DirectLost
            emit StopDirectHeartbeat
            emit ReturnToRegisterWithHint { tux_id: self.tux_id }
        }

        transition DirectLinkLostAttached {
            on input DirectLinkLost
            guard { self.phase == Phase::Attached }
            update {
                self.kennel_alive = false;
            }
            to DirectLost
            emit StopDirectHeartbeat
            emit ReturnToRegisterWithHint { tux_id: self.tux_id }
        }
    }
}

pub type State = TargetAttachmentAuthority;
pub type Event = TargetAttachmentInput;
pub type Effect = TargetAttachmentEffect;
pub type TransitionError = TargetAttachmentTransitionError;

pub fn transition(mut state: State, event: Event) -> Result<(State, Vec<Effect>), TransitionError> {
    let transition = TargetAttachmentMutator::apply(&mut state, event)?;
    Ok((state, transition.into_effects()))
}

pub fn kennel_released_input(lease_ref: LeaseRef) -> Event {
    match lease_ref {
        LeaseRef::Known { lease_id } => Event::KennelReleased {
            release_lease_id: Some(lease_id),
            pending_rebind: false,
        },
        LeaseRef::PendingRebind => Event::KennelReleased {
            release_lease_id: None,
            pending_rebind: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adopted() -> Event {
        Event::Adopted {
            target_id: "target-1".into(),
            lease_id: "lease-1".into(),
            tux_id: "tux-1".into(),
            tux_pubkey: "ed25519:tux".into(),
            tux_direct_addr: "tcp://1.2.3.4:9000".into(),
        }
    }

    #[test]
    fn adopted_emits_peer_refresh_and_attach_request() {
        let (state, effects) = transition(State::new(), adopted()).unwrap();
        assert_eq!(state.state().phase(), TargetAttachmentPhase::Attaching);
        assert_eq!(
            effects,
            vec![
                Effect::EnsureTuxPeer {
                    tux_pubkey: "ed25519:tux".into(),
                    tux_direct_addr: "tcp://1.2.3.4:9000".into()
                },
                Effect::SendAttachRequest {
                    tux_id: "tux-1".into(),
                    target_id: "target-1".into(),
                    lease_id: "lease-1".into()
                }
            ]
        );
    }

    #[test]
    fn stale_attach_ack_is_rejected() {
        let (state, _) = transition(State::new(), adopted()).unwrap();
        let result = transition(
            state,
            Event::DirectAttachAck {
                lease_id: "wrong".into(),
            },
        );
        let Err(err) = result else {
            panic!("stale attach ack should reject");
        };
        assert!(matches!(
            err,
            TransitionError::GuardRejected {
                phase: TargetAttachmentPhase::Attaching,
                ..
            }
        ));
    }

    #[test]
    fn direct_link_loss_preserves_tux_id() {
        let (state, _) = transition(State::new(), adopted()).unwrap();
        let (state, _) = transition(
            state,
            Event::DirectAttachAck {
                lease_id: "lease-1".into(),
            },
        )
        .unwrap();
        let (state, loss_effects) = transition(state, Event::DirectLinkLost).unwrap();

        assert_eq!(state.state().phase(), TargetAttachmentPhase::DirectLost);
        assert_eq!(state.state().tux_id, "tux-1");
        assert_eq!(
            loss_effects,
            vec![
                Effect::StopDirectHeartbeat,
                Effect::ReturnToRegisterWithHint {
                    tux_id: "tux-1".into()
                },
            ]
        );
    }
}
