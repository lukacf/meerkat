//! Trust management for Meerkat comms.

use serde::{Deserialize, Serialize};

use crate::identity::PubKey;

/// A trusted peer in the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPeer {
    /// Human-readable name for the peer.
    pub name: String,
    /// The peer's public key.
    pub pubkey: PubKey,
    /// Address to reach the peer (e.g., "uds:///tmp/meerkat.sock" or "tcp://host:port").
    pub addr: String,
}

/// Collection of trusted peers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPeers {
    /// List of trusted peers.
    pub peers: Vec<TrustedPeer>,
}

impl TrustedPeers {
    /// Create an empty trusted peers list.
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trusted_peer_fields() {
        let peer = TrustedPeer {
            name: "test-peer".to_string(),
            pubkey: PubKey::new([42u8; 32]),
            addr: "tcp://127.0.0.1:4200".to_string(),
        };
        assert_eq!(peer.name, "test-peer");
        assert_eq!(peer.pubkey.as_bytes()[0], 42);
        assert_eq!(peer.addr, "tcp://127.0.0.1:4200");
    }

    #[test]
    fn test_trusted_peers_fields() {
        let peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "peer1".to_string(),
                pubkey: PubKey::new([1u8; 32]),
                addr: "uds:///tmp/test.sock".to_string(),
            }],
        };
        assert_eq!(peers.peers.len(), 1);
        assert_eq!(peers.peers[0].name, "peer1");
    }

    #[test]
    fn test_trusted_peer_json_roundtrip() {
        let peer = TrustedPeer {
            name: "coding-meerkat".to_string(),
            pubkey: PubKey::new([7u8; 32]),
            addr: "uds:///tmp/meerkat-coding.sock".to_string(),
        };
        let json = serde_json::to_string(&peer).unwrap();
        let decoded: TrustedPeer = serde_json::from_str(&json).unwrap();
        assert_eq!(peer, decoded);
    }

    #[test]
    fn test_trusted_peers_json_roundtrip() {
        let peers = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "peer1".to_string(),
                    pubkey: PubKey::new([1u8; 32]),
                    addr: "tcp://192.168.1.50:4200".to_string(),
                },
                TrustedPeer {
                    name: "peer2".to_string(),
                    pubkey: PubKey::new([2u8; 32]),
                    addr: "uds:///tmp/peer2.sock".to_string(),
                },
            ],
        };
        let json = serde_json::to_string_pretty(&peers).unwrap();
        let decoded: TrustedPeers = serde_json::from_str(&json).unwrap();
        assert_eq!(peers, decoded);
    }
}
