// @generated — protocol helpers for `supervisor_trust_publish`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: PublishSupervisorTrustEdge
// Closure policy: AckRequired
// Liveness: eventual feedback under comms transport liveness — `send_bridge_response` surfaces the typed outcome

use crate::meerkat_machine::dsl::MeerkatMachineEffect;
use meerkat_core::comms::{PeerAddress, PeerId, PeerName, TrustedPeerDescriptor};

#[derive(Debug, Clone)]
pub struct SupervisorTrustPublishObligation {
    pub peer: SupervisorTrustPublishPeer,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorTrustPublishPeer {
    Valid {
        peer_id: PeerId,
        name: PeerName,
        address: PeerAddress,
        signing_public_key: Option<String>,
    },
    Invalid {
        reason: String,
    },
}

impl SupervisorTrustPublishPeer {
    fn from_effect_fields(
        peer_id: &str,
        name: &str,
        address: &str,
        signing_public_key: Option<String>,
    ) -> Self {
        let peer_id = match PeerId::parse(peer_id) {
            Ok(peer_id) => peer_id,
            Err(error) => {
                return Self::Invalid {
                    reason: format!("invalid peer_id: {error}"),
                };
            }
        };
        let name = match PeerName::new(name.to_string()) {
            Ok(name) => name,
            Err(error) => {
                return Self::Invalid {
                    reason: format!("invalid peer name: {error}"),
                };
            }
        };
        let address = match PeerAddress::parse(address) {
            Ok(address) => address,
            Err(error) => {
                return Self::Invalid {
                    reason: format!("invalid peer address: {error}"),
                };
            }
        };
        Self::Valid {
            peer_id,
            name,
            address,
            signing_public_key,
        }
    }

    pub fn peer_id(&self) -> Result<PeerId, String> {
        match self {
            Self::Valid { peer_id, .. } => Ok(*peer_id),
            Self::Invalid { reason } => Err(reason.clone()),
        }
    }

    pub fn matches_descriptor(&self, descriptor: &TrustedPeerDescriptor) -> Result<(), String> {
        match self {
            Self::Valid {
                peer_id,
                name,
                address,
                ..
            } if peer_id == &descriptor.peer_id
                && name == &descriptor.name
                && address == &descriptor.address =>
            {
                Ok(())
            }
            Self::Valid { .. } => {
                Err("typed publish peer does not match staged supervisor descriptor".to_string())
            }
            Self::Invalid { reason } => Err(reason.clone()),
        }
    }
}

pub fn extract_obligations(
    effects: &[MeerkatMachineEffect],
) -> Vec<SupervisorTrustPublishObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::PublishSupervisorTrustEdge {
                peer_id,
                name,
                address,
                signing_public_key,
                epoch,
            } => Some(SupervisorTrustPublishObligation {
                peer: SupervisorTrustPublishPeer::from_effect_fields(
                    peer_id,
                    name,
                    address,
                    signing_public_key.clone(),
                ),
                epoch: *epoch,
            }),
            _ => None,
        })
        .collect()
}
