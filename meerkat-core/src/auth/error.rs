//! Auth error types — generic, provider-neutral.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Top-level auth error surface.
///
/// Provider runtimes map their internal failures into these variants before
/// returning from `resolve_binding`. Callers inspect [`AuthError::kind`] for
/// stable wire classification.
#[derive(Debug, Clone, Error, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthError {
    /// Credential material was requested but none was available
    /// (env lookup returned `None`, file missing, etc.).
    #[error("missing secret for auth resolution")]
    MissingSecret,
    /// Backend/auth combination is not allowed by the provider runtime.
    #[error("unsupported combination: backend={backend} auth={auth}")]
    UnsupportedCombination { backend: String, auth: String },
    /// A required metadata field (workspace id, account id, etc.) was absent.
    #[error("missing required metadata: {0}")]
    MissingRequiredMetadata(String),
    /// Workspace/account identity did not match the binding constraints.
    #[error("workspace mismatch")]
    WorkspaceMismatch,
    /// Credential lease has expired.
    #[error("credential expired")]
    Expired,
    /// Refresh attempt failed (network, revoked token, etc.).
    #[error("refresh failed: {0}")]
    RefreshFailed(String),
    /// Interactive login is needed (OAuth, managed store not populated,
    /// platform default requiring browser flow, etc.).
    #[error("interactive login required")]
    InteractiveLoginRequired,
    /// Host-owned auth resolver is not available on this surface.
    #[error("host-owned auth unavailable on this surface")]
    HostOwnedUnavailable,
    /// Low-level I/O or parsing failure during resolution.
    #[error("auth I/O failure: {0}")]
    Io(String),
    /// Catch-all for provider-specific diagnostic messages that don't map
    /// to another variant.
    #[error("auth error: {0}")]
    Other(String),
}

impl AuthError {
    /// Return the stable discriminant kind for wire classification.
    pub fn kind(&self) -> AuthErrorKind {
        match self {
            Self::MissingSecret => AuthErrorKind::MissingSecret,
            Self::UnsupportedCombination { .. } => AuthErrorKind::UnsupportedCombination,
            Self::MissingRequiredMetadata(_) => AuthErrorKind::MissingRequiredMetadata,
            Self::WorkspaceMismatch => AuthErrorKind::WorkspaceMismatch,
            Self::Expired => AuthErrorKind::Expired,
            Self::RefreshFailed(_) => AuthErrorKind::RefreshFailed,
            Self::InteractiveLoginRequired => AuthErrorKind::InteractiveLoginRequired,
            Self::HostOwnedUnavailable => AuthErrorKind::HostOwnedUnavailable,
            Self::Io(_) => AuthErrorKind::Io,
            Self::Other(_) => AuthErrorKind::Other,
        }
    }
}

/// Stable discriminant for [`AuthError`]. Used in serialized error summaries
/// and for SDK-facing wire codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AuthErrorKind {
    MissingSecret,
    UnsupportedCombination,
    MissingRequiredMetadata,
    WorkspaceMismatch,
    Expired,
    RefreshFailed,
    InteractiveLoginRequired,
    HostOwnedUnavailable,
    Io,
    Other,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn kind_maps_each_variant() {
        assert_eq!(
            AuthError::MissingSecret.kind(),
            AuthErrorKind::MissingSecret
        );
        assert_eq!(
            AuthError::WorkspaceMismatch.kind(),
            AuthErrorKind::WorkspaceMismatch
        );
        assert_eq!(AuthError::Expired.kind(), AuthErrorKind::Expired);
        assert_eq!(
            AuthError::RefreshFailed("x".into()).kind(),
            AuthErrorKind::RefreshFailed,
        );
        assert_eq!(
            AuthError::InteractiveLoginRequired.kind(),
            AuthErrorKind::InteractiveLoginRequired,
        );
    }

    #[test]
    fn display_is_stable_nonempty() {
        for err in [
            AuthError::MissingSecret,
            AuthError::UnsupportedCombination {
                backend: "b".into(),
                auth: "a".into(),
            },
            AuthError::MissingRequiredMetadata("workspace_id".into()),
            AuthError::WorkspaceMismatch,
            AuthError::Expired,
            AuthError::RefreshFailed("timeout".into()),
            AuthError::InteractiveLoginRequired,
            AuthError::HostOwnedUnavailable,
            AuthError::Io("file missing".into()),
            AuthError::Other("x".into()),
        ] {
            assert!(!err.to_string().is_empty(), "{err:?}");
        }
    }

    #[test]
    fn error_kind_serde_roundtrip() {
        let k = AuthErrorKind::MissingSecret;
        let s = serde_json::to_string(&k).unwrap();
        assert_eq!(s, "\"missing_secret\"");
        let back: AuthErrorKind = serde_json::from_str(&s).unwrap();
        assert_eq!(back, k);
    }
}
