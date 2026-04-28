//! Auth lease, credential-material kinds, authorizer trait, and refresh semantics.
//!
//! `meerkat-core` owns the trait shape; concrete lease implementations
//! (`StaticLease`, `DynamicLease`) live in `meerkat-client/src/runtime/binding.rs`.
//! `meerkat-core` declares none of the Phase 2 shim surface — the
//! `ResolvedConnection.shim_credential` seam is entirely on the client side.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::error::AuthError;
use super::metadata::AuthMetadata;

/// Why a refresh or re-resolution was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AuthRefreshReason {
    StartupValidation,
    Preflight,
    Unauthorized,
    ExpiringSoon,
    Manual,
    ConnectionChanged,
}

/// The resolved credential material. `meerkat-client` resolvers produce
/// this and wrap it in an [`AuthLease`]; `ProviderRuntime::build_client`
/// reads it to construct the LLM wire-layer.
#[derive(Clone)]
pub enum ResolvedAuthKind {
    /// A pre-resolved secret (api key, static bearer, OAuth access
    /// token). Wrapped in `Arc<String>` to keep the clone cheap and
    /// to avoid multiplying the credential across `Arc<dyn AuthLease>`
    /// holders. Plan §6.11: replaces the legacy `StaticHeaders` with
    /// `__secret__` synthetic-header convention (dogma §5 "typed
    /// truth, never folklore").
    InlineSecret(Arc<String>),
    /// A vector of `(name, value)` header pairs — used by resolvers
    /// that pre-project wire-correct headers directly (some provider
    /// runtimes may do this post-§6.12 when `build_client` owns HTTP
    /// assembly). Empty/placeholder headers are no longer produced by
    /// any resolver in the repo.
    StaticHeaders(Vec<(String, String)>),
    /// A runtime authorizer invoked per request (AWS SigV4, Google
    /// Auth, Azure AD, host-supplied ExternalAuthorizer-Dynamic).
    DynamicAuthorizer(Arc<dyn HttpAuthorizer>),
    /// No credential material (e.g., self-hosted without auth).
    None,
}

impl std::fmt::Debug for ResolvedAuthKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InlineSecret(_) => f.debug_tuple("InlineSecret").field(&"<redacted>").finish(),
            Self::StaticHeaders(headers) => f
                .debug_tuple("StaticHeaders")
                .field(&headers.len())
                .finish(),
            Self::DynamicAuthorizer(auth) => f
                .debug_tuple("DynamicAuthorizer")
                .field(&auth.label())
                .finish(),
            Self::None => f.debug_struct("None").finish(),
        }
    }
}

/// Surface-safe projection of the resolved credential state. Returned by
/// external resolver handles (WASM, desktop bridges) where shipping a full
/// trait object is not practical. Serde-roundtrippable so WASM/RPC bridges
/// can cross the process boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedAuthEnvelope {
    /// A pre-resolved raw secret (api key, bearer token, OAuth access
    /// token). Plan §6.11 + dogma §5 closure: replaces the legacy
    /// `StaticHeaders` with a synthetic `"__secret__"` header-key
    /// convention. External resolvers that produce simple secrets
    /// should use this variant.
    InlineSecret {
        secret: String,
        metadata: AuthMetadata,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
        expires_at: Option<DateTime<Utc>>,
    },
    StaticHeaders {
        headers: Vec<(String, String)>,
        metadata: AuthMetadata,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
        expires_at: Option<DateTime<Utc>>,
    },
    DynamicAuthorizer {
        metadata: AuthMetadata,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
        expires_at: Option<DateTime<Utc>>,
    },
    None {
        metadata: AuthMetadata,
    },
}

/// Minimal request view passed to a dynamic authorizer.
pub struct HttpAuthorizationRequest<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a mut Vec<(String, String)>,
}

/// Dynamic authorizer trait. Used when auth artifacts need to be computed
/// per request (ADC, refreshed OAuth bearer, etc.).
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait HttpAuthorizer: Send + Sync {
    async fn authorize(&self, req: &mut HttpAuthorizationRequest<'_>) -> Result<(), AuthError>;
    fn label(&self) -> &str;

    /// Non-secret freshness projection for authorizers that cache expiring
    /// token material internally. Implementations that never observe an
    /// expiring credential should keep the default `None`.
    fn expires_at(&self) -> Option<DateTime<Utc>> {
        None
    }
}

/// Trait contract for a resolved credential lease. `meerkat-core` declares
/// only generic lifecycle methods. No Phase 2 shim surface lives here.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AuthLease: Send + Sync {
    fn kind(&self) -> &ResolvedAuthKind;
    fn metadata(&self) -> &AuthMetadata;
    fn expires_at(&self) -> Option<DateTime<Utc>>;
    fn source_label(&self) -> &str;
    async fn refresh(&self, reason: AuthRefreshReason) -> Result<(), AuthError>;
}

