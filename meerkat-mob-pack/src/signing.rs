use crate::digest::MobpackDigest;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSignature {
    pub signer_id: MobpackSignerId,
    pub public_key: MobpackPublicKey,
    pub digest: MobpackDigest,
    pub signature: MobpackSignatureBytes,
    pub timestamp: MobpackSignatureTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MobpackSignerId(String);

impl MobpackSignerId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err("mobpack signer id must not be empty".to_string());
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MobpackSignerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for MobpackSignerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MobpackSignerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobpackPublicKey([u8; 32]);

impl MobpackPublicKey {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn from_verifying_key(key: &VerifyingKey) -> Self {
        Self(key.to_bytes())
    }

    pub fn to_verifying_key(&self) -> Result<VerifyingKey, ed25519_dalek::SignatureError> {
        VerifyingKey::from_bytes(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Serialize for MobpackPublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for MobpackPublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes: [u8; 32] = hex::decode(value)
            .map_err(D::Error::custom)?
            .try_into()
            .map_err(|_| D::Error::custom("expected 32-byte public key"))?;
        Ok(Self(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobpackSignatureBytes([u8; 64]);

impl MobpackSignatureBytes {
    pub const fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    pub fn from_signature(signature: &Signature) -> Self {
        Self(signature.to_bytes())
    }

    pub fn to_signature(&self) -> Signature {
        Signature::from_bytes(&self.0)
    }
}

impl Serialize for MobpackSignatureBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for MobpackSignatureBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes: [u8; 64] = hex::decode(value)
            .map_err(D::Error::custom)?
            .try_into()
            .map_err(|_| D::Error::custom("expected 64-byte signature"))?;
        Ok(Self(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobpackSignatureTimestamp(DateTime<Utc>);

impl MobpackSignatureTimestamp {
    pub fn now() -> Self {
        Self(Utc::now())
    }

    pub fn parse(value: &str) -> Result<Self, chrono::ParseError> {
        Ok(Self(
            DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc),
        ))
    }
}

impl Serialize for MobpackSignatureTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    }
}

impl<'de> Deserialize<'de> for MobpackSignatureTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

pub fn sign_digest(signing_key: &SigningKey, digest: MobpackDigest) -> Signature {
    signing_key.sign(digest.as_bytes())
}

pub fn verify_digest(
    verifying_key: &VerifyingKey,
    digest: MobpackDigest,
    signature: &Signature,
) -> Result<(), ed25519_dalek::SignatureError> {
    verifying_key.verify(digest.as_bytes(), signature)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_signature_toml_roundtrip() {
        let sig = PackSignature {
            signer_id: MobpackSignerId::new("ci").unwrap(),
            public_key: MobpackPublicKey::from_bytes([0xab; 32]),
            digest: MobpackDigest::from_str(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            signature: MobpackSignatureBytes::from_bytes([0xcd; 64]),
            timestamp: MobpackSignatureTimestamp::parse("2026-02-24T00:00:00Z").unwrap(),
        };

        let encoded = toml::to_string(&sig).unwrap();
        let decoded: PackSignature = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, sig);
    }

    #[test]
    fn test_ed25519_sign_verify_roundtrip() {
        let signing_key_a = SigningKey::from_bytes(&[7u8; 32]);
        let signing_key_b = SigningKey::from_bytes(&[8u8; 32]);
        let digest = MobpackDigest::from_bytes([3u8; 32]);

        let sig = sign_digest(&signing_key_a, digest);
        assert!(verify_digest(&signing_key_a.verifying_key(), digest, &sig).is_ok());
        assert!(verify_digest(&signing_key_b.verifying_key(), digest, &sig).is_err());
    }
}
