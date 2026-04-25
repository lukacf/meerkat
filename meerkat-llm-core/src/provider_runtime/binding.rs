//! Normalized binding shapes, resolved-connection shim, and concrete
//! lease implementations used by Phase 2 provider runtimes.
//!
//! `NormalizedBackendKind` / `NormalizedAuthMethod` are typed sums over
//! per-provider enums declared in `providers/<p>/{backend,auth}.rs`.
//! `ResolvedConnection.shim_credential` is the **Phase 2-only** seam that
//! `build_client` reads to get the resolved secret. Phase 3 deletes this
//! field when `build_client` owns HTTP request assembly directly.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use meerkat_core::{
    AuthError, AuthLease, AuthMetadata, AuthProfile, AuthRefreshReason, BackendProfile,
    BindingPolicy, ConnectionRef, HttpAuthorizer, Provider, ResolvedAuthKind,
};

use meerkat_core::provider_matrix::anthropic::{AnthropicAuthMethod, AnthropicBackendKind};
use meerkat_core::provider_matrix::google::{GoogleAuthMethod, GoogleBackendKind};
use meerkat_core::provider_matrix::openai::{OpenAiAuthMethod, OpenAiBackendKind};

/// Provider-tagged normalized backend kind. Each variant is produced by the
/// matching provider runtime's `validate_binding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NormalizedBackendKind {
    OpenAi(OpenAiBackendKind),
    Anthropic(AnthropicBackendKind),
    Google(GoogleBackendKind),
}

/// Provider-tagged normalized auth method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NormalizedAuthMethod {
    OpenAi(OpenAiAuthMethod),
    Anthropic(AnthropicAuthMethod),
    Google(GoogleAuthMethod),
}

/// A binding that has been provider-validated but not yet resolved.
#[derive(Clone)]
pub struct ValidatedBinding {
    pub connection_ref: ConnectionRef,
    pub provider: Provider,
    pub backend: NormalizedBackendKind,
    pub auth: NormalizedAuthMethod,
    pub backend_profile: Arc<BackendProfile>,
    pub auth_profile: Arc<AuthProfile>,
    pub policy: BindingPolicy,
}

impl std::fmt::Debug for ValidatedBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidatedBinding")
            .field("connection_ref", &self.connection_ref)
            .field("provider", &self.provider)
            .field("backend", &self.backend)
            .field("auth", &self.auth)
            .field("backend_profile_id", &self.backend_profile.id)
            .field("auth_profile_id", &self.auth_profile.id)
            .finish()
    }
}

// Plan §6.11 deleted the legacy marker enum. Credential material
// lives on `auth_lease` directly via the typed `ResolvedAuthKind`
// variants: `InlineSecret(Arc<String>)` for simple-secret flows
// (dogma §5 closure — replaces the prior `__secret__` magic-header
// convention), `DynamicAuthorizer` for authorizer-backed flows,
// `StaticHeaders` for multi-header wire-level envelopes, and `None`
// for authless transports. `build_client` reads
// `ResolvedConnection::resolved_secret()` /
// `resolved_authorizer()`.

/// A fully resolved connection carries the trait-object lease alongside
/// backend metadata.
#[derive(Clone)]
pub struct ResolvedConnection {
    pub provider: Provider,
    pub backend: NormalizedBackendKind,
    pub backend_profile: Arc<BackendProfile>,
    pub auth_lease: Arc<dyn AuthLease>,
}

impl std::fmt::Debug for ResolvedConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedConnection")
            .field("provider", &self.provider)
            .field("backend", &self.backend)
            .field("backend_profile_id", &self.backend_profile.id)
            .finish()
    }
}

impl ResolvedConnection {
    /// Extract the resolved inline secret (api key, bearer token, OAuth
    /// access token) from the auth lease. Returns `None` for
    /// authorizer-backed leases. Plan §6.11 + dogma §5 closure:
    /// reads the typed `InlineSecret` variant of `ResolvedAuthKind`
    /// (replaces the prior `__secret__` synthetic-header convention).
    pub fn resolved_secret(&self) -> Option<String> {
        match self.auth_lease.kind() {
            meerkat_core::ResolvedAuthKind::InlineSecret(secret) => Some((**secret).clone()),
            _ => None,
        }
    }

