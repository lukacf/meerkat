//! Bounded model-facing projection from a retained native work owner.
//!
//! This text is not a permission, identity registry or serializable capability.
//! Reading it and sending it to a reviewer are separately authorized operations.

use super::ReviewerFailure;
use crate::authorization::OperationAuthorizationError;

pub const MAX_REVIEW_CONTEXT_BYTES: usize = 64 * 1024;

#[cfg(not(target_arch = "wasm32"))]
pub type ReviewContextFuture<'a> =
    futures::future::BoxFuture<'a, Result<ReviewContextMaterial, OperationAuthorizationError>>;
#[cfg(target_arch = "wasm32")]
pub type ReviewContextFuture<'a> =
    futures::future::LocalBoxFuture<'a, Result<ReviewContextMaterial, OperationAuthorizationError>>;

/// Complete bounded text supplied by a trusted context source. Construction
/// proves only the size/nonempty invariant; it establishes no authority.
pub struct ReviewContextMaterial(String);

impl ReviewContextMaterial {
    pub fn from_text(text: String) -> Result<Self, ReviewerFailure> {
        if text.len() > MAX_REVIEW_CONTEXT_BYTES || text.trim().is_empty() {
            return Err(ReviewerFailure::Unavailable);
        }
        Ok(Self(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ReviewContextMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReviewContextMaterial([REDACTED])")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn context_is_bounded_in_bytes_and_never_truncated_or_debug_disclosed() {
        assert!(ReviewContextMaterial::from_text(" \n".into()).is_err());
        assert!(
            ReviewContextMaterial::from_text("x".repeat(MAX_REVIEW_CONTEXT_BYTES + 1)).is_err()
        );
        assert!(ReviewContextMaterial::from_text("é".repeat(MAX_REVIEW_CONTEXT_BYTES)).is_err());
        let source = "x".repeat(MAX_REVIEW_CONTEXT_BYTES);
        let material = ReviewContextMaterial::from_text(source.clone()).unwrap();
        assert_eq!(material.as_str(), source);
        assert_eq!(format!("{material:?}"), "ReviewContextMaterial([REDACTED])");
    }
}
