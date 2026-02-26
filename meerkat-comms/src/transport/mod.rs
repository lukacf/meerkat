//! Transport layer for Meerkat comms.
//!
//! Provides length-prefix framing and address parsing for UDS and TCP transports.

#[cfg(not(target_arch = "wasm32"))]
pub mod codec;
pub mod plain_codec;
#[cfg(not(target_arch = "wasm32"))]
pub mod tcp;
#[cfg(unix)]
pub mod uds;

use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

use thiserror::Error;

/// Maximum payload size: 1 MB (1,048,576 bytes).
pub const MAX_PAYLOAD_SIZE: u32 = 1_048_576;

/// Errors that can occur during transport operations.
#[derive(Debug, Error)]
pub enum TransportError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Timeout waiting for peer")]
    Timeout,
    #[error("Message too large: {size} bytes (max {MAX_PAYLOAD_SIZE})")]
    MessageTooLarge { size: u32 },
    #[error("Invalid frame: {0}")]
    InvalidFrame(String),
    #[error("Invalid address format: {0}")]
    InvalidAddress(String),
    #[error("CBOR encoding error: {0}")]
    Cbor(String),
}

/// Peer address for connecting to a remote peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerAddr {
    /// Unix domain socket path.
    #[cfg(not(target_arch = "wasm32"))]
    Uds(PathBuf),
    /// TCP address as "host:port" string (supports both IP addresses and hostnames).
    /// DNS resolution happens at connect time via ToSocketAddrs.
    Tcp(String),
    /// In-process address for sub-agent communication.
    /// Messages are delivered directly via in-memory channels.
    Inproc(String),
}

impl PeerAddr {
    /// Parse an address string into a PeerAddr.
    ///
    /// Supported formats:
    /// - `uds:///path/to/socket.sock` (not available on wasm32)
    /// - `tcp://host:port` (host can be IP address or hostname)
    /// - `inproc://agent-name` (in-process delivery via registry)
    pub fn parse(s: &str) -> Result<Self, TransportError> {
        if let Some(_path) = s.strip_prefix("uds://") {
            #[cfg(not(target_arch = "wasm32"))]
            {
                Ok(PeerAddr::Uds(PathBuf::from(_path)))
            }
            #[cfg(target_arch = "wasm32")]
            {
                Err(TransportError::InvalidAddress(
                    "UDS transport is not available on wasm32".to_string(),
                ))
            }
        } else if let Some(addr_str) = s.strip_prefix("tcp://") {
            // Validate format: must have host:port structure
            if !addr_str.contains(':') {
                return Err(TransportError::InvalidAddress(
                    "TCP address must include port (host:port)".to_string(),
                ));
            }
            // Store as string for DNS resolution at connect time
            Ok(PeerAddr::Tcp(addr_str.to_string()))
        } else if let Some(name) = s.strip_prefix("inproc://") {
            if name.is_empty() {
                return Err(TransportError::InvalidAddress(
                    "Inproc address must include agent name".to_string(),
                ));
            }
            Ok(PeerAddr::Inproc(name.to_string()))
        } else {
            Err(TransportError::InvalidAddress(format!(
                "unknown scheme, expected uds://, tcp://, or inproc://: {s}"
            )))
        }
    }

    /// Check if this is an in-process address.
    pub fn is_inproc(&self) -> bool {
        matches!(self, PeerAddr::Inproc(_))
    }

    /// Get the agent name for inproc addresses.
    pub fn inproc_name(&self) -> Option<&str> {
        match self {
            PeerAddr::Inproc(name) => Some(name),
            _ => None,
        }
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::identity::{Keypair, PubKey};
    use crate::transport::codec::{EnvelopeFrame, TransportCodec};
    use crate::types::Envelope;
    use crate::types::MessageKind;
    use bytes::{Bytes, BytesMut};
    use std::sync::Arc;
    use tokio_util::codec::{Decoder, Encoder};
    use uuid::Uuid;

    fn make_test_envelope() -> Envelope {
        let keypair = Keypair::generate();
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: keypair.public_key(),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                body: "hello".to_string(),
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        };
        envelope.sign(&keypair);
        envelope
    }

    #[test]
    fn test_transport_error_io() {
        let err = TransportError::Io(io::Error::new(io::ErrorKind::NotFound, "not found"));
        assert!(err.to_string().contains("IO error"));
    }

    #[test]
    fn test_transport_error_timeout() {
        let err = TransportError::Timeout;
        assert!(err.to_string().contains("Timeout"));
    }

    #[test]
    fn test_transport_error_too_large() {
        let err = TransportError::MessageTooLarge { size: 2_000_000 };
        assert!(err.to_string().contains("too large"));
        assert!(err.to_string().contains("2000000"));
    }

    #[test]
    fn test_transport_error_invalid_frame() {
        let err = TransportError::InvalidFrame("bad cbor".to_string());
        assert!(err.to_string().contains("Invalid frame"));
    }

    #[test]
    fn test_peer_addr_uds_variant() {
        let addr = PeerAddr::Uds(PathBuf::from("/tmp/test.sock"));
        match addr {
            PeerAddr::Uds(path) => assert_eq!(path, PathBuf::from("/tmp/test.sock")),
            _ => panic!("expected Uds variant"),
        }
    }

    #[test]
    fn test_peer_addr_tcp_variant() {
        let addr = PeerAddr::Tcp("127.0.0.1:4200".to_string());
        match addr {
            PeerAddr::Tcp(a) => assert_eq!(a, "127.0.0.1:4200"),
            _ => panic!("expected Tcp variant"),
        }
    }

