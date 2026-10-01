//! Immutable issued-grant coordinates, not proof of issuance or current use.

use meerkat_core::auth::PrincipalRef;
use serde::{Deserialize, Serialize};

use crate::evidence::EvidenceId;

/// Identity of one process-local grant owner. A decoded value is only a claim;
/// the live host mints its own UUID v4 once and the generated owner retains it.
/// UUID v4 provides 122 random bits. It is not an authentication credential or
/// a restoration token, and it does not replace current grant resolution.
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct GrantAuthorityIncarnation(uuid::Uuid);

impl GrantAuthorityIncarnation {
    /// Check the shape of untrusted incarnation data, not ownership of it.
    ///
    /// # Errors
    /// Refuses any UUID that is not RFC 4122 variant, version 4.
    pub fn from_uuid(value: uuid::Uuid) -> Result<Self, GrantReferenceError> {
        if value.get_variant() != uuid::Variant::RFC4122
            || value.get_version() != Some(uuid::Version::Random)
        {
            return Err(GrantReferenceError::Incarnation);
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for GrantAuthorityIncarnation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_uuid(uuid::Uuid::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Debug for GrantAuthorityIncarnation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GrantAuthorityIncarnation([protected])")
    }
}

/// The issued identity, distinct from a later current-authority observation.
/// Zero issued revision is untrusted candidate data; only an actual retained
/// issued row establishes the reference. The configured generation alone does
/// not identify a process-local owner; each new owner mints a fresh incarnation.
/// This foundation does not restore a previous owner or its incarnation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantLineageRef {
    pub root_authority: PrincipalRef,
    pub authority_namespace: EvidenceId,
    pub authority_generation: u64,
    pub authority_incarnation: GrantAuthorityIncarnation,
    pub grant_id: EvidenceId,
    pub issued_revision: u64,
}

impl GrantLineageRef {
    /// Check data shape only. This does not resolve an actual issued grant.
    ///
    /// # Errors
    /// Refuses an unqualified issuer or absent authority generation.
    pub fn validate(&self) -> Result<(), GrantReferenceError> {
        self.root_authority
            .validate_qualified()
            .map_err(|_| GrantReferenceError::Unqualified)?;
        if self.authority_generation == 0 {
            return Err(GrantReferenceError::Shape);
        }
        Ok(())
    }
}

impl std::fmt::Debug for GrantLineageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GrantLineageRef([protected])")
    }
}

/// Data-shape errors only; actual issued rows are resolved by their owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GrantReferenceError {
    #[error("grant reference requires a qualified authority")]
    Unqualified,
    #[error("grant reference requires a nonzero authority generation")]
    Shape,
    #[error("grant incarnation requires an RFC 4122 version 4 UUID")]
    Incarnation,
}

#[cfg(test)]
mod tests;
