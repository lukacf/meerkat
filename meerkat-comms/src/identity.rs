//! Cryptographic identity types for Meerkat comms.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Ed25519 public key (32 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PubKey(pub [u8; 32]);

/// Ed25519 signature (64 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature(pub [u8; 64]);

// Custom serde implementation for Signature since serde doesn't support [u8; 64] by default
impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_bytes::serialize(&self.0[..], serializer)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: Vec<u8> = serde_bytes::deserialize(deserializer)?;
        if bytes.len() != 64 {
            return Err(serde::de::Error::invalid_length(bytes.len(), &"64 bytes"));
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&bytes);
        Ok(Signature(arr))
    }
}

impl PubKey {
    /// Create a new PubKey from raw bytes.
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Signature {
    /// Create a new Signature from raw bytes.
    pub fn new(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    /// Get the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn test_pubkey_size() {
        assert_eq!(size_of::<PubKey>(), 32);
    }

    #[test]
    fn test_signature_size() {
        assert_eq!(size_of::<Signature>(), 64);
    }

    #[test]
    fn test_pubkey_cbor_roundtrip() {
        let pubkey = PubKey::new([42u8; 32]);
        let mut buf = Vec::new();
        ciborium::into_writer(&pubkey, &mut buf).unwrap();
        let decoded: PubKey = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(pubkey, decoded);
    }

    #[test]
    fn test_signature_cbor_roundtrip() {
        let sig = Signature::new([99u8; 64]);
        let mut buf = Vec::new();
        ciborium::into_writer(&sig, &mut buf).unwrap();
        let decoded: Signature = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(sig, decoded);
    }
}
