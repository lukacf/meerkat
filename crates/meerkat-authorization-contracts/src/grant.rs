//! Immutable issued-grant coordinates, not proof of issuance or current use.

use meerkat_core::auth::PrincipalRef;
use serde::{Deserialize, Serialize};

use crate::evidence::EvidenceId;

/// The issued identity, distinct from a later current-authority observation.
/// Zero issued revision is untrusted candidate data; only an actual retained
/// issued row establishes the reference. An authority generation is immutable
/// across recovery of that same authority incarnation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantLineageRef {
    pub root_authority: PrincipalRef,
    pub authority_namespace: EvidenceId,
    pub authority_generation: u64,
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
}

#[cfg(test)]
mod tests;
