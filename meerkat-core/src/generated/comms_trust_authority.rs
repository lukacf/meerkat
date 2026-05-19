// @generated — Generated comms trust authority handoff helpers.

use crate::comms::CommsTrustMutationAuthority;

pub fn meerkat_machine_peer_projection(
    peer_id: impl Into<String>,
    epoch: u64,
) -> CommsTrustMutationAuthority {
    CommsTrustMutationAuthority::from_meerkat_machine_peer_projection(peer_id, epoch)
}

pub fn meerkat_machine_supervisor_publish(
    peer_id: impl Into<String>,
    epoch: u64,
) -> CommsTrustMutationAuthority {
    CommsTrustMutationAuthority::from_meerkat_machine_supervisor_publish(peer_id, epoch)
}

pub fn meerkat_machine_supervisor_revoke(
    peer_id: impl Into<String>,
    epoch: u64,
) -> CommsTrustMutationAuthority {
    CommsTrustMutationAuthority::from_meerkat_machine_supervisor_revoke(peer_id, epoch)
}

pub fn mob_machine_peer_wiring(
    peer_id: impl Into<String>,
    epoch: u64,
) -> CommsTrustMutationAuthority {
    CommsTrustMutationAuthority::from_mob_machine_peer_wiring(peer_id, epoch)
}

pub fn mob_machine_peer_unwiring(
    peer_id: impl Into<String>,
    epoch: u64,
) -> CommsTrustMutationAuthority {
    CommsTrustMutationAuthority::from_mob_machine_peer_unwiring(peer_id, epoch)
}

pub fn mob_machine_peer_repair(
    peer_id: impl Into<String>,
    epoch: u64,
) -> CommsTrustMutationAuthority {
    CommsTrustMutationAuthority::from_mob_machine_peer_repair(peer_id, epoch)
}

pub fn mob_machine_peer_retire(
    peer_id: impl Into<String>,
    epoch: u64,
) -> CommsTrustMutationAuthority {
    CommsTrustMutationAuthority::from_mob_machine_peer_retire(peer_id, epoch)
}
