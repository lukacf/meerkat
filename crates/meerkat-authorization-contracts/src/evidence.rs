//! Pure immutable evidence identifiers, digests and syntax errors.
//! None of these values proves authentication, retention or currentness.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt;

use crate::resource::ResourceRef;

/// Exact owner-issued identifier. Syntax is validated; issuance is not.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct EvidenceId(String);

impl EvidenceId {
    /// # Errors
    /// Refuses blank, oversized or control-bearing identifiers.
    pub fn new(value: impl Into<String>) -> Result<Self, EvidenceError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(EvidenceError::InvalidIdentifier);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for EvidenceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EvidenceId([protected])")
    }
}

impl<'de> Deserialize<'de> for EvidenceId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Byte digest, never an explanation, authority token or freshness proof.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvidenceDigest([u8; 32]);

impl fmt::Debug for EvidenceDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EvidenceDigest([protected])")
    }
}

impl EvidenceDigest {
    #[must_use]
    pub const fn from_array(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    #[must_use]
    pub const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceError {
    #[error("invalid evidence identifier")]
    InvalidIdentifier,
}

/// Protected immutable reference, not proof of authentication or custody.
/// Its owner binds the referenced observation and its declared retention scope.
/// Process-lifetime observations do not acquire durability from this reference,
/// and retained authentication evidence never renews present authorization.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalEvidenceRef {
    pub resource: ResourceRef,
    pub revision: EvidenceId,
    pub digest: EvidenceDigest,
}