    #[test]
    fn test_parse_uds_addr() {
        let addr = PeerAddr::parse("uds:///tmp/meerkat.sock").unwrap();
        match addr {
            PeerAddr::Uds(path) => assert_eq!(path, PathBuf::from("/tmp/meerkat.sock")),
            _ => panic!("expected Uds variant"),
        }
    }

    #[test]
    fn test_parse_tcp_addr_ip() {
        let addr = PeerAddr::parse("tcp://192.168.1.50:4200").unwrap();
        match addr {
            PeerAddr::Tcp(a) => {
                assert_eq!(a, "192.168.1.50:4200");
            }
            _ => panic!("expected Tcp variant"),
        }
    }

    #[test]
    fn test_parse_tcp_addr_hostname() {
        // Hostnames should be accepted and stored for resolution at connect time
        let addr = PeerAddr::parse("tcp://localhost:4200").unwrap();
        match addr {
            PeerAddr::Tcp(a) => {
                assert_eq!(a, "localhost:4200");
            }
            _ => panic!("expected Tcp variant"),
        }
    }

    #[test]
    fn test_parse_tcp_addr_fqdn() {
        // Fully qualified domain names should be accepted
        let addr = PeerAddr::parse("tcp://peer.example.com:4200").unwrap();
        match addr {
            PeerAddr::Tcp(a) => {
                assert_eq!(a, "peer.example.com:4200");
            }
            _ => panic!("expected Tcp variant"),
        }
    }

    #[test]
    fn test_parse_tcp_addr_missing_port() {
        // Must have port specified
        let result = PeerAddr::parse("tcp://localhost");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("port"));
    }

    #[test]
    fn test_parse_invalid_addr() {
        let result = PeerAddr::parse("http://example.com");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("unknown scheme"));
    }

    #[test]
    fn test_peer_addr_inproc_variant() {
        let addr = PeerAddr::Inproc("sub-agent-123".to_string());
        match addr {
            PeerAddr::Inproc(name) => assert_eq!(name, "sub-agent-123"),
            _ => panic!("expected Inproc variant"),
        }
    }

    #[test]
    fn test_parse_inproc_addr() {
        let addr = PeerAddr::parse("inproc://my-sub-agent").unwrap();
        match addr {
            PeerAddr::Inproc(name) => assert_eq!(name, "my-sub-agent"),
            _ => panic!("expected Inproc variant"),
        }
    }

    #[test]
    fn test_parse_inproc_addr_empty_name() {
        let result = PeerAddr::parse("inproc://");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("agent name"));
    }

    #[test]
    fn test_inproc_is_inproc() {
        let inproc = PeerAddr::Inproc("test".to_string());
        let uds = PeerAddr::Uds(PathBuf::from("/tmp/test.sock"));
        let tcp = PeerAddr::Tcp("localhost:8080".to_string());

        assert!(inproc.is_inproc());
        assert!(!uds.is_inproc());
        assert!(!tcp.is_inproc());
    }

    #[test]
    fn test_inproc_name() {
        let inproc = PeerAddr::Inproc("my-agent".to_string());
        let uds = PeerAddr::Uds(PathBuf::from("/tmp/test.sock"));

        assert_eq!(inproc.inproc_name(), Some("my-agent"));
        assert_eq!(uds.inproc_name(), None);
    }

    #[test]
    fn test_transport_codec_encode_format() {
        let envelope = make_test_envelope();
        let mut codec = TransportCodec::new(MAX_PAYLOAD_SIZE);
        let mut buf = BytesMut::new();
        codec
            .encode(
                EnvelopeFrame {
                    envelope,
                    raw: Arc::new(Bytes::new()),
                },
                &mut buf,
            )
            .unwrap();

        // First 4 bytes are big-endian length
        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        assert_eq!(len as usize, buf.len() - 4);
        assert!(len > 0);
    }

    #[test]
    fn test_transport_codec_decode_roundtrip() {
        let envelope = make_test_envelope();
        let envelope_id = envelope.id;
        let envelope_from = envelope.from;
        let mut codec = TransportCodec::new(MAX_PAYLOAD_SIZE);
        let mut buf = BytesMut::new();
        codec
            .encode(
                EnvelopeFrame {
                    envelope,
                    raw: Arc::new(Bytes::new()),
                },
                &mut buf,
            )
            .unwrap();

        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded.envelope.id, envelope_id);
        assert_eq!(decoded.envelope.from, envelope_from);
    }

    #[test]
    fn test_transport_codec_reject_oversized_payload() {
        // Craft a fake length prefix indicating > 1 MB
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&2_000_000u32.to_be_bytes());
        buf.extend_from_slice(&[0u8; 100]); // partial payload

        let mut codec = TransportCodec::new(MAX_PAYLOAD_SIZE);
        let err = codec.decode(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("message too large"));
    }

    #[test]
    fn test_transport_codec_envelope_roundtrip() {
        let envelope = make_test_envelope();
        let mut codec = TransportCodec::new(MAX_PAYLOAD_SIZE);
        let mut buf = BytesMut::new();
        codec
            .encode(
                EnvelopeFrame {
                    envelope: envelope.clone(),
                    raw: Arc::new(Bytes::new()),
                },
                &mut buf,
            )
            .unwrap();

        let decoded = codec.decode(&mut buf).unwrap().unwrap();

        assert_eq!(decoded.envelope.id, envelope.id);
        assert_eq!(decoded.envelope.from, envelope.from);
        assert_eq!(decoded.envelope.to, envelope.to);
        // Verify signature is preserved
        assert!(decoded.envelope.verify());
    }
}
