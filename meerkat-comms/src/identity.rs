//! Cryptographic identity types for Meerkat comms.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fs;
use std::path::Path;
use thiserror::Error;

/// Errors that can occur during identity operations.
#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("Invalid peer ID format: {0}")]
    InvalidPeerId(String),
    #[error("Invalid base64 encoding: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
    #[error("Invalid key length: expected {expected}, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid signature")]
    InvalidSignature,
}

/// Ed25519 public key (32 bytes).
///
/// Serialized as a CBOR byte string (not array) for interoperability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PubKey(pub [u8; 32]);

// Custom serde implementation for PubKey to serialize as byte string (CBOR major type 2)
// instead of array (CBOR major type 4) for cross-implementation compatibility.
impl Serialize for PubKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_bytes::serialize(&self.0[..], serializer)
    }
}

impl<'de> Deserialize<'de> for PubKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: Vec<u8> = serde_bytes::deserialize(deserializer)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::invalid_length(bytes.len(), &"32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(PubKey(arr))
    }
}

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

    /// Convert to canonical peer ID string format: "ed25519:<base64>"
    pub fn to_peer_id(&self) -> String {
        format!("ed25519:{}", BASE64.encode(self.0))
    }

    /// Parse a peer ID string back to a PubKey.
    pub fn from_peer_id(s: &str) -> Result<Self, IdentityError> {
        let prefix = "ed25519:";
        if !s.starts_with(prefix) {
            return Err(IdentityError::InvalidPeerId(format!(
                "must start with '{prefix}'"
            )));
        }
        let encoded = &s[prefix.len()..];
        let bytes = BASE64.decode(encoded)?;
        if bytes.len() != 32 {
            return Err(IdentityError::InvalidKeyLength {
                expected: 32,
                actual: bytes.len(),
            });
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }

    /// Verify a signature over data using this public key.
    pub fn verify(&self, data: &[u8], sig: &Signature) -> bool {
        let Ok(verifying_key) = VerifyingKey::from_bytes(&self.0) else {
            return false;
        };
        let signature = ed25519_dalek::Signature::from_bytes(&sig.0);
        verifying_key.verify(data, &signature).is_ok()
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

/// Ed25519 keypair for signing messages.
pub struct Keypair {
    signing_key: SigningKey,
}

impl Keypair {
    /// Generate a new random keypair.
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self { signing_key }
    }

    /// Create a keypair from a secret key.
    pub fn from_secret(secret: [u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(&secret);
        Self { signing_key }
    }

    /// Get the public key.
    pub fn public_key(&self) -> PubKey {
        PubKey(self.signing_key.verifying_key().to_bytes())
    }

    /// Sign data and return the signature.
    pub fn sign(&self, data: &[u8]) -> Signature {
        let sig = self.signing_key.sign(data);
        Signature(sig.to_bytes())
    }

    /// Save the keypair to a directory.
    /// Writes `identity.key` (secret, mode 0600) and `identity.pub` (public).
    pub fn save(&self, dir: &Path) -> Result<(), IdentityError> {
        fs::create_dir_all(dir)?;

        let key_path = dir.join("identity.key");
        fs::write(&key_path, self.signing_key.to_bytes())?;

        // Set restrictive permissions on private key (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            fs::set_permissions(&key_path, perms)?;
        }

        fs::write(
            dir.join("identity.pub"),
            self.signing_key.verifying_key().to_bytes(),
        )?;
        Ok(())
    }

    /// Load a keypair from a directory.
    pub fn load(dir: &Path) -> Result<Self, IdentityError> {
        let secret_bytes = fs::read(dir.join("identity.key"))?;
        if secret_bytes.len() != 32 {
            return Err(IdentityError::InvalidKeyLength {
                expected: 32,
                actual: secret_bytes.len(),
            });
        }
        let mut secret = [0u8; 32];
        secret.copy_from_slice(&secret_bytes);
        Ok(Self::from_secret(secret))
    }

    /// Load existing keypair or generate a new one.
    pub fn load_or_generate(dir: &Path) -> Result<Self, IdentityError> {
        let key_path = dir.join("identity.key");
        if key_path.exists() {
            Self::load(dir)
        } else {
            let keypair = Self::generate();
            keypair.save(dir)?;
            Ok(keypair)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;
    use tempfile::TempDir;

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

    // Phase 1: PeerId Conversion tests

    #[test]
    fn test_pubkey_to_peer_id() {
        let pubkey = PubKey::new([42u8; 32]);
        let peer_id = pubkey.to_peer_id();
        assert!(peer_id.starts_with("ed25519:"));
        // Base64 of 32 bytes is 44 chars (with padding)
        assert_eq!(peer_id.len(), "ed25519:".len() + 44);
    }

    #[test]
    fn test_pubkey_from_peer_id() {
        let original = PubKey::new([42u8; 32]);
        let peer_id = original.to_peer_id();
        let parsed = PubKey::from_peer_id(&peer_id).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_peer_id_format() {
        let pubkey = PubKey::new([1u8; 32]);
        let peer_id = pubkey.to_peer_id();
        // Validate format with regex-like check
        assert!(peer_id.starts_with("ed25519:"));
        let base64_part = &peer_id["ed25519:".len()..];
        // Base64 alphabet check (alphanumeric + / + = for padding)
        assert!(base64_part
            .chars()
            .all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn test_peer_id_roundtrip() {
        let original = PubKey::new([99u8; 32]);
        let peer_id = original.to_peer_id();
        let recovered = PubKey::from_peer_id(&peer_id).unwrap();
        assert_eq!(original, recovered);
    }

    // Phase 1: Keypair tests

    #[test]
    fn test_keypair_struct() {
        let keypair = Keypair::generate();
        let _ = keypair.public_key();
        // Just verify it compiles and doesn't panic
    }

    #[test]
    fn test_keypair_generate() {
        let keypair = Keypair::generate();
        let pubkey = keypair.public_key();
        assert_eq!(pubkey.as_bytes().len(), 32);
    }

    #[test]
    fn test_keypair_public_key() {
        let keypair = Keypair::generate();
        let pk1 = keypair.public_key();
        let pk2 = keypair.public_key();
        assert_eq!(pk1, pk2); // Same keypair should produce same pubkey
    }

    #[test]
    fn test_keypair_sign() {
        let keypair = Keypair::generate();
        let data = b"test message";
        let sig = keypair.sign(data);
        assert_eq!(sig.as_bytes().len(), 64);
    }

    #[test]
    fn test_pubkey_verify() {
        let keypair = Keypair::generate();
        let data = b"test message";
        let sig = keypair.sign(data);
        let pubkey = keypair.public_key();
        assert!(pubkey.verify(data, &sig));
    }

    // Phase 1: Key Persistence tests

    #[test]
    fn test_keypair_save() {
        let tmp = TempDir::new().unwrap();
        let keypair = Keypair::generate();
        keypair.save(tmp.path()).unwrap();
        assert!(tmp.path().join("identity.key").exists());
        assert!(tmp.path().join("identity.pub").exists());
    }

    #[test]
    fn test_keypair_load() {
        let tmp = TempDir::new().unwrap();
        let original = Keypair::generate();
        original.save(tmp.path()).unwrap();
        let loaded = Keypair::load(tmp.path()).unwrap();
        assert_eq!(original.public_key(), loaded.public_key());
    }

    #[test]
    fn test_keypair_load_or_generate_existing() {
        let tmp = TempDir::new().unwrap();
        let original = Keypair::generate();
        original.save(tmp.path()).unwrap();
        let loaded = Keypair::load_or_generate(tmp.path()).unwrap();
        assert_eq!(original.public_key(), loaded.public_key());
    }

    #[test]
    fn test_keypair_load_or_generate_new() {
        let tmp = TempDir::new().unwrap();
        assert!(!tmp.path().join("identity.key").exists());
        let keypair = Keypair::load_or_generate(tmp.path()).unwrap();
        assert!(tmp.path().join("identity.key").exists());
        assert_eq!(keypair.public_key().as_bytes().len(), 32);
    }

    // Phase 1: Security tests

    #[test]
    fn test_sign_verify_roundtrip() {
        let keypair = Keypair::generate();
        let data = b"important message";
        let sig = keypair.sign(data);
        assert!(keypair.public_key().verify(data, &sig));
    }

    #[test]
    fn test_tamper_detection() {
        let keypair = Keypair::generate();
        let data = b"original message";
        let sig = keypair.sign(data);
        let tampered = b"tampered message";
        assert!(!keypair.public_key().verify(tampered, &sig));
    }

    #[test]
    fn test_wrong_key_rejection() {
        let keypair1 = Keypair::generate();
        let keypair2 = Keypair::generate();
        let data = b"test data";
        let sig = keypair1.sign(data);
        // Verify with wrong key should fail
        assert!(!keypair2.public_key().verify(data, &sig));
    }

    #[test]
    fn test_keypair_persistence_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let original = Keypair::generate();
        original.save(tmp.path()).unwrap();
        let loaded = Keypair::load(tmp.path()).unwrap();
        // Sign with loaded key, verify with original's pubkey
        let data = b"persistence test";
        let sig = loaded.sign(data);
        assert!(original.public_key().verify(data, &sig));
    }
}