/// Non-secret constraints on a resolved binding (per-realm or per-profile).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthConstraints {
    #[serde(default)]
    pub require_workspace_id: bool,
    #[serde(default)]
    pub require_account_id: bool,
    #[serde(default)]
    pub allow_interactive_login: bool,
    #[serde(default = "default_true")]
    pub allow_refresh: bool,
}

impl Default for AuthConstraints {
    fn default() -> Self {
        Self {
            require_workspace_id: false,
            require_account_id: false,
            allow_interactive_login: false,
            allow_refresh: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn refresh_reason_roundtrip() {
        let r = AuthRefreshReason::ExpiringSoon;
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, "\"expiring_soon\"");
    }

    #[test]
    fn resolved_auth_kind_debug_smoke() {
        let k = ResolvedAuthKind::StaticHeaders(vec![("k".into(), "v".into())]);
        assert!(format!("{k:?}").contains("StaticHeaders"));
        let n = ResolvedAuthKind::None;
        assert!(format!("{n:?}").contains("None"));
    }

    #[test]
    fn auth_constraints_defaults() {
        let c = AuthConstraints::default();
        assert!(!c.require_workspace_id);
        assert!(!c.require_account_id);
        assert!(!c.allow_interactive_login);
        assert!(c.allow_refresh, "allow_refresh defaults to true");
    }

    #[test]
    fn resolved_auth_envelope_serde_roundtrip() {
        let meta = AuthMetadata {
            account_id: Some("acct_x".into()),
            ..AuthMetadata::default()
        };
        // Headers variant.
        let env = ResolvedAuthEnvelope::StaticHeaders {
            headers: vec![("Authorization".into(), "Bearer xyz".into())],
            metadata: meta,
            expires_at: None,
        };
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"kind\":\"static_headers\""));
        let back: ResolvedAuthEnvelope = serde_json::from_str(&s).unwrap();
        match back {
            ResolvedAuthEnvelope::StaticHeaders {
                headers, metadata, ..
            } => {
                assert_eq!(headers.len(), 1);
                assert_eq!(metadata.account_id.as_deref(), Some("acct_x"));
            }
            other => panic!("unexpected variant: {other:?}"),
        }

        // Dynamic + None variants.
        let dyn_env = ResolvedAuthEnvelope::DynamicAuthorizer {
            metadata: AuthMetadata::default(),
            expires_at: None,
        };
        let s = serde_json::to_string(&dyn_env).unwrap();
        assert!(s.contains("dynamic_authorizer"));
        let _back: ResolvedAuthEnvelope = serde_json::from_str(&s).unwrap();

        let none_env = ResolvedAuthEnvelope::None {
            metadata: AuthMetadata::default(),
        };
        let s = serde_json::to_string(&none_env).unwrap();
        assert!(s.contains("\"kind\":\"none\""));
        let _back: ResolvedAuthEnvelope = serde_json::from_str(&s).unwrap();
    }

    // Minimal stub lease to exercise object-safety of the trait.
    struct StubLease {
        kind: ResolvedAuthKind,
        metadata: AuthMetadata,
        source_label: String,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AuthLease for StubLease {
        fn kind(&self) -> &ResolvedAuthKind {
            &self.kind
        }
        fn metadata(&self) -> &AuthMetadata {
            &self.metadata
        }
        fn expires_at(&self) -> Option<DateTime<Utc>> {
            None
        }
        fn source_label(&self) -> &str {
            &self.source_label
        }
        async fn refresh(&self, _reason: AuthRefreshReason) -> Result<(), AuthError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn stub_lease_is_object_safe() {
        let lease: Arc<dyn AuthLease> = Arc::new(StubLease {
            kind: ResolvedAuthKind::None,
            metadata: AuthMetadata::default(),
            source_label: "stub".into(),
        });
        assert_eq!(lease.source_label(), "stub");
        assert!(matches!(lease.kind(), ResolvedAuthKind::None));
        assert!(lease.refresh(AuthRefreshReason::Manual).await.is_ok());
    }
}
