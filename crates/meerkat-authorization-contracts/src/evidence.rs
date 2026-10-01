//! Pure immutable evidence identifiers and syntax errors.
//! None of these values proves authentication, retention or currentness.

use serde::{Deserialize, Serialize};
use std::fmt;

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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceError {
    #[error("invalid evidence identifier")]
    InvalidIdentifier,
}