    /// Extract the resolved dynamic authorizer (AWS SigV4, Google Auth,
    /// Azure AD, ExternalAuthorizer-backed) from the auth lease. Returns
    /// `None` for non-authorizer leases. Plan §6.11.
    pub fn resolved_authorizer(&self) -> Option<Arc<dyn HttpAuthorizer>> {
        match self.auth_lease.kind() {
            meerkat_core::ResolvedAuthKind::DynamicAuthorizer(auth) => Some(auth.clone()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------
// Lease implementations
// ---------------------------------------------------------------------

/// Static lease holding pre-projected headers + metadata. Used for api_key
/// and static_bearer resolutions.
pub struct StaticLease {
    kind: ResolvedAuthKind,
    metadata: AuthMetadata,
    expires_at: Option<DateTime<Utc>>,
    source_label: String,
}

impl StaticLease {
    /// Construct a lease carrying pre-projected wire headers. Used by
    /// resolvers that know the full header set (future post-§6.12
    /// paths). For raw secrets (api keys / bearer tokens), callers
    /// should prefer [`StaticLease::inline_secret`].
    pub fn new(
        headers: Vec<(String, String)>,
        metadata: AuthMetadata,
        expires_at: Option<DateTime<Utc>>,
        source_label: impl Into<String>,
    ) -> Self {
        Self {
            kind: ResolvedAuthKind::StaticHeaders(headers),
            metadata,
            expires_at,
            source_label: source_label.into(),
        }
    }

    /// Construct a lease carrying a raw inline secret (api key, bearer
    /// token, OAuth access token). Plan §6.11 + dogma §5: the typed
    /// `ResolvedAuthKind::InlineSecret` variant replaces the earlier
    /// `StaticHeaders(vec![("__secret__", value)])` magic-string
    /// convention.
    pub fn inline_secret(
        secret: String,
        metadata: AuthMetadata,
        expires_at: Option<DateTime<Utc>>,
        source_label: impl Into<String>,
    ) -> Self {
        Self {
            kind: ResolvedAuthKind::InlineSecret(Arc::new(secret)),
            metadata,
            expires_at,
            source_label: source_label.into(),
        }
    }

    /// Construct a lease with no credential material (authorizer-backed
    /// flows where the runtime constructs the authorizer in
    /// `build_client`, not the resolver). Matches
    /// `ResolvedAuthKind::None`.
    pub fn empty_lease(metadata: AuthMetadata, source_label: impl Into<String>) -> Self {
        Self {
            kind: ResolvedAuthKind::None,
            metadata,
            expires_at: None,
            source_label: source_label.into(),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AuthLease for StaticLease {
    fn kind(&self) -> &ResolvedAuthKind {
        &self.kind
    }
    fn metadata(&self) -> &AuthMetadata {
        &self.metadata
    }
    fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_at
    }
    fn source_label(&self) -> &str {
        &self.source_label
    }
    async fn refresh(&self, _reason: AuthRefreshReason) -> Result<(), AuthError> {
        // StaticLease has no refresh semantics in Phase 2.
        Ok(())
    }
}

/// Dynamic lease wrapping a runtime authorizer. Phase 2 build_client does
/// not accept this shape — authorizer-backed flows use this directly
/// `build_client` returns DynamicAuthorizerNotYetSupportedInShimMode.
pub struct DynamicLease {
    authorizer: Arc<dyn HttpAuthorizer>,
    metadata: AuthMetadata,
    expires_at: Option<DateTime<Utc>>,
    source_label: String,
    kind: ResolvedAuthKind,
}

impl DynamicLease {
    pub fn new(
        authorizer: Arc<dyn HttpAuthorizer>,
        metadata: AuthMetadata,
        expires_at: Option<DateTime<Utc>>,
        source_label: impl Into<String>,
    ) -> Self {
        let kind = ResolvedAuthKind::DynamicAuthorizer(authorizer.clone());
        Self {
            authorizer,
            metadata,
            expires_at,
            source_label: source_label.into(),
            kind,
        }
    }

    pub fn authorizer(&self) -> &Arc<dyn HttpAuthorizer> {
        &self.authorizer
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AuthLease for DynamicLease {
    fn kind(&self) -> &ResolvedAuthKind {
        &self.kind
    }
    fn metadata(&self) -> &AuthMetadata {
        &self.metadata
    }
    fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_at
    }
    fn source_label(&self) -> &str {
        &self.source_label
    }
    async fn refresh(&self, reason: AuthRefreshReason) -> Result<(), AuthError> {
        Err(AuthError::RefreshFailed(format!(
            "dynamic lease '{}' cannot refresh in place for reason {reason:?}; re-resolve the typed connection_ref",
            self.source_label
        )))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // Plan §6.11 deleted the §6.11 deletion artifact cleanup.

    #[tokio::test]
    async fn static_lease_satisfies_trait() {
        let lease: Arc<dyn AuthLease> = Arc::new(StaticLease::new(
            Vec::new(),
            AuthMetadata::default(),
            None,
            "test",
        ));
        assert!(matches!(lease.kind(), ResolvedAuthKind::StaticHeaders(_)));
        assert_eq!(lease.source_label(), "test");
        assert!(lease.refresh(AuthRefreshReason::Manual).await.is_ok());
    }

    #[tokio::test]
    async fn dynamic_lease_refresh_reports_unsupported_instead_of_success() {
        #[derive(Debug)]
        struct TestAuthorizer;

        #[async_trait::async_trait]
        impl HttpAuthorizer for TestAuthorizer {
            async fn authorize(
                &self,
                _req: &mut meerkat_core::auth::HttpAuthorizationRequest<'_>,
            ) -> Result<(), AuthError> {
                Ok(())
            }

            fn label(&self) -> &'static str {
                "test-authorizer"
            }
        }

        let lease: Arc<dyn AuthLease> = Arc::new(DynamicLease::new(
            Arc::new(TestAuthorizer),
            AuthMetadata::default(),
            None,
            "dynamic:test",
        ));
        let err = lease
            .refresh(AuthRefreshReason::Manual)
            .await
            .expect_err("dynamic refresh must not report success without work");
        assert!(matches!(err, AuthError::RefreshFailed(_)));
    }
}
