// @generated — comms trust authority handoff helpers for MobMachine.
// Producer: MobMachine member/external peer trust effects.

use crate::machines::mob_machine as mob_dsl;
use meerkat_core::generated::comms_trust_authority as core_authority;

struct MemberTrustObligation {
    edge: mob_dsl::WiringEdge,
    a_peer_id: String,
    b_peer_id: String,
    epoch: u64,
}

impl core_authority::GeneratedMobMachineMemberTrustHandoff for MemberTrustObligation {
    fn edge_a(&self) -> &str {
        self.edge.a.0.as_str()
    }

    fn edge_b(&self) -> &str {
        self.edge.b.0.as_str()
    }

    fn a_peer_id(&self) -> &str {
        self.a_peer_id.as_str()
    }

    fn b_peer_id(&self) -> &str {
        self.b_peer_id.as_str()
    }

    fn epoch(&self) -> u64 {
        self.epoch
    }
}

struct ExternalPeerTrustObligation {
    peer_id: String,
    epoch: u64,
}

impl core_authority::GeneratedMobMachineExternalPeerTrustHandoff for ExternalPeerTrustObligation {
    fn peer_id(&self) -> &str {
        self.peer_id.as_str()
    }

    fn epoch(&self) -> u64 {
        self.epoch
    }
}

fn member_wiring_obligation_from_effect(
    effect: &mob_dsl::MobMachineEffect,
    edge: &mob_dsl::WiringEdge,
) -> Option<MemberTrustObligation> {
    match effect {
        mob_dsl::MobMachineEffect::MemberTrustWiringRequested {
            edge: effect_edge,
            a_peer_id,
            b_peer_id,
            epoch,
        } if effect_edge == edge => Some(MemberTrustObligation {
            edge: effect_edge.clone(),
            a_peer_id: a_peer_id.0.clone(),
            b_peer_id: b_peer_id.0.clone(),
            epoch: *epoch,
        }),
        _ => None,
    }
}

fn member_unwiring_obligation_from_effect(
    effect: &mob_dsl::MobMachineEffect,
    edge: &mob_dsl::WiringEdge,
) -> Option<MemberTrustObligation> {
    match effect {
        mob_dsl::MobMachineEffect::MemberTrustUnwiringRequested {
            edge: effect_edge,
            a_peer_id,
            b_peer_id,
            epoch,
        } if effect_edge == edge => Some(MemberTrustObligation {
            edge: effect_edge.clone(),
            a_peer_id: a_peer_id.0.clone(),
            b_peer_id: b_peer_id.0.clone(),
            epoch: *epoch,
        }),
        _ => None,
    }
}

pub fn member_wiring_handoff_from_effect(
    effect: &mob_dsl::MobMachineEffect,
    edge: &mob_dsl::WiringEdge,
) -> Option<core_authority::MobMachineMemberTrustHandoff> {
    member_wiring_obligation_from_effect(effect, edge).map(|obligation| {
        core_authority::MobMachineMemberTrustHandoff::from_generated_member_wiring(&obligation)
    })
}

pub fn member_unwiring_handoff_from_effect(
    effect: &mob_dsl::MobMachineEffect,
    edge: &mob_dsl::WiringEdge,
) -> Option<core_authority::MobMachineMemberTrustHandoff> {
    member_unwiring_obligation_from_effect(effect, edge).map(|obligation| {
        core_authority::MobMachineMemberTrustHandoff::from_generated_member_unwiring(&obligation)
    })
}

pub fn member_repair_handoff_from_effect(
    effect: &mob_dsl::MobMachineEffect,
    edge: &mob_dsl::WiringEdge,
) -> Option<core_authority::MobMachineMemberTrustHandoff> {
    member_wiring_obligation_from_effect(effect, edge).map(|obligation| {
        core_authority::MobMachineMemberTrustHandoff::from_generated_member_repair(&obligation)
    })
}

pub fn external_wiring_handoff(
    edge: &mob_dsl::ExternalPeerEdge,
    epoch: u64,
) -> core_authority::MobMachineExternalPeerTrustHandoff {
    core_authority::MobMachineExternalPeerTrustHandoff::from_generated_external_peer_wiring(
        &ExternalPeerTrustObligation {
            peer_id: edge.endpoint.peer_id.0.clone(),
            epoch,
        },
    )
}

pub fn external_unwiring_handoff(
    edge: &mob_dsl::ExternalPeerEdge,
    epoch: u64,
) -> core_authority::MobMachineExternalPeerTrustHandoff {
    core_authority::MobMachineExternalPeerTrustHandoff::from_generated_external_peer_unwiring(
        &ExternalPeerTrustObligation {
            peer_id: edge.endpoint.peer_id.0.clone(),
            epoch,
        },
    )
}

pub fn external_repair_handoff(
    edge: &mob_dsl::ExternalPeerEdge,
    epoch: u64,
) -> core_authority::MobMachineExternalPeerTrustHandoff {
    core_authority::MobMachineExternalPeerTrustHandoff::from_generated_external_peer_repair(
        &ExternalPeerTrustObligation {
            peer_id: edge.endpoint.peer_id.0.clone(),
            epoch,
        },
    )
}

pub fn external_reciprocal_wiring_handoff_from_effect(
    effect: &mob_dsl::MobMachineEffect,
    key: &mob_dsl::ExternalPeerKey,
    epoch: u64,
) -> Option<core_authority::MobMachineExternalPeerTrustHandoff> {
    match effect {
        mob_dsl::MobMachineEffect::ExternalPeerReciprocalTrustRequested {
            key: effect_key,
            peer_id,
            ..
        } if effect_key == key => Some(
            core_authority::MobMachineExternalPeerTrustHandoff::from_generated_external_peer_wiring(
                &ExternalPeerTrustObligation {
                    peer_id: peer_id.0.clone(),
                    epoch,
                },
            ),
        ),
        _ => None,
    }
}
